//! Local identity table `(project, kind, slug) -> id`, generalising
//! `artifact_import_origins` (migration 188) into a project-scoped mapping
//! with at most one target per key (ADR-005 slice 3,
//! `docs/design/adr-005-project-resource-library.md#identity`).
//!
//! Re-importing the same slug therefore always resolves — and, through
//! `upsert`, updates — the same local row instead of accumulating an
//! unrelated copy. This module also resolves the symbolic cross-resource
//! references the same slice introduces (`prompt:<slug>`, `workflow:<slug>`,
//! `qe:<slug>`, `qa:<slug>`, `skill:<slug>`, `plugin:<server>`).
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

/// Scope key for the identity table: the destination project's normalised
/// `repo_url` when known, else its path, else `""` for no project — never
/// the local project UUID, which is instance-specific and does not survive
/// a clone onto another machine.
pub fn project_key(conn: &Connection, project_id: Option<&str>) -> Result<String> {
    let Some(project_id) = project_id else {
        return Ok(String::new());
    };
    Ok(match crate::db::projects::get_project(conn, project_id)? {
        Some(project) => project
            .repo_url
            .as_deref()
            .map(crate::api::discover::normalize_repo_url)
            .unwrap_or(project.path),
        None => String::new(),
    })
}

/// Records that `slug` (of `kind`, within `project_key`) now maps to
/// `target_id`. Reimporting the same slug updates this same row rather than
/// creating a duplicate mapping.
pub fn upsert(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    slug: &str,
    target_id: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO resource_identities (project_key, kind, slug, target_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))
         ON CONFLICT(project_key, kind, slug) DO UPDATE SET
            target_id = excluded.target_id, updated_at = excluded.updated_at",
        rusqlite::params![project_key, kind, slug, target_id],
    )?;
    Ok(())
}

