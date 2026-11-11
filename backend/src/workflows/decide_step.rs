//! Executor for `StepType::Decide` (KT-1367).
//!
//! Asks the configured decision model typed questions about a templated
//! state, in one call to Ollama's `POST /v1/systemone`, and turns each answer
//! into a signal the step's `on_result` rules branch on. Zero agent tokens.
//!
//! Signals, besides `OK`:
//! - `noul`: `<id>_yes` or `<id>_no`;
//! - `choice`: `<id>_<option>`;
//! - `score`: `<id>_<level>`, levels numbered from 1;
//! - an answer below its confidence threshold: `<id>_unsure` instead, plus
//!   `low_confidence` once for the step.
//!
//! The rules are matched against every signal the step emits, in rule
//! order, so a step with many questions is not limited to the last lines of
//! its output.
//!
//! Confidence is the model's own for `choice` and `score`; `noul` returns a
//! probability only, so its confidence is `|2p − 1|`. The defaults differ per
//! kind because the scales do (docs/research/decision-step-bench-2026-10-21.md).

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::models::{
    ConditionAction, DecideConfig, DecideKind, DecideQuestion, RunStatus, StepConditionRule,
    StepResult, WorkflowStep,
};

use super::steps::StepOutcome;
use super::template::TemplateContext;

/// Measured defaults (30 real Kronn cases, clef-flash:9b, 2026-10-21).
pub const DEFAULT_MIN_CONFIDENCE_NOUL: f32 = 0.85;
pub const DEFAULT_MIN_CONFIDENCE_CHOICE: f32 = 0.35;
pub const DEFAULT_MIN_CONFIDENCE_SCORE: f32 = 0.30;

pub const MAX_QUESTIONS: usize = 64;
/// A cold model loads in 8 to 25 s; a warm answer takes under a second.
const CALL_TIMEOUT: Duration = Duration::from_secs(180);

pub fn default_min_confidence(kind: DecideKind) -> f32 {
    match kind {
        DecideKind::Noul => DEFAULT_MIN_CONFIDENCE_NOUL,
        DecideKind::Choice => DEFAULT_MIN_CONFIDENCE_CHOICE,
        DecideKind::Score => DEFAULT_MIN_CONFIDENCE_SCORE,
    }
}

