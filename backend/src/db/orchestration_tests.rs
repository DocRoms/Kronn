//! KT-317 — persistence + state-machine tests (DoD-6): constraints, forbidden
//! transitions, idempotence, concurrency, saga-checkpoint resume, lineage. No
//! Git integration (that is KT-320).

use super::*;
use rusqlite::{params, Connection};

use crate::models::{
    saga_resume_action, AgentType, BlockedReasonCode, CampaignWorkerSelection,
    CancellationCleanupPolicy, ExecutionRecoveryAction, ExecutionTimeoutKind,
    LaunchSingleTaskInput, MessageTarget, MessageTargetKind, ModelTier, OrchestrationActor,
    OrchestrationControlState, OrchestrationResiliencePolicy, OrchestrationRunInput,
    OrchestrationRunKind, PlanningActorKind, SagaResumeAction, TaskExecutionStatus,
    TaskWorkerScope, ValidationSpec,
};

fn seed_session(conn: &Connection, pk: i64, agent_type: &str, session_id: &str) {
    conn.execute(
        "INSERT INTO discussion_sessions \
         (id, disc_id, agent_type, session_id, role, status, joined_at) \
         VALUES (?1, ?2, ?3, ?4, 'peer', 'active', '2026-01-01T00:00:00Z')",
        params![pk, DISC, agent_type, session_id],
    )
    .unwrap();
}

const DISC: &str = "disc-parent";

fn setup() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, name, path, created_at, updated_at) \
         VALUES ('p1', 'P', '/p', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO discussions (id, title, created_at, updated_at) \
         VALUES (?1, 'D', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        params![DISC],
    )
    .unwrap();
    conn
}

fn seed_task(conn: &Connection, task_id: &str, number: i64) {
    conn.execute(
        "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at) \
         VALUES (?1, ?2, 'T', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        params![task_id, number],
    )
    .unwrap();
}

fn seed_plan_task(conn: &Connection, task_id: &str, number: i64, position: i64, placement: &str) {
    seed_task(conn, task_id, number);
    conn.execute(
        "UPDATE planning_tasks SET status = 'todo' WHERE id = ?1",
        [task_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO planning_task_projects (task_id, project_id) VALUES (?1, 'p1')",
        [task_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO planning_task_discussions \
         (task_id, discussion_id, placement, is_primary, position, created_at) \
         VALUES (?1, ?2, ?3, 0, ?4, '2026-01-01T00:00:00Z')",
        params![task_id, DISC, placement, position],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO planning_task_dod_items \
         (id, task_id, sentence, completed, position, created_at, updated_at) \
         VALUES (?1, ?2, 'done means tested', 0, 0, \
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        params![format!("dod-{task_id}"), task_id],
    )
    .unwrap();
}

fn campaign(
    conn: &Connection,
    concurrency: u32,
    cli_concurrency: u32,
) -> crate::models::OrchestrationRun {
    let mut input = OrchestrationRunInput::single_task(DISC);
    input.kind = OrchestrationRunKind::Campaign;
    input.project_id = Some("p1".into());
    input.target_branch = Some("main".into());
    input.max_concurrent_executions = concurrency;
    input.max_cli_concurrent_executions = cli_concurrency;
    input.allowed_agents = vec![AgentType::Codex, AgentType::ClaudeCode];
    input.default_worker = Some(CampaignWorkerSelection {
        target: MessageTarget::agent(AgentType::Codex),
        model: Some("gpt-test".into()),
        profile_id: Some("profile-test".into()),
    });
    create_orchestration_run(conn, &input).unwrap()
}

fn backend_actor() -> OrchestrationActor {
    OrchestrationActor {
        kind: PlanningActorKind::Backend,
        id: Some("orchestrator".into()),
        session_id: None,
        source_message_id: None,
    }
}

#[test]
fn task_worker_transport_contract_is_closed_over_all_three_identity_kinds() {
    for valid in [
        MessageTarget::discussion_agent(AgentType::Ollama),
        MessageTarget::discussion_agent(AgentType::LiteLlm),
        MessageTarget::discussion_agent(AgentType::Nvidia),
        MessageTarget::agent(AgentType::ClaudeCode),
        MessageTarget::agent(AgentType::Codex),
        MessageTarget::cli(AgentType::ClaudeCode, 42),
    ] {
        ensure_task_worker_transport_compatible(&valid)
            .unwrap_or_else(|error| panic!("valid target {valid:?} refused: {error}"));
    }

    for invalid in [
        MessageTarget::discussion_agent(AgentType::ClaudeCode),
        MessageTarget::discussion_agent(AgentType::Codex),
        MessageTarget::agent(AgentType::Ollama),
        MessageTarget::agent(AgentType::LiteLlm),
        MessageTarget::agent(AgentType::Nvidia),
        MessageTarget::cli(AgentType::Ollama, 42),
    ] {
        let error = ensure_task_worker_transport_compatible(&invalid)
            .expect_err("cross-transport worker target must fail closed");
        assert!(error.to_string().contains("incompatible worker transport"));
    }
}

/// Launch a fresh execution for `task_id` and drive it through `path`, asserting
/// each guarded transition is accepted. Returns the execution id.
fn launch_and_drive(
    conn: &Connection,
    task_id: &str,
    number: i64,
    path: &[TaskExecutionStatus],
) -> String {
    seed_task(conn, task_id, number);
    let id = launch_single_task(
        conn,
        &LaunchSingleTaskInput::new(task_id, DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution
    .id;
    for &to in path {
        assert!(
            transition_execution(conn, &id, to, &backend_actor(), serde_json::json!({})).unwrap(),
            "expected transition to {to:?} to be accepted"
        );
    }
    id
}

/// Independent transcription of the ADR §3 owner-table + diagram (l.199-281),
/// used to LOCK `can_transition_to`. Kept separate from the impl on purpose: if
/// either drifts, `transition_matrix_locks_all_fifteen_states` fails.
fn adr_legal(from: TaskExecutionStatus, to: TaskExecutionStatus) -> bool {
    use TaskExecutionStatus::*;
    if from == to || matches!(from, Done | Failed | Cancelled) {
        return false;
    }
    // Owner-table generalizations (l.278-281): any non-terminal may be
    // interrupted, cancelled or escalated.
    if matches!(to, Interrupted | Cancelled | Escalated) {
        return true;
    }
    // Interrupted is the universal resume point (owner-table + §3): structurally
    // it reaches ANY non-terminal target — modelled by nature here, NOT by copying
    // the diagram's partial arrow list (which strands AwaitingReview/Approved/
    // ChangesRequested/Pending). The concrete narrowing to `interrupted_from_status`
    // is a runtime guard, proven by the DB resume tests, not by this pure matrix.
    if from == Interrupted {
        return !to.is_terminal();
    }
    matches!(
        (from, to),
        (Pending, Provisioning)
            | (Provisioning, Working)
            | (Provisioning, Blocked)
            | (Provisioning, Failed)
            | (Blocked, Provisioning)
            | (Blocked, Applying)
            | (Working, AwaitingReview)
            | (AwaitingReview, Approved)
            | (AwaitingReview, ChangesRequested)
            | (Approved, Integrating)
            | (ChangesRequested, Working)
            // KT-319 tranche 3b: request_changes re-enters the provisioning handshake so a CLI
            // worker re-accepts a control offer before working the next attempt.
            | (ChangesRequested, Provisioning)
            | (Integrating, Validating)
            | (Integrating, ChangesRequested)
            | (Validating, Applying)
            | (Validating, ChangesRequested)
            | (Applying, Done)
            | (Applying, Integrating)
            | (Applying, Blocked)
            | (Escalated, Approved)
            | (Escalated, Working)
    )
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        params![name],
        |r| r.get(0),
    )
    .unwrap()
}

fn index_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)",
        params![name],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn migration_creates_tables_indexes_and_lineage_columns() {
    let conn = setup();
    assert!(table_exists(&conn, "orchestration_runs"));
    assert!(table_exists(&conn, "task_executions"));
    assert!(table_exists(&conn, "task_execution_events"));
    assert!(table_exists(&conn, "task_execution_validation_runs"));
    assert!(index_exists(
        &conn,
        "idx_task_executions_one_active_per_task"
    ));
    assert!(index_exists(&conn, "idx_task_executions_idempotency"));
    assert!(table_exists(&conn, "orchestration_run_events"));
    assert!(index_exists(&conn, "idx_task_executions_cli_active"));
    assert!(table_exists(&conn, "task_execution_recovery"));
    assert!(table_exists(&conn, "task_execution_assignment_events"));
    assert!(table_exists(&conn, "orchestration_reconciliation_events"));
    assert!(table_exists(&conn, "orchestration_run_resilience_policy"));

    for table in ["planning_task_events", "task_execution_events"] {
        let present: bool = conn
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name='actor_session_id')"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(present, "missing durable actor session on {table}");
    }

    // Lineage columns landed on discussion_workspaces.
    for col in ["parent_discussion_id", "base_sha", "task_execution_id"] {
        let present: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('discussion_workspaces') WHERE name=?1)",
                params![col],
                |r| r.get(0),
            )
            .unwrap();
        assert!(present, "missing lineage column {col}");
    }
}

#[test]
fn completed_native_dispatch_without_delivery_interrupts_the_working_execution() {
    let conn = setup();
    let exec_id = launch_and_drive(
        &conn,
        "t-undelivered",
        1099,
        &[
            TaskExecutionStatus::Provisioning,
            TaskExecutionStatus::Working,
        ],
    );
    conn.execute(
        "INSERT INTO messages
         (id, discussion_id, role, content, timestamp, sort_order, received_at)
         VALUES ('message-undelivered', ?1, 'User', 'go', ?2, 1, ?2)",
        params![DISC, chrono::Utc::now().to_rfc3339()],
    )
    .unwrap();
    crate::db::agent_dispatch::enqueue_for_latest_user(
        &conn,
        crate::db::agent_dispatch::NewLatestUserDispatch {
            id: "dispatch-undelivered",
            discussion_id: DISC,
            dedupe_key: "message:message-undelivered",
            agent_override: None,
            chain_prompt_ids: &[],
            batch_item: None,
            group_id: None,
            group_concurrency_limit: None,
        },
    )
    .unwrap();
    attach_execution_dispatch(&conn, &exec_id, "dispatch-undelivered").unwrap();

    let interrupted = interrupt_undelivered_execution_for_dispatch(
        &conn,
        "dispatch-undelivered",
        "worker_completed_without_delivery",
        &backend_actor(),
    )
    .unwrap()
    .expect("a terminal worker without a manifest must be checkpointed");
    assert_eq!(interrupted.execution_id, exec_id);
    assert_eq!(interrupted.parent_discussion_id, DISC);
    assert_eq!(interrupted.task_id, "t-undelivered");

    let execution = get_task_execution(&conn, &exec_id).unwrap().unwrap();
    assert_eq!(execution.status, TaskExecutionStatus::Interrupted);
    assert_eq!(
        execution.interrupted_from_status,
        Some(TaskExecutionStatus::Working)
    );
    let recovery = get_execution_recovery(&conn, &exec_id).unwrap().unwrap();
    assert_eq!(
        recovery.recovery_action,
        crate::models::ExecutionRecoveryAction::AwaitHuman
    );
    assert_eq!(
        recovery.recovery_reason,
        "worker_completed_without_delivery"
    );
    assert!(!recovery.pending);

    let events = list_execution_events(&conn, &exec_id).unwrap();
    let interruption = events
        .iter()
        .find(|event| event.to_status == Some(TaskExecutionStatus::Interrupted))
        .expect("the interruption must be journaled");
    assert_eq!(
        interruption.changes["reason"],
        "worker_completed_without_delivery"
    );

    assert!(
        interrupt_undelivered_execution_for_dispatch(
            &conn,
            "dispatch-undelivered",
            "worker_completed_without_delivery",
            &backend_actor(),
        )
        .unwrap()
        .is_none(),
        "settlement replay must not duplicate the interruption"
    );
}

