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

/// 0.8.3 — Format the list of OTHER Kronn-registered projects as a
/// candidate pool for the audit agent to look for companion-repo
/// evidence in. Scales to users with 200+ repos on disk because we
/// only consider projects already known to Kronn (typically 5-20),
/// not the whole filesystem.
///
/// Returns `None` when there are no other projects (the current one
/// is the only project in Kronn — nothing to suggest).
///
/// The block uses `## Other Kronn projects on this machine` as
/// header. Each entry shows name + path so the agent can verify
/// evidence with file reads. The current project is excluded by id
/// to keep self-references out of the suggestions.
pub(crate) fn format_kronn_projects_universe_for_prompt(
    other_projects: &[Project],
    current_project_id: &str,
) -> Option<String> {
    let candidates: Vec<&Project> = other_projects
        .iter()
        .filter(|p| p.id != current_project_id)
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let mut out = String::from(
        "## Other Kronn projects on this machine (companion-repo candidate pool)\n\
         The user has the following other projects registered in Kronn. If you find evidence in the current project — a manifest reference (`composer.json`, `package.json`, `go.mod`, `Cargo.toml`), a `docker-compose.yml` build path, a `.gitmodules` entry, a code import with an absolute path, or a README mention — that links this project to one of these, surface it.\n\
         \n\
         **Detection rule**: only suggest links you can BACK with a file:line citation. Do NOT guess from naming similarity alone (e.g. `front_api` vs `front_apollo` are not necessarily linked — verify with the manifests).\n\
         \n\
         **Efficient lookup**: each candidate is a Kronn-registered project, so it has a `docs/AGENTS.md` at the path above. When you need to confirm what a candidate does (to decide if it's actually a companion vs just a sibling repo), read its `docs/AGENTS.md` FIRST — that's where the canonical project description lives. Random file scans of unfamiliar repos waste tokens.\n\
         \n\
         **If you find any companion**, write a `## Suggested companion repos` section in `docs/AGENTS.md` listing each finding as:\n\
         \n\
         ```\n\
         - **<other-project-name>** — `<location>` — evidence: `<file:line or quote>`\n\
         ```\n\
         \n\
         If you find none, OMIT the section entirely — silence means \"no companions detected\".\n\
         \n\
         **Candidate pool** (only these projects are valid suggestions):\n\n"
    );
    for p in candidates {
        out.push_str(&format!("- **{}** — `{}`\n", p.name, p.path));
    }
    Some(out)
}

/// 0.8.3 — Format `Project.linked_repos` for inclusion in agent /
/// audit prompts. Returns `None` when the project has no linked
/// repos (so the caller can `if let Some(...)` and skip the
/// section header entirely). Capped at 20 entries by the API
/// validator so the block stays bounded.
///
/// The block uses `## Linked repositories (companion repos)` as
/// header so it composes with the existing `## Project briefing
/// (from the user)` block already injected by the audit pipeline.
/// Each entry shows kind + name + location + (optional) description
/// — agents see all four fields and decide when/how to read each
/// repo.
/// 0.8.4 (#295) — render the linked_repos table for `docs/linked-repos.md`.
/// Same content as the prompt-injection block but without the
/// "read this NOW" framing — this is a doc artifact the agent reads
/// on-demand. Returns `None` when there are no companion repos so
/// the caller can `unlink` the file (avoids stub clutter).
pub(crate) fn format_linked_repos_for_docs(repos: &[LinkedRepo]) -> Option<String> {
    if repos.is_empty() {
        return None;
    }
    let mut out = String::from(
        "# Linked repositories\n\n\
         > Companion repos for cross-project context. Read this file ONLY when the current task references something not in this repo.\n\n\
         ## How to read a linked repo\n\n\
         Start with `<repo-path>/docs/AGENTS.md` (same Kronn entry point used by this repo).\n\
         Only fall back to file scans / READMEs if AGENTS.md doesn't answer.\n\n\
         ## Repositories\n\n"
    );
    for r in repos {
        out.push_str(&format!(
            "- **{name}** ({kind}) → `{location}`",
            name = r.name,
            kind = r.kind,
            location = r.location,
        ));
        if !r.description.is_empty() {
            out.push_str(&format!(" — {}", r.description));
        }
        out.push('\n');
    }
    Some(out)
}

