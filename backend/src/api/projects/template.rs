// Template installation: idempotent copy of `templates/docs/`,
// `templates/CLAUDE.md`, the root redirector files, and the bootstrap
// prompt injection at the top of the entry doc. The helpers
// (`resolve_templates_dir`, `copy_dir_nondestructive`,
// `ensure_agent_writable_subfolders`, `inject_bootstrap_prompt`) are
// `pub(crate)` because `api::audit` reuses them during the audit pipeline.

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

    let project_path_str = project.path.clone();

    // Run filesystem I/O on blocking thread pool
    let install_result =
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let project_path = scanner::resolve_host_path(&project_path_str);
            if !project_path.exists() {
                return Err(format!(
                    "Project path not found: {}",
                    project_path.display()
                ));
            }

            // Permission check on whichever doc-folder the project already
            // has (`docs/`, legacy `ai/`, or alt `doc/`). Fresh projects
            // skip this — there's nothing to check yet.
            let existing_docs = crate::core::scanner::detect_docs_dir(&project_path);
            if existing_docs.exists() {
                if let Err(e) = crate::api::audit::check_ai_dir_permissions(&existing_docs) {
                    let folder_name = existing_docs
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("docs");
                    return Err(format!(
                    "{}/ directory exists but has permission issues that could not be fixed: {}. \
                     Run: sudo chown -R $(id -u):$(id -g) {}/{}/",
                    folder_name, e, project_path.display(), folder_name
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
            let docs_target = if existing_docs.exists() {
                existing_docs
            } else {
                project_path.join("docs")
            };
            let mut created_files = Vec::new();
            if docs_template.is_dir() {
                created_files.extend(copy_dir_nondestructive(&docs_template, &docs_target)?);
            }
            // 0.8.7 — copy the anti-hallu spec embedded in the binary into
            // the project's `docs/conventions/` so any agent running on this
            // project (with or without Kronn) can open the convention
            // locally. Idempotent : skip if already present (re-install
            // doesn't clobber user edits ; PR3 endpoint `/anti-hallu/inject`
            // is the explicit re-sync path).
            let conventions_dir = docs_target.join("conventions");
            let spec_path = conventions_dir.join("agents-md-format-v1.md");
            // no-follow primitive: a symlinked conventions/ (or dangling spec
            // link) must never route this write outside the project.
            if let Err(e) = crate::core::fs_guard::guarded_write_new(
                &project_path,
                &spec_path,
                crate::core::anti_halluc::SPEC_AGENTS_MD_V1.as_bytes(),
            ) {
                tracing::warn!("anti-hallu spec not installed: {e}");
            }
            ensure_agent_writable_subfolders(&project_path, &docs_target);
            // Human-friendly landing page for `docs/`. Idempotent, best-effort.
            if let Err(e) =
                crate::core::docs_migration::ensure_docs_index(&project_path, &docs_target)
            {
                tracing::warn!("docs index not installed: {e}");
            }

            // The FULL redirector set — every agent-context file the templates
            // ship (not just the 4 managed-block ones): a Gemini/Copilot user
            // got no entry point at all before this.
            for filename in &[
                "CLAUDE.md",
                ".cursorrules",
                ".windsurfrules",
                ".clinerules",
                "AGENTS.md",
                "GEMINI.md",
                ".github/copilot-instructions.md",
            ] {
                let src = template_dir.join(filename);
                let dst = project_path.join(filename);
                if src.exists() {
                    // no-follow: a symlinked .github/ (or dangling dst link)
                    // must never route the copy outside the project.
                    match crate::core::fs_guard::guarded_copy_new(&project_path, &src, &dst) {
                        Ok(true) => created_files.push(dst),
                        Ok(false) => {}
                        Err(e) => tracing::warn!("Failed to copy {}: {}", filename, e),
                    }
                }
            }

            // Pre-fill template placeholders with filesystem-derived defaults —
            // scoped EXCLUSIVELY to the files this very call created (ownership
            // by construction, Codex A2): a pre-existing docs/ tree or root file
            // is user content and is never walked.
            let _ = crate::core::docs_migration::prefill_files(&project_path, &created_files);

            // Resolve the entry file via detect_docs_entry so this code path
            // works for fresh installs (docs/AGENTS.md), legacy projects
            // (ai/index.md), and projects on the `doc/` singular convention.
            // The bootstrap prompt is only injected into an entry file THIS
            // call created (Codex A2) — a pre-existing user AGENTS.md is not
            // ours to rewrite, prompt block or not.
            let entry_file = crate::core::scanner::detect_docs_entry(&project_path);
            if entry_file.exists() && created_files.contains(&entry_file) {
                inject_bootstrap_prompt(&entry_file);
            }

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
    let docs_dir = crate::core::scanner::detect_docs_dir(&std::path::PathBuf::from(&project.path));
    let docs_dir_name = docs_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("docs");
    crate::core::mcp_scanner::ensure_gitignore_public(
        &project.path,
        &format!("{}/var/", docs_dir_name),
    );

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// Resolve the templates directory (Docker mount or local)
pub(crate) fn resolve_templates_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("KRONN_TEMPLATES_DIR") {
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
        for ancestor in exe.ancestors().skip(1).take(4) {
            let candidate = ancestor.join("templates");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    // Last resort: CWD-relative (pre-existing behaviour).
    std::path::PathBuf::from("templates")
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
    // The README copy below makes one point explicit: these folders
    // are EMPTY by default after the audit. The audit pipeline does
    // NOT seed them — they're discovery-time scratchpads filled by
    // agents (and humans) as they encounter conventions / gotchas /
    // contributors during real work. Without this notice, users
    // open the empty folders after their first audit and assume
    // something went wrong. 0.8.3 (F5).
    static SUBFOLDERS: &[(&str, &str)] = &[
        (
            "conventions",
            "# Conventions\n\n\
             > **Empty by design after the initial audit.** This folder fills up over time, one file at a time, as agents and humans encounter project-specific conventions while doing real work. The audit pipeline does NOT seed it — only `coding-rules.md` (one level up) is filled at audit time.\n\n\
             ## What goes here\n\
             Project-specific conventions discovered at runtime — naming patterns, idioms agents must follow, micro-rules that don't fit `coding-rules.md`.\n\n\
             One file per topic, e.g. `pnpm-vs-npm.md`, `git-signoff.md`, `commit-style.md`.\n\n\
             Curated by humans + agents. Cross-ref with `[[wikilinks]]` for Obsidian graph.\n",
        ),
        (
            "gotchas",
            "# Gotchas\n\n\
             > **Empty by design after the initial audit.** This folder fills up over time as agents (and humans) hit footguns during real work. The audit pipeline does NOT seed it — only generic `inconsistencies-tech-debt.md` (one level up) is filled at audit time.\n\n\
             ## What goes here\n\
             Sharp edges to know before touching code — runtime quirks, footguns, version-specific issues, MCP integration weirdness, framework bugs.\n\n\
             One file per topic, e.g. `jira-mcp-escapes-panels.md`, `migration-locks-table.md`.\n",
        ),
        (
            "people",
            "# People\n\n\
             > **Empty by design after the initial audit.** Optional folder. Skip if your project is solo or if collaborators don't need agent-tailored context.\n\n\
             ## What goes here\n\
             Team context — preferences, ownership areas, review style. Helps agents adapt to the human they're collaborating with.\n\n\
             One file per person if useful: `alice.md`, `bob.md`.\n",
        ),
    ];
    // The trust root is the EXPLICIT project root, never inferred (same
    // class as ensure_docs_index): the lstat walk never checks the root
    // itself, so rooting at docs_dir would let a symlinked docs/ route
    // these writes outside the project.
    for (name, readme) in SUBFOLDERS {
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

/// Recursively copy a directory, creating ONLY files that do not exist
/// at the destination, and returning the exact list of paths created by
/// THIS call — the only set downstream steps (placeholder prefill) are
/// allowed to touch. Existing files are never inspected or rewritten:
/// a 0-byte or placeholder-bearing destination is user state until a
/// manifest proves otherwise (Codex A2).
pub(crate) fn copy_dir_nondestructive(
    src: &std::path::Path,
    dst: &std::path::Path,
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
            created.extend(copy_dir_nondestructive(&src_path, &dst_path)?);
        } else if dst_lstat.is_err() {
            // An existing destination is NEVER touched (Codex A2) — not
            // even 0 B or placeholder-bearing: neither proves Kronn owns
            // the current content, and recovery of a corrupt skeleton is
            // an explicit operation, not an implicit side effect.
            std::fs::copy(&src_path, &dst_path).map_err(|e| {
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

/// Inject the bootstrap prompt at the top of the project's docs entry
/// file (`docs/AGENTS.md` post-pivot, legacy `ai/index.md`). Caller
/// passes the resolved entry path from `detect_docs_entry`.
pub(crate) fn inject_bootstrap_prompt(index_file: &std::path::Path) {
    let content = match std::fs::read_to_string(index_file) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Already has bootstrap prompt?
    if content.contains("KRONN:BOOTSTRAP:START") {
        return;
    }

    let prompt = r#"<!-- KRONN:BOOTSTRAP:START -->
<!-- ================================================================
     BOOTSTRAP INSTRUCTIONS — AUTO-GENERATED BY KRONN
     ================================================================
     This block is for AI agents only. It instructs you to analyze
     this repository and fill in the docs/ documentation skeleton.

     After completing the analysis, you MUST delete this entire block
     (from KRONN:BOOTSTRAP:START to KRONN:BOOTSTRAP:END).
     ================================================================ -->

> **FIRST-RUN TASK — Bootstrap docs/ documentation**
>
> This is a fresh `docs/` skeleton. You must analyze the repository and fill in all files.
>
> **Rules:**
> - All `docs/` files MUST be in **English**
> - Content is **AI + dev context** (factual, concise) — humans + agents both read these
> - Do NOT invent information — mark unknowns with `<!-- TODO: verify -->`
> - Replace ALL `{{PLACEHOLDERS}}` and `<!-- ... -->` comment placeholders with real content
> - Keep the existing file structure and headings — fill the blanks, do NOT rewrite from scratch
> - Cross-refs between docs files: prefer markdown links `[text](path.md)` over backtick paths (Obsidian-friendly graph view)
>
> **Steps (in order):**
>
> 1. **`docs/AGENTS.md`** — Analyze the project (README, configs, CI). Fill: project name, stack,
>    common tasks table, prerequisites, DO NOT rules, source of truth, code placement, stack table, date.
>
> 2. **`docs/glossary.md`** — Extract domain terms, abbreviations, internal names.
>    Organize by category (Architecture, Domain, Business, Third Parties). 30-60 terms.
>    Mark unknown terms with `<!-- TODO: ask user -->` for validation phase.
>
> 3. **`docs/repo-map.md`** — Map folder structure (2-3 levels), key files, entry points.
>
> 4. **`docs/coding-rules.md`** — One section per language. Linters, formatters, conventions, commands.
>
> 5. **`docs/testing-quality.md`** — Test frameworks, commands, CI gates, test file list, coverage.
>
> 6. **`docs/architecture/overview.md`** — Services table, key patterns, data flow, separation of concerns.
>
> 7. **`docs/operations/debug-operations.md`** — Common commands, Docker services, troubleshooting.
>
> 8. **`docs/inconsistencies-tech-debt.md`** — Scan source code across: dependencies (EOL/deprecated),
>    security (secrets, injection, auth), code quality (complexity, SRP, dead code), scalability (N+1, leaks),
>    maintainability (coupling, missing tests), compliance (GDPR, licenses), infrastructure (Docker, CI).
>    Create `docs/tech-debt/TD-*.md` detail files for each entry. Cite file paths.
>
> 9. **Review** — Check all files for consistency, completeness, no remaining placeholders.
>
> 10. **DELETE THIS ENTIRE BLOCK** (from `KRONN:BOOTSTRAP:START` to `KRONN:BOOTSTRAP:END`).
>
> 11. **Signal completion** — Write exactly `KRONN:BOOTSTRAP_COMPLETE` in your final message.
>
> When done, summarize: files filled, items needing human input, suggested deep-dives.

<!-- KRONN:BOOTSTRAP:END -->

"#;

    let new_content = format!("{}{}", prompt, content);
    if let Err(e) = std::fs::write(index_file, new_content) {
        tracing::warn!("Failed to inject bootstrap prompt: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn ensure_subfolders_readme_explicitly_says_empty_by_design() {
        // 0.8.3 F5 — users opened conventions/gotchas/people after
        // the first audit, saw only a tiny README, and assumed
        // something failed. The README now states upfront that these
        // folders are MEANT to be empty post-audit (they're filled
        // at runtime by agents and humans, not by the audit pipeline).
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir(&docs).unwrap();
        ensure_agent_writable_subfolders(tmp.path(), &docs);
        for sub in &["conventions", "gotchas", "people"] {
            let body = std::fs::read_to_string(docs.join(sub).join("README.md")).unwrap();
            assert!(
                body.contains("Empty by design"),
                "{sub}/README.md must explicitly call out the empty-by-design status (got: {body:?})"
            );
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

    #[test]
    fn install_never_rewrites_a_preexisting_user_entry() {
        // Codex A2 (pre-commit catch) — inject_bootstrap_prompt used to
        // rewrite whatever entry file detect_docs_entry found, including a
        // pre-existing user AGENTS.md. The install gate only injects into
        // an entry THIS call created; this exercises that exact decision.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("AGENTS.md"), &"template ".repeat(50));
        let user = "# My own agents file\nhands off\n";
        write(&dst.join("AGENTS.md"), user);
        let created = copy_dir_nondestructive(&src, &dst).unwrap();
        let entry = dst.join("AGENTS.md");
        if entry.exists() && created.contains(&entry) {
            inject_bootstrap_prompt(&entry);
        }
        assert_eq!(
            std::fs::read_to_string(&entry).unwrap(),
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
        crate::core::mcp_scanner::ensure_gitignore_public(project.to_str().unwrap(), "docs/var/");

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