#[test]
fn http_turn_telemetry_replaces_same_dispatch_and_preserves_rework() {
    let conn = setup();
    let exec_id = launch_and_drive(
        &conn,
        "t-http-telemetry",
        1100,
        &[
            TaskExecutionStatus::Provisioning,
            TaskExecutionStatus::Working,
        ],
    );
    let enqueue = |message_id: &str, dispatch_id: &str| {
        conn.execute(
            "INSERT INTO messages
             (id, discussion_id, role, content, timestamp, sort_order, received_at)
             VALUES (?1, ?2, 'User', 'go', ?3,
                     (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM messages WHERE discussion_id = ?2), ?3)",
            params![message_id, DISC, chrono::Utc::now().to_rfc3339()],
        )
        .unwrap();
        crate::db::agent_dispatch::enqueue_for_latest_user(
            &conn,
            crate::db::agent_dispatch::NewLatestUserDispatch {
                id: dispatch_id,
                discussion_id: DISC,
                dedupe_key: &format!("message:{message_id}"),
                agent_override: None,
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap();
        attach_execution_dispatch(&conn, &exec_id, dispatch_id).unwrap();
    };
    let turn = |prompt_tokens| crate::models::TaskExecutionHttpTurnUsage {
        turn: 1,
        dispatch_id: None,
        provider: "ollama".into(),
        phase: crate::models::TaskExecutionHttpPhase::Read,
        prompt_tokens,
        eval_tokens: 10,
        duration_ms: 1_000,
        provider_ok: true,
        requested_tools: vec!["read_file".into()],
        executed_tools: vec![crate::models::TaskExecutionHttpToolUsage {
            name: "read_file".into(),
            ok: true,
        }],
    };

    enqueue("message-http-1", "dispatch-http-1");
    record_http_turn_telemetry_for_dispatch(&conn, "dispatch-http-1", &[turn(100)]).unwrap();
    record_http_turn_telemetry_for_dispatch(&conn, "dispatch-http-1", &[turn(200)]).unwrap();
    let first = list_execution_events(&conn, &exec_id)
        .unwrap()
        .into_iter()
        .filter(|event| event.action == "http_turn_telemetry")
        .collect::<Vec<_>>();
    assert_eq!(first.len(), 1, "same dispatch must be idempotent");
    assert_eq!(first[0].changes["turns"][0]["prompt_tokens"], 200);

    enqueue("message-http-2", "dispatch-http-2");
    record_http_turn_telemetry_for_dispatch(&conn, "dispatch-http-2", &[turn(300)]).unwrap();
    let sessions = list_execution_events(&conn, &exec_id)
        .unwrap()
        .into_iter()
        .filter(|event| event.action == "http_turn_telemetry")
        .filter_map(|event| event.actor_session_id)
        .collect::<Vec<_>>();
    assert_eq!(sessions, vec!["dispatch-http-1", "dispatch-http-2"]);
}

#[test]
fn recovery_decision_keeps_four_independent_clocks_and_is_consumable_once() {
    let conn = setup();
    seed_plan_task(&conn, "t-recovery", 1100, 0, "active");
    let mut input = OrchestrationRunInput::single_task(DISC);
    input.kind = crate::models::OrchestrationRunKind::Campaign;
    input.timeout_secs = Some(600);
    let run = create_orchestration_run(&conn, &input).unwrap();
    set_resilience_policy(
        &conn,
        &run.id,
        &OrchestrationResiliencePolicy {
            activity_timeout_secs: Some(30),
            review_timeout_secs: Some(120),
            human_wait_timeout_secs: Some(300),
            cancellation_cleanup_policy: CancellationCleanupPolicy::RemoveIfClean,
        },
    )
    .unwrap();
    let execution = launch_task_in_run(
        &conn,
        &run.id,
        &LaunchSingleTaskInput::new("t-recovery", DISC),
        &CampaignWorkerSelection {
            target: MessageTarget::agent(AgentType::Codex),
            model: Some("gpt-recovery".into()),
            profile_id: None,
        },
        &backend_actor(),
    )
    .unwrap()
    .execution;
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Provisioning,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Working,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::AwaitingReview,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    reconcile_stale_task_executions(&conn).unwrap();
    let interrupted = get_task_execution(&conn, &execution.id).unwrap().unwrap();
    let recovery = set_execution_recovery(
        &conn,
        &interrupted,
        &run,
        ExecutionRecoveryAction::AwaitReview,
        "durable delivery awaits review",
    )
    .unwrap();
    assert!(recovery.pending);
    assert!(recovery.total_deadline_at.is_some());
    assert!(recovery.activity_deadline_at.is_some());
    assert!(recovery.review_deadline_at.is_some());
    assert!(recovery.human_wait_started_at.is_none());
    assert!(list_interrupted_execution_ids(&conn)
        .unwrap()
        .contains(&execution.id));

    clear_execution_recovery(&conn, &execution.id, "await_review").unwrap();
    let consumed = get_execution_recovery(&conn, &execution.id)
        .unwrap()
        .unwrap();
    assert!(!consumed.pending);
    assert_eq!(consumed.assignment_generation, 0);
    assert!(consumed.total_deadline_at.is_some());
    assert!(consumed.activity_deadline_at.is_some());
    assert!(consumed.review_deadline_at.is_some());
}

#[test]
fn interrupted_integration_may_rebuild_after_validation_or_apply_drift() {
    use TaskExecutionStatus::*;
    assert!(TaskExecutionStatus::interrupted_resume_allowed(
        Validating,
        Integrating
    ));
    assert!(TaskExecutionStatus::interrupted_resume_allowed(
        Applying,
        Integrating
    ));
    assert!(!TaskExecutionStatus::interrupted_resume_allowed(
        Working,
        Integrating
    ));
}

#[test]
fn cancellation_cascades_dispatch_and_returns_plan_task_to_todo() {
    let conn = setup();
    seed_plan_task(&conn, "t-cancel-tree", 1101, 0, "active");
    let execution = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t-cancel-tree", DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution;
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Provisioning,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Working,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    conn.execute(
        "UPDATE planning_tasks SET status = 'in_progress' WHERE id = 't-cancel-tree'",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at) \
         VALUES ('disc-cancel-child', 'child', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order) \
         VALUES ('msg-cancel', 'disc-cancel-child', 'User', 'work', \
                 '2026-01-01T00:00:00Z', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO agent_dispatch_jobs (id, discussion_id, trigger_message_id, \
             trigger_sort_order, dedupe_key, chain_prompt_ids_json, status, available_at, \
             created_at, updated_at) VALUES ('dispatch-cancel', 'disc-cancel-child', \
             'msg-cancel', 1, 'cancel-dedupe', '[]', 'Running', \
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE task_executions SET sub_discussion_id = 'disc-cancel-child', \
                dispatch_job_id = 'dispatch-cancel' WHERE id = ?1",
        [&execution.id],
    )
    .unwrap();

    let cancelled = cancel_execution_tree(
        &conn,
        &execution.id,
        "principal stopped the task",
        &backend_actor(),
    )
    .unwrap();
    assert_eq!(cancelled.status, TaskExecutionStatus::Cancelled);
    let dispatch: String = conn
        .query_row(
            "SELECT status FROM agent_dispatch_jobs WHERE id = 'dispatch-cancel'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dispatch, "Cancelled");
    let task: String = conn
        .query_row(
            "SELECT status FROM planning_tasks WHERE id = 't-cancel-tree'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(task, "todo");
}

#[test]
fn reassignment_preserves_git_and_records_provider_separately_from_identity() {
    let conn = setup();
    seed_task(&conn, "t-reassign", 1102);
    let execution = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t-reassign", DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution;
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Provisioning,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Working,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    conn.execute(
        "UPDATE task_executions SET candidate_target_sha = 'target-sha', \
                candidate_merge_sha = 'merge-sha', integrated_sha = NULL WHERE id = ?1",
        [&execution.id],
    )
    .unwrap();
    let selection = CampaignWorkerSelection {
        target: MessageTarget::agent(AgentType::ClaudeCode).with_tier(ModelTier::Reasoning),
        model: Some("claude-reasoning".into()),
        profile_id: Some("profile-reviewer".into()),
    };
    let reassigned = reassign_execution_worker(
        &conn,
        &execution.id,
        &selection,
        "provider unavailable",
        &backend_actor(),
    )
    .unwrap();
    assert_eq!(
        reassigned.candidate_target_sha.as_deref(),
        Some("target-sha")
    );
    assert_eq!(reassigned.candidate_merge_sha.as_deref(), Some("merge-sha"));
    assert_eq!(
        reassigned.worker_target_kind,
        Some(MessageTargetKind::Agent)
    );
    assert_eq!(reassigned.worker_agent_type.as_deref(), Some("ClaudeCode"));
    assert_eq!(reassigned.worker_model.as_deref(), Some("claude-reasoning"));
    let (provider, identity, generation): (String, String, i64) = conn
        .query_row(
            "SELECT worker_agent_type, worker_target_kind, generation \
             FROM task_execution_assignment_events WHERE task_execution_id = ?1",
            [&execution.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(provider, "ClaudeCode");
    assert_eq!(identity, "agent");
    assert_eq!(generation, 1);
}

#[test]
fn explicit_reassignment_accepts_an_escalated_worker_without_mutating_its_evidence() {
    let conn = setup();
    seed_task(&conn, "t-reassign-escalated", 1105);
    let execution = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t-reassign-escalated", DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution;
    for status in [
        TaskExecutionStatus::Provisioning,
        TaskExecutionStatus::Working,
        TaskExecutionStatus::Escalated,
    ] {
        transition_execution(
            &conn,
            &execution.id,
            status,
            &backend_actor(),
            serde_json::json!({}),
        )
        .unwrap();
    }
    conn.execute(
        "UPDATE task_executions SET candidate_target_sha = 'target-sha', \
                candidate_merge_sha = 'merge-sha' WHERE id = ?1",
        [&execution.id],
    )
    .unwrap();

    let reassigned = reassign_execution_worker(
        &conn,
        &execution.id,
        &CampaignWorkerSelection {
            target: MessageTarget::discussion_agent(AgentType::Ollama),
            model: Some("qwen3.6:35b-mlx".into()),
            profile_id: Some("profile-local-worker".into()),
        },
        "explicit fallback after a deterministic worker failure",
        &backend_actor(),
    )
    .unwrap();

    assert_eq!(reassigned.status, TaskExecutionStatus::Escalated);
    assert_eq!(
        reassigned.candidate_target_sha.as_deref(),
        Some("target-sha")
    );
    assert_eq!(reassigned.candidate_merge_sha.as_deref(), Some("merge-sha"));
    assert_eq!(reassigned.worker_agent_type.as_deref(), Some("Ollama"));
    assert_eq!(reassigned.worker_model.as_deref(), Some("qwen3.6:35b-mlx"));
    let recovery = get_execution_recovery(&conn, &execution.id)
        .unwrap()
        .expect("the explicit reassignment arms exactly one new generation");
    assert_eq!(recovery.assignment_generation, 1);
    assert!(recovery.pending);
}

#[test]
fn escalated_infrastructure_checkpoint_refuses_reassignment_without_any_mutation() {
    let conn = setup();
    seed_task(&conn, "t-reassign-infra-escalated", 1106);
    let execution = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t-reassign-infra-escalated", DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution;
    for status in [
        TaskExecutionStatus::Provisioning,
        TaskExecutionStatus::Working,
        TaskExecutionStatus::Escalated,
    ] {
        transition_execution(
            &conn,
            &execution.id,
            status,
            &backend_actor(),
            serde_json::json!({}),
        )
        .unwrap();
    }
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE task_execution_recovery SET recovery_action = 'block_missing_workspace', \
             recovery_reason = 'workspace missing', last_activity_at = ?2, \
             assignment_generation = 7, pending = 0, updated_at = ?2 \
         WHERE task_execution_id = ?1",
        params![execution.id, now],
    )
    .unwrap();
    let events_before = list_execution_events(&conn, &execution.id).unwrap().len();

    let error = reassign_execution_worker(
        &conn,
        &execution.id,
        &CampaignWorkerSelection {
            target: MessageTarget::discussion_agent(AgentType::Ollama),
            model: Some("qwen3.6:35b-mlx".into()),
            profile_id: None,
        },
        "a new worker cannot restore a missing workspace",
        &backend_actor(),
    )
    .expect_err("an infrastructure checkpoint must be repaired first");
    assert!(error.to_string().contains("block_missing_workspace"));

    let unchanged = get_task_execution(&conn, &execution.id).unwrap().unwrap();
    assert_eq!(unchanged.status, TaskExecutionStatus::Escalated);
    assert_eq!(unchanged.worker_agent_type, execution.worker_agent_type);
    assert_eq!(unchanged.worker_model, execution.worker_model);
    assert_eq!(unchanged.dispatch_job_id, execution.dispatch_job_id);
    let recovery = get_execution_recovery(&conn, &execution.id)
        .unwrap()
        .expect("the infrastructure checkpoint remains authoritative");
    assert_eq!(
        recovery.recovery_action,
        ExecutionRecoveryAction::BlockMissingWorkspace
    );
    assert_eq!(recovery.assignment_generation, 7);
    assert!(!recovery.pending);
    assert_eq!(
        list_execution_events(&conn, &execution.id).unwrap().len(),
        events_before
    );
    let assignment_events: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_execution_assignment_events \
             WHERE task_execution_id = ?1",
            [&execution.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(assignment_events, 0);
}

#[test]
fn reassignment_refuses_cross_transport_identity_before_any_durable_mutation() {
    let conn = setup();
    seed_task(&conn, "t-reassign-transport", 1104);
    let execution = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t-reassign-transport", DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution;
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Provisioning,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Working,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    let recovery_before = get_execution_recovery(&conn, &execution.id)
        .unwrap()
        .expect("working execution has a recovery projection");

    let error = reassign_execution_worker(
        &conn,
        &execution.id,
        &CampaignWorkerSelection {
            target: MessageTarget::discussion_agent(AgentType::ClaudeCode),
            model: None,
            profile_id: None,
        },
        "bad fallback",
        &backend_actor(),
    )
    .expect_err("a host CLI disguised as discussion_agent must be refused");
    assert!(error
        .to_string()
        .contains("host CLI providers must use kind=agent"));

    let unchanged = get_task_execution(&conn, &execution.id).unwrap().unwrap();
    assert_eq!(unchanged.status, TaskExecutionStatus::Working);
    assert_eq!(unchanged.worker_target_kind, None);
    assert_eq!(unchanged.dispatch_job_id, None);
    let recovery_after = get_execution_recovery(&conn, &execution.id)
        .unwrap()
        .expect("refusal keeps the existing recovery projection");
    assert_eq!(
        recovery_after.assignment_generation,
        recovery_before.assignment_generation
    );
    assert_eq!(recovery_after.pending, recovery_before.pending);
    assert_eq!(
        recovery_after.recovery_action,
        recovery_before.recovery_action
    );
    assert_eq!(
        recovery_after.recovery_reason,
        recovery_before.recovery_reason
    );
    let assignment_events: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_execution_assignment_events \
             WHERE task_execution_id = ?1",
            [&execution.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(assignment_events, 0, "refusal cannot arm a watchdog retry");
}

#[test]
fn reassignment_repairs_an_unadvanced_request_changes_attempt() {
    let conn = setup();
    seed_task(&conn, "t-reassign-rework", 1103);
    let execution = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t-reassign-rework", DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution;
    for status in [
        TaskExecutionStatus::Provisioning,
        TaskExecutionStatus::Working,
    ] {
        transition_execution(
            &conn,
            &execution.id,
            status,
            &backend_actor(),
            serde_json::json!({}),
        )
        .unwrap();
    }
    crate::db::worker_reviews::upsert_review(
        &conn,
        &execution.id,
        0,
        "request_changes",
        r#"{"version":"1","decision":"request_changes"}"#,
    )
    .unwrap();

    let reassigned = reassign_execution_worker(
        &conn,
        &execution.id,
        &CampaignWorkerSelection {
            target: MessageTarget::discussion_agent(AgentType::Ollama),
            model: None,
            profile_id: None,
        },
        "recover historical native rework",
        &backend_actor(),
    )
    .unwrap();
    assert_eq!(reassigned.attempt_no, 1);
    assert!(
        crate::db::worker_reviews::get_review(&conn, &execution.id, 1)
            .unwrap()
            .is_none()
    );
    let events = list_execution_events(&conn, &execution.id).unwrap();
    let repair = events
        .iter()
        .find(|event| event.action == "rework_attempt_repaired")
        .expect("the repair is auditable");
    assert_eq!(repair.changes["from_attempt"], 0);
    assert_eq!(repair.changes["to_attempt"], 1);
}

#[test]
fn reassignment_repairs_an_unadvanced_approved_attempt_after_validation_sendback() {
    let conn = setup();
    seed_task(&conn, "t-reassign-approved-rework", 1104);
    let execution = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t-reassign-approved-rework", DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution;
    for status in [
        TaskExecutionStatus::Provisioning,
        TaskExecutionStatus::Working,
    ] {
        transition_execution(
            &conn,
            &execution.id,
            status,
            &backend_actor(),
            serde_json::json!({}),
        )
        .unwrap();
    }
    crate::db::worker_reviews::upsert_review(
        &conn,
        &execution.id,
        0,
        "approve",
        r#"{"version":"1","decision":"approve"}"#,
    )
    .unwrap();

    let reassigned = reassign_execution_worker(
        &conn,
        &execution.id,
        &CampaignWorkerSelection {
            target: MessageTarget::discussion_agent(AgentType::Ollama),
            model: None,
            profile_id: None,
        },
        "recover validation sendback",
        &backend_actor(),
    )
    .unwrap();
    assert_eq!(reassigned.attempt_no, 1);
    assert!(
        crate::db::worker_reviews::get_review(&conn, &execution.id, 1)
            .unwrap()
            .is_none()
    );
    let events = list_execution_events(&conn, &execution.id).unwrap();
    let repair = events
        .iter()
        .find(|event| event.action == "rework_attempt_repaired")
        .expect("the approve-path repair is auditable");
    assert_eq!(repair.changes["from_attempt"], 0);
    assert_eq!(repair.changes["to_attempt"], 1);
    assert_eq!(
        repair.changes["reason"],
        "reviewed_attempt_was_not_advanced"
    );
}

#[test]
fn timeout_scan_reports_activity_total_review_and_human_wait_distinctly() {
    let conn = setup();
    let past = "2020-01-01T00:00:00Z";
    for (idx, status) in ["Working", "Working", "AwaitingReview", "Escalated"]
        .into_iter()
        .enumerate()
    {
        let task = format!("t-timeout-{idx}");
        seed_task(&conn, &task, 1200 + idx as i64);
        let execution = launch_single_task(
            &conn,
            &LaunchSingleTaskInput::new(&task, DISC),
            &backend_actor(),
        )
        .unwrap()
        .execution;
        conn.execute(
            "UPDATE task_executions SET status = ?2 WHERE id = ?1",
            params![execution.id, status],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_execution_recovery (task_execution_id, recovery_action, \
                 recovery_reason, last_activity_at, total_deadline_at, activity_deadline_at, \
                 review_deadline_at, human_wait_started_at, assignment_generation, pending, updated_at) \
             VALUES (?1, 'resume_worker', 'clock fixture', ?2, ?3, ?4, ?5, ?6, 0, 0, ?2)",
            params![
                execution.id,
                past,
                if idx == 1 { Some(past) } else { None },
                if idx == 0 { Some(past) } else { None },
                if idx == 2 { Some(past) } else { None },
                if idx == 3 { Some(past) } else { None },
            ],
        )
        .unwrap();
        if idx == 3 {
            let run_id = execution.orchestration_run_id;
            set_resilience_policy(
                &conn,
                &run_id,
                &OrchestrationResiliencePolicy {
                    human_wait_timeout_secs: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        }
    }
    let expired = expired_execution_timeouts(&conn, Utc::now()).unwrap();
    let kinds: Vec<ExecutionTimeoutKind> = expired.into_iter().map(|(_, kind)| kind).collect();
    assert!(kinds.contains(&ExecutionTimeoutKind::Activity));
    assert!(kinds.contains(&ExecutionTimeoutKind::TotalDuration));
    assert!(kinds.contains(&ExecutionTimeoutKind::ReviewWait));
    assert!(kinds.contains(&ExecutionTimeoutKind::HumanWait));
}

#[test]
fn activity_watchdog_redispatches_once_then_escalates_execution() {
    let conn = setup();
    seed_task(&conn, "t-watchdog", 1210);
    let execution = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t-watchdog", DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution;
    conn.execute(
        "INSERT INTO discussions (id, title, agent, created_at, updated_at)
         VALUES ('d-watchdog-child', 'Worker', 'Codex', ?1, ?1)",
        [Utc::now().to_rfc3339()],
    )
    .unwrap();
    let trigger = crate::models::DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: "watchdog-trigger".into(),
        role: crate::models::MessageRole::User,
        channel: crate::models::MessageChannel::Main,
        content: "work".into(),
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };
    crate::db::discussions::insert_message(&conn, "d-watchdog-child", &trigger).unwrap();
    crate::db::agent_dispatch::enqueue(
        &conn,
        crate::db::agent_dispatch::NewAgentDispatchJob {
            id: "watchdog-dispatch",
            discussion_id: "d-watchdog-child",
            trigger_message_id: "watchdog-trigger",
            trigger_sort_order: 0,
            dedupe_key: "watchdog:dispatch",
            agent_override: Some(&AgentType::Codex),
            chain_prompt_ids: &[],
            batch_item: None,
            group_id: None,
            group_concurrency_limit: None,
        },
    )
    .unwrap();
    crate::db::agent_dispatch::claim(&conn, "watchdog-dispatch").unwrap();
    crate::db::agent_dispatch::mark_agent_started(&conn, "watchdog-dispatch").unwrap();
    conn.execute(
        "UPDATE task_executions
         SET status = 'Working', sub_discussion_id = 'd-watchdog-child',
             dispatch_job_id = 'watchdog-dispatch' WHERE id = ?1",
        [&execution.id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO task_execution_recovery (
             task_execution_id, recovery_action, recovery_reason, last_activity_at,
             activity_deadline_at, pending, updated_at
         ) VALUES (?1, 'resume_worker', 'fixture', '2020-01-01T00:00:00Z',
                   '2020-01-01T00:00:00Z', 0, '2020-01-01T00:00:00Z')",
        [&execution.id],
    )
    .unwrap();

    assert!(apply_execution_timeout(&conn, &execution.id, ExecutionTimeoutKind::Activity).unwrap());
    assert_eq!(
        get_task_execution(&conn, &execution.id)
            .unwrap()
            .unwrap()
            .status,
        TaskExecutionStatus::Working
    );
    let first = crate::db::agent_dispatch::get(&conn, "watchdog-dispatch")
        .unwrap()
        .unwrap();
    assert_eq!(
        first.status,
        crate::db::agent_dispatch::DispatchStatus::Pending
    );
    assert_eq!(first.watchdog_redispatches, 1);
    assert_eq!(
        get_execution_recovery(&conn, &execution.id)
            .unwrap()
            .unwrap()
            .watchdog_redispatches,
        1
    );

    conn.execute(
        "UPDATE agent_dispatch_jobs SET available_at = ?1 WHERE id = 'watchdog-dispatch'",
        [Utc::now().to_rfc3339()],
    )
    .unwrap();
    crate::db::agent_dispatch::claim(&conn, "watchdog-dispatch").unwrap();
    crate::db::agent_dispatch::mark_agent_started(&conn, "watchdog-dispatch").unwrap();
    assert!(apply_execution_timeout(&conn, &execution.id, ExecutionTimeoutKind::Activity).unwrap());
    assert_eq!(
        get_task_execution(&conn, &execution.id)
            .unwrap()
            .unwrap()
            .status,
        TaskExecutionStatus::Escalated
    );
    assert_eq!(
        crate::db::agent_dispatch::get(&conn, "watchdog-dispatch")
            .unwrap()
            .unwrap()
            .failure_kind
            .as_deref(),
        Some("dispatch_stalled")
    );
}

#[test]
fn hard_quota_creates_an_immediate_human_wait_checkpoint() {
    let conn = setup();
    seed_task(&conn, "t-quota", 1211);
    let execution = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t-quota", DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution;
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Provisioning,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Working,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO discussions (id, title, agent, created_at, updated_at)
         VALUES ('d-quota-child', 'Quota worker', 'Codex', ?1, ?1)",
        [&now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages
         (id, discussion_id, role, content, timestamp, sort_order, received_at)
         VALUES ('quota-trigger', 'd-quota-child', 'User', 'work', ?1, 1, ?1)",
        [&now],
    )
    .unwrap();
    crate::db::agent_dispatch::enqueue_for_latest_user(
        &conn,
        crate::db::agent_dispatch::NewLatestUserDispatch {
            id: "quota-dispatch",
            discussion_id: "d-quota-child",
            dedupe_key: "quota:dispatch",
            agent_override: Some(&AgentType::Codex),
            chain_prompt_ids: &[],
            batch_item: None,
            group_id: None,
            group_concurrency_limit: None,
        },
    )
    .unwrap();
    conn.execute(
        "UPDATE task_executions
         SET sub_discussion_id = 'd-quota-child', dispatch_job_id = 'quota-dispatch'
         WHERE id = ?1",
        [&execution.id],
    )
    .unwrap();

    let escalated = escalate_execution_for_dispatch_quota(&conn, "quota-dispatch", "Codex")
        .unwrap()
        .expect("the dispatch belongs to an execution");
    assert_eq!(escalated.0, execution.id);
    assert_eq!(escalated.1, DISC);
    assert_eq!(
        get_task_execution(&conn, &execution.id)
            .unwrap()
            .unwrap()
            .status,
        TaskExecutionStatus::Escalated
    );
    let recovery = get_execution_recovery(&conn, &execution.id)
        .unwrap()
        .expect("quota escalation persists a human checkpoint");
    assert_eq!(
        recovery.recovery_action,
        ExecutionRecoveryAction::AwaitHuman
    );
    assert_eq!(recovery.recovery_reason, "quota_exhausted:Codex");
    assert!(recovery.human_wait_started_at.is_some());
    assert!(!recovery.pending);
}

#[test]
fn campaign_candidates_preserve_plan_order_and_explain_every_refusal() {
    let conn = setup();
    seed_plan_task(&conn, "t-first", 1001, 0, "active");
    seed_plan_task(&conn, "t-second", 1002, 1, "active");
    seed_plan_task(&conn, "t-later", 1003, 0, "later");
    let run = campaign(&conn, 1, 1);

    let initial = campaign_task_candidates(&conn, &run.id, None).unwrap();
    assert!(
        initial[0].launchable,
        "the first valid plan task owns the slot"
    );
    assert!(!initial[1].launchable);
    assert_eq!(initial[1].reasons[0].code, "plan_order");
    assert!(!initial[2].launchable);
    assert!(initial[2]
        .reasons
        .iter()
        .any(|reason| reason.code == "later"));

    let selection = run.default_worker.clone().unwrap();
    let mut input = LaunchSingleTaskInput::new("t-first", DISC);
    input.project_id = Some("p1".into());
    input.worker_target_kind = Some(selection.target.kind);
    input.worker_agent_type = Some(agent_type_to_db(&selection.target.agent_type));
    input.worker_model = selection.model.clone();
    input.worker_profile_id = selection.profile_id.clone();
    let launched =
        launch_task_in_run(&conn, &run.id, &input, &selection, &backend_actor()).unwrap();
    assert_eq!(launched.execution.worker_model.as_deref(), Some("gpt-test"));
    assert_eq!(
        launched.execution.worker_profile_id.as_deref(),
        Some("profile-test")
    );

    let capped = campaign_task_candidates(&conn, &run.id, None).unwrap();
    let second = capped
        .iter()
        .find(|item| item.task.id == "t-second")
        .unwrap();
    assert!(!second.launchable);
    assert!(second
        .reasons
        .iter()
        .any(|reason| reason.code == "concurrency_limit"));
}

#[test]
fn cli_campaign_limit_is_cross_run_within_the_principal_room() {
    let conn = setup();
    seed_session(&conn, 77, "Codex", "cli-77");
    seed_plan_task(&conn, "t-cli-a", 1011, 0, "active");
    seed_plan_task(&conn, "t-cli-b", 1012, 1, "active");
    let run_a = campaign(&conn, 2, 1);
    let run_b = campaign(&conn, 2, 1);
    let selection = CampaignWorkerSelection {
        target: MessageTarget::cli(AgentType::Codex, 77),
        model: None,
        profile_id: None,
    };
    let mut first = LaunchSingleTaskInput::new("t-cli-a", DISC);
    first.project_id = Some("p1".into());
    first.worker_target_kind = Some(MessageTargetKind::Cli);
    first.worker_cli_session_id = Some(77);
    first.worker_agent_type = Some("Codex".into());
    launch_task_in_run(&conn, &run_a.id, &first, &selection, &backend_actor()).unwrap();

    let candidates = campaign_task_candidates(&conn, &run_b.id, Some(&selection)).unwrap();
    let second = candidates
        .iter()
        .find(|item| item.task.id == "t-cli-b")
        .unwrap();
    assert!(!second.launchable);
    assert!(second
        .reasons
        .iter()
        .any(|reason| reason.code == "cli_concurrency_limit"));
}

#[test]
fn campaign_pause_resume_and_terminal_state_are_durable_and_audited() {
    let conn = setup();
    seed_plan_task(&conn, "t-control", 1021, 0, "active");
    let run = campaign(&conn, 1, 1);

    let paused = set_orchestration_control_state(
        &conn,
        &run.id,
        OrchestrationControlState::Paused,
        Some("operator pause"),
        &backend_actor(),
    )
    .unwrap();
    assert_eq!(paused.control_state, OrchestrationControlState::Paused);
    assert!(campaign_task_candidates(&conn, &run.id, None)
        .unwrap()
        .iter()
        .all(|candidate| !candidate.launchable));

    let resumed = set_orchestration_control_state(
        &conn,
        &run.id,
        OrchestrationControlState::Running,
        None,
        &backend_actor(),
    )
    .unwrap();
    assert_eq!(resumed.control_state, OrchestrationControlState::Running);
    assert!(set_orchestration_control_state(
        &conn,
        &run.id,
        OrchestrationControlState::Completed,
        Some("too early"),
        &backend_actor(),
    )
    .is_err());
    conn.execute(
        "UPDATE planning_tasks SET status = 'done' WHERE id = 't-control'",
        [],
    )
    .unwrap();
    let completed = set_orchestration_control_state(
        &conn,
        &run.id,
        OrchestrationControlState::Completed,
        Some("done"),
        &backend_actor(),
    )
    .unwrap();
    assert_eq!(
        completed.control_state,
        OrchestrationControlState::Completed
    );
    assert!(set_orchestration_control_state(
        &conn,
        &run.id,
        OrchestrationControlState::Running,
        None,
        &backend_actor(),
    )
    .is_err());
    let events: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM orchestration_run_events WHERE orchestration_run_id = ?1",
            [&run.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(events, 3);
}

#[test]
fn campaign_policy_rejects_zero_limits_and_waiting_cli_is_not_human_attention() {
    let conn = setup();
    let mut invalid = OrchestrationRunInput::single_task(DISC);
    invalid.kind = OrchestrationRunKind::Campaign;
    invalid.max_concurrent_executions = 0;
    assert!(create_orchestration_run(&conn, &invalid).is_err());
    invalid.max_concurrent_executions = 1;
    invalid.timeout_secs = Some(0);
    assert!(create_orchestration_run(&conn, &invalid).is_err());

    seed_session(&conn, 78, "Codex", "cli-78");
    seed_plan_task(&conn, "t-cli-wait", 1022, 0, "active");
    let run = campaign(&conn, 2, 1);
    let selection = CampaignWorkerSelection {
        target: MessageTarget::cli(AgentType::Codex, 78),
        model: None,
        profile_id: None,
    };
    let mut input = LaunchSingleTaskInput::new("t-cli-wait", DISC);
    input.project_id = Some("p1".into());
    input.worker_target_kind = Some(MessageTargetKind::Cli);
    input.worker_cli_session_id = Some(78);
    input.worker_agent_type = Some("Codex".into());
    let execution = launch_task_in_run(&conn, &run.id, &input, &selection, &backend_actor())
        .unwrap()
        .execution;
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Provisioning,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    block_execution(
        &conn,
        &execution.id,
        &backend_actor(),
        "waiting for the exact CLI",
        Some(BlockedReasonCode::AwaitingWorkerAcceptance),
    )
    .unwrap();
    let attention = principal_attention(&conn, &run.id).unwrap();
    assert_eq!(attention.awaiting_human, 0);
    assert_eq!(attention.active_executions, 1);
    assert!(attention
        .actions
        .iter()
        .any(|action| action.contains("wait for active")));
}

#[test]
fn escalated_campaign_holds_the_principal_and_terminal_child_notifies_parent() {
    let conn = setup();
    seed_plan_task(&conn, "t-gate", 1031, 0, "active");
    let run = campaign(&conn, 1, 1);
    let selection = run.default_worker.clone().unwrap();
    let mut input = LaunchSingleTaskInput::new("t-gate", DISC);
    input.project_id = Some("p1".into());
    input.worker_target_kind = Some(selection.target.kind);
    input.worker_agent_type = Some(agent_type_to_db(&selection.target.agent_type));
    let launched =
        launch_task_in_run(&conn, &run.id, &input, &selection, &backend_actor()).unwrap();
    assert!(transition_execution(
        &conn,
        &launched.execution.id,
        TaskExecutionStatus::Escalated,
        &backend_actor(),
        serde_json::json!({ "reason": "review budget" }),
    )
    .unwrap());
    assert_eq!(
        get_orchestration_run(&conn, &run.id)
            .unwrap()
            .unwrap()
            .control_state,
        OrchestrationControlState::AwaitingHuman
    );
    assert!(campaign_task_candidates(&conn, &run.id, None)
        .unwrap()
        .iter()
        .all(|candidate| !candidate.launchable));

    assert!(transition_execution(
        &conn,
        &launched.execution.id,
        TaskExecutionStatus::Working,
        &backend_actor(),
        serde_json::json!({ "human_decision": "resume" }),
    )
    .unwrap());
    assert_eq!(
        get_orchestration_run(&conn, &run.id)
            .unwrap()
            .unwrap()
            .control_state,
        OrchestrationControlState::Running
    );
    assert!(transition_execution(
        &conn,
        &launched.execution.id,
        TaskExecutionStatus::Cancelled,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap());
    let notices: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1 AND id LIKE 'orch-principal-terminal:%'",
            [DISC],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        notices, 1,
        "terminal child event wakes the principal durably"
    );
}

#[test]
fn failed_child_stops_campaign_until_a_human_decides() {
    let conn = setup();
    seed_plan_task(&conn, "t-failed", 1032, 0, "active");
    let run = campaign(&conn, 1, 1);
    let selection = run.default_worker.clone().unwrap();
    let mut input = LaunchSingleTaskInput::new("t-failed", DISC);
    input.project_id = Some("p1".into());
    input.worker_target_kind = Some(selection.target.kind);
    input.worker_agent_type = Some(agent_type_to_db(&selection.target.agent_type));
    let execution = launch_task_in_run(&conn, &run.id, &input, &selection, &backend_actor())
        .unwrap()
        .execution;
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Provisioning,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    transition_execution(
        &conn,
        &execution.id,
        TaskExecutionStatus::Failed,
        &backend_actor(),
        serde_json::json!({ "reason": "worker crashed" }),
    )
    .unwrap();
    let stopped = get_orchestration_run(&conn, &run.id).unwrap().unwrap();
    assert_eq!(
        stopped.control_state,
        OrchestrationControlState::AwaitingHuman
    );
    assert!(campaign_task_candidates(&conn, &run.id, None)
        .unwrap()
        .iter()
        .all(|candidate| !candidate.launchable));
}

#[test]
fn task_execution_status_terminal_set() {
    use TaskExecutionStatus::*;
    for s in [Done, Failed, Cancelled] {
        assert!(s.is_terminal(), "{s:?} must be terminal");
    }
    for s in [
        Pending,
        Provisioning,
        Blocked,
        Working,
        AwaitingReview,
        Approved,
        ChangesRequested,
        Integrating,
        Validating,
        Applying,
        Escalated,
        Interrupted,
    ] {
        assert!(!s.is_terminal(), "{s:?} must not be terminal");
    }
}

#[test]
fn can_transition_matches_adr_shape() {
    use TaskExecutionStatus::*;
    // Happy path skeleton.
    assert!(Pending.can_transition_to(Provisioning));
    assert!(Provisioning.can_transition_to(Working));
    assert!(Working.can_transition_to(AwaitingReview));
    assert!(AwaitingReview.can_transition_to(Approved));
    assert!(Approved.can_transition_to(Integrating));
    assert!(Integrating.can_transition_to(Validating));
    assert!(Validating.can_transition_to(Applying));
    assert!(Applying.can_transition_to(Done));
    // Review loop + drift.
    assert!(AwaitingReview.can_transition_to(ChangesRequested));
    assert!(ChangesRequested.can_transition_to(Working));
    assert!(Applying.can_transition_to(Integrating));
    // Any non-terminal may be interrupted; a terminal one may not.
    assert!(Working.can_transition_to(Interrupted));
    assert!(!Done.can_transition_to(Interrupted));
    // ADR §3 l.278 — any non-terminal may be Escalated (budget/hard-fail), not
    // only Working/AwaitingReview/ChangesRequested.
    for s in [
        Pending,
        Provisioning,
        Blocked,
        Approved,
        Integrating,
        Validating,
        Applying,
    ] {
        assert!(s.can_transition_to(Escalated), "{s:?} must reach Escalated");
    }
    assert!(!Done.can_transition_to(Escalated));
    // An interrupted Blocked re-enters the hold (structurally).
    assert!(Interrupted.can_transition_to(Blocked));
    // The coarse Interrupted arm reaches EVERY non-terminal target — including the
    // review states the diagram's partial list used to strand (Codex counter-
    // example: an interrupted AwaitingReview was stuck outside Cancel/Escalate).
    for s in [
        Pending,
        Provisioning,
        Blocked,
        Working,
        AwaitingReview,
        Approved,
        ChangesRequested,
        Integrating,
        Validating,
        Applying,
    ] {
        assert!(
            Interrupted.can_transition_to(s),
            "Interrupted must reach {s:?}"
        );
    }
    // ...but never a terminal directly (the resume lands on a non-terminal first).
    for s in [Done, Failed] {
        assert!(
            !Interrupted.can_transition_to(s),
            "Interrupted must not reach terminal {s:?}"
        );
    }
    // Illegal shortcuts.
    assert!(!AwaitingReview.can_transition_to(Integrating));
    assert!(!Pending.can_transition_to(Working));
    assert!(!Working.can_transition_to(Done));
    // Terminal is sticky.
    assert!(!Done.can_transition_to(Working));
    assert!(!Cancelled.can_transition_to(Working));
    assert!(!Failed.can_transition_to(Working));
    // No self-loop.
    assert!(!Working.can_transition_to(Working));
}

#[test]
fn transition_matrix_locks_all_fifteen_states() {
    use TaskExecutionStatus::*;
    let all = [
        Pending,
        Provisioning,
        Blocked,
        Working,
        AwaitingReview,
        Approved,
        ChangesRequested,
        Integrating,
        Validating,
        Applying,
        Escalated,
        Interrupted,
        Done,
        Failed,
        Cancelled,
    ];
    assert_eq!(all.len(), 15, "the ADR pins exactly 15 states");
    // Every ordered pair is checked against the independent ADR transcription, so
    // any drift in either the impl or the table is caught (225 pairs).
    for &from in &all {
        for &to in &all {
            assert_eq!(
                from.can_transition_to(to),
                adr_legal(from, to),
                "matrix mismatch {from:?} -> {to:?}"
            );
        }
    }
}

#[test]
fn launch_creates_single_task_run_and_pending_execution() {
    let conn = setup();
    seed_task(&conn, "t1", 1);
    let mut input = LaunchSingleTaskInput::new("t1", DISC);
    input.validations = vec![ValidationSpec {
        command: "cargo fmt --check".into(),
        quick_exec_id: None,
        timeout_secs: Some(120),
    }];
    let out = launch_single_task(&conn, &input, &backend_actor()).unwrap();

    assert!(!out.deduplicated);
    assert_eq!(out.execution.status, TaskExecutionStatus::Pending);
    assert_eq!(out.execution.orchestration_run_id, out.run.id);
    assert_eq!(out.run.kind.as_str(), "single_task");
    assert_eq!(out.run.validations.len(), 1);
    assert_eq!(out.run.validations[0].command, "cargo fmt --check");
    assert_eq!(out.run.validations[0].timeout_secs, Some(120));

    // The 'created' event was journaled with the backend actor.
    let events = list_execution_events(&conn, &out.execution.id).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, "created");
    assert_eq!(events[0].actor_kind, PlanningActorKind::Backend);
    assert_eq!(events[0].to_status, Some(TaskExecutionStatus::Pending));
}

#[test]
fn launch_persists_and_resolves_the_prelocalized_worker_scope_from_the_child_room() {
    let conn = setup();
    seed_task(&conn, "t-scope", 2);
    let scope = TaskWorkerScope::PrelocalizedEdit {
        path: "backend/src/lib.rs".into(),
        start_line: 40,
        end_line: 44,
    };
    let mut input = LaunchSingleTaskInput::new("t-scope", DISC);
    input.worker_scope = Some(scope.clone());
    input.worker_dod_ids = Some(vec!["dod-a".into(), "dod-b".into()]);
    let out = launch_single_task(&conn, &input, &backend_actor()).unwrap();
    assert_eq!(out.execution.worker_scope, Some(scope.clone()));
    assert_eq!(
        out.execution.worker_dod_ids,
        Some(vec!["dod-a".into(), "dod-b".into()])
    );

    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at) \
         VALUES ('d-scope-worker', 'Scoped worker', '2026-01-01T00:00:00Z', \
                 '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    set_execution_sub_discussion(&conn, &out.execution.id, "d-scope-worker").unwrap();
    let resolved = get_execution_for_sub_discussion(&conn, "d-scope-worker")
        .unwrap()
        .expect("child room resolves its execution");
    assert_eq!(resolved.worker_scope, Some(scope));
    assert_eq!(
        resolved.worker_dod_ids,
        Some(vec!["dod-a".into(), "dod-b".into()])
    );
}

#[test]
fn launch_is_idempotent_on_key() {
    let conn = setup();
    seed_task(&conn, "t1", 1);
    let mut input = LaunchSingleTaskInput::new("t1", DISC);
    input.idempotency_key = Some("launch-k".into());

    let first = launch_single_task(&conn, &input, &backend_actor()).unwrap();
    let second = launch_single_task(&conn, &input, &backend_actor()).unwrap();

    assert!(!first.deduplicated);
    assert!(second.deduplicated);
    assert_eq!(first.execution.id, second.execution.id);
    assert_eq!(first.run.id, second.run.id);

    let run_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM orchestration_runs", [], |r| r.get(0))
        .unwrap();
    let exec_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM task_executions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(run_count, 1, "no duplicate run on idempotent replay");
    assert_eq!(exec_count, 1, "no duplicate execution on idempotent replay");
}

#[test]
fn one_active_execution_per_task_is_enforced() {
    let conn = setup();
    seed_task(&conn, "t1", 1);
    let first = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t1", DISC),
        &backend_actor(),
    )
    .unwrap();

    // A second *active* launch (different/no key) is refused by the partial
    // unique index; the failed savepoint leaves no orphan run.
    let second = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t1", DISC),
        &backend_actor(),
    );
    assert!(second.is_err(), "a second active execution must be refused");
    let run_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM orchestration_runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        run_count, 1,
        "the refused launch must not leave an orphan run"
    );

    // Once the first is terminal, a fresh execution is allowed.
    assert!(transition_execution(
        &conn,
        &first.execution.id,
        TaskExecutionStatus::Cancelled,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap());
    assert!(get_active_execution_for_task(&conn, "t1")
        .unwrap()
        .is_none());
    let third = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t1", DISC),
        &backend_actor(),
    );
    assert!(
        third.is_ok(),
        "a new execution is allowed after the previous one ends"
    );
}

#[test]
fn transition_guards_illegal_and_journals_legal() {
    let conn = setup();
    seed_task(&conn, "t1", 1);
    let out = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t1", DISC),
        &backend_actor(),
    )
    .unwrap();
    let id = out.execution.id;

    // Illegal shortcut → Err, status unchanged.
    assert!(transition_execution(
        &conn,
        &id,
        TaskExecutionStatus::Done,
        &backend_actor(),
        serde_json::json!({}),
    )
    .is_err());
    assert_eq!(
        get_task_execution(&conn, &id).unwrap().unwrap().status,
        TaskExecutionStatus::Pending
    );

    // Legal transition → Ok(true), status moved, event recorded.
    assert!(transition_execution(
        &conn,
        &id,
        TaskExecutionStatus::Provisioning,
        &backend_actor(),
        serde_json::json!({"note": "claimed"}),
    )
    .unwrap());
    assert_eq!(
        get_task_execution(&conn, &id).unwrap().unwrap().status,
        TaskExecutionStatus::Provisioning
    );
    let events = list_execution_events(&conn, &id).unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.action, "transition");
    assert_eq!(last.from_status, Some(TaskExecutionStatus::Pending));
    assert_eq!(last.to_status, Some(TaskExecutionStatus::Provisioning));
}

#[test]
fn terminal_is_sticky_and_sets_finished_at() {
    let conn = setup();
    seed_task(&conn, "t1", 1);
    let out = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t1", DISC),
        &backend_actor(),
    )
    .unwrap();
    let id = out.execution.id;

    assert!(transition_execution(
        &conn,
        &id,
        TaskExecutionStatus::Cancelled,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap());
    let exec = get_task_execution(&conn, &id).unwrap().unwrap();
    assert_eq!(exec.status, TaskExecutionStatus::Cancelled);
    assert!(
        exec.finished_at.is_some(),
        "terminal must stamp finished_at"
    );

    // Any further transition out of a terminal state is a contract violation.
    assert!(transition_execution(
        &conn,
        &id,
        TaskExecutionStatus::Working,
        &backend_actor(),
        serde_json::json!({}),
    )
    .is_err());
}

#[test]
fn claim_status_is_a_compare_and_swap() {
    let conn = setup();
    seed_task(&conn, "t1", 1);
    let out = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t1", DISC),
        &backend_actor(),
    )
    .unwrap();
    let id = out.execution.id;
    let now = "2026-01-02T00:00:00Z";

    // First claim from the real status succeeds; a second claim from the now
    // stale status fails (the row already moved).
    assert!(run_state::claim_status(
        &conn,
        "task_executions",
        &id,
        "Pending",
        "Provisioning",
        now
    )
    .unwrap());
    assert!(
        !run_state::claim_status(&conn, "task_executions", &id, "Pending", "Working", now).unwrap()
    );
}

#[test]
fn reconcile_interrupts_journals_and_preserves_origin() {
    use TaskExecutionStatus::*;
    let conn = setup();
    seed_task(&conn, "t1", 1);
    seed_task(&conn, "t2", 2);

    // a: Pending (never advanced). b: Cancelled (terminal). c: Provisioning.
    // d: Working — so the interrupt origin is a non-trivial mid-flight state.
    let a = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t1", DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution
    .id;
    let b = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t2", DISC),
        &backend_actor(),
    )
    .unwrap()
    .execution
    .id;
    transition_execution(
        &conn,
        &b,
        Cancelled,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap();
    let c = launch_and_drive(&conn, "t3", 3, &[Provisioning]);
    let d = launch_and_drive(&conn, "t4", 4, &[Provisioning, Working]);

    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at) \
         VALUES ('disc-reconcile-child', 'Child', 'now', 'now')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order) \
         VALUES ('msg-reconcile', 'disc-reconcile-child', 'User', 'resume me', \
                 '2026-01-01T00:00:00Z', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO agent_dispatch_jobs (id, discussion_id, trigger_message_id, \
             trigger_sort_order, dedupe_key, chain_prompt_ids_json, status, available_at, \
             created_at, updated_at) VALUES ('dispatch-reconcile', 'disc-reconcile-child', \
             'msg-reconcile', 1, 'reconcile-dedupe', '[]', 'Pending', \
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE task_executions SET sub_discussion_id = 'disc-reconcile-child', \
                dispatch_job_id = 'dispatch-reconcile' WHERE id = ?1",
        params![&d],
    )
    .unwrap();

    let flipped = reconcile_stale_task_executions(&conn).unwrap();
    assert!(flipped.contains(&a));
    assert!(flipped.contains(&c));
    assert!(flipped.contains(&d));
    assert!(
        !flipped.contains(&b),
        "a terminal row must not be reconciled"
    );

    // Non-destructive + journaled (DoD-3): each flipped row is Interrupted, records
    // its exact origin for the §4bis resume, and carries a System-attributed event
    // `from → Interrupted` — never a bulk UPDATE that erases both.
    for (id, origin) in [(&a, Pending), (&c, Provisioning), (&d, Working)] {
        let exec = get_task_execution(&conn, id).unwrap().unwrap();
        assert_eq!(exec.status, Interrupted, "row must land in Interrupted");
        assert_eq!(
            exec.interrupted_from_status,
            Some(origin),
            "origin preserved for deterministic resume"
        );
        let last = list_execution_events(&conn, id)
            .unwrap()
            .into_iter()
            .last()
            .unwrap();
        assert_eq!(last.to_status, Some(Interrupted));
        assert_eq!(
            last.from_status,
            Some(origin),
            "the journaled event keeps the origin"
        );
        assert_eq!(
            last.actor_kind,
            PlanningActorKind::System,
            "boot reconcile is attributed to System, not a spoofable chat identity"
        );
    }
    assert_eq!(
        get_task_execution(&conn, &b).unwrap().unwrap().status,
        Cancelled,
        "a terminal row is untouched"
    );
    let dispatch_status: String = conn
        .query_row(
            "SELECT status FROM agent_dispatch_jobs WHERE id = 'dispatch-reconcile'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        dispatch_status, "Cancelled",
        "boot reconciliation quiesces a recovered response before classification"
    );
}

#[test]
fn blocked_resume_honours_origin_checkpoint() {
    use TaskExecutionStatus::*;
    let conn = setup();

    // Origin = Provisioning: Provisioning → Blocked. The resume is legal ONLY back
    // to Provisioning; Applying is refused even though it is structurally allowed.
    let a = launch_and_drive(&conn, "t1", 1, &[Provisioning, Blocked]);
    assert_eq!(
        get_task_execution(&conn, &a)
            .unwrap()
            .unwrap()
            .blocked_from_status,
        Some(Provisioning)
    );
    assert!(
        transition_execution(&conn, &a, Applying, &backend_actor(), serde_json::json!({})).is_err(),
        "a Provisioning-origin Blocked must not resume Applying"
    );
    assert!(transition_execution(
        &conn,
        &a,
        Provisioning,
        &backend_actor(),
        serde_json::json!({})
    )
    .unwrap());
    // Checkpoint consumed once resumed out of the hold.
    assert_eq!(
        get_task_execution(&conn, &a)
            .unwrap()
            .unwrap()
            .blocked_from_status,
        None
    );

    // Origin = Applying (§4bis dirty target): the mirror case — resume ONLY to
    // Applying, never Provisioning.
    let b = launch_and_drive(
        &conn,
        "t2",
        2,
        &[
            Provisioning,
            Working,
            AwaitingReview,
            Approved,
            Integrating,
            Validating,
            Applying,
            Blocked,
        ],
    );
    assert_eq!(
        get_task_execution(&conn, &b)
            .unwrap()
            .unwrap()
            .blocked_from_status,
        Some(Applying)
    );
    assert!(
        transition_execution(
            &conn,
            &b,
            Provisioning,
            &backend_actor(),
            serde_json::json!({})
        )
        .is_err(),
        "an Applying-origin Blocked must not resume Provisioning"
    );
    assert!(
        transition_execution(&conn, &b, Applying, &backend_actor(), serde_json::json!({})).unwrap()
    );
}

#[test]
fn interrupted_resume_honours_origin_checkpoint() {
    use TaskExecutionStatus::*;
    let conn = setup();

    // Origin = Provisioning: interrupt then resume. Only Provisioning is legal;
    // Applying is refused (a Provisioning interrupt cannot jump into the saga).
    let a = launch_and_drive(&conn, "t1", 1, &[Provisioning, Interrupted]);
    assert_eq!(
        get_task_execution(&conn, &a)
            .unwrap()
            .unwrap()
            .interrupted_from_status,
        Some(Provisioning)
    );
    assert!(
        transition_execution(&conn, &a, Applying, &backend_actor(), serde_json::json!({})).is_err(),
        "a Provisioning interrupt must not resume Applying"
    );
    assert!(transition_execution(
        &conn,
        &a,
        Provisioning,
        &backend_actor(),
        serde_json::json!({})
    )
    .unwrap());
    assert_eq!(
        get_task_execution(&conn, &a)
            .unwrap()
            .unwrap()
            .interrupted_from_status,
        None
    );

    // Origin = Applying: the saga may replay Applying or redirect to Integrating
    // on drift — both are reachable from the Applying origin.
    let b = launch_and_drive(
        &conn,
        "t2",
        2,
        &[
            Provisioning,
            Working,
            AwaitingReview,
            Approved,
            Integrating,
            Validating,
            Applying,
            Interrupted,
        ],
    );
    assert_eq!(
        get_task_execution(&conn, &b)
            .unwrap()
            .unwrap()
            .interrupted_from_status,
        Some(Applying)
    );
    assert!(transition_execution(
        &conn,
        &b,
        Integrating,
        &backend_actor(),
        serde_json::json!({})
    )
    .unwrap());

    // Chain Applying → Blocked → Interrupted: the interrupt cannot see through the
    // block to Applying; it re-enters Blocked (keeping the Applying deblock
    // target), and only then does the deblock reach Applying.
    let c = launch_and_drive(
        &conn,
        "t3",
        3,
        &[
            Provisioning,
            Working,
            AwaitingReview,
            Approved,
            Integrating,
            Validating,
            Applying,
            Blocked,
            Interrupted,
        ],
    );
    assert!(
        transition_execution(&conn, &c, Applying, &backend_actor(), serde_json::json!({})).is_err(),
        "an interrupted Blocked must not bypass the block to Applying"
    );
    assert!(
        transition_execution(&conn, &c, Blocked, &backend_actor(), serde_json::json!({})).unwrap()
    );
    let exec = get_task_execution(&conn, &c).unwrap().unwrap();
    assert_eq!(exec.status, Blocked);
    assert_eq!(
        exec.blocked_from_status,
        Some(Applying),
        "blocked_from is preserved across the interrupt"
    );
    assert_eq!(
        exec.interrupted_from_status, None,
        "interrupted_from is cleared on resume"
    );
    assert!(
        transition_execution(&conn, &c, Applying, &backend_actor(), serde_json::json!({})).unwrap()
    );
}

#[test]
fn interrupted_resume_gate_admits_every_non_terminal_origin() {
    use TaskExecutionStatus::*;
    // For EVERY non-terminal origin: (a) it can be interrupted; (b) the coarse
    // Interrupted arm no longer pre-filters — it reaches the origin structurally;
    // (c) the checkpoint helper allows the exact return; (d) it allows a legal
    // business successor of the origin when one exists (the saga may redirect on
    // drift); (e) it refuses an unrelated target. `successor = None` marks an
    // origin whose only interrupt-resume is its exact return: a Blocked re-enters
    // its own hold and reaches its `blocked_from` target through that hold, never
    // "through" the interrupt.
    let cases: &[(
        TaskExecutionStatus,
        Option<TaskExecutionStatus>,
        TaskExecutionStatus,
    )] = &[
        (Pending, Some(Provisioning), Working),
        (Provisioning, Some(Working), AwaitingReview),
        (Blocked, None, Working),
        (Working, Some(AwaitingReview), Applying),
        (AwaitingReview, Some(Approved), Provisioning),
        (AwaitingReview, Some(ChangesRequested), Integrating),
        (Approved, Some(Integrating), Working),
        (ChangesRequested, Some(Working), Applying),
        (Integrating, Some(Validating), Pending),
        (Validating, Some(Applying), Working),
        (Applying, Some(Integrating), Pending),
        (Escalated, Some(Approved), Provisioning),
    ];
    for &(origin, successor, unrelated) in cases {
        assert!(
            origin.can_transition_to(Interrupted),
            "{origin:?} must be interruptible"
        );
        assert!(
            Interrupted.can_transition_to(origin),
            "coarse gate must let Interrupted reach {origin:?} (checkpoint narrows)"
        );
        assert!(
            TaskExecutionStatus::interrupted_resume_allowed(origin, origin),
            "exact resume to {origin:?} must be allowed"
        );
        if let Some(s) = successor {
            assert!(
                TaskExecutionStatus::interrupted_resume_allowed(origin, s),
                "{origin:?} interrupt must be able to resume successor {s:?}"
            );
        }
        assert!(
            !TaskExecutionStatus::interrupted_resume_allowed(origin, unrelated),
            "{origin:?} interrupt must NOT resume unrelated {unrelated:?}"
        );
    }
}

#[test]
fn interrupted_review_state_resumes_and_decides() {
    use TaskExecutionStatus::*;
    let conn = setup();

    // The exact Codex counter-example: an AwaitingReview execution is interrupted
    // by a backend restart. Before the coarse-gate fix, `Interrupted → AwaitingReview`
    // (and → Approved / → ChangesRequested) was rejected by `can_transition_to`
    // BEFORE the checkpoint guard ran, stranding the row outside Cancel/Escalate.
    let id = launch_and_drive(
        &conn,
        "t1",
        1,
        &[Provisioning, Working, AwaitingReview, Interrupted],
    );
    assert_eq!(
        get_task_execution(&conn, &id)
            .unwrap()
            .unwrap()
            .interrupted_from_status,
        Some(AwaitingReview)
    );
    // Exact return to the interrupted state — now reachable end-to-end.
    assert!(
        transition_execution(
            &conn,
            &id,
            AwaitingReview,
            &backend_actor(),
            serde_json::json!({})
        )
        .unwrap(),
        "an interrupted AwaitingReview must resume its exact state"
    );
    let exec = get_task_execution(&conn, &id).unwrap().unwrap();
    assert_eq!(exec.status, AwaitingReview);
    assert_eq!(
        exec.interrupted_from_status, None,
        "checkpoint consumed on resume"
    );
    // And the review decision proceeds normally afterwards.
    assert!(transition_execution(
        &conn,
        &id,
        Approved,
        &backend_actor(),
        serde_json::json!({})
    )
    .unwrap());

    // A second row resumes STRAIGHT to a review decision (a legal successor of the
    // AwaitingReview origin): AwaitingReview → Interrupted → ChangesRequested.
    let id2 = launch_and_drive(
        &conn,
        "t2",
        2,
        &[Provisioning, Working, AwaitingReview, Interrupted],
    );
    assert!(
        transition_execution(
            &conn,
            &id2,
            ChangesRequested,
            &backend_actor(),
            serde_json::json!({})
        )
        .unwrap(),
        "an interrupted AwaitingReview may resume straight to a decision"
    );
    assert_eq!(
        get_task_execution(&conn, &id2).unwrap().unwrap().status,
        ChangesRequested
    );
    // But NOT to a target the origin could never reach (checkpoint still narrows).
    let id3 = launch_and_drive(
        &conn,
        "t3",
        3,
        &[Provisioning, Working, AwaitingReview, Interrupted],
    );
    assert!(
        transition_execution(
            &conn,
            &id3,
            Applying,
            &backend_actor(),
            serde_json::json!({})
        )
        .is_err(),
        "an interrupted AwaitingReview must not resume an unrelated Applying"
    );
}

#[test]
fn checkpoint_columns_reject_out_of_domain_values() {
    use TaskExecutionStatus::*;
    let conn = setup();
    let id = launch_and_drive(&conn, "t1", 1, &[Provisioning]);

    // blocked_from_status is constrained to the two states that can enter Blocked
    // (§3): Provisioning|Applying. A structurally-impossible origin is rejected at
    // the DB, never silently coerced to NULL by the row mapper.
    assert!(
        conn.execute(
            "UPDATE task_executions SET blocked_from_status = 'Working' WHERE id = ?1",
            params![id],
        )
        .is_err(),
        "an out-of-domain blocked_from_status must be rejected"
    );
    // interrupted_from_status admits any non-terminal-except-Interrupted; terminals,
    // Interrupted itself and bogus strings are rejected.
    for bad in ["Done", "Failed", "Cancelled", "Interrupted", "Martian"] {
        assert!(
            conn.execute(
                "UPDATE task_executions SET interrupted_from_status = ?2 WHERE id = ?1",
                params![id, bad],
            )
            .is_err(),
            "interrupted_from_status must reject {bad}"
        );
    }
    // Valid values are accepted.
    conn.execute(
        "UPDATE task_executions SET blocked_from_status = 'Applying' WHERE id = ?1",
        params![id],
    )
    .expect("a valid blocked_from_status is accepted");
    conn.execute(
        "UPDATE task_executions SET interrupted_from_status = 'AwaitingReview' WHERE id = ?1",
        params![id],
    )
    .expect("a valid interrupted_from_status is accepted");
}

#[test]
fn parse_checkpoint_is_strict() {
    use TaskExecutionStatus::*;
    // NULL → None; a valid state → Some; an INVALID string → hard error (never a
    // silent None, which would erase a resume target the §4bis saga depends on).
    assert_eq!(parse_checkpoint(25, None).unwrap(), None);
    assert_eq!(
        parse_checkpoint(25, Some("Applying".into())).unwrap(),
        Some(Applying)
    );
    assert!(
        parse_checkpoint(25, Some("Bogus".into())).is_err(),
        "a corrupt checkpoint must surface as an error, not vanish"
    );
}

#[test]
fn launch_response_v1_is_versioned() {
    let conn = setup();
    seed_task(&conn, "t1", 1);
    let outcome = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t1", DISC),
        &backend_actor(),
    )
    .unwrap();

    let resp = crate::models::LaunchTaskExecutionResponseV1::from(outcome.clone());
    assert_eq!(
        resp.schema_version,
        crate::models::ORCHESTRATION_SCHEMA_VERSION
    );
    assert_eq!(resp.execution.id, outcome.execution.id);
    assert!(!resp.deduplicated);

    // Wire contract (DoD-5): schema_version is present and the whole shape
    // round-trips through serde, so KT-318 can expose it unchanged.
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(
        json["schema_version"],
        crate::models::ORCHESTRATION_SCHEMA_VERSION
    );
    let back: crate::models::LaunchTaskExecutionResponseV1 = serde_json::from_value(json).unwrap();
    assert_eq!(back.execution.status, TaskExecutionStatus::Pending);
}

#[test]
fn saga_resume_action_covers_the_reconciliation_table() {
    use TaskExecutionStatus::*;
    let target = Some("aaa");
    let merge = Some("bbb");

    // 1. Integrating, candidate not built → rebuild.
    assert_eq!(
        saga_resume_action(Integrating, target, None, None, target, false),
        SagaResumeAction::RebuildCandidate
    );
    // 2. Validating, candidate valid, tip == anchor → re-run validations.
    assert_eq!(
        saga_resume_action(Validating, target, merge, None, target, false),
        SagaResumeAction::RunValidations
    );
    // 3. Validating, parent drifted → rebuild.
    assert_eq!(
        saga_resume_action(Validating, target, merge, None, Some("zzz"), false),
        SagaResumeAction::RebuildCandidate
    );
    // 4. Applying, apply not done, tip == anchor, clean → apply.
    assert_eq!(
        saga_resume_action(Applying, target, merge, None, target, false),
        SagaResumeAction::ApplyFastForward
    );
    // 5. Applying, tip == anchor, dirty → block.
    assert_eq!(
        saga_resume_action(Applying, target, merge, None, target, true),
        SagaResumeAction::BlockDirtyTarget
    );
    // 6. Applying, tip already == candidate (apply landed, close pending) → close.
    assert_eq!(
        saga_resume_action(Applying, target, merge, None, merge, false),
        SagaResumeAction::IdempotentClose
    );
    // 7. Done, tip == integrated → no-op.
    assert_eq!(
        saga_resume_action(Done, target, merge, Some("ccc"), Some("ccc"), false),
        SagaResumeAction::NoOp
    );

    // Missing Git truth and a missing persisted anchor are two unknowns, not
    // proof that the parent is still at the anchor. No Git action is allowed.
    assert_eq!(
        saga_resume_action(Integrating, None, merge, None, None, false),
        SagaResumeAction::RebuildCandidate
    );
    assert_eq!(
        saga_resume_action(Validating, None, merge, None, None, false),
        SagaResumeAction::RebuildCandidate
    );
    assert_eq!(
        saga_resume_action(Applying, None, merge, None, None, false),
        SagaResumeAction::RebuildCandidate
    );
}

#[test]
fn lineage_query_resolves_the_whole_chain() {
    let conn = setup();
    seed_task(&conn, "t1", 42);
    // A managed child workspace linked to the execution.
    conn.execute(
        "INSERT INTO discussion_workspaces \
         (id, disc_id, project_id, branch, canonical_path, ownership, state, created_at, updated_at) \
         VALUES ('ws1', ?1, 'p1', 'kronn/task/KT-42', '/repo/child', 'managed', 'attached', \
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        params![DISC],
    )
    .unwrap();

    let out = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t1", DISC),
        &backend_actor(),
    )
    .unwrap();
    conn.execute(
        "UPDATE task_executions SET workspace_id = 'ws1' WHERE id = ?1",
        params![out.execution.id],
    )
    .unwrap();

    let lineage = get_execution_lineage(&conn, &out.execution.id)
        .unwrap()
        .unwrap();
    assert_eq!(lineage.task_reference, "KT-42");
    assert_eq!(lineage.parent_discussion_id, DISC);
    assert_eq!(lineage.orchestration_run_kind.as_str(), "single_task");
    assert_eq!(
        lineage.workspace_canonical_path.as_deref(),
        Some("/repo/child")
    );
}

#[test]
fn validation_run_verdict_and_quick_exec_provenance() {
    let conn = setup();
    seed_task(&conn, "t1", 1);
    let out = launch_single_task(
        &conn,
        &LaunchSingleTaskInput::new("t1", DISC),
        &backend_actor(),
    )
    .unwrap();
    let id = out.execution.id;

    // Ad-hoc validation, exit 0 → passes.
    let ok = record_validation_run(
        &conn,
        &id,
        Some("bbb"),
        &ValidationSpec {
            command: "cargo test".into(),
            quick_exec_id: None,
            timeout_secs: Some(600),
        },
        Some(0),
        Some(1200),
        Some("ok"),
    )
    .unwrap();
    assert!(ok.passed());

    // A NULL exit code (process died) is NEVER a pass.
    let died = record_validation_run(
        &conn,
        &id,
        Some("bbb"),
        &ValidationSpec {
            command: "flaky".into(),
            quick_exec_id: None,
            timeout_secs: None,
        },
        None,
        None,
        None,
    )
    .unwrap();
    assert!(!died.passed());

    // Provenance: a validation sourced from a saved Quick Exec.
    conn.execute(
        "INSERT INTO quick_execs (id, name, command, created_at, updated_at) \
         VALUES ('qe1', 'QE', 'echo', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    let sourced = record_validation_run(
        &conn,
        &id,
        Some("bbb"),
        &ValidationSpec {
            command: "echo".into(),
            quick_exec_id: Some("qe1".into()),
            timeout_secs: None,
        },
        Some(1),
        Some(5),
        Some("boom"),
    )
    .unwrap();
    assert!(!sourced.passed());
    assert_eq!(sourced.quick_exec_id.as_deref(), Some("qe1"));

    let all = list_validation_runs(&conn, &id).unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn planning_task_events_accept_backend_actor_after_widening() {
    let conn = setup();
    seed_task(&conn, "t1", 1);
    // The widened CHECK now admits backend/system actors.
    conn.execute(
        "INSERT INTO planning_task_events \
         (id, task_id, action, actor_kind, actor_id, changes_json, created_at) \
         VALUES ('e-backend', 't1', 'closed', 'backend', 'orchestrator', '{}', '2026-01-02T00:00:00Z')",
        [],
    )
    .expect("backend actor must be accepted after migration 127");
    // An unknown actor kind is still rejected.
    let bad = conn.execute(
        "INSERT INTO planning_task_events \
         (id, task_id, action, actor_kind, changes_json, created_at) \
         VALUES ('e-bad', 't1', 'x', 'martian', '{}', '2026-01-02T00:00:00Z')",
        [],
    );
    assert!(bad.is_err(), "an unknown actor kind must still be rejected");
}

// ─── KT-318 socle: worker identity contract + attempt_no ──────────────────────

#[test]
fn worker_identity_round_trips_all_kinds_and_two_clis_stay_distinct() {
    use MessageTargetKind::*;
    let conn = setup();
    seed_session(&conn, 11, "Codex", "codex-a");
    seed_session(&conn, 12, "Codex", "codex-b");

    // discussion_agent — native principal, no session id.
    seed_task(&conn, "t-da", 1);
    let mut da = LaunchSingleTaskInput::new("t-da", DISC);
    da.worker_target_kind = Some(DiscussionAgent);
    da.worker_agent_type = Some("ClaudeCode".into());
    let da = launch_single_task(&conn, &da, &backend_actor())
        .unwrap()
        .execution;
    assert_eq!(da.worker_target_kind, Some(DiscussionAgent));
    assert_eq!(da.worker_cli_session_id, None);
    assert_eq!(da.worker_agent_type.as_deref(), Some("ClaudeCode"));
    assert_eq!(da.attempt_no, 0, "a fresh launch is attempt 0");

    // agent — one-shot native, no session id.
    seed_task(&conn, "t-ag", 2);
    let mut ag = LaunchSingleTaskInput::new("t-ag", DISC);
    ag.worker_target_kind = Some(Agent);
    ag.worker_agent_type = Some("Codex".into());
    let ag = launch_single_task(&conn, &ag, &backend_actor())
        .unwrap()
        .execution;
    assert_eq!(ag.worker_target_kind, Some(Agent));
    assert_eq!(ag.worker_cli_session_id, None);

    // Two CLI workers of the SAME provider must remain distinct identities.
    seed_task(&conn, "t-c1", 3);
    let mut c1 = LaunchSingleTaskInput::new("t-c1", DISC);
    c1.worker_target_kind = Some(Cli);
    c1.worker_agent_type = Some("Codex".into());
    c1.worker_cli_session_id = Some(11);
    let c1 = launch_single_task(&conn, &c1, &backend_actor())
        .unwrap()
        .execution;

    seed_task(&conn, "t-c2", 4);
    let mut c2 = LaunchSingleTaskInput::new("t-c2", DISC);
    c2.worker_target_kind = Some(Cli);
    c2.worker_agent_type = Some("Codex".into());
    c2.worker_cli_session_id = Some(12);
    let c2 = launch_single_task(&conn, &c2, &backend_actor())
        .unwrap()
        .execution;

    assert_eq!(c1.worker_target_kind, Some(Cli));
    assert_eq!(c2.worker_target_kind, Some(Cli));
    assert_eq!(
        c1.worker_agent_type, c2.worker_agent_type,
        "same provider string"
    );
    assert_ne!(
        c1.worker_cli_session_id, c2.worker_cli_session_id,
        "two CLIs of the same provider are distinguished by the exact session id"
    );

    // Re-read from DB (not just the launch return) to prove persistence + the
    // strict `parse_target_kind` round-trip.
    let reread = get_task_execution(&conn, &c1.id).unwrap().unwrap();
    assert_eq!(reread.worker_target_kind, Some(Cli));
    assert_eq!(reread.worker_cli_session_id, Some(11));
    assert_eq!(reread.worker_agent_type.as_deref(), Some("Codex"));
}

#[test]
fn external_worker_connection_round_trips_and_unknown_id_is_rejected_on_reload() {
    use MessageTargetKind::Agent;
    let conn = setup();
    conn.execute(
        "INSERT INTO external_api_connections \
         (id, display_name, mention_alias, credential_slug, origin_preset) \
         VALUES ('conn-known', 'Known', 'known', 'known', 'litellm')",
        [],
    )
    .unwrap();

    seed_task(&conn, "t-connection", 1);
    let mut input = LaunchSingleTaskInput::new("t-connection", DISC);
    input.worker_target_kind = Some(Agent);
    input.worker_agent_type = Some(agent_type_to_db(&AgentType::LiteLlm));
    input.worker_connection_id = Some("conn-known".into());
    let launched = launch_single_task(&conn, &input, &backend_actor())
        .unwrap()
        .execution;
    let reread = get_task_execution(&conn, &launched.id).unwrap().unwrap();
    assert_eq!(reread.worker_target_kind, Some(Agent));
    assert_eq!(reread.worker_agent_type.as_deref(), Some("LiteLlm"));
    assert_eq!(reread.worker_connection_id.as_deref(), Some("conn-known"));

    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at) \
         VALUES ('d-connection-worker', 'Connection worker', '2026-01-01T00:00:00Z', \
                 '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    set_execution_sub_discussion(&conn, &launched.id, "d-connection-worker").unwrap();

    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute(
        "UPDATE task_executions SET worker_connection_id = 'conn-missing' WHERE id = ?1",
        [&launched.id],
    )
    .unwrap();
    let error = get_task_execution(&conn, &launched.id).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown worker connection identifier: conn-missing"),
        "unknown connection must be an explicit reload error: {error:#}"
    );

    for (path, result) in [
        (
            "worker room",
            get_execution_for_sub_discussion(&conn, "d-connection-worker"),
        ),
        (
            "active task reconnect",
            get_active_execution_for_task(&conn, "t-connection"),
        ),
        (
            "latest task reconnect",
            get_latest_execution_for_task(&conn, "t-connection"),
        ),
    ] {
        let error = result.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown worker connection identifier: conn-missing"),
            "{path} must reject the unknown connection: {error:#}"
        );
    }

    let error = get_execution_lineage(&conn, &launched.id).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown worker connection identifier: conn-missing"),
        "lineage reload must reject the unknown connection: {error:#}"
    );
}

