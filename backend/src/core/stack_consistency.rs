//! KT-1388 — the stack versions the docs state must agree with each other and
//! with the project's manifests.
//!
//! docroms' audit wrote `Symfony 8.1` / `PHP ^8.4` in `AGENTS.md`, headed the
//! PHP section of `coding-rules.md` "Symfony 7.3" and gave `PHP ^8.2` in the
//! tech-debt index. Each document was written by a different step, each
//! looked right alone, and the judges found the contradiction in both runs.
//! This is the deterministic cross-check: no model, only the versions the
//! docs state about a framework / language / runtime, held against the
//! manifests and lock files, then against one another.
//!
//! Conservative on purpose: a version stated on a line that talks about
//! change (upgrade, EOL, "latest", "was", an arrow) is history or a target,
//! not what is in place, and is not read.

use std::path::Path;

/// One version a document or a manifest states for a technology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stated {
    /// Lower-case technology key (`php`, `symfony`, `node`…).
    pub tech: &'static str,
    /// `8.4` → `[8, 4]`.
    pub version: Vec<u32>,
    /// The version as written (`^8.4`, `7.3`).
    pub raw: String,
    /// `path:line`.
    pub place: String,
    /// A minimum (`^8.4`, `>=22`) rather than the version in place.
    pub floor: bool,
}

/// A statement that contradicts the manifests or an earlier document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The document holding the wrong statement.
    pub path: String,
    pub message: String,
}

/// `(display name, key)` of the technologies tracked.
const TECHS: &[(&str, &str)] = &[
    ("PHP", "php"),
    ("php", "php"),
    ("Symfony", "symfony"),
    ("Laravel", "laravel"),
    ("Node.js", "node"),
    ("Node", "node"),
    ("Python", "python"),
    ("React", "react"),
    ("Vue.js", "vue"),
    ("Vue", "vue"),
    ("Angular", "angular"),
    ("Next.js", "next"),
];

/// A line saying one of these talks about change, a target, a claim under
/// review or a date, not the version in place.
const HISTORY_WORDS: &[&str] = &[
    "upgrad",
    "migrat",
    "eol",
    "end of life",
    "end-of-life",
    "support",
    "latest",
    "previously",
    "formerly",
    "used to",
    "deprecated",
    "legacy",
    "claimed",
    "outdated",
    "stale",
    "instead",
    "rather than",
    " was ",
    " were ",
    " not ",
    "no longer",
    "bump",
    "→",
    "->",
];

/// Directories of the docs root that are dated snapshots, gabarits or history:
/// what they say about a version is not what is in place.
const SKIPPED_DIRS: &[&str] = &[
    "reports",
    "legacy",
    "templates",
    "examples",
    "tech-debt",
    "people",
];

/// Whether the document at `within_root` (its path under the docs root)
/// states current facts.
pub fn is_scanned(within_root: &str) -> bool {
    let in_skipped_dir = within_root
        .split_once('/')
        .is_some_and(|(top, _)| SKIPPED_DIRS.contains(&top));
    within_root != "learnings.md" && !in_skipped_dir
}

fn tech_key(name: &str) -> Option<&'static str> {
    TECHS
        .iter()
        .find(|(display, _)| *display == name)
        .map(|(_, key)| *key)
}

/// Every `\d+(.\d+){0,2}` of `text`, with the operator written right before it.
fn versions_in(text: &str) -> Vec<(Vec<u32>, bool, String)> {
    static NUMBER: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r"(\^|~|>=|≥|>|=)?\s*[vV]?(\d{1,3}(?:\.\d+){0,2})").expect("regex")
    });
    NUMBER
        .captures_iter(text)
        .filter_map(|c| {
            let whole = c.get(0)?;
            let number = c.get(2)?;
            // Not the tail of a longer token (`1.8-php8`, an id, a date).
            let before = text[..whole.start()].chars().next_back();
            let after = text[number.end()..].chars().next();
            if before.is_some_and(|ch| ch.is_alphanumeric() || ch == '.')
                || after.is_some_and(|ch| ch.is_ascii_digit())
            {
                return None;
            }
            let version = number
                .as_str()
                .split('.')
                .filter_map(|part| part.parse().ok())
                .collect::<Vec<u32>>();
            let floor = c.get(1).is_some_and(|op| op.as_str() != "=");
            Some((version, floor, whole.as_str().trim().to_string()))
        })
        .collect()
}

