//! KT-1743 — a provider edge/auth error (the Cloudflare "Attention Required"
//! challenge) is a network blip: the turn is redispatched with a backoff and the
//! room says so. A real auth loss, or a blip that does not clear, keeps the
//! failure path — with the cause.

use super::{
    finish_dispatch_turn, transient_upstream_delay, unsuccessful_completion_diagnostic,
    TRANSIENT_UPSTREAM_BACKOFF_SECONDS,
};
use crate::api::discussions::orchestration::is_transient_upstream_auth_error;
use std::sync::Arc;

/// Recorded 13/11 09:49-09:50 UTC, both workers.
const CLOUDFLARE_403: &str =
    "Failed to authenticate. API Error: 403 Attention Required! | Cloudflare";

/// The same refusal as Kronn persists it behind its hint.
const CLOUDFLARE_403_HINTED: &str = "[Agent provider error]\n\nFailed to authenticate. API Error: 403 Attention Required! | Cloudflare\n\n(HTTP 403; terminal_reason=api_error)";

#[test]
fn recorded_cloudflare_and_edge_403_messages_are_transient() {
    for message in [
        CLOUDFLARE_403,
        CLOUDFLARE_403_HINTED,
        "API Error: 403 <!DOCTYPE html><title>Attention Required! | Cloudflare</title>",
        "API Error: 522 Connection timed out | Cloudflare",
        "API Error: 524 A timeout occurred",
        "Request failed (HTTP 403)",
    ] {
        assert!(is_transient_upstream_auth_error(message), "{message}");
    }
}

#[test]
fn a_real_auth_loss_or_a_hard_quota_is_not_transient() {
    for message in [
        "Failed to authenticate. API Error: 401 {\"type\":\"authentication_error\"}",
        "Invalid API key · Please run /login",
        "Not logged in · Please run /login",
        "OAuth token has expired. Please run /login",
        "API Error: 401 Unauthorized",
        "You've hit your usage limit (HTTP 403)",
        // The 403 of a Cloudflare page does not excuse a revoked credential.
        "authentication_error: invalid x-api-key (Cloudflare)",
        "",
        "   ",
    ] {
        assert!(!is_transient_upstream_auth_error(message), "{message}");
    }
}

#[test]
fn a_long_report_that_quotes_a_403_is_the_agents_own_work() {
    let report = format!(
        "{}\nThe client returned API Error: 403 for the forbidden route, as the test expects.",
        "Implemented the change and ran the suite. ".repeat(60)
    );
    assert!(!is_transient_upstream_auth_error(&report));
}

#[test]
fn the_backoff_is_three_retries_over_ten_minutes() {
    assert_eq!(TRANSIENT_UPSTREAM_BACKOFF_SECONDS.iter().sum::<i64>(), 600);
    assert_eq!(transient_upstream_delay(1), Some(60));
    assert_eq!(transient_upstream_delay(2), Some(180));
    assert_eq!(transient_upstream_delay(3), Some(360));
    assert_eq!(transient_upstream_delay(4), None);
    assert_eq!(transient_upstream_delay(0), Some(60));
}