#[test]
fn worker_identity_check_is_all_or_nothing_and_session_fk_is_restrict() {
    let conn = setup();
    seed_session(&conn, 21, "Codex", "codex-x");
    let id = launch_and_drive(&conn, "t1", 1, &[]); // fresh Pending, no worker yet

    // A kind set without the mandatory agent_type is rejected for ALL three kinds
    // (MessageTarget.agent_type is non-optional).
    for kind in ["discussion_agent", "agent", "cli"] {
        assert!(
            conn.execute(
                "UPDATE task_executions SET worker_target_kind = ?2, \
                 worker_agent_type = NULL, worker_cli_session_id = NULL WHERE id = ?1",
                params![id, kind],
            )
            .is_err(),
            "{kind} without an agent type must be rejected"
        );
    }
    // A cli kind without an exact session id is rejected.
    assert!(
        conn.execute(
            "UPDATE task_executions SET worker_target_kind = 'cli', \
             worker_agent_type = 'Codex', worker_cli_session_id = NULL WHERE id = ?1",
            params![id],
        )
        .is_err(),
        "a cli worker without an exact session id must be rejected"
    );
    // A native kind carrying a CLI session id is rejected.
    assert!(
        conn.execute(
            "UPDATE task_executions SET worker_target_kind = 'agent', \
             worker_agent_type = 'Codex', worker_cli_session_id = 21 WHERE id = ?1",
            params![id],
        )
        .is_err(),
        "a native worker must not carry a CLI session id"
    );
    // A complete cli identity is accepted.
    conn.execute(
        "UPDATE task_executions SET worker_target_kind = 'cli', \
         worker_agent_type = 'Codex', worker_cli_session_id = 21 WHERE id = ?1",
        params![id],
    )
    .expect("a complete cli identity is accepted");

    // The session an execution worker references can no longer be deleted
    // (FK RESTRICT / NO ACTION): the audited worker identity is preserved.
    assert!(
        conn.execute("DELETE FROM discussion_sessions WHERE id = 21", [])
            .is_err(),
        "a session referenced by a worker identity must not be deletable"
    );
}

