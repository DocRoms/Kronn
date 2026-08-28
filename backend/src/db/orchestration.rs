//! 0.11.0 (KT-317) — persistence for the task-orchestration aggregate.
//!
//! Owns `orchestration_runs`, `task_executions`, their event journal and
//! validation runs (migration 127). The state machine is enforced here: every
//! transition is validated against `TaskExecutionStatus::can_transition_to`,
//! written with the sticky CAS in `run_state::claim_status`, and journaled with
//! an attributed actor — all three atomically. KT-318 provisioning lands its
//! DB-side primitives here — the breadcrumb setters, `block_execution`, and the
//! single ATOMIC `commit_provisioning_checkpoint` (brief + dispatch + Working +
//! task InProgress). The Git-interleaving saga that drives them lives in
//! `crate::api::orchestration`; the protected Git merge (KT-320) is still out of
//! scope.

use std::str::FromStr;

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

use crate::db::run_state;
use crate::models::{
    AgentType, BlockedReasonCode, CampaignTaskCandidate, CampaignTaskReason,
    CampaignWorkerSelection, CancellationCleanupPolicy, DiscussionMessage, ExecutionRecoveryAction,
    ExecutionTimeoutKind, IntegrationStrategy, LaunchOutcome, LaunchSingleTaskInput,
    MessageChannel, MessageRole, MessageTarget, MessageTargetKind, ModelTier, OrchestrationActor,
    OrchestrationControlState, OrchestrationResiliencePolicy, OrchestrationRun,
    OrchestrationRunInput, OrchestrationRunKind, OrchestrationRunStatus, PlanningActorKind,
    PlanningPlacement, PlanningTaskStatus, PrincipalAttention, ReviewVerdict, TaskExecution,
    TaskExecutionEvent, TaskExecutionLineage, TaskExecutionRecovery, TaskExecutionStatus,
    TaskExecutionValidationRun, ValidationSpec,
};

const RUN_STATE_TABLE: &str = "task_executions";

type TerminalWorkerContext = (
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
);

// Columns 0..=26 are the KT-317 shape; 27..=29 append attempt_no + the typed worker
// identity (KT-318); 30 appends blocked_reason_code (KT-328); 31 appends the
// exact worker profile (KT-321); 32 appends the principal-authored worker scope;
// 33 appends the launch-time ordered DoD id snapshot; 34 appends the stable
// external API connection identity used by configured HTTP runtimes.
// New columns are appended so no existing index shifts.
const EXEC_COLS: &str = "id, orchestration_run_id, task_id, parent_discussion_id, \
    sub_discussion_id, workspace_id, dispatch_job_id, base_sha, child_branch, \
    worker_agent_type, worker_model, worker_model_tier, status, review_rounds, \
    max_review_rounds, candidate_target_sha, candidate_merge_sha, integrated_sha, \
    backup_ref, blocked_reason, outcome_reason, idempotency_key, created_at, \
    updated_at, finished_at, blocked_from_status, interrupted_from_status, \
    attempt_no, worker_target_kind, worker_cli_session_id, blocked_reason_code, \
    worker_profile_id, worker_scope_json, worker_dod_ids_json, worker_connection_id";

const RUN_COLS: &str = "id, kind, discussion_id, project_id, target_workspace_id, \
    target_branch, max_review_rounds, max_concurrent_executions, token_budget, \
    integration_strategy, validation_json, escalation_notify_url, status, \
    created_at, updated_at, allow_self_review, control_state, control_reason, \
    timeout_secs, max_cli_concurrent_executions, allowed_agents_json, \
    default_worker_json, auto_continue";

fn parse_dt(value: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&value)
        .map(|date| date.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn parse_opt_dt(value: Option<String>) -> Option<DateTime<Utc>> {
    value.map(parse_dt)
}

fn parse_json_column<T: serde::de::DeserializeOwned>(
    idx: usize,
    value: &str,
) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, error.into())
    })
}

/// Run a closure inside a SAVEPOINT so its writes are atomic whether or not the
/// caller is already in a transaction (savepoints nest and also work at the top
/// level). Rolls back on error.
fn in_savepoint<T>(conn: &Connection, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    conn.execute_batch("SAVEPOINT orch_mut")?;
    match f(conn) {
        Ok(value) => {
            conn.execute_batch("RELEASE orch_mut")?;
            Ok(value)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO orch_mut; RELEASE orch_mut");
            Err(e)
        }
    }
}

// ─── Row mappers ─────────────────────────────────────────────────────────────

fn row_to_run(row: &Row) -> rusqlite::Result<OrchestrationRun> {
    let kind: String = row.get(1)?;
    let strategy: String = row.get(9)?;
    let validation_json: String = row.get(10)?;
    let status: String = row.get(12)?;
    let token_budget: Option<i64> = row.get(8)?;
    let control_state: String = row.get(16)?;
    let allowed_agents_json: String = row.get(20)?;
    let default_worker_json: Option<String> = row.get(21)?;
    Ok(OrchestrationRun {
        id: row.get(0)?,
        kind: OrchestrationRunKind::from_str(&kind).unwrap_or_default(),
        discussion_id: row.get(2)?,
        project_id: row.get(3)?,
        target_workspace_id: row.get(4)?,
        target_branch: row.get(5)?,
        max_review_rounds: row.get::<_, i64>(6)? as u32,
        max_concurrent_executions: row.get::<_, i64>(7)? as u32,
        token_budget: token_budget.map(|v| v as u64),
        integration_strategy: IntegrationStrategy::from_str(&strategy).unwrap_or_default(),
        validations: serde_json::from_str(&validation_json).unwrap_or_default(),
        escalation_notify_url: row.get(11)?,
        status: OrchestrationRunStatus::from_str(&status).unwrap_or_default(),
        created_at: parse_dt(row.get(13)?),
        updated_at: parse_dt(row.get(14)?),
        allow_self_review: row.get::<_, i64>(15)? != 0,
        control_state: OrchestrationControlState::from_str(&control_state).unwrap_or_default(),
        control_reason: row.get(17)?,
        timeout_secs: row.get::<_, Option<i64>>(18)?.map(|value| value as u32),
        max_cli_concurrent_executions: row.get::<_, i64>(19)? as u32,
        allowed_agents: parse_json_column(20, &allowed_agents_json)?,
        default_worker: default_worker_json
            .as_deref()
            .map(|json| parse_json_column(21, json))
            .transpose()?,
        auto_continue: row.get::<_, i64>(22)? != 0,
    })
}

/// Parse a nullable resume-checkpoint column strictly. Unlike the primary
/// `status` (which falls back to `Interrupted` so a corrupt row still lands in
/// the reconcile path), a corrupt *checkpoint* has no safe recovery: silently
/// dropping it to `None` — as a bare `.ok()` would — erases the exact resume
/// target the §4bis saga replays against. Surface it as a hard conversion error
/// instead. The migration-127 CHECK makes a bad value unwritable; this is the
/// defense-in-depth guard for a downgraded schema or externally-corrupted DB.
fn parse_checkpoint(
    idx: usize,
    raw: Option<String>,
) -> rusqlite::Result<Option<TaskExecutionStatus>> {
    raw.map(|s| TaskExecutionStatus::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                idx,
                rusqlite::types::Type::Text,
                e.to_string().into(),
            )
        })
}

/// Parse the stored `blocked_reason_code` strictly (like [`parse_target_kind`]): a
/// value outside the enum domain is a corrupt row, surfaced as a hard conversion
/// error rather than silently dropped. The column has no SQL CHECK, so this strict
/// read IS the domain guard (KT-328).
fn parse_blocked_reason_code(
    idx: usize,
    raw: Option<String>,
) -> rusqlite::Result<Option<BlockedReasonCode>> {
    raw.map(|s| BlockedReasonCode::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                idx,
                rusqlite::types::Type::Text,
                e.to_string().into(),
            )
        })
}

/// Map `MessageTargetKind` to its durable DB string (matches the migration-127
/// CHECK domain and the serde snake_case wire form). Kept local to the socle so
/// the shared `discussions` model surface stays untouched by KT-318.
fn target_kind_to_db(kind: Option<MessageTargetKind>) -> Option<&'static str> {
    kind.map(|k| match k {
        MessageTargetKind::DiscussionAgent => "discussion_agent",
        MessageTargetKind::Agent => "agent",
        MessageTargetKind::Cli => "cli",
    })
}

/// Parse the worker identity kind strictly (like [`parse_checkpoint`]): a value
/// outside the domain is a corrupt row, surfaced as a hard conversion error rather
/// than silently coerced to `None` — which would erase the worker's exact identity.
/// The migration-127 CHECK makes a bad value unwritable; this is defense-in-depth.
fn parse_target_kind(
    idx: usize,
    raw: Option<String>,
) -> rusqlite::Result<Option<MessageTargetKind>> {
    match raw.as_deref() {
        None => Ok(None),
        Some("discussion_agent") => Ok(Some(MessageTargetKind::DiscussionAgent)),
        Some("agent") => Ok(Some(MessageTargetKind::Agent)),
        Some("cli") => Ok(Some(MessageTargetKind::Cli)),
        Some(other) => Err(rusqlite::Error::FromSqlConversionFailure(
            idx,
            rusqlite::types::Type::Text,
            format!("unknown worker_target_kind: {other}").into(),
        )),
    }
}

/// Canonical DB string for a worker's provider — the same bare-PascalCase form
/// `discussions` stores `agent` under, so a task worktree's sub-discussion agent
/// and the execution's `worker_agent_type` never diverge. Kept local + exhaustive
/// (adding an `AgentType` fails to compile) so a new provider can't silently map
/// to an empty string.
pub(crate) fn agent_type_to_db(a: &AgentType) -> String {
    match a {
        AgentType::ClaudeCode => "ClaudeCode",
        AgentType::Codex => "Codex",
        AgentType::Vibe => "Vibe",
        AgentType::GeminiCli => "GeminiCli",
        AgentType::Kiro => "Kiro",
        AgentType::CopilotCli => "CopilotCli",
        AgentType::Ollama => "Ollama",
        AgentType::LiteLlm => "LiteLlm",
        AgentType::Nvidia => "Nvidia",
        AgentType::Custom => "Custom",
    }
    .to_string()
}

/// Parse a stored `worker_agent_type` back to the typed identity (defense-in-depth
/// like [`parse_target_kind`]): a value outside the domain is a corrupt row,
/// surfaced as a hard error rather than silently dropping the worker's provider
/// when the saga reconstructs the `MessageTarget` on resume.
pub(crate) fn agent_type_from_db(s: &str) -> Result<AgentType> {
    Ok(match s {
        "ClaudeCode" => AgentType::ClaudeCode,
        "Codex" => AgentType::Codex,
        "Vibe" => AgentType::Vibe,
        "GeminiCli" => AgentType::GeminiCli,
        "Kiro" => AgentType::Kiro,
        "CopilotCli" => AgentType::CopilotCli,
        "Ollama" => AgentType::Ollama,
        "LiteLlm" => AgentType::LiteLlm,
        "Nvidia" => AgentType::Nvidia,
        "Custom" => AgentType::Custom,
        other => bail!("unknown worker_agent_type: {other}"),
    })
}

fn model_tier_to_db(tier: &ModelTier) -> &'static str {
    match tier {
        ModelTier::Default => "default",
        ModelTier::Economy => "economy",
        ModelTier::Reasoning => "reasoning",
    }
}

fn row_to_execution(row: &Row) -> rusqlite::Result<TaskExecution> {
    let status: String = row.get(12)?;
    Ok(TaskExecution {
        id: row.get(0)?,
        orchestration_run_id: row.get(1)?,
        task_id: row.get(2)?,
        parent_discussion_id: row.get(3)?,
        sub_discussion_id: row.get(4)?,
        workspace_id: row.get(5)?,
        dispatch_job_id: row.get(6)?,
        base_sha: row.get(7)?,
        child_branch: row.get(8)?,
        worker_target_kind: parse_target_kind(28, row.get(28)?)?,
        worker_cli_session_id: row.get(29)?,
        worker_connection_id: row.get::<_, Option<String>>(34)?,
        worker_agent_type: row.get(9)?,
        worker_model: row.get(10)?,
        worker_model_tier: row.get(11)?,
        worker_profile_id: row.get::<_, Option<String>>(31)?,
        worker_scope: row
            .get::<_, Option<String>>(32)?
            .map(|value| parse_json_column(32, &value))
            .transpose()?,
        worker_dod_ids: row
            .get::<_, Option<String>>(33)?
            .map(|value| parse_json_column(33, &value))
            .transpose()?,
        attempt_no: row.get::<_, i64>(27)? as u32,
        // A status string outside the enum is a corrupt row, not a runtime
        // condition — fall back to Interrupted so it lands in the reconcile path
        // rather than crashing a list query.
        status: TaskExecutionStatus::from_str(&status).unwrap_or(TaskExecutionStatus::Interrupted),
        review_rounds: row.get::<_, i64>(13)? as u32,
        max_review_rounds: row.get::<_, i64>(14)? as u32,
        candidate_target_sha: row.get(15)?,
        candidate_merge_sha: row.get(16)?,
        integrated_sha: row.get(17)?,
        backup_ref: row.get(18)?,
        blocked_reason: row.get(19)?,
        blocked_reason_code: parse_blocked_reason_code(30, row.get(30)?)?,
        outcome_reason: row.get(20)?,
        idempotency_key: row.get(21)?,
        created_at: parse_dt(row.get(22)?),
        updated_at: parse_dt(row.get(23)?),
        finished_at: parse_opt_dt(row.get(24)?),
        blocked_from_status: parse_checkpoint(25, row.get(25)?)?,
        interrupted_from_status: parse_checkpoint(26, row.get(26)?)?,
    })
}

fn row_to_event(row: &Row) -> rusqlite::Result<TaskExecutionEvent> {
    let from_status: Option<String> = row.get(3)?;
    let to_status: Option<String> = row.get(4)?;
    let actor_kind: String = row.get(5)?;
    let changes: String = row.get(8)?;
    Ok(TaskExecutionEvent {
        id: row.get(0)?,
        task_execution_id: row.get(1)?,
        action: row.get(2)?,
        from_status: from_status.and_then(|s| TaskExecutionStatus::from_str(&s).ok()),
        to_status: to_status.and_then(|s| TaskExecutionStatus::from_str(&s).ok()),
        actor_kind: PlanningActorKind::from_str(&actor_kind).unwrap_or(PlanningActorKind::Backend),
        actor_id: row.get(6)?,
        actor_session_id: row.get(7)?,
        changes: serde_json::from_str(&changes).unwrap_or(serde_json::Value::Null),
        source_message_id: row.get(9)?,
        created_at: parse_dt(row.get(10)?),
    })
}

fn row_to_validation(row: &Row) -> rusqlite::Result<TaskExecutionValidationRun> {
    Ok(TaskExecutionValidationRun {
        id: row.get(0)?,
        task_execution_id: row.get(1)?,
        candidate_merge_sha: row.get(2)?,
        command: row.get(3)?,
        exit_code: row.get::<_, Option<i64>>(4)?.map(|v| v as i32),
        duration_ms: row.get(5)?,
        output: row.get(6)?,
        quick_exec_id: row.get(7)?,
        created_at: parse_dt(row.get(8)?),
    })
}

// ─── OrchestrationRun ────────────────────────────────────────────────────────

/// Record what a reclaim attempt did, durably.
///
/// KT-373 DoD-11. `tracing` says what happened while the process lived; this
/// says it after it dies. A deletion that actually took gigabytes off the disk
/// must remain answerable later — "what removed this, and on whose authority" is
/// not a question logs can answer once they have rotated.
///
/// Reuses `task_execution_events` rather than adding a table: the execution owns
/// the worktree, the actor kinds already include `backend`, and `changes_json`
/// carries the metrics. A refusal is recorded too — knowing that a cleanup was
/// declined, and why, is what tells a full disk from an unattempted one.
pub fn record_artifact_reclaim(
    conn: &Connection,
    canonical_path: &str,
    outcome: Result<(u64, bool), String>,
) -> Result<()> {
    let execution_id: Option<String> = conn
        .query_row(
            "SELECT task_execution_id FROM discussion_workspaces
              WHERE canonical_path = ?1 AND task_execution_id IS NOT NULL
              ORDER BY updated_at DESC
              LIMIT 1",
            params![canonical_path],
            |row| row.get(0),
        )
        .optional()?;
    // No owning execution means no row this event could hang from. The tracing
    // line still carries it; inventing a placeholder execution would be worse
    // than an audit gap we can name.
    let Some(execution_id) = execution_id else {
        return Ok(());
    };

    let (action, changes) = match outcome {
        Ok((bytes, partial)) => (
            "build_artifacts_reclaimed",
            serde_json::json!({
                "target": canonical_path,
                "bytes_reclaimed": bytes,
                // Say so rather than let a floor be read as a measurement.
                "bytes_are_partial": partial,
            }),
        ),
        Err(reason) => (
            "build_artifacts_refused",
            serde_json::json!({ "target": canonical_path, "reason": reason }),
        ),
    };
    record_execution_event(
        conn,
        &execution_id,
        action,
        None,
        None,
        &OrchestrationActor {
            kind: PlanningActorKind::Backend,
            id: Some("disk-maintenance".to_string()),
            session_id: None,
            source_message_id: None,
        },
        changes,
    )
}

