use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::core::quick_exec::QuickExecResult;
use crate::core::quick_exec::QuickExecSpec;
use crate::models::{
    AgentResumeFailureKind, AgentResumeJobKind, AgentResumeJobStatus, AgentResumeJobView, AgentType,
};

use super::parse_dt;

const COLUMNS: &str = "id, discussion_id, target_agent_json,
    source_dispatch_job_id, task_execution_id, quick_exec_id, kind, status,
    dedupe_key, reason, command_spec_json, result_json, failure_kind,
    scheduled_at, chain_depth, wake_budget, watchdog_redispatches,
    completion_dispatch_id, started_at, completed_at, last_error, created_at, updated_at";

#[derive(Debug, Clone)]
pub struct AgentResumeJobRecord {
    pub view: AgentResumeJobView,
    pub dedupe_key: String,
    pub command_spec: Option<QuickExecSpec>,
}

pub struct NewAgentResumeJob<'a> {
    pub id: &'a str,
    pub discussion_id: &'a str,
    pub target_agent: &'a AgentType,
    pub source_dispatch_job_id: Option<&'a str>,
    pub task_execution_id: Option<&'a str>,
    pub quick_exec_id: Option<&'a str>,
    pub kind: AgentResumeJobKind,
    pub dedupe_key: &'a str,
    pub reason: &'a str,
    pub command_spec: Option<&'a QuickExecSpec>,
    pub scheduled_at: DateTime<Utc>,
    pub chain_depth: u32,
    pub wake_budget: u32,
}

fn kind_db(kind: AgentResumeJobKind) -> &'static str {
    match kind {
        AgentResumeJobKind::Command => "Command",
        AgentResumeJobKind::Wake => "Wake",
    }
}

fn parse_kind(value: &str) -> Result<AgentResumeJobKind> {
    match value {
        "Command" => Ok(AgentResumeJobKind::Command),
        "Wake" => Ok(AgentResumeJobKind::Wake),
        other => anyhow::bail!("unknown agent resume job kind: {other}"),
    }
}

fn status_db(status: AgentResumeJobStatus) -> &'static str {
    match status {
        AgentResumeJobStatus::Pending => "Pending",
        AgentResumeJobStatus::Running => "Running",
        AgentResumeJobStatus::Completed => "Completed",
        AgentResumeJobStatus::Failed => "Failed",
        AgentResumeJobStatus::Cancelled => "Cancelled",
        AgentResumeJobStatus::QuotaExhausted => "QuotaExhausted",
        AgentResumeJobStatus::Escalated => "Escalated",
    }
}

fn parse_status(value: &str) -> Result<AgentResumeJobStatus> {
    match value {
        "Pending" => Ok(AgentResumeJobStatus::Pending),
        "Running" => Ok(AgentResumeJobStatus::Running),
        "Completed" => Ok(AgentResumeJobStatus::Completed),
        "Failed" => Ok(AgentResumeJobStatus::Failed),
        "Cancelled" => Ok(AgentResumeJobStatus::Cancelled),
        "QuotaExhausted" => Ok(AgentResumeJobStatus::QuotaExhausted),
        "Escalated" => Ok(AgentResumeJobStatus::Escalated),
        other => anyhow::bail!("unknown agent resume job status: {other}"),
    }
}

fn failure_db(kind: AgentResumeFailureKind) -> &'static str {
    match kind {
        AgentResumeFailureKind::CommandFailed => "command_failed",
        AgentResumeFailureKind::BackendRestarted => "backend_restarted",
        AgentResumeFailureKind::DispatchStalled => "dispatch_stalled",
        AgentResumeFailureKind::QuotaExhausted => "quota_exhausted",
        AgentResumeFailureKind::RuntimeUnavailable => "runtime_unavailable",
    }
}

fn parse_failure(value: &str) -> Result<AgentResumeFailureKind> {
    match value {
        "command_failed" => Ok(AgentResumeFailureKind::CommandFailed),
        "backend_restarted" => Ok(AgentResumeFailureKind::BackendRestarted),
        "dispatch_stalled" => Ok(AgentResumeFailureKind::DispatchStalled),
        "quota_exhausted" => Ok(AgentResumeFailureKind::QuotaExhausted),
        "runtime_unavailable" => Ok(AgentResumeFailureKind::RuntimeUnavailable),
        other => anyhow::bail!("unknown agent resume failure kind: {other}"),
    }
}

