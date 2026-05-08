// Project API surface — split by domain to keep each file digestible
// (TD-20260417-projects-monolith). Sub-modules are re-exported via
// `pub use *::*` so every existing `api::projects::Foo` call site keeps
// resolving without edits.

use crate::core::scanner;
use crate::models::*;
use crate::AppState;

pub mod bootstrap;
pub mod clone;
pub mod crud;
pub mod git;
pub mod migrate;
pub mod template;

pub use bootstrap::*;
pub use clone::*;
pub use crud::*;
pub use git::*;
pub use migrate::*;
pub use template::*;

/// Read briefing notes: try `<docs>/briefing.md` from the filesystem
/// first (path-agnostic — picks docs/ post-pivot or ai/ legacy), fall
/// back to the DB field.
pub(crate) fn resolve_briefing_notes(
    project_path: &std::path::Path,
    db_notes: &Option<String>,
) -> Option<String> {
    let briefing_file = scanner::detect_docs_dir(project_path).join("briefing.md");
    if let Ok(content) = std::fs::read_to_string(&briefing_file) {
        if !content.trim().is_empty() {
            return Some(content);
        }
    }
    db_notes.clone()
}

/// Populate audit_status, ai_todo_count and needs_docs_migration on a
/// project (computed from filesystem, NOT persisted in DB).
///
/// Side-effect : self-heals projects that migrated BEFORE the
/// `docs/index.md` generation shipped — if `docs/AGENTS.md` is there
/// but `docs/index.md` is missing, we drop one in. Idempotent and
/// silent (best-effort write, debug-logs on failure).
pub(crate) fn enrich_audit_status(project: &mut Project) {
    project.audit_status = scanner::detect_audit_status(&project.path);
    project.ai_todo_count = scanner::count_ai_todos(&project.path);
    let resolved = scanner::resolve_host_path(&project.path);
    project.needs_docs_migration = scanner::needs_docs_migration(&resolved);
    crate::core::docs_migration::backfill_docs_index(&resolved);
    // Self-heal `{{PROJECT_NAME}}` / `{{STACK_SUMMARY}}` / `{{TEST_CMD}}`
    // / `{{LINT_CMD}}` / `{{PROJECT_LANGUAGE}}` placeholders that the
    // agent's bootstrap step 1 was supposed to fill. We only fire when
    // the audit status is `TemplateInstalled` (= the scanner saw
    // unfilled `{{...}}` tokens) so projects past bootstrap pay zero
    // I/O cost. Targets only the 5 placeholders we can derive from
    // the filesystem; agent-filled fields are left alone. After the
    // first list-fetch following a Kronn upgrade, retroactively-broken
    // projects (e.g. amp-easy-backo, user-reported 2026-05-11) have
    // their cookie-cutter placeholders replaced with sensible defaults.
    if matches!(project.audit_status, crate::models::AiAuditStatus::TemplateInstalled) {
        let _ = crate::core::docs_migration::prefill_template_placeholders(&resolved);
        // Re-detect after the heal — the placeholder regex no longer
        // matches our 5, so the next status read can advance past
        // `TemplateInstalled` if all `{{...}}` were ours.
        project.audit_status = scanner::detect_audit_status(&project.path);
    }
}

/// Find the common parent directory of existing projects.
/// E.g. if projects are at /home/user/Repos/A and /home/user/Repos/B, returns /home/user/Repos.
pub(super) fn find_common_parent(projects: &[Project]) -> Option<String> {
    let paths: Vec<&str> = projects.iter().map(|p| p.path.as_str()).collect();
    if paths.is_empty() {
        return None;
    }
    let first: Vec<&str> = paths[0].split('/').collect();
    let mut prefix_len = first.len();
    for path in &paths[1..] {
        let parts: Vec<&str> = path.split('/').collect();
        prefix_len = prefix_len.min(parts.len());
        for i in 0..prefix_len {
            if first[i] != parts[i] {
                prefix_len = i;
                break;
            }
        }
    }
    if prefix_len <= 1 {
        return None; // just "/" — not useful
    }
    Some(first[..prefix_len].join("/"))
}

/// Determine the parent directory for new projects (shared between bootstrap and clone).
pub(super) async fn determine_parent_dir(state: &AppState) -> Result<String, String> {
    let existing = state
        .db
        .with_conn(crate::db::projects::list_projects)
        .await
        .unwrap_or_default();
    if let Some(common) = find_common_parent(&existing) {
        Ok(common)
    } else if let Ok(repos_dir) = std::env::var("KRONN_REPOS_DIR") {
        Ok(repos_dir)
    } else {
        let config = state.config.read().await;
        match config.scan.paths.first().cloned() {
            Some(p) => Ok(p),
            None => Err("No scan path configured and no existing projects.".to_string()),
        }
    }
}
