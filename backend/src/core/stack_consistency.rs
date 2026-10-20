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
//! Precise before it is broad. Measured on 754 real audit docs, most of what a
//! naive scan flags is not a contradiction: a port next to `php`, a test count,
//! a version quoted as history (`Since Symfony 7.4`), the version a CI job runs
//! (not the runtime), a constraint the stated version satisfies. So a number is
//! a version only right after a technology name, is never read when its line
//! is about change, CI, a tool or a count, and is compared with a manifest
//! constraint the way the package manager would (`>=22` admits 24, `^8.1`
//! admits 8.4, `8.x` admits 8.4).

use std::path::Path;

/// How a version is stated: the version in place, or a constraint on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `8.4`, `8.x`, `8.4.*`, `22.3.0`: the version in place (maybe partial).
    Exact,
    /// `^8.4`: the same major, at least this.
    Caret,
    /// Composer's `~8.1.0`: the same minor; a two-part `~8.1` moves up to the
    /// next major.
    Tilde,
    /// npm's `~18.2`: `>=18.2.0 <18.3.0`, the same minor whatever the length.
    NpmTilde,
    /// `>=22`, `22+`: this or anything later.
    AtLeast,
}

impl Kind {
    fn is_constraint(self) -> bool {
        self != Kind::Exact
    }
}

/// One version a document or a manifest states for a technology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stated {
    /// Lower-case technology key (`php`, `symfony`, `node`…).
    pub tech: &'static str,
    /// `8.4` → `[8, 4]`; `8.x` → `[8]`.
    pub version: Vec<u32>,
    /// The version as written (`^8.4`, `7.3`).
    pub raw: String,
    /// `path:line`.
    pub place: String,
    pub kind: Kind,
    pub origin: Origin,
}

/// Who states a version. The origins are not equal witnesses: a lock file is
/// what is installed, a CI job only what is tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A lock file: the version actually resolved. It wins over its constraint.
    Lock,
    /// A manifest constraint (`composer.json`, `package.json`).
    Declared,
    /// What the project runs on: a Dockerfile, `.node-version`, `.tool-versions`.
    Runtime,
    /// A CI job: what is tested, not the runtime. With no runtime fact it can
    /// vouch for a document; it never contradicts one.
    Ci,
    /// A document.
    Doc,
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
    ("node", "node"),
    ("Python", "python"),
    ("React", "react"),
    ("Vue.js", "vue"),
    ("Vue", "vue"),
    ("Angular", "angular"),
    ("Next.js", "next"),
];

/// A major version above this is not a version of that technology but a port,
/// a count, a year (`PHP 443`, `PHP 24 methods`).
fn plausible(tech: &str, major: u32) -> bool {
    major
        <= match tech {
            "php" => 9,
            "symfony" => 12,
            "laravel" => 15,
            "node" => 40,
            "python" => 3,
            "react" => 25,
            "vue" => 5,
            "angular" => 30,
            "next" => 25,
            _ => 0,
        }
}

/// A line saying one of these talks about change, a target, a claim under
/// review or a date, not the version in place.
const HISTORY_WORDS: &[&str] = &[
    "upgrad",
    "migrat",
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
    "since ",
    "as of ",
    "introduced",
    "added in",
    "available in",
    "ships ",
    "mismatch",
    "discrepan",
    "contradict",
    "inconsisten",
    "drift",
    "→",
    "->",
];

/// A word that makes the line about a CI job, a pipeline, a matrix or an end of
/// life: the version it names is what is tested or retired, not the version in
/// place.
const CI_WORDS: &[&str] = &[
    "ci",
    "cd",
    "jenkins",
    "jenkinsfile",
    "workflow",
    "workflows",
    "pipeline",
    "pipelines",
    "matrix",
    "gitlab",
    "circleci",
    "travis",
    "actions",
    "eol",
];

/// A tool that runs on its own PHP / Node, whatever the project's.
const TOOLS: &[&str] = &[
    "phpstan", "phpunit", "paratest", "psalm", "rector", "eslint", "prettier", "xdebug", "jest",
    "vitest",
];

/// What a number followed by one of these counts (or dates), never a version.
const COUNT_WORDS: &[&str] = &[
    "tests",
    "test",
    "methods",
    "files",
    "lines",
    "errors",
    "warnings",
    "issues",
    "classes",
    "helpers",
    "endpoints",
    "routes",
    "services",
    "rules",
    "cases",
    "specs",
    "checks",
    "jobs",
    "items",
    "findings",
    "hits",
    "assertions",
    "functions",
    "pages",
    "templates",
    "commands",
    "controllers",
    "entities",
    "bytes",
    "kb",
    "mb",
    "ms",
    "seconds",
    "minutes",
    "hours",
    "days",
    "requests",
    "jan",
    "feb",
    "mar",
    "apr",
    "may",
    "jun",
    "jul",
    "aug",
    "sep",
    "oct",
    "nov",
    "dec",
    "january",
    "february",
    "march",
    "april",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
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
        .find(|(display, _)| display.eq_ignore_ascii_case(name))
        .map(|(_, key)| *key)
}

