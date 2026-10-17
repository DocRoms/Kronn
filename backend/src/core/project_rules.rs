//! Bans from the briefing that carry a verifiable pattern (KT-1193): a
//! forbidden dependency, import or file glob, checked over the project.
//!
//! Same contract as the KT-152 baseline detectors: `file:line` (or path)
//! signals injected into step 8, whose disposition `validation.rs` enforces.
//! Read-only, size-capped, one bounded walk that never follows a symlink.

use std::path::{Path, PathBuf};

use crate::core::audit_baseline::{read_walked, rel_display, Capped, MAX_WALKED_FILES};
use crate::core::audit_detectors::{walk_including_generated as walk, DetectedSignal, Severity};
use crate::models::{ProjectRule, ProjectRuleKind, ProjectRulePattern, ProjectRulePatternKind};

pub const DETECTOR_ID: &str = "project-ban";

#[derive(Default)]
struct Files {
    manifests: Vec<PathBuf>,
    sources: Vec<PathBuf>,
    /// Every walked file, repository-relative, for the globs.
    all: Vec<String>,
}

fn is_manifest(name: &str) -> bool {
    matches!(
        name,
        "package.json" | "composer.json" | "Cargo.toml" | "pyproject.toml"
    ) || (name.starts_with("requirements") && name.ends_with(".txt"))
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
        if syntax_for(ext).is_some() {
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
            "pyproject.toml" => pyproject_dependency_line(&content, name),
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

/// `serde` for `serde`, `"serde"` and the dotted `serde.workspace`.
fn cargo_key_root(key: &str) -> &str {
    let key = key.trim();
    match key.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => key.split('.').next().unwrap_or(key),
    }
}

/// Line of a dependency in any `*dependencies` table, inline or as a
/// `[dependencies.name]` table, dotted keys included. A `package = "…"`
/// rename is not followed.
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
                if cargo_name(cargo_key_root(key)) == wanted {
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

/// The requirement name at the start of a PEP 508 string.
fn requirement_name(spec: &str) -> &str {
    let spec = spec.trim();
    let end = spec
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .unwrap_or(spec.len());
    &spec[..end]
}

/// PEP 621 `[project]` dependencies (optional ones included) and Poetry's
/// `[tool.poetry.*dependencies]` tables, located on the first line naming them.
fn pyproject_dependency_line(content: &str, name: &str) -> Option<usize> {
    let wanted = pep503(name);
    let doc: toml::Table = content.parse().ok()?;
    let same = |n: &str| pep503(n) == wanted;
    let in_list = |v: Option<&toml::Value>| {
        v.and_then(|v| v.as_array()).is_some_and(|a| {
            a.iter()
                .any(|x| x.as_str().is_some_and(|s| same(requirement_name(s))))
        })
    };
    let in_keys = |v: Option<&toml::Value>| {
        v.and_then(|v| v.as_table())
            .is_some_and(|t| t.keys().any(|k| same(k)))
    };
    let project = doc.get("project");
    let mut declared = in_list(project.and_then(|p| p.get("dependencies")))
        || project
            .and_then(|p| p.get("optional-dependencies"))
            .and_then(|o| o.as_table())
            .is_some_and(|o| o.values().any(|v| in_list(Some(v))));
    if let Some(poetry) = doc.get("tool").and_then(|t| t.get("poetry")) {
        declared = declared
            || in_keys(poetry.get("dependencies"))
            || in_keys(poetry.get("dev-dependencies"))
            || poetry
                .get("group")
                .and_then(|g| g.as_table())
                .is_some_and(|g| g.values().any(|grp| in_keys(grp.get("dependencies"))));
    }
    if !declared {
        return None;
    }
    let line = content
        .lines()
        .position(|l| {
            let lead = requirement_name(l.trim_start());
            same(lead) || quoted(l).iter().any(|q| same(requirement_name(q)))
        })
        .map(|i| i + 1)
        .unwrap_or(1);
    Some(line)
}

// ─── Imports ───────────────────────────────────────────────────────────

/// How a language spells an import, and what separates a module from its
/// submodules (`lodash/fp` is lodash, `lodash.debounce` is another package).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syntax {
    Js,
    Python,
    Rust,
    Php,
    Go,
    /// Java, Kotlin, Scala, C#.
    Dotted,
    C,
    Ruby,
    Swift,
}

fn syntax_for(ext: &str) -> Option<Syntax> {
    Some(match ext {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "vue" | "svelte" => Syntax::Js,
        "py" => Syntax::Python,
        "rs" => Syntax::Rust,
        "php" => Syntax::Php,
        "go" => Syntax::Go,
        "java" | "kt" | "scala" | "cs" => Syntax::Dotted,
        "c" | "h" | "cpp" | "hpp" | "cc" => Syntax::C,
        "rb" => Syntax::Ruby,
        "swift" => Syntax::Swift,
        _ => return None,
    })
}

fn separators(syntax: Syntax) -> &'static [&'static str] {
    match syntax {
        Syntax::Js | Syntax::Go | Syntax::C | Syntax::Ruby => &["/"],
        Syntax::Python | Syntax::Dotted | Syntax::Swift => &["."],
        Syntax::Rust => &["::"],
        Syntax::Php => &["\\", "/"],
    }
}

