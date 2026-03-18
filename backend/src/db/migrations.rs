use anyhow::Result;
use rusqlite::Connection;

/// Run all migrations in order. Each migration is idempotent.
pub fn run(conn: &Connection) -> Result<()> {
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
    ];

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

    Ok(())
}
