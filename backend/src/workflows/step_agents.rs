//! Agents chosen at launch for a workflow's Agent steps (KT-1025).
//!
//! The choice belongs to the run, never to the workflow: it is validated once
//! at admission, stored in the run's `trigger_context`, and applied to the
//! run's in-memory copy of the definition each time the runner (re)loads it,
//! so a Gate approval or an interrupted resume keeps it.

use std::collections::BTreeMap;

use crate::models::{
    AgentSettings, AgentType, AgentsConfig, StepAgentOverride, StepAgents, StepType, Workflow,
    WorkflowRun, WorkflowStep,
};

/// Where the validated choice lives inside `trigger_context`. Not a string,
/// so it never reaches the template context.
pub const TRIGGER_CONTEXT_KEY: &str = "__step_agents__";

const MAX_TEXT_CHARS: usize = 200;

/// A validated choice, keyed by the step's durable id (its name when it has
/// none), holding only steps whose agent, model or effort actually change.
pub type ResolvedStepAgents = BTreeMap<String, StepAgentOverride>;

pub fn step_key(step: &WorkflowStep) -> String {
    step.id.clone().unwrap_or_else(|| step.name.clone())
}

fn find_step<'a>(workflow: &'a Workflow, key: &str) -> Option<&'a WorkflowStep> {
    workflow
        .steps
        .iter()
        .find(|step| step.id.as_deref() == Some(key))
        .or_else(|| workflow.steps.iter().find(|step| step.name == key))
}

fn normalized_text(field: &str, key: &str, value: Option<&str>) -> Result<Option<String>, String> {
    let Some(text) = value.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    if text.chars().count() > MAX_TEXT_CHARS || text.chars().any(char::is_control) {
        return Err(format!(
            "`step_agents.{key}.{field}` must be one line of at most {MAX_TEXT_CHARS} characters"
        ));
    }
    Ok(Some(text.to_string()))
}

fn planned(step: &WorkflowStep) -> StepAgentOverride {
    let settings = step.agent_settings.as_ref();
    StepAgentOverride {
        agent: step.agent.clone(),
        model: settings.and_then(|s| s.model.clone()),
        reasoning_effort: settings.and_then(|s| s.reasoning_effort.clone()),
    }
}

/// Checks a launcher's `step_agents` against the workflow: every key names an
/// Agent step, at most once, and the step's declared tools suit the agent.
pub fn validate(workflow: &Workflow, requested: &StepAgents) -> Result<ResolvedStepAgents, String> {
    let mut resolved = ResolvedStepAgents::new();
    let mut keys: Vec<&String> = requested.keys().collect();
    keys.sort();
    for key in keys {
        let choice = &requested[key];
        let Some(step) = find_step(workflow, key) else {
            let agent_steps: Vec<&str> = workflow
                .steps
                .iter()
                .filter(|step| matches!(step.step_type, StepType::Agent))
                .map(|step| step.name.as_str())
                .collect();
            return Err(format!(
                "`step_agents`: this workflow has no step `{key}` (its Agent steps: {})",
                if agent_steps.is_empty() {
                    "none".to_string()
                } else {
                    agent_steps.join(", ")
                }
            ));
        };
        if !matches!(step.step_type, StepType::Agent) {
            return Err(format!(
                "`step_agents.{key}`: step `{}` is a {:?} step; only Agent steps take an agent",
                step.name, step.step_type
            ));
        }
        let canonical = step_key(step);
        let normalized = StepAgentOverride {
            agent: choice.agent.clone(),
            // KT-1998 — the shared model rule; stored trimmed, blank is none.
            model: choice
                .model
                .as_deref()
                .filter(|model| !model.trim().is_empty())
                .map(crate::models::setup::normalize_exact_model)
                .transpose()
                .map_err(|error| format!("`step_agents.{key}.model`: {error}"))?,
            reasoning_effort: normalized_text(
                "reasoning_effort",
                key,
                choice.reasoning_effort.as_deref(),
            )?,
        };
        let agent_changes = normalized.agent != step.agent;
        if agent_changes && normalized.agent == AgentType::Custom {
            return Err(format!(
                "`step_agents.{key}`: a named HTTP connection is chosen in the workflow, not at launch; pick another agent for step `{}`",
                step.name
            ));
        }
        if agent_changes {
            if let Some(tools) = step.agent_settings.as_ref().and_then(|s| s.tools.as_ref()) {
                tools.validate(&normalized.agent).map_err(|error| {
                    format!(
                        "`step_agents.{key}`: step `{}` declares tools {:?} cannot use ({error}); keep {:?} or change the step's tools in the workflow",
                        step.name, normalized.agent, step.agent
                    )
                })?;
            }
        }
        if normalized == planned(step) {
            continue;
        }
        // The step's budget stays, so it must still be one this agent can apply.
        let max_tokens = step.agent_settings.as_ref().and_then(|s| s.max_tokens);
        crate::agents::generation_settings::validate(
            &normalized.agent,
            normalized.reasoning_effort.as_deref(),
            max_tokens,
        )
        .map_err(|error| format!("`step_agents.{key}` (step `{}`): {error}", step.name))?;
        if resolved.insert(canonical, normalized).is_some() {
            return Err(format!(
                "`step_agents`: step `{}` is named twice (by id and by name)",
                step.name
            ));
        }
    }
    Ok(resolved)
}