/// `spec` is `module` or one of its submodules.
fn names_module(spec: &str, module: &str, syntax: Syntax) -> bool {
    let spec = spec.trim_start_matches('\\').trim_start_matches("::");
    match spec.strip_prefix(module) {
        Some("") => true,
        Some(rest) => separators(syntax).iter().any(|s| rest.starts_with(s)),
        None => false,
    }
}

fn is_comment(t: &str, syntax: Syntax) -> bool {
    t.starts_with("//")
        || t.starts_with("/*")
        || t.starts_with('*')
        || t.starts_with("--")
        || (t.starts_with('#') && syntax != Syntax::C)
}

/// Every quoted literal of the line.
fn quoted(t: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = t;
    while let Some(at) = rest.find(['\'', '"', '`']) {
        let quote = rest[at..].chars().next().unwrap_or('"');
        let after = &rest[at + 1..];
        let Some(end) = after.find(quote) else {
            break;
        };
        out.push(&after[..end]);
        rest = &after[end + 1..];
    }
    out
}

/// The path after a keyword, up to `;`, `{`, `,` or a space.
fn path_after<'a>(t: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = t.strip_prefix(keyword)?.trim_start();
    let end = rest
        .find(|c: char| c == ';' || c == '{' || c == ',' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

fn starts_with_word(t: &str, word: &str) -> bool {
    t.strip_prefix(word).is_some_and(|rest| {
        rest.is_empty() || !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_')
    })
}

/// The module names a source line imports, in that language's syntax.
/// `go_block` tracks a Go `import ( … )` group across lines.
fn import_specs<'a>(line: &'a str, syntax: Syntax, go_block: &mut bool) -> Vec<&'a str> {
    let t = line.trim();
    if t.is_empty() || is_comment(t, syntax) {
        return Vec::new();
    }
    match syntax {
        Syntax::Js => {
            let is_import = starts_with_word(t, "import")
                || t.contains("from '")
                || t.contains("from \"")
                || t.contains("require(")
                || t.contains("import(");
            if is_import {
                quoted(t)
            } else {
                Vec::new()
            }
        }
        Syntax::Python => {
            if let Some(module) = path_after(t, "from ").filter(|_| t.contains(" import")) {
                vec![module]
            } else if let Some(rest) = t.strip_prefix("import ") {
                rest.split(',')
                    .filter_map(|m| m.split_whitespace().next())
                    .collect()
            } else {
                Vec::new()
            }
        }
        Syntax::Rust => {
            let t = t
                .trim_start_matches("pub(crate) ")
                .trim_start_matches("pub ");
            path_after(t, "use ")
                .or_else(|| path_after(t, "extern crate "))
                .into_iter()
                .collect()
        }
        Syntax::Php => {
            if let Some(path) = path_after(t, "use ") {
                vec![path]
            } else if ["require", "include"].iter().any(|k| t.starts_with(k)) {
                quoted(t)
            } else {
                Vec::new()
            }
        }
        Syntax::Go => {
            if *go_block {
                if t.starts_with(')') {
                    *go_block = false;
                    return Vec::new();
                }
                return quoted(t);
            }
            if let Some(rest) = t.strip_prefix("import") {
                if rest.trim_start().starts_with('(') {
                    *go_block = !rest.contains(')');
                }
                return quoted(rest);
            }
            Vec::new()
        }
        Syntax::Dotted => {
            let t = t.replacen("import static ", "import ", 1);
            let path = path_after(&t, "import ").or_else(|| path_after(&t, "using "));
            // `t` is owned here; re-borrow the same slice from `line`.
            path.and_then(|p| line.find(p).map(|at| &line[at..at + p.len()]))
                .into_iter()
                .collect()
        }
        Syntax::C => {
            let Some(rest) = t
                .strip_prefix("#include")
                .or_else(|| t.strip_prefix("#import"))
            else {
                return Vec::new();
            };
            let rest = rest.trim();
            let inner = rest
                .strip_prefix('<')
                .and_then(|r| r.split('>').next())
                .or_else(|| rest.strip_prefix('"').and_then(|r| r.split('"').next()));
            inner.into_iter().collect()
        }
        Syntax::Ruby => {
            if ["require", "require_relative", "load"]
                .iter()
                .any(|k| starts_with_word(t, k))
            {
                quoted(t)
            } else {
                Vec::new()
            }
        }
        Syntax::Swift => path_after(t, "import ").into_iter().collect(),
    }
}

