//! KT-1459 — a step re-runs beside its main execution, once per shadow, on the
//! main step's exact input. Only the main output continues the workflow; a
//! shadow never runs a step that acts outside Kronn and never fails or delays
//! the main step.

use super::*;
use crate::db::step_shadows::{self, StepShadowRow};
use crate::models::{AgentType, ShadowAgent, ShadowBudget, ShadowPrompt, StepShadow};
use crate::workflows::shadow::{self, ShadowStatus};

// KT-1544 — the optional merge of a step with its shadows.
mod merge_runs;

/// Answers a prompt as scripted and records the prompt it received.
struct Scripted {
    prompts: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    answer: String,
    delay: Option<std::time::Duration>,
    usage: Option<(u64, u64)>,
    cost_micros: Option<u64>,
    fails: bool,
    /// A prompt containing the first of a pair is answered with the second;
    /// any other gets `answer`.
    replies: Vec<(String, String)>,
}

type Prompts = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

fn scripted(answer: &str) -> Scripted {
    Scripted {
        prompts: Default::default(),
        answer: answer.into(),
        delay: None,
        usage: None,
        cost_micros: None,
        fails: false,
        replies: Vec::new(),
    }
}

#[async_trait::async_trait]
impl crate::acp::AcpTransport for Scripted {
    async fn initialize(
        &self,
        _: crate::acp::AcpInitialize,
    ) -> Result<crate::acp::AcpNegotiatedCapabilities, crate::acp::AcpError> {
        Ok(crate::acp::AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities: std::collections::BTreeSet::from([
                crate::acp::AcpCapability::Sessions,
                crate::acp::AcpCapability::Streaming,
                crate::acp::AcpCapability::Cancellation,
                crate::acp::AcpCapability::McpInjection,
            ]),
        })
    }
    async fn create_session(&self) -> Result<crate::acp::AcpSessionTarget, crate::acp::AcpError> {
        crate::acp::AcpSessionTarget::new(crate::acp::AcpAgent::ClaudeCode, "shadow-turn")
    }
    async fn config_options(&self) -> Vec<crate::acp::AcpConfigOption> {
        Vec::new()
    }
    async fn set_config_option(
        &self,
        _: &crate::acp::AcpSessionTarget,
        _: &str,
        _: &str,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn resume_session(
        &self,
        _: &crate::acp::AcpSessionTarget,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn prompt(
        &self,
        _: &crate::acp::AcpSessionTarget,
        prompt: &str,
        events: tokio::sync::mpsc::Sender<crate::acp::AcpSessionEvent>,
    ) -> Result<(), crate::acp::AcpError> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        if self.fails {
            return Err(crate::acp::AcpError::Transport("the model is gone".into()));
        }
        let answer = self
            .replies
            .iter()
            .find(|(asked, _)| prompt.contains(asked.as_str()))
            .map_or_else(|| self.answer.clone(), |(_, reply)| reply.clone());
        let _ = events
            .send(crate::acp::AcpSessionEvent::TextDelta(answer))
            .await;
        if let Some((input_tokens, output_tokens)) = self.usage {
            let _ = events
                .send(crate::acp::AcpSessionEvent::Usage {
                    input_tokens,
                    output_tokens,
                    prompt_cache: Default::default(),
                })
                .await;
        }
        if let Some(usd_micros) = self.cost_micros {
            let _ = events
                .send(crate::acp::AcpSessionEvent::Cost { usd_micros })
                .await;
        }
        let _ = events.send(crate::acp::AcpSessionEvent::Completed).await;
        Ok(())
    }
    async fn cancel(&self, _: &crate::acp::AcpSessionTarget) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
}

struct Fixture {
    state: crate::AppState,
    tokens: TokensConfig,
    agents: AgentsConfig,
    work_dir: std::path::PathBuf,
    project: String,
    /// What the main agent (Claude Code) was asked.
    main_prompts: Prompts,
    _repo: tempfile::TempDir,
    _main_route: crate::agents::runner::test_acp_routes::RouteGuard,
    routes: Vec<crate::agents::runner::test_acp_routes::AgentRouteGuard>,
}

async fn fixture(project: &str, main: Scripted) -> Fixture {
    let (state, tokens, agents) = test_state_and_configs();
    let repo = git_project(&state, project).await;
    let repo_path = repo.path().to_string_lossy().into_owned();
    let work_dir =
        crate::agents::runner::resolve_agent_work_dir(Some(&repo_path), &repo_path).unwrap();
    let main_prompts = main.prompts.clone();
    let main_route =
        crate::agents::runner::test_acp_routes::route(&work_dir, std::sync::Arc::new(main));
    Fixture {
        state,
        tokens,
        agents,
        work_dir,
        project: project.into(),
        main_prompts,
        _repo: repo,
        _main_route: main_route,
        routes: Vec::new(),
    }
}

impl Fixture {
    /// A shadow on `agent` talks to its own scripted model.
    fn model_for(&mut self, agent: AgentType, model: Scripted) -> Prompts {
        let prompts = model.prompts.clone();
        self.routes
            .push(crate::agents::runner::test_acp_routes::route_agent(
                &self.work_dir,
                &agent,
                std::sync::Arc::new(model),
            ));
        prompts
    }

    /// `who` echoes the PR, `review` is the step with shadows, `summary` reads
    /// the main review.
    fn workflow(&self, shadows: Vec<StepShadow>) -> Workflow {
        let mut wf = make_workflow_with_artifacts(Default::default());
        wf.id = "wf-step-shadows".into();
        wf.project_id = Some(self.project.clone());
        wf.exec_allowlist = vec!["echo".into()];
        let mut review = fake_step("review");
        review.prompt_template = "Review PR-42 :: {{steps.who.summary}}".into();
        review.shadows = shadows;
        let mut summary = fake_step("summary");
        summary.prompt_template = "Summary of: {{steps.review.output}}".into();
        wf.steps = vec![exec_step("who", "echo", &["PR-42"]), review, summary];
        wf
    }

