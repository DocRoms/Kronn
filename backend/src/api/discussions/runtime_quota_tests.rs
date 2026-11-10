//! KT-1517 — a native agent whose provider account is out of quota or credits ends
//! its turn failed, tells the room why, and shows the state on its card.

use super::{finish_dispatch_turn, provider_quota_message};
use crate::db::discussion_sessions::{list_participant_views, PresenceState};
use std::sync::Arc;

const CODEX_CLI: &str = "ERROR: You've hit your usage limit. Upgrade to Pro (https://openai.com/chatgpt/pricing) or try again in 2 hours 13 minutes.";

async fn state_with_room() -> crate::AppState {
    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO discussions (id, title, agent, language, created_at, updated_at, awaiting_agent)
             VALUES ('d-quota', 'Review', 'Codex', 'en', datetime('now'), datetime('now'), 1)",
            [],
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

/// A claimed (Running) dispatch for a new user turn in the room.
async fn running_dispatch(
    state: &crate::AppState,
    turn: i64,
) -> crate::db::agent_dispatch::AgentDispatchJob {
    state
        .db
        .with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            let trigger = format!("u{turn}");
            // Ten slots per turn: the cards and replies a turn writes never
            // meet the next turn's message.
            let sort_order = turn * 10;
            conn.execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES (?1, 'd-quota', 'User', 'review', ?2, ?3, ?2)",
                rusqlite::params![trigger, now, sort_order],
            )?;
            // The sequence the room hands the next message: raw inserts skip it.
            conn.execute(
                "UPDATE discussions SET awaiting_agent = 1, next_message_seq = ?1 + 1
                 WHERE id = 'd-quota'",
                [sort_order],
            )?;
            let job_id = format!("job-{turn}");
            let dedupe = format!("quota-test-{turn}");
            crate::db::agent_dispatch::enqueue(
                conn,
                crate::db::agent_dispatch::NewAgentDispatchJob {
                    id: &job_id,
                    discussion_id: "d-quota",
                    trigger_message_id: &trigger,
                    trigger_sort_order: sort_order,
                    dedupe_key: &dedupe,
                    agent_override: None,
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: None,
                    group_concurrency_limit: None,
                },
            )?;
            crate::db::agent_dispatch::claim(conn, &job_id)?
                .ok_or_else(|| anyhow::anyhow!("dispatch was not claimable"))
        })
        .await
        .unwrap()
}

async fn quota_cards(state: &crate::AppState) -> Vec<serde_json::Value> {
    state
        .db
        .with_read_conn(|conn| crate::db::discussions::list_messages(conn, "d-quota"))
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == crate::models::MessageRole::System)
        .filter_map(|message| {
            message
                .content
                .strip_prefix("[kronn:agent-error]\n")
                .map(|json| serde_json::from_str(json).unwrap())
        })
        .collect()
}

async fn codex_presence(state: &crate::AppState) -> Option<PresenceState> {
    state
        .db
        .with_read_conn(|conn| list_participant_views(conn, "d-quota"))
        .await
        .unwrap()
        .into_iter()
        .find(|participant| participant.agent_type == "Codex")
        .map(|participant| participant.presence_state)
}

