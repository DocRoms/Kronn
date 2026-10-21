//! Tests for the per-phase audit cost — KT-1084.
//!
//! The failures being closed: only the audit phase was costed; a resumed run's
//! carried steps counted twice; and a phase nobody measured reading as 0.

use super::*;
use crate::db::audit_runs::{
    carry_over_steps, finalize_audit_step, insert_audit_step_start, insert_running, set_run_model,
    set_step_cost, set_validation_discussion, StepTokens,
};
use crate::db::migrations;
use chrono::{DateTime, Duration, Utc};

fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    migrations::run(&conn).unwrap();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO projects (id, name, path, created_at, updated_at)
         VALUES ('p', 'P', '/tmp/p', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn
}

fn codex_traffic() -> StepTokens {
    StepTokens {
        input: Some(1_000_000),
        output: Some(100_000),
        cache_read: Some(600_000),
        cache_write: None,
        ..StepTokens::UNKNOWN
    }
    .inclusive_for(&crate::models::AgentType::Codex)
}

fn run_at(conn: &Connection, id: &str, kind: &str, at: DateTime<Utc>) {
    insert_running(conn, id, "p", kind, "Codex", at).unwrap();
}

fn step(conn: &Connection, run: &str, index: u32, tokens: StepTokens, cost: Option<u64>) {
    let at = Utc::now();
    insert_audit_step_start(conn, run, index, "ai/x.md", at).unwrap();
    finalize_audit_step(
        conn,
        run,
        index,
        at,
        1_000,
        &tokens,
        tokens.total(),
        true,
        None,
        false,
    )
    .unwrap();
    if let Some(micros) = cost {
        set_step_cost(conn, run, index, micros).unwrap();
    }
}

fn discussion(conn: &Connection, id: &str, title: &str, created: DateTime<Utc>) {
    let at = created.to_rfc3339();
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at)
         VALUES (?1, 'p', ?2, 'Codex', 'fr', ?3, ?3)",
        params![id, title, at],
    )
    .unwrap();
}

/// One agent reply; `usage` is (input, cache_read, cache_write, output) as
/// `message_usage` stores it, the total being what `tokens_used` carries.
fn reply(
    conn: &Connection,
    disc: &str,
    tokens_used: i64,
    cost_usd: Option<f64>,
    usage: Option<(i64, i64, Option<i64>, i64)>,
) {
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO messages (id, discussion_id, role, agent_type, content, timestamp,
             sort_order, tokens_used, cost_usd)
         VALUES (?1, ?2, 'Agent', 'Codex', 'x', ?3,
                 (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM messages), ?4, ?5)",
        params![id, disc, Utc::now().to_rfc3339(), tokens_used, cost_usd],
    )
    .unwrap();
    if let Some((input, cache_read, cache_write, output)) = usage {
        conn.execute(
            "INSERT INTO message_usage
                 (message_id, input_tokens, cache_read_tokens, cache_write_tokens, output_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, input, cache_read, cache_write, output],
        )
        .unwrap();
    }
}