    async fn run(&self, wf: &Workflow, id: &str) -> WorkflowRun {
        let mut run = pending_run(id, &wf.id);
        run.project_id = wf.project_id.clone();
        let known = {
            let id = wf.id.clone();
            self.state
                .db
                .with_read_conn(move |conn| crate::db::workflows::get_workflow(conn, &id))
                .await
                .unwrap()
                .is_some()
        };
        if known {
            let run_db = run.clone();
            self.state
                .db
                .with_conn(move |conn| crate::db::workflows::insert_run(conn, &run_db))
                .await
                .unwrap();
        } else {
            insert_wf_and_run(&self.state, wf, &run).await;
        }
        execute_run(
            self.state.clone(),
            wf,
            &mut run,
            &self.tokens,
            &self.agents,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        run
    }

    async fn rows(&self, run_id: &str) -> Vec<StepShadowRow> {
        let id = run_id.to_string();
        self.state
            .db
            .with_read_conn(move |conn| step_shadows::list_for_run(conn, &id))
            .await
            .unwrap()
    }

    /// The shadow rows of `run_id`, once `count` of them exist and none runs.
    async fn settled(&self, run_id: &str, count: usize) -> Vec<StepShadowRow> {
        for _ in 0..400 {
            let shadows: Vec<StepShadowRow> = self
                .rows(run_id)
                .await
                .into_iter()
                .filter(|row| row.shadow_id != step_shadows::MAIN)
                .collect();
            if shadows.len() >= count
                && shadows
                    .iter()
                    .all(|row| row.status != ShadowStatus::Running)
            {
                return shadows;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("the shadows never settled: {:?}", self.rows(run_id).await);
    }
}

fn shadow_of(id: &str) -> StepShadow {
    StepShadow {
        id: id.into(),
        ..Default::default()
    }
}

fn on(agent: AgentType, model: &str) -> Option<ShadowAgent> {
    Some(ShadowAgent {
        agent,
        model: Some(model.into()),
        connection_id: None,
        reasoning_effort: None,
    })
}

fn prompts_of(prompts: &Prompts) -> Vec<String> {
    prompts.lock().unwrap().clone()
}

#[tokio::test]
async fn an_identical_shadow_reruns_only_its_step_on_the_main_steps_exact_input() {
    let fx = fixture("proj-shadow-identical", scripted("the review")).await;
    let wf = fx.workflow(vec![shadow_of("again")]);
    let run = fx.run(&wf, "run-identical").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let rows = fx.settled("run-identical", 1).await;

    // review (main), review (shadow), summary (main): the other steps ran once.
    let prompts = prompts_of(&fx.main_prompts);
    assert_eq!(prompts.len(), 3, "{prompts:?}");
    let reviews: Vec<&String> = prompts
        .iter()
        .filter(|p| p.contains("Review PR-42"))
        .collect();
    assert_eq!(reviews.len(), 2, "{prompts:?}");
    assert_eq!(
        reviews[0], reviews[1],
        "the shadow receives the prompt the main step received"
    );
    assert_eq!(
        run.step_results.len(),
        3,
        "a shadow adds no step to the run"
    );

    let shadow = &rows[0];
    assert_eq!(shadow.shadow_id, "again");
    assert_eq!(shadow.status, ShadowStatus::Success, "{shadow:?}");
    assert_eq!(shadow.output, "the review");
    // Nothing differs between the two: the identities are the same.
    let all = fx.rows("run-identical").await;
    let main = all
        .iter()
        .find(|row| row.shadow_id == step_shadows::MAIN)
        .expect("the main execution is recorded");
    assert_eq!(main.step_name, "review");
    assert_eq!(main.identity, shadow.identity);
    assert_eq!(main.identity.agent, "ClaudeCode");
    assert_eq!(
        main.result_index, 1,
        "the review is the run's second result"
    );
}

#[tokio::test]
async fn only_the_main_output_continues_the_workflow() {
    let mut fx = fixture("proj-shadow-main-only", scripted("main answer")).await;
    let other = fx.model_for(AgentType::Codex, scripted("shadow answer"));
    let mut shadow = shadow_of("other-model");
    shadow.agent = on(AgentType::Codex, "gpt-x");
    let wf = fx.workflow(vec![shadow]);
    let run = fx.run(&wf, "run-main-only").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let rows = fx.settled("run-main-only", 1).await;

    // One level changed: the agent, and with it the model.
    assert_eq!(
        prompts_of(&other).len(),
        1,
        "the shadow ran on its own agent"
    );
    assert_eq!(rows[0].output, "shadow answer");
    assert_eq!(rows[0].identity.agent, "Codex");
    assert_eq!(rows[0].identity.model.as_deref(), Some("gpt-x"));
    let main = fx
        .rows("run-main-only")
        .await
        .into_iter()
        .find(|row| row.shadow_id == step_shadows::MAIN)
        .unwrap();
    assert_eq!(main.identity.agent, "ClaudeCode");

    // The next step reads the main output, never the shadow's.
    let summary = run
        .step_results
        .iter()
        .find(|result| result.step_name == "summary")
        .unwrap();
    assert_eq!(summary.status, RunStatus::Success);
    let summary_prompt = prompts_of(&fx.main_prompts)
        .into_iter()
        .find(|prompt| prompt.contains("Summary of:"))
        .unwrap();
    assert!(summary_prompt.contains("main answer"), "{summary_prompt}");
    assert!(
        !summary_prompt.contains("shadow answer"),
        "{summary_prompt}"
    );
    assert_eq!(
        run.step_results
            .iter()
            .find(|result| result.step_name == "review")
            .unwrap()
            .output,
        "main answer"
    );
}

#[tokio::test]
async fn a_shadow_changing_the_prompt_loads_it_on_the_same_context() {
    let fx = fixture("proj-shadow-prompt", scripted("ok")).await;
    let mut inline = shadow_of("terse");
    inline.prompt = Some(ShadowPrompt::Inline {
        template: "Tersely PR-42 :: {{steps.who.summary}}".into(),
    });
    let wf = fx.workflow(vec![inline]);
    let run = fx.run(&wf, "run-prompt").await;
    assert_eq!(run.status, RunStatus::Success);
    let rows = fx.settled("run-prompt", 1).await;
    assert_eq!(rows[0].status, ShadowStatus::Success, "{:?}", rows[0]);
    let prompts = prompts_of(&fx.main_prompts);
    assert!(
        prompts
            .iter()
            .any(|p| p.contains("Tersely PR-42 :: exit 0")),
        "the shadow's own template saw the previous outputs: {prompts:?}"
    );
    let all = fx.rows("run-prompt").await;
    let main = all
        .iter()
        .find(|row| row.shadow_id == step_shadows::MAIN)
        .unwrap();
    assert_ne!(
        main.identity.prompt, rows[0].identity.prompt,
        "the header can say the prompt differs"
    );
    assert!(rows[0].identity.prompt.starts_with("inline:"));
}

#[tokio::test]
async fn a_shadow_loads_another_quick_prompt_pinned_with_the_run() {
    let fx = fixture("proj-shadow-qp", scripted("ok")).await;
    seed_qp(
        &fx.state,
        "qp-alt",
        "ALT TEMPLATE for {{steps.who.summary}}",
        &[],
    )
    .await;
    let mut alt = shadow_of("alt-prompt");
    alt.prompt = Some(ShadowPrompt::QuickPrompt {
        id: "qp-alt".into(),
        version: None,
    });
    let wf = fx.workflow(vec![alt]);
    let run = fx.run(&wf, "run-qp").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let rows = fx.settled("run-qp", 1).await;
    assert_eq!(rows[0].status, ShadowStatus::Success, "{:?}", rows[0]);
    assert!(
        prompts_of(&fx.main_prompts)
            .iter()
            .any(|p| p.contains("ALT TEMPLATE for exit 0")),
        "the shadow's Quick Prompt rendered on the main step's context"
    );
    assert!(
        rows[0].identity.prompt.starts_with("qp-alt"),
        "{:?}",
        rows[0].identity
    );
}

#[tokio::test]
async fn a_shadow_never_runs_a_step_that_acts_outside_kronn() {
    let fx = fixture("proj-shadow-outside", scripted("posted")).await;
    let mut wf = fx.workflow(vec![shadow_of("trial")]);
    // Saved by hand, past the validation that refuses it.
    wf.steps[1].external_effect = true;
    let run = fx.run(&wf, "run-outside").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let rows = fx.settled("run-outside", 1).await;
    assert_eq!(rows[0].status, ShadowStatus::Skipped, "{:?}", rows[0]);
    assert!(rows[0]
        .stopped
        .as_deref()
        .unwrap_or_default()
        .contains("outside Kronn"));
    assert!(rows[0].output.is_empty());
    let reviews = prompts_of(&fx.main_prompts)
        .into_iter()
        .filter(|p| p.contains("Review PR-42"))
        .count();
    assert_eq!(reviews, 1, "only the main execution asked the agent");
}

#[test]
fn only_an_agent_step_that_stays_inside_kronn_may_run_shadows() {
    let agent = fake_step("a");
    assert!(shadow::can_run_shadows(&agent));
    let mut posts = fake_step("p");
    posts.external_effect = true;
    assert!(!shadow::can_run_shadows(&posts));
    let mut gate = fake_step("g");
    gate.step_type = StepType::Gate;
    assert!(!shadow::can_run_shadows(&gate));
    let exec = exec_step("e", "echo", &[]);
    assert!(!shadow::can_run_shadows(&exec));
}

#[tokio::test]
async fn a_shadow_that_fails_never_fails_the_main_step() {
    let mut fx = fixture("proj-shadow-fails", scripted("fine")).await;
    let mut broken = scripted("never");
    broken.fails = true;
    fx.model_for(AgentType::Codex, broken);
    let mut crashing = shadow_of("crashing");
    crashing.agent = on(AgentType::Codex, "gpt-x");
    let mut missing = shadow_of("missing-prompt");
    missing.prompt = Some(ShadowPrompt::QuickPrompt {
        id: "no-such-quick-prompt".into(),
        version: None,
    });
    let wf = fx.workflow(vec![crashing, missing]);
    let run = fx.run(&wf, "run-fails").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let rows = fx.settled("run-fails", 2).await;
    assert!(
        rows.iter().all(|row| row.status == ShadowStatus::Failed),
        "{rows:?}"
    );
    let missing = rows
        .iter()
        .find(|r| r.shadow_id == "missing-prompt")
        .unwrap();
    assert!(
        missing.output.contains("no-such-quick-prompt"),
        "the row says why: {}",
        missing.output
    );
}

#[tokio::test]
async fn a_stalled_shadow_is_stopped_by_its_time_budget_and_never_delays_the_main_step() {
    let mut fx = fixture("proj-shadow-stall", scripted("quick")).await;
    let mut slow = scripted("too late");
    slow.delay = Some(std::time::Duration::from_secs(60));
    fx.model_for(AgentType::Codex, slow);
    let mut local = shadow_of("local");
    local.agent = on(AgentType::Codex, "qwen");
    local.budget = Some(ShadowBudget {
        max_duration_secs: Some(3),
        ..Default::default()
    });
    let wf = fx.workflow(vec![local]);
    let run = fx.run(&wf, "run-stall").await;
    // The run is over while the shadow still runs: the main step did not wait.
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let mid = fx.rows("run-stall").await;
    assert!(
        mid.iter()
            .any(|row| row.shadow_id == "local" && row.status == ShadowStatus::Running),
        "{mid:?}"
    );
    let rows = fx.settled("run-stall", 1).await;
    assert_eq!(rows[0].status, ShadowStatus::TimedOut, "{:?}", rows[0]);
    assert!(rows[0]
        .stopped
        .as_deref()
        .unwrap_or_default()
        .contains("time budget"));
    assert!(rows[0].duration_ms.unwrap_or_default() >= 3000);
}

/// Waits until `prompts` holds a prompt: the model is mid-step.
async fn until_asked(prompts: &Prompts) {
    for _ in 0..400 {
        if !prompts_of(prompts).is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("the model was never asked");
}

/// A shadow that is answering in 2 s, on a model of its own.
fn answering_late(fx: &mut Fixture) -> (StepShadow, Prompts) {
    let mut slow = scripted("too late");
    slow.delay = Some(std::time::Duration::from_secs(2));
    let asked = fx.model_for(AgentType::Codex, slow);
    let mut local = shadow_of("local");
    local.agent = on(AgentType::Codex, "qwen");
    (local, asked)
}

fn shadow_keys(state: &crate::AppState) -> Vec<String> {
    state
        .cancel_registry
        .lock()
        .unwrap()
        .keys()
        .filter(|key| key.starts_with("shadow:"))
        .cloned()
        .collect()
}

#[tokio::test]
async fn cancelling_the_run_tree_stops_the_shadows_that_outlive_the_runner_and_they_write_nothing_after(
) {
    let mut fx = fixture("proj-shadow-tree-cancel", scripted("quick")).await;
    let (local, asked) = answering_late(&mut fx);
    let wf = fx.workflow(vec![local]);
    let run = fx.run(&wf, "run-tree-cancel").await;
    // The main step is done and its runner gone; the shadow still answers.
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    until_asked(&asked).await;
    assert_eq!(shadow_keys(&fx.state).len(), 1, "the shadow is registered");

    crate::workflows::cancellation::cancel_run_tree(
        &fx.state,
        "run-tree-cancel",
        crate::workflows::cancellation::CancellationScope::RunTree,
        "operator stop",
    )
    .await
    .unwrap();

    let rows = fx.settled("run-tree-cancel", 1).await;
    assert_eq!(rows[0].status, ShadowStatus::Interrupted, "{:?}", rows[0]);
    assert!(rows[0].output.is_empty());
    // Well past the 2 s the model needed: the row has not been written again.
    tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
    let later = fx.settled("run-tree-cancel", 1).await;
    assert_eq!(later[0].status, ShadowStatus::Interrupted, "{:?}", later[0]);
    assert!(later[0].output.is_empty(), "{:?}", later[0]);
    assert_eq!(later[0].finished_at, rows[0].finished_at);
    assert!(shadow_keys(&fx.state).is_empty(), "the registry is clean");
}

#[tokio::test]
async fn cancelling_the_run_mid_step_stops_its_shadows_with_the_runs_token() {
    let mut slow_main = scripted("main answer");
    slow_main.delay = Some(std::time::Duration::from_secs(2));
    let mut fx = fixture("proj-shadow-run-cancel", slow_main).await;
    let (local, asked) = answering_late(&mut fx);
    let wf = fx.workflow(vec![local]);
    let state = fx.state.clone();
    let canceller = async {
        until_asked(&asked).await;
        let token = state
            .cancel_registry
            .lock()
            .unwrap()
            .get("run-mid-cancel")
            .cloned()
            .expect("the run is registered");
        token.cancel();
    };
    let (run, ()) = tokio::join!(fx.run(&wf, "run-mid-cancel"), canceller);
    assert_eq!(run.status, RunStatus::Cancelled, "{:?}", run.step_results);

    let rows = fx.settled("run-mid-cancel", 1).await;
    assert_eq!(rows[0].status, ShadowStatus::Interrupted, "{:?}", rows[0]);
    tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
    let later = fx.settled("run-mid-cancel", 1).await;
    assert_eq!(later[0].status, ShadowStatus::Interrupted, "{:?}", later[0]);
    assert!(later[0].output.is_empty(), "{:?}", later[0]);
    assert_eq!(later[0].finished_at, rows[0].finished_at);
    assert!(shadow_keys(&fx.state).is_empty(), "the registry is clean");
}

#[tokio::test]
async fn a_shadow_over_its_token_budget_is_marked_and_the_run_is_untouched() {
    // Every execution of the step answers with the same usage: the main one
    // and the shadows, which differ by their budget alone.
    let mut spender = scripted("long answer");
    spender.usage = Some((600, 600));
    spender.cost_micros = Some(2_500_000);
    let fx = fixture("proj-shadow-budget", spender).await;
    let budgeted = |id: &str, budget: ShadowBudget| {
        let mut shadow = shadow_of(id);
        shadow.agent = on(AgentType::ClaudeCode, "claude-opus-4-5");
        shadow.budget = Some(budget);
        shadow
    };
    let wf = fx.workflow(vec![
        budgeted(
            "tokens",
            ShadowBudget {
                max_tokens: Some(100),
                ..Default::default()
            },
        ),
        budgeted(
            "within",
            ShadowBudget {
                max_tokens: Some(1_000_000),
                max_cost_usd: Some(1000.0),
                ..Default::default()
            },
        ),
    ]);
    let run = fx.run(&wf, "run-budget").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let rows = fx.settled("run-budget", 2).await;
    let row = |id: &str| rows.iter().find(|r| r.shadow_id == id).unwrap();
    assert_eq!(
        row("tokens").status,
        ShadowStatus::OverBudget,
        "{:?}",
        row("tokens")
    );
    assert!(row("tokens")
        .stopped
        .as_deref()
        .unwrap()
        .contains("token budget"));
    assert!(row("tokens").tokens_used.unwrap() > 100);
    assert_eq!(
        row("within").status,
        ShadowStatus::Success,
        "{:?}",
        row("within")
    );
    // What it spent is reported with it, the output kept. (The cost budget is
    // checked on the cost an agent reports: see `a_budget_is_checked_on_what_the_shadow_spent`.)
    assert!(row("within").tokens_used.unwrap_or_default() > 0);
    assert_eq!(row("within").output, "long answer");
}

#[test]
fn a_budget_is_checked_on_what_the_shadow_spent() {
    let mut shadow = shadow_of("s");
    assert_eq!(
        shadow::budget_exceeded(&shadow, 1_000_000, Some(99.0)),
        None
    );
    shadow.budget = Some(ShadowBudget {
        max_tokens: Some(1000),
        max_cost_usd: Some(0.5),
        max_duration_secs: Some(60),
    });
    assert_eq!(
        shadow::budget_exceeded(&shadow, 1000, Some(0.5)),
        None,
        "at the limit"
    );
    assert!(shadow::budget_exceeded(&shadow, 1001, None)
        .unwrap()
        .contains("1001"));
    assert!(shadow::budget_exceeded(&shadow, 10, Some(0.51))
        .unwrap()
        .contains("cost"));
    assert_eq!(
        shadow::budget_exceeded(&shadow, 10, None),
        None,
        "a cost that is not reported cannot exceed the budget"
    );
}

#[tokio::test]
async fn a_shadow_covers_the_projects_it_names_and_samples_one_run_in_n() {
    let fx = fixture("proj-shadow-sampling", scripted("ok")).await;
    let mut sampled = shadow_of("sampled");
    sampled.sample_every = Some(2);
    let mut elsewhere = shadow_of("elsewhere");
    elsewhere.project_ids = vec!["another-project".into()];
    let mut here = shadow_of("here");
    here.project_ids = vec!["proj-shadow-sampling".into()];
    let wf = fx.workflow(vec![sampled, elsewhere, here]);

    for (id, expected) in [
        ("run-s1", vec!["here", "sampled"]),
        ("run-s2", vec!["here"]),
        ("run-s3", vec!["here", "sampled"]),
    ] {
        let run = fx.run(&wf, id).await;
        assert_eq!(run.status, RunStatus::Success);
        let rows = fx.settled(id, expected.len()).await;
        let mut ids: Vec<&str> = rows.iter().map(|r| r.shadow_id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, expected, "{id}");
    }
}

#[tokio::test]
async fn a_disabled_shadow_runs_nothing_and_leaves_no_trace() {
    let fx = fixture("proj-shadow-off", scripted("ok")).await;
    let mut parked = shadow_of("parked");
    parked.enabled = false;
    let wf = fx.workflow(vec![parked]);
    let run = fx.run(&wf, "run-off").await;
    assert_eq!(run.status, RunStatus::Success);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(fx.rows("run-off").await.is_empty());
    assert_eq!(
        prompts_of(&fx.main_prompts).len(),
        2,
        "review and summary only"
    );
}

#[tokio::test]
async fn the_comparison_sets_the_main_execution_beside_each_shadow() {
    let mut fx = fixture("proj-shadow-compare", scripted("main answer")).await;
    let mut other = scripted("other answer");
    other.usage = Some((10, 20));
    fx.model_for(AgentType::Codex, other);
    let mut differing = shadow_of("other-model");
    differing.agent = on(AgentType::Codex, "gpt-x");
    let wf = fx.workflow(vec![shadow_of("same"), differing]);
    let run = fx.run(&wf, "run-compare").await;
    fx.settled("run-compare", 2).await;

    let id = "run-compare".to_string();
    let steps = fx
        .state
        .db
        .with_read_conn(move |conn| {
            let run = crate::db::workflows::get_run(conn, &id)?.unwrap();
            shadow::comparisons(conn, &run)
        })
        .await
        .unwrap();
    assert_eq!(steps.len(), 1, "only the review ran shadows");
    let review = &steps[0];
    assert_eq!(review.step_name, "review");
    assert_eq!(review.main.shadow_id, None);
    assert_eq!(review.main.output, "main answer");
    assert_eq!(review.main.status, ShadowStatus::Success);
    assert_eq!(
        review.main.duration_ms,
        Some(run.step_results[1].duration_ms),
        "the main metrics are the run's own"
    );
    let ids: Vec<Option<&str>> = review
        .shadows
        .iter()
        .map(|s| s.shadow_id.as_deref())
        .collect();
    assert_eq!(ids, [Some("same"), Some("other-model")]);
    assert_eq!(review.shadows[0].output, "main answer");
    assert_eq!(review.shadows[1].output, "other answer");
    assert_eq!(review.shadows[1].identity.agent, "Codex");
    assert!(review.shadows[1].tokens_used.unwrap_or_default() > 0);
    assert!(review.shadows[1].duration_ms.is_some());
}

// ─── What a shadow changes ──────────────────────────────────────────────────

fn review_step() -> WorkflowStep {
    let mut review = fake_step("review");
    review.prompt_template = "Review {{pr}}".into();
    review.agent_settings = Some(crate::models::AgentSettings {
        model: Some("opus".into()),
        reasoning_effort: Some("high".into()),
        ..Default::default()
    });
    review.skill_ids = vec!["custom-review@4".into(), "rust".into()];
    review
}

#[tokio::test]
async fn a_shadow_changes_the_agent_model_and_connection_of_its_step() {
    let (state, _, _) = test_state_and_configs();
    let mut step = review_step();
    let mut local = shadow_of("qwen-local");
    local.agent = Some(ShadowAgent {
        agent: AgentType::Ollama,
        model: Some("qwen3:30b".into()),
        connection_id: Some("conn-ollama".into()),
        reasoning_effort: None,
    });
    shadow::apply_shadow(&state, &mut step, &local, None)
        .await
        .unwrap();
    assert_eq!(step.agent, AgentType::Ollama);
    let settings = step.agent_settings.as_ref().unwrap();
    assert_eq!(settings.model.as_deref(), Some("qwen3:30b"));
    assert_eq!(settings.connection_id.as_deref(), Some("conn-ollama"));
    // The step's effort is not carried over when the shadow names none.
    assert_eq!(settings.reasoning_effort, None);
    // Nothing else moved: same prompt, same skills.
    assert_eq!(step.prompt_template, "Review {{pr}}");
    assert_eq!(step.skill_ids, vec!["custom-review@4", "rust"]);
}

#[tokio::test]
async fn a_shadow_swaps_the_prompt_of_its_step() {
    let (state, _, _) = test_state_and_configs();
    let mut step = review_step();
    step.quick_prompt_id = Some("qp-old".into());
    let mut inline = shadow_of("inline");
    inline.prompt = Some(ShadowPrompt::Inline {
        template: "Review tersely: {{pr}}".into(),
    });
    shadow::apply_shadow(&state, &mut step, &inline, None)
        .await
        .unwrap();
    assert_eq!(step.prompt_template, "Review tersely: {{pr}}");
    assert_eq!(
        step.quick_prompt_id, None,
        "the step's own prompt link goes away"
    );

    let mut step = review_step();
    let mut other = shadow_of("other-qp");
    other.prompt = Some(ShadowPrompt::QuickPrompt {
        id: "qp-review".into(),
        version: Some(13),
    });
    shadow::apply_shadow(&state, &mut step, &other, None)
        .await
        .unwrap();
    assert_eq!(step.quick_prompt_id.as_deref(), Some("qp-review"));
    assert_eq!(step.quick_prompt_version, Some(13));
    assert!(
        step.prompt_template.is_empty(),
        "hydration loads the version"
    );
    assert!(
        step.skill_ids.is_empty(),
        "the new prompt's skills load instead of the old one's"
    );
}

#[tokio::test]
async fn a_shadow_pins_swaps_or_drops_skills() {
    let (state, _, _) = test_state_and_configs();
    let mut step = review_step();
    let mut skills = shadow_of("skills");
    skills.skills = [
        ("custom-review".to_string(), "custom-review@2".to_string()),
        ("rust".to_string(), String::new()),
    ]
    .into();
    shadow::apply_versions(&state, &mut step, &skills, None)
        .await
        .unwrap();
    // The pinned skill is matched on its base id and re-pinned; `rust` is dropped.
    assert_eq!(step.skill_ids, vec!["custom-review@2"]);
}

async fn seed_qp(state: &crate::AppState, id: &str, template: &str, skills: &[&str]) {
    let prompt: crate::models::QuickPrompt = serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "icon": "P", "prompt_template": template, "variables": [],
        "agent": "ClaudeCode", "skill_ids": skills,
        "created_at": chrono::Utc::now(), "updated_at": chrono::Utc::now()
    }))
    .unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &prompt))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_shadow_swaps_the_skills_a_step_inherits_from_its_quick_prompt() {
    let (state, _, _) = test_state_and_configs();
    seed_qp(&state, "qp-review", "Review.", &["custom-review", "rust"]).await;
    let mut step = fake_step("review");
    step.quick_prompt_id = Some("qp-review".into());
    let mut swapped = shadow_of("swapped");
    swapped.skills = [("custom-review".to_string(), "custom-review@2".to_string())].into();
    shadow::apply_versions(&state, &mut step, &swapped, None)
        .await
        .unwrap();
    assert_eq!(step.skill_ids, vec!["custom-review@2", "rust"]);
}

