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

use axum::response::sse::{Event, Sse};
use chrono::Utc;
use futures::StreamExt;
use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::agents::runner::{self, AgentIo};
use crate::models::*;
use crate::AppState;

use super::orchestration::detect_agent_error_hint;
use super::{
    configured_agent_global_timeout, detect_terminal_signal, truncate_after_signal,
    AgentStreamEvent, SseStream, DEFAULT_STALL_TIMEOUT_MIN, MAX_AGENT_RESPONSE_BYTES,
    NON_STREAMING_STALL_TIMEOUT,
};
use crate::api::disc_helpers::{
    agent_alias, agent_handoff_budget_instruction, agent_handoff_target_is_allowed, auth_mode_for,
    estimate_extra_context_len, extract_agent_handoff_markers,
};
use crate::api::disc_prompts::build_agent_prompt;

/// Build the native HTTP tool executor from durable discussion lineage.
///
/// The ordinary discussion path and the task-dispatch SSE path both end up in
/// `make_agent_stream_inner`. Resolving the worker scope here prevents those
/// entry points from drifting into different catalogues. A database error is
/// deliberately propagated: silently falling back to the broader principal
/// catalogue would give a worker tools and budgets it must not receive.
pub(crate) async fn native_http_tools_for_discussion(
    state: &AppState,
    discussion_id: &str,
    agent_type: &AgentType,
    source_message_id: Option<String>,
    source_dispatch_job_id: Option<String>,
    tool_free_judge: bool,
) -> anyhow::Result<Option<std::sync::Arc<dyn crate::agents::tools::ToolExecutor>>> {
    if tool_free_judge || !runner::is_http_chat_agent(agent_type) {
        return Ok(None);
    }

    let room = discussion_id.to_string();
    let worker_execution = state
        .db
        .with_read_conn(move |conn| {
            crate::db::orchestration::get_execution_for_sub_discussion(conn, &room)
        })
        .await?;
    let is_worker_room = worker_execution.is_some();
    tracing::info!(
        discussion_id,
        agent = ?agent_type,
        is_worker_room,
        "Resolved native HTTP tool scope from durable discussion lineage"
    );

    let disc_id = Some(discussion_id.to_string());
    let tools = if is_worker_room {
        crate::api::agent_tools::KronnToolExecutor::arc_for_worker_room(
            state.clone(),
            disc_id,
            agent_type.clone(),
            source_message_id,
            source_dispatch_job_id,
            worker_execution.and_then(|execution| execution.worker_scope),
        )
    } else {
        crate::api::agent_tools::KronnToolExecutor::arc(
            state.clone(),
            disc_id,
            agent_type.clone(),
            source_message_id,
            source_dispatch_job_id,
        )
    };
    Ok(Some(tools))
}

/// Resolve the opaque delivery capability for a CLI-backed `kind=agent`
/// worker. Ordinary discussion turns and exact joined-CLI workers return
/// `None`; a task dispatch must match the durable child room, provider and
/// trigger before any context reaches the spawned process.
async fn cli_task_worker_context(
    state: &AppState,
    discussion_id: &str,
    agent_type: &AgentType,
    dispatch_job_id: Option<&str>,
) -> anyhow::Result<Option<runner::TaskWorkerBridgeContext>> {
    if runner::is_http_chat_agent(agent_type) {
        return Ok(None);
    }
    let discussion_id_owned = discussion_id.to_string();
    let is_task_worker_room = state
        .db
        .with_read_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM task_executions \
                 WHERE sub_discussion_id = ?1 AND worker_target_kind = 'agent')",
                rusqlite::params![discussion_id_owned],
                |row| row.get::<_, bool>(0),
            )?)
        })
        .await?;
    let Some(dispatch_job_id) = dispatch_job_id else {
        anyhow::ensure!(
            !is_task_worker_room,
            "task-worker launch is missing its immutable dispatch id"
        );
        return Ok(None);
    };
    let lineage = state
        .db
        .with_read_conn({
            let dispatch_job_id = dispatch_job_id.to_string();
            move |conn| {
                let execution =
                    crate::db::orchestration::get_execution_for_dispatch(conn, &dispatch_job_id)?;
                let dispatch = crate::db::agent_dispatch::get(conn, &dispatch_job_id)?;
                Ok(execution.zip(dispatch))
            }
        })
        .await?;
    let Some((execution, dispatch)) = lineage else {
        anyhow::ensure!(
            !is_task_worker_room,
            "task-worker launch has no matching execution dispatch lineage"
        );
        return Ok(None);
    };

    anyhow::ensure!(
        execution.worker_target_kind == Some(MessageTargetKind::Agent),
        "task dispatch is not owned by a launched discussion agent"
    );
    anyhow::ensure!(
        execution.dispatch_job_id.as_deref() == Some(dispatch_job_id),
        "task dispatch is no longer the execution's current worker dispatch"
    );
    anyhow::ensure!(
        execution.sub_discussion_id.as_deref() == Some(discussion_id),
        "task dispatch child discussion does not match the spawned room"
    );
    anyhow::ensure!(
        dispatch.discussion_id == discussion_id,
        "task dispatch job does not belong to the spawned room"
    );
    let persisted_agent = execution
        .worker_agent_type
        .as_deref()
        .map(crate::db::orchestration::agent_type_from_db)
        .transpose()?;
    anyhow::ensure!(
        persisted_agent.as_ref() == Some(agent_type),
        "task dispatch provider does not match the spawned agent"
    );
    anyhow::ensure!(
        !dispatch.trigger_message_id.trim().is_empty(),
        "task dispatch has no trigger message"
    );

    Ok(Some(runner::TaskWorkerBridgeContext {
        execution_id: execution.id,
        discussion_id: discussion_id.to_string(),
        agent_type: crate::db::orchestration::agent_type_to_db(agent_type),
        dispatch_job_id: dispatch_job_id.to_string(),
        source_message_id: dispatch.trigger_message_id,
    }))
}

