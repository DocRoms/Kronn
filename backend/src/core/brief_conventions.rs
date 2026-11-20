//! KT-1747 — what a worker's brief says about the PROJECT it works in, found
//! from the worktree rather than assumed: the toolchains, the project's own
//! agent instructions (`AGENTS.md`, `CLAUDE.md`, an `ai/` folder), the commit
//! format its history shows, and whether it has doc citations or a web UI at
//! all (a brief of another project does not carry Kronn's rules for either).

use crate::core::project_toolchains::{
    conventions_lines_in, conventions_section_from, PackageRoot, ToolchainKind,
};
use std::path::Path;

/// The project's own instructions for an agent, most specific first.
const AI_DOCS: [&str; 8] = [
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    ".github/copilot-instructions.md",
    "docs/AGENTS.md",
    "ai/index.md",
    ".ai/index.md",
    ".cursorrules",
];
/// Pointers listed in a brief at most.
const MAX_AI_DOCS: usize = 3;
const EXCERPT_LINES: usize = 5;
const EXCERPT_CHARS: usize = 400;
const READ_CAP: u64 = 16 * 1024;
/// Doc trees searched for citations (the gate's roots, and the `ai/` folders).
const CITATION_ROOTS: [&str; 4] = ["docs", "backend/docs", "ai", ".ai"];
const MAX_DOC_FILES: usize = 300;
const MAX_DOC_DEPTH: usize = 4;

/// One of the project's own agent instruction files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiDoc {
    /// Relative to the repository, `/` separated.
    pub path: String,
    /// The first meaningful lines, flattened; empty when none is safe to show.
    pub excerpt: String,
}

/// A line that could carry a credential is never shown: the pointer matters, the
/// excerpt is a courtesy.
fn looks_sensitive(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "sk-",
        "ghp_",
        "github_pat_",
        "akia",
        "eyj",
        "-----begin",
        "password",
        "passwd",
        "secret",
        "api_key",
        "apikey",
        "token",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn excerpt_of(text: &str) -> String {
    let mut lines = Vec::new();
    let mut in_front_matter = false;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if index == 0 && line == "---" {
            in_front_matter = true;
            continue;
        }
        if in_front_matter {
            in_front_matter = line != "---";
            continue;
        }
        let line = line.trim_start_matches('#').trim();
        if line.is_empty() || line.starts_with("<!--") || looks_sensitive(line) {
            continue;
        }
        lines.push(line.to_string());
        if lines.len() == EXCERPT_LINES {
            break;
        }
    }
    let mut out = lines.join(" / ");
    if out.chars().count() > EXCERPT_CHARS {
        out = out.chars().take(EXCERPT_CHARS).collect::<String>() + "…";
    }
    out
}

/// The agent instruction files the project has, read inside the worktree.
pub fn ai_docs(worktree: &Path) -> Vec<AiDoc> {
    AI_DOCS
        .iter()
        .filter(|rel| worktree.join(rel).is_file())
        .take(MAX_AI_DOCS)
        .map(|rel| {
            let text = crate::core::rooted_io::read_capped(worktree, Path::new(rel), READ_CAP)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            AiDoc {
                path: (*rel).to_string(),
                excerpt: excerpt_of(&text),
            }
        })
        .collect()
}

/// Does the project keep `[src:]` citations in its docs? Only then is a worker
/// told to keep them resolving.
pub fn has_doc_citations(worktree: &Path) -> bool {
    let mut seen = 0usize;
    CITATION_ROOTS
        .iter()
        .any(|root| scan_for_citation(worktree, &worktree.join(root), 0, &mut seen))
}

fn scan_for_citation(worktree: &Path, dir: &Path, depth: usize, seen: &mut usize) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth < MAX_DOC_DEPTH && scan_for_citation(worktree, &path, depth + 1, seen) {
                return true;
            }
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            *seen += 1;
            if *seen > MAX_DOC_FILES {
                return false;
            }
            let Ok(rel) = path.strip_prefix(worktree) else {
                continue;
            };
            if crate::core::rooted_io::read_capped(worktree, rel, READ_CAP)
                .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains("[src:"))
            {
                return true;
            }
        }
    }
    false
}

/// Packages that make a Node root a web UI whose screens can be captured.
const WEB_UI_PACKAGES: [&str; 14] = [
    "react",
    "react-dom",
    "vue",
    "svelte",
    "@angular/core",
    "next",
    "nuxt",
    "preact",
    "solid-js",
    "astro",
    "lit",
    "vite",
    "@remix-run/react",
    "@sveltejs/kit",
];

/// Does the project have a web UI: a Node root that depends on a UI framework
/// or a dev server. A PHP, Rust or Make-only project has none, and is not asked
/// for screen captures.
pub fn has_web_ui(worktree: &Path, profile: &[PackageRoot]) -> bool {
    profile
        .iter()
        .filter(|root| matches!(root.kind, ToolchainKind::Node(_)))
        .any(|root| {
            let manifest = worktree.join(&root.rel_dir).join("package.json");
            let Ok(text) = std::fs::read_to_string(manifest) else {
                return false;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                return false;
            };
            ["dependencies", "devDependencies"].iter().any(|field| {
                value
                    .get(field)
                    .and_then(|deps| deps.as_object())
                    .is_some_and(|deps| WEB_UI_PACKAGES.iter().any(|name| deps.contains_key(*name)))
            })
        })
}

/// The "Project conventions" section of a brief: each toolchain, the project's
/// own agent instructions (a pointer and a short excerpt), and the commit
/// format its history shows. Empty when nothing is known.
pub fn section(worktree: &Path, profile: &[PackageRoot], fr: bool) -> String {
    let mut lines = conventions_lines_in(profile, fr);
    for doc in ai_docs(worktree) {
        let lead = if fr {
            "règles du projet pour un agent : lis"
        } else {
            "the project's own rules for an agent: read"
        };
        lines.push(if doc.excerpt.is_empty() {
            format!("{lead} `{}` first", doc.path)
        } else {
            format!("{lead} `{}` first (starts: « {} »)", doc.path, doc.excerpt)
        });
    }
    // The history is read only where there is one (a git worktree).
    if worktree.join(".git").exists() {
        if let Some(commit) = crate::core::commit_facts::CommitFacts::detect(worktree).brief_line()
        {
            let lead = if fr {
                "format des commits"
            } else {
                "commit format"
            };
            lines.push(format!("{lead}: {commit}"));
        }
    }
    conventions_section_from(&lines, fr)
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "brief_conventions_test.rs"]
mod tests;
