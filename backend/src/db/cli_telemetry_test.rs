//! Tests for joined-CLI telemetry — KT-190.
//!
//! Telemetry fails by returning a number that looks fine, so these tests are
//! mostly about the cases where a wrong answer would be invisible: an absent
//! counter read as zero, a replayed report rewinding a cursor and re-collecting,
//! a coverage figure that flatters itself.

use super::*;
use crate::db::migrations;
use chrono::Duration;

fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    migrations::run(&conn).unwrap();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO projects (id, name, path, created_at, updated_at)
         VALUES ('p', 'P', '/tmp/p', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, agent, language,
             created_at, updated_at)
         VALUES ('d', 'p', 'D', 'ClaudeCode', 'fr', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn
}

fn session(conn: &Connection, agent: &str, key: &str) -> i64 {
    crate::db::discussion_sessions::create_session(conn, "d", agent, Some(key), "peer").unwrap()
}

fn row(pk: i64) -> CliSessionTelemetry {
    CliSessionTelemetry {
        cli_session_pk: pk,
        vendor: "claude-code".into(),
        provenance: "claude-code-transcript".into(),
        input_tokens: Some(16_826),
        cache_creation_tokens: Some(61_095_483),
        cache_read_tokens: Some(4_077_307_836),
        output_tokens: Some(5_367_306),
        measured_responses: Some(7_640),
        models_json: Some(r#"{"claude-opus-5":5126}"#.into()),
        window_start: Some("2026-07-27T18:06:48Z".into()),
        window_end: Some("2026-08-05T05:47:43Z".into()),
        vendor_cost_usd: None,
        read_offset: 61_869_611,
        updated_at: Utc::now().to_rfc3339(),
    }
}

#[test]
fn counters_round_trip_without_being_merged() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(pk)).unwrap();
    let stored = get(&conn, pk).unwrap().unwrap();
    assert_eq!(stored.input_tokens, Some(16_826));
    assert_eq!(stored.cache_read_tokens, Some(4_077_307_836));
    // The measured figures from the real session: traffic and billable differ
    // by a factor of ~62, which is exactly why the schema keeps them apart.
    assert_eq!(stored.traffic_tokens(), Some(4_143_787_451));
    assert_eq!(stored.billable_tokens(), Some(66_479_615));
}

#[test]
fn an_absent_counter_stays_absent_rather_than_becoming_zero() {
    // Vibe publishes no cache breakdown. A 0 here would let a dashboard state
    // that Vibe performs no cache reads.
    let conn = test_db();
    let pk = session(&conn, "Vibe", "cli-v");
    let vibe = CliSessionTelemetry {
        vendor: "vibe".into(),
        provenance: "vibe-session-meta".into(),
        input_tokens: Some(14_126_817),
        cache_creation_tokens: None,
        cache_read_tokens: None,
        output_tokens: Some(39_907),
        vendor_cost_usd: Some(21.489_528),
        read_offset: 0,
        ..row(pk)
    };
    upsert(&conn, &vibe).unwrap();
    let stored = get(&conn, pk).unwrap().unwrap();
    assert_eq!(stored.cache_read_tokens, None);
    assert_eq!(stored.cache_creation_tokens, None);
    assert_eq!(stored.vendor_cost_usd, Some(21.489_528));
}

#[test]
fn billable_is_none_when_cache_reads_were_never_measured() {
    // Without cache reads, "billable" cannot be derived. Reporting traffic as
    // if it were billable would overstate cost by ~62x on a Claude session.
    let conn = test_db();
    let pk = session(&conn, "Vibe", "cli-v");
    upsert(
        &conn,
        &CliSessionTelemetry {
            cache_read_tokens: None,
            ..row(pk)
        },
    )
    .unwrap();
    let stored = get(&conn, pk).unwrap().unwrap();
    assert!(stored.traffic_tokens().is_some());
    assert_eq!(stored.billable_tokens(), None);
}

#[test]
fn traffic_is_none_when_nothing_at_all_was_measured() {
    // Summing four absent counters to 0 would publish a figure nobody measured.
    let conn = test_db();
    let pk = session(&conn, "CopilotCli", "cli-c");
    upsert(
        &conn,
        &CliSessionTelemetry {
            vendor: "copilot".into(),
            provenance: "none".into(),
            input_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            output_tokens: None,
            ..row(pk)
        },
    )
    .unwrap();
    assert_eq!(get(&conn, pk).unwrap().unwrap().traffic_tokens(), None);
}

#[test]
fn a_real_zero_is_kept_apart_from_an_absent_counter() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(
        &conn,
        &CliSessionTelemetry {
            input_tokens: Some(0),
            cache_creation_tokens: None,
            ..row(pk)
        },
    )
    .unwrap();
    let stored = get(&conn, pk).unwrap().unwrap();
    assert_eq!(stored.input_tokens, Some(0));
    assert_eq!(stored.cache_creation_tokens, None);
}

#[test]
fn the_read_cursor_never_rewinds() {
    // A replayed or out-of-order report would otherwise re-collect a span
    // already counted, silently doubling it.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(pk)).unwrap();
    upsert(
        &conn,
        &CliSessionTelemetry {
            read_offset: 42,
            ..row(pk)
        },
    )
    .unwrap();
    assert_eq!(read_offset(&conn, pk).unwrap(), 61_869_611);
}

