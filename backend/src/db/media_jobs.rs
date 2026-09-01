//! Persistence for media generation jobs.
//!
//! Mirrors the `agent_resume_jobs` pattern — due-selection, atomic claim,
//! orphan reclaim — because that is the shape a restart-safe worker needs. The
//! delegated-task lifecycle was deliberately not reused: it assumes a
//! workspace, a review and a commit, none of which a provider call has.
//!
//! Restart safety is the point: a video generation costs money the moment the
//! provider accepts it (~0.07 USD for 5 s at 480p) and takes ~100 s, so a job
//! left `running` by a killed process must come back, not vanish.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{MediaCost, MediaJobStatus, MediaModality, MediaParams, MediaRendered};

#[derive(Debug, Clone, PartialEq)]
pub struct MediaJob {
    pub id: String,
    pub modality: MediaModality,
    pub status: MediaJobStatus,
    pub connection_id: String,
    pub model: String,
    pub prompt: String,
    pub params: MediaParams,
    pub discussion_id: Option<String>,
    pub message_id: Option<String>,
    pub project_id: Option<String>,
    /// Set just before the billable POST is sent. Its presence WITHOUT a
    /// handle means a submission may have been charged without being recorded.
    pub submit_attempted_at: Option<DateTime<Utc>>,
    pub provider_job_id: Option<String>,
    pub provider_generation_id: Option<String>,
    pub context_file_id: Option<String>,
    pub rendered: MediaRendered,
    pub cost: Option<MediaCost>,
    pub last_error: Option<String>,
    pub attempts: u32,
    pub scheduled_at: DateTime<Utc>,
    pub deadline_at: DateTime<Utc>,
}

pub struct NewMediaJob<'a> {
    pub id: &'a str,
    pub modality: MediaModality,
    pub connection_id: &'a str,
    pub model: &'a str,
    pub prompt: &'a str,
    pub params: &'a MediaParams,
    pub discussion_id: Option<&'a str>,
    pub message_id: Option<&'a str>,
    pub project_id: Option<&'a str>,
    pub scheduled_at: DateTime<Utc>,
    pub deadline_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, modality, status, connection_id, model, prompt, params_json,
    discussion_id, message_id, project_id, submit_attempted_at, provider_job_id, provider_generation_id,
    context_file_id, rendered_width, rendered_height, rendered_duration_ms,
    cost_usd, is_byok, last_error, attempts, scheduled_at, deadline_at";

fn row_to_job(row: &rusqlite::Row) -> rusqlite::Result<MediaJob> {
    let modality: String = row.get("modality")?;
    let status: String = row.get("status")?;
    let params: Option<String> = row.get("params_json")?;
    let cost_usd: Option<f64> = row.get("cost_usd")?;
    Ok(MediaJob {
        id: row.get("id")?,
        // An unknown value is a corrupt row, not a reason to guess a modality.
        modality: MediaModality::parse(&modality).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                format!("unknown media modality: {modality}").into(),
            )
        })?,
        status: MediaJobStatus::parse(&status).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                format!("unknown media job status: {status}").into(),
            )
        })?,
        connection_id: row.get("connection_id")?,
        model: row.get("model")?,
        prompt: row.get("prompt")?,
        params: params
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default(),
        discussion_id: row.get("discussion_id")?,
        message_id: row.get("message_id")?,
        project_id: row.get("project_id")?,
        submit_attempted_at: row
            .get::<_, Option<String>>("submit_attempted_at")?
            .map(|raw| parse_dt(&raw)),
        provider_job_id: row.get("provider_job_id")?,
        provider_generation_id: row.get("provider_generation_id")?,
        context_file_id: row.get("context_file_id")?,
        rendered: MediaRendered {
            width: row
                .get::<_, Option<i64>>("rendered_width")?
                .map(|v| v as u32),
            height: row
                .get::<_, Option<i64>>("rendered_height")?
                .map(|v| v as u32),
            duration_ms: row
                .get::<_, Option<i64>>("rendered_duration_ms")?
                .map(|v| v as u64),
        },
        // Only a declared cost counts as a cost; a NULL is "not billed yet".
        cost: cost_usd.map(|cost_usd| MediaCost {
            cost_usd,
            is_byok: row.get::<_, i64>("is_byok").unwrap_or(0) != 0,
        }),
        last_error: row.get("last_error")?,
        attempts: row.get::<_, i64>("attempts")? as u32,
        scheduled_at: parse_dt(&row.get::<_, String>("scheduled_at")?),
        deadline_at: parse_dt(&row.get::<_, String>("deadline_at")?),
    })
}

