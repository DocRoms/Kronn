//! KT-1387 — the commands the audit docs tell a reader to run must exist.
//!
//! A runbook that says `make yarn-install` or `yarn install` for a repository
//! with no such Makefile target and no `package.json` sends the next agent on a
//! failing command. [`ProjectCommands`] reads what the repository really
//! defines (Makefile targets, `package.json` and `composer.json` scripts) and
//! [`ProjectCommands::problems`] holds every command a document cites in code
//! against it. Only commands written as code (a fenced line or an inline span)
//! that start with the tool are judged: prose and commands run inside another
//! container or host (`docker …`, `ssh …`) are not Kronn's to verify.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 20_000;
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    ".kronn",
    "node_modules",
    "vendor",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".venv",
    "venv",
    "__pycache__",
];
/// How many known names a message lists to help the correction.
const SHOWN_NAMES: usize = 12;

/// What the repository defines, gathered once per gate run.
#[derive(Debug, Default)]
pub(crate) struct ProjectCommands {
    makefiles: usize,
    make_targets: BTreeSet<String>,
    make_patterns: Vec<(String, String)>,
    /// A target name built by make itself (`$(…)`): the list cannot be closed.
    make_dynamic: bool,
    packages: usize,
    package_scripts: BTreeSet<String>,
    package_deps: BTreeSet<String>,
    composers: usize,
    composer_scripts: BTreeSet<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Make,
    Yarn,
    Npm,
    Pnpm,
    Composer,
}

impl Tool {
    fn of(token: &str) -> Option<Self> {
        match token {
            "make" => Some(Self::Make),
            "yarn" => Some(Self::Yarn),
            "npm" => Some(Self::Npm),
            "pnpm" => Some(Self::Pnpm),
            "composer" => Some(Self::Composer),
            _ => None,
        }
    }
}

impl ProjectCommands {
    pub(crate) fn scan(project: &Path) -> Self {
        let mut found = Self::default();
        let mut budget = MAX_ENTRIES;
        found.walk(project, 0, &mut budget);
        found
    }