#[test]
fn parse_target_kind_is_strict() {
    use MessageTargetKind::*;
    assert_eq!(parse_target_kind(28, None).unwrap(), None);
    assert_eq!(
        parse_target_kind(28, Some("cli".into())).unwrap(),
        Some(Cli)
    );
    assert_eq!(
        parse_target_kind(28, Some("discussion_agent".into())).unwrap(),
        Some(DiscussionAgent)
    );
    assert!(
        parse_target_kind(28, Some("provider".into())).is_err(),
        "a corrupt worker_target_kind must surface as an error, not vanish"
    );
}

#[test]
fn block_execution_round_trips_the_structured_code() {
    use TaskExecutionStatus::*;
    let conn = setup();

    // A CLI-offer park carries the structured code; a native checkpoint-refused park
    // carries None. Both round-trip through the new column and the strict read, and
    // the code is stored INDEPENDENTLY of the free-text reason (KT-334 branches on it).
    let coded = launch_and_drive(&conn, "t1", 1, &[Provisioning]);
    assert!(block_execution(
        &conn,
        &coded,
        &backend_actor(),
        "worker session already committed elsewhere",
        Some(BlockedReasonCode::WorkerSessionCommittedElsewhere),
    )
    .unwrap());
    let coded_row = get_task_execution(&conn, &coded).unwrap().unwrap();
    assert_eq!(coded_row.status, Blocked);
    assert_eq!(
        coded_row.blocked_reason_code,
        Some(BlockedReasonCode::WorkerSessionCommittedElsewhere),
        "the code round-trips independently of the prose reason"
    );
    assert!(
        coded_row.blocked_reason.is_some(),
        "prose reason kept for humans"
    );
    assert!(transition_execution(
        &conn,
        &coded,
        Provisioning,
        &backend_actor(),
        serde_json::json!({ "reason": "worker_attached" })
    )
    .unwrap());
    let resumed = get_task_execution(&conn, &coded).unwrap().unwrap();
    assert_eq!(resumed.status, Provisioning);
    assert_eq!(resumed.blocked_from_status, None);
    assert_eq!(resumed.blocked_reason, None);
    assert_eq!(resumed.blocked_reason_code, None);
    let blocked_event = list_execution_events(&conn, &coded)
        .unwrap()
        .into_iter()
        .find(|event| event.to_status == Some(Blocked))
        .expect("the original Blocked transition remains auditable");
    assert_eq!(
        blocked_event.changes["code"],
        BlockedReasonCode::WorkerSessionCommittedElsewhere.as_str()
    );
    assert_eq!(
        blocked_event.changes["reason"],
        "worker session already committed elsewhere"
    );

    let uncoded = launch_and_drive(&conn, "t2", 2, &[Provisioning]);
    assert!(block_execution(
        &conn,
        &uncoded,
        &backend_actor(),
        "provisioning failed",
        None
    )
    .unwrap());
    assert_eq!(
        get_task_execution(&conn, &uncoded)
            .unwrap()
            .unwrap()
            .blocked_reason_code,
        None,
        "a native checkpoint-refused block leaves the code NULL in V1"
    );
}

