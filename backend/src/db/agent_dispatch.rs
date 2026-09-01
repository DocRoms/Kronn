use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::models::ActiveAgentDispatch;
use crate::models::AgentType;

use super::parse_dt;

pub const MAX_DISPATCH_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl DispatchStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "Pending" => Ok(Self::Pending),
            "Running" => Ok(Self::Running),
            "Completed" => Ok(Self::Completed),
            "Failed" => Ok(Self::Failed),
            "Cancelled" => Ok(Self::Cancelled),
            other => anyhow::bail!("unknown dispatch status: {other}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentDispatchJob {
    pub id: String,
    pub discussion_id: String,
    pub trigger_message_id: String,
    pub trigger_sort_order: i64,
    pub dedupe_key: String,
    pub agent_override: Option<AgentType>,
    pub chain_prompt_ids: Vec<String>,
    pub next_chain_index: usize,
    pub batch_item: Option<String>,
    pub group_id: Option<String>,
    pub group_concurrency_limit: Option<u32>,
    pub status: DispatchStatus,
    pub attempts: u32,
    pub turn_attempts: u32,
    pub available_at: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    /// First instant at which the global agent permit was acquired and the
    /// native/HTTP runtime was about to be invoked. Unlike `claimed_at`, this
    /// excludes time spent queued behind the process-wide semaphore.
    pub agent_started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// Machine-readable terminal cause. In particular `quota_exhausted` must
    /// never be treated as a process crash by the watchdog.
    pub failure_kind: Option<String>,
    pub watchdog_redispatches: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub progress_phase: Option<String>,
    pub progress_detail: Option<String>,
    pub last_progress_at: Option<DateTime<Utc>>,
    /// Exact named external API connection selected for this responder.
    /// `AgentType::Custom` is shared by every non-legacy OpenAI-compatible
    /// connection, so this identity must survive queueing and retries.
    pub connection_id: Option<String>,
}

pub struct NewAgentDispatchJob<'a> {
    pub id: &'a str,
    pub discussion_id: &'a str,
    pub trigger_message_id: &'a str,
    pub trigger_sort_order: i64,
    pub dedupe_key: &'a str,
    pub agent_override: Option<&'a AgentType>,
    pub chain_prompt_ids: &'a [String],
    pub batch_item: Option<&'a str>,
    pub group_id: Option<&'a str>,
    pub group_concurrency_limit: Option<u32>,
}

fn map_job(row: &Row<'_>) -> rusqlite::Result<AgentDispatchJob> {
    let agent_json: Option<String> = row.get(5)?;
    let chain_json: String = row.get(6)?;
    let status: String = row.get(11)?;
    let parse_error = |index: usize, error: anyhow::Error| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, error.into())
    };

    Ok(AgentDispatchJob {
        id: row.get(0)?,
        discussion_id: row.get(1)?,
        trigger_message_id: row.get(2)?,
        trigger_sort_order: row.get(3)?,
        dedupe_key: row.get(4)?,
        agent_override: agent_json
            .map(|value| serde_json::from_str(&value).map_err(|error| parse_error(5, error.into())))
            .transpose()?,
        chain_prompt_ids: serde_json::from_str(&chain_json)
            .map_err(|error| parse_error(6, error.into()))?,
        next_chain_index: row.get::<_, i64>(7)?.max(0) as usize,
        batch_item: row.get(8)?,
        group_id: row.get(9)?,
        group_concurrency_limit: row
            .get::<_, Option<i64>>(10)?
            .map(|value| value.max(1) as u32),
        status: DispatchStatus::parse(&status).map_err(|error| parse_error(11, error))?,
        attempts: row.get::<_, i64>(12)?.max(0) as u32,
        turn_attempts: row.get::<_, i64>(13)?.max(0) as u32,
        available_at: parse_dt(row.get(14)?),
        claimed_at: row.get::<_, Option<String>>(15)?.map(parse_dt),
        agent_started_at: row.get::<_, Option<String>>(16)?.map(parse_dt),
        completed_at: row.get::<_, Option<String>>(17)?.map(parse_dt),
        last_error: row.get(18)?,
        failure_kind: row.get(19)?,
        watchdog_redispatches: row.get::<_, i64>(20)?.max(0) as u32,
        created_at: parse_dt(row.get(21)?),
        updated_at: parse_dt(row.get(22)?),
        progress_phase: row.get(23)?,
        progress_detail: row.get(24)?,
        last_progress_at: row.get::<_, Option<String>>(25)?.map(parse_dt),
        connection_id: row.get(26)?,
    })
}

const JOB_COLUMNS: &str = "id, discussion_id, trigger_message_id, trigger_sort_order,
    dedupe_key, agent_override_json, chain_prompt_ids_json, next_chain_index,
    batch_item, group_id, group_concurrency_limit, status, attempts, turn_attempts,
    available_at, claimed_at, agent_started_at, completed_at, last_error,
    failure_kind, watchdog_redispatches, created_at, updated_at,
    progress_phase, progress_detail, last_progress_at, connection_id";