fn map(row: &Row<'_>) -> rusqlite::Result<AgentResumeJobRecord> {
    let conversion = |index: usize, error: anyhow::Error| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, error.into())
    };
    let target_json: String = row.get(2)?;
    let kind_raw: String = row.get(6)?;
    let status_raw: String = row.get(7)?;
    let spec_json: Option<String> = row.get(10)?;
    let result_json: Option<String> = row.get(11)?;
    let failure_raw: Option<String> = row.get(12)?;
    Ok(AgentResumeJobRecord {
        view: AgentResumeJobView {
            id: row.get(0)?,
            discussion_id: row.get(1)?,
            target_agent: serde_json::from_str(&target_json)
                .map_err(|error| conversion(2, error.into()))?,
            source_dispatch_job_id: row.get(3)?,
            task_execution_id: row.get(4)?,
            quick_exec_id: row.get(5)?,
            kind: parse_kind(&kind_raw).map_err(|error| conversion(6, error))?,
            status: parse_status(&status_raw).map_err(|error| conversion(7, error))?,
            reason: row.get(9)?,
            scheduled_at: parse_dt(row.get(13)?),
            chain_depth: row.get::<_, i64>(14)?.max(0) as u32,
            wake_budget: row.get::<_, i64>(15)?.max(1) as u32,
            watchdog_redispatches: row.get::<_, i64>(16)?.max(0) as u32,
            completion_dispatch_id: row.get(17)?,
            result: result_json
                .map(|json| {
                    serde_json::from_str(&json).map_err(|error| conversion(11, error.into()))
                })
                .transpose()?,
            failure_kind: failure_raw
                .map(|value| parse_failure(&value).map_err(|error| conversion(12, error)))
                .transpose()?,
            started_at: row.get::<_, Option<String>>(18)?.map(parse_dt),
            completed_at: row.get::<_, Option<String>>(19)?.map(parse_dt),
            last_error: row.get(20)?,
            created_at: parse_dt(row.get(21)?),
            updated_at: parse_dt(row.get(22)?),
        },
        dedupe_key: row.get(8)?,
        command_spec: spec_json
            .map(|json| serde_json::from_str(&json).map_err(|error| conversion(10, error.into())))
            .transpose()?,
    })
}

pub fn create(conn: &Connection, new: NewAgentResumeJob<'_>) -> Result<AgentResumeJobRecord> {
    let now = Utc::now().to_rfc3339();
    let target_agent = serde_json::to_string(new.target_agent)?;
    let spec = new.command_spec.map(serde_json::to_string).transpose()?;
    conn.execute(
        "INSERT INTO agent_resume_jobs (
             id, discussion_id, target_agent_json, source_dispatch_job_id,
             task_execution_id, quick_exec_id, kind, status, dedupe_key, reason,
             command_spec_json, scheduled_at, chain_depth, wake_budget,
             created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'Pending', ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)
         ON CONFLICT(dedupe_key) DO NOTHING",
        params![
            new.id,
            new.discussion_id,
            target_agent,
            new.source_dispatch_job_id,
            new.task_execution_id,
            new.quick_exec_id,
            kind_db(new.kind),
            new.dedupe_key,
            new.reason,
            spec,
            new.scheduled_at.to_rfc3339(),
            i64::from(new.chain_depth),
            i64::from(new.wake_budget.clamp(1, 10)),
            now,
        ],
    )?;
    get_by_dedupe(conn, new.dedupe_key)?.context("agent resume job was not persisted")
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<AgentResumeJobRecord>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM agent_resume_jobs WHERE id = ?1"),
        [id],
        map,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_by_dedupe(conn: &Connection, key: &str) -> Result<Option<AgentResumeJobRecord>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM agent_resume_jobs WHERE dedupe_key = ?1"),
        [key],
        map,
    )
    .optional()
    .map_err(Into::into)
}