#[test]
fn the_cursor_advances_on_a_newer_report() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(pk)).unwrap();
    upsert(
        &conn,
        &CliSessionTelemetry {
            read_offset: 99_000_000,
            ..row(pk)
        },
    )
    .unwrap();
    assert_eq!(read_offset(&conn, pk).unwrap(), 99_000_000);
}

#[test]
fn the_window_start_is_pinned_and_the_end_follows() {
    // The session began when it began; only its end moves as it runs.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(pk)).unwrap();
    upsert(
        &conn,
        &CliSessionTelemetry {
            window_start: Some("2099-01-01T00:00:00Z".into()),
            window_end: Some("2026-08-06T00:00:00Z".into()),
            ..row(pk)
        },
    )
    .unwrap();
    let stored = get(&conn, pk).unwrap().unwrap();
    assert_eq!(stored.window_start.as_deref(), Some("2026-07-27T18:06:48Z"));
    assert_eq!(stored.window_end.as_deref(), Some("2026-08-06T00:00:00Z"));
}

#[test]
fn an_uncollected_session_reports_offset_zero() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    assert_eq!(read_offset(&conn, pk).unwrap(), 0);
    assert!(get(&conn, pk).unwrap().is_none());
}

#[test]
fn coverage_counts_unattributed_sessions_as_unknown() {
    let conn = test_db();
    let measured = session(&conn, "ClaudeCode", "cli-a");
    session(&conn, "ClaudeCode", "cli-b"); // never collected
    upsert(&conn, &row(measured)).unwrap();

    let claude = coverage(&conn)
        .unwrap()
        .into_iter()
        .find(|c| c.agent_type == "ClaudeCode")
        .unwrap();
    assert_eq!(claude.sessions, 2);
    assert_eq!(claude.attributed, 1);
    assert_eq!(claude.measured_ratio(), Some(0.5));
}

#[test]
fn coverage_does_not_credit_a_row_with_no_counters() {
    // A vendor with no collector gets a row saying "nothing measured". Counting
    // that as coverage would let the dashboard claim attribution it lacks.
    let conn = test_db();
    let pk = session(&conn, "CopilotCli", "cli-c");
    upsert(
        &conn,
        &CliSessionTelemetry {
            vendor: "copilot".into(),
            input_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            output_tokens: None,
            ..row(pk)
        },
    )
    .unwrap();
    let copilot = coverage(&conn)
        .unwrap()
        .into_iter()
        .find(|c| c.agent_type == "CopilotCli")
        .unwrap();
    assert_eq!(copilot.attributed, 1);
    assert_eq!(copilot.attributed_without_counters, 1);
    assert_eq!(copilot.measured_ratio(), Some(0.0));
}

#[test]
fn coverage_of_an_agent_with_no_sessions_is_unknown_not_zero_percent() {
    // 0% reads as a failure; "no sessions yet" is not one.
    let empty = TelemetryCoverage {
        agent_type: "Kiro".into(),
        sessions: 0,
        attributed: 0,
        attributed_without_counters: 0,
    };
    assert_eq!(empty.measured_ratio(), None);
}

// ── per-message attribution ─────────────────────────────────────────

fn cli_message(conn: &Connection, id: &str, sort_order: i64, at: &str, session_pk: i64) {
    cli_message_in_discussion(conn, "d", id, sort_order, at, session_pk);
}

fn cli_message_in_discussion(
    conn: &Connection,
    discussion_id: &str,
    id: &str,
    sort_order: i64,
    at: &str,
    session_pk: i64,
) {
    conn.execute(
        "INSERT INTO messages (id, discussion_id, role, content, timestamp,
             sort_order, tokens_used)
         VALUES (?1, ?2, 'Agent', 'hi', ?3, ?4, 0)",
        params![id, discussion_id, at, sort_order],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message_cli_authors (message_id, cli_session_id)
         VALUES (?1, ?2)",
        params![id, session_pk],
    )
    .unwrap();
}

fn usage(at: &str, output: i64) -> ResponseUsage {
    ResponseUsage {
        at: at.into(),
        input: Some(0),
        cache_creation: Some(0),
        cache_read: Some(0),
        output: Some(output),
    }
}