pub fn list_active_for_discussion(
    conn: &Connection,
    discussion_id: &str,
    default_agent: &AgentType,
) -> Result<Vec<ActiveAgentDispatch>> {
    let mut stmt = conn.prepare(
        "SELECT id, trigger_message_id, agent_override_json, status, attempts, last_error,
                connection_id
         FROM agent_dispatch_jobs
         WHERE discussion_id = ?1 AND status IN ('Pending', 'Running')
         ORDER BY trigger_sort_order ASC, created_at ASC",
    )?;
    let rows = stmt.query_map([discussion_id], |row| {
        let override_json: Option<String> = row.get(2)?;
        let agent_type = override_json
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok())
            .unwrap_or_else(|| default_agent.clone());
        Ok(ActiveAgentDispatch {
            id: row.get(0)?,
            trigger_message_id: row.get(1)?,
            agent_type,
            status: row.get(3)?,
            attempts: Some(row.get::<_, i64>(4)?.max(0) as u32),
            last_error: row.get(5)?,
            connection_id: row.get(6)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("list active agent dispatches")
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<AgentDispatchJob>> {
    conn.query_row(
        &format!("SELECT {JOB_COLUMNS} FROM agent_dispatch_jobs WHERE id = ?1"),
        [id],
        map_job,
    )
    .optional()
    .map_err(Into::into)
}

pub fn find_active_for_discussion(
    conn: &Connection,
    discussion_id: &str,
) -> Result<Option<AgentDispatchJob>> {
    conn.query_row(
        &format!(
            "SELECT {JOB_COLUMNS} FROM agent_dispatch_jobs
             WHERE discussion_id = ?1 AND status IN ('Pending', 'Running')
             ORDER BY CASE status WHEN 'Running' THEN 0 ELSE 1 END,
                      created_at, id
             LIMIT 1"
        ),
        [discussion_id],
        map_job,
    )
    .optional()
    .map_err(Into::into)
}

pub fn enqueue(conn: &Connection, new: NewAgentDispatchJob<'_>) -> Result<AgentDispatchJob> {
    enqueue_with_connection(conn, new, None)
}

pub fn enqueue_with_connection(
    conn: &Connection,
    new: NewAgentDispatchJob<'_>,
    connection_id: Option<&str>,
) -> Result<AgentDispatchJob> {
    let now = Utc::now().to_rfc3339();
    let agent_json = new.agent_override.map(serde_json::to_string).transpose()?;
    let chain_json = serde_json::to_string(new.chain_prompt_ids)?;
    conn.execute(
        "INSERT INTO agent_dispatch_jobs
         (id, discussion_id, trigger_message_id, trigger_sort_order, dedupe_key,
          agent_override_json, chain_prompt_ids_json, batch_item, group_id,
          group_concurrency_limit, status, available_at, created_at, updated_at,
          connection_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'Pending', ?11, ?11, ?11, ?12)
         ON CONFLICT(dedupe_key) DO NOTHING",
        params![
            new.id,
            new.discussion_id,
            new.trigger_message_id,
            new.trigger_sort_order,
            new.dedupe_key,
            agent_json,
            chain_json,
            new.batch_item,
            new.group_id,
            new.group_concurrency_limit.map(i64::from),
            now,
            connection_id,
        ],
    )?;

    if let Some(job) = conn
        .query_row(
            &format!("SELECT {JOB_COLUMNS} FROM agent_dispatch_jobs WHERE dedupe_key = ?1"),
            [new.dedupe_key],
            map_job,
        )
        .optional()?
    {
        return Ok(job);
    }

    anyhow::bail!("dispatch enqueue did not persist or match its dedupe key")
}

pub struct NewLatestUserDispatch<'a> {
    pub id: &'a str,
    pub discussion_id: &'a str,
    pub dedupe_key: &'a str,
    pub agent_override: Option<&'a AgentType>,
    pub chain_prompt_ids: &'a [String],
    pub batch_item: Option<&'a str>,
    pub group_id: Option<&'a str>,
    pub group_concurrency_limit: Option<u32>,
}

pub fn enqueue_for_latest_user(
    conn: &Connection,
    new: NewLatestUserDispatch<'_>,
) -> Result<AgentDispatchJob> {
    enqueue_for_latest_user_with_connection(conn, new, None)
}

pub fn enqueue_for_latest_user_with_connection(
    conn: &Connection,
    new: NewLatestUserDispatch<'_>,
    connection_id: Option<&str>,
) -> Result<AgentDispatchJob> {
    let trigger = conn
        .query_row(
            "SELECT id, sort_order FROM messages
             WHERE discussion_id = ?1 AND role = 'User' AND channel = 'main'
             ORDER BY sort_order DESC LIMIT 1",
            [new.discussion_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
        .context("cannot dispatch a discussion without a user message")?;
    enqueue_with_connection(
        conn,
        NewAgentDispatchJob {
            id: new.id,
            discussion_id: new.discussion_id,
            trigger_message_id: &trigger.0,
            trigger_sort_order: trigger.1,
            dedupe_key: new.dedupe_key,
            agent_override: new.agent_override,
            chain_prompt_ids: new.chain_prompt_ids,
            batch_item: new.batch_item,
            group_id: new.group_id,
            group_concurrency_limit: new.group_concurrency_limit,
        },
        connection_id,
    )
}

/// Enqueue an explicit human retry of one failed target. The original trigger
/// and agent override are copied verbatim, so a three-agent turn retries only
/// the provider named by its error card. The caller-scoped dedupe key makes a
/// double-click or transport replay harmless.
pub fn enqueue_retry(
    conn: &Connection,
    discussion_id: &str,
    failed_dispatch_id: &str,
    idempotency_key: &str,
    new_id: &str,
) -> Result<(AgentDispatchJob, bool)> {
    let failed = get(conn, failed_dispatch_id)?.context("failed dispatch not found")?;
    anyhow::ensure!(
        failed.discussion_id == discussion_id,
        "failed dispatch belongs to another discussion"
    );
    anyhow::ensure!(
        failed.status == DispatchStatus::Failed,
        "dispatch is not failed"
    );
    anyhow::ensure!(
        failed.group_id.is_none(),
        "workflow dispatch retry is not supported here"
    );
    // A primary-agent dispatch stores no override because it normally follows
    // `Discussion.agent`. The attributed System error freezes who actually
    // failed; use it so changing the discussion agent before clicking Retry
    // cannot silently redirect the retry to a different provider.
    let failed_agent = match failed.agent_override.clone() {
        Some(agent) => Some(agent),
        None => conn
            .query_row(
                "SELECT agent_type FROM messages
                 WHERE agent_dispatch_job_id = ?1 AND role = 'System'
                 ORDER BY sort_order DESC LIMIT 1",
                [failed_dispatch_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .map(|value| super::discussions::parse_agent_type(&value)),
    };
    let dedupe_key = format!("retry:{failed_dispatch_id}:{idempotency_key}");
    let existed = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_dispatch_jobs WHERE dedupe_key = ?1)",
        [&dedupe_key],
        |row| row.get::<_, bool>(0),
    )?;
    let failed_connection_id = failed.connection_id.clone();
    let job = enqueue_with_connection(
        conn,
        NewAgentDispatchJob {
            id: new_id,
            discussion_id,
            trigger_message_id: &failed.trigger_message_id,
            trigger_sort_order: failed.trigger_sort_order,
            dedupe_key: &dedupe_key,
            agent_override: failed_agent.as_ref(),
            chain_prompt_ids: &failed.chain_prompt_ids,
            batch_item: failed.batch_item.as_deref(),
            group_id: None,
            group_concurrency_limit: None,
        },
        failed_connection_id.as_deref(),
    )?;
    Ok((job, existed))
}

pub fn mark_error_retried(conn: &Connection, failed_dispatch_id: &str) -> Result<()> {
    let row = conn
        .query_row(
            "SELECT id, content FROM messages
             WHERE agent_dispatch_job_id = ?1 AND role = 'System'
             ORDER BY sort_order DESC LIMIT 1",
            [failed_dispatch_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((message_id, content)) = row else {
        return Ok(());
    };
    let Some(payload) = content.strip_prefix("[kronn:agent-error]\n") else {
        return Ok(());
    };
    let mut value: serde_json::Value = serde_json::from_str(payload)?;
    value["retried"] = serde_json::Value::Bool(true);
    conn.execute(
        "UPDATE messages SET content = ?2 WHERE id = ?1",
        params![message_id, format!("[kronn:agent-error]\n{value}")],
    )?;
    Ok(())
}

pub fn list_runnable_ids(conn: &Connection, limit: usize) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT candidate.id
         FROM agent_dispatch_jobs AS candidate
         JOIN discussions AS discussion ON discussion.id = candidate.discussion_id
         WHERE candidate.status = 'Pending'
           AND candidate.available_at <= ?1
           AND (
               (discussion.no_agent = 0 AND candidate.attempts < ?2)
               OR discussion.no_agent = 1
           )
         ORDER BY candidate.created_at, candidate.id LIMIT ?3",
    )?;
    let rows = statement.query_map(
        params![
            Utc::now().to_rfc3339(),
            i64::from(MAX_DISPATCH_ATTEMPTS),
            limit.max(1) as i64
        ],
        |row| row.get::<_, String>(0),
    )?;
    Ok(rows.filter_map(|row| row.ok()).collect())
}

pub fn list_exhausted_ids(conn: &Connection, limit: usize) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT candidate.id
         FROM agent_dispatch_jobs AS candidate
         JOIN discussions AS discussion ON discussion.id = candidate.discussion_id
         WHERE candidate.status = 'Pending'
           AND candidate.available_at <= ?1
           AND candidate.attempts >= ?2
           AND discussion.no_agent = 0
         ORDER BY candidate.created_at, candidate.id LIMIT ?3",
    )?;
    let rows = statement.query_map(
        params![
            Utc::now().to_rfc3339(),
            i64::from(MAX_DISPATCH_ATTEMPTS),
            limit.max(1) as i64
        ],
        |row| row.get::<_, String>(0),
    )?;
    Ok(rows.filter_map(|row| row.ok()).collect())
}

/// The agent a job will actually run as: its own override when the batch item
/// carries one, the discussion's agent otherwise. Written once here so the two
/// places that compare families can never drift apart.
const EFFECTIVE_AGENT: &str = "COALESCE(
    json_extract(%T%.agent_override_json, '$'),
    (SELECT agent FROM discussions WHERE id = %T%.discussion_id)
)";

/// Providers that consume the machine-wide local pool. Remote HTTP routes are
/// intentionally absent: their capacity belongs to the remote service.
const LOCAL_AGENT_SQL_LIST: &str =
    "'ClaudeCode','Codex','GeminiCli','Kiro','Vibe','CopilotCli','Ollama'";

fn effective_agent(alias: &str) -> String {
    EFFECTIVE_AGENT.replace("%T%", alias)
}

pub fn claim(conn: &Connection, id: &str) -> Result<Option<AgentDispatchJob>> {
    claim_with_limits(conn, id, None)
}

/// `limits` is a JSON object mapping an agent name to how many of its runs may
/// be `Running` at once, e.g. `{"Ollama":1,"ClaudeCode":2}`. The reserved
/// `__local_global` key additionally caps the sum of Ollama plus every CLI.
/// An agent absent from the map is unlimited — that is how a remote HTTP
/// provider stays uncapped and outside the machine-local pool.
/// `None` disables the check entirely, which is what the in-transaction callers
/// want: they claim a job the scheduler already admitted.
pub fn claim_with_limits(
    conn: &Connection,
    id: &str,
    limits: Option<&str>,
) -> Result<Option<AgentDispatchJob>> {
    let now = Utc::now().to_rfc3339();
    let candidate_agent = effective_agent("candidate");
    let family_agent = effective_agent("family");
    let local_family_agent = effective_agent("local_family");
    conn.query_row(
        &format!(
            "UPDATE agent_dispatch_jobs AS candidate
             SET status = 'Running',
                 attempts = attempts + 1,
                 turn_attempts = turn_attempts + 1,
                 claimed_at = ?2,
                 updated_at = ?2,
                 last_error = NULL
             WHERE id = ?1
               AND status = 'Pending'
               AND available_at <= ?2
               AND attempts < ?3
               AND EXISTS (
                   SELECT 1
                   FROM discussions AS discussion
                   WHERE discussion.id = candidate.discussion_id
                     AND discussion.no_agent = 0
               )
               AND NOT EXISTS (
                   SELECT 1
                   FROM agent_dispatch_jobs AS same_discussion
                   WHERE same_discussion.discussion_id = candidate.discussion_id
                     AND same_discussion.status = 'Running'
               )
               AND (
                    group_id IS NULL
                    OR group_concurrency_limit IS NULL
                    OR (
                        SELECT COUNT(*)
                        FROM agent_dispatch_jobs AS running
                        WHERE running.group_id = candidate.group_id
                          AND running.status = 'Running'
                    ) < group_concurrency_limit
               )
               AND (
                    ?4 IS NULL
                    OR json_extract(?4, '$.' || ({candidate_agent})) IS NULL
                    OR (
                        SELECT COUNT(*)
                        FROM agent_dispatch_jobs AS family
                        WHERE family.status = 'Running'
                          AND ({family_agent}) = ({candidate_agent})
                    ) < json_extract(?4, '$.' || ({candidate_agent}))
               )
               AND (
                    ?4 IS NULL
                    OR json_extract(?4, '$.__local_global') IS NULL
                    OR ({candidate_agent}) NOT IN ({local_agents})
                    OR (
                        SELECT COUNT(*)
                        FROM agent_dispatch_jobs AS local_family
                        WHERE local_family.status = 'Running'
                          AND ({local_family_agent}) IN ({local_agents})
                    ) < json_extract(?4, '$.__local_global')
               )
             RETURNING {JOB_COLUMNS}",
            local_agents = LOCAL_AGENT_SQL_LIST,
        ),
        params![id, now, i64::from(MAX_DISPATCH_ATTEMPTS), limits],
        map_job,
    )
    .optional()
    .map_err(anyhow::Error::from)
    .and_then(|claimed| {
        if claimed.is_some() {
            return Ok(claimed);
        }

        // A human can disable the native responder after a route decided to
        // enqueue but before this claim. Refuse the launch atomically above,
        // then retire that stale obligation so it cannot hot-loop forever.
        let cancelled = cancel_pending_job_if_agent_disabled(conn, id)?;
        if cancelled > 0 && !has_active_for_discussion_by_job(conn, id)? {
            conn.execute(
                "UPDATE discussions
                 SET awaiting_agent = 0
                 WHERE id = (
                     SELECT discussion_id FROM agent_dispatch_jobs WHERE id = ?1
                 )",
                [id],
            )?;
        }
        Ok(None)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentDispatchRestartRecovery {
    pub requeued: u64,
    pub cancelled_workflow_children: u64,
    /// Jobs retired because the room moved past the turn that triggered them.
    /// Counted separately from `requeued` so a restart that silently drops a
    /// turn is visible in the boot log rather than inferred later from an
    /// agent answering something nobody asked any more.
    pub superseded: u64,
}

pub fn recover_after_restart(conn: &Connection) -> Result<AgentDispatchRestartRecovery> {
    let now = Utc::now().to_rfc3339();
    let tx = conn.unchecked_transaction()?;
    // Workflow-owned batch groups have just been moved to Interrupted by the
    // boot reconcile. Replaying their child prompts would run outside the dead
    // parent's lifecycle (and may point at a worktree that no longer exists),
    // so retire BOTH Running and merely queued children fail-closed. Ordinary
    // discussion turns keep the historical restart retry semantics below.
    tx.execute(
        "UPDATE workflow_runs AS batch
            SET state = json_set(
                COALESCE(batch.state, '{}'),
                '$.dispatch_attempts', COALESCE((
                    SELECT SUM(dispatch.attempts)
                      FROM agent_dispatch_jobs dispatch
                     WHERE dispatch.group_id = batch.id
                ), 0),
                '$.redispatches', COALESCE((
                    SELECT SUM(MAX(dispatch.attempts - 1, 0))
                      FROM agent_dispatch_jobs dispatch
                     WHERE dispatch.group_id = batch.id
                ), 0),
                '$.restart_cancelled_children', COALESCE((
                    SELECT COUNT(*)
                      FROM agent_dispatch_jobs dispatch
                     WHERE dispatch.group_id = batch.id
                       AND dispatch.status IN ('Pending', 'Running')
                ), 0)
            )
          WHERE batch.run_type = 'batch'
            AND batch.status = 'Interrupted'
            AND EXISTS (
                SELECT 1 FROM workflow_runs parent
                 WHERE parent.id = batch.parent_run_id
                   AND parent.status = 'Interrupted'
            )",
        [],
    )?;
    let cancelled = tx.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Cancelled', completed_at = ?1, updated_at = ?1,
             claimed_at = NULL, agent_started_at = NULL,
             last_error = 'parent_workflow_interrupted'
         WHERE status IN ('Pending', 'Running')
           AND group_id IN (
               SELECT child.id
               FROM workflow_runs child
               JOIN workflow_runs parent ON parent.id = child.parent_run_id
               WHERE child.run_type = 'batch'
                 AND (child.status = 'Interrupted' OR parent.status = 'Interrupted')
           )",
        [&now],
    )?;
    tx.execute(
        "UPDATE discussions
            SET awaiting_agent = 0
          WHERE awaiting_agent = 1
            AND workflow_run_id IN (
                SELECT child.id
                FROM workflow_runs child
                JOIN workflow_runs parent ON parent.id = child.parent_run_id
                WHERE child.run_type = 'batch'
                  AND (child.status = 'Interrupted' OR parent.status = 'Interrupted')
            )",
        [],
    )?;
    // KT-333 — a restart used to re-serve every Running job as if it were new,
    // with no comparison to what the room had become. The measured episode:
    // trigger at sort_order 1540, the agent had already posted three partial
    // answers (1541-1543), and each restart handed it the same brief again
    // until the attempt budget ran out and the turn was lost outright. Both
    // outcomes are wrong, and neither was distinguishable from a normal retry.
    //
    // A turn is stale once the room has spoken past it. Retiring it costs the
    // request; re-serving it costs an answer to a question nobody is asking any
    // more, posted with the authority of a fresh reply — and someone has to
    // notice and undo it. The fail-closed treatment already applied to
    // interrupted workflow children above is the same judgement.
    let superseded = tx.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Cancelled', completed_at = ?1, updated_at = ?1,
             claimed_at = NULL, agent_started_at = NULL,
             last_error = 'superseded_by_newer_turns'
         WHERE status = 'Running'
           AND EXISTS (
               SELECT 1 FROM messages newer
                WHERE newer.discussion_id = agent_dispatch_jobs.discussion_id
                  AND newer.sort_order > agent_dispatch_jobs.trigger_sort_order
           )",
        [&now],
    )?;
    // Leaving `awaiting_agent` set would park the room on an answer that is
    // never coming: the job it was waiting for has just been retired. Same
    // release the workflow branch performs, for the same reason.
    tx.execute(
        "UPDATE discussions
            SET awaiting_agent = 0
          WHERE awaiting_agent = 1
            AND id IN (
                SELECT discussion_id FROM agent_dispatch_jobs
                 WHERE status = 'Cancelled'
                   AND last_error = 'superseded_by_newer_turns'
                   AND completed_at = ?1
            )",
        [&now],
    )?;
    let requeued = tx.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Pending', claimed_at = NULL, agent_started_at = NULL,
             available_at = ?1, updated_at = ?1,
             last_error = 'backend_restarted'
         WHERE status = 'Running'",
        [&now],
    )?;
    tx.commit()?;
    Ok(AgentDispatchRestartRecovery {
        requeued: requeued as u64,
        cancelled_workflow_children: cancelled as u64,
        superseded: superseded as u64,
    })
}

/// Compatibility wrapper for callers/tests interested only in ordinary
/// discussion jobs that remain retryable across a restart.
pub fn reset_running_after_restart(conn: &Connection) -> Result<u64> {
    Ok(recover_after_restart(conn)?.requeued)
}

/// Persist the boundary between queueing and real agent execution.
///
/// Returning `false` means cancellation or another terminal transition won
/// the race after claim; callers must then skip the provider invocation.
pub fn mark_agent_started(conn: &Connection, id: &str) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET agent_started_at = COALESCE(agent_started_at, ?2), updated_at = ?2,
             progress_phase = 'launching', progress_detail = NULL,
             last_progress_at = ?2
         WHERE id = ?1 AND status = 'Running'",
        params![id, now],
    )?;
    Ok(changed > 0)
}