#[test]
fn a_run_with_briefing_audit_and_validation_is_summed_per_phase() {
    let conn = test_db();
    let start = Utc::now() - Duration::hours(2);
    // A briefing from before the previous Full run belongs to that run, not this one.
    discussion(
        &conn,
        "old-brief",
        "Briefing projet",
        start - Duration::days(9),
    );
    reply(&conn, "old-brief", 999, Some(9.0), Some((1, 1, None, 1)));
    run_at(&conn, "old", "Full", start - Duration::days(8));

    discussion(
        &conn,
        "brief",
        "Project Briefing",
        start - Duration::minutes(10),
    );
    reply(
        &conn,
        "brief",
        3_000,
        Some(0.10),
        Some((2_000, 5_000, None, 1_000)),
    );
    run_at(&conn, "run", "Full", start);
    step(&conn, "run", 1, codex_traffic(), Some(100));
    step(&conn, "run", 2, codex_traffic(), Some(200));
    discussion(
        &conn,
        "valid",
        "Validation audit",
        start + Duration::minutes(30),
    );
    set_validation_discussion(&conn, "run", "valid").unwrap();
    reply(
        &conn,
        "valid",
        5_000,
        Some(0.25),
        Some((4_000, 0, Some(10), 1_000)),
    );
    reply(
        &conn,
        "valid",
        6_000,
        Some(0.50),
        Some((5_000, 0, None, 1_000)),
    );

    let cost = for_run(&conn, "run").unwrap().unwrap();

    let briefing = &cost.briefing;
    assert_eq!((briefing.items, briefing.measured_items), (1, 1));
    assert_eq!(briefing.total_tokens, Some(3_000));
    assert_eq!(briefing.input_tokens, Some(2_000));
    assert_eq!(briefing.cache_read_tokens, Some(5_000));
    assert_eq!(briefing.cost_usd_micros, Some(100_000));
    // A stored reply cost may be Kronn's pricing: it is never presented as reported.
    assert!(briefing.cost_estimated);

    let audit = &cost.audit;
    assert_eq!(audit.items, 2);
    assert_eq!(audit.total_tokens, Some(1_000_000), "2 x (400k + 100k)");
    assert_eq!(audit.input_tokens, Some(800_000));
    assert_eq!(audit.output_tokens, Some(200_000));
    assert_eq!(audit.cache_read_tokens, Some(1_200_000));
    assert_eq!(audit.cost_usd_micros, Some(300));
    assert!(!audit.cost_estimated, "agent-reported");

    let validation = &cost.validation;
    assert_eq!(validation.items, 2);
    assert_eq!(validation.total_tokens, Some(11_000));
    assert_eq!(validation.cache_write_tokens, Some(10));
    assert_eq!(validation.cost_usd_micros, Some(750_000));
    assert!(validation.cost_estimated);

    let total = &cost.total;
    assert_eq!(total.items, 5);
    assert_eq!(total.total_tokens, Some(1_014_000));
    assert_eq!(total.cost_usd_micros, Some(100_000 + 300 + 750_000));
    assert_eq!(total.cost_unknown_items, 0);
}

#[test]
fn a_resumed_run_does_not_count_its_carried_steps_twice() {
    let conn = test_db();
    let start = Utc::now() - Duration::hours(1);
    run_at(&conn, "a", "Full", start);
    step(&conn, "a", 1, codex_traffic(), Some(100));
    step(&conn, "a", 2, codex_traffic(), Some(200));

    // B resumes A: it inherits step 1 and runs step 2 again, for 50.
    run_at(&conn, "b", "Full", start + Duration::minutes(10));
    carry_over_steps(&conn, "b", "a", &[(1, "ai/x.md".into())], Utc::now()).unwrap();
    step(&conn, "b", 2, codex_traffic(), Some(50));

    let b = for_run(&conn, "b").unwrap().unwrap().audit;
    assert_eq!(b.items, 2);
    assert_eq!(b.carried_items, 1);
    assert_eq!(b.cost_usd_micros, Some(150), "A's step 1 once + B's step 2");
    assert_eq!(
        b.total_tokens,
        Some(1_000_000),
        "two steps, not three or four"
    );

    // C resumes B: step 1 is still A's, carried through B, and still counted once.
    run_at(&conn, "c", "Full", start + Duration::minutes(20));
    carry_over_steps(
        &conn,
        "c",
        "b",
        &[(1, "ai/x.md".into()), (2, "ai/x.md".into())],
        Utc::now(),
    )
    .unwrap();
    let c = for_run(&conn, "c").unwrap().unwrap().audit;
    assert_eq!((c.items, c.carried_items), (2, 2));
    assert_eq!(c.cost_usd_micros, Some(150));

    // The original run is untouched by the runs that carried from it.
    let a = for_run(&conn, "a").unwrap().unwrap().audit;
    assert_eq!(a.cost_usd_micros, Some(300));
}