/// The cumulative figure. `None` when never stamped — which must stay
/// distinguishable from "cost nothing".
fn session_tokens(conn: &Connection, id: &str) -> Option<i64> {
    conn.query_row(
        "SELECT session_tokens_at_message FROM messages WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )
    .unwrap()
}

fn per_message_tokens(conn: &Connection, id: &str) -> i64 {
    conn.query_row(
        "SELECT tokens_used FROM messages WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn each_message_carries_the_running_session_total() {
    // What @user asked for: message 1 shows 20k, message 2 shows 128k, and so
    // on. A cumulative figure states what the session had spent at that instant
    // and claims nothing about the message — which is the only honest reading,
    // since the CLI also read files and ran tests between two room messages.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    cli_message(&conn, "m1", 1, "2026-08-05T10:00:00Z", pk);
    cli_message(&conn, "m2", 2, "2026-08-05T10:10:00Z", pk);
    cli_message(&conn, "m3", 3, "2026-08-05T10:20:00Z", pk);

    let stamped = attribute_to_messages(
        &conn,
        pk,
        &[
            usage("2026-08-05T09:59:00Z", 20_000),
            usage("2026-08-05T10:05:00Z", 108_000),
            usage("2026-08-05T10:15:00Z", 92_000),
        ],
        0,
    )
    .unwrap();
    assert_eq!(stamped, 3);
    assert_eq!(session_tokens(&conn, "m1"), Some(20_000));
    assert_eq!(session_tokens(&conn, "m2"), Some(128_000));
    assert_eq!(session_tokens(&conn, "m3"), Some(220_000));
}

#[test]
fn the_running_total_never_goes_down() {
    // A cumulative figure that dropped would be a visible lie: a session's spend
    // only grows.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    cli_message(&conn, "m1", 1, "2026-08-05T10:00:00Z", pk);
    cli_message(&conn, "m2", 2, "2026-08-05T10:10:00Z", pk);
    attribute_to_messages(&conn, pk, &[usage("2026-08-05T09:00:00Z", 500)], 0).unwrap();
    let first = session_tokens(&conn, "m1").unwrap();
    let second = session_tokens(&conn, "m2").unwrap();
    assert!(second >= first, "{second} < {first}");
}

#[test]
fn tokens_used_is_left_alone_for_a_cli_message() {
    // THE separation. `tokens_used` renders as "this reply cost that much"; a
    // cumulative value in that slot would be read as a per-message cost.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    cli_message(&conn, "m1", 1, "2026-08-05T10:00:00Z", pk);
    attribute_to_messages(&conn, pk, &[usage("2026-08-05T09:00:00Z", 777)], 0).unwrap();
    assert_eq!(session_tokens(&conn, "m1"), Some(777));
    assert_eq!(
        per_message_tokens(&conn, "m1"),
        0,
        "a cumulative figure landed in the per-message column",
    );
}

#[test]
fn a_baseline_from_earlier_reports_is_carried_forward() {
    // A report covers only the newest slice. Without the baseline the figures
    // would restart from zero on every report and understate the session.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    cli_message(&conn, "m1", 1, "2026-08-05T10:00:00Z", pk);
    attribute_to_messages(&conn, pk, &[usage("2026-08-05T09:00:00Z", 100)], 1_000_000).unwrap();
    assert_eq!(session_tokens(&conn, "m1"), Some(1_000_100));
}

#[test]
fn responses_after_the_last_message_do_not_raise_it() {
    // They happened after it was posted; crediting them would date the figure
    // wrong.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    cli_message(&conn, "m1", 1, "2026-08-05T10:00:00Z", pk);
    attribute_to_messages(&conn, pk, &[usage("2026-08-05T11:00:00Z", 900)], 0).unwrap();
    assert_eq!(session_tokens(&conn, "m1"), None);
}

#[test]
fn another_sessions_messages_are_never_stamped() {
    // Two CLIs of the same provider are distinct identities; matching on
    // agent_type would label a peer's message with our spend.
    let conn = test_db();
    let mine = session(&conn, "ClaudeCode", "cli-a");
    let peer = session(&conn, "ClaudeCode", "cli-b");
    cli_message(&conn, "m-peer", 1, "2026-08-05T10:00:00Z", peer);
    let stamped =
        attribute_to_messages(&conn, mine, &[usage("2026-08-05T09:00:00Z", 500)], 0).unwrap();
    assert_eq!(stamped, 0);
    assert_eq!(session_tokens(&conn, "m-peer"), None);
}

#[test]
fn a_partial_re_read_does_not_lower_a_stamped_message() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    cli_message(&conn, "m1", 1, "2026-08-05T10:00:00Z", pk);
    attribute_to_messages(&conn, pk, &[usage("2026-08-05T09:00:00Z", 100)], 0).unwrap();
    attribute_to_messages(&conn, pk, &[usage("2026-08-05T09:00:00Z", 10)], 0).unwrap();
    assert_eq!(session_tokens(&conn, "m1"), Some(100));
}

#[test]
fn an_unparseable_response_timestamp_is_skipped_not_guessed() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    cli_message(&conn, "m1", 1, "2026-08-05T10:00:00Z", pk);
    attribute_to_messages(&conn, pk, &[usage("not a date", 400)], 0).unwrap();
    assert_eq!(session_tokens(&conn, "m1"), None);
}

#[test]
fn out_of_order_responses_still_accumulate_correctly() {
    // A transcript is appended in order, but nothing guarantees the wire keeps
    // it — and a running total computed on a shuffled list would be wrong.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    cli_message(&conn, "m1", 1, "2026-08-05T10:00:00Z", pk);
    cli_message(&conn, "m2", 2, "2026-08-05T10:10:00Z", pk);
    attribute_to_messages(
        &conn,
        pk,
        &[
            usage("2026-08-05T10:05:00Z", 50),
            usage("2026-08-05T09:00:00Z", 10),
        ],
        0,
    )
    .unwrap();
    assert_eq!(session_tokens(&conn, "m1"), Some(10));
    assert_eq!(session_tokens(&conn, "m2"), Some(60));
}

