//! Run retention (KT-984): what may be trimmed from old workflow runs, and how.
//!
//! The default action blanks the step outputs of old runs in place: the row,
//! its status, timings, counters and every link to it stay, so no foreign key
//! cascade fires. Deleting whole rows stays opt-in (`run_retention_days`) and
//! goes through the same eligibility rules.
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

/// Statuses a run never leaves. `Interrupted` and `WaitingApproval` are absent
/// on purpose: they resume from their step results.
const TERMINAL_STATUSES: &str = "'Success','Partial','Failed','Cancelled','StoppedByGuard'";

/// Plain workflow runs. Batch and compare rows carry human and AI ratings and
/// the provenance of their child discussions.
const PLAIN_RUN_TYPES: &str = "'linear','subworkflow'";

/// Every column that points at a workflow run and makes it "referenced": a run
/// named here is never trimmed, whatever its age.
///
/// Columns that point at a run but are not listed here are logs with their own
/// lifetime and read no step output: `agent_decisions.run_id`,
/// `api_call_logs.run_id`, `execution_variable_snapshots.run_id`,
/// `workflow_step_room_sessions.run_id`, `workflow_step_room_activity.run_id`.
pub const REFERENCING_COLUMNS: &[(&str, &str)] = &[
    // A run another run was launched from (sub-workflow, batch, retry).
    ("workflow_runs", "parent_run_id"),
    // The run whose TriggerWorkflow step started this one.
    ("workflow_runs", "triggered_by_run_id"),
    // The run a batch child discussion belongs to.
    ("discussions", "workflow_run_id"),
    // An ad-hoc comparison and its ordered scope.
    ("compare_run_scopes", "run_id"),
    // AI judge passes over a comparison.
    ("batch_compare_judge_runs", "run_id"),
    // Human (`manual_score`) and AI ratings.
    ("batch_compare_evaluations", "run_id"),
    // Live page data points and publications produced by the run.
    ("live_page_dataset_points", "workflow_run_id"),
    ("live_page_publications", "workflow_run_id"),
    // A shared run view, which renders the run's step outputs.
    ("shared_runs", "id"),
    // A question whose answer resumes the run.
    ("discussion_questions", "resume_run_id"),
    // Room messages attributed to one of the run's steps.
    ("workflow_step_room_activities", "run_id"),
];

/// What a blanked step output reads afterwards.
pub const REMOVED_OUTPUT: &str = "[Output removed by run retention]";

/// Rows rewritten per transaction: a step payload can weigh megabytes, and the
/// write connection is shared with every request.
pub const CHUNK_ROWS: usize = 25;

fn not_referenced(alias: &str) -> String {
    REFERENCING_COLUMNS
        .iter()
        .map(|(table, column)| {
            format!(
                " AND {alias}.id NOT IN (SELECT {column} FROM {table} WHERE {column} IS NOT NULL)"
            )
        })
        .collect()
}

/// The runs retention may touch, as a WHERE clause on `alias`. `?1` is the
/// cutoff: an RFC 3339 instant, compared as text like every stored timestamp.
/// A run that still owns a worktree (`workspace_path`) is left alone, and so
/// is a child whose parent can still resume and read it.
pub(crate) fn eligible_runs(alias: &str) -> String {
    format!(
        "{alias}.status IN ({TERMINAL_STATUSES})
         AND {alias}.finished_at IS NOT NULL
         AND {alias}.finished_at < ?1
         AND {alias}.run_type IN ({PLAIN_RUN_TYPES})
         AND {alias}.workspace_path IS NULL
         AND NOT EXISTS (
             SELECT 1 FROM workflow_runs parent
              WHERE parent.id = {alias}.parent_run_id
                AND parent.status NOT IN ({TERMINAL_STATUSES})
         ){}",
        not_referenced(alias)
    )
}

/// The candidates of one compaction chunk, oldest first. Kept separate so the
/// query-plan test checks the exact statement the purge runs. The index is
/// named: on a table without statistics the planner may pick another one and
/// read every candidate's row, payload included.
pub(crate) fn payload_candidates_sql() -> String {
    format!(
        "SELECT run.id FROM workflow_runs run INDEXED BY idx_workflow_runs_payload_retention
          WHERE run.payload_compacted_at IS NULL AND {}
          ORDER BY run.finished_at
          LIMIT ?2",
        eligible_runs("run")
    )
}

/// The cutoff for a retention of `days`.
pub fn cutoff(now: DateTime<Utc>, days: u32) -> String {
    (now - chrono::Duration::days(i64::from(days))).to_rfc3339()
}