/// Persist a provider/tool boundary without interpreting silence as failure.
pub fn mark_progress(
    conn: &Connection,
    id: &str,
    phase: &str,
    detail: Option<&str>,
) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    Ok(conn.execute(
        "UPDATE agent_dispatch_jobs SET progress_phase = ?2, progress_detail = ?3,
             last_progress_at = ?4, updated_at = ?4
         WHERE id = ?1 AND status = 'Running'",
        params![id, phase, detail, now],
    )? > 0)
}

pub fn mark_completed(conn: &Connection, id: &str) -> Result<bool> {
    set_terminal(conn, id, DispatchStatus::Completed, None)
}

pub fn mark_failed(conn: &Connection, id: &str, error: &str) -> Result<bool> {
    set_terminal(conn, id, DispatchStatus::Failed, Some(error))
}

pub fn mark_failed_with_kind(
    conn: &Connection,
    id: &str,
    error: &str,
    failure_kind: &str,
) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    Ok(conn.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Failed', completed_at = ?3, updated_at = ?3,
             last_error = ?2, failure_kind = ?4
         WHERE id = ?1 AND status IN ('Pending', 'Running')",
        params![id, error, now, failure_kind],
    )? > 0)
}

/// Claimed provider processes older than `before` whose cancel token vanished
/// are candidates for the crash watchdog. Callers still check the in-memory
/// registry immediately before transitioning to avoid killing a live run.
pub fn list_watchdog_candidates(
    conn: &Connection,
    before: DateTime<Utc>,
) -> Result<Vec<AgentDispatchJob>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {JOB_COLUMNS} FROM agent_dispatch_jobs
         WHERE status = 'Running' AND agent_started_at IS NOT NULL
           AND agent_started_at <= ?1 AND failure_kind IS NULL
         ORDER BY agent_started_at, id LIMIT 64"
    ))?;
    let rows = statement.query_map([before.to_rfc3339()], map_job)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogTransition {
    Redispatched,
    Escalated,
    Unchanged,
}