#[test]
fn the_running_total_survives_the_read_path() {
    // Stored but never selected is a defect I already shipped once this session
    // (`recovered_partial` sat in the DB and never reached a caller). The column
    // is read by index, and inserting it shifted `author_cli_ordinal` — so this
    // asserts BOTH survive, not just the new one.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    cli_message(&conn, "m1", 1, "2026-08-05T10:00:00Z", pk);
    attribute_to_messages(&conn, pk, &[usage("2026-08-05T09:00:00Z", 4242)], 0).unwrap();

    let listed = crate::db::discussions::list_messages(&conn, "d").unwrap();
    let message = listed.iter().find(|m| m.id == "m1").expect("m1 listed");
    assert_eq!(message.session_tokens_at_message, Some(4242));
    // The ordinal shares the query and shifted by one when the column landed.
    assert_eq!(message.author_cli_ordinal, Some(1));
    // And the per-message column stays untouched for a CLI message.
    assert_eq!(message.tokens_used, 0);
}

#[test]
fn no_responses_and_no_messages_are_both_no_ops() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    assert_eq!(attribute_to_messages(&conn, pk, &[], 0).unwrap(), 0);
    assert_eq!(
        attribute_to_messages(&conn, pk, &[usage("2026-08-05T10:00:00Z", 5)], 0).unwrap(),
        0
    );
}

// ── rollups: a total that hides an unmeasured session is a lie ──────

fn link_task(conn: &Connection, task_number: i64, disc_id: &str) {
    let now = Utc::now().to_rfc3339();
    let task_id = format!("task-{task_number}");
    conn.execute(
        "INSERT INTO planning_tasks (id, task_number, title, status, priority, rank,
             created_at, updated_at)
         VALUES (?1, ?2, 'T', 'todo', 'normal', 1024, ?3, ?3)",
        params![task_id, task_number, now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO planning_task_discussions (task_id, discussion_id, placement,
             is_primary, position, created_at)
         VALUES (?1, ?2, 'active', 0, 0, ?3)",
        params![task_id, disc_id, now],
    )
    .unwrap();
}

#[test]
fn task_spend_reports_the_unmeasured_sessions_beside_the_total() {
    // The case that matters: one session measured, one not. Presenting only the
    // sum would read as "this is what the task cost" while half is unknown.
    let conn = test_db();
    link_task(&conn, 999, "d");
    let measured = session(&conn, "ClaudeCode", "cli-a");
    session(&conn, "Codex", "cli-b"); // no collector yet
    upsert(&conn, &row(measured)).unwrap();

    let spend = spend_by_task(&conn).unwrap();
    let task = spend.iter().find(|s| s.object_key == "KT-999").unwrap();
    assert_eq!(task.sessions, 2);
    assert_eq!(task.measured_sessions, 1);
    assert_eq!(task.unmeasured_sessions, 1);
    assert_eq!(task.traffic_tokens, Some(4_143_787_451));
    assert_eq!(task.billable_tokens, Some(66_479_615));
}

#[test]
fn a_task_with_nothing_measured_reports_none_not_zero() {
    let conn = test_db();
    link_task(&conn, 998, "d");
    session(&conn, "Codex", "cli-b");

    let spend = spend_by_task(&conn).unwrap();
    let task = spend.iter().find(|s| s.object_key == "KT-998").unwrap();
    assert_eq!(task.traffic_tokens, None, "unknown was reported as zero");
    assert_eq!(task.billable_tokens, None);
    assert_eq!(task.unmeasured_sessions, 1);
}

#[test]
fn billable_is_none_when_one_measured_session_lacks_cache_reads() {
    // Claude Code splits caches, Vibe does not. Mixing them makes billable
    // underivable: summing what is available would understate the cache share
    // instead of admitting the gap.
    let conn = test_db();
    link_task(&conn, 997, "d");
    let claude = session(&conn, "ClaudeCode", "cli-a");
    let vibe = session(&conn, "Vibe", "cli-v");
    upsert(&conn, &row(claude)).unwrap();
    upsert(
        &conn,
        &CliSessionTelemetry {
            vendor: "vibe".into(),
            provenance: "vibe-session-meta".into(),
            input_tokens: Some(14_126_817),
            cache_creation_tokens: None,
            cache_read_tokens: None,
            output_tokens: Some(39_907),
            ..row(vibe)
        },
    )
    .unwrap();

    let spend = spend_by_task(&conn).unwrap();
    let task = spend.iter().find(|s| s.object_key == "KT-997").unwrap();
    assert_eq!(task.measured_sessions, 2);
    // Traffic IS the sum of what each vendor reported — that much is honest.
    assert_eq!(task.traffic_tokens, Some(4_143_787_451 + 14_166_724));
    assert_eq!(task.billable_tokens, None, "billable was guessed");
}

#[test]
fn project_spend_rolls_up_through_the_discussion() {
    let conn = test_db();
    let measured = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(measured)).unwrap();

    let spend = spend_by_project(&conn).unwrap();
    let project = spend.iter().find(|s| s.object_key == "p").unwrap();
    assert_eq!(project.measured_sessions, 1);
    assert_eq!(project.traffic_tokens, Some(4_143_787_451));
}

