// Background spawning helpers: fan-out an agent run on a discussion
// without blocking on the SSE response (used by the workflow runner's
// BatchQuickPrompt step), with optional silent-crash retry and
// optional Quick Prompt chaining inside the same discussion.

use crate::AppState;

use super::streaming::make_agent_stream;

/// Spawn an agent run on a discussion in the background, without SSE wrapping.
///
/// Used by the workflow runner's `BatchQuickPrompt` step executor to fan out
/// N child discs in parallel. Each call reuses the full `make_agent_stream`
/// pipeline (auth, worktree lock, agent spawn, batch progress hook) but the
/// returned SSE stream is immediately dropped.
///
/// The actual agent work runs in a detached `tokio::spawn` inside
/// `make_agent_stream` and keeps executing even after the SSE stream is
/// dropped — the spawned task checks `tx.is_closed()` only to skip streaming
/// chunks to a gone client, not to abort the run. Completion still persists
/// the agent message to DB and fires the batch progress WS events.
///
/// The `agent_semaphore` on `state` still caps concurrency across all fan-outs.
pub async fn spawn_agent_run_background(state: AppState, discussion_id: String) {
    spawn_agent_run_with_chain(state, discussion_id, Vec::new(), None).await;
}

/// Run an agent on `discussion_id` and, if the first attempt comes back with
/// the silent-crash signature, retry once.
///
/// The signature: an assistant message that starts with `[Agent exited with
/// error]` AND mentions `No output captured` — the discussion path's
/// canonical "Claude Code died, no stderr to explain why" footer (cf.
/// the message generation in `make_agent_stream` around line 1500).
///
/// Why retry pays off: the failure mode is intermittent, NOT deterministic.
/// Heavy parallel runs (Ticket Autopilot batch on 17 tickets, two workflows
/// concurrent) hit auth-file races, network pool saturation, and shared
/// resource contention that resolves on its own within a few seconds. A
/// fresh spawn after a brief backoff usually goes through.
///
/// Capped at 1 retry to keep token spend bounded — a deterministic failure
/// (auth genuinely expired, rate limit, broken prompt) won't be saved by
/// looping forever.
async fn run_with_silent_retry(state: &AppState, discussion_id: &str) {
    for attempt in 0..2 {
        let _sse = make_agent_stream(state.clone(), discussion_id.to_string(), None).await;
        drop(_sse);
        if attempt == 0 && last_message_is_silent_crash(state, discussion_id).await {
            tracing::warn!(
                "Discussion {} matched silent-crash pattern on attempt 1 — \
                 retrying once after a brief backoff (auth/network may have \
                 been transiently saturated)",
                discussion_id
            );
            // Wipe the failed message so the retry doesn't append a 2nd
            // assistant turn alongside the broken one. Idempotent: deletes
            // the trailing assistant message(s) since the last user turn.
            let did = state.db.clone();
            let did2 = discussion_id.to_string();
            let _ = did.with_conn(move |conn| {
                crate::db::discussions::delete_last_agent_messages(conn, &did2)
            }).await;
            // Backoff: lets the auth file lock release, the API rate
            // limiter window slide, etc. 5s is the sweet spot we observed
            // empirically — short enough not to feel laggy on the UI,
            // long enough to clear typical contention.
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            continue;
        }
        break;
    }
}

/// Inspect the most recent assistant message of a discussion: returns true
/// when it carries the canonical silent-crash footer. Defensive about DB
/// failures — those count as "not silent-crash" so the retry path doesn't
/// fire spuriously when the lookup itself goes wrong.
async fn last_message_is_silent_crash(state: &AppState, discussion_id: &str) -> bool {
    let did = discussion_id.to_string();
    let messages = match state.db.clone().with_conn(move |conn| {
        crate::db::discussions::list_messages(conn, &did)
    }).await {
        Ok(m) => m,
        Err(_) => return false,
    };
    let last_assistant = messages
        .iter()
        .rev()
        .find(|m| m.role == crate::models::MessageRole::Agent);
    match last_assistant {
        Some(m) => super::message_matches_silent_crash(&m.content),
        None => false,
    }
}

