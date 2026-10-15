//! KT-154 — assignees on planning tasks: typed identity, idempotence, the
//! agent self-assignment on `in_progress`, and the execution worker projection.
use super::*;
use crate::db::migrations;

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    migrations::run(&connection).unwrap();
    connection
}

fn seed_disc(conn: &Connection, id: &str) {
    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at) VALUES (?1, 'Plan', ?2, ?2)",
        params![id, Utc::now().to_rfc3339()],
    )
    .unwrap();
}

fn seed_cli_session(conn: &Connection, pk: i64, disc: &str, agent: &str, session: &str) {
    conn.execute(
        "INSERT INTO discussion_sessions
         (id, disc_id, agent_type, session_id, role, status, joined_at)
         VALUES (?1, ?2, ?3, ?4, 'peer', 'active', '2026-10-01')",
        params![pk, disc, agent, session],
    )
    .unwrap();
}

fn new_task(conn: &Connection, title: &str, status: PlanningTaskStatus) -> String {
    create_task(
        conn,
        &CreatePlanningTaskRequest {
            title: title.into(),
            discussion_id: None,
            idempotency_key: None,
            description: String::new(),
            status,
            priority: PlanningTaskPriority::Normal,
            parent_id: None,
            project_ids: Vec::new(),
            tags: Vec::new(),
            definition_of_done: Vec::new(),
            links: Vec::new(),
            actor: PlanningActor::default(),
        },
    )
    .unwrap()
    .summary
    .id
}

fn status_update(status: PlanningTaskStatus, actor: PlanningActor) -> UpdatePlanningTaskRequest {
    UpdatePlanningTaskRequest {
        title: None,
        description: None,
        status: Some(status),
        priority: None,
        parent_id: None,
        blocked_reason: None,
        rank: None,
        project_ids: None,
        tags: None,
        definition_of_done: None,
        links: None,
        actor,
    }
}

fn cli_assignee(
    pk: i64,
    agent_type: AgentType,
    role: Option<PlanningAssigneeRole>,
) -> PlanningAssigneeInput {
    PlanningAssigneeInput {
        kind: MessageTargetKind::Cli,
        agent_type,
        connection_id: None,
        cli_session_id: Some(pk),
        role,
    }
}

fn assign_request(assignee: PlanningAssigneeInput) -> AssignPlanningTaskRequest {
    AssignPlanningTaskRequest {
        assignee: Some(assignee),
        role: None,
        actor: PlanningActor::default(),
    }
}

fn unassign_request(assignee: PlanningAssigneeInput) -> UnassignPlanningTaskRequest {
    UnassignPlanningTaskRequest {
        assignee: Some(assignee),
        actor: PlanningActor::default(),
    }
}

fn agent_actor(agent: &str, session: &str) -> PlanningActor {
    PlanningActor {
        kind: PlanningActorKind::Agent,
        id: Some(agent.into()),
        session_id: Some(session.into()),
        source_message_id: None,
    }
}

fn assignees_of(conn: &Connection, task_id: &str) -> Vec<PlanningAssignee> {
    get_task(conn, task_id).unwrap().unwrap().summary.assignees
}

