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
use std::path::{Component, Path, PathBuf};

use crate::core::{fs_guard, rooted_io};

const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 20_000;
const MAX_INCLUDE_DEPTH: usize = 8;
/// Bounds on what `include` lines can make us read: files, bytes per file,
/// bytes overall, and directory entries listed for one `*` pattern.
const MAX_INCLUDE_FILES: usize = 256;
const MAX_INCLUDE_FILE_BYTES: u64 = 1 << 20;
const MAX_INCLUDE_TOTAL_BYTES: usize = 8 << 20;
const MAX_GLOB_ENTRIES: usize = 2_000;
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
    root: PathBuf,
    /// Makefiles already read (found by the walk or by an `include`).
    read_makefiles: BTreeSet<PathBuf>,
    makefiles: usize,
    /// Files and bytes read through `include` lines (bounded).
    included_files: usize,
    included_bytes: usize,
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
        let mut found = Self {
            root: project.to_path_buf(),
            ..Self::default()
        };
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
            _ => self.read_makefile(path, &body, 0),
        }
    }

    /// The targets a Makefile declares: rules (indented or not), names listed
    /// in `.PHONY`, and whatever the files it `include`s declare.
    fn read_makefile(&mut self, path: &Path, body: &str, depth: usize) {
        let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !self.read_makefiles.insert(key) {
            return;
        }
        self.makefiles += 1;
        let dir = path.parent().unwrap_or(&self.root).to_path_buf();
        let mut raw_lines = body.lines();
        let mut in_define = 0usize;
        while let Some(first) = raw_lines.next() {
            // A trailing backslash continues the line: a recipe (`docker run \`
            // then `-v a:b`) must not be read as a rule.
            let mut logical = first.to_string();
            while logical.ends_with('\\') {
                let Some(next) = raw_lines.next() else { break };
                logical.pop();
                logical.push(' ');
                logical.push_str(next.trim_start());
            }
            // A tab opens a recipe line; a rule may be indented with spaces.
            if logical.starts_with('\t') {
                continue;
            }
            let line = logical.trim();
            let word = line.split_whitespace().next().unwrap_or("");
            match word {
                "define" => in_define += 1,
                "endef" => in_define = in_define.saturating_sub(1),
                _ => {}
            }
            if in_define > 0 || word == "endef" || line.starts_with('#') {
                continue;
            }
            if matches!(word, "include" | "-include" | "sinclude") {
                if depth < MAX_INCLUDE_DEPTH {
                    let list = line[word.len()..].split(" #").next().unwrap_or("");
                    for included in list.split_whitespace() {
                        self.read_include(&dir, included, depth + 1);
                    }
                }
                continue;
            }
            if matches!(
                word,
                "ifeq"
                    | "ifneq"
                    | "ifdef"
                    | "ifndef"
                    | "else"
                    | "endif"
                    | "export"
                    | "unexport"
                    | "override"
                    | "private"
                    | "vpath"
                    | "undefine"
            ) {
                continue;
            }
            // A name only `.PHONY` lists is still a target `make` accepts.
            if let Some(names) = line
                .strip_prefix(".PHONY")
                .and_then(|rest| rest.trim_start().strip_prefix(':'))
            {
                for name in names.split(" #").next().unwrap_or("").split_whitespace() {
                    self.add_target(name);
                }
                continue;
            }
            if line.starts_with('.') {
                continue;
            }
            let Some(colon) = line.find(':') else {
                continue;
            };
            // `NAME := value`, `NAME ::= value` and `NAME = a:b` define
            // variables, not rules.
            if line[colon..].trim_start_matches(':').starts_with('=') || line[..colon].contains('=')
            {
                continue;
            }
            for target in line[..colon].split_whitespace() {
                self.add_target(target);
            }
        }
    }

    fn add_target(&mut self, target: &str) {
        if target == "&" {
            return;
        }
        if target.contains("$(") || target.contains("${") {
            self.make_dynamic = true;
        } else if let Some((prefix, suffix)) = target.split_once('%') {
            self.make_patterns
                .push((prefix.to_string(), suffix.to_string()));
        } else {
            self.make_targets.insert(target.to_string());
        }
    }

    /// Read a file an `include` names, whatever its extension. The path is
    /// taken from the including Makefile's directory, then the project root; a
    /// `*` in the file name matches any run of characters. A path built from a
    /// variable cannot be followed and is left alone.
    ///
    /// The project is a jail: a path that resolves outside it (`../`, an
    /// absolute path elsewhere) or passes through a symlink is never opened, so
    /// a name that only exists outside the project cannot reach the diagnostic
    /// sent to the agent. Each file is read with [`rooted_io::read`] (no link
    /// followed below the root) and what includes can make us read is bounded.
    fn read_include(&mut self, dir: &Path, included: &str, depth: usize) {
        if included.contains("$(") || included.contains("${") {
            return;
        }
        for base in [dir.to_path_buf(), self.root.clone()] {
            let Some(rel) = self.project_relative(&base, included) else {
                continue;
            };
            let Some(name) = rel.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            let parent = rel.parent().map(Path::to_path_buf).unwrap_or_default();
            let mut files = Vec::new();
            if let Some((prefix, suffix)) = name.split_once('*') {
                let listed = self.root.join(&parent);
                if fs_guard::assert_contained_no_symlink(&self.root, &listed).is_err() {
                    continue;
                }
                let Ok(entries) = fs::read_dir(&listed) else {
                    continue;
                };
                for entry in entries.flatten().take(MAX_GLOB_ENTRIES) {
                    let found = entry.file_name().to_string_lossy().into_owned();
                    // `file_type` does not follow a link: a symlink is no file.
                    if found.len() >= prefix.len() + suffix.len()
                        && found.starts_with(prefix)
                        && found.ends_with(suffix)
                        && entry.file_type().is_ok_and(|kind| kind.is_file())
                    {
                        files.push(parent.join(&found));
                    }
                }
                files.sort();
            } else if fs::symlink_metadata(self.root.join(&rel)).is_ok_and(|m| m.is_file()) {
                files.push(rel);
            }
            if files.is_empty() {
                continue;
            }
            for file in files {
                if let Some(body) = self.read_included_file(&file) {
                    let path = self.root.join(&file);
                    self.read_makefile(&path, &body, depth);
                }
            }
            return;
        }
    }

    /// `included` as a path relative to the project root, cleaned of `.` and
    /// `..`; `None` when it leaves the project (a `..` past the root, or an
    /// absolute path elsewhere). Purely lexical: nothing is opened.
    fn project_relative(&self, base: &Path, included: &str) -> Option<PathBuf> {
        let joined = base.join(included);
        let below = joined.strip_prefix(&self.root).ok()?;
        let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
        for component in below.components() {
            match component {
                Component::Normal(part) => parts.push(part),
                Component::CurDir => {}
                Component::ParentDir => {
                    parts.pop()?;
                }
                _ => return None,
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some(parts.into_iter().collect())
    }

    /// One included file's text, within the bounds and without following any
    /// link below the project root.
    fn read_included_file(&mut self, rel: &Path) -> Option<String> {
        if self.included_files >= MAX_INCLUDE_FILES {
            return None;
        }
        let size = fs::symlink_metadata(self.root.join(rel)).ok()?.len();
        if size > MAX_INCLUDE_FILE_BYTES
            || self.included_bytes.saturating_add(size as usize) > MAX_INCLUDE_TOTAL_BYTES
        {
            return None;
        }
        let bytes = rooted_io::read(&self.root, rel).ok()?;
        self.included_files += 1;
        self.included_bytes += bytes.len();
        String::from_utf8(bytes).ok()
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
        for (command, in_fence, line) in code_lines(content) {
            // A line that says the command is unavailable or gone does not claim
            // it exists: it is the very correction this check asks for.
            if denies_existence(&line) {
                continue;
            }
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
        // `yarn add/remove` is documentation shorthand for two built-ins.
        let shorthand = first.contains('/')
            && first
                .split('/')
                .all(|part| INSTALLING.contains(&part) || GLOBAL_OR_NEUTRAL.contains(&part));
        if !shorthand && (GLOBAL_OR_NEUTRAL.contains(&first) || !is_name(first)) {
            return None;
        }
        if shorthand || INSTALLING.contains(&first) {
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
        // `yarn eslint` / `pnpm vitest` / `yarn run eslint` run an installed
        // binary, which is named like a dependency. `npm run` and `pnpm run`
        // run scripts only; `npx` and `npm exec` are never judged.
        let runs_binaries = match tool {
            Tool::Yarn => true,
            Tool::Pnpm => !explicit,
            _ => false,
        };
        if runs_binaries && self.declares_binary(script) {
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

    /// Whether a binary named like this comes with a declared dependency: the
    /// dependency or a `bin` entry carries its name, a scoped package ends with
    /// it (`@scope/eslint-cli`), or it is a well-known alias of one.
    fn declares_binary(&self, bin: &str) -> bool {
        const ALIASES: &[(&str, &str)] = &[
            ("tsc", "typescript"),
            ("tsserver", "typescript"),
            ("ng", "@angular/cli"),
            ("vue-cli-service", "@vue/cli-service"),
        ];
        self.package_deps.contains(bin)
            || ALIASES
                .iter()
                .any(|(alias, package)| *alias == bin && self.package_deps.contains(*package))
            || self
                .package_deps
                .iter()
                .any(|dep| dep.rsplit_once('/').is_some_and(|(_, last)| last == bin))
            || bin == "tsc"
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

/// Whether a line of a document says that what it names is not there.
fn denies_existence(line: &str) -> bool {
    const PHRASES: &[&str] = &[
        "not available",
        "unavailable",
        "not exist",
        "doesn't exist",
        "no longer",
        "removed",
        "not defined",
        "not provided",
        "not supported",
        "not applicable",
        "there is no ",
        "indisponible",
        "n'est pas disponible",
        "n'existe pas",
        "n'existe plus",
        "supprimé",
        "inexistant",
        "non disponible",
    ];
    let lowered = line.to_lowercase();
    PHRASES.iter().any(|phrase| lowered.contains(phrase))
}

/// The code of a document: every line of a fenced block, every inline span.
/// The flag says whether it came from a fence (a real command line); the
/// last item is the whole source line, for what it says around the command.
fn code_lines(content: &str) -> Vec<(String, bool, String)> {
    let mut out = Vec::new();
    let mut fence: Option<String> = None;
    let mut in_comment = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(marker) = &fence {
            if trimmed.starts_with(marker.as_str()) {
                fence = None;
            } else {
                out.push((line.to_string(), true, line.to_string()));
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
                out.push((span.to_string(), false, line.to_string()));
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
