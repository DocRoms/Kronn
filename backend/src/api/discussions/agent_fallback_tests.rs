//! KT-1518 — a discussion turn whose provider cannot serve it continues on the
//! agent's fallback chain; a failure of the work itself never does.

use super::intercept_failed_turn_among;
use crate::api::discussions::agent_fallback_start::start_on_fallback_among;
use crate::core::agent_fallback as fallback;
use crate::db::agent_dispatch::AgentDispatchJob;
use crate::models::{AgentFallback, AgentType, FallbackGap, FallbackMode, MessageRole, ModelTier};
use std::sync::Arc;

const ROOM: &str = "d-fallback";
const CLAUDE_LIMIT: &str =
    "[Agent provider error] You've hit your limit · resets 11:59pm (UTC) (HTTP 429; terminal_reason=api_error)";

fn link(agent: AgentType) -> AgentFallback {
    AgentFallback {
        agent,
        model: None,
        mode: None,
    }
}

fn everyone() -> Vec<AgentType> {
    vec![
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::GeminiCli,
        AgentType::LiteLlm,
    ]
}

/// A room served by Claude Code, on the economy tier, whose config chains
/// Claude Code → `first` (→ `second` when given).
async fn state_with_chain(first: AgentFallback, second: Option<AgentFallback>) -> crate::AppState {
    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO discussions
             (id, title, agent, language, created_at, updated_at, awaiting_agent, model_tier)
             VALUES (?1, 'Review', 'ClaudeCode', 'en', datetime('now'), datetime('now'), 1, 'economy')",
            [ROOM],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let mut config = crate::core::config::default_config();
    config.agents.claude_code.fallback_chain = std::iter::once(first).chain(second).collect();
    // The chain of 0.15.3, carried onto the rows the way a start-up does.
    crate::core::agent_fallback::migrate_legacy_chains(&mut config.agents);
    crate::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

/// A claimed (Running) dispatch for a new user turn, with the failed reply the
/// agent left linked to it.
async fn running_dispatch(state: &crate::AppState, turn: i64) -> AgentDispatchJob {
    state
        .db
        .with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            let trigger = format!("u{turn}");
            let sort_order = turn * 10;
            conn.execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES (?1, ?2, 'User', 'review the diff', ?3, ?4, ?3)",
                rusqlite::params![trigger, ROOM, now, sort_order],
            )?;
            conn.execute(
                "UPDATE discussions SET awaiting_agent = 1, next_message_seq = ?1 + 1 WHERE id = ?2",
                rusqlite::params![sort_order, ROOM],
            )?;
            let job_id = format!("job-{turn}");
            let dedupe = format!("fallback-test-{turn}");
            crate::db::agent_dispatch::enqueue(
                conn,
                crate::db::agent_dispatch::NewAgentDispatchJob {
                    id: &job_id,
                    discussion_id: ROOM,
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
            let job = crate::db::agent_dispatch::claim(conn, &job_id)?
                .ok_or_else(|| anyhow::anyhow!("dispatch was not claimable"))?;
            conn.execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at,
                  agent_dispatch_job_id, agent_type)
                 VALUES (?1, ?2, 'Agent', 'the failed reply', ?3, ?4, ?3, ?5, 'ClaudeCode')",
                rusqlite::params![format!("a{turn}"), ROOM, now, sort_order + 1, job_id],
            )?;
            conn.execute(
                "UPDATE discussions SET next_message_seq = ?1 + 2 WHERE id = ?2",
                rusqlite::params![sort_order, ROOM],
            )?;
            Ok(job)
        })
        .await
        .unwrap()
}

/// What the dispatch row says now: status, agent override, last error.
/// The room's earlier dispatches are over: the next turn can be claimed.
async fn settle_earlier_dispatches(state: &crate::AppState) {
    state
        .db
        .with_conn(|conn| {
            conn.execute("UPDATE agent_dispatch_jobs SET status = 'Completed'", [])?;
            Ok(())
        })
        .await
        .unwrap();
}

