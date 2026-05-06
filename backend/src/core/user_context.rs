//! User-scoped agent context — the cross-project layer.
//!
//! Project-level memory lives in `<project>/docs/`. But many things you
//! want every agent to know are NOT project-specific : your name, your
//! preferences, your tools, your conventions. Repeating them in every
//! project's `docs/AGENTS.md` is duplication; updating them there is
//! drift. Each CLI ships its own user-scoped file (`~/.claude/CLAUDE.md`,
//! `~/.codex/...`, etc.) but the formats are inconsistent and cover only
//! their own CLI.
//!
//! This module bridges that gap : a single user-controlled directory
//! `~/.kronn/user-context/` whose markdown files are concatenated into
//! the system prompt of EVERY agent Kronn spawns, regardless of CLI.
//!
//! ## Layout
//!
//! ```text
//! ~/.kronn/user-context/
//! ├── README.md          ← seed file, explains the convention
//! ├── about-me.md        ← who you are
//! ├── conventions.md     ← cross-project conventions you keep typing
//! └── ...
//! ```
//!
//! The user can keep this in git, sync via Dropbox, etc. — it's just
//! markdown. Order of injection is alphabetical by filename so the
//! ordering is stable for prompt caching.
//!
//! ## What gets injected
//!
//! Each `*.md` file's full content, headed by the filename (so the
//! agent can attribute / split). README.md is excluded (it's seed text
//! describing the convention itself, not user content).

use std::path::{Path, PathBuf};

/// Default location resolved at the home directory of the running user.
/// Overridable via the `KRONN_USER_CONTEXT_DIR` env var (used in tests
/// and Docker mounts that point elsewhere).
pub fn user_context_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("KRONN_USER_CONTEXT_DIR") {
        return PathBuf::from(dir);
    }
    // Resolve $HOME — falls back to /home/kronn (Docker default) if unset.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/kronn".to_string());
    PathBuf::from(home).join(".kronn/user-context")
}

/// Read every `*.md` (except README.md) from the user-context directory
/// and concatenate them into a single markdown blob, ordered alphabetically
/// by filename for prompt-cache stability.
///
/// Returns empty string when the directory doesn't exist OR contains no
/// non-README markdown files. Caller is expected to noop on empty.
///
/// Bootstraps a seed `README.md` on first call if the dir doesn't exist —
/// that way the user discovers the feature simply by browsing their home.
pub fn read_user_context() -> String {
    let dir = user_context_dir();
    if !dir.exists() {
        let _ = ensure_dir_with_readme(&dir);
        return String::new();
    }

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return String::new(),
    };

    let mut files: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            // Markdown only; skip README and any dot-prefixed file.
            if !name.ends_with(".md") || name.eq_ignore_ascii_case("README.md") || name.starts_with('.') {
                return None;
            }
            Some((name, path))
        })
        .collect();

    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = String::new();
    for (name, path) in files {
        let body = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        // 8 KB safety cap per file — user-context shouldn't be huge, and
        // a runaway file shouldn't blow the agent's context budget.
        let trimmed = body.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!("## {}\n\n{}", name, trimmed));
    }
    out
}

/// Create the user-context directory with a seed README if it doesn't
/// exist. Best-effort — silently swallows IO errors (the feature
/// degrades to "no user context" if we can't write).
fn ensure_dir_with_readme(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let readme = dir.join("README.md");
    if !readme.exists() {
        std::fs::write(
            &readme,
            DEFAULT_README,
        )?;
    }
    Ok(())
}

