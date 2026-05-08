//! Slash-marker fallback for agents that don't speak MCP.
//!
//! Vibe (`vibe-runner.py`) and Ollama (HTTP streaming path) don't read
//! `.mcp.json` and have no way to invoke `kronn-internal` tools at
//! runtime. To still let those agents introspect the discussion they're
//! running in, Kronn:
//!
//!   1. Adds a slash-marker section to their system prompt telling them
//!      to emit `KRONN:DISC_*` lines when they need history (cf.
//!      `disc_prompts.rs` — the section is gated on `agent_speaks_mcp`).
//!   2. After the agent reply lands, post-processes the reply to find
//!      such markers (this module) and resolves each into a System
//!      message inserted into the disc, so the agent sees the result on
//!      its next turn.
//!
//! # Why not synchronous re-prompt?
//!
//! Detect-mid-stream → cancel → call → re-prompt would be
//! single-turn but doubles the spend on each introspection (two agent
//! runs) and is brittle (false-positive markers in the middle of a
//! sentence). Multi-turn round-trip is cheaper and more robust at the
//! cost of one extra turn per introspection. Trade documented in
//! TD-20260510-introspection-vibe.

use regex_lite::Regex;

use crate::AppState;

/// Marker types the parser recognises. The frontend pill counter (the
/// `🔧 N` badge in `ChatHeader.tsx`) increments once per resolved
/// marker, mirroring the MCP-driven counter on agents that speak it
/// natively. A user can therefore read "the agent looked at the
/// history N times this disc" regardless of which path was used.
#[derive(Debug, Clone, PartialEq)]
pub enum KronnMarker {
    /// `KRONN:DISC_META` — return high-level disc metadata.
    Meta,
    /// `KRONN:DISC_GET_MESSAGE <idx>` — fetch one message. Negative
    /// idx counts from the end (`-1` = last) — same convention as
    /// `disc_introspection::disc_get_message`.
    GetMessage { idx: i64 },
    /// `KRONN:DISC_SUMMARIZE <from> <to>` — generate or replay a
    /// cached summary for the half-open range `[from, to)`. The
    /// optional `refresh` keyword forces regeneration even when a
    /// cached summary covers the same range.
    Summarize { from: u32, to: u32, force_refresh: bool },
}

/// Parse `KRONN:DISC_*` markers from an agent reply. We require the
/// marker to start at column 0 of its own line — a permissive scan
/// would false-positive on prose like "the agent said KRONN:DISC_META
/// in the previous turn", which is a real failure mode on long Vibe
/// replies. Only markers on their own line count.
pub fn parse_markers(reply: &str) -> Vec<KronnMarker> {
    // Compile once. The unwrap is safe — pattern is a const string;
    // a panic here would mean a refactor introduced a typo and we
    // want to know in tests, not silently skip parsing in prod.
    let meta_re = Regex::new(r"(?m)^KRONN:DISC_META\b").unwrap();
    let get_re = Regex::new(r"(?m)^KRONN:DISC_GET_MESSAGE\s+(-?\d+)\b").unwrap();
    let sum_re = Regex::new(r"(?m)^KRONN:DISC_SUMMARIZE\s+(\d+)\s+(\d+)(?:\s+(refresh))?\b").unwrap();

    let mut out = Vec::new();
    for _ in meta_re.find_iter(reply) {
        out.push(KronnMarker::Meta);
    }
    for cap in get_re.captures_iter(reply) {
        if let Some(m) = cap.get(1) {
            if let Ok(idx) = m.as_str().parse::<i64>() {
                out.push(KronnMarker::GetMessage { idx });
            }
        }
    }
    for cap in sum_re.captures_iter(reply) {
        let from = cap.get(1).and_then(|m| m.as_str().parse::<u32>().ok());
        let to = cap.get(2).and_then(|m| m.as_str().parse::<u32>().ok());
        let refresh = cap.get(3).is_some();
        if let (Some(f), Some(t)) = (from, to) {
            if f < t {
                out.push(KronnMarker::Summarize { from: f, to: t, force_refresh: refresh });
            }
        }
    }
    // Cap the number of markers a single reply can fire to avoid a
    // runaway agent that emits 100× the marker. 5 is generous —
    // realistically an agent needs 1-2 introspection calls per turn.
    out.truncate(5);
    out
}