    fn walk(&mut self, dir: &Path, depth: usize, budget: &mut usize) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_dir() {
                if depth < MAX_DEPTH && !SKIPPED_DIRS.contains(&name.as_str()) {
                    self.walk(&entry.path(), depth + 1, budget);
                }
            } else if kind.is_file() {
                self.read_file(&entry.path(), &name);
            }
        }
    }

    fn read_file(&mut self, path: &Path, name: &str) {
        let is_makefile =
            matches!(name, "Makefile" | "makefile" | "GNUmakefile") || name.ends_with(".mk");
        if !is_makefile && name != "package.json" && name != "composer.json" {
            return;
        }
        let Ok(body) = fs::read_to_string(path) else {
            return;
        };
        match name {
            "package.json" => self.read_package(&body),
            "composer.json" => self.read_composer(&body),
            _ => self.read_makefile(&body),
        }
    }

    fn read_makefile(&mut self, body: &str) {
        self.makefiles += 1;
        for line in body.lines() {
            if line.starts_with(['\t', ' ', '#', '.']) {
                continue;
            }
            let Some(colon) = line.find(':') else {
                continue;
            };
            // `NAME := value` and `NAME = a:b` define variables, not rules.
            if line[colon..].starts_with(":=") || line[..colon].contains('=') {
                continue;
            }
            for target in line[..colon].split_whitespace() {
                if target.contains("$(") || target.contains("${") {
                    self.make_dynamic = true;
                } else if let Some((prefix, suffix)) = target.split_once('%') {
                    self.make_patterns
                        .push((prefix.to_string(), suffix.to_string()));
                } else {
                    self.make_targets.insert(target.to_string());
                }
            }
        }
    }

    fn read_package(&mut self, body: &str) {
        self.packages += 1;
        let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
            return;
        };
        if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
            self.package_scripts.extend(scripts.keys().cloned());
        }
        for section in ["dependencies", "devDependencies", "optionalDependencies"] {
            if let Some(deps) = json.get(section).and_then(|v| v.as_object()) {
                self.package_deps.extend(deps.keys().cloned());
            }
        }
        if let Some(bin) = json.get("bin").and_then(|v| v.as_object()) {
            self.package_deps.extend(bin.keys().cloned());
        }
    }

    fn read_composer(&mut self, body: &str) {
        self.composers += 1;
        let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
            return;
        };
        if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
            self.composer_scripts.extend(scripts.keys().cloned());
        }
    }

    /// One message per cited command the repository does not define, in
    /// document order and without repeats.
    pub(crate) fn problems(&self, content: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (command, in_fence) in code_lines(content) {
            for segment in split_segments(&command) {
                if let Some(problem) = self.judge(&segment, in_fence) {
                    if !out.contains(&problem) {
                        out.push(problem);
                    }
                }
            }
        }
        out
    }

    fn judge(&self, segment: &str, in_fence: bool) -> Option<String> {
        let tokens = command_tokens(segment);
        let tool = Tool::of(tokens.first()?)?;
        let args = &tokens[1..];
        match tool {
            Tool::Make => self.judge_make(args, in_fence),
            Tool::Yarn | Tool::Npm | Tool::Pnpm => self.judge_node(tool, &tokens, args),
            Tool::Composer => self.judge_composer(args),
        }
    }

    fn make_knows(&self, target: &str) -> bool {
        self.make_targets.contains(target)
            || self.make_dynamic
            || self.make_patterns.iter().any(|(prefix, suffix)| {
                target.len() >= prefix.len() + suffix.len()
                    && target.starts_with(prefix.as_str())
                    && target.ends_with(suffix.as_str())
            })
    }

    fn judge_make(&self, args: &[String], in_fence: bool) -> Option<String> {
        const RELOCATING: &[&str] = &["-C", "-f", "--directory", "--file", "--makefile"];
        if args.iter().any(|arg| {
            RELOCATING.contains(&arg.as_str())
                || arg.starts_with("--directory=")
                || arg.starts_with("--file=")
                || arg.starts_with("-C")
                || arg.starts_with("-f")
        }) {
            return None;
        }
        let targets: Vec<&String> = args
            .iter()
            .filter(|arg| !arg.starts_with('-') && !arg.contains('='))
            .take_while(|arg| is_name(arg))
            .collect();
        // A span in prose can be followed by a word that is not a target.
        let judged = if in_fence { targets.len() } else { 1 };
        for target in targets.into_iter().take(judged) {
            if self.make_knows(target) {
                continue;
            }
            return Some(if self.makefiles == 0 {
                format!("`make {target}` is cited but the repository has no Makefile")
            } else {
                format!(
                    "`make {target}` is not a target of any Makefile in the repository{}",
                    names_hint(" (targets", &self.make_targets)
                )
            });
        }
        None
    }

    fn judge_node(&self, tool: Tool, tokens: &[String], args: &[String]) -> Option<String> {
        const VALUED: &[&str] = &[
            "--prefix",
            "--cwd",
            "--workspace",
            "-w",
            "--filter",
            "-F",
            "-C",
            "--dir",
            "--registry",
            "--cache",
        ];
        const GLOBAL_OR_NEUTRAL: &[&str] = &[
            "exec",
            "dlx",
            "x",
            "init",
            "create",
            "link",
            "unlink",
            "config",
            "set",
            "get",
            "login",
            "logout",
            "whoami",
            "version",
            "help",
            "global",
            "info",
            "view",
            "search",
            "doctor",
            "completion",
            "bin",
            "root",
            "prefix",
            "fund",
            "pkg",
            "workspace",
            "workspaces",
            "cache",
            "set-script",
            "ping",
            "token",
            "team",
            "access",
            "owner",
            "dist-tag",
            "profile",
            "star",
            "stars",
            "unpublish",
            "deprecate",
            "explain",
            "why",
            "self-update",
            "env",
            "node",
            "licenses",
            "constraints",
            "plugin",
            "dedupe",
        ];
        const INSTALLING: &[&str] = &[
            "install",
            "i",
            "ci",
            "add",
            "remove",
            "rm",
            "uninstall",
            "un",
            "update",
            "up",
            "upgrade",
            "upgrade-interactive",
            "audit",
            "outdated",
            "prune",
            "rebuild",
            "ls",
            "list",
            "pack",
            "publish",
            "import",
            "fetch",
            "store",
            "patch",
            "install-test",
            "it",
        ];
        let mut words = Vec::new();
        let mut skip_value = false;
        for arg in args {
            if skip_value {
                skip_value = false;
                continue;
            }
            if arg == "-g" || arg == "--global" {
                return None;
            }
            if arg.starts_with('-') {
                skip_value = VALUED.contains(&arg.as_str());
                continue;
            }
            words.push(arg.as_str());
        }
        let name = tokens[0].as_str();
        let first = *words.first()?;
        if GLOBAL_OR_NEUTRAL.contains(&first) || !is_name(first) {
            return None;
        }
        if INSTALLING.contains(&first) {
            return (self.packages == 0).then(|| {
                format!(
                    "`{name} {first}` is cited but the repository has no package.json, so there is nothing for it to act on"
                )
            });
        }
        let (script, explicit) = if matches!(first, "run" | "run-script") {
            let script = *words.get(1)?;
            if !is_name(script) {
                return None;
            }
            (script, true)
        } else {
            (first, false)
        };
        if self.package_scripts.contains(script) {
            return None;
        }
        // `yarn eslint` / `pnpm vitest` run an installed binary, which is
        // named like a dependency; `npm` runs no bare binary.
        let may_be_binary = !explicit
            && tool != Tool::Npm
            && (self.package_deps.contains(script) || script == "tsc");
        if may_be_binary {
            return None;
        }
        let shown = if explicit {
            format!("{name} run {script}")
        } else {
            format!("{name} {script}")
        };
        Some(if self.packages == 0 {
            format!("`{shown}` is cited but the repository has no package.json")
        } else {
            format!(
                "`{shown}` is not a script of any package.json in the repository{}",
                names_hint(" (scripts", &self.package_scripts)
            )
        })
    }

    fn judge_composer(&self, args: &[String]) -> Option<String> {
        const NEUTRAL: &[&str] = &[
            "create-project",
            "global",
            "init",
            "self-update",
            "selfupdate",
            "diagnose",
            "config",
            "list",
            "help",
            "about",
            "archive",
            "browse",
            "home",
            "clear-cache",
            "clearcache",
            "cc",
            "exec",
            "status",
            "search",
            "suggests",
            "completion",
            "show",
            "check-platform-reqs",
            "licenses",
        ];
        const PROJECT: &[&str] = &[
            "install",
            "update",
            "require",
            "remove",
            "validate",
            "dump-autoload",
            "dumpautoload",
            "outdated",
            "audit",
            "bump",
            "reinstall",
            "fund",
            "why",
            "why-not",
            "prohibits",
            "depends",
        ];
        let mut words = Vec::new();
        let mut skip_value = false;
        for arg in args {
            if skip_value {
                skip_value = false;
                continue;
            }
            if arg.starts_with('-') {
                skip_value = matches!(arg.as_str(), "-d" | "--working-dir");
                continue;
            }
            words.push(arg.as_str());
        }
        let first = *words.first()?;
        if NEUTRAL.contains(&first) || !is_name(first) {
            return None;
        }
        if PROJECT.contains(&first) {
            return (self.composers == 0).then(|| {
                format!(
                    "`composer {first}` is cited but the repository has no composer.json, so there is nothing for it to act on"
                )
            });
        }
        let (script, shown) = if matches!(first, "run-script" | "run") {
            let script = *words.get(1)?;
            if !is_name(script) {
                return None;
            }
            (script, format!("composer {first} {script}"))
        } else {
            (first, format!("composer {first}"))
        };
        if self.composer_scripts.contains(script) {
            return None;
        }
        Some(if self.composers == 0 {
            format!("`{shown}` is cited but the repository has no composer.json")
        } else {
            format!(
                "`{shown}` is not a script of any composer.json in the repository{}",
                names_hint(" (scripts", &self.composer_scripts)
            )
        })
    }
}