async fn row(state: &crate::AppState, job_id: &str) -> (String, Option<String>, Option<String>) {
    let job_id = job_id.to_string();
    state
        .db
        .with_read_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT status, agent_override_json, last_error FROM agent_dispatch_jobs WHERE id = ?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        })
        .await
        .unwrap()
}

async fn room_system_lines(state: &crate::AppState) -> Vec<String> {
    state
        .db
        .with_read_conn(|conn| crate::db::discussions::list_messages(conn, ROOM))
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == MessageRole::System)
        .map(|message| message.content)
        .collect()
}

async fn replies(state: &crate::AppState) -> usize {
    state
        .db
        .with_read_conn(|conn| crate::db::discussions::list_messages(conn, ROOM))
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == MessageRole::Agent)
        .count()
}

async fn pin(state: &crate::AppState, job_id: &str) -> Option<fallback::Pin> {
    let job_id = job_id.to_string();
    state
        .db
        .with_read_conn(move |conn| fallback::get_pin(conn, &job_id))
        .await
        .unwrap()
}

async fn cooldown(state: &crate::AppState, agent: AgentType) -> Option<fallback::Cooldown> {
    state
        .db
        .with_read_conn(move |conn| fallback::active_cooldown(conn, &agent, chrono::Utc::now()))
        .await
        .unwrap()
}

// ─── A discussion turn ───────────────────────────────────────────────────

#[tokio::test]
async fn a_quota_refused_turn_continues_on_the_fallback_with_the_same_tier_and_the_room_says_so() {
    let state = state_with_chain(link(AgentType::Codex), None).await;
    let job = running_dispatch(&state, 1).await;

    assert!(intercept_failed_turn_among(&state, &job, CLAUDE_LIMIT, Some(everyone())).await);

    // The same dispatch runs again, on Codex, with the tier the room had.
    let (status, override_json, error) = row(&state, "job-1").await;
    assert_eq!(status, "Pending");
    assert_eq!(override_json.as_deref(), Some("\"Codex\""));
    assert_eq!(error.as_deref(), Some("agent_fallback:quota_exhausted"));
    let pinned = pin(&state, "job-1").await.expect("the turn is pinned");
    assert_eq!(pinned.to_agent, AgentType::Codex);
    assert_eq!(pinned.from_agent, AgentType::ClaudeCode);
    assert_eq!(pinned.tier, ModelTier::Economy);
    assert_eq!(
        pinned.model, None,
        "the refused agent's model is never carried over"
    );
    assert_eq!(pinned.hops, 1);

    // The refused reply is replaced by one line, in the room.
    assert_eq!(replies(&state).await, 0);
    let lines = room_system_lines(&state).await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("Claude Code · eco mode: quota exhausted until ")
            && lines[0].ends_with(" → continued with Codex · eco mode [CLI]"),
        "{}",
        lines[0]
    );

    // The refused agent is on cooldown until the reset the provider announced.
    let cooling = cooldown(&state, AgentType::ClaudeCode)
        .await
        .expect("cooldown");
    assert_eq!(cooling.reason, "quota_exhausted");
    assert!(
        lines[0].contains(&fallback::stamp(cooling.until)),
        "{}",
        lines[0]
    );
}

#[tokio::test]
async fn an_explicit_model_on_the_link_replaces_the_tier_in_the_room_line_and_the_pin() {
    let named = AgentFallback {
        model: Some("gpt-5.6-sol".into()),
        ..link(AgentType::Codex)
    };
    let state = state_with_chain(named, None).await;
    let job = running_dispatch(&state, 1).await;
    assert!(intercept_failed_turn_among(&state, &job, CLAUDE_LIMIT, Some(everyone())).await);
    assert_eq!(
        pin(&state, "job-1").await.unwrap().model.as_deref(),
        Some("gpt-5.6-sol")
    );
    assert!(
        room_system_lines(&state).await[0].ends_with("continued with Codex · gpt-5.6-sol [CLI]")
    );
}

