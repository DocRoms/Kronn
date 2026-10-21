//! Dated version sources for the audit (KT-1000).
//!
//! The audit agent has no web tool of its own, so "outdated" was judged from
//! the model's memory. This module gives it dated evidence instead: the
//! project's dependency watch (persisted by Kronn, never refreshed here), the
//! runtime versions the repository pins for CI, and which live source —
//! context7 or a web tool — the agent can query on top. Kronn picks that
//! source from configuration and states it in the prompt.

use std::path::Path;

use crate::core::anti_halluc::{
    dep_citation, dep_reference, verify_source_marker, SourceKind, SourceStatus,
};
use crate::models::{DependencyCheckStatus, DependencyUpdateSummary};

/// Rows of the watch rendered into a prompt; the rest is counted.
const MAX_RENDERED_PACKAGES: usize = 60;
/// CI files named in the block when no pin was parsed.
const MAX_LISTED_CI_FILES: usize = 15;
/// CI runtime pins kept per scan.
const MAX_RUNTIMES: usize = 40;
/// Workflow files read per scan.
const MAX_WORKFLOW_FILES: usize = 50;
/// A pin file larger than this is not a pin file.
const MAX_PIN_FILE_BYTES: u64 = 512 * 1024;

/// One outdated package as the dependency watch recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedPackage {
    pub manager: String,
    pub manifest: String,
    pub name: String,
    pub current: String,
    pub latest: String,
    pub major: bool,
    /// Declared in the manifest, not installed: `current` is empty.
    pub declared_only: bool,
}

/// A manifest the watch could not check: its packages have no dated source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncheckedManifest {
    pub manager: String,
    pub manifest: String,
    pub status: DependencyCheckStatus,
}

/// The dependency watch as injected into one audit run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencySnapshot {
    /// UTC check date, `YYYY-MM-DD`: the date every `deps:` citation carries.
    pub checked_on: String,
    pub packages: Vec<WatchedPackage>,
    pub unchecked: Vec<UncheckedManifest>,
    /// Manifests checked with nothing outdated.
    pub up_to_date: Vec<(String, String)>,
}

impl DependencySnapshot {
    pub fn from_summary(summary: &DependencyUpdateSummary) -> Self {
        let mut packages = Vec::new();
        let mut unchecked = Vec::new();
        let mut up_to_date = Vec::new();
        for manager in &summary.managers {
            match manager.status {
                DependencyCheckStatus::UpdatesAvailable => {
                    packages.extend(manager.packages.iter().map(|p| WatchedPackage {
                        manager: manager.manager.clone(),
                        manifest: manager.manifest.clone(),
                        name: p.name.clone(),
                        current: p.current.clone(),
                        latest: p.latest.clone(),
                        major: p.major,
                        declared_only: p.declared_only,
                    }))
                }
                DependencyCheckStatus::UpToDate => {
                    up_to_date.push((manager.manager.clone(), manager.manifest.clone()))
                }
                _ => unchecked.push(UncheckedManifest {
                    manager: manager.manager.clone(),
                    manifest: manager.manifest.clone(),
                    status: manager.status.clone(),
                }),
            }
        }
        Self {
            checked_on: summary.checked_at.format("%Y-%m-%d").to_string(),
            packages,
            unchecked,
            up_to_date,
        }
    }

    /// The citation an agent writes for one row.
    pub fn citation(&self, package: &WatchedPackage) -> String {
        format!(
            "[src: deps: {}/{}@{}]",
            package.manager, package.name, self.checked_on
        )
    }

    /// The row a `<manager>/<package>` reference names. Matched against the
    /// labels the watch holds rather than split at a `/`: the npm label is
    /// `JS / TS`, and a scoped package (`@scope/pkg`) holds a `/` too.
    fn holds(&self, reference: &str) -> Option<&WatchedPackage> {
        self.packages.iter().find(|p| {
            reference
                .get(..p.manager.len())
                .is_some_and(|m| m.eq_ignore_ascii_case(&p.manager))
                && reference[p.manager.len()..]
                    .strip_prefix('/')
                    .is_some_and(|name| name.trim() == p.name)
        })
    }
}