fn signal_safe(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 48
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Why a Decide config cannot run, if it cannot. Checked at save and again
/// before the call.
pub fn config_problem(config: &DecideConfig) -> Option<String> {
    if config.state.trim().is_empty() {
        return Some("`decide.state` is empty: say what the model reads".into());
    }
    if config.questions.is_empty() || config.questions.len() > MAX_QUESTIONS {
        return Some(format!(
            "`decide.questions` takes 1 to {MAX_QUESTIONS} questions"
        ));
    }
    if let Some(min) = config.min_confidence {
        if !min.is_finite() || !(0.0..=1.0).contains(&min) {
            return Some("`decide.min_confidence` must be between 0 and 1".into());
        }
    }
    let mut ids = BTreeSet::new();
    for question in &config.questions {
        if !signal_safe(&question.id) {
            return Some(format!(
                "question id `{}`: lowercase letters, digits and `_` only",
                question.id
            ));
        }
        if !ids.insert(question.id.as_str()) {
            return Some(format!("question id `{}` is used twice", question.id));
        }
        if question.instructions.trim().is_empty() {
            return Some(format!("question `{}` has no instructions", question.id));
        }
        match question.kind {
            DecideKind::Noul => {
                if question
                    .options
                    .keys()
                    .any(|key| key != "true" && key != "false")
                {
                    return Some(format!(
                        "question `{}`: a yes/no question only describes `true` and `false`",
                        question.id
                    ));
                }
            }
            DecideKind::Choice => {
                if !(2..=26).contains(&question.options.len()) {
                    return Some(format!(
                        "question `{}`: a choice takes 2 to 26 options",
                        question.id
                    ));
                }
                if let Some(bad) = question.options.keys().find(|key| !signal_safe(key)) {
                    return Some(format!(
                        "question `{}`: option `{bad}` must be lowercase letters, digits and `_`",
                        question.id
                    ));
                }
                if question.options.values().any(|text| text.trim().is_empty()) {
                    return Some(format!(
                        "question `{}`: every option says what it means",
                        question.id
                    ));
                }
            }
            DecideKind::Score => {
                if !(2..=10).contains(&question.levels.len())
                    || question.levels.iter().any(|level| level.trim().is_empty())
                {
                    return Some(format!(
                        "question `{}`: a score takes 2 to 10 named levels",
                        question.id
                    ));
                }
            }
        }
    }
    None
}

/// Why a workflow with Decide steps cannot run here: no decision model is
/// set. Refused at launch and shown as a blocker, never a silent skip.
pub fn launch_refusal(
    workflow: &crate::models::Workflow,
    model_tiers: &crate::models::ModelTiersConfig,
) -> Option<String> {
    let uses_decide = workflow
        .steps
        .iter()
        .chain(workflow.on_failure.iter())
        .any(|step| step.step_type == crate::models::StepType::Decide);
    let configured = model_tiers
        .ollama
        .decision
        .as_deref()
        .is_some_and(|model| !model.trim().is_empty());
    (uses_decide && !configured).then(|| {
        "This workflow has a Decide step but no decision model is configured: choose one in \
         Config › Agents › Ollama (row « Décision »)."
            .to_string()
    })
}

/// The `/v1/systemone` body (contract measured on Ollama 0.40.6).
pub fn request_body(model: &str, state: &str, questions: &[DecideQuestion]) -> Value {
    let mut asked = Map::new();
    for question in questions {
        let mut entry = json!({
            "type": match question.kind {
                DecideKind::Noul => "noul",
                DecideKind::Choice => "choice",
                DecideKind::Score => "score",
            },
            "instructions": question.instructions,
        });
        match question.kind {
            DecideKind::Noul if !question.options.is_empty() => {
                entry["criteria"] = json!(question.options);
            }
            DecideKind::Choice => entry["criteria"] = json!(question.options),
            DecideKind::Score => entry["criteria"] = json!(question.levels),
            DecideKind::Noul => {}
        }
        asked.insert(question.id.clone(), entry);
    }
    json!({ "model": model, "state": state, "questions": asked })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The answers, as the step's `data`.
    pub answers: Value,
    pub signals: Vec<String>,
    pub summary: String,
    pub low_confidence: bool,
}

/// Reads the model's answers into signals. A missing or malformed answer is
/// an error, never a silent default.
pub fn interpret(
    questions: &[DecideQuestion],
    response: &Value,
    min_confidence: Option<f32>,
) -> Result<Decision, String> {
    let answers = response
        .get("answers")
        .and_then(Value::as_object)
        .ok_or("the decision model returned no answers")?;
    let mut data = Map::new();
    let mut signals = vec!["OK".to_string()];
    let mut summary = Vec::new();
    let mut low = false;
    for question in questions {
        let answer = answers
            .get(&question.id)
            .ok_or_else(|| format!("no answer for question `{}`", question.id))?;
        let (value, confidence, detail) = match question.kind {
            DecideKind::Noul => {
                let p = answer["noul"]
                    .as_f64()
                    .ok_or_else(|| format!("question `{}`: no probability", question.id))?;
                let value = if p >= 0.5 { "yes" } else { "no" };
                (
                    value.to_string(),
                    (2.0 * p - 1.0).abs(),
                    json!({ "probability": p }),
                )
            }
            DecideKind::Choice => {
                let choice = answer["choice"]
                    .as_str()
                    .filter(|choice| question.options.contains_key(*choice))
                    .ok_or_else(|| {
                        format!(
                            "question `{}`: the choice is not one of its options",
                            question.id
                        )
                    })?;
                let confidence = answer["confidence"].as_f64().unwrap_or(0.0);
                (
                    choice.to_string(),
                    confidence,
                    json!({ "probabilities": answer["probabilities"] }),
                )
            }
            DecideKind::Score => {
                let probabilities = answer["probabilities"]
                    .as_object()
                    .ok_or_else(|| format!("question `{}`: no level probabilities", question.id))?;
                let (index, _) = probabilities
                    .iter()
                    .filter_map(|(level, p)| Some((level.parse::<usize>().ok()?, p.as_f64()?)))
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .ok_or_else(|| format!("question `{}`: no level probabilities", question.id))?;
                if index >= question.levels.len() {
                    return Err(format!("question `{}`: level out of range", question.id));
                }
                let confidence = answer["confidence"].as_f64().unwrap_or(0.0);
                (
                    (index + 1).to_string(),
                    confidence,
                    json!({
                        "level": question.levels[index],
                        "expected": answer["score"],
                        "probabilities": answer["probabilities"],
                    }),
                )
            }
        };
        let threshold = f64::from(min_confidence.unwrap_or(default_min_confidence(question.kind)));
        let trusted = confidence >= threshold;
        if trusted {
            signals.push(format!("{}_{value}", question.id));
        } else {
            signals.push(format!("{}_unsure", question.id));
            low = true;
        }
        summary.push(format!(
            "{}={}{} ({confidence:.2})",
            question.id,
            value,
            if trusted { "" } else { "?" }
        ));
        let mut entry = json!({
            "kind": question.kind,
            "answer": value,
            "confidence": confidence,
            "trusted": trusted,
        });
        if let (Some(entry), Some(detail)) = (entry.as_object_mut(), detail.as_object()) {
            entry.extend(detail.clone());
        }
        data.insert(question.id.clone(), entry);
    }
    if low {
        signals.push("low_confidence".to_string());
    }
    Ok(Decision {
        answers: Value::Object(data),
        signals,
        summary: summary.join(", "),
        low_confidence: low,
    })
}

/// The first rule, in rule order, whose signal the step emitted.
pub fn action_for(rules: &[StepConditionRule], signals: &[String]) -> Option<ConditionAction> {
    rules
        .iter()
        .filter(|rule| !rule.contains.is_empty())
        .find(|rule| signals.iter().any(|signal| signal == rule.contains.trim()))
        .map(|rule| rule.action.clone())
}

pub async fn execute_decide_step(
    step: &WorkflowStep,
    ctx: &TemplateContext,
    base_url: &str,
    model: Option<&str>,
) -> StepOutcome {
    let start = Instant::now();
    let Some(config) = step.decide.as_ref() else {
        return fail(step, start, None, "Decide step has no `decide` config.");
    };
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return fail(
            step,
            start,
            None,
            "No decision model is configured: choose one in Config › Agents › Ollama (row « Décision »).",
        );
    };
    if let Some(problem) = config_problem(config) {
        return fail(step, start, Some(model), format!("Decide step: {problem}."));
    }
    let state = match ctx.render_strict(&config.state) {
        Ok(state) => state,
        Err(e) => return fail(step, start, Some(model), format!("Decide state: {e}")),
    };
    let body = request_body(model, &state, &config.questions);
    let response = match call(base_url, &body).await {
        Ok(response) => response,
        Err(e) => {
            return fail(
                step,
                start,
                Some(model),
                format!("Decision model {model} at {base_url}: {e}"),
            )
        }
    };
    let decision = match interpret(&config.questions, &response, config.min_confidence) {
        Ok(decision) => decision,
        Err(e) => {
            return fail(
                step,
                start,
                Some(model),
                format!("Decision model {model}: {e}"),
            )
        }
    };
    let data = json!({
        "model": model,
        "answers": decision.answers,
        "low_confidence": decision.low_confidence,
    });
    let signals: Vec<&str> = decision.signals.iter().map(String::as_str).collect();
    let output = super::step_output_format::format_step_output(
        data,
        "OK",
        &format!("Decide: {}", decision.summary),
        None,
        &signals,
    );
    let condition_action = action_for(&step.on_result, &decision.signals);
    let condition_result = condition_action.as_ref().map(|action| match action {
        ConditionAction::Stop => "Stop".to_string(),
        ConditionAction::Skip => "Skip".to_string(),
        ConditionAction::Goto { step_name, .. } => format!("Goto:{step_name}"),
    });
    StepOutcome {
        result: result(
            step,
            start,
            Some(model),
            RunStatus::Success,
            output,
            condition_result,
        ),
        condition_action,
    }
}