#[test]
fn a_task_with_no_sessions_does_not_appear_at_all() {
    // Better absent than shown at zero: a task nobody worked on has no cost to
    // report, and a 0 would sit in a table next to real figures.
    let conn = test_db();
    link_task(&conn, 996, "d");
    let spend = spend_by_task(&conn).unwrap();
    assert!(spend.iter().all(|s| s.object_key != "KT-996"));
}

#[test]
fn deleting_a_session_removes_its_telemetry() {
    // Otherwise coverage would count rows whose session no longer exists.
    let conn = test_db();
    conn.execute("PRAGMA foreign_keys = ON", []).unwrap();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(pk)).unwrap();
    conn.execute("DELETE FROM discussion_sessions WHERE id = ?1", params![pk])
        .unwrap();
    assert!(get(&conn, pk).unwrap().is_none());
}

// ── budget assessment against real session state ────────────────────

#[test]
fn a_session_with_no_telemetry_is_unknown_not_healthy() {
    // The case that must never read as fine: a vendor with no collector. It is
    // not known to be cheap, only unwatched.
    let conn = test_db();
    let pk = session(&conn, "Codex", "cli-c");
    let out = assess_session(
        &conn,
        pk,
        &crate::core::session_budget::SessionBudget::default(),
    )
    .unwrap();
    assert_eq!(
        out.verdict,
        crate::core::session_budget::BudgetVerdict::Unknown
    );
}

#[test]
fn a_measured_runaway_session_is_told_to_rotate() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(pk)).unwrap(); // 4 143 787 451 tokens, the real figure
    let out = assess_session(
        &conn,
        pk,
        &crate::core::session_budget::SessionBudget::default(),
    )
    .unwrap();
    assert_eq!(
        out.verdict,
        crate::core::session_budget::BudgetVerdict::Rotate
    );
    assert!(out.reason.contains("traffic_tokens"), "{}", out.reason);
}

#[test]
fn active_time_ignores_a_fifteen_hour_pause_and_stays_under_the_cap() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(
        &conn,
        &CliSessionTelemetry {
            input_tokens: Some(1),
            cache_creation_tokens: Some(0),
            cache_read_tokens: Some(0),
            output_tokens: Some(1),
            measured_responses: Some(1),
            ..row(pk)
        },
    )
    .unwrap();

    // Make the old wall-clock implementation breach the 48-hour ceiling.
    conn.execute(
        "UPDATE discussion_sessions SET joined_at = ?1 WHERE id = ?2",
        params![(Utc::now() - Duration::hours(50)).to_rfc3339(), pk],
    )
    .unwrap();

    // Thirteen hours of turns every 30 minutes, then a 15-hour overnight
    // pause. The default 30-minute cap charges only the boundary after the
    // pause, not the night itself.
    let start = chrono::DateTime::parse_from_rfc3339("2026-08-05T08:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    for index in 0..=26 {
        cli_message(
            &conn,
            &format!("active-{index}"),
            index + 1,
            &(start + Duration::minutes(index * 30)).to_rfc3339(),
            pk,
        );
    }
    cli_message(
        &conn,
        "after-pause",
        28,
        &(start + Duration::hours(28)).to_rfc3339(),
        pk,
    );

    let out = assess_session(
        &conn,
        pk,
        &crate::core::session_budget::SessionBudget::default(),
    )
    .unwrap();
    assert_eq!(out.verdict, crate::core::session_budget::BudgetVerdict::Ok);
    let active = out
        .axes
        .iter()
        .find(|axis| axis.name == "active_hours")
        .unwrap();
    assert_eq!(active.current, Some(13.5));
}

#[test]
fn active_time_orders_timestamps_across_discussions_not_local_sort_order() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, agent, language,
             created_at, updated_at)
         VALUES ('d2', 'p', 'D2', 'ClaudeCode', 'fr', ?1, ?1)",
        params![now],
    )
    .unwrap();

    // sort_order is local to each discussion. The later turn in d2 has the
    // same local order as the earlier turn in d, so SQL ordering by that field
    // can put the timestamps backwards.
    cli_message_in_discussion(
        &conn,
        "d2",
        "newer-room",
        1,
        "2026-08-05T11:00:00+02:00",
        pk,
    );
    cli_message_in_discussion(&conn, "d", "older-room", 1, "2026-08-05T07:00:00Z", pk);

    assert_eq!(
        active_hours_for_session(&conn, pk, 30).unwrap(),
        Some(0.5),
        "the two-hour UTC gap is capped after chronological sorting"
    );
}

#[test]
fn inactivity_threshold_is_configurable_per_budget() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(
        &conn,
        &CliSessionTelemetry {
            input_tokens: Some(1),
            cache_creation_tokens: Some(0),
            cache_read_tokens: Some(0),
            output_tokens: Some(1),
            measured_responses: Some(1),
            ..row(pk)
        },
    )
    .unwrap();
    cli_message(&conn, "threshold-1", 1, "2026-08-05T08:00:00Z", pk);
    cli_message(&conn, "threshold-2", 2, "2026-08-05T10:00:00Z", pk);

    let budget = crate::core::session_budget::SessionBudget {
        max_inactive_gap_minutes: 15,
        ..crate::core::session_budget::SessionBudget::default()
    };
    let out = assess_session(&conn, pk, &budget).unwrap();
    let active = out
        .axes
        .iter()
        .find(|axis| axis.name == "active_hours")
        .unwrap();
    assert_eq!(active.current, Some(0.25));
}