/// Verdict on a `deps:` marker against the snapshot the audit injected.
/// `None` for any other marker. A row the snapshot does not hold, or a date
/// it does not carry, was not read from the watch: it is refused.
pub fn verify_dep_marker(
    raw: &str,
    snapshot: Option<&DependencySnapshot>,
) -> Option<(SourceStatus, String)> {
    if verify_source_marker(raw, None).kind != SourceKind::Deps {
        return None;
    }
    let (Some(_), Some((reference, date))) = (dep_citation(raw), dep_reference(raw)) else {
        return Some((
            SourceStatus::EmptyRef,
            "dependency watch citation must read `deps: <manager>/<package>@<YYYY-MM-DD>`"
                .to_string(),
        ));
    };
    let Some(snapshot) = snapshot else {
        return Some((
            SourceStatus::NotFound,
            "no dependency watch was injected into this audit".to_string(),
        ));
    };
    if date != snapshot.checked_on {
        return Some((
            SourceStatus::NotFound,
            format!(
                "the injected dependency watch is dated {}, not {date}",
                snapshot.checked_on
            ),
        ));
    }
    match snapshot.holds(&reference) {
        Some(row) => Some((
            SourceStatus::Verified,
            if row.declared_only {
                format!(
                    "dependency watch {}: {} declared, not installed → latest {}",
                    snapshot.checked_on, row.manifest, row.latest
                )
            } else {
                format!(
                    "dependency watch {}: {} {} → {}",
                    snapshot.checked_on, row.manifest, row.current, row.latest
                )
            },
        )),
        None => Some((
            SourceStatus::NotFound,
            format!(
                "`{reference}` is not in the dependency watch dated {}",
                snapshot.checked_on
            ),
        )),
    }
}

/// A runtime version the repository pins, with where it is pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiRuntime {
    pub runtime: String,
    pub version: String,
    /// Project-relative path.
    pub file: String,
    /// 1-based line.
    pub line: usize,
    /// Dockerfile multi-stage builds: which stage this `FROM` is.
    pub stage: Option<Stage>,
}

/// A stage of a multi-stage Dockerfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// An intermediate stage: the image it pins is not what ships.
    Build,
    /// The last stage: the image that runs.
    Runtime,
}

