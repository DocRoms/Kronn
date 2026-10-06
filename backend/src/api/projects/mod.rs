// Project API surface — split by domain to keep each file digestible
// (TD-20260417-projects-monolith). Sub-modules are re-exported via
// `pub use *::*` so every existing `api::projects::Foo` call site keeps
// resolving without edits.

use crate::core::scanner;
use crate::models::*;
use crate::AppState;

pub mod agent_files;
pub mod anti_hallu_inject;
pub mod bootstrap;
pub mod clone;
pub mod crud;
pub mod dependencies;
pub mod docker;
pub mod git;
pub mod migrate;
pub mod resource_links;
pub mod resource_prewarm;
pub mod resources;
pub mod skill_migration;
pub mod template;
pub mod used_skills;

pub use anti_hallu_inject::*;
pub use bootstrap::*;
pub use clone::*;
pub use crud::*;
pub use dependencies::*;
pub use docker::*;
pub use git::*;
pub use migrate::*;
pub use resources::*;
pub use skill_migration::*;
pub use template::*;
pub use used_skills::{used_skill_file, used_skills};

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
pub(crate) fn sync_linked_repos_doc(
    project_path: &std::path::Path,
    repos: &[LinkedRepo],
) -> std::io::Result<()> {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    if !docs_dir.is_dir() {
        return Ok(());
    }
    sync_linked_repos_doc_in(&docs_dir, repos)
}

pub(crate) fn sync_linked_repos_doc_in(
    docs_dir: &std::path::Path,
    repos: &[LinkedRepo],
) -> std::io::Result<()> {
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

/// KT-926 — the ONLY block that names a repo other than the audited one, and
/// only ones the user linked to the project themselves. The prompt goes to the
/// model provider, so nothing else about the machine's other Kronn projects is
/// ever rendered into an agent prompt: there is no pool of candidates.
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
    enrich_audit_status_with_runs(project, false);
}

/// `completed_full_run`: this instance recorded a Completed Full audit of the
/// project, which counts as audited when the branch carries no evidence file.
pub(crate) fn enrich_audit_status_with_runs(project: &mut Project, completed_full_run: bool) {
    let resolved = scanner::resolve_host_path(&project.path);
    // A project whose directory is gone — e.g. after a cross-OS import where
    // absolute paths don't translate (WSL `/home/...` ⇄ macOS `/Users/...`) —
    // is flagged for remap and skips every filesystem scan + self-heal write
    // below: they're pointless on a missing dir, and the self-heal writes could
    // land somewhere unintended. The UI surfaces the flag as a banner + badge.
    project.path_exists = resolved.exists();
    project.write_access = Some(scanner::diagnose_project_write_access(&project.path));
    if !project.path_exists {
        project.audit_status = crate::models::AiAuditStatus::default();
        project.ai_todo_count = 0;
        project.tech_debt_count = 0;
        project.needs_docs_migration = false;
        return;
    }
    project.audit_status =
        scanner::detect_audit_status_with_runs(&project.path, completed_full_run);
    project.ai_todo_count = scanner::count_ai_todos(&project.path);
    project.tech_debt_count = scanner::count_tech_debt(&project.path);
    project.needs_docs_migration = scanner::needs_docs_migration(&resolved);
    // Read paths must not write (Codex A2). Two heals used to fire from here on
    // every list and every get:
    //
    // - the retroactive `{{PROJECT_NAME}}` / `{{STACK_SUMMARY}}` / … placeholder
    //   fill, moved earlier to the WRITE paths that own the docs tree — template
    //   install and audit Phase 1 — where `prefill_template_placeholders` is
    //   invoked explicitly;
    // - `backfill_docs_index`, removed here for the same reason. Template
    //   install already calls `ensure_docs_index`, so the only projects it
    //   served were legacy trees whose index predates that call — and they heal
    //   on their next install or audit, exactly like the placeholders.
    //
    // A GET that writes is also a GET that can fail on a read-only checkout, and
    // it made `/api/projects` do disk work no reader asked for.
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
    } else if let Ok(repos_dir) = crate::core::child_env::var("KRONN_REPOS_DIR") {
        Ok(repos_dir)
    } else {
        let config = state.config.read().await;
        match config.scan.paths.first().cloned() {
            Some(p) => Ok(p),
            None => Err("No scan path configured and no existing projects.".to_string()),
        }
    }
}