#[tokio::test]
async fn dropping_every_skill_a_step_inherits_keeps_the_quick_prompt_content() {
    let (state, _, _) = test_state_and_configs();
    seed_qp(&state, "qp-review", "Review the PR.", &["custom-review"]).await;
    let mut step = fake_step("review");
    step.quick_prompt_id = Some("qp-review".into());
    let mut dropped = shadow_of("without-skill");
    dropped.skills = [("custom-review".to_string(), String::new())].into();
    shadow::apply_versions(&state, &mut step, &dropped, None)
        .await
        .unwrap();
    assert!(step.skill_ids.is_empty());
    assert_eq!(step.prompt_template, "Review the PR.");
    assert_eq!(
        step.quick_prompt_id, None,
        "no link is left that would hand the dropped skill back"
    );
}

#[tokio::test]
async fn a_shadow_loads_a_chosen_version_of_the_steps_quick_prompt() {
    let (state, _, _) = test_state_and_configs();
    seed_qp(&state, "qp-review", "Version one.", &["rust"]).await;
    let edit = {
        let mut prompt = state
            .db
            .with_read_conn(|conn| crate::db::quick_prompts::get_quick_prompt(conn, "qp-review"))
            .await
            .unwrap()
            .unwrap();
        prompt.prompt_template = "Version two.".into();
        prompt
    };
    state
        .db
        .with_conn(move |conn| crate::db::quick_prompts::update_quick_prompt(conn, &edit))
        .await
        .unwrap();
    let mut step = fake_step("review");
    step.quick_prompt_id = Some("qp-review".into());
    let mut v1 = shadow_of("v1");
    v1.quick_prompt_versions = [("qp-review".to_string(), 1)].into();
    shadow::apply_versions(&state, &mut step, &v1, None)
        .await
        .unwrap();
    assert_eq!(step.prompt_template, "Version one.");
    assert_eq!(step.quick_prompt_version, Some(1));
}

