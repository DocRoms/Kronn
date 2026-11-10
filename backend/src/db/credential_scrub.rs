//! KT-1656 — credentials that already sit in stored agent output.
//!
//! A worker that prints a credential file leaves it in more than one place: the
//! message it quoted it in, but also the copy a deletion keeps, the edit history,
//! the summary written from the transcript, the partial response of an
//! interrupted turn, the delivery and review of a task execution, the output of
//! a validation run, the step results of a workflow run. [`TARGETS`] lists every
//! column that can hold what an agent or a tool printed, and two passes read it:
//!
//! - [`scrub_known_shapes`], run by migration 293 on every install: a value
//!   with a vendor-shaped token (`ghp_…`, `AIza…`, `sk-…`, a JWT, a `Bearer`
//!   value, a `"password": "…"` field) is masked with
//!   [`crate::core::redact::redact_known_shapes`], and nothing else of the history is
//!   touched;
//! - [`redact_discussion`], run by the owner on **one** discussion they know was
//!   exposed (`POST /api/maintenance/redact-discussion`): the audit redaction
//!   (secret-named assignments and JSON keys too) over every column of every
//!   table that belongs to it, whatever the role of the message.
//!
//! Both are idempotent and rewrite only the values they change.
//!
//! The tables that are **not** in the list cannot hold such output, or hold it
//! under another guarantee: `FTS` shadow tables do not exist in the schema (no
//! `fts5` / virtual table is created anywhere), so nothing needs a rebuild after
//! the rewrite. See `docs/architecture/agent-vault-protection.md` for the
//! per-table reasoning.

use anyhow::{bail, Result};
use rusqlite::Connection;

/// A column that can hold what an agent or a tool printed.
pub(crate) struct Target {
    pub table: &'static str,
    pub column: &'static str,
    /// Extra condition of [`scrub_known_shapes`]: an agent message only, since
    /// what a human typed is theirs.
    pub generic_where: Option<&'static str>,
    /// Condition that selects the rows of one discussion (`?1` is its id).
    /// `None`: the table has no link to a discussion.
    pub discussion_where: Option<&'static str>,
}

const fn target(
    table: &'static str,
    column: &'static str,
    discussion_where: Option<&'static str>,
) -> Target {
    Target {
        table,
        column,
        generic_where: None,
        discussion_where,
    }
}

const BY_DISCUSSION: Option<&str> = Some("discussion_id = ?1");
const BY_PROPOSAL: Option<&str> =
    Some("proposal_id IN (SELECT id FROM planning_proposals WHERE discussion_id = ?1)");
const BY_EXECUTION: Option<&str> =
    Some("task_execution_id IN (SELECT id FROM task_executions WHERE sub_discussion_id = ?1)");