/// ` (targets: a, b, c)` — the first names known, to help the correction.
fn names_hint(label: &str, names: &BTreeSet<String>) -> String {
    if names.is_empty() {
        return format!("{label}: none defined)");
    }
    let shown: Vec<&str> = names.iter().take(SHOWN_NAMES).map(String::as_str).collect();
    let more = names.len().saturating_sub(SHOWN_NAMES);
    let tail = if more > 0 {
        format!(", … {more} more")
    } else {
        String::new()
    };
    format!("{label}: {}{tail})", shown.join(", "))
}

/// A plain command-line word (no placeholder, variable or shell syntax).
fn is_name(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/' | '@'))
}

/// The code of a document: every line of a fenced block, every inline span.
/// The flag says whether it came from a fence (a real command line).
fn code_lines(content: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut fence: Option<String> = None;
    let mut in_comment = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(marker) = &fence {
            if trimmed.starts_with(marker.as_str()) {
                fence = None;
            } else {
                out.push((line.to_string(), true));
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(trimmed[..3].to_string());
            continue;
        }
        let mut text = String::new();
        let mut rest = line;
        loop {
            if in_comment {
                match rest.find("-->") {
                    Some(end) => {
                        in_comment = false;
                        rest = &rest[end + 3..];
                    }
                    None => break,
                }
            }
            match rest.find("<!--") {
                Some(start) => {
                    text.push_str(&rest[..start]);
                    in_comment = true;
                    rest = &rest[start + 4..];
                }
                None => {
                    text.push_str(rest);
                    break;
                }
            }
        }
        for (index, span) in text.split('`').enumerate() {
            if index % 2 == 1 {
                out.push((span.to_string(), false));
            }
        }
    }
    out
}

fn split_segments(command: &str) -> Vec<String> {
    command
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace([';', '|'], "\n")
        .lines()
        .map(|segment| segment.trim().to_string())
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// The words of one segment, without its prompt, `sudo`, environment
/// assignments and trailing comment. A `docker …` or `ssh …` segment runs
/// elsewhere and is left out.
fn command_tokens(segment: &str) -> Vec<String> {
    let segment = segment.split(" #").next().unwrap_or(segment);
    let mut tokens: Vec<String> = segment
        .split_whitespace()
        .map(|t| t.trim_matches(|c| matches!(c, '`' | '"' | '\'' | ',' | ')' | '(')))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    while tokens.first().is_some_and(|t| {
        matches!(t.as_str(), "$" | ">" | "sudo" | "time" | "exec")
            || (t.contains('=') && !t.starts_with('-') && !t.contains('/'))
    }) {
        tokens.remove(0);
    }
    if tokens.first().is_some_and(|t| t.starts_with('#')) {
        return Vec::new();
    }
    tokens
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "doc_commands_test.rs"]
mod tests;