#[test]
fn turns_count_only_this_sessions_own_messages() {
    // Counting every message in the room would charge us for a peer's turns and
    // rotate a session that barely spoke.
    let conn = test_db();
    let mine = session(&conn, "ClaudeCode", "cli-a");
    let peer = session(&conn, "ClaudeCode", "cli-b");
    for (id, order, owner) in [("m1", 1, mine), ("m2", 2, peer), ("m3", 3, peer)] {
        cli_message(&conn, id, order, "2026-08-05T10:00:00Z", owner);
    }
    let budget = crate::core::session_budget::SessionBudget {
        max_turns: 2,
        ..crate::core::session_budget::SessionBudget::default()
    };
    let out = assess_session(&conn, mine, &budget).unwrap();
    let turns = out.axes.iter().find(|axis| axis.name == "turns").unwrap();
    assert_eq!(turns.current, Some(1.0), "peer turns were counted as ours");
}

#[test]
fn a_fresh_measured_session_is_ok() {
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(
        &conn,
        &CliSessionTelemetry {
            input_tokens: Some(1_000),
            cache_creation_tokens: Some(1_000),
            cache_read_tokens: Some(1_000),
            output_tokens: Some(1_000),
            ..row(pk)
        },
    )
    .unwrap();
    let out = assess_session(
        &conn,
        pk,
        &crate::core::session_budget::SessionBudget::default(),
    )
    .unwrap();
    assert_eq!(out.verdict, crate::core::session_budget::BudgetVerdict::Ok);
}

// ── rotation metrics: the gain AND the loss ─────────────────────────

#[test]
fn metrics_exclude_unmeasured_sessions_from_the_average_and_name_them() {
    // An average over 3 of 40 sessions must not pass for a fact about all 40.
    let conn = test_db();
    let measured = session(&conn, "ClaudeCode", "cli-a");
    session(&conn, "ClaudeCode", "cli-b"); // never collected
    upsert(&conn, &row(measured)).unwrap();
    cli_message(&conn, "m1", 1, "2026-08-05T10:00:00Z", measured);

    let metrics = rotation_metrics(&conn).unwrap();
    let claude = metrics
        .iter()
        .find(|m| m.agent_type == "ClaudeCode")
        .unwrap();
    assert_eq!(claude.measured_sessions, 1);
    assert_eq!(claude.unmeasured_sessions, 1);
    // 4 143 787 451 over one turn.
    assert_eq!(claude.median_traffic_per_turn, Some(4_143_787_451));
}

#[test]
fn the_median_is_used_so_one_runaway_session_cannot_set_the_figure() {
    // The reason it is a median: this session alone was 4.1 billion tokens, and a
    // mean would let it speak for every other session.
    let conn = test_db();
    for (key, traffic) in [("cli-a", 100), ("cli-b", 200), ("cli-c", 4_000_000_000_i64)] {
        let pk = session(&conn, "ClaudeCode", key);
        upsert(
            &conn,
            &CliSessionTelemetry {
                input_tokens: Some(traffic),
                cache_creation_tokens: Some(0),
                cache_read_tokens: Some(0),
                output_tokens: Some(0),
                ..row(pk)
            },
        )
        .unwrap();
        cli_message(&conn, &format!("m-{key}"), pk, "2026-08-05T10:00:00Z", pk);
    }
    let metrics = rotation_metrics(&conn).unwrap();
    let claude = metrics
        .iter()
        .find(|m| m.agent_type == "ClaudeCode")
        .unwrap();
    assert_eq!(claude.median_traffic_per_turn, Some(200));
    // The outlier is still reported — separately, as the worst case a cap is for.
    assert_eq!(claude.worst_traffic, Some(4_000_000_000));
}

#[test]
fn turns_that_spent_nothing_measurable_are_a_continuity_signal() {
    // A session that spoke without spending is almost always one whose telemetry
    // is missing. Counting that as efficiency would be exactly the wrong
    // conclusion, so it is reported as a loss signal instead.
    let conn = test_db();
    let pk = session(&conn, "Codex", "cli-c"); // no collector
    cli_message(&conn, "m1", 1, "2026-08-05T10:00:00Z", pk);
    cli_message(&conn, "m2", 2, "2026-08-05T10:01:00Z", pk);

    let metrics = rotation_metrics(&conn).unwrap();
    let codex = metrics.iter().find(|m| m.agent_type == "Codex").unwrap();
    assert_eq!(codex.turns_without_traffic, 2);
    assert_eq!(
        codex.median_traffic_per_turn, None,
        "an average was invented"
    );
}

#[test]
fn a_session_with_no_turns_does_not_produce_a_per_turn_figure() {
    // Dividing by zero turns would invent an enormous ratio out of a session
    // that never spoke.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(pk)).unwrap(); // traffic, but no messages
    let metrics = rotation_metrics(&conn).unwrap();
    let claude = metrics
        .iter()
        .find(|m| m.agent_type == "ClaudeCode")
        .unwrap();
    assert_eq!(claude.median_traffic_per_turn, None);
    // The traffic is still visible as the worst case.
    assert_eq!(claude.worst_traffic, Some(4_143_787_451));
}