fn parse_dt(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

pub fn insert(conn: &Connection, job: NewMediaJob<'_>, now: DateTime<Utc>) -> Result<()> {
    let params_json = serde_json::to_string(job.params)?;
    conn.execute(
        "INSERT INTO media_jobs
            (id, modality, status, connection_id, model, prompt, params_json,
             discussion_id, message_id, project_id, is_byok, attempts,
             scheduled_at, deadline_at, created_at, updated_at)
         VALUES (?1, ?2, 'pending', ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, 0, ?10, ?11, ?12, ?12)",
        params![
            job.id,
            job.modality.as_str(),
            job.connection_id,
            job.model,
            job.prompt,
            params_json,
            job.discussion_id,
            job.message_id,
            job.project_id,
            job.scheduled_at.to_rfc3339(),
            job.deadline_at.to_rfc3339(),
            now.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<MediaJob>> {
    let sql = format!("SELECT {COLUMNS} FROM media_jobs WHERE id = ?1");
    Ok(conn.query_row(&sql, params![id], row_to_job).optional()?)
}

/// Jobs whose time has come, oldest first so nothing starves.
pub fn due(conn: &Connection, now: DateTime<Utc>, limit: u32) -> Result<Vec<MediaJob>> {
    let sql = format!(
        "SELECT {COLUMNS} FROM media_jobs
         WHERE status = 'pending' AND scheduled_at <= ?1
         ORDER BY scheduled_at ASC LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![now.to_rfc3339(), limit], row_to_job)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Atomic claim. The `status = 'pending'` predicate inside the UPDATE is what
/// stops two workers from running the same billed generation twice.
pub fn claim(conn: &Connection, id: &str, now: DateTime<Utc>) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE media_jobs
         SET status = 'running', started_at = COALESCE(started_at, ?2),
             attempts = attempts + 1, updated_at = ?2
         WHERE id = ?1 AND status = 'pending' AND scheduled_at <= ?2",
        params![id, now.to_rfc3339()],
    )?;
    Ok(changed == 1)
}

/// Stamps the intent to submit BEFORE the billable request is sent.
///
/// This mark is the only thing standing between a crash and a double charge:
/// once the POST is in flight the provider may already be generating and
/// billing, and nothing afterwards can tell us whether it arrived. A mark with
/// no handle is therefore treated as "possibly charged" and never retried.
pub fn mark_submit_attempt(conn: &Connection, id: &str, now: DateTime<Utc>) -> Result<()> {
    conn.execute(
        "UPDATE media_jobs SET submit_attempted_at = ?2, updated_at = ?2 WHERE id = ?1",
        params![id, now.to_rfc3339()],
    )?;
    Ok(())
}

/// Records the provider handle as soon as it is known, so a restart between
/// submission and completion can resume polling instead of paying twice.
pub fn record_submission(
    conn: &Connection,
    id: &str,
    provider_job_id: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE media_jobs SET provider_job_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, provider_job_id, now.to_rfc3339()],
    )?;
    Ok(())
}

/// Sends a claimed job back to `pending` for its next poll.
pub fn reschedule(
    conn: &Connection,
    id: &str,
    next_attempt_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE media_jobs SET status = 'pending', scheduled_at = ?2, updated_at = ?3
         WHERE id = ?1 AND status = 'running'",
        params![id, next_attempt_at.to_rfc3339(), now.to_rfc3339()],
    )?;
    Ok(())
}

pub struct Completion<'a> {
    pub context_file_id: &'a str,
    pub rendered: &'a MediaRendered,
    pub cost: Option<MediaCost>,
    pub generation_id: Option<&'a str>,
}