/// Spawn an agent run and, after it completes, execute chained Quick Prompts
/// sequentially inside the SAME discussion. Each chain step:
///
/// 1. Load the QP → render its `prompt_template` with the batch item value
///    substituted for the first variable (if any) → insert as a User message
/// 2. Re-fire the agent (via `make_agent_stream`)
/// 3. Wait for the agent to finish
///
/// The batch progress hook fires only after the final chain step.
///
/// `chain_prompt_ids` is the list of QP IDs to fire AFTER the initial run.
/// Empty = no chain, same as `spawn_agent_run_background`.
///
/// `batch_item` is the raw item value (e.g. "EW-1234") that the primary
/// QP consumed. When `Some`, every chain QP with a first variable gets
/// that variable filled with the same value — so `analyse → review →
/// summary` on ticket EW-1234 all receive `EW-1234` in their respective
/// first var. When `None` (non-batch context), chain QPs are inserted
/// verbatim; templates with unfilled `{{var}}` will reach the agent as-is.
pub async fn spawn_agent_run_with_chain(
    state: AppState,
    discussion_id: String,
    chain_prompt_ids: Vec<String>,
    batch_item: Option<String>,
) {
    // First run — the initial QP prompt was already inserted by create_batch_run.
    // Wrapped in `run_with_silent_retry` to absorb the Claude Code CLI's
    // "exit 1 + zero output" failure mode that hits batch children under
    // concurrency pressure (auth file contention, network pool saturation).
    run_with_silent_retry(&state, &discussion_id).await;

    // Chain: for each subsequent QP, inject its prompt and re-run the agent.
    for (i, qp_id) in chain_prompt_ids.iter().enumerate() {
        // Load the QP
        let qp_id_clone = qp_id.clone();
        let qp = match state.db.with_conn(move |conn| {
            crate::db::quick_prompts::get_quick_prompt(conn, &qp_id_clone)
        }).await {
            Ok(Some(qp)) => qp,
            Ok(None) => {
                tracing::warn!("Chain QP '{}' not found — skipping (step {}/{})", qp_id, i + 1, chain_prompt_ids.len());
                continue;
            }
            Err(e) => {
                tracing::error!("Chain QP '{}' DB error: {} — aborting chain", qp_id, e);
                break;
            }
        };

        // ─ Phase 4 — `{{previous_qp.output}}` chain variable ────────────
        // Before rendering this QP's template, fetch the agent's most-recent
        // reply on this discussion. That reply is either:
        //   • the initial QP's response (when i == 0), or
        //   • the previous chain step's response (when i > 0).
        // Either way the agent's last message holds what the user means by
        // "previous_qp". When no agent message exists (agent crashed before
        // replying), the placeholder is substituted with an empty string —
        // template rendering must not fail the chain.
        let disc_for_lookup = discussion_id.clone();
        let previous_output = state.db.with_conn(move |conn| {
            crate::db::discussions::list_messages(conn, &disc_for_lookup)
        }).await
            .ok()
            .and_then(|msgs| msgs.into_iter().rev()
                .find(|m| matches!(m.role, crate::models::MessageRole::Agent))
                .map(|m| m.content))
            .unwrap_or_default();

        // Render the chain QP's template (pure helper — see tests below).
        let rendered_content = render_chain_qp_prompt(
            &qp.prompt_template,
            qp.variables.first().map(|v| v.name.as_str()),
            batch_item.as_deref(),
            &previous_output,
        );

        // Insert the QP prompt as a User message
        let msg = crate::models::DiscussionMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: crate::models::MessageRole::User,
            content: rendered_content,
            agent_type: None,
            timestamp: chrono::Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: Some(format!("⚡ {}", qp.name)),
            author_avatar_email: None, source_msg_id: None, duration_ms: None,
        };
        let disc_id_for_insert = discussion_id.clone();
        if let Err(e) = state.db.with_conn(move |conn| {
            crate::db::discussions::insert_message(conn, &disc_id_for_insert, &msg)
        }).await {
            tracing::error!("Failed to insert chain QP '{}' message: {} — aborting chain", qp.name, e);
            break;
        }

        tracing::info!(
            "Chain QP '{}'  ({}/{}) injected into disc {} — firing agent",
            qp.name, i + 1, chain_prompt_ids.len(), discussion_id
        );

        // Re-fire the agent
        let _sse = make_agent_stream(state.clone(), discussion_id.clone(), None).await;
        drop(_sse);
    }
}

