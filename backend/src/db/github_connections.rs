//! Per-project GitHub connection rows (design note §4.5). The token column
//! only ever holds ciphertext; reading it back needs the instance key.

use anyhow::Result;
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{GithubConnectionMode, GithubScope};

#[derive(Clone)]
pub struct GithubConnectionRow {
    pub project_id: String,
    pub mode: GithubConnectionMode,
    pub token_encrypted: Option<String>,
    pub scope: Option<GithubScope>,
    pub connected_on_upgrade: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

// Hand-written so the ciphertext stays out of Debug output as well.
impl std::fmt::Debug for GithubConnectionRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubConnectionRow")
            .field("project_id", &self.project_id)
            .field("mode", &self.mode)
            .field("has_token", &self.token_encrypted.is_some())
            .field("connected_on_upgrade", &self.connected_on_upgrade)
            .finish()
    }
}

fn parse_sqlite_time(raw: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
        .or_else(|| {
            DateTime::parse_from_rfc3339(raw)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        })
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GithubConnectionRow> {
    let mode: String = row.get(1)?;
    let scope_json: Option<String> = row.get(3)?;
    let updated_at: Option<String> = row.get(5)?;
    Ok(GithubConnectionRow {
        project_id: row.get(0)?,
        mode: GithubConnectionMode::parse(&mode),
        token_encrypted: row.get::<_, Option<String>>(2)?.filter(|t| !t.is_empty()),
        scope: scope_json.and_then(|json| serde_json::from_str(&json).ok()),
        connected_on_upgrade: row.get::<_, i64>(4)? != 0,
        updated_at: updated_at.as_deref().and_then(parse_sqlite_time),
    })
}

const COLUMNS: &str =
    "project_id, mode, token_encrypted, scope_json, connected_on_upgrade, updated_at";

/// Every row exactly as stored (raw columns), for [`restore_rows`].
pub struct RawRows(Vec<[Option<String>; 6]>);

/// Read every connection row verbatim, ciphertext included.
pub fn snapshot_rows(conn: &Connection) -> Result<RawRows> {
    let mut stmt = conn.prepare(
        "SELECT project_id, mode, token_encrypted, scope_json, CAST(connected_on_upgrade AS TEXT), updated_at \
         FROM project_github_connections",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok([
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ])
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(RawRows(rows))
}

/// Put back the snapshot rows whose project exists again (an import replaced
/// the `projects` table, whose cascade dropped them). Returns how many rows
/// could not come back because their project is gone.
pub fn restore_rows(conn: &Connection, rows: &RawRows) -> Result<usize> {
    let mut dropped = 0;
    for row in &rows.0 {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            [&row[0]],
            |r| r.get(0),
        )?;
        if !exists {
            dropped += 1;
            continue;
        }
        conn.execute(
            "INSERT OR REPLACE INTO project_github_connections \
             (project_id, mode, token_encrypted, scope_json, connected_on_upgrade, updated_at) \
             VALUES (?1, ?2, ?3, ?4, CAST(?5 AS INTEGER), ?6)",
            params![row[0], row[1], row[2], row[3], row[4], row[5]],
        )?;
    }
    Ok(dropped)
}

pub fn get(conn: &Connection, project_id: &str) -> Result<Option<GithubConnectionRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM project_github_connections WHERE project_id = ?1"),
            params![project_id],
            map_row,
        )
        .optional()?)
}

