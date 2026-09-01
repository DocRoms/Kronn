use crate::models::{SharedRun, SharedRunKind, SharedRunStatus};
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Row};

fn kind(v: &SharedRunKind) -> &'static str {
    match v {
        SharedRunKind::QuickPrompt => "quick_prompt",
        SharedRunKind::QuickApi => "quick_api",
        SharedRunKind::QuickExec => "quick_exec",
        SharedRunKind::Workflow => "workflow",
        SharedRunKind::Media => "media",
    }
}
fn status(v: &SharedRunStatus) -> &'static str {
    match v {
        SharedRunStatus::PreflightFailed => "preflight_failed",
        SharedRunStatus::Queued => "queued",
        SharedRunStatus::Running => "running",
        SharedRunStatus::Success => "success",
        SharedRunStatus::Failed => "failed",
        SharedRunStatus::Cancelled => "cancelled",
        SharedRunStatus::Timeout => "timeout",
    }
}
fn parse_kind(v: String) -> rusqlite::Result<SharedRunKind> {
    match v.as_str() {
        "quick_prompt" => Ok(SharedRunKind::QuickPrompt),
        "quick_api" => Ok(SharedRunKind::QuickApi),
        "quick_exec" => Ok(SharedRunKind::QuickExec),
        "workflow" => Ok(SharedRunKind::Workflow),
        "media" => Ok(SharedRunKind::Media),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            format!("unknown shared run kind: {v}").into(),
        )),
    }
}
fn parse_status(v: String) -> rusqlite::Result<SharedRunStatus> {
    match v.as_str() {
        "preflight_failed" => Ok(SharedRunStatus::PreflightFailed),
        "queued" => Ok(SharedRunStatus::Queued),
        "running" => Ok(SharedRunStatus::Running),
        "success" => Ok(SharedRunStatus::Success),
        "failed" => Ok(SharedRunStatus::Failed),
        "cancelled" => Ok(SharedRunStatus::Cancelled),
        "timeout" => Ok(SharedRunStatus::Timeout),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            format!("unknown shared run status: {v}").into(),
        )),
    }
}
fn timestamp(r: &Row<'_>, index: usize) -> rusqlite::Result<Option<DateTime<Utc>>> {
    r.get::<_, Option<String>>(index)?
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|date| date.with_timezone(&Utc))
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        index,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
        })
        .transpose()
}
fn row(r: &Row<'_>) -> rusqlite::Result<SharedRun> {
    let result: Option<String> = r.get(9)?;
    Ok(SharedRun {
        id: r.get(0)?,
        kind: parse_kind(r.get(1)?)?,
        source_id: r.get(2)?,
        project_id: r.get(3)?,
        discussion_id: r.get(4)?,
        status: parse_status(r.get(5)?)?,
        started_at: timestamp(r, 6)?,
        finished_at: timestamp(r, 7)?,
        duration_ms: r.get::<_, Option<i64>>(8)?.map(|v| v.max(0) as u64),
        result: result.and_then(|v| serde_json::from_str(&v).ok()),
        diagnostic: r.get(10)?,
        created_at: timestamp(r, 11)?.ok_or_else(|| {
            rusqlite::Error::InvalidColumnType(11, "created_at".into(), rusqlite::types::Type::Null)
        })?,
        updated_at: timestamp(r, 12)?.ok_or_else(|| {
            rusqlite::Error::InvalidColumnType(12, "updated_at".into(), rusqlite::types::Type::Null)
        })?,
    })
}
pub fn upsert(conn: &Connection, run: &SharedRun) -> Result<()> {
    conn.execute(
        "INSERT INTO shared_runs(id,kind,source_id,project_id,discussion_id,status,started_at,finished_at,duration_ms,result_json,diagnostic,created_at,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
         ON CONFLICT(id) DO UPDATE SET
             project_id=excluded.project_id,
             discussion_id=excluded.discussion_id,
             status=excluded.status,
             started_at=excluded.started_at,
             finished_at=excluded.finished_at,
             duration_ms=excluded.duration_ms,
             result_json=excluded.result_json,
             diagnostic=excluded.diagnostic,
             updated_at=excluded.updated_at",
        params![
            run.id,
            kind(&run.kind),
            run.source_id,
            run.project_id,
            run.discussion_id,
            status(&run.status),
            run.started_at.map(|v| v.to_rfc3339()),
            run.finished_at.map(|v| v.to_rfc3339()),
            run.duration_ms.map(|v| v as i64),
            run.result.as_ref().map(|v| v.to_string()),
            run.diagnostic,
            run.created_at.to_rfc3339(),
            run.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}
pub fn get(conn: &Connection, id: &str) -> Result<Option<SharedRun>> {
    let mut s = conn.prepare(
        "SELECT id,kind,source_id,project_id,discussion_id,status,started_at,finished_at,duration_ms,result_json,diagnostic,created_at,updated_at
         FROM shared_runs WHERE id=?1",
    )?;
    Ok(s.query_row([id], row).optional()?)
}
pub fn list(
    conn: &Connection,
    kind_filter: Option<&str>,
    source_id: Option<&str>,
    project_id: Option<&str>,
    discussion_id: Option<&str>,
    limit: u32,
) -> Result<Vec<SharedRun>> {
    let mut statement = conn.prepare(
        "SELECT id,kind,source_id,project_id,discussion_id,status,started_at,finished_at,duration_ms,result_json,diagnostic,created_at,updated_at
         FROM shared_runs
         WHERE (?1 IS NULL OR kind=?1) AND (?2 IS NULL OR source_id=?2)
           AND (?3 IS NULL OR project_id=?3) AND (?4 IS NULL OR discussion_id=?4)
         ORDER BY created_at DESC LIMIT ?5",
    )?;
    let rows = statement.query_map(
        params![
            kind_filter,
            source_id,
            project_id,
            discussion_id,
            limit.min(200)
        ],
        row,
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn sync_workflow(conn: &Connection, run: &crate::models::WorkflowRun) -> Result<()> {
    let project_id: Option<String> = conn
        .query_row(
            "SELECT project_id FROM workflows WHERE id=?1",
            [&run.workflow_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let status = match run.status {
        crate::models::RunStatus::Pending => SharedRunStatus::Queued,
        crate::models::RunStatus::Running | crate::models::RunStatus::WaitingApproval => {
            SharedRunStatus::Running
        }
        crate::models::RunStatus::Success => SharedRunStatus::Success,
        crate::models::RunStatus::Cancelled => SharedRunStatus::Cancelled,
        crate::models::RunStatus::StoppedByGuard => SharedRunStatus::Timeout,
        crate::models::RunStatus::Partial
        | crate::models::RunStatus::Failed
        | crate::models::RunStatus::Interrupted => SharedRunStatus::Failed,
    };
    let now = Utc::now();
    let duration_ms = run
        .finished_at
        .map(|finished| (finished - run.started_at).num_milliseconds().max(0) as u64);
    let completed = run
        .step_results
        .iter()
        .filter(|step| {
            !matches!(
                step.status,
                crate::models::RunStatus::Pending
                    | crate::models::RunStatus::Running
                    | crate::models::RunStatus::WaitingApproval
            )
        })
        .count();
    let current = run
        .step_results
        .iter()
        .find(|step| {
            matches!(
                step.status,
                crate::models::RunStatus::Pending
                    | crate::models::RunStatus::Running
                    | crate::models::RunStatus::WaitingApproval
            )
        })
        .map(|step| step.step_name.clone());
    let shared = SharedRun {
        id: run.id.clone(),
        kind: SharedRunKind::Workflow,
        source_id: run.workflow_id.clone(),
        project_id,
        discussion_id: None,
        status,
        started_at: Some(run.started_at),
        finished_at: run.finished_at,
        duration_ms,
        result: Some(serde_json::json!({
            "progress": {
                "completed": completed,
                "total": run.step_results.len(),
                "current_label": current,
            },
            "steps": run.step_results,
        })),
        diagnostic: None,
        created_at: run.started_at,
        updated_at: now,
    };
    upsert(conn, &shared)
}
use rusqlite::OptionalExtension;

/// Projects a media job onto the shared run model consumed by RunStatusCard.
///
/// One kind, `Media`, with the modality carried in `result.modality` — the
/// execution family is identical for image and video, only the output differs.
/// `SharedRun.id` is the job id so the mapping is 1:1 and a restart cannot
/// produce a duplicate run.
///
/// `progress` is deliberately never set: the provider does not measure it, and
/// an invented percentage looks authoritative while being fiction. The
/// diagnostic carries the job's bounded error only — never a payload or a
/// signed URL.
pub fn media_run(job: &crate::db::media_jobs::MediaJob) -> SharedRun {
    use crate::models::{MediaPhase, MediaRunResult};

    let phase = match job.status {
        crate::models::MediaJobStatus::Pending if job.provider_job_id.is_none() => {
            MediaPhase::Submitting
        }
        crate::models::MediaJobStatus::Pending => MediaPhase::Polling,
        crate::models::MediaJobStatus::Running => MediaPhase::Downloading,
        _ => MediaPhase::Persisting,
    };

    let mut result = MediaRunResult::new(job.modality, phase);
    result.generation_id = job.provider_generation_id.clone();
    result.asset_id = job.context_file_id.clone();
    result.message_id = job.message_id.clone();
    result.cost_usd = job.cost.map(|cost| cost.cost_usd);
    result.is_byok = job.cost.map(|cost| cost.is_byok);
    result.width = job.rendered.width;
    result.height = job.rendered.height;
    result.media_duration_ms = job.rendered.duration_ms;

    let now = Utc::now();
    SharedRun {
        id: job.id.clone(),
        kind: SharedRunKind::Media,
        // No persisted media template exists yet, so the connection is the
        // closest stable source identity.
        source_id: job.connection_id.clone(),
        project_id: job.project_id.clone(),
        discussion_id: job.discussion_id.clone(),
        status: job.status.shared_run_status(),
        started_at: None,
        finished_at: None,
        duration_ms: None,
        result: serde_json::to_value(&result).ok(),
        diagnostic: job.last_error.clone(),
        created_at: now,
        updated_at: now,
    }
}

/// Convenience for call sites that already hold a connection and only need the
/// row written. Callers that must also notify live views use
/// `api::shared_runs::publish_media_job`, which cannot forget the broadcast.
pub fn sync_media(conn: &Connection, job: &crate::db::media_jobs::MediaJob) -> Result<()> {
    upsert(conn, &media_run(job))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trips_measured_run_without_inventing_progress() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE discussions(id TEXT PRIMARY KEY); CREATE TABLE projects(id TEXT PRIMARY KEY);")
            .unwrap();
        conn.execute_batch(include_str!("sql/155_shared_runs.sql"))
            .unwrap();
        let now = Utc::now();
        let expected = SharedRun {
            id: "run-1".into(),
            kind: SharedRunKind::QuickApi,
            source_id: "qa-1".into(),
            project_id: None,
            discussion_id: None,
            status: SharedRunStatus::Success,
            started_at: Some(now),
            finished_at: Some(now),
            duration_ms: Some(42),
            result: Some(serde_json::json!({"ok":true})),
            diagnostic: None,
            created_at: now,
            updated_at: now,
        };
        upsert(&conn, &expected).unwrap();
        let actual = get(&conn, "run-1").unwrap().unwrap();
        assert_eq!(actual.id, "run-1");
        assert_eq!(actual.duration_ms, Some(42));
        assert_eq!(actual.result, expected.result);
    }

    /// The CHECK constraint on `kind` previously excluded `workflow` (a real
    /// bug: a Workflow SharedRun write would fail while QP/QA/QE succeeded).
    /// Round-trip every kind so a future migration regression on any one of
    /// them is caught, not just QuickApi.
    #[test]
    fn round_trips_every_shared_run_kind() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE discussions(id TEXT PRIMARY KEY); CREATE TABLE projects(id TEXT PRIMARY KEY);")
            .unwrap();
        conn.execute_batch(include_str!("sql/155_shared_runs.sql"))
            .unwrap();
        let now = Utc::now();
        for (i, kind) in [
            SharedRunKind::QuickPrompt,
            SharedRunKind::QuickApi,
            SharedRunKind::QuickExec,
            SharedRunKind::Workflow,
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("run-kind-{i}");
            let run = SharedRun {
                id: id.clone(),
                kind: kind.clone(),
                source_id: "source-1".into(),
                project_id: None,
                discussion_id: None,
                status: SharedRunStatus::Success,
                started_at: Some(now),
                finished_at: Some(now),
                duration_ms: Some(1),
                result: None,
                diagnostic: None,
                created_at: now,
                updated_at: now,
            };
            upsert(&conn, &run).unwrap_or_else(|e| panic!("upsert for {kind:?} failed: {e}"));
            let actual = get(&conn, &id).unwrap().unwrap();
            assert_eq!(
                serde_json::to_value(&actual.kind).unwrap(),
                serde_json::to_value(&kind).unwrap(),
                "kind must round-trip unchanged for {kind:?}"
            );
        }
    }

    #[test]
    fn corrupt_values_are_rejected_instead_of_invented() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE discussions(id TEXT PRIMARY KEY); CREATE TABLE projects(id TEXT PRIMARY KEY);").unwrap();
        conn.execute_batch(include_str!("sql/155_shared_runs.sql"))
            .unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints=ON; INSERT INTO shared_runs(id,kind,source_id,status,created_at,updated_at) VALUES('bad','mystery','x','unknown','not-a-date','not-a-date');").unwrap();
        assert!(get(&conn, "bad").is_err());
    }

    fn run(id: &str, project_id: Option<&str>, discussion_id: Option<&str>) -> SharedRun {
        let now = Utc::now();
        SharedRun {
            id: id.into(),
            kind: SharedRunKind::QuickApi,
            source_id: "qa-1".into(),
            project_id: project_id.map(String::from),
            discussion_id: discussion_id.map(String::from),
            status: SharedRunStatus::Success,
            started_at: Some(now),
            finished_at: Some(now),
            duration_ms: Some(1),
            result: None,
            diagnostic: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// A run belonging to project/discussion A must never be visible when a
    /// client rehydrates by scoping to project/discussion B — this is the
    /// isolation guarantee the shared list endpoint's scoping depends on.
    #[test]
    fn list_isolates_runs_across_projects_and_discussions() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE discussions(id TEXT PRIMARY KEY); CREATE TABLE projects(id TEXT PRIMARY KEY);").unwrap();
        conn.execute_batch(include_str!("sql/155_shared_runs.sql"))
            .unwrap();
        conn.execute("INSERT INTO projects(id) VALUES('proj-a'),('proj-b')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO discussions(id) VALUES('disc-a'),('disc-b')",
            [],
        )
        .unwrap();
        upsert(&conn, &run("run-a", Some("proj-a"), Some("disc-a"))).unwrap();
        upsert(&conn, &run("run-b", Some("proj-b"), Some("disc-b"))).unwrap();

        let by_project_a = list(&conn, None, None, Some("proj-a"), None, 50).unwrap();
        assert_eq!(
            by_project_a
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec!["run-a"]
        );

        let by_discussion_b = list(&conn, None, None, None, Some("disc-b"), 50).unwrap();
        assert_eq!(
            by_discussion_b
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            vec!["run-b"]
        );

        let cross_scope = list(&conn, None, None, Some("proj-a"), Some("disc-b"), 50).unwrap();
        assert!(
            cross_scope.is_empty(),
            "a project A / discussion B combination that never co-occurred must return nothing"
        );
    }

    /// `upsert` must propagate a genuine DB failure as `Err` rather than
    /// swallowing it — the QP/QA/QE call sites rely on this to decide
    /// between a fail-closed error and a silently-missing `SharedRun`
    /// projection (KT-243 review finding: two QP call sites used to only
    /// log the error and return success regardless).
    #[test]
    fn upsert_propagates_db_error_instead_of_silently_succeeding() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE discussions(id TEXT PRIMARY KEY); CREATE TABLE projects(id TEXT PRIMARY KEY);").unwrap();
        conn.execute_batch(include_str!("sql/155_shared_runs.sql"))
            .unwrap();
        // Simulate a real outage/corruption case: the table is gone.
        conn.execute_batch("DROP TABLE shared_runs;").unwrap();
        let result = upsert(&conn, &run("run-x", None, None));
        assert!(
            result.is_err(),
            "upsert against a missing table must return Err, not Ok"
        );
    }
}