/// The local id `(project_key, kind, slug)` currently maps to, if any.
pub fn lookup(
    conn: &Connection,
    project_key: &str,
    kind: &str,
    slug: &str,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT target_id FROM resource_identities WHERE project_key = ?1 AND kind = ?2 AND slug = ?3",
        rusqlite::params![project_key, kind, slug],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Resolves a symbolic reference (`prompt:<slug>`, `workflow:<slug>`,
/// `qe:<slug>`, `qa:<slug>`, `skill:<slug>`, `plugin:<server>`) to the local
/// identifier it names. Project-scoped kinds try the same project first,
/// then fall back to the global scope — matching the ADR's "resolved at
/// load time, first within the same project".
pub fn resolve_symbolic_reference(
    conn: &Connection,
    reference: &str,
    project_key: &str,
    project_id: Option<&str>,
) -> Result<Option<String>> {
    let Some((kind, slug)) = reference.split_once(':') else {
        return Ok(None);
    };
    if slug.is_empty() {
        return Ok(None);
    }
    match kind {
        "skill" => {
            return Ok(crate::core::skills::get_skill(&format!("custom-{slug}"))
                .or_else(|| crate::core::skills::get_skill(slug))
                .map(|skill| skill.id))
        }
        "plugin" => return crate::db::mcps::find_config_for_server(conn, slug, project_id),
        _ => {}
    }
    let table_kind = match kind {
        "prompt" => "quick_prompt",
        "workflow" => "workflow",
        "qe" => "quick_exec",
        "qa" => "quick_api",
        _ => return Ok(None),
    };
    if let Some(id) = lookup(conn, project_key, table_kind, slug)? {
        return Ok(Some(id));
    }
    if project_key.is_empty() {
        return Ok(None);
    }
    lookup(conn, "", table_kind, slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    #[test]
    fn upsert_reimporting_the_same_slug_updates_the_same_row_without_a_duplicate() {
        let conn = conn();
        upsert(&conn, "proj-a", "workflow", "triage", "wf-1").unwrap();
        upsert(&conn, "proj-a", "workflow", "triage", "wf-2").unwrap();
        assert_eq!(
            lookup(&conn, "proj-a", "workflow", "triage").unwrap(),
            Some("wf-2".into())
        );
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM resource_identities", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "reimporting the same slug must not duplicate the row");
    }

    #[test]
    fn upsert_is_scoped_per_project_and_kind() {
        let conn = conn();
        upsert(&conn, "proj-a", "workflow", "triage", "wf-a").unwrap();
        upsert(&conn, "proj-b", "workflow", "triage", "wf-b").unwrap();
        upsert(&conn, "proj-a", "quick_prompt", "triage", "qp-a").unwrap();
        assert_eq!(
            lookup(&conn, "proj-a", "workflow", "triage").unwrap(),
            Some("wf-a".into())
        );
        assert_eq!(
            lookup(&conn, "proj-b", "workflow", "triage").unwrap(),
            Some("wf-b".into())
        );
        assert_eq!(
            lookup(&conn, "proj-a", "quick_prompt", "triage").unwrap(),
            Some("qp-a".into())
        );
        assert_eq!(lookup(&conn, "proj-b", "quick_prompt", "triage").unwrap(), None);
    }

    #[test]
    fn resolve_symbolic_reference_covers_every_declared_kind() {
        let conn = conn();
        upsert(&conn, "proj-a", "quick_prompt", "review-pr", "qp-1").unwrap();
        upsert(&conn, "proj-a", "workflow", "triage", "wf-1").unwrap();
        upsert(&conn, "proj-a", "quick_exec", "lint", "qe-1").unwrap();
        upsert(&conn, "proj-a", "quick_api", "fetch", "qa-1").unwrap();
        assert_eq!(
            resolve_symbolic_reference(&conn, "prompt:review-pr", "proj-a", None).unwrap(),
            Some("qp-1".into())
        );
        assert_eq!(
            resolve_symbolic_reference(&conn, "workflow:triage", "proj-a", None).unwrap(),
            Some("wf-1".into())
        );
        assert_eq!(
            resolve_symbolic_reference(&conn, "qe:lint", "proj-a", None).unwrap(),
            Some("qe-1".into())
        );
        assert_eq!(
            resolve_symbolic_reference(&conn, "qa:fetch", "proj-a", None).unwrap(),
            Some("qa-1".into())
        );
        // Builtin skills are embedded at compile time — no filesystem
        // fixture needed, and no risk of writing under the real config dir.
        assert_eq!(
            resolve_symbolic_reference(&conn, "skill:rust", "proj-a", None).unwrap(),
            Some("rust".into())
        );
        assert_eq!(
            resolve_symbolic_reference(&conn, "skill:does-not-exist", "proj-a", None).unwrap(),
            None
        );
    }

    #[test]
    fn resolve_symbolic_reference_falls_back_to_the_global_scope() {
        let conn = conn();
        upsert(&conn, "", "quick_prompt", "review-pr", "qp-global").unwrap();
        assert_eq!(
            resolve_symbolic_reference(&conn, "prompt:review-pr", "proj-a", None).unwrap(),
            Some("qp-global".into())
        );
        // A same-project mapping still wins over the global fallback.
        upsert(&conn, "proj-a", "quick_prompt", "review-pr", "qp-local").unwrap();
        assert_eq!(
            resolve_symbolic_reference(&conn, "prompt:review-pr", "proj-a", None).unwrap(),
            Some("qp-local".into())
        );
    }

    #[test]
    fn resolve_symbolic_reference_rejects_malformed_or_unknown_input() {
        let conn = conn();
        assert_eq!(
            resolve_symbolic_reference(&conn, "not-a-reference", "", None).unwrap(),
            None
        );
        assert_eq!(
            resolve_symbolic_reference(&conn, "prompt:", "", None).unwrap(),
            None
        );
        assert_eq!(
            resolve_symbolic_reference(&conn, "unknown-kind:slug", "", None).unwrap(),
            None
        );
    }
}
