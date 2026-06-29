// The big one: `make_agent_stream` is the SSE-producing handler core
// shared by `send_message` and `run_agent`. It reads the discussion
// state, optionally re-attaches an Isolated worktree, spawns the agent
// process via `runner::start_agent_with_config`, multiplexes its
// stdout into typed `AgentStreamEvent`s, enforces stall + global
// timeouts, intercepts terminal `KRONN:*` signals to break out of
// runaway agents, persists the assistant message, fires the batch
// progress hook, and wraps the SSE in an `sse_limits::bounded`
// envelope so dropped clients don't OOM the server.
//
// Also hosts the lower-level helpers (`run_agent_streaming`,
// `run_agent_collect`) that the `orchestrate` handler calls per round.

use std::convert::Infallible;
use std::time::Duration;

use axum::{
    response::sse::{Event, Sse},
};
use chrono::Utc;
use uuid::Uuid;

use crate::agents::runner;
use crate::models::*;
use crate::AppState;

use crate::api::disc_helpers::{auth_mode_for, estimate_extra_context_len};
use crate::api::disc_prompts::build_agent_prompt;
use super::orchestration::detect_agent_error_hint;
use super::{
    detect_terminal_signal, truncate_after_signal, AgentStreamEvent, SseStream,
    AGENT_GLOBAL_TIMEOUT, DEFAULT_STALL_TIMEOUT_MIN, MAX_AGENT_RESPONSE_BYTES,
    NON_STREAMING_STALL_TIMEOUT,
};

// ── Decoder-loop detector (shared by make_agent_stream + run_agent_streaming) ──
//
// Guards against Claude Opus extended-thinking decoder loops (EW-7189:
// `</thinking>\n` × 6349 in one stream). When the same non-trivial text delta
// arrives `DECODER_LOOP_MAX_REPEATS` times in a row, the caller kills the
// agent. Whitespace / very-short deltas (". ", "\n") can repeat legitimately
// in formatted output, so they're ignored. `strip_thinking_leaks` in the
// parser normally catches the known leak, but the same mechanic could trigger
// on any repeating token — the detector stays kind-agnostic.
pub(super) const DECODER_LOOP_MAX_REPEATS: u32 = 50;
const DECODER_LOOP_MIN_LEN: usize = 3;

/// Stateful repeat detector. Caller owns `last`/`count` across the stream.
/// Returns `true` once the same non-trivial delta has repeated
/// `DECODER_LOOP_MAX_REPEATS` times — the caller then aborts the run.
/// Extracted (0.8.8) so both streaming loops share one implementation
/// instead of two byte-identical copies.
pub(super) fn is_decoder_loop(text: &str, last: &mut String, count: &mut u32) -> bool {
    if text.len() >= DECODER_LOOP_MIN_LEN && !text.trim().is_empty() {
        if text == *last {
            *count += 1;
            if *count >= DECODER_LOOP_MAX_REPEATS {
                return true;
            }
        } else {
            *last = text.to_string();
            *count = 1;
        }
    }
    false
}

/// How long the stall watchdog waits for stdout before killing the agent.
///
/// Streaming agents (Claude `--output-format stream-json`) emit a chunk every
/// few hundred ms, so a long silence genuinely means a hang → use the
/// configured stall. NON-streaming agents (`OutputMode::Text` — Codex `exec`
/// and friends) write their answer ONLY at the very end and are legitimately
/// silent on stdout for the whole run; applying the stall to them killed
/// slow-but-healthy runs and left an empty discussion (2026-06-23: every Codex
/// batch child died this way while the same workflow worked on Claude). For
/// those we drop the stall and rely solely on the absolute global deadline.
/// Pure — unit-tested.
pub(super) fn effective_stall_timeout(
    is_stream_json: bool,
    configured: std::time::Duration,
    global: std::time::Duration,
) -> std::time::Duration {
    if is_stream_json { configured } else { global }
}

/// Whether a finished child run counts as a SUCCESS for batch accounting.
///
/// A clean process exit with an EMPTY assistant reply is NOT a success — the
/// child produced nothing usable. Counting it as completed is how a batch
/// workflow reported a green "Success" while all its discussions were empty
/// (2026-06-23: Codex children exited 0 but silent → 16 empty discs counted as
/// "16 completed"). Require BOTH a clean exit AND a non-blank response. Pure —
/// unit-tested. Applies uniformly to every agent (an empty Claude reply isn't
/// a successful child either), so it doesn't single out one CLI.
pub(super) fn child_run_counts_as_success(exit_success: bool, response: &str) -> bool {
    exit_success && !response.trim().is_empty()
}

/// Hard byte-cap on a persisted agent message, applied at the persistence
/// boundary so EVERY path is bounded.
///
/// The streaming loop caps stdout at `MAX_AGENT_RESPONSE_BYTES`, but the
/// error/kill path REPLACES the response with the full captured stderr, which
/// is NOT capped — a killed verbose agent (Codex exec, silent-until-end) left a
/// 2.4 MB message that froze then crashed the browser tab on open (2026-06-23).
/// Char-boundary-safe: stderr carries UTF-8 (French errors, emoji from npm), so
/// a naive byte truncate would panic. Pure — unit-tested.
pub(super) fn cap_agent_response(mut content: String, limit: usize) -> String {
    if content.len() <= limit {
        return content;
    }
    let mut cut = limit;
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    content.truncate(cut);
    content.push_str("\n\n[… message tronqué — dépassait la limite de stockage …]");
    content
}

/// A completed tool call, classified into the transcript bucket the UI
/// renders it in. `mcp__kronn-internal__*` calls go to the Kronn-MCP banner ;
/// everything else (Claude-native Read/Bash/Edit, third-party MCP) to the
/// agent-native banner. Pure — extracted (0.8.8) from `make_agent_stream`'s
/// `ToolEnd` arm so the bucketing + arg-formatting is unit-testable.
pub(super) enum ToolRecord {
    Kronn(String),
    Native(String),
}

/// Format a finished tool call into its transcript record. kronn-internal
/// calls get pretty-printed args (`disc_get_message(4)`) ; native calls get
/// their raw input truncated to ~120 chars to keep the banner compact.
pub(super) fn classify_tool_call(tool: &str, input: &str) -> ToolRecord {
    if let Some(name) = tool.strip_prefix("mcp__kronn-internal__") {
        let pretty_args = pretty_kronn_args(name, input);
        ToolRecord::Kronn(format!("[kronn-internal: {}({})]", name, pretty_args))
    } else {
        let args = if input.is_empty() {
            String::new()
        } else {
            truncate_tool_args(input, 120)
        };
        ToolRecord::Native(format!("[agent-native: {}({})]", tool, args))
    }
}