fn stated(tech: &'static str, place: String, text: &str) -> Vec<Stated> {
    versions_in(text)
        .into_iter()
        .take(1)
        .map(|(version, floor, raw)| Stated {
            tech,
            version,
            raw,
            place: place.clone(),
            floor,
        })
        .collect()
}

// ─── Manifests ───────────────────────────────────────────────────────────

fn read(project: &Path, rel: &str) -> Option<String> {
    let path = project.join(rel);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    (meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 4 * 1024 * 1024)
        .then(|| std::fs::read_to_string(path).ok())
        .flatten()
}

fn line_of(content: &str, needle: &str) -> usize {
    content
        .lines()
        .position(|line| line.contains(needle))
        .map_or(1, |i| i + 1)
}

/// A constraint (`^7.4|^8.0`, `>=8.2 <9`, `8.4.*`): one statement per alternative.
fn constraint(tech: &'static str, place: &str, value: &str) -> Vec<Stated> {
    value
        .split('|')
        .filter(|alt| !alt.trim().is_empty())
        .flat_map(|alt| stated(tech, place.to_string(), alt))
        .collect()
}

fn json(project: &Path, rel: &str) -> Option<(String, serde_json::Value)> {
    let text = read(project, rel)?;
    let value = serde_json::from_str(&text).ok()?;
    Some((text, value))
}

fn composer_facts(project: &Path, out: &mut Vec<Stated>) {
    if let Some((text, value)) = json(project, "composer.json") {
        let place = |key: &str| format!("composer.json:{}", line_of(&text, &format!("\"{key}\"")));
        let require = value.get("require").and_then(|v| v.as_object());
        if let Some(php) = require.and_then(|r| r.get("php")).and_then(|v| v.as_str()) {
            out.extend(constraint("php", &place("php"), php));
        }
        let framework = [
            "symfony/framework-bundle",
            "symfony/symfony",
            "symfony/console",
        ]
        .into_iter()
        .find_map(|name| {
            require
                .and_then(|r| r.get(name))
                .and_then(|v| v.as_str())
                .map(|v| (name, v))
        });
        if let Some((name, version)) = framework {
            out.extend(constraint("symfony", &place(name), version));
        }
        if let Some(version) = require
            .and_then(|r| r.get("laravel/framework"))
            .and_then(|v| v.as_str())
        {
            out.extend(constraint("laravel", &place("laravel/framework"), version));
        }
        if let Some(pin) = value
            .pointer("/extra/symfony/require")
            .and_then(|v| v.as_str())
        {
            out.extend(constraint("symfony", &place("require"), pin));
        }
    }
    if let Some((text, value)) = json(project, "composer.lock") {
        for group in ["packages", "packages-dev"] {
            for package in value
                .get(group)
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
            {
                let name = package.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let tech = match name {
                    "symfony/framework-bundle" | "symfony/symfony" => "symfony",
                    "laravel/framework" => "laravel",
                    _ => continue,
                };
                if let Some(version) = package.get("version").and_then(|v| v.as_str()) {
                    let place = format!(
                        "composer.lock:{}",
                        line_of(&text, &format!("\"name\": \"{name}\""))
                    );
                    out.extend(stated(tech, place, version));
                }
            }
        }
    }
}

fn package_facts(project: &Path, out: &mut Vec<Stated>) {
    let Some((text, value)) = json(project, "package.json") else {
        return;
    };
    let place = |key: &str| format!("package.json:{}", line_of(&text, &format!("\"{key}\"")));
    if let Some(node) = value.pointer("/engines/node").and_then(|v| v.as_str()) {
        out.extend(constraint("node", &place("node"), node));
    }
    for (dep, tech) in [
        ("react", "react"),
        ("vue", "vue"),
        ("next", "next"),
        ("@angular/core", "angular"),
    ] {
        for group in ["dependencies", "devDependencies"] {
            if let Some(version) = value
                .get(group)
                .and_then(|g| g.get(dep))
                .and_then(|v| v.as_str())
            {
                out.extend(constraint(tech, &place(dep), version));
            }
        }
    }
}