fn words(lower: &str) -> impl Iterator<Item = &str> {
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

/// Whether a line (lower-cased) is about change, CI, a tool or a contradiction
/// someone already noted: what it says about a version is not what is in place.
fn off_topic(lower: &str) -> bool {
    HISTORY_WORDS.iter().any(|word| lower.contains(word))
        || lower.contains("php-cs-fixer")
        || words(lower).any(|word| CI_WORDS.contains(&word) || TOOLS.contains(&word))
}

/// `8.4` → `[8, 4]`, `8.x` / `8.4.*` → the numbers before the wildcard.
fn parse_number(number: &str) -> Vec<u32> {
    number
        .split('.')
        .map_while(|part| part.parse().ok())
        .collect()
}

/// The version a text begins with (`^8.4`, `>=22`, `8.x`, `22+`), none when
/// what follows shows the number is not one (`443:443`, `2026-04`, `24
/// methods`, `8.4abc`).
fn leading_version(text: &str) -> Option<(Kind, Vec<u32>, String)> {
    static LEAD: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r"^(\^|~>?|>=|≥|=)?\s*[vV]?(\d{1,3}(?:\.(?:\d+|[xX*]))*)(\+)?")
            .expect("regex")
    });
    let c = LEAD.captures(text)?;
    let whole = c.get(0)?;
    let number = c.get(2)?.as_str();
    let after = &text[whole.end()..];
    let mut next = after.chars();
    let first = next.next();
    let second = next.next();
    if first.is_some_and(|ch| ch.is_alphanumeric() || ch == '_') {
        return None;
    }
    if matches!(first, Some(':' | '/' | '-' | '.')) && second.is_some_and(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    let following = after
        .trim_start_matches(|ch: char| !ch.is_alphanumeric() && ch != '(')
        .split(|ch: char| !ch.is_alphabetic())
        .next()
        .unwrap_or("")
        .to_lowercase();
    if COUNT_WORDS.contains(&following.as_str()) {
        return None;
    }
    let version = parse_number(number);
    let kind = match c.get(1).map(|op| op.as_str()) {
        Some("^") => Kind::Caret,
        Some(op) if op.starts_with('~') => Kind::Tilde,
        Some(">=" | "≥") => Kind::AtLeast,
        _ if c.get(3).is_some() => Kind::AtLeast,
        _ => Kind::Exact,
    };
    let raw = format!(
        "{}{}{}",
        c.get(1).map_or("", |op| op.as_str()),
        number,
        c.get(3).map_or("", |plus| plus.as_str())
    );
    Some((kind, version, raw))
}

fn claim(tech: &'static str, place: String, text: &str, origin: Origin) -> Option<Stated> {
    let (kind, version, raw) = leading_version(text.trim())?;
    (!version.is_empty() && plausible(tech, version[0])).then_some(Stated {
        tech,
        version,
        raw,
        place,
        kind,
        origin,
    })
}

/// One statement per alternative of a constraint (`^7.4|^8.0`, `>=8.2 <9`,
/// `^20 || ^22`): the first lower bound of each, never an upper bound.
///
/// `npm` is the ecosystem: a two-part `~18.2` is not read the way Composer's is.
fn constraint(
    tech: &'static str,
    place: &str,
    value: &str,
    origin: Origin,
    npm: bool,
) -> Vec<Stated> {
    value
        .split('|')
        .filter(|alt| !alt.trim().is_empty())
        .filter_map(|alt| {
            alt.split(|c: char| c.is_whitespace() || c == ',')
                .filter(|token| !token.is_empty() && !token.starts_with('<'))
                .find_map(|token| claim(tech, place.to_string(), token, origin))
        })
        .map(|mut stated| {
            if npm && stated.kind == Kind::Tilde {
                stated.kind = Kind::NpmTilde;
            }
            stated
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
            out.extend(constraint(
                "php",
                &place("php"),
                php,
                Origin::Declared,
                false,
            ));
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
            out.extend(constraint(
                "symfony",
                &place(name),
                version,
                Origin::Declared,
                false,
            ));
        }
        if let Some(version) = require
            .and_then(|r| r.get("laravel/framework"))
            .and_then(|v| v.as_str())
        {
            out.extend(constraint(
                "laravel",
                &place("laravel/framework"),
                version,
                Origin::Declared,
                false,
            ));
        }
        if let Some(pin) = value
            .pointer("/extra/symfony/require")
            .and_then(|v| v.as_str())
        {
            out.extend(constraint(
                "symfony",
                &place("require"),
                pin,
                Origin::Declared,
                false,
            ));
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
                    out.extend(claim(tech, place, version, Origin::Lock));
                }
            }
        }
    }
}

/// The npm package behind each tracked technology.
const NPM_PACKAGES: &[(&str, &str)] = &[
    ("react", "react"),
    ("vue", "vue"),
    ("next", "next"),
    ("@angular/core", "angular"),
];

