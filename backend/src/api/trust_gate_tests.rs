//! KT-1492 — the gates that consult the trust level: an agent-written Quick
//! Exec and Exec step, a room scope request, the validations' reach. Each
//! asks under Standard, is granted under its level, and is journalled.

use axum::Json;

use crate::api::workflows::{approve_exec_steps_by_trust, create_as, WorkflowWriter};
use crate::db::trust::TrustOrigin;
use crate::models::*;
use crate::AppState;

fn state() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

async fn set_level(state: &AppState, scope: TrustScope, id: Option<&str>, level: TrustLevel) {
    let id = id.map(str::to_owned);
    state
        .db
        .with_conn(move |conn| {
            crate::db::trust::set_level(conn, scope, id.as_deref(), level, "Romu")
        })
        .await
        .unwrap();
}

async fn grants_logged(state: &AppState, approval: TrustApproval) -> Vec<TrustAuditEntry> {
    state
        .db
        .with_conn(|conn| crate::db::trust::audit_entries(conn, 100))
        .await
        .unwrap()
        .into_iter()
        .filter(|entry| entry.event == "grant" && entry.approval == Some(approval))
        .collect()
}

fn quick_exec(command: &str, args: &[&str]) -> CreateQuickExecRequest {
    CreateQuickExecRequest {
        name: "Collect AWS".into(),
        icon: None,
        description: String::new(),
        project_id: None,
        command: command.into(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        timeout_secs: Some(60),
        output_format: CollectQuickExecOutputFormat::Json,
        variables: vec![],
        unmodelled_args_approved: None,
    }
}

fn runs(item: &QuickExec) -> bool {
    let step = WorkflowStep {
        name: item.name.clone(),
        step_type: StepType::Exec,
        exec_command: Some(item.command.clone()),
        exec_args: item.args.clone(),
        exec_unmodelled_args_approved: item.unmodelled_args_approved,
        exec_agent_written: item.agent_written,
        ..Default::default()
    };
    crate::core::inline_code::runtime_refusal(&step).is_none()
}

fn room(id: &str) -> TrustOrigin {
    TrustOrigin::room(id)
}

#[tokio::test]
async fn an_agent_written_quick_exec_waits_under_standard() {
    let state = state();
    let line = quick_exec("aws", &["s3", "ls", "{{bucket}}"]);
    let created = crate::api::quick_execs::create_as_in(state.clone(), line, true, room("r"))
        .await
        .0
        .data
        .unwrap();
    assert_eq!(created.agent_written, Some(true));
    assert_eq!(created.unmodelled_args_approved, None);
    assert!(!runs(&created));
    assert!(grants_logged(&state, TrustApproval::QuickExecLine)
        .await
        .is_empty());
}

/// KT-1490 (#412): under Trusted the line is active at once and journalled.
#[tokio::test]
async fn under_trusted_an_agent_written_quick_exec_is_active_at_once_and_journalled() {
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    let line = quick_exec("aws", &["s3", "ls", "{{bucket}}"]);
    let response = crate::api::quick_execs::create_as_in(state.clone(), line, true, room("r"))
        .await
        .0;
    assert!(response.success, "{:?}", response.error);
    assert!(response.notice.is_none(), "nothing waits for a human");
    let created = response.data.unwrap();
    assert_eq!(created.unmodelled_args_approved, Some(true));
    assert_eq!(
        created.agent_written,
        Some(true),
        "the line is still the agent's"
    );
    assert!(runs(&created));
    let journal = grants_logged(&state, TrustApproval::QuickExecLine).await;
    assert_eq!(journal.len(), 1);
    assert!(journal[0].subject.contains("Collect AWS"));
    assert!(journal[0].detail.contains("granted by trust level trusted"));
    assert_eq!(journal[0].discussion_id.as_deref(), Some("r"));
}

#[tokio::test]
async fn an_agent_edit_to_another_line_is_active_at_once_under_trusted_too() {
    let state = state();
    let line = quick_exec("aws", &["s3", "ls", "{{bucket}}"]);
    let created = crate::api::quick_execs::create_as_in(state.clone(), line, true, room("r"))
        .await
        .0
        .data
        .unwrap();
    assert!(!runs(&created));
    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    let edited = crate::api::quick_execs::update_as_in(
        state.clone(),
        created.id,
        quick_exec("aws", &["s3", "ls", "{{bucket}}", "--recursive"]),
        true,
        room("r"),
    )
    .await
    .0
    .data
    .unwrap();
    assert!(runs(&edited));
    assert_eq!(
        grants_logged(&state, TrustApproval::QuickExecLine)
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn a_room_set_to_standard_keeps_its_quick_execs_waiting_under_a_full_instance() {
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Full).await;
    set_level(&state, TrustScope::Room, Some("r"), TrustLevel::Standard).await;
    let created = crate::api::quick_execs::create_as_in(
        state.clone(),
        quick_exec("aws", &["s3", "ls", "{{bucket}}"]),
        true,
        room("r"),
    )
    .await
    .0
    .data
    .unwrap();
    assert!(!runs(&created), "the narrowest explicit setting wins");
    assert!(grants_logged(&state, TrustApproval::QuickExecLine)
        .await
        .is_empty());
}

#[tokio::test]
async fn a_hard_refusal_stays_refused_under_full_access() {
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Full).await;
    // A value interpolated in program text is never data, whatever the level.
    let response = crate::api::quick_execs::create_as_in(
        state.clone(),
        quick_exec("python3", &["-c", "print('{{ticket}}')"]),
        true,
        room("r"),
    )
    .await
    .0;
    assert!(!response.success);
    assert!(grants_logged(&state, TrustApproval::QuickExecLine)
        .await
        .is_empty());
}

#[tokio::test]
async fn a_human_save_is_never_a_trust_grant() {
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Full).await;
    let created = crate::api::quick_execs::create_as_in(
        state.clone(),
        quick_exec("aws", &["s3", "ls", "{{bucket}}"]),
        false,
        TrustOrigin::default(),
    )
    .await
    .0;
    assert!(!created.success || created.data.unwrap().agent_written.is_none());
    assert!(grants_logged(&state, TrustApproval::QuickExecLine)
        .await
        .is_empty());
}

fn exec_workflow(args: serde_json::Value) -> CreateWorkflowRequest {
    serde_json::from_value(serde_json::json!({
        "name": "plan wf", "project_id": null, "trigger": {"type": "Manual"},
        "exec_allowlist": ["aws"],
        "steps": [{
            "name": "list", "step_type": {"type": "Exec"}, "exec_command": "aws",
            "exec_args": args
        }]
    }))
    .unwrap()
}

#[tokio::test]
async fn an_agent_written_exec_step_waits_under_standard_and_runs_under_trusted() {
    let state = state();
    let args = serde_json::json!(["s3", "ls", "{{x}}"]);
    let Json(waiting) = create_as(
        state.clone(),
        exec_workflow(args.clone()),
        WorkflowWriter::Agent,
    )
    .await;
    let waiting = approve_exec_steps_by_trust(&state, waiting, &room("r")).await;
    let step = &waiting.data.unwrap().steps[0];
    assert_eq!(step.exec_unmodelled_args_approved, None);
    assert!(crate::core::inline_code::runtime_refusal(step).is_some());

    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    let Json(created) = create_as(state.clone(), exec_workflow(args), WorkflowWriter::Agent).await;
    let granted = approve_exec_steps_by_trust(&state, created, &room("r")).await;
    assert!(granted.notice.is_none(), "{:?}", granted.notice);
    let workflow = granted.data.unwrap();
    let step = &workflow.steps[0];
    assert_eq!(step.exec_unmodelled_args_approved, Some(true));
    assert_eq!(step.exec_agent_written, Some(true));
    assert_eq!(crate::core::inline_code::runtime_refusal(step), None);
    assert!(
        !workflow.enabled,
        "an agent's workflow still starts disabled"
    );
    // Persisted, not only returned.
    let id = workflow.id.clone();
    let stored = state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.steps[0].exec_unmodelled_args_approved, Some(true));
    let journal = grants_logged(&state, TrustApproval::ExecStepLine).await;
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].object_kind.as_deref(), Some("workflow_step"));
    assert_eq!(journal[0].object_id.as_deref(), Some(workflow.id.as_str()));
    assert_eq!(journal[0].object_ref.as_deref(), Some("list"));
    assert!(
        journal[0].active,
        "the grant holds until its level is lowered"
    );
    assert_eq!(
        step.exec_approved_by_trust,
        Some(true),
        "the step runs under the level's sandbox"
    );
}

