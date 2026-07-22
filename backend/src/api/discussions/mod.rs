// Discussions API split by concern (TD-20260328-discussions-backend).
// `mod.rs` keeps only the cross-cutting bits — limit constants, the
// terminal-signal detector, the silent-crash matcher, the
// `AgentStreamEvent` enum that streaming + orchestration both yield,
// the `SseStream` typedef, and the unit tests for the pure helpers.
// Every handler/spawner lives in a sibling sub-module re-exported
// at the audit:: level via `pub use sub::*` so existing call sites
// (lib.rs routes, workflows::batch_step, src/api_tests.rs) keep
// resolving without edits.

use std::convert::Infallible;
use std::pin::Pin;
use std::time::Duration;

use axum::response::sse::Event;
use futures::stream::Stream;

pub mod context;
pub mod crud;
pub mod messaging;
pub mod orchestration;
pub mod runtime;
pub mod slash_markers;
pub mod streaming;

pub use context::*;
pub use crud::*;
pub use messaging::*;
pub use orchestration::*;
pub use runtime::*;
// `streaming` exposes only internal helpers (`make_agent_stream`,
// `run_agent_streaming`, `run_agent_collect`) — no public route
// handlers, so no `pub use` needed.

/// Maximum title length for discussions (characters).
pub(super) const MAX_TITLE_LEN: usize = 500;
/// Maximum content/prompt length (bytes, ~100 KB).
pub(super) const MAX_CONTENT_LEN: usize = 100_000;
/// Global timeout for a single agent stream (30 minutes).
pub(super) const AGENT_GLOBAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Default stall timeout (5 minutes) — overridden by config.server.agent_stall_timeout_min
pub(super) const DEFAULT_STALL_TIMEOUT_MIN: u32 = 5;
/// Stall ceiling for NON-streaming agents (Codex `exec` etc.): they're silent
/// on stdout until the very end, so the short streaming stall would kill a
/// slow-but-healthy run (the 2026-06-23 fix). But the global deadline (30 min)
/// is too long to hold a scarce concurrency slot for a genuinely-hung run —
/// 5 such runs squat the whole `agent_semaphore` and everything else queues
/// ("planted", 2026-06-24). 15 min is the middle ground: comfortably above a
/// real triage (the working ones run 1-3 min) while freeing the slot in half
/// the global window when an agent truly hangs.
pub(super) const NON_STREAMING_STALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Hard cap on a single agent reply (~2 MB). Beyond this we kill the agent
/// and append a partial-response footer. The bound is intentionally
/// generous — a normal Claude Code reply is ~50 KB even with tool calls,
/// long workflow runs are ~500 KB. Anything larger is almost always a
/// runaway loop (the "90 issues from a 46-issue plan" case) and the cost
/// of letting it continue dwarfs the cost of cutting it off.
pub(super) const MAX_AGENT_RESPONSE_BYTES: usize = 2_000_000;

/// Gated KRONN signals — when an agent emits any of these, it MUST stop.
///
/// Each signal marks a deliberate handoff back to the user (validate the
/// architecture, validate the plan, view the project board, etc.). Without
/// hard enforcement here, an agent that ignores the skill's "STOP HERE"
/// instruction can keep streaming indefinitely — for example creating
/// duplicate GitHub issues after KRONN:ISSUES_CREATED, which is exactly the
/// bug that produced 90 issues from a 46-issue plan.
///
/// Detection happens in the streaming loop: as soon as `full_response`
/// (uppercased suffix) contains one of these substrings, the loop breaks
/// and the agent subprocess is killed. The user picks up via the CTA
/// banners in DiscussionsPage.tsx and triggers the next stage with a fresh
/// message.
const TERMINAL_SIGNALS: &[&str] = &[
    "KRONN:REPO_READY",
    "KRONN:ARCHITECTURE_READY",
    "KRONN:PLAN_READY",
    "KRONN:STRUCTURE_READY", // alias for PLAN_READY — LLM hallucinates this when
    // Stage 2 produces a structural breakdown (modules,
    // chantiers) rather than an explicit "plan" header
    "KRONN:ISSUES_READY",   // canonical (consistent with the *_READY family)
    "KRONN:ISSUES_CREATED", // legacy alias — LLMs sometimes invent one or the other
    "KRONN:VALIDATION_COMPLETE",
    "KRONN:WORKFLOW_READY",
    "KRONN:BOOTSTRAP_COMPLETE",
    "KRONN:BRIEFING_COMPLETE",
];