#[test]
fn an_exhausted_worker_retry_ends_with_the_cause() {
    let diagnostic = unsuccessful_completion_diagnostic(CLOUDFLARE_403, true);
    assert!(
        diagnostic.contains("failure_kind=provider_upstream_error"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("403 Attention Required"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("3 automatic retries"), "{diagnostic}");
    // A genuine worker failure keeps its wording.
    let other = unsuccessful_completion_diagnostic("worker gave up", true);
    assert!(
        other.contains("failure_kind=worker_reported_failure"),
        "{other}"
    );
}

async fn state_with_room() -> crate::AppState {
    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO discussions (id, title, agent, language, created_at, updated_at, awaiting_agent)
             VALUES ('d-up', 'Worker', 'ClaudeCode', 'en', datetime('now'), datetime('now'), 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO messages
             (id, discussion_id, role, content, timestamp, sort_order, received_at)
             VALUES ('u1', 'd-up', 'User', 'work', datetime('now'), 10, datetime('now'))",
            [],
        )?;
        conn.execute(
            "UPDATE discussions SET next_message_seq = 11 WHERE id = 'd-up'",
            [],
        )?;
        crate::db::agent_dispatch::enqueue(
            conn,
            crate::db::agent_dispatch::NewAgentDispatchJob {
                id: "job-up",
                discussion_id: "d-up",
                trigger_message_id: "u1",
                trigger_sort_order: 10,
                dedupe_key: "up-test",
                agent_override: None,
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )?;
        Ok(())
    })
    .await
    .unwrap();
    crate::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

/// Claims the dispatch again, as the scheduler does once its backoff elapsed.
async fn claim(state: &crate::AppState) -> crate::db::agent_dispatch::AgentDispatchJob {
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE agent_dispatch_jobs SET available_at = '2000-01-01T00:00:00+00:00'
                 WHERE id = 'job-up' AND status = 'Pending'",
                [],
            )?;
            crate::db::agent_dispatch::claim(conn, "job-up")?
                .ok_or_else(|| anyhow::anyhow!("dispatch was not claimable"))
        })
        .await
        .unwrap()
}

async fn status(state: &crate::AppState) -> (String, Option<String>, Option<String>) {
    state
        .db
        .with_read_conn(|conn| {
            Ok(conn.query_row(
                "SELECT status, last_error, failure_kind FROM agent_dispatch_jobs WHERE id = 'job-up'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .await
        .unwrap()
}

async fn notices(state: &crate::AppState) -> Vec<String> {
    state
        .db
        .with_read_conn(|conn| crate::db::discussions::list_messages(conn, "d-up"))
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == crate::models::MessageRole::System)
        .map(|message| message.content)
        .filter(|content| content.contains("automatic retry"))
        .collect()
}

#[tokio::test]
async fn a_cloudflare_403_is_redispatched_with_backoff_and_announced() {
    let state = state_with_room().await;
    let job = claim(&state).await;
    assert_eq!(job.turn_attempts, 1);
    finish_dispatch_turn(&state, job, CLOUDFLARE_403.to_string(), false).await;

    let (dispatch_status, last_error, _) = status(&state).await;
    assert_eq!(dispatch_status, "Pending");
    assert_eq!(last_error.as_deref(), Some("transient_upstream_error"));
    let notices = notices(&state).await;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("retry 1/3"), "{}", notices[0]);
    assert!(notices[0].contains("in 60s"), "{}", notices[0]);
    assert!(notices[0].contains("Cloudflare"), "{}", notices[0]);
}

#[tokio::test]
async fn a_blip_that_never_clears_ends_failed_after_three_retries_with_the_cause() {
    let state = state_with_room().await;
    for expected_turn in 1..=3 {
        let job = claim(&state).await;
        assert_eq!(job.turn_attempts, expected_turn);
        finish_dispatch_turn(&state, job, CLOUDFLARE_403.to_string(), false).await;
        assert_eq!(status(&state).await.0, "Pending", "turn {expected_turn}");
    }
    let last = claim(&state).await;
    assert_eq!(last.turn_attempts, 4);
    finish_dispatch_turn(&state, last, CLOUDFLARE_403.to_string(), false).await;

    let (dispatch_status, last_error, _) = status(&state).await;
    assert_eq!(dispatch_status, "Failed");
    let last_error = last_error.unwrap();
    assert!(
        last_error.contains("403 Attention Required"),
        "{last_error}"
    );
    assert_eq!(notices(&state).await.len(), 3);
}

#[tokio::test]
async fn a_real_auth_loss_is_not_retried() {
    let state = state_with_room().await;
    let job = claim(&state).await;
    finish_dispatch_turn(
        &state,
        job,
        "Failed to authenticate. API Error: 401 Please run /login".to_string(),
        false,
    )
    .await;
    assert_eq!(status(&state).await.0, "Failed");
    assert!(notices(&state).await.is_empty());
}