async fn call(base_url: &str, body: &Value) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(CALL_TIMEOUT)
        .connect_timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .post(format!("{}/v1/systemone", base_url.trim_end_matches('/')))
        .json(body)
        .send()
        .await
        .map_err(|e| format!("not reachable ({e})"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        let reason = value["error"].as_str().unwrap_or(text.as_str());
        return Err(format!(
            "answered HTTP {status}: {}",
            reason.chars().take(300).collect::<String>()
        ));
    }
    if value.is_null() {
        return Err("answered something that is not JSON".into());
    }
    Ok(value)
}

fn result(
    step: &WorkflowStep,
    start: Instant,
    model: Option<&str>,
    status: RunStatus,
    output: String,
    condition_result: Option<String>,
) -> StepResult {
    StepResult {
        step_name: step.name.clone(),
        status,
        output,
        tokens_used: Some(0),
        duration_ms: start.elapsed().as_millis() as u64,
        started_at: None,
        condition_result,
        envelope_detected: None,
        step_kind: None,
        step_agent: None,
        step_model: model.map(str::to_string),
        step_api_plugin_slug: None,
        step_api_endpoint_path: None,
        is_rollback: false,
        child_run_id: None,
        agent_provenance: None,
        native_tool_calls: Box::default(),
        cached_prompt_tokens: None,
        cache_write_prompt_tokens: None,
        last_activity: None,
        quota_wait: None,
        terminal_stop: None,
        output_bytes: None,
    }
}

