//! "Use in Kronn" for a native skill outside `kronn/`: a project-scoped
//! pointer to a `SKILL.md` by repository-relative path, read fresh from the
//! source on every use rather than copied — no `kronn.lock` entry, no
//! catalog skill row (see `docs/design/adr-005-project-resource-library.md`,
//! KT-897).

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillReference {
    pub project_id: String,
    pub slug: String,
    pub relative_path: String,
    pub name: String,
    pub created_at: String,
}

fn map_reference(row: &rusqlite::Row<'_>) -> rusqlite::Result<SkillReference> {
    Ok(SkillReference {
        project_id: row.get(0)?,
        slug: row.get(1)?,
        relative_path: row.get(2)?,
        name: row.get(3)?,
        created_at: row.get(4)?,
    })
}

/// Records that `slug` is used in Kronn by reference to `relative_path`.
/// Re-using the same slug updates this same row rather than accumulating a
/// stale pointer to a since-moved file.
pub fn upsert(
    conn: &Connection,
    project_id: &str,
    slug: &str,
    relative_path: &str,
    name: &str,
    created_at: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO project_skill_references (project_id, slug, relative_path, name, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(project_id, slug) DO UPDATE SET
            relative_path = excluded.relative_path,
            name = excluded.name",
        rusqlite::params![project_id, slug, relative_path, name, created_at],
    )?;
    Ok(())
}

pub fn find(conn: &Connection, project_id: &str, slug: &str) -> Result<Option<SkillReference>> {
    conn.query_row(
        "SELECT project_id, slug, relative_path, name, created_at
           FROM project_skill_references
          WHERE project_id = ?1 AND slug = ?2",
        rusqlite::params![project_id, slug],
        map_reference,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_for_project(conn: &Connection, project_id: &str) -> Result<Vec<SkillReference>> {
    let mut statement = conn.prepare(
        "SELECT project_id, slug, relative_path, name, created_at
           FROM project_skill_references
          WHERE project_id = ?1",
    )?;
    let rows = statement.query_map([project_id], map_reference)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Every project's references, for a page that reads all projects at once.
pub fn list_all(conn: &Connection) -> Result<Vec<SkillReference>> {
    let mut statement = conn.prepare(
        "SELECT project_id, slug, relative_path, name, created_at
           FROM project_skill_references
          ORDER BY project_id, slug",
    )?;
    let rows = statement.query_map([], map_reference)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_on_the_same_slug_replaces_the_path_rather_than_duplicating() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        upsert(
            &conn,
            "project-1",
            "review",
            ".claude/skills/review/SKILL.md",
            "Review",
            "2026-09-28T00:00:00Z",
        )
        .unwrap();
        upsert(
            &conn,
            "project-1",
            "review",
            ".agents/skills/review/SKILL.md",
            "Review",
            "2026-09-28T00:01:00Z",
        )
        .unwrap();
        let found = find(&conn, "project-1", "review").unwrap().unwrap();
        assert_eq!(found.relative_path, ".agents/skills/review/SKILL.md");
        assert_eq!(list_for_project(&conn, "project-1").unwrap().len(), 1);
    }
}
