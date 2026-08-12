//! Tests for the telemetry ingestion endpoint — KT-190.
//!
//! Two things must hold: absence survives the round trip as absence, and a
//! report that cannot be attributed with certainty is REFUSED. Telemetry
//! attached to the wrong session is worse than telemetry that is missing,
//! because it looks like an answer.

use super::*;
use crate::core::config::default_config;
use crate::db::Database;
use std::sync::Arc;
use tokio::sync::RwLock;

async fn state_with_disc(disc_id: &'static str) -> AppState {
    let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
    db.with_conn(move |conn| {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('p-test', 'Test', '/tmp', ?1, ?1)",
            rusqlite::params![now],
        )?;
        conn.execute(
            "INSERT INTO discussions (id, project_id, title, created_at, updated_at)
             VALUES (?1, 'p-test', 'T', ?2, ?2)",
            rusqlite::params![disc_id, now],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    AppState::new_defaults(
        Arc::new(RwLock::new(default_config())),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

async fn join(state: &AppState, disc_id: &'static str, session_id: &'static str) -> i64 {
    state
        .db
        .with_conn(move |conn| {
            crate::db::discussion_sessions::create_session(
                conn,
                disc_id,
                "ClaudeCode",
                Some(session_id),
                "peer",
            )
        })
        .await
        .unwrap()
}

fn claude_report(session_id: &str) -> ReportTelemetryRequest {
    ReportTelemetryRequest {
        session_id: session_id.to_string(),
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
        timeline: Vec::new(),
    }
}

#[tokio::test]
async fn a_report_lands_and_the_counters_stay_apart() {
    let state = state_with_disc("d-t").await;
    let pk = join(&state, "d-t", "cli-a").await;

    let response = report_telemetry(
        State(state.clone()),
        Path("d-t".to_string()),
        Json(claude_report("cli-a")),
    )
    .await
    .0
    .data
    .expect("report accepted");
    assert_eq!(response.cli_session_pk, pk);
    assert_eq!(response.read_offset, 61_869_611);
    assert!(response.unmeasured.is_empty());

    let stored = state
        .db
        .with_conn(move |conn| crate::db::cli_telemetry::get(conn, pk))
        .await
        .unwrap()
        .unwrap();
    // The real measured session: traffic and billable differ by ~62x.
    assert_eq!(stored.traffic_tokens(), Some(4_143_787_451));
    assert_eq!(stored.billable_tokens(), Some(66_479_615));
}

#[tokio::test]
async fn an_absent_counter_is_stored_absent_and_echoed_back() {
    // Vibe publishes no cache split. The endpoint must not helpfully zero it.
    let state = state_with_disc("d-t").await;
    let pk = join(&state, "d-t", "cli-a").await;

    let response = report_telemetry(
        State(state.clone()),
        Path("d-t".to_string()),
        Json(ReportTelemetryRequest {
            vendor: "vibe".into(),
            provenance: "vibe-session-meta".into(),
            input_tokens: Some(14_126_817),
            cache_creation_tokens: None,
            cache_read_tokens: None,
            output_tokens: Some(39_907),
            vendor_cost_usd: Some(21.489_528),
            read_offset: 0,
            ..claude_report("cli-a")
        }),
    )
    .await
    .0
    .data
    .expect("report accepted");
    assert_eq!(
        response.unmeasured,
        vec!["cache_creation".to_string(), "cache_read".to_string()]
    );

    let stored = state
        .db
        .with_conn(move |conn| crate::db::cli_telemetry::get(conn, pk))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.cache_read_tokens, None);
    // Without cache reads, billable is not derivable — and must not be guessed.
    assert_eq!(stored.billable_tokens(), None);
    assert_eq!(stored.vendor_cost_usd, Some(21.489_528));
}

#[tokio::test]
async fn a_reported_timeline_stamps_the_session_running_total_on_each_message() {
    // End to end: the bridge sends timestamped responses, and each of that
    // session's messages ends up carrying what the SESSION had spent by then —
    // 20k, then 128k — which is what @user asked to see on a CLI bubble.
    let state = state_with_disc("d-t").await;
    let pk = join(&state, "d-t", "cli-a").await;
    state
        .db
        .with_conn(move |conn| {
            for (id, order, at) in [
                ("m1", 1, "2026-08-05T10:00:00Z"),
                ("m2", 2, "2026-08-05T10:10:00Z"),
            ] {
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content,
                         timestamp, sort_order, tokens_used)
                     VALUES (?1, 'd-t', 'Agent', 'hi', ?2, ?3, 0)",
                    rusqlite::params![id, at, order],
                )?;
                conn.execute(
                    "INSERT INTO message_cli_authors (message_id, cli_session_id)
                     VALUES (?1, ?2)",
                    rusqlite::params![id, pk],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

    let usage = |at: &str, output: i64| crate::db::cli_telemetry::ResponseUsage {
        at: at.to_string(),
        input: Some(0),
        cache_creation: Some(0),
        cache_read: Some(0),
        output: Some(output),
    };
    let response = report_telemetry(
        State(state.clone()),
        Path("d-t".to_string()),
        Json(ReportTelemetryRequest {
            timeline: vec![
                usage("2026-08-05T09:59:00Z", 20_000),
                usage("2026-08-05T10:05:00Z", 108_000),
            ],
            ..claude_report("cli-a")
        }),
    )
    .await
    .0
    .data
    .expect("report accepted");
    assert_eq!(response.messages_stamped, 2);

    let stamped: Vec<Option<i64>> = state
        .db
        .with_conn(|conn| {
            let mut statement =
                conn.prepare("SELECT session_tokens_at_message FROM messages ORDER BY sort_order")?;
            let rows: Vec<Option<i64>> = statement
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
        .unwrap();
    assert_eq!(stamped, vec![Some(20_000), Some(128_000)]);
}

#[tokio::test]
async fn the_resume_bundle_carries_the_plan_and_not_the_transcript() {
    // KT-193 DoD 3 — a fresh session must be able to carry on WITHOUT the
    // transcript. So the bundle holds the record that was written on purpose
    // (objective, open DoD, blockers) and never the conversation that produced
    // it, even when that conversation is enormous.
    let state = state_with_disc("d-bundle").await;
    state
        .db
        .with_conn(|conn| {
            let now = chrono::Utc::now().to_rfc3339();
            // A task linked as the PRIMARY objective, with one open and one
            // ticked DoD item.
            conn.execute(
                "INSERT INTO planning_tasks (id, task_number, title, description,
                     status, priority, rank, created_at, updated_at)
                 VALUES ('t-1', 777, 'Ship 0.9.4', 'Cut token spend', 'in_progress',
                     'critical', 1024, ?1, ?1)",
                rusqlite::params![now],
            )?;
            conn.execute(
                "INSERT INTO planning_task_dod_items (id, task_id, sentence, completed,
                     position, created_at, updated_at)
                 VALUES ('d-1', 't-1', 'Benchmark is green', 0, 0, ?1, ?1),
                        ('d-2', 't-1', 'Baseline measured', 1, 1, ?1, ?1)",
                rusqlite::params![now],
            )?;
            conn.execute(
                "INSERT INTO planning_task_discussions (task_id, discussion_id,
                     placement, is_primary, position, created_at)
                 VALUES ('t-1', 'd-bundle', 'active', 1, 0, ?1)",
                rusqlite::params![now],
            )?;
            // A gigantic message that must NOT reach the bundle.
            conn.execute(
                "INSERT INTO messages (id, discussion_id, role, content, timestamp,
                     sort_order)
                 VALUES ('m-huge', 'd-bundle', 'Agent', ?1, ?2, 1)",
                rusqlite::params!["S".repeat(200_000), now],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let bundle = resume_bundle(State(state.clone()), Path("d-bundle".to_string()))
        .await
        .0
        .data
        .expect("bundle built");

    assert!(bundle.objective.as_ref().unwrap().contains("Ship 0.9.4"));
    assert!(bundle
        .objective
        .as_ref()
        .unwrap()
        .contains("Cut token spend"));
    // Only the UNTICKED sentence: a fresh session needs what "done" still means,
    // not what is already done.
    assert_eq!(bundle.open_dod, vec!["Benchmark is green".to_string()]);
    // The 200 000-byte message is nowhere near it.
    assert!(
        bundle.bytes < 2_000,
        "{} B — the transcript leaked into the bundle",
        bundle.bytes
    );
    let json = serde_json::to_string(&bundle).unwrap();
    assert!(!json.contains("SSSS"), "message content reached the bundle");
}

#[tokio::test]
async fn a_discussion_with_no_plan_yields_an_empty_bundle_not_an_error() {
    // A fresh room has no objective yet. Failing here would make rotation
    // impossible exactly when it is cheapest.
    let state = state_with_disc("d-noplan").await;
    let bundle = resume_bundle(State(state.clone()), Path("d-noplan".to_string()))
        .await
        .0
        .data
        .expect("empty bundle is still a bundle");
    assert!(bundle.objective.is_none());
    assert!(bundle.open_dod.is_empty());
}

#[tokio::test]
async fn an_unknown_session_is_refused_not_stored() {
    let state = state_with_disc("d-t").await;
    let response = report_telemetry(
        State(state.clone()),
        Path("d-t".to_string()),
        Json(claude_report("never-joined")),
    )
    .await
    .0;
    assert!(response.data.is_none());
    assert!(response.error.unwrap().contains("unknown session"));
}

#[tokio::test]
async fn a_session_from_another_discussion_is_refused() {
    // A stale binding must not be able to attach numbers to the wrong room.
    let state = state_with_disc("d-t").await;
    state
        .db
        .with_conn(|conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, created_at, updated_at)
                 VALUES ('d-other', 'p-test', 'O', ?1, ?1)",
                rusqlite::params![now],
            )?;
            crate::db::discussion_sessions::create_session(
                conn,
                "d-other",
                "ClaudeCode",
                Some("cli-elsewhere"),
                "peer",
            )
        })
        .await
        .unwrap();

    let response = report_telemetry(
        State(state.clone()),
        Path("d-t".to_string()),
        Json(claude_report("cli-elsewhere")),
    )
    .await
    .0;
    assert!(response.data.is_none(), "cross-room report was accepted");
}

#[tokio::test]
async fn a_report_without_provenance_is_refused() {
    // A number whose origin is unstated cannot be audited later, which is the
    // whole difference between telemetry and a guess.
    let state = state_with_disc("d-t").await;
    join(&state, "d-t", "cli-a").await;
    let response = report_telemetry(
        State(state.clone()),
        Path("d-t".to_string()),
        Json(ReportTelemetryRequest {
            provenance: "   ".into(),
            ..claude_report("cli-a")
        }),
    )
    .await
    .0;
    assert!(response.data.is_none());
    assert!(response.error.unwrap().contains("provenance"));
}

#[tokio::test]
async fn a_negative_counter_is_refused() {
    // Storing it would poison every aggregate downstream, silently.
    let state = state_with_disc("d-t").await;
    join(&state, "d-t", "cli-a").await;
    let response = report_telemetry(
        State(state.clone()),
        Path("d-t".to_string()),
        Json(ReportTelemetryRequest {
            output_tokens: Some(-5),
            ..claude_report("cli-a")
        }),
    )
    .await
    .0;
    assert!(response.data.is_none());
    assert!(response.error.unwrap().contains("negative"));
}

#[tokio::test]
async fn a_stale_report_cannot_rewind_the_cursor() {
    // Two collectors, or one retrying out of order: the older offset must not
    // win, or the span between them is collected twice.
    let state = state_with_disc("d-t").await;
    join(&state, "d-t", "cli-a").await;
    let _first = report_telemetry(
        State(state.clone()),
        Path("d-t".to_string()),
        Json(claude_report("cli-a")),
    )
    .await;
    let response = report_telemetry(
        State(state.clone()),
        Path("d-t".to_string()),
        Json(ReportTelemetryRequest {
            read_offset: 42,
            ..claude_report("cli-a")
        }),
    )
    .await
    .0
    .data
    .expect("report accepted");
    assert_eq!(response.read_offset, 61_869_611, "cursor rewound");
}

#[tokio::test]
async fn coverage_reports_unattributed_sessions_as_unknown() {
    let state = state_with_disc("d-t").await;
    join(&state, "d-t", "cli-a").await;
    join(&state, "d-t", "cli-b").await;
    let _seed = report_telemetry(
        State(state.clone()),
        Path("d-t".to_string()),
        Json(claude_report("cli-a")),
    )
    .await;

    let rows = telemetry_coverage(State(state.clone()))
        .await
        .0
        .data
        .unwrap();
    let claude = rows
        .iter()
        .find(|row| row.agent_type == "ClaudeCode")
        .expect("ClaudeCode row");
    assert_eq!(claude.sessions, 2);
    assert_eq!(claude.attributed, 1);
    assert_eq!(claude.measured_ratio(), Some(0.5));
}
