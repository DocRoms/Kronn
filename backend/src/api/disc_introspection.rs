//! Discussion introspection — 3 endpoints that expose the conversation
//! as a queryable resource for the agent.
//!
//! Pre-fix Kronn ran an opportunistic auto-summary loop after every
//! agent reply (cf. `orchestration::maybe_generate_summary`). On big-
//! context models the summary fired before the agent ever needed it,
//! burning ~500-2000 tokens per call for nothing. The shipping
//! `summary_strategy` enum (DB migration 047) lets the user disable
//! that behaviour per-discussion.
//!
//! These endpoints close the loop: once auto-fire is off, the agent
//! decides at runtime whether it needs a summary, a specific message,
//! or just metadata. Routes:
//!
//! - `GET    /api/discussions/{id}/meta`               — counts + flags
//! - `GET    /api/discussions/{id}/message/{idx}`      — single message
//! - `POST   /api/discussions/{id}/summarize`          — on-demand summary
//!
//! The transport for the agent is an MCP stdio bridge
//! (`backend/scripts/disc-introspection-mcp.py`) auto-wired into the
//! per-disc `.mcp.json` when `summary_strategy != Off`. The bridge
//! turns each MCP tool call into one HTTP call against the routes
//! above, so the actual data lives in this single Rust module.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::*;
use crate::AppState;

/// Compact metadata returned by `disc_meta` — everything the agent might
/// need to decide whether to fetch context, without leaking the full
/// transcript. `tokens_used_total` is the cumulative billed token count
/// for the discussion (sum of every message's `tokens_used`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionMeta {
    pub id: String,
    pub title: String,
    pub agent: AgentType,
    pub tier: ModelTier,
    pub message_count: u32,
    pub tokens_used_total: u64,
    pub summary_strategy: SummaryStrategy,
    pub has_cached_summary: bool,
    /// 0-indexed position of the last message included in
    /// `summary_cache`. `None` means no summary has been generated yet.
    pub summary_up_to_msg_idx: Option<u32>,
    /// Number of non-system messages added since the cached summary was
    /// last refreshed. Lets the agent gauge whether the summary is fresh
    /// enough to trust.
    pub msgs_since_last_summary: u32,
    pub language: String,
    /// Long-poll pacing contract for multi-agent rooms (stab-1).
    #[serde(default)]
    pub poll_policy: PollBackoffPolicy,
    /// stab-3 — current server-computed regime for this disc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pacing: Option<PacingState>,
    pub project_id: Option<String>,
}

/// Single-message read shape — same fields as the underlying
/// `DiscussionMessage` minus internal-only metadata.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionMessageRead {
    pub idx: u32,
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    pub agent_type: Option<AgentType>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub tokens_used: u64,
    /// Files attached to this message (0.8.8). Lets an agent that navigates to
    /// an old message see what was uploaded with it instead of being blind to
    /// a discussed image. Empty for messages with no attachments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MessageAttachment>,
}

/// stab-1 (Romu) — EXPLICIT long-poll pacing contract, returned by
/// `disc_meta` and `peer-join` instead of living as an implicit convention
/// in each agent's prompt. Agents walk `poll_backoff_seconds` while the
/// room is silent (staying on the last value once exhausted) and reset to
/// the first entry as soon as a peer message arrives.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PollBackoffPolicy {
    pub poll_backoff_seconds: Vec<u32>,
    pub reset_on_peer_message: bool,
    pub max_delay_seconds: u32,
    /// stab-3 — poll interval while a HUMAN attention lease is active
    /// (a User message opens/renews the lease). Debated Claude/Codex,
    /// Romu's requirement: sub-minute answers while he is present.
    pub hot_poll_seconds: u32,
    /// How long a User message keeps the room in the hot regime.
    pub user_attention_lease_seconds: u32,
}

impl Default for PollBackoffPolicy {
    fn default() -> Self {
        Self {
            poll_backoff_seconds: vec![30, 30, 60, 60, 120, 120, 240, 240, 480],
            reset_on_peer_message: true,
            max_delay_seconds: 480,
            hot_poll_seconds: 40,
            user_attention_lease_seconds: 1800,
        }
    }
}