/// Whether `agent` can start here. Shared with the runner's own preflight so
/// launch and run refuse exactly the same agents.
///
/// Always false for `Custom`: it is a named HTTP connection, not an agent, so
/// nothing that knows no connection (a launch override, the fallback chain)
/// may pick it. A step kept on its own `Custom` agent is checked against its
/// connection by `steps::named_connection_refusal` (KT-2030).
pub fn agent_can_launch(
    agent: &AgentType,
    usable: &[AgentType],
    agents_config: &AgentsConfig,
) -> bool {
    if matches!(agent, AgentType::Custom) {
        return false;
    }
    // A LiteLLM proxy is a server the user declares: no local binary needed.
    let declared_proxy = matches!(agent, AgentType::LiteLlm)
        && agents_config
            .lite_llm
            .base_url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty());
    declared_proxy
        || usable
            .iter()
            .any(|known| std::mem::discriminant(known) == std::mem::discriminant(agent))
}

/// Whether `step`, as it will run, can start here (KT-2055). A step on a named
/// connection (`Custom`, or a native HTTP provider — `LiteLlm`, `Nvidia` —
/// naming one) runs over HTTP on that connection: no local binary and no
/// legacy `lite_llm.base_url` is needed, its connection and key are judged by
/// `steps::step_connection_refusal`. Any other step: `agent_can_launch`.
pub fn step_can_launch(
    step: &WorkflowStep,
    usable: &[AgentType],
    agents_config: &AgentsConfig,
) -> bool {
    let runs_on_its_connection = step.agent == AgentType::Custom
        || (crate::db::external_api_connections::legacy_credential_for_agent(&step.agent)
            .is_some()
            && super::steps::step_connection_id(step).is_some());
    runs_on_its_connection || agent_can_launch(&step.agent, usable, agents_config)
}

/// Refuses a chosen agent that is not installed and enabled.
pub fn check_available(
    workflow: &Workflow,
    resolved: &ResolvedStepAgents,
    usable: &[AgentType],
    agents_config: &AgentsConfig,
) -> Result<(), String> {
    // A step kept on its own agent is checked by the run preflight as today.
    // A changed one is judged as it will run, by the preflight's own rule.
    let missing: Vec<String> = resolved
        .iter()
        .filter_map(|(key, choice)| {
            let step = find_step(workflow, key)?;
            let mut planned = step.clone();
            apply_to_step(&mut planned, choice);
            (step.agent != choice.agent && !step_can_launch(&planned, usable, agents_config))
                .then(|| format!("'{}' → {:?}", step.name, choice.agent))
        })
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "`step_agents`: agent(s) not installed or not enabled: {}. Install or enable them in Config, or pick another agent.",
        missing.join(", ")
    ))
}

/// Replaces the step's agent, model and effort. Tier, budget and tools stay;
/// a named connection stays only while the agent does.
pub fn apply_to_step(step: &mut WorkflowStep, choice: &StepAgentOverride) {
    let agent_changes = step.agent != choice.agent;
    let settings = step
        .agent_settings
        .get_or_insert_with(AgentSettings::default);
    if agent_changes {
        settings.connection_id = None;
    }
    settings.model = choice.model.clone();
    settings.reasoning_effort = choice.reasoning_effort.clone();
    step.agent = choice.agent.clone();
}