#[test]
fn two_clis_of_one_provider_are_two_distinct_assignees() {
    let conn = connection();
    seed_disc(&conn, "d-assign");
    seed_cli_session(&conn, 11, "d-assign", "Codex", "cli-a");
    seed_cli_session(&conn, 12, "d-assign", "Codex", "cli-b");
    let id = new_task(&conn, "Assign me", PlanningTaskStatus::Todo);

    assign_task(
        &conn,
        &id,
        &assign_request(cli_assignee(
            11,
            AgentType::Codex,
            Some(PlanningAssigneeRole::Owner),
        )),
    )
    .unwrap();
    let detail = assign_task(
        &conn,
        &id,
        &assign_request(cli_assignee(
            12,
            AgentType::Codex,
            Some(PlanningAssigneeRole::Reviewer),
        )),
    )
    .unwrap();

    let assignees = &detail.summary.assignees;
    assert_eq!(assignees.len(), 2, "same provider, two sessions, two rows");
    assert_eq!(assignees[0].cli_session_id, Some(11));
    assert_eq!(assignees[0].cli_ordinal, Some(1));
    assert_eq!(assignees[0].role, Some(PlanningAssigneeRole::Owner));
    assert_eq!(assignees[1].cli_session_id, Some(12));
    assert_eq!(assignees[1].cli_ordinal, Some(2));
    assert_eq!(assignees[1].role, Some(PlanningAssigneeRole::Reviewer));
    assert!(assignees
        .iter()
        .all(|assignee| assignee.source == PlanningAssigneeSource::Manual));
}

#[test]
fn assigning_twice_is_idempotent_and_only_changes_the_role() {
    let conn = connection();
    seed_disc(&conn, "d-assign");
    seed_cli_session(&conn, 11, "d-assign", "Codex", "cli-a");
    let id = new_task(&conn, "Assign me", PlanningTaskStatus::Todo);
    let owner = assign_request(cli_assignee(
        11,
        AgentType::Codex,
        Some(PlanningAssigneeRole::Owner),
    ));

    assign_task(&conn, &id, &owner).unwrap();
    assign_task(&conn, &id, &owner).unwrap();
    let events_after_repeat = get_task(&conn, &id).unwrap().unwrap().events.len();
    let detail = assign_task(
        &conn,
        &id,
        &assign_request(cli_assignee(
            11,
            AgentType::Codex,
            Some(PlanningAssigneeRole::Reviewer),
        )),
    )
    .unwrap();

    assert_eq!(detail.summary.assignees.len(), 1);
    assert_eq!(
        detail.summary.assignees[0].role,
        Some(PlanningAssigneeRole::Reviewer)
    );
    // `created` + the single effective assignment: the repeat is not journalled.
    assert_eq!(events_after_repeat, 2);
}

#[test]
fn unassign_removes_one_identity_and_refuses_an_unknown_one() {
    let conn = connection();
    seed_disc(&conn, "d-assign");
    seed_cli_session(&conn, 11, "d-assign", "Codex", "cli-a");
    seed_cli_session(&conn, 12, "d-assign", "Codex", "cli-b");
    let id = new_task(&conn, "Assign me", PlanningTaskStatus::Todo);
    for pk in [11, 12] {
        assign_task(
            &conn,
            &id,
            &assign_request(cli_assignee(pk, AgentType::Codex, None)),
        )
        .unwrap();
    }

    let detail = unassign_task(
        &conn,
        &id,
        &unassign_request(cli_assignee(11, AgentType::Codex, None)),
    )
    .unwrap();
    assert_eq!(detail.summary.assignees.len(), 1);
    assert_eq!(detail.summary.assignees[0].cli_session_id, Some(12));

    let error = unassign_task(
        &conn,
        &id,
        &unassign_request(cli_assignee(11, AgentType::Codex, None)),
    )
    .unwrap_err();
    assert!(error.to_string().contains("not found"));
}