/// stab-3 — pacing state COMPUTED BY THE SERVER (the agents apply it, they
/// don't interpret): hot while the last User message is within the lease,
/// cold otherwise. In BOTH regimes `next_delay_seconds` is the instruction
/// to apply verbatim — in cold it is the backoff-ramp step derived
/// statelessly from the elapsed silence (see `pacing_for`).
/// Closed set — a typo'd regime must fail to COMPILE, and the generated TS
/// side gets the `"hot" | "cold"` union instead of `string` (Copilot round 5).
/// Wire format stays the lowercase string the bridge already reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum PacingRegime {
    Hot,
    Cold,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PacingState {
    pub regime: PacingRegime,
    pub next_delay_seconds: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attention_until: Option<String>,
}

impl Default for PacingState {
    /// Conservative cold-cap placeholder — response builders overwrite it
    /// with the real `pacing_for` result; a leaked default can only slow
    /// an agent down, never make it hammer the server.
    fn default() -> Self {
        Self {
            regime: PacingRegime::Cold,
            next_delay_seconds: PollBackoffPolicy::default().max_delay_seconds,
            attention_until: None,
        }
    }
}

/// Compute the pacing. Hot while the last USER message is within the
/// attention lease. Cold otherwise — with the backoff ramp derived
/// STATELESSLY from the time elapsed since the last message of ANY role
/// (Codex review: the server must decide the cold delay too, not leave a
/// "MAY ramp up" ambiguity to clients; deriving the ramp position from
/// elapsed time gives every client the same deterministic answer without
/// per-session counters). Walk the cumulative schedule: right after
/// activity the delays are short, converging to the cap as silence lasts.
pub fn pacing_for(
    last_user_msg_at: Option<chrono::DateTime<chrono::Utc>>,
    last_any_msg_at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
    policy: &PollBackoffPolicy,
) -> PacingState {
    if let Some(t) = last_user_msg_at {
        let lease_end = t + chrono::Duration::seconds(policy.user_attention_lease_seconds as i64);
        if now < lease_end {
            return PacingState {
                regime: PacingRegime::Hot,
                next_delay_seconds: policy.hot_poll_seconds,
                attention_until: Some(lease_end.to_rfc3339()),
            };
        }
    }
    let next_delay_seconds = match last_any_msg_at {
        Some(t) => {
            let elapsed = (now - t).num_seconds().max(0) as u64;
            let mut cum: u64 = 0;
            let mut pick = policy.max_delay_seconds;
            for &step in &policy.poll_backoff_seconds {
                cum += step as u64;
                if elapsed < cum {
                    pick = step;
                    break;
                }
            }
            pick
        }
        None => policy.max_delay_seconds,
    };
    PacingState {
        regime: PacingRegime::Cold,
        next_delay_seconds,
        attention_until: None,
    }
}

/// Lean attachment descriptor surfaced to agents via `disc_get_message`. The
/// `disk_path` lets a file-tool-capable agent open the image directly.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessageAttachment {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub disk_path: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SummarizeRequest {
    /// 0-based start index (inclusive). `None` = start of transcript.
    #[serde(default)]
    pub from: Option<u32>,
    /// 0-based end index (exclusive). `None` = up to the latest message.
    #[serde(default)]
    pub to: Option<u32>,
    /// Force regeneration even if the cached summary covers the same
    /// range. Useful when the agent thinks the cached summary is stale.
    #[serde(default)]
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SummarizeResponse {
    pub summary: String,
    pub from_idx: u32,
    pub to_idx: u32,
    pub generated: bool,
    /// Tokens spent generating the summary. `0` when served from cache.
    pub tokens_used: u64,
}

