//! Documentation locations for the briefing (KT-999): what the repository
//! already holds, the path rules for a location typed by hand, and the date
//! of the last commit touching each one.

use std::path::{Component, Path};
use std::time::{Duration, Instant};

use crate::models::{BriefingDetectedDoc, BriefingDocKind};

pub const MAX_LOCATIONS: usize = 30;
pub const PATH_MAX_CHARS: usize = 200;
const ROOT_SCAN_MAX: usize = 1_000;
const GIT_CALL_TIMEOUT: Duration = Duration::from_secs(3);
/// Total time spent on dates per request; later locations get none.
const GIT_TOTAL_BUDGET: Duration = Duration::from_secs(8);

/// Agent-instruction redirectors: they point at the AI doc, they are not docs.
const REDIRECTORS: &[&str] = &["CLAUDE.md", "GEMINI.md", "AGENTS.md"];
const ADR_PATHS: &[&str] = &[
    "adr",
    "adrs",
    "decisions",
    "docs/adr",
    "docs/adrs",
    "docs/decisions",
    "docs/architecture/decisions",
    "doc/adr",
    "doc/decisions",
];
const API_DIRS: &[&str] = &["apidoc", "apidocs", "api-docs", "docs/api", "doc/api"];
const ROOT_DOC_DIRS: &[&str] = &["docs", "doc", "documentation", "wiki"];

fn is_api_spec(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.starts_with("openapi") || lower.starts_with("swagger"))
        && [".yaml", ".yml", ".json"]
            .iter()
            .any(|ext| lower.ends_with(ext))
}

/// The kind of a normalised repository-relative path.
pub fn classify(path: &str, is_dir: bool) -> BriefingDocKind {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if is_dir && ADR_PATHS.contains(&lower.as_str()) {
        BriefingDocKind::Adr
    } else if (is_dir && API_DIRS.contains(&lower.as_str())) || (!is_dir && is_api_spec(name)) {
        BriefingDocKind::Api
    } else if !is_dir && name.starts_with("readme") {
        BriefingDocKind::Readme
    } else if !is_dir && name.starts_with("contributing") {
        BriefingDocKind::Contributing
    } else if is_dir && name == "wiki" {
        BriefingDocKind::Wiki
    } else if is_dir {
        BriefingDocKind::Docs
    } else {
        BriefingDocKind::Markdown
    }
}

/// Entry kind without following symlinks: `Some(true)` dir, `Some(false)` file.
fn plain_entry(path: &Path) -> Option<bool> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        None
    } else if kind.is_dir() {
        Some(true)
    } else if kind.is_file() {
        Some(false)
    } else {
        None
    }
}

fn file_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .take(ROOT_SCAN_MAX)
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect()
}