#[tokio::test]
async fn a_shadows_token_budget_caps_the_generation_of_an_agent_that_takes_one() {
    let mut step = fake_step("review");
    step.agent = AgentType::Ollama;
    let mut capped = shadow_of("capped");
    capped.budget = Some(ShadowBudget {
        max_tokens: Some(4000),
        ..Default::default()
    });
    let (state, _, _) = test_state_and_configs();
    shadow::apply_shadow(&state, &mut step, &capped, None)
        .await
        .unwrap();
    assert_eq!(
        step.agent_settings.as_ref().and_then(|s| s.max_tokens),
        Some(4000)
    );
    // The step's own lower cap stays.
    let mut step = fake_step("review");
    step.agent = AgentType::Ollama;
    step.agent_settings = Some(crate::models::AgentSettings {
        max_tokens: Some(1000),
        ..Default::default()
    });
    shadow::apply_shadow(&state, &mut step, &capped, None)
        .await
        .unwrap();
    assert_eq!(
        step.agent_settings.as_ref().and_then(|s| s.max_tokens),
        Some(1000)
    );
}

// ─── Saving ─────────────────────────────────────────────────────────────────

#[test]
fn shadows_are_checked_when_a_person_saves_them() {
    let with = |shadows: Vec<StepShadow>| {
        let mut step = fake_step("review");
        step.shadows = shadows;
        step
    };
    let refused =
        |shadows: Vec<StepShadow>| shadow::validate(&[with(shadows)]).expect_err("must be refused");
    assert!(shadow::validate(&[with(vec![shadow_of("a"), shadow_of("b")])]).is_ok());
    assert!(refused(vec![shadow_of("a"), shadow_of("a")]).contains("twice"));
    assert!(refused(vec![shadow_of("Not A Slug")]).contains("must be 1 to 40"));
    assert!(refused(vec![shadow_of("")]).contains("must be 1 to 40"));
    let many: Vec<StepShadow> = (0..9).map(|n| shadow_of(&format!("s{n}"))).collect();
    assert!(refused(many).contains("at most 8"));
    let mut sampled = shadow_of("s");
    sampled.sample_every = Some(0);
    assert!(refused(vec![sampled]).contains("samples one run"));
    let mut pinned = shadow_of("s");
    pinned.skills = [("custom-x".to_string(), "custom-x@zero".to_string())].into();
    assert!(refused(vec![pinned]).contains("custom-x"));
    let mut versions = shadow_of("s");
    versions.quick_prompt_versions = [("qp".to_string(), 0)].into();
    assert!(refused(vec![versions]).contains("no version 0"));
    let mut custom = shadow_of("s");
    custom.agent = Some(ShadowAgent {
        agent: AgentType::Custom,
        model: None,
        connection_id: None,
        reasoning_effort: None,
    });
    assert!(refused(vec![custom]).contains("needs the connection"));

    // Only an Agent step that stays inside Kronn.
    let mut posts = with(vec![shadow_of("s")]);
    posts.external_effect = true;
    assert!(shadow::validate(&[posts])
        .unwrap_err()
        .contains("outside Kronn"));
    let mut gate = with(vec![shadow_of("s")]);
    gate.step_type = StepType::Gate;
    assert!(shadow::validate(&[gate])
        .unwrap_err()
        .contains("Agent step"));
    // A step without shadows is never in question.
    let mut plain = fake_step("p");
    plain.external_effect = true;
    assert!(shadow::validate(&[plain]).is_ok());
}

