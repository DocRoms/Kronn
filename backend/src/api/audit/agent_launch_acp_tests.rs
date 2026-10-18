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
    /// Reports `REPORTED` usage and this cost, then works until stopped.
    PricedHang { usd_micros: u64 },
}

struct ScriptedAgent {
    turn: Turn,
    prompts: AtomicUsize,
    cancels: AtomicUsize,
    shutdowns: AtomicUsize,
    /// A file the agent writes as soon as it is prompted (KT-1211: the write
    /// a stopped step leaves behind).
    writes: std::sync::Mutex<Option<(std::path::PathBuf, String)>>,
    /// A directory the agent makes read-only right after its write: whatever
    /// settles the target afterwards can neither put it back nor mark it.
    locks: std::sync::Mutex<Option<std::path::PathBuf>>,
}

impl ScriptedAgent {
    fn new(turn: Turn) -> Arc<Self> {
        Arc::new(Self {
            turn,
            prompts: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            shutdowns: AtomicUsize::new(0),
            writes: std::sync::Mutex::new(None),
            locks: std::sync::Mutex::new(None),
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
        if let Some((path, content)) = self.writes.lock().unwrap().clone() {
            std::fs::write(path, content).unwrap();
        }
        #[cfg(unix)]
        if let Some(dir) = self.locks.lock().unwrap().clone() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
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
            Turn::PricedHang { usd_micros } => {
                let _ = events
                    .send(AcpSessionEvent::Usage {
                        input_tokens: REPORTED.input,
                        output_tokens: REPORTED.output,
                        prompt_cache: PromptCacheUsage::default(),
                    })
                    .await;
                let _ = events.send(AcpSessionEvent::Cost { usd_micros }).await;
                std::future::pending().await
            }
            Turn::ToolChain(calls) => {
                for call in 1..=u64::from(calls) {
                    let _ = events
                        .send(AcpSessionEvent::ToolCall {
                            id: Some(call.to_string()),
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
                    let _ = events
                        .send(AcpSessionEvent::ToolCallEnded {
                            id: Some(call.to_string()),
                        })
                        .await;
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

/// The project's `docs/` as an earlier audit left it: recorded and marked, so
/// the partial refresh is not held for the briefing's choice (KT-1299).
fn kronn_docs(project: &Path) {
    let docs = project.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(
        docs.join(".kronn.json"),
        r#"{"audits":[{"date":"2026-05-01","kronn_version":"0.8.4","type":"full"}]}"#,
    )
    .unwrap();
    std::fs::write(
        docs.join("audit-note.md"),
        "<!-- kronn:generated=2026-05-01 -->\n",
    )
    .unwrap();
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
    launch_with(state, pipeline, id, agent, false)
}

/// [`launch`], saying (or not) that the human accepts a process table Kronn
/// could not read (KT-1324).
fn launch_with(
    state: &AppState,
    pipeline: Pipeline,
    id: &str,
    agent: AgentType,
    accept_unverified_agents: bool,
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
                    retry_steps: None,
                    redo_steps: None,
                    accept_unverified_agents: Some(accept_unverified_agents),
                    fresh: None,
                }),
            )
            .await
            .into_response(),
            Pipeline::Partial => {
                let chain = crate::api::audit::assemble_chained_steps(AuditKind::Full);
                let step = chain
                    .iter()
                    .position(|step| {
                        crate::api::audit::partial_selectable(
                            step,
                            &crate::core::docs_location::DocsRoot::default(),
                        )
                    })
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
                        accept_unverified_agents: Some(accept_unverified_agents),
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
    until_within(20, what, condition).await;
}

/// [`until`] with a budget for a step an interpreter starts on a loaded machine.
async fn until_within(seconds: u64, what: &str, condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(seconds), async {
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

/// KT-1211 — a Stop in the middle of a step that has already written its
/// target never leaves that document half-written: it is put back as it was
/// before the step, and the step says so.
async fn a_stop_mid_write_puts_the_target_back(pipeline: Pipeline) {
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    let target = match pipeline {
        Pipeline::Full => {
            crate::api::audit::assemble_chained_steps(AuditKind::Docker)[0].target_file
        }
        Pipeline::Partial => {
            let chain = crate::api::audit::assemble_chained_steps(AuditKind::Full);
            chain
                .iter()
                .find(|step| {
                    crate::api::audit::partial_selectable(
                        step,
                        &crate::core::docs_location::DocsRoot::default(),
                    )
                })
                .expect("a refreshable section")
                .target_file
        }
    };
    let path = project.path().join(target);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    const BEFORE: &str = "# The doc before the step\n\nOriginal, whole content.\n";
    kronn_docs(project.path());
    std::fs::write(&path, BEFORE).unwrap();

    let agent = ScriptedAgent::new(Turn::Hang);
    *agent.writes.lock().unwrap() = Some((path.clone(), "# Half writ".to_string()));
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-half", project.path()).await;
    let run = launch(&state, pipeline, "proj-half", AgentType::OpenCode);
    until("the agent to be working on its step", || {
        agent.prompts.load(Ordering::SeqCst) > 0
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# Half writ",
        "{pipeline:?}: the agent has written its half"
    );

    let response = crate::api::audit::full::cancel_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("proj-half".to_string()),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .unwrap_or_else(|_| panic!("{pipeline:?}: the stopped audit never ended"))
        .unwrap();
    assert!(response.0.success, "{pipeline:?}: {:?}", response.0.error);

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        BEFORE,
        "{pipeline:?}: the killed step's write is undone"
    );
    let (status, steps) = latest_run_steps(&state, "proj-half").await;
    assert_eq!(status, "Cancelled", "{pipeline:?}");
    let stopped = steps
        .iter()
        .find(|row| row.target_state.is_some())
        .unwrap_or_else(|| panic!("{pipeline:?}: no step recorded what became of its target"));
    assert_eq!(stopped.target_state.as_deref(), Some("restored"));
    assert!(
        stopped.ended_at.is_some(),
        "{pipeline:?}: the step is settled, not dangling"
    );
    assert!(
        stopped
            .step_warning
            .as_deref()
            .unwrap_or("")
            .contains("stopped during step"),
        "{pipeline:?}: {:?}",
        stopped.step_warning
    );
}

#[tokio::test]
async fn a_stop_mid_write_of_a_full_audit_step_puts_its_target_back() {
    a_stop_mid_write_puts_the_target_back(Pipeline::Full).await;
}

#[tokio::test]
async fn a_stop_mid_write_of_a_partial_audit_step_puts_its_target_back() {
    a_stop_mid_write_puts_the_target_back(Pipeline::Partial).await;
}

/// The directory a test made read-only, writable again when it ends (so the
/// temp project can be removed even when an assertion panics).
#[cfg(unix)]
struct Unlock(std::path::PathBuf);

#[cfg(unix)]
impl Drop for Unlock {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

/// The chain target a pipeline's first step writes.
fn first_target(pipeline: Pipeline) -> &'static str {
    match pipeline {
        Pipeline::Full => {
            crate::api::audit::assemble_chained_steps(AuditKind::Docker)[0].target_file
        }
        Pipeline::Partial => {
            let chain = crate::api::audit::assemble_chained_steps(AuditKind::Full);
            chain
                .iter()
                .find(|step| {
                    crate::api::audit::partial_selectable(
                        step,
                        &crate::core::docs_location::DocsRoot::default(),
                    )
                })
                .expect("a refreshable section")
                .target_file
        }
    }
}

/// Waits until the audit has recorded what the agent wrote to the target of
/// the step in flight (what a crash can later vouch for).
async fn until_observed(state: &AppState, project: &str, step_index: u32, expected: &str) {
    let want = crate::api::audit::step_guard::fingerprint(Some(expected));
    let run_id = latest_run(state, project).await.id;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let run_id = run_id.clone();
            let seen = state
                .db
                .with_conn(move |conn| {
                    crate::db::audit_runs::step_snapshot(conn, &run_id, step_index)
                })
                .await
                .unwrap()
                .and_then(|snapshot| snapshot.observed_hash);
            if seen.as_deref() == Some(want.as_str()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the audit never recorded what the agent wrote");
}

/// KT-1211 — the restore AND the partial marker both fail (the directory
/// became read-only): the step is never recorded as `restored`, and accepting
/// it asks for an override. This is the guard against the pipeline forcing
/// `restored` onto a target that is in an unknown state.
#[cfg(unix)]
async fn a_stop_that_cannot_settle_the_target_is_not_recorded_as_restored(pipeline: Pipeline) {
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join(first_target(pipeline));
    let dir = path.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    const BEFORE: &str = "# The doc before the step\n\nOriginal, whole content.\n";
    kronn_docs(project.path());
    std::fs::write(&path, BEFORE).unwrap();

    let agent = ScriptedAgent::new(Turn::Hang);
    *agent.writes.lock().unwrap() = Some((path.clone(), "# Half writ".to_string()));
    *agent.locks.lock().unwrap() = Some(dir.clone());
    let _unlock = Unlock(dir.clone());
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-stuck", project.path()).await;
    let run = launch(&state, pipeline, "proj-stuck", AgentType::OpenCode);
    until("the agent to be working on its step", || {
        agent.prompts.load(Ordering::SeqCst) > 0
    })
    .await;

    let response = crate::api::audit::full::cancel_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("proj-stuck".to_string()),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .unwrap_or_else(|_| panic!("{pipeline:?}: the stopped audit never ended"))
        .unwrap();
    assert!(response.0.success, "{pipeline:?}: {:?}", response.0.error);

    // Nothing could be put back: the half document is still there.
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# Half writ",
        "{pipeline:?}"
    );
    let (_, steps) = latest_run_steps(&state, "proj-stuck").await;
    let stuck = steps
        .iter()
        .find(|row| {
            row.step_warning
                .as_deref()
                .is_some_and(|reason| reason.contains("stopped during step"))
        })
        .unwrap_or_else(|| panic!("{pipeline:?}: no stopped step: {steps:?}"));
    assert_eq!(
        stuck.target_state, None,
        "{pipeline:?}: a target that could be neither restored nor marked is not `restored`"
    );
    assert!(
        stuck
            .step_warning
            .as_deref()
            .unwrap()
            .contains("neither be put back nor marked"),
        "{pipeline:?}: {:?}",
        stuck.step_warning
    );
    assert!(stuck.ended_at.is_some());
    assert_eq!(
        crate::api::audit::accept::accept_blocker_of(stuck),
        Some(crate::api::audit::step_guard::AcceptBlocker::Killed),
        "{pipeline:?}: accepting it needs the override"
    );
    // Its snapshot is kept: the acceptance diff is built from it.
    let run_id = latest_run(&state, "proj-stuck").await.id;
    let step_index = stuck.step_index;
    let snapshot = state
        .db
        .with_conn(move |conn| crate::db::audit_runs::step_snapshot(conn, &run_id, step_index))
        .await
        .unwrap();
    assert_eq!(
        snapshot.and_then(|s| s.content).as_deref(),
        Some(BEFORE),
        "{pipeline:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_stop_that_cannot_settle_a_full_audit_target_is_not_recorded_as_restored() {
    a_stop_that_cannot_settle_the_target_is_not_recorded_as_restored(Pipeline::Full).await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_stop_that_cannot_settle_a_partial_audit_target_is_not_recorded_as_restored() {
    a_stop_that_cannot_settle_the_target_is_not_recorded_as_restored(Pipeline::Partial).await;
}

/// KT-1211 — the user edits the doc by hand between the crash and the next
/// boot: boot never overwrites that edit; it flags the file and keeps the
/// pre-step snapshot.
async fn a_hand_edit_between_a_crash_and_boot_survives(pipeline: Pipeline) {
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join(first_target(pipeline));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    const BEFORE: &str = "# The doc before the step\n\nOriginal, whole content.\n";
    kronn_docs(project.path());
    std::fs::write(&path, BEFORE).unwrap();

    let agent = ScriptedAgent::new(Turn::Hang);
    *agent.writes.lock().unwrap() = Some((path.clone(), "# Half writ".to_string()));
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-hand", project.path()).await;
    let run = launch(&state, pipeline, "proj-hand", AgentType::OpenCode);
    until("the agent to be working on its step", || {
        agent.prompts.load(Ordering::SeqCst) > 0
    })
    .await;
    let (_, open) = latest_run_steps(&state, "proj-hand").await;
    let step_index = open
        .iter()
        .find(|row| row.ended_at.is_none())
        .expect("the step in flight")
        .step_index;
    // The audit saw the agent's write; then the backend died, and the user
    // rewrote the doc by hand.
    until_observed(&state, "proj-hand", step_index, "# Half writ").await;
    const EDIT: &str = "# Mine\n\nA careful hand edit made after the crash.\n";
    std::fs::write(&path, EDIT).unwrap();

    let settled = state
        .db
        .with_conn(crate::api::audit::step_guard::settle_unfinished_at_boot)
        .await
        .unwrap();
    assert_eq!(settled, 1, "{pipeline:?}");
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("A careful hand edit made after the crash."),
        "{pipeline:?}: {after}"
    );
    assert!(
        crate::api::audit::step_guard::has_partial_marker(&after),
        "{pipeline:?}: {after}"
    );
    let (_, steps) = latest_run_steps(&state, "proj-hand").await;
    let row = steps
        .iter()
        .find(|row| row.step_index == step_index)
        .unwrap();
    assert_eq!(row.target_state.as_deref(), Some("partial"), "{pipeline:?}");
    let run_id = latest_run(&state, "proj-hand").await.id;
    let snapshot = state
        .db
        .with_conn(move |conn| crate::db::audit_runs::step_snapshot(conn, &run_id, step_index))
        .await
        .unwrap();
    assert_eq!(
        snapshot.and_then(|s| s.content).as_deref(),
        Some(BEFORE),
        "{pipeline:?}"
    );

    let _ = crate::api::audit::full::cancel_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("proj-hand".to_string()),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(10), run).await;
}

#[tokio::test]
async fn a_hand_edit_between_a_crash_and_boot_of_a_full_audit_survives() {
    a_hand_edit_between_a_crash_and_boot_survives(Pipeline::Full).await;
}

#[tokio::test]
async fn a_hand_edit_between_a_crash_and_boot_of_a_partial_audit_survives() {
    a_hand_edit_between_a_crash_and_boot_survives(Pipeline::Partial).await;
}

/// KT-1211 — the backend dies while a step is rewriting its target: nothing in
/// memory survives, so what boot sees is exactly the database as it stands
/// during the step. Boot puts the doc back and gives the step a verdict,
/// without waiting for the next launch.
async fn a_backend_crash_mid_step_is_settled_at_boot(pipeline: Pipeline) {
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    let target = match pipeline {
        Pipeline::Full => {
            crate::api::audit::assemble_chained_steps(AuditKind::Docker)[0].target_file
        }
        Pipeline::Partial => {
            let chain = crate::api::audit::assemble_chained_steps(AuditKind::Full);
            chain
                .iter()
                .find(|step| {
                    crate::api::audit::partial_selectable(
                        step,
                        &crate::core::docs_location::DocsRoot::default(),
                    )
                })
                .expect("a refreshable section")
                .target_file
        }
    };
    let path = project.path().join(target);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    const BEFORE: &str = "# The doc before the step\n\nOriginal, whole content.\n";
    kronn_docs(project.path());
    std::fs::write(&path, BEFORE).unwrap();

    let agent = ScriptedAgent::new(Turn::Hang);
    *agent.writes.lock().unwrap() = Some((path.clone(), "# Half writ".to_string()));
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-crash", project.path()).await;
    let run = launch(&state, pipeline, "proj-crash", AgentType::OpenCode);
    until("the agent to be working on its step", || {
        agent.prompts.load(Ordering::SeqCst) > 0
    })
    .await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Half writ");
    let (_, open) = latest_run_steps(&state, "proj-crash").await;
    let open = open
        .iter()
        .find(|row| row.ended_at.is_none())
        .expect("the step in flight");
    let step_index = open.step_index;
    // Boot puts back only what the audit saw the agent write.
    until_observed(&state, "proj-crash", step_index, "# Half writ").await;
    let run_id = latest_run(&state, "proj-crash").await.id;
    // The pre-step content is durable: that is what survives the crash.
    let snapshot = {
        let run_id = run_id.clone();
        state
            .db
            .with_conn(move |conn| crate::db::audit_runs::step_snapshot(conn, &run_id, step_index))
            .await
            .unwrap()
    };
    assert_eq!(
        snapshot.and_then(|s| s.content).as_deref(),
        Some(BEFORE),
        "{pipeline:?}: the pre-step content is persisted, not only in memory"
    );

    // Boot.
    let settled = state
        .db
        .with_conn(crate::api::audit::step_guard::settle_unfinished_at_boot)
        .await
        .unwrap();
    assert_eq!(settled, 1, "{pipeline:?}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        BEFORE,
        "{pipeline:?}: the half-written doc is put back at boot"
    );
    let (_, steps) = latest_run_steps(&state, "proj-crash").await;
    let row = steps
        .iter()
        .find(|row| row.step_index == step_index)
        .unwrap();
    assert!(
        row.ended_at.is_some(),
        "{pipeline:?}: the step has a verdict"
    );
    assert_eq!(
        row.target_state.as_deref(),
        Some("restored"),
        "{pipeline:?}"
    );
    assert!(
        row.step_warning
            .as_deref()
            .unwrap_or("")
            .contains("stopped during step"),
        "{pipeline:?}: {:?}",
        row.step_warning
    );

    let _ = crate::api::audit::full::cancel_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("proj-crash".to_string()),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(10), run).await;
}

#[tokio::test]
async fn a_backend_crash_mid_step_of_a_full_audit_is_settled_at_boot() {
    a_backend_crash_mid_step_is_settled_at_boot(Pipeline::Full).await;
}

#[tokio::test]
async fn a_backend_crash_mid_step_of_a_partial_audit_is_settled_at_boot() {
    a_backend_crash_mid_step_is_settled_at_boot(Pipeline::Partial).await;
}

/// KT-1324 — two runs of the project that no boot settled: the older one is
/// still `Running` with nothing pending (its agent may be alive), the newer one
/// holds a step that never ended and a half-written doc. A launch that looked
/// only at the newest run would miss the older one's agent. Returns the ids of
/// the run to reap, of the run to settle, and the doc.
async fn leave_runs_open(
    state: &AppState,
    project_id: &str,
    root: &Path,
    pipeline: Pipeline,
    before: &str,
) -> (String, String, std::path::PathBuf) {
    let target = first_target(pipeline);
    let path = root.join(target);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "# Half writ").unwrap();
    let (reaped, settled) = (
        format!("older-{}", uuid::Uuid::new_v4()),
        format!("newer-{}", uuid::Uuid::new_v4()),
    );
    let (older, newer, pid) = (reaped.clone(), settled.clone(), project_id.to_string());
    let (before, target) = (before.to_string(), target.to_string());
    state
        .db
        .with_conn(move |conn| {
            use crate::db::audit_runs as runs;
            let label = AuditKind::Docker.as_label();
            let now = chrono::Utc::now();
            let ago = |minutes| now - chrono::Duration::minutes(minutes);
            runs::insert_running(conn, &older, &pid, label, "OpenCode", ago(10))?;
            // Inserting a run drops the snapshots of the runs it supersedes:
            // the step of the newer run is recorded after it.
            runs::insert_running(conn, &newer, &pid, label, "OpenCode", ago(5))?;
            runs::insert_audit_step_start(conn, &newer, 1, &target, now)?;
            runs::save_step_snapshot(conn, &newer, 1, &target, Some(&before))?;
            runs::record_observed_output(
                conn,
                &newer,
                1,
                &crate::api::audit::step_guard::fingerprint(Some("# Half writ")),
            )?;
            Ok(())
        })
        .await
        .unwrap();
    (reaped, settled, path)
}

async fn run_is_settled(state: &AppState, run_id: &str) -> bool {
    let run_id = run_id.to_string();
    state
        .db
        .with_conn(move |conn| {
            Ok(crate::api::audit::step_guard::pending_steps(conn, &run_id)?.is_empty())
        })
        .await
        .unwrap()
}

fn error_of(body: &str) -> String {
    sse_events(body)
        .into_iter()
        .find(|(name, _)| name == "error")
        .map(|(_, data)| data["error"].as_str().unwrap_or_default().to_string())
        .unwrap_or_else(|| panic!("the launch was not refused: {body}"))
}

/// The typed `error_code` of a refused launch's `error` event, if it carries one.
fn error_code_of(body: &str) -> Option<String> {
    sse_events(body)
        .into_iter()
        .find(|(name, _)| name == "error")
        .and_then(|(_, data)| data["error_code"].as_str().map(str::to_string))
}

async fn stop_and_wait(state: &AppState, id: &str, run: tokio::task::JoinHandle<String>) {
    let _ = crate::api::audit::full::cancel_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path(id.to_string()),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(10), run).await;
}

/// KT-1324 — a launch settles the open runs of its project only once the agents
/// of every one of them are gone, not just the newest run's: a survivor of the
/// older run refuses the launch, naming the process and how to stop it, leaving
/// the doc and the step as they were and recording no run; once the agent is
/// gone the same launch settles the newer run's step, then runs.
async fn a_launch_settles_the_open_runs_only_once_their_agents_are_gone(pipeline: Pipeline) {
    use crate::api::audit::step_guard::{plant_reap_of_run, unplant_reap_of_run};
    use crate::core::dispatch_reaper::ReapReport;
    const BEFORE: &str = "# The doc before the step\n\nOriginal, whole content.\n";
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    kronn_docs(project.path());
    let agent = ScriptedAgent::new(Turn::Hang);
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-open", project.path()).await;
    let (reaped, settled, path) =
        leave_runs_open(&state, "proj-open", project.path(), pipeline, BEFORE).await;

    plant_reap_of_run(
        &reaped,
        ReapReport {
            survivors: vec![4242],
            ..Default::default()
        },
    );
    let body = launch(&state, pipeline, "proj-open", AgentType::OpenCode)
        .await
        .unwrap();
    let error = error_of(&body);
    assert!(error.contains("kill -9 4242"), "{pipeline:?}: {error}");
    assert_eq!(
        error_code_of(&body).as_deref(),
        Some("agents_still_running"),
        "{pipeline:?}: {body}"
    );
    assert!(error.contains(&reaped), "{pipeline:?}: {error}");
    assert!(!error.contains("ReapReport"), "{pipeline:?}: {error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Half writ");
    assert!(!run_is_settled(&state, &settled).await, "{pipeline:?}");
    assert_eq!(agent.prompts.load(Ordering::SeqCst), 0, "no agent started");
    let runs = state
        .db
        .with_conn(|conn| crate::db::audit_runs::list_recent(conn, "proj-open", 10))
        .await
        .unwrap();
    assert_eq!(
        runs.len(),
        2,
        "{pipeline:?}: a refused launch records no run"
    );

    // The agent is gone: the same launch settles the open step, then runs.
    unplant_reap_of_run(&reaped);
    let run = launch(&state, pipeline, "proj-open", AgentType::OpenCode);
    until("the agent to be working on its step", || {
        agent.prompts.load(Ordering::SeqCst) > 0
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        BEFORE,
        "{pipeline:?}: the half-written doc was put back before the new run took its snapshot"
    );
    assert!(run_is_settled(&state, &settled).await, "{pipeline:?}");
    stop_and_wait(&state, "proj-open", run).await;
}

#[tokio::test]
async fn a_full_launch_settles_the_open_runs_only_once_their_agents_are_gone() {
    a_launch_settles_the_open_runs_only_once_their_agents_are_gone(Pipeline::Full).await;
}

#[tokio::test]
async fn a_partial_launch_settles_the_open_runs_only_once_their_agents_are_gone() {
    a_launch_settles_the_open_runs_only_once_their_agents_are_gone(Pipeline::Partial).await;
}

/// KT-1324 — a process table nobody can read is no reason to block a project's
/// audits forever: the refusal says so in plain words, and the human's explicit
/// word on the launch settles the run.
async fn an_unreadable_process_table_can_be_accepted_by_the_human(pipeline: Pipeline) {
    use crate::api::audit::step_guard::{plant_reap_of_run, unplant_reap_of_run};
    use crate::core::dispatch_reaper::ReapReport;
    const BEFORE: &str = "# The doc before the step\n\nOriginal, whole content.\n";
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    kronn_docs(project.path());
    let agent = ScriptedAgent::new(Turn::Hang);
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-blind", project.path()).await;
    let (reaped, settled, path) =
        leave_runs_open(&state, "proj-blind", project.path(), pipeline, BEFORE).await;
    plant_reap_of_run(
        &reaped,
        ReapReport {
            unverified: true,
            ..Default::default()
        },
    );

    let body = launch(&state, pipeline, "proj-blind", AgentType::OpenCode)
        .await
        .unwrap();
    let error = error_of(&body);
    assert!(
        error.contains("could not read the process table"),
        "{pipeline:?}: {error}"
    );
    assert!(
        error.contains("accept_unverified_agents"),
        "{pipeline:?}: {error}"
    );
    assert!(!error.contains("ReapReport"), "{pipeline:?}: {error}");
    assert_eq!(
        error_code_of(&body).as_deref(),
        Some("agents_unverified"),
        "{pipeline:?}: {body}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Half writ");

    let run = launch_with(&state, pipeline, "proj-blind", AgentType::OpenCode, true);
    until("the agent to be working on its step", || {
        agent.prompts.load(Ordering::SeqCst) > 0
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        BEFORE,
        "{pipeline:?}"
    );
    assert!(run_is_settled(&state, &settled).await, "{pipeline:?}");
    unplant_reap_of_run(&reaped);
    stop_and_wait(&state, "proj-blind", run).await;
}

#[tokio::test]
async fn an_unreadable_process_table_can_be_accepted_by_the_human_on_a_full_launch() {
    an_unreadable_process_table_can_be_accepted_by_the_human(Pipeline::Full).await;
}

#[tokio::test]
async fn an_unreadable_process_table_can_be_accepted_by_the_human_on_a_partial_launch() {
    an_unreadable_process_table_can_be_accepted_by_the_human(Pipeline::Partial).await;
}

/// Puts a scripted `claude` first on `PATH` and forces the direct-CLI route
/// for one test, and restores the environment afterwards.
#[cfg(unix)]
struct ScriptedDirectClaude(Vec<(&'static str, Option<std::ffi::OsString>)>);

#[cfg(unix)]
impl ScriptedDirectClaude {
    fn install(bin: &Path) -> Self {
        let saved: Vec<_> = ["PATH", "KRONN_ACP_ADAPTER_CLAUDE"]
            .into_iter()
            .map(|name| (name, crate::core::child_env::var_os(name)))
            .collect();
        let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(
            std::env::split_paths(&crate::core::child_env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        crate::core::child_env::set_var("PATH", path);
        crate::core::child_env::set_var("KRONN_ACP_ADAPTER_CLAUDE", "0");
        Self(saved)
    }
}

#[cfg(unix)]
impl Drop for ScriptedDirectClaude {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            match value {
                Some(value) => crate::core::child_env::set_var(name, value),
                None => crate::core::child_env::remove_var(name),
            }
        }
    }
}

/// KT-1211 (M5) — a step run by a direct CLI agent records the agent's process
/// group and start stamp on the run while it works (so a boot after a crash can
/// kill it), and clears them once it is reaped. Both pipelines.
#[cfg(unix)]
async fn a_direct_cli_step_records_and_clears_its_process_group(pipeline: Pipeline) {
    let bin = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let script = bin.path().join("claude");
    std::fs::write(&script, "#!/bin/sh\nexec sleep 60\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _environment = ScriptedDirectClaude::install(bin.path());
    let state = new_state();
    add_project(&state, "proj-pgid", project.path()).await;
    let run = launch(&state, pipeline, "proj-pgid", AgentType::ClaudeCode);

    let recorded = || {
        state
            .db
            .with_conn(crate::db::audit_runs::recorded_agent_processes)
    };
    let mut found = Vec::new();
    for _ in 0..500 {
        found = recorded().await.unwrap();
        if !found.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        found.len(),
        1,
        "{pipeline:?}: the agent's process group is recorded while it works"
    );
    let (run_id, pgid, stamp) = found.remove(0);
    assert!(pgid > 1, "{pipeline:?}");
    assert!(
        stamp.is_some() && stamp == crate::core::agent_orphans::start_stamp(pgid as u32),
        "{pipeline:?}: the leader's start stamp is the live process's: {stamp:?}"
    );
    assert_eq!(run_id, latest_run(&state, "proj-pgid").await.id);

    let _ = crate::api::audit::full::cancel_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("proj-pgid".to_string()),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .expect("the audit ends");
    assert!(
        recorded().await.unwrap().is_empty(),
        "{pipeline:?}: a reaped agent leaves nothing for boot to kill"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(acp_adapter_env_toggle)]
async fn a_direct_cli_step_of_a_full_audit_records_and_clears_its_process_group() {
    a_direct_cli_step_records_and_clears_its_process_group(Pipeline::Full).await;
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(acp_adapter_env_toggle)]
async fn a_direct_cli_step_of_a_partial_audit_records_and_clears_its_process_group() {
    a_direct_cli_step_records_and_clears_its_process_group(Pipeline::Partial).await;
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

/// KT-1313 DoD — a step stopped mid-way is recorded with the tokens and the cost
/// it consumed, and the run's token counter never goes down.
async fn a_stopped_step_records_the_tokens_and_cost_it_consumed(pipeline: Pipeline) {
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    let agent = ScriptedAgent::new(Turn::PricedHang {
        usd_micros: 420_000,
    });
    let _route = route(project.path(), &agent);
    add_project(&state, "proj-spent", project.path()).await;
    let run = launch(&state, pipeline, "proj-spent", AgentType::OpenCode);

    let tracker = state.audit_tracker.clone();
    until("the step's tokens to show on the audit's progress", || {
        tracker
            .lock()
            .unwrap()
            .progress
            .get("proj-spent")
            .is_some_and(|p| p.step_tokens == Some(120))
    })
    .await;
    let response = crate::api::audit::full::cancel_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("proj-spent".to_string()),
    )
    .await;
    assert!(response.0.success, "{pipeline:?}: {:?}", response.0.error);
    let body = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .unwrap_or_else(|_| panic!("{pipeline:?}: the stopped audit never ended"))
        .unwrap();

    let (status, steps) = latest_run_steps(&state, "proj-spent").await;
    assert_eq!(status, "Cancelled", "{pipeline:?}");
    let stopped = steps.first().expect("the stopped step is recorded");
    assert_eq!(stopped.step_tokens, Some(120), "{pipeline:?}: {body}");
    assert_eq!(
        stopped.cumulative_tokens,
        Some(120),
        "{pipeline:?}: the run's tally holds the stopped step"
    );
    assert_eq!(
        stopped.cost_usd_micros,
        Some(420_000),
        "{pipeline:?}: the reported cost of what it consumed"
    );
    let shown: Vec<u64> = sse_events(&body)
        .iter()
        .filter(|(name, _)| name == "step_progress")
        .filter_map(|(_, data)| data["total_tokens_so_far"].as_u64())
        .collect();
    assert!(
        shown.windows(2).all(|pair| pair[0] <= pair[1]),
        "{pipeline:?}: the counter went down: {shown:?}"
    );
}

#[tokio::test]
async fn a_stopped_full_audit_step_records_the_tokens_and_cost_it_consumed() {
    a_stopped_step_records_the_tokens_and_cost_it_consumed(Pipeline::Full).await;
}

#[tokio::test]
async fn a_stopped_partial_audit_step_records_the_tokens_and_cost_it_consumed() {
    a_stopped_step_records_the_tokens_and_cost_it_consumed(Pipeline::Partial).await;
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

/// KT-1324 — a direct CLI agent carries its run's tag, and what a boot after a
/// backend stop finds by that tag alone (the pid is no longer recorded, as for
/// an adapter agent) is stopped: the agent's own descendants included, which
/// kept writing in the incident that founded this. Both pipelines.
#[cfg(unix)]
async fn a_boot_stops_the_tagged_agent_a_dead_backend_left_writing(pipeline: Pipeline) {
    // Python rather than `sh`: macOS shows no environment for a system binary,
    // and the tag is read from it.
    // Its absolute path goes in the shebang: the agent's own PATH is not ours.
    let Some(python) = ["python3", "python"].into_iter().find_map(|name| {
        let output = crate::core::cmd::sync_cmd(name, crate::core::child_env::ChildRoute::Tool)
            .args(["-c", "import sys; print(sys.executable)"])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!path.is_empty()).then_some(path)
    }) else {
        eprintln!("skipped: no python to script an agent with");
        return;
    };
    let bin = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let tag_file = out.path().join("tag");
    let canary = out.path().join("canary");
    // The agent reports the tag it was given and forks a descendant that
    // keeps writing (bounded, so a failing test leaves nothing running for
    // long) while it waits.
    let script = format!(
        "#!{python}\n\
         import os, time\n\
         open('{tag}', 'w').write(os.environ.get('KRONN_TASK_DISPATCH_ID', ''))\n\
         if os.fork() == 0:\n\
         \x20   for _ in range(1500):\n\
         \x20       open('{canary}', 'a').write('w\\n')\n\
         \x20       time.sleep(0.02)\n\
         \x20   os._exit(0)\n\
         time.sleep(60)\n",
        python = python,
        tag = tag_file.display(),
        canary = canary.display(),
    );
    let claude = bin.path().join("claude");
    std::fs::write(&claude, script).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _environment = ScriptedDirectClaude::install(bin.path());
    let state = new_state();
    add_project(&state, "proj-tag", project.path()).await;
    let run = launch(&state, pipeline, "proj-tag", AgentType::ClaudeCode);

    until_within(90, "the agent to report its tag", || {
        std::fs::read_to_string(&tag_file).is_ok_and(|tag| !tag.is_empty())
    })
    .await;
    let run_id = latest_run(&state, "proj-tag").await.id;
    assert_eq!(
        std::fs::read_to_string(&tag_file).unwrap(),
        crate::core::dispatch_reaper::audit_run_tag(&run_id),
        "{pipeline:?}: the agent's environment names its run"
    );
    until_within(90, "the agent's descendant to be writing", || {
        std::fs::metadata(&canary).is_ok_and(|meta| meta.len() > 0)
    })
    .await;

    // The backend stops; the new one finds no recorded group for this agent.
    let cleared = run_id.clone();
    state
        .db
        .with_conn(move |conn| crate::db::audit_runs::clear_agent_process(conn, &cleared))
        .await
        .unwrap();
    state
        .db
        .with_conn(crate::api::audit::step_guard::stop_agents_and_settle_at_boot)
        .await
        .unwrap();

    let written = std::fs::metadata(&canary).unwrap().len();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        std::fs::metadata(&canary).unwrap().len(),
        written,
        "{pipeline:?}: nothing of the run writes once boot has settled it"
    );

    let _ = crate::api::audit::full::cancel_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("proj-tag".to_string()),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .expect("the audit ends");
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(acp_adapter_env_toggle)]
async fn a_boot_stops_the_tagged_agent_of_a_full_audit_a_dead_backend_left_writing() {
    a_boot_stops_the_tagged_agent_a_dead_backend_left_writing(Pipeline::Full).await;
}

#[cfg(unix)]
#[tokio::test]
#[serial_test::serial(acp_adapter_env_toggle)]
async fn a_boot_stops_the_tagged_agent_of_a_partial_audit_a_dead_backend_left_writing() {
    a_boot_stops_the_tagged_agent_a_dead_backend_left_writing(Pipeline::Partial).await;
}

/// KT-1324 — a resume with no source change says nothing about the sources:
/// the run it continues was fingerprinted before Kronn's own install wrote its
/// root `AGENTS.md` pointer, the resume after it, and the two prints differed.
#[tokio::test]
async fn a_resume_with_no_source_change_does_not_warn_that_the_sources_changed() {
    use axum::response::IntoResponse;
    let git = |dir: &Path, args: &[&str]| {
        let ok = crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs")
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    };
    let state = new_state();
    let project = tempfile::tempdir().unwrap();
    git(project.path(), &["init", "-q"]);
    git(project.path(), &["config", "user.email", "t@t"]);
    git(project.path(), &["config", "user.name", "t"]);
    std::fs::write(project.path().join("app.js"), "console.log(1)").unwrap();
    git(project.path(), &["add", "-A"]);
    git(project.path(), &["commit", "-qm", "src"]);
    add_project(&state, "proj-fp", project.path()).await;

    // The first run: Kronn installs its files, the agent hangs, the run is
    // cut short.
    let hanging = ScriptedAgent::new(Turn::Hang);
    let first_route = route(project.path(), &hanging);
    let first = launch(&state, Pipeline::Full, "proj-fp", AgentType::OpenCode);
    until("the agent to be working on its step", || {
        hanging.prompts.load(Ordering::SeqCst) > 0
    })
    .await;
    // The backend stops: its run stays `Running` on disk until the next boot
    // flips it, and the next backend holds none of the old one's leases. The
    // old pipeline is left hanging, as a dead process's would be.
    let _stopped_backend = first;
    drop(first_route);
    state
        .db
        .with_conn(|conn| crate::db::audit_runs::reconcile_stale_runs(conn, 0))
        .await
        .unwrap();
    let interrupted = latest_run(&state, "proj-fp").await;
    assert_eq!(interrupted.status, "Interrupted");
    let state = AppState::new_defaults(
        state.config.clone(),
        state.db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let recorded = interrupted
        .source_fingerprint
        .clone()
        .expect("the run recorded the sources it audited");
    let now = tokio::task::spawn_blocking({
        let path = project.path().to_path_buf();
        move || crate::core::checksums::stable_source_tree_fingerprint(&path)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        now.as_deref(),
        Some(recorded.as_str()),
        "the sources did not move: the print the run recorded is the one they have now"
    );

    // The resume: same sources.
    let answering = ScriptedAgent::new(Turn::Answer(None));
    let _second_route = route(project.path(), &answering);
    let response = crate::api::audit::full::full_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("proj-fp".to_string()),
        axum::Json(LaunchAuditRequest {
            agent: AgentType::OpenCode,
            connection_id: None,
            tier: None,
            kind: None,
            custom_prompt: None,
            resume_run_id: Some(interrupted.id.clone()),
            retry_steps: None,
            redo_steps: None,
            accept_unverified_agents: None,
            fresh: None,
        }),
    )
    .await
    .into_response();
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(response.into_body(), 1 << 22)
            .await
            .unwrap(),
    )
    .into_owned();
    assert!(
        !body.contains("event: error"),
        "the resume must run: {body}"
    );
    let warnings: Vec<Value> = sse_events(&body)
        .into_iter()
        .filter(|(name, _)| name == "warning")
        .map(|(_, data)| data)
        .collect();
    assert!(
        warnings
            .iter()
            .all(|warning| !warning.to_string().contains("sources changed")),
        "no source moved: {warnings:?}"
    );
    let resumed = latest_run(&state, "proj-fp").await;
    assert_ne!(resumed.id, interrupted.id);
    assert_eq!(resumed.resume_drift, None);
}