fn read_pin_file(root: &Path, rel: &str) -> Option<String> {
    let path = root.join(rel);
    // A symlink could pull a file from outside the project into the prompt.
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_PIN_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn unquote(value: &str) -> &str {
    let value = value.split(" #").next().unwrap_or(value).trim();
    value.trim_matches(|c| c == '"' || c == '\'').trim()
}

fn first_value_line(text: &str) -> Option<(usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .find(|(_, l)| !l.is_empty() && !l.starts_with('#'))
}

fn push(out: &mut Vec<CiRuntime>, runtime: &str, version: &str, file: &str, line: usize) {
    if version.is_empty() || version.contains("${{") || out.len() >= MAX_RUNTIMES {
        return;
    }
    out.push(CiRuntime {
        runtime: runtime.to_string(),
        version: version.to_string(),
        file: file.to_string(),
        line,
        stage: None,
    });
}

/// `<runtime>-version: <value>` keys a CI step uses to pick a runtime.
const VERSION_KEYS: &[(&str, &str)] = &[
    ("node-version", "node"),
    ("python-version", "python"),
    ("go-version", "go"),
    ("java-version", "java"),
    ("php-version", "php"),
    ("ruby-version", "ruby"),
    ("dotnet-version", "dotnet"),
    ("deno-version", "deno"),
    ("bun-version", "bun"),
    ("rust-version", "rust"),
];

/// `nodeVersion`, `node_version`, `NODE-VERSION`… all read `node-version`.
fn normalize_version_key(key: &str) -> String {
    key.chars()
        .filter(|c| !matches!(c, '-' | '_'))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn runtime_of_version_key(key: &str) -> Option<&'static str> {
    let key = normalize_version_key(key);
    VERSION_KEYS
        .iter()
        .find(|(k, _)| normalize_version_key(k) == key)
        .map(|(_, runtime)| *runtime)
}

fn leading_spaces(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn scan_workflow(text: &str, file: &str, out: &mut Vec<CiRuntime>) {
    let rust_action = text.contains("rust-toolchain") || text.contains("rustup");
    let lines: Vec<&str> = text.lines().collect();
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim().trim_start_matches("- ").trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        // The `with:` key of a reusable workflow is the caller's spelling
        // (`nodeversion`, `node_version`, `NodeVersion`…), not the action's.
        let runtime = runtime_of_version_key(key)
            .or_else(|| (rust_action && key == "toolchain").then_some("rust"));
        let Some(runtime) = runtime else { continue };
        let value = unquote(value);
        if !value.is_empty() {
            push(out, runtime, value, file, i + 1);
            continue;
        }
        // The `inputs:` declaration of a reusable workflow: its `default:`.
        let indent = leading_spaces(raw);
        let default = lines[i + 1..]
            .iter()
            .enumerate()
            .take_while(|(_, l)| l.trim().is_empty() || leading_spaces(l) > indent)
            .find_map(|(offset, l)| {
                let (k, v) = l.trim().split_once(':')?;
                (k.trim() == "default").then(|| (i + 2 + offset, unquote(v).to_string()))
            });
        if let Some((line, value)) = default {
            push(out, runtime, &value, file, line);
        }
    }
}

/// The first `major[.minor[.patch]]` number in `text`.
fn first_version_number(text: &str) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let version: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let version = version.trim_end_matches('.');
    (!version.is_empty()).then(|| version.to_string())
}

/// The quoted string that follows `keyword` on `line`, when nothing but
/// whitespace, `(`, `:` or an argument name separates them.
fn quoted_after<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(found) = line[from..].find(keyword) {
        let start = from + found;
        let end = start + keyword.len();
        from = end;
        let boundary = line[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && !matches!(c, '_' | '-' | '\'' | '"'));
        if !boundary {
            continue;
        }
        let rest = &line[end..];
        let Some(quote) = rest.find(['\'', '"']) else {
            continue;
        };
        let between = &rest[..quote];
        let separated = between.is_empty()
            || between.starts_with(|c: char| c.is_whitespace() || matches!(c, '(' | ':'));
        let plain = between
            .chars()
            .all(|c| c.is_whitespace() || c.is_ascii_alphabetic() || matches!(c, '(' | ':'));
        if !separated || !plain {
            continue;
        }
        let delimiter = rest[quote..].chars().next()?;
        return rest[quote + 1..].split(delimiter).next();
    }
    None
}

/// The Node version in an agent label: `slavenodejs18`, `node22-arm`.
/// `node` / `nodejs` must be directly followed by a plausible major (10 to 99,
/// no leading zero): `build-node-1` or `docker-node-02` name a host, not Node.
fn node_label_version(label: &str) -> Option<String> {
    let lower = label.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find("node") {
        let mut rest = &lower[from + found + 4..];
        from += found + 4;
        rest = rest.strip_prefix("js").unwrap_or(rest);
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 2 && !rest.starts_with('0') {
            let follows = rest[2..].chars().next();
            // `node22-arm`, `node18.4`; not `node2024` or `node22x1`.
            if follows.is_none_or(|c| !c.is_ascii_alphanumeric()) {
                return first_version_number(rest);
            }
        }
    }
    None
}

/// Jenkins: `tools { nodejs '22' }` and node-versioned agent labels.
fn scan_jenkinsfile(text: &str, file: &str, out: &mut Vec<CiRuntime>) {
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("//") {
            continue;
        }
        if let Some(version) = quoted_after(line, "nodejs").and_then(first_version_number) {
            push(out, "node", &version, file, i + 1);
        }
        if let Some(version) = quoted_after(line, "label").and_then(node_label_version) {
            push(out, "node", &version, file, i + 1);
        }
    }
}