#[test]
fn interrupted_block_preserves_reason_until_the_hold_is_really_resumed() {
    use TaskExecutionStatus::*;
    let conn = setup();
    let id = launch_and_drive(&conn, "t1", 1, &[Provisioning]);
    block_execution(
        &conn,
        &id,
        &backend_actor(),
        "awaiting_worker_acceptance",
        Some(BlockedReasonCode::AwaitingWorkerAcceptance),
    )
    .unwrap();

    transition_execution(
        &conn,
        &id,
        Interrupted,
        &backend_actor(),
        serde_json::json!({ "reason": "boot_reconcile" }),
    )
    .unwrap();
    let interrupted = get_task_execution(&conn, &id).unwrap().unwrap();
    assert_eq!(interrupted.status, Interrupted);
    assert_eq!(interrupted.interrupted_from_status, Some(Blocked));
    assert_eq!(interrupted.blocked_from_status, Some(Provisioning));
    assert_eq!(
        interrupted.blocked_reason.as_deref(),
        Some("awaiting_worker_acceptance")
    );
    assert_eq!(
        interrupted.blocked_reason_code,
        Some(BlockedReasonCode::AwaitingWorkerAcceptance)
    );

    transition_execution(
        &conn,
        &id,
        Blocked,
        &backend_actor(),
        serde_json::json!({ "reason": "resume_blocked_hold" }),
    )
    .unwrap();
    let blocked_again = get_task_execution(&conn, &id).unwrap().unwrap();
    assert_eq!(blocked_again.blocked_from_status, Some(Provisioning));
    assert_eq!(
        blocked_again.blocked_reason_code,
        Some(BlockedReasonCode::AwaitingWorkerAcceptance)
    );

    transition_execution(
        &conn,
        &id,
        Provisioning,
        &backend_actor(),
        serde_json::json!({ "reason": "worker_attached" }),
    )
    .unwrap();
    let resumed = get_task_execution(&conn, &id).unwrap().unwrap();
    assert_eq!(resumed.status, Provisioning);
    assert_eq!(resumed.blocked_from_status, None);
    assert_eq!(resumed.interrupted_from_status, None);
    assert_eq!(resumed.blocked_reason, None);
    assert_eq!(resumed.blocked_reason_code, None);
}