#[test]
fn assignee_identity_is_validated_against_the_typed_regime() {
    let conn = connection();
    seed_disc(&conn, "d-assign");
    seed_cli_session(&conn, 11, "d-assign", "Codex", "cli-a");
    let id = new_task(&conn, "Assign me", PlanningTaskStatus::Todo);
    let refuse = |assignee: PlanningAssigneeInput| {
        assign_task(&conn, &id, &assign_request(assignee))
            .unwrap_err()
            .to_string()
    };

    // A CLI is its exact session, never a provider name.
    let mut bare_cli = cli_assignee(11, AgentType::Codex, None);
    bare_cli.cli_session_id = None;
    assert!(refuse(bare_cli).contains("cli_session_id"));
    // A session that does not exist, or belongs to another provider.
    assert!(refuse(cli_assignee(999, AgentType::Codex, None)).contains("not found"));
    assert!(refuse(cli_assignee(11, AgentType::ClaudeCode, None)).contains("not a"));
    // A native identity carries no session.
    let mut native_with_session = cli_assignee(11, AgentType::Codex, None);
    native_with_session.kind = MessageTargetKind::Agent;
    assert!(refuse(native_with_session).contains("cli_session_id"));
    // A Custom provider must pin its connection.
    let custom = PlanningAssigneeInput {
        kind: MessageTargetKind::Agent,
        agent_type: AgentType::Custom,
        connection_id: None,
        cli_session_id: None,
        role: None,
    };
    assert!(refuse(custom).contains("connection_id"));
    assert!(assignees_of(&conn, &id).is_empty());
}

