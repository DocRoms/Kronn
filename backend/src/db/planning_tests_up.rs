use super::*;

use crate::db::migrations;

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    migrations::run(&connection).unwrap();
    connection
}

fn request(title: &str) -> CreatePlanningTaskRequest {
    CreatePlanningTaskRequest {
        title: title.into(),
        discussion_id: None,
        idempotency_key: None,
        description: String::new(),
        status: PlanningTaskStatus::Todo,
        priority: PlanningTaskPriority::Normal,
        parent_id: None,
        project_ids: Vec::new(),
        tags: Vec::new(),
        definition_of_done: Vec::new(),
        links: Vec::new(),
        actor: PlanningActor::default(),
    }
}

fn update_request() -> UpdatePlanningTaskRequest {
    UpdatePlanningTaskRequest {
        title: None,
        description: None,
        status: None,
        priority: None,
        parent_id: None,
        blocked_reason: None,
        rank: None,
        project_ids: None,
        tags: None,
        definition_of_done: None,
        links: None,
        actor: PlanningActor::default(),
    }
}

#[test]
fn dod_ids_are_stable_and_individual_updates_do_not_clobber_siblings() {
    let connection = connection();
    let mut create = request("DoD");
    create.definition_of_done = vec![
        CreatePlanningDodItem {
            id: None,
            sentence: "First".into(),
            completed: false,
            verification: None,
        },
        CreatePlanningDodItem {
            id: None,
            sentence: "Second".into(),
            completed: false,
            verification: None,
        },
    ];
    let task = create_task(&connection, &create).unwrap();
    let first_id = task.definition_of_done[0].id.clone();
    let second_id = task.definition_of_done[1].id.clone();

    let mut replace = update_request();
    replace.definition_of_done = Some(vec![
        CreatePlanningDodItem {
            id: Some(second_id.clone()),
            sentence: "Second".into(),
            completed: false,
            verification: None,
        },
        CreatePlanningDodItem {
            id: Some(first_id.clone()),
            sentence: "First renamed".into(),
            completed: false,
            verification: None,
        },
    ]);
    let replaced = update_task(&connection, &task.summary.id, &replace).unwrap();
    assert_eq!(replaced.definition_of_done[0].id, second_id);
    assert_eq!(replaced.definition_of_done[1].id, first_id);

    update_dod_item(
        &connection,
        &task.summary.id,
        &first_id,
        &UpdatePlanningDodItemRequest {
            completed: true,
            actor: PlanningActor::default(),
            evidence: None,
        },
    )
    .unwrap();
    let updated = update_dod_item(
        &connection,
        &task.summary.id,
        &second_id,
        &UpdatePlanningDodItemRequest {
            completed: true,
            actor: PlanningActor::default(),
            evidence: None,
        },
    )
    .unwrap();
    assert!(updated.definition_of_done.iter().all(|item| item.completed));
}

/// KT-1254 — a post-integration item is a live check: listed on the summary
/// while unticked, ticked only with evidence (not via an array replace), and
/// it keeps its mode when the array is replaced without one.
#[test]
fn a_post_integration_dod_is_a_live_check_ticked_only_with_evidence() {
    let connection = connection();
    let dod = |sentence: &str, verification| CreatePlanningDodItem {
        id: None,
        sentence: sentence.into(),
        completed: false,
        verification,
    };
    let mut create = request("Live check");
    create.definition_of_done = vec![
        dod("Unit tests pass", None),
        dod(
            "Replayed on a real framing",
            Some(PlanningDodVerification::PostIntegration),
        ),
    ];
    let task = create_task(&connection, &create).unwrap();
    let (worker_id, live_id) = (
        task.definition_of_done[0].id.clone(),
        task.definition_of_done[1].id.clone(),
    );
    assert_eq!(
        task.definition_of_done[0].verification,
        PlanningDodVerification::Worker
    );
    assert_eq!(task.summary.live_checks.len(), 1);
    assert_eq!(task.summary.live_checks[0].dod_id, live_id);

    // Replacing the array without a mode keeps it, but `completed: true`
    // cannot tick a live check that has no evidence: the write is refused.
    let replacement = |live_completed: bool| {
        let mut replace = update_request();
        replace.definition_of_done = Some(vec![
            CreatePlanningDodItem {
                id: Some(worker_id.clone()),
                sentence: "Unit tests pass".into(),
                completed: true,
                verification: None,
            },
            CreatePlanningDodItem {
                id: Some(live_id.clone()),
                sentence: "Replayed on a real framing".into(),
                completed: live_completed,
                verification: None,
            },
        ]);
        replace
    };
    assert!(update_task(&connection, &task.summary.id, &replacement(true)).is_err());
    let replaced = update_task(&connection, &task.summary.id, &replacement(false)).unwrap();
    assert!(replaced.definition_of_done[0].completed);
    assert!(!replaced.definition_of_done[1].completed);
    assert_eq!(
        replaced.definition_of_done[1].verification,
        PlanningDodVerification::PostIntegration
    );

    let bare = update_dod_item(
        &connection,
        &task.summary.id,
        &live_id,
        &UpdatePlanningDodItemRequest {
            completed: true,
            actor: PlanningActor::default(),
            evidence: Some("  ".into()),
        },
    );
    assert!(bare.is_err(), "a blank evidence is no evidence");
    let ticked = update_dod_item(
        &connection,
        &task.summary.id,
        &live_id,
        &UpdatePlanningDodItemRequest {
            completed: true,
            actor: PlanningActor::default(),
            evidence: Some("replayed on a real framing: ok".into()),
        },
    )
    .unwrap();
    assert!(ticked.summary.live_checks.is_empty());
    assert_eq!(
        ticked.definition_of_done[1].evidence.as_deref(),
        Some("replayed on a real framing: ok")
    );
    // No integrated execution: ticking alone never closes a task.
    assert_eq!(ticked.summary.status, PlanningTaskStatus::Todo);
}