#[test]
fn parse_blocked_reason_code_is_strict() {
    use BlockedReasonCode::*;
    assert_eq!(parse_blocked_reason_code(30, None).unwrap(), None);
    assert_eq!(
        parse_blocked_reason_code(30, Some("awaiting_worker_acceptance".into())).unwrap(),
        Some(AwaitingWorkerAcceptance)
    );
    assert_eq!(
        parse_blocked_reason_code(30, Some("worker_session_committed_elsewhere".into())).unwrap(),
        Some(WorkerSessionCommittedElsewhere)
    );
    assert!(
        parse_blocked_reason_code(30, Some("some_future_code".into())).is_err(),
        "a code outside the enum domain must surface as an error, not vanish — the \
         column has no SQL CHECK, so this strict read IS the domain guard"
    );
}

/// The four checkpoint columns of migration 127 exist so a crash mid-integration can
/// be reconciled against the real Git refs at boot. Until now nobody wrote them, so
/// `saga_resume_action` decided on NULLs — it always read "rebuild", whatever had
/// actually happened. This drives the saga step by step and checks the reader gets
/// back, at every point, the action the state really calls for.
#[test]
fn integration_checkpoints_are_readable_by_the_resume_table() {
    const TARGET: &str = "1111111111111111111111111111111111111111";
    const MERGE: &str = "2222222222222222222222222222222222222222";
    const BACKUP: &str = "refs/kronn-backup/KT-900";

    let conn = setup();
    let exec = launch_and_drive(
        &conn,
        "t-saga-write",
        900,
        &[
            TaskExecutionStatus::Provisioning,
            TaskExecutionStatus::Working,
            TaskExecutionStatus::AwaitingReview,
            TaskExecutionStatus::Approved,
        ],
    );
    let cols = |conn: &Connection| -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        conn.query_row(
            "SELECT candidate_target_sha, candidate_merge_sha, integrated_sha, backup_ref \
               FROM task_executions WHERE id = ?1",
            params![exec],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    };

    // Anchored, candidate not built: the parent tip is pinned but there is nothing
    // to validate yet.
    let out = commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::CandidateAnchored { target_sha: TARGET },
        &backend_actor(),
    )
    .unwrap();
    assert_eq!(
        out,
        IntegrationCheckpointOutcome::Committed {
            status: TaskExecutionStatus::Integrating
        }
    );
    let (target, merge, integrated, _) = cols(&conn);
    assert_eq!(target.as_deref(), Some(TARGET));
    assert_eq!(merge, None);
    assert_eq!(
        saga_resume_action(
            TaskExecutionStatus::Integrating,
            target.as_deref(),
            merge.as_deref(),
            integrated.as_deref(),
            Some(TARGET),
            false,
        ),
        SagaResumeAction::RebuildCandidate
    );

    // Candidate built: same status, but the reader now has something to validate.
    commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::CandidateBuilt { merge_sha: MERGE },
        &backend_actor(),
    )
    .unwrap();
    let (target, merge, integrated, _) = cols(&conn);
    assert_eq!(merge.as_deref(), Some(MERGE));
    assert_eq!(
        saga_resume_action(
            TaskExecutionStatus::Integrating,
            target.as_deref(),
            merge.as_deref(),
            integrated.as_deref(),
            Some(TARGET),
            false,
        ),
        SagaResumeAction::RunValidations
    );

    commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::ValidationsStarted,
        &backend_actor(),
    )
    .unwrap();

    // Armed: the backup ref is pinned BEFORE the parent may move, and the parent is
    // still at the anchor — so the apply is a plain fast-forward.
    commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::ApplyArmed { backup_ref: BACKUP },
        &backend_actor(),
    )
    .unwrap();
    let (target, merge, integrated, backup) = cols(&conn);
    assert_eq!(backup.as_deref(), Some(BACKUP));
    assert_eq!(integrated, None);
    assert_eq!(
        saga_resume_action(
            TaskExecutionStatus::Applying,
            target.as_deref(),
            merge.as_deref(),
            integrated.as_deref(),
            Some(TARGET),
            false,
        ),
        SagaResumeAction::ApplyFastForward
    );
    // Same row, dirty parent: hold instead of applying over someone's work.
    assert_eq!(
        saga_resume_action(
            TaskExecutionStatus::Applying,
            target.as_deref(),
            merge.as_deref(),
            integrated.as_deref(),
            Some(TARGET),
            true,
        ),
        SagaResumeAction::BlockDirtyTarget
    );

    // Integrated: the parent became the candidate, and there is nothing left to redo.
    // Direct DB fixtures do not run the provisioning checkpoint that normally
    // flips the task to InProgress, so establish the real precondition here.
    conn.execute(
        "UPDATE planning_tasks SET status = 'in_progress' WHERE id = 't-saga-write'",
        [],
    )
    .unwrap();
    commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::Integrated {
            integrated_sha: MERGE,
        },
        &backend_actor(),
    )
    .unwrap();
    let (target, merge, integrated, _) = cols(&conn);
    assert_eq!(integrated.as_deref(), Some(MERGE));
    let task_status: String = conn
        .query_row(
            "SELECT status FROM planning_tasks WHERE id = 't-saga-write'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(task_status, "done", "task and execution finish atomically");
    assert_eq!(
        saga_resume_action(
            TaskExecutionStatus::Done,
            target.as_deref(),
            merge.as_deref(),
            integrated.as_deref(),
            Some(MERGE),
            false,
        ),
        SagaResumeAction::NoOp
    );
}