pub fn find_by_completion_dispatch(
    conn: &Connection,
    dispatch_id: &str,
) -> Result<Option<AgentResumeJobRecord>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM agent_resume_jobs WHERE completion_dispatch_id = ?1"),
        [dispatch_id],
        map,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_for_discussion(
    conn: &Connection,
    discussion_id: &str,
) -> Result<Vec<AgentResumeJobView>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM agent_resume_jobs
         WHERE discussion_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 100"
    ))?;
    let rows = statement.query_map([discussion_id], map)?;
    rows.map(|row| row.map(|record| record.view))
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn list_active_for_discussion(
    conn: &Connection,
    discussion_id: &str,
) -> Result<Vec<AgentResumeJobView>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM agent_resume_jobs
         WHERE discussion_id = ?1 AND status IN ('Pending', 'Running')
         ORDER BY scheduled_at, created_at, id"
    ))?;
    let rows = statement.query_map([discussion_id], map)?;
    rows.map(|row| row.map(|record| record.view))
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn list_runnable_ids(conn: &Connection, limit: usize) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT id FROM agent_resume_jobs
         WHERE status = 'Pending' AND scheduled_at <= ?1
         ORDER BY scheduled_at, created_at, id LIMIT ?2",
    )?;
    let rows = statement.query_map(
        params![Utc::now().to_rfc3339(), limit.max(1) as i64],
        |row| row.get::<_, String>(0),
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn claim(conn: &Connection, id: &str) -> Result<Option<AgentResumeJobRecord>> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_resume_jobs
         SET status = 'Running', started_at = COALESCE(started_at, ?2), updated_at = ?2
         WHERE id = ?1 AND status = 'Pending' AND scheduled_at <= ?2",
        params![id, now],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    get(conn, id)
}

pub fn recover_after_restart(conn: &Connection) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE agent_resume_jobs
         SET status = 'Pending', started_at = NULL, failure_kind = 'backend_restarted',
             last_error = 'backend_restarted', updated_at = ?1
         WHERE status = 'Running'",
        [now],
    )?;
    Ok(changed as u64)
}

pub fn release_after_error(conn: &Connection, id: &str, error: &str) -> Result<bool> {
    let available = Utc::now() + chrono::Duration::seconds(10);
    Ok(conn.execute(
        "UPDATE agent_resume_jobs
         SET status = 'Pending', started_at = NULL, scheduled_at = ?2,
             last_error = ?3, updated_at = ?4
         WHERE id = ?1 AND status = 'Running'",
        params![id, available.to_rfc3339(), error, Utc::now().to_rfc3339()],
    )? > 0)
}

pub fn cancel_for_discussion(conn: &Connection, discussion_id: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT id FROM agent_resume_jobs
         WHERE discussion_id = ?1 AND status IN ('Pending', 'Running')",
    )?;
    let ids = statement
        .query_map([discussion_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    conn.execute(
        "UPDATE agent_resume_jobs
         SET status = 'Cancelled', completed_at = ?2, updated_at = ?2
         WHERE discussion_id = ?1 AND status IN ('Pending', 'Running')",
        params![discussion_id, Utc::now().to_rfc3339()],
    )?;
    Ok(ids)
}

pub fn cancel(conn: &Connection, id: &str, discussion_id: &str) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE agent_resume_jobs
         SET status = 'Cancelled', completed_at = ?3, updated_at = ?3
         WHERE id = ?1 AND discussion_id = ?2 AND status IN ('Pending', 'Running')",
        params![id, discussion_id, Utc::now().to_rfc3339()],
    )? > 0)
}

pub struct SettleAgentResumeJob<'a> {
    pub id: &'a str,
    pub terminal_status: AgentResumeJobStatus,
    pub result: Option<&'a QuickExecResult>,
    pub failure_kind: Option<AgentResumeFailureKind>,
    pub last_error: Option<&'a str>,
    pub completion_dispatch_id: &'a str,
}

pub fn settle(conn: &Connection, input: SettleAgentResumeJob<'_>) -> Result<bool> {
    anyhow::ensure!(
        !input.terminal_status.is_active(),
        "agent resume job settlement requires a terminal status"
    );
    let now = Utc::now().to_rfc3339();
    let result_json = input.result.map(serde_json::to_string).transpose()?;
    Ok(conn.execute(
        "UPDATE agent_resume_jobs
         SET status = ?2, result_json = ?3, failure_kind = ?4, last_error = ?5,
             completion_dispatch_id = ?6, completed_at = ?7, updated_at = ?7
         WHERE id = ?1 AND status = 'Running'",
        params![
            input.id,
            status_db(input.terminal_status),
            result_json,
            input.failure_kind.map(failure_db),
            input.last_error,
            input.completion_dispatch_id,
            now,
        ],
    )? > 0)
}