/// Decide, from durable state alone, whether a managed worktree's build
/// artefacts may be reclaimed.
///
/// A terminal execution is **necessary and not sufficient**, which is the whole
/// reason this lives here rather than being inferred at the filesystem layer.
/// Four things must hold, and each has cost someone real work when assumed:
///
/// 1. The workspace is Kronn-`managed`. An `external` row is a checkout the user
///    attached; Kronn never owns its artefacts.
/// 2. Its owning execution reached `Done`/`Failed`/`Cancelled`. Review,
///    integration and validation states are deliberately non-terminal, so this
///    single check already excludes a worktree mid-review.
/// 3. No session is still attached to it. On 2026-08-21 a worktree was cleaned
///    because no compiler was visible; it belonged to a live agent between
///    builds. A session row says so where a process scan cannot.
/// 4. No unreleased worker lease holds its canonical path.
///
/// Returns `Unknown` — which refuses — whenever durable state cannot answer,
/// including the case where no workspace row exists for the path at all. A path
/// nothing claims is an inconsistency to report, not a directory to delete.
pub fn worktree_cleanup_liveness(
    conn: &Connection,
    canonical_path: &str,
) -> Result<crate::core::worktree::ExecutionLiveness> {
    use crate::core::worktree::ExecutionLiveness;

    let row: Option<(String, String, Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT ownership, state, task_execution_id, session_pk
               FROM discussion_workspaces
              WHERE canonical_path = ?1
              ORDER BY updated_at DESC
              LIMIT 1",
            params![canonical_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;

    let Some((ownership, state, task_execution_id, session_pk)) = row else {
        return Ok(ExecutionLiveness::Unknown(format!(
            "no workspace row claims {canonical_path}, so nothing durable says it is finished"
        )));
    };

    if ownership != "managed" {
        return Ok(ExecutionLiveness::Active(format!(
            "workspace {canonical_path} is {ownership}, not managed by Kronn"
        )));
    }

    let Some(execution_id) = task_execution_id else {
        return Ok(ExecutionLiveness::Unknown(format!(
            "managed workspace {canonical_path} has no owning task execution"
        )));
    };
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM task_executions WHERE id = ?1",
            params![&execution_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(status) = status else {
        return Ok(ExecutionLiveness::Unknown(format!(
            "execution {execution_id} owning {canonical_path} is missing"
        )));
    };
    let parsed = TaskExecutionStatus::from_str(&status)
        .map_err(|error| anyhow::anyhow!("unreadable execution status {status}: {error}"))?;
    if !parsed.is_terminal() {
        return Ok(ExecutionLiveness::Active(format!(
            "execution {execution_id} is {status}, which is not a finished state"
        )));
    }

    // A finished execution whose workspace is still attached to a live session
    // is exactly the 2026-08-21 shape: the work is over on paper, the agent is
    // still in the directory.
    if state == "attached" {
        if let Some(session_pk) = session_pk {
            let live: Option<String> = conn
                .query_row(
                    "SELECT status FROM discussion_sessions WHERE id = ?1",
                    params![session_pk],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(session_status) = live {
                if session_status == "active" {
                    return Ok(ExecutionLiveness::Active(format!(
                        "session {session_pk} is still attached to {canonical_path}"
                    )));
                }
            }
        }
    }

    let held_lease: Option<String> = conn
        .query_row(
            "SELECT branch FROM discussion_workspace_history_leases
              WHERE canonical_path = ?1 AND released_at IS NULL
              LIMIT 1",
            params![canonical_path],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(branch) = held_lease {
        return Ok(ExecutionLiveness::Active(format!(
            "an unreleased worker lease holds {canonical_path} on branch {branch}"
        )));
    }

    Ok(ExecutionLiveness::Terminal)
}

/// Is this discussion a worker's execution room?
///
/// KT-398 — a worker gets the full native catalogue, planning-management tools
/// included, even though it has exactly one task whose brief is already in its
/// prompt. Browsing the backlog is the principal's job. Answered here rather
/// than in the tool layer because only the durable lineage knows, and the
/// catalogue is built synchronously.
pub fn discussion_is_execution_room(conn: &Connection, discussion_id: &str) -> Result<bool> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM task_executions WHERE sub_discussion_id = ?1 LIMIT 1",
            params![discussion_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Resolve the durable execution that owns a worker room, including any
/// principal-authored mechanical scope consumed by the HTTP runner.
pub fn get_execution_for_sub_discussion(
    conn: &Connection,
    discussion_id: &str,
) -> Result<Option<TaskExecution>> {
    let execution = conn
        .query_row(
            &format!(
                "SELECT {EXEC_COLS} FROM task_executions WHERE sub_discussion_id = ?1 LIMIT 1"
            ),
            params![discussion_id],
            row_to_execution,
        )
        .optional()
        .map_err(anyhow::Error::from)?;
    validate_loaded_execution(conn, execution)
}

/// Resolve the active execution that durably owns a native worker's worktree.
/// This is the authority boundary for `git_commit`: a directory name that merely
/// looks like `.kronn/worktrees/...` proves nothing.
pub fn managed_working_execution_for_workspace(
    conn: &Connection,
    discussion_id: &str,
    canonical_path: &str,
) -> Result<Option<TaskExecution>> {
    let execution_id: Option<String> = conn
        .query_row(
            "SELECT te.id
               FROM task_executions te
               JOIN discussion_workspaces dw
                 ON dw.id = te.workspace_id
                AND dw.task_execution_id = te.id
              WHERE te.sub_discussion_id = ?1
                AND dw.disc_id = ?1
                AND dw.canonical_path = ?2
                AND dw.ownership = 'managed'
                AND dw.state = 'attached'
                AND te.status = 'Working'
              ORDER BY te.updated_at DESC
              LIMIT 1",
            params![discussion_id, canonical_path],
            |row| row.get(0),
        )
        .optional()?;
    match execution_id {
        Some(execution_id) => get_task_execution(conn, &execution_id),
        None => Ok(None),
    }
}

pub fn create_orchestration_run(
    conn: &Connection,
    input: &OrchestrationRunInput,
) -> Result<OrchestrationRun> {
    if input.max_concurrent_executions == 0 {
        bail!("max_concurrent_executions must be at least 1");
    }
    if input.max_cli_concurrent_executions == 0 {
        bail!("max_cli_concurrent_executions must be at least 1");
    }
    if input.max_review_rounds == 0 {
        bail!("max_review_rounds must be at least 1");
    }
    if input.token_budget == Some(0) {
        bail!("token_budget must be positive when set");
    }
    if input
        .token_budget
        .is_some_and(|budget| budget > i64::MAX as u64)
    {
        bail!("token_budget exceeds the durable SQLite range");
    }
    if input.timeout_secs == Some(0) {
        bail!("timeout_secs must be positive when set");
    }
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let validation_json = serde_json::to_string(&input.validations)?;
    let allowed_agents_json = serde_json::to_string(&input.allowed_agents)?;
    let default_worker_json = input
        .default_worker
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    conn.execute(
        &format!(
            "INSERT INTO orchestration_runs ({RUN_COLS}) \
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
                    ?17, NULL, ?18, ?19, ?20, ?21, ?22)"
        ),
        params![
            id,
            input.kind.as_str(),
            input.discussion_id,
            input.project_id,
            input.target_workspace_id,
            input.target_branch,
            input.max_review_rounds as i64,
            input.max_concurrent_executions as i64,
            input.token_budget.map(|v| v as i64),
            input.integration_strategy.as_str(),
            validation_json,
            input.escalation_notify_url,
            OrchestrationRunStatus::Active.as_str(),
            now,
            now,
            // DoD-7: no self-review by default; a launcher opt-in (KT-321) is the only
            // path that sets this true.
            0_i64,
            OrchestrationControlState::Running.as_str(),
            input.timeout_secs.map(i64::from),
            input.max_cli_concurrent_executions as i64,
            allowed_agents_json,
            default_worker_json,
            i64::from(input.auto_continue),
        ],
    )?;
    get_orchestration_run(conn, &id)?
        .ok_or_else(|| anyhow::anyhow!("orchestration_run vanished right after insert"))
}

pub fn get_orchestration_run(conn: &Connection, id: &str) -> Result<Option<OrchestrationRun>> {
    conn.query_row(
        &format!("SELECT {RUN_COLS} FROM orchestration_runs WHERE id = ?1"),
        params![id],
        row_to_run,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_active_campaign_for_discussion(
    conn: &Connection,
    discussion_id: &str,
) -> Result<Option<OrchestrationRun>> {
    conn.query_row(
        &format!(
            "SELECT {RUN_COLS} FROM orchestration_runs \
             WHERE discussion_id = ?1 AND kind = 'campaign' \
               AND control_state NOT IN ('completed', 'cancelled', 'failed') \
             ORDER BY created_at DESC LIMIT 1"
        ),
        params![discussion_id],
        row_to_run,
    )
    .optional()
    .map_err(Into::into)
}

fn record_run_event(
    conn: &Connection,
    run_id: &str,
    action: &str,
    from: Option<OrchestrationControlState>,
    to: Option<OrchestrationControlState>,
    actor: &OrchestrationActor,
    changes: serde_json::Value,
) -> Result<()> {
    conn.execute(
        "INSERT INTO orchestration_run_events \
         (id, orchestration_run_id, action, from_state, to_state, actor_kind, actor_id, \
          changes_json, source_message_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            Uuid::new_v4().to_string(),
            run_id,
            action,
            from.map(OrchestrationControlState::as_str),
            to.map(OrchestrationControlState::as_str),
            actor.kind.as_str(),
            actor.id,
            serde_json::to_string(&changes)?,
            actor.source_message_id,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Persist an operator/principal campaign hold or terminal outcome. Terminal
/// control states are sticky; resume is accepted only from a non-terminal hold.
pub fn set_orchestration_control_state(
    conn: &Connection,
    run_id: &str,
    to: OrchestrationControlState,
    reason: Option<&str>,
    actor: &OrchestrationActor,
) -> Result<OrchestrationRun> {
    in_savepoint(conn, |conn| {
        let run = get_orchestration_run(conn, run_id)?
            .ok_or_else(|| anyhow::anyhow!("orchestration run not found"))?;
        if run.control_state.is_terminal() && run.control_state != to {
            bail!(
                "terminal orchestration control state {} is sticky",
                run.control_state.as_str()
            );
        }
        if to == OrchestrationControlState::Running {
            let escalated: i64 = conn.query_row(
                "SELECT COUNT(*) FROM task_executions \
                 WHERE orchestration_run_id = ?1 AND status = 'Escalated'",
                [run_id],
                |row| row.get(0),
            )?;
            if escalated > 0 {
                bail!(
                    "cannot resume while {escalated} execution(s) still require a human decision"
                );
            }
        }
        if to == OrchestrationControlState::Completed {
            let active: i64 = conn.query_row(
                "SELECT COUNT(*) FROM task_executions \
                 WHERE orchestration_run_id = ?1 \
                   AND status NOT IN ('Done', 'Failed', 'Cancelled')",
                [run_id],
                |row| row.get(0),
            )?;
            if active > 0 {
                bail!("cannot complete while {active} execution(s) are active");
            }
            let plan = crate::db::planning::get_discussion_plan(conn, &run.discussion_id)?;
            let unfinished = plan
                .active
                .iter()
                .filter(|relation| {
                    !matches!(
                        relation.task.status,
                        PlanningTaskStatus::Done | PlanningTaskStatus::Archived
                    )
                })
                .count();
            if unfinished > 0 {
                bail!(
                    "cannot complete while {unfinished} active plan task(s) are unfinished; cancel the campaign instead"
                );
            }
        }
        if run.control_state != to {
            let coarse = match to {
                OrchestrationControlState::Completed => OrchestrationRunStatus::Completed,
                OrchestrationControlState::Cancelled => OrchestrationRunStatus::Cancelled,
                OrchestrationControlState::Failed => OrchestrationRunStatus::Failed,
                _ => OrchestrationRunStatus::Active,
            };
            conn.execute(
                "UPDATE orchestration_runs \
                 SET control_state = ?2, control_reason = ?3, status = ?4, updated_at = ?5 \
                 WHERE id = ?1",
                params![
                    run_id,
                    to.as_str(),
                    reason,
                    coarse.as_str(),
                    Utc::now().to_rfc3339()
                ],
            )?;
            record_run_event(
                conn,
                run_id,
                "control_state_changed",
                Some(run.control_state),
                Some(to),
                actor,
                serde_json::json!({ "reason": reason }),
            )?;
        }
        get_orchestration_run(conn, run_id)?
            .ok_or_else(|| anyhow::anyhow!("orchestration run vanished after control update"))
    })
}

/// Resolve an explicit worker override, the campaign default, or finally the
/// principal discussion's configured identity. The returned explanation is
/// persisted/surfaced by launch callers so "auto" is never a black box.
pub fn resolve_campaign_worker(
    conn: &Connection,
    run: &OrchestrationRun,
    worker_override: Option<&CampaignWorkerSelection>,
) -> Result<(CampaignWorkerSelection, String)> {
    let (selection, explanation) = if let Some(worker) = worker_override {
        (worker.clone(), "explicit launch override".to_string())
    } else if let Some(worker) = run.default_worker.as_ref() {
        (worker.clone(), "campaign default worker".to_string())
    } else {
        let discussion = crate::db::discussions::get_discussion(conn, &run.discussion_id)?
            .ok_or_else(|| anyhow::anyhow!("principal discussion vanished"))?;
        let target = if crate::agents::runner::is_http_chat_agent(&discussion.agent) {
            MessageTarget::discussion_agent(discussion.agent.clone())
        } else {
            MessageTarget::agent(discussion.agent.clone())
        };
        (
            CampaignWorkerSelection {
                target: target.with_tier(discussion.tier),
                model: discussion.model,
                profile_id: discussion.profile_ids.first().cloned(),
            },
            "automatic fallback to the principal discussion identity".to_string(),
        )
    };
    if !run.allowed_agents.is_empty()
        && !run
            .allowed_agents
            .iter()
            .any(|agent| agent == &selection.target.agent_type)
    {
        bail!("selected agent is not allowed by the campaign policy");
    }
    match selection.target.kind {
        MessageTargetKind::Cli => {
            let session_id = selection
                .target
                .cli_session_id
                .ok_or_else(|| anyhow::anyhow!("CLI worker selection has no exact session id"))?;
            let expected_agent = agent_type_to_db(&selection.target.agent_type);
            let available: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM discussion_sessions \
                 WHERE id = ?1 AND disc_id = ?2 AND agent_type = ?3 \
                   AND status <> 'left')",
                params![session_id, run.discussion_id, expected_agent],
                |row| row.get(0),
            )?;
            if !available {
                bail!("selected CLI session is not available in the principal discussion");
            }
        }
        MessageTargetKind::DiscussionAgent | MessageTargetKind::Agent
            if selection.target.cli_session_id.is_some() =>
        {
            bail!("native worker selection must not carry a CLI session id");
        }
        MessageTargetKind::DiscussionAgent | MessageTargetKind::Agent => {}
    }
    ensure_task_worker_transport_compatible(&selection.target)?;
    Ok((selection, explanation))
}

/// Fail closed before an orchestration worker is persisted or dispatched when
/// its typed identity cannot use the delivery transport associated with its
/// provider. `discussion_agent` is the server-side HTTP/native room path;
/// `agent` is a punctual host CLI process; `cli` is an already joined exact
/// session. Accepting a cross-family pair only defers the failure until the
/// delivery bridge preflight, where watchdog retries cannot repair it.
pub fn ensure_task_worker_transport_compatible(target: &MessageTarget) -> Result<()> {
    let is_http = crate::agents::runner::is_http_chat_agent(&target.agent_type);
    match target.kind {
        MessageTargetKind::DiscussionAgent if !is_http => bail!(
            "incompatible worker transport: host CLI providers must use kind=agent; \
             kind=discussion_agent is reserved for Ollama, LiteLLM and NVIDIA"
        ),
        MessageTargetKind::Agent if is_http => bail!(
            "incompatible worker transport: Ollama, LiteLLM and NVIDIA must use \
             kind=discussion_agent; kind=agent is reserved for host CLI processes"
        ),
        MessageTargetKind::Cli if is_http => {
            bail!("incompatible worker transport: HTTP providers cannot be joined CLI workers")
        }
        MessageTargetKind::Cli if target.cli_session_id.is_none() => {
            bail!("CLI worker selection has no exact session id")
        }
        MessageTargetKind::DiscussionAgent | MessageTargetKind::Agent
            if target.cli_session_id.is_some() =>
        {
            bail!("native worker selection must not carry a CLI session id")
        }
        _ => Ok(()),
    }
}

fn active_execution_counts(conn: &Connection, run: &OrchestrationRun) -> Result<(u32, u32)> {
    let active: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_executions \
         WHERE orchestration_run_id = ?1 AND status NOT IN ('Done', 'Failed', 'Cancelled')",
        [run.id.as_str()],
        |row| row.get(0),
    )?;
    // Deliberately scoped to the principal room, not just this run: two campaigns
    // cannot each believe they own the same CLI concurrency allowance.
    let cli: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_executions \
         WHERE parent_discussion_id = ?1 AND worker_target_kind = 'cli' \
           AND status NOT IN ('Done', 'Failed', 'Cancelled')",
        [run.discussion_id.as_str()],
        |row| row.get(0),
    )?;
    Ok((active as u32, cli as u32))
}

fn run_token_usage(conn: &Connection, run_id: &str) -> Result<u64> {
    let used: i64 = conn.query_row(
        "SELECT COALESCE(SUM(m.tokens_used), 0) \
         FROM task_executions e \
         JOIN messages m ON m.discussion_id = e.sub_discussion_id \
         WHERE e.orchestration_run_id = ?1",
        [run_id],
        |row| row.get(0),
    )?;
    Ok(used.max(0) as u64)
}

fn candidate_reason(code: &str, detail: impl Into<String>) -> CampaignTaskReason {
    CampaignTaskReason {
        code: code.to_string(),
        detail: detail.into(),
    }
}

/// Ordered, explained selection projection for the principal. Every linked plan
/// entry is returned; `launchable` is true only for the first tasks that fit the
/// remaining campaign slots after every durable policy/scope guard is applied.
pub fn campaign_task_candidates(
    conn: &Connection,
    run_id: &str,
    worker_override: Option<&CampaignWorkerSelection>,
) -> Result<Vec<CampaignTaskCandidate>> {
    let run = get_orchestration_run(conn, run_id)?
        .ok_or_else(|| anyhow::anyhow!("orchestration run not found"))?;
    let plan = crate::db::planning::get_discussion_plan(conn, &run.discussion_id)?;
    let worker = resolve_campaign_worker(conn, &run, worker_override);
    let (active_count, cli_count) = active_execution_counts(conn, &run)?;
    let remaining_slots = run.max_concurrent_executions.saturating_sub(active_count) as usize;
    let used_tokens = run_token_usage(conn, run_id)?;
    let token_exhausted = run.token_budget.is_some_and(|budget| used_tokens >= budget);
    let escalated: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_executions \
         WHERE orchestration_run_id = ?1 AND status = 'Escalated'",
        [run_id],
        |row| row.get(0),
    )?;

    let mut candidates = Vec::with_capacity(plan.active.len() + plan.later.len());
    for relation in plan.active.iter().chain(plan.later.iter()) {
        let mut reasons = Vec::new();
        if run.kind != OrchestrationRunKind::Campaign {
            reasons.push(candidate_reason("not_campaign", "run is not a campaign"));
        }
        if run.control_state != OrchestrationControlState::Running {
            reasons.push(candidate_reason(
                "campaign_not_running",
                format!("campaign is {}", run.control_state.as_str()),
            ));
        }
        if escalated > 0 {
            reasons.push(candidate_reason(
                "human_decision_required",
                "an escalated execution requires a human decision",
            ));
        }
        if relation.placement != PlanningPlacement::Active {
            reasons.push(candidate_reason("later", "task is in the Later section"));
        }
        if relation.task.status != PlanningTaskStatus::Todo {
            reasons.push(candidate_reason(
                "status",
                format!("task status is {:?}, not Todo", relation.task.status),
            ));
        }
        if !relation.active_blockers.is_empty() {
            reasons.push(candidate_reason(
                "active_blockers",
                format!("{} active blocker(s)", relation.active_blockers.len()),
            ));
        }
        if let Some(project_id) = run.project_id.as_deref() {
            if relation.task.project_ids.as_slice() != [project_id] {
                reasons.push(candidate_reason(
                    "out_of_scope",
                    "task does not belong exclusively to the campaign project",
                ));
            }
        } else if relation.task.project_ids.len() != 1 {
            reasons.push(candidate_reason(
                "project_ambiguous",
                "task must belong to exactly one project",
            ));
        }
        if let Some(task) = crate::db::planning::get_task(conn, &relation.task.id)? {
            if task.definition_of_done.is_empty() {
                reasons.push(candidate_reason(
                    "missing_definition_of_done",
                    "task has no Definition of Done",
                ));
            }
        }
        if get_active_execution_for_task(conn, &relation.task.id)?.is_some() {
            reasons.push(candidate_reason(
                "already_running",
                "task already has an active execution",
            ));
        }
        if remaining_slots == 0 {
            reasons.push(candidate_reason(
                "concurrency_limit",
                format!(
                    "campaign limit {} is reached",
                    run.max_concurrent_executions
                ),
            ));
        }
        if token_exhausted {
            reasons.push(candidate_reason(
                "token_budget",
                "campaign token budget is exhausted",
            ));
        }
        match &worker {
            Err(error) => reasons.push(candidate_reason("worker_policy", error.to_string())),
            Ok((selection, _))
                if selection.target.kind == MessageTargetKind::Cli
                    && cli_count >= run.max_cli_concurrent_executions =>
            {
                reasons.push(candidate_reason(
                    "cli_concurrency_limit",
                    format!(
                        "principal room CLI limit {} is reached",
                        run.max_cli_concurrent_executions
                    ),
                ));
            }
            Ok(_) => {}
        }
        candidates.push(CampaignTaskCandidate {
            task: relation.task.clone(),
            plan_position: relation.position.max(0) as u32,
            launchable: reasons.is_empty(),
            reasons,
        });
    }

    // Preserve plan order and expose only the tasks that fit the remaining
    // capacity. Later otherwise-valid tasks are explained, never silently skipped.
    let mut admitted = 0usize;
    for candidate in &mut candidates {
        if candidate.launchable {
            if admitted < remaining_slots {
                admitted += 1;
            } else {
                candidate.launchable = false;
                candidate.reasons.push(candidate_reason(
                    "plan_order",
                    "an earlier ready task owns the remaining campaign slot",
                ));
            }
        }
    }
    Ok(candidates)
}

pub fn principal_attention(conn: &Connection, run_id: &str) -> Result<PrincipalAttention> {
    let run = get_orchestration_run(conn, run_id)?
        .ok_or_else(|| anyhow::anyhow!("orchestration run not found"))?;
    let (active_executions, cli_executions) = active_execution_counts(conn, &run)?;
    let awaiting_review: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_executions WHERE orchestration_run_id = ?1 \
         AND status = 'AwaitingReview'",
        [run_id],
        |row| row.get(0),
    )?;
    let awaiting_human: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_executions WHERE orchestration_run_id = ?1 \
         AND (status = 'Escalated' OR (status = 'Blocked' AND \
              COALESCE(blocked_reason_code, '') <> 'awaiting_worker_acceptance'))",
        [run_id],
        |row| row.get(0),
    )?;
    let ready_tasks = campaign_task_candidates(conn, run_id, None)?
        .into_iter()
        .filter(|candidate| candidate.launchable)
        .count() as u32;
    let mut actions = Vec::new();
    if awaiting_review > 0 {
        actions.push(format!("review {awaiting_review} delivered execution(s)"));
    }
    if awaiting_human > 0 || run.control_state == OrchestrationControlState::AwaitingHuman {
        actions.push("human decision required before continuing".to_string());
    } else if ready_tasks > 0 {
        actions.push(format!("launch one of {ready_tasks} ready task(s)"));
    } else if active_executions > 0 {
        actions.push("wait for active child executions".to_string());
    } else {
        actions.push("complete the campaign or resolve the reported plan blockers".to_string());
    }
    Ok(PrincipalAttention {
        active_executions,
        cli_executions,
        awaiting_review: awaiting_review as u32,
        awaiting_human: awaiting_human as u32,
        ready_tasks,
        actions,
    })
}

// ─── TaskExecution ───────────────────────────────────────────────────────────

pub fn get_task_execution(conn: &Connection, id: &str) -> Result<Option<TaskExecution>> {
    let execution = conn
        .query_row(
            &format!("SELECT {EXEC_COLS} FROM task_executions WHERE id = ?1"),
            params![id],
            row_to_execution,
        )
        .optional()
        .map_err(anyhow::Error::from)?;
    validate_loaded_execution(conn, execution)
}

fn validate_loaded_execution(
    conn: &Connection,
    execution: Option<TaskExecution>,
) -> Result<Option<TaskExecution>> {
    if let Some(execution) = execution.as_ref() {
        validate_worker_connection(conn, execution)?;
    }
    Ok(execution)
}

fn validate_worker_connection(conn: &Connection, execution: &TaskExecution) -> Result<()> {
    let persisted_agent = execution
        .worker_agent_type
        .as_deref()
        .map(agent_type_from_db)
        .transpose()?;
    let requires_connection = execution.worker_target_kind.is_some()
        && matches!(
            persisted_agent,
            Some(AgentType::LiteLlm | AgentType::Nvidia | AgentType::Custom)
        );
    let Some(raw_connection_id) = execution.worker_connection_id.as_deref() else {
        if requires_connection {
            bail!("external worker target has no connection identifier");
        }
        return Ok(());
    };
    let connection_id = raw_connection_id.trim();
    if connection_id.is_empty() {
        bail!("worker connection identifier is empty");
    }
    let preset: Option<String> = conn
        .query_row(
            "SELECT origin_preset FROM external_api_connections WHERE id = ?1",
            [connection_id],
            |row| row.get(0),
        )
        .optional()?;
    let expected = match preset.as_deref() {
        Some("litellm") => AgentType::LiteLlm,
        Some("nvidia") => AgentType::Nvidia,
        Some("other") => AgentType::Custom,
        Some(other) => bail!("unknown external connection preset: {other}"),
        None => bail!("unknown worker connection identifier: {connection_id}"),
    };
    let persisted = persisted_agent
        .ok_or_else(|| anyhow::anyhow!("execution connection has no worker_agent_type"))?;
    if persisted != expected {
        bail!("worker connection does not match persisted provider");
    }
    Ok(())
}

/// The single active (non-terminal) execution for a task, if any. The partial
/// unique index guarantees there is at most one.
pub fn get_active_execution_for_task(
    conn: &Connection,
    task_id: &str,
) -> Result<Option<TaskExecution>> {
    let execution = conn
        .query_row(
            &format!(
                "SELECT {EXEC_COLS} FROM task_executions \
             WHERE task_id = ?1 AND status NOT IN ('Done', 'Failed', 'Cancelled') LIMIT 1"
            ),
            params![task_id],
            row_to_execution,
        )
        .optional()
        .map_err(anyhow::Error::from)?;
    validate_loaded_execution(conn, execution)
}

/// Latest execution for a task, including terminal history. Used by reconnecting
/// agents that retained the Planning reference but lost a transient chat cursor.
pub fn get_latest_execution_for_task(
    conn: &Connection,
    task_id: &str,
) -> Result<Option<TaskExecution>> {
    let execution = conn
        .query_row(
            &format!(
                "SELECT {EXEC_COLS} FROM task_executions \
             WHERE task_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1"
            ),
            params![task_id],
            row_to_execution,
        )
        .optional()
        .map_err(anyhow::Error::from)?;
    validate_loaded_execution(conn, execution)
}

/// Launch a single task: create the implicit `single_task` OrchestrationRun and
/// its `Pending` TaskExecution atomically (ADR §1). Idempotent — a retry with
/// the same `idempotency_key` returns the existing execution (and its run)
/// without creating a duplicate.
pub fn launch_single_task(
    conn: &Connection,
    input: &LaunchSingleTaskInput,
    actor: &OrchestrationActor,
) -> Result<LaunchOutcome> {
    if let Some(scope) = input.worker_scope.as_ref() {
        scope.validate().map_err(anyhow::Error::msg)?;
    }
    in_savepoint(conn, |conn| {
        // Idempotent replay: same task + key returns the existing execution.
        if let Some(key) = input.idempotency_key.as_deref() {
            if let Some(existing) = conn
                .query_row(
                    &format!(
                        "SELECT {EXEC_COLS} FROM task_executions \
                         WHERE task_id = ?1 AND idempotency_key = ?2"
                    ),
                    params![input.task_id, key],
                    row_to_execution,
                )
                .optional()?
            {
                let run = get_orchestration_run(conn, &existing.orchestration_run_id)?
                    .ok_or_else(|| anyhow::anyhow!("run missing for existing execution"))?;
                return Ok(LaunchOutcome {
                    run,
                    execution: existing,
                    deduplicated: true,
                });
            }
        }

        let mut run_input = OrchestrationRunInput::single_task(input.parent_discussion_id.clone());
        run_input.project_id = input.project_id.clone();
        run_input.target_workspace_id = input.target_workspace_id.clone();
        run_input.target_branch = input.target_branch.clone();
        run_input.max_review_rounds = input.max_review_rounds;
        run_input.validations = input.validations.clone();
        let run = create_orchestration_run(conn, &run_input)?;

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            &format!(
                // Trailing checkpoints NULL, NULL = blocked_from_status,
                // interrupted_from_status (a fresh Pending holds no resume target).
                // Then attempt_no = 0 (the initial attempt), the typed worker identity
                // (?14 kind, ?15 exact CLI session) — NULL unless the launch already
                // carries a chosen worker — blocked_reason_code = NULL (a fresh
                // launch is never Blocked), then ?16 is its exact profile.
                "INSERT INTO task_executions ({EXEC_COLS}) VALUES \
                 (?1, ?2, ?3, ?4, NULL, NULL, NULL, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, \
                  NULL, NULL, NULL, NULL, NULL, NULL, ?12, ?13, ?13, NULL, NULL, NULL, \
                  0, ?14, ?15, NULL, ?16, ?17, ?18, ?19)"
            ),
            params![
                id,
                run.id,
                input.task_id,
                input.parent_discussion_id,
                input.base_sha,
                input.child_branch,
                input.worker_agent_type,
                input.worker_model,
                input.worker_model_tier,
                TaskExecutionStatus::Pending.as_str(),
                input.max_review_rounds as i64,
                input.idempotency_key,
                now,
                target_kind_to_db(input.worker_target_kind),
                input.worker_cli_session_id,
                input.worker_profile_id,
                input
                    .worker_scope
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                input
                    .worker_dod_ids
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                input.worker_connection_id,
            ],
        )?;

        record_execution_event(
            conn,
            &id,
            "created",
            None,
            Some(TaskExecutionStatus::Pending),
            actor,
            serde_json::json!({ "task_id": input.task_id, "kind": "single_task" }),
        )?;

        let execution = get_task_execution(conn, &id)?
            .ok_or_else(|| anyhow::anyhow!("task_execution vanished right after insert"))?;
        Ok(LaunchOutcome {
            run,
            execution,
            deduplicated: false,
        })
    })
}

/// Atomically add one ordered, policy-compliant task to an existing campaign.
/// The candidate projection is recomputed inside the same savepoint as the
/// insert, so two concurrent principals cannot both consume the final slot.
pub fn launch_task_in_run(
    conn: &Connection,
    run_id: &str,
    input: &LaunchSingleTaskInput,
    selection: &CampaignWorkerSelection,
    actor: &OrchestrationActor,
) -> Result<LaunchOutcome> {
    if let Some(scope) = input.worker_scope.as_ref() {
        scope.validate().map_err(anyhow::Error::msg)?;
    }
    in_savepoint(conn, |conn| {
        if let Some(key) = input.idempotency_key.as_deref() {
            if let Some(existing) = conn
                .query_row(
                    &format!(
                        "SELECT {EXEC_COLS} FROM task_executions \
                         WHERE task_id = ?1 AND idempotency_key = ?2"
                    ),
                    params![input.task_id, key],
                    row_to_execution,
                )
                .optional()?
            {
                let run = get_orchestration_run(conn, &existing.orchestration_run_id)?
                    .ok_or_else(|| anyhow::anyhow!("run missing for existing execution"))?;
                if run.id != run_id {
                    bail!("idempotency key belongs to another orchestration run");
                }
                return Ok(LaunchOutcome {
                    run,
                    execution: existing,
                    deduplicated: true,
                });
            }
        }

        let run = get_orchestration_run(conn, run_id)?
            .ok_or_else(|| anyhow::anyhow!("orchestration run not found"))?;
        if run.kind != OrchestrationRunKind::Campaign {
            bail!("run is not a campaign");
        }
        if run.control_state != OrchestrationControlState::Running {
            bail!("campaign is {}, not running", run.control_state.as_str());
        }
        if input.parent_discussion_id != run.discussion_id {
            bail!("launch parent discussion is outside the campaign scope");
        }
        let candidates = campaign_task_candidates(conn, run_id, Some(selection))?;
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.task.id == input.task_id)
            .ok_or_else(|| anyhow::anyhow!("task is not linked to the campaign discussion"))?;
        if !candidate.launchable {
            let details = candidate
                .reasons
                .iter()
                .map(|reason| format!("{}: {}", reason.code, reason.detail))
                .collect::<Vec<_>>()
                .join("; ");
            bail!("task is not campaign-launchable: {details}");
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            &format!(
                "INSERT INTO task_executions ({EXEC_COLS}) VALUES \
                 (?1, ?2, ?3, ?4, NULL, NULL, NULL, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, \
                  NULL, NULL, NULL, NULL, NULL, NULL, ?12, ?13, ?13, NULL, NULL, NULL, \
                  0, ?14, ?15, NULL, ?16, ?17, ?18, ?19)"
            ),
            params![
                id,
                run.id,
                input.task_id,
                input.parent_discussion_id,
                input.base_sha,
                input.child_branch,
                input.worker_agent_type,
                input.worker_model,
                input.worker_model_tier,
                TaskExecutionStatus::Pending.as_str(),
                input.max_review_rounds as i64,
                input.idempotency_key,
                now,
                target_kind_to_db(input.worker_target_kind),
                input.worker_cli_session_id,
                input.worker_profile_id,
                input
                    .worker_scope
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                input
                    .worker_dod_ids
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                input.worker_connection_id,
            ],
        )?;
        record_execution_event(
            conn,
            &id,
            "created",
            None,
            Some(TaskExecutionStatus::Pending),
            actor,
            serde_json::json!({
                "task_id": input.task_id,
                "kind": "campaign",
                "worker_model": input.worker_model,
                "worker_profile_id": input.worker_profile_id,
            }),
        )?;
        let execution = get_task_execution(conn, &id)?
            .ok_or_else(|| anyhow::anyhow!("campaign execution vanished after insert"))?;
        Ok(LaunchOutcome {
            run,
            execution,
            deduplicated: false,
        })
    })
}