/// Render a chain QP's prompt template, substituting:
///   - `{{previous_qp.output}}` → the previous agent reply (Phase 4)
///   - `{{<first_var_name>}}` → `batch_item` (existing Phase 2 behavior)
///
/// Pure helper extracted from `spawn_agent_run_with_chain` so the
/// substitution rules are unit-testable without a tokio runtime / DB.
/// Order matters: chain-var substitution runs FIRST so a user-controlled
/// `batch_item` value can't smuggle a literal `{{previous_qp.output}}`
/// past us (no double-substitution surface). When no previous agent
/// reply is available, the chain-var resolves to empty string —
/// template rendering must never fail the chain.
pub(crate) fn render_chain_qp_prompt(
    template: &str,
    first_var_name: Option<&str>,
    batch_item: Option<&str>,
    previous_output: &str,
) -> String {
    let mut out = template.replace("{{previous_qp.output}}", previous_output);
    if let (Some(item), Some(var)) = (batch_item, first_var_name) {
        let placeholder = format!("{{{{{}}}}}", var);
        out = out.replace(&placeholder, item);
    }
    out
}

#[cfg(test)]
mod chain_render_tests {
    use super::render_chain_qp_prompt;

    #[test]
    fn previous_qp_output_is_substituted() {
        // Phase 4 — chain QP consumes the previous agent reply via
        // `{{previous_qp.output}}`. Use case: "brief → plan → tickets".
        let out = render_chain_qp_prompt(
            "Make tickets from this plan:\n{{previous_qp.output}}",
            None, None, "Step 1: foo\nStep 2: bar",
        );
        assert!(out.contains("Step 1: foo\nStep 2: bar"));
        assert!(!out.contains("{{previous_qp.output}}"));
    }

    #[test]
    fn previous_qp_output_substituted_with_empty_when_no_previous() {
        // If the agent crashed before replying, the chain var must
        // resolve to empty string — never leave the placeholder syntax
        // exposed to the agent prompt.
        let out = render_chain_qp_prompt(
            "Refine:\n{{previous_qp.output}}\n— done.",
            None, None, "",
        );
        assert_eq!(out, "Refine:\n\n— done.");
    }

    #[test]
    fn first_var_substituted_with_batch_item() {
        // Phase 2 behavior — first user-defined var receives the batch
        // item value. Unchanged by Phase 4.
        let out = render_chain_qp_prompt(
            "Analyse {{ticket}}",
            Some("ticket"), Some("EW-1234"), "",
        );
        assert_eq!(out, "Analyse EW-1234");
    }

    #[test]
    fn previous_output_and_batch_item_both_substituted() {
        let out = render_chain_qp_prompt(
            "On {{ticket}}: refine the plan below.\n{{previous_qp.output}}",
            Some("ticket"), Some("EW-1234"), "Plan v1",
        );
        assert_eq!(
            out,
            "On EW-1234: refine the plan below.\nPlan v1",
        );
    }

    #[test]
    fn batch_item_carrying_chain_var_does_not_double_substitute() {
        // Regression guard: a malicious `batch_item` MUST NOT smuggle
        // a `{{previous_qp.output}}` placeholder that would then be
        // resolved post-hoc. Order in `render_chain_qp_prompt` runs
        // chain-var substitution FIRST, so by the time batch_item lands
        // the chain-var pass is already over. The literal text from the
        // batch item survives intact.
        let out = render_chain_qp_prompt(
            "Title: {{ticket}}",
            Some("ticket"),
            Some("{{previous_qp.output}}-EW-1"),
            "<<should-not-leak>>",
        );
        assert_eq!(out, "Title: {{previous_qp.output}}-EW-1",
            "batch_item value must not be re-rendered against the chain var");
    }

    #[test]
    fn no_var_no_batch_item_returns_template_as_is() {
        let out = render_chain_qp_prompt("Static prompt", None, None, "");
        assert_eq!(out, "Static prompt");
    }
}