/// A version string: a leading digit, so `lts-alpine`, `current` and an
/// unresolved `$NODE_VERSION` are not versions.
fn plausible_node_version(value: &str) -> Option<&str> {
    let value = value.trim_start_matches('v');
    value
        .starts_with(|c: char| c.is_ascii_digit())
        .then_some(value)
}

/// Dockerfile: `FROM node:20-alpine` and `ARG NODE_VERSION=20`. One row per
/// `FROM node:…` stage with its line; in a multi-stage file the last `FROM`
/// is the stage that runs, the earlier ones only build.
fn scan_dockerfile(text: &str, file: &str, out: &mut Vec<CiRuntime>) {
    let first_new = out.len();
    let mut from_lines = 0;
    let mut last_from_line = 0;
    let mut node_stage_rows = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let mut words = raw.split_whitespace();
        let Some(instruction) = words.next() else {
            continue;
        };
        if instruction.eq_ignore_ascii_case("ARG") {
            let declared = words.next().and_then(|word| word.split_once('='));
            if let Some((name, value)) = declared {
                if runtime_of_version_key(name) == Some("node") {
                    if let Some(version) = plausible_node_version(unquote(value)) {
                        push(out, "node", version, file, i + 1);
                    }
                }
            }
        } else if instruction.eq_ignore_ascii_case("FROM") {
            from_lines += 1;
            last_from_line = i + 1;
            let Some(image) = words.find(|word| !word.starts_with("--")) else {
                continue;
            };
            let image = image.rsplit('/').next().unwrap_or(image);
            let image = image.split('@').next().unwrap_or(image);
            let Some((name, tag)) = image.split_once(':') else {
                continue;
            };
            let version = tag.split('-').next().unwrap_or(tag);
            if name == "node" {
                if let Some(version) = plausible_node_version(version) {
                    let before = out.len();
                    push(out, "node", version, file, i + 1);
                    if out.len() > before {
                        node_stage_rows.push(before);
                    }
                }
            }
        }
    }
    if from_lines > 1 {
        for index in node_stage_rows {
            if let Some(row) = out.get_mut(index).filter(|_| index >= first_new) {
                row.stage = Some(if row.line == last_from_line {
                    Stage::Runtime
                } else {
                    Stage::Build
                });
            }
        }
    }
}

fn is_jenkinsfile(name: &str) -> bool {
    name == "Jenkinsfile"
        || name.starts_with("Jenkinsfile.")
        || name.to_ascii_lowercase().ends_with(".jenkinsfile")
}

fn is_dockerfile(name: &str) -> bool {
    name == "Dockerfile" || name.starts_with("Dockerfile.") || name.ends_with(".Dockerfile")
}

/// Root-level regular files of `root` that `keep` selects, sorted.
fn root_files(root: &Path, keep: fn(&str) -> bool) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| keep(name))
        .collect();
    names.sort();
    names
}

fn workflow_files(root: &Path) -> Vec<String> {
    let mut workflows: Vec<String> = std::fs::read_dir(root.join(".github/workflows"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".yml") || name.ends_with(".yaml"))
        .map(|name| format!(".github/workflows/{name}"))
        .collect();
    workflows.sort();
    workflows.truncate(MAX_WORKFLOW_FILES);
    workflows
}

/// CI and build files present in the repository, whether or not Kronn parsed
/// a pin out of them: the audit is told what exists to read itself.
pub fn ci_files_present(root: &Path) -> Vec<String> {
    let mut files = workflow_files(root);
    for rel in [
        ".gitlab-ci.yml",
        ".circleci/config.yml",
        "azure-pipelines.yml",
        "bitbucket-pipelines.yml",
    ] {
        if std::fs::symlink_metadata(root.join(rel)).is_ok_and(|m| m.is_file()) {
            files.push(rel.to_string());
        }
    }
    files.extend(root_files(root, is_jenkinsfile));
    files.extend(root_files(root, is_dockerfile));
    files
}

