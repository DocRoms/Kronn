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