fn runtime_files(project: &Path, out: &mut Vec<Stated>) {
    for (file, tech) in [
        (".node-version", "node"),
        (".nvmrc", "node"),
        (".php-version", "php"),
        (".python-version", "python"),
    ] {
        if let Some(text) = read(project, file) {
            out.extend(stated(tech, format!("{file}:1"), text.trim()));
        }
    }
    if let Some(text) = read(project, ".tool-versions") {
        for (n, line) in text.lines().enumerate() {
            let mut words = line.split_whitespace();
            let tech = match words.next() {
                Some("nodejs" | "node") => "node",
                Some("php") => "php",
                Some("python") => "python",
                _ => continue,
            };
            if let Some(version) = words.next() {
                out.extend(stated(tech, format!(".tool-versions:{}", n + 1), version));
            }
        }
    }
}

/// `FROM php:8.4-fpm`, `FROM dunglas/frankenphp:1.8-php8.4-alpine`, `FROM node:22`,
/// `ARG PHP_VERSION=8.4`.
fn dockerfile_facts(rel: &str, text: &str, out: &mut Vec<Stated>) {
    for (n, line) in text.lines().enumerate() {
        let place = format!("{rel}:{}", n + 1);
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();
        if let Some(from) = lower.strip_prefix("from ") {
            let image = from
                .split_whitespace()
                .find(|word| !word.starts_with("--"))
                .unwrap_or("");
            let Some((name, tag)) = image.split_once(':') else {
                continue;
            };
            let base = name.rsplit('/').next().unwrap_or(name);
            let tech = match base {
                "php" => Some("php"),
                "node" => Some("node"),
                "python" => Some("python"),
                _ => None,
            };
            if let Some(tech) = tech {
                out.extend(stated(tech, place.clone(), tag));
            }
            if let Some(at) = tag.find("php") {
                if tech != Some("php") {
                    out.extend(stated("php", place, &tag[at + 3..]));
                }
            }
        } else if let Some(arg) = lower.strip_prefix("arg ") {
            let Some((name, value)) = arg.split_once('=') else {
                continue;
            };
            let tech = match name.trim() {
                "php_version" => "php",
                "node_version" => "node",
                "python_version" => "python",
                _ => continue,
            };
            out.extend(stated(tech, place, value.trim().trim_matches('"')));
        }
    }
}

fn dockerfiles(project: &Path, out: &mut Vec<Stated>) {
    let skip = |name: &str| matches!(name, "node_modules" | "vendor" | "target" | ".git" | "var");
    let mut found = Vec::new();
    for entry in walkdir::WalkDir::new(project)
        .max_depth(4)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !skip(&e.file_name().to_string_lossy()))
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.file_type().is_file()
            && (name == "Dockerfile"
                || name.starts_with("Dockerfile.")
                || name.ends_with(".Dockerfile"))
        {
            if let Ok(rel) = entry.path().strip_prefix(project) {
                found.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    for rel in found {
        if let Some(text) = read(project, &rel) {
            dockerfile_facts(&rel, &text, out);
        }
    }
}

/// `php-version: 8.4`, `node-version: [20, 22]`, `python-version: "3.12"`.
fn ci_facts(project: &Path, out: &mut Vec<Stated>) {
    let mut files = vec![".gitlab-ci.yml".to_string()];
    if let Ok(entries) = std::fs::read_dir(project.join(".github/workflows")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".yml") || name.ends_with(".yaml") {
                files.push(format!(".github/workflows/{name}"));
            }
        }
    }
    files.sort();
    for rel in files {
        let Some(text) = read(project, &rel) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            let line = line.trim().trim_start_matches("- ");
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let tech = match key.trim() {
                "php-version" => "php",
                "node-version" => "node",
                "python-version" => "python",
                _ => continue,
            };
            for (version, floor, raw) in versions_in(value) {
                out.push(Stated {
                    tech,
                    version,
                    raw,
                    place: format!("{rel}:{}", n + 1),
                    floor,
                });
            }
        }
    }
}