#[test]
fn an_agent_write_keeps_the_shadows_a_human_set() {
    let mut stored = fake_step("review");
    stored.id = Some("id-review".into());
    stored.shadows = vec![shadow_of("kept")];
    let mut sent = fake_step("review");
    sent.id = Some("id-review".into());
    let mut renamed = fake_step("renamed");
    renamed.id = Some("id-review".into());
    let mut fresh = fake_step("brand-new");
    fresh.shadows = vec![shadow_of("invented")];
    let mut steps = vec![sent, renamed, fresh];
    shadow::keep_stored_shadows(&mut steps, &[stored]);
    assert_eq!(steps[0].shadows[0].id, "kept");
    assert_eq!(steps[1].shadows[0].id, "kept", "matched on the durable id");
    assert!(steps[2].shadows.is_empty(), "a new step starts with none");
}

#[test]
fn an_imported_step_shadow_names_the_imported_skills_and_drops_qp_versions() {
    let mut wf = make_workflow_with_artifacts(Default::default());
    let mut review = fake_step("review");
    let mut s = shadow_of("s");
    s.skills = [("custom-a".to_string(), "custom-a@2".to_string())].into();
    s.quick_prompt_versions = [("qp-old".to_string(), 3), ("qp-local".to_string(), 2)].into();
    s.prompt = Some(ShadowPrompt::QuickPrompt {
        id: "qp-old".into(),
        version: Some(3),
    });
    review.shadows = vec![s];
    wf.steps = vec![review];
    let skills = [
        ("custom-a".to_string(), "custom-b".to_string()),
        ("custom-a@2".to_string(), "custom-c@1".to_string()),
    ]
    .into();
    let qps = [("qp-old".to_string(), "qp-new".to_string())].into();
    shadow::remap_imported(&mut wf, &skills, &qps);
    let s = &wf.steps[0].shadows[0];
    assert_eq!(
        s.skills.get("custom-b").map(String::as_str),
        Some("custom-c@1")
    );
    assert!(!s.skills.contains_key("custom-a"));
    assert!(
        !s.quick_prompt_versions.contains_key("qp-old"),
        "a Quick Prompt comes over without its history"
    );
    assert_eq!(s.quick_prompt_versions.get("qp-local"), Some(&2));
    assert_eq!(
        s.prompt,
        Some(ShadowPrompt::QuickPrompt {
            id: "qp-new".into(),
            version: None
        })
    );
}