#[test]
fn metrics_on_an_empty_database_are_empty_not_zeroed() {
    let conn = test_db();
    assert!(rotation_metrics(&conn).unwrap().is_empty());
}

#[test]
fn a_recovered_fragment_reaches_the_ui_read_path() {
    // KT-251 DoD 2 — the UI can only FOLD a fragment if it knows it is one. The
    // flag existed in the DB since the boot recovery was written, but stopped at
    // the backend: its only consumers were internal SQL, so a human saw two
    // ordinary Agent bubbles and reported "three agents".
    //
    // Also pins the two neighbours, because inserting a column into an
    // index-based SELECT shifted them once already this session.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    conn.execute(
        "INSERT INTO messages (id, discussion_id, role, content, timestamp,
             sort_order, recovered_partial)
         VALUES ('m-frag', 'd', 'Agent', 'cut mid-sent', ?1, 1, 1)",
        params![Utc::now().to_rfc3339()],
    )
    .unwrap();
    cli_message(&conn, "m-real", 2, "2026-08-05T10:00:00Z", pk);
    attribute_to_messages(&conn, pk, &[usage("2026-08-05T09:00:00Z", 42)], 0).unwrap();

    let listed = crate::db::discussions::list_messages(&conn, "d").unwrap();
    let fragment = listed.iter().find(|m| m.id == "m-frag").unwrap();
    let real = listed.iter().find(|m| m.id == "m-real").unwrap();
    assert!(
        fragment.recovered_partial,
        "the fragment is invisible to the UI"
    );
    assert!(
        !real.recovered_partial,
        "a real reply was marked as a fragment"
    );
    // Neighbours in the same indexed SELECT.
    assert_eq!(real.session_tokens_at_message, Some(42));
    assert_eq!(real.author_cli_ordinal, Some(1));
}

#[test]
fn the_in_flight_answer_id_is_readable_while_it_is_still_running() {
    // KT-251 DoD 3 — reported verbatim: "je ne vois pas encore d'id, bizarre
    // d'ailleurs, faudrait qu'on l'ait direct, ça t'aurait aidé au debug". The id
    // is assigned on the FIRST checkpoint, so it must be readable before the
    // answer finishes — that is the whole point.
    let conn = test_db();
    crate::db::discussions::set_partial_response(&conn, "d", Some("half an answer"), None).unwrap();
    let id = crate::db::discussions::pending_partial_message_id(&conn, "d").unwrap();
    assert!(id.is_some(), "an in-flight answer has no id to point at");

    // Stable across checkpoints: an id that changed mid-answer would be useless
    // for debugging, which is what it exists for.
    crate::db::discussions::set_partial_response(&conn, "d", Some("more of it"), None).unwrap();
    assert_eq!(
        crate::db::discussions::pending_partial_message_id(&conn, "d").unwrap(),
        id
    );
}

#[test]
fn no_in_flight_answer_reports_no_id() {
    // Distinct from "in flight with an unknown id": a fabricated or empty id
    // would send someone looking for a message that never existed.
    let conn = test_db();
    assert_eq!(
        crate::db::discussions::pending_partial_message_id(&conn, "d").unwrap(),
        None
    );
}

#[test]
fn completing_the_answer_releases_its_in_flight_id() {
    let conn = test_db();
    crate::db::discussions::set_partial_response(&conn, "d", Some("draft"), None).unwrap();
    crate::db::discussions::set_partial_response(&conn, "d", None, None).unwrap();
    assert_eq!(
        crate::db::discussions::pending_partial_message_id(&conn, "d").unwrap(),
        None
    );
}

// ── the two figures a discussion header shows — KT-254 ───────────────

fn message(conn: &Connection, tokens: i64) {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO messages (id, discussion_id, role, content, timestamp,
             sort_order, tokens_used)
         VALUES (?1, 'd', 'Agent', 'x', ?2,
                 (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM messages), ?3)",
        params![uuid::Uuid::new_v4().to_string(), now, tokens],
    )
    .unwrap();
}

#[test]
fn the_two_figures_are_never_added_together() {
    // THE rule. A per-reply cost and a whole-session running total have different
    // units; a sum would double-count the CLI's own messages and charge this
    // discussion for work it did elsewhere.
    let conn = test_db();
    message(&conn, 1_200);
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(pk)).unwrap();

    let cost = cost_for_discussion(&conn, "d").unwrap();
    assert_eq!(cost.in_app_tokens, 1_200);
    let traffic = cost.cli_traffic_tokens.expect("no CLI figure");
    assert!(traffic > 4_000_000_000, "traffic looks wrong: {traffic}");
    // The struct offers no total, and neither figure has absorbed the other.
    assert_ne!(cost.in_app_tokens, traffic);
    assert_eq!(cost.in_app_tokens + traffic, 1_200 + traffic);
}

#[test]
fn an_unmeasured_cli_session_leaves_the_figure_unknown_not_zero() {
    let conn = test_db();
    message(&conn, 500);
    session(&conn, "Codex", "cli-b");

    let cost = cost_for_discussion(&conn, "d").unwrap();
    assert_eq!(cost.in_app_tokens, 500);
    assert_eq!(cost.cli_traffic_tokens, None, "unknown became zero");
    assert!(!cost.cli_is_known());
    assert_eq!(cost.cli_sessions, 1);
    assert_eq!(cost.cli_sessions_unmeasured, 1);
}