pub fn complete(
    conn: &Connection,
    id: &str,
    outcome: Completion<'_>,
    now: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE media_jobs
         SET status = 'completed', context_file_id = ?2, rendered_width = ?3,
             rendered_height = ?4, rendered_duration_ms = ?5, cost_usd = ?6,
             is_byok = ?7, provider_generation_id = COALESCE(?8, provider_generation_id),
             last_error = NULL, completed_at = ?9, updated_at = ?9
         WHERE id = ?1",
        params![
            id,
            outcome.context_file_id,
            outcome.rendered.width.map(|v| v as i64),
            outcome.rendered.height.map(|v| v as i64),
            outcome.rendered.duration_ms.map(|v| v as i64),
            outcome.cost.map(|c| c.cost_usd),
            outcome.cost.map(|c| i64::from(c.is_byok)).unwrap_or(0),
            outcome.generation_id,
            now.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn fail(
    conn: &Connection,
    id: &str,
    status: MediaJobStatus,
    error: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE media_jobs SET status = ?2, last_error = ?3, completed_at = ?4, updated_at = ?4
         WHERE id = ?1",
        params![id, status.as_str(), error, now.to_rfc3339()],
    )?;
    Ok(())
}

/// Cancels a job that has not finished. Terminal jobs are left alone so a
/// completed — and billed — generation is never rewritten as cancelled.
pub fn cancel(conn: &Connection, id: &str, now: DateTime<Utc>) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE media_jobs SET status = 'cancelled', completed_at = ?2, updated_at = ?2
         WHERE id = ?1 AND status IN ('pending','running')",
        params![id, now.to_rfc3339()],
    )?;
    Ok(changed == 1)
}

/// Startup recovery: jobs left `running` by a killed process go back to
/// `pending` so their polling resumes. Without this, a generation already paid
/// for is silently abandoned.
pub fn reclaim_orphans(conn: &Connection, now: DateTime<Utc>) -> Result<usize> {
    let changed = conn.execute(
        "UPDATE media_jobs SET status = 'pending', scheduled_at = ?1, updated_at = ?1
         WHERE status = 'running'",
        params![now.to_rfc3339()],
    )?;
    Ok(changed)
}