/// 0.8.4 (#329 / F9) — true when the terminal signal means the disc
/// has fulfilled its purpose and should be archived from the active
/// sidebar. Validation / briefing / bootstrap discs all qualify:
/// they're single-shot lifecycle conversations. *_READY family
/// signals (REPO_READY, PLAN_READY, ISSUES_READY, etc.) do NOT —
/// those are in-disc handoffs between stages and the user still
/// needs the conversation visible to drive the next step.
///
/// Used by `streaming.rs` after persisting the agent's final reply
/// to decide whether to flip `archived = true`.
pub(crate) fn signal_should_auto_archive(signal: &str) -> bool {
    matches!(
        signal,
        "KRONN:VALIDATION_COMPLETE" | "KRONN:BRIEFING_COMPLETE" | "KRONN:BOOTSTRAP_COMPLETE"
    )
}

/// Returns the first terminal signal found in the *tail* of `text`, or None.
///
/// We only inspect the last ~256 bytes because terminal signals always sit on
/// the final line of the agent's reply. Scanning the entire `full_response`
/// every chunk would be O(n²) on long runs (100k+ chars) and is unnecessary —
/// the signal is on the very last line by skill convention.
///
/// CRITICAL: `text.len()` is a byte count, not a char count. If we slice at a
/// byte index that falls in the middle of a multibyte UTF-8 codepoint
/// (e.g. an accented French char like `é` = 2 bytes, an emoji = 4 bytes),
/// `&text[tail_start..]` panics with "byte index N is not a char boundary".
/// We back off the index until it lands on a valid char boundary — at most
/// 3 bytes since UTF-8 codepoints are 1–4 bytes.
pub(crate) fn detect_terminal_signal(text: &str) -> Option<&'static str> {
    let mut tail_start = text.len().saturating_sub(256);
    while tail_start > 0 && !text.is_char_boundary(tail_start) {
        tail_start -= 1;
    }
    let tail = &text[tail_start..];
    let tail_upper = tail.to_uppercase();
    TERMINAL_SIGNALS
        .iter()
        .copied()
        .find(|sig| tail_upper.contains(sig))
}

/// STRICT terminal-position check (Codex A5 v3): does `text`, once
/// right-trimmed, END on a line that is exactly `signal` (ASCII case
/// tolerated)? Unlike [`detect_terminal_signal`] — a lenient tail-window
/// `contains` meant for live stream truncation — this cannot be satisfied
/// by a quotation or an instruction that merely mentions the marker
/// mid-sentence. Gate-grade: used by validate-audit.
pub(crate) fn ends_with_terminal_signal(text: &str, signal: &str) -> bool {
    text.trim_end()
        .lines()
        .last()
        .map(|line| line.trim().eq_ignore_ascii_case(signal))
        .unwrap_or(false)
}