// ─── Provisioning (KT-318) ─────────────────────────────────────────────────
// DB-side primitives for the launch saga in `crate::api::orchestration`. Each
// setter is called by exactly one saga step, after its external effect lands, so a
// crash resumes from the execution row's own columns (never a title/breadcrumb
// search — the reviewer's resume-keyed-by-execution guard).

/// Link the freshly created sub-discussion to its execution.
pub fn set_execution_sub_discussion(
    conn: &Connection,
    exec_id: &str,
    sub_discussion_id: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE task_executions SET sub_discussion_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![exec_id, sub_discussion_id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Link the managed workspace and its exact Git coordinates to the execution
/// once the worktree HEAD is verified. KT-320 cleanup must never rediscover
/// ownership from a path alone; these are the durable expected branch/base.
pub fn set_execution_workspace(
    conn: &Connection,
    exec_id: &str,
    workspace_id: &str,
    base_sha: &str,
    child_branch: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE task_executions
            SET workspace_id = ?2, base_sha = ?3, child_branch = ?4, updated_at = ?5
          WHERE id = ?1",
        params![
            exec_id,
            workspace_id,
            base_sha,
            child_branch,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

pub fn attach_execution_dispatch(conn: &Connection, exec_id: &str, job_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_executions SET dispatch_job_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![exec_id, job_id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

pub fn get_execution_for_dispatch(
    conn: &Connection,
    dispatch_job_id: &str,
) -> Result<Option<TaskExecution>> {
    let execution_id: Option<String> = conn
        .query_row(
            "SELECT id FROM task_executions WHERE dispatch_job_id = ?1 LIMIT 1",
            [dispatch_job_id],
            |row| row.get(0),
        )
        .optional()?;
    execution_id
        .as_deref()
        .map(|id| get_task_execution(conn, id))
        .transpose()
        .map(Option::flatten)
}

/// Persist one bounded, payload-free HTTP provider trace for a native worker
/// dispatch. Re-finalizing the same dispatch replaces its trace; a later
/// rework has a different dispatch id and therefore remains a separate journal
/// event on the same execution.
pub fn record_http_turn_telemetry_for_dispatch(
    conn: &Connection,
    dispatch_job_id: &str,
    turns: &[crate::models::TaskExecutionHttpTurnUsage],
) -> Result<Option<String>> {
    if turns.is_empty() {
        return Ok(None);
    }
    in_savepoint(conn, |conn| {
        let Some(execution) = get_execution_for_dispatch(conn, dispatch_job_id)? else {
            return Ok(None);
        };
        conn.execute(
            "DELETE FROM task_execution_events
              WHERE task_execution_id = ?1
                AND action = 'http_turn_telemetry'
                AND actor_session_id = ?2",
            params![execution.id, dispatch_job_id],
        )?;
        record_execution_event(
            conn,
            &execution.id,
            "http_turn_telemetry",
            Some(execution.status),
            Some(execution.status),
            &OrchestrationActor {
                kind: PlanningActorKind::Backend,
                id: Some("http-agent-runner".to_string()),
                session_id: Some(dispatch_job_id.to_string()),
                source_message_id: None,
            },
            serde_json::json!({"version": 1, "turns": turns}),
        )?;
        Ok(Some(execution.id))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeliveredDispatchInterruption {
    pub execution_id: String,
    pub parent_discussion_id: String,
    pub task_id: String,
}

/// A native worker process may finish or fail without calling `task_exec_deliver`
/// (for example after forced synthesis, a self-reported blocker, or a provider
/// crash). Its dispatch is terminal at that point, so leaving the linked
/// business execution in `Working` creates a zombie with no process capable of
/// delivering it. Atomically checkpoint that case as `Interrupted` and park it
/// for the principal. A raced delivery has already moved to `AwaitingReview`
/// and is deliberately left untouched.
pub fn interrupt_undelivered_execution_for_dispatch(
    conn: &Connection,
    dispatch_job_id: &str,
    reason: &str,
    actor: &OrchestrationActor,
) -> Result<Option<UndeliveredDispatchInterruption>> {
    in_savepoint(conn, |conn| {
        let Some(execution) = get_execution_for_dispatch(conn, dispatch_job_id)? else {
            return Ok(None);
        };
        if execution.status != TaskExecutionStatus::Working {
            return Ok(None);
        }
        if !transition_execution(
            conn,
            &execution.id,
            TaskExecutionStatus::Interrupted,
            actor,
            serde_json::json!({
                "reason": reason,
                "dispatch_job_id": dispatch_job_id,
            }),
        )? {
            return Ok(None);
        }
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO task_execution_recovery (
                 task_execution_id, recovery_action, recovery_reason, last_activity_at,
                 human_wait_started_at, assignment_generation, pending, updated_at
             ) VALUES (?1, 'await_human', ?2, ?3, ?3, 0, 0, ?3)
             ON CONFLICT(task_execution_id) DO UPDATE SET
                recovery_action = 'await_human', recovery_reason = excluded.recovery_reason,
                 activity_deadline_at = NULL, review_deadline_at = NULL,
                 total_deadline_at = NULL, human_wait_started_at = excluded.human_wait_started_at,
                 pending = 0, updated_at = excluded.updated_at",
            params![execution.id, reason, now],
        )?;
        Ok(Some(UndeliveredDispatchInterruption {
            execution_id: execution.id,
            parent_discussion_id: execution.parent_discussion_id,
            task_id: execution.task_id,
        }))
    })
}

/// Turn a hard provider quota into an explicit human checkpoint. There is no
/// automatic retry here: the same exhausted account would only burn another
/// call. Provider fallback, when configured, is selected by KT-321 upstream.
pub fn escalate_execution_for_dispatch_quota(
    conn: &Connection,
    dispatch_job_id: &str,
    provider: &str,
) -> Result<Option<(String, String)>> {
    let Some(execution) = get_execution_for_dispatch(conn, dispatch_job_id)? else {
        return Ok(None);
    };
    if execution.status.is_terminal() {
        return Ok(Some((execution.id, execution.parent_discussion_id)));
    }
    if execution.status != TaskExecutionStatus::Escalated {
        transition_execution(
            conn,
            &execution.id,
            TaskExecutionStatus::Escalated,
            &OrchestrationActor {
                kind: PlanningActorKind::System,
                id: Some("provider-quota-guard".into()),
                session_id: None,
                source_message_id: None,
            },
            serde_json::json!({
                "failure_kind": "quota_exhausted",
                "provider": provider,
                "dispatch_job_id": dispatch_job_id,
            }),
        )?;
    }
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO task_execution_recovery (
             task_execution_id, recovery_action, recovery_reason, last_activity_at,
             assignment_generation, watchdog_redispatches, human_wait_started_at,
             pending, updated_at
         ) VALUES (?1, 'await_human', ?2, ?3, 0, 0, ?3, 0, ?3)
         ON CONFLICT(task_execution_id) DO UPDATE SET
             recovery_action = 'await_human', recovery_reason = excluded.recovery_reason,
             activity_deadline_at = NULL, review_deadline_at = NULL,
             total_deadline_at = NULL, human_wait_started_at = ?3,
             pending = 0, updated_at = ?3",
        params![execution.id, format!("quota_exhausted:{provider}"), now,],
    )?;
    Ok(Some((execution.id, execution.parent_discussion_id)))
}

/// Move an execution to `Blocked` and stamp a human-readable reason plus a structured
/// `code` (KT-328/KT-334: consumers branch on the code, never the prose), so a partial
/// provisioning failure leaves an explicitly resumable row (DoD-6) instead of a silent
/// orphan. The transition is the guarded, journaled CAS; the reason + code land in the
/// same savepoint. Returns whether the row moved (a raced terminal wins).
pub fn block_execution(
    conn: &Connection,
    exec_id: &str,
    actor: &OrchestrationActor,
    reason: &str,
    code: Option<BlockedReasonCode>,
) -> Result<bool> {
    in_savepoint(conn, |conn| {
        let moved = transition_execution(
            conn,
            exec_id,
            TaskExecutionStatus::Blocked,
            actor,
            serde_json::json!({
                "reason": reason,
                "code": code.map(|c| c.as_str()),
                "phase": "provisioning"
            }),
        )?;
        if moved {
            conn.execute(
                "UPDATE task_executions \
                 SET blocked_reason = ?2, blocked_reason_code = ?3, updated_at = ?4 \
                 WHERE id = ?1",
                params![
                    exec_id,
                    reason,
                    code.map(|c| c.as_str()),
                    Utc::now().to_rfc3339()
                ],
            )?;
        }
        Ok(moved)
    })
}

/// Inputs to [`commit_provisioning_checkpoint`] — the single atomic commit that
/// makes an execution durably launchable (ADR §4bis; DoD-7/8).
pub struct ProvisioningCheckpoint<'a> {
    pub exec_id: &'a str,
    /// The sub-discussion the brief is posted into.
    pub sub_discussion_id: &'a str,
    /// `KT-###` or the task uuid — the CAS resolves it.
    pub task_reference: &'a str,
    /// Attempt-scoped dedupe suffix (0 at launch; KT-319 bumps it).
    pub attempt_no: u32,
    /// The pre-built brief (its `id` is set deterministically by the caller).
    pub brief: &'a DiscussionMessage,
    /// The native worker identity (`DiscussionAgent`/`Agent`). A `Cli` kind is
    /// refused by the saga before we get here (its handshake is KT-328).
    pub target: &'a MessageTarget,
    pub actor: &'a OrchestrationActor,
}

/// The verdict of the final checkpoint.
pub enum CheckpointOutcome {
    /// Everything committed: brief + single dispatch visible, execution `Working`,
    /// task `InProgress`. Carries the dispatch job id pinned onto the execution.
    Committed { dispatch_job_id: String },
    /// The task-CAS refused (`NotTodo`/`BlockedByActive`); the WHOLE commit rolled
    /// back — no brief, no job, execution still `Provisioning`, task still `Todo`.
    TaskNotStarted(crate::db::planning::StartTaskCheckpoint),
    /// The execution left `Provisioning` beneath us (a raced cancel/interrupt);
    /// the whole commit rolled back.
    ExecutionRaced,
}

/// The final anti-race checkpoint of atomic provisioning (DoD-7/8). In ONE SQLite
/// transaction it (1) inserts the brief + its exact typed target + the single
/// native dispatch via the `_within_tx` variant, (2) pins the returned job id onto
/// the execution, (3) CAS `Provisioning → Working`, and (4) CAS the task
/// `Todo → InProgress` as the sole anti-race authority. Nothing is visible to the
/// dispatcher (`list_runnable_ids`) or `wait_for_peer` until `commit` — so a
/// worker can never start while the task is still `Todo`. Any refusal or raced
/// move rolls the entire commit back (the `Transaction` drops un-committed).
pub fn commit_provisioning_checkpoint(
    conn: &Connection,
    input: &ProvisioningCheckpoint,
) -> Result<CheckpointOutcome> {
    ensure_task_worker_transport_compatible(input.target)?;
    if matches!(input.target.kind, MessageTargetKind::Cli) {
        bail!(
            "Cli worker is not launchable in V1 (KT-328 handshake) — refuse before the checkpoint"
        );
    }
    let tx = conn.unchecked_transaction()?;

    // (1) Brief + typed target + the single native dispatch — all inside THIS tx.
    let targets = [input.target.clone()];
    let dispatch_key = format!("orch-dispatch:{}:{}", input.exec_id, input.attempt_no);
    let job_id = Uuid::new_v4().to_string();
    // DiscussionAgent → principal (no override); Agent → punctual override.
    let agent_override = match input.target.kind {
        MessageTargetKind::Agent => Some(&input.target.agent_type),
        _ => None,
    };
    let specs = [crate::db::discussions::UserDispatchSpec {
        job_id: &job_id,
        agent_override,
        dedupe_key: Some(&dispatch_key),
    }];
    let (_sort_order, jobs) =
        crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
            &tx,
            input.sub_discussion_id,
            input.brief,
            &targets,
            &specs,
            None, // backend author — no CLI session
        )?;
    // Single-task native provisioning enqueues EXACTLY one job. Assert it (a Vec of
    // >1 would be a silent double-enqueue) rather than a lenient `.first()`.
    let [job] = &jobs[..] else {
        bail!(
            "provisioning checkpoint expected exactly one native dispatch, got {}",
            jobs.len()
        );
    };
    attach_execution_dispatch(&tx, input.exec_id, &job.id)?;

    // (2) Execution Provisioning → Working (guarded, journaled).
    let moved = transition_execution(
        &tx,
        input.exec_id,
        TaskExecutionStatus::Working,
        input.actor,
        serde_json::json!({ "phase": "launchable", "dispatch_job_id": job.id }),
    )?;
    if !moved {
        // Raced out of Provisioning — return without commit; `tx` drops = rollback.
        return Ok(CheckpointOutcome::ExecutionRaced);
    }

    // (3) Sole anti-race authority: task Todo → InProgress in the SAME tx.
    match crate::db::planning::mark_task_in_progress_within_tx(
        &tx,
        input.task_reference,
        input.actor,
    )? {
        crate::db::planning::StartTaskCheckpoint::Started => {
            let dispatch_job_id = job.id.clone();
            tx.commit()?;
            Ok(CheckpointOutcome::Committed { dispatch_job_id })
        }
        // Non-Started: return without commit — task stays Todo, execution stays
        // Provisioning, no brief/job visible.
        other => Ok(CheckpointOutcome::TaskNotStarted(other)),
    }
}

/// Inputs to [`commit_cli_provisioning_checkpoint`] — the FINAL atomic checkpoint of
/// a CLI-worker handshake (KT-328 tranche 2, commit 2). The twin of
/// [`ProvisioningCheckpoint`], but the brief carries a `Cli` target and ZERO native
/// dispatch (the joined worker is woken by `wait_for_peer`, KT-330), and it also
/// settles the control offer and posts the durable "session attached" notice.
pub struct CliProvisioningCheckpoint<'a> {
    pub exec_id: &'a str,
    /// The sub-discussion the work brief is posted into.
    pub sub_discussion_id: &'a str,
    /// The origin room the durable attach notice is posted into.
    pub origin_discussion_id: &'a str,
    /// `KT-###` or the task uuid — the task CAS resolves it.
    pub task_reference: &'a str,
    /// The opaque control-offer id being settled (must be `accepting`).
    pub offer_id: &'a str,
    /// The pre-built work brief (its `id` is deterministic per `(exec, attempt)`).
    pub brief: &'a DiscussionMessage,
    /// The exact `Cli` worker target the brief is addressed to.
    pub target: &'a MessageTarget,
    /// The durable "session attached to execution X, child room Y" notice for the
    /// ORIGIN room — the session just left it, so this must never be silent (DoD-6).
    pub attach_notice: &'a DiscussionMessage,
    pub actor: &'a OrchestrationActor,
}

/// The verdict of the CLI final checkpoint.
pub enum CliCheckpointOutcome {
    /// Everything committed: brief visible in the child (Cli-targeted, no dispatch),
    /// execution `Working`, task `InProgress`, offer `accepted`, attach notice posted.
    Committed,
    /// Idempotent resume: the offer is already `accepted` — the handshake committed on
    /// a prior attempt, so this is a no-op (a crash after commit re-converges here).
    AlreadyCommitted,
    /// The offer is not `accepting` (accept was never staged, or it settled otherwise);
    /// `status` names the real state. Never a false success.
    OfferNotAccepting {
        status: crate::models::WorkerOfferStatus,
    },
    /// The task-CAS refused (`NotTodo`/`BlockedByActive`); the WHOLE commit rolled back
    /// — no brief, no notice, execution still `Blocked`, task still `Todo`, offer still
    /// `accepting` (resumable).
    TaskNotStarted(crate::db::planning::StartTaskCheckpoint),
    /// The execution or offer raced beneath us; the whole commit rolled back.
    ExecutionRaced,
}

/// The final anti-race checkpoint of a CLI-worker handshake (KT-328 tranche 2). In ONE
/// SQLite transaction it (1) resumes the durable hold `Blocked → Provisioning` (the
/// park set `blocked_from = Provisioning`, so it clears back exactly there — ADR §3),
/// (2) posts the work brief into the CHILD with the exact `Cli` target and ZERO
/// dispatch, (3) CAS `Provisioning → Working`, (4) CAS the task `Todo → InProgress` as
/// the sole anti-race authority, (5) settles the offer `accepting → accepted`, and (6)
/// posts the durable attach notice into the ORIGIN room. Nothing is visible until
/// `commit`; any refusal or raced move rolls the ENTIRE commit back (the `Transaction`
/// drops un-committed). Idempotent: an already-`accepted` offer short-circuits to
/// `AlreadyCommitted` before touching anything, and the brief/notice ids are
/// deterministic so a resume from `accepting` re-runs cleanly.
pub fn commit_cli_provisioning_checkpoint(
    conn: &Connection,
    input: &CliProvisioningCheckpoint,
) -> Result<CliCheckpointOutcome> {
    use crate::models::WorkerOfferStatus;
    if !matches!(input.target.kind, MessageTargetKind::Cli) {
        bail!("commit_cli_provisioning_checkpoint requires a Cli worker target");
    }
    let offer =
        crate::db::worker_offers::get_worker_offer(conn, input.offer_id)?.ok_or_else(|| {
            anyhow::anyhow!(
                "worker offer {} vanished before the checkpoint",
                input.offer_id
            )
        })?;
    match offer.status {
        WorkerOfferStatus::Accepted => return Ok(CliCheckpointOutcome::AlreadyCommitted),
        WorkerOfferStatus::Accepting => {}
        status => return Ok(CliCheckpointOutcome::OfferNotAccepting { status }),
    }

    let tx = conn.unchecked_transaction()?;

    // (1) Resume the durable hold to its origin (Blocked → Provisioning).
    let moved = transition_execution(
        &tx,
        input.exec_id,
        TaskExecutionStatus::Provisioning,
        input.actor,
        serde_json::json!({ "phase": "worker_accepted", "worker": "cli" }),
    )?;
    if !moved {
        return Ok(CliCheckpointOutcome::ExecutionRaced);
    }

    // (2) Work brief into the CHILD: exact Cli target, ZERO dispatch (no native spawn;
    // the joined worker is woken via wait_for_peer — KT-330).
    let targets = [input.target.clone()];
    crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
        &tx,
        input.sub_discussion_id,
        input.brief,
        &targets,
        &[],
        None,
    )?;

    // (3) Execution Provisioning → Working.
    let moved = transition_execution(
        &tx,
        input.exec_id,
        TaskExecutionStatus::Working,
        input.actor,
        serde_json::json!({ "phase": "launchable", "worker": "cli" }),
    )?;
    if !moved {
        return Ok(CliCheckpointOutcome::ExecutionRaced);
    }

    // (4) Sole anti-race authority: task Todo → InProgress in the SAME tx.
    match crate::db::planning::mark_task_in_progress_within_tx(
        &tx,
        input.task_reference,
        input.actor,
    )? {
        crate::db::planning::StartTaskCheckpoint::Started => {}
        other => return Ok(CliCheckpointOutcome::TaskNotStarted(other)),
    }

    // (5) Settle the offer accepting → accepted. A lost CAS (a raced cancel/expire)
    // rolls the whole commit back rather than committing a half-done handshake.
    if !crate::db::worker_offers::transition_offer_status(
        &tx,
        input.offer_id,
        WorkerOfferStatus::Accepting,
        WorkerOfferStatus::Accepted,
        None,
    )? {
        return Ok(CliCheckpointOutcome::ExecutionRaced);
    }

    // (6) Durable attach notice in the ORIGIN room (no target, no dispatch → visible
    // timeline entry, no spawn) — the session just left origin, never silently (DoD-6).
    crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
        &tx,
        input.origin_discussion_id,
        input.attach_notice,
        &[],
        &[],
        None,
    )?;

    tx.commit()?;
    Ok(CliCheckpointOutcome::Committed)
}

/// The verdict of the CLI rework re-accept checkpoint (KT-319 tranche 3b, DoD-9).
pub enum CliReworkOutcome {
    /// The hold resumed `Blocked → Provisioning → Working` and the offer settled
    /// `accepting → accepted`.
    Resumed,
    /// Idempotent resume: the offer is already `accepted` (a crash after commit or a
    /// duplicate re-accept re-converges here) — a no-op.
    AlreadyResumed,
    /// The offer is not `accepting` (the re-accept was never staged, or it settled
    /// otherwise); `status` names the real state. Never a false success.
    OfferNotAccepting {
        status: crate::models::WorkerOfferStatus,
    },
    /// The execution raced beneath the `Blocked → Working` CAS, or the offer settle lost
    /// its CAS; the whole commit rolled back — resumable.
    ExecutionRaced,
}

pub enum CliReassignmentOutcome {
    Resumed,
    AlreadyResumed,
    OfferNotAccepting {
        status: crate::models::WorkerOfferStatus,
    },
    ExecutionRaced,
}

/// Final checkpoint for a CLI reassignment after the exact replacement session
/// accepted and was moved to the existing child discussion. The task is already
/// InProgress, so this settles only the interrupted execution, the bounded
/// handoff message and the offer — atomically, without replaying provisioning.
pub fn commit_cli_reassignment_checkpoint(
    conn: &Connection,
    exec_id: &str,
    offer_id: &str,
    child_discussion_id: &str,
    handoff: &DiscussionMessage,
    target: &MessageTarget,
    actor: &OrchestrationActor,
) -> Result<CliReassignmentOutcome> {
    use crate::models::WorkerOfferStatus;
    let offer = crate::db::worker_offers::get_worker_offer(conn, offer_id)?
        .ok_or_else(|| anyhow::anyhow!("worker offer {offer_id} vanished"))?;
    match offer.status {
        WorkerOfferStatus::Accepted => return Ok(CliReassignmentOutcome::AlreadyResumed),
        WorkerOfferStatus::Accepting => {}
        status => return Ok(CliReassignmentOutcome::OfferNotAccepting { status }),
    }
    let tx = conn.unchecked_transaction()?;
    if !transition_execution(
        &tx,
        exec_id,
        TaskExecutionStatus::Working,
        actor,
        serde_json::json!({ "phase": "cli_reassignment_accepted" }),
    )? {
        return Ok(CliReassignmentOutcome::ExecutionRaced);
    }
    crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
        &tx,
        child_discussion_id,
        handoff,
        std::slice::from_ref(target),
        &[],
        None,
    )?;
    if !crate::db::worker_offers::transition_offer_status(
        &tx,
        offer_id,
        WorkerOfferStatus::Accepting,
        WorkerOfferStatus::Accepted,
        None,
    )? {
        return Ok(CliReassignmentOutcome::ExecutionRaced);
    }
    clear_execution_recovery(&tx, exec_id, "cli_worker_reassigned")?;
    tx.commit()?;
    Ok(CliReassignmentOutcome::Resumed)
}

/// The atomic re-accept checkpoint for a CLI worker resuming after a request_changes (KT-319
/// tranche 3b, DoD-9). The re-offer re-entered the provisioning handshake and parked the
/// execution `Provisioning → Blocked`; the worker re-accepted (offer now `accepting`), so in
/// ONE tx this (1) resumes the Provisioning-origin hold `Blocked → Provisioning → Working` —
/// the EXACT mirror of the initial handshake checkpoint, reusing its resume machinery — and
/// (2) settles the offer `accepting → accepted`.
///
/// Unlike the INITIAL handshake checkpoint it deliberately runs NO task-CAS and NO session
/// move: the task is ALREADY `InProgress` and the session ALREADY in the child (both from the
/// first accept — a request_changes leaves them untouched, only the round/attempt/status
/// change). The anti-race authority the task-CAS held at provisioning is REPLACED, not
/// removed, by the two CAS here: the offer `pending → accepting` stage (in
/// `accept_worker_offer`) already elected ONE staging winner — the loser never reaches this
/// checkpoint — and the `Blocked → Working` CAS elects one commit winner. A concurrent or
/// duplicate re-accept therefore yields exactly one `Resumed`; the rest are refused or
/// idempotent. No brief is re-posted — the findings reached the worker in the child at
/// request_changes time (DoD-4). Idempotent: an already-`accepted` offer short-circuits to
/// `AlreadyResumed` before touching anything.
pub fn commit_cli_rework_checkpoint(
    conn: &Connection,
    exec_id: &str,
    offer_id: &str,
    actor: &OrchestrationActor,
) -> Result<CliReworkOutcome> {
    use crate::models::WorkerOfferStatus;
    let offer = crate::db::worker_offers::get_worker_offer(conn, offer_id)?.ok_or_else(|| {
        anyhow::anyhow!("worker offer {offer_id} vanished before the rework checkpoint")
    })?;
    match offer.status {
        WorkerOfferStatus::Accepted => return Ok(CliReworkOutcome::AlreadyResumed),
        WorkerOfferStatus::Accepting => {}
        status => return Ok(CliReworkOutcome::OfferNotAccepting { status }),
    }

    let tx = conn.unchecked_transaction()?;
    // (1) Resume the Provisioning-origin hold `Blocked → Provisioning` (the re-offer parked it
    //     there, so `blocked_resume_allowed(Provisioning, Provisioning)` clears it), then
    //     `Provisioning → Working` — the EXACT mirror of the initial handshake checkpoint.
    if !transition_execution(
        &tx,
        exec_id,
        TaskExecutionStatus::Provisioning,
        actor,
        serde_json::json!({ "phase": "worker_reaccepted", "worker": "cli" }),
    )? {
        return Ok(CliReworkOutcome::ExecutionRaced);
    }
    if !transition_execution(
        &tx,
        exec_id,
        TaskExecutionStatus::Working,
        actor,
        serde_json::json!({ "phase": "rework_launchable", "worker": "cli" }),
    )? {
        return Ok(CliReworkOutcome::ExecutionRaced);
    }
    // (2) Settle the offer `accepting → accepted` (the second CAS). A lost CAS rolls the whole
    //     resume back rather than committing a half-done re-accept.
    if !crate::db::worker_offers::transition_offer_status(
        &tx,
        offer_id,
        WorkerOfferStatus::Accepting,
        WorkerOfferStatus::Accepted,
        None,
    )? {
        return Ok(CliReworkOutcome::ExecutionRaced);
    }
    tx.commit()?;
    Ok(CliReworkOutcome::Resumed)
}

/// Inputs to [`commit_delivery_checkpoint`] — the atomic worker-delivery step (KT-319
/// tranche 2). One SQLite transaction persists the manifest, flips the execution
/// `Working → AwaitingReview` (guarded + journaled via [`transition_execution`], never a
/// bare UPDATE), records the queryable `review_requested` obligation and posts the
/// principal-targeted review request into the PARENT room. Nothing is visible until
/// commit; any raced move rolls the whole commit back.
pub struct DeliveryCheckpoint<'a> {
    pub exec_id: &'a str,
    pub attempt_no: u32,
    /// The exact HEAD delivered (denormalized onto the delivery row for the DoD-5 check).
    pub head_sha: &'a str,
    /// The full validated DeliveryManifest v1 bytes.
    pub manifest_json: &'a str,
    /// The principal (parent) room the review request is posted into.
    pub parent_discussion_id: &'a str,
    /// Pre-built review-request message — its id is deterministic per `(exec, attempt)`, so
    /// a resume never double-posts (the message PK rejects it).
    pub review_request: &'a DiscussionMessage,
    /// The exact principal target the review request is addressed to (the parent's native
    /// agent). Posted with ZERO dispatch: a joined-CLI principal is woken via
    /// `wait_for_peer` (KT-330); a sleeping native principal's immediate wake is KT-335.
    pub principal_target: &'a MessageTarget,
    /// The `review_requested` event payload — carries the TARGETED principal identity so
    /// the obligation is auditable and queryable, never merely deduced (DoD-3).
    pub review_requested_changes: serde_json::Value,
    pub actor: &'a OrchestrationActor,
}