/// Runtime versions pinned by the repository's CI and toolchain files.
/// Read-only, deterministic, no process spawned.
pub fn ci_runtime_versions(root: &Path) -> Vec<CiRuntime> {
    let mut out = Vec::new();
    for (rel, runtime) in [
        (".nvmrc", "node"),
        (".node-version", "node"),
        (".python-version", "python"),
        (".ruby-version", "ruby"),
    ] {
        if let Some((line, value)) = read_pin_file(root, rel)
            .as_deref()
            .and_then(first_value_line)
            .map(|(n, v)| (n, unquote(v).trim_start_matches('v').to_string()))
        {
            push(&mut out, runtime, &value, rel, line);
        }
    }
    for rel in ["rust-toolchain.toml", "rust-toolchain"] {
        let Some(text) = read_pin_file(root, rel) else {
            continue;
        };
        let channel = text.lines().enumerate().find_map(|(i, l)| {
            let (key, value) = l.split_once('=')?;
            (key.trim() == "channel").then(|| (i + 1, unquote(value).to_string()))
        });
        let pin = channel.or_else(|| {
            first_value_line(&text)
                .filter(|(_, l)| !l.starts_with('['))
                .map(|(n, l)| (n, unquote(l).to_string()))
        });
        if let Some((line, value)) = pin {
            push(&mut out, "rust", &value, rel, line);
        }
    }
    if let Some(text) = read_pin_file(root, ".tool-versions") {
        for (i, l) in text.lines().enumerate() {
            let mut parts = l.split_whitespace();
            if let (Some(tool), Some(version)) = (parts.next(), parts.next()) {
                if tool.starts_with('#') {
                    continue;
                }
                let runtime = if tool == "nodejs" { "node" } else { tool };
                push(&mut out, runtime, version, ".tool-versions", i + 1);
            }
        }
    }
    let mut workflows = workflow_files(root);
    workflows.push(".gitlab-ci.yml".to_string());
    for rel in workflows {
        if let Some(text) = read_pin_file(root, &rel) {
            scan_workflow(&text, &rel, &mut out);
        }
    }
    for rel in root_files(root, is_jenkinsfile) {
        if let Some(text) = read_pin_file(root, &rel) {
            scan_jenkinsfile(&text, &rel, &mut out);
        }
    }
    for rel in root_files(root, is_dockerfile) {
        if let Some(text) = read_pin_file(root, &rel) {
            scan_dockerfile(&text, &rel, &mut out);
        }
    }
    out
}

/// Where the audit's "latest version / EOL / deprecated" judgements come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionSource {
    Context7,
    Web,
    DependencyWatch,
    None,
}

/// What the audit agent of this run can reach, decided by Kronn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VersionSourceInputs {
    /// context7 is configured for the project and this agent mounts it.
    pub context7: bool,
    /// This agent has a native web tool in an audit.
    pub web: bool,
    /// A dependency watch result is stored for the project.
    pub watch: bool,
}

/// The owner's cascade: context7, else web, else the dependency watch.
pub fn select_version_source(inputs: VersionSourceInputs) -> VersionSource {
    if inputs.context7 {
        VersionSource::Context7
    } else if inputs.web {
        VersionSource::Web
    } else if inputs.watch {
        VersionSource::DependencyWatch
    } else {
        VersionSource::None
    }
}

/// Whether an `.mcp.json` body declares an enabled `context7` server.
/// `stdio_only`: the agent drops servers without a `command` (ACP sessions).
pub fn context7_configured(mcp_json: &str, stdio_only: bool) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(mcp_json) else {
        return false;
    };
    let Some(servers) = value.get("mcpServers").and_then(|s| s.as_object()) else {
        return false;
    };
    servers.iter().any(|(name, cfg)| {
        name.eq_ignore_ascii_case("context7")
            && !cfg
                .get("disabled")
                .and_then(|d| d.as_bool())
                .unwrap_or(false)
            && (!stdio_only
                || cfg
                    .get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| !c.trim().is_empty()))
    })
}