/// What the project's own files say about its stack.
pub fn manifest_facts(project: &Path) -> Vec<Stated> {
    let mut out = Vec::new();
    composer_facts(project, &mut out);
    package_facts(project, &mut out);
    runtime_files(project, &mut out);
    dockerfiles(project, &mut out);
    ci_facts(project, &mut out);
    out
}

// ─── Documents ───────────────────────────────────────────────────────────

/// A markdown line without what is not prose: citation markers, link targets,
/// URLs.
fn prose(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("[src:") {
        out.push_str(&rest[..start]);
        match rest[start..].find(']') {
            Some(end) => rest = &rest[start + end + 1..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    let mut cleaned = String::with_capacity(out.len());
    let mut chars = out.as_str();
    while let Some(at) = chars.find("](") {
        cleaned.push_str(&chars[..at + 1]);
        match chars[at..].find(')') {
            Some(end) => chars = &chars[at + end + 1..],
            None => chars = "",
        }
    }
    cleaned.push_str(chars);
    cleaned
}

fn plain(cell: &str) -> &str {
    cell.trim()
        .trim_matches(|c| matches!(c, '*' | '_' | '`'))
        .trim()
}

/// The statements of one document, in line order.
pub fn doc_claims(rel: &str, content: &str) -> Vec<Stated> {
    static TECH: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(
            r"\b(PHP|php|Symfony|Laravel|Node\.js|Node|Python|React|Vue\.js|Vue|Angular|Next\.js)\s*(\^|~|>=|≥|=)?\s*[vV]?(\d{1,3}(?:\.\d+){0,2})",
        )
        .expect("regex")
    });
    let mut out = Vec::new();
    let mut fenced = false;
    for (n, line) in content.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let text = prose(line);
        let place = format!("{rel}:{}", n + 1);
        // A table row states its subject in the first cell and the version in
        // place in the second: the cells after are notes, targets and dates.
        let scanned = if text.trim_start().starts_with('|') {
            let cells: Vec<&str> = text.trim().trim_matches('|').split('|').collect();
            let first = cells.first().map_or("", |cell| plain(cell));
            let name = first
                .split(|c: char| c.is_whitespace() || c == '(' || c == '/')
                .next()
                .unwrap_or("");
            let scanned = cells.iter().take(2).copied().collect::<Vec<_>>().join(" ");
            if HISTORY_WORDS
                .iter()
                .any(|word| scanned.to_lowercase().contains(word))
            {
                continue;
            }
            if let (Some(tech), Some(cell)) = (tech_key(name), cells.get(1)) {
                // `| PHP | 8.4 |`, not `| PHP | see composer.json:7 |`.
                let value = plain(cell);
                let numeric = value
                    .trim_start_matches(['^', '~', '>', '=', '≥', 'v', 'V'])
                    .starts_with(|c: char| c.is_ascii_digit());
                if numeric && !TECH.is_match(cell) {
                    out.extend(stated(tech, place.clone(), value));
                }
            }
            scanned
        } else {
            if HISTORY_WORDS
                .iter()
                .any(|word| text.to_lowercase().contains(word))
            {
                continue;
            }
            text
        };
        for c in TECH.captures_iter(&scanned) {
            let (Some(name), Some(number)) = (c.get(1), c.get(3)) else {
                continue;
            };
            let Some(tech) = tech_key(name.as_str()) else {
                continue;
            };
            let after = scanned[number.end()..].chars().next();
            if after.is_some_and(|ch| ch.is_alphanumeric()) {
                continue;
            }
            let floor = c.get(2).is_some_and(|op| op.as_str() != "=");
            out.push(Stated {
                tech,
                version: number
                    .as_str()
                    .split('.')
                    .filter_map(|part| part.parse().ok())
                    .collect(),
                raw: format!(
                    "{}{}",
                    c.get(2).map_or("", |op| op.as_str()),
                    number.as_str()
                ),
                place: place.clone(),
                floor,
            });
        }
    }
    out
}

// ─── Comparison ──────────────────────────────────────────────────────────