#[test]
fn integrated_checkpoint_rolls_back_when_plan_task_is_not_in_progress() {
    let conn = setup();
    let exec = launch_and_drive(
        &conn,
        "t-saga-task-race",
        902,
        &[
            TaskExecutionStatus::Provisioning,
            TaskExecutionStatus::Working,
            TaskExecutionStatus::AwaitingReview,
            TaskExecutionStatus::Approved,
            TaskExecutionStatus::Integrating,
            TaskExecutionStatus::Validating,
            TaskExecutionStatus::Applying,
        ],
    );
    conn.execute(
        "UPDATE task_executions SET candidate_merge_sha = \
         'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' WHERE id = ?1",
        [&exec],
    )
    .unwrap();
    // The direct fixture intentionally leaves its task Todo, modelling a human
    // or concurrent writer moving it away from the required InProgress state.
    let outcome = commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::Integrated {
            integrated_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        },
        &backend_actor(),
    )
    .unwrap();
    assert_eq!(outcome, IntegrationCheckpointOutcome::TaskNotCompletable);
    let (status, integrated): (String, Option<String>) = conn
        .query_row(
            "SELECT status, integrated_sha FROM task_executions WHERE id = ?1",
            [&exec],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, TaskExecutionStatus::Applying.as_str());
    assert_eq!(
        integrated, None,
        "integrated_sha rolled back with the refused task CAS"
    );
}

#[test]
fn done_requires_the_integrated_candidate_and_every_configured_validation() {
    const MERGE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let conn = setup();
    let exec = launch_and_drive(
        &conn,
        "t-done-proof",
        903,
        &[
            TaskExecutionStatus::Provisioning,
            TaskExecutionStatus::Working,
            TaskExecutionStatus::AwaitingReview,
            TaskExecutionStatus::Approved,
            TaskExecutionStatus::Integrating,
            TaskExecutionStatus::Validating,
            TaskExecutionStatus::Applying,
        ],
    );
    conn.execute(
        "UPDATE planning_tasks SET status = 'in_progress' WHERE id = 't-done-proof'",
        [],
    )
    .unwrap();

    let direct = transition_execution(
        &conn,
        &exec,
        TaskExecutionStatus::Done,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap_err();
    assert!(direct.to_string().contains("candidate_merge_sha"));

    conn.execute(
        "UPDATE task_executions SET candidate_merge_sha = ?2 WHERE id = ?1",
        params![exec, MERGE],
    )
    .unwrap();
    conn.execute(
        "UPDATE orchestration_runs SET validation_json = \
         '[{\"command\":\"cargo test\",\"quick_exec_id\":null,\"timeout_secs\":600}]' \
         WHERE id = (SELECT orchestration_run_id FROM task_executions WHERE id = ?1)",
        [&exec],
    )
    .unwrap();

    let mismatched = commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::Integrated {
            integrated_sha: "cccccccccccccccccccccccccccccccccccccccc",
        },
        &backend_actor(),
    )
    .unwrap_err();
    assert!(mismatched
        .to_string()
        .contains("differs from candidate_merge_sha"));

    let missing = commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::Integrated {
            integrated_sha: MERGE,
        },
        &backend_actor(),
    )
    .unwrap_err();
    assert!(missing.to_string().contains("no passing result"));
    assert_eq!(
        get_task_execution(&conn, &exec).unwrap().unwrap().status,
        TaskExecutionStatus::Applying
    );

    record_validation_run(
        &conn,
        &exec,
        Some(MERGE),
        &ValidationSpec {
            command: "cargo test".into(),
            quick_exec_id: None,
            timeout_secs: Some(600),
        },
        Some(1),
        Some(50),
        Some("failed"),
    )
    .unwrap();
    assert!(commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::Integrated {
            integrated_sha: MERGE,
        },
        &backend_actor(),
    )
    .is_err());

    record_validation_run(
        &conn,
        &exec,
        Some(MERGE),
        &ValidationSpec {
            command: "cargo test".into(),
            quick_exec_id: None,
            timeout_secs: Some(600),
        },
        Some(0),
        Some(60),
        Some("passed"),
    )
    .unwrap();
    let completed = commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::Integrated {
            integrated_sha: MERGE,
        },
        &backend_actor(),
    )
    .unwrap();
    assert_eq!(
        completed,
        IntegrationCheckpointOutcome::Committed {
            status: TaskExecutionStatus::Done
        }
    );
    let execution = get_task_execution(&conn, &exec).unwrap().unwrap();
    assert_eq!(execution.integrated_sha.as_deref(), Some(MERGE));
    assert_eq!(execution.status, TaskExecutionStatus::Done);
}