/// Truncate `text` so it ends right after the first occurrence of `signal`.
///
/// Used after a terminal signal is detected: the LLM may have started writing
/// a follow-up sentence in the same chunk before our break landed (the
/// "STOP immediately" rule isn't always obeyed). Cutting after the signal
/// keeps the saved message clean — no orphan letter / half-sentence trailing
/// the marker.
///
/// Case-insensitive ASCII match. Safe with multibyte UTF-8 in `text`: we
/// search at the byte level using `eq_ignore_ascii_case` so we never need
/// to call `to_uppercase()` (which can shift byte positions on non-ASCII
/// chars and break our slice).
///
/// Returns the original text untouched if the signal is not found.
pub(crate) fn truncate_after_signal(text: &str, signal: &str) -> String {
    let needle = signal.as_bytes();
    let haystack = text.as_bytes();
    if needle.is_empty() || needle.len() > haystack.len() {
        return text.to_string();
    }
    let pos = haystack
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle));
    let Some(pos) = pos else {
        return text.to_string();
    };
    let end = pos + needle.len();
    // Defensive: end must land on a char boundary. Since the signal is pure
    // ASCII (KRONN:* / underscores / digits), if `pos` is on a char boundary
    // then so is `end` — but check anyway in case of pathological input.
    if text.is_char_boundary(end) {
        text[..end].to_string()
    } else {
        text.to_string()
    }
}

/// Pure, sync detector for the silent-crash footer pattern in a single
/// agent message. Extracted for testability — the async DB-touching wrapper
/// `last_message_is_silent_crash` calls this.
pub(crate) fn message_matches_silent_crash(content: &str) -> bool {
    content.contains("[Agent exited with error]") && content.contains("No output captured")
}

/// Multiplexed event yielded by the agent runner pipeline. `streaming`
/// emits these from the spawned subprocess parser; `orchestration`
/// emits the same shape (with extra `Round`/`AgentStart`/`AgentDone`
/// variants) so both can reuse the same SSE serialization.
#[derive(Clone, Debug)]
pub(super) enum AgentStreamEvent {
    Start,
    Meta { auth_mode: String },
    Chunk { data: serde_json::Value },
    Log { text: String },
    Done { data: serde_json::Value },
    Error { data: serde_json::Value },
    // Orchestration-specific:
    System { data: serde_json::Value },
    Round { data: serde_json::Value },
    AgentStart { data: serde_json::Value },
    AgentDone { data: serde_json::Value },
}

pub(super) type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

// Tests for `detect_terminal_signal` / `truncate_after_signal` /
// `message_matches_silent_crash` — appended below.
#[cfg(test)]
mod terminal_signal_tests {
    #[test]
    fn ends_with_terminal_signal_is_position_strict() {
        use super::ends_with_terminal_signal;
        const SIG: &str = "KRONN:VALIDATION_COMPLETE";
        // Positive: signal as the final line, trailing whitespace tolerated.
        assert!(ends_with_terminal_signal(
            "all done.\nKRONN:VALIDATION_COMPLETE",
            SIG
        ));
        assert!(ends_with_terminal_signal(
            "done\n  kronn:validation_complete  \n\n",
            SIG
        ));
        // Negatives (Codex A5 v3): a quotation or instruction MENTIONING
        // the marker mid-text must never satisfy the gate.
        assert!(!ends_with_terminal_signal(
            "quote KRONN:VALIDATION_COMPLETE then continue",
            SIG
        ));
        assert!(!ends_with_terminal_signal(
            "when finished, emit KRONN:VALIDATION_COMPLETE on its own line.\nStill working…",
            SIG
        ));
        assert!(!ends_with_terminal_signal("", SIG));
    }

    use super::detect_terminal_signal;

    #[test]
    fn detects_repo_ready_at_end() {
        let s = "All done.\nRepo created.\nKRONN:REPO_READY";
        assert_eq!(detect_terminal_signal(s), Some("KRONN:REPO_READY"));
    }

    #[test]
    fn detects_architecture_ready_lowercase() {
        let s = "Architecture summary.\nkronn:architecture_ready";
        assert_eq!(detect_terminal_signal(s), Some("KRONN:ARCHITECTURE_READY"));
    }

    #[test]
    fn detects_plan_ready() {
        let s = "Plan ready.\nKRONN:PLAN_READY";
        assert_eq!(detect_terminal_signal(s), Some("KRONN:PLAN_READY"));
    }

