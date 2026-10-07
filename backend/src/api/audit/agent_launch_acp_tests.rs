// KT-927 — an audit step on an ACP agent (OpenCode and the other native ACP
// agents, Claude and Codex through their adapters), in the Full pipeline and in
// the partial one: how a Stop reaches it, what the step is recorded as having
// cost, and what becomes of an agent whose read of an environment file is refused.
//
// The pipelines are the real handlers and the real `cancel_audit`; the agent is a
// scripted ACP transport routed in by working directory, except in the last test,
// where a scripted `opencode` executable is started by Kronn's own spawn.
use super::*;
use crate::acp::{
    AcpAgent, AcpCapability, AcpConfigOption, AcpError, AcpInitialize, AcpNegotiatedCapabilities,
    AcpSessionEvent, AcpSessionTarget, AcpTransport,
};
use crate::agents::runner::PromptCacheUsage;
use crate::models::{AuditKind, LaunchAuditRequest, PartialAuditRequest};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender;

#[derive(Clone, Copy)]
struct Reported {
    input: u64,
    output: u64,
    cache_read: Option<u64>,
}

#[derive(Clone, Copy)]
enum Turn {
    /// Works until the session is cancelled or its process shut down.
    Hang,
    /// Answers, reporting this usage — `None` when the runtime reports nothing.
    Answer(Option<Reported>),
    /// Answers with `REPORTED` usage, the cost and the served model given.
    Priced {
        usd_micros: Option<u64>,
        model: Option<&'static str>,
    },
    /// Calls this many tools, the usage growing with each, without a word of
    /// text; then works until stopped.
    ToolChain(u32),
}

struct ScriptedAgent {
    turn: Turn,
    prompts: AtomicUsize,
    cancels: AtomicUsize,
    shutdowns: AtomicUsize,
}