/// Resolve markers against the live discussion and return one
/// formatted System-message body per marker. Best-effort: a single
/// marker that errors out (out-of-range idx, DB hiccup, summary gen
/// failure) yields a System message describing the error rather than
/// dropping the whole batch. Each resolved marker bumps the disc's
/// `introspection_call_count` so the UI pill stays in sync.
pub async fn resolve_markers(
    state: &AppState,
    disc_id: &str,
    markers: &[KronnMarker],
) -> Vec<String> {
    use crate::models::MessageRole;

    let mut out = Vec::with_capacity(markers.len());
    for m in markers {
        // Bump the per-disc counter for every marker we attempt — the
        // count tracks "the agent looked at the history N times",
        // including failures (a failed look-up is information for
        // the user too). Best-effort.
        let did_bump = disc_id.to_string();
        let _ = state.db.with_conn(move |conn| {
            crate::db::discussions::bump_introspection_count(conn, &did_bump)
        }).await;

        match m {
            KronnMarker::Meta => {
                let did = disc_id.to_string();
                let disc = state.db.with_conn(move |conn| {
                    crate::db::discussions::get_discussion(conn, &did)
                }).await.ok().flatten();
                let body = match disc {
                    Some(d) => {
                        let n = d.messages.iter()
                            .filter(|m| !matches!(m.role, MessageRole::System))
                            .count();
                        format!(
                            "[kronn-internal: disc_meta → {{message_count: {}, agent: {:?}, summary_strategy: {:?}, msgs_since_summary: {}}}]",
                            n, d.agent, d.summary_strategy,
                            (n as u32).saturating_sub(d.summary_up_to_msg_idx.unwrap_or(0))
                        )
                    }
                    None => "[kronn-internal: disc_meta → ERROR: discussion not found]".into(),
                };
                out.push(body);
            }
            KronnMarker::GetMessage { idx } => {
                let did = disc_id.to_string();
                let disc = state.db.with_conn(move |conn| {
                    crate::db::discussions::get_discussion(conn, &did)
                }).await.ok().flatten();
                let body = match disc {
                    Some(d) => {
                        let total = d.messages.len() as i64;
                        let resolved = if *idx >= 0 { *idx } else { total + *idx };
                        if resolved < 0 || resolved >= total {
                            format!("[kronn-internal: disc_get_message({}) → ERROR: out of range (total {})]",
                                idx, total)
                        } else {
                            let msg = &d.messages[resolved as usize];
                            // Trim long messages — the agent's next turn already
                            // has the System message in its context window, no
                            // point spending tokens on a 5000-char dump.
                            let mut content = msg.content.clone();
                            if content.len() > 800 {
                                let mut truncate_at = 800;
                                while truncate_at > 0 && !content.is_char_boundary(truncate_at) {
                                    truncate_at -= 1;
                                }
                                content.truncate(truncate_at);
                                content.push_str("…[truncated]");
                            }
                            format!("[kronn-internal: disc_get_message({}) → {:?} message: {:?}]",
                                idx, msg.role, content)
                        }
                    }
                    None => format!("[kronn-internal: disc_get_message({}) → ERROR: discussion not found]", idx),
                };
                out.push(body);
            }
            KronnMarker::Summarize { from, to, force_refresh } => {
                // Reuse the same path the HTTP endpoint takes so cache
                // semantics and token tracking are identical.
                let req = crate::api::disc_introspection::SummarizeRequest {
                    from: Some(*from),
                    to: Some(*to),
                    force_refresh: *force_refresh,
                };
                // Inline the body of the handler — we have an AppState,
                // so we don't need to dispatch through the HTTP router.
                let body = resolve_summarize_inline(state, disc_id, req).await;
                out.push(body);
            }
        }
    }
    out
}

