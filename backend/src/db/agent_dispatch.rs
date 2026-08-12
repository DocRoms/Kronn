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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
        created_at: parse_dt(row.get(19)?),
        updated_at: parse_dt(row.get(20)?),
    })
}

const JOB_COLUMNS: &str = "id, discussion_id, trigger_message_id, trigger_sort_order,
    dedupe_key, agent_override_json, chain_prompt_ids_json, next_chain_index,
    batch_item, group_id, group_concurrency_limit, status, attempts, turn_attempts,
    available_at, claimed_at, agent_started_at, completed_at, last_error, created_at, updated_at";

pub fn list_active_for_discussion(
    conn: &Connection,
    discussion_id: &str,
    default_agent: &AgentType,
) -> Result<Vec<ActiveAgentDispatch>> {
    let mut stmt = conn.prepare(
        "SELECT id, trigger_message_id, agent_override_json, status
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
    let now = Utc::now().to_rfc3339();
    let agent_json = new.agent_override.map(serde_json::to_string).transpose()?;
    let chain_json = serde_json::to_string(new.chain_prompt_ids)?;
    conn.execute(
        "INSERT INTO agent_dispatch_jobs
         (id, discussion_id, trigger_message_id, trigger_sort_order, dedupe_key,
          agent_override_json, chain_prompt_ids_json, batch_item, group_id,
          group_concurrency_limit, status, available_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'Pending', ?11, ?11, ?11)
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
    enqueue(
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
    )
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

pub fn claim(conn: &Connection, id: &str) -> Result<Option<AgentDispatchJob>> {
    let now = Utc::now().to_rfc3339();
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
             RETURNING {JOB_COLUMNS}"
        ),
        params![id, now, i64::from(MAX_DISPATCH_ATTEMPTS)],
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

pub fn reset_running_after_restart(conn: &Connection) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET status = 'Pending', claimed_at = NULL, agent_started_at = NULL,
             available_at = ?1, updated_at = ?1,
             last_error = 'backend_restarted'
         WHERE status = 'Running'",
        [now],
    )?;
    Ok(changed as u64)
}

/// Persist the boundary between queueing and real agent execution.
///
/// Returning `false` means cancellation or another terminal transition won
/// the race after claim; callers must then skip the provider invocation.
pub fn mark_agent_started(conn: &Connection, id: &str) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_dispatch_jobs
         SET agent_started_at = COALESCE(agent_started_at, ?2), updated_at = ?2
         WHERE id = ?1 AND status = 'Running'",
        params![id, now],
    )?;
    Ok(changed > 0)
}

pub fn mark_completed(conn: &Connection, id: &str) -> Result<bool> {
    set_terminal(conn, id, DispatchStatus::Completed, None)
}

pub fn mark_failed(conn: &Connection, id: &str, error: &str) -> Result<bool> {
    set_terminal(conn, id, DispatchStatus::Failed, Some(error))
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
}
