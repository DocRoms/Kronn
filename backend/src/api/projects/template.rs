// Template installation: idempotent copy of `templates/docs/`, the shared
// `AGENTS.md` entry point, and detected agent adapters. The helpers
// (`resolve_templates_dir`, `copy_dir_nondestructive`,
// `ensure_agent_writable_subfolders`) are `pub(crate)` because `api::audit`
// reuses them during the audit pipeline.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::agents::runner;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

/// POST /api/projects/:id/install-template
/// Copies the AI template files into the project directory.
pub async fn install_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &project.id).await {
            Ok(lease) => lease,
            Err(error) => return Json(ApiResponse::err(error)),
        };
    let project_path_str = project.path.clone();
    // A Full run this instance recorded stands for the record, as at launch.
    let recorded_run = {
        let id = project.id.clone();
        state
            .db
            .with_read_conn(move |conn| crate::db::audit_runs::has_any_full(conn, &id))
            .await
            .unwrap_or(false)
    };

    // Run filesystem I/O on blocking thread pool
    let install_result = lease
        .blocking(move || -> Result<(), String> {
            let project_path = scanner::resolve_host_path(&project_path_str);
            if !project_path.exists() {
                return Err(format!(
                    "Project path not found: {}",
                    project_path.display()
                ));
            }

            // Snapshot target-specific agent integrations before Kronn creates
            // any instruction file. The shared AGENTS.md is always installed;
            // vendor adapters are created only for agents already declared by
            // the repository.
            let detected_adapters =
                crate::core::root_agent_files::detect_agent_adapters(&project_path);

            // Permission check on whichever doc-folder the project already
            // has (`docs/`, legacy `ai/`, or alt `doc/`). Fresh projects
            // skip this — there's nothing to check yet.
            // A configured folder that fails containment stops the install (KT-263).
            let existing_docs = crate::core::docs_location::guard_write(&project_path)?;
            if existing_docs.exists() {
                if let Err(e) = crate::api::audit::check_ai_dir_permissions(&existing_docs) {
                    let folder_name = existing_docs
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("docs");
                    return Err(format!(
                    "{}/ directory exists but has permission issues that could not be fixed: {}. \
                     Run: sudo chown -R $(id -u):$(id -g) {}/{}/",
                    folder_name,
                    e,
                    project_path.display(),
                    folder_name
                ));
                }
            }

            let template_dir = resolve_templates_dir();
            if !template_dir.exists() {
                return Err(format!(
                    "Templates directory not found: {}",
                    template_dir.display()
                ));
            }

            // 0.7.1+ : ship the modern `docs/AGENTS.md` convention. Fresh
            // projects always get docs/; legacy `ai/` projects keep their
            // layout until the operator clicks Migrate.
            let docs_template = template_dir.join("docs");
            // Unconfigured, a missing folder resolves to `docs/` (fresh project).
            let docs_target = existing_docs;
            let provenance_before = crate::core::doc_provenance::snapshot_tracked(&project_path)?;
            let docs_root = crate::core::docs_location::DocsRoot::for_project(&project_path)?;
            // A folder that is not a human's becomes Kronn's by this install:
            // recorded first, so the files written below are never counted as
            // docs Kronn did not write (KT-1299). A human's docs stay theirs
            // until the briefing's choice.
            let human_folder = matches!(
                crate::core::docs_folder::folder_state_with_runs(
                    &project_path,
                    docs_root.rel(),
                    recorded_run,
                ),
                crate::core::docs_folder::DocsFolderState::HumanDocs { .. }
            );
            if !human_folder {
                crate::core::kronn_state::mark_template_installed(&project_path)?;
            }
            crate::core::template_manifest::apply_renames(&project_path, &docs_template)?;
            let mut created_files = Vec::new();
            if docs_template.is_dir() {
                created_files.extend(copy_dir_nondestructive_in(
                    &docs_template,
                    &docs_target,
                    &docs_root,
                )?);
            }
            crate::core::template_manifest::record_install(&project_path, &docs_template)?;
            ensure_agent_writable_subfolders(&project_path, &docs_target);
            // Human-friendly landing page for `docs/`. Idempotent, best-effort.
            if let Err(e) =
                crate::core::docs_migration::ensure_docs_index(&project_path, &docs_target)
            {
                tracing::warn!("docs index not installed: {e}");
            }

            let adapter_report = crate::core::root_agent_files::install_detected_agent_files(
                &project_path,
                &template_dir,
                &detected_adapters,
            );
            created_files.extend(crate::core::root_agent_files::created_agent_paths(
                &project_path,
                &adapter_report,
            ));
            for failure in adapter_report.failed {
                tracing::warn!("Agent instruction install failed: {failure}");
            }

            // Pre-fill template placeholders with filesystem-derived defaults —
            // scoped EXCLUSIVELY to the files this very call created (ownership
            // by construction, Codex A2): a pre-existing docs/ tree or root file
            // is user content and is never walked.
            let _ = crate::core::docs_migration::prefill_files(&project_path, &created_files);
            crate::core::doc_provenance::note_snapshot_writes(
                &project_path,
                &provenance_before,
                crate::core::doc_provenance::DocWriter::TemplateReinstall,
            )
            .map_err(|e| {
                format!("Template files installed, but provenance could not be recorded: {e}")
            })?;

            // KT-841 — no more "FIRST-RUN TASK" prompt injected into
            // docs/AGENTS.md at install time: that block lived in the exact
            // file every subsequent agent reads as "the tiered context loader",
            // so a run that never reached Step 9/10's self-delete (interrupted
            // audit, or a manually-invoked agent that never ran Kronn's audit
            // at all) left it there permanently, confusing later reads. The
            // Full audit's own Step 1 prompt (`ANALYSIS_STEPS[0]`, sent
            // directly to the agent, never persisted to disk) is already
            // self-sufficient — it doesn't depend on this block. Kronn still
            // strips the LEGACY marker from projects installed before this fix
            // (see `remove_bootstrap_block`, called at Full-audit Phase 2).
            runner::fix_file_ownership(&project_path);

            Ok(())
        })
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = install_result {
        return Json(ApiResponse::err(e));
    }

    // Ensure the docs/var/ scratch dir is gitignored (legacy ai/var/ path
    // stays gitignored too on projects that haven't migrated yet).
    let project_root = scanner::resolve_host_path(&project.path);
    if let Some(docs_rel) =
        crate::core::docs_location::resolution(&project_root).relative(&project_root)
    {
        crate::core::mcp_scanner::ensure_local_exclude(&project.path, &format!("{docs_rel}/var/"));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// Resolve the templates directory (Docker mount or local)
pub(crate) fn resolve_templates_dir() -> std::path::PathBuf {
    if let Ok(dir) = crate::core::child_env::var("KRONN_TEMPLATES_DIR") {
        return std::path::PathBuf::from(dir);
    }
    // Docker default
    let docker_path = std::path::PathBuf::from("/app/templates");
    if docker_path.exists() {
        return docker_path;
    }
    // Local dev: resolve relative to the BINARY, not the CWD — a backend
    // started from anywhere else silently lost every template install.
    // target/{debug,release}/kronn → repo root is two levels up.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(candidate) = templates_dir_near_executable(&exe) {
            return candidate;
        }
    }
    // Last resort: CWD-relative (pre-existing behaviour).
    std::path::PathBuf::from("templates")
}