/// The verdict of the delivery checkpoint.
pub enum DeliveryCheckpointOutcome {
    /// Manifest persisted, execution `AwaitingReview`, obligation recorded + review posted.
    Delivered,
    /// Idempotent resume: the execution is already `AwaitingReview` WITH a delivery row for
    /// this attempt — a crash/double-click re-converges here as a no-op (DoD-8).
    AlreadyDelivered,
    /// The execution is not `Working` (nor an already-delivered `AwaitingReview`); `status`
    /// names the real state. Never a false success.
    NotDeliverable { status: TaskExecutionStatus },
    /// The execution raced beneath the `Working → AwaitingReview` CAS; the commit rolled back.
    ExecutionRaced,
}

/// The final atomic checkpoint of a worker delivery (KT-319 tranche 2). In ONE SQLite
/// transaction it (1) upserts the validated manifest for `(exec, attempt)` (idempotent),
/// (2) flips `Working → AwaitingReview` through the guarded, journaled CAS, (3) records
/// the queryable `review_requested` obligation carrying the targeted principal identity,
/// and (4) posts the principal-targeted review request in the parent room with ZERO
/// dispatch. Idempotent: an already-`AwaitingReview` execution that already has this
/// attempt's delivery short-circuits to `AlreadyDelivered`, and the review-request id is
/// deterministic so a resume never double-posts.
pub fn commit_delivery_checkpoint(
    conn: &Connection,
    input: &DeliveryCheckpoint,
) -> Result<DeliveryCheckpointOutcome> {
    use TaskExecutionStatus::*;
    let tx = conn.unchecked_transaction()?;
    let current: Option<String> = tx
        .query_row(
            "SELECT status FROM task_executions WHERE id = ?1",
            params![input.exec_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(current) = current else {
        return Ok(DeliveryCheckpointOutcome::ExecutionRaced);
    };
    let status = TaskExecutionStatus::from_str(&current)?;

    match status {
        // Idempotent resume ONLY if this exact attempt already has a delivery row — a
        // fully-committed prior delivery. (A rolled-back partial never persists: the whole
        // step is one tx.) Any other AwaitingReview is a genuine "not deliverable".
        AwaitingReview
            if crate::db::worker_deliveries::get_delivery(
                &tx,
                input.exec_id,
                input.attempt_no,
            )?
            .is_some() =>
        {
            Ok(DeliveryCheckpointOutcome::AlreadyDelivered)
        }
        Working => {
            // (1) Persist the validated manifest (idempotent upsert on (exec, attempt)).
            crate::db::worker_deliveries::upsert_delivery(
                &tx,
                input.exec_id,
                input.attempt_no,
                input.head_sha,
                input.manifest_json,
            )?;
            // (2) Working → AwaitingReview (guarded + journaled). The CAS is the sole authority.
            let moved = transition_execution(
                &tx,
                input.exec_id,
                AwaitingReview,
                input.actor,
                serde_json::json!({ "phase": "delivered", "head_sha": input.head_sha }),
            )?;
            if !moved {
                return Ok(DeliveryCheckpointOutcome::ExecutionRaced);
            }
            // (3) The queryable review obligation, carrying the targeted principal identity
            // (DoD-3): a lost review-request notification is thus detectable, not deduced.
            record_execution_event(
                &tx,
                input.exec_id,
                "review_requested",
                None,
                None,
                input.actor,
                input.review_requested_changes.clone(),
            )?;
            // (4) Principal-targeted review request in the PARENT room, ZERO dispatch (no
            // phantom native turn; a joined-CLI principal is woken by wait_for_peer).
            let targets = [input.principal_target.clone()];
            crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
                &tx,
                input.parent_discussion_id,
                input.review_request,
                &targets,
                &[],
                None,
            )?;
            tx.commit()?;
            Ok(DeliveryCheckpointOutcome::Delivered)
        }
        other => Ok(DeliveryCheckpointOutcome::NotDeliverable { status: other }),
    }
}

/// The findings hand-off delivered to the worker on `request_changes` (KT-319 DoD-4): a
/// structured message posted to the worker IN THE CHILD room and targeted to the worker.
/// A joined CLI is woken by `wait_for_peer`; a native worker uses this same message as the
/// trigger of its attempt-scoped durable dispatch. Its id is deterministic per
/// `(exec, attempt)` so a resume never double-posts.
pub struct ReviewFindingsDelivery<'a> {
    pub child_discussion_id: &'a str,
    pub message: &'a DiscussionMessage,
    pub worker_target: &'a MessageTarget,
}