#[tokio::test]
async fn each_provider_side_trigger_switches_a_room_turn() {
    let cases = [
        (
            "HTTP 429 Too Many Requests: rate_limit_exceeded",
            "rate_limit",
            "rate limited",
        ),
        (
            "Invalid authentication credentials (api error: 401)",
            "not_authenticated",
            "not authenticated",
        ),
    ];
    for (turn, (response, code, words)) in cases.into_iter().enumerate() {
        let state = state_with_chain(link(AgentType::Codex), None).await;
        let job = running_dispatch(&state, turn as i64 + 1).await;
        assert!(
            intercept_failed_turn_among(&state, &job, response, Some(everyone())).await,
            "{response}"
        );
        let (status, override_json, error) = row(&state, &job.id).await;
        assert_eq!(status, "Pending", "{response}");
        assert_eq!(override_json.as_deref(), Some("\"Codex\""), "{response}");
        assert_eq!(error, Some(format!("agent_fallback:{code}")));
        assert!(
            room_system_lines(&state).await[0]
                .starts_with(&format!("Claude Code · eco mode: {words}")),
            "{response}"
        );
    }
}

#[tokio::test]
async fn an_outage_is_retried_on_the_same_agent_first_and_only_then_falls_back() {
    let state = state_with_chain(link(AgentType::Codex), None).await;
    let job = running_dispatch(&state, 1).await;
    let outage = "[Agent provider error] API Error: 500 Internal Server Error";
    assert_eq!(job.turn_attempts, 1);

    assert!(intercept_failed_turn_among(&state, &job, outage, Some(everyone())).await);
    let (status, override_json, error) = row(&state, "job-1").await;
    assert_eq!(status, "Pending");
    assert_eq!(
        override_json, None,
        "the first outage is retried where it was"
    );
    assert_eq!(error.as_deref(), Some("provider_outage_retry"));
    assert!(pin(&state, "job-1").await.is_none());

    // The retry failed too: the retries are spent, the chain takes over.
    let spent = AgentDispatchJob {
        turn_attempts: 2,
        status: crate::db::agent_dispatch::DispatchStatus::Running,
        ..job
    };
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE agent_dispatch_jobs SET status = 'Running' WHERE id = 'job-1'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(intercept_failed_turn_among(&state, &spent, outage, Some(everyone())).await);
    let (_, override_json, error) = row(&state, "job-1").await;
    assert_eq!(override_json.as_deref(), Some("\"Codex\""));
    assert_eq!(error.as_deref(), Some("agent_fallback:outage"));
}

#[tokio::test]
async fn a_task_failure_a_refusal_a_test_failure_or_a_work_timeout_never_triggers_a_fallback() {
    let state = state_with_chain(link(AgentType::Codex), None).await;
    let job = running_dispatch(&state, 1).await;
    for failure in [
        "test result: FAILED. 12 passed; 3 failed; 0 ignored",
        "I can't help with that request.",
        "error[E0308]: mismatched types — the build failed",
        "Agent stalled (no output for 300s)",
        "Agent stalled (no output for 300s) — last line: HTTP 429 Too Many Requests",
    ] {
        assert!(
            !intercept_failed_turn_among(&state, &job, failure, Some(everyone())).await,
            "{failure}"
        );
    }
    // Nothing moved: still running on its own agent, no pin, no cooldown, no line.
    let (status, override_json, _) = row(&state, "job-1").await;
    assert_eq!(status, "Running");
    assert_eq!(override_json, None);
    assert!(pin(&state, "job-1").await.is_none());
    assert!(cooldown(&state, AgentType::ClaudeCode).await.is_none());
    assert!(room_system_lines(&state).await.is_empty());
    assert_eq!(replies(&state).await, 1, "the agent's own reply stays");
}

#[tokio::test]
async fn an_agent_without_a_fallback_keeps_the_failure_as_it_was() {
    let state = state_with_chain(link(AgentType::Codex), None).await;
    state
        .config
        .write()
        .await
        .agents
        .claude_code
        .mode_fallbacks
        .clear();
    let job = running_dispatch(&state, 1).await;
    assert!(!intercept_failed_turn_among(&state, &job, CLAUDE_LIMIT, Some(everyone())).await);
    assert_eq!(row(&state, "job-1").await.0, "Running");
    assert!(cooldown(&state, AgentType::ClaudeCode).await.is_none());
}