pub fn list(conn: &Connection) -> Result<Vec<GithubConnectionRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM project_github_connections ORDER BY project_id"
    ))?;
    let rows = stmt
        .query_map([], map_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Sets the mode. A token is kept only with `StoredToken`; any other mode
/// erases it. The upgrade notice flag is cleared on any explicit choice.
pub fn set_mode(
    conn: &Connection,
    project_id: &str,
    mode: GithubConnectionMode,
    token_encrypted: Option<&str>,
) -> Result<()> {
    let token = match mode {
        GithubConnectionMode::StoredToken => token_encrypted,
        _ => None,
    };
    conn.execute(
        "INSERT INTO project_github_connections
             (project_id, mode, token_encrypted, scope_json, connected_on_upgrade, updated_at)
         VALUES (?1, ?2, ?3, NULL, 0, datetime('now'))
         ON CONFLICT(project_id) DO UPDATE SET
             mode = excluded.mode,
             token_encrypted = excluded.token_encrypted,
             scope_json = CASE
                 WHEN project_github_connections.mode = excluded.mode
                      AND excluded.mode != 'stored_token'
                 THEN project_github_connections.scope_json
                 ELSE NULL END,
             connected_on_upgrade = 0,
             updated_at = excluded.updated_at",
        params![project_id, mode.as_str(), token],
    )?;
    Ok(())
}

/// Caches the scope last read from GitHub; creates a not-connected row if needed.
pub fn set_scope(conn: &Connection, project_id: &str, scope: &GithubScope) -> Result<()> {
    let json = serde_json::to_string(scope)?;
    conn.execute(
        "INSERT INTO project_github_connections (project_id, mode, scope_json)
         VALUES (?1, 'not_connected', ?2)
         ON CONFLICT(project_id) DO UPDATE SET scope_json = excluded.scope_json",
        params![project_id, json],
    )?;
    Ok(())
}

/// Every stored-token ciphertext, so the key reconcile knows encrypted data exists.
pub fn encrypted_tokens(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT token_encrypted FROM project_github_connections
         WHERE token_encrypted IS NOT NULL AND token_encrypted != ''",
    )?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Upgrade default: projects that existed before this setting and whose remote
/// is on GitHub keep receiving the token, with a one-time notice.
pub fn seed_on_upgrade(conn: &Connection) -> Result<usize> {
    let projects: Vec<(String, String, Option<String>)> = {
        let mut stmt = conn.prepare("SELECT id, path, repo_url FROM projects")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut seeded = 0;
    for (id, path, repo_url) in projects {
        if !crate::core::github_connection::project_is_on_github(&path, repo_url.as_deref()) {
            continue;
        }
        seeded += conn.execute(
            "INSERT OR IGNORE INTO project_github_connections
                 (project_id, mode, connected_on_upgrade) VALUES (?1, 'gh_login', 1)",
            params![id],
        )?;
    }
    Ok(seeded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::models::GithubTokenKind;

    fn insert_project(conn: &Connection, id: &str, path: &str, repo_url: Option<&str>) {
        conn.execute(
            "INSERT INTO projects (id, name, path, repo_url, created_at, updated_at)
             VALUES (?1, ?1, ?2, ?3, datetime('now'), datetime('now'))",
            params![id, path, repo_url],
        )
        .unwrap();
    }

    fn scope() -> GithubScope {
        GithubScope {
            verified: true,
            token_kind: GithubTokenKind::Classic,
            login: Some("octo".into()),
            scopes: vec!["repo".into()],
            repositories: vec![],
            repositories_truncated: false,
            broad: true,
            reason: None,
            checked_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn a_project_without_a_row_reads_as_absent() {
        let db = Database::open_in_memory().unwrap();
        let row = db
            .with_conn(|conn| {
                insert_project(conn, "p1", "/nowhere/p1", None);
                get(conn, "p1")
            })
            .await
            .unwrap();
        assert!(row.is_none());
    }

    #[tokio::test]
    async fn the_mode_machine_keeps_a_token_only_while_stored_token() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            insert_project(conn, "p1", "/nowhere/p1", None);
            set_mode(
                conn,
                "p1",
                GithubConnectionMode::StoredToken,
                Some("cipher"),
            )?;
            let row = get(conn, "p1")?.unwrap();
            assert_eq!(row.mode, GithubConnectionMode::StoredToken);
            assert_eq!(row.token_encrypted.as_deref(), Some("cipher"));
            assert_eq!(encrypted_tokens(conn)?, vec!["cipher".to_string()]);

            set_mode(conn, "p1", GithubConnectionMode::GhLogin, Some("ignored"))?;
            let row = get(conn, "p1")?.unwrap();
            assert_eq!(row.mode, GithubConnectionMode::GhLogin);
            assert!(
                row.token_encrypted.is_none(),
                "leaving stored_token erases it"
            );
            assert!(encrypted_tokens(conn)?.is_empty());

            set_scope(conn, "p1", &scope())?;
            set_mode(conn, "p1", GithubConnectionMode::GhLogin, None)?;
            assert!(
                get(conn, "p1")?.unwrap().scope.is_some(),
                "same mode keeps the scope"
            );
            set_mode(conn, "p1", GithubConnectionMode::NotConnected, None)?;
            let row = get(conn, "p1")?.unwrap();
            assert_eq!(row.mode, GithubConnectionMode::NotConnected);
            assert!(row.scope.is_none(), "a mode change drops the stale scope");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn set_scope_creates_a_not_connected_row() {
        let db = Database::open_in_memory().unwrap();
        let row = db
            .with_conn(|conn| {
                insert_project(conn, "p1", "/nowhere/p1", None);
                set_scope(conn, "p1", &scope())?;
                get(conn, "p1")
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.mode, GithubConnectionMode::NotConnected);
        assert_eq!(row.scope.as_ref().unwrap().scopes, vec!["repo".to_string()]);
        assert!(!format!("{row:?}").contains("cipher"));
    }

    #[tokio::test]
    async fn deleting_the_project_deletes_its_connection() {
        let db = Database::open_in_memory().unwrap();
        let rows = db
            .with_conn(|conn| {
                insert_project(conn, "p1", "/nowhere/p1", None);
                set_mode(
                    conn,
                    "p1",
                    GithubConnectionMode::StoredToken,
                    Some("cipher"),
                )?;
                conn.execute("DELETE FROM projects WHERE id = 'p1'", [])?;
                list(conn)
            })
            .await
            .unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn upgrade_connects_github_projects_only() {
        let db = Database::open_in_memory().unwrap();
        let remote_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(remote_dir.path().join(".git")).unwrap();
        std::fs::write(
            remote_dir.path().join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:octo/réseau.git\n",
        )
        .unwrap();
        let remote_path = remote_dir.path().to_string_lossy().into_owned();
        let (seeded, rows) = db
            .with_conn(move |conn| {
                insert_project(
                    conn,
                    "https",
                    "/nowhere/a",
                    Some("https://github.com/octo/a.git"),
                );
                insert_project(
                    conn,
                    "gitlab",
                    "/nowhere/b",
                    Some("https://gitlab.com/octo/b"),
                );
                insert_project(conn, "local", "/nowhere/c", None);
                insert_project(conn, "remote-only", &remote_path, None);
                insert_project(
                    conn,
                    "ghe",
                    "/nowhere/d",
                    Some("https://github.example.com/o/d"),
                );
                let seeded = seed_on_upgrade(conn)?;
                // Idempotent: a second pass inserts nothing.
                assert_eq!(seed_on_upgrade(conn)?, 0);
                Ok((seeded, list(conn)?))
            })
            .await
            .unwrap();
        assert_eq!(seeded, 2);
        let ids: Vec<_> = rows.iter().map(|row| row.project_id.as_str()).collect();
        assert_eq!(ids, vec!["https", "remote-only"]);
        assert!(rows
            .iter()
            .all(|row| row.mode == GithubConnectionMode::GhLogin && row.connected_on_upgrade));
    }
}