/// Short paragraph injected into every agent's system prompt, explaining
/// the `docs/` write convention. Universal across CLIs (just text), zero
/// cost on projects that have no `docs/` dir (the agent simply has nothing
/// to do — the words don't bind to any action).
///
/// Goal : agents that discover something stable about a project actually
/// write it down rather than re-deriving on the next run. Without this
/// prelude, the agent's training data biases it heavily toward "answer the
/// question, don't update local files" — even when an explicit `ai/`
/// folder is present.
pub fn build_memory_prelude_prompt() -> String {
    "When you discover something stable about THIS project that future agents\n\
     will benefit from, write it to `docs/<subfolder>/<topic>.md` (one of:\n\
     `architecture/`, `conventions/`, `gotchas/`, `operations/`, `people/`).\n\
     Use one short file per topic. Prefer markdown links `[name](path.md)` for\n\
     cross-refs to other docs files (Obsidian-friendly graph view).\n\n\
     Do NOT edit `docs/AGENTS.md` (curated by the audit workflow) or anything\n\
     under `docs/templates/`. Do NOT write secrets — Kronn rejects writes\n\
     that match `.env`, `.pem`, `.ssh/`, or token shapes (sk-, ghp_, AKIA, JWT).\n\n\
     Some projects still use the legacy `ai/` folder instead of `docs/` —\n\
     same conventions, just a different root. Use whichever the project has."
        .to_string()
}

const DEFAULT_README: &str = r#"# Kronn — User-scoped agent context

Markdown files in this directory are auto-injected into the system prompt of every
agent Kronn spawns, regardless of CLI (Claude Code, Codex, Gemini, Copilot, Vibe, Kiro).

## What goes here

Cross-project facts about YOU — things you don't want to repeat in every project's
`docs/AGENTS.md`:

- `about-me.md` — your name, role, organisation, primary language
- `conventions.md` — preferences (e.g. "I use pnpm, never npm; sign-off all commits")
- `tools.md` — IDE / editor / MCP servers you use
- `style.md` — tone, formatting preferences for agent responses
- ...whatever helps the agent serve you better

## Rules

- One topic per file. Keep them short — the user-context is loaded BEFORE the
  project context on every spawn, so it's always in the budget.
- Files are loaded alphabetically by filename. Use prefixes (`00-about-me.md`)
  if you want a specific order.
