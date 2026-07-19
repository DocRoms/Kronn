//! Migrate a Kronn-bootstrapped project from the legacy `ai/` convention
//! (Kronn ≤ 0.7.0) to the modern `docs/AGENTS.md` convention (0.7.1+).
//!
//! The migration is :
//! 1. `git mv ai docs` (preserves file history)
//! 2. `git mv docs/index.md docs/AGENTS.md` (entry-point rename)
//! 3. sed-pass on every `*.md` under `docs/` : replace internal path refs
//!    `ai/X` → `docs/X` (so cross-refs continue to resolve)
//! 4. Sed-pass on root redirectors (CLAUDE.md, AGENTS.md, GEMINI.md,
//!    .cursorrules, .clinerules, .windsurfrules, .vibe/, .kiro/, etc.) :
//!    `ai/index.md` → `docs/AGENTS.md`, `ai/X` → `docs/X`
//! 5. Optionally create a `ai` → `docs` symlink for 1-2 versions of
//!    rétro-compat (skipped on Windows; the operator can opt out).
//!
//! Idempotent : safe to re-run. If `docs/AGENTS.md` already exists,
//! returns early with `already_migrated`. If `ai/` doesn't exist,
//! returns `not_applicable`.
//!
//! No DB writes — purely filesystem. Caller (UI / API endpoint) is
//! expected to commit the result via the operator's normal git flow.

#[cfg(test)]
use std::path::PathBuf;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// `ai/` doesn't exist OR project never had a Kronn audit. No-op.
    NotApplicable,
    /// `docs/AGENTS.md` already exists — folder migration already
    /// happened. Carries the count of residual `ai/` refs we still
    /// cleaned in markdown / root redirectors when re-runs the rewrite
    /// pass over an already-migrated project (0.8.1: closes the gap
    /// where pre-0.8.1 migrations left stale `ai/...` refs inside
    /// already-moved `docs/` files). 0 when nothing to clean.
    AlreadyMigrated { refs_rewritten: usize },
    /// Migration succeeded. Caller can `git status` to inspect, commit
    /// when ready.
    Migrated {
        /// Number of files moved.
        files_moved: usize,
        /// Number of internal-path refs rewritten.
        refs_rewritten: usize,
        /// Whether we created a `ai` → `docs` rétro-compat symlink.
        symlink_created: bool,
    },
    /// Migration was attempted but a step failed. Field carries a
    /// human-readable message (also logged via tracing::error).
    Failed { reason: String },
}

/// Run the full migration pipeline on `project_path`.
///
/// `create_symlink` controls whether a `ai → docs` symlink is created
/// for rétro-compat after the move. Helpful when the operator can't
/// re-audit / re-bootstrap the project immediately and has external
/// scripts that hardcode `ai/` paths.
///
/// Returns the outcome — caller surfaces in UI / logs.
pub async fn migrate_project(
    project_path: &Path,
    create_symlink: bool,
) -> MigrationOutcome {
    if !project_path.is_dir() {
        return MigrationOutcome::Failed {
            reason: format!("Project path does not exist: {}", project_path.display()),
        };
    }

    // 1. Pre-flight checks.
    let ai_dir = project_path.join("ai");
    let docs_dir = project_path.join("docs");
    if docs_dir.join("AGENTS.md").is_file() {
        // Folder migration already done. 0.8.1: we still re-run the
        // ref-rewrite pass because pre-0.8.1 migrations could leave
        // stale `ai/...` references inside the already-moved `docs/`
        // files (user-reported on 2026-05-12). Both rewriters are
        // idempotent — calling them on a clean tree is a no-op that
        // reports `refs_rewritten = 0`. We don't touch `ai/` here:
        // if it lingers as a leftover, the operator can rm it after
        // verifying the rewrite landed.
        let mut refs_rewritten = rewrite_internal_refs(&docs_dir);
        refs_rewritten += rewrite_root_redirectors(project_path);
        return MigrationOutcome::AlreadyMigrated { refs_rewritten };
    }
    if !ai_dir.is_dir() {
        return MigrationOutcome::NotApplicable;
    }

    // 2. If `docs/` already exists with content, we can't `git mv ai docs`
    //    wholesale. Two sub-cases :
    //      a. No file would clash → merge file-by-file (preserves git
    //         history for `ai/*` files, leaves existing `docs/*` files
    //         alone). This is the common shape on projects that started
    //         a `docs/` for hand-written human docs before 0.7.1.
    //      b. At least one `ai/X` collides with an existing `docs/X` of
    //         different content → refuse with a precise list. Operator
    //         resolves the conflicts manually, then re-runs.
    let merge_needed = docs_dir.is_dir() && has_visible_files(&docs_dir);
    if merge_needed {
        let conflicts = find_merge_conflicts(&ai_dir, &docs_dir);
        if !conflicts.is_empty() {
            return MigrationOutcome::Failed {
                reason: format!(
                    "Cannot merge `ai/` into existing `docs/` — {} file(s) would collide \
                     with different content: {}. Manual merge required.",
                    conflicts.len(),
                    conflicts.join(", ")
                ),
            };
        }
    }

    // 3. Run the migration steps. Each step is idempotent enough to
    // recover from a partial earlier run (e.g. `git mv` failed but
    // `mv` worked — we re-detect at each step).
    let mut refs_rewritten = 0usize;

    let move_result = if merge_needed {
        merge_ai_into_docs(project_path).await
    } else {
        git_mv_ai_to_docs(project_path).await
    };
    if let Err(e) = move_result {
        return MigrationOutcome::Failed { reason: e };
    }
    let files_moved = count_md_files(&docs_dir);

    if let Err(e) = rename_index_to_agents(&docs_dir).await {
        return MigrationOutcome::Failed { reason: e };
    }

    // Generate a fresh `docs/index.md` (human-friendly hierarchy
    // overview) — the legacy `ai/index.md` was renamed to AGENTS.md
    // for the AI loader, but humans browsing the folder on GitHub
    // expect a plain README-shaped landing page.
    if let Err(e) = ensure_docs_index(project_path, &docs_dir) {
        // Non-fatal — log and continue. The migration result still
        // reports Migrated; the operator can `touch docs/index.md`
        // themselves if they care.
        tracing::warn!(target: "kronn::docs_migration",
            "Failed to write docs/index.md: {} — migration still succeeded", e);
    }

    refs_rewritten += rewrite_internal_refs(&docs_dir);
    refs_rewritten += rewrite_root_redirectors(project_path);

    // No placeholder prefill here (Codex A2): the migration MOVES the
    // user's own documents — any `{{TOKEN}}` inside them is user content,
    // not our skeleton. Prefill only ever runs on files a template
    // install just created (see `prefill_files`).

    let symlink_created = if create_symlink {
        create_ai_symlink(project_path).is_ok()
    } else {
        false
    };

    tracing::info!(
        target: "kronn::docs_migration",
        project = %project_path.display(),
        files_moved, refs_rewritten, symlink_created,
        "ai/ → docs/ migration succeeded"
    );

    MigrationOutcome::Migrated {
        files_moved,
        refs_rewritten,
        symlink_created,
    }
}