// ─── Storage and migration ──────────────────────────────────────────────────

#[tokio::test]
async fn shadows_are_stored_apart_from_the_steps_the_trust_checks_compare() {
    let (state, _, _) = test_state_and_configs();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-stored".into();
    let mut review = fake_step("review");
    review.shadows = vec![shadow_of("a"), shadow_of("b")];
    wf.steps = vec![review, fake_step("summary")];
    let stored = wf.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &stored))
        .await
        .unwrap();
    let (steps_json, shadow_json, read) = state
        .db
        .with_read_conn(|conn| {
            let (steps_json, shadow_json): (String, Option<String>) = conn.query_row(
                "SELECT steps_json, shadow_json FROM workflows WHERE id = 'wf-stored'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let read = crate::db::workflows::get_workflow(conn, "wf-stored")?.unwrap();
            Ok((steps_json, shadow_json, read))
        })
        .await
        .unwrap();
    assert!(!steps_json.contains("shadows"), "{steps_json}");
    let map: serde_json::Value = serde_json::from_str(&shadow_json.unwrap()).unwrap();
    let id = read.steps[0].id.clone().unwrap();
    assert_eq!(map[&id].as_array().unwrap().len(), 2, "{map}");
    assert_eq!(read.steps[0].shadows.len(), 2);
    assert!(read.steps[1].shadows.is_empty());

    // An agent's write of the same steps without shadows keeps what a human set.
    let mut edited = read.clone();
    edited.steps[0].shadows.clear();
    edited.name = "renamed by an agent".into();
    let by_agent = edited.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::update_workflow_as_agent(conn, &by_agent))
        .await
        .unwrap();
    let after_agent = state
        .db
        .with_read_conn(|conn| crate::db::workflows::get_workflow(conn, "wf-stored"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_agent.name, "renamed by an agent");
    assert_eq!(after_agent.steps[0].shadows.len(), 2, "human-only");

    // A person's write removes them.
    state
        .db
        .with_conn(move |conn| crate::db::workflows::update_workflow(conn, &edited))
        .await
        .unwrap();
    let after_human = state
        .db
        .with_read_conn(|conn| crate::db::workflows::get_workflow(conn, "wf-stored"))
        .await
        .unwrap()
        .unwrap();
    assert!(after_human.steps[0].shadows.is_empty());
}