fn source_statement(source: VersionSource, snapshot: Option<&DependencySnapshot>) -> String {
    let watch = snapshot
        .map(|s| format!("dependency watch dated {}", s.checked_on))
        .unwrap_or_else(|| "no dependency watch stored".to_string());
    match source {
        VersionSource::Context7 => format!("context7 (fallback: {watch})"),
        VersionSource::Web => format!("web (fallback: {watch})"),
        VersionSource::DependencyWatch | VersionSource::None => watch,
    }
}

/// What the block is read for: the tech-debt and security passes judge a gap
/// and write a TD; the project index fills the Stack table's currency cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionPurpose {
    Judgement,
    StackTable,
}

/// The prompt block for the steps that judge versions and standards.
pub fn render_version_block(
    source: VersionSource,
    snapshot: Option<&DependencySnapshot>,
    runtimes: &[CiRuntime],
    today: &str,
) -> String {
    render_version_block_for(VersionPurpose::Judgement, source, snapshot, runtimes, today)
}

/// [`render_version_block`] for one purpose: same dated rows, own instructions.
pub fn render_version_block_for(
    purpose: VersionPurpose,
    source: VersionSource,
    snapshot: Option<&DependencySnapshot>,
    runtimes: &[CiRuntime],
    today: &str,
) -> String {
    render_version_block_with_ci(purpose, source, snapshot, runtimes, &[], today)
}