/// Locate repository templates from binaries nested under Cargo's regular or
/// instrumented target trees. Coverage inserts an extra
/// `target/llvm-cov-target/` layer, so a fixed four-parent search stopped one
/// directory before the repository root.
fn templates_dir_near_executable(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    exe.ancestors().skip(1).take(8).find_map(|ancestor| {
        let candidate = ancestor.join("templates");
        candidate.is_dir().then_some(candidate)
    })
}

/// 0.7.1 — ensure the agent-writable `docs/` subfolders exist with a
/// minimal README explaining what each is for. Idempotent: only creates
/// folders/READMEs that are missing; never overwrites human-curated content.
///
/// The pattern : `docs/` itself has the curated entry (`AGENTS.md` +
/// architecture/operations/etc.). Agents write their RUNTIME-discovered
/// facts to one of these targeted subfolders, keeping the curated areas
/// pristine.
pub(crate) fn ensure_agent_writable_subfolders(
    project_root: &std::path::Path,
    docs_dir: &std::path::Path,
) {
    if !docs_dir.exists() {
        // Caller already failed to create docs/ — nothing to do here, the
        // bootstrap is in a degraded state.
        return;
    }
    // KT-819 DoD 11 — the audit seeds these folders from what the team stated
    // in the briefing (`api::audit::operating_seed`); they stay empty only when
    // the briefing gave nothing to seed, and agents and humans add to them.
    // The trust root is the EXPLICIT project root, never inferred (same
    // class as ensure_docs_index): the lstat walk never checks the root
    // itself, so rooting at docs_dir would let a symlinked docs/ route
    // these writes outside the project.
    for (name, readme) in SUBFOLDER_READMES {
        let folder = docs_dir.join(name);
        // no-follow: a symlinked subfolder (conventions/gotchas/people →
        // external dir) or dangling README link is skipped, never traversed.
        if std::fs::symlink_metadata(&folder)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            tracing::warn!(
                "{} is a symlink — skipping subfolder scaffold",
                folder.display()
            );
            continue;
        }
        if let Err(e) = crate::core::fs_guard::guarded_write_new(
            project_root,
            &folder.join("README.md"),
            readme.as_bytes(),
        ) {
            tracing::warn!("subfolder README not installed: {e}");
        }
    }
}