pub fn apply_watchdog_stall(conn: &Connection, id: &str) -> Result<WatchdogTransition> {
    let now = Utc::now();
    let retry_at = (now + Duration::seconds(2)).to_rfc3339();
    let now = now.to_rfc3339();
    let retried = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Pending', claimed_at = NULL, agent_started_at = NULL,
             available_at = ?2, watchdog_redispatches = 1,
             last_error = 'watchdog_stall_redispatch', updated_at = ?3
         WHERE id = ?1 AND status = 'Running' AND watchdog_redispatches = 0
           AND failure_kind IS NULL",
        params![id, retry_at, now],
    )?;
    if retried > 0 {
        return Ok(WatchdogTransition::Redispatched);
    }
    let escalated = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Failed', completed_at = ?2, updated_at = ?2,
             last_error = 'watchdog_stall_after_redispatch',
             failure_kind = 'dispatch_stalled'
         WHERE id = ?1 AND status = 'Running' AND watchdog_redispatches = 1
           AND failure_kind IS NULL",
        params![id, now],
    )?;
    Ok(if escalated > 0 {
        WatchdogTransition::Escalated
    } else {
        WatchdogTransition::Unchanged
    })
}

fn set_terminal(
    conn: &Connection,
    id: &str,
    status: DispatchStatus,
    error: Option<&str>,
) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET status = ?2, completed_at = ?3, updated_at = ?3, last_error = ?4
         WHERE id = ?1 AND status IN ('Pending', 'Running')",
        params![id, status.as_str(), now, error],
    )?;
    Ok(changed > 0)
}

pub fn retry_after(conn: &Connection, id: &str, delay_seconds: i64, error: &str) -> Result<bool> {
    let now = Utc::now();
    let available_at = (now + Duration::seconds(delay_seconds.max(0))).to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Pending', claimed_at = NULL, agent_started_at = NULL, available_at = ?2,
             updated_at = ?3, last_error = ?4
         WHERE id = ?1 AND status = 'Running'",
        params![id, available_at, now.to_rfc3339(), error],
    )?;
    Ok(changed > 0)
}

/// Return a claimed job to the durable queue when its agent runtime could not
/// be started. Runtime availability is not an execution attempt: keep both
/// counters unchanged so the obligation can survive until the runtime comes
/// back (or the user explicitly cancels it).
pub fn defer_runtime_unavailable(
    conn: &Connection,
    id: &str,
    delay_seconds: i64,
    error: &str,
) -> Result<bool> {
    let now = Utc::now();
    let available_at = (now + Duration::seconds(delay_seconds.max(0))).to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Pending',
             attempts = MAX(attempts - 1, 0),
             turn_attempts = MAX(turn_attempts - 1, 0),
             claimed_at = NULL,
             agent_started_at = NULL,
             available_at = ?2,
             updated_at = ?3,
             last_error = ?4
         WHERE id = ?1 AND status = 'Running'",
        params![id, available_at, now.to_rfc3339(), error],
    )?;
    Ok(changed > 0)
}

pub fn release_unstarted_claim(conn: &Connection, id: &str, error: &str) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Pending',
             attempts = MAX(attempts - 1, 0),
             turn_attempts = MAX(turn_attempts - 1, 0),
             claimed_at = NULL,
             agent_started_at = NULL,
             available_at = ?2,
             updated_at = ?2,
             last_error = ?3
         WHERE id = ?1 AND status = 'Running'",
        params![id, now, error],
    )?;
    Ok(changed > 0)
}

pub fn cancel_for_discussion(conn: &Connection, discussion_id: &str) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Cancelled', completed_at = ?2, updated_at = ?2,
             last_error = 'cancelled'
         WHERE id = (
             SELECT id FROM agent_dispatch_jobs
             WHERE discussion_id = ?1 AND status IN ('Pending', 'Running')
             ORDER BY CASE status WHEN 'Running' THEN 0 ELSE 1 END,
                      created_at, id
             LIMIT 1
         )",
        params![discussion_id, now],
    )?;
    Ok(changed as u64)
}

/// Cancel one exact durable response without touching sibling jobs queued for
/// the same discussion. The discussion id is part of the predicate so an id
/// copied from another room cannot be used as a cross-room cancellation
/// handle.
pub fn cancel_for_discussion_by_id(
    conn: &Connection,
    discussion_id: &str,
    dispatch_id: &str,
) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Cancelled', completed_at = ?3, updated_at = ?3,
             last_error = 'cancelled'
         WHERE id = ?2 AND discussion_id = ?1
           AND status IN ('Pending', 'Running')",
        params![discussion_id, dispatch_id, now],
    )?;
    Ok(changed > 0)
}

/// Cancel every queued native response for a discussion. Running work is left
/// alone: the no-agent toggle prevents future claims, but does not pretend an
/// already-started process was stopped.
pub fn cancel_pending_for_discussion(conn: &Connection, discussion_id: &str) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Cancelled', completed_at = ?2, updated_at = ?2,
             last_error = 'agent_disabled'
         WHERE discussion_id = ?1 AND status = 'Pending'",
        params![discussion_id, now],
    )?;
    Ok(changed as u64)
}

fn cancel_pending_job_if_agent_disabled(conn: &Connection, id: &str) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_dispatch_jobs AS candidate
         SET status = 'Cancelled', completed_at = ?2, updated_at = ?2,
             last_error = 'agent_disabled'
         WHERE id = ?1
           AND status = 'Pending'
           AND EXISTS (
               SELECT 1 FROM discussions AS discussion
               WHERE discussion.id = candidate.discussion_id
                 AND discussion.no_agent = 1
           )",
        params![id, now],
    )?;
    Ok(changed as u64)
}

