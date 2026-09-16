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

type QuickPromptSnapshot = (
    std::collections::HashMap<String, String>,
    Vec<crate::core::execution_variables::VariableProvenance>,
);

fn load_quick_prompt_snapshot(
    conn: &rusqlite::Connection,
    discussion_id: &str,
    workflow_run_id: Option<&str>,
    key: &[u8; 32],
) -> anyhow::Result<Option<QuickPromptSnapshot>> {
    let load = |kind, id| -> anyhow::Result<Option<QuickPromptSnapshot>> {
        let Some(values) = crate::db::execution_variable_snapshots::load_values(
            conn,
            kind,
            id,
            key,
            chrono::Utc::now(),
        )?
        else {
            return Ok(None);
        };
        let metadata = crate::db::execution_variable_snapshots::metadata(conn, kind, id)?
            .ok_or_else(|| anyhow::anyhow!("Quick Prompt variable snapshot metadata missing"))?;
        Ok(Some((values, metadata.provenance)))
    };

    for (kind, id) in [
        ("quick_prompt", Some(discussion_id)),
        ("quick_prompt_batch_item", Some(discussion_id)),
        ("quick_prompt_compare", workflow_run_id),
    ] {
        if let Some(id) = id {
            if let Some(snapshot) = load(kind, id)? {
                return Ok(Some(snapshot));
            }
        }
    }
    if let Some(batch_run_id) = workflow_run_id {
        let parent_id: Option<String> = conn
            .query_row(
                "SELECT parent_run_id FROM workflow_runs WHERE id=?1",
                [batch_run_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(parent_id) = parent_id {
            return load("workflow", &parent_id);
        }
    }
    Ok(None)
}

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
/// Moves the tool calls an ACP runtime reported into the transcript's tool
/// lists.
///
/// ACP has no stdout event stream, so its runtime reports each call into the
/// run's stderr capture, one name per line. Until 0.13.0 it forwarded them on
/// the channel carrying the reply instead, which glued
/// `[ClaudeCode tool: ToolSearch]` into the middle of the agent's sentences and
/// left the group under the message empty — the calls were both in the wrong
/// place and missing from the right one.
///
/// Classified through the same `classify_tool_call` the CLI path uses, so
/// kronn-internal and agent-native keep splitting visually. ACP reports a name
/// without arguments, hence the empty input.
pub(super) fn lift_acp_tool_calls(
    stderr_lines: &[String],
    kronn_tool_calls: &mut Vec<String>,
    native_tool_calls: &mut Vec<String>,
) {
    for name in stderr_lines
        .iter()
        .filter_map(|line| line.strip_prefix(runner::ACP_TOOL_MARKER))
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        match classify_tool_call(name, "") {
            ToolRecord::Kronn(record) => kronn_tool_calls.push(record),
            ToolRecord::Native(record) => native_tool_calls.push(record),
        }
    }
}

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
    // A NUL byte in the command line is settled before anything runs, so
    // deferring it is pure repetition: issue 201 logged the same refusal 282
    // times, once every 30 s, and diagnosed nothing. Both wordings are matched
    // — Kronn's own pre-spawn check, and the OS message if one slips past it.
    let deterministic_nul_byte =
        error.contains("contains a NUL byte") || error.contains("nul byte found in provided data");
    if matches!(
        agent_type,
        AgentType::LiteLlm | AgentType::Nvidia | AgentType::Ollama | AgentType::Custom
    ) || error.starts_with("Project path not found:")
        || error.starts_with("Copilot task worker cannot start:")
        || non_retryable_http_status
        || deterministic_nul_byte
    {
        AgentExecutionOutcome::PreflightFailed {
            // Keep the reason. It used to be replaced by "agent execution
            // preflight failed" for everything except Copilot and a NUL byte,
            // and that sentence tells an operator nothing they can act on: two
            // agents vanished from a room twenty seconds after being mentioned,
            // and finding out why took reading the database.
            //
            // The generic wording protected nothing either — the sibling
            // branch below already surfaces this very string as
            // `RuntimeUnavailable { reason }`. The only thing it withheld was
            // the diagnosis. Kronn's own pre-spawn checks word these messages
            // (`Project path not found: …`, `Copilot task worker cannot
            // start: …`), and the carrier rule still holds: they name what
            // offends, never its value.
            diagnostic: error.to_string(),
        }
    } else {
        AgentExecutionOutcome::RuntimeUnavailable {
            reason: error.to_string(),
        }
    }
}