/// 0.8.4 (#295) — write `docs/linked-repos.md` from the project's
/// `linked_repos` list, or delete the file when the list is empty.
/// Idempotent. Called on project CRUD (linked_repos PUT) AND on each
/// audit Phase 1 so existing projects pick up the file on next audit.
///
/// Two variants:
///   - `sync_linked_repos_doc(project_path)` — auto-detects `docs/`
///     via `detect_docs_dir`; no-op if not bootstrapped yet.
///   - `sync_linked_repos_doc_in(docs_dir)` — called by the audit
///     Phase 1 which already knows the exact docs path.
pub(crate) fn sync_linked_repos_doc(project_path: &std::path::Path, repos: &[LinkedRepo]) -> std::io::Result<()> {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    if !docs_dir.is_dir() {
        return Ok(());
    }
    sync_linked_repos_doc_in(&docs_dir, repos)
}

pub(crate) fn sync_linked_repos_doc_in(docs_dir: &std::path::Path, repos: &[LinkedRepo]) -> std::io::Result<()> {
    if !docs_dir.is_dir() {
        return Ok(());
    }
    let target = docs_dir.join("linked-repos.md");
    match format_linked_repos_for_docs(repos) {
        Some(body) => std::fs::write(&target, body),
        None => match std::fs::remove_file(&target) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
    }
}

pub(crate) fn format_linked_repos_for_prompt(repos: &[LinkedRepo]) -> Option<String> {
    if repos.is_empty() {
        return None;
    }
    let mut out = String::from(
        "## Linked repositories (companion repos)\n\
         This project has companion repos that you may need to read for cross-project context (frontend ↔ API, app ↔ IaC, etc.). When a task references a concept that isn't in the current repo, check the relevant companion before asking the user — the path/URL is given below.\n\
         \n\
         **How to read a linked repo** — start with `<repo-path>/docs/AGENTS.md` (Kronn's canonical project-context entry point — every Kronn-bootstrapped repo has one). Only fall back to random file scans / READMEs if AGENTS.md doesn't answer your question. This saves you parse cycles AND ensures you use the same context layer the repo's own agents use.\n\n"
    );
    for r in repos {
        out.push_str(&format!(
            "- **{name}** ({kind}) → `{location}`",
            name = r.name,
            kind = r.kind,
            location = r.location,
        ));
        if !r.description.is_empty() {
            out.push_str(&format!(" — {}", r.description));
        }
        out.push('\n');
    }
    Some(out)
}

/// 0.8.3 — consolidate `linked_repos` + Kronn-projects-universe blocks
/// into a single ready-to-append context string for any agent surface
/// (discussion streaming, orchestration handoffs, workflow runner).
///
/// Pre-pads each block with `\n\n` so the caller can splice it onto the
/// end of any prompt without worrying about delimiters. Returns an
/// empty string when:
///   - `project_id` is `None` (general discussion, no project binding)
///   - the project has neither `linked_repos` nor sibling projects in Kronn
///
/// One DB round-trip for the project, a second for `list_projects`
/// (the candidate-pool for companion suggestion). Both queries are
/// cheap; callers wire this helper into any agent-prompt assembly that
/// has access to `AppState`.
///
/// 0.8.4 (#295) — only `compute_companion_context_for_audit` injects
/// `linked_repos` now; the disc/WF surfaces call the regular version
/// which drops `linked_repos` since `docs/linked-repos.md` is on disk
/// and the agent reads it on-demand (saves 500-2000 tokens/message).
/// The Kronn-projects-universe block stays everywhere — it's small
/// + lists Kronn-managed companions which is qualitatively different.
pub(crate) async fn compute_companion_context(
    state: &AppState,
    project_id: Option<&str>,
) -> String {
    compute_companion_context_inner(state, project_id, false).await
}

/// 0.8.4 (#295) — audit-only variant that ALSO injects the
/// `linked_repos` block (cross-repo findings depend on it — proven
/// -39% tokens on big-ticket migrations). Disc/WF use the regular fn
/// which skips linked_repos to keep per-message prompts lean.
///
/// Today the audit pipeline composes its own block via
/// `format_linked_repos_for_prompt` for finer-grained control over
/// the prompt sections; this helper is kept (with its dedicated tests)
/// as the canonical entry point for future audit-surface refactors.
#[allow(dead_code)]
pub(crate) async fn compute_companion_context_for_audit(
    state: &AppState,
    project_id: Option<&str>,
) -> String {
    compute_companion_context_inner(state, project_id, true).await
}