/// The escalation solicitation posted to the PRINCIPAL in the parent room when the review
/// budget is exhausted (KT-319 DoD-6): `review_rounds` reached `max_review_rounds`, so instead
/// of re-offering another attempt the run pauses in `Escalated` and asks a human to decide.
/// Posted with ZERO dispatch (a joined-CLI principal is woken by `wait_for_peer`; a sleeping
/// native principal's immediate wake is KT-335). Deterministic id per `(exec, attempt)` so a
/// resume never double-posts.
pub struct EscalationDelivery<'a> {
    pub parent_discussion_id: &'a str,
    pub message: &'a DiscussionMessage,
    pub principal_target: &'a MessageTarget,
}

/// The CLI-worker re-offer posted when a request_changes stays UNDER the review budget
/// (KT-319 tranche 3b, DoD-9). The offer id is minted server-side FIRST so the pre-built
/// `control_message` embeds the exact id; the checkpoint opens the offer with that id and
/// posts the message inside ONE tx, so the message body and the offer row can never
/// disagree. `sub_discussion_id` is BOTH the offer origin and child — the worker is
/// already in the sub-discussion (it never left during the review), so it is re-offered
/// and woken in place, and the accept routes to the rework checkpoint (no session move).
pub struct ReworkReoffer<'a> {
    /// Pre-generated opaque offer id (embedded in `control_message`).
    pub offer_id: &'a str,
    /// The next attempt the re-offer opens (`exec.attempt_no + 1`).
    pub new_attempt_no: u32,
    /// The worker session the re-offer targets (this execution's exact CLI worker).
    pub target_cli_session_id: i64,
    /// The worker's sub-discussion — offer origin AND child (re-offered in place).
    pub sub_discussion_id: &'a str,
    /// Pre-built control-offer message (deterministic id per `(exec, new_attempt)`, embeds
    /// `offer_id`), posted to the worker in the child with ZERO dispatch.
    pub control_message: &'a DiscussionMessage,
    /// The exact `Cli` target the control offer is addressed to.
    pub control_target: &'a MessageTarget,
}

/// Native-worker reactivation committed with the review decision. Unlike a CLI
/// worker, a native worker has no joined session waiting on the child room: the
/// findings message must therefore own a fresh durable dispatch. Both ids are
/// minted before the checkpoint and are attempt-scoped, so a retry observes the
/// same atomic outcome instead of reviving a terminal dispatch from the prior
/// attempt.
pub struct NativeReworkDispatch<'a> {
    pub job_id: &'a str,
    pub dedupe_key: &'a str,
}

/// Inputs to [`commit_review_checkpoint`] — the atomic principal-decision step (KT-319
/// tranche 3a/3b). One SQLite transaction persists the ReviewDecision, then flips the
/// execution via [`transition_execution`] (guarded CAS + journal, never a bare UPDATE):
/// `AwaitingReview → Approved` for approve, or for request_changes bumps `review_rounds`,
/// posts the findings to the worker in the child, then EITHER escalates (budget exhausted)
/// OR re-activates the worker: `AwaitingReview → ChangesRequested`, bumps `attempt_no`,
/// cancel-first + opens the next re-offer, and parks `ChangesRequested → Blocked`
/// (awaiting_worker_acceptance). Nothing is visible until commit; any raced move rolls the
/// whole commit back.
pub struct ReviewCheckpoint<'a> {
    pub exec_id: &'a str,
    pub attempt_no: u32,
    pub verdict: ReviewVerdict,
    /// The full validated ReviewDecision v1 bytes (persisted for audit/history — DoD-2/8).
    pub decision_json: &'a str,
    /// request_changes only: the findings message posted to the worker in the child. `None`
    /// for approve (approve touches neither the child nor the worktree).
    pub findings: Option<ReviewFindingsDelivery<'a>>,
    /// request_changes only: the solicitation posted to the principal if the round bump reaches
    /// `max_review_rounds` (DoD-6). Built alongside `findings`; used only on the escalate branch.
    pub escalation: Option<EscalationDelivery<'a>>,
    /// request_changes below the budget only: the CLI re-offer that re-activates the worker
    /// for the next attempt (DoD-9). `None` for approve, for the escalate branch, and for a
    /// non-CLI worker (which uses `native_dispatch`).
    pub reactivation: Option<ReworkReoffer<'a>>,
    /// request_changes below the budget for a native worker: fresh durable
    /// dispatch triggered by the findings message. Mutually exclusive with
    /// `reactivation` (which is the joined-CLI handshake).
    pub native_dispatch: Option<NativeReworkDispatch<'a>>,
    pub actor: &'a OrchestrationActor,
}

/// The verdict of the review checkpoint.
pub enum ReviewCheckpointOutcome {
    /// approve committed: the ReviewDecision is persisted and the execution is `Approved`.
    Approved,
    /// request_changes committed: decision persisted, `review_rounds` bumped, findings posted
    /// to the worker in the child, execution `ChangesRequested`.
    ChangesRequested,
    /// request_changes committed AND exhausted the review budget (`review_rounds` reached
    /// `max_review_rounds`): decision persisted, round bumped, execution `Escalated`, and the
    /// principal solicited in the parent room (DoD-6). No re-offer — a human decides.
    Escalated,
    /// The execution is not `AwaitingReview`; `status` names the real state (reachable only
    /// after the service's authz, so not an oracle). A sequential re-decide of an already
    /// decided attempt lands here — no duplicate review, no duplicate transition (DoD-8).
    NotReviewable { status: TaskExecutionStatus },
    /// The execution raced beneath the review CAS (a concurrent decider won); the commit
    /// rolled back so neither the review row nor the transition persisted (DoD-8).
    ExecutionRaced,
}

/// The final atomic checkpoint of a principal review (KT-319 tranche 3a). In ONE SQLite
/// transaction it (1) upserts the validated ReviewDecision for `(exec, attempt)` (idempotent),
/// then (2) for approve flips `AwaitingReview → Approved`; for request_changes bumps
/// `review_rounds`, posts the findings to the worker in the child (ZERO dispatch, sub-disc +
/// worktree untouched — DoD-4), and flips `AwaitingReview → ChangesRequested`. Every flip
/// goes through the guarded, journaled CAS ([`transition_execution`]) — never a bare status
/// UPDATE. A concurrent decider that loses the CAS rolls back with nothing persisted.
pub fn commit_review_checkpoint(
    conn: &Connection,
    input: &ReviewCheckpoint,
) -> Result<ReviewCheckpointOutcome> {
    use TaskExecutionStatus::*;
    let tx = conn.unchecked_transaction()?;
    let row: Option<(String, i64, i64)> = tx
        .query_row(
            "SELECT status, review_rounds, max_review_rounds FROM task_executions WHERE id = ?1",
            params![input.exec_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((current, review_rounds, max_review_rounds)) = row else {
        return Ok(ReviewCheckpointOutcome::ExecutionRaced);
    };
    let status = TaskExecutionStatus::from_str(&current)?;
    if status != AwaitingReview {
        // A committed review always leaves AwaitingReview (Approved/ChangesRequested), so a
        // sequential re-decide lands here — the status gate is the idempotency guard.
        return Ok(ReviewCheckpointOutcome::NotReviewable { status });
    }

    // (1) Persist the validated decision (idempotent upsert on (exec, attempt)).
    let decision_str = match input.verdict {
        ReviewVerdict::Approve => "approve",
        ReviewVerdict::RequestChanges => "request_changes",
    };
    crate::db::worker_reviews::upsert_review(
        &tx,
        input.exec_id,
        input.attempt_no,
        decision_str,
        input.decision_json,
    )?;

    match input.verdict {
        ReviewVerdict::Approve => {
            // (2) AwaitingReview → Approved (guarded CAS + journal; the CAS is the sole
            // anti-race authority). Approved is NOT terminal: KT-320 resumes it to
            // Integrating.
            let moved = transition_execution(
                &tx,
                input.exec_id,
                Approved,
                input.actor,
                serde_json::json!({
                    "phase": "reviewed",
                    "verdict": "approve",
                    "attempt": input.attempt_no,
                }),
            )?;
            if !moved {
                return Ok(ReviewCheckpointOutcome::ExecutionRaced);
            }
            tx.commit()?;
            Ok(ReviewCheckpointOutcome::Approved)
        }
        ReviewVerdict::RequestChanges => {
            // (2a) Record this review round (a counter, not a status — transition_execution owns
            // status; this scoped UPDATE never touches `status`).
            tx.execute(
                "UPDATE task_executions SET review_rounds = review_rounds + 1 WHERE id = ?1",
                params![input.exec_id],
            )?;
            let new_rounds = review_rounds + 1;

            // (2b) Budget gate (DoD-6): `max_review_rounds` is the number of review rounds the
            // run is CONFIGURED to allow — a run set to N must deliver N rounds, so the budget is
            // exhausted only ONCE a request_changes pushes `review_rounds` PAST it (`>`, not `>=`;
            // `max = N` gives N re-offers, escalates on the N+1-th). At/under the cap it
            // re-activates the worker; past it, it pauses in `Escalated` and solicits a human.
            if new_rounds > max_review_rounds {
                // AwaitingReview → Escalated (the generalized budget escape, ADR §3).
                let moved = transition_execution(
                    &tx,
                    input.exec_id,
                    Escalated,
                    input.actor,
                    serde_json::json!({
                        "phase": "reviewed",
                        "verdict": "request_changes",
                        "attempt": input.attempt_no,
                        "escalation": "review_budget_exhausted",
                        "review_rounds": new_rounds,
                        "max_review_rounds": max_review_rounds,
                    }),
                )?;
                if !moved {
                    return Ok(ReviewCheckpointOutcome::ExecutionRaced);
                }
                let escalation = input
                    .escalation
                    .as_ref()
                    .expect("request_changes requires an escalation delivery");
                // Auditable, queryable obligation: the `escalated` event names the TARGETED
                // principal identity, so a lost solicitation is detectable, not deduced (mirrors
                // the delivery path's `review_requested`).
                record_execution_event(
                    &tx,
                    input.exec_id,
                    "escalated",
                    None,
                    None,
                    input.actor,
                    serde_json::json!({
                        "reason": "review_budget_exhausted",
                        "review_rounds": new_rounds,
                        "max_review_rounds": max_review_rounds,
                        "attempt": input.attempt_no,
                        "principal_discussion_id": escalation.parent_discussion_id,
                        "principal_target": escalation.principal_target,
                    }),
                )?;
                // Durable solicitation to the principal in the PARENT room, ZERO dispatch.
                let targets = [escalation.principal_target.clone()];
                crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
                    &tx,
                    escalation.parent_discussion_id,
                    escalation.message,
                    &targets,
                    &[],
                    None,
                )?;
                tx.commit()?;
                return Ok(ReviewCheckpointOutcome::Escalated);
            }

            // (2c) Below the cap: hand the findings to the worker in the CHILD. A joined CLI
            // wakes through wait_for_peer (zero dispatch); a native worker needs a fresh,
            // attempt-scoped durable dispatch in this same checkpoint.
            let findings = input
                .findings
                .as_ref()
                .expect("request_changes requires a findings delivery");
            let targets = [findings.worker_target.clone()];
            let native_specs = input.native_dispatch.as_ref().map(|native| {
                let agent_override = match findings.worker_target.kind {
                    MessageTargetKind::Agent => Some(&findings.worker_target.agent_type),
                    _ => None,
                };
                [crate::db::discussions::UserDispatchSpec {
                    job_id: native.job_id,
                    agent_override,
                    dedupe_key: Some(native.dedupe_key),
                }]
            });
            let (_sort_order, native_jobs) =
                crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
                    &tx,
                    findings.child_discussion_id,
                    findings.message,
                    &targets,
                    native_specs
                        .as_ref()
                        .map(|specs| specs.as_slice())
                        .unwrap_or(&[]),
                    None,
                )?;
            // (2d) AwaitingReview → ChangesRequested (guarded CAS + journal).
            let moved = transition_execution(
                &tx,
                input.exec_id,
                ChangesRequested,
                input.actor,
                serde_json::json!({
                    "phase": "reviewed",
                    "verdict": "request_changes",
                    "attempt": input.attempt_no,
                }),
            )?;
            if !moved {
                return Ok(ReviewCheckpointOutcome::ExecutionRaced);
            }

            // Every accepted rework is a distinct attempt. This must happen for native workers
            // too: recovery and dispatch dedupe keys are attempt-scoped, and reusing the reviewed
            // attempt would silently resolve to its already-Completed job after a restart.
            tx.execute(
                "UPDATE task_executions SET attempt_no = attempt_no + 1, updated_at = ?2 WHERE id = ?1",
                params![input.exec_id, Utc::now().to_rfc3339()],
            )?;

            // (2e) Re-activate the CLI worker for the next attempt (DoD-9), atomically with the
            // decision above — a crash re-runs the whole request_changes cleanly (nothing was
            // AwaitingReview-committed). Native workers take the atomic branch below instead.
            if let Some(re) = input.reactivation.as_ref() {
                // Cancel-first (DoD-9): no live offer of this execution may survive, so the
                // re-offer can only `Opened` — never `SessionCommittedElsewhere` onto itself.
                crate::db::worker_offers::cancel_live_offers_for_execution(&tx, input.exec_id)?;
                // Open the re-offer with the pre-minted id the control message already embeds,
                // targeting the worker's session; origin == child == the sub-discussion (the
                // worker never left, so it is re-offered + woken in place).
                let new_offer = crate::db::worker_offers::NewWorkerOffer {
                    id: Some(re.offer_id),
                    task_execution_id: input.exec_id,
                    attempt_no: re.new_attempt_no,
                    target_cli_session_id: re.target_cli_session_id,
                    origin_discussion_id: re.sub_discussion_id,
                    child_discussion_id: re.sub_discussion_id,
                    expires_at: None,
                    offer_message_id: None,
                    reason: None,
                };
                let (reason, code) = match crate::db::worker_offers::open_worker_offer(
                    &tx, &new_offer,
                )? {
                    crate::db::worker_offers::OpenOutcome::Opened(offer) => {
                        if offer.offer_message_id.is_none() {
                            let targets = [re.control_target.clone()];
                            crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
                                    &tx,
                                    re.sub_discussion_id,
                                    re.control_message,
                                    &targets,
                                    &[],
                                    None,
                                )?;
                            crate::db::worker_offers::set_offer_message(
                                &tx,
                                &offer.id,
                                &re.control_message.id,
                            )?;
                        }
                        (
                            "awaiting_worker_acceptance".to_string(),
                            BlockedReasonCode::AwaitingWorkerAcceptance,
                        )
                    }
                    // Defensive: cancel-first rules out a self-clash, so this only fires if
                    // the session is committed to ANOTHER execution — park with the naming
                    // code so a human can re-offer / pick a native worker.
                    crate::db::worker_offers::OpenOutcome::SessionCommittedElsewhere {
                        blocking,
                    } => (
                        format!(
                            "worker session already committed to execution {} (attempt {}) — \
                                 re-offer or choose a native worker",
                            blocking.task_execution_id, blocking.attempt_no
                        ),
                        BlockedReasonCode::WorkerSessionCommittedElsewhere,
                    ),
                };
                // Re-enter the provisioning handshake, THEN park Blocked — the EXACT mirror of
                // the initial handshake (`ChangesRequested → Provisioning → Blocked`). This
                // makes `blocked_from = Provisioning` (already in the frozen-127 CHECK domain),
                // so the re-accept resumes through Provisioning with no new schema.
                if !transition_execution(
                    &tx,
                    input.exec_id,
                    Provisioning,
                    input.actor,
                    serde_json::json!({ "phase": "rework_reprovision", "attempt": re.new_attempt_no }),
                )? {
                    return Ok(ReviewCheckpointOutcome::ExecutionRaced);
                }
                // Provisioning → Blocked(reason, code). The worker re-accepts the control offer to
                // resume (that accept routes to commit_cli_rework_checkpoint). The acceptance
                // window is a VISIBLE, coded Blocked — never a silent wait.
                if !block_execution(&tx, input.exec_id, input.actor, &reason, Some(code))? {
                    return Ok(ReviewCheckpointOutcome::ExecutionRaced);
                }
            } else if let Some(native) = input.native_dispatch.as_ref() {
                let [job] = &native_jobs[..] else {
                    bail!(
                        "native rework checkpoint expected exactly one dispatch, got {}",
                        native_jobs.len()
                    );
                };
                if job.id != native.job_id {
                    bail!("native rework dispatch resolved to a stale dedupe job");
                }
                attach_execution_dispatch(&tx, input.exec_id, &job.id)?;
                if !transition_execution(
                    &tx,
                    input.exec_id,
                    Working,
                    input.actor,
                    serde_json::json!({
                        "phase": "native_rework",
                        "attempt": input.attempt_no + 1,
                        "dispatch_job_id": job.id,
                    }),
                )? {
                    return Ok(ReviewCheckpointOutcome::ExecutionRaced);
                }
            }

            tx.commit()?;
            Ok(ReviewCheckpointOutcome::ChangesRequested)
        }
    }
}

