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
    static SUBFOLDERS: &[(&str, &str)] = &[
        (
            "conventions",
            "# Conventions\n\n\
             Project-specific conventions discovered by agents at runtime — code style, build, CI, naming patterns.\n\n\
             One file per topic, e.g. `pnpm-vs-npm.md`, `git-signoff.md`, `commit-style.md`.\n\n\
             Curated by humans + agents. Cross-ref with `[[wikilinks]]` for Obsidian graph.\n",
        ),
        (
            "gotchas",
            "# Gotchas\n\n\
             Sharp edges agents (and humans) should know before touching code — runtime quirks, footguns, version-specific issues.\n\n\
             One file per topic, e.g. `jira-mcp-escapes-panels.md`, `migration-locks-table.md`.\n",
        ),
        (
            "people",
            "# People\n\n\
             Optional team context — preferences, ownership, review style.\n\
             Helps agents adapt to the human they're collaborating with.\n\n\
             One file per person if useful: `alice.md`, `bob.md`. Skip if not relevant for your project.\n",
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

/// Recursively copy a directory, skipping files that already exist at the destination.
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
        } else if !dst_path.exists() {
            std::fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("copy {} -> {}: {}", src_path.display(), dst_path.display(), e))?;
        }
    }
    Ok(())
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
> 8. **`docs/operations/mcp-servers.md`** — MCP servers if .mcp.json exists.
>    Only create `docs/operations/mcp-servers/<slug>.md` if there are project-specific rules to document.
>
> 9. **`docs/inconsistencies-tech-debt.md`** — Scan source code across: dependencies (EOL/deprecated),
>    security (secrets, injection, auth), code quality (complexity, SRP, dead code), scalability (N+1, leaks),
>    maintainability (coupling, missing tests), compliance (GDPR, licenses), infrastructure (Docker, CI).
>    Create `docs/tech-debt/TD-*.md` detail files for each entry. Cite file paths.
>
> 10. **Review** — Check all files for consistency, completeness, no remaining placeholders.
>
> 11. **DELETE THIS ENTIRE BLOCK** (from `KRONN:BOOTSTRAP:START` to `KRONN:BOOTSTRAP:END`).
>
> 12. **Signal completion** — Write exactly `KRONN:BOOTSTRAP_COMPLETE` in your final message.
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
}
