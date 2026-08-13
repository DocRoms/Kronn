//! Durable baselines for the Context Architecture Audit (KT-260).

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::core::context_audit::ContextAudit;

/// Load a project's baseline, creating it from `current` when absent.
///
/// Read + write happen in one SQLite transaction so concurrent refreshes cannot
/// both compare against the same stale baseline and then lose one observation.
pub fn load_or_create_snapshot(
    conn: &Connection,
    project_id: &str,
    current: &ContextAudit,
) -> Result<Option<ContextAudit>> {
    let tx = conn.unchecked_transaction()?;
    let previous_json: Option<String> = tx
        .query_row(
            "SELECT audit_json FROM context_audit_snapshots WHERE project_id = ?1",
            [project_id],
            |row| row.get(0),
        )
        .optional()?;
    if previous_json.is_none() {
        let current_json = serde_json::to_string(current).context("serialize context audit")?;
        tx.execute(
            "INSERT OR IGNORE INTO context_audit_snapshots
             (project_id, audit_json, captured_at)
             VALUES (?1, ?2, datetime('now'))",
            params![project_id, current_json],
        )?;
    }
    tx.commit()?;

    previous_json
        .map(|json| serde_json::from_str(&json).context("decode previous context audit snapshot"))
        .transpose()
}

/// Explicitly accept the current observation as the new baseline.
pub fn replace_snapshot(conn: &Connection, project_id: &str, current: &ContextAudit) -> Result<()> {
    let current_json = serde_json::to_string(current).context("serialize context audit")?;
    conn.execute(
        "INSERT INTO context_audit_snapshots (project_id, audit_json, captured_at)
         VALUES (?1, ?2, datetime('now'))
         ON CONFLICT(project_id) DO UPDATE SET
           audit_json = excluded.audit_json,
           captured_at = excluded.captured_at",
        params![project_id, current_json],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context_audit::{audit_repo, drift};

    #[test]
    fn snapshot_is_project_scoped_and_returns_the_previous_observation() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let root =
            std::env::temp_dir().join(format!("kronn-context-snapshot-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("AGENTS.md"), "# Rules\nKeep this concise.\n").unwrap();
        let path = root.to_string_lossy().to_string();
        (|| -> anyhow::Result<()> {
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p1', 'P1', ?1, datetime('now'), datetime('now'))",
                [&path],
            )?;
            let first = audit_repo(&root);
            assert!(load_or_create_snapshot(&conn, "p1", &first)?.is_none());

            // Same structure and path, more always-loaded content: this is the
            // regression that KT-260 specifically needs to surface.
            std::fs::write(
                root.join("AGENTS.md"),
                "# Rules\nKeep this concise.\nThis extra rule is loaded for every task.\n",
            )?;
            let second = audit_repo(&root);
            let previous = load_or_create_snapshot(&conn, "p1", &second)?.expect("baseline");
            let change = drift(&previous, &second);
            assert_eq!(change.grown[0].0, "AGENTS.md");
            assert!(change.grown[0].1 > 0);
            // Merely reading again must not acknowledge the drift (React dev
            // mode and retries may issue duplicate GETs).
            let still_first = load_or_create_snapshot(&conn, "p1", &second)?.unwrap();
            assert_eq!(
                still_first.files.first().map(|file| file.bytes),
                first.files.first().map(|file| file.bytes)
            );
            replace_snapshot(&conn, "p1", &second)?;
            let accepted = load_or_create_snapshot(&conn, "p1", &second)?.unwrap();
            assert!(drift(&accepted, &second).grown.is_empty());
            Ok(())
        })()
        .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
