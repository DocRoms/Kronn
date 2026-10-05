//! Where a run may publish: a page or room a step names through a template,
//! once rendered, must belong to the run's project. A template can carry a
//! caller's variable or a prior step's output, so the stored value proves
//! nothing; a literal one is the workflow author's choice.

/// Whether a step of a run for `run_project` may act on a resource of
/// `target`. A literal value was chosen by the workflow's author (and a
/// bridge token cannot save one outside its project): always allowed. A
/// rendered value must sit in the run's project, or be project-less for a
/// project-less run.
pub fn target_allowed(target: Option<&str>, run_project: Option<&str>, literal: bool) -> bool {
    literal || target == run_project
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
    fn a_rendered_target_must_sit_in_the_run_s_project() {
        assert!(target_allowed(Some("p1"), Some("p1"), false));
        assert!(!target_allowed(Some("p2"), Some("p1"), false));
        assert!(!target_allowed(None, Some("p1"), false));
        assert!(!target_allowed(Some("p1"), None, false));
        assert!(target_allowed(None, None, false));
        // A literal is the author's choice, whatever its project.
        assert!(target_allowed(Some("p2"), Some("p1"), true));
        assert!(target_allowed(Some("p1"), None, true));
        assert!(is_literal("page-a") && !is_literal("{{page}}"));
    }
}