#[test]
fn self_assignment_resolves_the_exact_joined_session() {
    let conn = connection();
    seed_disc(&conn, "d-assign");
    seed_cli_session(&conn, 11, "d-assign", "Codex", "cli-a");
    seed_cli_session(&conn, 12, "d-assign", "Codex", "cli-b");
    let id = new_task(&conn, "Assign me", PlanningTaskStatus::Todo);

    let detail = assign_task(
        &conn,
        &id,
        &AssignPlanningTaskRequest {
            assignee: None,
            role: Some(PlanningAssigneeRole::Reviewer),
            actor: agent_actor("Codex", "cli-b"),
        },
    )
    .unwrap();
    assert_eq!(detail.summary.assignees.len(), 1);
    assert_eq!(detail.summary.assignees[0].cli_session_id, Some(12));
    assert_eq!(
        detail.summary.assignees[0].role,
        Some(PlanningAssigneeRole::Reviewer)
    );

    // An unknown durable session is refused, never guessed as "some Codex".
    let error = assign_task(
        &conn,
        &id,
        &AssignPlanningTaskRequest {
            assignee: None,
            role: None,
            actor: agent_actor("Codex", "cli-unknown"),
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("cannot assign itself"));
}

#[test]
fn an_agent_taking_a_task_is_owner_unless_someone_is_already_on_it() {
    let conn = connection();
    seed_disc(&conn, "d-assign");
    seed_cli_session(&conn, 11, "d-assign", "Codex", "cli-a");
    seed_cli_session(&conn, 12, "d-assign", "Codex", "cli-b");
    let free = new_task(&conn, "Free task", PlanningTaskStatus::Todo);
    let taken = new_task(&conn, "Taken task", PlanningTaskStatus::Todo);
    let by_human = new_task(&conn, "Human task", PlanningTaskStatus::Todo);

    let detail = update_task(
        &conn,
        &free,
        &status_update(
            PlanningTaskStatus::InProgress,
            agent_actor("Codex", "cli-a"),
        ),
    )
    .unwrap();
    assert_eq!(detail.summary.assignees.len(), 1);
    assert_eq!(detail.summary.assignees[0].cli_session_id, Some(11));
    assert_eq!(
        detail.summary.assignees[0].role,
        Some(PlanningAssigneeRole::Owner)
    );

    // Somebody else owns it: the second agent does not displace them.
    assign_task(
        &conn,
        &taken,
        &assign_request(cli_assignee(
            12,
            AgentType::Codex,
            Some(PlanningAssigneeRole::Owner),
        )),
    )
    .unwrap();
    let detail = update_task(
        &conn,
        &taken,
        &status_update(
            PlanningTaskStatus::InProgress,
            agent_actor("Codex", "cli-a"),
        ),
    )
    .unwrap();
    assert_eq!(detail.summary.assignees.len(), 1);
    assert_eq!(detail.summary.assignees[0].cli_session_id, Some(12));

    // A human moving a task never invents an assignee.
    let detail = update_task(
        &conn,
        &by_human,
        &status_update(PlanningTaskStatus::InProgress, PlanningActor::default()),
    )
    .unwrap();
    assert!(detail.summary.assignees.is_empty());
}

#[test]
fn a_live_execution_worker_is_shown_without_assignment_and_vanishes_with_it() {
    let conn = connection();
    seed_disc(&conn, "d-exec");
    seed_cli_session(&conn, 21, "d-exec", "ClaudeCode", "cli-w");
    seed_cli_session(&conn, 22, "d-exec", "Codex", "cli-r");
    let id = new_task(&conn, "Executed", PlanningTaskStatus::InProgress);
    conn.execute(
        "INSERT INTO orchestration_runs
             (id, kind, discussion_id, max_review_rounds, max_concurrent_executions,
              integration_strategy, validation_json, status, created_at, updated_at)
         VALUES ('run-1', 'single_task', 'd-exec', 3, 1, 'two_phase_ff_only', '[]',
                 'active', 'now', 'now')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO task_executions
             (id, orchestration_run_id, task_id, parent_discussion_id, status,
              review_rounds, max_review_rounds, attempt_no, worker_target_kind,
              worker_cli_session_id, worker_agent_type, worker_model_tier,
              created_at, updated_at)
         VALUES ('exec-1', 'run-1', ?1, 'd-exec', 'Working', 0, 3, 0, 'cli', 21,
                 'ClaudeCode', 'reasoning', 'now', 'now')",
        [&id],
    )
    .unwrap();

    let live = assignees_of(&conn, &id);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].source, PlanningAssigneeSource::Execution);
    assert_eq!(live[0].kind, MessageTargetKind::Cli);
    assert_eq!(live[0].agent_type, AgentType::ClaudeCode);
    assert_eq!(live[0].cli_session_id, Some(21));
    assert_eq!(live[0].tier, Some(ModelTier::Reasoning));
    assert_eq!(live[0].execution_id.as_deref(), Some("exec-1"));
    assert_eq!(live[0].execution_status, Some(TaskExecutionStatus::Working));

    // The same agent assigned by hand folds into ONE row; a reviewer is added.
    assign_task(
        &conn,
        &id,
        &assign_request(cli_assignee(
            21,
            AgentType::ClaudeCode,
            Some(PlanningAssigneeRole::Owner),
        )),
    )
    .unwrap();
    assign_task(
        &conn,
        &id,
        &assign_request(cli_assignee(
            22,
            AgentType::Codex,
            Some(PlanningAssigneeRole::Reviewer),
        )),
    )
    .unwrap();
    let merged = assignees_of(&conn, &id);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].source, PlanningAssigneeSource::Manual);
    assert_eq!(merged[0].tier, Some(ModelTier::Reasoning));
    assert_eq!(merged[0].execution_id.as_deref(), Some("exec-1"));

    // The batched plan projection carries the very same list.
    link_discussion(
        &conn,
        &id,
        &LinkPlanningDiscussionRequest {
            discussion_id: "d-exec".into(),
            placement: PlanningPlacement::Active,
            is_primary: false,
            position: None,
            actor: PlanningActor::default(),
        },
    )
    .unwrap();
    let plan = get_discussion_plan(&conn, "d-exec").unwrap();
    assert_eq!(plan.active[0].task.assignees, merged);

    // Terminal execution: the derived worker is gone, manual rows remain.
    conn.execute(
        "UPDATE task_executions SET status = 'Done' WHERE id = 'exec-1'",
        [],
    )
    .unwrap();
    let after = assignees_of(&conn, &id);
    assert_eq!(after.len(), 2);
    assert!(after.iter().all(|assignee| {
        assignee.source == PlanningAssigneeSource::Manual && assignee.execution_id.is_none()
    }));
    let plan = get_discussion_plan(&conn, "d-exec").unwrap();
    assert_eq!(plan.active[0].task.assignees, after);
}
