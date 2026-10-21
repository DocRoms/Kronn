//! Durable state of an audit run's validation (migration 262, KT-1122): when
//! its linked discussion first signalled completion, who started it and who
//! validated it. The run → discussion link itself is
//! `audit_runs.validation_discussion_id` (076).
//!
//! Completion is a fact about the run, not about the last message of the
//! discussion: the first clean `KRONN:VALIDATION_COMPLETE` stamps it and no
//! later message (a commit note, a DoD update) can undo it.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

pub const VALIDATION_SIGNAL: &str = "KRONN:VALIDATION_COMPLETE";

/// The one parser of the validation signal, shared by every reader: the
/// completion hook, the validate gate, the timeline and the repair. The
/// frontend never re-parses; it reads the `finished` verdict of the server.
pub fn is_validation_signal(content: &str) -> bool {
    crate::api::discussions::ends_with_terminal_signal(content, VALIDATION_SIGNAL)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationRecord {
    pub completed_at: Option<String>,
    pub completed_message_id: Option<String>,
    pub started_by: Option<String>,
    pub validated_by: Option<String>,
    pub validated_at: Option<String>,
}

pub fn record(conn: &Connection, run_id: &str) -> Result<ValidationRecord> {
    Ok(conn
        .query_row(
            "SELECT validation_completed_at, validation_completed_message_id,
                    validation_started_by, validated_by, validated_at
             FROM audit_runs WHERE id = ?1",
            [run_id],
            |row| {
                Ok(ValidationRecord {
                    completed_at: row.get(0)?,
                    completed_message_id: row.get(1)?,
                    started_by: row.get(2)?,
                    validated_by: row.get(3)?,
                    validated_at: row.get(4)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

/// The earliest main-channel agent message of `disc_id` that ends on the
/// signal, as `(message_id, timestamp)`.
pub fn first_signal(conn: &Connection, disc_id: &str) -> Result<Option<(String, String)>> {
    let mut statement = conn.prepare(
        "SELECT id, timestamp, content FROM messages
         WHERE discussion_id = ?1 AND role = 'Agent' AND channel = 'main'
           AND content LIKE '%KRONN:VALIDATION_COMPLETE%'
         ORDER BY sort_order ASC, rowid ASC",
    )?;
    let rows = statement.query_map([disc_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (id, timestamp, content) = row?;
        if is_validation_signal(&content) {
            return Ok(Some((id, timestamp)));
        }
    }
    Ok(None)
}

/// Stamp completion on every run linked to `disc_id`. Set once: a later
/// message never moves or clears it. Returns the rows changed.
pub fn record_completion(
    conn: &Connection,
    disc_id: &str,
    message_id: &str,
    at: &str,
) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE audit_runs
         SET validation_completed_at = ?3, validation_completed_message_id = ?2
         WHERE validation_discussion_id = ?1
           AND validation_completed_at IS NULL
           AND status IN ('Completed', 'Interrupted')",
        params![disc_id, message_id, at],
    )?)
}

/// Hook for every persisted agent message: a clean signal completes the run
/// it validates. A no-op for any other discussion.
pub fn on_agent_message(
    conn: &Connection,
    disc_id: &str,
    message_id: &str,
    content: &str,
    at: &str,
) -> Result<()> {
    if is_validation_signal(content) {
        record_completion(conn, disc_id, message_id, at)?;
    }
    Ok(())
}

/// Completion of `run_id`'s linked validation as `(message_id, at)`: the
/// stored stamp, else the earliest clean signal in the discussion (rows that
/// predate the hook, or that a path bypassed it).
pub fn effective_completion(
    conn: &Connection,
    run_id: &str,
    disc_id: &str,
) -> Result<Option<(String, String)>> {
    let stored = record(conn, run_id)?;
    if let (Some(at), Some(message)) = (stored.completed_at, stored.completed_message_id) {
        return Ok(Some((message, at)));
    }
    first_signal(conn, disc_id)
}

pub fn set_started_by(conn: &Connection, run_id: &str, by: &str) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET validation_started_by = ?2
         WHERE id = ?1 AND validation_started_by IS NULL",
        params![run_id, by],
    )?;
    Ok(())
}

pub fn record_validated(conn: &Connection, run_id: &str, by: &str, at: &str) -> Result<()> {
    conn.execute(
        "UPDATE audit_runs SET validated_by = ?2, validated_at = ?3 WHERE id = ?1",
        params![run_id, by, at],
    )?;
    Ok(())
}

/// Title family of generated validation discussions: "Validation audit AI",
/// "Validation audit <kind>" for a sub-audit, "Validation audit partiel (…)".
pub fn is_validation_title(title: &str) -> bool {
    title == "Validation audit AI" || title.starts_with("Validation audit ")
}

/// Strict provenance of a never-linked, generated validation discussion: a
/// generated title AND a first message that is the generated validation
/// prompt (a user message quoting the exact signal it asks for).
fn has_generated_provenance(conn: &Connection, disc_id: &str, title: &str) -> Result<bool> {
    if !is_validation_title(title) {
        return Ok(false);
    }
    let first: Option<(String, String)> = conn
        .query_row(
            "SELECT role, content FROM messages
             WHERE discussion_id = ?1 AND channel = 'main'
             ORDER BY sort_order ASC, rowid ASC LIMIT 1",
            [disc_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(first.is_some_and(|(role, content)| {
        role == "User" && content.contains(&format!("\"{VALIDATION_SIGNAL}\""))
    }))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RepairReport {
    pub completed: usize,
    pub archived: usize,
}

/// Bring existing data in line, idempotently: stamp completion from the
/// earliest clean signal of each linked discussion, then archive the open
/// validations an older audit left behind.
///
/// A discussion is archived only when ALL hold: it is not the latest linked
/// run's validation; same project; generated title and first message are the
/// generated validation prompt; created before that run started; no queued or
/// running agent job and no agent awaited. Nothing else is touched.
pub fn repair(conn: &Connection) -> Result<RepairReport> {
    let mut report = RepairReport::default();
    let pending: Vec<String> = {
        let mut statement = conn.prepare(
            "SELECT DISTINCT validation_discussion_id FROM audit_runs
             WHERE validation_discussion_id IS NOT NULL
               AND validation_completed_at IS NULL
               AND status IN ('Completed', 'Interrupted')",
        )?;
        let ids = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids
    };
    for disc_id in pending {
        if let Some((message_id, at)) = first_signal(conn, &disc_id)? {
            report.completed += record_completion(conn, &disc_id, &message_id, &at)?;
        }
    }

    // The latest LINKED run of each project, with the discussion it keeps.
    let latest_linked: Vec<(String, String, String)> = {
        let mut statement = conn.prepare(
            "SELECT r.project_id, r.validation_discussion_id, r.started_at
             FROM audit_runs r
             WHERE r.validation_discussion_id IS NOT NULL
               AND r.rowid = (
                   SELECT l.rowid FROM audit_runs l
                   WHERE l.project_id = r.project_id
                     AND l.validation_discussion_id IS NOT NULL
                   ORDER BY l.started_at DESC, l.rowid DESC LIMIT 1)",
        )?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (project_id, keep, started_at) in latest_linked {
        let candidates: Vec<(String, String)> = {
            let mut statement = conn.prepare(
                "SELECT d.id, d.title FROM discussions d
                 WHERE d.project_id = ?1 AND d.id != ?2 AND d.archived = 0
                   AND d.awaiting_agent = 0
                   AND d.title LIKE 'Validation audit%'
                   AND julianday(d.created_at) < julianday(?3)
                   AND NOT EXISTS (
                       SELECT 1 FROM agent_dispatch_jobs j
                       WHERE j.discussion_id = d.id
                         AND j.status IN ('Pending', 'Running'))",
            )?;
            let rows = statement
                .query_map(params![project_id, keep, started_at], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (id, title) in candidates {
            if has_generated_provenance(conn, &id, &title)? {
                report.archived +=
                    conn.execute("UPDATE discussions SET archived = 1 WHERE id = ?1", [&id])?;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('p1', 'P', '/tmp/p1', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn
    }

    fn discussion(conn: &Connection, id: &str, title: &str, created: &str, archived: bool) {
        conn.execute(
            "INSERT INTO discussions (id, project_id, title, agent, language, created_at, updated_at, archived)
             VALUES (?1, 'p1', ?2, 'ClaudeCode', 'fr', ?3, ?3, ?4)",
            params![id, title, created, archived],
        )
        .unwrap();
    }

    fn message(conn: &Connection, disc: &str, id: &str, order: i64, role: &str, content: &str) {
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order, tokens_used)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            params![id, disc, role, content, format!("2026-08-28T21:{:02}:00+00:00", order), order],
        )
        .unwrap();
    }

    const PROMPT: &str =
        "Valide.\n\n## Sortie\némets la phrase exacte : \"KRONN:VALIDATION_COMPLETE\".";

    fn completed_run(conn: &Connection, id: &str, started: &str, disc: &str) {
        let started = chrono::DateTime::parse_from_rfc3339(started)
            .unwrap()
            .with_timezone(&chrono::Utc);
        crate::db::audit_runs::insert_running(conn, id, "p1", "Full", "ClaudeCode", started)
            .unwrap();
        crate::db::audit_runs::complete(
            conn,
            id,
            started,
            "Completed",
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            100,
            None,
            None,
        )
        .unwrap();
        crate::db::audit_runs::set_validation_discussion(conn, id, disc).unwrap();
    }

    fn archived(conn: &Connection, id: &str) -> bool {
        conn.query_row(
            "SELECT archived FROM discussions WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn the_parser_tolerates_emphasis_but_stays_position_strict() {
        assert!(is_validation_signal("Done.\nKRONN:VALIDATION_COMPLETE"));
        assert!(is_validation_signal("Done.\n**KRONN:VALIDATION_COMPLETE**"));
        assert!(is_validation_signal("Done.\n**KRONN:VALIDATION_COMPLETE"));
        assert!(is_validation_signal("Done.\n`KRONN:VALIDATION_COMPLETE`\n"));
        assert!(!is_validation_signal(
            "I will emit KRONN:VALIDATION_COMPLETE later."
        ));
        assert!(!is_validation_signal(
            "KRONN:VALIDATION_COMPLETE\nmore work"
        ));
    }

    /// The table the frontend's `endsWithValidationSignal` test reads too
    /// (`frontend/src/lib/__tests__/validationSignal.test.ts`): one rule, two
    /// implementations, one list of cases.
    #[test]
    fn the_shared_signal_case_table_holds_for_the_backend_parser() {
        let table: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/validation_signal_cases.json"
        ))
        .unwrap();
        assert_eq!(table["signal"], VALIDATION_SIGNAL);
        let cases = table["cases"].as_array().unwrap();
        assert!(cases.len() >= 10, "the table must keep its coverage");
        for case in cases {
            let content = case["content"].as_str().unwrap();
            let expected = case["ends_with_signal"].as_bool().unwrap();
            assert_eq!(is_validation_signal(content), expected, "{content:?}");
            assert_eq!(
                crate::api::discussions::ends_with_terminal_signal(content, VALIDATION_SIGNAL),
                expected,
                "{content:?}"
            );
        }
    }

    #[test]
    fn completion_is_set_once_and_survives_later_messages() {
        let conn = conn();
        discussion(
            &conn,
            "d1",
            "Validation audit AI",
            "2026-08-28T20:00:00+00:00",
            false,
        );
        completed_run(&conn, "r1", "2026-08-28T19:00:00+00:00", "d1");
        on_agent_message(&conn, "d1", "m1", "Phase 1 done", "t1").unwrap();
        assert_eq!(record(&conn, "r1").unwrap().completed_at, None);
        on_agent_message(&conn, "d1", "m2", "Fin.\nKRONN:VALIDATION_COMPLETE", "t2").unwrap();
        on_agent_message(&conn, "d1", "m3", "Commit pushed for PR #34", "t3").unwrap();
        on_agent_message(&conn, "d1", "m4", "Again\nKRONN:VALIDATION_COMPLETE", "t4").unwrap();
        let stored = record(&conn, "r1").unwrap();
        assert_eq!(stored.completed_at.as_deref(), Some("t2"));
        assert_eq!(stored.completed_message_id.as_deref(), Some("m2"));
    }

    #[test]
    fn inserting_an_agent_message_stamps_the_run_through_the_real_write_path() {
        let conn = conn();
        discussion(
            &conn,
            "d1",
            "Validation audit AI",
            "2026-08-28T20:00:00+00:00",
            false,
        );
        completed_run(&conn, "r1", "2026-08-28T19:00:00+00:00", "d1");
        let mut msg: crate::models::DiscussionMessage = serde_json::from_value(serde_json::json!({
            "id": "m-signal", "role": "Agent", "content": "Fin.\nKRONN:VALIDATION_COMPLETE",
            "timestamp": "2026-08-28T21:37:00Z", "tokens_used": 0
        }))
        .unwrap();
        crate::db::discussions::insert_message(&conn, "d1", &msg).unwrap();
        msg.id = "m-later".into();
        msg.content = "Commit pushed for PR #34".into();
        crate::db::discussions::insert_message(&conn, "d1", &msg).unwrap();
        let stored = record(&conn, "r1").unwrap();
        assert_eq!(stored.completed_message_id.as_deref(), Some("m-signal"));
        assert_eq!(
            effective_completion(&conn, "r1", "d1")
                .unwrap()
                .map(|c| c.0)
                .as_deref(),
            Some("m-signal")
        );
    }

    #[test]
    fn relinking_to_another_discussion_drops_the_previous_completion() {
        let conn = conn();
        discussion(
            &conn,
            "d1",
            "Validation audit AI",
            "2026-08-28T20:00:00+00:00",
            true,
        );
        completed_run(&conn, "r1", "2026-08-28T19:00:00+00:00", "d1");
        record_completion(&conn, "d1", "m", "t").unwrap();
        crate::db::audit_runs::set_validation_discussion(&conn, "r1", "d1").unwrap();
        assert!(record(&conn, "r1").unwrap().completed_at.is_some());
        crate::db::audit_runs::set_validation_discussion(&conn, "r1", "d2").unwrap();
        assert_eq!(record(&conn, "r1").unwrap().completed_at, None);
    }

    #[test]
    fn repair_recognises_an_old_signal_and_archives_only_proven_stale_validations() {
        let conn = conn();
        // front-africanews: the linked validation signalled, then got more
        // agent messages; an April validation of an older audit is still open.
        discussion(
            &conn,
            "linked",
            "Validation audit AI",
            "2026-08-28T20:00:00+00:00",
            true,
        );
        message(&conn, "linked", "l0", 0, "User", PROMPT);
        message(
            &conn,
            "linked",
            "l1",
            1,
            "Agent",
            "Fin.\nKRONN:VALIDATION_COMPLETE",
        );
        message(
            &conn,
            "linked",
            "l2",
            2,
            "Agent",
            "Commit pushed for PR #34",
        );
        discussion(
            &conn,
            "april",
            "Validation audit AI",
            "2026-04-02T10:00:00+00:00",
            false,
        );
        message(&conn, "april", "a0", 0, "User", PROMPT);
        discussion(
            &conn,
            "sub",
            "Validation audit RGAA 4.1",
            "2026-04-03T10:00:00+00:00",
            false,
        );
        message(&conn, "sub", "s0", 0, "User", PROMPT);
        // Look-alikes that must stay open.
        discussion(
            &conn,
            "forged",
            "Validation audit AI",
            "2026-04-04T10:00:00+00:00",
            false,
        );
        message(&conn, "forged", "f0", 0, "User", "please validate");
        discussion(
            &conn,
            "retitled",
            "My notes",
            "2026-04-05T10:00:00+00:00",
            false,
        );
        message(&conn, "retitled", "t0", 0, "User", PROMPT);
        discussion(
            &conn,
            "busy",
            "Validation audit AI",
            "2026-04-06T10:00:00+00:00",
            false,
        );
        message(&conn, "busy", "b0", 0, "User", PROMPT);
        conn.execute(
            "UPDATE discussions SET awaiting_agent = 1 WHERE id = 'busy'",
            [],
        )
        .unwrap();
        discussion(
            &conn,
            "newer",
            "Validation audit AI",
            "2026-09-01T10:00:00+00:00",
            false,
        );
        message(&conn, "newer", "n0", 0, "User", PROMPT);
        completed_run(&conn, "r1", "2026-08-28T19:00:00+00:00", "linked");

        let first = repair(&conn).unwrap();
        assert_eq!(
            first,
            RepairReport {
                completed: 1,
                archived: 2
            }
        );
        assert_eq!(
            record(&conn, "r1").unwrap().completed_message_id.as_deref(),
            Some("l1")
        );
        assert!(archived(&conn, "april") && archived(&conn, "sub"));
        for open in ["forged", "retitled", "busy", "newer"] {
            assert!(!archived(&conn, open), "{open} must stay open");
        }
        assert_eq!(
            repair(&conn).unwrap(),
            RepairReport::default(),
            "idempotent"
        );
    }
}