fn fail(
    step: &WorkflowStep,
    start: Instant,
    model: Option<&str>,
    message: impl Into<String>,
) -> StepOutcome {
    StepOutcome {
        result: result(step, start, model, RunStatus::Failed, message.into(), None),
        condition_action: None,
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::models::StepType;

    fn question(id: &str, kind: DecideKind) -> DecideQuestion {
        let mut question = DecideQuestion {
            id: id.into(),
            kind,
            instructions: "Is it ready?".into(),
            ..Default::default()
        };
        match kind {
            DecideKind::Choice => {
                for (option, meaning) in [
                    ("local", "trivial"),
                    ("economy", "routine"),
                    ("advanced", "hard"),
                ] {
                    question.options.insert(option.into(), meaning.into());
                }
            }
            DecideKind::Score => {
                question.levels = ["no risk", "low", "medium", "high", "critical"]
                    .map(String::from)
                    .to_vec();
            }
            DecideKind::Noul => {}
        }
        question
    }

    /// Where an action leads, for comparison (`ConditionAction` has no `==`).
    fn target(action: Option<ConditionAction>) -> Option<String> {
        action.map(|action| match action {
            ConditionAction::Goto { step_name, .. } => step_name,
            other => format!("{other:?}"),
        })
    }

    fn config(questions: Vec<DecideQuestion>) -> DecideConfig {
        DecideConfig {
            state: "{{steps.tests.output}}".into(),
            questions,
            min_confidence: None,
        }
    }

    /// Answers captured from clef-flash:9b on Ollama 0.40.6 (2026-10-21).
    fn captured() -> Value {
        json!({"model": "clef-flash:9b", "answers": {
            "ship": {"type": "noul", "noul": 0.9615338079816756},
            "route": {"type": "choice", "choice": "economy",
                "probabilities": {"local": 0.392, "economy": 0.520, "advanced": 0.088},
                "confidence": 0.16221558633015942},
            "risk": {"type": "score", "score": 0.66,
                "legend": {"0": "no risk", "1": "low", "2": "medium", "3": "high", "4": "critical"},
                "probabilities": {"0": 0.439, "1": 0.506, "2": 0.023, "3": 0.017, "4": 0.016},
                "confidence": 0.42482176606689104}
        }})
    }

    #[test]
    fn a_config_is_refused_with_the_reason_before_any_call() {
        assert_eq!(
            config_problem(&config(vec![question("ship", DecideKind::Noul)])),
            None
        );
        let cases: Vec<(DecideConfig, &str)> = vec![
            (config(vec![]), "1 to 64"),
            (
                config(vec![question("Ship!", DecideKind::Noul)]),
                "lowercase",
            ),
            (
                config(vec![
                    question("a", DecideKind::Noul),
                    question("a", DecideKind::Noul),
                ]),
                "used twice",
            ),
            (
                config(vec![DecideQuestion {
                    levels: vec!["only".into()],
                    ..question("risk", DecideKind::Score)
                }]),
                "2 to 10",
            ),
            (
                config(vec![{
                    let mut q = question("route", DecideKind::Choice);
                    q.options.insert("Bad Option".into(), "x".into());
                    q
                }]),
                "option `Bad Option`",
            ),
            (
                config(vec![{
                    let mut q = question("ship", DecideKind::Noul);
                    q.options.insert("maybe".into(), "x".into());
                    q
                }]),
                "`true` and `false`",
            ),
            (
                DecideConfig {
                    min_confidence: Some(1.5),
                    ..config(vec![question("ship", DecideKind::Noul)])
                },
                "between 0 and 1",
            ),
            (
                DecideConfig {
                    state: "  ".into(),
                    ..config(vec![question("ship", DecideKind::Noul)])
                },
                "empty",
            ),
        ];
        for (config, expected) in cases {
            let problem = config_problem(&config).expect("refused");
            assert!(problem.contains(expected), "{problem}");
        }
    }

    #[test]
    fn the_request_follows_the_measured_systemone_contract() {
        let body = request_body(
            "clef-flash:9b",
            "tests passed",
            &[
                question("ship", DecideKind::Noul),
                question("route", DecideKind::Choice),
                question("risk", DecideKind::Score),
            ],
        );
        assert_eq!(body["model"], "clef-flash:9b");
        assert_eq!(body["state"], "tests passed");
        assert_eq!(
            body["questions"]["ship"],
            json!({"type": "noul", "instructions": "Is it ready?"})
        );
        assert_eq!(body["questions"]["route"]["criteria"]["economy"], "routine");
        assert_eq!(body["questions"]["risk"]["criteria"][4], "critical");
    }

    #[test]
    fn answers_become_signals_and_an_unsure_one_escalates() {
        let questions = [
            question("ship", DecideKind::Noul),
            question("route", DecideKind::Choice),
            question("risk", DecideKind::Score),
        ];
        let decision = interpret(&questions, &captured(), None).unwrap();
        // ship: p 0.96 → confidence 0.92 ≥ 0.85; route: 0.16 < 0.35; risk: 0.42 ≥ 0.30.
        assert_eq!(
            decision.signals,
            ["OK", "ship_yes", "route_unsure", "risk_2", "low_confidence"]
        );
        assert!(decision.low_confidence);
        assert_eq!(decision.answers["risk"]["level"], "low");
        assert_eq!(decision.answers["route"]["trusted"], false);

        // A step that trusts more lowers its threshold.
        let trusting = interpret(&questions, &captured(), Some(0.1)).unwrap();
        assert_eq!(
            trusting.signals,
            ["OK", "ship_yes", "route_economy", "risk_2"]
        );
        assert!(!trusting.low_confidence);
    }

    #[test]
    fn a_missing_or_foreign_answer_is_an_error_never_a_default() {
        let questions = [question("route", DecideKind::Choice)];
        let missing = interpret(&questions, &json!({"answers": {}}), None).unwrap_err();
        assert!(missing.contains("no answer"), "{missing}");
        let foreign = interpret(
            &questions,
            &json!({"answers": {"route": {"type": "choice", "choice": "elsewhere", "confidence": 0.9}}}),
            None,
        )
        .unwrap_err();
        assert!(foreign.contains("not one of its options"), "{foreign}");
    }

    #[test]
    fn rules_match_any_emitted_signal_in_rule_order() {
        let rule = |contains: &str, step: &str| StepConditionRule {
            contains: contains.into(),
            action: ConditionAction::Goto {
                step_name: step.into(),
                max_iterations: None,
            },
        };
        let signals: Vec<String> = [
            "OK",
            "a_yes",
            "b_no",
            "c_1",
            "d_2",
            "e_3",
            "f_unsure",
            "low_confidence",
        ]
        .map(String::from)
        .to_vec();
        // Far from the end of the output, still matched.
        assert_eq!(
            target(action_for(
                &[rule("nothing", "x"), rule("a_yes", "merge")],
                &signals
            )),
            Some("merge".into())
        );
        assert_eq!(
            target(action_for(
                &[rule("low_confidence", "human"), rule("a_yes", "merge")],
                &signals
            )),
            Some("human".into())
        );
        assert_eq!(target(action_for(&[rule("a_no", "x")], &signals)), None);
    }

    /// The whole step against a mock Ollama: rendered state, one call, the
    /// answers in the envelope, and the run branching on them.
    #[tokio::test]
    async fn a_decide_step_asks_ollama_and_branches_on_the_answer() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .and(body_partial_json(
                json!({"model": "clef-flash:9b", "state": "1104 passed; 0 failed"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(captured()))
            .expect(1)
            .mount(&server)
            .await;
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("tests", "1104 passed; 0 failed");
        let step = WorkflowStep {
            name: "decide".into(),
            step_type: StepType::Decide,
            decide: Some(config(vec![question("ship", DecideKind::Noul)])),
            on_result: vec![StepConditionRule {
                contains: "ship_yes".into(),
                action: ConditionAction::Goto {
                    step_name: "merge".into(),
                    max_iterations: None,
                },
            }],
            ..Default::default()
        };
        let outcome = execute_decide_step(&step, &ctx, &server.uri(), Some("clef-flash:9b")).await;
        assert_eq!(
            outcome.result.status,
            RunStatus::Success,
            "{}",
            outcome.result.output
        );
        assert_eq!(outcome.result.tokens_used, Some(0));
        assert_eq!(outcome.result.step_model.as_deref(), Some("clef-flash:9b"));
        assert!(
            outcome.result.output.contains("[SIGNAL: ship_yes]"),
            "{}",
            outcome.result.output
        );
        assert!(outcome.result.output.contains("\"probability\""));
        assert_eq!(target(outcome.condition_action), Some("merge".into()));
        assert_eq!(
            outcome.result.condition_result.as_deref(),
            Some("Goto:merge")
        );
    }

    #[tokio::test]
    async fn without_a_model_or_a_reachable_ollama_the_step_fails_with_a_sentence() {
        let ctx = {
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("tests", "ok");
            ctx
        };
        let step = WorkflowStep {
            name: "decide".into(),
            step_type: StepType::Decide,
            decide: Some(config(vec![question("ship", DecideKind::Noul)])),
            ..Default::default()
        };
        let no_model = execute_decide_step(&step, &ctx, "http://127.0.0.1:9", None).await;
        assert_eq!(no_model.result.status, RunStatus::Failed);
        assert!(
            no_model.result.output.contains("No decision model"),
            "{}",
            no_model.result.output
        );
        let down =
            execute_decide_step(&step, &ctx, "http://127.0.0.1:9", Some("clef-flash:9b")).await;
        assert_eq!(down.result.status, RunStatus::Failed);
        assert!(
            down.result.output.contains("not reachable"),
            "{}",
            down.result.output
        );
    }

    #[test]
    fn the_saved_decision_model_is_read_from_config_toml() {
        assert!(crate::core::config::saved_decision_model_in(
            "[agents.model_tiers.ollama]\ndecision = \"clef-flash:9b\"\n"
        ));
        assert!(!crate::core::config::saved_decision_model_in(
            "[agents.model_tiers.ollama]\ndecision = \"  \"\n"
        ));
        assert!(!crate::core::config::saved_decision_model_in("not toml ["));
    }

    /// Saving refuses a Decide step without its config, or with a bad one,
    /// and says which.
    #[test]
    fn saving_refuses_a_misconfigured_decide_step() {
        let mut step = WorkflowStep {
            name: "ready".into(),
            step_type: StepType::Decide,
            ..Default::default()
        };
        let missing = crate::api::workflows::validate_step_required_fields(&step).unwrap_err();
        assert!(missing.contains("`decide` est obligatoire"), "{missing}");
        step.decide = Some(config(vec![]));
        let empty = crate::api::workflows::validate_step_required_fields(&step).unwrap_err();
        assert!(empty.contains("1 to 64"), "{empty}");
        step.decide = Some(config(vec![question("ship", DecideKind::Noul)]));
        assert_eq!(
            crate::api::workflows::validate_step_required_fields(&step),
            Ok(())
        );
    }
}