/// Autoreview 🟡 2: an agent in a room with no level that edits a Full workflow
/// gets nothing from that workflow's level.
#[tokio::test]
async fn a_room_with_no_level_does_not_take_the_level_of_the_workflow_it_edits() {
    let state = state();
    let Json(created) = create_as(
        state.clone(),
        exec_workflow(serde_json::json!(["s3", "ls", "{{x}}"])),
        WorkflowWriter::Agent,
    )
    .await;
    let id = created.data.as_ref().unwrap().id.clone();
    set_level(&state, TrustScope::Workflow, Some(&id), TrustLevel::Full).await;
    let result = approve_exec_steps_by_trust(&state, created, &room("r")).await;
    let step = &result.data.unwrap().steps[0];
    assert_eq!(step.exec_unmodelled_args_approved, None);
    assert_eq!(step.exec_approved_by_trust, None);
    assert!(grants_logged(&state, TrustApproval::ExecStepLine)
        .await
        .is_empty());
}

/// The room's own level still decides for an agent editing a workflow.
#[tokio::test]
async fn the_room_level_decides_for_an_agent_editing_a_workflow() {
    let state = state();
    set_level(&state, TrustScope::Room, Some("r"), TrustLevel::Trusted).await;
    let Json(created) = create_as(
        state.clone(),
        exec_workflow(serde_json::json!(["s3", "ls", "{{x}}"])),
        WorkflowWriter::Agent,
    )
    .await;
    let id = created.data.as_ref().unwrap().id.clone();
    set_level(
        &state,
        TrustScope::Workflow,
        Some(&id),
        TrustLevel::Standard,
    )
    .await;
    let result = approve_exec_steps_by_trust(&state, created, &room("r")).await;
    assert_eq!(
        result.data.unwrap().steps[0].exec_unmodelled_args_approved,
        Some(true)
    );
}

async fn stored_quick_exec(state: &AppState, id: &str) -> QuickExec {
    let id = id.to_owned();
    state
        .db
        .with_conn(move |conn| crate::db::quick_execs::get_quick_exec(conn, &id))
        .await
        .unwrap()
        .unwrap()
}

async fn withdrawn_logged(state: &AppState) -> Vec<TrustAuditEntry> {
    state
        .db
        .with_conn(|conn| crate::db::trust::audit_entries(conn, 100))
        .await
        .unwrap()
        .into_iter()
        .filter(|entry| entry.event == "withdrawn")
        .collect()
}