#[cfg(test)]
mod native_http_tools_scope_tests {
    use super::{cli_task_worker_context, native_http_tools_for_discussion};
    use crate::agents::tools::ToolRunMode;
    use crate::db::agent_dispatch::NewAgentDispatchJob;
    use crate::models::{
        AgentType, LaunchSingleTaskInput, MessageTargetKind, OrchestrationActor, PlanningActorKind,
        TaskWorkerScope,
    };
    use crate::{AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn test_state() -> AppState {
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        AppState::new_defaults(config, db, DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    fn backend_actor() -> OrchestrationActor {
        OrchestrationActor {
            kind: PlanningActorKind::Backend,
            id: Some("streaming-scope-test".into()),
            session_id: None,
            source_message_id: None,
        }
    }

    #[tokio::test]
    async fn task_dispatch_sse_uses_worker_tools_while_an_ordinary_room_stays_general() {
        let state = test_state();
        state
            .db
            .with_conn(|conn| {
                let now = "2026-08-24T00:00:00Z";
                conn.execute(
                    "INSERT INTO discussions (id, title, created_at, updated_at)
                     VALUES ('d-parent', 'Parent', ?1, ?1),
                            ('d-worker', 'Worker', ?1, ?1)",
                    [now],
                )?;
                conn.execute(
                    "INSERT INTO planning_tasks
                     (id, task_number, title, created_at, updated_at)
                     VALUES ('t-worker', 1, 'Worker task', ?1, ?1)",
                    [now],
                )?;
                let scope = TaskWorkerScope::PrelocalizedEdit {
                    path: "backend/src/lib.rs".into(),
                    start_line: 40,
                    end_line: 44,
                };
                let mut launch = LaunchSingleTaskInput::new("t-worker", "d-parent");
                launch.worker_scope = Some(scope);
                let execution =
                    crate::db::orchestration::launch_single_task(conn, &launch, &backend_actor())?
                        .execution;
                crate::db::orchestration::set_execution_sub_discussion(
                    conn,
                    &execution.id,
                    "d-worker",
                )?;
                Ok(())
            })
            .await
            .expect("seed execution lineage");

        let worker = native_http_tools_for_discussion(
            &state,
            "d-worker",
            &AgentType::Ollama,
            Some("worker-trigger".into()),
            Some("worker-dispatch".into()),
            false,
        )
        .await
        .expect("resolve worker scope")
        .expect("Ollama receives native tools");
        assert_eq!(worker.run_mode(), ToolRunMode::Worker);
        assert_eq!(
            worker.worker_scope(),
            Some(TaskWorkerScope::PrelocalizedEdit {
                path: "backend/src/lib.rs".into(),
                start_line: 40,
                end_line: 44,
            })
        );
        let worker_names = worker
            .catalogue()
            .into_iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert!(worker_names.iter().any(|name| name == "task_exec_deliver"));
        assert!(!worker_names.iter().any(|name| name == "task_list"));

        let principal = native_http_tools_for_discussion(
            &state,
            "d-parent",
            &AgentType::Ollama,
            Some("principal-trigger".into()),
            Some("principal-dispatch".into()),
            false,
        )
        .await
        .expect("resolve principal scope")
        .expect("Ollama receives native tools");
        assert_eq!(principal.run_mode(), ToolRunMode::General);
        assert!(principal.catalogue().iter().any(|tool| {
            tool["function"]["name"]
                .as_str()
                .is_some_and(|name| name == "task_list")
        }));
    }

    #[tokio::test]
    async fn cli_task_worker_context_is_derived_from_exact_dispatch_lineage() {
        let state = test_state();
        let execution_id = state
            .db
            .with_conn(|conn| {
                let now = "2026-08-24T00:00:00Z";
                conn.execute(
                    "INSERT INTO discussions (id, title, created_at, updated_at)
                     VALUES ('d-parent', 'Parent', ?1, ?1),
                            ('d-worker', 'Worker', ?1, ?1),
                            ('d-foreign', 'Foreign', ?1, ?1)",
                    [now],
                )?;
                conn.execute(
                    "INSERT INTO planning_tasks
                     (id, task_number, title, created_at, updated_at)
                     VALUES ('t-cli-worker', 2, 'CLI worker task', ?1, ?1)",
                    [now],
                )?;
                let mut input = LaunchSingleTaskInput::new("t-cli-worker", "d-parent");
                input.worker_target_kind = Some(MessageTargetKind::Agent);
                input.worker_agent_type = Some(crate::db::orchestration::agent_type_to_db(
                    &AgentType::Codex,
                ));
                let execution =
                    crate::db::orchestration::launch_single_task(conn, &input, &backend_actor())?
                        .execution;
                crate::db::orchestration::set_execution_sub_discussion(
                    conn,
                    &execution.id,
                    "d-worker",
                )?;
                crate::db::discussions::insert_message(
                    conn,
                    "d-worker",
                    &crate::models::DiscussionMessage {
                        recovered_partial: false,
                        session_tokens_at_message: None,
                        author_cli_ordinal: None,
                        model: None,
                        lint_report: None,
                        id: "trigger-a".into(),
                        role: crate::models::MessageRole::User,
                        channel: crate::models::MessageChannel::Main,
                        content: "bounded work".into(),
                        agent_type: None,
                        timestamp: chrono::Utc::now(),
                        tokens_used: 0,
                        auth_mode: None,
                        model_tier: None,
                        cost_usd: None,
                        author_pseudo: None,
                        author_avatar_email: None,
                        source_msg_id: None,
                        duration_ms: None,
                        target_agent: None,
                        reply_to_message_id: None,
                    },
                )?;
                crate::db::agent_dispatch::enqueue(
                    conn,
                    NewAgentDispatchJob {
                        id: "dispatch-a",
                        discussion_id: "d-worker",
                        trigger_message_id: "trigger-a",
                        trigger_sort_order: 1,
                        dedupe_key: "dispatch-a",
                        agent_override: Some(&AgentType::Codex),
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
                crate::db::orchestration::attach_execution_dispatch(
                    conn,
                    &execution.id,
                    "dispatch-a",
                )?;
                Ok(execution.id)
            })
            .await
            .expect("seed exact CLI worker lineage");

        let context =
            cli_task_worker_context(&state, "d-worker", &AgentType::Codex, Some("dispatch-a"))
                .await
                .expect("resolve exact lineage")
                .expect("CLI task worker receives a delivery capability");
        assert_eq!(context.execution_id, execution_id);
        assert_eq!(context.discussion_id, "d-worker");
        assert_eq!(context.agent_type, "Codex");
        assert_eq!(context.dispatch_job_id, "dispatch-a");
        assert_eq!(context.source_message_id, "trigger-a");

        assert!(cli_task_worker_context(
            &state,
            "d-foreign",
            &AgentType::Codex,
            Some("dispatch-a"),
        )
        .await
        .is_err());
        assert!(cli_task_worker_context(
            &state,
            "d-worker",
            &AgentType::ClaudeCode,
            Some("dispatch-a"),
        )
        .await
        .is_err());
        assert!(cli_task_worker_context(
            &state,
            "d-worker",
            &AgentType::Ollama,
            Some("dispatch-a"),
        )
        .await
        .expect("HTTP providers do not use the CLI bridge")
        .is_none());
        assert!(cli_task_worker_context(
            &state,
            "d-worker",
            &AgentType::Codex,
            Some("unknown-dispatch"),
        )
        .await
        .is_err());
        assert!(
            cli_task_worker_context(&state, "d-worker", &AgentType::Codex, None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn unreadable_execution_lineage_returns_no_http_executor() {
        let state = test_state();
        state
            .db
            .with_conn(|conn| {
                // Corrupt only this disposable in-memory fixture so the real
                // lookup takes its database-error path rather than the valid
                // "ordinary room" (`Ok(false)`) path.
                conn.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE task_executions;")?;
                Ok(())
            })
            .await
            .expect("make execution lineage unreadable");

        let result = native_http_tools_for_discussion(
            &state,
            "d-unknown",
            &AgentType::Ollama,
            Some("trigger".into()),
            Some("dispatch".into()),
            false,
        )
        .await;
        assert!(
            result.is_err(),
            "an unreadable lineage must refuse the run, never return General tools"
        );
    }
}

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
/// those we apply the configured timeout with a 15-minute safety floor, while
/// the absolute 30-minute global deadline remains the final ceiling. Pure —
/// unit-tested.
pub(super) fn effective_stall_timeout(
    is_stream_json: bool,
    configured: std::time::Duration,
    non_streaming_floor: std::time::Duration,
) -> std::time::Duration {
    if is_stream_json {
        configured
    } else {
        configured.max(non_streaming_floor)
    }
}

/// Select the explicit wall-clock budget for this provider. Keeping the
/// choice pure prevents an Ollama-only multiplier from drifting back into the
/// runtime while Settings continues to display a different number.
fn effective_global_timeout(
    agent_type: &AgentType,
    hosted_minutes: u32,
    local_minutes: u32,
) -> Duration {
    configured_agent_global_timeout(if *agent_type == AgentType::Ollama {
        local_minutes
    } else {
        hosted_minutes
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentTimeoutReason {
    Stall(Duration),
    Global(Duration),
}

fn timeout_notice(reason: AgentTimeoutReason) -> String {
    let duration = match reason {
        AgentTimeoutReason::Stall(duration) | AgentTimeoutReason::Global(duration) => duration,
    };
    let minutes = duration.as_secs().div_ceil(60);
    match reason {
        AgentTimeoutReason::Stall(_) => format!(
            "⚠️ **Agent interrupted by Kronn after {minutes} min without output.** \
             Retry the turn, or increase **Config > Server > Agent inactivity timeout** \
             before retrying. Non-streaming agents keep a 15-minute safety floor."
        ),
        AgentTimeoutReason::Global(_) => format!(
            "⚠️ **Agent interrupted by Kronn after reaching the {minutes}-minute global execution limit.** \
             Retry the turn to resume from the durable discussion context."
        ),
    }
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

/// Broadcast a batch state that was already persisted by the caller.
///
/// Durable dispatch settlement updates the dispatch job and its parent batch
/// counters in one transaction. Keeping the broadcast separate lets that path
/// notify the UI after commit without incrementing the counters a second time.
pub(crate) fn broadcast_batch_progress(state: &AppState, disc_id: &str, updated_run: &WorkflowRun) {
    let is_final = matches!(
        updated_run.status,
        RunStatus::Success | RunStatus::Partial | RunStatus::Failed
    );
    let event = if is_final {
        WsMessage::BatchRunFinished {
            run_id: updated_run.id.clone(),
            discussion_id: disc_id.to_string(),
            batch_name: updated_run.batch_name.clone(),
            batch_total: updated_run.batch_total,
            batch_completed: updated_run.batch_completed,
            batch_failed: updated_run.batch_failed,
        }
    } else {
        WsMessage::BatchRunProgress {
            run_id: updated_run.id.clone(),
            discussion_id: disc_id.to_string(),
            batch_total: updated_run.batch_total,
            batch_completed: updated_run.batch_completed,
            batch_failed: updated_run.batch_failed,
        }
    };
    let _ = state.ws_broadcast.send(event);
    if is_final {
        tracing::info!(
            "Batch run {} finished: {}/{} ok, {} failed",
            updated_run.id,
            updated_run.batch_completed,
            updated_run.batch_total,
            updated_run.batch_failed
        );
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
    make_agent_stream_inner(state, discussion_id, agent_override, None, None, None, None).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentExecutionOutcome {
    Finished { success: bool },
    PreflightFailed { diagnostic: String },
    RuntimeUnavailable { reason: String },
}

fn agent_start_failure_outcome(agent_type: &AgentType, error: &str) -> AgentExecutionOutcome {
    let non_retryable_http_status = agent_http_status(error)
        .is_some_and(|status| (400..500).contains(&status) && !matches!(status, 408 | 425 | 429));
    if matches!(
        agent_type,
        AgentType::LiteLlm | AgentType::Nvidia | AgentType::Ollama | AgentType::Custom
    ) || error.starts_with("Project path not found:")
        || error.starts_with("Copilot task worker cannot start:")
        || non_retryable_http_status
    {
        AgentExecutionOutcome::PreflightFailed {
            diagnostic: if error.starts_with("Copilot task worker cannot start:") {
                error.to_string()
            } else {
                "agent execution preflight failed".into()
            },
        }
    } else {
        AgentExecutionOutcome::RuntimeUnavailable {
            reason: error.to_string(),
        }
    }
}

fn agent_http_status(error: &str) -> Option<u16> {
    error
        .split_once(" error ")
        .and_then(|(_, suffix)| suffix.split_whitespace().next())
        .and_then(|status| status.parse::<u16>().ok())
}

/// A model-routing failure is useful to operators in full, but dumping a
/// nested LiteLLM/Vertex JSON body into the transcript makes the discussion
/// unreadable. Keep the raw diagnostic in a machine-readable System event so
/// the UI can collapse it, while exposing the HTTP code, attempted model and
/// reasoning tier for a one-click settings shortcut.
fn agent_start_error_content(
    agent_type: &AgentType,
    model: Option<&str>,
    tier: crate::models::ModelTier,
    language: &str,
    error: &str,
    retry_dispatch_id: Option<&str>,
) -> Option<String> {
    let backend = format!("{agent_type:?}");
    let status = agent_http_status(error);
    let is_model_error =
        status.is_some_and(|code| matches!(code, 400 | 404 | 422)) && model.is_some();
    if !is_model_error && !matches!(agent_type, AgentType::LiteLlm | AgentType::Ollama) {
        return None;
    }
    let summary = if is_model_error {
        let status = status.expect("model error has an HTTP status");
        let model = model.expect("model error has an attempted model");
        match language {
            "fr" => format!(
                "{backend} a répondu HTTP {status} : le modèle « {model} » est introuvable, indisponible dans cette région ou non autorisé."
            ),
            "es" => format!(
                "{backend} respondió HTTP {status}: el modelo «{model}» no existe, no está disponible en esta región o no está autorizado."
            ),
            "zh" => format!(
                "{backend} 返回 HTTP {status}：模型“{model}”不存在、在此区域不可用或未获授权。"
            ),
            _ => format!(
                "{backend} returned HTTP {status}: model “{model}” was not found, is unavailable in this region, or is not authorized."
            ),
        }
    } else if let Some(status) = status {
        match language {
            "fr" => format!(
                "{backend} a échoué avec le code HTTP {status}. Vérifiez l'accès, le VPN ou le service, puis relancez uniquement cet agent."
            ),
            "es" => format!(
                "{backend} falló con el código HTTP {status}. Comprueba el acceso, la VPN o el servicio y vuelve a ejecutar solo este agente."
            ),
            "zh" => format!(
                "{backend} 请求失败（HTTP {status}）。请检查访问权限、VPN 或服务，然后仅重试此智能体。"
            ),
            _ => format!(
                "{backend} failed with HTTP {status}. Check access, the VPN or service, then retry only this agent."
            ),
        }
    } else {
        match language {
            "fr" => format!(
                "{backend} est momentanément inaccessible. Vérifiez la connexion, le VPN ou le service, puis relancez uniquement cet agent."
            ),
            "es" => format!(
                "{backend} no está disponible temporalmente. Comprueba la conexión, la VPN o el servicio y vuelve a ejecutar solo este agente."
            ),
            "zh" => format!(
                "{backend} 暂时无法访问。请检查网络、VPN 或服务，然后仅重试此智能体。"
            ),
            _ => format!(
                "{backend} is temporarily unreachable. Check the connection, VPN or service, then retry only this agent."
            ),
        }
    };
    let tier = match tier {
        crate::models::ModelTier::Economy => "economy",
        crate::models::ModelTier::Default => "default",
        crate::models::ModelTier::Reasoning => "reasoning",
    };
    let payload = serde_json::json!({
        "kind": if is_model_error { "model_error" } else { "agent_error" },
        "status": status,
        "summary": summary,
        "detail": error,
        "tier": tier,
        "retry_dispatch_id": retry_dispatch_id,
        "retried": false,
    });
    Some(format!("[kronn:agent-error]\n{payload}"))
}

fn finish_tracked_preflight(
    completion_tx: &mut Option<tokio::sync::oneshot::Sender<AgentExecutionOutcome>>,
) {
    if let Some(sender) = completion_tx.take() {
        let _ = sender.send(AgentExecutionOutcome::PreflightFailed {
            diagnostic: "agent execution preflight failed".into(),
        });
    }
}

fn clear_awaiting_after_terminal(
    conn: &rusqlite::Connection,
    discussion_id: &str,
    tracked_dispatch: bool,
) -> anyhow::Result<()> {
    if tracked_dispatch
        || crate::db::agent_dispatch::has_active_for_discussion(conn, discussion_id)?
    {
        // A plural turn may still have Pending jobs after this model replies.
        // The dispatch settlement transaction computes the authoritative value
        // once the current job becomes terminal; clearing here creates a false
        // idle window in which the remaining model placeholder disappears.
        return Ok(());
    }
    crate::db::discussions::set_awaiting_agent(conn, discussion_id, false)?;
    // retention=0 means run-lifetime only. Once the final dispatch for this
    // discussion is terminal, irreversibly discard any QP child ciphertext.
    crate::db::execution_variable_snapshots::purge_run_lifetime_snapshot(
        conn,
        "quick_prompt",
        discussion_id,
        Utc::now(),
    )?;
    crate::db::execution_variable_snapshots::purge_run_lifetime_snapshot(
        conn,
        "quick_prompt_batch_item",
        discussion_id,
        Utc::now(),
    )?;
    Ok(())
}

#[cfg(test)]
mod awaiting_terminal_tests {
    use super::clear_awaiting_after_terminal;

    #[test]
    fn tracked_reply_does_not_clear_a_plural_turn_before_dispatch_settlement() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at, awaiting_agent)
             VALUES ('d-plural', 'Plural', ?1, ?1, 1)",
            [&now],
        )
        .unwrap();

        clear_awaiting_after_terminal(&conn, "d-plural", true).unwrap();
        let still_awaiting: bool = conn
            .query_row(
                "SELECT awaiting_agent FROM discussions WHERE id = 'd-plural'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(still_awaiting);

        clear_awaiting_after_terminal(&conn, "d-plural", false).unwrap();
        let cleared: bool = conn
            .query_row(
                "SELECT awaiting_agent FROM discussions WHERE id = 'd-plural'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!cleared);
    }
}

#[cfg(test)]
mod dispatch_prompt_snapshot_tests {
    use super::{discussion_at_dispatch_trigger, independent_sibling_notice};
    use crate::models::Discussion;

    fn discussion_with_turns() -> Discussion {
        serde_json::from_value(serde_json::json!({
            "id": "d-plural",
            "project_id": null,
            "title": "Independent answers",
            "agent": "Codex",
            "language": "fr",
            "participants": ["Codex", "ClaudeCode"],
            "messages": [
                {
                    "id": "u1",
                    "role": "User",
                    "content": "Répondez séparément",
                    "agent_type": null,
                    "timestamp": "2026-08-11T08:00:00Z"
                },
                {
                    "id": "a-codex",
                    "role": "Agent",
                    "content": "Première réponse",
                    "agent_type": "Codex",
                    "timestamp": "2026-08-11T08:00:01Z",
                    "reply_to_message_id": "u1"
                },
                {
                    "id": "u2",
                    "role": "User",
                    "content": "Question suivante",
                    "agent_type": null,
                    "timestamp": "2026-08-11T08:00:02Z"
                }
            ],
            "message_count": 3,
            "non_system_message_count": 3,
            "summary_cache": "Résumé calculé après la première réponse",
            "summary_up_to_msg_idx": 1,
            "created_at": "2026-08-11T08:00:00Z",
            "updated_at": "2026-08-11T08:00:02Z"
        }))
        .expect("valid discussion fixture")
    }

    #[test]
    fn plural_responder_sees_completed_siblings_but_not_later_user_turns() {
        let snapshot = discussion_at_dispatch_trigger(&discussion_with_turns(), Some("u1"));

        assert_eq!(snapshot.messages.len(), 2);
        assert_eq!(snapshot.messages[0].id, "u1");
        assert_eq!(snapshot.messages[1].id, "a-codex");
        assert!(!snapshot.messages.iter().any(|message| message.id == "u2"));
        assert_eq!(snapshot.message_count, 2);
        assert_eq!(snapshot.non_system_message_count, 2);
        assert_eq!(snapshot.summary_cache, None);
        assert_eq!(snapshot.summary_up_to_msg_idx, None);
    }

    #[test]
    fn missing_trigger_keeps_the_full_conversation() {
        let disc = discussion_with_turns();
        let snapshot = discussion_at_dispatch_trigger(&disc, Some("missing"));

        assert_eq!(snapshot.messages.len(), 3);
        assert_eq!(snapshot.summary_cache, disc.summary_cache);
    }

    #[test]
    fn agent_handoff_trigger_still_excludes_later_turns() {
        let snapshot = discussion_at_dispatch_trigger(&discussion_with_turns(), Some("a-codex"));

        assert_eq!(snapshot.messages.len(), 2);
        assert_eq!(snapshot.messages[0].id, "u1");
        assert_eq!(snapshot.messages[1].id, "a-codex");
        assert!(!snapshot.messages.iter().any(|message| message.id == "u2"));
    }

    #[test]
    fn sibling_notice_encourages_complement_without_relaunch() {
        let notice = independent_sibling_notice("fr", "@codex, @ollama");

        assert!(notice.contains("complète-les utilement"));
        assert!(notice.contains("ne le relance pas"));
        assert!(notice.contains("@codex, @ollama"));
        assert!(independent_sibling_notice("fr", "").is_empty());
    }
}

pub(crate) async fn make_agent_stream_tracked(
    state: AppState,
    discussion_id: String,
    agent_override: Option<AgentType>,
    tier_override: Option<crate::models::ModelTier>,
    dispatch_job_id: String,
) -> (
    Sse<SseStream>,
    tokio::sync::oneshot::Receiver<AgentExecutionOutcome>,
) {
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    let stream = make_agent_stream_inner(
        state,
        discussion_id,
        agent_override,
        tier_override,
        Some(dispatch_job_id),
        None,
        Some(completion_tx),
    )
    .await;
    (stream, completion_rx)
}

pub(crate) async fn make_agent_stream_tracked_with_initial_event(
    state: AppState,
    discussion_id: String,
    agent_override: Option<AgentType>,
    tier_override: Option<crate::models::ModelTier>,
    dispatch_job_id: String,
    initial_event: Event,
) -> (
    Sse<SseStream>,
    tokio::sync::oneshot::Receiver<AgentExecutionOutcome>,
) {
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    let stream = make_agent_stream_inner(
        state,
        discussion_id,
        agent_override,
        tier_override,
        Some(dispatch_job_id),
        Some(initial_event),
        Some(completion_tx),
    )
    .await;
    (stream, completion_rx)
}

fn prepend_initial_event(stream: SseStream, initial_event: Option<Event>) -> SseStream {
    match initial_event {
        Some(event) => {
            Box::pin(futures::stream::once(async move { Ok::<_, Infallible>(event) }).chain(stream))
        }
        None => stream,
    }
}

/// Freeze the conversational view at the message that owns this dispatch.
///
/// Plural targets are executed one at a time inside a discussion so their
/// file operations cannot race. A responder sees completed direct siblings so
/// it can complement them, but never later User turns or unrelated handoff
/// branches. The durable transcript itself remains untouched.
fn discussion_at_dispatch_trigger(
    disc: &Discussion,
    trigger_message_id: Option<&str>,
) -> Discussion {
    let Some(trigger_message_id) = trigger_message_id else {
        return disc.clone();
    };
    let Some(trigger_index) = disc
        .messages
        .iter()
        .position(|message| message.id == trigger_message_id)
    else {
        return disc.clone();
    };
    if trigger_index + 1 >= disc.messages.len() {
        return disc.clone();
    }

    let mut snapshot = disc.clone();
    let trigger = &disc.messages[trigger_index];
    snapshot.messages = if matches!(trigger.role, MessageRole::User) {
        // A later responder can complement an earlier sibling or answer its
        // concrete question without creating another dispatch. Later User
        // turns and unrelated/handoff branches remain invisible.
        disc.messages[..=trigger_index]
            .iter()
            .chain(disc.messages[trigger_index + 1..].iter().filter(|message| {
                matches!(message.role, MessageRole::Agent)
                    && matches!(message.channel, MessageChannel::Main)
                    && message.reply_to_message_id.as_deref() == Some(trigger_message_id)
            }))
            .cloned()
            .collect()
    } else {
        disc.messages[..=trigger_index].to_vec()
    };
    snapshot.message_count = snapshot.messages.len() as u32;
    snapshot.non_system_message_count = snapshot
        .messages
        .iter()
        .filter(|message| !matches!(message.role, MessageRole::System))
        .count() as u32;
    // A summary may have been refreshed after this dispatch was accepted.
    // Keeping it could leak sibling/later turns even though the raw messages
    // were truncated, so the bounded prompt uses raw pre-trigger history.
    snapshot.summary_cache = None;
    snapshot.summary_up_to_msg_idx = None;
    snapshot
}

fn independent_sibling_notice(language: &str, aliases: &str) -> String {
    if aliases.is_empty() {
        return String::new();
    }
    match language {
        "fr" => format!(
            "--- Réponses multi-agents complémentaires ---\n\
             Les agents suivants ont chacun un tour déjà programmé pour ce même message : {aliases}. \
             Les réponses déjà terminées sont visibles dans le contexte. Réponds directement à \
             l'utilisateur et complète-les utilement sans lancer de débat automatique. Tu peux \
             répondre brièvement à une demande concrète d'un autre agent, mais ne le relance pas.\n\n"
        ),
        "es" => format!(
            "--- Respuestas multiagente complementarias ---\n\
             Cada uno de estos agentes ya tiene un turno programado para el mismo mensaje: {aliases}. \
             Las respuestas ya terminadas aparecen en el contexto. Responde directamente al usuario \
             y complétalas de forma útil sin iniciar un debate automático. Puedes responder brevemente \
             a una petición concreta de otro agente, pero no vuelvas a iniciarlo.\n\n"
        ),
        "zh" => format!(
            "--- 多智能体互补回复 ---\n\
             以下智能体都已为同一条消息安排了一次回复：{aliases}。已完成的回复会显示在上下文中。\
             请直接回复用户并提供有价值的补充，不要自动展开辩论。你可以简短回应另一个智能体的\
             具体请求，但不要再次启动它。\n\n"
        ),
        _ => format!(
            "--- Complementary multi-agent replies ---\n\
             Each of these agents already has one turn scheduled for the same message: {aliases}. \
             Completed replies are visible in the context. Reply directly to the user and add useful \
             complementary points without starting an automatic debate. You may briefly answer a \
             concrete request from another agent, but do not launch it again.\n\n"
        ),
    }
}

async fn make_agent_stream_inner(
    state: AppState,
    discussion_id: String,
    agent_override: Option<AgentType>,
    tier_override: Option<crate::models::ModelTier>,
    dispatch_job_id: Option<String>,
    mut initial_event: Option<Event>,
    mut completion_tx: Option<tokio::sync::oneshot::Sender<AgentExecutionOutcome>>,
) -> Sse<SseStream> {
    let tracked_dispatch = dispatch_job_id.is_some();
    let dispatch_metadata = if let Some(job_id) = dispatch_job_id.as_ref() {
        let job_id = job_id.clone();
        state
            .db
            .with_conn(move |conn| crate::db::agent_dispatch::get(conn, &job_id))
            .await
            .ok()
            .flatten()
            .map(|job| (job.trigger_message_id, job.group_id, job.connection_id))
    } else {
        let did = discussion_id.clone();
        state
            .db
            .with_read_conn(move |conn| {
                crate::db::discussions::latest_main_user_message_id(conn, &did)
            })
            .await
            .ok()
            .flatten()
            .map(|trigger| (trigger, None, None))
    };
    let dispatch_trigger_message_id = dispatch_metadata
        .as_ref()
        .map(|(trigger, _, _)| trigger.clone());
    let dispatch_group_id = dispatch_metadata
        .as_ref()
        .and_then(|(_, group, _)| group.clone());
    let dispatch_connection_id = dispatch_metadata.and_then(|(_, _, connection)| connection);
    // 0.8.5 — capture the agent-run start wallclock. The delta between
    // this and the moment we commit the Agent message gives us the
    // real reply duration in milliseconds (excludes user typing time).
    // Stored on `messages.duration_ms` for the QP-metrics aggregator.
    let run_started_at: std::time::Instant = std::time::Instant::now();

    // Extract info from DB
    let disc = state
        .db
        .with_conn({
            let did = discussion_id.clone();
            move |conn| crate::db::discussions::get_discussion(conn, &did)
        })
        .await
        .ok()
        .flatten();

    if disc.is_none() {
        finish_tracked_preflight(&mut completion_tx);
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default()
                    .event("error")
                    .data("{\"error\":\"Discussion not found\"}"),
            )
        }));
        return Sse::new(prepend_initial_event(stream, initial_event.take()));
    }

    let disc = match disc {
        Some(d) => d,
        None => {
            finish_tracked_preflight(&mut completion_tx);
            let stream: SseStream = Box::pin(futures::stream::once(async {
                Ok::<_, Infallible>(
                    Event::default()
                        .event("error")
                        .data(serde_json::json!({ "error": "Discussion not found" }).to_string()),
                )
            }));
            return Sse::new(prepend_initial_event(stream, initial_event.take()));
        }
    };
    let agent_type = agent_override.unwrap_or_else(|| disc.agent.clone());
    let external_connection = if let Some(connection_id) = dispatch_connection_id.as_ref() {
        let lookup_id = connection_id.clone();
        match state
            .db
            .with_read_conn(move |conn| crate::db::external_api_connections::get(conn, &lookup_id))
            .await
        {
            Ok(Some(connection))
                if crate::db::external_api_connections::target_for_connection(&connection)
                    .agent_type
                    == agent_type =>
            {
                Some(connection)
            }
            Ok(Some(_)) => {
                finish_tracked_preflight(&mut completion_tx);
                let stream: SseStream = Box::pin(futures::stream::once(async move {
                    Ok::<_, Infallible>(Event::default().event("error").data(
                        serde_json::json!({
                            "error": "The selected external API connection no longer matches this agent target. Select it again."
                        })
                        .to_string(),
                    ))
                }));
                return Sse::new(prepend_initial_event(stream, initial_event.take()));
            }
            _ => {
                finish_tracked_preflight(&mut completion_tx);
                let stream: SseStream = Box::pin(futures::stream::once(async move {
                    Ok::<_, Infallible>(Event::default().event("error").data(
                        serde_json::json!({
                            "error": "The selected external API connection no longer exists. Recreate or select it again in Settings → Agents."
                        })
                        .to_string(),
                    ))
                }));
                return Sse::new(prepend_initial_event(stream, initial_event.take()));
            }
        }
    } else {
        None
    };
    let mut attached_handoff_agents = vec![disc.agent.clone()];
    for participant in &disc.participants {
        if !attached_handoff_agents.contains(participant) {
            attached_handoff_agents.push(participant.clone());
        }
    }
    attached_handoff_agents.retain(|agent| agent != &agent_type && agent_alias(agent).is_some());
    let auth_status = {
        let config = state.config.read().await;
        crate::agents::agent_auth_status(&agent_type, &config)
    };
    if auth_status.ready == Some(false) {
        let persisted_error =
            auth_required_system_message(&agent_type, &disc.language, auth_status.setup_command);
        let safe_error = persisted_error.content.clone();
        let did = discussion_id.clone();
        if let Err(db_error) = state
            .db
            .with_conn(move |conn| {
                let inserted = crate::db::discussions::insert_message(conn, &did, &persisted_error);
                let cleared = clear_awaiting_after_terminal(conn, &did, tracked_dispatch);
                inserted.and(cleared)
            })
            .await
        {
            tracing::error!("Failed to persist agent auth preflight error: {db_error}");
        }
        let stream: SseStream = Box::pin(futures::stream::once(async move {
            Ok::<_, Infallible>(
                Event::default()
                    .event("error")
                    .data(serde_json::json!({ "error": safe_error }).to_string()),
            )
        }));
        finish_tracked_preflight(&mut completion_tx);
        return Sse::new(prepend_initial_event(stream, initial_event.take()));
    }
    let disc_tier = tier_override.unwrap_or(disc.tier);
    // 0.8.10 — explicit per-discussion model (e.g. inherited from a launching
    // Quick Prompt) wins over the tier; None → resolve from tier as before.
    let disc_model = if tier_override.is_some() {
        None
    } else {
        disc.model.clone()
    };
    let disc_model = disc_model.or_else(|| {
        external_connection.as_ref().and_then(|connection| {
            let selected = match disc_tier {
                crate::models::ModelTier::Economy => &connection.economy_model,
                crate::models::ModelTier::Default => &connection.default_model,
                crate::models::ModelTier::Reasoning => &connection.reasoning_model,
            };
            selected
                .clone()
                .or_else(|| connection.default_model.clone())
        })
    });
    let skill_ids = disc.skill_ids.clone();
    let directive_ids = disc.directive_ids.clone();
    let profile_ids = disc.profile_ids.clone();
    let tool_free_judge = {
        let did = discussion_id.clone();
        state
            .db
            .with_read_conn(move |conn| crate::db::compare::is_judge_discussion(conn, &did))
            .await
            .unwrap_or(false)
    };
    let native_http_tools = match native_http_tools_for_discussion(
        &state,
        &discussion_id,
        &agent_type,
        dispatch_trigger_message_id.clone(),
        dispatch_job_id.clone(),
        tool_free_judge,
    )
    .await
    {
        Ok(tools) => tools,
        Err(error) => {
            tracing::error!(
                discussion_id,
                agent = ?agent_type,
                "Unable to resolve native HTTP tool scope: {error}"
            );
            finish_tracked_preflight(&mut completion_tx);
            let stream: SseStream = Box::pin(futures::stream::once(async move {
                Ok::<_, Infallible>(
                    Event::default().event("error").data(
                        serde_json::json!({
                            "error": "Unable to verify the agent execution scope; the run was not started"
                        })
                        .to_string(),
                    ),
                )
            }));
            return Sse::new(prepend_initial_event(stream, initial_event.take()));
        }
    };
    let cli_task_worker_context = match cli_task_worker_context(
        &state,
        &discussion_id,
        &agent_type,
        dispatch_job_id.as_deref(),
    )
    .await
    {
        Ok(context) => context,
        Err(error) => {
            tracing::error!(
                discussion_id,
                agent = ?agent_type,
                "Unable to resolve CLI task-worker delivery scope: {error}"
            );
            finish_tracked_preflight(&mut completion_tx);
            let stream: SseStream = Box::pin(futures::stream::once(async move {
                Ok::<_, Infallible>(
                    Event::default().event("error").data(
                        serde_json::json!({
                            "error": "Unable to verify the agent execution scope; the run was not started"
                        })
                        .to_string(),
                    ),
                )
            }));
            return Sse::new(prepend_initial_event(stream, initial_event.take()));
        }
    };
    let mut workspace_path = if tool_free_judge {
        None
    } else {
        disc.workspace_path.clone()
    };
    let project_path = if tool_free_judge {
        String::new()
    } else if let Some(ref pid) = disc.project_id {
        let pid = pid.clone();
        state
            .db
            .with_conn(move |conn| {
                let p = crate::db::projects::get_project(conn, &pid)?;
                Ok(p.map(|p| p.path).unwrap_or_default())
            })
            .await
            .unwrap_or_default()
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
                state
                    .db
                    .with_conn(move |conn| {
                        let p = crate::db::projects::get_project(conn, &pid)?;
                        Ok(p.map(|p| p.name).unwrap_or_default())
                    })
                    .await
                    .unwrap_or_default()
            } else {
                String::new()
            };

            match crate::core::worktree::reattach_worktree(repo_path, &pname, &disc.title, branch) {
                Ok(info) => {
                    let did = disc.id.clone();
                    let wp = info.path.clone();
                    let wb = info.branch.clone();
                    let _ = state
                        .db
                        .with_conn(move |conn| {
                            crate::db::discussions::update_discussion_workspace(
                                conn, &did, &wp, &wb,
                            )
                        })
                        .await;
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
                    // Same terminal handling as the agent-start-failed arm.
                    // Persist the error and clear the enqueue-time awaiting
                    // marker. Durable dispatch settlement owns batch progress
                    // atomically with the job's terminal state.
                    let persisted_err = DiscussionMessage {
                        recovered_partial: false,
                        session_tokens_at_message: None,
                        author_cli_ordinal: None,
                        model: None,
                        lint_report: None,
                        id: Uuid::new_v4().to_string(),
                        role: MessageRole::System,
                        channel: MessageChannel::Main,
                        content: format!("Erreur: {}", err_msg),
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
                        target_agent: None,
                        reply_to_message_id: dispatch_trigger_message_id.clone(),
                    };
                    let did = discussion_id.clone();
                    if let Err(db_err) = state
                        .db
                        .with_conn(move |conn| {
                            // Both ops even if the insert fails.
                            let inserted =
                                crate::db::discussions::insert_message(conn, &did, &persisted_err);
                            let cleared =
                                clear_awaiting_after_terminal(conn, &did, tracked_dispatch);
                            inserted.and(cleared)
                        })
                        .await
                    {
                        tracing::error!("Failed to persist re-lock preflight error: {db_err}");
                    }
                    let stream: SseStream = Box::pin(futures::stream::once(async move {
                        Ok::<_, Infallible>(
                            Event::default()
                                .event("error")
                                .data(serde_json::json!({ "error": err_msg }).to_string()),
                        )
                    }));
                    finish_tracked_preflight(&mut completion_tx);
                    return Sse::new(prepend_initial_event(stream, initial_event.take()));
                }
            }
        }
    }

    // Validation discussions are a second agent boundary over the audit
    // artifacts. Detect them through the durable run link, never a mutable or
    // localized title, and sanitize before prompt construction or spawn.
    let validation_redaction_scope = if let Some(ref project_id) = disc.project_id {
        let did = disc.id.clone();
        let pid = project_id.clone();
        match state
            .db
            .with_conn(move |conn| {
                crate::db::audit_runs::validation_discussion_belongs_to_project(conn, &did, &pid)
            })
            .await
        {
            Ok(true) => {
                let root = crate::core::scanner::resolve_host_path(
                    workspace_path.as_deref().unwrap_or(&project_path),
                );
                let targets: Vec<String> =
                    crate::api::audit::assemble_chained_steps(crate::models::AuditKind::Full)
                        .into_iter()
                        .map(|step| step.target_file.to_string())
                        .collect();
                if let Err(error) = crate::api::audit::redact_artifacts::sanitize_all(
                    &root,
                    &targets,
                    "validation-pre-agent",
                ) {
                    tracing::error!(target: "kronn::invariant", disc_id = %discussion_id,
                        error = %error, "validation artifact redaction failed before agent spawn");
                    let safe_error = "Validation bloquée : impossible de garantir la suppression des secrets dans les artefacts d’audit.";
                    let persisted_err = DiscussionMessage {
                        recovered_partial: false,
                        session_tokens_at_message: None,
                        author_cli_ordinal: None,
                        model: None,
                        lint_report: None,
                        id: Uuid::new_v4().to_string(),
                        role: MessageRole::System,
                        channel: MessageChannel::Main,
                        content: safe_error.to_string(),
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
                        target_agent: None,
                        reply_to_message_id: dispatch_trigger_message_id.clone(),
                    };
                    let did = discussion_id.clone();
                    if let Err(db_error) = state
                        .db
                        .with_conn(move |conn| {
                            let inserted =
                                crate::db::discussions::insert_message(conn, &did, &persisted_err);
                            let cleared =
                                clear_awaiting_after_terminal(conn, &did, tracked_dispatch);
                            inserted.and(cleared)
                        })
                        .await
                    {
                        tracing::error!(
                            "Failed to persist validation redaction preflight error: {db_error}"
                        );
                    }
                    let stream: SseStream = Box::pin(futures::stream::once(async move {
                        Ok::<_, Infallible>(
                            Event::default()
                                .event("error")
                                .data(serde_json::json!({ "error": safe_error }).to_string()),
                        )
                    }));
                    finish_tracked_preflight(&mut completion_tx);
                    return Sse::new(prepend_initial_event(stream, initial_event.take()));
                }
                Some((root, targets))
            }
            Ok(false) => None,
            Err(error) => {
                tracing::error!(target: "kronn::invariant", disc_id = %discussion_id,
                    error = %error, "could not resolve durable validation-discussion link");
                let safe_error = "Impossible de vérifier le périmètre de cette discussion avant le lancement de l’agent.";
                let persisted_err = DiscussionMessage {
                    recovered_partial: false,
                    session_tokens_at_message: None,
                    author_cli_ordinal: None,
                    model: None,
                    lint_report: None,
                    id: Uuid::new_v4().to_string(),
                    role: MessageRole::System,
                    channel: MessageChannel::Main,
                    content: safe_error.to_string(),
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
                    target_agent: None,
                    reply_to_message_id: dispatch_trigger_message_id.clone(),
                };
                let did = discussion_id.clone();
                if let Err(db_error) = state
                    .db
                    .with_conn(move |conn| {
                        let inserted =
                            crate::db::discussions::insert_message(conn, &did, &persisted_err);
                        let cleared = clear_awaiting_after_terminal(conn, &did, tracked_dispatch);
                        inserted.and(cleared)
                    })
                    .await
                {
                    tracing::error!(
                        "Failed to persist validation-link preflight error: {db_error}"
                    );
                }
                let stream: SseStream = Box::pin(futures::stream::once(async move {
                    Ok::<_, Infallible>(
                        Event::default()
                            .event("error")
                            .data(serde_json::json!({ "error": safe_error }).to_string()),
                    )
                }));
                finish_tracked_preflight(&mut completion_tx);
                return Sse::new(prepend_initial_event(stream, initial_event.take()));
            }
        }
    } else {
        None
    };

    // For general discussions (no project), write .mcp.json + build MCP context.
    // For project discussions, also ensure the .mcp.json is fresh on disk
    // (covers the case where MCPs were added/toggled since the last sync).
    let global_mcp_context = if tool_free_judge {
        // The verdict must be reproducible from the captured payload alone.
        // `Some("")` explicitly overrides both project and global MCP config.
        Some(String::new())
    } else if project_path.is_empty() {
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
            .and_then(|t| {
                disc.project_id
                    .as_ref()
                    .map(|pid| t.progress.contains_key(pid))
            })
            .unwrap_or(false);
        if !audit_running {
            if let Some(ref pid) = disc.project_id {
                let secret = {
                    let cfg = state.config.read().await;
                    cfg.encryption_secret.clone()
                };
                if let Some(secret) = secret {
                    let pid = pid.clone();
                    let _ = state
                        .db
                        .with_conn(move |conn| {
                            crate::core::mcp_scanner::sync_project_with_report(conn, &pid, &secret);
                            Ok::<_, anyhow::Error>(())
                        })
                        .await;
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
                .and_then(|v| {
                    v.get("mcpServers")
                        .and_then(|m| m.as_object())
                        .map(|m| m.len())
                })
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
        let plugin_block = {
            let secret = {
                let cfg = state.config.read().await;
                cfg.encryption_secret.clone()
            };
            match secret {
                Some(secret) => {
                    let project_id = disc.project_id.clone();
                    let secret_c = secret.clone();
                    // Decrypt configs only to resolve NON-SECRET values used
                    // by the broker metadata block (tenant id, workspace
                    // slug, …). Auth values stay in this backend process and
                    // are never rendered into agent context.
                    let (api_plugins, preference_plugins) = state
                        .db
                        .with_conn(move |conn| {
                            let api_plugins =
                                crate::core::mcp_scanner::collect_active_api_plugins_for_scope(
                                    conn,
                                    project_id.as_deref(),
                                    &secret_c,
                                )?;
                            let preference_plugins =
                                crate::core::mcp_scanner::collect_active_plugin_preferences(
                                    conn,
                                    project_id.as_deref(),
                                )?;
                            Ok::<_, anyhow::Error>((api_plugins, preference_plugins))
                        })
                        .await
                        .unwrap_or_default();

                    // Token exchange belongs exclusively to `api_call` at
                    // request time. Resolving it here used to put the bearer
                    // in `--append-system-prompt`, argv and logs even when the
                    // agent never called the API.
                    let api_block = crate::core::mcp_scanner::build_api_context_block(&api_plugins);
                    let preference_block =
                        crate::core::mcp_scanner::build_plugin_invocation_preferences(
                            &preference_plugins,
                        );
                    format!("{api_block}{preference_block}")
                }
                None => String::new(),
            }
        };

        if plugin_block.is_empty() {
            // No API metadata or multi-interface preference is active — let
            // runner.rs fall back to reading MCP contexts from disk.
            None
        } else {
            // We must pre-combine the disk-read MCP context with the generated
            // plugin block, since
            // `mcp_context_override = Some(...)` short-circuits the
            // disk read in runner.rs.
            let disk_ctx = crate::core::mcp_scanner::read_all_mcp_contexts(&project_path);
            let combined = if disk_ctx.is_empty() {
                plugin_block
            } else {
                format!("{}\n{}", disk_ctx, plugin_block)
            };
            Some(combined)
        }
    };

    // Load context files for prompt injection
    let context_files_prompt = {
        let did = discussion_id.clone();
        let entries = state
            .db
            .with_conn(move |conn| {
                crate::db::discussions::get_context_files_for_prompt(conn, &did)
                    .map_err(|e| anyhow::anyhow!(e))
            })
            .await
            .unwrap_or_default();
        crate::core::context_files::build_context_prompt(&entries)
    };

    // Inject user bio (first exchange only) + global context (always).
    let (handoffs_disabled, handoffs_unlimited) = {
        let did = discussion_id.clone();
        state
            .db
            .with_read_conn(move |conn| {
                crate::db::discussions::get_disc_agent_handoff_policy(conn, &did)
            })
            .await
            .ok()
            .flatten()
            .unwrap_or((false, false))
    };
    let (
        tokens,
        full_access,
        model_tiers_config,
        http_endpoints,
        user_bio,
        global_context,
        handoffs_enabled,
        handoff_paid_limit,
        handoff_blocked_agents,
    ) = {
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
                _ => config
                    .server
                    .global_context
                    .clone()
                    .filter(|g| !g.trim().is_empty()),
            }
        };
        (
            config.tokens.clone(),
            fa,
            config.agents.model_tiers.clone(),
            crate::models::setup::HttpEndpoints::from_agents(&config.agents),
            bio,
            gc,
            config.server.agent_handoffs_enabled,
            (!config.server.agent_handoff_paid_unlimited && !handoffs_unlimited)
                .then_some(config.server.agent_handoff_paid_limit.min(5)),
            config.server.agent_handoff_blocked_agents.clone(),
        )
    };
    let external_http_runtime = external_connection.as_ref().and_then(|connection| {
        connection
            .endpoint
            .as_ref()
            .map(|endpoint| runner::ExternalHttpRuntime {
                display_name: connection.display_name.clone(),
                mention_alias: connection.mention_alias.clone(),
                endpoint: endpoint.clone(),
                api_key: tokens
                    .active_key_for(&connection.credential_slug)
                    .filter(|key| !key.trim().is_empty())
                    .map(str::to_string),
            })
    });

    // Build the context preamble: user bio (first exchange) + global context (always)
    let context_files_prompt = {
        let mut preamble = String::new();
        if let Some(ref bio) = user_bio {
            let pseudo = disc
                .messages
                .first()
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
    let companion_context =
        crate::api::projects::compute_companion_context(&state, disc.project_id.as_deref()).await;
    let context_files_prompt = if companion_context.is_empty() {
        context_files_prompt
    } else {
        format!("{}{}", context_files_prompt, companion_context)
    };

    // Planning stays pull-based: inject no task body, list or description.
    // Only signal that linked state changed since this discussion's last
    // agent reply; the agent can then call plan_get/task_changes if relevant.
    let planning_change_count = {
        let planning_disc_id = discussion_id.clone();
        state
            .db
            .with_read_conn(move |conn| {
                crate::db::planning::change_count_since_last_agent(conn, &planning_disc_id)
            })
            .await
            .unwrap_or(0)
    };
    let context_files_prompt = if planning_change_count == 0 {
        context_files_prompt
    } else {
        let notice = match disc.language.as_str() {
            "fr" => format!(
                "--- Plan de discussion modifié ({planning_change_count} changement(s)) ---\n\
                 Appelle `plan_get` seulement si ce plan est utile à la demande actuelle.\n\n"
            ),
            "es" => format!(
                "--- Plan de conversación modificado ({planning_change_count} cambio(s)) ---\n\
                 Llama a `plan_get` solo si el plan es útil para la solicitud actual.\n\n"
            ),
            _ => format!(
                "--- Discussion plan changed ({planning_change_count} change(s)) ---\n\
                 Call `plan_get` only if the plan is relevant to the current request.\n\n"
            ),
        };
        format!("{context_files_prompt}{notice}")
    };
    let handoff_paid_remaining = if handoffs_enabled && !handoffs_disabled {
        match handoff_paid_limit {
            Some(limit) => {
                let did = discussion_id.clone();
                let parent_id = dispatch_trigger_message_id.clone();
                let spent = state
                    .db
                    .with_read_conn(move |conn| {
                        crate::db::discussions::agent_handoff_paid_count_for_reply(
                            conn,
                            &did,
                            parent_id.as_deref(),
                        )
                    })
                    .await
                    .unwrap_or(0);
                Some(limit.saturating_sub(spent))
            }
            None => None,
        }
    } else {
        Some(0)
    };
    let root_turn_scheduled_agents = {
        let did = discussion_id.clone();
        let parent_id = dispatch_trigger_message_id.clone();
        state
            .db
            .with_read_conn(move |conn| {
                crate::db::discussions::native_agents_scheduled_for_root_turn(
                    conn,
                    &did,
                    parent_id.as_deref(),
                )
            })
            .await
            .unwrap_or_default()
    };
    let sibling_aliases = root_turn_scheduled_agents
        .iter()
        .filter(|agent| **agent != agent_type)
        .filter_map(agent_alias)
        .collect::<Vec<_>>()
        .join(", ");
    // These agents already own a dispatch for the same root User turn. Do not
    // advertise them as handoff targets to this runner, and explain why so the
    // generated answer does not wait for a redundant acknowledgement.
    attached_handoff_agents.retain(|agent| !root_turn_scheduled_agents.contains(agent));
    let sibling_notice = independent_sibling_notice(&disc.language, &sibling_aliases);
    let context_files_prompt = format!("{context_files_prompt}{sibling_notice}");
    let ollama_handoff_available = attached_handoff_agents.iter().any(|agent| {
        *agent == AgentType::Ollama
            && agent_handoff_target_is_allowed(agent, &handoff_blocked_agents)
    });
    let attached_aliases = attached_handoff_agents
        .iter()
        .filter(|agent| agent_handoff_target_is_allowed(agent, &handoff_blocked_agents))
        .filter_map(agent_alias)
        .collect::<Vec<_>>()
        .join(", ");
    let context_files_prompt = if handoffs_enabled
        && !handoffs_disabled
        && !attached_aliases.is_empty()
    {
        let budget = agent_handoff_budget_instruction(
            &disc.language,
            handoff_paid_remaining,
            ollama_handoff_available,
        );
        let notice = match disc.language.as_str() {
            "fr" => format!(
                "--- Agents autorisés à travailler ensemble ---\n\
                 Pour demander l'aide d'un autre agent, adresse-lui une demande directe dans ta réponse finale avec l'un de ces alias : {attached_aliases}, puis ajoute `<!-- kronn:handoff @alias -->` en remplaçant `@alias` par sa vraie valeur. {budget} Seul ce marqueur lance l'agent ; une mention normale reste informative. Le marqueur sera retiré avant l'enregistrement du message.\n\n"
            ),
            "es" => format!(
                "--- Agentes autorizados a trabajar juntos ---\n\
                 Para pedir ayuda a otro agente, dirígele una petición directa en tu respuesta final con uno de estos alias: {attached_aliases}, y añade `<!-- kronn:handoff @alias -->` sustituyendo `@alias` por su valor real. {budget} Solo este marcador inicia al agente; una mención normal es informativa. El marcador se elimina antes de guardar el mensaje.\n\n"
            ),
            "zh" => format!(
                "--- 允许智能体协同工作 ---\n\
                 如需向另一个智能体求助，请在最终回复中使用以下别名直接提出请求：{attached_aliases}，并添加 `<!-- kronn:handoff @alias -->`，将 `@alias` 替换为真实别名。{budget} 只有此标记会启动智能体；普通提及仅用于说明。保存消息前会移除此标记。\n\n"
            ),
            _ => format!(
                "--- Agents allowed to work together ---\n\
                 To ask another agent for help, address it with a direct request in your final reply using one of these aliases: {attached_aliases}, then add `<!-- kronn:handoff @alias -->` with the real alias substituted. {budget} Only this marker launches the agent; a normal mention is informational. The marker is removed before the message is stored.\n\n"
            ),
        };
        format!("{context_files_prompt}{notice}")
    } else {
        context_files_prompt
    };

    // Estimate extra_context size so build_agent_prompt can respect the agent's budget.
    // This mirrors what runner::start_agent_with_config will build.
    let extra_context_len = estimate_extra_context_len(
        &skill_ids,
        &directive_ids,
        &profile_ids,
        &project_path,
        global_mcp_context.as_deref(),
        &agent_type,
    ) + context_files_prompt.len();
    let mut prompt_disc =
        discussion_at_dispatch_trigger(&disc, dispatch_trigger_message_id.as_deref());
    // QP values are never persisted in messages. Hydrate only this temporary
    // dispatch copy from the immutable encrypted snapshot.
    // Lineage is intentionally not part of the public Discussion model, so
    // read the durable QP marker here. It distinguishes ordinary discussion
    // dispatch (which has no snapshot) from a QP launch whose snapshot must
    // exist and decrypt before an agent can start.
    let qp_launch = {
        let did = disc.id.clone();
        state
            .db
            .with_read_conn(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT originating_qp_id IS NOT NULL FROM discussions WHERE id=?1",
                        [did],
                        |row| row.get::<_, bool>(0),
                    )
                    .optional()?
                    .unwrap_or(false))
            })
            .await
            .unwrap_or(false)
    };
    if qp_launch {
        let secret = match state.config.read().await.encryption_secret.clone() {
            Some(secret) => secret,
            None => {
                finish_tracked_preflight(&mut completion_tx);
                let stream: SseStream = Box::pin(futures::stream::once(async {
                    Ok::<_, Infallible>(Event::default().event("error").data(
                        serde_json::json!({"error": "Quick Prompt variable snapshot key unavailable"}).to_string(),
                    ))
                }));
                return Sse::new(prepend_initial_event(stream, initial_event.take()));
            }
        };
        let key = match crate::core::crypto::parse_secret(&secret) {
            Ok(key) => key,
            Err(_) => {
                finish_tracked_preflight(&mut completion_tx);
                let stream: SseStream = Box::pin(futures::stream::once(async {
                    Ok::<_, Infallible>(Event::default().event("error").data(
                        serde_json::json!({"error": "Quick Prompt variable snapshot key unavailable"}).to_string(),
                    ))
                }));
                return Sse::new(prepend_initial_event(stream, initial_event.take()));
            }
        };
        let disc_id = disc.id.clone();
        let workflow_run_id = disc.workflow_run_id.clone();
        let values = state
            .db
            .with_conn(move |conn| {
                for (kind, id) in [
                    ("quick_prompt", Some(disc_id.as_str())),
                    ("quick_prompt_batch_item", Some(disc_id.as_str())),
                    ("quick_prompt_compare", workflow_run_id.as_deref()),
                ] {
                    if let Some(id) = id {
                        if let Some(values) = crate::db::execution_variable_snapshots::load_values(
                            conn,
                            kind,
                            id,
                            &key,
                            chrono::Utc::now(),
                        )? {
                            return Ok(Some(values));
                        }
                    }
                }
                // A BatchQuickPrompt child points at its child batch
                // run. Environment variables are resolved once on the
                // parent Workflow run, so follow that durable link and
                // reuse the immutable parent snapshot on every child
                // dispatch/resume.
                if let Some(batch_run_id) = workflow_run_id.as_deref() {
                    let parent_id: Option<String> = conn
                        .query_row(
                            "SELECT parent_run_id FROM workflow_runs WHERE id=?1",
                            [batch_run_id],
                            |row| row.get(0),
                        )
                        .optional()?;
                    if let Some(parent_id) = parent_id {
                        if let Some(values) = crate::db::execution_variable_snapshots::load_values(
                            conn,
                            "workflow",
                            &parent_id,
                            &key,
                            chrono::Utc::now(),
                        )? {
                            return Ok(Some(values));
                        }
                    }
                }
                Ok::<_, anyhow::Error>(None)
            })
            .await;
        let values = match values {
            Ok(Some(values)) => values,
            Ok(None) | Err(_) => {
                // A QP dispatch may never fall through with placeholders: it
                // would turn a failed preflight or expired snapshot into an
                // agent side effect with incomplete input.
                finish_tracked_preflight(&mut completion_tx);
                let stream: SseStream = Box::pin(futures::stream::once(async {
                    Ok::<_, Infallible>(Event::default().event("error").data(
                        serde_json::json!({"error": "Quick Prompt variable snapshot unavailable or expired"}).to_string(),
                    ))
                }));
                return Sse::new(prepend_initial_event(stream, initial_event.take()));
            }
        };
        if let Some(first_message) = prompt_disc.messages.first_mut() {
            first_message.content = values
                .iter()
                .fold(first_message.content.clone(), |rendered, (name, value)| {
                    rendered.replace(&format!("{{{{{name}}}}}"), value)
                });
        }
    }
    let prompt = build_agent_prompt(&prompt_disc, &agent_type, extra_context_len);

    let auth_mode_str = auth_mode_for(&agent_type, &tokens);

    // KT-37 — resolve the concrete model this run will ATTEMPT, once, with the
    // same precedence the runner uses (per-disc/QP override → tier → provider
    // default). Reused by the terminal message, the mid-stream checkpoint, and
    // the spawn-error provenance so all three agree. `None` = provider-default
    // run with no --model flag.
    let attempted_model = runner::effective_model_flag(
        disc_model.as_deref(),
        &agent_type,
        disc_tier,
        Some(&model_tiers_config),
    );

    let runtime_target_id = external_connection
        .as_ref()
        .map(|connection| crate::db::model_catalog::http_runtime_target_id(&connection.id));
    if let Some(failure) = crate::core::model_catalog::preflight_check(
        &state.db,
        runtime_target_id.as_deref(),
        agent_type.clone(),
        disc_tier,
        disc_model.as_deref(),
        Some(&model_tiers_config),
    )
    .await
    {
        let payload = serde_json::json!({
            "error": "model_catalog_preflight_failed",
            "preflight_failure": failure,
        });
        finish_tracked_preflight(&mut completion_tx);
        let stream: SseStream = Box::pin(futures::stream::once(async move {
            Ok::<_, Infallible>(Event::default().event("error").data(payload.to_string()))
        }));
        return Sse::new(prepend_initial_event(stream, initial_event.take()));
    }

    let disc_id = discussion_id.clone();
    let disc_project_id = disc.project_id.clone();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentStreamEvent>(64);

    // An agent run WILL start past this point (every preflight
    // early-return is above): mark the disc as owed a reply so a restart
    // before the first durable trace is caught by the boot reconcile.
    // Setting it here (not in the callers) means a failed preflight never
    // leaves a stuck flag → no bogus interruption notice at the next boot.
    // Batch children are additionally marked at create_batch_run (their
    // pre-spawn queue lives in RAM); re-setting here is idempotent.
    // Cleared on delivery/error by the task's terminal paths. Best-effort.
    {
        let did_mark = disc_id.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| {
                crate::db::discussions::set_awaiting_agent(conn, &did_mark, true)
            })
            .await
        {
            tracing::warn!(
                "make_agent_stream: failed to mark awaiting_agent for {}: {}",
                disc_id,
                e
            );
        }
    }

    // Durable responses use their dispatch id as the cancellation key. This
    // lets the UI stop one queued/running reply without killing a sibling from
    // another turn in the same discussion. Legacy non-dispatch streams keep
    // the discussion id key used by the global Stop action.
    let cancel_key = dispatch_job_id.clone().unwrap_or_else(|| disc_id.clone());
    let cancel_guard = crate::CancelGuard::insert(&state.cancel_registry, cancel_key);
    let cancel_token = cancel_guard.token.clone();

    // Spawn background task — always saves to DB even if client disconnects
    let semaphore = state.agent_semaphore.clone();
    tokio::spawn(async move {
        // Keep the guard alive for the lifetime of this task. Dropping it at
        // the end of the move closure removes the token from the registry.
        let _cancel_guard = cancel_guard;
        // Only processes and inference running on this machine consume the
        // machine-wide pool. Remote HTTP providers own their capacity and are
        // admitted independently by the dispatch scheduler.
        let _permit = if crate::agents::runner::is_local_agent(&agent_type) {
            match semaphore.acquire_owned().await {
                Ok(p) => Some(p),
                Err(_) => {
                    let _ = tx
                        .send(AgentStreamEvent::Error {
                            data: serde_json::json!({ "error": "Server shutting down" }),
                        })
                        .await;
                    if let Some(sender) = completion_tx.take() {
                        let _ = sender.send(AgentExecutionOutcome::RuntimeUnavailable {
                            reason: "server_shutting_down".to_string(),
                        });
                    }
                    return;
                }
            }
        } else {
            None
        };

        // Durable execution boundary: `claimed_at` means the dispatcher owns
        // the job, while `agent_started_at` means queueing is over and a paid
        // or local provider call is about to begin. Persist it only after any
        // required local-capacity permit is held so capacity wait is measurable.
        if let Some(job_id) = dispatch_job_id.as_ref() {
            let started_id = job_id.clone();
            match state
                .db
                .with_conn(move |conn| {
                    crate::db::agent_dispatch::mark_agent_started(conn, &started_id)
                })
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    // Cancellation won after claim but before provider start.
                    // The completion observer sees the durable Cancelled row.
                    return;
                }
                Err(error) => {
                    tracing::error!(
                        dispatch_job_id = %job_id,
                        "Unable to persist agent start boundary: {error}"
                    );
                    if let Some(sender) = completion_tx.take() {
                        let _ = sender.send(AgentExecutionOutcome::RuntimeUnavailable {
                            reason: "agent_start_boundary_persist_failed".to_string(),
                        });
                    }
                    return;
                }
            }
        }

        if let Some(run_id) = dispatch_group_id.as_ref() {
            let _ = state
                .ws_broadcast
                .send(crate::models::WsMessage::BatchRunChildStarted {
                    run_id: run_id.clone(),
                    discussion_id: disc_id.clone(),
                });
        }

        let _ = tx.send(AgentStreamEvent::Start).await;
        let _ = tx
            .send(AgentStreamEvent::Meta {
                auth_mode: auth_mode_str.clone(),
            })
            .await;

        let mut tracked_execution_succeeded = false;
        // KT-405 — cloned out of the lock (never held across an await), so
        // an HTTP run can honour a persistent per-model context override.
        let (ollama_context_overrides, http_request_timeout) = {
            let cfg = state.config.read().await;
            (
                cfg.server.ollama_context_overrides.clone(),
                effective_global_timeout(
                    &agent_type,
                    cfg.server.agent_global_timeout_min,
                    cfg.server.local_agent_global_timeout_min,
                ),
            )
        };
        if let Some(job_id) = dispatch_job_id.as_ref() {
            let progress_id = job_id.clone();
            if let Err(error) = state
                .db
                .with_conn(move |conn| {
                    crate::db::agent_dispatch::mark_progress(
                        conn,
                        &progress_id,
                        "upstream_wait",
                        None,
                    )?;
                    Ok(())
                })
                .await
            {
                tracing::warn!(dispatch_job_id = %job_id, "Unable to persist provider-call boundary: {error}");
            }
        }

        match runner::start_agent_with_config(runner::AgentStartConfig {
            work_dir: workspace_path.as_deref(),
            full_access,
            skill_ids: &skill_ids,
            directive_ids: &directive_ids,
            profile_ids: &profile_ids,
            mcp_context_override: global_mcp_context.as_deref(),
            tier: disc_tier,
            model_tiers: Some(&model_tiers_config),
            http_endpoints: Some(&http_endpoints),
            external_http: external_http_runtime.as_ref(),
            ollama_context_overrides: Some(&ollama_context_overrides),
            http_request_timeout: Some(http_request_timeout),
            cancel_token: Some(cancel_token.clone()),
            model_override: disc_model.as_deref(),
            context_files_prompt: &context_files_prompt,
            // Forward to the agent process env so the kronn-internal MCP
            // bridge knows which discussion to introspect when called.
            discussion_id: Some(&discussion_id),
            acp_session_store: Some(runner::AcpSessionStore::new(
                state.db.clone(),
                discussion_id.clone(),
            )),
            task_worker_context: cli_task_worker_context.as_ref(),
            // Only HTTP agents consume this: CLI agents already reach the same
            // primitives through the stdio bridge, and handing them a second
            // channel would duplicate the surface for no gain.
            tools: native_http_tools,
            ..runner::AgentStartConfig::new(&agent_type, &project_path, &prompt, &tokens)
        })
        .await
        {
            Ok(mut process) => {
                let _runtime_guard = dispatch_job_id.as_ref().map(|job_id| {
                    crate::AgentRuntimeGuard::insert(&state.agent_runtime_registry, job_id.clone())
                });
                let mut full_response = String::new();
                let mut stream_json_tokens: u64 = 0;
                let mut stream_json_cost: Option<f64> = None;
                let mut stream_json_failure: Option<runner::StreamJsonFailure> = None;
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
                let stall_timeout_min = {
                    let cfg = state.config.read().await;
                    if cfg.server.agent_stall_timeout_min > 0 {
                        cfg.server.agent_stall_timeout_min
                    } else {
                        DEFAULT_STALL_TIMEOUT_MIN
                    }
                };
                let global_timeout = http_request_timeout;
                // KT-403 — no hidden Ollama multiplier. The effective value is
                // the explicit local-agent budget shown in Settings (240 min by
                // default), so the UI and the runtime report the same policy.
                let global_deadline = tokio::time::Instant::now() + global_timeout;

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
                // KT-37 — carry the agent + attempted model into every checkpoint
                // so a restart-time recovery rebuilds the message with provenance.
                let checkpoint_agent = agent_type.clone();
                let checkpoint_model = attempted_model.clone();
                let checkpoint_dispatch_id = dispatch_job_id.clone();
                let checkpoint_trigger_message_id = dispatch_trigger_message_id.clone();
                let checkpoint_connection_id = dispatch_connection_id.clone();
                // Await each best-effort flush. Serial writes cannot land out
                // of order or resurrect a stale draft after terminal cleanup.
                let do_checkpoint = |partial: String| {
                    let did = checkpoint_disc_id.clone();
                    let db = checkpoint_db.clone();
                    let agent = checkpoint_agent.clone();
                    let model = checkpoint_model.clone();
                    let dispatch_id = checkpoint_dispatch_id.clone();
                    let trigger_message_id = checkpoint_trigger_message_id.clone();
                    let connection_id = checkpoint_connection_id.clone();
                    async move {
                        if let Err(e) = db
                            .with_conn(move |conn| {
                                if let (Some(dispatch_id), Some(trigger_message_id)) =
                                    (dispatch_id.as_deref(), trigger_message_id.as_deref())
                                {
                                    crate::db::discussions::set_partial_response_for_dispatch(
                                        conn,
                                        &did,
                                        &partial,
                                        (&agent, model.as_deref()),
                                        dispatch_id,
                                        trigger_message_id,
                                        connection_id.as_deref(),
                                    )
                                    .map(|_| ())
                                } else {
                                    crate::db::discussions::set_partial_response(
                                        conn,
                                        &did,
                                        Some(&partial),
                                        Some((&agent, model.as_deref())),
                                    )
                                }
                            })
                            .await
                        {
                            tracing::warn!("partial_response checkpoint failed: {}", e);
                        }
                    }
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
                            Err(e) => {
                                tracing::warn!("stderr lock poisoned: {}", e);
                                break;
                            }
                        };
                        if lines.len() > last_len {
                            for line in &lines[last_len..] {
                                let trimmed = line.trim();
                                if !trimmed.is_empty() {
                                    let _ = log_tx
                                        .send(AgentStreamEvent::Log {
                                            text: trimmed.to_string(),
                                        })
                                        .await;
                                }
                            }
                            last_len = lines.len();
                        }
                        if log_tx.is_closed() {
                            break;
                        }
                    }
                });
                // Streaming agents use the configured stall; non-streaming
                // (Text) agents are silent until the end and rely on the global
                // deadline instead. See `effective_stall_timeout`.
                let stall_timeout = effective_stall_timeout(
                    is_stream_json,
                    Duration::from_secs(stall_timeout_min as u64 * 60),
                    NON_STREAMING_STALL_TIMEOUT,
                );
                let mut was_interrupted = false;
                let mut timeout_reason: Option<AgentTimeoutReason> = None;
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
                // Ollama streams raw token fragments — concatenate as-is; every
                // other text-mode agent streams LINES that need the '\n' put back.
                let raw_stream = process.raw_token_stream();
                while let Some(line) = tokio::select! {
                    line = process.next_line() => line,
                    _ = cancel_token.cancelled() => {
                        tracing::info!("Agent stream for disc {} cancelled by user", disc_id);
                        stopped_on_cancel = true;
                        None
                    }
                    _ = tokio::time::sleep_until(global_deadline) => {
                        tracing::warn!("Agent stream global timeout ({:?}) exceeded", global_timeout);
                        was_interrupted = true;
                        timeout_reason = Some(AgentTimeoutReason::Global(global_timeout));
                        None
                    }
                    _ = async {
                        tokio::time::sleep(stall_timeout).await
                    } => {
                        tracing::warn!("Agent stream stall timeout ({:?}) — no output", stall_timeout);
                        was_interrupted = true;
                        timeout_reason = Some(AgentTimeoutReason::Stall(stall_timeout));
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
                                if is_decoder_loop(
                                    &text,
                                    &mut last_text_delta,
                                    &mut repeat_delta_count,
                                ) {
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
                                    do_checkpoint(full_response.clone()).await;
                                    last_checkpoint = tokio::time::Instant::now();
                                    chunks_since_checkpoint = 0;
                                }
                                if !client_gone {
                                    let chunk = serde_json::json!({ "text": text });
                                    let _ = tx.send(AgentStreamEvent::Chunk { data: chunk }).await;
                                }
                                // Terminal-signal detection — see TERMINAL_SIGNALS doc.
                                if let Some(sig) = detect_terminal_signal(&full_response) {
                                    tracing::info!(
                                        "Terminal signal {} detected — stopping agent",
                                        sig
                                    );
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
                            runner::StreamJsonEvent::Usage {
                                input_tokens,
                                output_tokens,
                                cost_usd,
                            } => {
                                stream_json_tokens =
                                    stream_json_tokens.max(input_tokens + output_tokens);
                                if let Some(c) = cost_usd {
                                    stream_json_cost = Some(c);
                                }
                            }
                            runner::StreamJsonEvent::TerminalError(failure) => {
                                stream_json_tokens = stream_json_tokens
                                    .max(failure.input_tokens + failure.output_tokens);
                                if let Some(cost) = failure.cost_usd {
                                    stream_json_cost = Some(cost);
                                }
                                stream_json_failure = Some(failure);
                            }
                            runner::StreamJsonEvent::ToolStart(name) => {
                                if let Some(job_id) = dispatch_job_id.as_ref() {
                                    let progress_id = job_id.clone();
                                    let progress_tool = name.clone();
                                    if let Err(error) = state
                                        .db
                                        .with_conn(move |conn| {
                                            crate::db::agent_dispatch::mark_progress(
                                                conn,
                                                &progress_id,
                                                "tool_activity",
                                                Some(&progress_tool),
                                            )?;
                                            Ok(())
                                        })
                                        .await
                                    {
                                        tracing::warn!(dispatch_job_id = %job_id, "Unable to persist tool progress: {error}");
                                    }
                                }
                                current_tool = Some(name);
                                current_tool_input.clear();
                            }
                            runner::StreamJsonEvent::ToolInputDelta(partial) => {
                                current_tool_input.push_str(&partial);
                            }
                            runner::StreamJsonEvent::ToolEnd => {
                                if let Some(ref tool) = current_tool {
                                    let log = crate::api::disc_git::format_tool_log(
                                        tool,
                                        &current_tool_input,
                                    );
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
                                        ToolRecord::Native(record) => {
                                            native_tool_calls.push(record)
                                        }
                                    }
                                }
                                current_tool = None;
                                current_tool_input.clear();
                                if let Some(job_id) = dispatch_job_id.as_ref() {
                                    let progress_id = job_id.clone();
                                    if let Err(error) = state
                                        .db
                                        .with_conn(move |conn| {
                                            crate::db::agent_dispatch::mark_progress(
                                                conn,
                                                &progress_id,
                                                "upstream_wait",
                                                Some("tool_completed"),
                                            )?;
                                            Ok(())
                                        })
                                        .await
                                    {
                                        tracing::warn!(dispatch_job_id = %job_id, "Unable to persist post-tool progress: {error}");
                                    }
                                }
                            }
                            runner::StreamJsonEvent::Skip => {}
                        }
                    } else {
                        if !raw_stream && !full_response.is_empty() {
                            full_response.push('\n');
                        }
                        full_response.push_str(&line);
                        chunks_since_checkpoint += 1;
                        if chunks_since_checkpoint >= CHECKPOINT_CHUNKS
                            || last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL
                        {
                            do_checkpoint(full_response.clone()).await;
                            last_checkpoint = tokio::time::Instant::now();
                            chunks_since_checkpoint = 0;
                        }

                        if !client_gone {
                            let text_with_nl = if !raw_stream && full_response.len() > line.len() {
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
                if was_interrupted
                    || stopped_on_signal.is_some()
                    || stopped_on_size
                    || stopped_on_cancel
                    || stopped_on_loop
                {
                    process.kill().await;
                }

                let status = process.child.wait().await;
                process.fix_ownership();
                let validation_redaction_error =
                    validation_redaction_scope
                        .as_ref()
                        .and_then(|(root, targets)| {
                            crate::api::audit::redact_artifacts::sanitize_all(
                                root,
                                targets,
                                "validation-post-agent",
                            )
                            .err()
                        });
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
                let mut success = if stopped_on_signal.is_some() {
                    true
                } else if stopped_on_cancel {
                    false
                } else {
                    !was_interrupted && status.map(|s| s.success()).unwrap_or(false)
                };
                // A structured failed `result` is authoritative even if a CLI
                // wrapper happens to return exit 0. Preserve its provider text
                // before the generic empty-output fallback can overwrite it.
                if stream_json_failure.is_some() {
                    success = false;
                }

                // A failed post-agent sweep invalidates every terminal signal.
                // Replace the response so VALIDATION_COMPLETE cannot be
                // persisted, archived, or accepted by downstream UI logic.
                let validation_redaction_failed = validation_redaction_error.is_some();
                if let Some(error) = validation_redaction_error {
                    tracing::error!(target: "kronn::invariant", disc_id = %disc_id,
                        error = %error, "validation artifact redaction failed after agent exit");
                    success = false;
                    stopped_on_signal = None;
                    full_response = "Validation bloquée : la suppression des secrets dans les artefacts d’audit a échoué après l’exécution de l’agent. Aucun signal de validation n’a été accepté.".to_string();
                }

                let stderr_lines = process.captured_stderr_flushed().await;
                // `ollama_tokens:prompt:eval` is an internal accounting marker the
                // token parser reads out of stderr. It has no meaning for a reader,
                // and it leaked verbatim into the failure bubble (seen in the room:
                // "ollama_tokens:9019:14" printed twice above a real error). Drop it
                // here, at the one place stderr becomes user-facing, rather than
                // renaming the marker and breaking the parser that consumes it.
                let stderr_text = stderr_lines
                    .iter()
                    .filter(|line| {
                        let line = line.trim_start();
                        !line.starts_with("ollama_tokens:")
                            && !line.starts_with("kronn_http_turn:")
                            && !line.starts_with("kronn_http_tool_exec:")
                            && !line.starts_with("[provider-retry:")
                    })
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                // Provider retries are operational facts, not part of the
                // model's answer. Keep them out of the failure body/QP result,
                // but preserve them durably as one compact System trace below.
                let provider_retry_trace = stderr_lines
                    .iter()
                    .filter_map(|line| {
                        line.trim()
                            .strip_prefix("[provider-retry: ")
                            .and_then(|line| line.strip_suffix(']'))
                            .map(str::to_string)
                    })
                    .collect::<Vec<_>>();
                let http_turn_telemetry = runner::parse_http_turn_telemetry(&stderr_lines);
                if let Some(dispatch_id) = dispatch_job_id
                    .as_deref()
                    .filter(|_| !http_turn_telemetry.is_empty())
                {
                    let dispatch_id = dispatch_id.to_string();
                    let turns = http_turn_telemetry.clone();
                    if let Err(error) = state
                        .db
                        .with_conn(move |conn| {
                            crate::db::orchestration::record_http_turn_telemetry_for_dispatch(
                                conn,
                                &dispatch_id,
                                &turns,
                            )
                        })
                        .await
                    {
                        tracing::warn!(
                            target: "kronn::orchestration",
                            error = %error,
                            "failed to persist HTTP turn telemetry"
                        );
                    }
                }

                // A timeout must be explicit even when the agent produced no
                // stdout. Previously that exact case fell through to
                // `exit code: None`, hiding that Kronn deliberately killed the
                // process at its watchdog deadline.
                if let Some(reason) = timeout_reason {
                    let notice = timeout_notice(reason);
                    if full_response.is_empty() {
                        full_response = notice;
                    } else {
                        full_response.push_str(&format!("\n\n---\n{notice}"));
                    }
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
                    // The cancel token carries no reason, so this covers a human
                    // pressing stop AND the batch budget cancelling the run.
                    // Naming only the first sent people looking for a stop they
                    // never made; state the fact, list what causes it.
                    let footer = "\n\n---\n⏹️ **Exécution interrompue.** Le process de l'agent a été arrêté — soit par un stop manuel, soit parce que la durée maximale d'exécution configurée a été atteinte.";
                    if full_response.is_empty() {
                        full_response = footer.trim_start_matches('\n').to_string();
                    } else {
                        full_response.push_str(footer);
                    }
                }

                if let Some(failure) = stream_json_failure.as_ref() {
                    let notice = failure.user_message();
                    if full_response.is_empty() {
                        full_response = notice;
                    } else {
                        full_response.push_str(&format!("\n\n---\n{notice}"));
                    }
                }

                if full_response.is_empty() && !success {
                    tracing::error!(
                        "Agent {:?} exited with error ({}). stderr ({} lines): {}",
                        agent_type,
                        exit_info,
                        stderr_lines.len(),
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
                        full_response = format!(
                            "[Agent exited with error] ({})\n\n{}",
                            exit_info, stderr_text
                        );
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
                if !success && !was_interrupted && !validation_redaction_failed {
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

                // HTTP agents have no stdout tool events to parse: their loop
                // records each call in the run's stderr capture, already in the
                // `[kronn-internal: …]` shape. Lift them into the same list the
                // CLI path fills so both render identically in the transcript.
                if runner::is_http_chat_agent(&agent_type) {
                    kronn_tool_calls.extend(
                        stderr_lines
                            .iter()
                            .filter(|l| l.starts_with("[kronn-internal:"))
                            .cloned(),
                    );
                }

                let tokens_used = if stream_json_tokens > 0 {
                    stream_json_tokens
                } else if let Some(reported) = process.reported_token_usage() {
                    reported
                } else {
                    let (cleaned, count) =
                        runner::parse_token_usage(&agent_type, &full_response, &stderr_lines);
                    if count > 0 {
                        full_response = cleaned;
                    }
                    count
                };

                // Hard cap before persistence — covers EVERY path (incl. the
                // error/kill stderr capture above, which bypasses the streaming
                // cap), so a multi-MB message can't reach the DB or crash the UI
                // renderer on open. See `cap_agent_response`.
                full_response = runner::strip_leading_thinking_blocks(&full_response);
                full_response = cap_agent_response(full_response, MAX_AGENT_RESPONSE_BYTES);

                // Ordinary @alias references in an answer are informational.
                // Only the hidden marker requested by the collaboration prompt
                // authorizes a new native dispatch, and it never reaches the
                // persisted transcript.
                let (cleaned_response, marked_handoffs) =
                    extract_agent_handoff_markers(&full_response);
                full_response = cleaned_response;

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
                            let at_str = serde_json::to_string(&agent_type)
                                .unwrap_or_default()
                                .trim_matches('"')
                                .to_string();
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
                    state
                        .db
                        .with_conn(move |conn| {
                            let p = crate::db::projects::get_project(conn, &pid)?;
                            Ok(p.map(|p| {
                                p.linked_repos
                                    .into_iter()
                                    .map(|lr| lr.location)
                                    .filter(|loc| {
                                        !loc.starts_with("http://") && !loc.starts_with("https://")
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default())
                        })
                        .await
                        .unwrap_or_default()
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
                // An agent message must never be BLANK. Observed in production: a
                // reasoning model behind LiteLLM burnt 15 526 tokens in 9 s and
                // persisted an empty message — the run was correctly marked failed,
                // but the room showed nothing at all, so the failure was
                // indistinguishable from an agent with nothing to say.
                //
                // Two known paths lead here, and the reader cannot tell them apart
                // from the outside: a private reasoning block that never closed (the
                // leading-thinking filter drops it wholesale, on purpose — we do not
                // leak scratchpads), and a stream cut before its terminal chunk. So
                // state what IS known — no visible output, the model, the tokens
                // spent — instead of leaving a blank.
                if full_response.trim().is_empty() {
                    let spent = if tokens_used > 0 {
                        format!(" après {tokens_used} tokens")
                    } else {
                        String::new()
                    };
                    let model_note = attempted_model
                        .as_deref()
                        .map(|model| format!(" (`{model}`)"))
                        .unwrap_or_default();
                    // If the provider said WHY it stopped, that is the diagnosis; the
                    // generic causes below are only for when it said nothing.
                    let stop_reason = stderr_lines
                        .iter()
                        .find_map(|line| line.split("finish_reason: ").nth(1))
                        .map(str::trim)
                        .filter(|reason| !reason.is_empty());
                    full_response = match stop_reason {
                        Some("length") => format!(
                            "⚠️ **Aucune sortie visible**{model_note}{spent} — le modèle a épuisé \
                             son budget de sortie avant d'écrire une réponse.\n\n\
                             C'est le comportement typique d'un modèle de raisonnement : il a \
                             consommé ses tokens en réfléchissant. Relancer ne changera rien. \
                             Choisis un modèle non-raisonnant pour ce tier, ou une question plus \
                             étroite."
                        ),
                        Some(reason) => format!(
                            "⚠️ **Aucune sortie visible**{model_note}{spent} — le fournisseur a \
                             arrêté la génération (`{reason}`) sans produire de texte.\n\n\
                             Le run est enregistré comme échoué."
                        ),
                        None => format!(
                            "⚠️ **Aucune sortie visible**{model_note}{spent}.\n\n\
                             Le run est enregistré comme échoué et le fournisseur n'a pas dit \
                             pourquoi il s'est arrêté. Deux causes connues : un bloc de \
                             raisonnement privé jamais refermé (supprimé volontairement, jamais \
                             affiché) ou un flux coupé avant sa fin. Relancer suffit le plus \
                             souvent ; si cela se répète sur le même modèle, change de modèle."
                        ),
                    };
                }
                tracked_execution_succeeded = child_run_was_success;

                // Concrete model this reply ran on — resolved once before spawn
                // (`attempted_model`) so a non-zero exit / stall / cancel with
                // partial output still carries it. Stored per-message so the UI
                // can show "Ollama · qwen3:32b" even when the model changes
                // mid-thread. `None` for provider-default runs with no flag.
                let candidate_handoffs = if child_run_was_success {
                    marked_handoffs
                        .into_iter()
                        .filter(|candidate| attached_handoff_agents.contains(candidate))
                        .filter(|candidate| {
                            agent_handoff_target_is_allowed(candidate, &handoff_blocked_agents)
                        })
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                let agent_msg = DiscussionMessage {
                    recovered_partial: false,
                    session_tokens_at_message: None,
                    author_cli_ordinal: None,
                    id: Uuid::new_v4().to_string(),
                    role: MessageRole::Agent,
                    channel: MessageChannel::Main,
                    content: full_response,
                    agent_type: Some(agent_type.clone()),
                    timestamp: Utc::now(),
                    tokens_used,
                    auth_mode: Some(auth_mode_str.clone()),
                    model_tier: tier_label,
                    model: attempted_model.clone(),
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
                    target_agent: None,
                    reply_to_message_id: dispatch_trigger_message_id.clone(),
                };

                let did = disc_id.clone();
                let msg = agent_msg.clone();
                let source_agent = agent_type.clone();
                let dispatch_id = dispatch_job_id.clone();
                match state
                    .db
                    .with_conn(move |conn| {
                        crate::db::discussions::insert_native_agent_message_with_handoffs(
                            conn,
                            &did,
                            &msg,
                            child_run_was_success,
                            dispatch_id.as_deref(),
                            &source_agent,
                            &candidate_handoffs,
                            handoffs_enabled,
                            handoff_paid_limit,
                        )
                    })
                    .await
                {
                    Ok(outcome) => {
                        if !outcome.dispatched_agents.is_empty() {
                            tracing::info!(
                                discussion_id = %disc_id,
                                source_agent = ?agent_type,
                                targets = ?outcome.dispatched_agents,
                                "scheduled bounded agent handoff"
                            );
                            state.agent_dispatch_notify.notify_one();
                        }
                    }
                    Err(e) => tracing::error!("Failed to save agent message: {e}"),
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
                        recovered_partial: false,
                        session_tokens_at_message: None,
                        author_cli_ordinal: None,
                        model: None,
                        lint_report: None,
                        id: Uuid::new_v4().to_string(),
                        role: MessageRole::System,
                        channel: MessageChannel::Main,
                        content: crate::core::anti_halluc::enforce_refusal_message(
                            fabricated_count,
                        ),
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
                        target_agent: None,
                        reply_to_message_id: dispatch_trigger_message_id.clone(),
                    };
                    let did_ref = disc_id.clone();
                    let m = refusal.clone();
                    if let Err(e) = state
                        .db
                        .with_conn(move |conn| {
                            crate::db::discussions::insert_message(conn, &did_ref, &m)
                        })
                        .await
                    {
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
                // MCP notice in `disc_prompts.rs` — Vibe + Ollama.
                // For other agents we still scan (cheap regex) but
                // only respect markers if the agent actually emitted
                // one — defensive, no behaviour change for them.
                let markers = super::slash_markers::parse_markers(&agent_msg.content);
                if !markers.is_empty() {
                    let resolutions =
                        super::slash_markers::resolve_markers(&state, &disc_id, &markers).await;
                    for body in resolutions {
                        let sys_msg = DiscussionMessage {
                            recovered_partial: false,
                            session_tokens_at_message: None,
                            author_cli_ordinal: None,
                            model: None,
                            lint_report: None,
                            id: Uuid::new_v4().to_string(),
                            role: MessageRole::System,
                            channel: MessageChannel::Main,
                            content: body,
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
                            target_agent: None,
                            reply_to_message_id: dispatch_trigger_message_id.clone(),
                        };
                        let did_sys = disc_id.clone();
                        let m = sys_msg.clone();
                        if let Err(e) = state
                            .db
                            .with_conn(move |conn| {
                                crate::db::discussions::insert_message(conn, &did_sys, &m)
                            })
                            .await
                        {
                            tracing::warn!("Failed to insert slash-marker system message: {e}");
                        }
                    }
                    tracing::info!(
                        "Resolved {} slash-marker(s) for disc {}",
                        markers.len(),
                        disc_id
                    );
                }

                // ── HTTP provider retry trace ──────────────────────────
                if !provider_retry_trace.is_empty() {
                    let sys_msg = DiscussionMessage {
                        recovered_partial: false,
                        session_tokens_at_message: None,
                        author_cli_ordinal: None,
                        model: None,
                        lint_report: None,
                        id: Uuid::new_v4().to_string(),
                        role: MessageRole::System,
                        channel: MessageChannel::Main,
                        content: format!(
                            "↻ **Provider retry**\n\n{}",
                            provider_retry_trace.join("\n")
                        ),
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
                        target_agent: None,
                        reply_to_message_id: dispatch_trigger_message_id.clone(),
                    };
                    let did_sys = disc_id.clone();
                    let m = sys_msg.clone();
                    if let Err(e) = state
                        .db
                        .with_conn(move |conn| {
                            crate::db::discussions::insert_message(conn, &did_sys, &m)
                        })
                        .await
                    {
                        tracing::warn!("Failed to persist provider retry trace: {e}");
                    } else {
                        crate::api::federation::federate_message(&state, &disc_id, &sys_msg).await;
                    }
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
                            recovered_partial: false,
                            session_tokens_at_message: None,
                            author_cli_ordinal: None,
                            model: None,
                            lint_report: None,
                            id: Uuid::new_v4().to_string(),
                            role: MessageRole::System,
                            channel: MessageChannel::Main,
                            content: body.clone(),
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
                            target_agent: None,
                            reply_to_message_id: dispatch_trigger_message_id.clone(),
                        };
                        let did_sys = disc_id.clone();
                        let m = sys_msg.clone();
                        if let Err(e) = state
                            .db
                            .with_conn(move |conn| {
                                crate::db::discussions::insert_message(conn, &did_sys, &m)
                            })
                            .await
                        {
                            tracing::warn!(
                                "Failed to insert kronn-internal tool-call system message: {e}"
                            );
                        }
                    }
                    tracing::info!(
                        "Persisted {} kronn-internal MCP tool-call(s) for disc {}",
                        kronn_tool_calls.len(),
                        disc_id
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
                            recovered_partial: false,
                            session_tokens_at_message: None,
                            author_cli_ordinal: None,
                            model: None,
                            lint_report: None,
                            id: Uuid::new_v4().to_string(),
                            role: MessageRole::System,
                            channel: MessageChannel::Main,
                            content: body.clone(),
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
                            target_agent: None,
                            reply_to_message_id: dispatch_trigger_message_id.clone(),
                        };
                        let did_sys = disc_id.clone();
                        let m = sys_msg.clone();
                        if let Err(e) = state
                            .db
                            .with_conn(move |conn| {
                                crate::db::discussions::insert_message(conn, &did_sys, &m)
                            })
                            .await
                        {
                            tracing::warn!(
                                "Failed to insert agent-native tool-call system message: {e}"
                            );
                        }
                    }
                    tracing::info!(
                        "Persisted {} agent-native tool-call(s) for disc {}",
                        native_tool_calls.len(),
                        disc_id
                    );
                }

                // Clear the in-flight checkpoint — the final message is now in
                // `messages`, so partial_response would be redundant + would
                // double up at the next backend boot if we left it dangling.
                // Same call also clears the awaiting_agent marker: the agent
                // delivered, this disc is no longer "owed a run". Keeping the
                // flag to 0 ONLY on delivery (not at task-start) means an
                // interruption mid-run stays flagged and the boot reconcile
                // catches it — no blind window.
                let did_clear = disc_id.clone();
                let dispatch_id_for_clear = dispatch_job_id.clone();
                let _ = state
                    .db
                    .with_conn(move |conn| {
                        // Attempt both clears even if the first fails — a `?` here
                        // would leave the awaiting marker stale on a partial-clear
                        // error and trigger needless boot reconcile work.
                        let partial = if let Some(dispatch_id) = dispatch_id_for_clear.as_deref() {
                            crate::db::discussions::clear_partial_response_for_dispatch(
                                conn,
                                &did_clear,
                                dispatch_id,
                            )
                            .map(|_| ())
                        } else {
                            crate::db::discussions::set_partial_response(
                                conn, &did_clear, None, None,
                            )
                        };
                        let awaiting =
                            clear_awaiting_after_terminal(conn, &did_clear, tracked_dispatch);
                        partial.and(awaiting)
                    })
                    .await;

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
                        let archived = state
                            .db
                            .with_conn(move |conn| {
                                crate::db::discussions::update_discussion(
                                    conn,
                                    &did_archive,
                                    None,
                                    Some(true),
                                    None,
                                    None,
                                )
                            })
                            .await;
                        match archived {
                            Ok(true) => tracing::info!(
                                "Auto-archived discussion {} after terminal signal {}",
                                disc_id,
                                sig,
                            ),
                            Ok(false) => tracing::warn!(
                                "Auto-archive of disc {} returned no-op (disc deleted?)",
                                disc_id,
                            ),
                            Err(e) => tracing::warn!(
                                "Auto-archive failed for disc {} on {}: {}",
                                disc_id,
                                sig,
                                e,
                            ),
                        }
                    }
                }

                // Detect KRONN:BRIEFING_COMPLETE marker
                if success
                    && agent_msg
                        .content
                        .to_uppercase()
                        .contains("KRONN:BRIEFING_COMPLETE")
                {
                    if let Some(ref pid) = disc_project_id {
                        let briefing_project_id = pid.clone();
                        let briefing_project_path = project_path.clone();
                        let briefing_state = state.clone();
                        tokio::spawn(async move {
                            // Read briefing.md from the project's docs folder.
                            // Path-agnostic — works on docs/ post-pivot AND legacy ai/.
                            let resolved =
                                crate::core::scanner::resolve_host_path(&briefing_project_path);
                            let briefing_file = crate::core::scanner::detect_docs_dir(&resolved)
                                .join("briefing.md");
                            let notes = tokio::task::spawn_blocking(move || {
                                std::fs::read_to_string(&briefing_file).ok()
                            })
                            .await
                            .unwrap_or(None);

                            if let Some(content) = notes {
                                let pid = briefing_project_id.clone();
                                if let Err(e) = briefing_state
                                    .db
                                    .with_conn(move |conn| {
                                        crate::db::projects::update_project_briefing_notes(
                                            conn,
                                            &pid,
                                            Some(&content),
                                        )
                                    })
                                    .await
                                {
                                    tracing::error!(
                                        "Failed to save briefing notes for project {}: {e}",
                                        briefing_project_id
                                    );
                                } else {
                                    tracing::info!(
                                        "Briefing notes saved for project {}",
                                        briefing_project_id
                                    );
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
                            &summary_state,
                            &summary_disc_id,
                            &summary_agent_type,
                            &summary_tokens,
                        )
                        .await;
                    });
                }

                let done = serde_json::json!({ "message_id": agent_msg.id, "success": success, "tokens_used": tokens_used });
                let _ = tx.send(AgentStreamEvent::Done { data: done }).await;
            }
            Err(e) => {
                // The caller token can now win while the initial HTTP request
                // is still waiting for headers, before an AgentProcess exists.
                // That is an intentional stop, not a provider/preflight
                // failure: do not persist an error bubble or make the dispatch
                // observer requeue it. The durable job is already Cancelled.
                if cancel_token.is_cancelled() {
                    tracing::info!(
                        "Agent start for disc {} cancelled before provider acceptance",
                        disc_id
                    );
                    if let Some(sender) = completion_tx.take() {
                        let _ = sender.send(AgentExecutionOutcome::Finished { success: false });
                    }
                    return;
                }
                tracing::error!("Agent start failed: {}", e);

                let tracked_outcome = completion_tx
                    .as_ref()
                    .map(|_| agent_start_failure_outcome(&agent_type, &e));
                if matches!(
                    &tracked_outcome,
                    Some(AgentExecutionOutcome::RuntimeUnavailable { .. })
                ) {
                    // Keep the established durable deferral for absent CLI
                    // binaries. HTTP-native agents are excluded above: an
                    // unavailable proxy/local service must be visible and
                    // explicitly retryable instead of spinning forever.
                    let err = serde_json::json!({ "error": e });
                    let _ = tx.send(AgentStreamEvent::Error { data: err }).await;
                    if let (Some(sender), Some(outcome)) = (completion_tx.take(), tracked_outcome) {
                        let _ = sender.send(outcome);
                    }
                    return;
                }

                // KT-37 — a genuine spawn failure (NOT owed/retried: that path
                // returned above) carries the agent + attempted model so the UI
                // can label the failed turn's provenance. Role stays System.
                let tier_label = match disc_tier {
                    crate::models::ModelTier::Economy => "economy",
                    crate::models::ModelTier::Default => "default",
                    crate::models::ModelTier::Reasoning => "reasoning",
                };
                if agent_type == AgentType::LiteLlm {
                    if let (Some(status), Some(model)) =
                        (agent_http_status(&e), attempted_model.as_deref())
                    {
                        if matches!(status, 400 | 404 | 422) {
                            let endpoint = crate::api::lite_llm::resolve_base_url_pub(
                                http_endpoints.lite_llm.as_deref(),
                            );
                            let model = model.to_string();
                            let raw_error = e.clone();
                            if let Err(db_error) = state
                                .db
                                .with_conn(move |conn| {
                                    crate::db::lite_llm_model_failures::record(
                                        conn, &endpoint, &model, status, &raw_error,
                                    )?;
                                    Ok(())
                                })
                                .await
                            {
                                tracing::warn!(
                                    "Failed to remember LiteLLM model failure: {db_error}"
                                );
                            }
                        }
                    }
                }
                let content = agent_start_error_content(
                    &agent_type,
                    attempted_model.as_deref(),
                    disc_tier,
                    &disc.language,
                    &e,
                    dispatch_job_id.as_deref(),
                )
                .unwrap_or_else(|| format!("Erreur: {e}"));
                let err_msg = DiscussionMessage {
                    recovered_partial: false,
                    session_tokens_at_message: None,
                    author_cli_ordinal: None,
                    model: attempted_model.clone(),
                    lint_report: None,
                    id: Uuid::new_v4().to_string(),
                    role: MessageRole::System,
                    channel: MessageChannel::Main,
                    content,
                    agent_type: Some(agent_type.clone()),
                    timestamp: Utc::now(),
                    tokens_used: 0,
                    auth_mode: None,
                    model_tier: Some(tier_label.to_string()),
                    cost_usd: None,
                    author_pseudo: None,
                    author_avatar_email: None,
                    source_msg_id: None,
                    duration_ms: None,
                    target_agent: None,
                    reply_to_message_id: dispatch_trigger_message_id.clone(),
                };

                let did = disc_id.clone();
                let error_message_id = err_msg.id.clone();
                let error_dispatch_id = dispatch_job_id.clone();
                let err_msg_fed = err_msg.clone();
                if let Err(db_err) = state
                    .db
                    .with_conn(move |conn| {
                        // The agent was handled (it failed to start), so it's
                        // no longer "owed a run": clear the marker so the boot
                        // reconcile doesn't later flag this as interrupted. Both
                        // ops run even if the insert fails — a `?` would leave the
                        // marker stale exactly when the run never started.
                        let inserted = crate::db::discussions::insert_message(conn, &did, &err_msg);
                        let linked = match error_dispatch_id.as_deref() {
                            Some(job_id) => conn
                                .execute(
                                    "UPDATE messages SET agent_dispatch_job_id = ?2 WHERE id = ?1",
                                    rusqlite::params![error_message_id, job_id],
                                )
                                .map(|_| ())
                                .map_err(anyhow::Error::from),
                            None => Ok(()),
                        };
                        let cleared = clear_awaiting_after_terminal(conn, &did, tracked_dispatch);
                        inserted.and(linked).and(cleared)
                    })
                    .await
                {
                    tracing::error!("Failed to save agent error message: {db_err}");
                }
                // F1 — let the peer see the turn failed instead of silence.
                crate::api::federation::federate_message(&state, &disc_id, &err_msg_fed).await;

                let err = serde_json::json!({ "error": e });
                let _ = tx.send(AgentStreamEvent::Error { data: err }).await;
                if let (Some(sender), Some(outcome)) = (completion_tx.take(), tracked_outcome) {
                    let _ = sender.send(outcome);
                }
            }
        }
        if let Some(sender) = completion_tx.take() {
            let _ = sender.send(AgentExecutionOutcome::Finished {
                success: tracked_execution_succeeded,
            });
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

    let stream = prepend_initial_event(stream, initial_event.take());
    Sse::new(crate::core::sse_limits::bounded(stream))
}

fn auth_required_system_message(
    agent_type: &AgentType,
    language: &str,
    setup_command: Option<&str>,
) -> DiscussionMessage {
    let agent = format!("{agent_type:?}");
    let setup = setup_command
        .map(|command| format!(" `{command}`"))
        .unwrap_or_default();
    let content = match language {
        "fr" => format!(
            "Configuration requise : {agent} est installé, mais son authentification n’est pas prête. Lancez{setup} ou ajoutez sa clé dans Config → Agents, puis réessayez."
        ),
        "es" => format!(
            "Configuración necesaria: {agent} está instalado, pero su autenticación no está lista. Ejecuta{setup} o añade su clave en Config → Agentes y vuelve a intentarlo."
        ),
        _ => format!(
            "Configuration required: {agent} is installed, but its authentication is not ready. Run{setup} or add its key in Config → Agents, then try again."
        ),
    };
    DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: Uuid::new_v4().to_string(),
        role: MessageRole::System,
        channel: MessageChannel::Main,
        content,
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
        target_agent: None,
        reply_to_message_id: None,
    }
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
    global_timeout: Duration,
) -> AgentRunResult {
    let mut full_response = String::new();
    let mut stream_tokens: u64 = 0;
    let mut stream_json_failure: Option<runner::StreamJsonFailure> = None;
    let mut current_tool: Option<String> = None;
    let mut tool_input = String::new();
    let is_stream_json = process.output_mode() == runner::OutputMode::StreamJson;
    let raw_stream = process.raw_token_stream();
    let deadline = tokio::time::Instant::now() + global_timeout;

    let mut signal_stop = false;
    // KT-80 — an orchestration round killed at its deadline must say so too.
    // Without this the round fell through to `exit code: None`, the same opaque
    // message the send path stopped producing.
    let mut timeout_reason: Option<AgentTimeoutReason> = None;
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
                                runner::StreamJsonEvent::TerminalError(failure) => {
                                    stream_tokens = stream_tokens.max(
                                        failure.input_tokens + failure.output_tokens,
                                    );
                                    stream_json_failure = Some(failure);
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
                            let nl = if raw_stream || full_response.is_empty() { "" } else { "\n" };
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
                timeout_reason = Some(AgentTimeoutReason::Global(global_timeout));
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

    if let Some(failure) = stream_json_failure.as_ref() {
        let notice = failure.user_message();
        if full_response.is_empty() {
            full_response = notice;
        } else {
            full_response.push_str(&format!("\n\n---\n{notice}"));
        }
    }

    // KT-80 — a deliberate kill at the deadline is explained first: an exit code
    // of `None` describes the signal, not the cause, and the reader cannot tell
    // a crash from a watchdog.
    if let Some(reason) = timeout_reason {
        let notice = timeout_notice(reason);
        full_response = if full_response.is_empty() {
            notice
        } else {
            format!("{full_response}\n\n---\n{notice}")
        };
    } else if full_response.is_empty() && !success {
        let exit_info = match &status {
            Some(s) => format!("exit code: {:?}", s.code),
            None => "exit status unavailable".to_string(),
        };
        tracing::error!(
            "Agent {:?} exited with error ({}). stderr: {}",
            agent_type,
            exit_info,
            // Char-count truncation — see twin site above for rationale.
            if stderr_text.chars().count() > 500 {
                stderr_text.chars().take(500).collect::<String>()
            } else {
                stderr_text.clone()
            }
        );
        full_response = if stderr_text.is_empty() {
            format!("[Agent exited with error] ({})", exit_info)
        } else {
            format!(
                "[Agent exited with error] ({})\n\n{}",
                exit_info, stderr_text
            )
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
    } else if let Some(reported) = process.reported_token_usage() {
        reported
    } else {
        let (cleaned, count) = runner::parse_token_usage(agent_type, &full_response, &stderr);
        if count > 0 {
            full_response = cleaned;
        }
        count
    };

    AgentRunResult {
        response: full_response,
        tokens_used,
    }
}

/// Run an agent silently (no SSE streaming), return collected text.
/// Used for conversation summarization before debate.
///
/// Generic over [`runner::AgentIo`] (0.8.8 test-seam refactor) so the
/// accumulation + stream-json-vs-raw + teardown logic is unit-testable with
/// a `ScriptedProcess`, without spawning a real CLI. Production passes a real
/// `AgentProcess`; both impl `AgentIo`.
pub(super) async fn run_agent_collect(
    mut process: impl runner::AgentIo,
    global_timeout: Duration,
) -> String {
    let mut output = String::new();
    let is_json = process.output_mode() == runner::OutputMode::StreamJson;
    let deadline = tokio::time::Instant::now() + global_timeout;
    loop {
        tokio::select! {
            line = process.next_line() => {
                match line {
                    Some(l) => {
                        if is_json {
                            match runner::parse_claude_stream_line(&l) {
                                runner::StreamJsonEvent::Text(text) => output.push_str(&text),
                                runner::StreamJsonEvent::TerminalError(failure) => {
                                    if !output.is_empty() {
                                        output.push_str("\n\n---\n");
                                    }
                                    output.push_str(&failure.user_message());
                                }
                                _ => {}
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
        "disc_get_message" => {
            let selector = val
                .get("message_id")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
                .or_else(|| val.get("idx").map(|value| value.to_string()))
                .unwrap_or_default();
            let before = val
                .get("before")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let after = val
                .get("after")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            match (before, after) {
                (0, 0) => selector,
                _ => format!("{}, -{}/+{}", selector, before, after),
            }
        }
        "disc_summarize" => {
            let from = val.get("from").and_then(|v| v.as_i64());
            let to = val.get("to").and_then(|v| v.as_i64());
            let force = val
                .get("force_refresh")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
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
    fn get_message_renders_reference_and_context_window() {
        assert_eq!(
            pretty_kronn_args(
                "disc_get_message",
                r#"{"message_id":"MSG-12345678","before":2,"after":3}"#,
            ),
            "MSG-12345678, -2/+3",
        );
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
            pretty_kronn_args(
                "disc_summarize",
                r#"{"from":0,"to":5,"force_refresh":true}"#
            ),
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
    use super::{
        agent_start_error_content, agent_start_failure_outcome, auth_required_system_message,
        cap_agent_response, child_run_counts_as_success, configured_agent_global_timeout,
        effective_global_timeout, effective_stall_timeout, AgentExecutionOutcome,
        NON_STREAMING_STALL_TIMEOUT,
    };
    use crate::models::{AgentType, MessageRole};
    use std::time::Duration;

    #[test]
    fn missing_sdk_auth_is_an_actionable_system_message_not_an_agent_reply() {
        let message = auth_required_system_message(&AgentType::Vibe, "fr", Some("vibe --setup"));
        assert_eq!(message.role, MessageRole::System);
        assert_eq!(message.agent_type, None);
        assert!(message.content.contains("Configuration requise"));
        assert!(message.content.contains("vibe --setup"));
        assert!(!message.content.contains("MISTRAL_API_KEY"));
    }

    // ── #1 — stall watchdog must not apply to non-streaming agents ──
    // (2026-06-23: Codex `exec` is silent on stdout until the very end; the
    // no-chunk stall killed slow-but-healthy runs → empty discussions.)

    #[test]
    fn streaming_agent_keeps_configured_stall() {
        let configured = Duration::from_secs(5 * 60);
        assert_eq!(
            effective_stall_timeout(true, configured, configured_agent_global_timeout(30),),
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
        assert!(
            NON_STREAMING_STALL_TIMEOUT > configured,
            "must outlast the short streaming stall (else slow non-streamers die early)"
        );
        assert!(
            NON_STREAMING_STALL_TIMEOUT < configured_agent_global_timeout(30),
            "must be SHORTER than the global, else a hung run squats its slot too long"
        );
    }

    #[test]
    fn configured_global_timeout_accepts_the_full_ui_range() {
        assert_eq!(
            configured_agent_global_timeout(240),
            Duration::from_secs(240 * 60),
        );
        assert_eq!(
            configured_agent_global_timeout(0),
            Duration::from_secs(60),
            "manually edited zero values must still retain a safety deadline",
        );
    }

    #[test]
    fn ollama_uses_the_explicit_local_budget_without_a_multiplier() {
        assert_eq!(
            effective_global_timeout(&AgentType::Ollama, 30, 137),
            Duration::from_secs(137 * 60),
        );
        assert_eq!(
            effective_global_timeout(&AgentType::ClaudeCode, 30, 137),
            Duration::from_secs(30 * 60),
        );
    }

    #[test]
    fn non_streaming_agent_timeout_can_be_increased_above_the_floor() {
        let configured = Duration::from_secs(20 * 60);
        assert_eq!(
            effective_stall_timeout(false, configured, NON_STREAMING_STALL_TIMEOUT),
            configured,
        );
    }

    #[test]
    fn empty_timeout_notice_names_the_deadline_instead_of_exit_code_none() {
        let notice = super::timeout_notice(super::AgentTimeoutReason::Stall(
            NON_STREAMING_STALL_TIMEOUT,
        ));
        assert!(notice.contains("15 min"));
        assert!(notice.contains("Agent inactivity timeout"));
        assert!(!notice.contains("exit code"));
        assert!(!notice.contains("None"));
    }

    // ── #2 — empty-but-clean-exit child is NOT a batch success ──
    // (made a batch workflow report green Success over 16 empty discs.)

    #[test]
    fn clean_exit_with_real_reply_is_success() {
        assert!(child_run_counts_as_success(
            true,
            "Triage:\n- clear: EW-1 ready to frame"
        ));
    }

    #[test]
    fn clean_exit_with_blank_reply_is_not_success() {
        assert!(
            !child_run_counts_as_success(true, ""),
            "empty reply ≠ success"
        );
        assert!(
            !child_run_counts_as_success(true, "   \n\t  "),
            "whitespace-only ≠ success"
        );
    }

    #[test]
    fn failed_exit_is_never_success_even_with_partial_text() {
        assert!(!child_run_counts_as_success(
            false,
            "partial output before crash"
        ));
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
        assert!(
            out.len() <= 2_000_000 + 80,
            "must be bounded near the limit, got {}",
            out.len()
        );
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

    #[test]
    fn http_agents_surface_runtime_outages_but_cli_runtime_absence_stays_deferred() {
        assert_eq!(
            agent_start_failure_outcome(&AgentType::LiteLlm, "LiteLLM unreachable at http://proxy"),
            AgentExecutionOutcome::PreflightFailed {
                diagnostic: "agent execution preflight failed".into()
            }
        );
        assert_eq!(
            agent_start_failure_outcome(&AgentType::Ollama, "Ollama unreachable at localhost"),
            AgentExecutionOutcome::PreflightFailed {
                diagnostic: "agent execution preflight failed".into()
            }
        );
        assert_eq!(
            agent_start_failure_outcome(&AgentType::Codex, "Binary 'codex' not found"),
            AgentExecutionOutcome::RuntimeUnavailable {
                reason: "Binary 'codex' not found".into()
            }
        );
        assert_eq!(
            agent_start_failure_outcome(
                &AgentType::CopilotCli,
                "Copilot task worker cannot start: phase=auth; failure_kind=invalid_auth"
            ),
            AgentExecutionOutcome::PreflightFailed {
                diagnostic:
                    "Copilot task worker cannot start: phase=auth; failure_kind=invalid_auth".into()
            }
        );
    }

    #[test]
    fn model_http_error_is_a_structured_actionable_system_event() {
        let raw = r#"LiteLLM error 404 Not Found: {"error":{"message":"Vertex details"}}"#;
        let content = agent_start_error_content(
            &AgentType::LiteLlm,
            Some("vertex_ai/mistral-large-2411"),
            crate::models::ModelTier::Default,
            "fr",
            raw,
            Some("job-lite"),
        )
        .expect("404 model errors should be structured");

        let json = content
            .strip_prefix("[kronn:agent-error]\n")
            .expect("system event marker");
        let payload: serde_json::Value = serde_json::from_str(json).expect("valid payload");
        assert_eq!(payload["status"], 404);
        assert_eq!(payload["tier"], "default");
        assert!(payload["summary"].as_str().unwrap().contains("HTTP 404"));
        assert!(payload["summary"]
            .as_str()
            .unwrap()
            .contains("vertex_ai/mistral-large-2411"));
        assert_eq!(payload["detail"], raw);
        assert_eq!(payload["retry_dispatch_id"], "job-lite");
    }

    #[test]
    fn unreachable_http_agent_is_a_structured_retryable_event() {
        let content = agent_start_error_content(
            &AgentType::LiteLlm,
            Some("model-a"),
            crate::models::ModelTier::Default,
            "en",
            "LiteLLM unreachable at http://proxy: connection refused",
            Some("job-lite"),
        )
        .expect("HTTP runtime failures must stay visible");
        let payload: serde_json::Value =
            serde_json::from_str(content.strip_prefix("[kronn:agent-error]\n").unwrap()).unwrap();
        assert_eq!(payload["kind"], "agent_error");
        assert_eq!(payload["status"], serde_json::Value::Null);
        assert_eq!(payload["retry_dispatch_id"], "job-lite");
    }
}

#[cfg(test)]
mod truncate_tool_args_tests {
    use super::truncate_tool_args;

    #[test]
    fn short_input_passes_through_unchanged() {
        assert_eq!(truncate_tool_args("hello", 120), "hello");
        assert_eq!(
            truncate_tool_args(r#"{"file":"a.rs"}"#, 120),
            r#"{"file":"a.rs"}"#
        );
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
        assert_eq!(out.chars().count(), 21); // 20 + ellipsis
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
    use std::time::Duration;

    const TEST_GLOBAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

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
        let out = run_agent_collect(proc, TEST_GLOBAL_TIMEOUT).await;
        assert_eq!(out, "first\nsecond\nthird");
    }

    #[tokio::test]
    async fn empty_stream_yields_empty_string() {
        let proc = ScriptedProcess::raw(Vec::<String>::new());
        let out = run_agent_collect(proc, TEST_GLOBAL_TIMEOUT).await;
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
        let out = run_agent_collect(proc, TEST_GLOBAL_TIMEOUT).await;
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
        let out = run_agent_collect(proc, TEST_GLOBAL_TIMEOUT).await;
        assert_eq!(out, "plain log noisereal");
    }

    #[tokio::test]
    async fn raw_mode_single_line_no_leading_newline() {
        let proc = ScriptedProcess::raw(["only"]);
        assert_eq!(run_agent_collect(proc, TEST_GLOBAL_TIMEOUT).await, "only");
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
    use crate::agents::runner::ScriptedProcess;
    use crate::api::discussions::AgentStreamEvent;
    use crate::models::AgentType;
    use std::time::Duration;

    const TEST_GLOBAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

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

    /// KT-80 — the orchestration loop had the same hole as the send path: a
    /// round killed at the configured global deadline produced
    /// `[Agent exited with error] (exit code: None)`, which describes the signal
    /// and hides the cause. Paused clock so the deadline is reached instantly.
    #[tokio::test(start_paused = true)]
    async fn an_orchestration_round_killed_at_the_deadline_says_so() {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let proc = ScriptedProcess::hanging();
        let timeout = Duration::from_secs(120 * 60);
        let res = run_agent_streaming(proc, &tx, &meta(), &AgentType::ClaudeCode, timeout).await;

        assert!(
            res.response.contains("120-minute global execution limit"),
            "the round must name the deadline it hit, got {:?}",
            res.response
        );
        assert!(
            !res.response.contains("exit code"),
            "an exit code must not stand in for the explanation: {:?}",
            res.response
        );
    }

    #[tokio::test]
    async fn raw_accumulates_and_sends_chunks() {
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        let proc = ScriptedProcess::raw(["line one", "line two"]);
        let res = run_agent_streaming(
            proc,
            &tx,
            &meta(),
            &AgentType::ClaudeCode,
            TEST_GLOBAL_TIMEOUT,
        )
        .await;
        drop(tx);
        assert_eq!(res.response, "line one\nline two");
        let chunks = drain(rx)
            .into_iter()
            .filter(|e| matches!(e, AgentStreamEvent::Chunk { .. }))
            .count();
        assert_eq!(chunks, 2, "one Chunk per raw line");
    }

    #[tokio::test]
    async fn stream_json_text_accumulates() {
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        let proc = ScriptedProcess::stream_json([text_delta("Hello "), text_delta("world")]);
        let res = run_agent_streaming(
            proc,
            &tx,
            &meta(),
            &AgentType::ClaudeCode,
            TEST_GLOBAL_TIMEOUT,
        )
        .await;
        drop(tx);
        assert_eq!(res.response, "Hello world");
        assert!(drain(rx)
            .iter()
            .any(|e| matches!(e, AgentStreamEvent::Chunk { .. })));
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
        let res = run_agent_streaming(
            proc,
            &tx,
            &meta(),
            &AgentType::ClaudeCode,
            TEST_GLOBAL_TIMEOUT,
        )
        .await;
        drop(tx);
        // Tool JSON must NOT leak into the prose response.
        assert_eq!(res.response, "Reading file. Done.");
        let logs: Vec<_> = drain(rx)
            .into_iter()
            .filter(|e| matches!(e, AgentStreamEvent::Log { .. }))
            .collect();
        assert_eq!(
            logs.len(),
            1,
            "exactly one Log event for the Read tool call"
        );
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
        let res = run_agent_streaming(
            proc,
            &tx,
            &meta(),
            &AgentType::ClaudeCode,
            TEST_GLOBAL_TIMEOUT,
        )
        .await;
        drop(tx);
        assert!(res.response.contains("Architecture proposed."));
        assert!(
            !res.response.contains("trailing line must never be reached"),
            "content after the terminal signal must be truncated: {:?}",
            res.response
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
        let res = run_agent_streaming(
            proc,
            &tx,
            &meta(),
            &AgentType::ClaudeCode,
            TEST_GLOBAL_TIMEOUT,
        )
        .await;
        drop(tx);
        assert!(
            res.response.contains("Decoder loop detected"),
            "expected decoder-loop abort marker, got: {:?}",
            res.response.chars().rev().take(80).collect::<String>()
        );
        let _ = drain(rx);
    }

    #[tokio::test]
    async fn failed_fable_result_surfaces_structured_quota_instead_of_no_output() {
        let fable_429 = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"You've hit your org's monthly spend limit · run /usage-credits to manage your plan.","api_error_status":429,"terminal_reason":"api_error","cost_usd":0,"usage":{"input_tokens":0,"output_tokens":0}}"#;
        let (tx, rx) = tokio::sync::mpsc::channel(10);
        let proc = ScriptedProcess::stream_json([fable_429]).with_exit(false, Some(1));
        let res = run_agent_streaming(
            proc,
            &tx,
            &meta(),
            &AgentType::ClaudeCode,
            TEST_GLOBAL_TIMEOUT,
        )
        .await;
        drop(tx);

        assert!(
            res.response.contains("monthly spend limit"),
            "got: {:?}",
            res.response
        );
        assert!(res.response.contains("HTTP 429"), "got: {:?}", res.response);
        assert!(
            res.response.contains("terminal_reason=api_error"),
            "got: {:?}",
            res.response
        );
        assert!(
            !res.response.contains("No output captured"),
            "got: {:?}",
            res.response
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
        let res = run_agent_streaming(
            proc,
            &tx,
            &meta(),
            &AgentType::ClaudeCode,
            TEST_GLOBAL_TIMEOUT,
        )
        .await;
        drop(tx);
        assert!(
            res.response.contains("[Agent exited with error]"),
            "got: {:?}",
            res.response
        );
        assert!(
            res.response.contains("boom: something failed"),
            "stderr should surface: {:?}",
            res.response
        );
        let _ = drain(rx);
    }

    #[tokio::test]
    async fn empty_response_clean_exit_is_no_response() {
        let (tx, rx) = tokio::sync::mpsc::channel(10);
        let proc = ScriptedProcess::stream_json(Vec::<String>::new()); // success, no output
        let res = run_agent_streaming(
            proc,
            &tx,
            &meta(),
            &AgentType::ClaudeCode,
            TEST_GLOBAL_TIMEOUT,
        )
        .await;
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
            assert!(
                !is_decoder_loop(". ", &mut last, &mut count),
                "short delta must not fire"
            );
            assert!(
                !is_decoder_loop("\n\n\n", &mut last, &mut count),
                "whitespace delta must not fire"
            );
            assert!(
                !is_decoder_loop("a", &mut last, &mut count),
                "1-char delta must not fire"
            );
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
                assert!(
                    s.starts_with("[kronn-internal: disc_get_message("),
                    "got {s}"
                );
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
                assert!(
                    s.contains('…'),
                    "long input should be truncated with ellipsis: {s}"
                );
                assert!(
                    s.len() < big.len(),
                    "record must be shorter than the raw input"
                );
            }
            ToolRecord::Kronn(_) => panic!("Write is native"),
        }
    }
}
