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

fn new_state() -> AppState {
    AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
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
        let saved: Vec<_> = ["PATH", "FIXTURE_OUT", "OPENCODE_CONFIG_CONTENT"]
            .into_iter()
            .map(|name| (name, std::env::var_os(name)))
            .collect();
        let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        std::env::set_var("PATH", path);
        std::env::set_var("FIXTURE_OUT", out);
        std::env::remove_var("OPENCODE_CONFIG_CONTENT");
        Self(saved)
    }
}

#[cfg(unix)]
impl Drop for ScriptedOpenCodeOnPath {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
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
    // counted — 100 in, 20 out.
    assert!(body.contains("carried on after the refusal"), "{body}");
    let done = first_step_done(&body);
    assert_eq!(done["tokens"], json!(120), "{done}");
}