/// Applies a validated choice to a run's copy of the workflow.
pub fn apply(workflow: &mut Workflow, resolved: &ResolvedStepAgents) {
    for (key, choice) in resolved {
        let position = workflow
            .steps
            .iter()
            .position(|step| step.id.as_deref() == Some(key.as_str()))
            .or_else(|| workflow.steps.iter().position(|step| step.name == *key));
        match position {
            Some(index) if matches!(workflow.steps[index].step_type, StepType::Agent) => {
                apply_to_step(&mut workflow.steps[index], choice);
            }
            _ => tracing::warn!(
                step = %key,
                "launch agent choice names a step this workflow no longer has as an Agent step; it runs on its own agent"
            ),
        }
    }
}

/// The choice stored on a run, if any.
pub fn from_run(run: &WorkflowRun) -> Option<ResolvedStepAgents> {
    let value = run.trigger_context.as_ref()?.get(TRIGGER_CONTEXT_KEY)?;
    match serde_json::from_value::<ResolvedStepAgents>(value.clone()) {
        Ok(resolved) if !resolved.is_empty() => Some(resolved),
        Ok(_) => None,
        Err(error) => {
            tracing::warn!(run_id = %run.id, %error, "unreadable launch agent choice ignored");
            None
        }
    }
}

/// The run's launch context with the agent choice of step `key` set, or
/// removed when `choice` is `None` (the step is back on its own agent). The
/// run's other choices and launch values are kept.
pub fn context_with_choice(
    run: &WorkflowRun,
    key: &str,
    choice: Option<&StepAgentOverride>,
) -> serde_json::Result<serde_json::Value> {
    let mut stored = from_run(run).unwrap_or_default();
    match choice {
        Some(choice) => {
            stored.insert(key.to_string(), choice.clone());
        }
        None => {
            stored.remove(key);
        }
    }
    let mut context = match &run.trigger_context {
        Some(serde_json::Value::Object(map)) => map.clone(),
        _ => serde_json::Map::new(),
    };
    if stored.is_empty() {
        context.remove(TRIGGER_CONTEXT_KEY);
    } else {
        context.insert(TRIGGER_CONTEXT_KEY.into(), serde_json::to_value(&stored)?);
    }
    Ok(serde_json::Value::Object(context))
}

