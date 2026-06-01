// Template installation: idempotent copy of `templates/docs/`,
// `templates/CLAUDE.md`, the root redirector files, and the bootstrap
// prompt injection at the top of the entry doc. The helpers
// (`resolve_templates_dir`, `copy_dir_nondestructive`,
// `ensure_agent_writable_subfolders`, `inject_bootstrap_prompt`) are
// `pub(crate)` because `api::audit` reuses them during the audit pipeline.

use axum::{extract::{Path, State}, Json};

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
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    // Run filesystem I/O on blocking thread pool
    let install_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        if !project_path.exists() {
            return Err(format!("Project path not found: {}", project_path.display()));
        }

        // Permission check on whichever doc-folder the project already
        // has (`docs/`, legacy `ai/`, or alt `doc/`). Fresh projects
        // skip this — there's nothing to check yet.
        let existing_docs = crate::core::scanner::detect_docs_dir(&project_path);
        if existing_docs.exists() {
            if let Err(e) = crate::api::audit::check_ai_dir_permissions(&existing_docs) {
                let folder_name = existing_docs.file_name().and_then(|n| n.to_str()).unwrap_or("docs");
                return Err(format!(
                    "{}/ directory exists but has permission issues that could not be fixed: {}. \
                     Run: sudo chown -R $(id -u):$(id -g) {}/{}/",
                    folder_name, e, project_path.display(), folder_name
                ));
            }
        }

        let template_dir = resolve_templates_dir();
        if !template_dir.exists() {
            return Err(format!("Templates directory not found: {}", template_dir.display()));
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
        if docs_template.is_dir() {
            copy_dir_nondestructive(&docs_template, &docs_target)?;
        }
        // 0.8.7 — copy the anti-hallu spec embedded in the binary into
        // the project's `docs/conventions/` so any agent running on this
        // project (with or without Kronn) can open the convention
        // locally. Idempotent : skip if already present (re-install
        // doesn't clobber user edits ; PR3 endpoint `/anti-hallu/inject`
        // is the explicit re-sync path).
        let conventions_dir = docs_target.join("conventions");
        let _ = std::fs::create_dir_all(&conventions_dir);
        let spec_path = conventions_dir.join("agents-md-format-v1.md");
        if !spec_path.exists() {
            let _ = std::fs::write(
                &spec_path,
                crate::core::anti_halluc::SPEC_AGENTS_MD_V1,
            );
        }
        ensure_agent_writable_subfolders(&docs_target)?;
        // Human-friendly landing page for `docs/`. Idempotent.
        let _ = crate::core::docs_migration::ensure_docs_index(&docs_target);

        // Pre-fill template placeholders with filesystem-derived defaults
        // (project name from dir, stack/test/lint cmds from package
        // managers, language defaults to English). Idempotent — only
        // replaces literal `{{TOKEN}}` substrings, never overwrites
        // filled content. Without this pass, fresh installs ship with
        // raw `{{PROJECT_NAME}}` cookie-cutter syntax visible to the
        // user until the agent's bootstrap step 1 fires, which doesn't
        // always happen on the first try.
        let _ = crate::core::docs_migration::prefill_template_placeholders(&project_path);

        for filename in &["CLAUDE.md", ".cursorrules", ".windsurfrules", ".clinerules"] {
            let src = template_dir.join(filename);
            let dst = project_path.join(filename);
            if src.exists() && !dst.exists() {
                if let Err(e) = std::fs::copy(&src, &dst) {
                    tracing::warn!("Failed to copy {}: {}", filename, e);
                }
            }
        }

        // Resolve the entry file via detect_docs_entry so this code path
        // works for fresh installs (docs/AGENTS.md), legacy projects
        // (ai/index.md), and projects on the `doc/` singular convention.
        let entry_file = crate::core::scanner::detect_docs_entry(&project_path);
        if entry_file.exists() {
            inject_bootstrap_prompt(&entry_file);
        }

        runner::fix_file_ownership(&project_path);

        Ok(())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = install_result {
        return Json(ApiResponse::err(e));
    }

    // Ensure the docs/var/ scratch dir is gitignored (legacy ai/var/ path
    // stays gitignored too on projects that haven't migrated yet).
    let docs_dir = crate::core::scanner::detect_docs_dir(&std::path::PathBuf::from(&project.path));
    let docs_dir_name = docs_dir.file_name().and_then(|n| n.to_str()).unwrap_or("docs");
    crate::core::mcp_scanner::ensure_gitignore_public(&project.path, &format!("{}/var/", docs_dir_name));

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
    // Local dev fallback: relative to binary
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
pub(crate) fn ensure_agent_writable_subfolders(docs_dir: &std::path::Path) -> Result<(), String> {
    if !docs_dir.exists() {
        // Caller already failed to create docs/ — nothing to do here, the
        // bootstrap is in a degraded state.
        return Ok(());
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
    for (name, readme) in SUBFOLDERS {
        let folder = docs_dir.join(name);
        if !folder.exists() {
            std::fs::create_dir_all(&folder)
                .map_err(|e| format!("mkdir {}: {}", folder.display(), e))?;
        }
        let readme_path = folder.join("README.md");
        if !readme_path.exists() {
            std::fs::write(&readme_path, readme)
                .map_err(|e| format!("write {}: {}", readme_path.display(), e))?;
        }
    }
    Ok(())
}

/// Recursively copy a directory, skipping files that already exist at
/// the destination — EXCEPT when the destination file is corrupted
/// (empty / truncated). Corruption almost always comes from a prior
/// audit that failed mid-write (timeout, CLI crash, sandbox abort)
/// and left a 0-byte file behind. Without this guard, the next audit
/// has nothing to fill in for that step — Step 9 (`inconsistencies-
/// tech-debt.md`) is the canonical victim because it's the longest
/// step and most likely to hit a CLI timeout.
///
/// Threshold heuristic: if dest is smaller than 25% of source AND
/// source is non-trivial (≥ 200 B), the dest is treated as corrupt
/// and re-copied from the template. The user's own content is never
/// at risk because the templates are static and the user's edits to
/// any post-audit file are still ≥ 25% of the template size in any
/// realistic scenario.
pub(crate) fn copy_dir_nondestructive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {}", dst.display(), e))?;

    let entries = std::fs::read_dir(src)
        .map_err(|e| format!("read_dir {}: {}", src.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("entry: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_nondestructive(&src_path, &dst_path)?;
        } else if !dst_path.exists() || is_corrupted_template_file(&src_path, &dst_path) {
            std::fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("copy {} -> {}: {}", src_path.display(), dst_path.display(), e))?;
        }
    }
    Ok(())
}

/// Heuristic to detect a destination file that is the leftover of a
/// failed prior audit (empty / truncated). The check is conservative:
/// we only consider files where the template source is non-trivial
/// (≥ 200 B) AND the dest is < 25% of source. Both conditions reduce
/// the false-positive rate on legitimate short user files.
fn is_corrupted_template_file(src: &std::path::Path, dst: &std::path::Path) -> bool {
    let (Ok(src_meta), Ok(dst_meta)) = (std::fs::metadata(src), std::fs::metadata(dst)) else {
        return false;
    };
    let src_size = src_meta.len();
    let dst_size = dst_meta.len();
    // Source must be a "real" template, not an empty placeholder we
    // accidentally ship — otherwise the heuristic flags everything.
    if src_size < 200 {
        return false;
    }
    // Dest is corrupted if it's empty OR < 25% of source. The "< 25%"
    // is generous: even a user who deleted half the template still
    // has ≥ 50%. A user who keeps only the title (e.g. 30 B) is
    // unusual — and re-copying the template is a safe operation
    // since the user is presumably starting fresh anyway.
    dst_size == 0 || dst_size * 4 < src_size
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
        ensure_agent_writable_subfolders(&docs).unwrap();
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
        ensure_agent_writable_subfolders(&docs).unwrap();
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
        ensure_agent_writable_subfolders(&docs).unwrap();
        let after = std::fs::read_to_string(docs.join("conventions/README.md")).unwrap();
        assert_eq!(after, custom, "must not overwrite existing README");
    }

    #[test]
    fn ensure_subfolders_noop_when_docs_dir_missing() {
        // Bootstrap in a degraded state — caller failed to create docs/
        // but we shouldn't crash here.
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("does_not_exist");
        assert!(ensure_agent_writable_subfolders(&docs).is_ok());
        assert!(!docs.exists());
    }

    // ─── copy_dir_nondestructive: corruption-repair regression suite
    //
    // 0.8.3 user bug on DOCROMS_WEB: a prior audit failed mid-Step-9
    // and left `inconsistencies-tech-debt.md` at 0 bytes. The next
    // audit's `copy_dir_nondestructive` saw the file existed and
    // skipped it — Step 9 then asked Claude to fill a totally blank
    // file with no template to inherit, and produced nothing.
    //
    // The repair heuristic re-copies a dest file ONLY when the
    // template src is ≥ 200 B AND dest is < 25% of src. These tests
    // pin the threshold + the "don't touch healthy dest" promise.

    fn write(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).unwrap(); }
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
        assert_eq!(std::fs::read_to_string(dst.join("a.md")).unwrap(), user_content);
    }

    #[test]
    fn copy_nondestructive_repairs_empty_dest() {
        // The exact DOCROMS_WEB scenario: prior audit left 0-byte file.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        let template = "# Tech debt index\n".to_string() + &"placeholder ".repeat(50);
        write(&src.join("inconsistencies-tech-debt.md"), &template);
        write(&dst.join("inconsistencies-tech-debt.md"), "");
        copy_dir_nondestructive(&src, &dst).unwrap();
        let after = std::fs::read_to_string(dst.join("inconsistencies-tech-debt.md")).unwrap();
        assert_eq!(after, template, "0-byte dest must be repaired from template");
    }

    #[test]
    fn copy_nondestructive_repairs_truncated_dest() {
        // Truncated mid-write (e.g. CLI crashed after writing the
        // first line) — must also be treated as corrupt.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        let template = "x".repeat(1000); // 1000 B
        write(&src.join("a.md"), &template);
        write(&dst.join("a.md"), "x"); // 1 B → < 25% of 1000
        copy_dir_nondestructive(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("a.md")).unwrap(), template);
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
        assert_eq!(std::fs::read_to_string(dst.join("a.md")).unwrap(), user_kept);
    }

    #[test]
    fn copy_nondestructive_recurses_into_subdirs() {
        // Corruption in a nested file (e.g. docs/tech-debt/TD-…)
        // must also be repaired by the recursive walk.
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("tech-debt/TEMPLATE.md"), &"x".repeat(500));
        write(&dst.join("tech-debt/TEMPLATE.md"), "");
        copy_dir_nondestructive(&src, &dst).unwrap();
        let after = std::fs::read_to_string(dst.join("tech-debt/TEMPLATE.md")).unwrap();
        assert_eq!(after.len(), 500);
    }
}