/// A retried step must not write twice. The status gate is the guard: replaying an
/// already-committed step reports where the execution really stands instead of
/// re-pinning an anchor the saga has already moved past.
#[test]
fn replaying_an_integration_step_does_not_write_again() {
    const TARGET: &str = "3333333333333333333333333333333333333333";
    const OTHER: &str = "4444444444444444444444444444444444444444";

    let conn = setup();
    let exec = launch_and_drive(
        &conn,
        "t-saga-replay",
        901,
        &[
            TaskExecutionStatus::Provisioning,
            TaskExecutionStatus::Working,
            TaskExecutionStatus::AwaitingReview,
            TaskExecutionStatus::Approved,
        ],
    );

    commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::CandidateAnchored { target_sha: TARGET },
        &backend_actor(),
    )
    .unwrap();

    let replay = commit_integration_checkpoint(
        &conn,
        &exec,
        IntegrationStep::CandidateAnchored { target_sha: OTHER },
        &backend_actor(),
    )
    .unwrap();
    assert_eq!(
        replay,
        IntegrationCheckpointOutcome::NotInStep {
            status: TaskExecutionStatus::Integrating
        }
    );

    let anchor: Option<String> = conn
        .query_row(
            "SELECT candidate_target_sha FROM task_executions WHERE id = ?1",
            params![exec],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        anchor.as_deref(),
        Some(TARGET),
        "the original anchor must survive a replay"
    );
}

/// Drive an execution all the way to `Done`, satisfying the merge-sha invariant
/// the state machine enforces before an execution may finish.
fn launch_and_finish(conn: &Connection, task_id: &str, number: i64) -> String {
    let execution = launch_and_drive(
        conn,
        task_id,
        number,
        &[
            TaskExecutionStatus::Provisioning,
            TaskExecutionStatus::Working,
            TaskExecutionStatus::AwaitingReview,
            TaskExecutionStatus::Approved,
            TaskExecutionStatus::Integrating,
            TaskExecutionStatus::Validating,
            TaskExecutionStatus::Applying,
        ],
    );
    conn.execute(
        "UPDATE task_executions SET candidate_merge_sha = \
         'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', integrated_sha = \
         'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' WHERE id = ?1",
        [&execution],
    )
    .unwrap();
    assert!(transition_execution(
        conn,
        &execution,
        TaskExecutionStatus::Done,
        &backend_actor(),
        serde_json::json!({}),
    )
    .unwrap());
    execution
}

// ── KT-373 — durable authorisation for reclaiming build artefacts ────────────
//
// Terminal is necessary and NOT sufficient. Each test below is a way a
// worktree can be finished on paper and still in use in fact.

/// Attach a managed workspace row to an execution, at a canonical path.
fn seed_managed_workspace(
    conn: &Connection,
    id: &str,
    execution_id: &str,
    canonical_path: &str,
    state: &str,
    session_pk: Option<i64>,
) {
    conn.execute(
        "INSERT INTO discussion_workspaces \
         (id, disc_id, session_pk, project_id, workspace_path, canonical_path, branch, \
          ownership, state, created_at, updated_at, task_execution_id) \
         VALUES (?1, ?2, ?3, 'p1', ?4, ?4, 'kt/branch', 'managed', ?5, \
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', ?6)",
        params![id, DISC, session_pk, canonical_path, state, execution_id],
    )
    .unwrap();
}

#[test]
fn git_commit_authority_requires_the_exact_working_managed_workspace() {
    let conn = setup();
    let execution = launch_and_drive(
        &conn,
        "task-native-commit",
        9010,
        &[
            TaskExecutionStatus::Provisioning,
            TaskExecutionStatus::Working,
        ],
    );
    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at) \
         VALUES ('disc-native-worker', 'Worker', '2026-01-01T00:00:00Z', \
                 '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussion_workspaces \
         (id, disc_id, project_id, workspace_path, canonical_path, branch, head_sha, \
          ownership, state, created_at, updated_at, task_execution_id) \
         VALUES ('ws-native-commit', 'disc-native-worker', 'p1', '/repo/task', \
                 '/repo/task', 'task/branch', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', \
                 'managed', 'attached', '2026-01-01T00:00:00Z', \
                 '2026-01-01T00:00:00Z', ?1)",
        [&execution],
    )
    .unwrap();
    conn.execute(
        "UPDATE task_executions \
            SET sub_discussion_id = 'disc-native-worker', workspace_id = 'ws-native-commit' \
          WHERE id = ?1",
        [&execution],
    )
    .unwrap();

    let authorised =
        managed_working_execution_for_workspace(&conn, "disc-native-worker", "/repo/task")
            .unwrap()
            .expect("the exact active managed execution must be resolved");
    assert_eq!(authorised.id, execution);
    assert!(
        managed_working_execution_for_workspace(&conn, DISC, "/repo/task")
            .unwrap()
            .is_none()
    );
    assert!(
        managed_working_execution_for_workspace(&conn, "disc-native-worker", "/repo/other",)
            .unwrap()
            .is_none()
    );

    conn.execute(
        "UPDATE discussion_workspaces SET ownership = 'external' WHERE id = 'ws-native-commit'",
        [],
    )
    .unwrap();
    assert!(
        managed_working_execution_for_workspace(&conn, "disc-native-worker", "/repo/task",)
            .unwrap()
            .is_none()
    );
    conn.execute(
        "UPDATE discussion_workspaces SET ownership = 'managed' WHERE id = 'ws-native-commit'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE task_executions SET status = 'AwaitingReview' WHERE id = ?1",
        [&execution],
    )
    .unwrap();
    assert!(
        managed_working_execution_for_workspace(&conn, "disc-native-worker", "/repo/task",)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_finished_execution_with_nothing_holding_it_authorises_cleanup() {
    let conn = setup();
    let execution = launch_and_finish(&conn, "task-cleanup-ok", 9001);
    seed_managed_workspace(
        &conn,
        "ws-ok",
        &execution,
        "/repo/.kronn/worktrees/a",
        "detached",
        None,
    );

    assert_eq!(
        worktree_cleanup_liveness(&conn, "/repo/.kronn/worktrees/a").unwrap(),
        crate::core::worktree::ExecutionLiveness::Terminal,
    );
}

#[test]
fn a_worktree_mid_review_is_refused_without_any_extra_check() {
    // Review, integration and validation are deliberately non-terminal, so the
    // status check alone already covers "someone is looking at this".
    let conn = setup();
    let execution = launch_and_drive(
        &conn,
        "task-cleanup-review",
        9002,
        &[
            TaskExecutionStatus::Provisioning,
            TaskExecutionStatus::Working,
            TaskExecutionStatus::AwaitingReview,
        ],
    );
    seed_managed_workspace(
        &conn,
        "ws-review",
        &execution,
        "/repo/.kronn/worktrees/b",
        "attached",
        None,
    );

    let verdict = worktree_cleanup_liveness(&conn, "/repo/.kronn/worktrees/b").unwrap();
    match verdict {
        crate::core::worktree::ExecutionLiveness::Active(reason) => {
            assert!(reason.contains("AwaitingReview"), "got: {reason}");
        }
        other => panic!("a worktree under review must be refused, got {other:?}"),
    }
}

#[test]
fn a_finished_execution_whose_session_is_still_attached_is_refused() {
    // The 2026-08-21 shape exactly: the work is over on paper, the agent is
    // still in the directory between builds. A process scan said "idle"; the
    // session row says otherwise.
    let conn = setup();
    seed_session(&conn, 4242, "ClaudeCode", "sess-live");
    let execution = launch_and_finish(&conn, "task-cleanup-attached", 9003);
    seed_managed_workspace(
        &conn,
        "ws-attached",
        &execution,
        "/repo/.kronn/worktrees/c",
        "attached",
        Some(4242),
    );

    match worktree_cleanup_liveness(&conn, "/repo/.kronn/worktrees/c").unwrap() {
        crate::core::worktree::ExecutionLiveness::Active(reason) => {
            assert!(reason.contains("still attached"), "got: {reason}");
        }
        other => panic!("a live attached session must refuse cleanup, got {other:?}"),
    }
}

#[test]
fn an_unreleased_worker_lease_refuses_even_a_detached_finished_worktree() {
    let conn = setup();
    seed_session(&conn, 4243, "Codex", "sess-lease");
    let execution = launch_and_finish(&conn, "task-cleanup-lease", 9004);
    seed_managed_workspace(
        &conn,
        "ws-lease",
        &execution,
        "/repo/.kronn/worktrees/d",
        "detached",
        None,
    );
    conn.execute(
        "INSERT INTO discussion_workspace_history_leases \
         (id, disc_id, session_pk, canonical_path, branch, backup_ref, head_sha, \
          acquired_at, expires_at) \
         VALUES ('lease-1', ?1, 4243, '/repo/.kronn/worktrees/d', 'kt/branch', \
                 'refs/kronn/backup', 'deadbeef', '2026-01-01T00:00:00Z', \
                 '2099-01-01T00:00:00Z')",
        params![DISC],
    )
    .unwrap();

    match worktree_cleanup_liveness(&conn, "/repo/.kronn/worktrees/d").unwrap() {
        crate::core::worktree::ExecutionLiveness::Active(reason) => {
            assert!(reason.contains("lease"), "got: {reason}");
        }
        other => panic!("an unreleased lease must refuse cleanup, got {other:?}"),
    }
}

#[test]
fn an_external_workspace_is_never_ours_to_reclaim() {
    let conn = setup();
    let execution = launch_and_finish(&conn, "task-cleanup-external", 9005);
    conn.execute(
        "INSERT INTO discussion_workspaces \
         (id, disc_id, project_id, workspace_path, canonical_path, branch, ownership, \
          state, created_at, updated_at, task_execution_id) \
         VALUES ('ws-ext', ?1, 'p1', '/user/checkout', '/user/checkout', 'main', 'external', \
                 'attached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', ?2)",
        params![DISC, execution],
    )
    .unwrap();

    match worktree_cleanup_liveness(&conn, "/user/checkout").unwrap() {
        crate::core::worktree::ExecutionLiveness::Active(reason) => {
            assert!(reason.contains("external"), "got: {reason}");
        }
        other => panic!("an external checkout is never cleaned, got {other:?}"),
    }
}

#[test]
fn a_path_no_workspace_row_claims_refuses_instead_of_defaulting_to_clean() {
    // An unclaimed directory is an inconsistency to report, not a directory to
    // delete. Silence must never read as permission.
    let conn = setup();
    match worktree_cleanup_liveness(&conn, "/repo/.kronn/worktrees/ghost").unwrap() {
        crate::core::worktree::ExecutionLiveness::Unknown(reason) => {
            assert!(reason.contains("no workspace row"), "got: {reason}");
        }
        other => panic!("an unclaimed path must be Unknown, got {other:?}"),
    }
}

#[test]
fn a_reclaim_leaves_a_durable_record_that_outlives_the_logs() {
    // KT-373 DoD-11. `tracing` answers "what happened" while the process lives.
    // A deletion that took gigabytes off the disk has to stay answerable after
    // the logs rotate — on whose authority, on which target, for how much.
    let conn = setup();
    let execution = launch_and_finish(&conn, "task-audit-ok", 9101);
    seed_managed_workspace(
        &conn,
        "ws-audit",
        &execution,
        "/repo/.kronn/worktrees/e",
        "detached",
        None,
    );

    record_artifact_reclaim(&conn, "/repo/.kronn/worktrees/e", Ok((4_000_000_000, true))).unwrap();

    let (action, changes, actor): (String, String, String) = conn
        .query_row(
            "SELECT action, changes_json, actor_kind FROM task_execution_events
              WHERE task_execution_id = ?1 AND action LIKE 'build_artifacts%'",
            params![execution],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(action, "build_artifacts_reclaimed");
    assert_eq!(actor, "backend");
    let changes: serde_json::Value = serde_json::from_str(&changes).unwrap();
    assert_eq!(changes["bytes_reclaimed"], 4_000_000_000u64);
    // A floor must never be readable as a measurement.
    assert_eq!(changes["bytes_are_partial"], true);
    assert_eq!(changes["target"], "/repo/.kronn/worktrees/e");
}

#[test]
fn a_refused_reclaim_is_recorded_too_with_its_reason() {
    // A disk that stayed full because cleanup was declined looks nothing like
    // one nobody tried to clean. Only the record tells them apart.
    let conn = setup();
    let execution = launch_and_finish(&conn, "task-audit-refused", 9102);
    seed_managed_workspace(
        &conn,
        "ws-audit-r",
        &execution,
        "/repo/.kronn/worktrees/f",
        "detached",
        None,
    );

    record_artifact_reclaim(
        &conn,
        "/repo/.kronn/worktrees/f",
        Err("Cargo holds a build lock".into()),
    )
    .unwrap();

    let (action, changes): (String, String) = conn
        .query_row(
            "SELECT action, changes_json FROM task_execution_events
              WHERE task_execution_id = ?1 AND action LIKE 'build_artifacts%'",
            params![execution],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(action, "build_artifacts_refused");
    let changes: serde_json::Value = serde_json::from_str(&changes).unwrap();
    assert!(changes["reason"].as_str().unwrap().contains("build lock"));
}

#[test]
fn an_unowned_path_is_not_forced_into_someone_elses_audit_trail() {
    // No owning execution means no row this event could honestly hang from.
    // Inventing one would corrupt another execution's history to avoid an audit
    // gap we can simply name.
    let conn = setup();
    record_artifact_reclaim(&conn, "/repo/.kronn/worktrees/ghost", Ok((1, false)))
        .expect("an unowned path is not an error");

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_execution_events WHERE action LIKE 'build_artifacts%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "nothing is attributed to an execution that owns nothing"
    );
}