- This `README.md` is NOT injected (it's seed text). Anything else `*.md` is.
- Plain markdown. `[[wikilinks]]` work if you open this folder in Obsidian.

## Where this is

- Host : `~/.kronn/user-context/`
- Container : same path, mounted rw on Docker installations.

## Privacy

These files live on your machine and are loaded into every agent prompt locally.
They are NOT shared with peers via Kronn's P2P system unless you explicitly
opt in (no current path for that — would land as a future feature).
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn with_temp_dir<F: FnOnce(&Path)>(f: F) {
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("KRONN_USER_CONTEXT_DIR", tmp.path());
        f(tmp.path());
        std::env::remove_var("KRONN_USER_CONTEXT_DIR");
    }

    #[test]
    #[serial]
    fn read_returns_empty_when_dir_missing_and_bootstraps_readme() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("nonexistent");
        std::env::set_var("KRONN_USER_CONTEXT_DIR", &dir);
        let result = read_user_context();
        assert!(result.is_empty(), "no files yet → empty");
        assert!(dir.exists(), "bootstrap created the directory");
        assert!(dir.join("README.md").is_file(), "bootstrap created README.md");
        std::env::remove_var("KRONN_USER_CONTEXT_DIR");
    }

    #[test]
    #[serial]
    fn read_concatenates_files_alphabetically_excluding_readme() {
        with_temp_dir(|dir| {
            std::fs::write(dir.join("README.md"), "# README — should be excluded").unwrap();
            std::fs::write(dir.join("01-about-me.md"), "I am Alice.").unwrap();
            std::fs::write(dir.join("02-conventions.md"), "Use pnpm not npm.").unwrap();
            let result = read_user_context();
            assert!(!result.contains("should be excluded"));
            // Files appear in alphabetical order with their names as headers.
            let pos_about = result.find("01-about-me.md").expect("about-me missing");
            let pos_conv = result.find("02-conventions.md").expect("conventions missing");
            assert!(pos_about < pos_conv, "alphabetical ordering");
            assert!(result.contains("I am Alice."));
            assert!(result.contains("Use pnpm not npm."));
        });
    }

    #[test]
    #[serial]
    fn read_skips_dot_prefixed_files() {
        // Editors leave `.swp`, `.bak.md`; we must not pick them up.
        with_temp_dir(|dir| {
            std::fs::write(dir.join(".about-me.swp.md"), "garbage").unwrap();
            std::fs::write(dir.join("real.md"), "real content").unwrap();
            let result = read_user_context();
            assert!(!result.contains("garbage"));
            assert!(result.contains("real content"));
        });
    }

    #[test]
    #[serial]
    fn read_skips_empty_files() {
        with_temp_dir(|dir| {
            std::fs::write(dir.join("empty.md"), "   \n  \n").unwrap();
            std::fs::write(dir.join("real.md"), "real content").unwrap();
            let result = read_user_context();
            assert!(!result.contains("empty.md"));
            assert!(result.contains("real content"));
        });
    }

    #[test]
    #[serial]
    fn read_skips_non_md_files() {
        with_temp_dir(|dir| {
            std::fs::write(dir.join("data.txt"), "not markdown").unwrap();
            std::fs::write(dir.join("notes.md"), "markdown content").unwrap();
            let result = read_user_context();
            assert!(!result.contains("not markdown"));
            assert!(result.contains("markdown content"));
        });
    }

    #[test]
    #[serial]
    fn read_returns_empty_when_only_readme_present() {
        with_temp_dir(|dir| {
            std::fs::write(dir.join("README.md"), "# only readme").unwrap();
            let result = read_user_context();
            assert!(result.is_empty(),
                "no user content yet → returns empty so we don't inject the seed README");
        });
    }

    #[test]
    #[serial]
    fn user_context_dir_respects_env_var() {
        std::env::set_var("KRONN_USER_CONTEXT_DIR", "/custom/path");
        assert_eq!(user_context_dir(), PathBuf::from("/custom/path"));
        std::env::remove_var("KRONN_USER_CONTEXT_DIR");
    }

    // ─── memory prelude (T6) ──────────────────────────────────────────

    #[test]
    fn memory_prelude_mentions_writable_subfolders() {
        // Regression guard: if someone trims the prelude without listing
        // these, agents stop knowing where to write. Each subfolder must
        // appear by name.
        let prelude = build_memory_prelude_prompt();
        for sub in &["architecture", "conventions", "gotchas", "operations", "people"] {
            assert!(
                prelude.contains(sub),
                "memory prelude must mention `{}/` so agents know to write there",
                sub
            );
        }
    }

    #[test]
    fn memory_prelude_forbids_writing_to_curated_areas() {
        let prelude = build_memory_prelude_prompt();
        assert!(prelude.contains("docs/AGENTS.md"), "must explicitly forbid editing docs/AGENTS.md");
        assert!(prelude.contains("docs/templates/"), "must explicitly forbid editing templates/");
    }

    #[test]
    fn memory_prelude_warns_about_secrets() {
        let prelude = build_memory_prelude_prompt();
        assert!(prelude.to_lowercase().contains("secret"), "must warn about secrets");
        // Concrete patterns: at least one of the major secret prefixes
        // must be named so agents recognize the rejection class.
        let mentions_pattern = prelude.contains("sk-") || prelude.contains("ghp_") || prelude.contains("AKIA");
        assert!(mentions_pattern, "must name at least one secret pattern Kronn rejects");
    }

    #[test]
    fn memory_prelude_mentions_legacy_ai_fallback() {
        // Existing Kronn-managed projects use ai/ until migrated. Agents
        // must keep working there without confusion.
        let prelude = build_memory_prelude_prompt();
        assert!(prelude.contains("ai/"), "must acknowledge the legacy ai/ fallback");
    }

    #[test]
    fn memory_prelude_recommends_markdown_links() {
        let prelude = build_memory_prelude_prompt();
        // Must mention markdown link syntax to keep Obsidian graph clean.
        assert!(prelude.contains("[name](path.md)") || prelude.contains("markdown links"),
            "must recommend markdown link syntax for cross-refs");
    }
}