/// Every column that can carry an agent transcript or tool output.
pub(crate) const TARGETS: &[Target] = &[
    // The transcript itself, and the copies a deletion or an edit keeps.
    Target {
        table: "messages",
        column: "content",
        generic_where: Some("role = 'Agent'"),
        discussion_where: BY_DISCUSSION,
    },
    target("message_tombstones", "content", BY_DISCUSSION),
    target("message_revision_events", "content", BY_DISCUSSION),
    // Text written from the transcript, or from a turn still in flight.
    target("discussions", "partial_response", Some("id = ?1")),
    target("discussions", "summary_cache", Some("id = ?1")),
    target("disc_summary_ranges", "summary", BY_DISCUSSION),
    target(
        "discussion_important_messages",
        "payload_json",
        BY_DISCUSSION,
    ),
    target("discussion_questions", "payload_json", BY_DISCUSSION),
    target("discussion_questions", "answer_json", BY_DISCUSSION),
    target("discussion_questions", "resume_error", BY_DISCUSSION),
    // The cards ingested from the `kronn-*` fences of an agent message.
    target("discussion_actions", "values_json", BY_DISCUSSION),
    target("discussion_actions", "diagnostic", BY_DISCUSSION),
    target("planning_proposal_items", "payload_json", BY_PROPOSAL),
    target("planning_proposal_items", "rejected_reason", BY_PROPOSAL),
    // Dispatch and resume of the agent turns.
    target("agent_dispatch_jobs", "last_error", BY_DISCUSSION),
    target("agent_dispatch_jobs", "progress_detail", BY_DISCUSSION),
    target("agent_resume_jobs", "result_json", BY_DISCUSSION),
    target("agent_resume_jobs", "last_error", BY_DISCUSSION),
    target("agent_resume_jobs", "reason", BY_DISCUSSION),
    target("shared_runs", "result_json", BY_DISCUSSION),
    target("shared_runs", "exec_details_json", BY_DISCUSSION),
    target("shared_runs", "diagnostic", BY_DISCUSSION),
    // Task executions: what the worker delivered, what the principal decided,
    // what the validation commands printed, why it stopped.
    target("task_execution_deliveries", "manifest_json", BY_EXECUTION),
    target("task_execution_reviews", "decision_json", BY_EXECUTION),
    target("task_execution_validation_runs", "output", BY_EXECUTION),
    target(
        "task_executions",
        "blocked_reason",
        Some("sub_discussion_id = ?1"),
    ),
    target(
        "task_executions",
        "outcome_reason",
        Some("sub_discussion_id = ?1"),
    ),
    target(
        "task_execution_events",
        "changes_json",
        Some("task_execution_id IN (SELECT id FROM task_executions WHERE sub_discussion_id = ?1)"),
    ),
    // Planning: the evidence a worker attaches to a DoD item, the events that
    // record it.
    target("planning_task_dod_items", "evidence", None),
    target("planning_task_events", "changes_json", None),
    target("planning_tasks", "description", None),
    target("planning_tasks", "blocked_reason", None),
    target("review_findings", "evidence", None),
    target("learnings", "evidence_json", BY_DISCUSSION),
    // Workflows and the quick runs: the output of each step, and the failures.
    target("workflow_runs", "step_results_json", None),
    target("workflow_run_pins", "content_json", None),
    target("workflow_watch_state", "last_result", None),
    target("workflow_watch_state", "last_error", None),
    target("quick_exec_runs", "summary", None),
    target("quick_exec_runs", "diagnostics", None),
    target("delivery_summaries", "canonical_json", BY_DISCUSSION),
    target("tasks", "last_status_json", None),
    // What a tool call returned, and what a probe or a call reported.
    target("api_call_logs", "request_excerpt", Some("disc_id = ?1")),
    target("api_call_logs", "response_excerpt", Some("disc_id = ?1")),
    target("api_call_logs", "error_message", Some("disc_id = ?1")),
    target("media_jobs", "last_error", BY_DISCUSSION),
    target("mcp_probe_results", "summary", None),
    target("mcp_auth_failures", "message", None),
    target("lite_llm_model_failures", "error_message", None),
    target("trust_audit_log", "detail", Some("discussion_id = ?1")),
    // Audit runs: the notes and recommendations an agent wrote.
    target("audit_runs", "documentary_notes", None),
    target("audit_runs", "recommendations_json", None),
    target("audit_runs", "step_outcomes_json", None),
    target("audit_cross_findings", "record_json", None),
];

/// What a pass rewrote, per table and column, for the log and the response.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Rewritten {
    pub total: usize,
    pub by_column: Vec<(String, usize)>,
}

impl Rewritten {
    fn add(&mut self, table: &str, column: &str, count: usize) {
        if count > 0 {
            self.total += count;
            self.by_column.push((format!("{table}.{column}"), count));
        }
    }
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    // The names are compile-time constants of `TARGETS`, never caller input.
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        [table, column],
        |row| row.get(0),
    )?)
}

/// Rewrite the text values of `target` that `mask` changes, among the rows
/// `condition` selects. A value that is not text (NULL, a number) is left alone.
fn rewrite(
    conn: &Connection,
    target: &Target,
    condition: &str,
    params: &[&dyn rusqlite::ToSql],
    mask: &dyn Fn(&str) -> String,
) -> Result<usize> {
    if !has_column(conn, target.table, target.column)? {
        return Ok(0);
    }
    let select = format!(
        "SELECT rowid, {column} FROM {table} WHERE {column} IS NOT NULL AND ({condition})",
        table = target.table,
        column = target.column,
    );
    let mut changed: Vec<(i64, String)> = Vec::new();
    {
        let mut statement = conn.prepare(&select)?;
        let mut rows = statement.query(params)?;
        while let Some(row) = rows.next()? {
            let rowid: i64 = row.get(0)?;
            let Ok(value) = row.get_ref(1)?.as_str() else {
                continue;
            };
            let masked = mask(value);
            if masked != value {
                changed.push((rowid, masked));
            }
        }
    }
    let update = format!(
        "UPDATE {table} SET {column} = ?1 WHERE rowid = ?2",
        table = target.table,
        column = target.column,
    );
    for (rowid, value) in &changed {
        conn.execute(&update, rusqlite::params![value, rowid])?;
    }
    Ok(changed.len())
}

/// Migration 293: mask the vendor-shaped credentials every [`TARGETS`] column
/// holds. Returns what it rewrote.
pub(crate) fn scrub_known_shapes(conn: &Connection) -> Result<Rewritten> {
    let mut report = Rewritten::default();
    for target in TARGETS {
        let condition = target.generic_where.unwrap_or("1 = 1");
        let count = rewrite(conn, target, condition, &[], &|text| {
            crate::core::redact::redact_known_shapes(text)
        })?;
        report.add(target.table, target.column, count);
    }
    if report.total > 0 {
        tracing::warn!(
            "KT-1656: masked known-shape credentials in {} stored value(s)",
            report.total
        );
    }
    Ok(report)
}