/// Applies the run's stored choice to its copy of the workflow. Idempotent.
pub fn apply_from_run(workflow: &mut Workflow, run: &WorkflowRun) -> bool {
    match from_run(run) {
        Some(resolved) => {
            apply(workflow, &resolved);
            true
        }
        None => false,
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::models::{StepTools, WorkflowSafety, WorkflowTrigger};

    fn agent_step(name: &str, agent: AgentType) -> WorkflowStep {
        serde_json::from_value(serde_json::json!({
            "id": format!("id-{name}"),
            "name": name,
            "step_type": { "type": "Agent" },
            "agent": agent,
            "prompt_template": "do it",
        }))
        .unwrap()
    }

    fn workflow(steps: Vec<WorkflowStep>) -> Workflow {
        Workflow {
            refs: Default::default(),
            retention: None,
            project_scope: None,
            pinned: false,
            id: "wf".into(),
            name: "wf".into(),
            project_id: None,
            trigger: WorkflowTrigger::Manual,
            steps,
            actions: vec![],
            safety: WorkflowSafety {
                sandbox: false,
                max_files: None,
                max_lines: None,
                require_approval: false,
            },
            workspace_config: None,
            concurrency_limit: None,
            concurrency_key: None,
            guards: None,
            artifacts: Default::default(),
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![],
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn choice(agent: AgentType, model: Option<&str>, effort: Option<&str>) -> StepAgentOverride {
        StepAgentOverride {
            agent,
            model: model.map(Into::into),
            reasoning_effort: effort.map(Into::into),
        }
    }

    fn request(entries: &[(&str, StepAgentOverride)]) -> StepAgents {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn a_choice_is_keyed_by_the_step_id_whether_named_by_id_or_name() {
        let wf = workflow(vec![agent_step("écrire", AgentType::ClaudeCode)]);
        for key in ["id-écrire", "écrire"] {
            let resolved = validate(
                &wf,
                &request(&[(key, choice(AgentType::Codex, Some("gpt-5"), None))]),
            )
            .unwrap();
            assert_eq!(resolved.keys().collect::<Vec<_>>(), vec!["id-écrire"]);
        }
    }

    #[test]
    fn an_unknown_override_field_is_refused_by_the_wire_format() {
        let error = serde_json::from_value::<StepAgentOverride>(serde_json::json!({
            "agent": "Codex", "connection_id": "conn-1"
        }))
        .unwrap_err();
        assert!(error.to_string().contains("connection_id"), "{error}");
    }

    #[test]
    fn an_unknown_step_is_refused_with_the_agent_steps_it_could_name() {
        let wf = workflow(vec![agent_step("review", AgentType::ClaudeCode)]);
        let error = validate(
            &wf,
            &request(&[("nope", choice(AgentType::Codex, None, None))]),
        )
        .unwrap_err();
        assert!(error.contains("no step `nope`"), "{error}");
        assert!(error.contains("review"), "{error}");
    }

    #[test]
    fn a_non_agent_step_is_refused() {
        let mut gate = agent_step("gate", AgentType::ClaudeCode);
        gate.step_type = StepType::Gate;
        let wf = workflow(vec![gate]);
        let error = validate(
            &wf,
            &request(&[("gate", choice(AgentType::Codex, None, None))]),
        )
        .unwrap_err();
        assert!(error.contains("only Agent steps"), "{error}");
    }

    #[test]
    fn the_same_step_named_twice_is_refused() {
        let wf = workflow(vec![agent_step("a", AgentType::ClaudeCode)]);
        let error = validate(
            &wf,
            &request(&[
                ("a", choice(AgentType::Codex, None, None)),
                ("id-a", choice(AgentType::Vibe, None, None)),
            ]),
        )
        .unwrap_err();
        assert!(error.contains("named twice"), "{error}");
    }

    #[test]
    fn an_unchanged_choice_is_dropped_and_blank_text_means_the_default() {
        let mut step = agent_step("a", AgentType::Codex);
        step.agent_settings = Some(AgentSettings {
            model: Some("gpt-5".into()),
            ..Default::default()
        });
        let wf = workflow(vec![step]);
        let resolved = validate(
            &wf,
            &request(&[("a", choice(AgentType::Codex, Some("  gpt-5 "), Some("  ")))]),
        )
        .unwrap();
        assert!(resolved.is_empty(), "{resolved:?}");
    }

    #[test]
    fn multi_line_or_oversized_text_is_refused() {
        let wf = workflow(vec![agent_step("a", AgentType::ClaudeCode)]);
        let long = "m".repeat(crate::models::setup::MAX_MODEL_ID_CHARS + 1);
        for (model, effort) in [
            (Some("a\nb"), None),
            (Some(long.as_str()), None),
            (None, Some("hi\u{7}")),
        ] {
            let error = validate(
                &wf,
                &request(&[("a", choice(AgentType::Codex, model, effort))]),
            )
            .unwrap_err();
            assert!(error.contains("one line"), "{error}");
        }
    }

    #[test]
    fn declared_claude_tools_refuse_another_agent_and_explain() {
        let mut step = agent_step("a", AgentType::ClaudeCode);
        step.agent_settings = Some(AgentSettings {
            tools: Some(StepTools {
                cli: vec!["Read".into()],
                kronn_internal: vec![],
            }),
            ..Default::default()
        });
        let wf = workflow(vec![step]);
        let error = validate(
            &wf,
            &request(&[("a", choice(AgentType::Codex, None, None))]),
        )
        .unwrap_err();
        assert!(error.contains("declares tools"), "{error}");
        assert!(error.contains("ClaudeCode"), "{error}");
        // The same tools on the same agent stay valid with another model.
        assert!(validate(
            &wf,
            &request(&[("a", choice(AgentType::ClaudeCode, Some("opus"), None))])
        )
        .is_ok());
    }

    #[test]
    fn a_kept_budget_refuses_a_cli_agent_that_cannot_apply_it() {
        let mut step = agent_step("a", AgentType::Ollama);
        step.agent_settings = Some(AgentSettings {
            max_tokens: Some(4096),
            ..Default::default()
        });
        let wf = workflow(vec![step]);
        let error = validate(
            &wf,
            &request(&[("a", choice(AgentType::Codex, None, None))]),
        )
        .unwrap_err();
        assert!(error.contains("max_tokens"), "{error}");
        assert!(error.contains("step `a`"), "{error}");
        assert!(validate(
            &wf,
            &request(&[("a", choice(AgentType::LiteLlm, None, None))])
        )
        .is_ok());
    }

    #[test]
    fn an_effort_the_agent_cannot_apply_is_refused() {
        let wf = workflow(vec![agent_step("a", AgentType::ClaudeCode)]);
        let error = validate(
            &wf,
            &request(&[("a", choice(AgentType::Ollama, None, Some("extreme")))]),
        )
        .unwrap_err();
        assert!(error.contains("Ollama reasoning effort"), "{error}");
    }

    #[test]
    fn switching_to_a_named_connection_agent_is_refused() {
        let wf = workflow(vec![agent_step("a", AgentType::ClaudeCode)]);
        let error = validate(
            &wf,
            &request(&[("a", choice(AgentType::Custom, None, None))]),
        )
        .unwrap_err();
        assert!(error.contains("named HTTP connection"), "{error}");
    }

    // KT-2030: a step kept on its own named connection is no override to
    // `Custom`: launch accepts it and leaves it to the run preflight, which
    // checks its connection; the connection stays on the step.
    #[test]
    fn a_step_kept_on_its_named_connection_goes_to_the_run_preflight() {
        let mut step = agent_step("jeu", AgentType::Custom);
        step.agent_settings = Some(AgentSettings {
            connection_id: Some("conn-or".into()),
            ..Default::default()
        });
        let mut wf = workflow(vec![step]);
        let resolved = validate(
            &wf,
            &request(&[("jeu", choice(AgentType::Custom, Some("z-ai/glm"), None))]),
        )
        .unwrap();
        let config = crate::core::config::default_config().agents;
        // Custom is never "usable": the launch check must not refuse it here.
        assert!(!agent_can_launch(&AgentType::Custom, &[], &config));
        check_available(&wf, &resolved, &[], &config).unwrap();
        apply(&mut wf, &resolved);
        let settings = wf.steps[0].agent_settings.as_ref().unwrap();
        assert_eq!(settings.connection_id.as_deref(), Some("conn-or"));
        assert_eq!(settings.model.as_deref(), Some("z-ai/glm"));
    }

    // KT-2055: a LiteLlm/Nvidia step naming a connection runs over HTTP on it:
    // launchable with no local binary and no legacy `lite_llm.base_url`.
    #[test]
    fn a_provider_step_naming_a_connection_launches_on_it_without_binary_or_base_url() {
        let mut config = crate::core::config::default_config().agents;
        config.lite_llm.base_url = None;
        for agent in [AgentType::LiteLlm, AgentType::Nvidia] {
            let mut step = agent_step("jeu", agent.clone());
            assert!(
                !step_can_launch(&step, &[], &config),
                "{agent:?} without a connection needs the agent itself"
            );
            step.agent_settings = Some(AgentSettings {
                connection_id: Some("   ".into()),
                ..Default::default()
            });
            assert!(
                !step_can_launch(&step, &[], &config),
                "a blank id is no connection"
            );
            step.agent_settings = Some(AgentSettings {
                connection_id: Some("conn-proxy-2".into()),
                ..Default::default()
            });
            assert!(step_can_launch(&step, &[], &config), "{agent:?}");
        }
        // A CLI naming a connection still needs its CLI.
        let mut cli = agent_step("cli", AgentType::Codex);
        cli.agent_settings = Some(AgentSettings {
            connection_id: Some("conn-proxy-2".into()),
            ..Default::default()
        });
        assert!(!step_can_launch(&cli, &[], &config));

        // Launch: a step kept on its connection with another model passes; a
        // step switched onto LiteLlm loses its connection and needs LiteLlm.
        let mut kept = agent_step("jeu", AgentType::LiteLlm);
        kept.agent_settings = Some(AgentSettings {
            connection_id: Some("conn-proxy-2".into()),
            ..Default::default()
        });
        let wf = workflow(vec![kept, agent_step("autre", AgentType::ClaudeCode)]);
        let resolved = validate(
            &wf,
            &request(&[("jeu", choice(AgentType::LiteLlm, Some("m2"), None))]),
        )
        .unwrap();
        check_available(&wf, &resolved, &[], &config).unwrap();
        let resolved = validate(
            &wf,
            &request(&[("autre", choice(AgentType::LiteLlm, None, None))]),
        )
        .unwrap();
        let error = check_available(&wf, &resolved, &[], &config).unwrap_err();
        assert!(error.contains("'autre' → LiteLlm"), "{error}");
    }

    #[test]
    fn an_unavailable_agent_is_refused_by_step_name() {
        let wf = workflow(vec![agent_step("lead", AgentType::ClaudeCode)]);
        let resolved = validate(
            &wf,
            &request(&[("lead", choice(AgentType::Codex, None, None))]),
        )
        .unwrap();
        let config = crate::core::config::default_config().agents;
        let error = check_available(&wf, &resolved, &[AgentType::ClaudeCode], &config).unwrap_err();
        assert!(error.contains("'lead' → Codex"), "{error}");
        assert!(check_available(&wf, &resolved, &[AgentType::Codex], &config).is_ok());
    }

    #[test]
    fn applying_keeps_budget_and_tier_and_a_connection_only_for_the_same_agent() {
        let mut http = agent_step("http", AgentType::Ollama);
        http.agent_settings = Some(AgentSettings {
            connection_id: Some("conn-1".into()),
            max_tokens: Some(4096),
            tier: Some(crate::models::ModelTier::Reasoning),
            ..Default::default()
        });
        let mut wf = workflow(vec![http.clone()]);

        // Same agent, another model: the connection identity stays.
        let resolved = validate(
            &wf,
            &request(&[(
                "http",
                choice(AgentType::Ollama, Some("qwen3"), Some("high")),
            )]),
        )
        .unwrap();
        apply(&mut wf, &resolved);
        let settings = wf.steps[0].agent_settings.clone().unwrap();
        assert_eq!(settings.connection_id.as_deref(), Some("conn-1"));
        assert_eq!(settings.model.as_deref(), Some("qwen3"));
        assert_eq!(settings.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(settings.max_tokens, Some(4096));
        assert_eq!(settings.tier, Some(crate::models::ModelTier::Reasoning));

        // Another agent: the connection belonged to the old one. Applying twice
        // (a resume) changes nothing more.
        let mut wf = workflow(vec![http]);
        let resolved = validate(
            &wf,
            &request(&[("http", choice(AgentType::LiteLlm, None, None))]),
        )
        .unwrap();
        apply(&mut wf, &resolved);
        apply(&mut wf, &resolved);
        let settings = wf.steps[0].agent_settings.clone().unwrap();
        assert_eq!(wf.steps[0].agent, AgentType::LiteLlm);
        assert_eq!(settings.connection_id, None);
        assert_eq!(settings.model, None);
        assert_eq!(settings.max_tokens, Some(4096));
        assert_eq!(settings.tier, Some(crate::models::ModelTier::Reasoning));
    }

    #[test]
    fn applying_keeps_the_declared_tools() {
        let tools = StepTools {
            cli: vec![],
            kronn_internal: vec!["task_get".into()],
        };
        let mut step = agent_step("a", AgentType::ClaudeCode);
        step.agent_settings = Some(AgentSettings {
            tools: Some(tools.clone()),
            ..Default::default()
        });
        let mut wf = workflow(vec![step]);
        let resolved = validate(
            &wf,
            &request(&[("a", choice(AgentType::Codex, Some("gpt-5"), None))]),
        )
        .unwrap();
        apply(&mut wf, &resolved);
        assert_eq!(wf.steps[0].agent, AgentType::Codex);
        assert_eq!(
            wf.steps[0].agent_settings.as_ref().unwrap().tools,
            Some(tools)
        );
    }

    #[test]
    fn a_run_without_a_stored_choice_leaves_the_workflow_untouched() {
        let mut wf = workflow(vec![agent_step("a", AgentType::ClaudeCode)]);
        let before = serde_json::to_value(&wf.steps).unwrap();
        let mut run: WorkflowRun = serde_json::from_value(serde_json::json!({
            "id": "r", "workflow_id": "wf", "status": "Pending",
            "trigger_context": { "type": "manual" }, "step_results": [], "tokens_used": 0,
            "workspace_path": null, "started_at": chrono::Utc::now(), "finished_at": null,
        }))
        .unwrap();
        assert!(!apply_from_run(&mut wf, &run));
        assert_eq!(serde_json::to_value(&wf.steps).unwrap(), before);

        run.trigger_context = Some(serde_json::json!({
            TRIGGER_CONTEXT_KEY: { "id-a": { "agent": "Codex" } }
        }));
        assert!(apply_from_run(&mut wf, &run));
        assert_eq!(wf.steps[0].agent, AgentType::Codex);
    }
}
