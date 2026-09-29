//! Tests for the per-reply counters — KT-894.
//!
//! The failure being closed: a total that includes 98.6% cache reads shown as
//! what a run consumed. So the tests centre on the totals telling the cache from
//! the real input, and on every absence staying an absence — a reply that only
//! reported a total must never turn into "all of it was input".

use super::*;
use crate::core::pricing::{price_reply, TokenCounters, UnknownCost};
use crate::db::cli_telemetry::cost_for_discussion;
use crate::db::migrations;
use chrono::Utc;

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
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, agent, language,
             created_at, updated_at)
         VALUES ('d', 'p', 'D', 'Codex', 'fr', ?1, ?1)",
        params![now],
    )
    .unwrap();
    conn
}

fn message(conn: &Connection, tokens: i64, cost_usd: Option<f64>) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO messages (id, discussion_id, role, content, timestamp,
             sort_order, tokens_used, cost_usd)
         VALUES (?1, 'd', 'Agent', 'x', ?2,
                 (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM messages), ?3, ?4)",
        params![id, Utc::now().to_rfc3339(), tokens, cost_usd],
    )
    .unwrap();
    id
}

/// The real KT-837 counters, as the Codex reply path resolves them.
fn kt837() -> TokenCounters {
    TokenCounters::from_agent_report("Codex", 25_209_778, 51_617, Some(24_851_584), None).unwrap()
}

#[test]
fn the_total_is_split_into_real_input_cache_and_output() {
    let conn = test_db();
    let priced = price_reply("Codex", Some("gpt-4.1"), 25_261_395, None, Some(kt837()));
    let id = message(&conn, 25_261_395, priced.cost_usd);
    record(
        &conn,
        &id,
        &MessageUsage {
            counters: priced.counters,
            cost_unknown: priced.cost_unknown,
        },
    )
    .unwrap();

    let cost = cost_for_discussion(&conn, "d").unwrap();
    // The historical total is untouched...
    assert_eq!(cost.in_app_tokens, 25_261_395);
    // ...and no longer stands alone: the cache is told apart from the real input.
    let split = cost.in_app_breakdown.expect("counters were recorded");
    assert_eq!(split.messages, 1);
    assert_eq!(split.input_tokens, 358_194);
    assert_eq!(split.cache_read_tokens, 24_851_584);
    assert_eq!(split.output_tokens, 51_617);
    assert_eq!(split.cache_write_tokens, None, "not reported is not zero");
    assert_eq!(
        split.input_tokens + split.cache_read_tokens + split.output_tokens,
        cost.in_app_tokens,
        "the parts are disjoint and add up to the total"
    );
    assert!(
        split.cache_read_tokens > 50 * split.input_tokens,
        "the cache dwarfs the real input, which is the whole point"
    );
    assert!(cost.in_app_cost_unknown_reasons.is_empty());
}

#[test]
fn a_reply_that_only_reported_a_total_has_no_breakdown_not_all_input() {
    let conn = test_db();
    let priced = price_reply("Codex", Some("gpt-4.1"), 5_000, None, None);
    let id = message(&conn, 5_000, priced.cost_usd);
    record(
        &conn,
        &id,
        &MessageUsage {
            counters: priced.counters,
            cost_unknown: priced.cost_unknown,
        },
    )
    .unwrap();

    let cost = cost_for_discussion(&conn, "d").unwrap();
    assert_eq!(cost.in_app_tokens, 5_000);
    assert_eq!(cost.in_app_breakdown, None, "unknown became real input");
    assert_eq!(
        cost.in_app_cost_unknown_reasons,
        vec![UnknownCost::NoTokenBreakdown.reason().to_string()]
    );
}

#[test]
fn counters_without_a_cache_read_are_not_stored_as_a_split() {
    // Input that cannot be told apart from cache is not "real input".
    let conn = test_db();
    let id = message(&conn, 1_000, None);
    let counters = TokenCounters::from_disjoint_input(900, 100, None, None);
    record(
        &conn,
        &id,
        &MessageUsage {
            counters: Some(counters),
            cost_unknown: Some(UnknownCost::CacheReadNotReported),
        },
    )
    .unwrap();

    let cost = cost_for_discussion(&conn, "d").unwrap();
    assert_eq!(cost.in_app_breakdown, None);
    assert_eq!(cost.in_app_cost_unknown_reasons.len(), 1);
}

#[test]
fn the_breakdown_covers_only_the_replies_that_reported_it() {
    let conn = test_db();
    let reported = message(&conn, 1_000, Some(0.01));
    record(
        &conn,
        &reported,
        &MessageUsage {
            counters: Some(TokenCounters::from_disjoint_input(
                100,
                50,
                Some(850),
                Some(0),
            )),
            cost_unknown: None,
        },
    )
    .unwrap();
    let total_only = message(&conn, 4_000, None);
    record(
        &conn,
        &total_only,
        &MessageUsage {
            counters: None,
            cost_unknown: Some(UnknownCost::NoTokenBreakdown),
        },
    )
    .unwrap();

    let cost = cost_for_discussion(&conn, "d").unwrap();
    assert_eq!(cost.in_app_messages, 2);
    let split = cost.in_app_breakdown.unwrap();
    // One of the two replies: the reader can see the figures are partial.
    assert_eq!(split.messages, 1);
    assert_eq!(split.cache_read_tokens, 850);
    assert_eq!(split.cache_write_tokens, Some(0));
    assert!(split.messages < cost.in_app_messages);
    assert_eq!(cost.in_app_cost_unknown_reasons.len(), 1);
}

#[test]
fn a_priced_reply_carries_no_unknown_reason() {
    let conn = test_db();
    let priced = price_reply("Codex", Some("gpt-4.1"), 25_261_395, None, Some(kt837()));
    let id = message(&conn, 25_261_395, priced.cost_usd);
    record(
        &conn,
        &id,
        &MessageUsage {
            counters: priced.counters,
            cost_unknown: priced.cost_unknown,
        },
    )
    .unwrap();
    assert_eq!(
        unknown_cost_reasons(&conn, "d").unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn nothing_is_written_when_there_is_nothing_to_say() {
    let conn = test_db();
    let id = message(&conn, 0, None);
    record(
        &conn,
        &id,
        &MessageUsage {
            counters: None,
            cost_unknown: None,
        },
    )
    .unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM message_usage", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn recording_twice_replaces_the_row() {
    let conn = test_db();
    let id = message(&conn, 1_000, None);
    let first = MessageUsage {
        counters: None,
        cost_unknown: Some(UnknownCost::NoTokenBreakdown),
    };
    record(&conn, &id, &first).unwrap();
    let second = MessageUsage {
        counters: Some(TokenCounters::from_disjoint_input(
            10,
            5,
            Some(985),
            Some(0),
        )),
        cost_unknown: None,
    };
    record(&conn, &id, &second).unwrap();
    assert!(unknown_cost_reasons(&conn, "d").unwrap().is_empty());
    assert_eq!(
        breakdown_for_discussion(&conn, "d")
            .unwrap()
            .unwrap()
            .messages,
        1
    );
}

#[test]
fn deleting_the_message_removes_its_usage() {
    let conn = test_db();
    let id = message(&conn, 1_000, None);
    record(
        &conn,
        &id,
        &MessageUsage {
            counters: None,
            cost_unknown: Some(UnknownCost::NoTokenBreakdown),
        },
    )
    .unwrap();
    conn.execute("DELETE FROM messages WHERE id = ?1", [&id])
        .unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM message_usage", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}