fn import_hits(root: &Path, sources: &[PathBuf], module: &str) -> Vec<String> {
    let mut out = Vec::new();
    for path in sources {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let Some(syntax) = syntax_for(ext) else {
            continue;
        };
        let Some(content) = read_walked(path) else {
            continue;
        };
        let mut go_block = false;
        for (i, line) in content.lines().enumerate() {
            let specs = import_specs(line, syntax, &mut go_block);
            if specs.iter().any(|s| names_module(s, module, syntax)) {
                out.push(format!("{}:{}", rel_display(root, path), i + 1));
            }
        }
    }
    out
}

// ─── File globs ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobToken {
    Byte(u8),
    /// `?`: one character other than `/`.
    One,
    /// `*`: any run without `/`.
    Star,
    /// `**`: any run, `/` included.
    Any,
    /// `**/`: nothing, or any run ending with `/`.
    AnyDir,
}

/// Consecutive stars collapse into one `**`, so no pattern adds work.
fn glob_tokens(pattern: &str) -> Vec<GlobToken> {
    let bytes = pattern.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'*' => {
                let mut n = 0;
                while i < bytes.len() && bytes[i] == b'*' {
                    n += 1;
                    i += 1;
                }
                if n == 1 {
                    out.push(GlobToken::Star);
                } else if i < bytes.len() && bytes[i] == b'/' {
                    i += 1;
                    out.push(GlobToken::AnyDir);
                } else {
                    out.push(GlobToken::Any);
                }
                continue;
            }
            b'?' => out.push(GlobToken::One),
            b => out.push(GlobToken::Byte(b)),
        }
        i += 1;
    }
    out
}

/// Glob match in O(pattern × path) time: a bottom-up table, no backtracking.
pub(crate) fn glob_matches(pattern: &str, path: &str) -> bool {
    let tokens = glob_tokens(pattern);
    let text = path.as_bytes();
    let m = text.len();
    // `next[j]`: tokens after the current one match `text[j..]`.
    let mut next = vec![false; m + 1];
    next[m] = true;
    for token in tokens.iter().rev() {
        let mut cur = vec![false; m + 1];
        // For AnyDir: some `k >= j` with `text[k] == '/'` and `next[k + 1]`.
        let mut dir_ahead = false;
        for j in (0..=m).rev() {
            let here = text.get(j).copied();
            cur[j] = match *token {
                GlobToken::Byte(b) => here == Some(b) && next[j + 1],
                GlobToken::One => here.is_some_and(|c| c != b'/') && next[j + 1],
                GlobToken::Star => next[j] || (here.is_some_and(|c| c != b'/') && cur[j + 1]),
                GlobToken::Any => next[j] || (here.is_some() && cur[j + 1]),
                GlobToken::AnyDir => {
                    if here == Some(b'/') && next[j + 1] {
                        dir_ahead = true;
                    }
                    next[j] || dir_ahead
                }
            };
        }
        next = cur;
    }
    next[0]
}

/// A glob without `/` matches a file name at any depth, as in `.gitignore`.
pub(crate) fn glob_hit(pattern: &str, rel: &str) -> bool {
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