impl ScriptedAgent {
    fn new(turn: Turn) -> Arc<Self> {
        Arc::new(Self {
            turn,
            prompts: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            shutdowns: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl AcpTransport for ScriptedAgent {
    async fn initialize(&self, _: AcpInitialize) -> Result<AcpNegotiatedCapabilities, AcpError> {
        Ok(AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities: BTreeSet::from([
                AcpCapability::Sessions,
                AcpCapability::Streaming,
                AcpCapability::Cancellation,
                AcpCapability::McpInjection,
            ]),
        })
    }
    async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
        AcpSessionTarget::new(AcpAgent::OpenCode, "scripted-session")
    }
    async fn config_options(&self) -> Vec<AcpConfigOption> {
        Vec::new()
    }
    async fn set_config_option(
        &self,
        _: &AcpSessionTarget,
        _: &str,
        _: &str,
    ) -> Result<(), AcpError> {
        Ok(())
    }
    async fn resume_session(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
        Ok(())
    }
    async fn prompt(
        &self,
        _: &AcpSessionTarget,
        _: &str,
        events: Sender<AcpSessionEvent>,
    ) -> Result<(), AcpError> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        match self.turn {
            Turn::Hang => std::future::pending().await,
            Turn::Answer(reported) => {
                let _ = events.send(AcpSessionEvent::TextDelta("done".into())).await;
                if let Some(reported) = reported {
                    let _ = events
                        .send(AcpSessionEvent::Usage {
                            input_tokens: reported.input,
                            output_tokens: reported.output,
                            prompt_cache: PromptCacheUsage {
                                cached_prompt_tokens: reported.cache_read,
                                cache_write_prompt_tokens: None,
                            },
                        })
                        .await;
                }
                let _ = events.send(AcpSessionEvent::Completed).await;
                Ok(())
            }
            Turn::Priced { usd_micros, model } => {
                if let Some(model) = model {
                    let _ = events
                        .send(AcpSessionEvent::ModelObserved(model.into()))
                        .await;
                }
                let _ = events.send(AcpSessionEvent::TextDelta("done".into())).await;
                let _ = events
                    .send(AcpSessionEvent::Usage {
                        input_tokens: REPORTED.input,
                        output_tokens: REPORTED.output,
                        prompt_cache: PromptCacheUsage::default(),
                    })
                    .await;
                if let Some(usd_micros) = usd_micros {
                    let _ = events.send(AcpSessionEvent::Cost { usd_micros }).await;
                }
                let _ = events.send(AcpSessionEvent::Completed).await;
                Ok(())
            }
            Turn::ToolChain(calls) => {
                for call in 1..=u64::from(calls) {
                    let _ = events
                        .send(AcpSessionEvent::ToolCall {
                            name: "Read".into(),
                        })
                        .await;
                    let _ = events
                        .send(AcpSessionEvent::ToolActivity(
                            // As a generic ACP runtime reports it: a secret in
                            // the title and the raw input, neither ever shown.
                            crate::agents::activity::ToolActivityUpdate::from_acp(&json!({
                                "toolCallId": call.to_string(), "kind": "read",
                                "title": format!("cat {}", crate::agents::activity::tests::SENTINEL),
                                "rawInput": {"file_path": crate::agents::activity::tests::SENTINEL},
                                "locations": [{"path": crate::agents::activity::tests::SENTINEL}]
                            }))
                            .unwrap(),
                        ))
                        .await;
                    let _ = events.send(AcpSessionEvent::ToolCallEnded).await;
                    let _ = events
                        .send(AcpSessionEvent::Usage {
                            input_tokens: 100 * call,
                            output_tokens: 10 * call,
                            prompt_cache: PromptCacheUsage::default(),
                        })
                        .await;
                }
                std::future::pending().await
            }
        }
    }
    async fn cancel(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), AcpError> {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
enum Pipeline {
    Full,
    Partial,
}

const REPORTED: Reported = Reported {
    input: 100,
    output: 20,
    cache_read: Some(30),
};

/// OpenCode runs only with its full-access setting on: the user turned it on.
fn new_state() -> AppState {
    let mut config = crate::core::config::default_config();
    config.agents.open_code.full_access = true;
    // The launch boundary reads the saved setting: this test's thread says on.
    std::mem::forget(crate::core::config::test_saved_access::set(
        &AgentType::OpenCode,
        true,
    ));
    state_with(config)
}

fn state_with(config: crate::models::AppConfig) -> AppState {
    AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        Arc::new(crate::db::Database::open_in_memory().unwrap()),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

async fn add_project(state: &AppState, id: &str, path: &Path) {
    let row: crate::models::Project = serde_json::from_value(json!({
        "id": id, "name": id, "path": path.to_string_lossy(),
        "repo_url": null, "token_override": null, "ai_config": {"detected": false, "configs": []},
        "created_at": chrono::Utc::now().to_rfc3339(), "updated_at": chrono::Utc::now().to_rfc3339()
    }))
    .unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &row))
        .await
        .unwrap();
}

/// Send every ACP launch in `project` to `agent`.
fn route(
    project: &Path,
    agent: &Arc<ScriptedAgent>,
) -> crate::agents::runner::test_acp_routes::RouteGuard {
    let project = project.to_string_lossy().into_owned();
    let work_dir = crate::agents::runner::resolve_agent_work_dir(Some(&project), &project).unwrap();
    crate::agents::runner::test_acp_routes::route(&work_dir, agent.clone())
}

/// Start a pipeline the way a request does and hand back its SSE body once it
/// ends. The Full run is the shortest specialized chain; the partial run
/// refreshes one section.
fn launch(
    state: &AppState,
    pipeline: Pipeline,
    id: &str,
    agent: AgentType,
) -> tokio::task::JoinHandle<String> {
    use axum::response::IntoResponse;
    let state = state.clone();
    let id = id.to_string();
    tokio::spawn(async move {
        let response = match pipeline {
            Pipeline::Full => crate::api::audit::full::full_audit(
                axum::extract::State(state),
                axum::extract::Path(id),
                axum::Json(LaunchAuditRequest {
                    agent,
                    connection_id: None,
                    tier: None,
                    kind: Some(AuditKind::Docker),
                    custom_prompt: None,
                    resume_run_id: None,
                }),
            )
            .await
            .into_response(),
            Pipeline::Partial => {
                let chain = crate::api::audit::assemble_chained_steps(AuditKind::Full);
                let step = chain
                    .iter()
                    .position(crate::api::audit::partial_selectable)
                    .expect("a refreshable section")
                    + 1;
                crate::api::audit::drift::partial_audit(
                    axum::extract::State(state),
                    axum::extract::Path(id),
                    axum::Json(PartialAuditRequest {
                        agent,
                        connection_id: None,
                        tier: None,
                        steps: vec![step],
                    }),
                )
                .await
                .into_response()
            }
        };
        let body = axum::body::to_bytes(response.into_body(), 1 << 22)
            .await
            .unwrap();
        String::from_utf8_lossy(&body).into_owned()
    })
}

fn sse_events(body: &str) -> Vec<(String, Value)> {
    body.split("\n\n")
        .filter_map(|block| {
            let mut name = None;
            let mut data = None;
            for line in block.lines() {
                if let Some(value) = line.strip_prefix("event:") {
                    name = Some(value.trim().to_string());
                } else if let Some(value) = line.strip_prefix("data:") {
                    data = Some(value.trim().to_string());
                }
            }
            Some((name?, serde_json::from_str(&data?).ok()?))
        })
        .collect()
}

fn first_step_done(body: &str) -> Value {
    sse_events(body)
        .into_iter()
        .find(|(name, _)| name == "step_done")
        .unwrap_or_else(|| panic!("no step_done in the stream: {body}"))
        .1
}

async fn until(what: &str, condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for: {what}"));
}

async fn latest_run_steps(
    state: &AppState,
    id: &str,
) -> (String, Vec<crate::models::AuditRunStep>) {
    let id = id.to_string();
    state
        .db
        .with_conn(move |conn| {
            let run = crate::db::audit_runs::list_recent(conn, &id, 1)?.remove(0);
            let steps = crate::db::audit_runs::list_audit_steps(conn, &run.id)?;
            Ok((run.status, steps))
        })
        .await
        .unwrap()
}

/// DoD — a Stop reaches an ACP step and ends it in seconds. The agent never
/// finishes by itself: only the session cancel and the process shutdown can end
/// the step, which is what killing the lifeline's PID never did (about 8 minutes
/// on an OpenCode audit).
async fn a_stop_ends_an_acp_step_in_seconds(pipeline: Pipeline) {
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    let agent = ScriptedAgent::new(Turn::Hang);
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-stop", project.path()).await;
    let run = launch(&state, pipeline, "proj-stop", AgentType::OpenCode);
    until("the agent to be working on its step", || {
        agent.prompts.load(Ordering::SeqCst) > 0
    })
    .await;

    let stopped = Instant::now();
    let response = crate::api::audit::full::cancel_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("proj-stop".to_string()),
    )
    .await;
    let body = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .unwrap_or_else(|_| panic!("{pipeline:?}: the stopped audit never ended"))
        .unwrap();

    assert!(response.0.success, "{pipeline:?}: {:?}", response.0.error);
    assert!(
        stopped.elapsed() < Duration::from_secs(10),
        "{pipeline:?}: stopping took {:?}",
        stopped.elapsed()
    );
    assert!(
        agent.cancels.load(Ordering::SeqCst) >= 1,
        "{pipeline:?}: the ACP session is cancelled"
    );
    assert!(
        agent.shutdowns.load(Ordering::SeqCst) >= 1,
        "{pipeline:?}: the agent's process is shut down"
    );
    assert!(
        sse_events(&body)
            .iter()
            .any(|(name, _)| name == "cancelled"),
        "{pipeline:?}: {body}"
    );
    let (status, _) = latest_run_steps(&state, "proj-stop").await;
    assert_eq!(status, "Cancelled", "{pipeline:?}");
}

#[tokio::test]
async fn cancel_audit_stops_an_opencode_step_of_a_full_audit_in_seconds() {
    a_stop_ends_an_acp_step_in_seconds(Pipeline::Full).await;
}

#[tokio::test]
async fn cancel_audit_stops_an_opencode_step_of_a_partial_audit_in_seconds() {
    a_stop_ends_an_acp_step_in_seconds(Pipeline::Partial).await;
}

/// DoD — the tokens an ACP step consumed are recorded, input, output and cache as
/// the runtime gave them, on the step and on the run.
async fn an_acp_step_records_the_tokens_its_runtime_reported(pipeline: Pipeline) {
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    let agent = ScriptedAgent::new(Turn::Answer(Some(REPORTED)));
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-usage", project.path()).await;

    let body = launch(&state, pipeline, "proj-usage", AgentType::OpenCode)
        .await
        .unwrap();

    // OpenCode's cache accounting is unknown to pricing: input as reported.
    let done = first_step_done(&body);
    assert_eq!(done["tokens"], json!(120), "{pipeline:?}: {done}");
    assert_eq!(done["total_tokens"], json!(120), "{pipeline:?}: {done}");
    let (_, steps) = latest_run_steps(&state, "proj-usage").await;
    let step = &steps[0];
    assert_eq!(step.step_tokens, Some(120), "{pipeline:?}");
    assert_eq!(step.input_tokens, Some(100), "{pipeline:?}");
    assert_eq!(step.output_tokens, Some(20), "{pipeline:?}");
    assert_eq!(step.cache_read_tokens, Some(30), "{pipeline:?}");
    assert_eq!(
        step.cache_write_tokens, None,
        "{pipeline:?}: a cache figure the runtime did not give is absent, not 0"
    );
    assert_eq!(step.cumulative_tokens, Some(120), "{pipeline:?}");
}

#[tokio::test]
async fn a_full_audit_step_on_opencode_records_its_tokens() {
    an_acp_step_records_the_tokens_its_runtime_reported(Pipeline::Full).await;
}

#[tokio::test]
async fn a_partial_audit_step_on_opencode_records_its_tokens() {
    an_acp_step_records_the_tokens_its_runtime_reported(Pipeline::Partial).await;
}

/// DoD — a runtime that reports nothing gives an UNKNOWN figure, never 0.
async fn an_acp_step_whose_runtime_reports_nothing_is_unknown_not_zero(pipeline: Pipeline) {
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    let agent = ScriptedAgent::new(Turn::Answer(None));
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-silent", project.path()).await;

    let body = launch(&state, pipeline, "proj-silent", AgentType::OpenCode)
        .await
        .unwrap();

    let done = first_step_done(&body);
    assert!(done["tokens"].is_null(), "{pipeline:?}: {done}");
    assert!(done["total_tokens"].is_null(), "{pipeline:?}: {done}");
    let (_, steps) = latest_run_steps(&state, "proj-silent").await;
    let step = &steps[0];
    assert_eq!(step.step_tokens, None, "{pipeline:?}");
    assert_eq!(step.cumulative_tokens, None, "{pipeline:?}");
    assert_eq!(step.input_tokens, None, "{pipeline:?}");
    assert_eq!(step.output_tokens, None, "{pipeline:?}");
    assert_eq!(step.cache_read_tokens, None, "{pipeline:?}");
    assert_eq!(step.cache_write_tokens, None, "{pipeline:?}");
}

#[tokio::test]
async fn a_full_audit_step_whose_runtime_reports_nothing_is_unknown_not_zero() {
    an_acp_step_whose_runtime_reports_nothing_is_unknown_not_zero(Pipeline::Full).await;
}

#[tokio::test]
async fn a_partial_audit_step_whose_runtime_reports_nothing_is_unknown_not_zero() {
    an_acp_step_whose_runtime_reports_nothing_is_unknown_not_zero(Pipeline::Partial).await;
}

/// Puts a scripted `opencode` first on `PATH` for one test and restores the
/// environment afterwards, even when the test fails.
#[cfg(unix)]
struct ScriptedOpenCodeOnPath(Vec<(&'static str, Option<std::ffi::OsString>)>);

#[cfg(unix)]
impl ScriptedOpenCodeOnPath {
    fn install(bin: &Path, out: &Path) -> Self {
        let saved: Vec<_> = ["PATH", "OPENCODE_FIXTURE_OUT", "OPENCODE_CONFIG_CONTENT"]
            .into_iter()
            .map(|name| (name, crate::core::child_env::var_os(name)))
            .collect();
        let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(
            std::env::split_paths(&crate::core::child_env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        crate::core::child_env::set_var("PATH", path);
        crate::core::child_env::set_var("OPENCODE_FIXTURE_OUT", out);
        crate::core::child_env::remove_var("OPENCODE_CONFIG_CONTENT");
        Self(saved)
    }
}

#[cfg(unix)]
impl Drop for ScriptedOpenCodeOnPath {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            match value {
                Some(value) => crate::core::child_env::set_var(name, value),
                None => crate::core::child_env::remove_var(name),
            }
        }
    }
}

/// DoD — the partial audit starts OpenCode the way the Full one does: through
/// Kronn's own ACP spawn, so the agent gets the environment-file policy, a
/// refused read does not end its step, and the usage its turn reports is counted.
#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(acp_adapter_env_toggle)]
async fn a_partial_audit_on_opencode_gets_the_read_policy_and_survives_a_refused_read() {
    let bin = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    crate::acp::test_support::write_fake_opencode(bin.path());
    let _environment = ScriptedOpenCodeOnPath::install(bin.path(), out.path());
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    add_project(&state, "proj-policy", project.path()).await;

    let body = launch(
        &state,
        Pipeline::Partial,
        "proj-policy",
        AgentType::OpenCode,
    )
    .await
    .unwrap();

    let reply: Value =
        serde_json::from_str(&std::fs::read_to_string(out.path().join("reply.json")).unwrap())
            .unwrap();
    assert_eq!(
        reply["result"]["outcome"],
        json!({"outcome": "selected", "optionId": "reject"}),
        "the refused read is answered, not left hanging"
    );
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(out.path().join("config.json")).unwrap())
            .expect("OpenCode was started with Kronn's inline configuration");
    assert_eq!(config["permission"]["read"]["*.env"], "deny");
    assert_eq!(config["permission"]["read"]["*.env.dist"], "allow");
    assert_eq!(config["experimental"]["continue_loop_on_deny"], true);
    // The step ran on to its end: its text reached the stream and its usage was
    // counted — 100 in, 20 out, as OpenCode reports them.
    assert!(body.contains("carried on after the refusal"), "{body}");
    let done = first_step_done(&body);
    assert_eq!(done["tokens"], json!(120), "{done}");
}

async fn latest_run(state: &AppState, id: &str) -> crate::models::AuditRun {
    let id = id.to_string();
    state
        .db
        .with_conn(move |conn| Ok(crate::db::audit_runs::list_recent(conn, &id, 1)?.remove(0)))
        .await
        .unwrap()
}

/// KT-997 DoD — the cost the runtime reported is recorded on the step and sent
/// with `step_done`; the run names the model the runtime served.
async fn an_acp_step_records_its_reported_cost_and_model(pipeline: Pipeline) {
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    let agent = ScriptedAgent::new(Turn::Priced {
        usd_micros: Some(420_000),
        model: Some("scripted-model-é"),
    });
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-cost", project.path()).await;

    let body = launch(&state, pipeline, "proj-cost", AgentType::OpenCode)
        .await
        .unwrap();

    let done = first_step_done(&body);
    assert_eq!(
        done["cost_usd_micros"],
        json!(420_000),
        "{pipeline:?}: {done}"
    );
    let (_, steps) = latest_run_steps(&state, "proj-cost").await;
    assert_eq!(steps[0].cost_usd_micros, Some(420_000), "{pipeline:?}");
    assert_eq!(
        latest_run(&state, "proj-cost").await.model.as_deref(),
        Some("scripted-model-é"),
        "{pipeline:?}: the observed model, not a configured guess"
    );
}

#[tokio::test]
async fn a_full_audit_step_records_the_cost_and_model_its_runtime_reported() {
    an_acp_step_records_its_reported_cost_and_model(Pipeline::Full).await;
}

#[tokio::test]
async fn a_partial_audit_step_records_the_cost_and_model_its_runtime_reported() {
    an_acp_step_records_its_reported_cost_and_model(Pipeline::Partial).await;
}

/// KT-997 DoD — no reported cost stays unknown (NULL, `null`), never 0; with no
/// observed model the run carries the configured one, labelled as such.
async fn an_unreported_cost_stays_unknown(pipeline: Pipeline) {
    let state = new_state();
    state
        .config
        .write()
        .await
        .agents
        .model_tiers
        .open_code
        .reasoning = Some("cfg-model".into());
    let project = tempfile::tempdir().unwrap();
    let agent = ScriptedAgent::new(Turn::Priced {
        usd_micros: None,
        model: None,
    });
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-nocost", project.path()).await;

    let body = launch(&state, pipeline, "proj-nocost", AgentType::OpenCode)
        .await
        .unwrap();

    let done = first_step_done(&body);
    assert!(done["cost_usd_micros"].is_null(), "{pipeline:?}: {done}");
    assert_eq!(
        done["tokens"],
        json!(120),
        "{pipeline:?}: tokens are still known"
    );
    let (_, steps) = latest_run_steps(&state, "proj-nocost").await;
    assert_eq!(steps[0].cost_usd_micros, None, "{pipeline:?}");
    assert_eq!(
        latest_run(&state, "proj-nocost").await.model.as_deref(),
        Some("cfg-model (configured)"),
        "{pipeline:?}"
    );
}

#[tokio::test]
async fn a_full_audit_step_without_a_reported_cost_stays_unknown() {
    an_unreported_cost_stays_unknown(Pipeline::Full).await;
}

#[tokio::test]
async fn a_partial_audit_step_without_a_reported_cost_stays_unknown() {
    an_unreported_cost_stays_unknown(Pipeline::Partial).await;
}

/// KT-950 DoD — an ACP step that only calls tools, without a word of text,
/// shows its last tool, its call count and its tokens while it works, through
/// the same probe as an HTTP agent; and a Stop still ends it.
async fn a_tool_chain_without_text_shows_progress_and_stops(pipeline: Pipeline) {
    const CALLS: u32 = 40;
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    let agent = ScriptedAgent::new(Turn::ToolChain(CALLS));
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-tools", project.path()).await;
    let run = launch(&state, pipeline, "proj-tools", AgentType::OpenCode);

    let tracker = state.audit_tracker.clone();
    let progress = move || tracker.lock().unwrap().progress.get("proj-tools").cloned();
    until("the tool chain to show on the audit's progress", || {
        progress().is_some_and(|p| {
            p.current_tool_call_count == Some(CALLS)
                && p.step_tokens == Some(u64::from(CALLS) * 110)
                && p.recent_activity.as_ref().is_some_and(|recent| {
                    recent.entries.len() == crate::agents::activity::RECENT_MAX_ENTRIES
                })
        })
    })
    .await;
    let shown = progress().unwrap();
    // The chip and the details panel: categories only, bounded.
    assert_eq!(shown.current_tool.as_deref(), Some("Read"), "{pipeline:?}");
    let recent = shown
        .recent_activity
        .clone()
        .expect("the step's recent actions");
    assert!(recent
        .entries
        .iter()
        .all(|e| e.category == crate::models::ActivityCategory::Read));
    let progress_json = serde_json::to_string(&shown).unwrap();
    assert!(
        !progress_json.contains("SENTINEL") && !progress_json.contains("hunter2"),
        "{pipeline:?}: {progress_json}"
    );

    let response = crate::api::audit::full::cancel_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("proj-tools".to_string()),
    )
    .await;
    assert!(response.0.success, "{pipeline:?}: {:?}", response.0.error);
    let body = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .unwrap_or_else(|_| panic!("{pipeline:?}: the stopped audit never ended"))
        .unwrap();

    let events = sse_events(&body);
    let tool = events
        .iter()
        .rev()
        .find(|(name, _)| name == "tool_call")
        .unwrap_or_else(|| panic!("{pipeline:?}: no tool_call: {body}"));
    assert_eq!(tool.1["calls"], json!(CALLS), "{pipeline:?}");
    let activity = events
        .iter()
        .rev()
        .find(|(name, _)| name == "activity")
        .unwrap_or_else(|| panic!("{pipeline:?}: no activity: {body}"));
    assert_eq!(
        activity.1["recent"]["entries"][0],
        json!({"category": "Read", "at": activity.1["recent"]["entries"][0]["at"]}),
        "{pipeline:?}"
    );
    // Neither the `tool_call` nor the `activity` events, nor anything else in
    // the stream, carry the title or the raw input.
    assert!(
        !body.contains("SENTINEL") && !body.contains("hunter2"),
        "{pipeline:?}: {body}"
    );
    let progress_event = events
        .iter()
        .rev()
        .find(|(name, _)| name == "step_progress")
        .unwrap_or_else(|| panic!("{pipeline:?}: no step_progress: {body}"));
    assert_eq!(
        progress_event.1["step_tokens"],
        json!(u64::from(CALLS) * 110),
        "{pipeline:?}"
    );
    assert!(
        !events.iter().any(|(name, _)| name == "chunk"),
        "{pipeline:?}: activity never becomes text in the answer: {body}"
    );
    assert!(
        events.iter().any(|(name, _)| name == "cancelled"),
        "{pipeline:?}"
    );
    assert!(agent.cancels.load(Ordering::SeqCst) >= 1, "{pipeline:?}");
    let (status, _) = latest_run_steps(&state, "proj-tools").await;
    assert_eq!(status, "Cancelled", "{pipeline:?}");
}

#[tokio::test]
async fn a_full_audit_tool_chain_without_text_shows_progress_and_stops() {
    a_tool_chain_without_text_shows_progress_and_stops(Pipeline::Full).await;
}

#[tokio::test]
async fn a_partial_audit_tool_chain_without_text_shows_progress_and_stops() {
    a_tool_chain_without_text_shows_progress_and_stops(Pipeline::Partial).await;
}

/// An audit is no explicit choice: OpenCode without its own full-access
/// setting is refused before the audit starts, and OpenCode never starts.
#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(acp_adapter_env_toggle)]
async fn an_audit_on_opencode_without_its_full_access_setting_is_refused_before_spawn() {
    let bin = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    crate::acp::test_support::write_fake_opencode(bin.path());
    let _environment = ScriptedOpenCodeOnPath::install(bin.path(), out.path());
    let mut config = crate::core::config::default_config();
    config.agents.open_code.full_access = false;
    config.language = "en".into();
    let state = state_with(config);
    let project = tempfile::tempdir().unwrap();
    add_project(&state, "proj-refused", project.path()).await;
    for pipeline in [Pipeline::Full, Pipeline::Partial] {
        let body = launch(&state, pipeline, "proj-refused", AgentType::OpenCode)
            .await
            .unwrap();
        assert!(
            body.contains("Config › Agents › OpenCode › Full access"),
            "{body}"
        );
    }
    assert!(
        !out.path().join("config.json").exists(),
        "the scripted OpenCode was never started"
    );
}

/// Revoking full access between two steps of a running audit refuses the next
/// spawn: the launcher's own value, taken at the start, is not the authority.
#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(acp_adapter_env_toggle)]
async fn revoking_full_access_during_an_audit_refuses_its_next_step() {
    let bin = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    crate::acp::test_support::write_fake_opencode(bin.path());
    let _environment = ScriptedOpenCodeOnPath::install(bin.path(), out.path());
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    let launcher = AuditAgentLauncher::for_request(&state, &AgentType::OpenCode, None)
        .await
        .expect("full access is on when the audit starts");
    let tokens = state.config.read().await.tokens.clone();
    let path = project.path().to_string_lossy().into_owned();
    let start = || {
        launcher.start(
            &AgentType::OpenCode,
            crate::models::ModelTier::Default,
            project.path(),
            &path,
            "step",
            &tokens,
            None,
            "step-1",
            None,
        )
    };
    let mut first = start().await.expect("the first step starts");
    while first.next_line().await.is_some() {}
    let _revoked = crate::core::config::test_saved_access::set(&AgentType::OpenCode, false);
    let refused = start().await.err().expect("the next step is refused");
    assert!(
        refused.starts_with(crate::agents::runner::NATIVE_FULL_ACCESS_REQUIRED),
        "{refused}"
    );
}