/// Autoreview 🔴 1 + 🟡 3: the line a level approved is marked, so it runs under
/// the level's sandbox, and lowering the level takes the approval back.
#[tokio::test]
async fn a_level_approved_quick_exec_is_marked_and_lowering_the_level_takes_it_back() {
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    let created = crate::api::quick_execs::create_as_in(
        state.clone(),
        quick_exec("aws", &["s3", "ls", "{{bucket}}"]),
        true,
        room("r"),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(created.approved_by_trust, Some(true));
    let journal = grants_logged(&state, TrustApproval::QuickExecLine).await;
    assert_eq!(journal[0].object_kind.as_deref(), Some("quick_exec"));
    assert_eq!(journal[0].object_id.as_deref(), Some(created.id.as_str()));
    assert!(runs(&stored_quick_exec(&state, &created.id).await));

    set_level(&state, TrustScope::Instance, None, TrustLevel::Standard).await;

    let stored = stored_quick_exec(&state, &created.id).await;
    assert_eq!(stored.unmodelled_args_approved, None);
    assert_eq!(stored.approved_by_trust, None);
    assert!(!runs(&stored), "the line waits for a human again");
    let taken_back = withdrawn_logged(&state).await;
    assert_eq!(taken_back.len(), 1);
    assert_eq!(
        taken_back[0].object_id.as_deref(),
        Some(created.id.as_str())
    );
}

#[tokio::test]
async fn a_human_save_never_carries_the_mark_and_a_changed_line_loses_it() {
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    let created = crate::api::quick_execs::create_as_in(
        state.clone(),
        quick_exec("aws", &["s3", "ls", "{{bucket}}"]),
        true,
        room("r"),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(created.approved_by_trust, Some(true));

    // The mark is the server's: a request cannot forge it, an unchanged line keeps it.
    let mut same = quick_exec("aws", &["s3", "ls", "{{bucket}}"]);
    same.unmodelled_args_approved = Some(true);
    let kept = crate::api::quick_execs::update_as_in(
        state.clone(),
        created.id.clone(),
        same,
        false,
        TrustOrigin::default(),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(kept.approved_by_trust, Some(true));

    // A human's edit of the line re-approves it as theirs, with no mark.
    let mut edited = quick_exec("aws", &["s3", "ls", "{{bucket}}", "--recursive"]);
    edited.unmodelled_args_approved = Some(true);
    let human = crate::api::quick_execs::update_as_in(
        state.clone(),
        created.id.clone(),
        edited,
        false,
        TrustOrigin::default(),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(human.unmodelled_args_approved, Some(true));
    assert_eq!(human.approved_by_trust, None);

    // And a level lowered afterwards leaves a human's approval alone.
    set_level(&state, TrustScope::Instance, None, TrustLevel::Standard).await;
    assert_eq!(
        stored_quick_exec(&state, &created.id)
            .await
            .unmodelled_args_approved,
        Some(true)
    );
}

#[tokio::test]
async fn a_level_approved_exec_step_is_taken_back_when_the_level_is_lowered() {
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    let Json(created) = create_as(
        state.clone(),
        exec_workflow(serde_json::json!(["s3", "ls", "{{x}}"])),
        WorkflowWriter::Agent,
    )
    .await;
    let granted = approve_exec_steps_by_trust(&state, created, &room("r")).await;
    let workflow = granted.data.unwrap();
    assert_eq!(workflow.steps[0].exec_approved_by_trust, Some(true));

    set_level(&state, TrustScope::Instance, None, TrustLevel::Standard).await;

    let id = workflow.id.clone();
    let stored = state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.steps[0].exec_unmodelled_args_approved, None);
    assert_eq!(stored.steps[0].exec_approved_by_trust, None);
    assert!(crate::core::inline_code::runtime_refusal(&stored.steps[0]).is_some());
    let taken_back = withdrawn_logged(&state).await;
    assert_eq!(taken_back.len(), 1);
    assert_eq!(taken_back[0].object_kind.as_deref(), Some("workflow_step"));
    assert_eq!(taken_back[0].object_ref.as_deref(), Some("list"));
}

/// An agent's later save of the very step a level approved keeps the approval
/// and the mark; any other line loses both.
#[tokio::test]
async fn an_agent_save_keeps_the_mark_only_on_the_line_a_level_approved() {
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    let Json(created) = create_as(
        state.clone(),
        exec_workflow(serde_json::json!(["s3", "ls", "{{x}}"])),
        WorkflowWriter::Agent,
    )
    .await;
    let workflow = approve_exec_steps_by_trust(&state, created, &room("r"))
        .await
        .data
        .unwrap();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Standard).await;
    // Re-approve by hand to see a human's approval carry no mark.
    let id = workflow.id.clone();
    state
        .db
        .with_conn(move |conn| {
            let mut stored = crate::db::workflows::get_workflow(conn, &id)?
                .ok_or_else(|| anyhow::anyhow!("workflow {id} vanished"))?;
            stored.steps[0].exec_unmodelled_args_approved = Some(true);
            crate::db::workflows::update_workflow_as_agent(conn, &stored)?;
            Ok(())
        })
        .await
        .unwrap();
    let mut steps = workflow.steps.clone();
    steps[0].exec_approved_by_trust = Some(true); // a forged mark
    let stored = state
        .db
        .with_conn({
            let id = workflow.id.clone();
            move |conn| crate::db::workflows::get_workflow(conn, &id)
        })
        .await
        .unwrap()
        .unwrap();
    crate::api::workflows::mark_line_writers(&mut steps, &stored.steps, WorkflowWriter::Agent);
    assert_eq!(
        steps[0].exec_approved_by_trust, None,
        "the mark never comes from the request"
    );

    // Stored with the mark: the same lines, still approved, keep it; a changed
    // line (or one no longer approved) does not.
    let mut marked = stored.steps.clone();
    marked[0].exec_approved_by_trust = Some(true);
    let mut same = marked.clone();
    crate::api::workflows::mark_line_writers(&mut same, &marked, WorkflowWriter::Agent);
    assert_eq!(same[0].exec_approved_by_trust, Some(true));
    let mut changed = marked.clone();
    changed[0].exec_args.push("--recursive".into());
    crate::api::workflows::mark_line_writers(&mut changed, &marked, WorkflowWriter::Agent);
    assert_eq!(changed[0].exec_approved_by_trust, None);
    let mut unapproved = marked.clone();
    unapproved[0].exec_unmodelled_args_approved = None;
    crate::api::workflows::mark_line_writers(&mut unapproved, &marked, WorkflowWriter::Agent);
    assert_eq!(unapproved[0].exec_approved_by_trust, None);
}

#[test]
fn trusted_opens_docker_and_full_drops_the_sandbox_and_nothing_narrows() {
    use crate::api::orchestration::widened_by_trust;
    use ValidationsAccess::{Docker, Full, Sandbox};
    let cases = [
        (Sandbox, TrustLevel::Standard, Sandbox, None),
        (
            Sandbox,
            TrustLevel::Trusted,
            Docker,
            Some(TrustApproval::ValidationsDocker),
        ),
        (
            Sandbox,
            TrustLevel::Full,
            Full,
            Some(TrustApproval::ValidationsUnconfined),
        ),
        (Docker, TrustLevel::Trusted, Docker, None),
        (
            Docker,
            TrustLevel::Full,
            Full,
            Some(TrustApproval::ValidationsUnconfined),
        ),
        (Full, TrustLevel::Standard, Full, None),
        (Full, TrustLevel::Full, Full, None),
    ];
    for (access, level, expected, approval) in cases {
        assert_eq!(
            widened_by_trust(access, level),
            (expected, approval),
            "{access:?} under {level:?}"
        );
    }
}

// ── The human routes ─────────────────────────────────────────────────────

use crate::api::trust::Confirm;
use crate::core::trust_proof::Channel;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serial_test::serial;

fn set(level: TrustLevel, acknowledged_risks: bool, proof: Option<&str>) -> Json<SetTrustRequest> {
    Json(SetTrustRequest {
        level,
        acknowledged_risks,
        acknowledged_vault_unprotected: false,
        proof: proof.map(str::to_owned),
    })
}

/// A request that acknowledges the missing vault protection as well.
fn set_unprotected(level: TrustLevel, proof: Option<&str>) -> Json<SetTrustRequest> {
    Json(SetTrustRequest {
        level,
        acknowledged_risks: true,
        acknowledged_vault_unprotected: true,
        proof: proof.map(str::to_owned),
    })
}

fn bridge() -> Confirm {
    Confirm::on(Channel::Bridge)
}

/// An instance with an authenticator enrolled in a data directory of its own.
struct Enrolled {
    state: AppState,
    seed: Vec<u8>,
    recovery: Vec<String>,
    dir: tempfile::TempDir,
}

impl Enrolled {
    /// The code of the step after the one that confirmed the enrolment.
    fn next_code(&self) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        crate::core::totp::code_at(&self.seed, crate::core::totp::step_of(now) + 1)
    }
}

fn unenrolled() -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("KRONN_DATA_DIR", dir.path());
    let state = state();
    let secret = crate::core::crypto::generate_secret();
    state.config.try_write().unwrap().encryption_secret = Some(secret);
    (state, dir)
}

/// The human-admin secret of the test instance: the file's content, the root of
/// trust of the first enrolment.
async fn admin_secret(state: &AppState) -> String {
    state
        .db
        .with_conn(|conn| {
            Ok(
                crate::db::human_credentials::create_admin_secret(conn, "test")?
                    .expose()
                    .to_owned(),
            )
        })
        .await
        .unwrap()
}

fn enrolled() -> Enrolled {
    let (state, dir) = unenrolled();
    let key = crate::core::crypto::parse_secret(
        state
            .config
            .try_read()
            .unwrap()
            .encryption_secret
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let enrolment =
        crate::core::trust_proof::begin_enrolment(dir.path(), &key, "me", now, false).unwrap();
    let seed = crate::core::totp::base32_decode(&enrolment.secret).unwrap();
    let code = crate::core::totp::code_at(&seed, crate::core::totp::step_of(now));
    let recovery =
        crate::core::trust_proof::confirm_enrolment(dir.path(), &key, &code, now).unwrap();
    Enrolled {
        state,
        seed,
        recovery,
        dir,
    }
}

#[tokio::test]
#[serial]
async fn raising_a_level_needs_the_risk_panel_confirmation_on_the_server_too() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    for level in [TrustLevel::Trusted, TrustLevel::Full] {
        let (status, Json(refused)) = crate::api::trust::put_instance(
            State(state.clone()),
            Confirm::default(),
            set(level, false, Some(&enrolled.recovery[0])),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!refused.success);
    }
    let level = state
        .db
        .with_conn(crate::db::trust::instance_level)
        .await
        .unwrap();
    assert_eq!(level, TrustLevel::Standard, "nothing was stored");

    let (status, Json(granted)) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Full, true, Some(&enrolled.next_code())),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{:?}", granted.error);
    assert_eq!(granted.data.unwrap().effective.level, TrustLevel::Full);

    // Lowering or revoking never asks: no panel confirmation, no proof.
    let (status, Json(lowered)) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Trusted, false, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{:?}", lowered.error);
    assert_eq!(lowered.data.unwrap().effective.level, TrustLevel::Trusted);
    let (status, Json(revoked)) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Standard, false, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revoked.data.unwrap().effective.level, TrustLevel::Standard);
}

/// Autoreview 🔴 1: a request without the proof, even from loopback (the tests
/// call the handler as the middleware's "local human" would), changes nothing.
#[tokio::test]
#[serial]
async fn raising_a_level_needs_a_proof_an_agent_cannot_drive_through_the_ui() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    for proof in [None, Some("000000"), Some("not a code")] {
        let (status, Json(refused)) = crate::api::trust::put_instance(
            State(state.clone()),
            Confirm::default(),
            set(TrustLevel::Full, true, proof),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{proof:?}");
        assert!(!refused.success);
    }
    let level = state
        .db
        .with_conn(crate::db::trust::instance_level)
        .await
        .unwrap();
    assert_eq!(level, TrustLevel::Standard);

    // The code that confirmed the enrolment is spent: replaying it is refused.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let replay = crate::core::totp::code_at(&enrolled.seed, crate::core::totp::step_of(now));
    let (status, _) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Trusted, true, Some(&replay)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A recovery code works once.
    let recovery = &enrolled.recovery[0];
    let (status, _) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Trusted, true, Some(recovery)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Standard, false, None),
    )
    .await;
    let (status, _) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Trusted, true, Some(recovery)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a used recovery code");
}