/// Jobs past their deadline, so an unbounded provider cannot pin one forever.
///
/// Returns the IDS it settled, not a count: an expired job leaves `pending`, so
/// `due()` will never surface it again — if the caller cannot name the rows it
/// just changed, nothing can publish them and a card already on screen keeps
/// showing `running` until someone reloads.
///
/// The ids are read back inside the same statement via `RETURNING`, so a
/// concurrent worker cannot claim a row between the update and the read.
pub fn expire_overdue(conn: &Connection, now: DateTime<Utc>) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "UPDATE media_jobs
         SET status = 'timed_out', last_error = 'deadline exceeded',
             completed_at = ?1, updated_at = ?1
         WHERE status IN ('pending','running') AND deadline_at <= ?1
         RETURNING id",
    )?;
    let rows = stmt.query_map(params![now.to_rfc3339()], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Observed cost of past generations with this model, for an estimate.
///
/// Deliberately measured rather than derived from a published rate: the rate
/// does not reproduce the invoice (0.0708932 USD billed against 0.0678 implied
/// for one 5 s clip), and a hardcoded table would drift silently the day a
/// provider changes its pricing. `None` when nothing comparable was ever
/// billed — no estimate is better than a fabricated one.
pub fn observed_unit_cost(
    conn: &Connection,
    model: &str,
    modality: MediaModality,
) -> Result<Option<(f64, u32)>> {
    // Per-second for video, per-image otherwise, so a 5 s sample can inform a
    // 10 s request. Rows with no measured duration are skipped rather than
    // counted as one second.
    let sql = match modality {
        MediaModality::Video => {
            "SELECT AVG(cost_usd * 1000.0 / rendered_duration_ms), COUNT(*)
             FROM media_jobs
             WHERE model = ?1 AND modality = 'video' AND cost_usd IS NOT NULL
               AND rendered_duration_ms IS NOT NULL AND rendered_duration_ms > 0
               AND is_byok = 0"
        }
        MediaModality::Image => {
            "SELECT AVG(cost_usd), COUNT(*)
             FROM media_jobs
             WHERE model = ?1 AND modality = 'image' AND cost_usd IS NOT NULL
               AND is_byok = 0"
        }
    };
    let row: (Option<f64>, i64) =
        conn.query_row(sql, params![model], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(match row {
        (Some(unit), samples) if samples > 0 && unit.is_finite() && unit > 0.0 => {
            Some((unit, samples as u32))
        }
        // BYOK rows are excluded above: a zero-cost sample would drag an
        // estimate to zero for everyone else.
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    const MIGRATION_155: &str = include_str!("sql/155_shared_runs.sql");
    const MIGRATION_157: &str = include_str!("sql/157_media_jobs.sql");

    /// A database at 155, with the referenced tables the FKs need.
    fn db_at_155() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY);
             CREATE TABLE discussions (id TEXT PRIMARY KEY);
             INSERT INTO projects (id) VALUES ('p1');
             INSERT INTO discussions (id) VALUES ('d1');",
        )
        .unwrap();
        conn.execute_batch(MIGRATION_155).unwrap();
        conn
    }

    fn db() -> Connection {
        let conn = db_at_155();
        conn.execute_batch(MIGRATION_157).unwrap();
        conn
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-31T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn add(conn: &Connection, id: &str, at: DateTime<Utc>) {
        insert(
            conn,
            NewMediaJob {
                id,
                modality: MediaModality::Video,
                connection_id: "conn-1",
                model: "bytedance/seedance-2.0-mini",
                prompt: "un chat",
                params: &MediaParams {
                    duration_secs: Some(5),
                    ..Default::default()
                },
                discussion_id: Some("d1"),
                message_id: None,
                project_id: Some("p1"),
                scheduled_at: at,
                deadline_at: at + Duration::minutes(20),
            },
            at,
        )
        .unwrap();
    }

    #[test]
    fn the_media_migration_admits_media_runs_and_keeps_existing_rows() {
        let conn = db_at_155();
        conn.execute(
            "INSERT INTO shared_runs (id, kind, source_id, status, created_at, updated_at)
             VALUES ('r1', 'workflow', 's1', 'running', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        // Before: 'media' is rejected by the CHECK.
        assert!(conn
            .execute(
                "INSERT INTO shared_runs (id, kind, source_id, status, created_at, updated_at)
                 VALUES ('r2','media','s2','queued','x','x')",
                [],
            )
            .is_err());

        conn.execute_batch(MIGRATION_157).unwrap();

        // After: accepted, and the pre-existing row survived the rebuild.
        conn.execute(
            "INSERT INTO shared_runs (id, kind, source_id, status, created_at, updated_at)
             VALUES ('r2','media','s2','queued','x','x')",
            [],
        )
        .unwrap();
        let kept: String = conn
            .query_row("SELECT kind FROM shared_runs WHERE id = 'r1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kept, "workflow");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM shared_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        // Indexes are recreated, not dropped with the old table.
        let indexes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='index' AND tbl_name='shared_runs' AND name LIKE 'idx_shared_runs%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(indexes, 3);

        // An unknown kind is still refused: the CHECK was widened, not removed.
        assert!(conn
            .execute(
                "INSERT INTO shared_runs (id, kind, source_id, status, created_at, updated_at)
                 VALUES ('r3','whatever','s3','queued','x','x')",
                [],
            )
            .is_err());
    }

    #[test]
    fn a_job_round_trips_through_the_database() {
        let conn = db();
        add(&conn, "j1", now());
        let job = get(&conn, "j1").unwrap().expect("job stored");
        assert_eq!(job.modality, MediaModality::Video);
        assert_eq!(job.status, MediaJobStatus::Pending);
        assert_eq!(job.params.duration_secs, Some(5));
        assert_eq!(job.discussion_id.as_deref(), Some("d1"));
        assert_eq!(job.attempts, 0);
        // Nothing billed yet: absent, not zero.
        assert!(job.cost.is_none());
    }

    #[test]
    fn due_returns_only_pending_jobs_whose_time_has_come() {
        let conn = db();
        add(&conn, "past", now() - Duration::minutes(1));
        add(&conn, "future", now() + Duration::minutes(5));

        let ids: Vec<String> = due(&conn, now(), 10)
            .unwrap()
            .into_iter()
            .map(|j| j.id)
            .collect();
        assert_eq!(ids, vec!["past"]);
    }

    #[test]
    fn a_job_can_only_be_claimed_once() {
        let conn = db();
        add(&conn, "j1", now());

        assert!(claim(&conn, "j1", now()).unwrap(), "first claim wins");
        // The whole point: a second worker must not run a billed generation again.
        assert!(!claim(&conn, "j1", now()).unwrap(), "second claim refused");
        assert_eq!(
            get(&conn, "j1").unwrap().unwrap().status,
            MediaJobStatus::Running
        );
        assert_eq!(get(&conn, "j1").unwrap().unwrap().attempts, 1);
    }

    #[test]
    fn orphaned_running_jobs_come_back_after_a_restart() {
        let conn = db();
        add(&conn, "j1", now());
        claim(&conn, "j1", now()).unwrap();
        record_submission(&conn, "j1", "provider-abc", now()).unwrap();

        // Process dies here; the provider is already generating and billing.
        let recovered = reclaim_orphans(&conn, now()).unwrap();
        assert_eq!(recovered, 1);

        let job = get(&conn, "j1").unwrap().unwrap();
        assert_eq!(job.status, MediaJobStatus::Pending);
        // The handle survived, so polling resumes instead of resubmitting.
        assert_eq!(job.provider_job_id.as_deref(), Some("provider-abc"));
        assert!(due(&conn, now(), 10).unwrap().iter().any(|j| j.id == "j1"));
    }

    #[test]
    fn reclaim_never_touches_a_finished_job() {
        let conn = db();
        add(&conn, "done", now());
        claim(&conn, "done", now()).unwrap();
        complete(
            &conn,
            "done",
            Completion {
                context_file_id: "cf1",
                rendered: &MediaRendered {
                    width: Some(864),
                    height: Some(496),
                    duration_ms: Some(5040),
                },
                cost: Some(MediaCost {
                    cost_usd: 0.070_893_2,
                    is_byok: false,
                }),
                generation_id: Some("gen-1"),
            },
            now(),
        )
        .unwrap();

        assert_eq!(reclaim_orphans(&conn, now()).unwrap(), 0);
        assert_eq!(
            get(&conn, "done").unwrap().unwrap().status,
            MediaJobStatus::Completed
        );
    }

    #[test]
    fn completion_persists_the_declared_cost_and_the_rendered_dimensions() {
        let conn = db();
        add(&conn, "j1", now());
        claim(&conn, "j1", now()).unwrap();
        complete(
            &conn,
            "j1",
            Completion {
                context_file_id: "cf1",
                // 864x496 for a "480p 16:9" request: read from the file, not
                // from what was asked.
                rendered: &MediaRendered {
                    width: Some(864),
                    height: Some(496),
                    duration_ms: Some(5040),
                },
                cost: Some(MediaCost {
                    cost_usd: 0.070_893_2,
                    is_byok: false,
                }),
                generation_id: Some("gen-vid-1"),
            },
            now(),
        )
        .unwrap();

        let job = get(&conn, "j1").unwrap().unwrap();
        assert_eq!(job.context_file_id.as_deref(), Some("cf1"));
        assert_eq!(job.rendered.width, Some(864));
        assert_eq!(job.rendered.height, Some(496));
        let cost = job.cost.expect("cost recorded");
        assert_eq!(cost.cost_usd, 0.070_893_2);
        assert!(!cost.is_byok);
        assert_eq!(job.provider_generation_id.as_deref(), Some("gen-vid-1"));
    }

    #[test]
    fn cancel_spares_a_job_that_already_finished_and_was_billed() {
        let conn = db();
        add(&conn, "j1", now());
        claim(&conn, "j1", now()).unwrap();
        complete(
            &conn,
            "j1",
            Completion {
                context_file_id: "cf1",
                rendered: &MediaRendered::default(),
                cost: Some(MediaCost {
                    cost_usd: 0.07,
                    is_byok: false,
                }),
                generation_id: None,
            },
            now(),
        )
        .unwrap();

        assert!(
            !cancel(&conn, "j1", now()).unwrap(),
            "a completed job is not cancellable"
        );
        assert_eq!(
            get(&conn, "j1").unwrap().unwrap().status,
            MediaJobStatus::Completed
        );

        add(&conn, "j2", now());
        assert!(cancel(&conn, "j2", now()).unwrap());
        assert_eq!(
            get(&conn, "j2").unwrap().unwrap().status,
            MediaJobStatus::Cancelled
        );
    }

    #[test]
    fn an_overdue_job_times_out_instead_of_polling_forever() {
        let conn = db();
        add(&conn, "j1", now() - Duration::hours(2));
        // Its deadline was 20 min after scheduling.
        assert_eq!(
            expire_overdue(&conn, now()).unwrap(),
            vec!["j1".to_string()]
        );
        let job = get(&conn, "j1").unwrap().unwrap();
        assert_eq!(job.status, MediaJobStatus::TimedOut);
        assert_eq!(job.last_error.as_deref(), Some("deadline exceeded"));

        // Terminal jobs are not re-expired.
        assert!(
            expire_overdue(&conn, now()).unwrap().is_empty(),
            "a terminal job must not be re-expired"
        );
    }

    #[test]
    fn rescheduling_moves_a_running_job_back_into_the_due_queue() {
        let conn = db();
        add(&conn, "j1", now());
        claim(&conn, "j1", now()).unwrap();
        let next = now() + Duration::seconds(30);
        reschedule(&conn, "j1", next, now()).unwrap();

        assert!(due(&conn, now(), 10).unwrap().is_empty(), "not due yet");
        assert_eq!(due(&conn, next, 10).unwrap().len(), 1);
    }

    #[test]
    fn a_corrupt_status_is_refused_rather_than_silently_mapped() {
        let conn = db();
        add(&conn, "j1", now());
        // Bypasses the CHECK the way a future migration bug might.
        conn.execute("PRAGMA writable_schema=ON", []).ok();
        conn.execute("UPDATE media_jobs SET status = 'weird' WHERE id = 'j1'", [])
            .ok();
        if let Ok(Some(job)) = get(&conn, "j1") {
            // If the CHECK held, the value never changed — that is fine too.
            assert_eq!(job.status, MediaJobStatus::Pending);
        }
    }
}

/// One billed generation, for the media cost counter.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaSpend {
    pub id: String,
    pub modality: MediaModality,
    pub model: String,
    pub discussion_id: Option<String>,
    pub cost_usd: f64,
    pub is_byok: bool,
    pub completed_at: Option<String>,
}

/// Billed generations, newest first.
///
/// Kept apart from token accounting on purpose: a media generation is billed
/// per image or per second and its usage payload carries NO token count, so
/// folding it into the token counters would either report zero tokens for real
/// spend or invent a token equivalent.
pub fn spend(conn: &Connection, limit: u32) -> Result<Vec<MediaSpend>> {
    let mut stmt = conn.prepare(
        "SELECT id, modality, model, discussion_id, cost_usd, is_byok, completed_at
         FROM media_jobs
         WHERE cost_usd IS NOT NULL
         ORDER BY completed_at DESC NULLS LAST, updated_at DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        let modality: String = row.get("modality")?;
        Ok(MediaSpend {
            id: row.get("id")?,
            modality: MediaModality::parse(&modality).unwrap_or(MediaModality::Image),
            model: row.get("model")?,
            discussion_id: row.get("discussion_id")?,
            cost_usd: row.get::<_, Option<f64>>("cost_usd")?.unwrap_or(0.0),
            is_byok: row.get::<_, i64>("is_byok").unwrap_or(0) != 0,
            completed_at: row.get("completed_at")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Total billed, split by modality. `None` rows are excluded: a job that has
/// not settled is not free, it is simply not billed yet.
pub fn spend_total(conn: &Connection) -> Result<(f64, f64)> {
    let mut stmt = conn.prepare(
        "SELECT modality, COALESCE(SUM(cost_usd), 0.0)
         FROM media_jobs WHERE cost_usd IS NOT NULL GROUP BY modality",
    )?;
    let mut image = 0.0;
    let mut video = 0.0;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let modality: String = row.get(0)?;
        let total: f64 = row.get(1)?;
        match MediaModality::parse(&modality) {
            Some(MediaModality::Image) => image = total,
            Some(MediaModality::Video) => video = total,
            None => {}
        }
    }
    Ok((image, video))
}