/// Re-sync a project's on-disk agent assets to its CURRENT (DB) path.
///
/// Writes the MCP plugins the project is configured for (`.mcp.json`,
/// `.gemini/settings.json`, …) plus its native skill/profile files
/// (`SKILL.md`, agent files). Call this after the project's path changes —
/// a remap or a clone-into-existing — so the new directory gets the same
/// plugins/skills the project had before the move. Without it, a freshly
/// remapped project shows its configured plugins in the UI but the agent
/// running in that directory never sees them on disk.
///
/// Best-effort: every step logs on failure and never fails the caller.
/// MCP sync is skipped when no encryption secret is configured (the env
/// secrets can't be decrypted to write them out).
pub(crate) async fn resync_project_assets(state: &AppState, project_id: &str) {
    let secret = state.config.read().await.encryption_secret.clone();
    let pid = project_id.to_string();
    let _ = state
        .db
        .with_conn(move |conn| {
            // Plugins (MCP configs) → per-agent config files in the project root.
            // The sync reads the project's path from the DB,
            // so it picks up the path we just updated.
            if let Some(ref s) = secret {
                crate::core::mcp_scanner::sync_project_with_report(conn, &pid, s);
            } else {
                tracing::debug!("Skipping MCP re-sync for project {pid}: no encryption secret configured");
            }
            // Native skills + profiles → SKILL.md / agent files, unless the
            // project keeps Kronn's files out of its repository (KT-971).
            let outside = crate::db::projects::agent_files_policy(conn, &pid)?
                == AgentFilesPolicy::Outside;
            if let Some(project) = crate::db::projects::get_project(conn, &pid)?.filter(|_| !outside) {
                let profile_ids: Vec<String> = project.default_profile_id.iter().cloned().collect();
                if let Err(e) = crate::core::native_files::sync_project_native_files_full(
                    &project.path,
                    &project.default_skill_ids,
                    &profile_ids,
                ) {
                    tracing::warn!("Native skill/profile re-sync failed for project {pid} after path change: {e}");
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .await;
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
    fn enriching_a_project_never_writes_to_its_docs_tree() {
        // `enrich_audit_status` serves GET /api/projects and GET /api/projects/{id}.
        // It used to backfill `docs/index.md` from there — a read path that
        // writes, which also fails on a read-only checkout. Both write paths
        // that own the tree call `ensure_docs_index` themselves.
        let dir = tempfile::TempDir::new().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("AGENTS.md"), "# Agents\n").unwrap();
        // The exact shape the old backfill fired on: AGENTS.md present,
        // index.md absent.
        assert!(!docs.join("index.md").exists());

        let mut project = crate::models::Project {
            id: "p-read-only".into(),
            name: "read-only".into(),
            path: dir.path().to_string_lossy().into_owned(),
            repo_url: None,
            token_override: None,
            ai_config: crate::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: crate::models::AiAuditStatus::NoTemplate,
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            write_access: None,
            mcp_sync_report: None,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: vec![],
            workspace: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        enrich_audit_status(&mut project);

        assert!(
            !docs.join("index.md").exists(),
            "reading a project must not create files in its docs tree"
        );
        // The read itself still works: this is not a regression in what it reports.
        assert!(project.path_exists);
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
            lr(
                "backend-api",
                "api",
                "/home/priol/Repos/backend-api",
                "GraphQL schema lives here",
            ),
            lr("infra", "iac", "https://github.com/org/infra", ""),
        ])
        .expect("non-empty list must produce a block");
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
        let block =
            format_linked_repos_for_prompt(&[lr("api", "api", "/path/to/api", "")]).unwrap();
        assert!(
            block.contains("docs/AGENTS.md"),
            "skill must instruct the agent to start with docs/AGENTS.md when reading linked repos"
        );
        assert!(
            block.to_lowercase().contains("canonical")
                || block.to_lowercase().contains("entry point"),
            "block should frame docs/AGENTS.md as the canonical entry point"
        );
    }

    #[test]
    fn format_linked_repos_for_prompt_explains_when_to_consult() {
        // The header sentence must teach the agent the trigger:
        // "cross-project context" / "check the relevant companion
        // before asking the user". Locks in the why so future edits
        // of the helper don't accidentally strip the rationale.
        let block = format_linked_repos_for_prompt(&[lr("api", "api", "/x", "")]).unwrap();
        assert!(block.to_lowercase().contains("cross-project"));
        assert!(
            block.to_lowercase().contains("before asking the user")
                || block.to_lowercase().contains("when a task references")
        );
    }

    fn mk_project(id: &str, name: &str, path: &str) -> Project {
        let now = chrono::Utc::now();
        Project {
            id: id.into(),
            name: name.into(),
            path: path.into(),
            repo_url: None,
            token_override: None,
            ai_config: AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: Default::default(),
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            write_access: None,
            mcp_sync_report: None,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: vec![],
            workspace: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn enrich_flags_missing_path_and_skips_scans() {
        // A path that cannot exist (post cross-OS import) must be flagged
        // `path_exists = false` so the UI can offer a remap, and the heavy
        // filesystem scans must be skipped (defaults left in place).
        let mut p = mk_project("p1", "ghost", "/nonexistent/kronn/ghost-project-xyz");
        p.ai_todo_count = 99; // pretend a stale enrichment lingered
        enrich_audit_status(&mut p);
        assert!(!p.path_exists, "missing dir must flag path_exists = false");
        assert_eq!(
            p.ai_todo_count, 0,
            "missing dir must reset to defaults, not scan"
        );
        assert_eq!(p.audit_status, AiAuditStatus::default());
    }

    #[test]
    fn enrich_marks_existing_path_present() {
        // A real directory (the OS temp dir always exists) resolves present.
        let tmp = std::env::temp_dir();
        let mut p = mk_project("p2", "real", tmp.to_str().unwrap());
        p.path_exists = false; // force the opposite to prove enrich sets it
        enrich_audit_status(&mut p);
        assert!(
            p.path_exists,
            "an existing directory must flag path_exists = true"
        );
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
        assert!(
            body.starts_with("# Linked repositories"),
            "doc must start with a Markdown H1 (it lives at `docs/linked-repos.md`)"
        );
        assert!(
            body.contains("Read this file ONLY when"),
            "doc must teach the agent when to read (pull semantics)"
        );
        assert!(
            body.contains("docs/AGENTS.md"),
            "doc must still point at the canonical companion entry point"
        );
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
            lr("api", "api", "/r/api", "GraphQL"),
        ];
        sync_linked_repos_doc_in(&docs, &repos).unwrap();
        let target = docs.join("linked-repos.md");
        assert!(target.exists(), "non-empty list must write the file");
        let body = std::fs::read_to_string(&target).unwrap();
        assert!(
            body.contains("front") && body.contains("api"),
            "both entries must be in the file"
        );
        // Now sync with []: file must vanish.
        sync_linked_repos_doc_in(&docs, &[]).unwrap();
        assert!(
            !target.exists(),
            "empty list must remove the stale file (no contradictory state on disk)"
        );
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

    // Note: what an agent prompt says about OTHER repos (KT-926) is pinned
    // end to end where a full AppState is wired: the audit pipelines in
    // `api/audit/agent_launch_tests.rs`, discussions in
    // `api/discussions/companion_privacy_tests.rs`, workflow Agent steps in
    // `workflows/runner.rs`. Unit-level here covers the format functions.
}
