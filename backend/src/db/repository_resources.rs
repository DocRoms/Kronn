use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceAlignment {
    pub project_key: String,
    pub kind: String,
    pub slug: String,
    pub target_id: String,
    pub repository_hash: String,
    pub database_hash: String,
    pub aligned_at: String,
    pub imported: bool,
}

fn map_alignment(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResourceAlignment> {
    Ok(ResourceAlignment {
        project_key: row.get(0)?,
        kind: row.get(1)?,
        slug: row.get(2)?,
        target_id: row.get(3)?,
        repository_hash: row.get(4)?,
        database_hash: row.get(5)?,
        aligned_at: row.get(6)?,
        imported: row.get(7)?,
    })
}

// Keep the call explicit: every persisted baseline column is security-relevant
// and grouping them into an optional bag would make omissions harder to spot.
#[allow(clippy::too_many_arguments)]
pub fn upsert_alignment(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    slug: &str,
    target_id: &str,
    repository_hash: &str,
    database_hash: &str,
    aligned_at: &str,
    imported: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO repository_resource_alignments
            (project_key, kind, slug, target_id, repository_hash, database_hash, aligned_at, imported)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(project_key, kind, slug) DO UPDATE SET
            target_id = excluded.target_id,
            repository_hash = excluded.repository_hash,
            database_hash = excluded.database_hash,
            aligned_at = excluded.aligned_at,
            imported = repository_resource_alignments.imported OR excluded.imported",
        rusqlite::params![
            project_key,
            kind,
            slug,
            target_id,
            repository_hash,
            database_hash,
            aligned_at,
            imported,
        ],
    )?;
    Ok(())
}

pub fn find_alignment(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    slug: &str,
) -> Result<Option<ResourceAlignment>> {
    conn.query_row(
        "SELECT project_key, kind, slug, target_id, repository_hash, database_hash,
                aligned_at, imported
           FROM repository_resource_alignments
          WHERE project_key = ?1 AND kind = ?2 AND slug = ?3",
        rusqlite::params![project_key, kind, slug],
        map_alignment,
    )
    .optional()
    .map_err(Into::into)
}

/// Forgets the baseline of one resource: it then reads as never aligned, so a
/// difference between its two sides is a conflict for a human to settle rather
/// than a guess at which one changed.
pub fn delete_alignment(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    slug: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM repository_resource_alignments
          WHERE project_key = ?1 AND kind = ?2 AND slug = ?3",
        rusqlite::params![project_key, kind, slug],
    )?;
    Ok(())
}

pub fn find_alignment_by_target(
    conn: &Connection,
    kind: &str,
    target_id: &str,
) -> Result<Option<ResourceAlignment>> {
    conn.query_row(
        "SELECT project_key, kind, slug, target_id, repository_hash, database_hash,
                aligned_at, imported
           FROM repository_resource_alignments
          WHERE kind = ?1 AND target_id = ?2
          ORDER BY aligned_at DESC LIMIT 1",
        rusqlite::params![kind, target_id],
        map_alignment,
    )
    .optional()
    .map_err(Into::into)
}

pub fn approve(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    slug: &str,
    content_hash: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO repository_resource_approvals
            (project_key, kind, slug, content_hash, approved_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        rusqlite::params![project_key, kind, slug, content_hash],
    )?;
    Ok(())
}

pub fn is_approved(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    slug: &str,
    content_hash: &str,
) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM repository_resource_approvals
              WHERE project_key = ?1 AND kind = ?2 AND slug = ?3 AND content_hash = ?4",
            rusqlite::params![project_key, kind, slug, content_hash],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_is_bound_to_one_content_hash() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        upsert_alignment(
            &conn,
            "repo",
            "quick_exec",
            "lint",
            "qe-1",
            "file-a",
            "db-a",
            "2026-09-28T10:00:00Z",
            true,
        )
        .unwrap();
        approve(&conn, "repo", "quick_exec", "lint", "db-a").unwrap();
        assert!(is_approved(&conn, "repo", "quick_exec", "lint", "db-a").unwrap());
        assert!(!is_approved(&conn, "repo", "quick_exec", "lint", "db-b").unwrap());
    }

    #[test]
    fn imported_provenance_survives_later_publications_for_executable_resources() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();

        for kind in ["workflow", "quick_prompt", "quick_api", "quick_exec"] {
            upsert_alignment(
                &conn,
                "repo",
                kind,
                "deploy",
                &format!("{kind}-1"),
                "imported-repository-hash",
                "imported-database-hash",
                "2026-09-28T10:00:00Z",
                true,
            )
            .unwrap();
            upsert_alignment(
                &conn,
                "repo",
                kind,
                "deploy",
                &format!("{kind}-1"),
                "published-repository-hash",
                "published-database-hash",
                "2026-09-28T11:00:00Z",
                false,
            )
            .unwrap();

            let alignment = find_alignment(&conn, "repo", kind, "deploy")
                .unwrap()
                .unwrap();
            assert!(alignment.imported, "{kind} lost its imported provenance");
            assert_eq!(alignment.repository_hash, "published-repository-hash");
            assert_eq!(alignment.database_hash, "published-database-hash");
        }
    }
}
