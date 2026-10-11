//! Bans from the briefing that carry a verifiable pattern (KT-1193): a
//! forbidden dependency, import or file glob, checked over the project.
//!
//! Same contract as the KT-152 baseline detectors: `file:line` (or path)
//! signals injected into step 8, whose disposition `validation.rs` enforces.
//! Read-only, size-capped, one bounded walk that never follows a symlink.

use std::path::{Path, PathBuf};

use crate::core::audit_baseline::{read_walked, rel_display, Capped, MAX_WALKED_FILES};
use crate::core::audit_detectors::{walk, DetectedSignal, Severity, SOURCE_EXTS};
use crate::models::{ProjectRule, ProjectRuleKind, ProjectRulePattern, ProjectRulePatternKind};

pub const DETECTOR_ID: &str = "project-ban";

/// Lines that declare an import in the stacks Kronn audits; a module name
/// elsewhere (prose, comments, identifiers) is not an import.
const IMPORT_KEYWORDS: &[&str] = &[
    "import",
    "require",
    "require_once",
    "from",
    "use",
    "include",
    "include_once",
    "extern crate",
    "@use",
];

#[derive(Default)]
struct Files {
    manifests: Vec<PathBuf>,
    sources: Vec<PathBuf>,
    /// Every walked file, repository-relative, for the globs.
    all: Vec<String>,
}

fn is_manifest(name: &str) -> bool {
    matches!(name, "package.json" | "composer.json" | "Cargo.toml")
        || (name.starts_with("requirements") && name.ends_with(".txt"))
}

fn collect(root: &Path) -> Files {
    let mut files = Files::default();
    for entry in walk(root).take(MAX_WALKED_FILES) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy();
        if is_manifest(&name) {
            files.manifests.push(path.to_path_buf());
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        if SOURCE_EXTS.contains(&ext) {
            files.sources.push(path.to_path_buf());
        }
        files.all.push(rel_display(root, path));
    }
    files.manifests.sort();
    files.sources.sort();
    files.all.sort();
    files
}

/// The bans with a pattern, each as (text, pattern).
fn checked_bans(rules: &[ProjectRule]) -> Vec<(&str, &ProjectRulePattern)> {
    rules
        .iter()
        .filter(|r| r.kind == ProjectRuleKind::Ban)
        .filter_map(|r| {
            r.pattern
                .as_ref()
                .filter(|p| !p.value.trim().is_empty())
                .map(|p| (r.text.as_str(), p))
        })
        .collect()
}

pub fn pattern_kind_label(kind: ProjectRulePatternKind) -> &'static str {
    match kind {
        ProjectRulePatternKind::Dependency => "dependency",
        ProjectRulePatternKind::Import => "import",
        ProjectRulePatternKind::FileGlob => "file glob",
    }
}

/// Every hit of every checked ban; empty without one, and then no walk.
pub fn detect_ban_violations(root: &Path, rules: &[ProjectRule]) -> Vec<DetectedSignal> {
    let bans = checked_bans(rules);
    if bans.is_empty() {
        return Vec::new();
    }
    let files = collect(root);
    let mut sink = Capped::new(DETECTOR_ID, "Maintainability", Severity::Medium);
    for (text, pattern) in bans {
        let value = pattern.value.trim();
        let title = format!(
            "Ban violated: {text} ({} `{value}`)",
            pattern_kind_label(pattern.kind)
        );
        let hits = match pattern.kind {
            ProjectRulePatternKind::Dependency => dependency_hits(root, &files.manifests, value),
            ProjectRulePatternKind::Import => import_hits(root, &files.sources, value),
            ProjectRulePatternKind::FileGlob => glob_hits(&files.all, value),
        };
        for evidence in hits {
            sink.push(title.clone(), evidence);
        }
    }
    sink.finish()
}

// ─── Dependencies ──────────────────────────────────────────────────────

fn dependency_hits(root: &Path, manifests: &[PathBuf], name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for path in manifests {
        let Some(content) = read_walked(path) else {
            continue;
        };
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let line = match file.as_str() {
            "package.json" => json_dependency_line(
                &content,
                &[
                    "dependencies",
                    "devDependencies",
                    "peerDependencies",
                    "optionalDependencies",
                ],
                name,
                false,
            ),
            "composer.json" => {
                json_dependency_line(&content, &["require", "require-dev"], name, true)
            }
            "Cargo.toml" => cargo_dependency_line(&content, name),
            _ => requirements_dependency_line(&content, name),
        };
        if let Some(line) = line {
            out.push(format!("{}:{line}", rel_display(root, path)));
        }
    }
    out
}