#[test]
fn a_phase_nobody_measured_is_unmeasured_never_zero() {
    let conn = test_db();
    let start = Utc::now() - Duration::hours(1);
    run_at(&conn, "run", "Full", start);
    step(&conn, "run", 1, StepTokens::UNKNOWN, None);
    discussion(
        &conn,
        "brief",
        "Briefing projet",
        start - Duration::minutes(5),
    );
    // A joined CLI's reply: no usage was reported.
    reply(&conn, "brief", 0, None, None);
    discussion(&conn, "valid", "Validation", start + Duration::minutes(5));
    set_validation_discussion(&conn, "run", "valid").unwrap();

    let cost = for_run(&conn, "run").unwrap().unwrap();
    for phase in [&cost.briefing, &cost.audit] {
        assert_eq!((phase.items, phase.measured_items), (1, 0));
        assert_eq!(phase.total_tokens, None);
        assert_eq!(phase.input_tokens, None);
        assert_eq!(phase.cost_usd_micros, None);
        assert_eq!(phase.cost_unknown_items, 1);
    }
    // A validation discussion with no agent reply has nothing to total either.
    assert_eq!(cost.validation, AuditPhaseCost::default());
    assert_eq!(cost.total.total_tokens, None);
    assert_eq!(cost.total.cost_usd_micros, None);
    assert_eq!(cost.total.cost_unknown_items, 2);
}

#[test]
fn a_phase_with_one_unknown_cost_is_a_floor_and_says_so() {
    let conn = test_db();
    run_at(&conn, "run", "Full", Utc::now());
    step(&conn, "run", 1, codex_traffic(), Some(100));
    step(&conn, "run", 2, StepTokens::UNKNOWN, None);

    let audit = for_run(&conn, "run").unwrap().unwrap().audit;
    assert_eq!(audit.cost_usd_micros, Some(100));
    assert_eq!(audit.cost_unknown_items, 1);
    assert_eq!(audit.measured_items, 1);
}

#[test]
fn an_estimated_step_cost_marks_the_phase_estimated() {
    let conn = test_db();
    run_at(&conn, "run", "Full", Utc::now());
    set_run_model(&conn, "run", "gpt-5").unwrap();
    step(&conn, "run", 1, codex_traffic(), None);

    let audit = for_run(&conn, "run").unwrap().unwrap().audit;
    assert!(audit.cost_usd_micros.is_some_and(|micros| micros > 0));
    assert!(audit.cost_estimated);
}

#[test]
fn a_reply_with_only_a_total_counts_in_the_total_not_in_the_split() {
    let conn = test_db();
    run_at(&conn, "run", "Full", Utc::now());
    discussion(&conn, "valid", "Validation", Utc::now());
    set_validation_discussion(&conn, "run", "valid").unwrap();
    reply(&conn, "valid", 7_000, None, None);

    let validation = for_run(&conn, "run").unwrap().unwrap().validation;
    assert_eq!(validation.total_tokens, Some(7_000));
    assert_eq!(validation.input_tokens, None, "not all of it was input");
    assert_eq!(validation.split_unknown_items, 1);
    assert_eq!(validation.cost_usd_micros, None);
    assert_eq!(validation.cost_unknown_items, 1);
}

#[test]
fn a_partial_run_has_no_briefing_phase_and_an_unknown_run_no_cost() {
    let conn = test_db();
    discussion(
        &conn,
        "brief",
        "Project Briefing",
        Utc::now() - Duration::minutes(5),
    );
    reply(
        &conn,
        "brief",
        3_000,
        Some(0.10),
        Some((2_000, 5_000, None, 1_000)),
    );
    run_at(&conn, "partial", "Partial", Utc::now());
    let cost = for_run(&conn, "partial").unwrap().unwrap();
    assert_eq!(cost.briefing, AuditPhaseCost::default());
    assert!(for_run(&conn, "nope").unwrap().is_none());
}