async fn settled(state: &crate::AppState, job_id: &'static str) -> (String, Option<String>, bool) {
    state
        .db
        .with_read_conn(move |conn| {
            let (status, kind) = conn.query_row(
                "SELECT status, failure_kind FROM agent_dispatch_jobs WHERE id = ?1",
                [job_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )?;
            let awaiting = conn.query_row(
                "SELECT awaiting_agent FROM discussions WHERE id = 'd-quota'",
                [],
                |row| row.get::<_, bool>(0),
            )?;
            Ok((status, kind, awaiting))
        })
        .await
        .unwrap()
}

/// Every shape a native agent ends a quota-refused turn in: the provider's own
/// wording is what the card must quote, whether the reply is the raw text
/// (Claude Code) or Kronn's hint with the raw output folded behind it (the
/// streaming path, Codex echoing its prompt first).
fn shapes() -> Vec<(&'static str, String, &'static str)> {
    vec![
        (
            "Codex CLI usage limit, folded behind the hint",
            format!(
                "⛔ **Limite du plan atteinte.** Le quota/les crédits de cet agent sont épuisés.\nRecharge des crédits ou attends le reset, puis relance.\n\n<!-- kronn:context title=\"détails techniques\" -->\nOpenAI Codex v0.1\nuser\nReview the diff and publish findings\n{CODEX_CLI}\n<!-- /kronn:context -->"
            ),
            CODEX_CLI,
        ),
        (
            "Codex over an API key",
            "You exceeded your current quota, please check your plan and billing details."
                .to_string(),
            "You exceeded your current quota, please check your plan and billing details.",
        ),
        (
            "Claude Code monthly spend limit",
            "[Agent provider error]\n\nYou've hit your org's monthly spend limit · run /usage-credits to manage your plan.\n\n(HTTP 429; terminal_reason=api_error)".to_string(),
            "You've hit your org's monthly spend limit · run /usage-credits to manage your plan.",
        ),
        (
            "Claude Code prepaid balance",
            "[Agent provider error]\n\nYour credit balance is too low to access the Anthropic API. Please go to Plans & Billing.\n\n(HTTP 400; terminal_reason=api_error)".to_string(),
            "Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing.",
        ),
        (
            "Claude Code extra usage",
            "You're out of extra usage · resets 4pm (Europe/Paris)".to_string(),
            "You're out of extra usage · resets 4pm (Europe/Paris)",
        ),
    ]
}

#[test]
fn the_provider_message_is_the_line_that_carries_the_quota_signal() {
    for (name, response, expected) in shapes() {
        assert_eq!(provider_quota_message(&response), expected, "{name}");
    }
    // No recognisable line: the start of the raw output, never nothing.
    assert_eq!(provider_quota_message("  plain refusal  "), "plain refusal");
}

/// DoD 1 — a quota refusal ends the turn failed and leaves a System card with the
/// provider's message and the way out, instead of a room that looks
/// idle-but-working.
#[tokio::test]
async fn a_quota_refused_turn_fails_and_leaves_a_card_with_the_provider_message() {
    for (name, response, expected) in shapes() {
        let state = state_with_room().await;
        let job = running_dispatch(&state, 1).await;
        finish_dispatch_turn(&state, job, response, false).await;

        let (status, kind, awaiting) = settled(&state, "job-1").await;
        assert_eq!(status, "Failed", "{name}");
        assert_eq!(kind.as_deref(), Some("quota_exhausted"), "{name}");
        assert!(
            !awaiting,
            "{name}: the room must not stay awaiting the agent"
        );

        let cards = quota_cards(&state).await;
        assert_eq!(cards.len(), 1, "{name}: {cards:?}");
        let card = &cards[0];
        assert_eq!(card["kind"], "agent_error", "{name}");
        assert_eq!(card["detail"], expected, "{name}");
        let summary = card["summary"].as_str().unwrap();
        assert!(summary.contains(expected), "{name}: {summary}");
        assert!(summary.contains("Codex"), "{name}: {summary}");
        assert!(summary.contains("another agent"), "{name}: {summary}");
        assert!(summary.contains("retry"), "{name}: {summary}");
        assert_eq!(card["retry_dispatch_id"], "job-1", "{name}");
    }
}

/// DoD 2 — the agent shows the quota state while it lasts, and a later
/// successful turn clears it.
#[tokio::test]
async fn the_quota_state_shows_on_the_agent_until_a_later_turn_succeeds() {
    let state = state_with_room().await;
    assert_eq!(codex_presence(&state).await, None);

    let refused = running_dispatch(&state, 1).await;
    finish_dispatch_turn(&state, refused, CODEX_CLI.to_string(), false).await;
    assert_eq!(
        codex_presence(&state).await,
        Some(PresenceState::QuotaExhausted)
    );

    let next = running_dispatch(&state, 2).await;
    finish_dispatch_turn(&state, next, "Findings published.".to_string(), true).await;
    let (status, _, _) = settled(&state, "job-2").await;
    assert_eq!(status, "Completed");
    assert_ne!(
        codex_presence(&state).await,
        Some(PresenceState::QuotaExhausted),
        "a successful turn must clear the quota badge"
    );
}