fn same_prefix(a: &[u32], b: &[u32]) -> bool {
    let n = a.len().min(b.len());
    n > 0 && a[..n] == b[..n]
}

/// Whether a manifest statement leaves room for the version a document gives.
fn agrees_with_fact(claim: &Stated, fact: &Stated) -> bool {
    match (claim.floor, fact.floor) {
        // Two minimums name the same requirement or they differ.
        (true, true) | (false, false) => same_prefix(&claim.version, &fact.version),
        // A version in place against a minimum: it must satisfy it.
        (false, true) => {
            claim.version.first() == fact.version.first()
                && claim
                    .version
                    .get(1)
                    .zip(fact.version.get(1))
                    .is_none_or(|(have, floor)| have >= floor)
        }
        // A minimum the doc gives against a version in place: it must admit it.
        (true, false) => {
            claim.version.first() == fact.version.first()
                && claim
                    .version
                    .get(1)
                    .zip(fact.version.get(1))
                    .is_none_or(|(floor, have)| have >= floor)
        }
    }
}

fn label(tech: &str) -> &'static str {
    TECHS
        .iter()
        .find(|(_, key)| *key == tech)
        .map_or("", |(display, _)| *display)
}

/// The statements of `docs` (`(path, content)`, the entry file first) that
/// contradict the manifests of `project`, or — for a technology no manifest
/// states — the first statement made about it. One conflict per document,
/// technology and version.
pub fn conflicts(project: &Path, docs: &[(String, String)]) -> Vec<Conflict> {
    let facts = manifest_facts(project);
    let mut anchors: Vec<Stated> = Vec::new();
    let mut seen: std::collections::BTreeSet<(String, &str, String)> = Default::default();
    let mut out = Vec::new();
    for (rel, content) in docs {
        for claim in doc_claims(rel, content) {
            let of_tech: Vec<&Stated> = facts.iter().filter(|f| f.tech == claim.tech).collect();
            // (the statement it contradicts, whether it is a manifest's)
            let reference: Option<(Stated, bool)> = if of_tech.is_empty() {
                match anchors.iter().find(|a| a.tech == claim.tech) {
                    Some(anchor) => {
                        (!agrees_with_fact(&claim, anchor)).then(|| (anchor.clone(), false))
                    }
                    None => {
                        anchors.push(claim.clone());
                        None
                    }
                }
            } else {
                // A minimum is compared with the minimums when the manifests
                // state any: `PHP ^8.2` against an image tag proves nothing.
                let minimums: Vec<&Stated> = of_tech.iter().filter(|f| f.floor).copied().collect();
                let pool = if claim.floor && !minimums.is_empty() {
                    minimums
                } else {
                    of_tech
                };
                if pool.iter().any(|fact| agrees_with_fact(&claim, fact)) {
                    None
                } else {
                    pool.first().map(|fact| ((*fact).clone(), true))
                }
            };
            let Some((reference, from_manifest)) = reference else {
                continue;
            };
            if !seen.insert((rel.clone(), claim.tech, claim.raw.clone())) {
                continue;
            }
            let against = if from_manifest {
                "the source states"
            } else {
                "the documentation states"
            };
            out.push(Conflict {
                path: rel.clone(),
                message: format!(
                    "Stack version conflict: `{} {}` at {} contradicts {} `{}` at {} — read the manifest and correct the wrong place so every document gives the version in place",
                    label(claim.tech),
                    claim.raw,
                    claim.place,
                    against,
                    reference.raw,
                    reference.place,
                ),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    const COMPOSER: &str = "{\n  \"require\": {\n    \"php\": \"^8.4\",\n    \"symfony/framework-bundle\": \"^8.1\"\n  }\n}\n";

    fn docs() -> Vec<(String, String)> {
        vec![
            (
                "docs/AGENTS.md".into(),
                "**Project:** Docroms Web. Symfony 8.1 + FrankenPHP 1.8 (PHP 8.4), multilingual.\n"
                    .into(),
            ),
            (
                "docs/coding-rules.md".into(),
                "# Coding rules\n\n- Prefer small diffs.\n\n## PHP (Symfony 7.3)\n\n### Tools\n"
                    .into(),
            ),
            (
                "docs/inconsistencies-tech-debt.md".into(),
                "| PHP runtime | PHP ^8.2 per composer.json | aligned |\n".into(),
            ),
        ]
    }

    #[test]
    fn docroms_symfony_7_3_heading_contradicts_the_manifest_and_agents_md() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "composer.json", COMPOSER);
        let found = conflicts(tmp.path(), &docs());
        let symfony = found
            .iter()
            .find(|c| c.path == "docs/coding-rules.md")
            .unwrap_or_else(|| panic!("{found:?}"));
        assert!(
            symfony
                .message
                .contains("`Symfony 7.3` at docs/coding-rules.md:5"),
            "{symfony:?}"
        );
        assert!(symfony.message.contains("composer.json:4"), "{symfony:?}");
        let php = found
            .iter()
            .find(|c| c.path == "docs/inconsistencies-tech-debt.md")
            .unwrap_or_else(|| panic!("{found:?}"));
        assert!(php.message.contains("PHP ^8.2"), "{php:?}");
        assert!(php.message.contains("composer.json:3"), "{php:?}");
        // The entry file agrees with the manifest: nothing to correct there.
        assert!(
            found.iter().all(|c| c.path != "docs/AGENTS.md"),
            "{found:?}"
        );
    }

    #[test]
    fn without_a_manifest_the_documents_are_held_against_each_other() {
        let tmp = tempfile::tempdir().unwrap();
        let found = conflicts(tmp.path(), &docs());
        let symfony = found
            .iter()
            .find(|c| c.path == "docs/coding-rules.md")
            .unwrap_or_else(|| panic!("{found:?}"));
        assert!(symfony.message.contains("docs/AGENTS.md:1"), "{symfony:?}");
        assert!(
            symfony.message.contains("documentation states"),
            "{symfony:?}"
        );
    }

    #[test]
    fn a_consistent_set_of_documents_and_manifests_has_no_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "composer.json", COMPOSER);
        write(
            tmp.path(),
            "docker/frankenphp/Dockerfile",
            "FROM dunglas/frankenphp:1.8-php8.4-alpine\n",
        );
        write(
            tmp.path(),
            ".github/workflows/deploy.yml",
            "jobs:\n  t:\n    steps:\n      - with:\n          php-version: '8.4'\n",
        );
        let docs = vec![
            (
                "docs/AGENTS.md".to_string(),
                "Symfony 8.1 on PHP 8.4\n| PHP | ^8.4 per manifest; image frankenphp:1.8-php8.4-alpine; CI uses PHP 8.4 |\n".to_string(),
            ),
            (
                "docs/coding-rules.md".to_string(),
                "## PHP (Symfony 8.1)\n\nPHP 8 attributes carry the routes.\n".to_string(),
            ),
        ];
        assert_eq!(conflicts(tmp.path(), &docs), Vec::new());
    }

    #[test]
    fn a_version_stated_as_history_or_a_target_is_not_read() {
        let docs = vec![
            ("docs/AGENTS.md".to_string(), "Symfony 8.1\n".to_string()),
            (
                "docs/decisions.md".to_string(),
                "Upgrade from Symfony 6.4 to 8.1 planned.\nSymfony 7.3 → 8.1 done.\nSymfony 7.3 reaches EOL.\n```\nSymfony 5.4\n```\n".to_string(),
            ),
        ];
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(conflicts(tmp.path(), &docs), Vec::new());
    }

    #[test]
    fn a_runtime_files_version_is_a_fact() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".node-version", "22.3.0\n");
        let docs = vec![(
            "docs/AGENTS.md".to_string(),
            "Runs on Node.js 18.\n".to_string(),
        )];
        let found = conflicts(tmp.path(), &docs);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].message.contains(".node-version:1"), "{found:?}");
    }

    #[test]
    fn dated_snapshots_and_sheets_are_not_scanned() {
        assert!(is_scanned("coding-rules.md"));
        assert!(is_scanned("architecture/overview.md"));
        assert!(!is_scanned("reports/audit-2026.md"));
        assert!(!is_scanned("tech-debt/TD-1.md"));
        assert!(!is_scanned("learnings.md"));
    }
}