/// The interrogeable review obligation (KT-319 DoD-3): every execution AWAITING a review
/// from the principal of `parent_discussion_id`. The durable `AwaitingReview` state IS the
/// queryable obligation, so a lost review-request notification is detectable rather than
/// merely deduced.
pub fn list_reviews_due_for_discussion(
    conn: &Connection,
    parent_discussion_id: &str,
) -> Result<Vec<TaskExecution>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {EXEC_COLS} FROM task_executions \
         WHERE parent_discussion_id = ?1 AND status = ?2 \
         ORDER BY created_at ASC, rowid ASC"
    ))?;
    let rows = stmt
        .query_map(
            params![
                parent_discussion_id,
                TaskExecutionStatus::AwaitingReview.as_str()
            ],
            row_to_execution,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// `Done` is more than a structurally legal state edge: it is the proof that the
/// exact candidate landed and every configured validation passed for that same
/// SHA. Keep this guard in the shared transition primitive so a future backend
/// caller cannot bypass the integration saga accidentally.
fn assert_execution_can_finish(conn: &Connection, exec_id: &str) -> Result<()> {
    let execution = get_task_execution(conn, exec_id)?
        .ok_or_else(|| anyhow::anyhow!("task execution not found"))?;
    let candidate = execution
        .candidate_merge_sha
        .as_deref()
        .filter(|sha| !sha.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("execution cannot finish without candidate_merge_sha"))?;
    let integrated = execution
        .integrated_sha
        .as_deref()
        .filter(|sha| !sha.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("execution cannot finish without integrated_sha"))?;
    if candidate != integrated {
        bail!(
            "execution cannot finish: integrated_sha {integrated} differs from candidate_merge_sha {candidate}"
        );
    }

    let run = get_orchestration_run(conn, &execution.orchestration_run_id)?
        .ok_or_else(|| anyhow::anyhow!("orchestration run not found"))?;
    let validation_runs = list_validation_runs(conn, exec_id)?;
    for required in &run.validations {
        let latest = validation_runs
            .iter()
            .filter(|validation| {
                validation.candidate_merge_sha.as_deref() == Some(candidate)
                    && validation.command == required.command
                    && validation.quick_exec_id == required.quick_exec_id
            })
            .max_by_key(|validation| validation.created_at);
        if !latest.is_some_and(crate::models::TaskExecutionValidationRun::passed) {
            bail!(
                "execution cannot finish: validation {:?} has no passing result for candidate {candidate}",
                required.command
            );
        }
    }
    Ok(())
}

/// Attempt a guarded state transition. Returns:
///   • `Err`   — the transition is not permitted by the state machine (a
///     contract violation, including any move out of a terminal state);
///   • `Ok(false)` — the transition is legal but the row moved beneath us
///     (raced CAS, or the row is gone) — the caller should stop;
///   • `Ok(true)`  — the row moved; a journal event was recorded atomically.
pub fn transition_execution(
    conn: &Connection,
    exec_id: &str,
    to: TaskExecutionStatus,
    actor: &OrchestrationActor,
    changes: serde_json::Value,
) -> Result<bool> {
    use TaskExecutionStatus::*;
    let row: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT status, blocked_from_status, interrupted_from_status \
             FROM task_executions WHERE id = ?1",
            params![exec_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((current, blocked_from_s, interrupted_from_s)) = row else {
        return Ok(false);
    };
    let from = TaskExecutionStatus::from_str(&current)?;

    // Coarse structural gate first (ADR §3).
    if !from.can_transition_to(to) {
        bail!(
            "illegal task-execution transition {} -> {}",
            from.as_str(),
            to.as_str()
        );
    }
    if to == Done {
        assert_execution_can_finish(conn, exec_id)?;
    }

    // Checkpoint-aware resume guard (ADR §3). A resume out of a Blocked/Interrupted
    // hold must honour the durable origin, not merely the structural matrix — else
    // a Provisioning-origin hold could resume Applying. Cancel/Interrupt/Escalate
    // are the generalized escapes and skip the checkpoint.
    if !matches!(to, Cancelled | Interrupted | Escalated) {
        match from {
            Blocked => {
                let origin = blocked_from_s
                    .as_deref()
                    .and_then(|s| TaskExecutionStatus::from_str(s).ok())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Blocked execution {exec_id} has no blocked_from_status checkpoint"
                        )
                    })?;
                if !TaskExecutionStatus::blocked_resume_allowed(origin, to) {
                    bail!(
                        "illegal Blocked resume {} -> {} (blocked_from = {})",
                        from.as_str(),
                        to.as_str(),
                        origin.as_str()
                    );
                }
            }
            Interrupted => {
                let origin = interrupted_from_s
                    .as_deref()
                    .and_then(|s| TaskExecutionStatus::from_str(s).ok())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Interrupted execution {exec_id} has no interrupted_from_status checkpoint"
                        )
                    })?;
                if !TaskExecutionStatus::interrupted_resume_allowed(origin, to) {
                    bail!(
                        "illegal Interrupted resume {} -> {} (interrupted_from = {})",
                        from.as_str(),
                        to.as_str(),
                        origin.as_str()
                    );
                }
            }
            _ => {}
        }
    }

    in_savepoint(conn, |conn| {
        let now = Utc::now().to_rfc3339();
        let moved = run_state::claim_status(
            conn,
            RUN_STATE_TABLE,
            exec_id,
            from.as_str(),
            to.as_str(),
            &now,
        )?;
        if !moved {
            return Ok(false);
        }
        if to.is_terminal() {
            conn.execute(
                "UPDATE task_executions SET finished_at = ?2 \
                 WHERE id = ?1 AND finished_at IS NULL",
                params![exec_id, now],
            )?;
            return_cli_worker_to_origin(conn, exec_id, to)?;
            notify_principal_of_terminal(conn, exec_id, to)?;
            if to == TaskExecutionStatus::Failed {
                park_campaign_after_failed_execution(conn, exec_id, actor)?;
            }
        }

        // Maintain the durable resume checkpoints (ADR §3). They only ever reflect
        // a *live* hold: set on entry to Blocked/Interrupted, preserved across an
        // interrupt of a Blocked row (so the deblock still targets `blocked_from`),
        // and cleared once the hold is resumed/advanced out of.
        if to == Blocked {
            // A boot reconcile may decide that an interrupted Provisioning or
            // Applying execution must park. Preserve the ORIGINAL checkpoint,
            // not the synthetic `Interrupted` hop, otherwise the later deblock
            // would have no legal resume target.
            let blocked_origin = if from == Interrupted {
                let interrupted_origin = interrupted_from_s.as_deref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Interrupted execution {exec_id} has no origin for a Blocked resume"
                    )
                })?;
                if interrupted_origin == Blocked.as_str() {
                    blocked_from_s.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "Interrupted Blocked execution {exec_id} has no blocked_from_status"
                        )
                    })?
                } else {
                    interrupted_origin
                }
            } else {
                from.as_str()
            };
            if !matches!(blocked_origin, "Provisioning" | "Applying") {
                bail!(
                    "execution {exec_id} cannot enter Blocked from recovery origin {blocked_origin}"
                );
            }
            conn.execute(
                "UPDATE task_executions SET blocked_from_status = ?2 WHERE id = ?1",
                params![exec_id, blocked_origin],
            )?;
        }
        if to == Interrupted {
            conn.execute(
                "UPDATE task_executions SET interrupted_from_status = ?2 WHERE id = ?1",
                params![exec_id, from.as_str()],
            )?;
        }
        // `blocked_reason{,_code}` describe an ACTIVE hold, not its history. Keep
        // them only while the execution is Blocked, or while a Blocked row is
        // temporarily Interrupted and can still resume that exact hold. Every
        // effective resume/advance clears the reason together with the checkpoint
        // in this savepoint. This also makes an unrelated active/terminal
        // transition self-heal a stale pre-KT-426 blocker instead of exposing two
        // contradictory states through task_exec_status.
        let preserves_blocked_hold = to == Blocked || (from == Blocked && to == Interrupted);
        if !preserves_blocked_hold {
            conn.execute(
                "UPDATE task_executions \
                 SET blocked_from_status = NULL, blocked_reason = NULL, \
                     blocked_reason_code = NULL \
                 WHERE id = ?1",
                params![exec_id],
            )?;
        }
        if from == Interrupted {
            conn.execute(
                "UPDATE task_executions SET interrupted_from_status = NULL WHERE id = ?1",
                params![exec_id],
            )?;
        }

        if to.is_terminal() {
            conn.execute(
                "UPDATE task_execution_recovery SET pending = 0, total_deadline_at = NULL, \
                        activity_deadline_at = NULL, review_deadline_at = NULL, \
                        human_wait_started_at = NULL, updated_at = ?2 \
                 WHERE task_execution_id = ?1",
                params![exec_id, Utc::now().to_rfc3339()],
            )?;
        } else {
            touch_execution_activity(conn, exec_id)?;
        }

        sync_campaign_human_gate(conn, exec_id, from, to, actor)?;

        record_execution_event(
            conn,
            exec_id,
            "transition",
            Some(from),
            Some(to),
            actor,
            changes,
        )?;
        Ok(true)
    })
}

fn park_campaign_after_failed_execution(
    conn: &Connection,
    exec_id: &str,
    actor: &OrchestrationActor,
) -> Result<()> {
    let run_id: String = conn.query_row(
        "SELECT orchestration_run_id FROM task_executions WHERE id = ?1",
        [exec_id],
        |row| row.get(0),
    )?;
    let Some(run) = get_orchestration_run(conn, &run_id)? else {
        return Ok(());
    };
    if run.kind == OrchestrationRunKind::Campaign && !run.control_state.is_terminal() {
        set_orchestration_control_state(
            conn,
            &run_id,
            OrchestrationControlState::AwaitingHuman,
            Some("a child execution failed; decide whether the campaign may continue"),
            actor,
        )?;
    }
    Ok(())
}

fn sync_campaign_human_gate(
    conn: &Connection,
    exec_id: &str,
    from: TaskExecutionStatus,
    to: TaskExecutionStatus,
    actor: &OrchestrationActor,
) -> Result<()> {
    if to != TaskExecutionStatus::Escalated && from != TaskExecutionStatus::Escalated {
        return Ok(());
    }
    let run_id: String = conn.query_row(
        "SELECT orchestration_run_id FROM task_executions WHERE id = ?1",
        [exec_id],
        |row| row.get(0),
    )?;
    let Some(run) = get_orchestration_run(conn, &run_id)? else {
        return Ok(());
    };
    if run.kind != OrchestrationRunKind::Campaign {
        return Ok(());
    }
    if to == TaskExecutionStatus::Escalated {
        set_orchestration_control_state(
            conn,
            &run_id,
            OrchestrationControlState::AwaitingHuman,
            Some("an execution escalated beyond the automatic review policy"),
            actor,
        )?;
    } else {
        let remaining: i64 = conn.query_row(
            "SELECT COUNT(*) FROM task_executions \
             WHERE orchestration_run_id = ?1 AND status = 'Escalated'",
            [run_id.as_str()],
            |row| row.get(0),
        )?;
        if remaining == 0 && run.control_state == OrchestrationControlState::AwaitingHuman {
            set_orchestration_control_state(
                conn,
                &run_id,
                OrchestrationControlState::Running,
                None,
                actor,
            )?;
        }
    }
    Ok(())
}

