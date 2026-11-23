//! The Artifacts a person made from the executions of a step (KT-1990): one
//! per execution (the main one or a shadow), found again by the run view, and
//! grouped per run in the Artifact library. Only the human route of the run view
//! writes a link ([`link`]).

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// One execution's Artifact.
#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct StepArtifactLink {
    pub page_id: String,
    pub run_id: String,
    pub result_index: u32,
    pub step_name: String,
    /// `""` for the main execution.
    pub shadow_id: String,
    pub label: String,
    #[ts(optional)]
    pub model: Option<String>,
    pub created_at: String,
}

/// The Artifacts of one run, as the library folder shows them.
#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct StepArtifactFolder {
    pub run_id: String,
    pub workflow_id: String,
    pub workflow_name: String,
    /// The run's place among its workflow's runs, the oldest being #1, as the
    /// batch folders of the Discussions sidebar count them.
    pub run_number: u32,
    pub run_started_at: String,
    /// Step by step in run order; per step the main execution first, then the
    /// shadows in the order they started.
    pub artifacts: Vec<StepArtifactLink>,
}

fn map_link(row: &rusqlite::Row<'_>) -> rusqlite::Result<StepArtifactLink> {
    Ok(StepArtifactLink {
        page_id: row.get(0)?,
        run_id: row.get(1)?,
        result_index: row.get(2)?,
        step_name: row.get(3)?,
        shadow_id: row.get(4)?,
        label: row.get(5)?,
        model: row.get(6)?,
        created_at: row.get(7)?,
    })
}

const LINK_COLUMNS: &str = "a.page_id, a.run_id, a.result_index, a.step_name, a.shadow_id,
                            a.label, a.model, a.created_at";

/// The order of a run's executions: step by step, the main execution first,
/// then the shadows as they started (the order of `step_shadow_runs`).
const EXECUTION_ORDER: &str = "a.result_index, (a.shadow_id <> ''),
     (SELECT s.started_at FROM step_shadow_runs s
       WHERE s.run_id = a.run_id AND s.result_index = a.result_index
         AND s.shadow_id = a.shadow_id),
     a.created_at";