fn persist_agent_start_error(
    conn: &rusqlite::Connection,
    discussion_id: &str,
    message: &DiscussionMessage,
    dispatch_id: Option<&str>,
    tracked_dispatch: bool,
) -> anyhow::Result<()> {
    // Still clear the untracked obligation if inserting the diagnostic fails.
    let inserted = crate::db::discussions::insert_message(conn, discussion_id, message);
    let linked = match dispatch_id {
        Some(job_id) => conn
            .execute(
                "UPDATE messages SET agent_dispatch_job_id = ?2 WHERE id = ?1",
                rusqlite::params![message.id, job_id],
            )
            .map(|_| ())
            .map_err(anyhow::Error::from),
        None => Ok(()),
    };
    let cleared = clear_awaiting_after_terminal(conn, discussion_id, tracked_dispatch);
    inserted.and(linked).and(cleared)
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

/// Settle a tracked run that never reached the agent, saying why.
///
/// The reason used to be hard-coded to "agent execution preflight failed",
/// which is how a refusal reached `agent_dispatch_jobs.last_error` carrying
/// nothing an operator could act on — 76 rows of it in one instance. Each
/// caller names the condition it just detected instead.
fn finish_tracked_preflight(
    completion_tx: &mut Option<tokio::sync::oneshot::Sender<AgentExecutionOutcome>>,
    diagnostic: &str,
) {
    if let Some(sender) = completion_tx.take() {
        let _ = sender.send(AgentExecutionOutcome::PreflightFailed {
            diagnostic: diagnostic.to_string(),
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
    use super::{
        discussion_at_dispatch_trigger, independent_sibling_notice, load_quick_prompt_snapshot,
    };
    use crate::{
        core::execution_variables::VariableProvenance,
        models::{Discussion, PromptVariableSource},
    };

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

    #[test]
    fn quick_prompt_dispatch_uses_snapshot_provenance_after_prompt_mutation_and_deletion() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory database");
        crate::db::migrations::run(&conn).expect("schema");
        let key = [7_u8; 32];
        let now = chrono::Utc::now().to_rfc3339();
        let launch_variables = serde_json::json!([{
            "name": "__kronn_template_env__API_TOKEN",
            "label": "ordinary",
            "placeholder": "",
            "required": true,
            "source": "UserInput",
            "allow_manual_override": false
        }])
        .to_string();
        conn.execute(
            "INSERT INTO quick_prompts
             (id, name, icon, prompt_template, variables_json, agent, created_at, updated_at)
             VALUES ('qp-before-mutation', 'Before mutation', 'x', 'unused', ?1, 'Codex', ?2, ?2)",
            rusqlite::params![launch_variables, now],
        )
        .expect("Quick Prompt at launch");
        let values = std::collections::HashMap::from([
            (
                "__kronn_template_env__API_TOKEN".to_string(),
                "ordinary".to_string(),
            ),
            (
                "__kronn_template_env__API_TOKEN#2".to_string(),
                "environment".to_string(),
            ),
        ]);
        let provenance = vec![
            VariableProvenance {
                name: "__kronn_template_env__API_TOKEN".to_string(),
                source: PromptVariableSource::UserInput,
                source_ref: None,
                effective_source_ref: "user_input".to_string(),
                overridden: false,
            },
            VariableProvenance {
                name: "__kronn_template_env__API_TOKEN#2".to_string(),
                source: PromptVariableSource::ProjectEnv,
                source_ref: Some("<env.API_TOKEN>".to_string()),
                effective_source_ref: "mcp_config:cfg:<env.API_TOKEN>".to_string(),
                overridden: false,
            },
        ];
        crate::db::execution_variable_snapshots::insert(
            &conn,
            crate::db::execution_variable_snapshots::NewSnapshot {
                run_kind: "quick_prompt",
                run_id: "discussion-after-delete",
                project_id: None,
                environment_ref: "project_mcp_configs",
                resolved_at: chrono::Utc::now(),
                retention_days: 30,
                expires_at: None,
                values: &values,
                provenance: &provenance,
            },
            &key,
        )
        .expect("launch-time snapshot");
        // The launch declaration made the synthetic name end in `#2`.  A later
        // edit removes that declaration, so reading this mutable QP at dispatch
        // would instead look for the unsuffixed value and leave env placeholders.
        conn.execute(
            "UPDATE quick_prompts SET variables_json='[]' WHERE id='qp-before-mutation'",
            [],
        )
        .expect("mutate Quick Prompt after launch");

        let (values, provenance) =
            load_quick_prompt_snapshot(&conn, "discussion-after-delete", None, &key)
                .expect("load snapshot after mutation")
                .expect("snapshot remains after the Quick Prompt is edited");
        assert_eq!(
            crate::models::render_quick_prompt_template_from_snapshot(
                "{{__kronn_template_env__API_TOKEN}} {{env.API_TOKEN}} <env.API_TOKEN>",
                &values,
                &provenance,
            ),
            "ordinary environment environment"
        );

        crate::db::quick_prompts::delete_quick_prompt(&conn, "qp-before-mutation")
            .expect("delete Quick Prompt after launch");

        let (values, provenance) =
            load_quick_prompt_snapshot(&conn, "discussion-after-delete", None, &key)
                .expect("load snapshot")
                .expect("snapshot remains after the Quick Prompt is deleted");
        assert_eq!(
            crate::models::render_quick_prompt_template_from_snapshot(
                "{{__kronn_template_env__API_TOKEN}} {{env.API_TOKEN}} <env.API_TOKEN>",
                &values,
                &provenance,
            ),
            "ordinary environment environment"
        );
    }

    #[test]
    fn quick_prompt_snapshot_lookup_keeps_batch_compare_and_workflow_parent_paths() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory database");
        crate::db::migrations::run(&conn).expect("schema");
        let key = [8_u8; 32];
        let insert_snapshot = |kind: &str, run_id: &str, value: &str| {
            let values =
                std::collections::HashMap::from([("source".to_string(), value.to_string())]);
            crate::db::execution_variable_snapshots::insert(
                &conn,
                crate::db::execution_variable_snapshots::NewSnapshot {
                    run_kind: kind,
                    run_id,
                    project_id: None,
                    environment_ref: "project_mcp_configs",
                    resolved_at: chrono::Utc::now(),
                    retention_days: 30,
                    expires_at: None,
                    values: &values,
                    provenance: &[],
                },
                &key,
            )
            .expect("snapshot");
        };

        insert_snapshot("quick_prompt_batch_item", "batch-discussion", "batch");
        let (values, _) = load_quick_prompt_snapshot(&conn, "batch-discussion", None, &key)
            .expect("load batch snapshot")
            .expect("batch snapshot");
        assert_eq!(values.get("source"), Some(&"batch".to_string()));

        insert_snapshot("quick_prompt_compare", "compare-run", "compare");
        let (values, _) =
            load_quick_prompt_snapshot(&conn, "compare-discussion", Some("compare-run"), &key)
                .expect("load comparison snapshot")
                .expect("comparison snapshot");
        assert_eq!(values.get("source"), Some(&"compare".to_string()));

        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO workflows
             (id, name, trigger_json, steps_json, actions_json, safety_json, enabled, created_at, updated_at)
             VALUES ('workflow-for-snapshot', 'Workflow', '{}', '[]', '[]', '{}', 1, ?1, ?1)",
            [&now],
        )
        .expect("workflow");
        conn.execute(
            "INSERT INTO workflow_runs
             (id, workflow_id, status, step_results_json, started_at)
             VALUES ('workflow-parent', 'workflow-for-snapshot', 'Running', '[]', ?1)",
            [&now],
        )
        .expect("parent run");
        conn.execute(
            "INSERT INTO workflow_runs
             (id, workflow_id, status, step_results_json, started_at, parent_run_id)
             VALUES ('workflow-child', 'workflow-for-snapshot', 'Running', '[]', ?1, 'workflow-parent')",
            [&now],
        )
        .expect("child run");
        insert_snapshot("workflow", "workflow-parent", "parent");

        let (values, _) =
            load_quick_prompt_snapshot(&conn, "child-discussion", Some("workflow-child"), &key)
                .expect("load parent workflow snapshot")
                .expect("parent workflow snapshot");
        assert_eq!(values.get("source"), Some(&"parent".to_string()));
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

/// The messages written since the agent last saw the discussion.
///
/// `None` means "no trustworthy delta exists", and the caller must send the
/// full history. Two distinct cases return it, both on purpose:
///   - the marker is gone: the history no longer matches what the agent was
///     shown (edited, pruned, rebuilt), so any slice would be a guess;
///   - nothing follows it: there is nothing to say, and resuming would send an
///     empty turn.
///
/// Everything after the marker is returned, never just the last message: in a
/// room, the human or another agent routinely writes between two turns of the
/// same agent, and dropping that would be an invisible loss.
fn messages_not_yet_seen(
    messages: &[crate::models::DiscussionMessage],
    last_seen: &str,
) -> Option<Vec<crate::models::DiscussionMessage>> {
    let position = messages
        .iter()
        .position(|message| message.id == last_seen)?;
    let unseen = &messages[position + 1..];
    (!unseen.is_empty()).then(|| unseen.to_vec())
}

/// Decide whether this turn can continue the CLI's own conversation.
///
/// Kronn's default sends the ordinary bounded discussion prompt. When the
/// runtime still holds a proven conversation, the turn can instead carry only
/// the retained messages it has not seen. This reduces repeated input, but
/// does not establish any provider's caching or billing behavior.
///
/// Returns the prompt to send and, when resuming, the conversation to resume.
/// The two travel together on purpose: a delta prompt WITHOUT `--resume` loses
/// the history, and a full prompt WITH `--resume` states it twice. Every path
/// that cannot prove both is right returns the full prompt and no id.
/// Which connection a reply dispatches through: the one its dispatch job
/// carries, else the discussion's durable sticky target. The job wins so an
/// explicit one-off target (a mention, a retry against another connection)
/// is never silently replaced by the room's default.
/// What a connection whose target does not match the agent being started means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionMismatch {
    /// The job itself named this connection. A job that names a connection
    /// serving another agent is inconsistent, and starting it anyway would run
    /// the turn on something the caller did not ask for.
    Refuse,
    /// The connection was only inherited from the room. The room's sticky
    /// connection describes the room's EXTERNAL agent (KT-545); a sibling
    /// explicitly targeting a native CLI is not that agent, so the room's
    /// connection says nothing about it — absent, not wrong.
    ///
    /// Treating it as wrong refused the start of every native agent mentioned
    /// alongside an external one: mention @openrouter, @claudecode and
    /// @opencode on one message and only OpenRouter ever ran.
    Ignore,
}

pub(crate) fn connection_mismatch(inherited: bool) -> ConnectionMismatch {
    if inherited {
        ConnectionMismatch::Ignore
    } else {
        ConnectionMismatch::Refuse
    }
}

pub(crate) fn effective_connection_id<'a>(
    dispatch: Option<&'a String>,
    discussion: Option<&'a String>,
) -> Option<&'a String> {
    dispatch.or(discussion)
}