/// The README each agent-writable subfolder starts with. Shipped as
/// `templates/docs/<folder>/README.md` too: a test keeps both copies equal.
pub(crate) const SUBFOLDER_READMES: &[(&str, &str)] = &[
    (
        "conventions",
        "# Conventions\n\n\
         > **Seeded by the audit** from the Git workflow the team stated in the briefing (`git-workflow.md`), when it did; empty otherwise. It then fills up one file at a time, as agents and humans meet project-specific conventions while doing real work.\n\n\
         ## What goes here\n\
         Project-specific conventions — naming patterns, idioms agents must follow, micro-rules that don't fit `coding-rules.md` (one level up).\n\n\
         One file per topic, e.g. `pnpm-vs-npm.md`, `git-signoff.md`, `commit-style.md`.\n\n\
         Curated by humans + agents. Cross-ref with `[[wikilinks]]` for Obsidian graph.\n",
    ),
    (
        "gotchas",
        "# Gotchas\n\n\
         > **Seeded by the audit** from the traps the team listed in the briefing (`known-traps.md`), when it did; empty otherwise. It then fills up as agents (and humans) hit footguns during real work.\n\n\
         ## What goes here\n\
         Sharp edges to know before touching code — runtime quirks, footguns, version-specific issues, MCP integration weirdness, framework bugs.\n\n\
         One file per topic, e.g. `jira-mcp-escapes-panels.md`, `migration-locks-table.md`.\n",
    ),
    (
        "people",
        "# People\n\n\
         > **Seeded by the audit** from the roles, names and contacts the team entered in the briefing (`team.md`), when it did; empty otherwise. Skip the folder if your project is solo.\n\n\
         ## What goes here\n\
         Team context — preferences, ownership areas, review style. Helps agents adapt to the human they're collaborating with.\n\n\
         One file per person if useful: `alice.md`, `bob.md`.\n",
    ),
];