    #[test]
    fn detects_issues_created() {
        let s = "Created 12 issues.\nKRONN:ISSUES_CREATED";
        assert_eq!(detect_terminal_signal(s), Some("KRONN:ISSUES_CREATED"));
    }

    #[test]
    fn detects_issues_ready_canonical_variant() {
        // Real-world bug: Claude hallucinated KRONN:ISSUES_READY because the
        // *_READY family (REPO_READY, ARCHITECTURE_READY, PLAN_READY) makes
        // the LLM "harmonize" the last signal name. v3 of the skill uses
        // ISSUES_READY as canonical; both must be detected so old skills /
        // mid-conversation drift don't fall through the cracks.
        let s = "Created 13 epics.\nKRONN:ISSUES_READY";
        assert_eq!(detect_terminal_signal(s), Some("KRONN:ISSUES_READY"));
    }

    #[test]
    fn detects_structure_ready_alias_for_plan_ready() {
        // Real-world bug: when Stage 2 produces a "structure modulaire /
        // 15 chantiers" breakdown rather than an explicit "plan" header,
        // Claude emits KRONN:STRUCTURE_READY instead of KRONN:PLAN_READY.
        // We accept it as an alias so the agent stops cleanly and the
        // frontend CTA still fires.
        let s = "Structure Core/Dilem/Shared, 15 chantiers.\nKRONN:STRUCTURE_READY";
        assert_eq!(detect_terminal_signal(s), Some("KRONN:STRUCTURE_READY"));
    }

    #[test]
    fn ignores_text_without_signal() {
        let s = "Just a long agent reply with no terminal marker.";
        assert_eq!(detect_terminal_signal(s), None);
    }

    #[test]
    fn ignores_signals_buried_more_than_256_chars_from_end() {
        // The signal is at the START of a long reply — we only inspect the
        // tail. This is fine because real agents emit the signal as the
        // very last thing they print; tail-only inspection is the perf
        // win that lets us check on every chunk in O(1).
        let mut s = String::from("KRONN:PLAN_READY");
        s.push_str(&"a".repeat(300));
        assert_eq!(detect_terminal_signal(&s), None);
    }

    #[test]
    fn handles_empty_string() {
        assert_eq!(detect_terminal_signal(""), None);
    }

    #[test]
    fn does_not_match_unknown_signal() {
        let s = "End.\nKRONN:NOT_A_REAL_SIGNAL";
        assert_eq!(detect_terminal_signal(s), None);
    }

    // ─── 0.8.4 (#329 / F9) auto-archive predicate ───────────────────

    #[test]
    fn auto_archive_fires_for_lifecycle_completions() {
        // The three single-shot lifecycle signals must auto-archive
        // their disc — the user has no reason to keep the conversation
        // active after a successful sign-off.
        use super::signal_should_auto_archive;
        assert!(signal_should_auto_archive("KRONN:VALIDATION_COMPLETE"));
        assert!(signal_should_auto_archive("KRONN:BRIEFING_COMPLETE"));
        assert!(signal_should_auto_archive("KRONN:BOOTSTRAP_COMPLETE"));
    }

    #[test]
    fn auto_archive_does_not_fire_for_intermediate_signals() {
        // The *_READY family is an in-disc handoff between stages
        // (cadrage → architecture → plan → issues). Archiving here
        // would hide the conversation right when the user needs it
        // to drive the next stage — opposite of the desired UX.
        use super::signal_should_auto_archive;
        assert!(!signal_should_auto_archive("KRONN:REPO_READY"));
        assert!(!signal_should_auto_archive("KRONN:ARCHITECTURE_READY"));
        assert!(!signal_should_auto_archive("KRONN:PLAN_READY"));
        assert!(!signal_should_auto_archive("KRONN:STRUCTURE_READY"));
        assert!(!signal_should_auto_archive("KRONN:ISSUES_READY"));
        assert!(!signal_should_auto_archive("KRONN:ISSUES_CREATED"));
        assert!(!signal_should_auto_archive("KRONN:WORKFLOW_READY"));
    }