#[tokio::test]
async fn a_fallback_billed_by_api_key_needs_no_opt_in() {
    let state = state_with_chain(link(AgentType::LiteLlm), None).await;
    let job = running_dispatch(&state, 1).await;
    assert!(intercept_failed_turn_among(&state, &job, CLAUDE_LIMIT, Some(everyone())).await);
    assert_eq!(row(&state, "job-1").await.1.as_deref(), Some("\"LiteLlm\""));
    let lines = room_system_lines(&state).await;
    assert!(
        lines[0].ends_with("→ continued with LiteLLM · eco mode [HTTP]"),
        "{}",
        lines[0]
    );
}

#[tokio::test]
async fn a_fallback_that_is_not_installed_is_not_used() {
    let state = state_with_chain(link(AgentType::Codex), None).await;
    let job = running_dispatch(&state, 1).await;
    let only_claude = vec![AgentType::ClaudeCode];
    assert!(!intercept_failed_turn_among(&state, &job, CLAUDE_LIMIT, Some(only_claude)).await);
    assert!(room_system_lines(&state).await[0].contains("Codex is not installed or not enabled"));
}

#[tokio::test]
async fn a_fallback_that_fails_too_moves_on_to_the_next_link_with_the_same_tier() {
    let state = state_with_chain(link(AgentType::Codex), Some(link(AgentType::GeminiCli))).await;
    let job = running_dispatch(&state, 1).await;
    assert!(intercept_failed_turn_among(&state, &job, CLAUDE_LIMIT, Some(everyone())).await);

    // The dispatch is claimed again, now as Codex, and Codex is refused too.
    let second = state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE agent_dispatch_jobs SET status = 'Running' WHERE id = 'job-1'",
                [],
            )?;
            crate::db::agent_dispatch::get(conn, "job-1")
        })
        .await
        .unwrap()
        .expect("the dispatch is still there");
    assert_eq!(second.agent_override, Some(AgentType::Codex));
    assert!(
        intercept_failed_turn_among(
            &state,
            &second,
            "HTTP 429 Too Many Requests",
            Some(everyone())
        )
        .await
    );
    let pinned = pin(&state, "job-1").await.unwrap();
    assert_eq!(pinned.to_agent, AgentType::GeminiCli);
    assert_eq!(
        pinned.from_agent,
        AgentType::ClaudeCode,
        "the origin, whose chain is the one walked"
    );
    assert_eq!(pinned.hops, 2);
    assert_eq!(pinned.tier, ModelTier::Economy);
    let lines = room_system_lines(&state).await;
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Codex · eco mode: rate limited")
                && line.ends_with("continued with Gemini CLI · eco mode [CLI]")),
        "{lines:?}"
    );
}

// ─── New turns, cooldown, return ─────────────────────────────────────────