/// Every terminal child event leaves a bounded, deterministic obligation in the
/// principal room. It carries a typed target but zero native dispatch: joined
/// principals wake via `wait_for_peer`; KT-335 owns immediate native wakeups.
fn notify_principal_of_terminal(
    conn: &Connection,
    exec_id: &str,
    terminal: TaskExecutionStatus,
) -> Result<()> {
    let context: Option<(String, Option<String>, i64, String)> = conn
        .query_row(
            "SELECT e.parent_discussion_id, e.sub_discussion_id, t.task_number, t.title \
             FROM task_executions e \
             JOIN planning_tasks t ON t.id = e.task_id \
             WHERE e.id = ?1",
            [exec_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((parent, child, task_number, title)) = context else {
        return Ok(());
    };
    let reference = format!("KT-{task_number}");
    let Some(principal) = crate::db::discussions::get_discussion(conn, &parent)? else {
        return Ok(());
    };
    let child_link = child
        .as_deref()
        .map(|id| format!(" Sous-discussion : `{id}`."))
        .unwrap_or_default();
    let content = format!(
        "**Événement d'orchestration — {reference}**\n\n\
         L'exécution `{exec_id}` (**{title}**) est maintenant `{status}`.{child_link} \
         Relis l'état de la campagne et poursuis avec la prochaine tâche prête ; \
         n'enchaîne pas si une décision humaine est signalée.",
        status = terminal.as_str(),
    );
    let message = DiscussionMessage {
        id: format!("orch-principal-terminal:{exec_id}:{}", terminal.as_str()),
        role: MessageRole::User,
        channel: MessageChannel::Main,
        content,
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        session_tokens_at_message: None,
        recovered_partial: false,
        auth_mode: None,
        model_tier: None,
        model: None,
        cost_usd: None,
        author_pseudo: Some("Orchestrateur".to_string()),
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        lint_report: None,
        target_agent: None,
        reply_to_message_id: None,
        author_cli_ordinal: None,
    };
    let targets = [MessageTarget::discussion_agent(principal.agent)];
    crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
        conn,
        &parent,
        &message,
        &targets,
        &[],
        None,
    )?;
    Ok(())
}

/// KT-320 DoD-9 — a joined CLI worker physically leaves the origin room while
/// it works in the child. Every terminal transition returns that exact session
/// to the origin and leaves a durable trace in BOTH rooms, in the same savepoint
/// as the terminal CAS. Native agents have no movable source binding, so this is
/// deliberately CLI-only.
fn return_cli_worker_to_origin(
    conn: &Connection,
    exec_id: &str,
    terminal: TaskExecutionStatus,
) -> Result<()> {
    let context: Option<TerminalWorkerContext> = conn
        .query_row(
            "SELECT parent_discussion_id, sub_discussion_id, worker_target_kind, \
                    worker_cli_session_id, worker_agent_type \
               FROM task_executions WHERE id = ?1",
            [exec_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((origin, Some(child), Some(kind), Some(session_pk), worker_agent)) = context else {
        return Ok(());
    };
    if kind != "cli" || origin == child {
        return Ok(());
    }

    // The durable session row gives the external source identity used by the
    // binding history. A terminal execution must never steal a session that has
    // since been rebound to an unrelated room; fail the whole terminal CAS.
    let source: Option<(String, String)> = conn
        .query_row(
            "SELECT agent_type, session_id FROM discussion_sessions WHERE id = ?1",
            [session_pk],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((source_agent, source_session_id)) = source {
        let current = crate::db::disc_source::find_disc_by_source_session(
            conn,
            &source_agent,
            &source_session_id,
        )?;
        match current.as_deref() {
            Some(current) if current == child => {
                // `bind_to_source` composes in this savepoint and closes the
                // child's open history row before reopening the origin row.
                crate::db::disc_source::bind_to_source(
                    conn,
                    &origin,
                    &source_agent,
                    &source_session_id,
                )?;
            }
            Some(current) if current == origin => {}
            Some(current) => bail!(
                "terminal worker return refused: session ownership moved from child {child} to {current}"
            ),
            None => {
                // A left/expired source has no live binding to move. Its durable
                // room trace still lands below, and the terminal transition must
                // not be held hostage by an already-closed CLI session.
            }
        }
        crate::db::discussion_sessions::move_session_to_discussion(conn, session_pk, &origin)?;
    }

    let worker = worker_agent.unwrap_or_else(|| "CLI worker".to_string());
    let terminal = terminal.as_str();
    let timestamp = Utc::now();
    let message = |id: String, content: String| DiscussionMessage {
        id,
        role: MessageRole::System,
        channel: MessageChannel::Main,
        content,
        agent_type: None,
        timestamp,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        model: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        lint_report: None,
        target_agent: None,
        reply_to_message_id: None,
        session_tokens_at_message: None,
        recovered_partial: false,
        author_cli_ordinal: None,
    };
    let child_message = message(
        format!("orch-return-child:{exec_id}:{terminal}"),
        format!(
            "↩ Worker `{worker}` returned to the origin discussion `{origin}` after execution `{exec_id}` reached `{terminal}`."
        ),
    );
    let origin_message = message(
        format!("orch-return-origin:{exec_id}:{terminal}"),
        format!(
            "↩ Worker `{worker}` returned from child discussion `{child}` after execution `{exec_id}` reached `{terminal}`."
        ),
    );
    crate::db::discussions::insert_message(conn, &child, &child_message)?;
    crate::db::discussions::insert_message(conn, &origin, &origin_message)?;
    Ok(())
}

/// One step of the integration saga (ADR §4bis). Each variant names the columns it
/// pins together with the transition it commits, because the two must land in the
/// same transaction: `saga_resume_action` reads the columns back at boot and infers
/// what happened from the pair. A status that disagrees with its checkpoints is not
/// a stale row, it is an unreadable one.
#[derive(Debug, Clone, Copy)]
pub enum IntegrationStep<'a> {
    /// Approved → Integrating. Pins the parent tip the candidate is built on; this
    /// sha is the CAS anchor every later step compares the real ref against.
    CandidateAnchored { target_sha: &'a str },
    /// Stays in Integrating. Records the candidate once it exists — the reader tells
    /// "not built yet" from "built" by this column alone, so it earns its own step.
    CandidateBuilt { merge_sha: &'a str },
    /// Integrating → Validating.
    ValidationsStarted,
    /// Validating → Applying. Pins the backup ref, written before the parent moves:
    /// after this point the parent can change, and the ref is how it comes back.
    ApplyArmed { backup_ref: &'a str },
    /// Applying → Done. Records what the parent actually became.
    Integrated { integrated_sha: &'a str },
}

impl IntegrationStep<'_> {
    /// The status this step departs from. A step attempted from anywhere else is a
    /// replay or a race, never a write.
    fn expects(&self) -> TaskExecutionStatus {
        use TaskExecutionStatus::*;
        match self {
            Self::CandidateAnchored { .. } => Approved,
            Self::CandidateBuilt { .. } => Integrating,
            Self::ValidationsStarted => Integrating,
            Self::ApplyArmed { .. } => Validating,
            Self::Integrated { .. } => Applying,
        }
    }

    /// The status this step commits, or `None` when it only pins a column.
    fn advances_to(&self) -> Option<TaskExecutionStatus> {
        use TaskExecutionStatus::*;
        match self {
            Self::CandidateAnchored { .. } => Some(Integrating),
            Self::CandidateBuilt { .. } => None,
            Self::ValidationsStarted => Some(Validating),
            Self::ApplyArmed { .. } => Some(Applying),
            Self::Integrated { .. } => Some(Done),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum IntegrationCheckpointOutcome {
    /// The columns and the transition landed together; `status` is where the
    /// execution now stands.
    Committed { status: TaskExecutionStatus },
    /// The execution is not in the status this step departs from. A replay of an
    /// already-committed step lands here instead of writing a second time — the
    /// status gate IS the idempotency guard, as everywhere else in this module.
    NotInStep { status: TaskExecutionStatus },
    /// The execution was ready to finish but its plan task was no longer
    /// `InProgress`. The whole checkpoint (including integrated_sha) rolled
    /// back so the two durable aggregates cannot disagree.
    TaskNotCompletable,
    /// The execution vanished beneath the checkpoint.
    ExecutionRaced,
}

/// Commit one integration-saga step: pin its checkpoint columns and its transition
/// in a single transaction (ADR §4bis, KT-320 DoD-7). The four columns declared by
/// migration 127 come alive here — until this, the boot-time reconciliation reasoned
/// over columns nobody ever wrote, so every resume decision was taken on NULLs.
pub fn commit_integration_checkpoint(
    conn: &Connection,
    exec_id: &str,
    step: IntegrationStep<'_>,
    actor: &OrchestrationActor,
) -> Result<IntegrationCheckpointOutcome> {
    let tx = conn.unchecked_transaction()?;
    let current: Option<String> = tx
        .query_row(
            "SELECT status FROM task_executions WHERE id = ?1",
            params![exec_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(current) = current else {
        return Ok(IntegrationCheckpointOutcome::ExecutionRaced);
    };
    let status = TaskExecutionStatus::from_str(&current)?;
    if status != step.expects() {
        return Ok(IntegrationCheckpointOutcome::NotInStep { status });
    }

    // Pin the columns FIRST: a crash between the write and the transition leaves a
    // checkpoint ahead of its status, which the reader treats as "not there yet" and
    // simply redoes. The reverse order would leave a status claiming work whose
    // anchor was never recorded — the one shape `saga_resume_action` cannot recover.
    let now = Utc::now().to_rfc3339();
    match step {
        IntegrationStep::CandidateAnchored { target_sha } => {
            tx.execute(
                "UPDATE task_executions
                    SET candidate_target_sha = ?2, candidate_merge_sha = NULL,
                        integrated_sha = NULL, updated_at = ?3
                  WHERE id = ?1",
                params![exec_id, target_sha, now],
            )?;
        }
        IntegrationStep::CandidateBuilt { merge_sha } => {
            tx.execute(
                "UPDATE task_executions SET candidate_merge_sha = ?2, updated_at = ?3 WHERE id = ?1",
                params![exec_id, merge_sha, now],
            )?;
        }
        IntegrationStep::ValidationsStarted => {}
        IntegrationStep::ApplyArmed { backup_ref } => {
            tx.execute(
                "UPDATE task_executions SET backup_ref = ?2, updated_at = ?3 WHERE id = ?1",
                params![exec_id, backup_ref, now],
            )?;
        }
        IntegrationStep::Integrated { integrated_sha } => {
            tx.execute(
                "UPDATE task_executions SET integrated_sha = ?2, updated_at = ?3 WHERE id = ?1",
                params![exec_id, integrated_sha, now],
            )?;
            let task_id: String = tx.query_row(
                "SELECT task_id FROM task_executions WHERE id = ?1",
                [exec_id],
                |row| row.get(0),
            )?;
            if !crate::db::planning::mark_task_done_within_tx(&tx, &task_id, actor)? {
                return Ok(IntegrationCheckpointOutcome::TaskNotCompletable);
            }
        }
    }

    let landed = match step.advances_to() {
        Some(to) => {
            if !transition_execution(&tx, exec_id, to, actor, serde_json::json!({}))? {
                return Ok(IntegrationCheckpointOutcome::ExecutionRaced);
            }
            to
        }
        None => {
            // A column-only step still deserves a journal line: without it the
            // candidate appears in the row with nothing saying when it was built.
            record_execution_event(
                &tx,
                exec_id,
                "integration_checkpoint",
                Some(status),
                Some(status),
                actor,
                serde_json::json!({}),
            )?;
            status
        }
    };
    tx.commit()?;
    Ok(IntegrationCheckpointOutcome::Committed { status: landed })
}

// ─── Events ──────────────────────────────────────────────────────────────────

/// Journal one transition/event with an attributed actor. Autonomous transitions
/// use `PlanningActorKind::Backend`/`System`; a worker/principal action uses
/// `Agent` (or `Human`), matching the non-spoofable identities planning uses.
pub fn record_execution_event(
    conn: &Connection,
    exec_id: &str,
    action: &str,
    from: Option<TaskExecutionStatus>,
    to: Option<TaskExecutionStatus>,
    actor: &OrchestrationActor,
    changes: serde_json::Value,
) -> Result<()> {
    conn.execute(
        "INSERT INTO task_execution_events \
         (id, task_execution_id, action, from_status, to_status, actor_kind, actor_id, \
          actor_session_id, changes_json, source_message_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            Uuid::new_v4().to_string(),
            exec_id,
            action,
            from.map(|s| s.as_str()),
            to.map(|s| s.as_str()),
            actor.kind.as_str(),
            actor.id,
            actor.session_id,
            serde_json::to_string(&changes)?,
            actor.source_message_id,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn list_execution_events(conn: &Connection, exec_id: &str) -> Result<Vec<TaskExecutionEvent>> {
    let mut stmt = conn.prepare(
        "SELECT id, task_execution_id, action, from_status, to_status, actor_kind, \
                actor_id, actor_session_id, changes_json, source_message_id, created_at \
         FROM task_execution_events WHERE task_execution_id = ?1 \
         ORDER BY created_at ASC, rowid ASC",
    )?;
    let events = stmt
        .query_map(params![exec_id], row_to_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(events)
}

// ─── Validation runs ─────────────────────────────────────────────────────────

/// Record a validation run against the exact candidate commit (ADR §6). The
/// `exit_code` is the verdict; the caller derives pass/fail via `passed()`.
pub fn record_validation_run(
    conn: &Connection,
    exec_id: &str,
    candidate_merge_sha: Option<&str>,
    spec: &ValidationSpec,
    exit_code: Option<i32>,
    duration_ms: Option<i64>,
    output: Option<&str>,
) -> Result<TaskExecutionValidationRun> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO task_execution_validation_runs \
         (id, task_execution_id, candidate_merge_sha, command, exit_code, duration_ms, \
          output, quick_exec_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            exec_id,
            candidate_merge_sha,
            spec.command,
            exit_code.map(|v| v as i64),
            duration_ms,
            output,
            spec.quick_exec_id,
            now,
        ],
    )?;
    Ok(TaskExecutionValidationRun {
        id,
        task_execution_id: exec_id.to_string(),
        candidate_merge_sha: candidate_merge_sha.map(str::to_string),
        command: spec.command.clone(),
        exit_code,
        duration_ms,
        output: output.map(str::to_string),
        quick_exec_id: spec.quick_exec_id.clone(),
        created_at: parse_dt(now),
    })
}

/// A successful validation is immutable evidence for one exact candidate. A
/// recovery may restart between two commands, so it must not append a second
/// row for a command that already completed for that same candidate.
pub fn has_passing_validation_run(
    conn: &Connection,
    exec_id: &str,
    candidate_merge_sha: &str,
    spec: &ValidationSpec,
) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_execution_validation_runs \
         WHERE task_execution_id = ?1 AND candidate_merge_sha = ?2 \
           AND command = ?3 AND quick_exec_id IS ?4 AND exit_code = 0)",
        params![
            exec_id,
            candidate_merge_sha,
            spec.command,
            spec.quick_exec_id
        ],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn list_validation_runs(
    conn: &Connection,
    exec_id: &str,
) -> Result<Vec<TaskExecutionValidationRun>> {
    let mut stmt = conn.prepare(
        "SELECT id, task_execution_id, candidate_merge_sha, command, exit_code, \
                duration_ms, output, quick_exec_id, created_at \
         FROM task_execution_validation_runs WHERE task_execution_id = ?1 \
         ORDER BY created_at ASC, rowid ASC",
    )?;
    let runs = stmt
        .query_map(params![exec_id], row_to_validation)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(runs)
}

// ─── Lineage ─────────────────────────────────────────────────────────────────

/// Resolve the whole lineage chain in one query (DoD-4): OrchestrationRun →
/// TaskExecution → task → parent/sub-discussion → workspace, without rebuilding
/// it from chat messages.
pub fn get_execution_lineage(
    conn: &Connection,
    exec_id: &str,
) -> Result<Option<TaskExecutionLineage>> {
    let sql = format!(
        "SELECT {cols}, r.kind, t.task_number, t.title, dw.canonical_path \
         FROM task_executions te \
         JOIN orchestration_runs r ON r.id = te.orchestration_run_id \
         JOIN planning_tasks t ON t.id = te.task_id \
         LEFT JOIN discussion_workspaces dw ON dw.id = te.workspace_id \
         WHERE te.id = ?1",
        cols = EXEC_COLS
            .split(", ")
            .map(|c| format!("te.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let lineage = conn
        .query_row(&sql, params![exec_id], |row| {
            let execution = row_to_execution(row)?;
            // EXEC_COLS spans indices 0..=34 (35 columns); the JOIN columns follow.
            let kind: String = row.get(35)?;
            let task_number: i64 = row.get(36)?;
            let task_title: String = row.get(37)?;
            let workspace_canonical_path: Option<String> = row.get(38)?;
            Ok(TaskExecutionLineage {
                parent_discussion_id: execution.parent_discussion_id.clone(),
                sub_discussion_id: execution.sub_discussion_id.clone(),
                orchestration_run_kind: OrchestrationRunKind::from_str(&kind).unwrap_or_default(),
                task_reference: format!("KT-{task_number}"),
                task_title,
                workspace_canonical_path,
                execution,
            })
        })
        .optional()
        .map_err(anyhow::Error::from)?;
    if let Some(lineage) = lineage.as_ref() {
        validate_worker_connection(conn, &lineage.execution)?;
    }
    Ok(lineage)
}

/// Compact execution-to-discussion edges for sidebar grouping. The relation is
/// sourced from `task_executions` itself; `discussions.workflow_run_id` remains
/// reserved for its FK to the distinct workflow engine aggregate.
pub fn list_execution_discussion_links(
    conn: &Connection,
) -> Result<Vec<crate::models::ExecutionDiscussionLink>> {
    let mut statement = conn.prepare(
        "SELECT te.id, te.orchestration_run_id, te.task_id, t.task_number, t.title, \
                te.parent_discussion_id, te.sub_discussion_id, te.status \
           FROM task_executions te \
           JOIN planning_tasks t ON t.id = te.task_id \
          WHERE te.sub_discussion_id IS NOT NULL \
          ORDER BY te.created_at ASC, te.id ASC",
    )?;
    let rows = statement
        .query_map([], |row| {
            let status: String = row.get(7)?;
            Ok(crate::models::ExecutionDiscussionLink {
                execution_id: row.get(0)?,
                orchestration_run_id: row.get(1)?,
                task_id: row.get(2)?,
                task_reference: format!("KT-{}", row.get::<_, i64>(3)?),
                task_title: row.get(4)?,
                parent_discussion_id: row.get(5)?,
                sub_discussion_id: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                status: TaskExecutionStatus::from_str(&status)
                    .unwrap_or(TaskExecutionStatus::Interrupted),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ─── Recovery / resilience (KT-322) ────────────────────────────────────────

fn recovery_row(row: &Row) -> rusqlite::Result<TaskExecutionRecovery> {
    let action: String = row.get(1)?;
    Ok(TaskExecutionRecovery {
        task_execution_id: row.get(0)?,
        recovery_action: ExecutionRecoveryAction::from_str(&action).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, error.into())
        })?,
        recovery_reason: row.get(2)?,
        last_activity_at: parse_dt(row.get(3)?),
        total_deadline_at: parse_opt_dt(row.get(4)?),
        activity_deadline_at: parse_opt_dt(row.get(5)?),
        review_deadline_at: parse_opt_dt(row.get(6)?),
        human_wait_started_at: parse_opt_dt(row.get(7)?),
        assignment_generation: row.get::<_, i64>(8)?.max(0) as u32,
        watchdog_redispatches: row.get::<_, i64>(9)?.max(0) as u32,
        pending: row.get::<_, i64>(10)? != 0,
        updated_at: parse_dt(row.get(11)?),
    })
}

pub fn get_resilience_policy(
    conn: &Connection,
    run_id: &str,
) -> Result<OrchestrationResiliencePolicy> {
    conn.query_row(
        "SELECT activity_timeout_secs, review_timeout_secs, human_wait_timeout_secs, \
                cancellation_cleanup_policy \
         FROM orchestration_run_resilience_policy WHERE orchestration_run_id = ?1",
        [run_id],
        |row| {
            let cleanup: String = row.get(3)?;
            let cancellation_cleanup_policy = match cleanup.as_str() {
                "preserve" => CancellationCleanupPolicy::Preserve,
                "remove_if_clean" => CancellationCleanupPolicy::RemoveIfClean,
                other => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        format!("unknown cancellation cleanup policy: {other}").into(),
                    ))
                }
            };
            Ok(OrchestrationResiliencePolicy {
                activity_timeout_secs: row.get::<_, Option<i64>>(0)?.map(|value| value as u32),
                review_timeout_secs: row.get::<_, Option<i64>>(1)?.map(|value| value as u32),
                human_wait_timeout_secs: row.get::<_, Option<i64>>(2)?.map(|value| value as u32),
                cancellation_cleanup_policy,
            })
        },
    )
    .optional()
    .map(|value| value.unwrap_or_default())
    .map_err(Into::into)
}

pub fn set_resilience_policy(
    conn: &Connection,
    run_id: &str,
    policy: &OrchestrationResiliencePolicy,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO orchestration_run_resilience_policy (\
             orchestration_run_id, activity_timeout_secs, review_timeout_secs, \
             human_wait_timeout_secs, cancellation_cleanup_policy, created_at, updated_at\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6) \
         ON CONFLICT(orchestration_run_id) DO UPDATE SET \
             activity_timeout_secs = excluded.activity_timeout_secs, \
             review_timeout_secs = excluded.review_timeout_secs, \
             human_wait_timeout_secs = excluded.human_wait_timeout_secs, \
             cancellation_cleanup_policy = excluded.cancellation_cleanup_policy, \
             updated_at = excluded.updated_at",
        params![
            run_id,
            policy.activity_timeout_secs,
            policy.review_timeout_secs,
            policy.human_wait_timeout_secs,
            policy.cancellation_cleanup_policy.as_str(),
            now,
        ],
    )?;
    Ok(())
}

pub fn get_execution_recovery(
    conn: &Connection,
    exec_id: &str,
) -> Result<Option<TaskExecutionRecovery>> {
    conn.query_row(
        "SELECT task_execution_id, recovery_action, recovery_reason, last_activity_at, \
                total_deadline_at, activity_deadline_at, review_deadline_at, \
                human_wait_started_at, assignment_generation, watchdog_redispatches, \
                pending, updated_at \
         FROM task_execution_recovery WHERE task_execution_id = ?1",
        [exec_id],
        recovery_row,
    )
    .optional()
    .map_err(Into::into)
}

/// Persist the reconciler's decision and all four independent clocks. The
/// business execution remains `Interrupted` until a guarded resume consumes it.
pub fn set_execution_recovery(
    conn: &Connection,
    execution: &TaskExecution,
    run: &OrchestrationRun,
    action: ExecutionRecoveryAction,
    reason: &str,
) -> Result<TaskExecutionRecovery> {
    let now = Utc::now();
    let policy = get_resilience_policy(conn, &run.id)?;
    let total_deadline = run
        .timeout_secs
        .map(|seconds| execution.created_at + chrono::Duration::seconds(i64::from(seconds)));
    let activity_deadline = policy
        .activity_timeout_secs
        .map(|seconds| now + chrono::Duration::seconds(i64::from(seconds)));
    let review_deadline =
        if execution.interrupted_from_status == Some(TaskExecutionStatus::AwaitingReview) {
            policy
                .review_timeout_secs
                .map(|seconds| now + chrono::Duration::seconds(i64::from(seconds)))
        } else {
            None
        };
    let human_wait_started = if matches!(
        action,
        ExecutionRecoveryAction::AwaitHuman
            | ExecutionRecoveryAction::BlockMissingWorkspace
            | ExecutionRecoveryAction::BlockMissingDiscussion
            | ExecutionRecoveryAction::BlockAgentUnavailable
            | ExecutionRecoveryAction::BlockDirtyTarget
    ) {
        Some(now)
    } else {
        None
    };
    conn.execute(
        "INSERT INTO task_execution_recovery (\
             task_execution_id, recovery_action, recovery_reason, last_activity_at, \
             total_deadline_at, activity_deadline_at, review_deadline_at, \
             human_wait_started_at, assignment_generation, pending, updated_at\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 1, ?4) \
         ON CONFLICT(task_execution_id) DO UPDATE SET \
             recovery_action = excluded.recovery_action, recovery_reason = excluded.recovery_reason, \
             total_deadline_at = excluded.total_deadline_at, \
             activity_deadline_at = excluded.activity_deadline_at, \
             review_deadline_at = excluded.review_deadline_at, \
             human_wait_started_at = excluded.human_wait_started_at, \
             pending = 1, \
             updated_at = excluded.updated_at",
        params![
            execution.id,
            action.as_str(),
            reason,
            now.to_rfc3339(),
            total_deadline.map(|value| value.to_rfc3339()),
            activity_deadline.map(|value| value.to_rfc3339()),
            review_deadline.map(|value| value.to_rfc3339()),
            human_wait_started.map(|value| value.to_rfc3339()),
        ],
    )?;
    record_reconciliation_event(
        conn,
        "execution",
        &execution.id,
        "classified",
        serde_json::json!({ "action": action.as_str(), "reason": reason }),
    )?;
    get_execution_recovery(conn, &execution.id)?
        .ok_or_else(|| anyhow::anyhow!("recovery decision vanished after upsert"))
}

pub fn touch_execution_activity(conn: &Connection, exec_id: &str) -> Result<()> {
    let now = Utc::now();
    type RecoveryClocks = (
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        String,
        String,
    );
    let clocks: Option<RecoveryClocks> = conn
        .query_row(
            "SELECT p.activity_timeout_secs, p.review_timeout_secs, \
                    p.human_wait_timeout_secs, r.timeout_secs, e.created_at, e.status \
             FROM task_executions e \
             JOIN orchestration_runs r ON r.id = e.orchestration_run_id \
             LEFT JOIN orchestration_run_resilience_policy p \
               ON p.orchestration_run_id = e.orchestration_run_id \
             WHERE e.id = ?1",
            [exec_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((activity_secs, review_secs, _human_secs, total_secs, created_at, status)) = clocks
    else {
        return Ok(());
    };
    let deadline = activity_secs.map(|seconds| now + chrono::Duration::seconds(seconds));
    let review_deadline = if status == TaskExecutionStatus::AwaitingReview.as_str() {
        review_secs.map(|seconds| now + chrono::Duration::seconds(seconds))
    } else {
        None
    };
    let human_started = if status == TaskExecutionStatus::Escalated.as_str() {
        Some(now)
    } else {
        None
    };
    let total_deadline =
        total_secs.map(|seconds| parse_dt(created_at.clone()) + chrono::Duration::seconds(seconds));
    conn.execute(
        "INSERT OR IGNORE INTO task_execution_recovery (\
             task_execution_id, recovery_action, recovery_reason, last_activity_at, \
             total_deadline_at, activity_deadline_at, review_deadline_at, \
             human_wait_started_at, assignment_generation, pending, updated_at\
         ) VALUES (?1, 'resume_worker', 'runtime clocks', ?2, ?3, ?4, ?5, ?6, 0, 0, ?2)",
        params![
            exec_id,
            now.to_rfc3339(),
            total_deadline.map(|value| value.to_rfc3339()),
            deadline.map(|value| value.to_rfc3339()),
            review_deadline.map(|value| value.to_rfc3339()),
            human_started.map(|value| value.to_rfc3339()),
        ],
    )?;
    conn.execute(
        "UPDATE task_execution_recovery SET last_activity_at = ?2, \
                activity_deadline_at = ?3, review_deadline_at = ?4, \
                human_wait_started_at = ?5, updated_at = ?2 \
         WHERE task_execution_id = ?1",
        params![
            exec_id,
            now.to_rfc3339(),
            deadline.map(|value| value.to_rfc3339()),
            review_deadline.map(|value| value.to_rfc3339()),
            human_started.map(|value| value.to_rfc3339()),
        ],
    )?;
    Ok(())
}

/// Expired clocks with their precise semantic kind. Human waits are reported,
/// never auto-cancelled by this query; the watchdog decides policy explicitly.
pub fn expired_execution_timeouts(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<Vec<(String, ExecutionTimeoutKind)>> {
    let mut stmt = conn.prepare(
        "SELECT r.task_execution_id, e.status, r.total_deadline_at, \
                r.activity_deadline_at, r.review_deadline_at, r.human_wait_started_at, \
                p.human_wait_timeout_secs \
         FROM task_execution_recovery r \
         JOIN task_executions e ON e.id = r.task_execution_id \
         LEFT JOIN orchestration_run_resilience_policy p \
           ON p.orchestration_run_id = e.orchestration_run_id \
         WHERE e.status NOT IN ('Done', 'Failed', 'Cancelled')",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<i64>>(6)?,
        ))
    })?;
    let mut expired = Vec::new();
    for row in rows {
        let (id, status, total, activity, review, human_since, human_secs) = row?;
        let parse = |value: Option<String>| value.map(parse_dt);
        if parse(total).is_some_and(|deadline| deadline <= now) {
            expired.push((id, ExecutionTimeoutKind::TotalDuration));
        } else if status == TaskExecutionStatus::AwaitingReview.as_str()
            && parse(review).is_some_and(|deadline| deadline <= now)
        {
            expired.push((id, ExecutionTimeoutKind::ReviewWait));
        } else if let (Some(since), Some(seconds)) = (parse(human_since), human_secs) {
            if since + chrono::Duration::seconds(seconds) <= now {
                expired.push((id, ExecutionTimeoutKind::HumanWait));
            }
        } else if parse(activity).is_some_and(|deadline| deadline <= now) {
            expired.push((id, ExecutionTimeoutKind::Activity));
        }
    }
    Ok(expired)
}

pub fn apply_execution_timeout(
    conn: &Connection,
    exec_id: &str,
    kind: ExecutionTimeoutKind,
) -> Result<bool> {
    in_savepoint(conn, |conn| {
        let execution = get_task_execution(conn, exec_id)?
            .ok_or_else(|| anyhow::anyhow!("unknown task execution"))?;
        if execution.status.is_terminal() {
            return Ok(false);
        }
        let label = match kind {
            ExecutionTimeoutKind::Activity => "activity_timeout",
            ExecutionTimeoutKind::TotalDuration => "total_duration_timeout",
            ExecutionTimeoutKind::ReviewWait => "review_wait_timeout",
            ExecutionTimeoutKind::HumanWait => "human_wait_timeout",
        };
        if kind == ExecutionTimeoutKind::Activity {
            let watchdog_redispatches: u32 = conn
                .query_row(
                    "SELECT watchdog_redispatches FROM task_execution_recovery \
                     WHERE task_execution_id = ?1",
                    [exec_id],
                    |row| Ok(row.get::<_, i64>(0)?.max(0) as u32),
                )
                .optional()?
                .unwrap_or(0);
            if watchdog_redispatches == 0 {
                if let Some(dispatch_id) = execution.dispatch_job_id.as_deref() {
                    if crate::db::agent_dispatch::apply_watchdog_stall(conn, dispatch_id)?
                        == crate::db::agent_dispatch::WatchdogTransition::Redispatched
                    {
                        let activity_timeout_secs: Option<i64> = conn
                            .query_row(
                                "SELECT policy.activity_timeout_secs \
                                 FROM orchestration_run_resilience_policy policy \
                                 WHERE policy.orchestration_run_id = ?1",
                                [&execution.orchestration_run_id],
                                |row| row.get(0),
                            )
                            .optional()?
                            .flatten();
                        let now = Utc::now();
                        let next_deadline = activity_timeout_secs
                            .map(|seconds| now + chrono::Duration::seconds(seconds));
                        conn.execute(
                            "UPDATE task_execution_recovery \
                             SET watchdog_redispatches = 1, recovery_action = 'resume_worker', \
                                 recovery_reason = 'watchdog_stall_redispatch', \
                                 last_activity_at = ?2, activity_deadline_at = ?3, \
                                 pending = 1, updated_at = ?2 WHERE task_execution_id = ?1",
                            params![
                                exec_id,
                                now.to_rfc3339(),
                                next_deadline.map(|value| value.to_rfc3339()),
                            ],
                        )?;
                        record_reconciliation_event(
                            conn,
                            "execution",
                            exec_id,
                            "watchdog_stall_redispatch",
                            serde_json::json!({"dispatch_job_id": dispatch_id}),
                        )?;
                        return Ok(true);
                    }
                }
            } else if let Some(dispatch_id) = execution.dispatch_job_id.as_deref() {
                // The only retry budget was already consumed. Settle the
                // dispatch cause before escalating the business execution so
                // attention/presence readers see the same honest state.
                let _ = crate::db::agent_dispatch::apply_watchdog_stall(conn, dispatch_id)?;
            }
        }
        record_reconciliation_event(
            conn,
            "execution",
            exec_id,
            label,
            serde_json::json!({ "status": execution.status.as_str() }),
        )?;
        if execution.status != TaskExecutionStatus::Escalated {
            transition_execution(
                conn,
                exec_id,
                TaskExecutionStatus::Escalated,
                &OrchestrationActor {
                    kind: PlanningActorKind::System,
                    id: Some("orchestration-watchdog".into()),
                    session_id: None,
                    source_message_id: None,
                },
                serde_json::json!({ "timeout_kind": label }),
            )?;
        }
        conn.execute(
            "UPDATE task_execution_recovery SET activity_deadline_at = NULL, \
                    review_deadline_at = NULL, total_deadline_at = NULL, \
                    human_wait_started_at = NULL, recovery_action = 'await_human', \
                    recovery_reason = ?2, pending = 0, updated_at = ?3 \
             WHERE task_execution_id = ?1",
            params![exec_id, label, Utc::now().to_rfc3339()],
        )?;
        Ok(true)
    })
}

pub fn record_reconciliation_event(
    conn: &Connection,
    subject_kind: &str,
    subject_id: &str,
    action: &str,
    details: serde_json::Value,
) -> Result<()> {
    conn.execute(
        "INSERT INTO orchestration_reconciliation_events \
         (id, subject_kind, subject_id, action, details_json, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            Uuid::new_v4().to_string(),
            subject_kind,
            subject_id,
            action,
            serde_json::to_string(&details)?,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn clear_execution_recovery(conn: &Connection, exec_id: &str, applied: &str) -> Result<()> {
    record_reconciliation_event(
        conn,
        "execution",
        exec_id,
        "recovery_applied",
        serde_json::json!({ "action": applied }),
    )?;
    conn.execute(
        "UPDATE task_execution_recovery SET pending = 0, updated_at = ?2 \
         WHERE task_execution_id = ?1",
        params![exec_id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Resume an interrupted integration by rebuilding its candidate against an
/// observed real target tip. The interrupted origin guard is honoured, and all
/// stale candidate/apply checkpoints are cleared atomically before Git runs.
pub fn resume_rebuild_candidate(
    conn: &Connection,
    exec_id: &str,
    target_sha: &str,
    actor: &OrchestrationActor,
) -> Result<bool> {
    in_savepoint(conn, |conn| {
        if !transition_execution(
            conn,
            exec_id,
            TaskExecutionStatus::Integrating,
            actor,
            serde_json::json!({ "recovery": "rebuild_candidate", "target_sha": target_sha }),
        )? {
            return Ok(false);
        }
        conn.execute(
            "UPDATE task_executions SET candidate_target_sha = ?2, \
                    candidate_merge_sha = NULL, integrated_sha = NULL, backup_ref = NULL, \
                    updated_at = ?3 WHERE id = ?1",
            params![exec_id, target_sha, Utc::now().to_rfc3339()],
        )?;
        record_execution_event(
            conn,
            exec_id,
            "integration_reanchored",
            Some(TaskExecutionStatus::Integrating),
            Some(TaskExecutionStatus::Integrating),
            actor,
            serde_json::json!({ "target_sha": target_sha }),
        )?;
        Ok(true)
    })
}

/// Replace only the worker coordinates. The sub-discussion, workspace,
/// deliveries, review history and every Git SHA remain untouched.
pub fn reassign_execution_worker(
    conn: &Connection,
    exec_id: &str,
    selection: &CampaignWorkerSelection,
    reason: &str,
    actor: &OrchestrationActor,
) -> Result<TaskExecution> {
    in_savepoint(conn, |conn| {
        let execution = get_task_execution(conn, exec_id)?
            .ok_or_else(|| anyhow::anyhow!("unknown task execution"))?;
        if execution.status.is_terminal() {
            bail!("a terminal execution cannot be reassigned");
        }
        if execution.status == TaskExecutionStatus::Escalated {
            let recovery = get_execution_recovery(conn, exec_id)?;
            let structurally_reassignable = recovery.as_ref().is_none_or(|recovery| {
                matches!(
                    recovery.recovery_action,
                    ExecutionRecoveryAction::ResumeWorker
                        | ExecutionRecoveryAction::AwaitReview
                        | ExecutionRecoveryAction::AwaitHuman
                        | ExecutionRecoveryAction::BlockAgentUnavailable
                )
            });
            if !structurally_reassignable {
                let action = recovery
                    .as_ref()
                    .map(|recovery| recovery.recovery_action.as_str())
                    .unwrap_or("unknown");
                bail!(
                    "execution {exec_id} is Escalated by recovery action `{action}`; repair that \
                     infrastructure/integration checkpoint before reassigning a worker"
                );
            }
        }
        let resumable_worker_state = matches!(
            execution.status,
            TaskExecutionStatus::Working
                | TaskExecutionStatus::ChangesRequested
                | TaskExecutionStatus::Escalated
        ) || (execution.status == TaskExecutionStatus::Interrupted
            && matches!(
                execution.interrupted_from_status,
                Some(TaskExecutionStatus::Working | TaskExecutionStatus::ChangesRequested)
            ));
        if !resumable_worker_state {
            bail!(
                "execution {} is {}, not a resumable worker state",
                exec_id,
                execution.status.as_str()
            );
        }
        let run = get_orchestration_run(conn, &execution.orchestration_run_id)?
            .ok_or_else(|| anyhow::anyhow!("orchestration run vanished"))?;
        resolve_campaign_worker(conn, &run, Some(selection))?;
        if let Some(dispatch_id) = execution.dispatch_job_id.as_deref() {
            crate::db::agent_dispatch::cancel_for_discussion_by_id(
                conn,
                execution.sub_discussion_id.as_deref().unwrap_or(""),
                dispatch_id,
            )?;
        }
        crate::db::worker_offers::cancel_live_offers_for_execution(conn, exec_id)?;
        let now = Utc::now().to_rfc3339();
        // Repair historical rework rows that still point at an already-reviewed attempt.
        // Pre-KT-385 request_changes forgot to advance it; pre-KT-497 validation/integration
        // send-backs did the same after an approve. In both cases attempt N already owns its
        // delivery/review/message audit identities, so the next delivery must use N+1.
        // Correct rows already point at an unreviewed attempt, making this idempotent.
        let repaired_rework_attempt =
            crate::db::worker_reviews::get_review(conn, exec_id, execution.attempt_no)?.is_some();
        if repaired_rework_attempt {
            conn.execute(
                "UPDATE task_executions SET attempt_no = attempt_no + 1, updated_at = ?2 \
                 WHERE id = ?1",
                params![exec_id, now],
            )?;
            record_execution_event(
                conn,
                exec_id,
                "rework_attempt_repaired",
                Some(execution.status),
                Some(execution.status),
                actor,
                serde_json::json!({
                    "from_attempt": execution.attempt_no,
                    "to_attempt": execution.attempt_no + 1,
                    "reason": "reviewed_attempt_was_not_advanced",
                }),
            )?;
        }
        conn.execute(
            "INSERT INTO task_execution_recovery (\
                 task_execution_id, recovery_action, recovery_reason, last_activity_at, \
                 assignment_generation, updated_at\
             ) VALUES (?1, 'resume_worker', ?2, ?3, 1, ?3) \
             ON CONFLICT(task_execution_id) DO UPDATE SET \
                 recovery_action = 'resume_worker', recovery_reason = excluded.recovery_reason, \
                 assignment_generation = assignment_generation + 1, pending = 1, \
                 updated_at = excluded.updated_at",
            params![exec_id, reason, now],
        )?;
        let generation: i64 = conn.query_row(
            "SELECT assignment_generation FROM task_execution_recovery WHERE task_execution_id = ?1",
            [exec_id],
            |row| row.get(0),
        )?;
        conn.execute(
            "UPDATE task_executions SET worker_target_kind = ?2, worker_cli_session_id = ?3, \
                    worker_agent_type = ?4, worker_model = ?5, worker_model_tier = ?6, \
                    worker_profile_id = ?7, dispatch_job_id = NULL, updated_at = ?8, \
                    worker_connection_id = ?9 \
             WHERE id = ?1",
            params![
                exec_id,
                target_kind_to_db(Some(selection.target.kind)),
                selection.target.cli_session_id,
                agent_type_to_db(&selection.target.agent_type),
                selection.model,
                selection.target.tier.as_ref().map(model_tier_to_db),
                selection.profile_id,
                now,
                selection.target.connection_id,
            ],
        )?;
        conn.execute(
            "INSERT INTO task_execution_assignment_events (\
                 id, task_execution_id, generation, worker_target_kind, worker_cli_session_id, \
                 worker_agent_type, worker_model, worker_model_tier, worker_profile_id, reason, \
                 actor_kind, actor_id, source_message_id, created_at\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                Uuid::new_v4().to_string(),
                exec_id,
                generation,
                target_kind_to_db(Some(selection.target.kind)),
                selection.target.cli_session_id,
                agent_type_to_db(&selection.target.agent_type),
                selection.model,
                selection.target.tier.as_ref().map(model_tier_to_db),
                selection.profile_id,
                reason,
                actor.kind.as_str(),
                actor.id,
                actor.source_message_id,
                now,
            ],
        )?;
        record_execution_event(
            conn,
            exec_id,
            "worker_reassigned",
            Some(execution.status),
            Some(execution.status),
            actor,
            serde_json::json!({
                "generation": generation,
                "provider": agent_type_to_db(&selection.target.agent_type),
                "identity_kind": target_kind_to_db(Some(selection.target.kind)),
                "cli_session_id": selection.target.cli_session_id,
                "reason": reason,
            }),
        )?;
        get_task_execution(conn, exec_id)?
            .ok_or_else(|| anyhow::anyhow!("execution vanished after reassignment"))
    })
}

pub fn cancel_execution_tree(
    conn: &Connection,
    exec_id: &str,
    reason: &str,
    actor: &OrchestrationActor,
) -> Result<TaskExecution> {
    in_savepoint(conn, |conn| {
        let execution = get_task_execution(conn, exec_id)?
            .ok_or_else(|| anyhow::anyhow!("unknown task execution"))?;
        if execution.status.is_terminal() {
            return Ok(execution);
        }
        if let (Some(discussion_id), Some(dispatch_id)) = (
            execution.sub_discussion_id.as_deref(),
            execution.dispatch_job_id.as_deref(),
        ) {
            crate::db::agent_dispatch::cancel_for_discussion_by_id(
                conn,
                discussion_id,
                dispatch_id,
            )?;
        }
        crate::db::worker_offers::cancel_live_offers_for_execution(conn, exec_id)?;
        transition_execution(
            conn,
            exec_id,
            TaskExecutionStatus::Cancelled,
            actor,
            serde_json::json!({ "reason": reason, "cascade": true }),
        )?;
        conn.execute(
            "UPDATE task_executions SET outcome_reason = ?2 WHERE id = ?1",
            params![exec_id, reason],
        )?;
        crate::db::planning::restore_task_todo_after_cancellation(conn, &execution.task_id, actor)?;
        record_reconciliation_event(
            conn,
            "execution",
            exec_id,
            "cancelled",
            serde_json::json!({ "reason": reason }),
        )?;
        get_task_execution(conn, exec_id)?
            .ok_or_else(|| anyhow::anyhow!("execution vanished after cancellation"))
    })
}

// ─── Boot reconcile (persistence half; KT-322 wires the tree resume) ─────────

/// Non-terminal statuses — the set the boot reconcile flips to `Interrupted`.
const NON_TERMINAL_STATUSES: &[&str] = &[
    "Pending",
    "Provisioning",
    "Blocked",
    "Working",
    "AwaitingReview",
    "Approved",
    "ChangesRequested",
    "Integrating",
    "Validating",
    "Applying",
    "Escalated",
];

/// Flip every in-flight TaskExecution to `Interrupted` at boot, journaling each
/// move and preserving its origin, returning the ids that moved. At boot there is
/// no in-process driver, so every non-terminal (non-`Interrupted`) row is a zombie.
///
/// Unlike a bulk UPDATE, this reconcile is **non-destructive and journaled**
/// (DoD-3): each row is moved through the guarded `transition_execution`, so it
/// (a) records `interrupted_from_status` — the exact pre-interruption state the
/// §4bis resume needs — and (b) writes a `System`-attributed event `from →
/// Interrupted`. A reconcile that erased the origin or skipped the journal would
/// make the resume non-deterministic and the audit trail lie. Wiring this into
/// the boot sequence and the guarded per-state resume (using `saga_resume_action`
/// against real Git refs) is KT-322; KT-317 ships the primitive + its tests.
pub fn reconcile_stale_task_executions(conn: &Connection) -> Result<Vec<String>> {
    let placeholders = NON_TERMINAL_STATUSES
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let ids: Vec<String> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT id FROM task_executions \
             WHERE status IN ({placeholders}) OR status = 'Interrupted'"
        ))?;
        // Bind to a local so the MappedRows temporary drops before `stmt`.
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(NON_TERMINAL_STATUSES.iter()),
                |row| row.get(0),
            )?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        rows
    };

    // The canonical boot-reconcile identity: a `System` actor a chat message
    // cannot forge (migration 127 widened the actor CHECK to admit it).
    let actor = OrchestrationActor {
        kind: PlanningActorKind::System,
        id: Some("boot-reconcile".into()),
        session_id: None,
        source_message_id: None,
    };

    let mut moved = Vec::new();
    for id in ids {
        // Quiesce the exact durable response in the same savepoint as the
        // interrupt. `recover_after_restart` runs first and may have requeued a
        // Running job; it must not become claimable before Git classification.
        // Already-Interrupted rows are included so a crash during a previous
        // boot cannot leave an old response live on the next one.
        let transitioned = in_savepoint(conn, |conn| {
            let execution = get_task_execution(conn, &id)?
                .ok_or_else(|| anyhow::anyhow!("execution vanished during boot reconcile"))?;
            if let (Some(child), Some(dispatch_id)) = (
                execution.sub_discussion_id.as_deref(),
                execution.dispatch_job_id.as_deref(),
            ) {
                crate::db::agent_dispatch::cancel_for_discussion_by_id(conn, child, dispatch_id)?;
            }
            if execution.status == TaskExecutionStatus::Interrupted {
                return Ok(false);
            }
            transition_execution(
                conn,
                &id,
                TaskExecutionStatus::Interrupted,
                &actor,
                serde_json::json!({ "reason": "boot_reconcile" }),
            )
        })?;
        if transitioned {
            moved.push(id);
        }
    }
    Ok(moved)
}

/// All quiescent interrupted executions. A pending decision is included: the
/// prior process may have died after classifying but before applying it, and
/// boot must replay that durable decision instead of stranding the row.
pub fn list_interrupted_execution_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT e.id FROM task_executions e \
         WHERE e.status = 'Interrupted' \
         ORDER BY e.created_at, e.id",
    )?;
    let rows = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(rows)
}

#[cfg(test)]
#[path = "orchestration_tests.rs"]
mod tests;