async fn git_mv_ai_to_docs(project_path: &Path) -> Result<(), String> {
    // Try `git mv` first (preserves history). Falls back to plain
    // `mv` if the project isn't a git repo OR `git mv` fails for some
    // reason (untracked content, etc.).
    let r = crate::core::cmd::async_cmd("git")
        .args(["mv", "ai", "docs"])
        .current_dir(project_path)
        .output()
        .await;
    if let Ok(out) = r {
        if out.status.success() {
            return Ok(());
        }
        tracing::warn!(
            "git mv ai docs failed: {} — falling back to fs::rename",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // Fallback : plain rename. History will be lost on the moved files
    // but the move still happens.
    std::fs::rename(project_path.join("ai"), project_path.join("docs"))
        .map_err(|e| format!("rename ai → docs failed: {}", e))
}

/// Walk every file under `ai/` (relative to `project_path`) and list the
/// ones whose `docs/<rel>` counterpart already exists with DIFFERENT
/// content. Identical content is treated as a no-conflict duplicate
/// (the merge step will simply discard the `ai/` copy, no data loss).
fn find_merge_conflicts(ai_dir: &Path, docs_dir: &Path) -> Vec<String> {
    let mut conflicts = Vec::new();
    walk_files(ai_dir, &mut |abs_path, rel_path| {
        let target = docs_dir.join(rel_path);
        if !target.exists() {
            return;
        }
        // Both files exist — only flag if their bytes differ. Identical
        // content is fine, the move will overwrite-with-same.
        let same = match (std::fs::read(abs_path), std::fs::read(&target)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if !same {
            conflicts.push(rel_path.to_string_lossy().to_string());
        }
    });
    conflicts.sort();
    conflicts
}

/// Per-file `git mv ai/<rel> docs/<rel>` for every file under `ai/`.
/// Falls back to `fs::rename` per file if `git mv` fails (untracked
/// content, .gitignore'd files, etc.). Removes the empty `ai/` dir
/// at the end. Caller must have run `find_merge_conflicts` first.
async fn merge_ai_into_docs(project_path: &Path) -> Result<(), String> {
    let ai_dir = project_path.join("ai");
    let docs_dir = project_path.join("docs");

    // Collect files first so we don't mutate the directory while walking.
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    walk_files(&ai_dir, &mut |_abs, rel| files.push(rel.to_path_buf()));

    for rel in files {
        let dest = docs_dir.join(&rel);
        if let Some(parent) = dest.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("mkdir {} failed: {}", parent.display(), e))?;
            }
        }
        let src_rel = std::path::PathBuf::from("ai").join(&rel);
        let dst_rel = std::path::PathBuf::from("docs").join(&rel);
        let src_str = src_rel.to_string_lossy();
        let dst_str = dst_rel.to_string_lossy();

        let r = crate::core::cmd::async_cmd("git")
            .args(["mv", "-f", &src_str, &dst_str])
            .current_dir(project_path)
            .output()
            .await;
        let git_ok = matches!(&r, Ok(o) if o.status.success());
        if !git_ok {
            // Fallback : plain rename. Loses git history on this one
            // file, but the merge still completes.
            std::fs::rename(ai_dir.join(&rel), &dest)
                .map_err(|e| format!("merge {} → {}: {}", src_rel.display(), dst_rel.display(), e))?;
        }
    }

    // 4. Clean up : remove the now-empty `ai/` tree.
    if let Err(e) = std::fs::remove_dir_all(&ai_dir) {
        // Not fatal — operator can `rm -rf ai/` themselves; we logged
        // the migration as success because the move part worked.
        tracing::warn!(target: "kronn::docs_migration",
            "Failed to clean up empty `ai/` after merge: {}", e);
    }
    Ok(())
}

/// Walk every file (any extension) recursively under `dir`. Skips the
/// usual VCS/build noise. Calls `cb(absolute_path, relative_path)`.
fn walk_files(dir: &Path, cb: &mut dyn FnMut(&Path, &Path)) {
    fn inner(root: &Path, current: &Path, cb: &mut dyn FnMut(&Path, &Path)) {
        let entries = match std::fs::read_dir(current) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(
                name.as_str(),
                ".git" | "node_modules" | "target" | "vendor" | ".kronn" | "dist" | "build"
            ) {
                continue;
            }
            if path.is_dir() {
                inner(root, &path, cb);
            } else if path.is_file() {
                if let Ok(rel) = path.strip_prefix(root) {
                    cb(&path, rel);
                }
            }
        }
    }
    inner(dir, dir, cb);
}

async fn rename_index_to_agents(docs_dir: &Path) -> Result<(), String> {
    let from = docs_dir.join("index.md");
    let to = docs_dir.join("AGENTS.md");
    if !from.is_file() {
        // Already renamed by a prior partial run, or never existed.
        return Ok(());
    }
    if to.is_file() {
        // Both exist — caller's preview flow should have caught this.
        // Safe path : leave both, log, continue.
        tracing::warn!(
            "Both docs/index.md and docs/AGENTS.md exist — keeping both, manual merge needed"
        );
        return Ok(());
    }
    let r = crate::core::cmd::async_cmd("git")
        .args(["mv", "docs/index.md", "docs/AGENTS.md"])
        .current_dir(docs_dir.parent().unwrap_or(docs_dir))
        .output()
        .await;
    if let Ok(out) = r {
        if out.status.success() {
            return Ok(());
        }
    }
    std::fs::rename(&from, &to)
        .map_err(|e| format!("rename docs/index.md → AGENTS.md failed: {}", e))
}

/// Self-heal pass : if a project already has `docs/AGENTS.md` (or
/// `doc/AGENTS.md`) but no `index.md` next to it, generate one. Used
/// on read-paths (`enrich_audit_status`) so projects that migrated
/// BEFORE the index.md generation shipped automatically catch up
/// without the operator having to re-trigger anything.
///
/// Best-effort : a write failure is logged but never returned — the
/// caller path is just `GET /api/projects` enrichment.
pub fn backfill_docs_index(project_path: &Path) {
    for folder in ["docs", "doc"] {
        let dir = project_path.join(folder);
        if !dir.is_dir() {
            continue;
        }
        // Only backfill on the post-pivot layout. Legacy `ai/index.md`
        // is the LLM entry — different semantics, leave alone.
        if dir.join("AGENTS.md").is_file() && !dir.join("index.md").exists() {
            if let Err(e) = ensure_docs_index(project_path, &dir) {
                tracing::debug!(target: "kronn::docs_migration",
                    "backfill index.md skipped for {}: {}", dir.display(), e);
            }
            return;
        }
    }
}

