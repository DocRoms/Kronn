use std::path::Path;
use anyhow::Result;
use rusqlite::Connection;

/// Run all migrations in order. Each migration is idempotent.
/// If `db_path` points to an existing file and there are pending migrations,
/// a backup is created at `<db_path>.backup` before applying them.
pub fn run(conn: &Connection) -> Result<()> {
    run_with_backup(conn, None)
}

/// Run all migrations, optionally backing up the database file first.
pub fn run_with_backup(conn: &Connection, db_path: Option<&Path>) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );"
    )?;

    let migrations: &[(&str, &str)] = &[
        ("001_initial", include_str!("sql/001_initial.sql")),
        ("002_mcp_redesign", include_str!("sql/002_mcp_redesign.sql")),
        ("003_workflows", include_str!("sql/003_workflows.sql")),
        ("004_token_tracking", include_str!("sql/004_token_tracking.sql")),
        ("005_discussion_archive", include_str!("sql/005_discussion_archive.sql")),
        ("006_discussion_skills", include_str!("sql/006_discussion_skills.sql")),
        ("007_project_skills", include_str!("sql/007_project_skills.sql")),
        ("008_discussions_index", include_str!("sql/008_discussions_index.sql")),
        ("009_profiles", include_str!("sql/009_profiles.sql")),
        ("010_directives", include_str!("sql/010_directives.sql")),
        ("011_multi_profiles", include_str!("sql/011_multi_profiles.sql")),
        ("012_mcp_general", include_str!("sql/012_mcp_general.sql")),
        ("013_discussion_worktrees", include_str!("sql/013_discussion_worktrees.sql")),
        ("014_summary_cache", include_str!("sql/014_summary_cache.sql")),
        ("015_model_tier", include_str!("sql/015_model_tier.sql")),
        ("016_message_model_tier", include_str!("sql/016_message_model_tier.sql")),
        ("017_message_count", include_str!("sql/017_message_count.sql")),
        ("018_briefing_notes", include_str!("sql/018_briefing_notes.sql")),
        ("019_pin_first_message", include_str!("sql/019_pin_first_message.sql")),
        ("020_fix_worktree_paths", include_str!("sql/020_fix_worktree_paths.sql")),
        ("021_message_identity", include_str!("sql/021_message_identity.sql")),
        ("022_contacts", include_str!("sql/022_contacts.sql")),
        ("023_shared_discussions", include_str!("sql/023_shared_discussions.sql")),
        ("024_message_cost", include_str!("sql/024_message_cost.sql")),
        ("025_context_files", include_str!("sql/025_context_files.sql")),
        // 026: idempotent column addition (handled below, not via SQL file)
        ("027_quick_prompts", include_str!("sql/026_quick_prompts.sql")),
        ("028_quick_prompt_descriptions", include_str!("sql/027_quick_prompt_descriptions.sql")),
        ("029_batch_workflow_runs", include_str!("sql/028_batch_workflow_runs.sql")),
        ("030_workflow_run_parent", include_str!("sql/030_workflow_run_parent.sql")),
        ("031_partial_response", include_str!("sql/031_partial_response.sql")),
        ("032_partial_response_started_at", include_str!("sql/032_partial_response_started_at.sql")),
        ("033_discussion_pinned", include_str!("sql/033_discussion_pinned.sql")),
        ("034_test_mode_fields", include_str!("sql/034_test_mode_fields.sql")),
        ("035_mcp_server_api_spec", include_str!("sql/035_mcp_server_api_spec.sql")),
    ];

    // Check if there are pending migrations before backing up
    if let Some(path) = db_path {
        if path.exists() {
            let has_pending = migrations.iter().any(|(name, _)| {
                let applied: bool = conn.query_row(
                    "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?1",
                    [name],
                    |row| row.get(0),
                ).unwrap_or(false);
                !applied
            });
            if has_pending {
                let backup_path = path.with_extension("db.backup");
                if let Err(e) = std::fs::copy(path, &backup_path) {
                    tracing::warn!("Failed to backup database before migration: {}", e);
                } else {
                    tracing::info!("Database backed up to {}", backup_path.display());
                }
            }
        }
    }

    for (name, sql) in migrations {
        let already_applied: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?1",
            [name],
            |row| row.get(0),
        )?;

        if !already_applied {
            tracing::info!("Running migration: {}", name);
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO _migrations (name) VALUES (?1)",
                [name],
            )?;
        }
    }

    // Idempotent schema fixups (safe to run multiple times, handles upgrades from
    // older 025 that didn't include disk_path)
    let _ = conn.execute_batch("ALTER TABLE context_files ADD COLUMN disk_path TEXT;");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_with_backup_creates_backup_file() {
        // Create a temp directory and a SQLite file with some data
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // Create and populate the database, then close the connection
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, val TEXT);").unwrap();
            conn.execute("INSERT INTO t(val) VALUES (?1)", ["hello"]).unwrap();
        }

        // Open a new connection and run migrations (which will create a backup)
        let conn = Connection::open(&db_path).unwrap();
        run_with_backup(&conn, Some(&db_path)).expect("run_with_backup should succeed");

        // Verify the backup file was created
        let backup_path = db_path.with_extension("db.backup");
        assert!(backup_path.exists(), "Backup file should exist at {:?}", backup_path);

        // Verify the original file still exists
        assert!(db_path.exists(), "Original database file should still exist");

        // Verify the backup contains valid data by opening it as a SQLite DB
        let backup_conn = Connection::open(&backup_path).unwrap();
        let val: String = backup_conn.query_row(
            "SELECT val FROM t WHERE id = 1", [], |row| row.get(0),
        ).unwrap();
        assert_eq!(val, "hello", "Backup database should contain original data");

        // Verify the original database still has our data (migrations don't destroy it)
        let val: String = conn.query_row("SELECT val FROM t WHERE id = 1", [], |row| row.get(0)).unwrap();
        assert_eq!(val, "hello");
    }

    #[test]
    fn run_with_backup_no_backup_when_no_path() {
        // When db_path is None, no backup should be attempted (in-memory DB)
        let conn = Connection::open_in_memory().unwrap();
        run_with_backup(&conn, None).expect("run_with_backup with None path should succeed");
        // No assertion on files — just ensure it doesn't panic
    }
}
