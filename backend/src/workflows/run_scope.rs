//! Where a run may publish: pages and rooms named by a step, once rendered,
//! must belong to the run's project. A template can carry a caller's variable
//! or a prior step's output, so the stored value proves nothing.

/// Whether a step of a run for `run_project` may act on a resource of
/// `target`. A project-less resource is allowed to a project-less run, or
/// when the workflow names it literally (its author chose it).
pub fn target_allowed(target: Option<&str>, run_project: Option<&str>, literal: bool) -> bool {
    match (target, run_project) {
        (Some(target), Some(run)) => target == run,
        (None, None) => true,
        (None, Some(_)) => literal,
        (Some(_), None) => false,
    }
}

/// Whether a stored step field is used as written, with no template.
pub fn is_literal(template: &str) -> bool {
    !template.contains("{{")
}

/// The projects of every page an id or slug resolves to, as the publisher
/// resolves it (`id OR slug`).
pub fn page_projects(
    conn: &rusqlite::Connection,
    page: &str,
) -> anyhow::Result<Vec<Option<String>>> {
    let mut statement =
        conn.prepare("SELECT project_id FROM live_pages WHERE id = ?1 OR slug = ?1")?;
    let projects = statement
        .query_map([page], |row| row.get::<_, Option<String>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(projects)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_run_s_project_or_a_literal_shared_target_is_allowed() {
        assert!(target_allowed(Some("p1"), Some("p1"), false));
        assert!(!target_allowed(Some("p2"), Some("p1"), true));
        assert!(!target_allowed(Some("p1"), None, true));
        assert!(target_allowed(None, None, false));
        assert!(target_allowed(None, Some("p1"), true));
        assert!(!target_allowed(None, Some("p1"), false));
        assert!(is_literal("page-a") && !is_literal("{{page}}"));
    }
}