fn legacy_steps() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"id": "s-review", "name": "review", "step_type": {"type": "Agent"}}),
        serde_json::json!({"id": "s-triage", "name": "triage", "step_type": {"type": "Agent"}}),
        serde_json::json!({"id": "s-post", "name": "post", "step_type": {"type": "Agent"}, "external_effect": true}),
        serde_json::json!({"id": "s-exec", "name": "run", "step_type": {"type": "Exec"}}),
        serde_json::json!({"name": "no-id", "step_type": {"type": "Agent"}}),
    ]
}

#[test]
fn a_flat_workflow_shadow_becomes_one_shadow_on_each_agent_step() {
    let legacy = serde_json::json!({
        "enabled": true, "project_ids": ["p1"],
        "skills": {"custom-review": "custom-review@2"},
        "quick_prompt_versions": {"qp-review": 3}
    });
    let map = shadow::legacy_shadow_to_steps(&legacy, &legacy_steps());
    let mut ids: Vec<&String> = map.keys().collect();
    ids.sort();
    assert_eq!(
        ids,
        ["s-review", "s-triage"],
        "Agent steps that stay inside Kronn, and carry an id"
    );
    for shadows in map.values() {
        assert_eq!(shadows.len(), 1);
        let s = &shadows[0];
        assert_eq!(s.id, "shadow", "the variant its runs always had");
        assert!(s.enabled);
        assert_eq!(s.project_ids, vec!["p1"]);
        assert_eq!(s.skills["custom-review"], "custom-review@2");
        assert_eq!(s.quick_prompt_versions["qp-review"], 3);
        assert!(s.agent.is_none() && s.prompt.is_none() && s.budget.is_none());
    }
    // A shadow that was off stays off on the steps.
    let off =
        shadow::legacy_shadow_to_steps(&serde_json::json!({"enabled": false}), &legacy_steps());
    assert!(!off["s-review"][0].enabled);
    assert!(off["s-review"][0].is_identical());
}