/// Documentation locations at well-known places, symlinks skipped, sorted by
/// kind then path, at most [`MAX_LOCATIONS`].
pub fn detect(root: &Path) -> Vec<(String, BriefingDocKind, bool)> {
    let mut found: Vec<(String, bool)> = Vec::new();
    for name in file_names(root) {
        let upper = name.to_ascii_uppercase();
        let doc_file = upper.starts_with("README")
            || upper.starts_with("CONTRIBUTING")
            || (upper.ends_with(".MD") && !REDIRECTORS.contains(&name.as_str()))
            || is_api_spec(&name);
        if doc_file {
            found.push((name, false));
        }
    }
    let dirs = ROOT_DOC_DIRS
        .iter()
        .chain(ADR_PATHS)
        .chain(API_DIRS)
        .copied();
    for dir in dirs {
        if plain_entry(&root.join(dir)) == Some(true) && no_symlink_on_the_way(root, dir) {
            found.push((dir.to_string(), true));
        }
    }
    for base in ["docs", "doc", "api", "spec", "specs", "openapi"] {
        if plain_entry(&root.join(base)) != Some(true) {
            continue;
        }
        for name in file_names(&root.join(base)) {
            if is_api_spec(&name) {
                found.push((format!("{base}/{name}"), false));
            }
        }
    }
    let mut out: Vec<(String, BriefingDocKind, bool)> = found
        .into_iter()
        .filter(|(p, _)| p.chars().count() <= PATH_MAX_CHARS && safe_chars(p))
        .map(|(p, is_dir)| {
            let kind = classify(&p, is_dir);
            (p, kind, is_dir)
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    out.dedup_by(|a, b| a.0 == b.0);
    out.truncate(MAX_LOCATIONS);
    out
}

fn no_symlink_on_the_way(root: &Path, rel: &str) -> bool {
    let mut cur = root.to_path_buf();
    rel.split('/').all(|part| {
        cur.push(part);
        plain_entry(&cur).is_some()
    })
}

/// Characters a path may carry into the briefing's Markdown and its machine copy.
fn safe_chars(path: &str) -> bool {
    !path.chars().any(char::is_control)
        && !path.contains(['\\', '`', '|', '<', '>'])
        && !path.contains("{{")
        && !path.contains("}}")
}

/// A location typed by hand: relative, normalised, inside the root once
/// symlinks are resolved, and present. Returns the path and whether it is a
/// directory.
pub fn normalize_path(root: &Path, raw: &str) -> Result<(String, bool), String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("documentation path: empty".into());
    }
    if trimmed.chars().count() > PATH_MAX_CHARS {
        return Err(format!(
            "documentation path: at most {PATH_MAX_CHARS} characters"
        ));
    }
    if !safe_chars(trimmed) {
        return Err(format!(
            "documentation path `{trimmed}`: use plain `/` separators, no control characters, backquotes, `|`, `<`, `>` or braces"
        ));
    }
    crate::core::repository_resources::validate_relative(trimmed).map_err(|_| {
        format!("documentation path `{trimmed}` must be relative to the repository, without `..`")
    })?;
    let parts: Vec<String> = Path::new(trimmed)
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        return Err(format!("documentation path `{trimmed}` names nothing"));
    }
    let relative = parts.join("/");
    let resolved = crate::core::fs_guard::resolve_contained_read(root, Path::new(&relative))
        .map_err(|_| format!("documentation path `{relative}` resolves outside the repository"))?;
    let meta = std::fs::metadata(&resolved)
        .map_err(|_| format!("documentation path `{relative}` does not exist"))?;
    if !meta.is_dir() && !meta.is_file() {
        return Err(format!(
            "documentation path `{relative}` is neither a file nor a directory"
        ));
    }
    Ok((relative, meta.is_dir()))
}

/// `YYYY-MM-DD` of the last commit touching `relative`, or `None` outside a
/// repository, on a timeout or for a path git never saw.
pub async fn last_commit_date(root: &Path, relative: &str) -> Option<String> {
    let mut cmd = crate::core::cmd::async_git_cmd();
    cmd.args([
        "--literal-pathspecs",
        "log",
        "-1",
        "--format=%cs",
        "--",
        relative,
    ])
    .current_dir(root)
    .stdin(std::process::Stdio::null())
    .kill_on_drop(true);
    let output = tokio::time::timeout(GIT_CALL_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let date = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let shaped = date.len() == 10
        && date.char_indices().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    shaped.then_some(date)
}

/// Dates for each path, in order, within [`GIT_TOTAL_BUDGET`].
pub async fn last_commit_dates(root: &Path, paths: &[String]) -> Vec<Option<String>> {
    let started = Instant::now();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        if started.elapsed() >= GIT_TOTAL_BUDGET {
            out.push(None);
            continue;
        }
        out.push(last_commit_date(root, path).await);
    }
    out
}

/// The detected locations with their last commit date.
pub async fn detect_with_dates(root: &Path) -> Vec<BriefingDetectedDoc> {
    let found = detect(root);
    let paths: Vec<String> = found.iter().map(|(p, _, _)| p.clone()).collect();
    let dates = last_commit_dates(root, &paths).await;
    found
        .into_iter()
        .zip(dates)
        .map(|((path, kind, is_dir), last_commit)| BriefingDetectedDoc {
            path,
            kind,
            is_dir,
            last_commit,
        })
        .collect()
}

#[cfg(test)]
#[path = "briefing_docs_test.rs"]
mod tests;