/// Write a fresh `docs/index.md` if none exists. The contents are a
/// short, human-readable map of the docs/ tree — what each subfolder
/// is for, where AGENTS.md lives, how to extend the convention. It
/// exists FOR HUMANS browsing the folder on GitHub or in their IDE;
/// AI agents read `docs/AGENTS.md` instead.
///
/// Idempotent : skips silently if `docs/index.md` already exists, so
/// re-running the migration (or operators who hand-craft an index)
/// won't get clobbered.
pub(crate) fn ensure_docs_index(project_root: &Path, docs_dir: &Path) -> Result<(), String> {
    let index_path = docs_dir.join("index.md");
    let body = build_docs_index_body(docs_dir);
    // no-follow create-only: a pre-existing index (including a dangling
    // symlink) is left alone; nothing is ever written through a link.
    // The trust root is the EXPLICIT project root, never inferred: the
    // lstat walk starts below the root, so rooting at docs_dir would never
    // check docs_dir itself — a symlinked docs/ would route the write
    // outside the project. A guard refusal PROPAGATES — best-effort is the
    // callers' decision (warn vs debug), not a silent success channel.
    crate::core::fs_guard::guarded_write_new(project_root, &index_path, body.as_bytes())
        .map(|_| ())
}

fn build_docs_index_body(docs_dir: &Path) -> String {
    let mut out = String::new();
    out.push_str("# Project documentation\n\n");
    out.push_str(
        "This folder is the project's living knowledge base, shared by humans and AI agents alike.\n\n",
    );
    out.push_str("## Entry points\n\n");
    out.push_str(
        "- **[AGENTS.md](AGENTS.md)** — Tiered context loader read by Claude Code, Codex, Gemini, Vibe, Copilot, Kiro and any agent that follows the `AGENTS.md` convention. Start here if you're an LLM.\n",
    );
    out.push_str(
        "- **This file (`index.md`)** — Plain landing page for humans browsing the folder. Extend it with whatever helps onboarding.\n\n",
    );

    // Discover subfolders that exist and surface them with a one-line
    // hint per known role. Unknown folders are listed too — better to
    // mention than to hide.
    let known: &[(&str, &str)] = &[
        ("conventions", "Coding conventions, lint rules, naming choices."),
        ("gotchas", "Footguns, surprising behaviors, things to remember."),
        ("people", "Who does what, contact points, decision owners."),
        ("architecture", "High-level diagrams and component overviews."),
        ("operations", "Runbooks, on-call notes, deploy procedures."),
        ("tech-debt", "Known debts, planned removals, deprecation notes."),
        ("decisions", "Architecture Decision Records (ADRs)."),
        ("templates", "Skeletons used by tooling — do not edit ad-hoc."),
    ];

    let mut subfolders: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(docs_dir) {
        for e in entries.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            subfolders.push(name);
        }
    }
    subfolders.sort();

    if !subfolders.is_empty() {
        out.push_str("## Layout\n\n");
        for name in &subfolders {
            let hint = known
                .iter()
                .find_map(|(k, h)| (*k == name.as_str()).then_some(*h))
                .unwrap_or("Project-specific docs.");
            out.push_str(&format!("- **`{name}/`** — {hint}\n"));
        }
        out.push('\n');
    }

    out.push_str("## Adding to the docs\n\n");
    out.push_str("- Drop a new markdown file into the matching subfolder; update this `index.md` if you create a new top-level folder.\n");
    out.push_str("- Cross-link with relative markdown links so the graph stays navigable in Obsidian / GitHub.\n");
    out.push_str("- Keep AI-loaded files (anything `AGENTS.md` references) free of secrets — Kronn enforces this on agent writes.\n");

    out
}

/// Walk every `.md` file under `docs/` and rewrite `ai/...` path refs
/// to `docs/...`. Special-case : `ai/index.md` → `docs/AGENTS.md`.
fn rewrite_internal_refs(docs_dir: &Path) -> usize {
    let mut count = 0usize;
    walk_md_files(docs_dir, &mut |path| {
        let original = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let updated = rewrite_refs_in_text(&original);
        if updated != original && std::fs::write(path, updated).is_ok() {
            count += 1;
        }
    });
    count
}

/// Same operation on root redirectors. Walked separately because
/// they live OUTSIDE `docs/`.
fn rewrite_root_redirectors(project_path: &Path) -> usize {
    static REDIRECTORS: &[&str] = &[
        // README is the single most common home for the "AI agents MUST read
        // ai/index.md" pointer — if we skip it, the migration leaves a stale
        // `ai/index.md` reference that `rewrite_refs_in_text` would otherwise
        // correctly rewrite to `docs/AGENTS.md`.
        "README.md",
        "CLAUDE.md",
        "AGENTS.md",
        "GEMINI.md",
        ".cursorrules",
        ".windsurfrules",
        ".clinerules",
        ".kiro/steering/instructions.md",
        ".vibe/instructions.md",
        ".github/copilot-instructions.md",
        ".cursor/rules/repo-instructions.mdc",
        ".env.mcp.example",
    ];
    let mut count = 0usize;
    for &rel in REDIRECTORS {
        let path = project_path.join(rel);
        if !path.is_file() {
            continue;
        }
        let original = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let updated = rewrite_refs_in_text(&original);
        if updated != original && std::fs::write(&path, updated).is_ok() {
            count += 1;
        }
    }
    count
}

/// Pure-text path rewrite. Exposed for unit tests.
pub(crate) fn rewrite_refs_in_text(text: &str) -> String {
    let mut t = text.to_string();
    // Specific rename first (most specific).
    t = t.replace("ai/index.md", "docs/AGENTS.md");
    // Then any `ai/X.md` → `docs/X.md` where X is path-shaped. Use a
    // regex to avoid touching the literal word "ai" in prose ("the ai
    // directory was..." stays untouched).
    static RE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r"\bai/([\w/.\-]+)").unwrap()
    });
    t = RE.replace_all(&t, "docs/$1").to_string();
    t
}

fn create_ai_symlink(project_path: &Path) -> std::io::Result<()> {
    let ai_path = project_path.join("ai");
    if ai_path.exists() {
        return Ok(()); // already there (a leftover from a prior run, or live link)
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("docs", &ai_path)
    }
    #[cfg(not(unix))]
    {
        // Windows: skip silently. The operator can re-audit instead.
        Ok(())
    }
}

/// Recursively walk `dir`, calling `cb` on each `*.md` file (case
/// insensitive on the extension). Skips `.git/`, `node_modules/`, etc.
fn walk_md_files(dir: &Path, cb: &mut dyn FnMut(&Path)) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(
            name.as_str(),
            ".git" | "node_modules" | "target" | "vendor" | ".kronn" | "dist" | "build"
        ) {
            continue;
        }
        if path.is_dir() {
            walk_md_files(&path, cb);
        } else if path.is_file() && name.to_lowercase().ends_with(".md") {
            cb(&path);
        }
    }
}