/// Reflect a failure of the completion dispatch back onto the durable source
/// job so status readers do not mistake "command finished" for "agent resumed".
pub fn mark_completion_dispatch_failure(
    conn: &Connection,
    dispatch_id: &str,
    status: AgentResumeJobStatus,
    failure_kind: AgentResumeFailureKind,
    error: &str,
) -> Result<bool> {
    anyhow::ensure!(
        matches!(
            status,
            AgentResumeJobStatus::QuotaExhausted | AgentResumeJobStatus::Escalated
        ),
        "completion dispatch failure must be quota exhausted or escalated"
    );
    Ok(conn.execute(
        "UPDATE agent_resume_jobs
         SET status = ?2, failure_kind = ?3, last_error = ?4,
             completed_at = COALESCE(completed_at, ?5), updated_at = ?5
         WHERE completion_dispatch_id = ?1
           AND status IN ('Completed', 'Failed')",
        params![
            dispatch_id,
            status_db(status),
            failure_db(failure_kind),
            error,
            Utc::now().to_rfc3339(),
        ],
    )? > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::quick_exec::Summariser;
    use crate::db::migrations;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrations::run(&connection).unwrap();
        let now = Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('d1', 'Resume jobs', ?1, ?1)",
                [&now],
            )
            .unwrap();
        connection
    }

    fn command_spec() -> QuickExecSpec {
        QuickExecSpec {
            binary: "cargo".into(),
            argv: vec!["check".into()],
            cwd: std::path::PathBuf::from("/tmp/kronn-resume-test"),
            timeout_secs: Some(30),
            stdin: None,
            summariser: Summariser::CargoTest,
        }
    }

    fn create_command<'a>(
        connection: &Connection,
        id: &'a str,
        dedupe_key: &'a str,
        spec: &'a QuickExecSpec,
    ) -> AgentResumeJobRecord {
        create(
            connection,
            NewAgentResumeJob {
                id,
                discussion_id: "d1",
                target_agent: &AgentType::Ollama,
                source_dispatch_job_id: None,
                task_execution_id: None,
                quick_exec_id: None,
                kind: AgentResumeJobKind::Command,
                dedupe_key,
                reason: "validate backend",
                command_spec: Some(spec),
                scheduled_at: Utc::now(),
                chain_depth: 0,
                wake_budget: 3,
            },
        )
        .unwrap()
    }

    #[test]
    fn durable_job_is_idempotent_and_recovers_after_backend_restart() {
        let connection = connection();
        let spec = command_spec();
        let first = create_command(&connection, "job-1", "resume:d1:ollama:check", &spec);
        let replay = create_command(&connection, "job-2", "resume:d1:ollama:check", &spec);
        assert_eq!(
            first.view.id, replay.view.id,
            "dedupe must return the original row"
        );

        assert!(claim(&connection, "job-1").unwrap().is_some());
        assert_eq!(recover_after_restart(&connection).unwrap(), 1);
        let recovered = get(&connection, "job-1").unwrap().unwrap();
        assert_eq!(recovered.view.status, AgentResumeJobStatus::Pending);
        assert_eq!(
            recovered.view.failure_kind,
            Some(AgentResumeFailureKind::BackendRestarted)
        );
        assert_eq!(list_runnable_ids(&connection, 10).unwrap(), vec!["job-1"]);
    }

    #[test]
    fn cancelling_discussion_settles_every_active_resume_obligation() {
        let connection = connection();
        let spec = command_spec();
        create_command(&connection, "job-1", "resume:d1:ollama:one", &spec);
        create_command(&connection, "job-2", "resume:d1:ollama:two", &spec);
        claim(&connection, "job-1").unwrap();

        let mut cancelled = cancel_for_discussion(&connection, "d1").unwrap();
        cancelled.sort();
        assert_eq!(cancelled, vec!["job-1", "job-2"]);
        for id in cancelled {
            assert_eq!(
                get(&connection, &id).unwrap().unwrap().view.status,
                AgentResumeJobStatus::Cancelled
            );
        }
        assert!(list_runnable_ids(&connection, 10).unwrap().is_empty());
    }
}