#[tokio::test]
#[serial]
async fn without_an_enrolment_raising_is_refused_and_says_where_to_enrol() {
    let (state, _dir) = unenrolled();
    let (status, Json(refused)) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Trusted, true, Some("123456")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(refused.error.unwrap().contains("Settings"));
}

#[tokio::test]
#[serial]
async fn five_wrong_codes_lock_the_raise_for_sixty_seconds() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    for _ in 0..5 {
        let _ = crate::api::trust::put_instance(
            State(state.clone()),
            Confirm::default(),
            set(TrustLevel::Full, true, Some("000000")),
        )
        .await;
    }
    let (status, Json(locked)) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Full, true, Some(&enrolled.next_code())),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(locked.error.unwrap().contains("locked"));
}

async fn audit(state: &AppState) -> Vec<crate::models::TrustAuditEntry> {
    state
        .db
        .with_conn(|conn| crate::db::trust::audit_entries(conn, 200))
        .await
        .unwrap()
}

async fn instance_level(state: &AppState) -> TrustLevel {
    state
        .db
        .with_conn(crate::db::trust::instance_level)
        .await
        .unwrap()
}

/// "The code is the proof, whatever the channel": a bridge token with no valid
/// code changes nothing, on any trust route.
#[tokio::test]
#[serial]
async fn a_bridge_call_without_a_valid_code_changes_nothing() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    for proof in [None, Some("000000"), Some("not a code")] {
        let (status, _) = crate::api::trust::put_instance(
            State(state.clone()),
            bridge(),
            set(TrustLevel::Full, true, proof),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{proof:?}");
    }
    // Even lowering and following the instance again present a code on this channel.
    let (status, _) = crate::api::trust::put_room(
        State(state.clone()),
        bridge(),
        Path("r".into()),
        set(TrustLevel::Standard, false, None),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    let (status, _) =
        crate::api::trust::delete_room(State(state.clone()), bridge(), Path("r".into())).await;
    assert_ne!(status, StatusCode::OK);
    let (status, _) = crate::api::trust::disable_proof(
        State(state.clone()),
        bridge(),
        Json(TrustProofRequest {
            proof: Some("000000".into()),
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(instance_level(&state).await, TrustLevel::Standard);
    // The enrolment is still there, and no real recovery code was spent.
    let status = crate::core::trust_proof::status(enrolled.dir.path(), 0).unwrap();
    assert!(status.enrolled);
    assert_eq!(
        status.recovery_codes_left,
        crate::core::trust_proof::RECOVERY_CODES
    );
}

/// The routes that hand back the authenticator seed or recovery codes never
/// answer a bridge token, code or not: the reply would reach a model.
#[tokio::test]
#[serial]
async fn the_seed_and_the_recovery_codes_are_never_handed_to_a_bridge_token() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    let (status, _) = crate::api::trust::enroll_proof(
        State(state.clone()),
        bridge(),
        Json(TrustProofRequest {
            proof: Some(enrolled.recovery[0].clone()),
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = crate::api::trust::renew_recovery_codes(
        State(state.clone()),
        bridge(),
        Json(TrustProofRequest {
            proof: Some(enrolled.recovery[0].clone()),
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = crate::api::trust::confirm_proof(
        State(state.clone()),
        bridge(),
        Json(ConfirmTrustProofRequest {
            code: "000000".into(),
            admin_secret: None,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        crate::core::trust_proof::status(enrolled.dir.path(), 0)
            .unwrap()
            .recovery_codes_left,
        crate::core::trust_proof::RECOVERY_CODES,
        "the refusals spent nothing"
    );
}

/// The owner may give the code to an agent: a bridge call carrying a valid code
/// is accepted for that one action, journalled with its channel, and the code
/// is spent on that route and object only.
#[tokio::test]
#[serial]
async fn a_bridge_call_carrying_a_valid_code_is_accepted_once_for_one_route_and_object() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    let code = enrolled.next_code();
    let (status, Json(granted)) = crate::api::trust::put_instance(
        State(state.clone()),
        bridge(),
        set(TrustLevel::Trusted, true, Some(&code)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{:?}", granted.error);
    assert_eq!(instance_level(&state).await, TrustLevel::Trusted);

    // The code is bound to the call that spent it, and recorded.
    assert_eq!(
        crate::core::trust_proof::last_use(enrolled.dir.path()).unwrap(),
        Some(crate::core::trust_proof::Binding {
            channel: Channel::Bridge,
            route: "PUT /api/trust/instance".into(),
            object: "instance".into(),
        })
    );
    // Journalled with the channel, route and object; the code itself is not stored.
    let entries = audit(&state).await;
    let spent = entries
        .iter()
        .find(|entry| entry.event == "proof")
        .expect("the use of the code is journalled");
    assert!(
        spent.subject.contains("PUT /api/trust/instance"),
        "{spent:?}"
    );
    assert!(spent.subject.contains("instance"), "{spent:?}");
    assert!(spent.detail.contains("bridge channel"), "{spent:?}");
    assert!(
        entries
            .iter()
            .all(|entry| !entry.subject.contains(&code) && !entry.detail.contains(&code)),
        "the code is never journalled"
    );

    // The same code on another route and object is a replay: refused.
    let (status, _) = crate::api::trust::put_instance(
        State(state.clone()),
        bridge(),
        set(TrustLevel::Full, true, Some(&code)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = crate::api::trust::disable_proof(
        State(state.clone()),
        bridge(),
        Json(TrustProofRequest {
            proof: Some(code),
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(instance_level(&state).await, TrustLevel::Trusted);
    let refused = audit(&state)
        .await
        .into_iter()
        .filter(|entry| entry.event == "proof_refused")
        .count();
    assert_eq!(refused, 2, "each replay is journalled as a refusal");

    // A bridge call may lower too, with a code (a recovery code works).
    let (status, _) = crate::api::trust::put_instance(
        State(state.clone()),
        bridge(),
        set(TrustLevel::Standard, false, Some(&enrolled.recovery[0])),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(instance_level(&state).await, TrustLevel::Standard);
}

/// A bridge call carrying a valid code can also disable the confirmation.
#[tokio::test]
#[serial]
async fn a_bridge_call_with_a_valid_code_can_disable_the_confirmation() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    let (status, Json(off)) = crate::api::trust::disable_proof(
        State(state.clone()),
        bridge(),
        Json(TrustProofRequest {
            proof: Some(enrolled.next_code()),
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{:?}", off.error);
    assert!(!off.data.unwrap().enrolled);
}

/// The header carries the code on any channel, and the channel is journalled:
/// a browser request is the UI, anything else is plain HTTP.
#[tokio::test]
#[serial]
async fn the_header_carries_the_code_and_the_channel_is_journalled() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    let (status, Json(done)) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::on(Channel::Http).with_header(&enrolled.next_code()),
        set(TrustLevel::Trusted, true, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{:?}", done.error);
    let (status, _) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::on(Channel::Ui),
        set(TrustLevel::Full, true, Some(&enrolled.recovery[0])),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let channels: Vec<String> = audit(&state)
        .await
        .into_iter()
        .filter(|entry| entry.event == "proof")
        .map(|entry| entry.detail)
        .collect();
    assert!(
        channels.iter().any(|d| d.contains("http channel")),
        "{channels:?}"
    );
    assert!(
        channels.iter().any(|d| d.contains("ui channel")),
        "{channels:?}"
    );

    // The body field wins over the header, and a wrong header is refused.
    let (status, _) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::on(Channel::Http).with_header("000000"),
        set(TrustLevel::Trusted, false, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "lowering needs no code off the bridge"
    );
    let (status, _) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::on(Channel::Http).with_header("000000"),
        set(TrustLevel::Full, true, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// 5 failures lock the confirmation for 60 s, whatever the channel carried them.
#[tokio::test]
#[serial]
async fn failures_on_every_channel_count_toward_the_same_lockout() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    let channels = [
        Channel::Ui,
        Channel::Http,
        Channel::Bridge,
        Channel::Http,
        Channel::Bridge,
    ];
    for (index, channel) in channels.into_iter().enumerate() {
        let (status, _) = crate::api::trust::put_instance(
            State(state.clone()),
            Confirm::on(channel),
            set(TrustLevel::Full, true, Some("000000")),
        )
        .await;
        // The fifth wrong code is the one that starts the lockout.
        let last = index + 1 == channels.len();
        assert_eq!(
            status,
            if last {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::FORBIDDEN
            }
        );
    }
    for channel in [Channel::Ui, Channel::Http, Channel::Bridge] {
        let (status, Json(locked)) = crate::api::trust::put_instance(
            State(state.clone()),
            Confirm::on(channel),
            set(TrustLevel::Full, true, Some(&enrolled.next_code())),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{channel:?}");
        assert!(locked.error.unwrap().contains("locked"));
    }
    assert_eq!(instance_level(&state).await, TrustLevel::Standard);
    // Refusals for a lockout are not journalled: a flood cannot fill the journal.
    let refused = audit(&state)
        .await
        .into_iter()
        .filter(|entry| entry.event == "proof_refused")
        .count();
    assert_eq!(refused, 5);
}

/// Dropping an override must not be a way round the proof.
#[tokio::test]
#[serial]
async fn following_a_higher_instance_level_is_not_a_way_round_the_proof() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Full).await;
    set_level(&state, TrustScope::Room, Some("r"), TrustLevel::Standard).await;
    let now = chrono::Utc::now().to_rfc3339();
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO discussions(id, title, created_at, updated_at) VALUES ('r', 'r', ?1, ?1)",
                [&now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let (status, Json(refused)) =
        crate::api::trust::delete_room(State(state.clone()), Confirm::default(), Path("r".into()))
            .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{:?}", refused.error);
    let trust = state
        .db
        .with_conn(|conn| crate::db::trust::effective(conn, &room("r")))
        .await
        .unwrap();
    assert_eq!(
        trust.level,
        TrustLevel::Standard,
        "the room kept its override"
    );
}

/// A host with no sandbox does not refuse a level: the raise still needs the
/// proof, and an acknowledgement that names the missing vault protection.
#[tokio::test]
#[serial]
async fn a_host_without_a_sandbox_allows_a_level_but_the_acknowledgement_names_the_missing_protection(
) {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    crate::core::trust_sandbox::set_test_unavailable(Some("`bwrap` is not installed"));
    let (status, Json(refused)) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Trusted, true, Some(&enrolled.next_code())),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let message = refused.error.unwrap();
    assert!(
        message.contains("Kronn vault protection is unavailable on this machine")
            && message.contains("no sandbox-exec or bubblewrap")
            && message.contains("bwrap")
            && message.contains("acknowledged_vault_unprotected"),
        "{message}"
    );
    // The overview and the control state it too.
    let (_, Json(overview)) = crate::api::trust::overview(State(state.clone())).await;
    let sandbox = overview.data.unwrap().sandbox;
    assert!(!sandbox.available && !sandbox.protected);
    assert!(sandbox.protect_vault);
    assert!(sandbox
        .missing
        .unwrap()
        .contains("Kronn vault protection is unavailable on this machine"));
    assert_eq!(instance_level(&state).await, TrustLevel::Standard);

    // The acknowledgement alone is not enough: the proof is still required, and
    // the refusal above did not spend the code.
    let (status, _) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set_unprotected(TrustLevel::Trusted, None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, Json(granted)) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set_unprotected(TrustLevel::Trusted, Some(&enrolled.next_code())),
    )
    .await;
    crate::core::trust_sandbox::set_test_unavailable(None);
    assert_eq!(status, StatusCode::OK, "{:?}", granted.error);
    assert_eq!(instance_level(&state).await, TrustLevel::Trusted);
}

/// "Protect the Kronn vault from agents" off: the same acknowledgement is asked,
/// naming the setting.
#[tokio::test]
#[serial]
async fn with_the_vault_protection_off_the_raise_names_the_setting() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    crate::core::trust_sandbox::set_protect_vault(false);
    let (status, Json(refused)) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Full, true, Some(&enrolled.next_code())),
    )
    .await;
    let (_, Json(overview)) = crate::api::trust::overview(State(state.clone())).await;
    let sandbox = overview.data.unwrap().sandbox;
    let (status_ack, _) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set_unprotected(TrustLevel::Full, Some(&enrolled.next_code())),
    )
    .await;
    crate::core::trust_sandbox::set_protect_vault(true);
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(refused.error.unwrap().contains("protect_vault_from_agents"));
    assert!(!sandbox.protect_vault && !sandbox.protected);
    assert!(
        sandbox.available,
        "the host could confine; the owner chose not to"
    );
    assert_eq!(status_ack, StatusCode::OK);
    assert_eq!(instance_level(&state).await, TrustLevel::Full);
}

#[tokio::test]
#[serial]
async fn the_enrolment_routes_issue_a_seed_confirm_it_and_need_a_proof_to_change_it() {
    let (state, dir) = unenrolled();
    let admin = admin_secret(&state).await;
    let (status, Json(started)) = crate::api::trust::enroll_proof(
        State(state.clone()),
        Confirm::default(),
        Json(TrustProofRequest {
            admin_secret: Some(admin.clone()),
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let started = started.data.unwrap();
    assert!(started.uri.starts_with("otpauth://totp/Kronn:"));
    let seed = crate::core::totp::base32_decode(&started.secret).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let code = crate::core::totp::code_at(&seed, crate::core::totp::step_of(now));

    let (status, _) = crate::api::trust::confirm_proof(
        State(state.clone()),
        Confirm::default(),
        Json(ConfirmTrustProofRequest {
            code: "000000".into(),
            admin_secret: Some(admin.clone()),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, Json(codes)) = crate::api::trust::confirm_proof(
        State(state.clone()),
        Confirm::default(),
        Json(ConfirmTrustProofRequest {
            code,
            admin_secret: Some(admin.clone()),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let codes = codes.data.unwrap().codes;
    assert_eq!(codes.len(), 10);

    // Changing or disabling it needs a proof (the admin secret alone is not enough).
    let (status, _) = crate::api::trust::enroll_proof(
        State(state.clone()),
        Confirm::default(),
        Json(TrustProofRequest {
            admin_secret: Some(admin),
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = crate::api::trust::disable_proof(
        State(state.clone()),
        Confirm::default(),
        Json(TrustProofRequest {
            proof: None,
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, Json(still)) = crate::api::trust::get_proof().await;
    assert!(still.data.unwrap().enrolled);

    let (status, Json(renewed)) = crate::api::trust::renew_recovery_codes(
        State(state.clone()),
        Confirm::default(),
        Json(TrustProofRequest {
            proof: Some(codes[0].clone()),
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renewed.data.unwrap().codes.len(), 10);

    let (status, Json(off)) = crate::api::trust::disable_proof(
        State(state.clone()),
        Confirm::default(),
        Json(TrustProofRequest {
            proof: Some(codes[1].clone()),
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, 403u16, "the old recovery codes stopped working");
    assert!(!off.success);
    drop(dir);
}

/// The first enrolment is a trust root: nothing proves the caller yet, so a
/// token-less loopback caller (an agent's `curl`) with no admin secret gets no
/// seed, confirms nothing and reads no recovery code.
#[tokio::test]
#[serial]
async fn the_first_enrolment_needs_the_admin_secret_and_an_agent_without_it_gets_nothing() {
    let (state, dir) = unenrolled();
    let admin = admin_secret(&state).await;
    let enroll = |secret: Option<&str>| {
        let state = state.clone();
        let request = TrustProofRequest {
            admin_secret: secret.map(str::to_owned),
            ..Default::default()
        };
        async move {
            crate::api::trust::enroll_proof(State(state), Confirm::default(), Json(request)).await
        }
    };
    for refused in [None, Some(""), Some("kr-admin-not-the-secret")] {
        let (status, Json(body)) = enroll(refused).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{refused:?}");
        assert!(body.data.is_none());
    }
    assert!(
        !crate::core::trust_proof::status(dir.path(), 0)
            .unwrap()
            .pending,
        "a refused call must not start an enrolment"
    );

    // The owner starts one.
    let (status, Json(started)) = enroll(Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    let seed = crate::core::totp::base32_decode(&started.data.unwrap().secret).unwrap();

    // The hijack: an agent replaces the owner's pending seed with its own.
    let (status, _) = enroll(None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Even with the secret, a pending enrolment is not replaced.
    let (status, Json(again)) = enroll(Some(&admin)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again:?}");
    // A disable cannot clear it either: it needs a proof, and none exists yet.
    let (status, _) = crate::api::trust::disable_proof(
        State(state.clone()),
        Confirm::default(),
        Json(TrustProofRequest::default()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        crate::core::trust_proof::status(dir.path(), 0)
            .unwrap()
            .pending,
        "the owner's pending enrolment survived"
    );

    // An agent that guesses the first code cannot confirm without the secret
    // and so never reaches the recovery codes.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let code = crate::core::totp::code_at(&seed, crate::core::totp::step_of(now));
    for secret in [None, Some("wrong".to_owned())] {
        let (status, Json(body)) = crate::api::trust::confirm_proof(
            State(state.clone()),
            Confirm::default(),
            Json(ConfirmTrustProofRequest {
                code: code.clone(),
                admin_secret: secret,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.data.is_none(), "recovery codes reached a caller");
    }
    let (status, Json(codes)) = crate::api::trust::confirm_proof(
        State(state.clone()),
        Confirm::default(),
        Json(ConfirmTrustProofRequest {
            code,
            admin_secret: Some(admin),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(codes.data.unwrap().codes.len(), 10);
    drop(dir);
}

/// A bridge token never enrols, admin secret or not.
#[tokio::test]
#[serial]
async fn a_bridge_caller_cannot_enrol_even_with_the_admin_secret() {
    let (state, dir) = unenrolled();
    let admin = admin_secret(&state).await;
    let (status, _) = crate::api::trust::enroll_proof(
        State(state.clone()),
        Confirm::on(Channel::Bridge),
        Json(TrustProofRequest {
            admin_secret: Some(admin),
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        !crate::core::trust_proof::status(dir.path(), 0)
            .unwrap()
            .pending
    );
}

/// The owner cancels their own pending enrolment with the admin secret, at
/// once; an agent (no secret, or a bridge token) cannot.
#[tokio::test]
#[serial]
async fn the_owner_cancels_a_pending_enrolment_with_the_admin_secret() {
    let (state, dir) = unenrolled();
    let admin = admin_secret(&state).await;
    let (status, _) = crate::api::trust::enroll_proof(
        State(state.clone()),
        Confirm::default(),
        Json(TrustProofRequest {
            admin_secret: Some(admin.clone()),
            ..Default::default()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pending = || {
        crate::core::trust_proof::status(dir.path(), 0)
            .unwrap()
            .pending
    };
    assert!(pending());

    let cancel = |confirm: Confirm, secret: Option<String>| {
        let state = state.clone();
        async move {
            crate::api::trust::disable_proof(
                State(state),
                confirm,
                Json(TrustProofRequest {
                    admin_secret: secret,
                    ..Default::default()
                }),
            )
            .await
            .0
        }
    };
    for (confirm, secret) in [
        (Confirm::default(), None),
        (
            Confirm::default(),
            Some("kr-admin-not-the-secret".to_owned()),
        ),
        (Confirm::on(Channel::Bridge), Some(admin.clone())),
    ] {
        assert_eq!(cancel(confirm, secret.clone()).await, StatusCode::FORBIDDEN);
        assert!(pending(), "a refused cancel cleared the owner's enrolment");
    }

    assert_eq!(
        cancel(Confirm::default(), Some(admin)).await,
        StatusCode::OK
    );
    assert!(!pending());
}

/// The failure that starts a lockout writes an alert line to the journal.
#[tokio::test]
#[serial]
async fn a_lockout_writes_an_alert_to_the_journal() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    for _ in 0..crate::core::trust_proof::MAX_FAILURES {
        let (_, _) = crate::api::trust::put_instance(
            State(state.clone()),
            Confirm::default(),
            set(TrustLevel::Trusted, true, Some("000000")),
        )
        .await;
    }
    let entries = state
        .db
        .with_read_conn(|conn| crate::db::trust::audit_entries(conn, 50))
        .await
        .unwrap();
    let alerts: Vec<_> = entries
        .iter()
        .filter(|entry| entry.detail.contains("ALERT"))
        .collect();
    assert_eq!(alerts.len(), 1, "{entries:?}");
    assert_eq!(alerts[0].event, "proof_refused");
    assert!(alerts[0].detail.contains("locked for 60 s"));
}

/// A guess at a recovery code while locked is journalled, not free.
#[tokio::test]
#[serial]
async fn a_wrong_recovery_code_during_a_lockout_is_journalled() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    for _ in 0..crate::core::trust_proof::MAX_FAILURES {
        let (_, _) = crate::api::trust::put_instance(
            State(state.clone()),
            Confirm::default(),
            set(TrustLevel::Trusted, true, Some("000000")),
        )
        .await;
    }
    let (status, _) = crate::api::trust::put_instance(
        State(state.clone()),
        Confirm::default(),
        set(TrustLevel::Trusted, true, Some("abcd-efgh-jkmn")),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let entries = state
        .db
        .with_read_conn(|conn| crate::db::trust::audit_entries(conn, 50))
        .await
        .unwrap();
    assert!(
        entries.iter().any(|entry| entry.event == "proof_refused"
            && entry.detail.contains("wrong recovery code while locked")),
        "{entries:?}"
    );
}

#[tokio::test]
#[serial]
async fn a_room_and_a_workflow_carry_their_own_level_and_revoke_in_one_call() {
    let enrolled = enrolled();
    let state = enrolled.state.clone();
    let now = chrono::Utc::now().to_rfc3339();
    let stamp = now.clone();
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO discussions(id, title, created_at, updated_at) VALUES ('r', 'r', ?1, ?1)",
                [&stamp],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let (_, Json(on)) = crate::api::trust::put_room(
        State(state.clone()),
        Confirm::default(),
        Path("r".into()),
        set(TrustLevel::Trusted, true, Some(&enrolled.next_code())),
    )
    .await;
    let view = on.data.unwrap();
    assert_eq!(view.effective.level, TrustLevel::Trusted);
    assert_eq!(view.effective.source, TrustScope::Room);
    assert_eq!(view.setting.set_by.as_deref(), Some("human"));

    let (_, Json(off)) =
        crate::api::trust::delete_room(State(state.clone()), Confirm::default(), Path("r".into()))
            .await;
    let view = off.data.unwrap();
    assert_eq!(view.effective.level, TrustLevel::Standard);
    assert!(!view.effective.explicit);

    let (status, _) =
        crate::api::trust::get_room(State(state.clone()), Path("missing".into())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = crate::api::trust::put_workflow(
        State(state.clone()),
        Confirm::default(),
        Path("missing".into()),
        set(TrustLevel::Standard, false, None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[serial]
async fn the_overview_lists_the_policy_the_gates_use_and_what_is_never_covered() {
    let (state, _dir) = unenrolled();
    let (_, Json(overview)) = crate::api::trust::overview(State(state)).await;
    let overview = overview.data.unwrap();
    assert_eq!(overview.effects, crate::core::trust::effects());
    assert_eq!(overview.never_covered, NeverCovered::ALL.to_vec());
    assert_eq!(overview.instance.level, None);
    assert!(!overview.proof.enrolled);
    assert!(overview.sandbox.available);
}

#[tokio::test]
async fn a_destructive_action_on_kronn_data_keeps_its_confirmation_under_full_access() {
    // The destructive gate is a pure function of the request: no trust level is
    // an input, so Full access cannot lift it.
    use axum::http::Method;
    for path in [
        "/api/projects/p1",
        "/api/workflows/w1",
        "/api/discussions/d1",
    ] {
        assert!(
            crate::is_destructive(&Method::DELETE, path),
            "DELETE {path} must stay a confirmed, destructive request"
        );
    }
}

// ── Owner review (P1): grant, save and revoke are one unit ────────────────

async fn active_grants(state: &AppState) -> usize {
    state
        .db
        .with_conn(|conn| crate::db::trust::audit_entries(conn, 1000))
        .await
        .unwrap()
        .into_iter()
        .filter(|entry| entry.event == "grant" && entry.active)
        .count()
}

async fn quick_exec_count(state: &AppState) -> usize {
    state
        .db
        .with_conn(crate::db::quick_execs::list_quick_execs)
        .await
        .unwrap()
        .len()
}

fn pending_quick_exec() -> crate::api::quick_execs::PendingGrant {
    crate::api::quick_execs::PendingGrant {
        origin: room("r"),
        subject: "Quick Exec \u{ab} Collect AWS \u{bb}".into(),
        project_id: None,
    }
}

fn approved_item(id: &str) -> QuickExec {
    QuickExec {
        id: id.into(),
        name: "Collect AWS".into(),
        icon: "x".into(),
        description: String::new(),
        project_id: None,
        command: "aws".into(),
        args: vec!["s3".into(), "ls".into(), "{{bucket}}".into()],
        timeout_secs: 30,
        output_format: CollectQuickExecOutputFormat::Json,
        variables: vec![],
        pinned: false,
        unmodelled_args_approved: Some(true),
        agent_written: Some(true),
        approved_by_trust: Some(true),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

/// The level read when the line was validated is lowered before it is saved:
/// the save is refused, nothing approved is persisted, no grant is journalled.
#[tokio::test]
async fn a_level_lowered_between_the_check_and_the_save_stores_no_approval() {
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    // The gate has read Trusted and approved the line…
    let item = approved_item("qe-late");
    let pending = Some(pending_quick_exec());
    // …and the owner lowers the level before the line reaches the database.
    set_level(&state, TrustScope::Instance, None, TrustLevel::Standard).await;

    let saved = crate::api::quick_execs::store_quick_exec(&state, &item, true, pending).await;

    assert_eq!(
        saved,
        Err(crate::api::quick_execs::TRUST_CHANGED.to_string())
    );
    assert_eq!(quick_exec_count(&state).await, 0, "nothing was persisted");
    assert_eq!(active_grants(&state).await, 0);
    assert!(grants_logged(&state, TrustApproval::QuickExecLine)
        .await
        .is_empty());
}

/// The other order: the save comes first, then the owner lowers the level; the
/// grant the save journalled is active, so the lowering finds and withdraws it.
#[tokio::test]
async fn a_level_lowered_right_after_the_save_withdraws_the_approval_it_gave() {
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    let item = approved_item("qe-first");

    crate::api::quick_execs::store_quick_exec(&state, &item, true, Some(pending_quick_exec()))
        .await
        .unwrap();
    assert_eq!(active_grants(&state).await, 1);
    set_level(&state, TrustScope::Instance, None, TrustLevel::Standard).await;

    let stored = stored_quick_exec(&state, "qe-first").await;
    assert_eq!(stored.unmodelled_args_approved, None);
    assert_eq!(stored.approved_by_trust, None);
    assert_eq!(active_grants(&state).await, 0);
    assert_eq!(withdrawn_logged(&state).await.len(), 1);
}

/// Saves and the lowering racing for real: whatever the interleaving, no
/// approved line is left without an active grant, and none stays approved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_saves_and_a_lowering_never_leave_an_approval_behind() {
    for round in 0..8 {
        let state = state();
        set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
        let mut saves = Vec::new();
        for n in 0..12 {
            let state = state.clone();
            saves.push(tokio::spawn(async move {
                let mut line = quick_exec("aws", &["s3", "ls", "{{bucket}}"]);
                line.name = format!("Collect {round}-{n}");
                crate::api::quick_execs::create_as_in(state, line, true, room("r")).await
            }));
        }
        let lowering = {
            let state = state.clone();
            tokio::spawn(async move {
                set_level(&state, TrustScope::Instance, None, TrustLevel::Standard).await
            })
        };
        for save in saves {
            let Json(response) = save.await.unwrap();
            // A save either lands or is refused because the level went down.
            assert!(
                response.success
                    || response.error.as_deref() == Some(crate::api::quick_execs::TRUST_CHANGED),
                "{:?}",
                response.error
            );
        }
        lowering.await.unwrap();

        let rows = state
            .db
            .with_conn(crate::db::quick_execs::list_quick_execs)
            .await
            .unwrap();
        assert!(
            rows.iter().all(|row| row.approved_by_trust != Some(true)
                && row.unmodelled_args_approved != Some(true)),
            "round {round}: a line stayed approved after the level went down"
        );
        assert_eq!(active_grants(&state).await, 0, "round {round}");
    }
}

/// The Exec steps are approved on the workflow as stored now: an edit made
/// since the response was built is kept, not overwritten by the older copy.
#[tokio::test]
async fn a_level_approval_of_exec_steps_does_not_overwrite_a_concurrent_edit() {
    let state = state();
    let Json(created) = create_as(
        state.clone(),
        exec_workflow(serde_json::json!(["s3", "ls", "{{x}}"])),
        WorkflowWriter::Agent,
    )
    .await;
    let id = created.data.as_ref().unwrap().id.clone();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    // A human renames the workflow after the agent's response was built.
    let rename = id.clone();
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE workflows SET name = 'renamed by a human' WHERE id = ?1",
                [&rename],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let granted = approve_exec_steps_by_trust(&state, created, &room("r")).await;

    let workflow = granted.data.unwrap();
    assert_eq!(workflow.name, "renamed by a human");
    let stored = state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.name, "renamed by a human");
    assert_eq!(stored.steps[0].exec_unmodelled_args_approved, Some(true));
}

/// Owner review (P1) 2: the room badge writes an explicit Standard; what the
/// room got from the instance is withdrawn with it, and another room keeps its own.
#[tokio::test]
async fn revoking_a_room_withdraws_what_it_inherited_from_the_instance() {
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    let mine = crate::api::quick_execs::create_as_in(
        state.clone(),
        quick_exec("aws", &["s3", "ls", "{{bucket}}"]),
        true,
        room("mine"),
    )
    .await
    .0
    .data
    .unwrap();
    let other = crate::api::quick_execs::create_as_in(
        state.clone(),
        quick_exec("aws", &["ec2", "describe", "{{id}}"]),
        true,
        room("other"),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(mine.approved_by_trust, Some(true));

    // The one-click revoke of the room "mine".
    set_level(&state, TrustScope::Room, Some("mine"), TrustLevel::Standard).await;

    let mine = stored_quick_exec(&state, &mine.id).await;
    assert_eq!(mine.unmodelled_args_approved, None);
    assert_eq!(mine.approved_by_trust, None);
    let other = stored_quick_exec(&state, &other.id).await;
    assert_eq!(
        other.approved_by_trust,
        Some(true),
        "another room is untouched"
    );
    assert_eq!(withdrawn_logged(&state).await.len(), 1);
}

/// A Quick Exec the level approved, in room "r", read as an editor would.
async fn trusted_quick_exec(state: &AppState) -> QuickExec {
    set_level(state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    let created = crate::api::quick_execs::create_as_in(
        state.clone(),
        quick_exec("aws", &["s3", "ls", "{{bucket}}"]),
        true,
        room("r"),
    )
    .await
    .0
    .data
    .unwrap();
    stored_quick_exec(state, &created.id).await
}

/// Owner follow-up (P1): a metadata-only edit loads the approved line, the
/// level is lowered (the approval is withdrawn), then the edit finishes with
/// the line it loaded. The stale approval must not come back, with no grant
/// left for a later revoke to withdraw.
#[tokio::test]
async fn an_unchanged_line_save_interrupted_by_a_revoke_does_not_restore_the_approval() {
    let state = state();
    let mut loaded = trusted_quick_exec(&state).await;
    assert_eq!(loaded.approved_by_trust, Some(true));

    set_level(&state, TrustScope::Instance, None, TrustLevel::Standard).await;
    loaded.description = "metadata only".into();
    crate::api::quick_execs::store_quick_exec(&state, &loaded, false, None)
        .await
        .unwrap();

    let stored = stored_quick_exec(&state, &loaded.id).await;
    assert_eq!(stored.description, "metadata only", "the edit itself lands");
    assert_eq!(stored.unmodelled_args_approved, None);
    assert_eq!(stored.approved_by_trust, None);
    assert!(!runs(&stored), "the line waits for a human again");
    assert_eq!(active_grants(&state).await, 0);
    // A second revoke finds nothing to withdraw and nothing left approved.
    set_level(&state, TrustScope::Room, Some("r"), TrustLevel::Standard).await;
    let stored = stored_quick_exec(&state, &loaded.id).await;
    assert_eq!(stored.unmodelled_args_approved, None);
    assert_eq!(stored.approved_by_trust, None);
}

/// Control: with no revoke in between, the edit keeps the level's approval and
/// its grant stays active (so a later revoke still withdraws it).
#[tokio::test]
async fn an_unchanged_line_save_keeps_a_live_level_approval_and_its_grant() {
    let state = state();
    let mut loaded = trusted_quick_exec(&state).await;
    loaded.description = "metadata only".into();

    crate::api::quick_execs::store_quick_exec(&state, &loaded, false, None)
        .await
        .unwrap();

    let stored = stored_quick_exec(&state, &loaded.id).await;
    assert_eq!(stored.unmodelled_args_approved, Some(true));
    assert_eq!(stored.approved_by_trust, Some(true));
    assert_eq!(active_grants(&state).await, 1);
    set_level(&state, TrustScope::Instance, None, TrustLevel::Standard).await;
    assert_eq!(
        stored_quick_exec(&state, &loaded.id)
            .await
            .approved_by_trust,
        None
    );
}

/// The stale mark is dropped, but a human's own approval of that very line,
/// given after the revoke, is not.
#[tokio::test]
async fn a_stale_level_mark_does_not_erase_a_humans_approval_given_since() {
    let state = state();
    let mut loaded = trusted_quick_exec(&state).await;
    set_level(&state, TrustScope::Instance, None, TrustLevel::Standard).await;
    let mut by_human = stored_quick_exec(&state, &loaded.id).await;
    by_human.unmodelled_args_approved = Some(true);
    state
        .db
        .with_conn(move |conn| crate::db::quick_execs::update_quick_exec(conn, &by_human))
        .await
        .unwrap();

    loaded.description = "metadata only".into();
    crate::api::quick_execs::store_quick_exec(&state, &loaded, false, None)
        .await
        .unwrap();

    let stored = stored_quick_exec(&state, &loaded.id).await;
    assert_eq!(stored.unmodelled_args_approved, Some(true));
    assert_eq!(stored.approved_by_trust, None);
}

/// The same, for the Exec steps of a workflow: a save that read the workflow
/// before a revoke must not write the withdrawn approval back.
#[tokio::test]
async fn a_workflow_save_interrupted_by_a_revoke_does_not_restore_the_approval() {
    use crate::api::workflows::{before_write_hook, update_as};
    let state = state();
    set_level(&state, TrustScope::Instance, None, TrustLevel::Trusted).await;
    let Json(created) = create_as(
        state.clone(),
        exec_workflow(serde_json::json!(["s3", "ls", "{{x}}"])),
        WorkflowWriter::Agent,
    )
    .await;
    let workflow = approve_exec_steps_by_trust(&state, created, &room("r"))
        .await
        .data
        .unwrap();
    assert_eq!(workflow.steps[0].exec_approved_by_trust, Some(true));

    let (reached, resume) = before_write_hook::arm(&workflow.id);
    let edit = {
        let (state, id) = (state.clone(), workflow.id.clone());
        tokio::spawn(async move {
            let rename = serde_json::from_value(serde_json::json!({ "name": "renamed" })).unwrap();
            update_as(state, id, rename, WorkflowWriter::Human).await
        })
    };
    // The edit has read the approved workflow; the owner revokes; the edit goes on.
    reached.notified().await;
    set_level(&state, TrustScope::Instance, None, TrustLevel::Standard).await;
    resume.notify_one();
    let Json(response) = edit.await.unwrap();
    assert!(response.success, "{:?}", response.error);

    let answered = response.data.unwrap();
    assert_eq!(answered.steps[0].exec_approved_by_trust, None);
    let id = workflow.id.clone();
    let stored = state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.name, "renamed", "the edit itself lands");
    assert_eq!(stored.steps[0].exec_unmodelled_args_approved, None);
    assert_eq!(stored.steps[0].exec_approved_by_trust, None);
    assert!(crate::core::inline_code::runtime_refusal(&stored.steps[0]).is_some());
    assert_eq!(active_grants(&state).await, 0);
}

/// KT-1490 — a request the MCP bridge marked as an agent's (no bridge token) is
/// the agent channel on the trust routes, even with an `Origin` a script can set.
#[tokio::test]
async fn an_agent_marked_request_is_the_agent_channel_on_the_trust_routes() {
    use axum::extract::FromRequestParts;

    async fn channel_of(marked: bool, origin: bool) -> Channel {
        let mut request = axum::http::Request::builder().uri("/api/trust/instance");
        if origin {
            request = request.header("origin", "http://localhost:5173");
        }
        let (mut parts, _) = request.body(()).unwrap().into_parts();
        if marked {
            parts
                .extensions
                .insert(crate::core::bridge_token::AgentMarker::default());
        }
        let Ok(confirm) = Confirm::from_request_parts(&mut parts, &()).await;
        confirm.channel_for_tests()
    }

    assert_eq!(channel_of(true, false).await, Channel::Bridge);
    assert_eq!(channel_of(true, true).await, Channel::Bridge);
    assert_eq!(channel_of(false, true).await, Channel::Ui);
    assert_eq!(channel_of(false, false).await, Channel::Http);
}