/// Mirror of `disc_introspection::disc_summarize`'s body without the
/// axum extraction. Returning a formatted System-message body so the
/// caller can drop it into the disc directly.
async fn resolve_summarize_inline(
    state: &AppState,
    disc_id: &str,
    req: crate::api::disc_introspection::SummarizeRequest,
) -> String {
    use crate::models::MessageRole;
    let did = disc_id.to_string();
    let disc = match state.db.with_conn(move |conn| {
        crate::db::discussions::get_discussion(conn, &did)
    }).await {
        Ok(Some(d)) => d,
        Ok(None) => return "[kronn-internal: disc_summarize → ERROR: discussion not found]".into(),
        Err(e) => return format!("[kronn-internal: disc_summarize → ERROR: db: {}]", e),
    };
    let total = disc.messages.iter()
        .filter(|m| !matches!(m.role, MessageRole::System))
        .count() as u32;
    let from_idx = req.from.unwrap_or(0).min(total);
    let to_idx = req.to.unwrap_or(total).min(total);
    if from_idx >= to_idx {
        return format!("[kronn-internal: disc_summarize({},{}) → ERROR: invalid range]", from_idx, to_idx);
    }
    if !req.force_refresh {
        let did_lookup = disc_id.to_string();
        if let Ok(Some((cached, _))) = state.db.with_conn(move |conn| {
            crate::db::discussions::get_ranged_summary(conn, &did_lookup, from_idx, to_idx)
        }).await {
            return format!("[kronn-internal: disc_summarize({},{}) cached → {}]", from_idx, to_idx, cached);
        }
    }
    let tokens_config = state.config.read().await.tokens.clone();
    match crate::api::discussions::orchestration::generate_summary_on_demand(
        state, &disc, from_idx, to_idx, &tokens_config,
    ).await {
        Ok((s, t, model_name)) => {
            let summary_clone = s.clone();
            let model_name_clone = model_name.clone();
            let did_save = disc_id.to_string();
            let _ = state.db.with_conn(move |conn| {
                crate::db::discussions::upsert_ranged_summary(
                    conn, &did_save, from_idx, to_idx, &summary_clone, t, model_name_clone.as_deref(),
                )
            }).await;
            format!("[kronn-internal: disc_summarize({},{}) → {}]", from_idx, to_idx, s)
        }
        Err(e) => format!("[kronn-internal: disc_summarize({},{}) → ERROR: {}]", from_idx, to_idx, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meta_marker_alone_on_line() {
        assert_eq!(parse_markers("KRONN:DISC_META"), vec![KronnMarker::Meta]);
    }

    #[test]
    fn parses_meta_marker_in_multi_line_reply() {
        let reply = "Sure, let me check.\nKRONN:DISC_META\nWaiting for the result.";
        assert_eq!(parse_markers(reply), vec![KronnMarker::Meta]);
    }

    #[test]
    fn ignores_marker_in_prose_mid_line() {
        // Prose mention should NOT trigger — only column-0 markers count.
        // A naive scan would false-positive here; this test pins that.
        let reply = "I previously said KRONN:DISC_META, but didn't actually call it.";
        assert!(parse_markers(reply).is_empty());
    }

    #[test]
    fn parses_get_message_with_positive_idx() {
        assert_eq!(
            parse_markers("KRONN:DISC_GET_MESSAGE 4"),
            vec![KronnMarker::GetMessage { idx: 4 }],
        );
    }

    #[test]
    fn parses_get_message_with_negative_idx() {
        assert_eq!(
            parse_markers("KRONN:DISC_GET_MESSAGE -1"),
            vec![KronnMarker::GetMessage { idx: -1 }],
        );
    }

    #[test]
    fn parses_summarize_with_range() {
        assert_eq!(
            parse_markers("KRONN:DISC_SUMMARIZE 0 10"),
            vec![KronnMarker::Summarize { from: 0, to: 10, force_refresh: false }],
        );
    }

    #[test]
    fn parses_summarize_with_refresh_flag() {
        assert_eq!(
            parse_markers("KRONN:DISC_SUMMARIZE 0 10 refresh"),
            vec![KronnMarker::Summarize { from: 0, to: 10, force_refresh: true }],
        );
    }

    #[test]
    fn rejects_summarize_with_invalid_range() {
        // from >= to: nothing to summarise; shouldn't produce a marker
        // (so the agent sees nothing rather than getting back an error
        // — the system prompt told it `from < to`).
        assert!(parse_markers("KRONN:DISC_SUMMARIZE 10 5").is_empty());
        assert!(parse_markers("KRONN:DISC_SUMMARIZE 5 5").is_empty());
    }

    #[test]
    fn handles_multiple_markers_in_one_reply() {
        let reply = "KRONN:DISC_META\nSome text in between.\nKRONN:DISC_GET_MESSAGE 0";
        let markers = parse_markers(reply);
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0], KronnMarker::Meta);
        assert_eq!(markers[1], KronnMarker::GetMessage { idx: 0 });
    }

    #[test]
    fn caps_marker_count_at_5() {
        // A runaway Vibe could emit 100× the same marker per reply.
        // We cap at 5 to bound work + tokens spent on the System reply.
        let reply = (0..20).map(|_| "KRONN:DISC_META").collect::<Vec<_>>().join("\n");
        assert_eq!(parse_markers(&reply).len(), 5);
    }

    #[test]
    fn empty_reply_yields_no_markers() {
        assert!(parse_markers("").is_empty());
        assert!(parse_markers("Some normal agent reply.").is_empty());
    }
}