/// The Artifact of one execution, if a person made one.
pub fn for_execution(
    conn: &Connection,
    run_id: &str,
    result_index: u32,
    shadow_id: &str,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT page_id FROM step_execution_artifacts
              WHERE run_id = ?1 AND result_index = ?2 AND shadow_id = ?3",
            params![run_id, result_index, shadow_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// The Artifacts of a run, keyed by (`result_index`, `shadow_id`).
pub fn for_run(conn: &Connection, run_id: &str) -> Result<HashMap<(u32, String), String>> {
    let mut stmt = conn.prepare(
        "SELECT result_index, shadow_id, page_id FROM step_execution_artifacts WHERE run_id = ?1",
    )?;
    let rows = stmt
        .query_map([run_id], |row| {
            Ok((
                (row.get::<_, u32>(0)?, row.get::<_, String>(1)?),
                row.get(2)?,
            ))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    Ok(rows)
}

/// Records that `link.page_id` was made from its execution. The caller creates
/// the Artifact in the same transaction; a second link for the same execution
/// is refused by the table's unique key.
pub fn link(conn: &Connection, link: &StepArtifactLink) -> Result<()> {
    conn.execute(
        "INSERT INTO step_execution_artifacts
             (page_id, run_id, result_index, step_name, shadow_id, label, model, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            link.page_id,
            link.run_id,
            link.result_index,
            link.step_name,
            link.shadow_id,
            link.label,
            link.model,
            link.created_at,
        ],
    )?;
    Ok(())
}

/// Every run that has Artifacts, the most recent run first. A run's number
/// counts the runs of its workflow that started before it; between two runs
/// that started at the same instant the id breaks the tie, so no two runs of a
/// workflow share a number.
pub fn list_folders(conn: &Connection) -> Result<Vec<StepArtifactFolder>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {LINK_COLUMNS}, r.workflow_id, w.name, r.started_at,
                (SELECT COUNT(*) FROM workflow_runs o
                  WHERE o.workflow_id = r.workflow_id
                    AND (o.started_at < r.started_at
                         OR (o.started_at = r.started_at AND o.id <= r.id)))
           FROM step_execution_artifacts a
           JOIN workflow_runs r ON r.id = a.run_id
           JOIN workflows w ON w.id = r.workflow_id
          ORDER BY r.started_at DESC, a.run_id DESC, {EXECUTION_ORDER}"
    ))?;
    let mut folders: Vec<StepArtifactFolder> = Vec::new();
    let rows = stmt.query_map([], |row| {
        Ok((
            map_link(row)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, String>(10)?,
            row.get::<_, i64>(11)?,
        ))
    })?;
    for row in rows {
        let (link, workflow_id, workflow_name, started_at, number) = row?;
        match folders.last_mut() {
            Some(folder) if folder.run_id == link.run_id => folder.artifacts.push(link),
            _ => folders.push(StepArtifactFolder {
                run_id: link.run_id.clone(),
                workflow_id,
                workflow_name,
                run_number: u32::try_from(number).unwrap_or(0),
                run_started_at: started_at,
                artifacts: vec![link],
            }),
        }
    }
    Ok(folders)
}

/// The Artifacts made from the runs a delete would remove: one run, or every
/// run of a workflow.
pub enum RunScope<'a> {
    Run(&'a str),
    Workflow(&'a str),
}

impl RunScope<'_> {
    fn clause(&self) -> (&'static str, &str) {
        match self {
            RunScope::Run(id) => ("run_id = ?1", id),
            RunScope::Workflow(id) => (
                "run_id IN (SELECT id FROM workflow_runs WHERE workflow_id = ?1)",
                id,
            ),
        }
    }
}

/// How many Artifacts the runs of `scope` have.
pub fn count(conn: &Connection, scope: RunScope<'_>) -> Result<u32> {
    let (clause, id) = scope.clause();
    Ok(conn.query_row(
        &format!("SELECT COUNT(*) FROM step_execution_artifacts WHERE {clause}"),
        [id],
        |row| row.get(0),
    )?)
}

/// Deletes the Artifacts of the runs of `scope` (their revisions, datasets
/// and links go with them). The caller deletes the runs in the same
/// transaction.
pub fn delete(conn: &Connection, scope: RunScope<'_>) -> Result<usize> {
    let (clause, id) = scope.clause();
    Ok(conn.execute(
        &format!(
            "DELETE FROM live_pages WHERE id IN
                 (SELECT page_id FROM step_execution_artifacts WHERE {clause})"
        ),
        [id],
    )?)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
                 VALUES ('wf', 'Jeux é', '\"Manual\"', '[]', '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z'),
                        ('wf-2', 'Other', '\"Manual\"', '[]', '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z');
             INSERT INTO workflow_runs (id, workflow_id, status, started_at)
                 VALUES ('run-1', 'wf', 'Success', '2026-11-01T00:00:00Z'),
                        ('run-2', 'wf', 'Success', '2026-11-02T00:00:00Z'),
                        ('run-3', 'wf-2', 'Success', '2026-11-03T00:00:00Z');",
        )
        .unwrap();
        conn
    }

    fn page(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO live_pages (id, title, slug, current_revision_id, data_revision,
                                     created_at, updated_at, pinned, archived)
             VALUES (?1, ?1, ?1, ?2, 0, '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z', 0, 0)",
            params![id, format!("rev-{id}")],
        )
        .unwrap();
    }

    fn made(conn: &Connection, page_id: &str, run_id: &str, shadow_id: &str) {
        page(conn, page_id);
        link(
            conn,
            &StepArtifactLink {
                page_id: page_id.into(),
                run_id: run_id.into(),
                result_index: 0,
                step_name: "jeu".into(),
                shadow_id: shadow_id.into(),
                label: if shadow_id.is_empty() {
                    "main"
                } else {
                    shadow_id
                }
                .into(),
                model: Some("m".into()),
                created_at: "2026-11-05T00:00:00Z".into(),
            },
        )
        .unwrap();
    }

    fn pages(conn: &Connection) -> Vec<String> {
        conn.prepare("SELECT id FROM live_pages ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn one_artifact_per_execution() {
        let conn = db();
        made(&conn, "p-main", "run-1", "");
        page(&conn, "p-again");
        let mut again = StepArtifactLink {
            page_id: "p-again".into(),
            run_id: "run-1".into(),
            result_index: 0,
            step_name: "jeu".into(),
            shadow_id: String::new(),
            label: "main".into(),
            model: None,
            created_at: "2026-11-05T00:00:00Z".into(),
        };
        assert!(
            link(&conn, &again).is_err(),
            "the same execution is linked once"
        );
        again.shadow_id = "qwen".into();
        link(&conn, &again).unwrap();
        assert_eq!(
            for_execution(&conn, "run-1", 0, "").unwrap().as_deref(),
            Some("p-main")
        );
        assert_eq!(
            for_execution(&conn, "run-1", 0, "qwen").unwrap().as_deref(),
            Some("p-again")
        );
        assert_eq!(for_execution(&conn, "run-1", 1, "").unwrap(), None);
        assert_eq!(for_run(&conn, "run-1").unwrap().len(), 2);
    }

    #[test]
    fn deleting_an_artifact_removes_its_link() {
        let conn = db();
        made(&conn, "p-main", "run-1", "");
        conn.execute("DELETE FROM live_pages WHERE id = 'p-main'", [])
            .unwrap();
        assert_eq!(for_execution(&conn, "run-1", 0, "").unwrap(), None);
        assert!(list_folders(&conn).unwrap().is_empty());
    }

    #[test]
    fn folders_group_a_run_main_first_then_shadows_in_start_order() {
        let conn = db();
        for (shadow, started) in [("", "00"), ("late", "09"), ("early", "05")] {
            conn.execute(
                "INSERT INTO step_shadow_runs
                    (id, run_id, result_index, step_name, shadow_id, status, identity_json, started_at)
                 VALUES (?1, 'run-2', 0, 'jeu', ?2, 'success', '{}', ?3)",
                params![format!("row-{shadow}"), shadow, format!("2026-11-02T00:00:{started}Z")],
            )
            .unwrap();
        }
        made(&conn, "p-late", "run-2", "late");
        made(&conn, "p-main", "run-2", "");
        made(&conn, "p-early", "run-2", "early");
        made(&conn, "p-old", "run-1", "");
        let folders = list_folders(&conn).unwrap();
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].run_id, "run-2", "the most recent run first");
        assert_eq!((folders[0].run_number, folders[1].run_number), (2, 1));
        assert_eq!(folders[0].workflow_name, "Jeux é");
        let order: Vec<&str> = folders[0]
            .artifacts
            .iter()
            .map(|a| a.shadow_id.as_str())
            .collect();
        assert_eq!(order, ["", "early", "late"]);
    }

    /// KT-1999 — two runs of one workflow that started at the same instant get
    /// two numbers: the id breaks the tie.
    #[test]
    fn runs_started_at_the_same_instant_get_distinct_numbers() {
        let conn = db();
        conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, status, started_at)
                 VALUES ('run-2b', 'wf', 'Success', '2026-11-02T00:00:00Z')",
            [],
        )
        .unwrap();
        made(&conn, "p-2", "run-2", "");
        made(&conn, "p-2b", "run-2b", "");
        made(&conn, "p-1", "run-1", "");
        let numbers: Vec<(String, u32)> = list_folders(&conn)
            .unwrap()
            .into_iter()
            .map(|f| (f.run_id, f.run_number))
            .collect();
        let numbers: Vec<(&str, u32)> = numbers.iter().map(|(r, n)| (r.as_str(), *n)).collect();
        assert_eq!(numbers, [("run-2b", 3), ("run-2", 2), ("run-1", 1)]);
    }

    #[test]
    fn a_run_or_workflow_delete_takes_its_artifacts_and_no_other() {
        let conn = db();
        made(&conn, "p-1", "run-1", "");
        made(&conn, "p-2", "run-2", "");
        made(&conn, "p-2b", "run-2", "s");
        made(&conn, "p-3", "run-3", "");
        page(&conn, "p-free");
        assert_eq!(count(&conn, RunScope::Run("run-2")).unwrap(), 2);
        assert_eq!(count(&conn, RunScope::Workflow("wf")).unwrap(), 3);
        assert_eq!(count(&conn, RunScope::Workflow("wf-2")).unwrap(), 1);

        assert_eq!(delete(&conn, RunScope::Run("run-2")).unwrap(), 2);
        assert_eq!(pages(&conn), ["p-1", "p-3", "p-free"]);
        assert_eq!(delete(&conn, RunScope::Workflow("wf")).unwrap(), 1);
        assert_eq!(pages(&conn), ["p-3", "p-free"]);
    }
}