#[allow(clippy::too_many_arguments)]
async fn resume_with_delta_if_possible(
    store: &runner::AcpSessionStore,
    agent_type: &AgentType,
    work_dir: Option<&str>,
    project_path: &str,
    prompt_disc: &crate::models::Discussion,
    extra_context_len: usize,
    full_prompt: String,
    is_task_worker: bool,
) -> (String, Option<String>, Option<String>) {
    let checkpoint = prompt_disc
        .messages
        .last()
        .map(|message| message.id.clone());
    // A task worker opens on a fresh worktree with the task as its first turn.
    // Resuming a room's conversation there would hand it a history it has no
    // business seeing — and the runner refuses the id anyway, which would
    // leave the delta prompt travelling alone.
    let native_acp = runner::AcpSessionStore::tracks_native_delta(agent_type);
    // An enabled Claude/Codex adapter owns an ACP session too.  It must use
    // its adapter runtime key and the same proven delta/id pair; the old
    // direct-CLI key is not interchangeable with it.
    let adapted_acp = matches!(
        crate::acp::resolve_acp_route(agent_type),
        crate::acp::AcpProductionRoute::AdaptedAcp
    );
    let acp_delta = native_acp || adapted_acp;
    let cli_print = runner::AcpSessionStore::tracks_cli_print(agent_type);
    if is_task_worker || (!acp_delta && !cli_print) {
        return (full_prompt, None, checkpoint);
    }
    let Ok(scope) = runner::resolve_agent_work_dir(work_dir, project_path) else {
        return (full_prompt, None, checkpoint);
    };
    let Ok(Some(completed)) = store
        .load_completed_checkpoint(agent_type, &scope, !acp_delta)
        .await
    else {
        return (full_prompt, None, checkpoint);
    };
    let conversation_id = completed.conversation_id;
    // Claude can probe its own `--print --resume` session before shrinking the
    // prompt: `conversation_id` there names a `<id>.jsonl` file under
    // `~/.claude/projects/...`. That probe is meaningless once `acp_delta` is
    // true — the id loaded above then comes from the ACP adapter runtime key
    // (`claude_cli_adapter_v1`/`opencode_acp_v1`), never a `--print` file, so
    // testing it against the CLI-print store would always miss and silently
    // force a full prompt on every adapted/native turn. Native ACP exposes no
    // side-effect-free probe either, so the runner owns the full-prompt
    // fallback there when negotiated continuation reports a safe absence.
    if cli_print && !acp_delta && !runner::cli_print_session_is_resumable(&scope, &conversation_id)
    {
        return (full_prompt, None, checkpoint);
    }
    let Some(mut unseen) =
        messages_not_yet_seen(&prompt_disc.messages, &completed.input_message_id)
    else {
        return (full_prompt, None, checkpoint);
    };
    // Only the exact durable response of the completed turn is already in the
    // provider's conversation. Never skip every message by this agent: a
    // different CLI of the same provider may have written in the interval.
    if !unseen
        .iter()
        .any(|message| message.id == completed.output_message_id)
    {
        return (full_prompt, None, checkpoint);
    }
    unseen.retain(|message| message.id != completed.output_message_id);
    if unseen.is_empty() {
        return (full_prompt, None, checkpoint);
    }
    let mut delta_disc = prompt_disc.clone();
    delta_disc.messages = unseen;
    let delta_prompt = crate::api::disc_prompts::build_agent_delta_prompt(
        &delta_disc,
        agent_type,
        extra_context_len,
    );
    // A delta that saves nothing is not worth the divergence risk it carries.
    if delta_prompt.len() >= full_prompt.len() {
        return (full_prompt, None, checkpoint);
    }
    tracing::debug!(
        conversation_id = %conversation_id,
        saved_bytes = full_prompt.len() - delta_prompt.len(),
        "resuming the CLI conversation instead of replaying the discussion"
    );
    (delta_prompt, Some(conversation_id), checkpoint)
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
        finish_tracked_preflight(&mut completion_tx, "discussion not found");
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
            finish_tracked_preflight(&mut completion_tx, "discussion not found");
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
    // A reply with no dispatch job carries no connection (the non-dispatch
    // branch above fills it with None). The discussion holds the durable
    // sticky target for exactly this case (KT-545 DoD #4) — without this
    // fallback an ordinary reply in a room backed by an external connection
    // failed with "the selected external API connection is unavailable",
    // although the connection existed and was recorded on the discussion.
    // Whether the connection below was chosen for THIS dispatch or merely
    // inherited from the room. The two must not be treated alike on a mismatch:
    // see the `Ok(Some(_)) if inherited_connection` arm.
    let inherited_connection = dispatch_connection_id.is_none();
    let effective_connection_id =
        effective_connection_id(dispatch_connection_id.as_ref(), disc.connection_id.as_ref());
    let external_connection = if let Some(connection_id) = effective_connection_id {
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
            // The room's sticky connection describes the room's EXTERNAL agent
            // (KT-545). A sibling explicitly targeting a native CLI is not that
            // agent, and the room's connection says nothing about it — so it is
            // simply absent here, not wrong.
            //
            // Treating it as wrong refused the start of every native agent
            // mentioned alongside an external one: mention @openrouter,
            // @claudecode and @opencode on one message and only OpenRouter ever
            // ran, the other two vanishing with no visible reason.
            Ok(Some(_))
                if connection_mismatch(inherited_connection) == ConnectionMismatch::Ignore =>
            {
                None
            }
            Ok(Some(_)) => {
                finish_tracked_preflight(
                    &mut completion_tx,
                    "the selected external API connection no longer matches this agent target",
                );
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
                finish_tracked_preflight(
                    &mut completion_tx,
                    "the selected external API connection no longer exists",
                );
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
        finish_tracked_preflight(
            &mut completion_tx,
            "agent authentication preflight refused the start",
        );
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
            crate::http_transport::connection_tier_model(connection, disc_tier)
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
            finish_tracked_preflight(
                &mut completion_tx,
                "unable to resolve the native HTTP tool scope",
            );
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
            finish_tracked_preflight(
                &mut completion_tx,
                "unable to resolve the CLI task-worker delivery scope",
            );
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
                    finish_tracked_preflight(
                        &mut completion_tx,
                        "agent start refused before reaching the provider",
                    );
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
                    finish_tracked_preflight(
                        &mut completion_tx,
                        "agent start refused before reaching the provider",
                    );
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
                finish_tracked_preflight(
                    &mut completion_tx,
                    "agent start refused before reaching the provider",
                );
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
            let disk_ctx = crate::core::mcp_scanner::build_mcp_server_listing(&project_path);
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
                finish_tracked_preflight(
                    &mut completion_tx,
                    "Quick Prompt variable snapshot key unavailable",
                );
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
                finish_tracked_preflight(
                    &mut completion_tx,
                    "Quick Prompt variable snapshot key unavailable",
                );
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
        let snapshot = state
            .db
            .with_conn(move |conn| {
                load_quick_prompt_snapshot(conn, &disc_id, workflow_run_id.as_deref(), &key)
            })
            .await;
        let (values, provenance) = match snapshot {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) | Err(_) => {
                // A QP dispatch may never fall through with placeholders: it
                // would turn a failed preflight or expired snapshot into an
                // agent side effect with incomplete input.
                finish_tracked_preflight(
                    &mut completion_tx,
                    "Quick Prompt variable snapshot unavailable or expired",
                );
                let stream: SseStream = Box::pin(futures::stream::once(async {
                    Ok::<_, Infallible>(Event::default().event("error").data(
                        serde_json::json!({"error": "Quick Prompt variable snapshot unavailable or expired"}).to_string(),
                    ))
                }));
                return Sse::new(prepend_initial_event(stream, initial_event.take()));
            }
        };
        if let Some(first_message) = prompt_disc.messages.first_mut() {
            first_message.content = crate::models::render_quick_prompt_template_from_snapshot(
                &first_message.content,
                &values,
                &provenance,
            );
        }
    }
    let prompt = build_agent_prompt(&prompt_disc, &agent_type, extra_context_len);

    // KT-562 — the same discussion, re-narrated in full at every turn, is what
    // made Kronn slower than the same CLI driven by hand. Continue the
    // conversation the CLI already holds whenever that can be proven safe.
    let acp_session_store = runner::AcpSessionStore::new(state.db.clone(), discussion_id.clone());
    let full_prompt = prompt.clone();
    let (prompt, cli_resume_id, acp_progress_message_id) = resume_with_delta_if_possible(
        &acp_session_store,
        &agent_type,
        workspace_path.as_deref(),
        &project_path,
        &prompt_disc,
        extra_context_len,
        prompt,
        cli_task_worker_context.is_some(),
    )
    .await;

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

    let qp_reasoning_effort = if tier_override.is_none() && external_connection.is_none() {
        let did = discussion_id.clone();
        let agent = agent_type.clone();
        let model = attempted_model.clone();
        match state
            .db
            .with_read_conn(move |conn| {
                crate::db::discussion_effort::for_run(
                    conn,
                    &did,
                    &agent,
                    disc_tier,
                    model.as_deref(),
                )
            })
            .await
        {
            Ok(effort) => effort,
            Err(error) => {
                tracing::error!("Unable to read launch-time Quick Prompt effort: {error}");
                finish_tracked_preflight(
                    &mut completion_tx,
                    "unable to read the launch-time Quick Prompt effort",
                );
                let stream: SseStream = Box::pin(futures::stream::once(async move {
                    Ok::<_, Infallible>(Event::default().event("error").data(
                        serde_json::json!({"error": "quick_prompt_effort_snapshot_unavailable"}).to_string()
                    ))
                }));
                return Sse::new(prepend_initial_event(stream, initial_event.take()));
            }
        }
    } else {
        None
    };

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
        finish_tracked_preflight(
            &mut completion_tx,
            "model catalogue preflight refused this model",
        );
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
            reasoning_effort_override: qp_reasoning_effort.as_deref(),
            context_files_prompt: &context_files_prompt,
            // Forward to the agent process env so the kronn-internal MCP
            // bridge knows which discussion to introspect when called.
            discussion_id: Some(&discussion_id),
            acp_session_store: Some(acp_session_store.clone()),
            native_acp_full_prompt: Some(&full_prompt),
            cli_resume_id: cli_resume_id.as_deref(),
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
                // Scope a resumable conversation to the directory it ran in: a
                // moved or regenerated worktree must not resume a thread that
                // knew another tree. Captured here because the borrow of
                // `process` below outlives the read.
                let session_scope = process.work_dir.clone();
                let mut cli_session_persisted = false;
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
                                // Tell the client a tool STARTED, not only that
                                // one finished. A single Bash can run 80 s
                                // (issue 202's worst case), and until now that
                                // whole time was a frozen placeholder while
                                // Kronn already knew what was running.
                                if !client_gone {
                                    let _ = tx
                                        .send(AgentStreamEvent::Log {
                                            text: format!("→ {name}"),
                                        })
                                        .await;
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
                            runner::StreamJsonEvent::SessionId(session_id) => {
                                // Capture the CLI's init identity and invalidate
                                // the previous checkpoint. Only this store's
                                // successful durable response certifies the turn;
                                // an interrupted turn cannot supply a delta.
                                if !cli_session_persisted
                                    && runner::AcpSessionStore::tracks_cli_print(&agent_type)
                                {
                                    cli_session_persisted = true;
                                    if let Err(error) = acp_session_store
                                        .persist_cli_print(&agent_type, &session_scope, &session_id)
                                        .await
                                    {
                                        // Losing the id costs a full-history
                                        // turn next time, never correctness.
                                        tracing::warn!(
                                            disc_id = %discussion_id,
                                            "Unable to persist the CLI conversation id: {error}"
                                        );
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

                lift_acp_tool_calls(&stderr_lines, &mut kronn_tool_calls, &mut native_tool_calls);

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
                let checkpoint = if child_run_was_success {
                    runner::resolve_agent_work_dir(workspace_path.as_deref(), &project_path)
                        .ok()
                        .zip(acp_progress_message_id.as_deref())
                        .and_then(|(scope, input_id)| {
                            let acp_delta =
                                runner::AcpSessionStore::tracks_native_delta(&agent_type)
                                    || matches!(
                                        crate::acp::resolve_acp_route(&agent_type),
                                        crate::acp::AcpProductionRoute::AdaptedAcp
                                    );
                            acp_session_store.completion_checkpoint(
                                &agent_type,
                                &scope,
                                !acp_delta,
                                input_id,
                                &agent_msg.id,
                            )
                        })
                } else {
                    None
                };
                match state
                    .db
                    .with_conn(move |conn| {
                        crate::db::discussions::insert_native_agent_message_with_checkpoint(
                            conn,
                            &did,
                            &msg,
                            child_run_was_success,
                            dispatch_id.as_deref(),
                            &source_agent,
                            &candidate_handoffs,
                            handoffs_enabled,
                            handoff_paid_limit,
                            checkpoint.as_ref(),
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
                let error_dispatch_id = dispatch_job_id.clone();
                let err_msg_fed = err_msg.clone();
                if let Err(db_err) = state
                    .db
                    .with_conn(move |conn| {
                        persist_agent_start_error(
                            conn,
                            &did,
                            &err_msg,
                            error_dispatch_id.as_deref(),
                            tracked_dispatch,
                        )
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
                                    // Same reason as the discussion loop: a
                                    // debate round showed a frozen "thinking"
                                    // line for the whole of a long tool call.
                                    if !tx.is_closed() {
                                        let _ = tx.send(AgentStreamEvent::Log {
                                            text: format!("→ {name}"),
                                        }).await;
                                    }
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
                                // A debate round is scored on its own; rounds do
                                // not resume one another's conversation.
                                runner::StreamJsonEvent::SessionId(_)
                                | runner::StreamJsonEvent::Skip => {}
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
        connection_mismatch, effective_global_timeout, effective_stall_timeout,
        finish_tracked_preflight, lift_acp_tool_calls, AgentExecutionOutcome, ConnectionMismatch,
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
    fn a_rooms_connection_never_refuses_a_native_sibling() {
        // Reported twice from real rooms: three agents mentioned, three
        // placeholders, then only the external one ever answers. The database
        // said why the moment the refusal carried its reason —
        // "the selected external API connection no longer matches this agent
        // target" on every ClaudeCode and OpenCode job in the room.
        //
        // The room was backed by OpenRouter, so its sticky connection serves
        // AgentType::Custom. A sibling targeting a native CLI inherited it and
        // was refused for not being the agent the connection serves.
        assert_eq!(
            connection_mismatch(true),
            ConnectionMismatch::Ignore,
            "a connection inherited from the room says nothing about a native target"
        );
        assert_eq!(
            connection_mismatch(false),
            ConnectionMismatch::Refuse,
            "but a job that NAMES a connection serving another agent is inconsistent \
             and must not be started on something the caller did not ask for"
        );
    }

    #[test]
    fn an_acp_tool_call_lands_in_the_group_under_the_reply() {
        // Reported from a real room: messages read
        // "Je vais essayer d'accéder à ce lien.[ClaudeCode tool: ToolSearch]
        // [ClaudeCode tool: WebFetch]Réponse courte : ..." — the calls inside
        // the sentence, and nothing in the group below where they belong.
        let stderr = vec![
            format!("{}ToolSearch", crate::agents::runner::ACP_TOOL_MARKER),
            format!(
                "{}mcp__kronn-internal__disc_append",
                crate::agents::runner::ACP_TOOL_MARKER
            ),
            "a plain diagnostic line that is not a tool call".to_string(),
        ];
        let mut kronn = Vec::new();
        let mut native = Vec::new();
        lift_acp_tool_calls(&stderr, &mut kronn, &mut native);

        assert_eq!(
            native,
            vec!["[agent-native: ToolSearch()]".to_string()],
            "an agent's own tool is native"
        );
        assert_eq!(
            kronn,
            vec!["[kronn-internal: disc_append()]".to_string()],
            "and Kronn's own stays in its bucket, as on the CLI path"
        );
    }

    #[test]
    fn what_the_runtime_writes_is_what_the_transcript_reads() {
        // The two halves live in different files (runner.rs emits, this one
        // lifts). A silent format drift between them would put the calls
        // nowhere at all, with nothing failing.
        let emitted = format!("{}Read", crate::agents::runner::ACP_TOOL_MARKER);
        let mut kronn = Vec::new();
        let mut native = Vec::new();
        lift_acp_tool_calls(&[emitted], &mut kronn, &mut native);
        assert_eq!(native.len(), 1, "the emitted shape must be recognised");
    }

    #[test]
    fn ordinary_stderr_is_never_mistaken_for_a_tool_call() {
        let mut kronn = Vec::new();
        let mut native = Vec::new();
        lift_acp_tool_calls(
            &[
                "ACP transport failed: broken pipe".to_string(),
                String::new(),
                // The marker with nothing after it is not a call either.
                crate::agents::runner::ACP_TOOL_MARKER.to_string(),
            ],
            &mut kronn,
            &mut native,
        );
        assert!(kronn.is_empty() && native.is_empty());
    }

    #[tokio::test]
    async fn a_tracked_preflight_reports_the_condition_it_detected() {
        // The second path into `PreflightFailed`. It hard-coded "agent
        // execution preflight failed" for all fifteen of its call sites, so a
        // refusal reached agent_dispatch_jobs.last_error carrying nothing —
        // 76 such rows in one instance, none of them diagnosable.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut slot = Some(tx);
        finish_tracked_preflight(
            &mut slot,
            "the selected external API connection no longer exists",
        );
        assert!(slot.is_none(), "the sender is consumed exactly once");
        let outcome = rx.await.expect("the tracked run must be settled");
        let AgentExecutionOutcome::PreflightFailed { diagnostic } = outcome else {
            panic!("a refused preflight must not be reported as anything else");
        };
        assert_eq!(
            diagnostic,
            "the selected external API connection no longer exists"
        );
        assert_ne!(diagnostic, "agent execution preflight failed");
    }

    #[tokio::test]
    async fn finishing_a_preflight_twice_settles_it_once() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut slot = Some(tx);
        finish_tracked_preflight(&mut slot, "discussion not found");
        // A second call must be a no-op rather than a panic on a taken sender.
        finish_tracked_preflight(&mut slot, "something else entirely");
        let AgentExecutionOutcome::PreflightFailed { diagnostic } = rx.await.unwrap() else {
            panic!("unexpected outcome");
        };
        assert_eq!(
            diagnostic, "discussion not found",
            "the first reason stands"
        );
    }

    #[test]
    fn a_refused_preflight_says_what_it_refused_on() {
        // Two agents vanished from a room twenty seconds after being mentioned,
        // and the only trace was "agent execution preflight failed" — a
        // sentence nobody can act on. Finding the cause meant reading the
        // database. Every refusal now carries the reason Kronn already knew.
        for (agent, error) in [
            (AgentType::ClaudeCode, "Project path not found: /gone/repo"),
            (AgentType::OpenCode, "Project path not found: /gone/repo"),
            (AgentType::Custom, "connection has no endpoint"),
        ] {
            let outcome = agent_start_failure_outcome(&agent, error);
            let AgentExecutionOutcome::PreflightFailed { diagnostic } = outcome else {
                panic!("{agent:?}: a settled refusal must not be deferred as retryable");
            };
            assert_eq!(
                diagnostic, error,
                "{agent:?}: the refusal must carry what it refused on",
            );
            assert_ne!(diagnostic, "agent execution preflight failed");
        }
    }

    #[test]
    fn a_retryable_outage_is_still_told_apart_from_a_settled_refusal() {
        // Keeping the reason must not blur the decision that matters: a missing
        // binary stays deferred, because it can appear between two attempts.
        assert!(matches!(
            agent_start_failure_outcome(&AgentType::Codex, "Binary 'codex' not found"),
            AgentExecutionOutcome::RuntimeUnavailable { .. }
        ));
        assert!(matches!(
            agent_start_failure_outcome(&AgentType::ClaudeCode, "Project path not found: /x"),
            AgentExecutionOutcome::PreflightFailed { .. }
        ));
    }

    #[test]
    fn http_agents_surface_runtime_outages_but_cli_runtime_absence_stays_deferred() {
        // The classification is what changes here — retryable or not — never
        // whether the operator gets to read why.
        assert_eq!(
            agent_start_failure_outcome(&AgentType::LiteLlm, "LiteLLM unreachable at http://proxy"),
            AgentExecutionOutcome::PreflightFailed {
                diagnostic: "LiteLLM unreachable at http://proxy".into()
            }
        );
        assert_eq!(
            agent_start_failure_outcome(&AgentType::Ollama, "Ollama unreachable at localhost"),
            AgentExecutionOutcome::PreflightFailed {
                diagnostic: "Ollama unreachable at localhost".into()
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
    fn a_nul_byte_in_the_command_line_is_a_hard_stop_not_a_deferral() {
        // Issue 201: this exact refusal was deferred and replayed 282 times,
        // every 30 s. A NUL byte does not go away by waiting.
        let kronn_check = "npx cannot start: environment variable ANTHROPIC_API_KEY contains a \
                           NUL byte, which the operating system refuses in a command line.";
        assert_eq!(
            agent_start_failure_outcome(&AgentType::ClaudeCode, kronn_check),
            AgentExecutionOutcome::PreflightFailed {
                diagnostic: kronn_check.into()
            },
            "the carrier's name must reach the operator, not a generic preflight message"
        );

        // The OS wording, in case a NUL slips past the pre-spawn check.
        let os_message = "Spawn failed for npx: nul byte found in provided data";
        assert_eq!(
            agent_start_failure_outcome(&AgentType::ClaudeCode, os_message),
            AgentExecutionOutcome::PreflightFailed {
                diagnostic: os_message.into()
            }
        );

        // And a genuinely transient absence still defers, as before.
        assert_eq!(
            agent_start_failure_outcome(&AgentType::ClaudeCode, "Binary 'claude' not found"),
            AgentExecutionOutcome::RuntimeUnavailable {
                reason: "Binary 'claude' not found".into()
            }
        );
    }

    #[test]
    fn kt633_nul_failure_persists_a_visible_error_and_settles_the_dispatch() {
        use crate::db::agent_dispatch::{self, DispatchStatus, NewAgentDispatchJob};
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at, awaiting_agent)
            VALUES ('nul-room', 'NUL fixture', datetime('now'), datetime('now'), 1)",
            [],
        )
        .unwrap();
        let mut trigger = crate::api::orchestration::orchestrator_message(
            "nul-trigger".into(),
            "question".into(),
        );
        trigger.role = MessageRole::User;
        crate::db::discussions::insert_message(&conn, "nul-room", &trigger).unwrap();
        agent_dispatch::enqueue(
            &conn,
            NewAgentDispatchJob {
                id: "nul-job",
                discussion_id: "nul-room",
                trigger_message_id: "nul-trigger",
                trigger_sort_order: 0,
                dedupe_key: "nul-once",
                agent_override: Some(&AgentType::ClaudeCode),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap();
        agent_dispatch::claim(&conn, "nul-job").unwrap().unwrap();
        let error = "Spawn failed for npx: nul byte found in provided data";
        let AgentExecutionOutcome::PreflightFailed { diagnostic } =
            agent_start_failure_outcome(&AgentType::ClaudeCode, error)
        else {
            panic!("a deterministic NUL failure must not defer");
        };
        let content = agent_start_error_content(
            &AgentType::ClaudeCode,
            None,
            crate::models::ModelTier::Default,
            "en",
            &diagnostic,
            Some("nul-job"),
        )
        .unwrap_or_else(|| format!("Erreur: {diagnostic}"));
        let mut message =
            crate::api::orchestration::orchestrator_message("nul-error".into(), content);
        message.role = MessageRole::System;
        message.agent_type = Some(AgentType::ClaudeCode);
        super::persist_agent_start_error(&conn, "nul-room", &message, Some("nul-job"), true)
            .unwrap();
        crate::api::discussions::runtime::persist_dispatch_settlement(
            &conn,
            "nul-job",
            "nul-room",
            None,
            crate::db::workflows::BatchChildOutcome::Failed,
            Some(&diagnostic),
            None,
        )
        .unwrap();
        let job = agent_dispatch::get(&conn, "nul-job").unwrap().unwrap();
        assert_eq!(job.status, DispatchStatus::Failed);
        assert_eq!(job.attempts, 1);
        assert_eq!(job.last_error.as_deref(), Some(error));
        assert!(agent_dispatch::list_runnable_ids(&conn, 10)
            .unwrap()
            .is_empty());
        assert!(!agent_dispatch::defer_runtime_unavailable(&conn, "nul-job", 30, error).unwrap());
        let (role, content, dispatch_id): (String, String, String) = conn
            .query_row(
                "SELECT role, content, agent_dispatch_job_id FROM messages WHERE id='nul-error'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(role, "System");
        assert!(content.contains(error));
        assert_eq!(dispatch_id, "nul-job");
        let awaiting: bool = conn
            .query_row(
                "SELECT awaiting_agent FROM discussions WHERE id='nul-room'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!awaiting);
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
        // ToolStart → ToolInputDelta → ToolEnd produces TWO Log events — the
        // tool starting, then the human-readable breadcrumb once it is done —
        // and neither pollutes the response text.
        //
        // The start event was added deliberately: emitting only on ToolEnd
        // left the UI frozen for the entire duration of a call (80 s at worst
        // in issue 202) while Kronn already knew which tool was running.
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
            2,
            "one Log when the Read tool starts, one when it completes"
        );
        if let AgentStreamEvent::Log { text } = &logs[0] {
            assert_eq!(
                text, "→ Read",
                "the start event announces the tool and nothing else yet"
            );
        }
        if let AgentStreamEvent::Log { text } = &logs[1] {
            assert!(text.contains("Read"), "log should name the tool: {text}");
            assert_ne!(
                text, "→ Read",
                "the completion event must be the breadcrumb, not a repeat of the start"
            );
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
mod resume_delta_tests {
    //! KT-562 — what a resumed turn actually sends. Every wrong answer here is
    //! an invisible one: the agent replies confidently to a history it was
    //! never given.
    use super::runner;
    use super::{messages_not_yet_seen, resume_with_delta_if_possible};
    use crate::agents::runner::AcpSessionStore;
    use crate::models::{AgentType, DiscussionMessage, MessageChannel, MessageRole};
    use chrono::Utc;

    fn message(id: &str, role: MessageRole) -> DiscussionMessage {
        DiscussionMessage {
            id: id.to_string(),
            role,
            channel: MessageChannel::Main,
            content: format!("content of {id}"),
            agent_type: Some(AgentType::ClaudeCode),
            timestamp: Utc::now(),
            tokens_used: 0,
            session_tokens_at_message: None,
            recovered_partial: false,
            auth_mode: None,
            model_tier: None,
            model: None,
            cost_usd: None,
            author_pseudo: None,
            author_avatar_email: None,
            author_cli_ordinal: None,
            source_msg_id: None,
            duration_ms: None,
            lint_report: None,
            target_agent: None,
            reply_to_message_id: None,
        }
    }

    fn seed_completed_checkpoint(
        conn: &rusqlite::Connection,
        discussion_id: &str,
        agent_type: &str,
        runtime: &str,
        scope: &str,
        session_id: &str,
    ) -> anyhow::Result<()> {
        use crate::db::acp_runtime_sessions::{self, SessionKey, TurnCompletion};
        let key = SessionKey {
            discussion_id: discussion_id.into(),
            agent_type: agent_type.into(),
            runtime: runtime.into(),
            project_scope: scope.into(),
        };
        acp_runtime_sessions::begin_turn(conn, &key, session_id, "fixture-completed-turn")?;
        assert!(acp_runtime_sessions::complete_turn(
            conn,
            &TurnCompletion {
                key,
                turn_id: "fixture-completed-turn".into(),
                input_message_id: "m1".into(),
                output_message_id: "own-reply".into(),
            }
        )?);
        Ok(())
    }

    #[test]
    fn delta_rendering_preserves_peer_messages_and_ignores_old_summary_offsets() {
        let disc: crate::models::Discussion = serde_json::from_value(serde_json::json!({
            "id": "render-delta", "title": "Delta", "agent": "OpenCode", "language": "en",
            "participants": ["OpenCode"], "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "messages": [message("peer", MessageRole::Agent), message("human", MessageRole::User)],
            "pin_first_message": true, "summary_cache": "OLD_ALREADY_SEEN_SUMMARY", "summary_up_to_msg_idx": 90
        })).unwrap();
        let prompt =
            crate::api::disc_prompts::build_agent_delta_prompt(&disc, &AgentType::OpenCode, 0);
        assert!(prompt.contains("content of peer"));
        assert!(prompt.contains("content of human"));
        assert!(!prompt.contains("OLD_ALREADY_SEEN_SUMMARY"));
        assert!(!prompt.contains("INSTRUCTIONS DU PROTOCOLE"));
    }

    #[test]
    fn delta_rendering_does_not_silently_truncate_an_unseen_message_to_fit_the_budget() {
        let mut first = message("unseen-first", MessageRole::User);
        first.content.push_str(&"x".repeat(1000));
        let disc: crate::models::Discussion = serde_json::from_value(serde_json::json!({
            "id": "render-delta", "title": "Delta", "agent": "OpenCode", "language": "en",
            "participants": ["OpenCode"], "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "messages": [first, message("unseen-last", MessageRole::User)]
        })).unwrap();
        let full = super::build_agent_prompt(&disc, &AgentType::OpenCode, usize::MAX);
        let delta = crate::api::disc_prompts::build_agent_delta_prompt(
            &disc,
            &AgentType::OpenCode,
            usize::MAX,
        );
        assert!(!full.contains("content of unseen-first"));
        assert!(delta.contains("content of unseen-first"));
        assert!(delta.contains("content of unseen-last"));
        // The discussion's size gate must reject this oversized delta and
        // use the honest bounded full prompt, not certify omitted input.
        assert!(delta.len() >= full.len());
    }

    #[test]
    fn everything_written_since_the_marker_travels_not_only_the_last_message() {
        // The case that makes "just send the new message" wrong: between the
        // agent's own reply and its next turn, the human AND another agent
        // wrote. Both must reach it.
        let history = vec![
            message("m1", MessageRole::User),
            message("m2", MessageRole::Agent),
            message("m3", MessageRole::User),
            message("m4", MessageRole::Agent),
            message("m5", MessageRole::User),
        ];
        let unseen = messages_not_yet_seen(&history, "m2").expect("a delta exists");
        let ids: Vec<_> = unseen.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["m3", "m4", "m5"]);
    }

    #[test]
    fn a_marker_that_left_the_history_forces_the_full_prompt() {
        // Edited away, pruned, discussion rebuilt: the slice would be a guess,
        // so there must be no slice at all.
        let history = vec![message("m1", MessageRole::User)];
        assert!(messages_not_yet_seen(&history, "gone").is_none());
        assert!(messages_not_yet_seen(&[], "m1").is_none());
    }

    #[test]
    fn nothing_new_since_the_marker_is_not_a_delta() {
        // Resuming here would send an empty turn.
        let history = vec![
            message("m1", MessageRole::User),
            message("m2", MessageRole::Agent),
        ];
        assert!(messages_not_yet_seen(&history, "m2").is_none());
    }

    #[test]
    fn the_marker_itself_is_never_repeated() {
        // It is the last message the agent SAW, so re-sending it would show it
        // its own reply a second time.
        let history = vec![
            message("m1", MessageRole::User),
            message("m2", MessageRole::Agent),
            message("m3", MessageRole::User),
        ];
        let unseen = messages_not_yet_seen(&history, "m2").expect("a delta exists");
        assert_eq!(unseen.len(), 1);
        assert_eq!(unseen[0].id, "m3");
    }

    #[tokio::test]
    async fn delta_decision_checkpoints_the_input_boundary_not_the_future_reply() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        let store = AcpSessionStore::new(db.clone(), "delta-discussion");
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('delta-discussion', 'Delta', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let scope = crate::agents::runner::resolve_agent_work_dir(Some("."), ".").unwrap();
        let scope_for_db = scope.to_string_lossy().into_owned();
        db.with_conn(move |conn| {
            seed_completed_checkpoint(
                conn,
                "delta-discussion",
                "OpenCode",
                "opencode_acp_v1",
                &scope_for_db,
                "native-id",
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let discussion: crate::models::Discussion = serde_json::from_value(serde_json::json!({
            "id": "delta-discussion",
            "project_id": null,
            "title": "Delta",
            "agent": "OpenCode",
            "language": "en",
            "participants": ["OpenCode"],
            "messages": [message("m1", MessageRole::User), message("own-reply", MessageRole::Agent), message("m2", MessageRole::User)],
            "message_count": 3,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        let (prompt, resume_id, checkpoint) = resume_with_delta_if_possible(
            &store,
            &AgentType::OpenCode,
            Some("."),
            ".",
            &discussion,
            0,
            "full prompt with m1 and m2 ".repeat(200),
            false,
        )
        .await;
        assert_eq!(resume_id.as_deref(), Some("native-id"));
        assert_eq!(checkpoint.as_deref(), Some("m2"));
        assert!(prompt.contains("content of m2"));
    }

    #[tokio::test]
    async fn delta_without_a_size_gain_keeps_the_full_prompt_and_declines_resume() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        let store = AcpSessionStore::new(db.clone(), "delta-no-gain");
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('delta-no-gain', 'Delta', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let scope = crate::agents::runner::resolve_agent_work_dir(Some("."), ".").unwrap();
        let scope_for_db = scope.to_string_lossy().into_owned();
        db.with_conn(move |conn| {
            seed_completed_checkpoint(
                conn,
                "delta-no-gain",
                "OpenCode",
                "opencode_acp_v1",
                &scope_for_db,
                "native-id",
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let discussion: crate::models::Discussion = serde_json::from_value(serde_json::json!({
            "id": "delta-no-gain",
            "project_id": null,
            "title": "Delta",
            "agent": "OpenCode",
            "language": "en",
            "participants": ["OpenCode"],
            "messages": [message("m1", MessageRole::User), message("own-reply", MessageRole::Agent), message("m2", MessageRole::User)],
            "message_count": 3,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        let full_prompt = "short full prompt".to_owned();
        let (prompt, resume_id, checkpoint) = resume_with_delta_if_possible(
            &store,
            &AgentType::OpenCode,
            Some("."),
            ".",
            &discussion,
            0,
            full_prompt.clone(),
            false,
        )
        .await;
        assert_eq!(prompt, full_prompt);
        assert_eq!(resume_id, None);
        assert_eq!(checkpoint.as_deref(), Some("m2"));
    }

    /// KT-621 review — the CLI-print resumability probe checks for a
    /// `--print --resume` `<id>.jsonl` file, which an ACP-adapter session id
    /// never has. Before the fix that probe still ran whenever
    /// `AcpSessionStore::tracks_cli_print` was true (Claude), regardless of
    /// which route actually produced the id, so it always missed for an
    /// adapted Claude session and silently forced a full prompt on every
    /// turn — the adapter's delta continuation never fired.
    #[tokio::test]
    #[serial_test::serial(acp_adapter_env_toggle)]
    async fn adapted_claude_resume_is_not_gated_by_the_print_resume_file_probe() {
        struct RestoreAdapterEnv(Option<std::ffi::OsString>);
        impl Drop for RestoreAdapterEnv {
            fn drop(&mut self) {
                if let Some(value) = self.0.as_ref() {
                    std::env::set_var("KRONN_ACP_ADAPTER_CLAUDE", value);
                } else {
                    std::env::remove_var("KRONN_ACP_ADAPTER_CLAUDE");
                }
            }
        }
        let _restore = RestoreAdapterEnv(std::env::var_os("KRONN_ACP_ADAPTER_CLAUDE"));
        std::env::set_var("KRONN_ACP_ADAPTER_CLAUDE", "1");

        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        let store = AcpSessionStore::new(db.clone(), "adapted-claude-disc");
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('adapted-claude-disc', 'Adapted', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let scope = crate::agents::runner::resolve_agent_work_dir(Some("."), ".").unwrap();
        let scope_for_db = scope.to_string_lossy().into_owned();
        db.with_conn(move |conn| {
            seed_completed_checkpoint(
                conn,
                "adapted-claude-disc",
                "ClaudeCode",
                "claude_cli_adapter_v1",
                &scope_for_db,
                // Never a real `--print --resume` conversation id — an
                // adapter-minted id has no matching `.jsonl` file anywhere,
                // by construction, so this proves the probe was skipped
                // rather than having happened to pass.
                "adapter-session-with-no-print-file",
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let discussion: crate::models::Discussion = serde_json::from_value(serde_json::json!({
            "id": "adapted-claude-disc", "project_id": null, "title": "Adapted", "agent": "ClaudeCode",
            "language": "en", "participants": ["ClaudeCode"],
            "messages": [message("m1", MessageRole::User), message("own-reply", MessageRole::Agent), message("m2", MessageRole::User)],
            "message_count": 3,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        let (prompt, resume_id, checkpoint) = resume_with_delta_if_possible(
            &store,
            &AgentType::ClaudeCode,
            Some("."),
            ".",
            &discussion,
            0,
            "full prompt with m1 and m2 ".repeat(200),
            false,
        )
        .await;

        assert_eq!(
            resume_id.as_deref(),
            Some("adapter-session-with-no-print-file"),
            "the adapter's own recorded session must be resumed without a --print file probe"
        );
        assert_eq!(checkpoint.as_deref(), Some("m2"));
        assert!(prompt.contains("content of m2"));
        assert!(!prompt.contains("content of m1"));
    }

    /// KT-621 — the production chain, not the helper in isolation: the exact
    /// decision function above, feeding the exact `start_agent_with_config`
    /// NativeAcp route (only its I/O boundary is a fixture), against an
    /// on-disk database closed and reopened between turns — the way a
    /// backend restart or a second HTTP request actually behaves. A peer
    /// message written between turns must survive the delta untouched, and
    /// the runtime's own reply from turn 1 must never come back.
    #[tokio::test]
    async fn two_turns_through_the_real_decision_and_spawn_chain_survive_a_db_reopen() {
        use crate::acp::{
            AcpAgent, AcpCapability, AcpConfigOption, AcpError as TransportError, AcpInitialize,
            AcpNegotiatedCapabilities, AcpSessionEvent, AcpSessionTarget, AcpTransport,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex as StdMutex;

        struct ChainTransport {
            created: AtomicUsize,
            resumed: AtomicUsize,
            prompts: StdMutex<Vec<String>>,
        }

        #[async_trait::async_trait]
        impl AcpTransport for ChainTransport {
            async fn initialize(
                &self,
                _: AcpInitialize,
            ) -> Result<AcpNegotiatedCapabilities, TransportError> {
                Ok(AcpNegotiatedCapabilities {
                    protocol_version: 1,
                    capabilities: std::collections::BTreeSet::from([
                        AcpCapability::Sessions,
                        AcpCapability::Resume,
                        AcpCapability::Streaming,
                        AcpCapability::Cancellation,
                        AcpCapability::McpInjection,
                    ]),
                })
            }
            async fn create_session(&self) -> Result<AcpSessionTarget, TransportError> {
                self.created.fetch_add(1, Ordering::SeqCst);
                AcpSessionTarget::new(AcpAgent::OpenCode, "chain-native-session")
            }
            async fn config_options(&self) -> Vec<AcpConfigOption> {
                Vec::new()
            }
            async fn set_config_option(
                &self,
                _: &AcpSessionTarget,
                _: &str,
                _: &str,
            ) -> Result<(), TransportError> {
                Ok(())
            }
            async fn resume_session(&self, _: &AcpSessionTarget) -> Result<(), TransportError> {
                self.resumed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            async fn prompt(
                &self,
                _: &AcpSessionTarget,
                prompt: &str,
                events: tokio::sync::mpsc::Sender<AcpSessionEvent>,
            ) -> Result<(), TransportError> {
                self.prompts.lock().unwrap().push(prompt.to_owned());
                events
                    .send(AcpSessionEvent::TextDelta("kt621-own-native-answer".into()))
                    .await
                    .unwrap();
                events.send(AcpSessionEvent::Completed).await.unwrap();
                Ok(())
            }
            async fn cancel(&self, _: &AcpSessionTarget) -> Result<(), TransportError> {
                Ok(())
            }
            async fn shutdown(&self) -> Result<(), TransportError> {
                Ok(())
            }
        }

        let fixture = std::sync::Arc::new(ChainTransport {
            created: AtomicUsize::new(0),
            resumed: AtomicUsize::new(0),
            prompts: StdMutex::new(Vec::new()),
        });
        let transport: std::sync::Arc<dyn AcpTransport> = fixture.clone();

        let project = tempfile::tempdir().unwrap();
        let project_path = project.path().to_str().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("chain.db");
        let tokens = crate::models::setup::TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: Vec::new(),
            disabled_overrides: Vec::new(),
        };
        let scope = runner::resolve_agent_work_dir(Some(project_path), project_path).unwrap();

        let db = std::sync::Arc::new(crate::db::Database::open_path(&db_path).unwrap());
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, agent, created_at, updated_at)
                 VALUES ('chain-disc', 'Chain', 'OpenCode', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )?;
            let mut first = message("m1", MessageRole::User);
            first.content = "content of m1 ".repeat(200);
            crate::db::discussions::insert_message(conn, "chain-disc", &first)?;
            Ok(())
        })
        .await
        .unwrap();
        let store = AcpSessionStore::new(db.clone(), "chain-disc");

        // --- Turn 1: nothing recorded yet — must send the full transcript
        // and create a fresh native session.
        let discussion_t1 = db
            .with_conn(|conn| crate::db::discussions::get_discussion(conn, "chain-disc"))
            .await
            .unwrap()
            .unwrap();
        let full_prompt_t1 = super::build_agent_prompt(&discussion_t1, &AgentType::OpenCode, 0);
        let (prompt1, resume_id1, checkpoint1) = resume_with_delta_if_possible(
            &store,
            &AgentType::OpenCode,
            Some(project_path),
            project_path,
            &discussion_t1,
            0,
            full_prompt_t1,
            false,
        )
        .await;
        assert_eq!(resume_id1, None);
        assert_eq!(checkpoint1.as_deref(), Some("m1"));

        let mut turn1 = runner::start_agent_with_config(runner::AgentStartConfig {
            cli_resume_id: resume_id1.as_deref(),
            acp_session_store: Some(store.clone()),
            discussion_id: Some("chain-disc"),
            test_acp_transport: Some(transport.clone()),
            ..runner::AgentStartConfig::new(&AgentType::OpenCode, project_path, &prompt1, &tokens)
        })
        .await
        .unwrap();
        while turn1.next_line().await.is_some() {}
        assert!(turn1.child.wait().await.unwrap().success());
        let completion1 = store
            .completion_checkpoint(
                &AgentType::OpenCode,
                &scope,
                false,
                checkpoint1.as_deref().unwrap(),
                "own-reply-1",
            )
            .unwrap();
        db.with_conn(move |conn| {
            // Another agent writes after the input snapshot, but before our
            // reply is persisted. A cursor at the reply would lose this peer.
            let mut peer = message("peer-before-reply", MessageRole::Agent);
            // A different CLI of the same provider is still unseen input.
            peer.agent_type = Some(AgentType::OpenCode);
            crate::db::discussions::insert_message(conn, "chain-disc", &peer)?;
            let mut reply = message("own-reply-1", MessageRole::Agent);
            reply.agent_type = Some(AgentType::OpenCode);
            reply.content = "kt621-own-native-answer".into();
            crate::db::discussions::insert_native_agent_message_with_checkpoint(
                conn,
                "chain-disc",
                &reply,
                true,
                None,
                &AgentType::OpenCode,
                &[],
                false,
                None,
                Some(&completion1),
            )?;
            crate::db::discussions::insert_message(
                conn,
                "chain-disc",
                &message("m2", MessageRole::User),
            )?;
            Ok(())
        })
        .await
        .unwrap();
        // This is the same atomic response/checkpoint writer used by the
        // streaming handler, not an independently mirrored cursor update.
        assert_eq!(fixture.created.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.resumed.load(Ordering::SeqCst), 0);

        // --- Reopen the database from disk: the session id and the cursor
        // must both survive a reconnect, not just live in the open handle.
        drop(turn1);
        drop(store);
        assert_eq!(std::sync::Arc::strong_count(&db), 1);
        drop(db);
        let db2 = std::sync::Arc::new(crate::db::Database::open_path(&db_path).unwrap());
        let store2 = AcpSessionStore::new(db2.clone(), "chain-disc");

        // --- Turn 2: carry both the interleaved peer and the next human
        // message, but never our own reply already held by the runtime.
        let discussion_t2 = db2
            .with_conn(|conn| crate::db::discussions::get_discussion(conn, "chain-disc"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(discussion_t2.messages.len(), 4);
        let full_prompt_t2 = super::build_agent_prompt(&discussion_t2, &AgentType::OpenCode, 0);
        let (prompt2, resume_id2, checkpoint2) = resume_with_delta_if_possible(
            &store2,
            &AgentType::OpenCode,
            Some(project_path),
            project_path,
            &discussion_t2,
            0,
            full_prompt_t2.clone(),
            false,
        )
        .await;
        assert_eq!(resume_id2.as_deref(), Some("chain-native-session"));
        assert_eq!(checkpoint2.as_deref(), Some("m2"));
        assert!(prompt2.contains("content of m2"));
        assert!(
            prompt2.contains("content of peer-before-reply"),
            "interleaved peer must survive: {prompt2}"
        );
        assert!(
            !prompt2.contains("kt621-own-native-answer"),
            "the real transcript includes our reply, but the resumed runtime already has it"
        );
        assert!(
            !prompt2.contains("content of m1"),
            "the delta must not resend what the runtime already saw"
        );

        let mut turn2 = runner::start_agent_with_config(runner::AgentStartConfig {
            cli_resume_id: resume_id2.as_deref(),
            native_acp_full_prompt: Some(&full_prompt_t2),
            acp_session_store: Some(store2.clone()),
            discussion_id: Some("chain-disc"),
            test_acp_transport: Some(transport.clone()),
            ..runner::AgentStartConfig::new(&AgentType::OpenCode, project_path, &prompt2, &tokens)
        })
        .await
        .unwrap();
        while turn2.next_line().await.is_some() {}
        assert!(turn2.child.wait().await.unwrap().success());
        let completion2 = store2
            .completion_checkpoint(
                &AgentType::OpenCode,
                &scope,
                false,
                checkpoint2.as_deref().unwrap(),
                "own-reply-2",
            )
            .unwrap();
        db2.with_conn(move |conn| {
            let mut reply = message("own-reply-2", MessageRole::Agent);
            reply.agent_type = Some(AgentType::OpenCode);
            reply.content = "kt621-own-native-answer".into();
            crate::db::discussions::insert_native_agent_message_with_checkpoint(
                conn,
                "chain-disc",
                &reply,
                true,
                None,
                &AgentType::OpenCode,
                &[],
                false,
                None,
                Some(&completion2),
            )
        })
        .await
        .unwrap();

        assert_eq!(
            fixture.created.load(Ordering::SeqCst),
            1,
            "a resumed turn must not create a stranger session"
        );
        assert_eq!(fixture.resumed.load(Ordering::SeqCst), 1);
        {
            let prompts = fixture.prompts.lock().unwrap();
            assert_eq!(prompts.len(), 2);
            assert!(prompts[1].ends_with(prompt2.as_str()));
            assert!(!prompts[1].contains("content of m1"));
            assert!(!prompts[1].contains("kt621-own-native-answer"));
            assert!(prompts[1].contains("content of peer-before-reply"));
        }

        // --- Reopen once more: the durable cursor after turn 2 is exactly
        // "m2", not the reply that turn 2 itself produced.
        drop(turn2);
        drop(store2);
        assert_eq!(std::sync::Arc::strong_count(&db2), 1);
        drop(db2);
        let db3 = std::sync::Arc::new(crate::db::Database::open_path(&db_path).unwrap());
        let store3 = AcpSessionStore::new(db3.clone(), "chain-disc");
        let loaded = store3
            .load_completed_checkpoint(&AgentType::OpenCode, &scope, false)
            .await
            .unwrap();
        assert_eq!(
            loaded,
            Some(crate::db::acp_runtime_sessions::CompletedCheckpoint {
                conversation_id: "chain-native-session".into(),
                input_message_id: "m2".into(),
                output_message_id: "own-reply-2".into(),
            })
        );
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

#[cfg(test)]
mod connection_fallback_tests {
    use super::effective_connection_id;

    #[test]
    fn dispatch_job_wins_over_the_room_default() {
        // An explicit one-off target must never be replaced by the sticky one.
        let job = "job-connection".to_string();
        let sticky = "room-connection".to_string();
        assert_eq!(
            effective_connection_id(Some(&job), Some(&sticky)),
            Some(&job)
        );
    }

    #[test]
    fn a_reply_without_a_dispatch_job_uses_the_discussion() {
        // Regression: such a reply lost the connection and failed with "the
        // selected external API connection is unavailable", although the
        // discussion recorded it as its durable target (KT-545 DoD #4).
        let sticky = "room-connection".to_string();
        assert_eq!(effective_connection_id(None, Some(&sticky)), Some(&sticky));
    }

    #[test]
    fn none_when_neither_side_carries_one() {
        assert_eq!(effective_connection_id(None, None), None);
    }
}