/// KT-1254 — an agent with no joined principal session can neither declare
/// a live check on a new task nor drop one from an existing task; a human can.
#[test]
fn an_agent_without_a_principal_session_cannot_declare_or_drop_a_live_check() {
    let connection = connection();
    let agent = PlanningActor {
        kind: PlanningActorKind::Agent,
        id: Some("ClaudeCode".into()),
        session_id: Some("sess-x".into()),
        source_message_id: None,
        token_declared: false,
    };
    let live = |sentence: &str| CreatePlanningDodItem {
        id: None,
        sentence: sentence.into(),
        completed: false,
        verification: Some(PlanningDodVerification::PostIntegration),
    };
    let mut by_agent = request("Declared by an agent");
    by_agent.definition_of_done = vec![live("Replayed")];
    by_agent.actor = agent.clone();
    assert!(create_task(&connection, &by_agent).is_err());

    let mut by_human = request("Declared by a human");
    by_human.definition_of_done = vec![live("Replayed")];
    let task = create_task(&connection, &by_human).unwrap();
    assert_eq!(task.summary.live_checks.len(), 1);

    let mut drop_all = update_request();
    drop_all.definition_of_done = Some(vec![]);
    drop_all.actor = agent;
    assert!(update_task(&connection, &task.summary.id, &drop_all).is_err());
    assert_eq!(
        get_task(&connection, &task.summary.id)
            .unwrap()
            .unwrap()
            .summary
            .live_checks
            .len(),
        1
    );
    drop_all.actor = PlanningActor::default();
    let emptied = update_task(&connection, &task.summary.id, &drop_all).unwrap();
    assert!(emptied.summary.live_checks.is_empty());
}

/// KT-1254 — unticking a live check through an array replace drops its
/// evidence and reopens a closed task; the stale proof cannot re-tick it.
#[test]
fn unticking_a_live_check_clears_its_evidence_and_reopens_the_task() {
    let connection = connection();
    let mut create = request("Untick");
    create.status = PlanningTaskStatus::InProgress;
    create.definition_of_done = vec![CreatePlanningDodItem {
        id: None,
        sentence: "Replayed on a real framing".into(),
        completed: false,
        verification: Some(PlanningDodVerification::PostIntegration),
    }];
    let task = create_task(&connection, &create).unwrap();
    let live_id = task.definition_of_done[0].id.clone();
    update_dod_item(
        &connection,
        &task.summary.id,
        &live_id,
        &UpdatePlanningDodItemRequest {
            completed: true,
            actor: PlanningActor::default(),
            evidence: Some("replayed: ok".into()),
        },
    )
    .unwrap();
    let mut close = update_request();
    close.status = Some(PlanningTaskStatus::Done);
    let closed = update_task(&connection, &task.summary.id, &close).unwrap();
    assert_eq!(closed.summary.status, PlanningTaskStatus::Done);

    let replacement = |completed: bool| {
        let mut replace = update_request();
        replace.definition_of_done = Some(vec![CreatePlanningDodItem {
            id: Some(live_id.clone()),
            sentence: "Replayed on a real framing".into(),
            completed,
            verification: None,
        }]);
        replace
    };
    // An agent with no principal session cannot drop the proof.
    let mut by_agent = replacement(false);
    by_agent.actor = PlanningActor {
        kind: PlanningActorKind::Agent,
        id: Some("ClaudeCode".into()),
        session_id: Some("sess-x".into()),
        source_message_id: None,
        token_declared: false,
    };
    assert!(update_task(&connection, &task.summary.id, &by_agent).is_err());
    let untouched = get_task(&connection, &task.summary.id).unwrap().unwrap();
    assert!(untouched.definition_of_done[0].completed);
    assert_eq!(untouched.summary.status, PlanningTaskStatus::Done);

    let unticked = update_task(&connection, &task.summary.id, &replacement(false)).unwrap();
    assert!(!unticked.definition_of_done[0].completed);
    assert_eq!(unticked.definition_of_done[0].evidence, None);
    assert_eq!(unticked.summary.status, PlanningTaskStatus::InProgress);
    assert_eq!(unticked.summary.live_checks.len(), 1);

    // The proof went with the tick: re-ticking by array replace is refused.
    assert!(update_task(&connection, &task.summary.id, &replacement(true)).is_err());
}