fn package_facts(project: &Path, out: &mut Vec<Stated>) {
    let Some((text, value)) = json(project, "package.json") else {
        return;
    };
    let place = |key: &str| format!("package.json:{}", line_of(&text, &format!("\"{key}\"")));
    if let Some(node) = value.pointer("/engines/node").and_then(|v| v.as_str()) {
        out.extend(constraint(
            "node",
            &place("node"),
            node,
            Origin::Declared,
            true,
        ));
    }
    for (dep, tech) in NPM_PACKAGES {
        for group in ["dependencies", "devDependencies"] {
            if let Some(version) = value
                .get(group)
                .and_then(|g| g.get(*dep))
                .and_then(|v| v.as_str())
            {
                out.extend(constraint(
                    tech,
                    &place(dep),
                    version,
                    Origin::Declared,
                    true,
                ));
            }
        }
    }
}

/// The versions the JS lock files resolved for the tracked packages: only the
/// project's own (top-level) install, never a nested copy a dependency pulled.
fn js_lock_facts(project: &Path, out: &mut Vec<Stated>) {
    if let Some((text, value)) = json(project, "package-lock.json") {
        for (dep, tech) in NPM_PACKAGES {
            // lockfileVersion 2/3 list `node_modules/<name>`; 1 lists `dependencies`.
            let version = value
                .pointer(&format!(
                    "/packages/node_modules~1{}/version",
                    dep.replace('/', "~1")
                ))
                .or_else(|| {
                    value
                        .get("dependencies")
                        .and_then(|d| d.get(*dep))
                        .and_then(|d| d.get("version"))
                })
                .and_then(|v| v.as_str());
            if let Some(version) = version {
                let at = line_of(&text, &format!("\"node_modules/{dep}\""));
                out.extend(claim(
                    tech,
                    format!("package-lock.json:{at}"),
                    version,
                    Origin::Lock,
                ));
            }
        }
    }
    if let Some(text) = read(project, "yarn.lock") {
        // `react@^18.2.0, react@^18.0.0:` then `  version "18.2.0"`.
        let lines: Vec<&str> = text.lines().collect();
        for (dep, tech) in NPM_PACKAGES {
            for (n, line) in lines.iter().enumerate() {
                let head = line.trim_start_matches('"');
                let is_entry = !line.starts_with(' ')
                    && line.trim_end().ends_with(':')
                    && head.starts_with(&format!("{dep}@"));
                if !is_entry {
                    continue;
                }
                let version = lines[n + 1..]
                    .iter()
                    .take_while(|l| l.starts_with(' '))
                    .find_map(|l| {
                        l.trim()
                            .strip_prefix("version")
                            .map(|v| v.trim().trim_matches(|c| matches!(c, '"' | ':')).trim())
                    });
                if let Some(version) = version {
                    out.extend(claim(
                        tech,
                        format!("yarn.lock:{}", n + 1),
                        version,
                        Origin::Lock,
                    ));
                }
            }
        }
    }
    if let Some(text) = read(project, "pnpm-lock.yaml") {
        // `importers:` come first; `packages:` / `snapshots:` hold nested copies.
        let lines: Vec<&str> = text
            .lines()
            .take_while(|l| !l.starts_with("packages:") && !l.starts_with("snapshots:"))
            .collect();
        for (dep, tech) in NPM_PACKAGES {
            for (n, line) in lines.iter().enumerate() {
                let unquoted = line.trim().replace('\'', "");
                let Some(rest) = unquoted.strip_prefix(&format!("{dep}:")) else {
                    continue;
                };
                let rest = rest.trim();
                let version = if rest.is_empty() {
                    lines[n + 1..].iter().take(3).find_map(|l| {
                        l.trim()
                            .strip_prefix("version:")
                            .map(|v| v.trim().trim_matches('\''))
                    })
                } else {
                    Some(rest.trim_matches('\''))
                };
                if let Some(version) = version {
                    out.extend(claim(
                        tech,
                        format!("pnpm-lock.yaml:{}", n + 1),
                        version,
                        Origin::Lock,
                    ));
                }
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
            out.extend(claim(
                tech,
                format!("{file}:1"),
                text.trim(),
                Origin::Runtime,
            ));
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
                out.extend(claim(
                    tech,
                    format!(".tool-versions:{}", n + 1),
                    version,
                    Origin::Runtime,
                ));
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
                out.extend(claim(tech, place.clone(), tag, Origin::Runtime));
            }
            if let Some(at) = tag.find("php") {
                if tech != Some("php") {
                    out.extend(claim("php", place, &tag[at + 3..], Origin::Runtime));
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
            out.extend(claim(
                tech,
                place,
                value.trim().trim_matches('"'),
                Origin::Runtime,
            ));
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
            let place = format!("{rel}:{}", n + 1);
            for item in value
                .split(|c: char| matches!(c, '[' | ']' | ',' | '\'' | '"') || c.is_whitespace())
            {
                out.extend(claim(tech, place.clone(), item, Origin::Ci));
            }
        }
    }
}

/// What the project's own files say about its stack.
pub fn manifest_facts(project: &Path) -> Vec<Stated> {
    let mut out = Vec::new();
    composer_facts(project, &mut out);
    package_facts(project, &mut out);
    js_lock_facts(project, &mut out);
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

/// The technology a table's first cell is about: its name alone, optionally
/// followed by `version` / `runtime`… (`| PHP runtime | … |`), never
/// `PHP unit/component tests`.
fn subject_of(cell: &str) -> Option<&'static str> {
    let without_note = cell.split('(').next().unwrap_or("");
    let mut words = plain(without_note).split_whitespace();
    let tech = tech_key(plain(words.next()?))?;
    match words.next() {
        None => Some(tech),
        Some(second)
            if words.next().is_none()
                && matches!(
                    second.to_lowercase().as_str(),
                    "version" | "runtime" | "framework" | "language" | "core" | "engine"
                ) =>
        {
            Some(tech)
        }
        Some(_) => None,
    }
}

/// The statements of one document, in line order.
pub fn doc_claims(rel: &str, content: &str) -> Vec<Stated> {
    static TECH: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(
            r"\b(PHP|php|Symfony|Laravel|Node\.js|Node|node|Python|React|Vue\.js|Vue|Angular|Next\.js)\b\s*:?\s*",
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
            let scanned = cells
                .iter()
                .take(2)
                .copied()
                .collect::<Vec<_>>()
                .join(" | ");
            if off_topic(&scanned.to_lowercase()) {
                continue;
            }
            if let (Some(tech), Some(cell)) =
                (cells.first().and_then(|c| subject_of(c)), cells.get(1))
            {
                // `| PHP | 8.4 |`, not `| PHP | see composer.json:7 |`.
                if !TECH.is_match(cell) {
                    out.extend(claim(tech, place.clone(), plain(cell), Origin::Doc));
                }
            }
            scanned
        } else {
            if off_topic(&text.to_lowercase()) {
                continue;
            }
            text
        };
        for c in TECH.captures_iter(&scanned) {
            let (Some(whole), Some(name)) = (c.get(0), c.get(1)) else {
                continue;
            };
            // `FrankenPHP 1.8`, `foo/php 8`, `--php 8`: not the technology.
            let before = scanned[..name.start()].chars().next_back();
            if before.is_some_and(|ch| matches!(ch, '/' | '-' | '_' | '.' | '@')) {
                continue;
            }
            let Some(tech) = tech_key(name.as_str()) else {
                continue;
            };
            out.extend(claim(
                tech,
                place.clone(),
                &scanned[whole.end()..],
                Origin::Doc,
            ));
        }
    }
    out
}

// ─── Comparison ──────────────────────────────────────────────────────────

/// `have` is at least `bound`, over the digits both state.
fn at_least(have: &[u32], bound: &[u32]) -> bool {
    let n = have.len().min(bound.len());
    have[..n] >= bound[..n]
}

/// Whether the version in place `have` is allowed by `kind bound`, the way a
/// package manager reads it.
fn satisfies(have: &[u32], kind: Kind, bound: &[u32]) -> bool {
    if have.is_empty() || bound.is_empty() {
        return true;
    }
    let n = have.len().min(bound.len());
    match kind {
        Kind::Exact => have[..n] == bound[..n],
        Kind::AtLeast => at_least(have, bound),
        Kind::Caret => {
            have[0] == bound[0]
                && at_least(have, bound)
                // `^0.3` stays on its minor.
                && (bound[0] != 0 || n < 2 || have[1] == bound[1])
        }
        Kind::Tilde => {
            have[0] == bound[0]
                && at_least(have, bound)
                // `~8.1.0` stays on its minor; `~8.1` may move to 8.9.
                && (bound.len() < 3 || have.len() < 2 || have[1] == bound[1])
        }
        // npm: `~18.2` is `>=18.2.0 <18.3.0`, `~18` is `>=18.0.0 <19.0.0`.
        Kind::NpmTilde => {
            have[0] == bound[0]
                && at_least(have, bound)
                && (bound.len() < 2 || have.len() < 2 || have[1] == bound[1])
        }
    }
}

/// Whether one statement leaves room for the other.
fn agrees(claim: &Stated, other: &Stated) -> bool {
    match (claim.kind, other.kind) {
        (Kind::Exact, Kind::Exact) => satisfies(&claim.version, Kind::Exact, &other.version),
        (Kind::Exact, kind) => satisfies(&claim.version, kind, &other.version),
        (kind, Kind::Exact) => satisfies(&other.version, kind, &claim.version),
        // Two constraints name the same requirement or they differ.
        _ => {
            let n = claim.version.len().min(other.version.len());
            n > 0 && claim.version[..n] == other.version[..n]
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
            let runtime: Vec<&Stated> = of_tech
                .iter()
                .filter(|f| f.origin != Origin::Ci)
                .copied()
                .collect();
            // (the statement it contradicts, whether it is a manifest's)
            let reference: Option<(Stated, bool)> = if runtime.is_empty() {
                // Only a CI job (or nothing) states it: the documents decide
                // among themselves.
                if of_tech.iter().any(|fact| agrees(&claim, fact)) {
                    continue;
                }
                match anchors.iter().find(|a| a.tech == claim.tech) {
                    Some(anchor) => (!agrees(&claim, anchor)).then(|| (anchor.clone(), false)),
                    None => {
                        anchors.push(claim.clone());
                        None
                    }
                }
            } else {
                // What is installed speaks before what is allowed: a
                // permissive `^8.1` must not hide a lock at 8.1.0 behind a
                // document saying 8.4. A constraint is compared with the
                // constraints when the manifests state any (`PHP ^8.2` against
                // an image tag proves nothing). A CI job is no witness of the
                // runtime: it never vouches for a runtime claim.
                let constraints: Vec<&Stated> = runtime
                    .iter()
                    .filter(|f| f.kind.is_constraint())
                    .copied()
                    .collect();
                let locked: Vec<&Stated> = runtime
                    .iter()
                    .filter(|f| f.origin == Origin::Lock)
                    .copied()
                    .collect();
                let pool: Vec<&Stated> = if claim.kind.is_constraint() && !constraints.is_empty() {
                    constraints
                } else if !locked.is_empty() {
                    locked
                } else {
                    runtime.clone()
                };
                if pool.iter().any(|fact| agrees(&claim, fact)) {
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

// ─── Prevention: the facts handed to the steps that state versions ───────

/// The most a facts block may weigh: it rides in three step prompts.
const FACTS_BUDGET: usize = 1_800;

/// The versions of one origin, deduplicated and capped: `^8.1, ^8.4`.
fn listed(facts: &[&Stated], origin: Origin) -> Option<String> {
    let mut raws: Vec<&str> = Vec::new();
    let mut first = None;
    for fact in facts.iter().filter(|f| f.origin == origin) {
        if !raws.contains(&fact.raw.as_str()) {
            raws.push(&fact.raw);
        }
        first.get_or_insert(&fact.place);
    }
    raws.truncate(3);
    first.map(|place| format!("{} ({place})", raws.join(", ")))
}

/// What the project's manifests, lock files, runtime files and CI say about
/// its frameworks, languages and runtimes, worded for the steps that write
/// those versions (the index, the coding rules, the final review). Computed,
/// not asked: the agent copies it instead of recalling a version, which is how
/// `Symfony 7.3` ended up under a project on 8.1. `None` when no file states
/// any.
pub fn facts_block(project: &Path) -> Option<String> {
    let facts = manifest_facts(project);
    let mut lines = Vec::new();
    for key in [
        "php", "symfony", "laravel", "node", "python", "react", "vue", "angular", "next",
    ] {
        let of_tech: Vec<&Stated> = facts.iter().filter(|f| f.tech == key).collect();
        let parts: Vec<String> = [
            ("locked", Origin::Lock),
            ("declared", Origin::Declared),
            ("runtime", Origin::Runtime),
            ("CI-tested", Origin::Ci),
        ]
        .into_iter()
        .filter_map(|(name, origin)| listed(&of_tech, origin).map(|text| format!("{name} {text}")))
        .collect();
        if !parts.is_empty() {
            lines.push(format!("- {}: {}", label(key), parts.join("; ")));
        }
    }
    if lines.is_empty() {
        return None;
    }
    let mut block = String::from(
        "## Stack facts (computed by Kronn from the manifests, lock files and runtime files)\n\
State every framework, language and runtime version from these facts: the locked version when there is one, else the declared constraint. Never infer, recall or copy another version, and give the same one in every document. A CI-tested version is what a job runs, not the runtime.\n",
    );
    for line in lines {
        if block.len() + line.len() + 1 > FACTS_BUDGET {
            break;
        }
        block.push_str(&line);
        block.push('\n');
    }
    Some(block)
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

    /// The conflicts of one document body against one manifest.
    fn found_in(manifest: (&str, &str), body: &str) -> Vec<Conflict> {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), manifest.0, manifest.1);
        conflicts(
            tmp.path(),
            &[("docs/overview.md".to_string(), body.to_string())],
        )
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
    fn the_three_measured_symfony_7_3_headings_are_found_against_a_caret_constraint() {
        // coding-rules.md:15 of docroms v1-r2, v2-r1 and v2-r2, composer.json:19.
        let composer = "{\n\"name\": \"docroms\",\n\"require\": {\n    \"php\": \">=8.4\",\n    \"symfony/framework-bundle\": \"^8.1\"\n}\n}\n";
        for heading in [
            "## PHP (Symfony 7.3)",
            "## PHP (Symfony 7.3).",
            "## Symfony 7.3",
        ] {
            let found = found_in(("composer.json", composer), &format!("{heading}\n"));
            assert_eq!(found.len(), 1, "{heading}: {found:?}");
            assert!(found[0].message.contains("`Symfony 7.3`"), "{found:?}");
            assert!(found[0].message.contains("composer.json:5"), "{found:?}");
        }
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
                "Symfony 8.1 on PHP 8.4\n| PHP | ^8.4 per manifest; image frankenphp:1.8-php8.4-alpine |\n".to_string(),
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

    // ── Measured false positives: the real lines of the A/B and matrix docs ──

    #[test]
    fn a_port_or_a_count_next_to_a_technology_is_not_a_version() {
        for line in [
            // docroms architecture/overview.md:11, debug-operations.md:32.
            "| `php` | `443` (HTTPS) | FrankenPHP (worker mode) + bundled Caddy, PHP 8.4 image |",
            "| `php` | `443:443` | HTTPS site server — FrankenPHP (worker mode) + bundled Caddy; serves `public/` |",
            // africanews testing-quality.md:36.
            "| PHP unit/component tests | 24 methods: 15 DFP builder, 4 Adobe Analytics, 3 video extension |",
            "Xdebug listens on PHP :9003 for the IDE.",
            "PHP 24 methods are covered; PHP 12 files changed on 2026-04-30.",
        ] {
            let found = found_in(("composer.json", COMPOSER), &format!("{line}\n"));
            assert_eq!(found, Vec::new(), "{line}");
        }
    }

    #[test]
    fn a_historical_mention_is_not_the_version_in_place() {
        for line in [
            // docroms production-infrastructure.md:97.
            "Since **Symfony 7.4**, `symfony/runtime` ships `FrankenPhpWorkerRunner` and",
            "Symfony 7.4 deprecated the old runner.",
            "The `--worker` flag was introduced in Symfony 7.3.",
        ] {
            let found = found_in(("composer.json", COMPOSER), &format!("{line}\n"));
            assert_eq!(found, Vec::new(), "{line}");
        }
    }

    #[test]
    fn what_a_ci_job_runs_is_not_the_runtime() {
        let package = "{\n  \"engines\": {\n    \"node\": \">=22\"\n  }\n}\n";
        for line in [
            // apollo docs, six repos' worth of the same shape.
            "| Node.js (Jenkinsfile) | 18 | Behind stable | Medium — Jenkins pipeline uses EOL Node 18; package.json requires ≥22 |",
            "| Node.js (CI Quality) | 20 | Behind stable | Medium — CI Quality test workflow uses Node 20; production requires ≥22 |",
            "| TD-20261019-node-version-ci-mismatch | CI tests use Node.js 20 but package.json requires >=22; version compatibility gap. | CI/Infra |",
            "      - Nodejs version: Node 22 pinned in CD workflows, Node 20 and 18 in other workflows",
            "| Node.js (CI) | 20 | Supported | Low — LTS, maintained until 2026-04-30 |",
            "| Node version (CI) | 20.x | .github/workflows/CI_Quality-test.yaml |",
            "1. **Quality (CI)**: Triggered by PR label `ci-test`; runs linting (ESLint), tests (Jest), and build checks on Node 20.",
            "| `.github/workflows/CI_Quality-test.yaml` | CI pipeline: runs lint and test on `ci-test` label. Runs on Node 20. |",
            "| Node version (CI test) | 20 |",
            "TD: CI tests on Node 20 but package.json requires Node >=22; deployment uses Node 22.",
        ] {
            let found = found_in(("package.json", package), &format!("{line}\n"));
            assert_eq!(found, Vec::new(), "{line}");
        }
    }

    #[test]
    fn a_tool_running_on_its_own_php_is_not_the_project_runtime() {
        // africanews TESTING.md:183 — the project runs PHP 7.3.
        let found = found_in(
            ("docker/php/Dockerfile", "FROM php:7.3-fpm\n"),
            "installed, no `phpstan*.neon` config, php-cs-fixer v2 on PHP 8.3, no root\n",
        );
        assert_eq!(found, Vec::new());
    }

    #[test]
    fn a_ci_matrix_never_contradicts_a_document_and_vouches_only_without_a_runtime_fact() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            ".github/workflows/ci.yml",
            "jobs:\n  t:\n    strategy:\n      matrix:\n        php-version: [8.3, 8.4]\n",
        );
        write(tmp.path(), "composer.json", COMPOSER);
        let docs = vec![(
            "docs/AGENTS.md".to_string(),
            "PHP 8.4 in production.\nPHP 8.3 in production.\n".to_string(),
        )];
        let found = conflicts(tmp.path(), &docs);
        // 8.4 matches ^8.4. 8.3 is in the matrix but the manifest states the
        // runtime: a job that tests 8.3 does not make 8.3 the runtime.
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].message.contains("`PHP 8.3`"), "{found:?}");
        assert!(found[0].message.contains("composer.json:3"), "{found:?}");
        // With no composer.json the matrix alone states no runtime.
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            ".github/workflows/ci.yml",
            "jobs:\n  t:\n    strategy:\n      matrix:\n        php-version: [8.3]\n",
        );
        let found = conflicts(
            tmp.path(),
            &[(
                "docs/AGENTS.md".to_string(),
                "PHP 8.4 runs it.\n".to_string(),
            )],
        );
        assert_eq!(found, Vec::new(), "{found:?}");
    }

    // ── Constraints are compared the way the package manager would ──

    #[test]
    fn a_constraint_is_satisfied_semantically() {
        let node = "{\"engines\": {\"node\": \">=22\"}}";
        let php = "{\"require\": {\"php\": \"^8.1\"}}";
        let wildcard = "{\"require\": {\"php\": \"8.4.*\"}}";
        for (manifest, doc) in [
            (("package.json", node), "Node.js 24 in production."),
            (("package.json", node), "Node 22.14.0 in production."),
            (("package.json", node), "Node 22+ is required."),
            (("composer.json", php), "PHP 8.4 in production."),
            (("composer.json", php), "PHP 8 in production."),
            (("composer.json", php), "PHP >=8.1 is required."),
            (("composer.json", wildcard), "PHP 8.x in production."),
            (("composer.json", wildcard), "PHP 8.4.2 in production."),
        ] {
            assert_eq!(found_in(manifest, &format!("{doc}\n")), Vec::new(), "{doc}");
        }
        // `lts/*` and `latest` state no version at all.
        assert_eq!(
            found_in(
                ("package.json", "{\"engines\": {\"node\": \"lts/*\"}}"),
                "Node 22 in production.\n"
            ),
            Vec::new()
        );
    }

    #[test]
    fn a_stated_version_the_constraint_forbids_is_a_conflict() {
        let node = "{\"engines\": {\"node\": \">=22\"}}";
        let tilde = "{\"require\": {\"php\": \"~8.1.0\"}}";
        let patch = "{\"require\": {\"php\": \"^8.1.2\"}}";
        let major = "{\"require\": {\"symfony/framework-bundle\": \"^8.1\"}}";
        for (manifest, doc) in [
            (("package.json", node), "Node.js 18 in production."),
            (("composer.json", tilde), "PHP 8.4 in production."),
            (("composer.json", patch), "PHP 8.1.1 in production."),
            (("composer.json", major), "Symfony 7.3 in production."),
            (("composer.json", patch), "PHP ^8.4 is required."),
        ] {
            assert_eq!(found_in(manifest, &format!("{doc}\n")).len(), 1, "{doc}");
        }
    }

    // ── Round 2: the lock wins, CI is no runtime witness, tilde per ecosystem ──

    const SYMFONY_CARET: &str =
        "{\n  \"require\": {\n    \"symfony/framework-bundle\": \"^8.1\"\n  }\n}\n";
    const SYMFONY_LOCK: &str = "{\n  \"packages\": [\n    {\n      \"name\": \"symfony/framework-bundle\",\n      \"version\": \"v8.1.0\"\n    }\n  ]\n}\n";

    fn with_files(files: &[(&str, &str)], doc: &str) -> Vec<Conflict> {
        let tmp = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            write(tmp.path(), rel, body);
        }
        conflicts(
            tmp.path(),
            &[("docs/overview.md".to_string(), format!("{doc}\n"))],
        )
    }

    #[test]
    fn a_permissive_constraint_does_not_hide_a_contradicting_lock() {
        // extra-fixtures/lock_disagrees_with_doc_while_constraint_allows:
        // `^8.1` admits 8.4, but composer.lock pins 8.1.0.
        let files = [
            ("composer.json", SYMFONY_CARET),
            ("composer.lock", SYMFONY_LOCK),
        ];
        let found = with_files(&files, "Symfony 8.4");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].message.contains("`Symfony 8.4`"), "{found:?}");
        assert!(
            found[0].message.contains("`8.1.0` at composer.lock:"),
            "{found:?}"
        );
        // extra-fixtures/lock_matches_doc: the control stays quiet, so does a
        // partial or a patch-level statement of the locked version.
        for doc in ["Symfony 8.1", "Symfony 8", "Symfony 8.1.0", "Symfony 8.x"] {
            assert_eq!(with_files(&files, doc), Vec::new(), "{doc}");
        }
        // Without the lock the constraint is all there is.
        assert_eq!(
            with_files(&[("composer.json", SYMFONY_CARET)], "Symfony 8.4"),
            Vec::new()
        );
        // A claim that is itself a constraint is held to the constraints.
        assert_eq!(with_files(&files, "Symfony ^8.1 is required"), Vec::new());
        assert_eq!(with_files(&files, "Symfony ^7.3 is required").len(), 1);
    }

    #[test]
    fn a_ci_matrix_does_not_validate_a_runtime_claim() {
        // extra-fixtures/ci_agreement_masks_production_disagreement: engines
        // `>=22`, a CI job on Node 20, the doc says the runtime is Node.js 20.
        let package = "{\n  \"engines\": {\n    \"node\": \">=22\"\n  }\n}\n";
        let ci = ".github/workflows/test.yml";
        let job = "jobs:\n  t:\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 20\n";
        let files = [("package.json", package), (ci, job)];
        let found = with_files(&files, "Node.js 20");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].message.contains("package.json:3"), "{found:?}");
        // The runtime the manifest allows is still fine, CI or not.
        assert_eq!(with_files(&files, "Node.js 22"), Vec::new());
        assert_eq!(with_files(&files, "Node.js 24"), Vec::new());
        // A line that says it is about CI is skipped, not flagged.
        assert_eq!(with_files(&files, "CI runs Node.js 20"), Vec::new());
    }

    #[test]
    fn tilde_follows_the_ecosystem_that_wrote_it() {
        // extra-fixtures/npm_two_component_tilde: npm `~18.2` is >=18.2.0 <18.3.0.
        let npm = "{\n  \"dependencies\": {\n    \"react\": \"~18.2\"\n  }\n}\n";
        assert_eq!(
            with_files(&[("package.json", npm)], "React 18.2"),
            Vec::new()
        );
        assert_eq!(
            with_files(&[("package.json", npm)], "React 18.2.4"),
            Vec::new()
        );
        let found = with_files(&[("package.json", npm)], "React 18.3");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].message.contains("package.json:3"), "{found:?}");
        // `~18` is >=18.0.0 <19.0.0.
        let npm_major = "{\"dependencies\": {\"react\": \"~18\"}}";
        assert_eq!(
            with_files(&[("package.json", npm_major)], "React 18.3"),
            Vec::new()
        );
        assert_eq!(
            with_files(&[("package.json", npm_major)], "React 19").len(),
            1
        );
        // extra-fixtures/composer_two_component_tilde: Composer `~8.1` keeps
        // its meaning, `>=8.1 <9.0`.
        let composer = "{\"require\": {\"php\": \"~8.1\"}}";
        assert_eq!(
            with_files(&[("composer.json", composer)], "PHP 8.4"),
            Vec::new()
        );
        assert_eq!(
            with_files(&[("composer.json", composer)], "PHP 9.0").len(),
            1
        );
    }

    #[test]
    fn the_js_lock_files_state_the_resolved_top_level_version() {
        let package = "{\"dependencies\": {\"react\": \"^18.0.0\"}}";
        let npm_lock = "{\n  \"lockfileVersion\": 3,\n  \"packages\": {\n    \"node_modules/react\": {\n      \"version\": \"18.2.0\"\n    }\n  }\n}\n";
        let yarn_lock = "# yarn lockfile v1\n\nreact@^18.0.0:\n  version \"18.2.0\"\n  resolved \"https://x\"\n\nother@^1:\n  version \"1.0.0\"\n";
        let pnpm_lock = "lockfileVersion: '9.0'\n\nimporters:\n  .:\n    dependencies:\n      react:\n        specifier: ^18.0.0\n        version: 18.2.0\n\npackages:\n  react@17.0.2:\n    resolution: {}\nsnapshots:\n  foo@1.0.0:\n    dependencies:\n      react: 17.0.2\n";
        for (name, body) in [
            ("package-lock.json", npm_lock),
            ("yarn.lock", yarn_lock),
            ("pnpm-lock.yaml", pnpm_lock),
        ] {
            let files = [("package.json", package), (name, body)];
            assert_eq!(with_files(&files, "React 18.2"), Vec::new(), "{name}");
            let found = with_files(&files, "React 18.4");
            assert_eq!(found.len(), 1, "{name}: {found:?}");
            assert!(found[0].message.contains(&format!("{name}:")), "{found:?}");
        }
    }

    #[test]
    fn the_facts_block_lists_lock_constraint_runtime_and_ci() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "composer.json", COMPOSER);
        write(tmp.path(), "composer.lock", SYMFONY_LOCK);
        write(
            tmp.path(),
            "Dockerfile",
            "FROM dunglas/frankenphp:1.8-php8.4-alpine\n",
        );
        write(
            tmp.path(),
            ".github/workflows/ci.yml",
            "jobs:\n  t:\n    strategy:\n      matrix:\n        php-version: [8.3, 8.4]\n",
        );
        write(
            tmp.path(),
            "package.json",
            "{\"engines\": {\"node\": \">=22\"}}",
        );
        let block = facts_block(tmp.path()).expect("facts");
        assert!(block.contains("Never infer"), "{block}");
        let symfony = block.lines().find(|l| l.starts_with("- Symfony")).unwrap();
        assert!(
            symfony.contains("locked 8.1.0 (composer.lock:"),
            "{symfony}"
        );
        assert!(
            symfony.contains("declared ^8.1 (composer.json:"),
            "{symfony}"
        );
        let php = block.lines().find(|l| l.starts_with("- PHP")).unwrap();
        assert!(php.contains("declared ^8.4 (composer.json:3)"), "{php}");
        assert!(php.contains("runtime 8.4 (Dockerfile:1)"), "{php}");
        assert!(
            php.contains("CI-tested 8.3, 8.4 (.github/workflows/ci.yml:"),
            "{php}"
        );
        assert!(
            block.contains("- Node.js: declared >=22 (package.json:"),
            "{block}"
        );
        assert!(block.len() <= FACTS_BUDGET, "{} bytes", block.len());
        // No file states a stack: no block.
        assert_eq!(facts_block(tempfile::tempdir().unwrap().path()), None);
    }

    #[test]
    fn version_contexts_are_php_8_4_caret_colon_and_lowercase_node() {
        let found = found_in(
            ("composer.json", COMPOSER),
            "PHP 8.3 here.\nphp: 8.2\nPHP ^8.1\n",
        );
        assert_eq!(found.len(), 3, "{found:?}");
        let found = found_in(
            ("package.json", "{\"engines\": {\"node\": \">=22\"}}"),
            "Install node 18 first.\n",
        );
        assert_eq!(found.len(), 1, "{found:?}");
    }
}