#[test]
fn a_discussion_with_no_cli_session_still_reports_its_agents() {
    // The in-app figure stands on its own: 0 CLI sessions is not missing data.
    let conn = test_db();
    message(&conn, 900);
    let cost = cost_for_discussion(&conn, "d").unwrap();
    assert_eq!(cost.in_app_tokens, 900);
    assert_eq!(cost.in_app_messages, 1);
    assert_eq!(cost.cli_sessions, 0);
    assert_eq!(cost.cli_traffic_tokens, None);
}

#[test]
fn a_cli_message_is_not_counted_as_an_in_app_reply() {
    // A joined CLI's messages carry tokens_used = 0 by construction. Counting them
    // would report a message count with no cost behind it.
    let conn = test_db();
    message(&conn, 0);
    message(&conn, 700);
    let cost = cost_for_discussion(&conn, "d").unwrap();
    assert_eq!(cost.in_app_messages, 1);
    assert_eq!(cost.in_app_tokens, 700);
}

#[test]
fn billable_stays_unknown_when_one_measured_session_hides_its_cache() {
    // Mixing a vendor that splits caches with one that does not makes the
    // difference underivable. Summing what is available would understate the
    // cache share instead of admitting the gap.
    let conn = test_db();
    let with_cache = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(with_cache)).unwrap();
    let without = session(&conn, "Vibe", "cli-c");
    let mut partial = row(without);
    partial.cache_read_tokens = None;
    partial.cache_creation_tokens = None;
    upsert(&conn, &partial).unwrap();

    let cost = cost_for_discussion(&conn, "d").unwrap();
    assert!(
        cost.cli_traffic_tokens.is_some(),
        "traffic is still derivable"
    );
    assert_eq!(cost.cli_billable_tokens, None, "billable was invented");
    assert_eq!(cost.cli_sessions_measured, 2);
}

#[test]
fn an_empty_discussion_costs_zero_in_app_and_unknown_on_the_cli_side() {
    // The asymmetry is deliberate: no agent replied is a fact, no CLI measurement
    // is an absence.
    let conn = test_db();
    let cost = cost_for_discussion(&conn, "d").unwrap();
    assert_eq!(cost.in_app_tokens, 0);
    assert_eq!(cost.in_app_messages, 0);
    assert_eq!(cost.cli_traffic_tokens, None);
}

#[test]
fn another_discussions_cost_does_not_leak_in() {
    let conn = test_db();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, agent, language,
             created_at, updated_at)
         VALUES ('other', 'p', 'O', 'ClaudeCode', 'fr', ?1, ?1)",
        params![now],
    )
    .unwrap();
    message(&conn, 1_000);
    let cost = cost_for_discussion(&conn, "other").unwrap();
    assert_eq!(cost.in_app_tokens, 0);
}

#[test]
fn a_sessions_running_total_is_counted_once_not_once_per_message() {
    // The 30-billion trap, exactly. `session_tokens_at_message` is CUMULATIVE, so
    // summing it over a session's messages multiplies that session's cost by its
    // message count — 4 308 007 075 x 7 was nearly published as a measurement.
    // The figure must come from the session's own row, once.
    let conn = test_db();
    let pk = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(pk)).unwrap();
    let single = cost_for_discussion(&conn, "d")
        .unwrap()
        .cli_traffic_tokens
        .unwrap();

    // Now stamp ten messages with rising running totals, as a real collector does.
    for step in 1..=10 {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, timestamp,
                 sort_order, tokens_used, session_tokens_at_message)
             VALUES (?1, 'd', 'Agent', 'x', ?2, ?3, 0, ?4)",
            params![
                uuid::Uuid::new_v4().to_string(),
                now,
                step,
                single / 10 * step
            ],
        )
        .unwrap();
    }

    let after = cost_for_discussion(&conn, "d").unwrap();
    assert_eq!(
        after.cli_traffic_tokens,
        Some(single),
        "the running total was counted once per message"
    );
    // And those CLI messages carry no per-reply cost, so the in-app figure is
    // untouched by them.
    assert_eq!(after.in_app_tokens, 0);
    assert_eq!(after.in_app_messages, 0);
}

#[test]
fn a_mixed_room_reports_both_sides_at_once() {
    // The common case, not the edge case: Kronn agents and joined CLIs in one
    // discussion. Verified rather than assumed.
    let conn = test_db();
    message(&conn, 1_240_000);
    message(&conn, 60_000);
    let measured = session(&conn, "ClaudeCode", "cli-a");
    upsert(&conn, &row(measured)).unwrap();
    session(&conn, "Codex", "cli-b"); // joined, never measured

    let cost = cost_for_discussion(&conn, "d").unwrap();
    assert_eq!(cost.in_app_tokens, 1_300_000);
    assert_eq!(cost.in_app_messages, 2);
    assert!(cost.cli_traffic_tokens.is_some());
    assert_eq!(cost.cli_sessions, 2);
    assert_eq!(cost.cli_sessions_measured, 1);
    assert_eq!(
        cost.cli_sessions_unmeasured, 1,
        "the unmeasured session must be reported, or the figure looks complete"
    );
}