fn has_active_for_discussion_by_job(conn: &Connection, id: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM agent_dispatch_jobs AS active
             WHERE active.discussion_id = (
                 SELECT discussion_id FROM agent_dispatch_jobs WHERE id = ?1
             )
               AND active.status IN ('Pending', 'Running')
         )",
        [id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn has_active_for_discussion(conn: &Connection, discussion_id: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM agent_dispatch_jobs
             WHERE discussion_id = ?1 AND status IN ('Pending', 'Running')
         )",
        [discussion_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn latest_completed_agent_message(
    conn: &Connection,
    job: &AgentDispatchJob,
) -> Result<Option<(String, String, bool)>> {
    conn.query_row(
        "SELECT id, content, agent_run_succeeded FROM messages
         WHERE discussion_id = ?1
           AND role = 'Agent'
           AND sort_order > ?2
           AND recovered_partial = 0
           AND agent_run_succeeded IS NOT NULL
           AND agent_dispatch_job_id = ?3
         ORDER BY sort_order DESC LIMIT 1",
        params![job.discussion_id, job.trigger_sort_order, job.id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, bool>(2)?)),
    )
    .optional()
    .map_err(Into::into)
}

pub fn advance_chain_trigger(
    conn: &Connection,
    job_id: &str,
    message: &crate::models::DiscussionMessage,
) -> Result<bool> {
    let transaction = conn.unchecked_transaction()?;
    let job = get(&transaction, job_id)?.context("dispatch job not found")?;
    if job.status != DispatchStatus::Running {
        return Ok(false);
    }
    let sort_order =
        crate::db::discussions::insert_message(&transaction, &job.discussion_id, message)?;
    let now = Utc::now().to_rfc3339();
    let changed = transaction.execute(
        "UPDATE agent_dispatch_jobs
         SET trigger_message_id = ?2, trigger_sort_order = ?3,
             next_chain_index = next_chain_index + 1,
             status = 'Pending', turn_attempts = 0, claimed_at = NULL,
             agent_started_at = NULL,
             available_at = ?4, updated_at = ?4, last_error = NULL
         WHERE id = ?1 AND status = 'Running'",
        params![job_id, message.id, sort_order, now],
    )?;
    crate::db::discussions::set_awaiting_agent(&transaction, &job.discussion_id, true)?;
    transaction.commit()?;
    Ok(changed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrations::run(&connection).unwrap();
        let now = Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('d1', 'Dispatch', ?1, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES ('u1', 'd1', 'User', 'go', ?1, 1, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE discussions SET next_message_seq = 2 WHERE id = 'd1'",
                [],
            )
            .unwrap();
        connection
    }

    fn enqueue_default(connection: &Connection, id: &str, dedupe_key: &str) -> AgentDispatchJob {
        enqueue_for_latest_user(
            connection,
            NewLatestUserDispatch {
                id,
                discussion_id: "d1",
                dedupe_key,
                agent_override: None,
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn named_connection_survives_queue_claim_and_retry() {
        let connection = connection();
        connection
            .execute(
                "INSERT INTO external_api_connections
                 (id, display_name, mention_alias, endpoint, credential_slug, origin_preset)
                 VALUES ('router', 'OpenRouter', 'openrouter',
                         'https://openrouter.ai/api', 'conn-router', 'open_router')",
                [],
            )
            .unwrap();

        let queued = enqueue_for_latest_user_with_connection(
            &connection,
            NewLatestUserDispatch {
                id: "router-job",
                discussion_id: "d1",
                dedupe_key: "router-turn",
                agent_override: Some(&AgentType::Custom),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
            Some("router"),
        )
        .unwrap();
        assert_eq!(queued.connection_id.as_deref(), Some("router"));
        let claimed = claim(&connection, &queued.id).unwrap().unwrap();
        assert_eq!(claimed.connection_id.as_deref(), Some("router"));
        mark_failed(&connection, &queued.id, "temporary refusal").unwrap();

        let (retry, existed) =
            enqueue_retry(&connection, "d1", &queued.id, "retry-once", "router-retry").unwrap();
        assert!(!existed);
        assert_eq!(retry.connection_id.as_deref(), Some("router"));
    }

    /// The room speaks past a running job, the way an agent's own partial
    /// answers did in the measured episode.
    fn room_moves_past(connection: &Connection, sort_order: i64, id: &str) {
        let now = Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES (?1, 'd1', 'Agent', 'je reprends comme implémenteur…', ?2, ?3, ?2)",
                rusqlite::params![id, now, sort_order],
            )
            .unwrap();
    }

    #[test]
    fn a_restart_retires_a_job_the_room_has_already_moved_past() {
        // KT-333 — the exact shape of the incident: the job is claimed and
        // running, the agent posts partial answers, the backend restarts. The
        // old behaviour handed the same brief back as if it were new, again
        // and again, until the attempt budget ran out and the turn was lost.
        let connection = connection();
        let job = enqueue_default(&connection, "stale-job", "turn-1");
        claim(&connection, &job.id).unwrap().unwrap();
        mark_agent_started(&connection, &job.id).unwrap();
        connection
            .execute(
                "UPDATE discussions SET awaiting_agent = 1 WHERE id = 'd1'",
                [],
            )
            .unwrap();
        room_moves_past(&connection, 2, "partial-answer");

        let recovery = recover_after_restart(&connection).unwrap();

        assert_eq!(recovery.superseded, 1);
        assert_eq!(
            recovery.requeued, 0,
            "a stale job must not also be counted as retryable",
        );
        let stale = get(&connection, "stale-job").unwrap().unwrap();
        assert_eq!(stale.status, DispatchStatus::Cancelled);
        assert_eq!(
            stale.last_error.as_deref(),
            Some("superseded_by_newer_turns"),
            "the reason has to be readable later, not inferred from a silence",
        );
        // The room must not stay parked on an answer that is never coming.
        let awaiting: i64 = connection
            .query_row(
                "SELECT awaiting_agent FROM discussions WHERE id = 'd1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(awaiting, 0, "the discussion is released, not left waiting");
    }

    #[test]
    fn a_restart_still_replays_a_job_nothing_has_spoken_past() {
        // The control that gives the test above its meaning: if the room did
        // not move, the turn was never answered and is still owed. Retiring it
        // here would trade a wrong answer for a missing one.
        let connection = connection();
        let job = enqueue_default(&connection, "fresh-job", "turn-1");
        claim(&connection, &job.id).unwrap().unwrap();
        mark_agent_started(&connection, &job.id).unwrap();

        let recovery = recover_after_restart(&connection).unwrap();

        assert_eq!(recovery.superseded, 0);
        assert_eq!(recovery.requeued, 1);
        let fresh = get(&connection, "fresh-job").unwrap().unwrap();
        assert_eq!(fresh.status, DispatchStatus::Pending);
        assert_eq!(fresh.last_error.as_deref(), Some("backend_restarted"));
    }

    #[test]
    fn restart_requeues_the_job_and_preserves_its_checkpoint_as_agent_history() {
        let connection = connection();
        let job = enqueue_default(&connection, "checkpoint-job", "turn-1");
        claim(&connection, &job.id).unwrap().unwrap();
        mark_agent_started(&connection, &job.id).unwrap();
        crate::db::discussions::set_partial_response_for_dispatch(
            &connection,
            "d1",
            "analyse sauvegardée avant le reboot",
            (&AgentType::ClaudeCode, Some("sonnet-test")),
            "checkpoint-job",
            &job.trigger_message_id,
            None,
        )
        .unwrap();

        // This is the exact main.rs order: dispatches become retryable before
        // checkpoints are materialised into transcript history.
        let dispatch_recovery = recover_after_restart(&connection).unwrap();
        let recovered_discussions =
            crate::db::discussions::recover_partial_responses(&connection).unwrap();

        assert_eq!(dispatch_recovery.requeued, 1);
        assert_eq!(recovered_discussions, vec!["d1"]);
        let resumed = get(&connection, "checkpoint-job").unwrap().unwrap();
        assert_eq!(resumed.status, DispatchStatus::Pending);
        assert_eq!(resumed.last_error.as_deref(), Some("backend_restarted"));
        let recovered: (String, i64, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT content, recovered_partial, reply_to_message_id,
                        agent_dispatch_job_id
                 FROM messages
                  WHERE discussion_id = 'd1' AND role = 'Agent'
                  ORDER BY sort_order DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert!(recovered.0.contains("analyse sauvegardée avant le reboot"));
        assert_eq!(recovered.1, 1);
        assert_eq!(recovered.2.as_deref(), Some("u1"));
        assert_eq!(recovered.3.as_deref(), Some("checkpoint-job"));
    }

    #[test]
    fn retiring_a_stale_job_changes_nothing_about_dispatch_idempotence() {
        // DoD-3 asks that the orchestration replay path (KT-318,
        // `orch-dispatch:{exec}:{attempt}`) survive this fix, so both halves of
        // it are pinned here.
        //
        // A dedupe key is spent for good — `enqueue` hands back whatever job
        // already holds it, terminal or not. That predates this change (a
        // Completed or workflow-Cancelled job behaves the same) and is exactly
        // what makes replay idempotent, so retiring a stale job must not be
        // read as freeing its key. A genuine re-dispatch does not need it
        // freed: it carries the next attempt number.
        let connection = connection();
        let job = enqueue_default(&connection, "stale-job", "orch-dispatch:exec-1:1");
        claim(&connection, &job.id).unwrap().unwrap();
        room_moves_past(&connection, 2, "partial-answer");
        assert_eq!(recover_after_restart(&connection).unwrap().superseded, 1);

        // Same key, same job: replaying a dispatch stays a no-op rather than
        // becoming a second run of the same turn.
        let replayed = enqueue_default(&connection, "would-be-duplicate", "orch-dispatch:exec-1:1");
        assert_eq!(
            replayed.id, "stale-job",
            "an idempotent replay must not spawn a second job",
        );
        assert_eq!(replayed.status, DispatchStatus::Cancelled);

        // The next attempt is a different key, and it queues normally: the
        // retirement closes the stale turn without blocking the live one.
        let next_attempt = enqueue_default(&connection, "attempt-2", "orch-dispatch:exec-1:2");
        assert_eq!(next_attempt.id, "attempt-2");
        assert_eq!(next_attempt.status, DispatchStatus::Pending);
    }

    #[test]
    fn explicit_retry_copies_only_the_failed_target_and_is_idempotent() {
        let connection = connection();
        let failed = enqueue(
            &connection,
            NewAgentDispatchJob {
                id: "j-lite-failed",
                discussion_id: "d1",
                trigger_message_id: "u1",
                trigger_sort_order: 1,
                dedupe_key: "turn-1-lite",
                agent_override: Some(&AgentType::LiteLlm),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap();
        mark_failed(&connection, &failed.id, "vpn unavailable").unwrap();

        let (retry, duplicate) = enqueue_retry(
            &connection,
            "d1",
            "j-lite-failed",
            "click-1",
            "j-lite-retry",
        )
        .unwrap();
        assert!(!duplicate);
        assert_eq!(retry.trigger_message_id, "u1");
        assert_eq!(retry.agent_override, Some(AgentType::LiteLlm));

        let (same, duplicate) = enqueue_retry(
            &connection,
            "d1",
            "j-lite-failed",
            "click-1",
            "must-not-be-inserted",
        )
        .unwrap();
        assert!(duplicate);
        assert_eq!(same.id, "j-lite-retry");
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs WHERE trigger_message_id = 'u1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2, "failed LiteLLM + its one retry only");
    }

    #[test]
    fn retry_marks_the_linked_agent_error_without_touching_its_diagnostics() {
        let connection = connection();
        let now = Utc::now().to_rfc3339();
        let content = concat!(
            "[kronn:agent-error]\n",
            r#"{"kind":"agent_error","summary":"LiteLLM indisponible","detail":"VPN requis","retry_dispatch_id":"j-lite-failed","retried":false}"#
        );
        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at,
                  agent_dispatch_job_id)
                 VALUES ('m-lite-error', 'd1', 'System', ?1, ?2, 2, ?2, 'j-lite-failed')",
                params![content, now],
            )
            .unwrap();

        mark_error_retried(&connection, "j-lite-failed").unwrap();

        let updated: String = connection
            .query_row(
                "SELECT content FROM messages WHERE id = 'm-lite-error'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(
            updated
                .strip_prefix("[kronn:agent-error]\n")
                .expect("structured agent error marker"),
        )
        .unwrap();
        assert_eq!(payload["retried"], true);
        assert_eq!(payload["detail"], "VPN requis");
        assert_eq!(payload["retry_dispatch_id"], "j-lite-failed");
    }

    #[test]
    fn retry_freezes_the_failed_primary_agent_from_the_error_message() {
        let connection = connection();
        let failed = enqueue_default(&connection, "j-primary-failed", "turn-primary");
        mark_failed(&connection, &failed.id, "vpn unavailable").unwrap();
        let now = Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, agent_type, timestamp, sort_order,
                  received_at, agent_dispatch_job_id)
                 VALUES ('m-primary-error', 'd1', 'System', '[kronn:agent-error]',
                         'LiteLlm', ?1, 2, ?1, 'j-primary-failed')",
                [&now],
            )
            .unwrap();
        connection
            .execute("UPDATE discussions SET agent = 'Codex' WHERE id = 'd1'", [])
            .unwrap();

        let (retry, duplicate) = enqueue_retry(
            &connection,
            "d1",
            "j-primary-failed",
            "click-primary",
            "j-primary-retry",
        )
        .unwrap();

        assert!(!duplicate);
        assert_eq!(retry.agent_override, Some(AgentType::LiteLlm));
    }

    #[test]
    fn enqueue_is_idempotent_and_claim_is_exclusive() {
        let connection = connection();
        let first = enqueue_default(&connection, "j1", "message:u1");
        let duplicate = enqueue_default(&connection, "j2", "message:u1");
        assert_eq!(first.id, "j1");
        assert_eq!(duplicate.id, "j1");

        let claimed = claim(&connection, "j1").unwrap().unwrap();
        assert_eq!(claimed.status, DispatchStatus::Running);
        assert_eq!(claimed.attempts, 1);
        assert!(claim(&connection, "j1").unwrap().is_none());
    }

    /// A second discussion, so a per-agent cap can be tested for what it is:
    /// a limit across the whole instance, not the existing one-run-per-room rule.
    fn seed_second_discussion(connection: &Connection) {
        let now = Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('d2', 'Dispatch 2', ?1, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES ('u2', 'd2', 'User', 'go', ?1, 1, ?1)",
                [&now],
            )
            .unwrap();
    }

    fn enqueue_for(connection: &Connection, id: &str, discussion: &str, agent: &AgentType) {
        enqueue(
            connection,
            NewAgentDispatchJob {
                id,
                discussion_id: discussion,
                trigger_message_id: if discussion == "d1" { "u1" } else { "u2" },
                trigger_sort_order: 1,
                dedupe_key: id,
                agent_override: Some(agent),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn claim_refuses_a_second_run_of_an_agent_already_at_its_limit() {
        let connection = connection();
        seed_second_discussion(&connection);
        enqueue_for(&connection, "j-a", "d1", &AgentType::Ollama);
        enqueue_for(&connection, "j-b", "d2", &AgentType::Ollama);

        let limits = r#"{"Ollama":1}"#;
        assert!(claim_with_limits(&connection, "j-a", Some(limits))
            .unwrap()
            .is_some());
        assert!(
            claim_with_limits(&connection, "j-b", Some(limits))
                .unwrap()
                .is_none(),
            "the local model serves one slot: a second run must wait, not queue on it"
        );
    }

    #[test]
    fn claim_leaves_an_agent_absent_from_the_map_uncapped() {
        let connection = connection();
        seed_second_discussion(&connection);
        enqueue_for(&connection, "j-a", "d1", &AgentType::LiteLlm);
        enqueue_for(&connection, "j-b", "d2", &AgentType::LiteLlm);

        // Remote providers are deliberately left out of the map.
        let limits = r#"{"Ollama":1}"#;
        assert!(claim_with_limits(&connection, "j-a", Some(limits))
            .unwrap()
            .is_some());
        assert!(
            claim_with_limits(&connection, "j-b", Some(limits))
                .unwrap()
                .is_some(),
            "a remote endpoint someone else scales must not be held back"
        );
    }

    #[test]
    fn claim_counts_every_agent_on_its_own() {
        let connection = connection();
        seed_second_discussion(&connection);
        enqueue_for(&connection, "j-a", "d1", &AgentType::Ollama);
        enqueue_for(&connection, "j-b", "d2", &AgentType::ClaudeCode);

        let limits = r#"{"Ollama":1,"ClaudeCode":1}"#;
        assert!(claim_with_limits(&connection, "j-a", Some(limits))
            .unwrap()
            .is_some());
        assert!(
            claim_with_limits(&connection, "j-b", Some(limits))
                .unwrap()
                .is_some(),
            "one agent at its cap must not block a different one"
        );
    }

    #[test]
    fn claim_caps_the_sum_of_different_local_agent_families() {
        let connection = connection();
        seed_second_discussion(&connection);
        enqueue_for(&connection, "j-a", "d1", &AgentType::Ollama);
        enqueue_for(&connection, "j-b", "d2", &AgentType::ClaudeCode);

        let limits = r#"{"Ollama":5,"ClaudeCode":5,"__local_global":1}"#;
        assert!(claim_with_limits(&connection, "j-a", Some(limits))
            .unwrap()
            .is_some());
        assert!(
            claim_with_limits(&connection, "j-b", Some(limits))
                .unwrap()
                .is_none(),
            "different local families must still share the machine-wide pool"
        );
    }

    #[test]
    fn remote_agents_neither_consume_nor_obey_the_local_global_pool() {
        let limits = r#"{"Ollama":5,"ClaudeCode":5,"__local_global":1}"#;

        let remote_first = connection();
        seed_second_discussion(&remote_first);
        enqueue_for(&remote_first, "j-remote", "d1", &AgentType::LiteLlm);
        enqueue_for(&remote_first, "j-local", "d2", &AgentType::Ollama);
        assert!(claim_with_limits(&remote_first, "j-remote", Some(limits))
            .unwrap()
            .is_some());
        assert!(claim_with_limits(&remote_first, "j-local", Some(limits))
            .unwrap()
            .is_some());

        let local_first = connection();
        seed_second_discussion(&local_first);
        enqueue_for(&local_first, "j-local", "d1", &AgentType::Ollama);
        enqueue_for(&local_first, "j-remote", "d2", &AgentType::Nvidia);
        assert!(claim_with_limits(&local_first, "j-local", Some(limits))
            .unwrap()
            .is_some());
        assert!(
            claim_with_limits(&local_first, "j-remote", Some(limits))
                .unwrap()
                .is_some(),
            "a remote candidate must bypass a full local pool"
        );
    }

    #[test]
    fn active_dispatches_keep_their_turn_and_concrete_agent() {
        let connection = connection();
        enqueue_default(&connection, "j-default", "message:u1");
        enqueue(
            &connection,
            NewAgentDispatchJob {
                id: "j-ollama",
                discussion_id: "d1",
                trigger_message_id: "u1",
                trigger_sort_order: 1,
                dedupe_key: "message:u1:Ollama",
                agent_override: Some(&AgentType::Ollama),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap();
        claim(&connection, "j-default").unwrap();

        let active = list_active_for_discussion(&connection, "d1", &AgentType::LiteLlm).unwrap();
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].trigger_message_id, "u1");
        assert_eq!(active[0].agent_type, AgentType::LiteLlm);
        assert_eq!(active[0].status, "Running");
        assert_eq!(active[1].agent_type, AgentType::Ollama);
        assert_eq!(active[1].status, "Pending");
    }

    #[test]
    fn pending_jobs_queue_and_only_one_claims_per_discussion() {
        let connection = connection();
        enqueue_default(&connection, "j1", "message:u1");
        let second = enqueue_default(&connection, "j2", "force:j2");
        assert_eq!(second.id, "j2");

        assert!(claim(&connection, "j1").unwrap().is_some());
        assert!(claim(&connection, "j2").unwrap().is_none());
        assert!(mark_completed(&connection, "j1").unwrap());
        assert!(claim(&connection, "j2").unwrap().is_some());
    }

    #[test]
    fn cancelling_one_dispatch_preserves_same_agent_siblings() {
        let connection = connection();
        enqueue_default(&connection, "j-old", "message:u-old");
        enqueue_default(&connection, "j-new", "message:u-new");
        enqueue_default(&connection, "j-latest", "message:u-latest");
        assert!(claim(&connection, "j-old").unwrap().is_some());

        assert!(cancel_for_discussion_by_id(&connection, "d1", "j-old").unwrap());
        assert_eq!(
            get(&connection, "j-old").unwrap().unwrap().status,
            DispatchStatus::Cancelled
        );
        assert_eq!(
            get(&connection, "j-new").unwrap().unwrap().status,
            DispatchStatus::Pending
        );
        assert_eq!(
            get(&connection, "j-latest").unwrap().unwrap().status,
            DispatchStatus::Pending
        );
        assert!(has_active_for_discussion(&connection, "d1").unwrap());

        assert!(
            !cancel_for_discussion_by_id(&connection, "another-disc", "j-new").unwrap(),
            "a dispatch id is not a cross-discussion cancellation capability"
        );
        assert_eq!(
            get(&connection, "j-new").unwrap().unwrap().status,
            DispatchStatus::Pending
        );
    }

    #[test]
    fn claim_retires_a_queued_job_when_native_agent_is_disabled() {
        let connection = connection();
        enqueue_default(&connection, "j1", "message:u1");
        connection
            .execute(
                "UPDATE discussions
                 SET no_agent = 1, awaiting_agent = 1
                 WHERE id = 'd1'",
                [],
            )
            .unwrap();

        assert_eq!(
            list_runnable_ids(&connection, 10).unwrap(),
            vec!["j1".to_string()],
            "disabled pending work stays visible only long enough for claim to retire it"
        );
        assert!(claim(&connection, "j1").unwrap().is_none());
        let job = get(&connection, "j1").unwrap().unwrap();
        assert_eq!(job.status, DispatchStatus::Cancelled);
        assert_eq!(job.last_error.as_deref(), Some("agent_disabled"));
        assert!(list_runnable_ids(&connection, 10).unwrap().is_empty());
        assert_eq!(
            connection
                .query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = 'd1'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn lifecycle_timestamps_distinguish_queue_claim_start_and_settlement() {
        let connection = connection();
        let job = enqueue(
            &connection,
            NewAgentDispatchJob {
                id: "lifecycle-job",
                discussion_id: "d1",
                trigger_message_id: "u1",
                trigger_sort_order: 1,
                dedupe_key: "lifecycle-key",
                agent_override: None,
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap();
        assert!(job.claimed_at.is_none());
        assert!(job.agent_started_at.is_none());
        assert!(job.completed_at.is_none());

        let claimed = claim(&connection, "lifecycle-job").unwrap().unwrap();
        assert!(claimed.claimed_at.is_some());
        assert!(claimed.agent_started_at.is_none());
        assert!(mark_agent_started(&connection, "lifecycle-job").unwrap());
        let launching = get(&connection, "lifecycle-job").unwrap().unwrap();
        assert_eq!(launching.progress_phase.as_deref(), Some("launching"));
        assert!(mark_progress(&connection, "lifecycle-job", "upstream_wait", None).unwrap());
        let upstream = get(&connection, "lifecycle-job").unwrap().unwrap();
        assert_eq!(upstream.progress_phase.as_deref(), Some("upstream_wait"));
        assert!(mark_completed(&connection, "lifecycle-job").unwrap());

        let settled = get(&connection, "lifecycle-job").unwrap().unwrap();
        assert!(settled.created_at <= settled.claimed_at.unwrap());
        assert!(settled.agent_started_at.is_some());
        assert!(settled.completed_at >= settled.agent_started_at);
        assert_eq!(settled.status, DispatchStatus::Completed);
    }

    #[test]
    fn disabled_exhausted_job_is_retired_instead_of_reported_failed() {
        let connection = connection();
        enqueue_default(&connection, "j1", "message:u1");
        connection
            .execute(
                "UPDATE agent_dispatch_jobs SET attempts = ?1 WHERE id = 'j1'",
                [i64::from(MAX_DISPATCH_ATTEMPTS)],
            )
            .unwrap();
        connection
            .execute("UPDATE discussions SET no_agent = 1 WHERE id = 'd1'", [])
            .unwrap();

        assert!(list_exhausted_ids(&connection, 10).unwrap().is_empty());
        assert_eq!(
            list_runnable_ids(&connection, 10).unwrap(),
            vec!["j1".to_string()]
        );
        assert!(claim(&connection, "j1").unwrap().is_none());
        assert_eq!(
            get(&connection, "j1").unwrap().unwrap().status,
            DispatchStatus::Cancelled
        );
    }

    #[test]
    fn disabling_native_agent_cancels_every_pending_job_and_clears_awaiting() {
        let connection = connection();
        enqueue_default(&connection, "j1", "message:u1");
        enqueue_default(&connection, "j2", "force:j2");
        crate::db::discussions::set_awaiting_agent(&connection, "d1", true).unwrap();

        assert!(crate::db::discussions::set_disc_no_agent(&connection, "d1", true).unwrap());
        for id in ["j1", "j2"] {
            let job = get(&connection, id).unwrap().unwrap();
            assert_eq!(job.status, DispatchStatus::Cancelled);
            assert_eq!(job.last_error.as_deref(), Some("agent_disabled"));
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = 'd1'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn disabling_native_agent_rolls_back_the_flag_when_queue_retirement_fails() {
        let connection = connection();
        enqueue_default(&connection, "j1", "message:u1");
        crate::db::discussions::set_awaiting_agent(&connection, "d1", true).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_agent_disabled_cancellation
                 BEFORE UPDATE OF status ON agent_dispatch_jobs
                 WHEN NEW.status = 'Cancelled'
                 BEGIN
                     SELECT RAISE(ABORT, 'injected cancellation failure');
                 END;",
            )
            .unwrap();

        assert!(
            crate::db::discussions::set_disc_no_agent(&connection, "d1", true).is_err(),
            "the setter must surface a queue-retirement failure"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT no_agent, awaiting_agent FROM discussions WHERE id = 'd1'",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            (0, 1),
            "the mode and awaiting flag must roll back together"
        );
        assert_eq!(
            get(&connection, "j1").unwrap().unwrap().status,
            DispatchStatus::Pending
        );
    }

    #[test]
    fn restart_requeues_running_job_without_losing_attempt_count() {
        let connection = connection();
        enqueue_default(&connection, "j1", "message:u1");
        claim(&connection, "j1").unwrap().unwrap();

        assert_eq!(reset_running_after_restart(&connection).unwrap(), 1);
        let recovered = get(&connection, "j1").unwrap().unwrap();
        assert_eq!(recovered.status, DispatchStatus::Pending);
        assert_eq!(recovered.attempts, 1);
        assert_eq!(recovered.last_error.as_deref(), Some("backend_restarted"));
        assert!(claim(&connection, "j1").unwrap().is_some());
    }

    #[test]
    fn restart_never_redispatches_children_of_an_interrupted_workflow() {
        let connection = connection();
        connection
            .execute_batch(
                "INSERT INTO workflows (
                     id, name, trigger_json, steps_json, actions_json,
                     safety_json, enabled, created_at, updated_at
                 ) VALUES (
                     'wf-parent', 'Parent', '\"Manual\"', '[]', '[]', '{}', 1, 'now', 'now'
                 );
                 INSERT INTO workflows (
                     id, name, trigger_json, steps_json, actions_json,
                     safety_json, enabled, created_at, updated_at
                 ) VALUES (
                     'wf-batch', 'Batch', '\"Manual\"', '[]', '[]', '{}', 1, 'now', 'now'
                 );
                 INSERT INTO workflow_runs (
                     id, workflow_id, status, step_results_json, tokens_used,
                     started_at, finished_at, run_type, batch_total,
                     batch_completed, batch_failed
                 ) VALUES (
                     'parent-run', 'wf-parent', 'Interrupted', '[]', 0,
                     'now', 'now', 'linear', 0, 0, 0
                 );
                 INSERT INTO workflow_runs (
                     id, workflow_id, status, step_results_json, tokens_used,
                     started_at, finished_at, run_type, batch_total,
                     batch_completed, batch_failed, parent_run_id
                 ) VALUES (
                     'batch-run', 'wf-batch', 'Interrupted', '[]', 0,
                     'now', 'now', 'batch', 2, 0, 0, 'parent-run'
                 );
                 UPDATE discussions
                    SET workflow_run_id = 'batch-run', awaiting_agent = 1
                  WHERE id = 'd1';",
            )
            .unwrap();
        enqueue(
            &connection,
            NewAgentDispatchJob {
                id: "running-child",
                discussion_id: "d1",
                trigger_message_id: "u1",
                trigger_sort_order: 1,
                dedupe_key: "batch-child-running",
                agent_override: None,
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: Some("batch-run"),
                group_concurrency_limit: Some(2),
            },
        )
        .unwrap();
        claim(&connection, "running-child").unwrap().unwrap();
        enqueue(
            &connection,
            NewAgentDispatchJob {
                id: "queued-child",
                discussion_id: "d1",
                trigger_message_id: "u1",
                trigger_sort_order: 1,
                dedupe_key: "batch-child-queued",
                agent_override: None,
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: Some("batch-run"),
                group_concurrency_limit: Some(2),
            },
        )
        .unwrap();

        let recovery = recover_after_restart(&connection).unwrap();

        assert_eq!(recovery.requeued, 0);
        assert_eq!(recovery.cancelled_workflow_children, 2);
        let child = get(&connection, "running-child").unwrap().unwrap();
        assert_eq!(child.status, DispatchStatus::Cancelled);
        assert_eq!(
            child.last_error.as_deref(),
            Some("parent_workflow_interrupted")
        );
        assert_eq!(child.attempts, 1, "the spent attempt remains observable");
        let queued = get(&connection, "queued-child").unwrap().unwrap();
        assert_eq!(queued.status, DispatchStatus::Cancelled);
        assert_eq!(queued.attempts, 0, "queued work never spent an attempt");
        let metrics: String = connection
            .query_row(
                "SELECT state FROM workflow_runs WHERE id = 'batch-run'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let metrics: serde_json::Value = serde_json::from_str(&metrics).unwrap();
        assert_eq!(metrics["dispatch_attempts"], 1);
        assert_eq!(metrics["redispatches"], 0);
        assert_eq!(metrics["restart_cancelled_children"], 2);
        assert!(list_runnable_ids(&connection, 10).unwrap().is_empty());
        assert_eq!(
            connection
                .query_row(
                    "SELECT awaiting_agent FROM discussions WHERE id = 'd1'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn dropped_handoff_releases_claim_without_spending_an_attempt() {
        let connection = connection();
        enqueue_default(&connection, "j1", "message:u1");
        claim(&connection, "j1").unwrap().unwrap();

        assert!(release_unstarted_claim(&connection, "j1", "handoff").unwrap());
        let released = get(&connection, "j1").unwrap().unwrap();
        assert_eq!(released.status, DispatchStatus::Pending);
        assert_eq!(released.attempts, 0);
        assert_eq!(released.turn_attempts, 0);
        let reclaimed = claim(&connection, "j1").unwrap().unwrap();
        assert_eq!(reclaimed.attempts, 1);
        assert_eq!(reclaimed.turn_attempts, 1);
    }

    #[test]
    fn unavailable_runtime_defers_without_spending_an_attempt() {
        let connection = connection();
        enqueue_default(&connection, "j1", "message:u1");
        claim(&connection, "j1").unwrap().unwrap();

        assert!(defer_runtime_unavailable(&connection, "j1", 30, "runtime_unavailable").unwrap());
        let deferred = get(&connection, "j1").unwrap().unwrap();
        assert_eq!(deferred.status, DispatchStatus::Pending);
        assert_eq!(deferred.attempts, 0);
        assert_eq!(deferred.turn_attempts, 0);
        assert!(deferred.claimed_at.is_none());
        assert_eq!(deferred.last_error.as_deref(), Some("runtime_unavailable"));
        assert!(
            deferred.available_at > Utc::now(),
            "the dispatcher must not hot-loop while the runtime is absent"
        );
        assert!(
            claim(&connection, "j1").unwrap().is_none(),
            "the job must remain delayed until available_at"
        );
    }

    #[test]
    fn group_limit_prevents_claim_until_a_sibling_finishes() {
        let connection = connection();
        let now = Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('d2', 'Dispatch 2', ?1, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES ('u2', 'd2', 'User', 'go', ?1, 1, ?1)",
                [&now],
            )
            .unwrap();
        let empty = Vec::new();
        enqueue(
            &connection,
            NewAgentDispatchJob {
                id: "j1",
                discussion_id: "d1",
                trigger_message_id: "u1",
                trigger_sort_order: 1,
                dedupe_key: "message:u1",
                agent_override: None,
                chain_prompt_ids: &empty,
                batch_item: None,
                group_id: Some("batch"),
                group_concurrency_limit: Some(1),
            },
        )
        .unwrap();
        enqueue(
            &connection,
            NewAgentDispatchJob {
                id: "j2",
                discussion_id: "d2",
                trigger_message_id: "u2",
                trigger_sort_order: 1,
                dedupe_key: "message:u2",
                agent_override: None,
                chain_prompt_ids: &empty,
                batch_item: None,
                group_id: Some("batch"),
                group_concurrency_limit: Some(1),
            },
        )
        .unwrap();

        assert!(claim(&connection, "j1").unwrap().is_some());
        assert!(claim(&connection, "j2").unwrap().is_none());
        assert!(mark_completed(&connection, "j1").unwrap());
        assert!(claim(&connection, "j2").unwrap().is_some());
    }

    #[test]
    fn completion_lookup_ignores_recovered_and_unclassified_agent_messages() {
        let connection = connection();
        let job = enqueue_default(&connection, "j1", "message:u1");
        let now = Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at,
                  recovered_partial)
                 VALUES ('a-recovered', 'd1', 'Agent', 'partial', ?1, 2, ?1, 1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at)
                 VALUES ('a-legacy', 'd1', 'Agent', 'unknown', ?1, 3, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at,
                  agent_run_succeeded, agent_dispatch_job_id)
                 VALUES ('a-other-job', 'd1', 'Agent', 'previous turn', ?1, 4, ?1, 1, 'j0')",
                [&now],
            )
            .unwrap();
        assert!(latest_completed_agent_message(&connection, &job)
            .unwrap()
            .is_none());

        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order, received_at,
                  agent_run_succeeded, agent_dispatch_job_id)
                 VALUES ('a-durable', 'd1', 'Agent', 'failed reply', ?1, 5, ?1, 0, 'j1')",
                [&now],
            )
            .unwrap();
        let completion = latest_completed_agent_message(&connection, &job)
            .unwrap()
            .unwrap();
        assert_eq!(completion.0, "a-durable");
        assert_eq!(completion.1, "failed reply");
        assert!(!completion.2);
    }

    #[test]
    fn exhausted_jobs_leave_the_runnable_queue() {
        let connection = connection();
        enqueue_default(&connection, "j1", "message:u1");
        connection
            .execute(
                "UPDATE agent_dispatch_jobs SET attempts = ?1 WHERE id = 'j1'",
                [i64::from(MAX_DISPATCH_ATTEMPTS)],
            )
            .unwrap();

        assert!(list_runnable_ids(&connection, 10).unwrap().is_empty());
        assert_eq!(
            list_exhausted_ids(&connection, 10).unwrap(),
            vec!["j1".to_string()]
        );
    }

    #[test]
    fn watchdog_redispatches_once_then_escalates_durably() {
        let connection = connection();
        enqueue_default(&connection, "j1", "message:u1");
        claim(&connection, "j1").unwrap();
        mark_agent_started(&connection, "j1").unwrap();
        connection
            .execute(
                "UPDATE agent_dispatch_jobs SET agent_started_at = ?1 WHERE id = 'j1'",
                [(Utc::now() - Duration::minutes(5)).to_rfc3339()],
            )
            .unwrap();
        assert_eq!(
            list_watchdog_candidates(&connection, Utc::now())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            apply_watchdog_stall(&connection, "j1").unwrap(),
            WatchdogTransition::Redispatched
        );
        let first = get(&connection, "j1").unwrap().unwrap();
        assert_eq!(first.status, DispatchStatus::Pending);
        assert_eq!(first.watchdog_redispatches, 1);

        connection
            .execute(
                "UPDATE agent_dispatch_jobs SET available_at = ?1 WHERE id = 'j1'",
                [Utc::now().to_rfc3339()],
            )
            .unwrap();
        claim(&connection, "j1").unwrap();
        mark_agent_started(&connection, "j1").unwrap();
        assert_eq!(
            apply_watchdog_stall(&connection, "j1").unwrap(),
            WatchdogTransition::Escalated
        );
        let second = get(&connection, "j1").unwrap().unwrap();
        assert_eq!(second.status, DispatchStatus::Failed);
        assert_eq!(second.failure_kind.as_deref(), Some("dispatch_stalled"));
    }

    #[test]
    fn quota_exhaustion_is_terminal_and_never_a_watchdog_candidate() {
        let connection = connection();
        enqueue_default(&connection, "j1", "message:u1");
        claim(&connection, "j1").unwrap();
        mark_agent_started(&connection, "j1").unwrap();
        assert!(mark_failed_with_kind(
            &connection,
            "j1",
            "provider plan exhausted",
            "quota_exhausted",
        )
        .unwrap());
        let job = get(&connection, "j1").unwrap().unwrap();
        assert_eq!(job.failure_kind.as_deref(), Some("quota_exhausted"));
        assert!(list_watchdog_candidates(&connection, Utc::now())
            .unwrap()
            .is_empty());
    }
}