/// Shared SSE stream builder.
///
/// 0.8.6 phase 4 — visibility bumped to `pub(crate)` so the MCP-remote
/// route `qp_run` can fire-and-forget the agent in a background
/// `tokio::spawn`. The spawned task drops the returned `Sse` handle ;
/// the internal channel's senders use `let _ = tx.send(...)` so a
/// dropped receiver does NOT cancel the agent — the message still
/// gets persisted to DB.
pub(crate) async fn make_agent_stream(
    state: AppState,
    discussion_id: String,
    agent_override: Option<AgentType>,
) -> Sse<SseStream> {
    // 0.8.5 — capture the agent-run start wallclock. The delta between
    // this and the moment we commit the Agent message gives us the
    // real reply duration in milliseconds (excludes user typing time).
    // Stored on `messages.duration_ms` for the QP-metrics aggregator.
    let run_started_at: std::time::Instant = std::time::Instant::now();

    // Extract info from DB
    let disc = state.db.with_conn({
        let did = discussion_id.clone();
        move |conn| crate::db::discussions::get_discussion(conn, &did)
    }).await.ok().flatten();

    if disc.is_none() {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default().event("error").data("{\"error\":\"Discussion not found\"}")
            )
        }));
        return Sse::new(stream);
    }

    let disc = match disc {
        Some(d) => d,
        None => {
            let stream: SseStream = Box::pin(futures::stream::once(async {
                Ok::<_, Infallible>(Event::default().event("error").data(
                    serde_json::json!({ "error": "Discussion not found" }).to_string()
                ))
            }));
            return Sse::new(stream);
        }
    };
    let agent_type = agent_override.unwrap_or_else(|| disc.agent.clone());
    let disc_tier = disc.tier;
    let skill_ids = disc.skill_ids.clone();
    let directive_ids = disc.directive_ids.clone();
    let profile_ids = disc.profile_ids.clone();
    let mut workspace_path = disc.workspace_path.clone();
    // Captured for the batch progress hook at the end of the stream — if
    // this disc was spawned by a batch run, we increment its counters and
    // broadcast a WS event when it finishes.
    let batch_run_id = disc.workflow_run_id.clone();

    // ── Batch child START hook ──────────────────────────────────────────
    // Symmetric to the BatchRunProgress/BatchRunFinished broadcast at the end
    // of the stream. Batch children run server-side with no SSE consumer on
    // the client, so without this the per-disc `sendingMap` is never set to
    // `true` and an in-flight child shows no "agent working" spinner. Fire it
    // the moment the run begins so any connected client flips the indicator on.
    if let Some(ref run_id) = batch_run_id {
        let _ = state.ws_broadcast.send(WsMessage::BatchRunChildStarted {
            run_id: run_id.clone(),
            discussion_id: discussion_id.clone(),
        });
    }

    let project_path = if let Some(ref pid) = disc.project_id {
        let pid = pid.clone();
        state.db.with_conn(move |conn| {
            let p = crate::db::projects::get_project(conn, &pid)?;
            Ok(p.map(|p| p.path).unwrap_or_default())
        }).await.unwrap_or_default()
    } else {
        String::new()
    };

    // Auto re-lock: if discussion is Isolated but worktree was unlocked, re-create it
    if disc.workspace_mode == "Isolated" && workspace_path.is_none() && !project_path.is_empty() {
        if let Some(ref branch) = disc.worktree_branch {
            let resolved = crate::core::scanner::resolve_host_path(&project_path);
            let repo_path = std::path::Path::new(&resolved);

            // Fetch project name for slug
            let pname = if let Some(ref pid) = disc.project_id {
                let pid = pid.clone();
                state.db.with_conn(move |conn| {
                    let p = crate::db::projects::get_project(conn, &pid)?;
                    Ok(p.map(|p| p.name).unwrap_or_default())
                }).await.unwrap_or_default()
            } else {
                String::new()
            };

            match crate::core::worktree::reattach_worktree(repo_path, &pname, &disc.title, branch) {
                Ok(info) => {
                    let did = disc.id.clone();
                    let wp = info.path.clone();
                    let wb = info.branch.clone();
                    let _ = state.db.with_conn(move |conn| {
                        crate::db::discussions::update_discussion_workspace(conn, &did, &wp, &wb)
                    }).await;
                    tracing::info!("Auto re-locked worktree for discussion '{}'", disc.title);
                    workspace_path = Some(info.path);
                }
                Err(e) => {
                    tracing::warn!("Auto re-lock failed for '{}': {}", disc.title, e);
                    let err_msg = if e.contains("currently checked out") {
                        e.clone()
                    } else {
                        format!("Failed to re-create worktree: {}", e)
                    };
                    let stream: SseStream = Box::pin(futures::stream::once(async move {
                        Ok::<_, Infallible>(
                            Event::default().event("error").data(
                                serde_json::json!({ "error": err_msg }).to_string()
                            )
                        )
                    }));
                    return Sse::new(stream);
                }
            }
        }
    }

    // For general discussions (no project), write .mcp.json + build MCP context.
    // For project discussions, also ensure the .mcp.json is fresh on disk
    // (covers the case where MCPs were added/toggled since the last sync).
    let global_mcp_context = if project_path.is_empty() {
        tracing::debug!(target: "kronn::mcp", disc_id = %discussion_id, "no project — loading global MCPs only");
        crate::api::disc_git::prepare_general_mcp(&state, &workspace_path).await
    } else {
        // Re-sync the project's .mcp.json BEFORE the agent reads it.
        // Without this, MCPs toggled/added after the last startup sync
        // (or a batch discussion spawned right after a new MCP config)
        // would have a stale or empty .mcp.json on disk.
        //
        // 0.8.3 (#280) — SKIP the sync when an audit is currently
        // running on this project. The audit pipeline has installed
        // an `AuditMcpSwap` that filtered `.mcp.json` to the audit
        // allowlist; re-writing the file here would clobber the swap
        // and silently break the audit (the agent's next step would
        // see all 15 MCPs again, losing the perf optimization). The
        // user's discussion still sees the filtered subset until the
        // audit finishes — the frontend banner explains why (see
        // ProjectCard / DiscussionsPage).
        let audit_running = state
            .audit_tracker
            .lock()
            .ok()
            .and_then(|t| disc.project_id.as_ref().map(|pid| t.progress.contains_key(pid)))
            .unwrap_or(false);
        if !audit_running {
            if let Some(ref pid) = disc.project_id {
                let secret = {
                    let cfg = state.config.read().await;
                    cfg.encryption_secret.clone()
                };
                if let Some(secret) = secret {
                    let pid = pid.clone();
                    let _ = state.db.with_conn(move |conn| {
                        let _ = crate::core::mcp_scanner::sync_project_mcps_to_disk(conn, &pid, &secret);
                        Ok::<_, anyhow::Error>(())
                    }).await;
                }
            }
        } else {
            tracing::debug!(
                target: "kronn::mcp",
                disc_id = %discussion_id,
                "audit in progress on project — skipping `.mcp.json` sync to preserve the audit-mode filter"
            );
        }

        // Log what the agent will see so debug-mode users can verify
        let mcp_path = crate::core::scanner::resolve_host_path(&project_path).join(".mcp.json");
        if mcp_path.exists() {
            let server_count = std::fs::read_to_string(&mcp_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("mcpServers").and_then(|m| m.as_object()).map(|m| m.len()))
                .unwrap_or(0);
            tracing::debug!(target: "kronn::mcp",
                disc_id = %discussion_id,
                project = %project_path,
                mcp_json_servers = server_count,
                "project .mcp.json found — {} MCP server(s) will be available to the agent",
                server_count,
            );
        } else {
            tracing::warn!(target: "kronn::mcp",
                disc_id = %discussion_id,
                project = %project_path,
                "project .mcp.json NOT FOUND — agent will have NO MCP tools. \
                 Check: is the project linked to any MCP config? Is the MCP global or project-scoped?",
            );
        }

        // Build the API plugin block and — if present — combine with the
        // disk-read MCP context so both reach the agent via
        // `mcp_context_override`. Without this, API plugins never surface
        // because `.mcp.json` doesn't carry them by design.
        let api_block = if let Some(ref pid) = disc.project_id {
            let secret = {
                let cfg = state.config.read().await;
                cfg.encryption_secret.clone()
            };
            match secret {
                Some(secret) => {
                    let pid = pid.clone();
                    let secret_c = secret.clone();
                    // Step 1 (blocking): decrypt configs from the DB.
                    let mut plugins = state.db.with_conn(move |conn| {
                        crate::core::mcp_scanner::collect_active_api_plugins(
                            conn, &pid, &secret_c,
                        )
                    }).await.unwrap_or_default();

                    // Step 2 (async): for every plugin whose auth is
                    // OAuth2ClientCredentials, resolve a fresh bearer token
                    // via the cache. We inject the result under the virtual
                    // env keys `__access_token__` / `__token_error__` so the
                    // sync context builder can read them without knowing
                    // the auth flow. Per-plugin isolation: one bad token
                    // doesn't hide the others.
                    for (server, config_id, env) in plugins.iter_mut() {
                        if let Some(ref spec) = server.api_spec {
                            if matches!(spec.auth, crate::models::ApiAuthKind::OAuth2ClientCredentials { .. }) {
                                // 2026-06-10 — the cache key is the EXACT
                                // config id surfaced by the collector. The
                                // previous DB re-derive matched the FIRST
                                // config of the server on the project, so
                                // two instances with different credentials
                                // shared (and corrupted) one cached token.
                                match crate::core::oauth2_cache::resolve_token(
                                    &state.oauth2_cache, config_id, &spec.auth, env,
                                ).await {
                                    Ok(tok) => { env.insert("__access_token__".into(), tok); }
                                    Err(e) => {
                                        tracing::warn!(
                                            "OAuth2 token exchange failed for plugin '{}' config {}: {}",
                                            server.name, config_id, e
                                        );
                                        env.insert("__token_error__".into(), e);
                                    }
                                }
                            }
                        }
                    }

                    // Step 3 (sync): render the block.
                    crate::core::mcp_scanner::build_api_context_block(&plugins)
                }
                None => String::new(),
            }
        } else {
            String::new()
        };

        if api_block.is_empty() {
            // No API plugins active — let runner.rs fall back to reading
            // MCP contexts from disk (unchanged legacy path).
            None
        } else {
            // At least one API plugin active — we must pre-combine the
            // disk-read MCP context with the API block, since
            // `mcp_context_override = Some(...)` short-circuits the
            // disk read in runner.rs.
            let disk_ctx = crate::core::mcp_scanner::read_all_mcp_contexts(&project_path);
            let combined = if disk_ctx.is_empty() {
                api_block
            } else {
                format!("{}\n{}", disk_ctx, api_block)
            };
            Some(combined)
        }
    };

    // Load context files for prompt injection
    let context_files_prompt = {
        let did = discussion_id.clone();
        let entries = state.db.with_conn(move |conn| {
            crate::db::discussions::get_context_files_for_prompt(conn, &did).map_err(|e| anyhow::anyhow!(e))
        }).await.unwrap_or_default();
        crate::core::context_files::build_context_prompt(&entries)
    };

    // Inject user bio (first exchange only) + global context (always).
    let (tokens, full_access, model_tiers_config, user_bio, global_context) = {
        let config = state.config.read().await;
        let fa = config.agents.full_access_for(&agent_type);
        let bio = if disc.messages.len() <= 2 {
            config.server.bio.clone().filter(|b| !b.trim().is_empty())
        } else {
            None
        };
        let gc = {
            let mode = config.server.global_context_mode.as_str();
            let has_project = disc.project_id.is_some();
            match mode {
                "never" => None,
                "no_project" if has_project => None,
                _ => config.server.global_context.clone().filter(|g| !g.trim().is_empty()),
            }
        };
        (config.tokens.clone(), fa, config.agents.model_tiers.clone(), bio, gc)
    };

    // Build the context preamble: user bio (first exchange) + global context (always)
    let context_files_prompt = {
        let mut preamble = String::new();
        if let Some(ref bio) = user_bio {
            let pseudo = disc.messages.first()
                .and_then(|m| m.author_pseudo.as_deref())
                .unwrap_or("User");
            preamble.push_str(&format!("--- About the user ({}) ---\n{}\n\n", pseudo, bio));
        }
        if let Some(ref gc) = global_context {
            preamble.push_str(&format!("--- Global context ---\n{}\n\n", gc));
        }
        format!("{}{}", preamble, context_files_prompt)
    };

    // 0.8.3 (TD-265) — companion-repo context (linked_repos + Kronn
    // projects universe). Same blocks the audit pipeline and workflow
    // runner already inject. Without this, an agent chatting in a
    // discussion can't see what companion repos the user has wired —
    // it would re-ask "do you have a frontend repo for this?" every
    // turn even though the user has `front_api` registered as a
    // linked_repo on the project. Empty string for general (no-project)
    // discussions; cheap (2 DB reads) on project discussions.
    let companion_context = crate::api::projects::compute_companion_context(
        &state,
        disc.project_id.as_deref(),
    ).await;
    let context_files_prompt = if companion_context.is_empty() {
        context_files_prompt
    } else {
        format!("{}{}", context_files_prompt, companion_context)
    };

    // Estimate extra_context size so build_agent_prompt can respect the agent's budget.
    // This mirrors what runner::start_agent_with_config will build.
    let extra_context_len = estimate_extra_context_len(
        &skill_ids, &directive_ids, &profile_ids,
        &project_path, global_mcp_context.as_deref(), &agent_type,
    ) + context_files_prompt.len();
    let prompt = build_agent_prompt(&disc, &agent_type, extra_context_len);

    let auth_mode_str = auth_mode_for(&agent_type, &tokens);

    let disc_id = discussion_id.clone();
    let disc_project_id = disc.project_id.clone();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentStreamEvent>(64);

    // Register a cancellation token keyed by the disc id so the "⏹ Arrêter"
    // UI (POST /api/discussions/:id/stop) can trigger it. The CancelGuard
    // removes the entry from the registry when this task's scope exits —
    // either on normal completion or via panic/early return.
    let cancel_guard = crate::CancelGuard::insert(&state.cancel_registry, disc_id.clone());
    let cancel_token = cancel_guard.token.clone();

    // Spawn background task — always saves to DB even if client disconnects
    let semaphore = state.agent_semaphore.clone();
    tokio::spawn(async move {
        // Keep the guard alive for the lifetime of this task. Dropping it at
        // the end of the move closure removes the token from the registry.
        let _cancel_guard = cancel_guard;
        // Acquire semaphore permit — limits concurrent agent processes
        let _permit = match semaphore.acquire_owned().await {
            Ok(p) => p,
            Err(_) => {
                let _ = tx.send(AgentStreamEvent::Error {
                    data: serde_json::json!({ "error": "Server shutting down" }),
                }).await;
                return;
            }
        };

        let _ = tx.send(AgentStreamEvent::Start).await;
        let _ = tx.send(AgentStreamEvent::Meta { auth_mode: auth_mode_str.clone() }).await;

        match runner::start_agent_with_config(runner::AgentStartConfig {
            work_dir: workspace_path.as_deref(),
            full_access,
            skill_ids: &skill_ids, directive_ids: &directive_ids, profile_ids: &profile_ids,
            mcp_context_override: global_mcp_context.as_deref(),
            tier: disc_tier, model_tiers: Some(&model_tiers_config),
            context_files_prompt: &context_files_prompt,
            // Forward to the agent process env so the kronn-internal MCP
            // bridge knows which discussion to introspect when called.
            discussion_id: Some(&discussion_id),
            ..runner::AgentStartConfig::new(&agent_type, &project_path, &prompt, &tokens)
        }).await {
            Ok(mut process) => {
                let mut full_response = String::new();
                let mut stream_json_tokens: u64 = 0;
                let mut stream_json_cost: Option<f64> = None;
                let is_stream_json = process.output_mode == runner::OutputMode::StreamJson;
                // Track current tool for rich log messages
                let mut current_tool: Option<String> = None;
                let mut current_tool_input = String::new();
                // Capture kronn-internal MCP tool calls so we can persist them as
                // System messages after the agent reply lands. Same shape as the
                // slash-marker fallback for Vibe/Ollama (`slash_markers.rs`), so
                // the UI shows a uniform `[kronn-internal: …]` badge regardless
                // of which agent path triggered the introspection.
                let mut kronn_tool_calls: Vec<String> = Vec::new();
                // 0.8.6 phase 4 — also capture EVERY OTHER tool call (Claude
                // Code natives like `Read` / `Bash` / `Edit` / `Grep`, plus
                // third-party MCP servers wired in the project). Same shape
                // but with the `[agent-native: …]` prefix so the frontend
                // can render them in a SEPARATE banner from Kronn-MCP calls.
                // User feedback 2026-05-22 : the live in-stream tool log
                // disappears when the stream ends, leaving no trace for
                // post-hoc debug. Persisting them keeps the audit trail.
                let mut native_tool_calls: Vec<String> = Vec::new();
                let global_deadline = tokio::time::Instant::now() + AGENT_GLOBAL_TIMEOUT;

                // Periodic checkpoint of full_response → discussions.partial_response
                // so a backend crash/restart doesn't lose what the agent has thought.
                // Throttled to ~30s OR 100 chunks (whichever first) to bound DB writes
                // even during high-throughput agents like Claude Code.
                let mut last_checkpoint = tokio::time::Instant::now();
                let mut chunks_since_checkpoint: usize = 0;
                const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(30);
                const CHECKPOINT_CHUNKS: usize = 100;
                let checkpoint_disc_id = disc_id.clone();
                let checkpoint_db = state.db.clone();
                // Helper: best-effort flush, never propagates DB errors to the agent loop.
                let do_checkpoint = |partial: String| {
                    let did = checkpoint_disc_id.clone();
                    let db = checkpoint_db.clone();
                    tokio::spawn(async move {
                        if let Err(e) = db.with_conn(move |conn| {
                            crate::db::discussions::set_partial_response(conn, &did, Some(&partial))
                        }).await {
                            tracing::warn!("partial_response checkpoint failed: {}", e);
                        }
                    });
                };

                // Stream stderr logs to the client in real-time
                let stderr_log_capture = process.stderr_capture.clone();
                let log_tx = tx.clone();
                let log_task = tokio::spawn(async move {
                    let mut last_len = 0;
                    loop {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        let lines = match stderr_log_capture.lock() {
                            Ok(g) => g.clone(),
                            Err(e) => { tracing::warn!("stderr lock poisoned: {}", e); break; }
                        };
                        if lines.len() > last_len {
                            for line in &lines[last_len..] {
                                let trimmed = line.trim();
                                if !trimmed.is_empty() {
                                    let _ = log_tx.send(AgentStreamEvent::Log { text: trimmed.to_string() }).await;
                                }
                            }
                            last_len = lines.len();
                        }
                        if log_tx.is_closed() { break; }
                    }
                });
                let stall_timeout_min = {
                    let cfg = state.config.read().await;
                    let t = cfg.server.agent_stall_timeout_min;
                    if t > 0 { t } else { DEFAULT_STALL_TIMEOUT_MIN }
                };
                // Streaming agents use the configured stall; non-streaming
                // (Text) agents are silent until the end and rely on the global
                // deadline instead. See `effective_stall_timeout`.
                let stall_timeout = effective_stall_timeout(
                    is_stream_json,
                    Duration::from_secs(stall_timeout_min as u64 * 60),
                    NON_STREAMING_STALL_TIMEOUT,
                );
                let mut was_interrupted = false;
                // Set when we break the loop because the agent emitted a
                // terminal signal (KRONN:ARCHITECTURE_READY, etc.). Used to
                // distinguish from a stall timeout when killing the process
                // — both paths end up calling kill() but only stalls add a
                // partial-response footer.
                let mut stopped_on_signal: Option<&'static str> = None;
                // Set when we break because full_response exceeded
                // MAX_AGENT_RESPONSE_BYTES. We then kill the child and
                // append a footer so the user sees what happened.
                let mut stopped_on_size: bool = false;
                // Set when the user clicked "⏹ Arrêter" from the UI and the
                // POST /api/discussions/:id/stop handler triggered our token.
                // We then kill the child and save the partial response with
                // a footer so the user sees what happened.
                let mut stopped_on_cancel: bool = false;
                // Runaway-repeat detector — guards against Claude Opus
                // extended-thinking decoder loops (observed on EW-7189:
                // `</thinking>\n` × 6349 in one stream). When the same
                // non-trivial delta arrives N times in a row we kill the
                // child. Detection lives in the shared `is_decoder_loop`
                // helper (module top) ; these own the per-stream state.
                let mut last_text_delta = String::new();
                let mut repeat_delta_count: u32 = 0;
                let mut stopped_on_loop: bool = false;

                // Stall timeout pattern: the `tokio::time::sleep(stall_timeout)` future
                // is created fresh on each iteration of the `while let` loop because the
                // entire `select!` block is re-evaluated. This is intentional — each time
                // process.next_line() yields a line, we re-enter the loop, creating a NEW
                // sleep future, effectively resetting the stall timer. If the agent produces
                // no output for `stall_timeout`, the sleep wins the select! and we break.
                // The global_deadline sleep_until is NOT reset (absolute deadline).
                while let Some(line) = tokio::select! {
                    line = process.next_line() => line,
                    _ = cancel_token.cancelled() => {
                        tracing::info!("Agent stream for disc {} cancelled by user", disc_id);
                        stopped_on_cancel = true;
                        None
                    }
                    _ = tokio::time::sleep_until(global_deadline) => {
                        tracing::warn!("Agent stream global timeout ({:?}) exceeded", AGENT_GLOBAL_TIMEOUT);
                        was_interrupted = true;
                        None
                    }
                    _ = async {
                        tokio::time::sleep(stall_timeout).await
                    } => {
                        tracing::warn!("Agent stream stall timeout ({:?}) — no output", stall_timeout);
                        was_interrupted = true;
                        None
                    }
                } {
                    // Client disconnected — keep running to save result in DB
                    let client_gone = tx.is_closed();

                    if is_stream_json {
                        match runner::parse_claude_stream_line(&line) {
                            runner::StreamJsonEvent::Text(text) => {
                                // Loop-repeat detection — see constants above.
                                // Non-whitespace deltas of >= REPEAT_MIN_LEN are
                                // the dangerous ones; whitespace/very short
                                // deltas (". ", "\n") can repeat legitimately
                                // in formatted output without signalling a
                                // decoder loop.
                                if is_decoder_loop(&text, &mut last_text_delta, &mut repeat_delta_count) {
                                    tracing::warn!(
                                        "Agent stream entered a decoder loop — same delta {:?} repeated {} times, aborting",
                                        text.chars().take(40).collect::<String>(),
                                        repeat_delta_count,
                                    );
                                    stopped_on_loop = true;
                                    was_interrupted = true;
                                    break;
                                }
                                full_response.push_str(&text);
                                chunks_since_checkpoint += 1;
                                // Throttled checkpoint to DB (Option A) — survives backend restart
                                if chunks_since_checkpoint >= CHECKPOINT_CHUNKS
                                    || last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL
                                {
                                    do_checkpoint(full_response.clone());
                                    last_checkpoint = tokio::time::Instant::now();
                                    chunks_since_checkpoint = 0;
                                }
                                if !client_gone {
                                    let chunk = serde_json::json!({ "text": text });
                                    let _ = tx.send(AgentStreamEvent::Chunk { data: chunk }).await;
                                }
                                // Terminal-signal detection — see TERMINAL_SIGNALS doc.
                                if let Some(sig) = detect_terminal_signal(&full_response) {
                                    tracing::info!("Terminal signal {} detected — stopping agent", sig);
                                    // Strip anything the LLM wrote AFTER the signal in
                                    // the same chunk (orphan letters, half-sentences).
                                    // The skill rule is "STOP immediately after the
                                    // signal" — we enforce it visually so the saved
                                    // message ends cleanly on the marker.
                                    full_response = truncate_after_signal(&full_response, sig);
                                    stopped_on_signal = Some(sig);
                                    break;
                                }
                                if full_response.len() > MAX_AGENT_RESPONSE_BYTES {
                                    tracing::warn!(
                                        "Agent response exceeded {} bytes — killing to prevent runaway",
                                        MAX_AGENT_RESPONSE_BYTES
                                    );
                                    stopped_on_size = true;
                                    break;
                                }
                            }
                            runner::StreamJsonEvent::Usage { input_tokens, output_tokens, cost_usd } => {
                                stream_json_tokens = stream_json_tokens.max(input_tokens + output_tokens);
                                if let Some(c) = cost_usd {
                                    stream_json_cost = Some(c);
                                }
                            }
                            runner::StreamJsonEvent::ToolStart(name) => {
                                current_tool = Some(name);
                                current_tool_input.clear();
                            }
                            runner::StreamJsonEvent::ToolInputDelta(partial) => {
                                current_tool_input.push_str(&partial);
                            }
                            runner::StreamJsonEvent::ToolEnd => {
                                if let Some(ref tool) = current_tool {
                                    let log = crate::api::disc_git::format_tool_log(tool, &current_tool_input);
                                    if !client_gone {
                                        let _ = tx.send(AgentStreamEvent::Log { text: log }).await;
                                    }
                                    // Persist tool calls in the disc transcript
                                    // so the UI banner can render them after the
                                    // agent reply lands. Two source buckets so
                                    // the frontend can split them visually :
                                    //   - `mcp__kronn-internal__*` → kronn-internal
                                    //     (the deagentified MCP exposed by Kronn)
                                    //   - everything else → agent-native (Claude
                                    //     Code's own Read/Bash/Edit, third-party
                                    //     MCP servers, etc.).
                                    match classify_tool_call(tool, &current_tool_input) {
                                        ToolRecord::Kronn(record) => kronn_tool_calls.push(record),
                                        ToolRecord::Native(record) => native_tool_calls.push(record),
                                    }
                                }
                                current_tool = None;
                                current_tool_input.clear();
                            }
                            runner::StreamJsonEvent::Skip => {}
                        }
                    } else {
                        if !full_response.is_empty() {
                            full_response.push('\n');
                        }
                        full_response.push_str(&line);
                        chunks_since_checkpoint += 1;
                        if chunks_since_checkpoint >= CHECKPOINT_CHUNKS
                            || last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL
                        {
                            do_checkpoint(full_response.clone());
                            last_checkpoint = tokio::time::Instant::now();
                            chunks_since_checkpoint = 0;
                        }

                        if !client_gone {
                            let text_with_nl = if full_response.len() > line.len() {
                                format!("\n{}", line)
                            } else {
                                line.clone()
                            };
                            let chunk = serde_json::json!({ "text": text_with_nl });
                            let _ = tx.send(AgentStreamEvent::Chunk { data: chunk }).await;
                        }
                        if let Some(sig) = detect_terminal_signal(&full_response) {
                            tracing::info!("Terminal signal {} detected — stopping agent", sig);
                            full_response = truncate_after_signal(&full_response, sig);
                            stopped_on_signal = Some(sig);
                            break;
                        }
                        if full_response.len() > MAX_AGENT_RESPONSE_BYTES {
                            tracing::warn!(
                                "Agent response exceeded {} bytes — killing to prevent runaway",
                                MAX_AGENT_RESPONSE_BYTES
                            );
                            stopped_on_size = true;
                            break;
                        }
                    }
                }

                // Stop the stderr log streamer
                log_task.abort();

                // Kill agent on timeout/stall OR terminal signal OR size cap
                // OR user-triggered cancel OR decoder-loop detection
                // (process may still be running and producing output here).
                if was_interrupted || stopped_on_signal.is_some() || stopped_on_size || stopped_on_cancel || stopped_on_loop {
                    let _ = process.child.kill().await;
                }

                let status = process.child.wait().await;
                process.fix_ownership();
                let exit_info = match &status {
                    Ok(s) => format!("exit code: {:?}", s.code()),
                    Err(e) => format!("wait error: {}", e),
                };
                // A signal-driven stop is a SUCCESS even though we killed the
                // child — the agent did exactly what we asked. Wait status
                // will report a non-zero exit code from SIGKILL, so we
                // explicitly mark these as successful.
                // A user cancel is NOT a success — we want the run to be
                // flagged as failed so batch counters see it as a failure
                // and the UI treats the partial response as interrupted.
                let success = if stopped_on_signal.is_some() {
                    true
                } else if stopped_on_cancel {
                    false
                } else {
                    !was_interrupted && status.map(|s| s.success()).unwrap_or(false)
                };

                let stderr_lines = process.captured_stderr_flushed().await;
                let stderr_text = stderr_lines.join("\n");

                // Mark partial responses with actionable hint. `stopped_on_loop`
                // also sets `was_interrupted`, but its dedicated footer below
                // is more specific — skip the generic stall footer when a
                // loop was detected.
                if was_interrupted && !stopped_on_loop && !full_response.is_empty() {
                    full_response.push_str(&format!(
                        "\n\n---\n⚠️ **Partial response** — the agent was interrupted after {} min without output. \
                        You can increase the timeout in **Config > Server > Agent inactivity timeout**.",
                        stall_timeout_min
                    ));
                }
                if stopped_on_loop {
                    full_response.push_str(&format!(
                        "\n\n---\n🔁 **Decoder loop detected** — the agent emitted the same token {} times \
                        in a row and was killed to stop the pollution. This is a known failure mode \
                        (often extended-thinking leak on Opus). Try re-running with a fresh prompt — \
                        adjusting the question wording usually avoids it.",
                        DECODER_LOOP_MAX_REPEATS
                    ));
                }
                if stopped_on_size {
                    full_response.push_str(&format!(
                        "\n\n---\n🛑 **Response cut off** — the agent produced more than {} KB of output, \
                        which usually means it's stuck in a loop. Killed to prevent runaway costs. \
                        Review the work above and decide whether to continue with a fresh prompt.",
                        MAX_AGENT_RESPONSE_BYTES / 1024
                    ));
                }
                if stopped_on_cancel {
                    let footer = "\n\n---\n⏹️ **Interrompu par l'utilisateur.** Le process de l'agent a été tué.";
                    if full_response.is_empty() {
                        full_response = footer.trim_start_matches('\n').to_string();
                    } else {
                        full_response.push_str(footer);
                    }
                }

                if full_response.is_empty() && !success {
                    tracing::error!(
                        "Agent {:?} exited with error ({}). stderr ({} lines): {}",
                        agent_type, exit_info, stderr_lines.len(),
                        // Truncate stderr by char count, not byte count.
                        // Agent stderr may contain UTF-8 (French error
                        // messages, emoji from npm, etc.) — `&s[..500]`
                        // would panic on a non-boundary byte.
                        if stderr_text.chars().count() > 500 {
                            stderr_text.chars().take(500).collect::<String>()
                        } else {
                            stderr_text.clone()
                        }
                    );
                    if stderr_text.is_empty() {
                        // No output at all — likely auth/session issue
                        full_response = format!(
                            "[Agent exited with error] ({})\n\n\
                            ⚠️ **No output captured.** Possible causes:\n\
                            - Expired session → run `/login` in the terminal\n\
                            - Invalid API key → check Config > Tokens\n\
                            - Agent not installed or not found",
                            exit_info
                        );
                    } else {
                        full_response = format!("[Agent exited with error] ({})\n\n{}", exit_info, stderr_text);
                    }
                }

                // Detect known error patterns (quota/usage-limit, auth, rate
                // limit, MCP…) and LEAD with the clean, actionable hint instead
                // of burying it under a wall of raw stderr. 2026-06-24: a Codex
                // quota error dumped 32 KB of echoed prompt + stderr, with the
                // real "you've hit your usage limit" signal lost at the bottom.
                // Now the hint is the headline; the raw output folds into a
                // collapsible "détails techniques" card (kronn:context marker,
                // rendered by MessageBody). No recognised hint → raw as before.
                if !success && !was_interrupted {
                    let all_output = format!("{}\n{}", full_response, stderr_text);
                    if let Some(hint) = detect_agent_error_hint(&all_output, &agent_type) {
                        let raw = full_response.trim();
                        full_response = if raw.is_empty() {
                            hint
                        } else {
                            format!(
                                "{hint}\n\n<!-- kronn:context title=\"détails techniques\" -->\n{raw}\n<!-- /kronn:context -->"
                            )
                        };
                    }
                }

                let tokens_used = if stream_json_tokens > 0 {
                    stream_json_tokens
                } else {
                    let (cleaned, count) = runner::parse_token_usage(&agent_type, &full_response, &stderr_lines);
                    if count > 0 {
                        full_response = cleaned;
                    }
                    count
                };

                // Hard cap before persistence — covers EVERY path (incl. the
                // error/kill stderr capture above, which bypasses the streaming
                // cap), so a multi-MB message can't reach the DB or crash the UI
                // renderer on open. See `cap_agent_response`.
                full_response = cap_agent_response(full_response, MAX_AGENT_RESPONSE_BYTES);

                // Save agent response to DB — always runs even if client is gone
                let tier_label = match disc_tier {
                    crate::models::ModelTier::Economy => Some("economy".to_string()),
                    crate::models::ModelTier::Reasoning => Some("reasoning".to_string()),
                    crate::models::ModelTier::Default => None, // Don't clutter with "default"
                };
                // Cost: use real cost from Claude Code if available, else estimate from pricing table
                let cost_usd = stream_json_cost.or_else(|| {
                    if tokens_used > 0 {
                        {
                            let at_str = serde_json::to_string(&agent_type).unwrap_or_default().trim_matches('"').to_string();
                            crate::core::pricing::estimate_cost(&at_str, tokens_used)
                        }
                    } else {
                        None
                    }
                });

                // 0.8.7 anti-hallucination P2 — lint the finalized reply:
                // niveau 0 heuristic + niveau 1 mechanical [src:] verification
                // against the project's host filesystem (the tree the agent
                // saw). Skipped when the mode is off ; non-blocking either way.
                // Computed BEFORE `full_response` is moved into the message.
                // Resolve citations against the tree the agent actually ran in
                // (Isolated worktree first, then the main checkout), keep the
                // report only when it has a signal, and emit telemetry. All of
                // that lives in the unit-tested `finalize_lint_report` helper.
                // 0.8.8 — also resolve citations against the project's declared
                // linked_repos (filesystem locations only), so an agent citing a
                // sibling repo (front_apollo, …) isn't flagged "couldn't verify".
                let linked_repo_paths: Vec<String> = if let Some(ref pid) = disc.project_id {
                    let pid = pid.clone();
                    state.db.with_conn(move |conn| {
                        let p = crate::db::projects::get_project(conn, &pid)?;
                        Ok(p.map(|p| p.linked_repos.into_iter()
                            .map(|lr| lr.location)
                            .filter(|loc| !loc.starts_with("http://") && !loc.starts_with("https://"))
                            .collect::<Vec<_>>())
                            .unwrap_or_default())
                    }).await.unwrap_or_default()
                } else {
                    Vec::new()
                };
                let lint_report = crate::core::anti_halluc::finalize_lint_report(
                    &full_response,
                    workspace_path.as_deref(),
                    &project_path,
                    &linked_repo_paths,
                );

                // Computed BEFORE `full_response` is moved into the message
                // below — reused by the batch-progress hook so an empty-but-
                // clean-exit child isn't mis-counted as a batch success.
                let child_run_was_success = child_run_counts_as_success(success, &full_response);

                let agent_msg = DiscussionMessage {
                    id: Uuid::new_v4().to_string(),
                    role: MessageRole::Agent,
                    content: full_response,
                    agent_type: Some(agent_type.clone()),
                    timestamp: Utc::now(),
                    tokens_used,
                    auth_mode: Some(auth_mode_str.clone()),
                    model_tier: tier_label,
                    cost_usd,
                    author_pseudo: None,
                    author_avatar_email: None,
                    source_msg_id: None,
                    // 0.8.5 — wallclock duration of THIS agent run. Captured
                    // from `run_started_at` (set at the very top of
                    // `make_agent_stream`) to now-commit. Used by the
                    // QP-metrics aggregator to compute avg first-reply
                    // duration per QP version.
                    duration_ms: Some(run_started_at.elapsed().as_millis() as u64),
                    lint_report,
                };

                let did = disc_id.clone();
                let msg = agent_msg.clone();
                if let Err(e) = state.db.with_conn(move |conn| {
                    crate::db::discussions::insert_message(conn, &did, &msg)
                }).await {
                    tracing::error!("Failed to save agent message: {e}");
                }
                // F1 — federate the native-runner reply to peers of a shared
                // disc. Previously ONLY MCP `disc_append` + UI `send_message`
                // federated, so a reply produced by Kronn's own runner was
                // invisible to the other instance. No-op for a local disc.
                crate::api::federation::federate_message(&state, &disc_id, &agent_msg).await;

                // 0.8.8 PR-B — enforce-mode P3 fail-fast (non-destructive). The
                // agent message above is kept (with its red pill); when it
                // carries a fabricated `[src:]` citation, append a System refusal
                // so the human arbitrates a correction. No auto-retry — on a user
                // disc the user decides. Inert outside enforce / when clean.
                let fabricated_count = agent_msg
                    .lint_report
                    .as_ref()
                    .map(|r| r.fabricated_count)
                    .unwrap_or(0);
                if crate::core::anti_halluc::enforce_refusal_needed(
                    crate::core::anti_halluc::current_mode(),
                    fabricated_count,
                ) {
                    let refusal = DiscussionMessage {
                        lint_report: None,
                        id: Uuid::new_v4().to_string(),
                        role: MessageRole::System,
                        content: crate::core::anti_halluc::enforce_refusal_message(fabricated_count),
                        agent_type: None,
                        timestamp: Utc::now(),
                        tokens_used: 0,
                        auth_mode: None,
                        model_tier: None,
                        cost_usd: None,
                        author_pseudo: None,
                        author_avatar_email: None,
                        source_msg_id: None,
                        duration_ms: None,
                    };
                    let did_ref = disc_id.clone();
                    let m = refusal.clone();
                    if let Err(e) = state.db.with_conn(move |conn| {
                        crate::db::discussions::insert_message(conn, &did_ref, &m)
                    }).await {
                        tracing::warn!("Failed to insert enforce refusal system message: {e}");
                    }
                    tracing::info!(
                        "enforce P3: disc {} agent reply has {} fabricated citation(s) — refusal surfaced",
                        disc_id, fabricated_count
                    );
                }

                // ── Slash-marker fallback (Vibe / Ollama) ──────────────
                // Agents that don't speak MCP can request introspection
                // by emitting `KRONN:DISC_*` lines in their reply. Scan
                // here, resolve each marker against the live disc, and
                // append one System message per marker so the agent
                // sees the answer on its next turn. Cf.
                // `slash_markers.rs` for the parser + resolver.
                //
                // Gated on the same agent set that *doesn't* get the
                // MCP notice in `disc_prompts.rs` — Vibe + Ollama +
                // Codex (the latter currently blocked by upstream
                // sandbox; see TD-20260510-codex-mcp-sandbox-block).
                // For other agents we still scan (cheap regex) but
                // only respect markers if the agent actually emitted
                // one — defensive, no behaviour change for them.
                let markers = super::slash_markers::parse_markers(&agent_msg.content);
                if !markers.is_empty() {
                    let resolutions = super::slash_markers::resolve_markers(&state, &disc_id, &markers).await;
                    for body in resolutions {
                        let sys_msg = DiscussionMessage {
                            lint_report: None,
                            id: Uuid::new_v4().to_string(),
                            role: MessageRole::System,
                            content: body,
                            agent_type: None,
                            timestamp: Utc::now(),
                            tokens_used: 0,
                            auth_mode: None,
                            model_tier: None,
                            cost_usd: None,
                            author_pseudo: None,
                            author_avatar_email: None, source_msg_id: None, duration_ms: None,
                        };
                        let did_sys = disc_id.clone();
                        let m = sys_msg.clone();
                        if let Err(e) = state.db.with_conn(move |conn| {
                            crate::db::discussions::insert_message(conn, &did_sys, &m)
                        }).await {
                            tracing::warn!("Failed to insert slash-marker system message: {e}");
                        }
                    }
                    tracing::info!(
                        "Resolved {} slash-marker(s) for disc {}",
                        markers.len(), disc_id
                    );
                }

                // ── kronn-internal MCP tool-call trace ─────────────────
                // For stream-JSON agents (Claude Code et al), persist
                // each `mcp__kronn-internal__*` call captured during
                // the stream as a System message. Same shape as the
                // slash-marker fallback so MessageBubble can render
                // both with the same `[kronn-internal: …]` badge.
                // Result is NOT included — for MCP agents the agent's
                // own reply already quotes/uses it. We only need the
                // call trace so the user can see "the agent looked at
                // message #4" in the transcript.
                if !kronn_tool_calls.is_empty() {
                    for body in kronn_tool_calls.iter() {
                        let sys_msg = DiscussionMessage {
                            lint_report: None,
                            id: Uuid::new_v4().to_string(),
                            role: MessageRole::System,
                            content: body.clone(),
                            agent_type: None,
                            timestamp: Utc::now(),
                            tokens_used: 0,
                            auth_mode: None,
                            model_tier: None,
                            cost_usd: None,
                            author_pseudo: None,
                            author_avatar_email: None, source_msg_id: None, duration_ms: None,
                        };
                        let did_sys = disc_id.clone();
                        let m = sys_msg.clone();
                        if let Err(e) = state.db.with_conn(move |conn| {
                            crate::db::discussions::insert_message(conn, &did_sys, &m)
                        }).await {
                            tracing::warn!("Failed to insert kronn-internal tool-call system message: {e}");
                        }
                    }
                    tracing::info!(
                        "Persisted {} kronn-internal MCP tool-call(s) for disc {}",
                        kronn_tool_calls.len(), disc_id
                    );
                }

                // 0.8.6 phase 4 — also persist native tool calls (Claude
                // Code's Read/Bash/Edit, third-party MCP servers). Same
                // shape as kronn-internal but with `[agent-native: …]`
                // prefix so the frontend banner can split them out.
                // Limits the audit-trail gap user flagged 2026-05-22 :
                // live tool log disappears on stream end, leaving no
                // post-hoc trace for debug.
                if !native_tool_calls.is_empty() {
                    for body in native_tool_calls.iter() {
                        let sys_msg = DiscussionMessage {
                            lint_report: None,
                            id: Uuid::new_v4().to_string(),
                            role: MessageRole::System,
                            content: body.clone(),
                            agent_type: None,
                            timestamp: Utc::now(),
                            tokens_used: 0,
                            auth_mode: None,
                            model_tier: None,
                            cost_usd: None,
                            author_pseudo: None,
                            author_avatar_email: None, source_msg_id: None, duration_ms: None,
                        };
                        let did_sys = disc_id.clone();
                        let m = sys_msg.clone();
                        if let Err(e) = state.db.with_conn(move |conn| {
                            crate::db::discussions::insert_message(conn, &did_sys, &m)
                        }).await {
                            tracing::warn!("Failed to insert agent-native tool-call system message: {e}");
                        }
                    }
                    tracing::info!(
                        "Persisted {} agent-native tool-call(s) for disc {}",
                        native_tool_calls.len(), disc_id
                    );
                }

                // Clear the in-flight checkpoint — the final message is now in
                // `messages`, so partial_response would be redundant + would
                // double up at the next backend boot if we left it dangling.
                let did_clear = disc_id.clone();
                let _ = state.db.with_conn(move |conn| {
                    crate::db::discussions::set_partial_response(conn, &did_clear, None)
                }).await;

                // ── 0.8.4 (#329 / F9) Auto-archive on validation complete ──
                //
                // When a validation disc emits `KRONN:VALIDATION_COMPLETE`,
                // its job is over: the agent has reviewed the audit, the TD
                // status updates landed, the project flips to `Validated`.
                // Pre-fix the disc stayed visible in the sidebar forever,
                // accumulating one new disc per audit run (Marc-persona
                // discovery during the 0.8.4 Playwright pass: 3 stale
                // "Validation audit AI" discs after a Full + 2 sub-audits).
                //
                // Archiving silently lifts the noise — the disc is still
                // reachable via the Archives toggle if the user wants to
                // re-read the conversation, but it stops cluttering the
                // active list.
                //
                // Bootstrap + briefing discs follow the same lifecycle and
                // are handled here too (they ship the *_COMPLETE family).
                if let Some(sig) = stopped_on_signal {
                    if super::signal_should_auto_archive(sig) {
                        let did_archive = disc_id.clone();
                        let archived = state.db.with_conn(move |conn| {
                            crate::db::discussions::update_discussion(
                                conn, &did_archive, None, Some(true), None, None,
                            )
                        }).await;
                        match archived {
                            Ok(true) => tracing::info!(
                                "Auto-archived discussion {} after terminal signal {}",
                                disc_id, sig,
                            ),
                            Ok(false) => tracing::warn!(
                                "Auto-archive of disc {} returned no-op (disc deleted?)",
                                disc_id,
                            ),
                            Err(e) => tracing::warn!(
                                "Auto-archive failed for disc {} on {}: {}",
                                disc_id, sig, e,
                            ),
                        }
                    }
                }

                // ── Batch progress hook ────────────────────────────────
                // If this disc was spawned by a batch workflow run, bump
                // its counters. Broadcast a progress or finished event so
                // the sidebar pill + any open batch monitor updates live.
                if let Some(ref run_id) = batch_run_id {
                    let run_id_inner = run_id.clone();
                    // Empty-but-clean-exit children are NOT successes (Codex
                    // silent-exit bug). Computed above, before `full_response`
                    // was moved into the persisted message.
                    let child_succeeded = child_run_was_success;
                    let ws_tx = state.ws_broadcast.clone();
                    let batch_updated = state.db.with_conn(move |conn| {
                        crate::db::workflows::increment_batch_progress(conn, &run_id_inner, child_succeeded)
                    }).await;
                    match batch_updated {
                        Ok(Some(updated_run)) => {
                            let is_final = matches!(updated_run.status, RunStatus::Success | RunStatus::Failed);
                            let event = if is_final {
                                WsMessage::BatchRunFinished {
                                    run_id: updated_run.id.clone(),
                                    discussion_id: disc_id.clone(),
                                    batch_name: updated_run.batch_name.clone(),
                                    batch_total: updated_run.batch_total,
                                    batch_completed: updated_run.batch_completed,
                                    batch_failed: updated_run.batch_failed,
                                }
                            } else {
                                WsMessage::BatchRunProgress {
                                    run_id: updated_run.id.clone(),
                                    discussion_id: disc_id.clone(),
                                    batch_total: updated_run.batch_total,
                                    batch_completed: updated_run.batch_completed,
                                    batch_failed: updated_run.batch_failed,
                                }
                            };
                            let _ = ws_tx.send(event);
                            if is_final {
                                tracing::info!(
                                    "Batch run {} finished: {}/{} ok, {} failed",
                                    updated_run.id, updated_run.batch_completed,
                                    updated_run.batch_total, updated_run.batch_failed
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(e) => tracing::error!("Failed to update batch progress: {e}"),
                    }
                }

                // Detect KRONN:BRIEFING_COMPLETE marker
                if success && agent_msg.content.to_uppercase().contains("KRONN:BRIEFING_COMPLETE") {
                    if let Some(ref pid) = disc_project_id {
                        let briefing_project_id = pid.clone();
                        let briefing_project_path = project_path.clone();
                        let briefing_state = state.clone();
                        tokio::spawn(async move {
                            // Read briefing.md from the project's docs folder.
                            // Path-agnostic — works on docs/ post-pivot AND legacy ai/.
                            let resolved = crate::core::scanner::resolve_host_path(&briefing_project_path);
                            let briefing_file = crate::core::scanner::detect_docs_dir(&resolved).join("briefing.md");
                            let notes = tokio::task::spawn_blocking(move || {
                                std::fs::read_to_string(&briefing_file).ok()
                            }).await.unwrap_or(None);

                            if let Some(content) = notes {
                                let pid = briefing_project_id.clone();
                                if let Err(e) = briefing_state.db.with_conn(move |conn| {
                                    crate::db::projects::update_project_briefing_notes(conn, &pid, Some(&content))
                                }).await {
                                    tracing::error!("Failed to save briefing notes for project {}: {e}", briefing_project_id);
                                } else {
                                    tracing::info!("Briefing notes saved for project {}", briefing_project_id);
                                }
                            } else {
                                tracing::warn!("BRIEFING_COMPLETE detected but ai/briefing.md not found for project {}", briefing_project_id);
                            }
                        });
                    }
                }

                // Trigger background summary generation if conversation is long enough
                if success {
                    let summary_state = state.clone();
                    let summary_disc_id = disc_id.clone();
                    let summary_agent_type = agent_type.clone();
                    let summary_tokens = tokens.clone();
                    tokio::spawn(async move {
                        super::orchestration::maybe_generate_summary(
                            &summary_state, &summary_disc_id,
                            &summary_agent_type, &summary_tokens,
                        ).await;
                    });
                }

                let done = serde_json::json!({ "message_id": agent_msg.id, "success": success, "tokens_used": tokens_used });
                let _ = tx.send(AgentStreamEvent::Done { data: done }).await;
            }
            Err(e) => {
                tracing::error!("Agent start failed: {}", e);

                let err_msg = DiscussionMessage {
                    lint_report: None,
                    id: Uuid::new_v4().to_string(),
                    role: MessageRole::System,
                    content: format!("Erreur: {}", e),
                    agent_type: None,
                    timestamp: Utc::now(),
                    tokens_used: 0,
                    auth_mode: None,
                    model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None, source_msg_id: None, duration_ms: None,
                };

                let did = disc_id.clone();
                let err_msg_fed = err_msg.clone();
                if let Err(db_err) = state.db.with_conn(move |conn| {
                    crate::db::discussions::insert_message(conn, &did, &err_msg)
                }).await {
                    tracing::error!("Failed to save agent error message: {db_err}");
                }
                // F1 — let the peer see the turn failed instead of silence.
                crate::api::federation::federate_message(&state, &disc_id, &err_msg_fed).await;

                let err = serde_json::json!({ "error": e });
                let _ = tx.send(AgentStreamEvent::Error { data: err }).await;
            }
        }
    });

    // Thin SSE reader — just maps channel events to SSE
    let stream: SseStream = Box::pin(async_stream::try_stream! {
        while let Some(evt) = rx.recv().await {
            match evt {
                AgentStreamEvent::Start => {
                    yield Event::default().event("start").data("{}");
                }
                AgentStreamEvent::Meta { auth_mode } => {
                    yield Event::default().event("meta").data(
                        serde_json::json!({ "auth_mode": auth_mode }).to_string()
                    );
                }
                AgentStreamEvent::Chunk { data } => {
                    yield Event::default().event("chunk").data(data.to_string());
                }
                AgentStreamEvent::Done { data } => {
                    yield Event::default().event("done").data(data.to_string());
                }
                AgentStreamEvent::Log { text } => {
                    yield Event::default().event("log").data(
                        serde_json::json!({ "text": text }).to_string()
                    );
                }
                AgentStreamEvent::Error { data } => {
                    yield Event::default().event("error").data(data.to_string());
                }
                _ => {}
            }
        }
    });

    Sse::new(crate::core::sse_limits::bounded(stream))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Orchestration helpers — extracted from orchestrate() to reduce duplication
// ═══════════════════════════════════════════════════════════════════════════════

/// Metadata for SSE chunk events emitted during agent streaming.
pub(super) struct AgentStreamMeta {
    pub(super) agent_name: String,
    pub(super) agent_type: AgentType,
    pub(super) round_label: serde_json::Value,
}

/// Result of running a single agent to completion.
pub(super) struct AgentRunResult {
    pub(super) response: String,
    pub(super) tokens_used: u64,
}

/// Run an agent process to completion, streaming output via tx.
/// Handles stream-json and plain text modes, tool logging, error detection, and token parsing.
/// Does NOT save to DB — caller handles that (format differs per call site).
pub(super) async fn run_agent_streaming(
    mut process: impl runner::AgentIo,
    tx: &tokio::sync::mpsc::Sender<AgentStreamEvent>,
    meta: &AgentStreamMeta,
    agent_type: &AgentType,
) -> AgentRunResult {
    let mut full_response = String::new();
    let mut stream_tokens: u64 = 0;
    let mut current_tool: Option<String> = None;
    let mut tool_input = String::new();
    let is_stream_json = process.output_mode() == runner::OutputMode::StreamJson;
    let deadline = tokio::time::Instant::now() + AGENT_GLOBAL_TIMEOUT;

    let mut signal_stop = false;
    // Shared decoder-loop detector (`is_decoder_loop`, module top). Orchestration
    // runs use the same Claude model and can exhibit the same failure mode; we
    // break out and return whatever text arrived before the loop started.
    let mut last_text_delta = String::new();
    let mut repeat_delta_count: u32 = 0;
    loop {
        tokio::select! {
            line = process.next_line() => {
                match line {
                    Some(line) => {
                        if is_stream_json {
                            match runner::parse_claude_stream_line(&line) {
                                runner::StreamJsonEvent::Text(text) => {
                                    // Decoder-loop guard — shared detector.
                                    if is_decoder_loop(&text, &mut last_text_delta, &mut repeat_delta_count) {
                                        tracing::warn!(
                                            "Orchestration agent entered a decoder loop — delta {:?} repeated {} times, aborting",
                                            text.chars().take(40).collect::<String>(),
                                            repeat_delta_count,
                                        );
                                        process.kill().await;
                                        full_response.push_str("\n\n---\n🔁 **Decoder loop detected** — agent killed.");
                                        break;
                                    }
                                    full_response.push_str(&text);
                                    if !tx.is_closed() {
                                        let chunk = serde_json::json!({
                                            "text": text, "agent": meta.agent_name,
                                            "agent_type": meta.agent_type, "round": meta.round_label,
                                        });
                                        let _ = tx.send(AgentStreamEvent::Chunk { data: chunk }).await;
                                    }
                                }
                                runner::StreamJsonEvent::Usage { input_tokens, output_tokens, .. } => {
                                    stream_tokens = stream_tokens.max(input_tokens + output_tokens);
                                }
                                runner::StreamJsonEvent::ToolStart(name) => {
                                    current_tool = Some(name);
                                    tool_input.clear();
                                }
                                runner::StreamJsonEvent::ToolInputDelta(partial) => {
                                    tool_input.push_str(&partial);
                                }
                                runner::StreamJsonEvent::ToolEnd => {
                                    if let Some(ref tool) = current_tool {
                                        if !tx.is_closed() {
                                            let _ = tx.send(AgentStreamEvent::Log {
                                                text: crate::api::disc_git::format_tool_log(tool, &tool_input),
                                            }).await;
                                        }
                                    }
                                    current_tool = None;
                                    tool_input.clear();
                                }
                                runner::StreamJsonEvent::Skip => {}
                            }
                        } else {
                            let nl = if full_response.is_empty() { "" } else { "\n" };
                            full_response.push_str(&format!("{}{}", nl, line));
                            if !tx.is_closed() {
                                let chunk = serde_json::json!({
                                    "text": format!("{}{}", nl, line), "agent": meta.agent_name,
                                    "agent_type": meta.agent_type, "round": meta.round_label,
                                });
                                let _ = tx.send(AgentStreamEvent::Chunk { data: chunk }).await;
                            }
                        }
                        // Same terminal-signal enforcement as the regular run loop:
                        // an orchestrated agent that emits e.g. KRONN:ARCHITECTURE_READY
                        // should hand back to the user, not keep streaming.
                        if let Some(sig) = detect_terminal_signal(&full_response) {
                            tracing::info!("Terminal signal {} detected (orchestration) — stopping agent", sig);
                            full_response = truncate_after_signal(&full_response, sig);
                            signal_stop = true;
                            break;
                        }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                tracing::warn!("Agent {:?} timed out (round: {})", agent_type, meta.round_label);
                process.kill().await;
                break;
            }
        }
    }
    if signal_stop {
        process.kill().await;
    }

    let status = process.wait().await;
    process.fix_ownership();
    let success = status.map(|s| s.success).unwrap_or(false);
    let stderr = process.captured_stderr_flushed().await;
    let stderr_text = stderr.join("\n");

    if full_response.is_empty() && !success {
        let exit_info = match &status {
            Some(s) => format!("exit code: {:?}", s.code),
            None => "exit status unavailable".to_string(),
        };
        tracing::error!("Agent {:?} exited with error ({}). stderr: {}",
            agent_type, exit_info,
            // Char-count truncation — see twin site above for rationale.
            if stderr_text.chars().count() > 500 {
                stderr_text.chars().take(500).collect::<String>()
            } else {
                stderr_text.clone()
            });
        full_response = if stderr_text.is_empty() {
            format!("[Agent exited with error] ({})", exit_info)
        } else {
            format!("[Agent exited with error] ({})\n\n{}", exit_info, stderr_text)
        };
    } else if full_response.is_empty() {
        full_response = "[No response]".to_string();
    }

    if !success {
        let all_output = format!("{}\n{}", full_response, stderr_text);
        if let Some(hint) = detect_agent_error_hint(&all_output, agent_type) {
            full_response.push_str(&format!("\n\n{}", hint));
        }
    }

    let tokens_used = if stream_tokens > 0 {
        stream_tokens
    } else {
        let (cleaned, count) = runner::parse_token_usage(agent_type, &full_response, &stderr);
        if count > 0 { full_response = cleaned; }
        count
    };

    AgentRunResult { response: full_response, tokens_used }
}

/// Run an agent silently (no SSE streaming), return collected text.
/// Used for conversation summarization before debate.
///
/// Generic over [`runner::AgentIo`] (0.8.8 test-seam refactor) so the
/// accumulation + stream-json-vs-raw + teardown logic is unit-testable with
/// a `ScriptedProcess`, without spawning a real CLI. Production passes a real
/// `AgentProcess`; both impl `AgentIo`.
pub(super) async fn run_agent_collect(mut process: impl runner::AgentIo) -> String {
    let mut output = String::new();
    let is_json = process.output_mode() == runner::OutputMode::StreamJson;
    let deadline = tokio::time::Instant::now() + AGENT_GLOBAL_TIMEOUT;
    loop {
        tokio::select! {
            line = process.next_line() => {
                match line {
                    Some(l) => {
                        if is_json {
                            if let runner::StreamJsonEvent::Text(text) = runner::parse_claude_stream_line(&l) {
                                output.push_str(&text);
                            }
                        } else {
                            if !output.is_empty() { output.push('\n'); }
                            output.push_str(&l);
                        }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                tracing::warn!("Agent timed out during silent collection");
                process.kill().await;
                break;
            }
        }
    }
    let _ = process.wait().await;
    output.trim().to_string()
}


/// Render `kronn-internal` tool args as a compact human-readable
/// string for the System-message badge in the disc transcript. The
/// goal is "the user understands at a glance what the agent asked":
///
///   disc_meta             → `disc_meta()`         (no args)
///   disc_get_message(4)   → `disc_get_message(4)` (idx)
///   disc_summarize(0,10)  → `disc_summarize(0..10)` (range)
///
/// Falls through to the raw JSON when the shape is unfamiliar — better
/// to render `{"foo":"bar"}` than to drop the call from the trace.
/// 0.8.6 phase 4 — truncate raw tool args for the `[agent-native: ...]`
/// trace. Some native tools (`Edit`, `Write`) carry large file contents
/// as args — persisting them verbatim would blow up the disc transcript
/// and the banner would be unusable. We keep the start of the JSON, cut
/// on a char boundary (defensive for French / emoji / multi-byte file
/// paths), and append `…` to signal the truncation.
///
/// Single-line collapse : agent stream-JSON sometimes serialises multi-
/// line code blocks with literal `\n` ; we replace those with a space
/// so the persisted trace stays one-line-per-call.
fn truncate_tool_args(raw: &str, max_chars: usize) -> String {
    let collapsed = raw.replace('\n', " ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let mut out: String = collapsed.chars().take(max_chars).collect();
    out.push('…');
    out
}

fn pretty_kronn_args(tool_name: &str, raw_json: &str) -> String {
    let val: serde_json::Value = match serde_json::from_str(raw_json) {
        Ok(v) => v,
        // No JSON yet (rare — empty input deltas) → blank args.
        Err(_) => return String::new(),
    };
    match tool_name {
        "disc_meta" => String::new(),
        "disc_get_message" => val.get("idx")
            .map(|i| i.to_string())
            .unwrap_or_default(),
        "disc_summarize" => {
            let from = val.get("from").and_then(|v| v.as_i64());
            let to = val.get("to").and_then(|v| v.as_i64());
            let force = val.get("force_refresh").and_then(|v| v.as_bool()).unwrap_or(false);
            match (from, to) {
                (Some(f), Some(t)) if force => format!("{}..{}, refresh", f, t),
                (Some(f), Some(t)) => format!("{}..{}", f, t),
                _ => raw_json.to_string(),
            }
        }
        // Unknown tool under the kronn-internal namespace — surface the
        // raw JSON so the user can still tell what was asked.
        _ => raw_json.to_string(),
    }
}

#[cfg(test)]
mod pretty_kronn_args_tests {
    use super::pretty_kronn_args;

    #[test]
    fn meta_renders_no_args() {
        assert_eq!(pretty_kronn_args("disc_meta", "{}"), "");
    }

    #[test]
    fn get_message_extracts_idx() {
        assert_eq!(pretty_kronn_args("disc_get_message", r#"{"idx":4}"#), "4");
        assert_eq!(pretty_kronn_args("disc_get_message", r#"{"idx":-1}"#), "-1");
    }

    #[test]
    fn summarize_renders_range() {
        assert_eq!(
            pretty_kronn_args("disc_summarize", r#"{"from":0,"to":10}"#),
            "0..10",
        );
    }

    #[test]
    fn summarize_with_refresh_appends_flag() {
        assert_eq!(
            pretty_kronn_args("disc_summarize", r#"{"from":0,"to":5,"force_refresh":true}"#),
            "0..5, refresh",
        );
    }

    #[test]
    fn unknown_tool_falls_back_to_raw_json() {
        let out = pretty_kronn_args("disc_future_tool", r#"{"weird":1}"#);
        assert_eq!(out, r#"{"weird":1}"#);
    }

    #[test]
    fn malformed_json_yields_blank_args() {
        // Corruption / empty deltas → blank rather than panic; the
        // System message still says `[kronn-internal: tool()]` which
        // tells the user the call happened even if we can't show args.
        assert_eq!(pretty_kronn_args("disc_get_message", "not-json"), "");
    }
}

#[cfg(test)]
mod agent_lifecycle_tests {
    use super::{effective_stall_timeout, child_run_counts_as_success, cap_agent_response, AGENT_GLOBAL_TIMEOUT, NON_STREAMING_STALL_TIMEOUT};
    use std::time::Duration;

    // ── #1 — stall watchdog must not apply to non-streaming agents ──
    // (2026-06-23: Codex `exec` is silent on stdout until the very end; the
    // no-chunk stall killed slow-but-healthy runs → empty discussions.)

    #[test]
    fn streaming_agent_keeps_configured_stall() {
        let configured = Duration::from_secs(5 * 60);
        assert_eq!(
            effective_stall_timeout(true, configured, AGENT_GLOBAL_TIMEOUT),
            configured,
            "Claude (stream-json) must KEEP its short stall — don't regress streaming",
        );
    }

    #[test]
    fn non_streaming_agent_uses_bounded_stall_not_global() {
        let configured = Duration::from_secs(5 * 60);
        // Non-streaming agents bypass the SHORT streaming stall but get a
        // BOUNDED ceiling (not the full 30-min global) so a hung run frees its
        // concurrency slot in reasonable time — the 2026-06-24 clog fix.
        assert_eq!(
            effective_stall_timeout(false, configured, NON_STREAMING_STALL_TIMEOUT),
            NON_STREAMING_STALL_TIMEOUT,
            "Codex/Text agents use the bounded non-streaming stall",
        );
        assert!(NON_STREAMING_STALL_TIMEOUT > configured,
            "must outlast the short streaming stall (else slow non-streamers die early)");
        assert!(NON_STREAMING_STALL_TIMEOUT < AGENT_GLOBAL_TIMEOUT,
            "must be SHORTER than the global, else a hung run squats its slot too long");
    }

    // ── #2 — empty-but-clean-exit child is NOT a batch success ──
    // (made a batch workflow report green Success over 16 empty discs.)

    #[test]
    fn clean_exit_with_real_reply_is_success() {
        assert!(child_run_counts_as_success(true, "Triage:\n- clear: EW-1 ready to frame"));
    }

    #[test]
    fn clean_exit_with_blank_reply_is_not_success() {
        assert!(!child_run_counts_as_success(true, ""), "empty reply ≠ success");
        assert!(!child_run_counts_as_success(true, "   \n\t  "), "whitespace-only ≠ success");
    }

    #[test]
    fn failed_exit_is_never_success_even_with_partial_text() {
        assert!(!child_run_counts_as_success(false, "partial output before crash"));
    }

    // ── cap_agent_response — the source fix: no multi-MB message reaches
    // the DB / UI, even on the error/kill stderr-capture path. ──

    #[test]
    fn small_response_is_left_untouched() {
        let s = "a normal reply".to_string();
        assert_eq!(cap_agent_response(s.clone(), 2_000_000), s);
    }

    #[test]
    fn oversized_response_is_capped_with_marker() {
        let huge = "x".repeat(3_000_000); // ~2.4 MB Codex dump shape
        let out = cap_agent_response(huge, 2_000_000);
        assert!(out.len() <= 2_000_000 + 80, "must be bounded near the limit, got {}", out.len());
        assert!(out.contains("tronqué"), "must signal truncation");
    }

    #[test]
    fn cap_is_char_boundary_safe_on_utf8() {
        // 'é' is 2 bytes — a cut landing mid-char would panic without the
        // is_char_boundary guard (French stderr / emoji are common).
        let s = "é".repeat(1000); // 2000 bytes
        let out = cap_agent_response(s, 1001); // 1001 lands mid-'é'
        // No panic + still valid UTF-8 (String guarantees it if no panic).
        assert!(out.contains("tronqué"));
        assert!(out.len() <= 1001 + 80);
    }
}

#[cfg(test)]
mod truncate_tool_args_tests {
    use super::truncate_tool_args;

    #[test]
    fn short_input_passes_through_unchanged() {
        assert_eq!(truncate_tool_args("hello", 120), "hello");
        assert_eq!(truncate_tool_args(r#"{"file":"a.rs"}"#, 120), r#"{"file":"a.rs"}"#);
    }

    #[test]
    fn long_input_truncates_with_ellipsis() {
        let raw = "x".repeat(200);
        let out = truncate_tool_args(&raw, 50);
        // 50 chars + 1 ellipsis = 51
        assert_eq!(out.chars().count(), 51);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn collapses_newlines_to_spaces() {
        // Native tools like `Edit` or `Write` carry multi-line content
        // in their JSON args. We persist as a one-liner so the disc
        // transcript stays readable.
        let raw = "line1\nline2\nline3";
        assert_eq!(truncate_tool_args(raw, 120), "line1 line2 line3");
    }

    #[test]
    fn char_boundary_safe_with_multibyte() {
        // French accents + emoji are multi-byte ; naive [..N] slicing
        // would panic. .chars().take() is boundary-safe by definition.
        let raw = "écoute 🦀 ".repeat(30);
        let out = truncate_tool_args(&raw, 20);
        assert_eq!(out.chars().count(), 21);  // 20 + ellipsis
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(truncate_tool_args("", 120), "");
    }

    #[test]
    fn input_exactly_at_limit_not_truncated() {
        // Boundary case : input length == max chars → no ellipsis.
        let raw = "x".repeat(50);
        assert_eq!(truncate_tool_args(&raw, 50), "x".repeat(50));
    }
}

#[cfg(test)]
mod run_agent_collect_tests {
    //! Unit tests for the silent-collection loop, driven by a scripted
    //! `AgentIo` (no real subprocess). Pins the raw-vs-stream-json branch,
    //! line accumulation, trimming, and empty-stream handling — the logic
    //! that was previously untestable because it required spawning a CLI.
    use super::run_agent_collect;
    use crate::agents::runner::ScriptedProcess;

    /// Helper: a claude `--output-format stream-json` text-delta line.
    fn text_delta(s: &str) -> String {
        format!(
            r#"{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":{}}}}}}}"#,
            serde_json::to_string(s).unwrap()
        )
    }

    #[tokio::test]
    async fn raw_mode_joins_lines_with_newline_and_trims() {
        let proc = ScriptedProcess::raw(["  first", "second", "third  "]);
        let out = run_agent_collect(proc).await;
        assert_eq!(out, "first\nsecond\nthird");
    }

    #[tokio::test]
    async fn empty_stream_yields_empty_string() {
        let proc = ScriptedProcess::raw(Vec::<String>::new());
        let out = run_agent_collect(proc).await;
        assert_eq!(out, "");
    }

    #[tokio::test]
    async fn stream_json_accumulates_only_text_events() {
        // Mix text deltas with a tool-use line + a non-text event ; only the
        // text must survive into the collected summary.
        let proc = ScriptedProcess::stream_json([
            text_delta("Hello "),
            // A tool-start / non-text event the parser classifies as non-Text:
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"x","name":"Read","input":{}}}}"#.to_string(),
            text_delta("world"),
        ]);
        let out = run_agent_collect(proc).await;
        assert_eq!(out, "Hello world");
    }

    #[tokio::test]
    async fn stream_json_non_json_falls_back_to_raw_text() {
        // CONTRACT (parse_claude_stream_line, runner.rs): in stream-json mode
        // a NON-JSON line is passed through as raw Text — a deliberate
        // "never silently lose agent output" choice. A valid JSON object with
        // no recognized `type` (e.g. `{}`) IS skipped. This test pins both so
        // the fallback isn't accidentally "fixed" into dropping real output.
        let proc = ScriptedProcess::stream_json([
            "plain log noise".to_string(), // non-JSON → kept as text
            text_delta("real"),            // text_delta → kept
            "{}".to_string(),              // typeless JSON → skipped
        ]);
        let out = run_agent_collect(proc).await;
        assert_eq!(out, "plain log noisereal");
    }

    #[tokio::test]
    async fn raw_mode_single_line_no_leading_newline() {
        let proc = ScriptedProcess::raw(["only"]);
        assert_eq!(run_agent_collect(proc).await, "only");
    }
}

#[cfg(test)]
mod run_agent_streaming_tests {
    //! Unit tests for the SSE-producing agent loop, driven by a scripted
    //! `AgentIo`. These pin the bug-prone paths the 2026-05-28 QA audit
    //! flagged as untested : tool-call event → Log emission, terminal-signal
    //! truncation, decoder-loop abort, and the error-exit message — all
    //! without spawning a CLI or burning tokens.
    use super::{run_agent_streaming, AgentStreamMeta};
    use crate::api::discussions::AgentStreamEvent;
    use crate::agents::runner::ScriptedProcess;
    use crate::models::AgentType;

    fn text_delta(s: &str) -> String {
        format!(
            r#"{{"type":"stream_event","event":{{"type":"content_block_delta","delta":{{"type":"text_delta","text":{}}}}}}}"#,
            serde_json::to_string(s).unwrap()
        )
    }
    fn tool_start(name: &str) -> String {
        format!(
            r#"{{"type":"stream_event","event":{{"type":"content_block_start","content_block":{{"type":"tool_use","name":"{}"}}}}}}"#,
            name
        )
    }
    fn tool_input(partial: &str) -> String {
        format!(
            r#"{{"type":"stream_event","event":{{"delta":{{"type":"input_json_delta","partial_json":{}}}}}}}"#,
            serde_json::to_string(partial).unwrap()
        )
    }
    fn tool_end() -> String {
        r#"{"type":"stream_event","event":{"type":"content_block_stop"}}"#.to_string()
    }

    fn meta() -> AgentStreamMeta {
        AgentStreamMeta {
            agent_name: "TestAgent".into(),
            agent_type: AgentType::ClaudeCode,
            round_label: serde_json::json!("round-1"),
        }
    }

    /// Drain a finished channel into a Vec for assertions.
    fn drain(mut rx: tokio::sync::mpsc::Receiver<AgentStreamEvent>) -> Vec<AgentStreamEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    #[tokio::test]
    async fn raw_accumulates_and_sends_chunks() {
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        let proc = ScriptedProcess::raw(["line one", "line two"]);
        let res = run_agent_streaming(proc, &tx, &meta(), &AgentType::ClaudeCode).await;
        drop(tx);
        assert_eq!(res.response, "line one\nline two");
        let chunks = drain(rx).into_iter()
            .filter(|e| matches!(e, AgentStreamEvent::Chunk { .. }))
            .count();
        assert_eq!(chunks, 2, "one Chunk per raw line");
    }

    #[tokio::test]
    async fn stream_json_text_accumulates() {
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        let proc = ScriptedProcess::stream_json([text_delta("Hello "), text_delta("world")]);
        let res = run_agent_streaming(proc, &tx, &meta(), &AgentType::ClaudeCode).await;
        drop(tx);
        assert_eq!(res.response, "Hello world");
        assert!(drain(rx).iter().any(|e| matches!(e, AgentStreamEvent::Chunk { .. })));
    }

    #[tokio::test]
    async fn tool_call_emits_a_log_event() {
        // ToolStart → ToolInputDelta → ToolEnd must produce exactly one Log
        // event (the human-readable tool-call breadcrumb), not pollute the
        // response text.
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        let proc = ScriptedProcess::stream_json([
            text_delta("Reading file. "),
            tool_start("Read"),
            tool_input("{\"path\":\"src/lib.rs\"}"),
            tool_end(),
            text_delta("Done."),
        ]);
        let res = run_agent_streaming(proc, &tx, &meta(), &AgentType::ClaudeCode).await;
        drop(tx);
        // Tool JSON must NOT leak into the prose response.
        assert_eq!(res.response, "Reading file. Done.");
        let logs: Vec<_> = drain(rx).into_iter()
            .filter(|e| matches!(e, AgentStreamEvent::Log { .. }))
            .collect();
        assert_eq!(logs.len(), 1, "exactly one Log event for the Read tool call");
        if let AgentStreamEvent::Log { text } = &logs[0] {
            assert!(text.contains("Read"), "log should name the tool: {text}");
        }
    }

    #[tokio::test]
    async fn terminal_signal_stops_and_truncates() {
        // A KRONN:* terminal marker mid-stream must stop the loop and
        // truncate everything after the signal — the agent hands back to
        // the user instead of streaming on.
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        let proc = ScriptedProcess::raw([
            "Architecture proposed.",
            "KRONN:ARCHITECTURE_READY",
            "this trailing line must never be reached",
        ]);
        let res = run_agent_streaming(proc, &tx, &meta(), &AgentType::ClaudeCode).await;
        drop(tx);
        assert!(res.response.contains("Architecture proposed."));
        assert!(
            !res.response.contains("trailing line must never be reached"),
            "content after the terminal signal must be truncated: {:?}", res.response
        );
        let _ = drain(rx);
    }

    #[tokio::test]
    async fn decoder_loop_is_detected_and_aborted() {
        // The same text delta repeated past MAX_REPEAT_DELTAS (50) is the
        // extended-thinking decoder-loop failure (EW-7189). The loop must
        // kill the agent and append a marker rather than stream forever.
        let mut lines = Vec::new();
        for _ in 0..60 {
            lines.push(text_delta("RepeatedChunk")); // ≥3 chars, non-empty
        }
        let (tx, rx) = tokio::sync::mpsc::channel(500);
        let proc = ScriptedProcess::stream_json(lines);
        let res = run_agent_streaming(proc, &tx, &meta(), &AgentType::ClaudeCode).await;
        drop(tx);
        assert!(
            res.response.contains("Decoder loop detected"),
            "expected decoder-loop abort marker, got: {:?}",
            res.response.chars().rev().take(80).collect::<String>()
        );
        let _ = drain(rx);
    }

    #[tokio::test]
    async fn empty_response_with_failed_exit_formats_error() {
        // No output + non-zero exit → the "[Agent exited with error]" message
        // so the user sees a diagnostic instead of a blank reply.
        let (tx, rx) = tokio::sync::mpsc::channel(10);
        let proc = ScriptedProcess::stream_json(Vec::<String>::new())
            .with_exit(false, Some(1))
            .with_stderr(["boom: something failed"]);
        let res = run_agent_streaming(proc, &tx, &meta(), &AgentType::ClaudeCode).await;
        drop(tx);
        assert!(res.response.contains("[Agent exited with error]"), "got: {:?}", res.response);
        assert!(res.response.contains("boom: something failed"), "stderr should surface: {:?}", res.response);
        let _ = drain(rx);
    }

    #[tokio::test]
    async fn empty_response_clean_exit_is_no_response() {
        let (tx, rx) = tokio::sync::mpsc::channel(10);
        let proc = ScriptedProcess::stream_json(Vec::<String>::new()); // success, no output
        let res = run_agent_streaming(proc, &tx, &meta(), &AgentType::ClaudeCode).await;
        drop(tx);
        assert_eq!(res.response, "[No response]");
        let _ = drain(rx);
    }
}

#[cfg(test)]
mod stream_helpers_tests {
    //! Pure helpers extracted (0.8.8) from the two streaming loops so they're
    //! tested once instead of living as byte-identical copies.
    use super::{classify_tool_call, is_decoder_loop, ToolRecord, DECODER_LOOP_MAX_REPEATS};

    // ── is_decoder_loop ────────────────────────────────────────────────

    #[test]
    fn decoder_loop_fires_after_threshold_repeats() {
        let (mut last, mut count) = (String::new(), 0u32);
        let mut fired_at = None;
        for i in 1..=DECODER_LOOP_MAX_REPEATS + 5 {
            if is_decoder_loop("</thinking>\n", &mut last, &mut count) {
                fired_at = Some(i);
                break;
            }
        }
        // First call sets count=1, so the Nth identical delta makes count==N ;
        // fires exactly when count reaches the threshold.
        assert_eq!(fired_at, Some(DECODER_LOOP_MAX_REPEATS));
    }

    #[test]
    fn decoder_loop_resets_on_different_delta() {
        let (mut last, mut count) = (String::new(), 0u32);
        // 40 of "aaa", then a different delta, then 40 of "bbb" — neither run
        // reaches 50, so it never fires.
        for _ in 0..40 {
            assert!(!is_decoder_loop("aaa", &mut last, &mut count));
        }
        assert!(!is_decoder_loop("bbb", &mut last, &mut count));
        assert_eq!(count, 1, "counter resets when the delta changes");
        for _ in 0..40 {
            assert!(!is_decoder_loop("bbb", &mut last, &mut count));
        }
    }

    #[test]
    fn decoder_loop_ignores_short_and_whitespace_deltas() {
        // Deltas < 3 chars OR whitespace-only repeat legitimately in formatted
        // output (". ", "\n") and must NEVER trip the detector.
        let (mut last, mut count) = (String::new(), 0u32);
        for _ in 0..200 {
            assert!(!is_decoder_loop(". ", &mut last, &mut count), "short delta must not fire");
            assert!(!is_decoder_loop("\n\n\n", &mut last, &mut count), "whitespace delta must not fire");
            assert!(!is_decoder_loop("a", &mut last, &mut count), "1-char delta must not fire");
        }
        assert_eq!(count, 0, "ignored deltas never increment the counter");
    }

    #[test]
    fn decoder_loop_does_not_fire_just_below_threshold() {
        let (mut last, mut count) = (String::new(), 0u32);
        for _ in 0..(DECODER_LOOP_MAX_REPEATS - 1) {
            assert!(!is_decoder_loop("repeated", &mut last, &mut count));
        }
        assert_eq!(count, DECODER_LOOP_MAX_REPEATS - 1);
    }

    // ── classify_tool_call ─────────────────────────────────────────────

    #[test]
    fn kronn_internal_tool_goes_to_kronn_bucket_with_pretty_args() {
        let r = classify_tool_call("mcp__kronn-internal__disc_get_message", r#"{"idx":4}"#);
        match r {
            ToolRecord::Kronn(s) => {
                assert!(s.starts_with("[kronn-internal: disc_get_message("), "got {s}");
                assert!(s.contains('4'), "pretty args should surface the idx: {s}");
            }
            ToolRecord::Native(_) => panic!("kronn-internal prefix must map to Kronn bucket"),
        }
    }

    #[test]
    fn native_tool_goes_to_native_bucket() {
        let r = classify_tool_call("Read", r#"{"path":"src/lib.rs"}"#);
        match r {
            ToolRecord::Native(s) => {
                assert!(s.starts_with("[agent-native: Read("), "got {s}");
                assert!(s.contains("src/lib.rs"));
            }
            ToolRecord::Kronn(_) => panic!("non-kronn tool must map to Native bucket"),
        }
    }

    #[test]
    fn native_tool_with_empty_input_has_empty_args() {
        let r = classify_tool_call("Bash", "");
        match r {
            ToolRecord::Native(s) => assert_eq!(s, "[agent-native: Bash()]"),
            ToolRecord::Kronn(_) => panic!("Bash is native"),
        }
    }

    #[test]
    fn native_tool_long_input_is_truncated() {
        // Edit/Write can carry huge content — the native record truncates to
        // keep the transcript banner compact (~120 chars + ellipsis).
        let big = format!(r#"{{"content":"{}"}}"#, "x".repeat(500));
        let r = classify_tool_call("Write", &big);
        match r {
            ToolRecord::Native(s) => {
                assert!(s.contains('…'), "long input should be truncated with ellipsis: {s}");
                assert!(s.len() < big.len(), "record must be shorter than the raw input");
            }
            ToolRecord::Kronn(_) => panic!("Write is native"),
        }
    }
}
