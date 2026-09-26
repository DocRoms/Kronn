//! KT-793 — a workflow Agent step with a `room_id` is the principal of that
//! room without an invite token, at launch and again after `/resume`. Real
//! runner, real process boundary (a fake CLI that records its environment),
//! real HTTP handlers, a real database reopened as a restarted backend would.
#![cfg(unix)]

use std::{ffi::OsString, os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration};

use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

use kronn::db::Database;
use kronn::models::{
    RunStatus, StepType, Workflow, WorkflowRun, WorkflowSafety, WorkflowStep, WorkflowTrigger,
};
use kronn::AppState;

struct Environment(Vec<(&'static str, Option<OsString>)>);
impl Environment {
    fn set(&mut self, name: &'static str, value: impl AsRef<std::ffi::OsStr>) {
        if !self.0.iter().any(|(key, _)| *key == name) {
            self.0.push((name, std::env::var_os(name)));
        }
        // This binary holds one current-thread test: nothing else reads the
        // environment it changes.
        unsafe { std::env::set_var(name, value) };
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (name, value) in self.0.iter().rev() {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

const ROOM: &str = "room-793";
const TASK: &str = "task-793";
const RUN: &str = "run-793";

/// A `claude` that records the capability Kronn gave it, then holds its turn
/// open until the test releases it, like an orchestrator waiting on a worker.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
if [ "$1" = auth ]; then
  printf '%s\n' '{"loggedIn":true}'
  exit 0
fi
case " $* " in
  *" --print "*) ;;
  *) exit 0 ;;
esac
cat >/dev/null
n=$(ls "$KT793_DIR" | grep -c '^launch-')
printf '%s' "$KRONN_WORKFLOW_STEP_CONTEXT" > "$KT793_DIR/context-$n"
printf '%s' "$KRONN_DISCUSSION_ID" > "$KT793_DIR/discussion-$n"
printf '%s\n' "$@" > "$KT793_DIR/argv-$n"
: > "$KT793_DIR/launch-$n"
i=0
while [ ! -e "$KT793_DIR/release-$n" ] && [ $i -lt 3000 ]; do
  sleep 0.01
  i=$((i + 1))
done
printf '%s\n' '{"type":"result","subtype":"success","result":"orchestrated"}'
"#;

async fn post(state: &AppState, uri: &str, body: serde_json::Value) -> serde_json::Value {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = kronn::build_router_with_auth(state.clone(), false)
        .oneshot(request)
        .await
        .unwrap();
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn join(state: &AppState, context: &serde_json::Value, session: &str) -> serde_json::Value {
    post(
        state,
        "/api/discussions/workflow-step-join",
        serde_json::json!({
            "workflow_step": context,
            "agent_type": "ClaudeCode",
            "session_id": session,
        }),
    )
    .await
}

async fn prepare(state: &AppState, session: &str) -> serde_json::Value {
    post(
        state,
        "/api/orchestration/tool/prepare",
        serde_json::json!({
            "task_reference": TASK,
            "parent_discussion_id": ROOM,
            "worker": {"kind": "agent", "agent_type": "Codex"},
            "worker_scope_intent": "generic",
            "source_agent": "ClaudeCode",
            "source_session_id": session,
        }),
    )
    .await
}

async fn wait_for_file(path: &Path) {
    let waited = tokio::time::timeout(Duration::from_secs(30), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{} never appeared", path.display());
}

async fn run_status(db: &Database) -> RunStatus {
    db.with_read_conn(|conn| Ok(kronn::db::workflows::get_run(conn, RUN)?.unwrap().status))
        .await
        .unwrap()
}

async fn session_status(db: &Database, session: &str) -> String {
    let session = session.to_string();
    db.with_read_conn(move |conn| {
        Ok(conn.query_row(
            "SELECT status FROM discussion_sessions WHERE session_id = ?1",
            [session],
            |row| row.get(0),
        )?)
    })
    .await
    .unwrap()
}

fn workflow() -> Workflow {
    let steps: Vec<WorkflowStep> = serde_json::from_value(serde_json::json!([
        {
            "name": "jeton",
            "step_type": {"type": "JsonData"},
            "json_data_payload": {"room": ROOM},
        },
        {
            "name": "orchestrateur",
            "step_type": {"type": "Agent"},
            "agent": "ClaudeCode",
            "prompt_template": "Orchestrate the ticket.",
            "room_id": "{{steps.jeton.data.room}}",
        },
    ]))
    .unwrap();
    assert!(matches!(steps[1].step_type, StepType::Agent));
    Workflow {
        pinned: false,
        id: "wf-793".into(),
        name: "autoCode".into(),
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

fn pending_run() -> WorkflowRun {
    WorkflowRun {
        id: RUN.into(),
        workflow_id: "wf-793".into(),
        status: RunStatus::Pending,
        trigger_context: None,
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: chrono::Utc::now(),
        finished_at: None,
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_no_response: 0,
        batch_name: None,
        parent_run_id: None,
        state: Default::default(),
        produced_branches: vec![],
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
        concurrency_key: None,
        triggered_by_run_id: None,
    }
}

fn state_over(db: Arc<Database>) -> AppState {
    AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(
            kronn::core::config::default_config(),
        )),
        db,
        kronn::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

fn read_context(dir: &Path, launch: usize) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join(format!("context-{launch}"))).unwrap())
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn a_room_step_is_principal_without_a_token_at_launch_and_after_resume() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let marks = dir.path().join("marks");
    for path in [&bin, &marks] {
        std::fs::create_dir(path).unwrap();
    }
    for name in ["host", "data", "host-bin", "tmp"] {
        std::fs::create_dir(dir.path().join(name)).unwrap();
    }
    let mut env = Environment(Vec::new());
    env.set("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    env.set("KRONN_HOST_HOME", dir.path().join("host"));
    env.set("KRONN_DATA_DIR", dir.path().join("data"));
    env.set("KRONN_HOST_BIN", dir.path().join("host-bin"));
    env.set("TMPDIR", dir.path().join("tmp"));
    env.set("KRONN_ACP_ADAPTER_CLAUDE", "0");
    env.set("KT793_DIR", &marks);
    let claude = bin.join("claude");
    std::fs::write(&claude, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o700)).unwrap();

    let db_path = dir.path().join("kronn.db");
    let db = Arc::new(Database::open_path(&db_path).unwrap());
    let state = state_over(db.clone());
    let wf = workflow();
    let mut run = pending_run();
    {
        let (wf, run) = (wf.clone(), run.clone());
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES (?1, 'Ticket room', '2026-09-26T00:00:00Z', '2026-09-26T00:00:00Z')",
                [ROOM],
            )?;
            conn.execute(
                "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at)
                 VALUES (?1, 7931, 'Sub-task', '2026-09-26T00:00:00Z', '2026-09-26T00:00:00Z')",
                [TASK],
            )?;
            kronn::db::workflows::insert_workflow(conn, &wf)?;
            kronn::db::workflows::insert_run(conn, &run)?;
            Ok(())
        })
        .await
        .unwrap();
    }
    let config = kronn::core::config::default_config();
    let (tokens, agents) = (config.tokens.clone(), config.agents.clone());

    // ── Launch: the step's agent is running and holds its room capability.
    let first_run = {
        let (state, wf, tokens, agents) =
            (state.clone(), wf.clone(), tokens.clone(), agents.clone());
        tokio::spawn(async move {
            kronn::workflows::runner::execute_run(
                state, &wf, &mut run, &tokens, &agents, None, None, None,
            )
            .await
        })
    };
    wait_for_file(&marks.join("launch-0")).await;
    let first = read_context(&marks, 0);
    assert_eq!(
        std::fs::read_to_string(marks.join("discussion-0")).unwrap(),
        ROOM
    );
    assert_eq!(first["discussion_id"], ROOM);
    assert_eq!(first["run_id"], RUN);
    assert_eq!(first["step_key"], "orchestrateur");

    let stranger = prepare(&state, "bridge-first").await;
    assert_eq!(
        stranger["success"], false,
        "no membership before the join: {stranger}"
    );
    let mut forged = first.clone();
    forged["capability"] = "kr-step-guess".into();
    assert_eq!(
        join(&state, &forged, "bridge-first").await["success"],
        false
    );

    // DoD 1 — no invite token, no disc_join: the capability joins the room and
    // `task_exec_prepare` accepts the step as principal.
    let joined = join(&state, &first, "bridge-first").await;
    assert_eq!(joined["success"], true, "{joined}");
    assert_eq!(joined["data"]["disc_id"], ROOM);
    let prepared = prepare(&state, "bridge-first").await;
    assert_eq!(prepared["success"], true, "{prepared}");
    assert_eq!(prepared["data"]["parent_discussion_id"], ROOM, "{prepared}");

    // ── The backend dies mid-step, then boots again on the same database.
    first_run.abort();
    let _ = first_run.await;
    drop(state);
    drop(db);
    let db = Arc::new(Database::open_path(&db_path).unwrap());
    let state = state_over(db.clone());
    assert_eq!(run_status(&db).await, RunStatus::Interrupted);
    assert_eq!(session_status(&db, "bridge-first").await, "left");
    assert_eq!(prepare(&state, "bridge-first").await["success"], false);
    assert_eq!(
        join(&state, &first, "bridge-first").await["success"],
        false,
        "a capability does not survive its process"
    );

    // DoD 2 — `/resume` replays the step with the earlier outputs and a fresh
    // capability: the replayed agent is the room's principal again. The replay
    // goes through the ACP adapter, the launch above through the direct CLI.
    env.set("KRONN_ACP_ADAPTER_CLAUDE", "1");
    let resumed = post(
        &state,
        &format!("/api/workflow-runs/{RUN}/resume"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(resumed["success"], true, "{resumed}");
    wait_for_file(&marks.join("launch-1")).await;
    let replay = read_context(&marks, 1);
    let adapter_arg = |launch: usize| {
        std::fs::read_to_string(marks.join(format!("argv-{launch}")))
            .unwrap()
            .lines()
            .any(|arg| arg == "--session-id")
    };
    assert!(
        !adapter_arg(0) && adapter_arg(1),
        "direct CLI, then ACP adapter"
    );
    assert_eq!(replay["discussion_id"], ROOM);
    assert_eq!(replay["run_id"], RUN);
    assert_ne!(replay["capability"], first["capability"]);
    assert_eq!(
        join(&state, &replay, "bridge-replay").await["success"],
        true
    );
    let prepared = prepare(&state, "bridge-replay").await;
    assert_eq!(prepared["success"], true, "{prepared}");
    assert_eq!(
        join(&state, &first, "bridge-first").await["success"],
        false,
        "the interrupted step's capability stays dead"
    );

    // ── The step ends: its membership ends with it.
    std::fs::write(marks.join("release-1"), "").unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let status = run_status(&db).await;
            if !matches!(status, RunStatus::Running | RunStatus::Pending) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the resumed run finishes");
    assert_eq!(finished, RunStatus::Success);
    let left = tokio::time::timeout(Duration::from_secs(10), async {
        while session_status(&db, "bridge-replay").await != "left" {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(left.is_ok(), "the finished step's session left the room");
    assert_eq!(prepare(&state, "bridge-replay").await["success"], false);
    assert_eq!(join(&state, &replay, "bridge-late").await["success"], false);
}