#[tokio::test]
async fn new_turns_go_straight_to_the_fallback_while_the_agent_cools_down_and_return_after() {
    let state = state_with_chain(link(AgentType::Codex), None).await;
    let first = running_dispatch(&state, 1).await;
    assert!(intercept_failed_turn_among(&state, &first, CLAUDE_LIMIT, Some(everyone())).await);

    // A new turn while Claude Code is cooling down starts on Codex.
    settle_earlier_dispatches(&state).await;
    let next = running_dispatch(&state, 2).await;
    let moved = start_on_fallback_among(&state, next, Some(everyone())).await;
    assert_eq!(moved.agent_override, Some(AgentType::Codex));
    let pinned = pin(&state, "job-2").await.expect("pinned before it runs");
    assert_eq!(pinned.tier, ModelTier::Economy);
    assert!(room_system_lines(&state).await.iter().any(|line| line
        .starts_with("Claude Code · eco mode: quota exhausted until ")
        && line.ends_with("→ continued with Codex · eco mode [CLI]")));

    // Once the reset has passed the room's own agent is used again.
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE agent_fallback_cooldowns SET until = ?1",
                [(chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339()],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    settle_earlier_dispatches(&state).await;
    let after = running_dispatch(&state, 3).await;
    let back = start_on_fallback_among(&state, after, Some(everyone())).await;
    assert_eq!(back.agent_override, None, "back on the room's own agent");
    assert!(pin(&state, "job-3").await.is_none());
}

#[tokio::test]
async fn a_turn_with_no_cooldown_anywhere_starts_where_it_always_did() {
    let state = state_with_chain(link(AgentType::Codex), None).await;
    let job = running_dispatch(&state, 1).await;
    let same = start_on_fallback_among(&state, job.clone(), Some(everyone())).await;
    assert_eq!(same.agent_override, job.agent_override);
    assert!(pin(&state, "job-1").await.is_none());
}

/// KT-2048 — the Fable refusal of 2026-11-23, verbatim.
const FABLE: &str = "claude-fable-5-2[1m]";
const FABLE_REFUSAL: &str = "[Agent provider error] You've hit your Fable limit · resets Nov 28 at 8pm (Europe/Paris). Switch to another model, or manage usage credits at https://claude.ai/admin-settings/usage, to continue. (HTTP 429; terminal_reason=api_error)";

async fn room_on_model(state: &crate::AppState, model: &str) {
    let model = model.to_string();
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE discussions SET model = ?1 WHERE id = ?2",
                rusqlite::params![model, ROOM],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

async fn cooldown_for(
    state: &crate::AppState,
    agent: AgentType,
    model: &'static str,
) -> Option<fallback::Cooldown> {
    state
        .db
        .with_read_conn(move |conn| {
            fallback::active_cooldown_for(conn, &agent, model, chrono::Utc::now())
        })
        .await
        .unwrap()
}

/// KT-2048 — a room on Fable refused for Fable: the room's Fable turns move to
/// the fallback, the agent is not cooled for its other models, and a room on
/// another model of the same agent starts where it always did.
#[tokio::test]
async fn a_room_refused_for_one_model_cools_that_model_and_not_the_agent() {
    let state = state_with_chain(link(AgentType::Codex), None).await;
    room_on_model(&state, FABLE).await;
    let first = running_dispatch(&state, 1).await;
    assert!(intercept_failed_turn_among(&state, &first, FABLE_REFUSAL, Some(everyone())).await);

    assert!(
        cooldown(&state, AgentType::ClaudeCode).await.is_none(),
        "the refusal names one model: the agent is not cooled"
    );
    assert!(cooldown_for(&state, AgentType::ClaudeCode, FABLE)
        .await
        .is_some());

    // The room's next Fable turn is served by the fallback, not called again.
    settle_earlier_dispatches(&state).await;
    let next = running_dispatch(&state, 2).await;
    let moved = start_on_fallback_among(&state, next, Some(everyone())).await;
    assert_eq!(moved.agent_override, Some(AgentType::Codex));
    assert!(pin(&state, "job-2").await.is_some());

    // A room on another model of Claude Code is not held by the Fable row.
    room_on_model(&state, "claude-opus-5-5").await;
    settle_earlier_dispatches(&state).await;
    let opus = running_dispatch(&state, 3).await;
    let same = start_on_fallback_among(&state, opus.clone(), Some(everyone())).await;
    assert_eq!(same.agent_override, opus.agent_override);
    assert!(pin(&state, "job-3").await.is_none());
}

#[tokio::test]
async fn a_discussion_override_replaces_the_agents_chain_and_an_empty_one_cuts_it() {
    // The agent's default chain is Codex; this discussion says Gemini instead.
    let state = state_with_chain(link(AgentType::Codex), None).await;
    state
        .db
        .with_conn(|conn| {
            crate::core::agent_fallback::set_discussion_override(
                conn,
                ROOM,
                FallbackMode::Eco,
                Some(&[link(AgentType::GeminiCli)]),
            )
        })
        .await
        .unwrap();
    let job = running_dispatch(&state, 1).await;
    assert!(intercept_failed_turn_among(&state, &job, CLAUDE_LIMIT, Some(everyone())).await);
    let pinned = pin(&state, "job-1").await.unwrap();
    assert_eq!(pinned.to_agent, AgentType::GeminiCli);
    assert_eq!(pinned.tier, ModelTier::Economy);

    // Another discussion cancels the fallback altogether: the failure is kept.
    let cut = state_with_chain(link(AgentType::Codex), None).await;
    cut.db
        .with_conn(|conn| {
            crate::core::agent_fallback::set_discussion_override(
                conn,
                ROOM,
                FallbackMode::Eco,
                Some(&[]),
            )
        })
        .await
        .unwrap();
    let job = running_dispatch(&cut, 1).await;
    assert!(!intercept_failed_turn_among(&cut, &job, CLAUDE_LIMIT, Some(everyone())).await);
    assert_eq!(row(&cut, "job-1").await.0, "Running");
    assert!(pin(&cut, "job-1").await.is_none());
}

#[tokio::test]
async fn the_discussion_settings_store_replace_cancel_and_remove_the_override_per_mode() {
    use crate::api::discussions::agent_fallback_override::{
        get_discussion_fallback, set_discussion_fallback, SetDiscussionFallbackRequest,
    };
    use axum::extract::{Path, State};
    use axum::Json;

    let state = state_with_chain(link(AgentType::Codex), None).await;
    let set = |mode: FallbackMode, chain: Option<Vec<AgentFallback>>| {
        set_discussion_fallback(
            State(state.clone()),
            Path(ROOM.to_string()),
            Json(SetDiscussionFallbackRequest {
                mode,
                chain_override: chain,
            }),
        )
    };
    let overridden =
        |data: &crate::api::discussions::agent_fallback_override::DiscussionFallback,
         mode: FallbackMode| {
            data.overrides
                .iter()
                .find(|entry| entry.mode == mode)
                .map(|entry| entry.chain.clone())
        };

    // No override yet: the agent's default applies.
    let read = get_discussion_fallback(State(state.clone()), Path(ROOM.to_string())).await;
    assert!(read.0.data.unwrap().overrides.is_empty());

    // Its own fallback on one mode, trimmed model, a view with the gaps.
    let own = vec![
        AgentFallback {
            model: Some("  sonnet ".into()),
            ..link(AgentType::GeminiCli)
        },
        link(AgentType::Ollama),
    ];
    let saved = set(FallbackMode::Standard, Some(own)).await.0.data.unwrap();
    let chain = overridden(&saved, FallbackMode::Standard).unwrap();
    assert_eq!(chain[0].model.as_deref(), Some("sonnet"));
    assert!(saved.views[0].links[1]
        .gaps
        .contains(&FallbackGap::CappedContext));
    assert_eq!(
        overridden(&saved, FallbackMode::Eco),
        None,
        "other modes follow the default"
    );

    // A fallback of another kind of mode, a loop or a chain past the depth is
    // refused and nothing is stored.
    let image_for_text = AgentFallback {
        mode: Some(FallbackMode::Image),
        ..link(AgentType::Codex)
    };
    let wrong = set(FallbackMode::Standard, Some(vec![image_for_text])).await;
    assert!(!wrong.0.success);
    assert!(wrong.0.error.unwrap().contains("must be a text"));
    let looped = set(
        FallbackMode::Standard,
        Some(vec![link(AgentType::Codex), link(AgentType::Codex)]),
    )
    .await;
    assert!(!looped.0.success);
    assert!(looped.0.error.unwrap().contains("loop"));
    let long: Vec<AgentFallback> = [
        AgentType::Codex,
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::Vibe,
        AgentType::CopilotCli,
    ]
    .into_iter()
    .map(link)
    .collect();
    assert!(!set(FallbackMode::Standard, Some(long)).await.0.success);
    let still = get_discussion_fallback(State(state.clone()), Path(ROOM.to_string())).await;
    assert_eq!(
        overridden(&still.0.data.unwrap(), FallbackMode::Standard)
            .unwrap()
            .len(),
        2
    );

    // An empty chain cuts the fallback on that mode; `None` goes back to the default.
    let cancelled = set(FallbackMode::Eco, Some(Vec::new()))
        .await
        .0
        .data
        .unwrap();
    assert_eq!(overridden(&cancelled, FallbackMode::Eco), Some(Vec::new()));
    assert!(overridden(&cancelled, FallbackMode::Standard).is_some());
    let removed = set(FallbackMode::Eco, None).await.0.data.unwrap();
    assert!(overridden(&removed, FallbackMode::Eco).is_none());

    // An unknown discussion is refused.
    let unknown = set_discussion_fallback(
        State(state.clone()),
        Path("nope".to_string()),
        Json(SetDiscussionFallbackRequest {
            mode: FallbackMode::Eco,
            chain_override: Some(vec![link(AgentType::Codex)]),
        }),
    )
    .await;
    assert!(!unknown.0.success);
}

// ─── KT-2012: the model the room line credits to the failing agent ───────

/// [`state_with_chain`], the room holding the exact model `claude-room-exact`
/// and Codex chained to Gemini CLI, so a mentioned Codex can fall back too.
async fn state_with_room_model() -> crate::AppState {
    let state = state_with_chain(link(AgentType::Codex), None).await;
    {
        let mut config = state.config.write().await;
        config.agents.codex.fallback_chain = vec![link(AgentType::GeminiCli)];
        crate::core::agent_fallback::migrate_legacy_chains(&mut config.agents);
    }
    state
        .db
        .with_conn(|conn| {
            crate::db::discussions::update_discussion_model(conn, ROOM, Some("claude-room-exact"))
        })
        .await
        .unwrap();
    state
}

/// The dispatch `job_id` now runs `@codex`: a mention, not the room's agent.
async fn mention_codex(state: &crate::AppState, job: &mut AgentDispatchJob) {
    let job_id = job.id.clone();
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE agent_dispatch_jobs SET agent_override_json = ?2 WHERE id = ?1",
                rusqlite::params![job_id, serde_json::to_string(&AgentType::Codex)?],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    job.agent_override = Some(AgentType::Codex);
}

#[tokio::test]
async fn the_room_line_credits_the_discussions_exact_model_to_its_own_agent() {
    let state = state_with_room_model().await;
    let job = running_dispatch(&state, 1).await;
    assert!(intercept_failed_turn_among(&state, &job, CLAUDE_LIMIT, Some(everyone())).await);
    let lines = room_system_lines(&state).await;
    assert!(
        lines[0].starts_with("Claude Code · claude-room-exact: quota exhausted"),
        "the room's own agent ran the exact model: {}",
        lines[0]
    );
}

#[tokio::test]
async fn the_room_line_never_credits_the_discussions_exact_model_to_a_mentioned_agent() {
    let state = state_with_room_model().await;
    let mut job = running_dispatch(&state, 1).await;
    mention_codex(&state, &mut job).await;
    assert!(intercept_failed_turn_among(&state, &job, CLAUDE_LIMIT, Some(everyone())).await);
    let lines = room_system_lines(&state).await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("Codex · eco mode: quota exhausted"),
        "Codex was asked to run its own mode, not the room's model: {}",
        lines[0]
    );
    assert!(!lines[0].contains("claude-room-exact"), "{}", lines[0]);
}

#[tokio::test]
async fn a_mentioned_turn_moved_off_a_cooling_agent_never_names_the_rooms_model() {
    let state = state_with_room_model().await;
    state
        .db
        .with_conn(|conn| {
            let now = chrono::Utc::now();
            fallback::set_cooldown(
                conn,
                &AgentType::Codex,
                fallback::Trigger::QuotaExhausted,
                Some(now + chrono::Duration::hours(1)),
                now,
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let mut next = running_dispatch(&state, 1).await;
    mention_codex(&state, &mut next).await;
    let moved = start_on_fallback_among(&state, next, Some(everyone())).await;
    assert_eq!(moved.agent_override, Some(AgentType::GeminiCli));
    let lines = room_system_lines(&state).await;
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Codex · eco mode: quota exhausted")),
        "{lines:?}"
    );
    assert!(
        lines.iter().all(|line| !line.contains("claude-room-exact")),
        "{lines:?}"
    );
}
