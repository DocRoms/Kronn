//! Dated version sources for the audit (KT-1000).
//!
//! The audit agent has no web tool of its own, so "outdated" was judged from
//! the model's memory. This module gives it dated evidence instead: the
//! project's dependency watch (persisted by Kronn, never refreshed here), the
//! runtime versions the repository pins for CI, and which live source —
//! context7 or a web tool — the agent can query on top. Kronn picks that
//! source from configuration and states it in the prompt.

use std::path::Path;

use crate::core::anti_halluc::{dep_citation, verify_source_marker, SourceKind, SourceStatus};
use crate::models::{DependencyCheckStatus, DependencyUpdateSummary};

/// Rows of the watch rendered into a prompt; the rest is counted.
const MAX_RENDERED_PACKAGES: usize = 60;
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

    fn holds(&self, manager: &str, package: &str) -> Option<&WatchedPackage> {
        self.packages
            .iter()
            .find(|p| p.manager.eq_ignore_ascii_case(manager) && p.name == package)
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
    let Some((manager, package, date)) = dep_citation(raw) else {
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
    match snapshot.holds(&manager, &package) {
        Some(row) => Some((
            SourceStatus::Verified,
            format!(
                "dependency watch {}: {} {} → {}",
                snapshot.checked_on, row.manifest, row.current, row.latest
            ),
        )),
        None => Some((
            SourceStatus::NotFound,
            format!(
                "`{manager}/{package}` is not in the dependency watch dated {}",
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

fn scan_workflow(text: &str, file: &str, out: &mut Vec<CiRuntime>) {
    let rust_action = text.contains("rust-toolchain") || text.contains("rustup");
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim().trim_start_matches("- ").trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let runtime = VERSION_KEYS
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, r)| *r)
            .or_else(|| (rust_action && key == "toolchain").then_some("rust"));
        if let Some(runtime) = runtime {
            push(out, runtime, unquote(value), file, i + 1);
        }
    }
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
    workflows.push(".gitlab-ci.yml".to_string());
    for rel in workflows {
        if let Some(text) = read_pin_file(root, &rel) {
            scan_workflow(&text, &rel, &mut out);
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
                        p.current,
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
        out.push_str("None found in `.github/workflows`, `.gitlab-ci.yml`, `.nvmrc`, `.node-version`, `.python-version`, `.ruby-version`, `rust-toolchain(.toml)`, `.tool-versions`.\n");
    } else {
        out.push_str("| Runtime | Version | Cite as |\n|---|---|---|\n");
        for r in runtimes {
            out.push_str(&format!(
                "| {} | {} | [src: file: {}:{}] |\n",
                r.runtime, r.version, r.file, r.line
            ));
        }
    }
    out
}

#[cfg(test)]
#[path = "version_sources_test.rs"]
mod tests;