/// [`render_version_block_for`] that also names the CI files present, so an
/// empty pin table reads "none parsed" rather than "none exists" (KT-1279).
pub fn render_version_block_with_ci(
    purpose: VersionPurpose,
    source: VersionSource,
    snapshot: Option<&DependencySnapshot>,
    runtimes: &[CiRuntime],
    ci_files: &[String],
    today: &str,
) -> String {
    let mut out = String::from("## Technologies and standards — dated version sources\n\n");
    match purpose {
        VersionPurpose::Judgement => out.push_str(&format!(
            "**Source used: {}.** Kronn chose it from this run's configuration; write this line, \
as `source used: …`, in the evidence cell of the `Dependencies` coverage row.\n\n",
            source_statement(source, snapshot)
        )),
        VersionPurpose::StackTable => out.push_str(&format!(
            "**Source used: {}.** Kronn chose it from this run's configuration; it feeds the \
`Latest stable` and `Support end` cells of the Stack table.\n\n",
            source_statement(source, snapshot)
        )),
    }
    match source {
        VersionSource::Context7 => out.push_str(
            "- The `context7` MCP is mounted for you: resolve the library id, then query its docs for the \
latest stable version, end of life and deprecations. Cite each answer as \
[src: api: context7/<library-id>@<today>] or with the documentation URL it returns as [src: url: <url>].\n",
        ),
        VersionSource::Web => out.push_str(
            "- You have a web tool: read the official release or end-of-life page (project site, \
registry page, endoflife.date). Cite it as [src: url: <url>] and give the date you read it.\n",
        ),
        VersionSource::DependencyWatch | VersionSource::None => out.push_str(
            "- No live source is available to you in this run: do NOT judge a version as outdated, \
EOL or deprecated from memory. Only the dated rows below are evidence.\n",
        ),
    }
    match purpose {
        VersionPurpose::Judgement => out.push_str(&format!(
            "- Today is {today}. A version gap, an EOL runtime or a deprecated pattern becomes a TD carrying \
its proof (the manifest or CI `[src: file: …]` line) AND its dated source citation.\n\
- Whatever no source above backs is written as `not verified: no source` — never asserted.\n\n"
        )),
        VersionPurpose::StackTable => out.push_str(&format!(
            "- Today is {today}. A Stack row's `Latest stable` and `Support end` cells each carry their \
source citation AND its date; the `In place` cell keeps the lockfile or manifest `[src: file: …]` line.\n\
- Whatever no source above backs is written as `not verified: no source` — never filled from memory.\n\n"
        )),
    }

    match snapshot {
        Some(snapshot) => {
            out.push_str(&format!(
                "### Dependency watch (checked {} by Kronn's registry query; outdated packages only, capped per manifest)\n\n",
                snapshot.checked_on
            ));
            if snapshot.packages.is_empty() {
                out.push_str("No outdated package recorded.\n");
            } else {
                out.push_str(
                    "| Manager | Manifest | Package | In place | Latest | Gap | Cite as |\n|---|---|---|---|---|---|---|\n",
                );
                for p in snapshot.packages.iter().take(MAX_RENDERED_PACKAGES) {
                    out.push_str(&format!(
                        "| {} | {} | {} | {} | {} | {} | {} |\n",
                        p.manager,
                        p.manifest,
                        p.name,
                        if p.declared_only {
                            "not installed (declared range only; installed version unknown)"
                        } else {
                            p.current.as_str()
                        },
                        p.latest,
                        if p.major { "major" } else { "minor/patch" },
                        snapshot.citation(p)
                    ));
                }
                let rest = snapshot.packages.len().saturating_sub(MAX_RENDERED_PACKAGES);
                if rest > 0 {
                    out.push_str(&format!("\n…and {rest} more row(s) not shown.\n"));
                }
            }
            for (manager, manifest) in &snapshot.up_to_date {
                out.push_str(&format!("- {manifest} ({manager}): up to date on {}.\n", snapshot.checked_on));
            }
            for m in &snapshot.unchecked {
                out.push_str(&format!(
                    "- {} ({}): not checked ({:?}) — its packages have no dated source.\n",
                    m.manifest, m.manager, m.status
                ));
            }
            out.push_str(
                "\nA `deps:` citation is checked against this table: a package or a date it does not hold is refused as fabricated.\n\n",
            );
        }
        None => out.push_str(
            "### Dependency watch\n\nNo dependency watch result is stored for this project (run « Dépendances » on its overview).\n\n",
        ),
    }

    out.push_str("### Runtime versions pinned in the repository (parsed by Kronn)\n\n");
    if runtimes.is_empty() {
        out.push_str(
            "None parsed by Kronn from `.github/workflows`, `.gitlab-ci.yml`, `Jenkinsfile`, \
`Dockerfile`, `.nvmrc`, `.node-version`, `.python-version`, `.ruby-version`, \
`rust-toolchain(.toml)`, `.tool-versions`. This is NOT evidence that the project pins no runtime: ",
        );
        if ci_files.is_empty() {
            out.push_str("no CI file was found either.\n");
        } else {
            let shown: Vec<String> = ci_files
                .iter()
                .take(MAX_LISTED_CI_FILES)
                .map(|f| format!("`{f}`"))
                .collect();
            out.push_str(&format!("CI files present: {}", shown.join(", ")));
            let rest = ci_files.len().saturating_sub(MAX_LISTED_CI_FILES);
            if rest > 0 {
                out.push_str(&format!(" …and {rest} more"));
            }
            out.push_str(". Read them for a runtime pin before asserting there is none.\n");
        }
    } else {
        out.push_str("| Runtime | Version | Cite as |\n|---|---|---|\n");
        for r in runtimes {
            let role = match r.stage {
                Some(Stage::Runtime) => " (final stage: what runs)",
                Some(Stage::Build) => " (build stage)",
                None => "",
            };
            out.push_str(&format!(
                "| {}{} | {} | [src: file: {}:{}] |\n",
                r.runtime, role, r.version, r.file, r.line
            ));
        }
    }
    out
}

#[cfg(test)]
#[path = "version_sources_test.rs"]
mod tests;