/// `GET /api/discussions/{id}/meta`
pub async fn disc_meta(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<DiscussionMeta>> {
    let did = id.clone();
    let disc = match state.db.with_conn(move |conn| {
        crate::db::discussions::get_discussion(conn, &did)
    }).await {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    // Bump the per-disc tool counter (UI pill in ChatHeader). Best-effort —
    // a counter-write failure must not fail the introspection call.
    let did_bump = id.clone();
    let _ = state.db.with_conn(move |conn| {
        crate::db::discussions::bump_introspection_count(conn, &did_bump)
    }).await;

    let non_system_count = disc.messages.iter()
        .filter(|m| !matches!(m.role, MessageRole::System))
        .count() as u32;
    let last_summary = disc.summary_up_to_msg_idx.unwrap_or(0);
    let msgs_since_last_summary = non_system_count.saturating_sub(last_summary);

    let tokens_used_total: u64 = disc.messages.iter().map(|m| m.tokens_used).sum();

    // Same computation as the wait/join paths — pacing_for_disc reads the
    // RECEPTION-clock anchors (072), while `disc.messages[..].timestamp` is
    // the authored clock (Copilot round 3: the two answers must not diverge
    // on a federated message stamped in the past).
    let pacing = crate::api::disc_invite::pacing_for_disc(&state, &id).await;
    Json(ApiResponse::ok(DiscussionMeta {
        poll_policy: PollBackoffPolicy::default(),
        pacing: Some(pacing),
        id: disc.id,
        title: disc.title,
        agent: disc.agent,
        tier: disc.tier,
        message_count: non_system_count,
        tokens_used_total,
        summary_strategy: disc.summary_strategy,
        has_cached_summary: disc.summary_cache.is_some(),
        summary_up_to_msg_idx: disc.summary_up_to_msg_idx,
        msgs_since_last_summary,
        language: disc.language,
        project_id: disc.project_id,
    }))
}

/// `GET /api/discussions/{id}/message/{idx}`
///
/// Negative-index semantics: `idx == u32::MAX` (i.e. -1 in two's
/// complement) is treated as "last message". Anything past the end
/// returns 404. We avoid signed integer parsing in the path so the
/// router stays simple.
pub async fn disc_get_message(
    State(state): State<AppState>,
    Path((id, idx_param)): Path<(String, String)>,
) -> Json<ApiResponse<DiscussionMessageRead>> {
    let did = id.clone();
    let disc = match state.db.with_conn(move |conn| {
        crate::db::discussions::get_discussion(conn, &did)
    }).await {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let did_bump = id.clone();
    let _ = state.db.with_conn(move |conn| {
        crate::db::discussions::bump_introspection_count(conn, &did_bump)
    }).await;

    let total = disc.messages.len();
    if total == 0 {
        return Json(ApiResponse::err("Discussion has no messages"));
    }

    // Parse the idx parameter, accepting negative numbers as "from end".
    // `-1` = last, `-2` = second-to-last, etc.
    let resolved_idx: usize = match idx_param.parse::<i64>() {
        Ok(n) if n >= 0 => n as usize,
        Ok(n) => {
            // Negative: count from the end. `n == -1` → total - 1.
            let from_end = (-n) as usize;
            if from_end > total {
                return Json(ApiResponse::err(format!(
                    "Negative index {} out of range (total {})", n, total
                )));
            }
            total - from_end
        }
        Err(_) => return Json(ApiResponse::err("Invalid idx — must be an integer")),
    };

    if resolved_idx >= total {
        return Json(ApiResponse::err(format!(
            "Index {} out of range (total {})", resolved_idx, total
        )));
    }

    let msg = &disc.messages[resolved_idx];
    let msg_id = msg.id.clone();
    let attachments = state.db.with_conn(move |conn| {
        crate::db::discussions::list_context_files_for_message(conn, &msg_id).map_err(|e| anyhow::anyhow!(e))
    }).await.unwrap_or_default()
        .into_iter()
        .map(|f| MessageAttachment {
            id: f.id,
            filename: f.filename,
            mime_type: f.mime_type,
            disk_path: f.disk_path,
        })
        .collect();
    Json(ApiResponse::ok(DiscussionMessageRead {
        idx: resolved_idx as u32,
        id: msg.id.clone(),
        role: msg.role.clone(),
        content: msg.content.clone(),
        agent_type: msg.agent_type.clone(),
        timestamp: msg.timestamp,
        tokens_used: msg.tokens_used,
        attachments,
    }))
}

/// `POST /api/discussions/{id}/summarize`
///
/// Re-uses the cached summary when the requested range matches what's
/// already stored, otherwise falls back to the on-demand path that the
/// orchestration code already implements. For now we keep the
/// implementation simple — full-range only — and queue the ranged
/// cache (with `(from, to)` keying) for a follow-up. The response
/// always includes the actual range that was summarised so the agent
/// knows what it got.
pub async fn disc_summarize(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SummarizeRequest>,
) -> Json<ApiResponse<SummarizeResponse>> {
    let did = id.clone();
    let disc = match state.db.with_conn(move |conn| {
        crate::db::discussions::get_discussion(conn, &did)
    }).await {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let did_bump = id.clone();
    let _ = state.db.with_conn(move |conn| {
        crate::db::discussions::bump_introspection_count(conn, &did_bump)
    }).await;

    let total_non_system = disc.messages.iter()
        .filter(|m| !matches!(m.role, MessageRole::System))
        .count() as u32;
    let from_idx = req.from.unwrap_or(0).min(total_non_system);
    let to_idx = req.to.unwrap_or(total_non_system).min(total_non_system);
    if from_idx >= to_idx {
        return Json(ApiResponse::err(format!(
            "Invalid range: from {} >= to {}", from_idx, to_idx
        )));
    }

    // Cache lookups, in order of preference:
    //   1. Exact ranged-cache hit on `(from, to)` — free.
    //   2. Prefix hit on the legacy `summary_cache` (full-from-zero) —
    //      free, applies when `from == 0` and `to` matches
    //      `summary_up_to_msg_idx`. Kept for backward compatibility
    //      with disc rows generated before migration 048.
    //   3. Miss → run the inline summariser, cache the result.
    if !req.force_refresh {
        let did_for_lookup = id.clone();
        if let Ok(Some((cached, t))) = state.db.with_conn(move |conn| {
            crate::db::discussions::get_ranged_summary(conn, &did_for_lookup, from_idx, to_idx)
        }).await {
            return Json(ApiResponse::ok(SummarizeResponse {
                summary: cached,
                from_idx,
                to_idx,
                generated: false,
                tokens_used: t,
            }));
        }
        if from_idx == 0 && Some(to_idx) == disc.summary_up_to_msg_idx {
            if let Some(ref summary) = disc.summary_cache {
                return Json(ApiResponse::ok(SummarizeResponse {
                    summary: summary.clone(),
                    from_idx,
                    to_idx,
                    generated: false,
                    tokens_used: 0,
                }));
            }
        }
    }

    let tokens_config = state.config.read().await.tokens.clone();
    match crate::api::discussions::orchestration::generate_summary_on_demand(
        &state,
        &disc,
        from_idx,
        to_idx,
        &tokens_config,
    ).await {
        Ok((s, t, model_name)) => {
            // Persist to the ranged cache so the next call with the same
            // (from, to) is free. Use the discussion id from the path
            // param. Best-effort: a write error doesn't fail the response
            // — the agent already has the summary text in hand.
            let summary_clone = s.clone();
            let model_name_clone = model_name.clone();
            let did = id.clone();
            let _ = state.db.with_conn(move |conn| {
                crate::db::discussions::upsert_ranged_summary(
                    conn, &did, from_idx, to_idx,
                    &summary_clone, t, model_name_clone.as_deref(),
                )
            }).await;
            Json(ApiResponse::ok(SummarizeResponse {
                summary: s,
                from_idx,
                to_idx,
                generated: true,
                tokens_used: t,
            }))
        }
        Err(e) => Json(ApiResponse::err(format!("Summary generation failed: {}", e))),
    }
}

#[cfg(test)]
mod tests {
    // Behavioural tests live in tests/api_tests.rs (integration) — the
    // routes need a full AppState + DB. Here we only unit-test the
    // pure idx-resolution logic of `disc_get_message`.

    /// Mirror of the negative-index resolution from `disc_get_message`.
    /// Kept in sync by hand since the real handler is async + db-backed.
    fn resolve_idx(idx_str: &str, total: usize) -> Result<usize, String> {
        let n = idx_str.parse::<i64>().map_err(|_| "parse".to_string())?;
        if n >= 0 {
            let i = n as usize;
            if i >= total { return Err("out".into()); }
            Ok(i)
        } else {
            let from_end = (-n) as usize;
            if from_end > total { return Err("out".into()); }
            Ok(total - from_end)
        }
    }

    #[test]
    fn positive_idx_in_range() {
        assert_eq!(resolve_idx("0", 5), Ok(0));
        assert_eq!(resolve_idx("4", 5), Ok(4));
    }

    #[test]
    fn positive_idx_out_of_range_errors() {
        assert!(resolve_idx("5", 5).is_err());
        assert!(resolve_idx("100", 5).is_err());
    }

    #[test]
    fn negative_one_returns_last() {
        assert_eq!(resolve_idx("-1", 5), Ok(4));
    }

    #[test]
    fn negative_two_returns_penultimate() {
        assert_eq!(resolve_idx("-2", 5), Ok(3));
    }

    #[test]
    fn negative_at_total_returns_first() {
        // `-N` where N == total resolves to index 0 (the first message).
        assert_eq!(resolve_idx("-5", 5), Ok(0));
    }

    #[test]
    fn negative_past_start_errors() {
        // `-6` on a 5-message thread is out of range — no wraparound.
        assert!(resolve_idx("-6", 5).is_err());
    }

    #[test]
    fn invalid_string_errors() {
        assert!(resolve_idx("abc", 5).is_err());
        assert!(resolve_idx("", 5).is_err());
    }

    #[test]
    fn poll_backoff_policy_contract() {
        use super::PollBackoffPolicy;
        // stab-1 (Romu) — the pacing sequence is a PRODUCT contract: fine
        // early (30s), capped at 8 min, reset on peer message. Agents and
        // bridges rely on this exact shape.
        let p = PollBackoffPolicy::default();
        assert_eq!(p.poll_backoff_seconds.first(), Some(&30));
        assert_eq!(p.poll_backoff_seconds, vec![30, 30, 60, 60, 120, 120, 240, 240, 480]);
        assert_eq!(p.max_delay_seconds, 480, "cap = last step of the sequence");
        assert_eq!(*p.poll_backoff_seconds.last().unwrap(), p.max_delay_seconds);
        assert!(p.reset_on_peer_message);
        // stab-3 — hot regime: sub-minute while a human is around, 30 min lease.
        assert_eq!(p.hot_poll_seconds, 40);
        assert_eq!(p.user_attention_lease_seconds, 1800);
    }

    #[test]
    fn pacing_regime_follows_the_user_attention_lease() {
        use super::{pacing_for, PollBackoffPolicy};
        let p = PollBackoffPolicy::default();
        let now = chrono::Utc::now();
        let ago = |m: i64| Some(now - chrono::Duration::minutes(m));

        // User spoke 5 min ago → hot, 40s, lease end exposed.
        let hot = pacing_for(ago(5), ago(5), now, &p);
        assert_eq!(hot.regime, super::PacingRegime::Hot);
        assert_eq!(hot.next_delay_seconds, 40);
        assert!(hot.attention_until.is_some());

        // User spoke 31 min ago, nothing since → cold at the CAP
        // (elapsed 1860s exceeds the whole ramp schedule of 1380s).
        let cold = pacing_for(ago(31), ago(31), now, &p);
        assert_eq!(cold.regime, super::PacingRegime::Cold);
        assert_eq!(cold.next_delay_seconds, 480);
        assert!(cold.attention_until.is_none());

        // No message ever → cold, cap.
        assert_eq!(pacing_for(None, None, now, &p).regime, super::PacingRegime::Cold);
        assert_eq!(pacing_for(None, None, now, &p).next_delay_seconds, 480);
    }

    #[test]
    fn cold_ramp_is_derived_statelessly_from_elapsed_silence() {
        // Codex review (stab-3 blocker) — the server decides the COLD delay
        // too: ramp position = where the elapsed silence falls in the
        // cumulative schedule (30,60,120,180,300,420,660,900,1380). Every
        // client gets the same deterministic answer, no per-session state.
        use super::{pacing_for, PollBackoffPolicy};
        let p = PollBackoffPolicy::default();
        let now = chrono::Utc::now();
        let cold_after = |secs: i64| {
            // Agent peer spoke `secs` ago; the user has been gone for hours.
            pacing_for(
                Some(now - chrono::Duration::hours(3)),
                Some(now - chrono::Duration::seconds(secs)),
                now,
                &p,
            )
            .next_delay_seconds
        };
        assert_eq!(cold_after(0), 30, "right after activity: shortest step");
        assert_eq!(cold_after(45), 30, "within the second 30s step");
        assert_eq!(cold_after(90), 60, "then 1m");
        assert_eq!(cold_after(250), 120, "then 2m");
        assert_eq!(cold_after(700), 240, "then 4m");
        assert_eq!(cold_after(1000), 480, "tail of the schedule");
        assert_eq!(cold_after(5000), 480, "long silence: the cap");
    }
}