fn count_md_files(dir: &Path) -> usize {
    let mut n = 0usize;
    walk_md_files(dir, &mut |_| n += 1);
    n
}

/// Pre-fill the obvious template placeholders (`{{PROJECT_NAME}}`,
/// `{{STACK_SUMMARY}}`, `{{TEST_CMD}}`, `{{LINT_CMD}}`,
/// `{{PROJECT_LANGUAGE}}`) on every `*.md` under `docs/` and the root
/// redirector files.
///
/// **Why**: the template files ship with `{{...}}` placeholders that the
/// agent's bootstrap pass is supposed to fill. For projects where the
/// bootstrap was never run (or skipped step 1), the raw placeholder
/// syntax leaks to users — confusing and unprofessional. This pre-fill
/// gives sensible defaults derived from the filesystem; the agent
/// refines them later when bootstrap runs.
///
/// **Idempotent**: only replaces literal `{{TOKEN}}` substrings; if the
/// placeholder is already filled, the regex doesn't match and we skip.
///
/// **Scope**: EXCLUSIVELY the `files` the caller just created in the
/// current operation — ownership by construction. There is deliberately
/// no walk-the-tree variant (Codex A2): a global walk rewrote tokens
/// inside pre-existing user documents.
///
/// Returns the number of files modified (for telemetry / UI feedback).
pub fn prefill_files(project_path: &Path, files: &[std::path::PathBuf]) -> usize {
    // Scoped by contract (Codex A2): ONLY the files the caller just
    // created may be rewritten — ownership is proven by construction.
    // There is deliberately no walk-the-tree variant anymore: a global
    // walk rewrote `{{TOKEN}}`s inside pre-existing user documents.
    if files.is_empty() {
        return 0;
    }
    let replacements = compute_replacements(project_path);
    if replacements.is_empty() {
        return 0;
    }
    let mut files_changed = 0usize;
    for path in files {
        // Defense in depth: even a wrong internal caller list may not make
        // us rewrite through a symlink or outside the project root.
        if crate::core::fs_guard::assert_contained_no_symlink(project_path, path).is_err() {
            tracing::warn!("prefill skipped non-contained path {}", path.display());
            continue;
        }
        if path.is_file() && rewrite_file_with_replacements(path, &replacements) {
            files_changed += 1;
        }
    }
    files_changed
}