/// Blank the step outputs of at most `limit` eligible runs finished before
/// `cutoff`, keeping every other field of each step. Returns the rows changed;
/// fewer than `limit` means nothing is left to do.
pub fn compact_run_payloads_chunk(conn: &Connection, cutoff: &str, limit: usize) -> Result<usize> {
    let sql = format!(
        "UPDATE workflow_runs
            SET step_results_json = CASE WHEN json_valid(step_results_json)
                    THEN (SELECT json_group_array(
                              CASE WHEN COALESCE(json_extract(value, '$.output'), '') = ''
                                   THEN json(value)
                                   ELSE json_set(value, '$.output', ?3) END)
                            FROM json_each(step_results_json))
                    ELSE '[]' END,
                payload_compacted_at = ?4
          WHERE id IN ({})",
        payload_candidates_sql()
    );
    Ok(conn.execute(
        &sql,
        params![
            cutoff,
            limit as i64,
            REMOVED_OUTPUT,
            Utc::now().to_rfc3339()
        ],
    )?)
}

/// Delete at most `limit` eligible runs finished before `cutoff`. Opt-in only
/// (`run_retention_days`), same rules as the payload trim.
pub fn delete_runs_chunk(conn: &Connection, cutoff: &str, limit: usize) -> Result<usize> {
    let sql = format!(
        "DELETE FROM workflow_runs
          WHERE id IN (SELECT run.id FROM workflow_runs run
                        WHERE {}
                        ORDER BY run.finished_at
                        LIMIT ?2)",
        eligible_runs("run")
    );
    Ok(conn.execute(&sql, params![cutoff, limit as i64])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "2026-01-01T00:00:00+00:00";
    const CUTOFF: &str = "2026-06-01T00:00:00+00:00";

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        // Reference rows below point at runs only; their other parents are
        // irrelevant to what retention reads.
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn.execute(
            "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
             VALUES ('wf', 'Revue é', '\"Manual\"', '[]', ?1, ?1)",
            [OLD],
        )
        .unwrap();
        conn
    }

    fn payload(output: &str) -> String {
        serde_json::json!([
            {"step_name": "Analyse 🚀", "status": "Success", "output": output,
             "tokens_used": 12, "duration_ms": 30},
            {"step_name": "vide", "status": "Success", "output": "",
             "tokens_used": null, "duration_ms": 1}
        ])
        .to_string()
    }

    fn run(conn: &Connection, id: &str, status: &str, run_type: &str, finished_at: Option<&str>) {
        conn.execute(
            "INSERT INTO workflow_runs
                 (id, workflow_id, status, step_results_json, tokens_used, started_at,
                  finished_at, run_type)
             VALUES (?1, 'wf', ?2, ?3, 42, ?4, ?5, ?6)",
            params![
                id,
                status,
                payload(&"x".repeat(50_000)),
                OLD,
                finished_at,
                run_type
            ],
        )
        .unwrap();
    }

    /// Insert a row in `table` whose `column` names `run_id`, filling every
    /// other required column with a placeholder.
    fn reference(conn: &Connection, table: &str, column: &str, run_id: &str) {
        let columns: Vec<(String, String, bool, bool)> = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, Option<String>>(4)?.is_some(),
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut names = Vec::new();
        let mut values: Vec<String> = Vec::new();
        for (name, kind, not_null, has_default) in columns {
            let value = if name == column {
                run_id.to_string()
            } else if table == "shared_runs" && name == "kind" {
                "workflow".into()
            } else if table == "shared_runs" && name == "status" {
                "success".into()
            } else if not_null && !has_default {
                if kind.eq_ignore_ascii_case("INTEGER") {
                    "1".into()
                } else {
                    format!("{table}-{name}-{run_id}")
                }
            } else {
                continue;
            };
            names.push(name);
            values.push(value);
        }
        let placeholders: Vec<String> = (1..=values.len()).map(|i| format!("?{i}")).collect();
        conn.execute(
            &format!(
                "INSERT INTO {table} ({}) VALUES ({})",
                names.join(", "),
                placeholders.join(", ")
            ),
            rusqlite::params_from_iter(values.iter()),
        )
        .unwrap_or_else(|e| panic!("seed {table}.{column}: {e}"));
    }

    fn compacted(conn: &Connection, id: &str) -> bool {
        conn.query_row(
            "SELECT payload_compacted_at IS NOT NULL FROM workflow_runs WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn compact_all(conn: &Connection) -> usize {
        let mut total = 0;
        loop {
            let n = compact_run_payloads_chunk(conn, CUTOFF, CHUNK_ROWS).unwrap();
            total += n;
            if n < CHUNK_ROWS {
                return total;
            }
        }
    }

    #[test]
    fn only_old_terminal_plain_unreferenced_runs_are_trimmed() {
        // KT-984 DAT-1 / WF-9 — the purge deleted compares, batches and runs
        // still owning a worktree, cascading into human ratings.
        let conn = db();
        run(&conn, "plain", "Success", "linear", Some(OLD));
        run(&conn, "plain-failed", "Failed", "linear", Some(OLD));
        run(&conn, "parent-done", "Success", "linear", Some(OLD));
        run(&conn, "sub-of-done", "Success", "subworkflow", Some(OLD));
        conn.execute(
            "UPDATE workflow_runs SET parent_run_id = 'parent-done' WHERE id = 'sub-of-done'",
            [],
        )
        .unwrap();

        run(
            &conn,
            "recent",
            "Success",
            "linear",
            Some("2026-09-01T00:00:00+00:00"),
        );
        run(&conn, "running", "Running", "linear", None);
        run(&conn, "interrupted", "Interrupted", "linear", Some(OLD));
        run(&conn, "waiting", "WaitingApproval", "linear", Some(OLD));
        run(&conn, "worktree", "Success", "linear", Some(OLD));
        conn.execute(
            "UPDATE workflow_runs SET workspace_path = '/repo/.kronn/worktrees/run' WHERE id = 'worktree'",
            [],
        )
        .unwrap();
        run(&conn, "batch", "Success", "batch", Some(OLD));
        run(&conn, "compare", "Success", "compare", Some(OLD));
        run(
            &conn,
            "waiting-parent",
            "WaitingApproval",
            "linear",
            Some(OLD),
        );
        run(
            &conn,
            "child-of-waiting",
            "Success",
            "subworkflow",
            Some(OLD),
        );
        conn.execute(
            "UPDATE workflow_runs SET parent_run_id = 'waiting-parent' WHERE id = 'child-of-waiting'",
            [],
        )
        .unwrap();

        let mut referenced = Vec::new();
        for (table, column) in REFERENCING_COLUMNS {
            let id = format!("ref-{table}-{column}");
            run(&conn, &id, "Success", "linear", Some(OLD));
            if *table == "workflow_runs" {
                // The referencing row is another, recent run.
                let child = format!("{id}-child");
                run(
                    &conn,
                    &child,
                    "Success",
                    "linear",
                    Some("2026-09-01T00:00:00+00:00"),
                );
                conn.execute(
                    &format!("UPDATE workflow_runs SET {column} = ?1 WHERE id = ?2"),
                    params![id, child],
                )
                .unwrap();
            } else {
                reference(&conn, table, column, &id);
            }
            referenced.push(id);
        }

        let trimmed = compact_all(&conn);

        let expected = ["plain", "plain-failed", "sub-of-done"];
        assert_eq!(trimmed, expected.len());
        for id in expected {
            assert!(compacted(&conn, id), "{id} is trimmed");
        }
        for id in [
            "parent-done",
            "recent",
            "running",
            "interrupted",
            "waiting",
            "worktree",
            "batch",
            "compare",
            "waiting-parent",
            "child-of-waiting",
        ]
        .iter()
        .map(|id| id.to_string())
        .chain(referenced)
        {
            assert!(!compacted(&conn, &id), "{id} is left alone");
        }
        let untouched: String = conn
            .query_row(
                "SELECT step_results_json FROM workflow_runs WHERE id = 'compare'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(untouched.contains(&"x".repeat(50_000)));
    }

    #[test]
    fn a_trimmed_run_keeps_its_steps_and_metadata() {
        let conn = db();
        run(&conn, "plain", "Success", "linear", Some(OLD));
        compact_all(&conn);
        let (results, tokens, status, finished): (String, i64, String, String) = conn
            .query_row(
                "SELECT step_results_json, tokens_used, status, finished_at
                   FROM workflow_runs WHERE id = 'plain'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (tokens, status.as_str(), finished.as_str()),
            (42, "Success", OLD)
        );
        let steps: Vec<crate::models::StepResult> = serde_json::from_str(&results).unwrap();
        assert_eq!(steps.len(), 2, "every step stays decodable");
        assert_eq!(steps[0].step_name, "Analyse 🚀");
        assert_eq!(steps[0].output, REMOVED_OUTPUT);
        assert_eq!(steps[0].tokens_used, Some(12));
        assert_eq!(steps[1].output, "", "an empty output stays empty");
        assert!(
            results.len() < 1_000,
            "the payload is gone: {} bytes",
            results.len()
        );
        assert_eq!(
            compact_all(&conn),
            0,
            "a trimmed run is not rewritten again"
        );
    }

    #[test]
    fn the_trim_works_in_bounded_chunks() {
        // KT-984 DAT-3 — one unbounded DELETE held the write connection.
        let conn = db();
        for index in 0..(CHUNK_ROWS * 2 + 3) {
            run(
                &conn,
                &format!("run-{index:03}"),
                "Success",
                "linear",
                Some(OLD),
            );
        }
        assert_eq!(
            compact_run_payloads_chunk(&conn, CUTOFF, CHUNK_ROWS).unwrap(),
            CHUNK_ROWS
        );
        assert_eq!(
            compact_run_payloads_chunk(&conn, CUTOFF, CHUNK_ROWS).unwrap(),
            CHUNK_ROWS
        );
        assert_eq!(
            compact_run_payloads_chunk(&conn, CUTOFF, CHUNK_ROWS).unwrap(),
            3
        );
        assert_eq!(
            compact_run_payloads_chunk(&conn, CUTOFF, CHUNK_ROWS).unwrap(),
            0
        );
    }

    #[test]
    fn invalid_step_json_becomes_an_empty_list() {
        let conn = db();
        run(&conn, "corrupt", "Success", "linear", Some(OLD));
        conn.execute(
            "UPDATE workflow_runs SET step_results_json = '{not json' WHERE id = 'corrupt'",
            [],
        )
        .unwrap();
        compact_all(&conn);
        let results: String = conn
            .query_row(
                "SELECT step_results_json FROM workflow_runs WHERE id = 'corrupt'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(results, "[]");
    }

    #[test]
    fn opt_in_deletion_follows_the_same_rules() {
        let conn = db();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run(&conn, "plain", "Success", "linear", Some(OLD));
        run(&conn, "compare", "Success", "compare", Some(OLD));
        run(&conn, "interrupted", "Interrupted", "linear", Some(OLD));
        conn.execute(
            "INSERT INTO compare_run_scopes (run_id, selection_key, created_at) VALUES ('compare', 'k', ?1)",
            [OLD],
        )
        .unwrap();
        assert_eq!(delete_runs_chunk(&conn, CUTOFF, CHUNK_ROWS).unwrap(), 1);
        let left: Vec<String> = conn
            .prepare("SELECT id FROM workflow_runs ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(left, ["compare", "interrupted"]);
        let scopes: i64 = conn
            .query_row("SELECT COUNT(*) FROM compare_run_scopes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(scopes, 1, "the comparison scope survives");
    }

    #[test]
    fn candidates_are_found_without_reading_run_payloads() {
        // Every column the selection reads sits after step_results_json.
        let conn = db();
        let plan = crate::db::query_plan(&conn, &payload_candidates_sql());
        assert!(
            crate::db::table_reads_outside_index(&plan, &["run", "workflow_runs"]).is_empty(),
            "{plan:?}"
        );
        assert!(
            plan.iter()
                .any(|line| line.contains("idx_workflow_runs_payload_retention")),
            "{plan:?}"
        );
    }

    #[test]
    fn every_referencing_column_exists() {
        let conn = db();
        for (table, column) in REFERENCING_COLUMNS {
            conn.prepare(&format!("SELECT {column} FROM {table} LIMIT 0"))
                .unwrap_or_else(|e| panic!("{table}.{column}: {e}"));
        }
    }

    #[test]
    fn every_foreign_key_to_workflow_runs_is_classified() {
        // A new foreign key to runs must be weighed and listed, never forgotten.
        let conn = db();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for table in tables {
            let keys: Vec<(String, String)> = conn
                .prepare(&format!("PRAGMA foreign_key_list(\"{table}\")"))
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get::<_, String>(2)?, row.get::<_, String>(3)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            for (parent, column) in keys {
                if parent != "workflow_runs" {
                    continue;
                }
                let known = REFERENCING_COLUMNS
                    .iter()
                    .any(|(t, c)| *t == table && *c == column);
                assert!(
                    known,
                    "{table}.{column} references workflow_runs and is not classified"
                );
            }
        }
    }

    #[test]
    fn cutoff_is_comparable_with_stored_timestamps() {
        let now = DateTime::parse_from_rfc3339("2026-10-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(cutoff(now, 30), "2026-09-05T12:00:00+00:00");
    }
}