#[test]
fn workflow_lanes_become_shadows_with_the_overrides_each_step_had() {
    let legacy = serde_json::json!({
        "enabled": true,
        "lanes": [
            {"id": "qwen-local", "label": "Qwen", "role": "contributor",
             "agent": {"agent": "Ollama", "model": "qwen3:30b", "connection_id": "c1"},
             "step_agents": {"triage": {"agent": "Codex", "model": "gpt-x"}},
             "prompt": {"type": "inline", "template": "Tersely."}, "prompt_steps": ["review"],
             "budget": {"max_tokens": 5000, "max_duration_secs": 7200},
             "sample_every": 3},
            {"id": "v3.10.1", "role": "witness", "skills": {"rust": ""}}
        ]
    });
    let map = shadow::legacy_shadow_to_steps(&legacy, &legacy_steps());
    let review = &map["s-review"];
    assert_eq!(review.len(), 2);
    assert_eq!(review[0].id, "qwen-local");
    assert_eq!(review[0].label, "Qwen");
    assert_eq!(review[0].agent.as_ref().unwrap().agent, AgentType::Ollama);
    assert_eq!(
        review[0].agent.as_ref().unwrap().connection_id.as_deref(),
        Some("c1")
    );
    assert!(matches!(
        review[0].prompt,
        Some(ShadowPrompt::Inline { .. })
    ));
    assert_eq!(review[0].budget.as_ref().unwrap().max_tokens, Some(5000));
    assert_eq!(review[0].sample_every, Some(3));
    assert_eq!(review[1].id, "v3.10.1");
    assert_eq!(review[1].skills["rust"], "");

    // `step_agents` names the step it applies to; `prompt_steps` the prompt's.
    let triage = &map["s-triage"];
    assert_eq!(triage[0].agent.as_ref().unwrap().agent, AgentType::Codex);
    assert!(
        triage[0].prompt.is_none(),
        "the prompt targeted the review only"
    );
}

#[tokio::test]
async fn the_migration_moves_a_workflow_shadow_onto_the_steps_of_its_workflow() {
    let (state, _, _) = test_state_and_configs();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-legacy".into();
    wf.steps = vec![fake_step("review"), exec_step("run", "echo", &[])];
    let stored = wf.clone();
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &stored)?;
            // As the KT-1449 build stored it.
            conn.execute(
                "UPDATE workflows SET shadow_json = ?1 WHERE id = 'wf-legacy'",
                [r#"{"enabled":true,"skills":{"rust":"security"}}"#],
            )?;
            // A second workflow already holds step shadows: left as they are.
            let mut other = stored.clone();
            other.id = "wf-current".into();
            other.steps[0].shadows = vec![shadow_of("kept")];
            crate::db::workflows::insert_workflow(conn, &other)?;
            crate::workflows::shadow::migrate_workflow_shadows(conn)?;
            // Idempotent: a second pass finds nothing left to move.
            crate::workflows::shadow::migrate_workflow_shadows(conn)?;
            Ok(())
        })
        .await
        .unwrap();
    let (legacy, current) = state
        .db
        .with_read_conn(|conn| {
            Ok((
                crate::db::workflows::get_workflow(conn, "wf-legacy")?.unwrap(),
                crate::db::workflows::get_workflow(conn, "wf-current")?.unwrap(),
            ))
        })
        .await
        .unwrap();
    assert_eq!(legacy.steps[0].shadows.len(), 1);
    assert_eq!(legacy.steps[0].shadows[0].id, "shadow");
    assert_eq!(legacy.steps[0].shadows[0].skills["rust"], "security");
    assert!(
        legacy.steps[1].shadows.is_empty(),
        "an Exec step carries none"
    );
    assert_eq!(current.steps[0].shadows[0].id, "kept");
}

#[tokio::test]
async fn a_shadow_left_running_by_a_restart_is_marked_interrupted() {
    let (state, _, _) = test_state_and_configs();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-restart".into();
    let run = pending_run("run-restart", &wf.id);
    insert_wf_and_run(&state, &wf, &run).await;
    let row = StepShadowRow {
        run_id: "run-restart".into(),
        result_index: 0,
        step_name: "review".into(),
        shadow_id: "s".into(),
        label: "S".into(),
        status: ShadowStatus::Running,
        identity: Default::default(),
        output: String::new(),
        tokens_used: None,
        duration_ms: None,
        cost_usd: None,
        model: None,
        stopped: None,
        started_at: chrono::Utc::now().to_rfc3339(),
        finished_at: None,
    };
    let rows = state
        .db
        .with_conn(move |conn| {
            step_shadows::insert(conn, &row)?;
            assert_eq!(step_shadows::interrupt_running(conn)?, 1);
            step_shadows::list_for_run(conn, "run-restart")
        })
        .await
        .unwrap();
    assert_eq!(rows[0].status, ShadowStatus::Interrupted);
    assert!(rows[0]
        .stopped
        .as_deref()
        .unwrap_or_default()
        .contains("restarted"));
}

#[tokio::test]
async fn a_pr_review_step_runs_an_identical_shadow_and_a_local_one_beside_the_main_step() {
    let mut fx = fixture("proj-shadow-pr", scripted("main review")).await;
    // The local model answers through the agent route of the test.
    let local = fx.model_for(AgentType::Codex, scripted("local review"));
    let mut local_shadow = shadow_of("qwen-local");
    local_shadow.label = "Qwen (Ollama)".into();
    local_shadow.agent = Some(ShadowAgent {
        agent: AgentType::Codex,
        model: Some("qwen3:30b".into()),
        connection_id: None,
        reasoning_effort: None,
    });
    let mut wf = fx.workflow(vec![shadow_of("identical"), local_shadow]);
    // A later step that posts: it runs in the main path only.
    let mut post = exec_step("post", "echo", &["posted"]);
    post.external_effect = true;
    wf.steps.push(post);
    let run = fx.run(&wf, "run-pr").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    let rows = fx.settled("run-pr", 2).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(prompts_of(&local).len(), 1);
    assert_eq!(rows[1].label, "Qwen (Ollama)");
    assert_eq!(rows[1].identity.model.as_deref(), Some("qwen3:30b"));
    // The posting step ran once, in the run: shadows never reach it.
    let names: Vec<&str> = run
        .step_results
        .iter()
        .map(|result| result.step_name.as_str())
        .collect();
    assert_eq!(names, ["who", "review", "summary", "post"], "{names:?}");
}