/// Recursively copy a directory, creating ONLY files that do not exist
/// at the destination, and returning the exact list of paths created by
/// THIS call — the only set downstream steps (placeholder prefill) are
/// allowed to touch. Existing files are never inspected or rewritten:
/// a 0-byte or placeholder-bearing destination is user state until a
/// manifest proves otherwise (Codex A2).
#[cfg(test)]
pub(crate) fn copy_dir_nondestructive(
    src: &std::path::Path,
    dst: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, String> {
    copy_dir_nondestructive_in(
        src,
        dst,
        &crate::core::docs_location::DocsRoot::default_root(),
    )
}

/// `copy_dir_nondestructive` with the markdown templates' `docs/` references
/// rewritten to `docs_root` (KT-1192); byte copies under the default root.
pub(crate) fn copy_dir_nondestructive_in(
    src: &std::path::Path,
    dst: &std::path::Path,
    docs_root: &crate::core::docs_location::DocsRoot,
) -> Result<Vec<std::path::PathBuf>, String> {
    // The destination root itself must not be a symlink: `exists()` and
    // `create_dir_all` follow links, so a symlinked docs/ would route the
    // whole install into a directory outside the ownership boundary.
    if let Ok(meta) = std::fs::symlink_metadata(dst) {
        if meta.file_type().is_symlink() {
            return Err(format!(
                "{} is a symlink — refusing to install templates through it",
                dst.display()
            ));
        }
    }
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {}", dst.display(), e))?;

    let entries =
        std::fs::read_dir(src).map_err(|e| format!("read_dir {}: {}", src.display(), e))?;

    let mut created = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("entry: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        // lstat, not stat: a symlink destination (dir, file or DANGLING —
        // where `exists()` reports false) is a pre-existing entry pointing
        // wherever the user chose. Never recurse through it, never copy
        // over it (Codex A2 symlink hardening).
        let dst_lstat = std::fs::symlink_metadata(&dst_path);
        if dst_lstat
            .as_ref()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            continue;
        }

        if src_path.is_dir() {
            created.extend(copy_dir_nondestructive_in(&src_path, &dst_path, docs_root)?);
        } else if dst_lstat.is_err() {
            // An existing destination is NEVER touched (Codex A2) — not
            // even 0 B or placeholder-bearing: neither proves Kronn owns
            // the current content, and recovery of a corrupt skeleton is
            // an explicit operation, not an implicit side effect.
            let markdown = src_path.extension().is_some_and(|ext| ext == "md");
            let copied = if docs_root.is_default() || !markdown {
                std::fs::copy(&src_path, &dst_path).map(|_| ())
            } else {
                std::fs::read_to_string(&src_path).and_then(|text| {
                    std::fs::write(&dst_path, docs_root.relocate(&text).as_bytes())
                })
            };
            copied.map_err(|e| {
                format!(
                    "copy {} -> {}: {}",
                    src_path.display(),
                    dst_path.display(),
                    e
                )
            })?;
            created.push(dst_path);
        }
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_lookup_reaches_repo_root_from_llvm_cov_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let templates = tmp.path().join("templates");
        std::fs::create_dir(&templates).unwrap();
        let executable = tmp
            .path()
            .join("target/llvm-cov-target/debug/deps/kronn-tests");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();

        assert_eq!(templates_dir_near_executable(&executable), Some(templates),);
    }

    #[test]
    fn ensure_subfolders_creates_three_folders_with_readme() {
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir(&docs).unwrap();
        ensure_agent_writable_subfolders(tmp.path(), &docs);
        for sub in &["conventions", "gotchas", "people"] {
            let path = docs.join(sub);
            assert!(path.is_dir(), "{} folder not created", sub);
            let readme = path.join("README.md");
            assert!(readme.is_file(), "{}/README.md not created", sub);
        }
    }

    #[test]
    fn subfolder_readmes_say_what_seeds_them_and_match_the_shipped_template() {
        // KT-819 DoD 11 — the audit seeds these folders, so the README says
        // what from and that an empty folder means the briefing was silent.
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir(&docs).unwrap();
        ensure_agent_writable_subfolders(tmp.path(), &docs);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
        for (sub, _) in SUBFOLDER_READMES {
            let body = std::fs::read_to_string(docs.join(sub).join("README.md")).unwrap();
            assert!(
                body.contains("Seeded by the audit") && body.contains("empty otherwise"),
                "{sub}/README.md must say what seeds the folder (got: {body:?})"
            );
            assert!(!body.contains("Empty by design"), "{sub}: {body:?}");
            let shipped = std::fs::read_to_string(root.join(sub).join("README.md"))
                .unwrap_or_else(|e| panic!("templates/docs/{sub}/README.md: {e}"));
            assert_eq!(shipped, body, "{sub}: template and installer copies differ");
        }
    }

    #[test]
    fn ensure_subfolders_is_idempotent_does_not_overwrite_existing_readme() {
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir(&docs).unwrap();
        std::fs::create_dir_all(docs.join("conventions")).unwrap();
        let custom = "# My custom conventions README\nSome content here.";
        std::fs::write(docs.join("conventions/README.md"), custom).unwrap();
        ensure_agent_writable_subfolders(tmp.path(), &docs);
        let after = std::fs::read_to_string(docs.join("conventions/README.md")).unwrap();
        assert_eq!(after, custom, "must not overwrite existing README");
    }

    #[test]
    fn ensure_subfolders_noop_when_docs_dir_missing() {
        // Bootstrap in a degraded state — caller failed to create docs/
        // but we shouldn't crash here.
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("does_not_exist");
        ensure_agent_writable_subfolders(tmp.path(), &docs);
        assert!(!docs.exists());
    }

    #[cfg(unix)]
    #[test]
    fn subfolder_scaffold_refuses_a_symlinked_docs_dir() {
        // Copilot r4 — same class as ensure_docs_index: docs/ itself being
        // a symlink must not route the scaffold READMEs outside the root.
        let tmp = tempfile::TempDir::new().unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let docs = project.join("docs");
        std::os::unix::fs::symlink(&external, &docs).unwrap();

        ensure_agent_writable_subfolders(&project, &docs);

        assert!(
            std::fs::read_dir(&external).unwrap().next().is_none(),
            "no scaffold may land through a symlinked docs/"
        );
        assert!(
            std::fs::symlink_metadata(&docs)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link itself stays intact"
        );
    }

    // ─── copy_dir_nondestructive: non-overwrite / ownership-boundary suite
    //
    // Codex A2 — `copy_dir_nondestructive` is CREATE-ONLY: it seeds files
    // that don't exist and never touches an existing one, whatever its
    // size or content. Neither 0 bytes nor {{UPPER_SNAKE}} placeholders
    // prove Kronn ownership (both can be deliberate user state), so the
    // historical "re-copy corrupt dest" heuristic was removed; a stub
    // left by a crashed step is caught by the step VALIDATOR (which fails
    // the step) — never silently rewritten here. These tests pin that
    // ownership boundary.

    fn write(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn copy_nondestructive_creates_missing_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("a.md"), &"x".repeat(500));
        copy_dir_nondestructive(&src, &dst).unwrap();
        assert!(dst.join("a.md").is_file());
    }

    #[test]
    fn copy_nondestructive_skips_healthy_existing_file() {
        // The user has edited the doc — must NOT be overwritten.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("a.md"), &"x".repeat(500));
        let user_content = "# My fresh content".repeat(50); // healthy size, distinct from template
        write(&dst.join("a.md"), &user_content);
        copy_dir_nondestructive(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("a.md")).unwrap(),
            user_content
        );
    }

    #[test]
    fn copy_nondestructive_leaves_an_existing_empty_file_intact() {
        // Codex A2 — a 0-byte file is user STATE (maybe intentionally
        // reserved), not proof of Kronn ownership: never re-seed it.
        // Corrupt-skeleton recovery is a future explicit, manifest-backed
        // operation.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        let template = "# Tech debt index\n".to_string() + &"placeholder ".repeat(50);
        write(&src.join("inconsistencies-tech-debt.md"), &template);
        write(&dst.join("inconsistencies-tech-debt.md"), "");
        let created = copy_dir_nondestructive(&src, &dst).unwrap();
        let after = std::fs::read_to_string(dst.join("inconsistencies-tech-debt.md")).unwrap();
        assert_eq!(after, "", "an existing 0-byte file stays byte-intact");
        assert!(created.is_empty(), "nothing was created — the file existed");
    }

    #[test]
    fn copy_nondestructive_leaves_intentional_placeholders_intact() {
        // Codex A2 — {{UPPER_SNAKE}} does NOT prove Kronn ownership: a user
        // template with intentional placeholders is indistinguishable from
        // our skeleton by that signal, so it must stay byte-intact.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("a.md"), &"x".repeat(1000));
        write(&dst.join("a.md"), "# User template\n| {{DECISION_1}} |");
        let created = copy_dir_nondestructive(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("a.md")).unwrap(),
            "# User template\n| {{DECISION_1}} |",
            "intentional user placeholders must survive byte-intact"
        );
        assert!(created.is_empty());
    }

    #[test]
    fn copy_nondestructive_never_clobbers_a_short_user_file() {
        // Codex A2 — the old <25% heuristic destroyed legitimately short
        // user files. Without an ownership marker, the file is sacred.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("a.md"), &"x".repeat(1000));
        write(&dst.join("a.md"), "# My own short glossary\nterm: def"); // user content, 3% of template
        copy_dir_nondestructive(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("a.md")).unwrap(),
            "# My own short glossary\nterm: def",
            "a short file without Kronn markers must stay byte-intact"
        );
    }

    #[test]
    fn copy_nondestructive_does_not_repair_small_template_files() {
        // Source < 200 B (e.g. a tiny README) — the heuristic is
        // disabled to avoid touching legitimately-small user files.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("README.md"), "# Short");
        write(&dst.join("README.md"), "");
        copy_dir_nondestructive(&src, &dst).unwrap();
        // Dest stays empty because src is < 200 B → repair skipped.
        assert_eq!(std::fs::read_to_string(dst.join("README.md")).unwrap(), "");
    }

    #[test]
    fn copy_nondestructive_preserves_dest_just_above_threshold() {
        // 25% threshold: just above must be preserved (the user
        // legitimately deleted 70% of the template — that's their
        // call, not ours).
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("a.md"), &"x".repeat(1000));
        let user_kept = "y".repeat(300); // 300 B = 30% of 1000 → above threshold
        write(&dst.join("a.md"), &user_kept);
        copy_dir_nondestructive(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("a.md")).unwrap(),
            user_kept
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial_test::serial(kronn_templates_env)]
    async fn fresh_docs_agents_md_never_carries_a_bootstrap_instruction() {
        // KT-841 — a "FIRST-RUN TASK" block used to be injected into
        // docs/AGENTS.md at install time; an interrupted first run left it in
        // the file every later agent reads. Exercised through the install
        // route itself, so a re-injection anywhere in it fails here.
        struct RestoreTemplatesEnv(Option<std::ffi::OsString>);
        impl Drop for RestoreTemplatesEnv {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => crate::core::child_env::set_var("KRONN_TEMPLATES_DIR", value),
                    None => crate::core::child_env::remove_var("KRONN_TEMPLATES_DIR"),
                }
            }
        }
        let _restore = RestoreTemplatesEnv(crate::core::child_env::var_os("KRONN_TEMPLATES_DIR"));
        // This checkout's own templates, whatever another test left in the env.
        crate::core::child_env::set_var(
            "KRONN_TEMPLATES_DIR",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates"),
        );
        let tmp = tempfile::TempDir::new().unwrap();
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
        let path = tmp.path().to_string_lossy().to_string();
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "INSERT INTO projects (id, name, path, created_at, updated_at)
                     VALUES ('p1', 'Projet été', ?1, datetime('now'), datetime('now'))",
                    [path],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let response = install_template(
            axum::extract::State(state),
            axum::extract::Path("p1".to_string()),
        )
        .await
        .0;
        assert!(response.success, "{:?}", response.error);
        let body = std::fs::read_to_string(tmp.path().join("docs/AGENTS.md"))
            .expect("the install wrote docs/AGENTS.md");
        assert!(!body.contains("FIRST-RUN TASK"), "{body}");
        assert!(!body.contains("KRONN:BOOTSTRAP:START"), "{body}");
        assert!(!body.contains("KRONN:BOOTSTRAP:END"), "{body}");
    }

    #[test]
    fn install_never_rewrites_a_preexisting_user_entry() {
        // A pre-existing user AGENTS.md must stay byte-intact through
        // install — `copy_dir_nondestructive` never overwrites an existing
        // destination file.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("AGENTS.md"), &"template ".repeat(50));
        let user = "# My own agents file\nhands off\n";
        write(&dst.join("AGENTS.md"), user);
        copy_dir_nondestructive(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("AGENTS.md")).unwrap(),
            user,
            "a pre-existing user entry file must stay byte-intact through install"
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_orchestration_writes_zero_bytes_outside_a_boobytrapped_project() {
        // Codex A2 end-to-end — the FULL install sequence (copy, anti-hallu
        // spec, subfolder scaffolds, docs index, redirectors, prefill) runs
        // against a project rigged with every known symlink trap. Nothing
        // may land outside the project root and every link must survive.
        let tmp = tempfile::TempDir::new().unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        let template_dir = tmp.path().join("templates");
        write(
            &template_dir.join("docs/AGENTS.md"),
            &"template ".repeat(50),
        );
        write(
            &template_dir.join("docs/architecture/overview.md"),
            &"o".repeat(300),
        );
        write(&template_dir.join("CLAUDE.md"), &"redirect ".repeat(40));
        write(
            &template_dir.join(".github/copilot-instructions.md"),
            &"copilot ".repeat(40),
        );

        let project = tmp.path().join("project");
        let docs = project.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        // Trap 1: conventions/ symlinked outside.
        std::os::unix::fs::symlink(&external, docs.join("conventions")).unwrap();
        // Trap 2: gotchas/ symlinked outside (subfolder scaffold path).
        std::os::unix::fs::symlink(&external, docs.join("gotchas")).unwrap();
        // Trap 3: .github symlinked outside (redirector parent).
        std::os::unix::fs::symlink(&external, project.join(".github")).unwrap();
        // Trap 4: docs/index.md dangling symlink.
        std::os::unix::fs::symlink(tmp.path().join("nowhere"), docs.join("index.md")).unwrap();
        // Trap 5: .gitignore dangling symlink (append path).
        std::os::unix::fs::symlink(tmp.path().join("nowhere-gi"), project.join(".gitignore"))
            .unwrap();

        // The exact install sequence, same order as the route.
        let mut created = copy_dir_nondestructive(&template_dir.join("docs"), &docs).unwrap();
        let _ = crate::core::fs_guard::guarded_write_new(
            &project,
            &docs.join("conventions/agents-md-format-v1.md"),
            crate::core::anti_halluc::SPEC_AGENTS_MD_V1.as_bytes(),
        );
        ensure_agent_writable_subfolders(&project, &docs);
        let _ = crate::core::docs_migration::ensure_docs_index(&project, &docs);
        for filename in &["CLAUDE.md", ".github/copilot-instructions.md"] {
            let src = template_dir.join(filename);
            let dst = project.join(filename);
            if let Ok(true) = crate::core::fs_guard::guarded_copy_new(&project, &src, &dst) {
                created.push(dst);
            }
        }
        let _ = crate::core::docs_migration::prefill_files(&project, &created);
        crate::core::mcp_scanner::ensure_local_exclude(project.to_str().unwrap(), "docs/var/");

        // ZERO bytes outside the root; every trap intact.
        assert!(
            std::fs::read_dir(&external).unwrap().next().is_none(),
            "the external directory must stay empty"
        );
        assert!(
            !tmp.path().join("nowhere").exists(),
            "nothing written through the dangling link"
        );
        assert!(
            !tmp.path().join("nowhere-gi").exists(),
            "gitignore append refused through the link"
        );
        for trap in [
            docs.join("conventions"),
            docs.join("gotchas"),
            project.join(".github"),
            docs.join("index.md"),
            project.join(".gitignore"),
        ] {
            assert!(
                std::fs::symlink_metadata(&trap)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "{} must survive as a symlink",
                trap.display()
            );
        }
        // And the legitimate installs DID land inside the project.
        assert!(docs.join("AGENTS.md").is_file());
        assert!(docs.join("architecture/overview.md").is_file());
        assert!(project.join("CLAUDE.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn copy_nondestructive_never_follows_a_symlinked_subdir() {
        // Codex A2 symlink hardening — a destination subdir that is a
        // symlink to an external directory must not be entered: the
        // external target stays empty and nothing is reported created.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        write(&src.join("architecture/overview.md"), &"x".repeat(300));
        std::fs::create_dir_all(&dst).unwrap();
        std::os::unix::fs::symlink(&external, dst.join("architecture")).unwrap();
        let created = copy_dir_nondestructive(&src, &dst).unwrap();
        assert!(
            created.is_empty(),
            "nothing may be created through the link"
        );
        assert!(
            std::fs::read_dir(&external).unwrap().next().is_none(),
            "the external target must stay empty"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_nondestructive_skips_a_dangling_symlink_file() {
        // A dangling symlink reports exists()=false but IS a pre-existing
        // user entry — copying would write through the link.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("glossary.md"), &"x".repeat(300));
        std::fs::create_dir_all(&dst).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("nowhere"), dst.join("glossary.md")).unwrap();
        let created = copy_dir_nondestructive(&src, &dst).unwrap();
        assert!(created.is_empty());
        let meta = std::fs::symlink_metadata(dst.join("glossary.md")).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "the dangling link must survive untouched"
        );
        assert!(
            !tmp.path().join("nowhere").exists(),
            "nothing written through the link"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_nondestructive_refuses_a_symlinked_destination_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        write(&src.join("a.md"), &"x".repeat(300));
        let dst = tmp.path().join("docs");
        std::os::unix::fs::symlink(&external, &dst).unwrap();
        let err = copy_dir_nondestructive(&src, &dst).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert!(std::fs::read_dir(&external).unwrap().next().is_none());
    }

    #[test]
    fn copy_nondestructive_recurses_into_subdirs() {
        // A MISSING nested file is created by the recursive walk (and
        // reported in the created list); an existing one stays intact.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("tech-debt/TEMPLATE.md"), &"x".repeat(500));
        write(&src.join("tech-debt/README.md"), &"y".repeat(300));
        write(&dst.join("tech-debt/README.md"), ""); // exists → untouched
        let created = copy_dir_nondestructive(&src, &dst).unwrap();
        let after = std::fs::read_to_string(dst.join("tech-debt/TEMPLATE.md")).unwrap();
        assert_eq!(after.len(), 500, "missing nested file gets created");
        assert_eq!(created, vec![dst.join("tech-debt/TEMPLATE.md")]);
        assert_eq!(
            std::fs::read_to_string(dst.join("tech-debt/README.md")).unwrap(),
            ""
        );
    }
}