    #[test]
    fn auto_archive_predicate_covers_every_terminal_signal() {
        // Forward-compat: every signal in TERMINAL_SIGNALS must be a
        // known case (yes or no). If a new signal is added without an
        // explicit decision, this test fires so the maintainer thinks
        // about whether the new flow is single-shot or multi-stage.
        use super::{signal_should_auto_archive, TERMINAL_SIGNALS};
        for sig in TERMINAL_SIGNALS {
            // True or false, but never a panic / unhandled case.
            let _: bool = signal_should_auto_archive(sig);
        }
    }

    #[test]
    fn detects_signal_with_trailing_newline() {
        let s = "Done.\nKRONN:BOOTSTRAP_COMPLETE\n";
        assert_eq!(detect_terminal_signal(s), Some("KRONN:BOOTSTRAP_COMPLETE"));
    }

    #[test]
    fn handles_multibyte_utf8_at_byte_boundary() {
        // Regression: a previous version of detect_terminal_signal sliced at
        // text.len() - 256 bytes without checking char boundaries, which
        // panics if a multibyte UTF-8 codepoint spans the cut. Real bug:
        // a French agent reply in markdown was full of accented chars (é/è/à)
        // and one fell exactly on the 256-byte boundary → panic, agent task
        // killed silently, user saw nothing. Build a string that GUARANTEES
        // a multibyte char straddles the cut, then make sure we don't panic.
        //
        // 'é' is 2 bytes in UTF-8. 257 'é' chars = 514 bytes total. The cut
        // at 514 - 256 = 258 lands on the second byte of the 130th é.
        let s = "é".repeat(257);
        // Must not panic.
        let result = detect_terminal_signal(&s);
        assert_eq!(result, None);
    }

    #[test]
    fn handles_4byte_emoji_at_boundary() {
        // 4-byte UTF-8 (emoji 🚀 = 4 bytes). Stress the back-off logic with
        // a wider codepoint.
        let s = "🚀".repeat(80); // 320 bytes total, cut at 64
        let result = detect_terminal_signal(&s);
        assert_eq!(result, None);
    }

    #[test]
    fn detects_signal_after_french_text() {
        // Realistic case: a long French markdown reply ending with the signal.
        let s = format!(
            "{}\nÉtape terminée — synthèse des trois profils ci-dessus.\nKRONN:ARCHITECTURE_READY",
            "Voici l'analyse détaillée de l'architecture proposée. ".repeat(20)
        );
        assert_eq!(detect_terminal_signal(&s), Some("KRONN:ARCHITECTURE_READY"));
    }

    #[test]
    fn truncate_strips_orphan_letter_after_signal() {
        // Real bug from the first successful Bootstrap++ run: Claude wrote
        // "...analysis.\nKRONN:ARCHITECTURE_READY\n\nJ" — the LLM started its
        // next sentence ("J'attends ta validation...") in the same chunk
        // before our break landed. We should cut after the signal so the
        // saved DB content has no orphan letter.
        let s = "Section 10 done.\n\n---\n\nKRONN:ARCHITECTURE_READY\n\nJ";
        let result = super::truncate_after_signal(s, "KRONN:ARCHITECTURE_READY");
        assert_eq!(
            result,
            "Section 10 done.\n\n---\n\nKRONN:ARCHITECTURE_READY"
        );
    }

    #[test]
    fn truncate_strips_full_followup_sentence() {
        let s = "Done.\nKRONN:PLAN_READY\n\nJ'attends ta validation pour passer aux issues.";
        let result = super::truncate_after_signal(s, "KRONN:PLAN_READY");
        assert_eq!(result, "Done.\nKRONN:PLAN_READY");
    }