/// The id of the discussion `reference` names: the exact id, or a prefix of at
/// least eight characters that only one discussion starts with.
pub(crate) fn resolve_discussion(conn: &Connection, reference: &str) -> Result<String> {
    let reference = reference.trim();
    if reference.len() < 8 || !reference.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        bail!("give a discussion id: the full id, or at least its first 8 characters");
    }
    let mut statement = conn.prepare(
        "SELECT id FROM discussions WHERE id = ?1 OR id LIKE ?1 || '%' ORDER BY id LIMIT 3",
    )?;
    let ids: Vec<String> = statement
        .query_map([reference], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    match ids.as_slice() {
        [] => bail!("no discussion starts with {reference}"),
        [one] => Ok(one.clone()),
        _ if ids.iter().any(|id| id == reference) => Ok(reference.to_string()),
        _ => bail!("{reference} matches several discussions: give more of the id"),
    }
}

/// The owner-run audit of one discussion: the audit redaction over every column
/// of every table that belongs to it, whatever the role of the message. Returns
/// the id it resolved and what it rewrote.
pub(crate) fn redact_discussion(conn: &Connection, reference: &str) -> Result<(String, Rewritten)> {
    let discussion_id = resolve_discussion(conn, reference)?;
    let mut report = Rewritten::default();
    for target in TARGETS {
        let Some(condition) = target.discussion_where else {
            continue;
        };
        let count = rewrite(conn, target, condition, &[&discussion_id], &|text| {
            crate::core::redact::redact_for_audit_artifact(text).0
        })?;
        report.add(target.table, target.column, count);
    }
    tracing::warn!(
        "KT-1656: the audit redaction rewrote {} stored value(s) of discussion {}",
        report.total,
        discussion_id
    );
    Ok((discussion_id, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `cat ~/.gemini/settings.json` prints: no vendor shape on the values.
    const SETTINGS: &str = r#"{"mcpServers": {"github": {"env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "t0kenNotShapedLikeAVendor01", "LOG_LEVEL": "debug"}}}}"#;
    const SECRET: &str = "t0kenNotShapedLikeAVendor01";
    const ROOM_A: &str = "dddddddd-0000-4000-8000-000000000001";
    const ROOM_B: &str = "eeeeeeee-0000-4000-8000-000000000002";

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn insert_discussion(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at, summary_cache, partial_response)
             VALUES (?1, 't', '2026-11-10T00:00:00Z', '2026-11-10T00:00:00Z', ?2, ?2)",
            rusqlite::params![id, SETTINGS],
        )
        .unwrap();
        for (order, role) in ["Agent", "System", "User"].into_iter().enumerate() {
            conn.execute(
                "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order)
                 VALUES (?1, ?2, ?3, ?4, '2026-11-10T00:00:00Z', ?5)",
                rusqlite::params![format!("{id}-{role}"), id, role, SETTINGS, order as i64],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO message_tombstones (id, discussion_id, role, content, timestamp, sort_order, revision_event_id, tombstoned_at)
             VALUES (?1, ?2, 'Agent', ?3, '2026-11-10T00:00:00Z', 9, 'rev', '2026-11-10T00:00:00Z')",
            rusqlite::params![format!("{id}-tomb"), id, SETTINGS],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO disc_summary_ranges (discussion_id, from_idx, to_idx, summary, created_at)
             VALUES (?1, 0, 1, ?2, '2026-11-10T00:00:00Z')",
            rusqlite::params![id, SETTINGS],
        )
        .unwrap();
    }

    /// How many of the seven places a discussion keeps still hold the secret.
    fn count_holding(conn: &Connection, discussion: &str) -> i64 {
        let holds = |sql: &str| -> i64 {
            conn.query_row(sql, [discussion, SECRET], |row| row.get(0))
                .unwrap()
        };
        holds("SELECT COUNT(*) FROM messages WHERE discussion_id = ?1 AND instr(content, ?2) > 0")
            + holds(
                "SELECT COUNT(*) FROM message_tombstones WHERE discussion_id = ?1 AND instr(content, ?2) > 0",
            )
            + holds(
                "SELECT COUNT(*) FROM disc_summary_ranges WHERE discussion_id = ?1 AND instr(summary, ?2) > 0",
            )
            + holds(
                "SELECT COUNT(*) FROM discussions WHERE id = ?1 AND instr(summary_cache, ?2) > 0",
            )
            + holds(
                "SELECT COUNT(*) FROM discussions WHERE id = ?1 AND instr(partial_response, ?2) > 0",
            )
    }

    /// A column named in `TARGETS` that the schema renamed would make a pass
    /// skip it silently (`has_column`): the list is checked against the schema.
    #[test]
    fn every_target_names_a_column_that_exists() {
        let conn = database();
        for target in TARGETS {
            assert!(
                has_column(&conn, target.table, target.column).unwrap(),
                "{}.{} is not in the schema",
                target.table,
                target.column
            );
            // And its discussion condition prepares.
            if let Some(condition) = target.discussion_where {
                conn.prepare(&format!(
                    "SELECT rowid FROM {} WHERE {}",
                    target.table, condition
                ))
                .unwrap_or_else(|error| panic!("{}: {error}", target.table));
            }
        }
        // No FTS or other virtual table holds a copy that needs a rebuild.
        let virtual_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE sql LIKE 'CREATE VIRTUAL TABLE%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(virtual_tables, 0);
    }

    #[test]
    fn the_audit_redacts_one_discussion_in_every_table_and_names_no_other() {
        let conn = database();
        insert_discussion(&conn, ROOM_A);
        insert_discussion(&conn, ROOM_B);
        // Known-shape upgrade pass: the unshaped token is not its business.
        assert_eq!(scrub_known_shapes(&conn).unwrap().total, 0);

        let (id, report) = redact_discussion(&conn, "dddddddd").unwrap();
        assert_eq!(id, ROOM_A);
        // 3 messages (any role), a tombstone, a range, summary and partial.
        assert_eq!(report.total, 7, "{report:?}");
        assert_eq!(count_holding(&conn, &id), 0);
        assert_eq!(
            count_holding(&conn, ROOM_B),
            7,
            "another discussion is left as written"
        );
        let kept: String = conn
            .query_row(
                "SELECT content FROM messages WHERE id = ?1",
                [format!("{ROOM_A}-User")],
                |row| row.get(0),
            )
            .unwrap();
        assert!(kept.contains("\"LOG_LEVEL\": \"debug\""), "{kept}");
        serde_json::from_str::<serde_json::Value>(&kept).unwrap();

        // One-shot and idempotent.
        assert_eq!(redact_discussion(&conn, &id).unwrap().1.total, 0);
    }

    /// The shapes of the other agents' login files are masked by the upgrade
    /// pass (`~/.gemini/oauth_creds.json`, fine-grained GitHub, STS keys).
    #[test]
    fn the_upgrade_pass_masks_the_oauth_fine_grained_and_sts_shapes() {
        const SHAPES: [&str; 4] = [
            "ya29.a0AfH6SMBx3Zk9QwErTyUiOpAsDfGhJkLzXcVbNm12345",
            "1//0gAbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdEfGhIjKlMnOpQrStUv",
            "github_pat_11ABCDEFG0abcdefghijkl_0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefghijk",
            "ASIAY34FZKBOKMUTVV7A",
        ];
        let conn = database();
        insert_discussion(&conn, ROOM_A);
        for (order, shape) in SHAPES.iter().enumerate() {
            conn.execute(
                "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order)
                 VALUES (?1, ?2, 'Agent', ?3, '2026-11-10T00:00:00Z', ?4)",
                rusqlite::params![
                    format!("shape-{order}"),
                    ROOM_A,
                    format!("{{\"a\": \"{shape}\"}} and prose"),
                    100 + order as i64
                ],
            )
            .unwrap();
        }
        let report = scrub_known_shapes(&conn).unwrap();
        assert_eq!(report.total, 4, "{report:?}");
        for (order, shape) in SHAPES.iter().enumerate() {
            let content: String = conn
                .query_row(
                    "SELECT content FROM messages WHERE id = ?1",
                    [format!("shape-{order}")],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(!content.contains(shape), "{content}");
            assert!(content.ends_with(" and prose"), "{content}");
            serde_json::from_str::<serde_json::Value>(content.trim_end_matches(" and prose"))
                .unwrap();
        }
        assert_eq!(scrub_known_shapes(&conn).unwrap().total, 0, "idempotent");
    }

    #[test]
    fn a_reference_must_name_exactly_one_discussion() {
        let conn = database();
        insert_discussion(&conn, "abcdef01-0000-4000-8000-000000000001");
        insert_discussion(&conn, "abcdef01-0000-4000-8000-000000000002");
        assert!(resolve_discussion(&conn, "abcdef01").is_err(), "ambiguous");
        assert!(resolve_discussion(&conn, "abcdef0").is_err(), "too short");
        assert!(resolve_discussion(&conn, "zzzzzzzz").is_err(), "not an id");
        assert!(resolve_discussion(&conn, "12345678").is_err(), "no match");
        assert!(resolve_discussion(&conn, "%%%%%%%%").is_err(), "wildcards");
        assert_eq!(
            resolve_discussion(&conn, "abcdef01-0000-4000-8000-000000000002").unwrap(),
            "abcdef01-0000-4000-8000-000000000002"
        );
    }
}