/// Build the (placeholder → value) map from filesystem heuristics.
/// Only emits entries for placeholders we can answer with confidence;
/// unknown ones stay as `{{...}}` for the agent to fill during bootstrap.
fn compute_replacements(project_path: &Path) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();

    // PROJECT_NAME — directory basename, Title-Cased + dashes/underscores → spaces.
    if let Some(name) = project_path.file_name().and_then(|s| s.to_str()) {
        let pretty = name
            .replace(['-', '_'], " ")
            .split_whitespace()
            .map(|w| {
                let mut chars = w.chars();
                match chars.next() {
                    Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        if !pretty.is_empty() {
            out.push(("{{PROJECT_NAME}}", pretty));
        }
    }

    // Stack signals → drive STACK_SUMMARY, TEST_CMD, LINT_CMD.
    let stack = detect_stack_signals(project_path);
    if let Some(summary) = stack.summary() {
        out.push(("{{STACK_SUMMARY}}", summary));
    }
    if let Some(test_cmd) = stack.test_cmd() {
        out.push(("{{TEST_CMD}}", test_cmd));
    }
    if let Some(lint_cmd) = stack.lint_cmd() {
        out.push(("{{LINT_CMD}}", lint_cmd));
    }

    // PROJECT_LANGUAGE: detect from a README locale hint, default English.
    // We don't want to guess wrong here — the agent overrides easily.
    out.push(("{{PROJECT_LANGUAGE}}", "English".to_string()));

    out
}

/// Replace every occurrence of each placeholder with its value, write back
/// only if the content changed. Returns `true` on a real write.
fn rewrite_file_with_replacements(path: &Path, replacements: &[(&'static str, String)]) -> bool {
    let original = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut updated = original.clone();
    for (token, value) in replacements {
        if updated.contains(token) {
            updated = updated.replace(token, value);
        }
    }
    if updated == original {
        return false;
    }
    std::fs::write(path, updated).is_ok()
}

/// Compact view of which package managers / build tools are present in
/// the project. Drives `STACK_SUMMARY`, `TEST_CMD`, `LINT_CMD`.
#[derive(Default)]
struct StackSignals {
    rust: bool,
    typescript: bool,
    javascript: bool,
    python: bool,
    go: bool,
    php: bool,
    /// `pnpm` if `pnpm-lock.yaml`, `npm` if `package-lock.json`, etc.
    js_pm: Option<&'static str>,
}

impl StackSignals {
    /// "Rust + TypeScript + Python" style. None when no signal is found.
    fn summary(&self) -> Option<String> {
        let mut parts: Vec<&'static str> = Vec::new();
        if self.rust { parts.push("Rust"); }
        if self.typescript { parts.push("TypeScript"); }
        else if self.javascript { parts.push("JavaScript"); }
        if self.python { parts.push("Python"); }
        if self.go { parts.push("Go"); }
        if self.php { parts.push("PHP"); }
        if parts.is_empty() { None } else { Some(parts.join(" + ")) }
    }

    /// Best-effort `test` command(s) chained with `&&`. Conservative on
    /// purpose — we only emit commands we're confident the project
    /// supports given the lockfile / config evidence.
    fn test_cmd(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if self.rust { parts.push("cargo test".into()); }
        if let Some(pm) = self.js_pm {
            if self.typescript || self.javascript {
                parts.push(format!("{} test", pm));
            }
        }
        if self.python { parts.push("pytest".into()); }
        if self.go { parts.push("go test ./...".into()); }
        if self.php { parts.push("phpunit".into()); }
        if parts.is_empty() { None } else { Some(parts.join(" && ")) }
    }

    fn lint_cmd(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if self.rust { parts.push("cargo clippy --all-targets -- -D warnings".into()); }
        if let Some(pm) = self.js_pm {
            if self.typescript {
                parts.push(format!("{} tsc --noEmit && {} lint", pm, pm));
            } else if self.javascript {
                parts.push(format!("{} lint", pm));
            }
        }
        if self.python { parts.push("ruff check".into()); }
        if self.go { parts.push("go vet ./...".into()); }
        if self.php { parts.push("phpcs".into()); }
        if parts.is_empty() { None } else { Some(parts.join(" && ")) }
    }
}

fn detect_stack_signals(project_path: &Path) -> StackSignals {
    let mut s = StackSignals::default();
    // Recurse one level: many monorepos put `backend/Cargo.toml` and
    // `frontend/package.json`. Detecting at the root only would miss
    // them and leave the test/lint cmds blank.
    let candidates: Vec<std::path::PathBuf> = std::iter::once(project_path.to_path_buf())
        .chain(
            std::fs::read_dir(project_path)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.path())
                .filter(|p| {
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    !matches!(
                        name,
                        ".git" | "node_modules" | "target" | "vendor" | ".kronn"
                            | "dist" | "build" | "docs" | "doc" | "ai"
                    )
                })
        )
        .collect();
    for p in &candidates {
        if p.join("Cargo.toml").is_file() {
            s.rust = true;
        }
        if p.join("package.json").is_file() {
            if p.join("tsconfig.json").is_file() || p.join("tsconfig.app.json").is_file() {
                s.typescript = true;
            } else {
                s.javascript = true;
            }
            // Resolve a package manager once. Pin to the first one we see;
            // monorepos that mix pnpm + npm hit weirder problems than a
            // shaky test_cmd default.
            if s.js_pm.is_none() {
                if p.join("pnpm-lock.yaml").is_file() {
                    s.js_pm = Some("pnpm");
                } else if p.join("yarn.lock").is_file() {
                    s.js_pm = Some("yarn");
                } else if p.join("bun.lockb").is_file() {
                    s.js_pm = Some("bun");
                } else if p.join("package-lock.json").is_file() {
                    s.js_pm = Some("npm");
                } else {
                    s.js_pm = Some("npm"); // sensible default when no lockfile
                }
            }
        }
        if p.join("pyproject.toml").is_file()
            || p.join("requirements.txt").is_file()
            || p.join("setup.py").is_file()
        {
            s.python = true;
        }
        if p.join("go.mod").is_file() {
            s.go = true;
        }
        if p.join("composer.json").is_file() {
            s.php = true;
        }
    }
    s
}

fn has_visible_files(dir: &Path) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with('.') {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    /// Test-only stand-in for the removed global walk: enumerates the
    /// fixture's docs/*.md + root redirectors and prefills THAT list —
    /// the production API only ever receives freshly-created files.
    fn prefill_all_for_tests(root: &Path) -> usize {
        let mut files = Vec::new();
        let docs = crate::core::scanner::detect_docs_dir(root);
        if docs.is_dir() {
            walk_md_files(&docs, &mut |p| files.push(p.to_path_buf()));
        }
        for rel in [
            "README.md", "CLAUDE.md", "AGENTS.md", "GEMINI.md",
            ".cursorrules", ".windsurfrules", ".clinerules",
            ".kiro/steering/instructions.md", ".vibe/instructions.md",
            ".github/copilot-instructions.md", ".cursor/rules/repo-instructions.mdc",
        ] {
            let p = root.join(rel);
            if p.is_file() { files.push(p); }
        }
        prefill_files(root, &files)
    }

    use super::*;

    // ─── rewrite_refs_in_text (pure logic) ────────────────────────────

    #[test]
    fn rewrites_internal_md_path() {
        assert_eq!(
            rewrite_refs_in_text("See `ai/architecture/overview.md` for details."),
            "See `docs/architecture/overview.md` for details."
        );
    }

    #[test]
    fn rewrites_index_to_agents() {
        assert_eq!(
            rewrite_refs_in_text("Read [ai/index.md](ai/index.md) first."),
            "Read [docs/AGENTS.md](docs/AGENTS.md) first."
        );
    }

    #[test]
    fn rewrites_tech_debt_subfolder_path() {
        assert_eq!(
            rewrite_refs_in_text("Detail file: ai/tech-debt/TD-20260315-x.md"),
            "Detail file: docs/tech-debt/TD-20260315-x.md"
        );
    }

    #[test]
    fn does_not_touch_word_ai_in_prose() {
        // The word "ai" not followed by `/` should survive.
        let s = "AI agents read this file. The ai is your friend.";
        assert_eq!(rewrite_refs_in_text(s), s);
    }

    #[test]
    fn handles_multiple_occurrences_per_line() {
        let s = "Cross-ref: ai/foo.md and also ai/bar.md and even ai/sub/baz.md";
        assert_eq!(
            rewrite_refs_in_text(s),
            "Cross-ref: docs/foo.md and also docs/bar.md and even docs/sub/baz.md"
        );
    }

    #[test]
    fn handles_path_with_dots_and_dashes() {
        assert_eq!(
            rewrite_refs_in_text("File: ai/inconsistencies-tech-debt.md"),
            "File: docs/inconsistencies-tech-debt.md"
        );
    }

    // ─── End-to-end migration on a tempdir ───────────────────────────

    async fn make_legacy_project() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        // Init git so `git mv` works.
        let _ = crate::core::cmd::async_cmd("git").args(["init", "-q", "-b", "main"]).current_dir(&root).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["config", "user.email", "test@kronn.local"]).current_dir(&root).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["config", "user.name", "test"]).current_dir(&root).output().await.unwrap();
        // Build a minimal legacy ai/ tree.
        std::fs::create_dir_all(root.join("ai/architecture")).unwrap();
        std::fs::write(
            root.join("ai/index.md"),
            "# AI context\nRead `ai/architecture/overview.md` and ai/glossary.md.\n",
        ).unwrap();
        std::fs::write(root.join("ai/glossary.md"), "# Glossary\nSee ai/index.md.\n").unwrap();
        std::fs::write(
            root.join("ai/architecture/overview.md"),
            "# Architecture\nFolder structure: ai/repo-map.md.\n",
        ).unwrap();
        // Add a root redirector that references ai/.
        std::fs::write(root.join("CLAUDE.md"), "Read ai/index.md for context.\n").unwrap();
        // README is the most common home for the AI-entry pointer — it MUST be
        // rewritten too (regression: it was missing from REDIRECTORS).
        std::fs::write(
            root.join("README.md"),
            "# Project\n> For AI agents: you MUST read `ai/index.md` first.\n",
        ).unwrap();
        // Commit so git mv works on tracked files.
        let _ = crate::core::cmd::async_cmd("git").args(["add", "."]).current_dir(&root).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["commit", "-q", "-m", "init"]).current_dir(&root).output().await.unwrap();
        (tmp, root)
    }

    #[tokio::test]
    async fn end_to_end_migration_succeeds() {
        let (_tmp, root) = make_legacy_project().await;
        let outcome = migrate_project(&root, false).await;
        match outcome {
            MigrationOutcome::Migrated { files_moved, refs_rewritten, symlink_created } => {
                assert!(files_moved >= 3, "expected ≥ 3 files, got {}", files_moved);
                assert!(refs_rewritten >= 1, "expected ≥ 1 ref rewritten, got {}", refs_rewritten);
                assert!(!symlink_created, "no symlink requested");
            }
            other => panic!("expected Migrated, got {:?}", other),
        }
        // ai/ should be gone, docs/AGENTS.md should exist with rewritten refs.
        assert!(!root.join("ai").exists() || root.join("ai").is_symlink(), "ai/ should be removed");
        let agents = root.join("docs/AGENTS.md");
        assert!(agents.is_file(), "docs/AGENTS.md should exist");
        let agents_content = std::fs::read_to_string(&agents).unwrap();
        assert!(agents_content.contains("docs/architecture/overview.md"));
        assert!(agents_content.contains("docs/glossary.md"));
        assert!(!agents_content.contains("ai/architecture"));
        // Root redirector should be rewritten too.
        let claude = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert!(claude.contains("docs/AGENTS.md"));
        assert!(!claude.contains("ai/index.md"));
        // README — the AI-entry pointer must be remapped to the canonical
        // `docs/AGENTS.md`, NOT left at `ai/index.md` (regression guard).
        let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
        assert!(readme.contains("docs/AGENTS.md"), "README should point to docs/AGENTS.md, got: {readme}");
        assert!(!readme.contains("ai/index.md"), "README must not keep the stale ai/index.md pointer");
    }

    #[tokio::test]
    async fn already_migrated_returns_early_no_stale_refs() {
        // Clean already-migrated tree → AlreadyMigrated with 0 refs rewritten.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/AGENTS.md"), "# already there\nSee `docs/glossary.md`.").unwrap();
        std::fs::create_dir_all(root.join("ai")).unwrap();
        let outcome = migrate_project(root, false).await;
        assert_eq!(outcome, MigrationOutcome::AlreadyMigrated { refs_rewritten: 0 });
    }

    #[tokio::test]
    async fn already_migrated_cleans_stale_ai_refs() {
        // 0.8.1 fix: a project already at `docs/` layout but with
        // residual `ai/...` refs inside its markdown gets those refs
        // rewritten when the migration endpoint is re-triggered.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(
            root.join("docs/AGENTS.md"),
            "# Index\nSee `ai/glossary.md` for terms and `ai/repo-map.md` for layout.\n",
        ).unwrap();
        std::fs::write(
            root.join("docs/glossary.md"),
            "Refer back to `ai/AGENTS.md` for context.\n",
        ).unwrap();
        let outcome = migrate_project(root, false).await;
        match outcome {
            MigrationOutcome::AlreadyMigrated { refs_rewritten } => {
                // 2 files touched (AGENTS.md + glossary.md). The counter
                // is "files changed", not "individual refs rewritten",
                // mirroring how migrate-from-scratch counts.
                assert_eq!(refs_rewritten, 2, "expected 2 files cleaned, got {refs_rewritten}");
            }
            other => panic!("expected AlreadyMigrated, got {other:?}"),
        }
        // Verify the rewrite actually happened on disk.
        let agents = std::fs::read_to_string(root.join("docs/AGENTS.md")).unwrap();
        assert!(agents.contains("docs/glossary.md"), "ref not rewritten: {agents}");
        assert!(!agents.contains("ai/glossary.md"), "stale ref leaked: {agents}");
        let glossary = std::fs::read_to_string(root.join("docs/glossary.md")).unwrap();
        assert!(glossary.contains("docs/AGENTS.md"));
        assert!(!glossary.contains("ai/AGENTS.md"));
    }

    #[tokio::test]
    async fn not_applicable_when_no_ai_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outcome = migrate_project(tmp.path(), false).await;
        assert_eq!(outcome, MigrationOutcome::NotApplicable);
    }

    #[tokio::test]
    async fn merges_when_existing_docs_has_no_name_conflicts() {
        // Common shape: project started a `docs/` for human docs (e.g.
        // `docs/handbook.md`) before bootstrapping ai/. Migration should
        // merge `ai/*` into the existing `docs/` instead of refusing —
        // no file collides, no data loss.
        let (_tmp, root) = make_legacy_project().await;
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/handbook.md"), "# Handbook (human)").unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["add", "."]).current_dir(&root).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["commit", "-q", "-m", "human docs"]).current_dir(&root).output().await.unwrap();

        let outcome = migrate_project(&root, false).await;
        match outcome {
            MigrationOutcome::Migrated { files_moved, .. } => {
                // 3 ai/ files + the existing handbook.md.
                assert!(files_moved >= 4, "expected ≥ 4 files, got {}", files_moved);
            }
            other => panic!("expected Migrated, got {:?}", other),
        }
        // The ai/ tree is consumed, the human docs/handbook.md is still there,
        // and the agents entry-point exists.
        assert!(!root.join("ai").exists() || root.join("ai").is_symlink());
        assert!(root.join("docs/AGENTS.md").is_file());
        assert!(root.join("docs/handbook.md").is_file(),
            "human docs file must survive the merge");
    }

    #[tokio::test]
    async fn refuses_when_merge_would_collide_with_different_content() {
        // Same filename in both trees with different bytes → operator
        // must resolve manually. Migration refuses with a precise list.
        let (_tmp, root) = make_legacy_project().await;
        std::fs::create_dir_all(root.join("docs")).unwrap();
        // ai/glossary.md is "# Glossary\n..."; create a docs/glossary.md
        // with different content to trigger the conflict.
        std::fs::write(root.join("docs/glossary.md"), "# Different glossary\n").unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["add", "."]).current_dir(&root).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["commit", "-q", "-m", "conflict"]).current_dir(&root).output().await.unwrap();

        let outcome = migrate_project(&root, false).await;
        match outcome {
            MigrationOutcome::Failed { reason } => {
                assert!(reason.contains("glossary.md"),
                    "reason should name the conflicting file: {}", reason);
            }
            other => panic!("expected Failed, got {:?}", other),
        }
        // Nothing was moved on refusal — both trees are intact.
        assert!(root.join("ai/index.md").is_file());
        assert!(root.join("docs/glossary.md").is_file());
    }

    // ─── backfill_docs_index ──────────────────────────────────────────

    #[test]
    fn backfill_creates_index_when_agents_present_but_no_index() {
        // Mirrors the Kronn case: project was migrated to docs/AGENTS.md
        // BEFORE the index.md generation shipped, so docs/ has AGENTS.md
        // but no index.md. The next list-fetch should self-heal it.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/AGENTS.md"), "# tiered\n").unwrap();
        assert!(!root.join("docs/index.md").exists());

        backfill_docs_index(root);

        assert!(root.join("docs/index.md").is_file(),
            "self-heal should generate the missing index.md");
    }

    #[test]
    fn backfill_is_a_noop_on_legacy_ai_only_projects() {
        // Legacy projects have ai/index.md as the LLM entry. We must NOT
        // touch them — the operator is expected to migrate first.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("ai")).unwrap();
        std::fs::write(root.join("ai/index.md"), "# legacy LLM entry\n").unwrap();

        backfill_docs_index(root);

        // No `docs/` should appear.
        assert!(!root.join("docs").exists());
        assert!(!root.join("docs/index.md").exists());
    }

    #[test]
    fn backfill_preserves_hand_written_index() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/AGENTS.md"), "# tiered\n").unwrap();
        std::fs::write(root.join("docs/index.md"), "# my custom landing\n").unwrap();

        backfill_docs_index(root);

        // Hand-crafted content survives the self-heal.
        let body = std::fs::read_to_string(root.join("docs/index.md")).unwrap();
        assert_eq!(body, "# my custom landing\n");
    }

    #[test]
    fn backfill_supports_singular_doc_layout() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("doc")).unwrap();
        std::fs::write(root.join("doc/AGENTS.md"), "# tiered\n").unwrap();

        backfill_docs_index(root);

        assert!(root.join("doc/index.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn backfill_refuses_a_symlinked_docs_dir_even_when_probes_follow() {
        // Copilot r4 family — `is_dir()`/`is_file()` FOLLOW links, so the
        // backfill happily selects a symlinked docs/ pointing at an
        // external tree carrying AGENTS.md. The write sink must still
        // refuse: the guard walk (rooted at the explicit project root)
        // lstats the docs component.
        let tmp = tempfile::TempDir::new().unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(external.join("AGENTS.md"), "# agents").unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        std::os::unix::fs::symlink(&external, project.join("docs")).unwrap();

        backfill_docs_index(&project);

        assert!(
            !external.join("index.md").exists(),
            "the backfill must never write through a symlinked docs/"
        );
        assert!(
            std::fs::symlink_metadata(project.join("docs"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link itself stays intact"
        );
        assert_eq!(
            std::fs::read_to_string(external.join("AGENTS.md")).unwrap(),
            "# agents",
            "the external tree is byte-untouched"
        );
    }

    // ─── ensure_docs_index ────────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn ensure_docs_index_refuses_a_symlinked_docs_dir() {
        // Copilot r4 — the trust root is the EXPLICIT project root: a
        // docs/ that is itself a symlink must never route the index outside
        // the project (the lstat walk never checks the root itself).
        let tmp = tempfile::TempDir::new().unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let docs = project.join("docs");
        std::os::unix::fs::symlink(&external, &docs).unwrap();

        let err = ensure_docs_index(&project, &docs).unwrap_err();
        assert!(err.contains("symlink"), "{err}");

        assert!(
            !external.join("index.md").exists(),
            "nothing may be written through a symlinked docs/"
        );
        assert!(
            std::fs::symlink_metadata(&docs)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link itself stays intact"
        );
    }

    #[test]
    fn ensure_docs_index_writes_when_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().join("proj");
        let docs = project.join("docs");
        std::fs::create_dir_all(docs.join("conventions")).unwrap();
        std::fs::create_dir_all(docs.join("operations")).unwrap();

        ensure_docs_index(&project, &docs).unwrap();

        let body = std::fs::read_to_string(docs.join("index.md")).unwrap();
        assert!(body.contains("# Project documentation"));
        // Mentions AGENTS.md as the LLM entry point.
        assert!(body.contains("AGENTS.md"));
        // Lists the discovered subfolders with their canonical hint.
        assert!(body.contains("`conventions/`"));
        assert!(body.contains("`operations/`"));
    }

    #[test]
    fn ensure_docs_index_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().join("proj");
        let docs = project.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("index.md"), "# my hand-written index").unwrap();

        ensure_docs_index(&project, &docs).unwrap();

        // The hand-written body must survive a re-run.
        let body = std::fs::read_to_string(docs.join("index.md")).unwrap();
        assert_eq!(body, "# my hand-written index");
    }

    #[test]
    fn ensure_docs_index_handles_unknown_subfolders() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().join("proj");
        let docs = project.join("docs");
        std::fs::create_dir_all(docs.join("custom-stuff")).unwrap();

        ensure_docs_index(&project, &docs).unwrap();

        let body = std::fs::read_to_string(docs.join("index.md")).unwrap();
        assert!(body.contains("`custom-stuff/`"));
        assert!(body.contains("Project-specific docs"));
    }

    #[tokio::test]
    async fn migration_creates_docs_index_for_humans() {
        let (_tmp, root) = make_legacy_project().await;
        let outcome = migrate_project(&root, false).await;
        assert!(matches!(outcome, MigrationOutcome::Migrated { .. }));

        // Both AGENTS.md (LLM entry) and index.md (human entry) must
        // exist after the migration completes.
        assert!(root.join("docs/AGENTS.md").is_file(), "AGENTS.md missing post-migration");
        assert!(root.join("docs/index.md").is_file(), "index.md missing post-migration");
        let index = std::fs::read_to_string(root.join("docs/index.md")).unwrap();
        assert!(index.contains("AGENTS.md"));
    }

    #[tokio::test]
    async fn merges_when_duplicate_files_are_byte_identical() {
        // Edge case: same filename, same content. No conflict, the
        // merge step happily overwrites the destination with itself
        // (or git mv noops).
        let (_tmp, root) = make_legacy_project().await;
        std::fs::create_dir_all(root.join("docs")).unwrap();
        let dup = "# Glossary\nSee ai/index.md.\n";
        std::fs::write(root.join("docs/glossary.md"), dup).unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["add", "."]).current_dir(&root).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["commit", "-q", "-m", "dup"]).current_dir(&root).output().await.unwrap();

        let outcome = migrate_project(&root, false).await;
        assert!(matches!(outcome, MigrationOutcome::Migrated { .. }));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn create_symlink_flag_creates_ai_link() {
        let (_tmp, root) = make_legacy_project().await;
        let outcome = migrate_project(&root, true).await;
        match outcome {
            MigrationOutcome::Migrated { symlink_created, .. } => {
                assert!(symlink_created, "expected symlink to be created");
            }
            other => panic!("expected Migrated, got {:?}", other),
        }
        let ai = root.join("ai");
        assert!(ai.is_symlink() || ai.exists(), "ai → docs symlink should exist");
    }

    // ─── prefill_template_placeholders ────────────────────────────────

    /// Drops a minimal Rust-project skeleton (Cargo.toml + a docs/
    /// AGENTS.md riddled with the 5 placeholders we auto-fill).
    fn write_rust_with_placeholders() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::write(root.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(
            root.join("docs/AGENTS.md"),
            "# {{PROJECT_NAME}} — {{STACK_SUMMARY}}\n\
             Working language: {{PROJECT_LANGUAGE}}\n\
             Test: {{TEST_CMD}}\n\
             Lint: {{LINT_CMD}}\n\
             ## Rules\n- {{DO_NOT_1}}\n- {{DO_NOT_2}}\n",
        ).unwrap();
        (tmp, root)
    }

    #[test]
    fn prefill_replaces_the_five_known_placeholders() {
        // Project named `amp-easy-backo` with a Cargo.toml at root →
        // we expect: PROJECT_NAME=Amp Easy Backo, STACK_SUMMARY=Rust,
        // TEST_CMD=cargo test, LINT_CMD=cargo clippy …, language=English.
        // The two unknown placeholders (DO_NOT_1/2) stay untouched —
        // those are for the agent to fill during bootstrap step 1.
        let (_tmp, generic_root) = write_rust_with_placeholders();
        // Rename to a meaningful directory name so PROJECT_NAME comes out right.
        let parent = generic_root.parent().unwrap();
        let renamed = parent.join("amp-easy-backo");
        std::fs::rename(&generic_root, &renamed).unwrap();

        let modified = prefill_all_for_tests(&renamed);
        assert!(modified >= 1, "at least the AGENTS.md should be touched");

        let body = std::fs::read_to_string(renamed.join("docs/AGENTS.md")).unwrap();
        assert!(body.contains("Amp Easy Backo"), "PROJECT_NAME pretty-formatted");
        assert!(body.contains("# Amp Easy Backo — Rust"), "STACK_SUMMARY filled");
        assert!(body.contains("Test: cargo test"));
        assert!(body.contains("Lint: cargo clippy --all-targets -- -D warnings"));
        assert!(body.contains("Working language: English"));
        // Unknown placeholders we don't touch — bootstrap agent's job.
        assert!(body.contains("{{DO_NOT_1}}"));
        assert!(body.contains("{{DO_NOT_2}}"));
        // Cleanup so the rename doesn't leak.
        std::fs::remove_dir_all(&renamed).ok();
    }

    #[test]
    fn prefill_is_idempotent_and_noops_when_no_placeholders_left() {
        let (_tmp, root) = write_rust_with_placeholders();
        let _ = prefill_all_for_tests(&root);
        // Second pass — no `{{KNOWN}}` left for us to replace.
        let modified = prefill_all_for_tests(&root);
        assert_eq!(modified, 0, "second pass must be a noop");
    }

    #[test]
    fn prefill_handles_typescript_monorepo_subfolder() {
        // Many projects put their JS deps in `frontend/package.json`,
        // not at the root. `detect_stack_signals` walks one level
        // down to catch that — without the walk, TEST_CMD/LINT_CMD
        // would come out blank on these layouts.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frontend")).unwrap();
        std::fs::write(root.join("frontend/package.json"), "{}").unwrap();
        std::fs::write(root.join("frontend/tsconfig.json"), "{}").unwrap();
        std::fs::write(root.join("frontend/pnpm-lock.yaml"), "lockfileVersion: 9.0\n").unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(
            root.join("docs/AGENTS.md"),
            "Stack: {{STACK_SUMMARY}}\nTest: {{TEST_CMD}}\nLint: {{LINT_CMD}}\n",
        ).unwrap();

        prefill_all_for_tests(root);

        let body = std::fs::read_to_string(root.join("docs/AGENTS.md")).unwrap();
        assert!(body.contains("Stack: TypeScript"), "got: {body}");
        assert!(body.contains("Test: pnpm test"), "got: {body}");
        assert!(body.contains("Lint: pnpm tsc --noEmit && pnpm lint"), "got: {body}");
    }

    #[test]
    fn prefill_chains_test_and_lint_commands_for_multi_stack() {
        // Rust + Python both detected → `cargo test && pytest`,
        // `cargo clippy … && ruff check`. Stack summary lists both.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::write(root.join("pyproject.toml"), "[project]\nname = \"x\"\n").unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(
            root.join("docs/AGENTS.md"),
            "Stack: {{STACK_SUMMARY}}\nTest: {{TEST_CMD}}\nLint: {{LINT_CMD}}\n",
        ).unwrap();

        prefill_all_for_tests(root);

        let body = std::fs::read_to_string(root.join("docs/AGENTS.md")).unwrap();
        assert!(body.contains("Stack: Rust + Python"), "got: {body}");
        assert!(body.contains("Test: cargo test && pytest"));
        assert!(body.contains("Lint: cargo clippy --all-targets -- -D warnings && ruff check"));
    }

    #[test]
    fn prefill_also_rewrites_root_redirectors() {
        // CLAUDE.md / .cursorrules / etc. ship from the same template
        // tree and carry the same placeholders. They must be patched
        // alongside docs/*.md so agents on either side of the entry
        // see consistent values.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/AGENTS.md"), "stub\n").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "Project: {{PROJECT_NAME}} ({{STACK_SUMMARY}})").unwrap();
        std::fs::write(root.join(".cursorrules"), "Stack: {{STACK_SUMMARY}}").unwrap();

        prefill_all_for_tests(root);

        let claude = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        let cursor = std::fs::read_to_string(root.join(".cursorrules")).unwrap();
        assert!(!claude.contains("{{PROJECT_NAME}}"), "CLAUDE.md still raw");
        assert!(!claude.contains("{{STACK_SUMMARY}}"));
        assert!(!cursor.contains("{{STACK_SUMMARY}}"));
        assert!(cursor.contains("Stack: Rust"));
    }

    #[test]
    fn prefill_is_safe_on_a_project_with_no_docs_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No docs/ at all — must return 0, not panic, not create the dir.
        let n = prefill_all_for_tests(tmp.path());
        assert_eq!(n, 0);
        assert!(!tmp.path().join("docs").exists());
    }

    #[tokio::test]
    async fn migration_preserves_user_placeholders() {
        // Regression: the user-reported `amp-easy-backo` shape. The
        // legacy `ai/` had raw `{{PROJECT_NAME}}` etc. that no agent
        // ever filled. Running the migration should now move AND
        // prefill in a single operator action.
        let (_tmp, root) = make_legacy_project().await;
        // Drop a placeholdered file inside ai/ before migration.
        std::fs::write(
            root.join("ai/agents.md"),
            "{{PROJECT_NAME}} — Test: {{TEST_CMD}}\n",
        ).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["add", "."]).current_dir(&root).output().await.unwrap();
        let _ = crate::core::cmd::async_cmd("git").args(["commit", "-q", "-m", "+placeholders"]).current_dir(&root).output().await.unwrap();

        let outcome = migrate_project(&root, false).await;
        assert!(matches!(outcome, MigrationOutcome::Migrated { .. }));

        let body = std::fs::read_to_string(root.join("docs/agents.md")).unwrap();
        // Codex A2 — the migration moves the USER's documents: their
        // placeholders are user content and must survive byte-level.
        // Prefill only ever runs on files a template install created.
        assert!(body.contains("{{PROJECT_NAME}}"),
            "migration must NOT prefill user placeholders — got: {body}");
        assert!(body.contains("{{TEST_CMD}}"),
            "migration must NOT prefill user placeholders — got: {body}");
    }
}