    #[test]
    fn truncate_case_insensitive_match() {
        // The LLM may emit the signal in lowercase (rare but legal per skill).
        let s = "Done.\nkronn:architecture_ready\n\nMore text.";
        let result = super::truncate_after_signal(s, "KRONN:ARCHITECTURE_READY");
        assert_eq!(result, "Done.\nkronn:architecture_ready");
    }

    #[test]
    fn truncate_safe_with_french_accents_before_signal() {
        // Multibyte UTF-8 chars before the signal must not throw off the
        // byte-level slicing. Bytes for "Étape" = 6, "à" = 2, etc.
        let s = "Étape 1 — Analyse complète. Voilà.\nKRONN:ARCHITECTURE_READY\n\nfollow-up";
        let result = super::truncate_after_signal(s, "KRONN:ARCHITECTURE_READY");
        assert_eq!(
            result,
            "Étape 1 — Analyse complète. Voilà.\nKRONN:ARCHITECTURE_READY"
        );
    }

    #[test]
    fn truncate_no_change_when_signal_absent() {
        let s = "Just text without any signal.";
        let result = super::truncate_after_signal(s, "KRONN:ARCHITECTURE_READY");
        assert_eq!(result, "Just text without any signal.");
    }

    #[test]
    fn truncate_no_change_when_signal_at_very_end() {
        let s = "Done.\nKRONN:ISSUES_CREATED";
        let result = super::truncate_after_signal(s, "KRONN:ISSUES_CREATED");
        assert_eq!(result, "Done.\nKRONN:ISSUES_CREATED");
    }

    #[test]
    fn max_response_bytes_constant_is_sane() {
        // Compile-time bounds check via const assertions — these become
        // build errors if someone bumps MAX_AGENT_RESPONSE_BYTES outside the
        // safe range. A normal Claude reply is ~50 KB, a 100-issue workflow
        // is ~500 KB. 2 MB catches anything 4× larger as a likely runaway.
        const _BOUND_LO: () = assert!(
            super::MAX_AGENT_RESPONSE_BYTES >= 1_000_000,
            "size cap must allow at least 1 MB so legitimate large runs aren't cut off"
        );
        const _BOUND_HI: () = assert!(
            super::MAX_AGENT_RESPONSE_BYTES <= 5_000_000,
            "size cap must stay under 5 MB so a runaway agent can't burn $$$"
        );
    }
}

#[cfg(test)]
mod silent_crash_detector_tests {
    use super::message_matches_silent_crash;

    #[test]
    fn matches_canonical_silent_crash_footer() {
        let msg = "[Agent exited with error] (exit code: Some(1))\n\n\
                   ⚠️ **No output captured.** Possible causes:\n\
                   - Expired session → run `/login` in the terminal";
        assert!(message_matches_silent_crash(msg));
    }

    #[test]
    fn rejects_normal_agent_response() {
        // A successful agent response shouldn't trigger the retry path.
        let msg = "Implemented the BrandContext service. All 44 tests pass.";
        assert!(!message_matches_silent_crash(msg));
    }

    #[test]
    fn rejects_partial_match_either_marker_alone() {
        // Both substrings must be present — neither one alone is enough,
        // because legitimate diagnostics might mention "No output" without
        // the full silent-crash pattern.
        assert!(!message_matches_silent_crash(
            "[Agent exited with error] (exit code: Some(2))"
        ));
        assert!(!message_matches_silent_crash(
            "⚠️ No output captured from the test runner."
        ));
    }

    #[test]
    fn rejects_other_agent_failure_modes() {
        // Stall timeout has its own message — distinct from silent crash.
        // Retrying it would be wrong (the stall is real, not transient).
        let stall = "⚠️ Partial response — the agent was interrupted after 40 min without output.";
        assert!(!message_matches_silent_crash(stall));
    }

    #[test]
    fn empty_message_is_not_a_silent_crash() {
        assert!(!message_matches_silent_crash(""));
    }
}
