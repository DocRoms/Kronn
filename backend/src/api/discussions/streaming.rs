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
};

/// Shared SSE stream builder
pub(super) async fn make_agent_stream(
    state: AppState,
    discussion_id: String,
    agent_override: Option<AgentType>,
) -> Sse<SseStream> {
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
                    for (server, env) in plugins.iter_mut() {
                        if let Some(ref spec) = server.api_spec {
                            if matches!(spec.auth, crate::models::ApiAuthKind::OAuth2ClientCredentials { .. }) {
                                // Look up the config id from the server id +
                                // the project — we need it as the cache key.
                                // `server.id` is stable per registry entry;
                                // multiple configs on the same server would
                                // overwrite each other if we keyed on it.
                                // Find the actual config id via the DB.
                                let server_id = server.id.clone();
                                let pid2 = disc.project_id.clone().unwrap_or_default();
                                let config_id = state.db.with_conn(move |conn| {
                                    let configs = crate::db::mcps::list_configs(conn)?;
                                    let id = configs.iter()
                                        .find(|c| c.server_id == server_id
                                            && (c.is_global || c.project_ids.iter().any(|p| p == &pid2)))
                                        .map(|c| c.id.clone());
                                    Ok(id)
                                }).await.ok().flatten().unwrap_or_else(|| server.id.clone());
                                match crate::core::oauth2_cache::resolve_token(
                                    &state.oauth2_cache, &config_id, &spec.auth, env,
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
            agent_type: &agent_type, project_path: &project_path,
            work_dir: workspace_path.as_deref(),
            prompt: &prompt, tokens: &tokens, full_access,
            skill_ids: &skill_ids, directive_ids: &directive_ids, profile_ids: &profile_ids,
            mcp_context_override: global_mcp_context.as_deref(),
            tier: disc_tier, model_tiers: Some(&model_tiers_config),
            context_files_prompt: &context_files_prompt,
            // Forward to the agent process env so the kronn-internal MCP
            // bridge knows which discussion to introspect when called.
            discussion_id: Some(&discussion_id),
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
                let stall_timeout = Duration::from_secs(stall_timeout_min as u64 * 60);
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
                // child. `strip_thinking_leaks` in the parser normally
                // catches the known leak, but the same mechanic could
                // trigger on any other repeating token, so the detector
                // stays kind-agnostic. Counts only post-strip text (empty
                // chunks become `Skip` and never hit this loop).
                let mut last_text_delta = String::new();
                let mut repeat_delta_count: u32 = 0;
                let mut stopped_on_loop: bool = false;
                const MAX_REPEAT_DELTAS: u32 = 50;
                const REPEAT_MIN_LEN: usize = 3;

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
                                if text.len() >= REPEAT_MIN_LEN && !text.trim().is_empty() {
                                    if text == last_text_delta {
                                        repeat_delta_count += 1;
                                        if repeat_delta_count >= MAX_REPEAT_DELTAS {
                                            tracing::warn!(
                                                "Agent stream entered a decoder loop — same delta {:?} repeated {} times, aborting",
                                                text.chars().take(40).collect::<String>(),
                                                repeat_delta_count,
                                            );
                                            stopped_on_loop = true;
                                            was_interrupted = true;
                                            break;
                                        }
                                    } else {
                                        last_text_delta = text.clone();
                                        repeat_delta_count = 1;
                                    }
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
                                    // Persist kronn-internal calls so the
                                    // disc transcript carries a uniform
                                    // tool-call trace (UI badge in
                                    // MessageBubble). Format mirrors
                                    // `slash_markers.rs`: `[kronn-internal:
                                    // disc_get_message(4)]`.
                                    if let Some(name) = tool.strip_prefix("mcp__kronn-internal__") {
                                        let pretty_args = pretty_kronn_args(name, &current_tool_input);
                                        kronn_tool_calls.push(format!(
                                            "[kronn-internal: {}({})]",
                                            name, pretty_args
                                        ));
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
                        MAX_REPEAT_DELTAS
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

                // Detect error patterns in both stdout and stderr and add helpful guidance
                if !success && !was_interrupted {
                    let all_output = format!("{}\n{}", full_response, stderr_text);
                    let error_hint = detect_agent_error_hint(&all_output, &agent_type);
                    if let Some(hint) = error_hint {
                        full_response.push_str(&format!("\n\n{}", hint));
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
                    author_pseudo: None, author_avatar_email: None,
                };

                let did = disc_id.clone();
                let msg = agent_msg.clone();
                if let Err(e) = state.db.with_conn(move |conn| {
                    crate::db::discussions::insert_message(conn, &did, &msg)
                }).await {
                    tracing::error!("Failed to save agent message: {e}");
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
                            author_avatar_email: None,
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
                            author_avatar_email: None,
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

                // Clear the in-flight checkpoint — the final message is now in
                // `messages`, so partial_response would be redundant + would
                // double up at the next backend boot if we left it dangling.
                let did_clear = disc_id.clone();
                let _ = state.db.with_conn(move |conn| {
                    crate::db::discussions::set_partial_response(conn, &did_clear, None)
                }).await;

                // ── Batch progress hook ────────────────────────────────
                // If this disc was spawned by a batch workflow run, bump
                // its counters. Broadcast a progress or finished event so
                // the sidebar pill + any open batch monitor updates live.
                if let Some(ref run_id) = batch_run_id {
                    let run_id_inner = run_id.clone();
                    let child_succeeded = success;
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
                    id: Uuid::new_v4().to_string(),
                    role: MessageRole::System,
                    content: format!("Erreur: {}", e),
                    agent_type: None,
                    timestamp: Utc::now(),
                    tokens_used: 0,
                    auth_mode: None,
                    model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None,
                };

                let did = disc_id.clone();
                if let Err(db_err) = state.db.with_conn(move |conn| {
                    crate::db::discussions::insert_message(conn, &did, &err_msg)
                }).await {
                    tracing::error!("Failed to save agent error message: {db_err}");
                }

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
    mut process: runner::AgentProcess,
    tx: &tokio::sync::mpsc::Sender<AgentStreamEvent>,
    meta: &AgentStreamMeta,
    agent_type: &AgentType,
) -> AgentRunResult {
    let mut full_response = String::new();
    let mut stream_tokens: u64 = 0;
    let mut current_tool: Option<String> = None;
    let mut tool_input = String::new();
    let is_stream_json = process.output_mode == runner::OutputMode::StreamJson;
    let deadline = tokio::time::Instant::now() + AGENT_GLOBAL_TIMEOUT;

    let mut signal_stop = false;
    // Same decoder-loop detector as the main stream loop. Orchestration runs
    // use the same Claude model and can exhibit the same failure mode; we
    // break out and return whatever text arrived before the loop started.
    let mut last_text_delta = String::new();
    let mut repeat_delta_count: u32 = 0;
    const MAX_REPEAT_DELTAS: u32 = 50;
    const REPEAT_MIN_LEN: usize = 3;
    loop {
        tokio::select! {
            line = process.next_line() => {
                match line {
                    Some(line) => {
                        if is_stream_json {
                            match runner::parse_claude_stream_line(&line) {
                                runner::StreamJsonEvent::Text(text) => {
                                    // Decoder-loop guard — same mechanic as the main
                                    // stream loop (see MAX_REPEAT_DELTAS above).
                                    if text.len() >= REPEAT_MIN_LEN && !text.trim().is_empty() {
                                        if text == last_text_delta {
                                            repeat_delta_count += 1;
                                            if repeat_delta_count >= MAX_REPEAT_DELTAS {
                                                tracing::warn!(
                                                    "Orchestration agent entered a decoder loop — delta {:?} repeated {} times, aborting",
                                                    text.chars().take(40).collect::<String>(),
                                                    repeat_delta_count,
                                                );
                                                let _ = process.child.kill().await;
                                                full_response.push_str("\n\n---\n🔁 **Decoder loop detected** — agent killed.");
                                                break;
                                            }
                                        } else {
                                            last_text_delta = text.clone();
                                            repeat_delta_count = 1;
                                        }
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
                let _ = process.child.kill().await;
                break;
            }
        }
    }
    if signal_stop {
        let _ = process.child.kill().await;
    }

    let status = process.child.wait().await;
    process.fix_ownership();
    let success = status.as_ref().map(|s| s.success()).unwrap_or(false);
    let stderr = process.captured_stderr_flushed().await;
    let stderr_text = stderr.join("\n");

    if full_response.is_empty() && !success {
        let exit_info = match &status {
            Ok(s) => format!("exit code: {:?}", s.code()),
            Err(e) => format!("wait error: {}", e),
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
pub(super) async fn run_agent_collect(mut process: runner::AgentProcess) -> String {
    let mut output = String::new();
    let is_json = process.output_mode == runner::OutputMode::StreamJson;
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
                let _ = process.child.kill().await;
                break;
            }
        }
    }
    let _ = process.child.wait().await;
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