async fn compute_companion_context_inner(
    state: &AppState,
    project_id: Option<&str>,
    include_linked_repos: bool,
) -> String {
    let Some(pid) = project_id else {
        return String::new();
    };
    let pid_clone = pid.to_string();
    let project_opt = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid_clone))
        .await
        .ok()
        .flatten();
    let Some(project) = project_opt else {
        return String::new();
    };
    // 0.8.4 (#295) — gate the linked_repos injection on the
    // `include_linked_repos` flag. False = disc/WF (push→pull
    // migration), True = audit (cross-repo findings need it inline).
    let linked_block = if include_linked_repos {
        format_linked_repos_for_prompt(&project.linked_repos)
    } else {
        None
    };
    let pid_for_universe = pid.to_string();
    let universe_block = match state
        .db
        .with_conn(crate::db::projects::list_projects)
        .await
    {
        Ok(all) => format_kronn_projects_universe_for_prompt(&all, &pid_for_universe),
        Err(e) => {
            tracing::warn!(
                "Failed to load Kronn projects for companion-context block: {}",
                e
            );
            None
        }
    };
    let mut extra = String::new();
    if let Some(b) = linked_block {
        extra.push_str("\n\n");
        extra.push_str(&b);
    }
    if let Some(b) = universe_block {
        extra.push_str("\n\n");
        extra.push_str(&b);
    }
    extra
}

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
    project.tech_debt_count = scanner::count_tech_debt(&project.path);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lr(name: &str, kind: &str, location: &str, description: &str) -> LinkedRepo {
        LinkedRepo {
            id: format!("lr-{name}"),
            name: name.into(),
            kind: kind.into(),
            location: location.into(),
            description: description.into(),
        }
    }

    #[test]
    fn format_linked_repos_for_prompt_returns_none_when_empty() {
        // Empty list = caller skips the section entirely (no header
        // injected for a no-op).
        assert!(format_linked_repos_for_prompt(&[]).is_none());
    }

    #[test]
    fn format_linked_repos_for_prompt_renders_each_entry_with_kind_and_location() {
        let block = format_linked_repos_for_prompt(&[
            lr("backend-api", "api", "/home/priol/Repos/backend-api", "GraphQL schema lives here"),
            lr("infra", "iac", "https://github.com/org/infra", ""),
        ]).expect("non-empty list must produce a block");
        assert!(block.contains("## Linked repositories"));
        assert!(block.contains("**backend-api** (api)"));
        assert!(block.contains("/home/priol/Repos/backend-api"));
        assert!(block.contains("GraphQL schema lives here"));
        // Entry without description has no trailing "— "
        assert!(block.contains("**infra** (iac) → `https://github.com/org/infra`\n"));
        assert!(!block.contains("**infra** (iac) → `https://github.com/org/infra` —"));
    }

    #[test]
    fn format_linked_repos_for_prompt_instructs_to_read_agents_md_first() {
        // Critical hint: when the agent needs to read a linked repo,
        // it must start with `<repo>/docs/AGENTS.md`. Without this
        // instruction it does random file scans on unfamiliar
        // codebases and burns tokens. Lock the hint here.
        let block = format_linked_repos_for_prompt(&[
            lr("api", "api", "/path/to/api", "")
        ]).unwrap();
        assert!(block.contains("docs/AGENTS.md"),
            "skill must instruct the agent to start with docs/AGENTS.md when reading linked repos");
        assert!(block.to_lowercase().contains("canonical")
             || block.to_lowercase().contains("entry point"),
            "block should frame docs/AGENTS.md as the canonical entry point");
    }

    #[test]
    fn format_linked_repos_for_prompt_explains_when_to_consult() {
        // The header sentence must teach the agent the trigger:
        // "cross-project context" / "check the relevant companion
        // before asking the user". Locks in the why so future edits
        // of the helper don't accidentally strip the rationale.
        let block = format_linked_repos_for_prompt(&[
            lr("api", "api", "/x", "")
        ]).unwrap();
        assert!(block.to_lowercase().contains("cross-project"));
        assert!(block.to_lowercase().contains("before asking the user")
             || block.to_lowercase().contains("when a task references"));
    }

    // ─── format_kronn_projects_universe_for_prompt — 0.8.3 ─────────────

    fn mk_project(id: &str, name: &str, path: &str) -> Project {
        let now = chrono::Utc::now();
        Project {
            id: id.into(), name: name.into(), path: path.into(),
            repo_url: None, token_override: None,
            ai_config: AiConfigStatus { detected: false, configs: vec![] },
            audit_status: Default::default(),
            ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false,
            default_skill_ids: vec![], default_profile_id: None,
            briefing_notes: None, linked_repos: vec![],
            created_at: now, updated_at: now,
        }
    }

    #[test]
    fn universe_returns_none_when_only_current_project_exists() {
        // No other projects = nothing to suggest. Caller skips the
        // block entirely (no header injected for a no-op).
        let projects = vec![mk_project("p1", "alone", "/home/u/Repos/alone")];
        assert!(format_kronn_projects_universe_for_prompt(&projects, "p1").is_none());
    }

    #[test]
    fn universe_excludes_current_project_by_id() {
        // The current project must NOT appear in its own suggestion
        // pool (would be a self-reference).
        let projects = vec![
            mk_project("p1", "current", "/r/current"),
            mk_project("p2", "front_api", "/r/front_api"),
        ];
        let block = format_kronn_projects_universe_for_prompt(&projects, "p1").unwrap();
        assert!(block.contains("front_api"));
        assert!(!block.contains("**current**"), "current project must not be suggested as its own companion");
    }

    #[test]
    fn universe_instructs_evidence_required() {
        // The prompt must demand evidence (file:line citation) so
        // the agent doesn't suggest links from naming similarity
        // alone. Locks the safety rail in place.
        let projects = vec![
            mk_project("p1", "current", "/r/current"),
            mk_project("p2", "front_api", "/r/front_api"),
        ];
        let block = format_kronn_projects_universe_for_prompt(&projects, "p1").unwrap();
        assert!(block.to_lowercase().contains("evidence"),
            "block must require evidence before suggesting a link");
        assert!(block.contains("file:line") || block.contains("citation"),
            "block must demand a citation");
        assert!(block.to_lowercase().contains("guess") || block.to_lowercase().contains("naming similarity"),
            "block must warn against naming-similarity guesses");
    }

    #[test]
    fn universe_instructs_to_read_agents_md_first_on_candidates() {
        // Same rule as for actual linked_repos: when probing a
        // candidate, start with its docs/AGENTS.md instead of
        // random file scans.
        let projects = vec![
            mk_project("p1", "current", "/r/current"),
            mk_project("p2", "front_api", "/r/front_api"),
        ];
        let block = format_kronn_projects_universe_for_prompt(&projects, "p1").unwrap();
        assert!(block.contains("docs/AGENTS.md"),
            "universe block must instruct the agent to read candidates' AGENTS.md FIRST");
    }

    #[test]
    fn universe_instructs_to_write_findings_to_agents_md() {
        // The output side: agent should write findings to a
        // specific section in docs/AGENTS.md so the user can read
        // + manually add them via the UI.
        let projects = vec![
            mk_project("p1", "current", "/r/current"),
            mk_project("p2", "front_api", "/r/front_api"),
        ];
        let block = format_kronn_projects_universe_for_prompt(&projects, "p1").unwrap();
        assert!(block.contains("## Suggested companion repos"),
            "universe block must specify the section name the agent should write to");
        assert!(block.contains("OMIT the section entirely") || block.contains("omit the section entirely")
             || block.to_lowercase().contains("silence means"),
            "block must instruct: no findings = no section (avoid noise)");
    }

    #[test]
    fn universe_lists_each_candidate_with_name_and_path() {
        let projects = vec![
            mk_project("p1", "current", "/r/current"),
            mk_project("p2", "front_api", "/r/front_api"),
            mk_project("p3", "infra", "/r/infra"),
        ];
        let block = format_kronn_projects_universe_for_prompt(&projects, "p1").unwrap();
        assert!(block.contains("**front_api** — `/r/front_api`"));
        assert!(block.contains("**infra** — `/r/infra`"));
    }

    // ─── compute_companion_context — 0.8.3 (TD-265) ──────────────────────
    //
    // Integration-style tests for the async helper that consolidates the
    // two blocks into a single ready-to-append context string for any
    // agent surface (workflow runner, discussions, orchestration).

    use crate::db::Database;
    use crate::AppState;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn test_state() -> AppState {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    #[tokio::test]
    async fn compute_companion_context_returns_empty_for_no_project() {
        // General discussions (no project) get an empty context — the
        // caller must short-circuit and skip the section header
        // entirely so the prompt prelude stays clean.
        let state = test_state();
        let ctx = compute_companion_context(&state, None).await;
        assert!(ctx.is_empty(), "no project → empty string, got: {ctx:?}");
    }

    #[tokio::test]
    async fn compute_companion_context_returns_empty_for_unknown_project() {
        // A project_id that doesn't resolve also yields empty —
        // defensive guard against stale references (run row pointing
        // at a deleted project, etc.). Symmetric with the
        // None case so callers don't have to branch.
        let state = test_state();
        let ctx = compute_companion_context(&state, Some("nonexistent")).await;
        assert!(ctx.is_empty(), "unknown project → empty string, got: {ctx:?}");
    }

    #[tokio::test]
    async fn compute_companion_context_drops_linked_repos_for_disc_wf_pulls() {
        // 0.8.4 (#295) — the disc/WF code path uses
        // `compute_companion_context` (no `_for_audit`). It must NOT
        // inject the linked_repos block inline anymore — the agent
        // reads `docs/linked-repos.md` on-demand instead, saving
        // 500-2000 tokens per message on a long discussion. The
        // audit variant gets the inline block via
        // `compute_companion_context_for_audit` (pinned in the test
        // right below).
        let state = test_state();
        let pid = "p_test".to_string();
        let project = Project {
            id: pid.clone(),
            name: "test-current".into(),
            path: "/r/test-current".into(),
            linked_repos: vec![lr(
                "test-api",
                "api",
                "/r/test-api",
                "GraphQL schema lives here",
            )],
            ..mk_project(&pid, "test-current", "/r/test-current")
        };
        state.db.with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &project)?;
            Ok::<_, anyhow::Error>(())
        }).await.expect("insert project");

        let ctx = compute_companion_context(&state, Some(&pid)).await;
        assert!(!ctx.contains("Linked repositories (companion repos)"),
            "disc/WF must NOT inject linked_repos inline anymore (read docs/linked-repos.md on-demand), got: {ctx:?}");
    }

    #[tokio::test]
    async fn compute_companion_context_for_audit_keeps_linked_repos_inline() {
        // 0.8.4 (#295) — the audit code path uses
        // `compute_companion_context_for_audit` which KEEPS the inline
        // block. Cross-repo findings depend on it (proven -39% tokens
        // + bug catch on EW-7247).
        let state = test_state();
        let pid = "p_test".to_string();
        let project = Project {
            id: pid.clone(),
            name: "test-current".into(),
            path: "/r/test-current".into(),
            linked_repos: vec![lr(
                "test-api",
                "api",
                "/r/test-api",
                "GraphQL schema lives here",
            )],
            ..mk_project(&pid, "test-current", "/r/test-current")
        };
        state.db.with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &project)?;
            Ok::<_, anyhow::Error>(())
        }).await.expect("insert project");

        let ctx = compute_companion_context_for_audit(&state, Some(&pid)).await;
        assert!(ctx.contains("Linked repositories (companion repos)"),
            "audit variant must keep linked_repos block inline, got: {ctx:?}");
        assert!(ctx.contains("**test-api** (api)"),
            "linked repo entry must be rendered");
        // Pre-padding so the caller can splice without worrying about
        // delimiters — the very first char of the context should be newline.
        assert!(ctx.starts_with("\n\n"),
            "context must be pre-padded with \\n\\n");
    }

    // ── Source-level wiring guards — 0.8.3 (TD-267 + TD-268) ──────────────
    //
    // The companion_context helper above is well-tested in isolation,
    // but the FULL benefit only materializes when each agent surface
    // actually CALLS it. A future refactor (e.g. removing an unused
    // import, "tidying" prompt assembly) could silently drop the call
    // and we wouldn't notice until a user complains that the agent
    // forgot a linked repo.
    //
    // These tests read the source files at compile time and assert
    // the wiring is in place. Brittle by design — any move/rename
    // surfaces here loudly. The single line each test scans for is
    // the contract; if you legitimately move the call site, update
    // the regex too.

    fn read_source(rel: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    #[test]
    fn discussions_streaming_calls_compute_companion_context() {
        // TD-265 + TD-267: the user-facing chat path must inject
        // linked_repos into context_files_prompt before passing it to
        // start_agent_with_config. Without this wiring, a user
        // chatting in a project discussion can't see the companion
        // repos they registered on the project.
        let src = read_source("src/api/discussions/streaming.rs");
        assert!(
            src.contains("compute_companion_context"),
            "discussions/streaming.rs must call compute_companion_context — \
             user chat with an agent regresses to no linked_repos visibility"
        );
        assert!(
            src.contains("companion_context"),
            "the result must be named `companion_context` (consumed by context_files_prompt below)"
        );
    }

    #[test]
    fn orchestration_calls_compute_companion_context_once_at_setup() {
        // TD-265 + TD-268: the multi-agent debate orchestration
        // computes the block ONCE at setup and reuses it across all
        // agent rounds + the synthesis pass. Computing it per-round
        // would N-double the DB cost; not computing it at all would
        // regress the multi-agent path to no linked_repos visibility.
        let src = read_source("src/api/discussions/orchestration.rs");
        let occurrences = src.matches("compute_companion_context").count();
        assert!(
            occurrences >= 1,
            "orchestration.rs must call compute_companion_context at least once \
             (currently {} call(s)) — multi-agent debate regresses to no linked_repos",
            occurrences
        );
        // Pre-computed binding is named `companion_context` and is
        // referenced in the user-facing agent calls (debate + synthesis)
        // via `&companion_context` as `context_files_prompt`. The 3
        // internal summarization calls intentionally pass `""` because
        // companion repos are noise in a "compress conversation" prompt.
        let debate_and_synth_refs = src.matches("context_files_prompt: &companion_context").count();
        assert!(
            debate_and_synth_refs >= 2,
            "expected at least 2 user-facing agent calls to pass &companion_context as context_files_prompt \
             (debate round + final synthesis), found {}",
            debate_and_synth_refs
        );
    }

    #[test]
    fn orchestration_summarization_passes_empty_context_files_prompt() {
        // The 3 internal summarization sites (line 286 main-disc
        // summary, line 689 generate_summary_on_demand, line 864
        // generate_summary_on_demand inner) compress conversation
        // history into a brief — they don't reason about the project.
        // Injecting linked_repos there would waste tokens on every
        // summary pass without producing better summaries.
        //
        // This guard fires if someone "fixes" them by symmetrically
        // adding `&companion_context` and we end up paying 3x the
        // companion_context tokens per turn.
        let src = read_source("src/api/discussions/orchestration.rs");
        // We need at least 3 explicit `context_files_prompt: ""` to
        // remain. There may be 4 (one per summarization call) — that's
        // also fine. What we DON'T want is for that count to drop.
        let empty_calls = src.matches("context_files_prompt: \"\"").count();
        assert!(
            empty_calls >= 3,
            "expected ≥3 summarization sites to keep `context_files_prompt: \"\"` (token-saver), \
             found {}. If you intentionally added companion_context to a summary pass, \
             confirm it actually improves the summary before relaxing this guard.",
            empty_calls
        );
    }

    #[test]
    fn workflow_runner_calls_compute_companion_context() {
        // TD-258: workflow runner is the original surface for the
        // cross-repo evidence wiring. Same guard as the discussion
        // surfaces above — the run-once-at-start pattern is what
        // keeps the cost bounded.
        let src = read_source("src/workflows/runner.rs");
        assert!(
            src.contains("compute_companion_context"),
            "workflow runner must call compute_companion_context once per run"
        );
        assert!(
            src.contains("agent_extra_context"),
            "the result must be named `agent_extra_context` — passed to execute_step's extra_context param"
        );
    }

    #[test]
    fn workflow_test_step_endpoint_calls_compute_companion_context() {
        // TD-265: the test-step preview SSE endpoint (api/workflows.rs)
        // is the user-facing "try a single step before saving" path.
        // Without this wiring, the preview would diverge from the
        // production run prompt, defeating the "what you see is what
        // you'll get" guarantee.
        let src = read_source("src/api/workflows.rs");
        assert!(
            src.contains("compute_companion_context"),
            "test-step endpoint must call compute_companion_context for preview/prod parity"
        );
    }

    #[tokio::test]
    async fn compute_companion_context_emits_universe_block_when_other_projects_exist() {
        // Two projects in DB → the universe block (candidate pool) is
        // emitted for the current project, listing the other one.
        let state = test_state();
        let current = mk_project("p_current", "current", "/r/current");
        let sibling = mk_project("p_sibling", "sibling-repo", "/r/sibling-repo");
        let cur_clone = current.clone();
        let sib_clone = sibling.clone();
        state.db.with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &cur_clone)?;
            crate::db::projects::insert_project(conn, &sib_clone)?;
            Ok::<_, anyhow::Error>(())
        }).await.expect("insert projects");

        let ctx = compute_companion_context(&state, Some("p_current")).await;
        assert!(ctx.contains("## Other Kronn projects"),
            "universe block must be present, got: {ctx:?}");
        assert!(ctx.contains("**sibling-repo** — `/r/sibling-repo`"),
            "sibling project must be listed in candidate pool");
        // Current project must NOT self-reference in its own pool.
        assert!(!ctx.contains("**current** — `/r/current`"),
            "current project must be excluded from its own candidate pool");
    }

    // ─── 0.8.4 (#295) — push → pull migration ─────────────────────────

    #[test]
    fn format_linked_repos_for_docs_returns_none_when_empty() {
        // Empty list → no file. Caller deletes any stale on-disk file.
        assert!(format_linked_repos_for_docs(&[]).is_none());
    }

    #[test]
    fn format_linked_repos_for_docs_renders_pull_friendly_header() {
        // The doc artifact framing is different from the prompt block:
        // "Read this file ONLY when the current task references something
        // not in this repo" tells the AGENT (reading the doc) when to
        // load it. The prompt block was framed as "you will need this
        // NOW" — wrong for a pull pattern.
        let repos = vec![lr("front", "frontend", "/r/front", "")];
        let body = format_linked_repos_for_docs(&repos).expect("non-empty");
        assert!(body.starts_with("# Linked repositories"),
            "doc must start with a Markdown H1 (it lives at `docs/linked-repos.md`)");
        assert!(body.contains("Read this file ONLY when"),
            "doc must teach the agent when to read (pull semantics)");
        assert!(body.contains("docs/AGENTS.md"),
            "doc must still point at the canonical companion entry point");
    }

    #[test]
    fn sync_linked_repos_doc_in_writes_then_removes() {
        // Round-trip: write the file with N entries, then sync with
        // an empty list — the file disappears so the agent doesn't
        // read a stale doc that contradicts the project state.
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        let repos = vec![
            lr("front", "frontend", "/r/front", "React app"),
            lr("api",   "api",      "/r/api",   "GraphQL"),
        ];
        sync_linked_repos_doc_in(&docs, &repos).unwrap();
        let target = docs.join("linked-repos.md");
        assert!(target.exists(), "non-empty list must write the file");
        let body = std::fs::read_to_string(&target).unwrap();
        assert!(body.contains("front") && body.contains("api"),
            "both entries must be in the file");
        // Now sync with []: file must vanish.
        sync_linked_repos_doc_in(&docs, &[]).unwrap();
        assert!(!target.exists(),
            "empty list must remove the stale file (no contradictory state on disk)");
    }

    #[test]
    fn sync_linked_repos_doc_in_idempotent_on_missing_docs_dir() {
        // Pre-bootstrap projects don't have docs/ yet. The CRUD must
        // not crash; the audit Phase 1 will recall the helper later.
        let tmp = tempfile::TempDir::new().unwrap();
        let not_a_dir = tmp.path().join("missing-docs");
        sync_linked_repos_doc_in(&not_a_dir, &[lr("x", "api", "/r/x", "")]).unwrap();
        // No file created (target dir doesn't exist).
        assert!(!not_a_dir.join("linked-repos.md").exists());
    }

    // Note: end-to-end coverage of the audit vs disc gating lives in
    // the higher-level audit / discussions integration tests where a
    // full AppState is already wired. Unit-level here is sufficient
    // for the helpers + the format functions.
}