/// Line of `"name":` in the manifest, once the JSON says it is declared.
fn json_dependency_line(
    content: &str,
    sections: &[&str],
    name: &str,
    case_insensitive: bool,
) -> Option<usize> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    let same = |a: &str| {
        if case_insensitive {
            a.eq_ignore_ascii_case(name)
        } else {
            a == name
        }
    };
    let declared = sections.iter().any(|s| {
        json.get(*s)
            .and_then(|v| v.as_object())
            .is_some_and(|deps| deps.keys().any(|k| same(k)))
    });
    if !declared {
        return None;
    }
    let line = content
        .lines()
        .position(|l| {
            let t = l.trim_start();
            t.strip_prefix('"')
                .and_then(|rest| rest.split_once('"'))
                .is_some_and(|(key, after)| same(key) && after.trim_start().starts_with(':'))
        })
        .map(|i| i + 1)
        .unwrap_or(1);
    Some(line)
}

fn cargo_name(name: &str) -> String {
    name.trim()
        .trim_matches('"')
        .replace('_', "-")
        .to_ascii_lowercase()
}

/// Line of a dependency in any `*dependencies` table, inline or as a
/// `[dependencies.name]` table. A `package = "…"` rename is not followed.
fn cargo_dependency_line(content: &str, name: &str) -> Option<usize> {
    let wanted = cargo_name(name);
    let mut in_deps = false;
    for (i, raw) in content.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if let Some(header) = line.strip_prefix('[') {
            let header = header.trim_start_matches('[').trim_end_matches(']').trim();
            in_deps = header.ends_with("dependencies");
            if let Some((table, dep)) = header.rsplit_once('.') {
                if table.ends_with("dependencies") && cargo_name(dep) == wanted {
                    return Some(i + 1);
                }
            }
            continue;
        }
        if in_deps {
            if let Some((key, _)) = line.split_once('=') {
                if cargo_name(key) == wanted {
                    return Some(i + 1);
                }
            }
        }
    }
    None
}

/// PEP 503 normalisation: case-insensitive, runs of `-_.` are one `-`.
fn pep503(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            dash = true;
        } else {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

fn requirements_dependency_line(content: &str, name: &str) -> Option<usize> {
    let wanted = pep503(name);
    content.lines().enumerate().find_map(|(i, raw)| {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('-') {
            return None;
        }
        let pkg: String = line
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .collect();
        (pep503(&pkg) == wanted).then_some(i + 1)
    })
}

// ─── Imports ───────────────────────────────────────────────────────────

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '$')
}

/// `needle` in `line` with no name character glued on either side.
pub(crate) fn word_match(line: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(at) = line[from..].find(needle) {
        let start = from + at;
        let end = start + needle.len();
        let before = line[..start].chars().next_back();
        let after = line[end..].chars().next();
        if !before.is_some_and(is_name_char) && !after.is_some_and(is_name_char) {
            return true;
        }
        from = start + line[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

fn is_import_line(line: &str) -> bool {
    IMPORT_KEYWORDS.iter().any(|k| word_match(line, k))
}

fn import_hits(root: &Path, sources: &[PathBuf], module: &str) -> Vec<String> {
    let mut out = Vec::new();
    for path in sources {
        let Some(content) = read_walked(path) else {
            continue;
        };
        for (i, line) in content.lines().enumerate() {
            if is_import_line(line) && word_match(line, module) {
                out.push(format!("{}:{}", rel_display(root, path), i + 1));
            }
        }
    }
    out
}

// ─── File globs ────────────────────────────────────────────────────────

/// A glob without `/` matches a file name at any depth, as in `.gitignore`.
pub(crate) fn glob_hit(pattern: &str, rel: &str) -> bool {
    use crate::api::agent_workspace_tools::glob_matches;
    if pattern.contains('/') {
        glob_matches(pattern, rel)
    } else {
        glob_matches(pattern, rel.rsplit('/').next().unwrap_or(rel))
    }
}

fn glob_hits(all: &[String], pattern: &str) -> Vec<String> {
    all.iter()
        .filter(|rel| glob_hit(pattern, rel))
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "project_rules_test.rs"]
mod tests;
