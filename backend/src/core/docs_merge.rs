//! Template v2 install by merge into an already-documented repository.
//!
//! The planner maps every existing doc, section by section, onto a v2 file
//! (the correspondence table), copies mapped sections verbatim into the v2
//! files that do not exist yet, and never moves, rewrites or quarantines a
//! source. Planning is pure over a snapshot of existing paths, so the same
//! plan is computed for a local checkout, a worktree on a remote base, or a
//! resumed worktree (where previously created files are not part of the base).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Slug of the merge worktree; its branch is `kronn/<slug>`.
pub const MERGE_SLUG: &str = "template-v2-merge";
pub const MERGE_BRANCH: &str = "kronn/template-v2-merge";
/// The correspondence table ships in the diff so the reviewer sees it next to the files.
pub const REPORT_REL: &str = "reports/template-v2-merge.md";

/// Directories that hold documentation, standard or not.
pub const DOCS_DIR_CANDIDATES: &[&str] = &["docs", "doc", "ai", "documentation"];
/// Runtime scratch and the old quarantine are inventoried like any doc, except `var/`.
const SKIPPED_DOC_SUBDIRS: &[&str] = &["var"];
/// Root markdown files that are history, not guidance.
const SKIPPED_ROOT_FILES: &[&str] = &["changelog.md"];
const PROSE_EXTENSIONS: &[&str] = &["md", "markdown", "mdx", "txt", "rst", "adoc"];

const IMPORT_START: &str = "<!-- kronn:imported-start -->";
const IMPORT_END: &str = "<!-- kronn:imported-end -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingAction {
    /// Copied verbatim into a v2 file that this merge creates.
    Extracted,
    /// Mapped, but the v2 file already exists: nothing is written, the content stays in its source.
    TargetExists,
    /// No v2 section fits: left in place for a human to place.
    Unmapped,
    /// Not prose (image, data, binary, symlink): left in place.
    NotProse,
    /// Kronn's own generated block in a root agent file: left in place.
    KronnManaged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Correspondence {
    /// Repo-relative source path.
    pub source: String,
    /// Heading of the section; `None` for a preamble or a whole file.
    pub section: Option<String>,
    /// 1-based inclusive line range in the source (0..0 for non-prose files).
    pub start_line: usize,
    pub end_line: usize,
    /// Repo-relative v2 file the section belongs to.
    pub target: Option<String>,
    pub action: MappingAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedFile {
    /// Repo-relative path of a file that does not exist yet.
    pub path: String,
    pub content: String,
    /// Number of sections copied into it.
    pub imported_sections: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MergePlan {
    /// Repo-relative v2 docs directory.
    pub docs_dir: String,
    pub correspondence: Vec<Correspondence>,
    pub writes: Vec<PlannedFile>,
    /// v2 files whose path already exists: never written.
    pub existing_targets: Vec<String>,
}

impl MergePlan {
    pub fn unmapped(&self) -> impl Iterator<Item = &Correspondence> {
        self.correspondence
            .iter()
            .filter(|row| row.action == MappingAction::Unmapped)
    }
}

/// Where the v2 files go. An explicit location (KT-263) wins; otherwise an
/// existing `docs/` or `doc/` is kept, and anything else gets `docs/`.
pub fn choose_docs_dir(existing: &BTreeSet<String>, explicit: Option<&str>) -> String {
    if let Some(dir) = explicit
        .map(|d| d.trim_matches('/'))
        .filter(|d| !d.is_empty())
    {
        return dir.to_string();
    }
    let has_dir = |dir: &str| {
        let prefix = format!("{dir}/");
        existing.iter().any(|p| p.starts_with(&prefix))
    };
    if !has_dir("docs") && has_dir("doc") {
        return "doc".into();
    }
    "docs".into()
}

/// Repo-relative files the merge reads as sources, from the snapshot.
pub fn source_paths(existing: &BTreeSet<String>) -> Vec<String> {
    existing
        .iter()
        .filter(|path| is_source_path(path))
        .cloned()
        .collect()
}

fn is_source_path(path: &str) -> bool {
    let mut parts = path.split('/');
    let Some(first) = parts.next() else {
        return false;
    };
    match parts.next() {
        None => {
            let lower = first.to_ascii_lowercase();
            lower.ends_with(".md") && !SKIPPED_ROOT_FILES.contains(&lower.as_str())
        }
        Some(second) => {
            DOCS_DIR_CANDIDATES.contains(&first) && !SKIPPED_DOC_SUBDIRS.contains(&second)
        }
    }
}

/// One template file, relative to the v2 docs directory.
#[derive(Debug, Clone)]
pub struct TemplateFile {
    pub rel: String,
    pub bytes: Vec<u8>,
}

/// The v2 template tree plus the anti-hallucination spec that the install also seeds.
pub fn load_template(template_docs: &Path) -> Result<Vec<TemplateFile>, String> {
    let mut files = Vec::new();
    walk_files(template_docs, template_docs, &mut |rel, abs| {
        let bytes =
            std::fs::read(abs).map_err(|e| format!("read template {}: {e}", abs.display()))?;
        files.push(TemplateFile { rel, bytes });
        Ok(())
    })?;
    let spec = "conventions/agents-md-format-v1.md";
    if !files.iter().any(|f| f.rel == spec) {
        files.push(TemplateFile {
            rel: spec.into(),
            bytes: crate::core::anti_halluc::SPEC_AGENTS_MD_V1
                .as_bytes()
                .to_vec(),
        });
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(files)
}

/// Every regular file under `dir`, repo-relative to `root`, without following symlinks.
/// Symlinks are reported too (as paths) so they show up in the table as left in place.
pub fn snapshot_tree(root: &Path) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    walk_entries(root, root, &mut |rel| {
        out.insert(rel);
    })?;
    Ok(out)
}

fn walk_entries(root: &Path, dir: &Path, sink: &mut dyn FnMut(String)) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
        let name = entry.file_name();
        if name == ".git" || name == ".kronn" || name == "node_modules" || name == "target" {
            continue;
        }
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("lstat {}: {e}", path.display()))?;
        if meta.is_dir() {
            walk_entries(root, &path, sink)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            sink(rel_string(rel));
        }
    }
    Ok(())
}

fn walk_files(
    root: &Path,
    dir: &Path,
    sink: &mut dyn FnMut(String, &Path) -> Result<(), String>,
) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("read_dir {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("lstat {}: {e}", path.display()))?;
        if meta.is_dir() {
            walk_files(root, &path, sink)?;
        } else if meta.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                sink(rel_string(rel), &path)?;
            }
        }
    }
    Ok(())
}

fn rel_string(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Keyword rules, most specific first; targets are relative to the v2 docs dir.
const RULES: &[(&str, &[&str])] = &[
    ("operations/mcp-servers.md", &["mcp"]),
    (
        "inconsistencies-tech-debt.md",
        &[
            "tech debt",
            "technical debt",
            "known issue",
            "known issues",
            "inconsistency",
            "inconsistencies",
            "limitations",
            "todo",
        ],
    ),
    (
        "glossary.md",
        &[
            "glossary",
            "terminology",
            "vocabulary",
            "terms",
            "definitions",
            "acronyms",
            "lexique",
        ],
    ),
    (
        "decisions.md",
        &["decision", "decisions", "adr", "adrs", "rationale"],
    ),
    (
        "workflow/commits.md",
        &[
            "commit",
            "commits",
            "commitizen",
            "conventional commits",
            "sign off",
            "signoff",
        ],
    ),
    (
        "workflow/pull-requests.md",
        &[
            "pull request",
            "pull requests",
            "pr",
            "prs",
            "merge request",
            "code review",
            "review",
            "reviews",
            "codeowners",
        ],
    ),
    (
        "workflow/tickets.md",
        &[
            "ticket", "tickets", "jira", "issue", "issues", "tracker", "backlog", "sprint",
        ],
    ),
    (
        "workflow/ci-cd.md",
        &[
            "ci",
            "cd",
            "ci cd",
            "pipeline",
            "pipelines",
            "deploy",
            "deployment",
            "deployments",
            "release",
            "releases",
            "github actions",
        ],
    ),
    (
        "testing-quality.md",
        &[
            "test",
            "tests",
            "testing",
            "qa",
            "quality",
            "coverage",
            "e2e",
            "unit tests",
        ],
    ),
    (
        "environments.md",
        &[
            "environment",
            "environments",
            "staging",
            "preprod",
            "production",
            "prod",
            "hosts",
            "urls",
            "recette",
        ],
    ),
    (
        "operations/debug-operations.md",
        &[
            "debug",
            "debugging",
            "troubleshoot",
            "troubleshooting",
            "operations",
            "runbook",
            "monitoring",
            "logs",
            "logging",
            "incident",
            "incidents",
            "on call",
        ],
    ),
    (
        "coding-rules.md",
        &[
            "coding",
            "code style",
            "style guide",
            "convention",
            "conventions",
            "naming",
            "lint",
            "linting",
            "formatting",
        ],
    ),
    (
        "repo-map.md",
        &[
            "structure",
            "layout",
            "repo map",
            "repository map",
            "directory",
            "directories",
            "folders",
            "tree",
        ],
    ),
    (
        "architecture/overview.md",
        &[
            "architecture",
            "design",
            "components",
            "data flow",
            "system",
            "ecosystem",
            "stack",
            "infrastructure",
            "overview",
        ],
    ),
    (
        "examples/README.md",
        &[
            "example", "examples", "recipe", "recipes", "how to", "tutorial", "cookbook",
        ],
    ),
    (
        "workflow/README.md",
        &[
            "workflow",
            "process",
            "contributing",
            "contribution",
            "git flow",
            "branch",
            "branches",
            "branching",
        ],
    ),
    (
        "AGENTS.md",
        &[
            "agents",
            "agent",
            "claude",
            "gemini",
            "do not",
            "don t",
            "never",
            "critical",
            "golden rule",
            "rules",
            "prerequisites",
            "setup",
            "installation",
            "install",
            "getting started",
            "quick start",
            "quickstart",
        ],
    ),
];

/// v2 target (relative to the docs dir) for a heading or a file context.
pub fn classify(text: &str) -> Option<&'static str> {
    let normalized: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let padded = format!(
        " {} ",
        normalized.split_whitespace().collect::<Vec<_>>().join(" ")
    );
    RULES.iter().find_map(|(target, keywords)| {
        keywords
            .iter()
            .any(|kw| padded.contains(&format!(" {kw} ")))
            .then_some(*target)
    })
}

/// Every target the rules can produce (used to check them against the template).
pub fn rule_targets() -> impl Iterator<Item = &'static str> {
    RULES.iter().map(|(target, _)| *target)
}

#[derive(Debug)]
struct Section<'a> {
    heading: Option<String>,
    start: usize,
    lines: Vec<&'a str>,
    kronn_managed: bool,
}

impl Section<'_> {
    fn end(&self) -> usize {
        self.start + self.lines.len().saturating_sub(1)
    }
    fn has_content(&self) -> bool {
        self.lines.iter().any(|l| !l.trim().is_empty())
    }
}

fn heading_text(line: &str) -> Option<String> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = line.trim_start_matches(' ');
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    if !(1..=2).contains(&hashes) {
        return None;
    }
    let after = &rest[hashes..];
    if !(after.is_empty() || after.starts_with(' ') || after.starts_with('\t')) {
        return None;
    }
    Some(after.trim().trim_end_matches('#').trim().to_string())
}

/// Split at H1/H2 headings outside code fences; Kronn's managed block is its own section.
fn split_sections(content: &str) -> Vec<Section<'_>> {
    use crate::core::root_agent_files::{KRONN_BLOCK_END, KRONN_BLOCK_START};
    let mut sections: Vec<Section> = Vec::new();
    let mut current = Section {
        heading: None,
        start: 1,
        lines: Vec::new(),
        kronn_managed: false,
    };
    let mut fence: Option<&str> = None;
    for (idx, line) in content.lines().enumerate() {
        let number = idx + 1;
        let trimmed = line.trim_start();
        if current.kronn_managed {
            current.lines.push(line);
            if line.trim() == KRONN_BLOCK_END {
                sections.push(std::mem::replace(
                    &mut current,
                    Section {
                        heading: None,
                        start: number + 1,
                        lines: Vec::new(),
                        kronn_managed: false,
                    },
                ));
            }
            continue;
        }
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            current.lines.push(line);
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            current.lines.push(line);
            continue;
        }
        let starts_block = line.trim() == KRONN_BLOCK_START;
        let heading = if starts_block {
            None
        } else {
            heading_text(line)
        };
        if starts_block || heading.is_some() {
            let next = Section {
                heading,
                start: number,
                lines: vec![line],
                kronn_managed: starts_block,
            };
            let previous = std::mem::replace(&mut current, next);
            if !previous.lines.is_empty() {
                sections.push(previous);
            }
            continue;
        }
        current.lines.push(line);
    }
    if !current.lines.is_empty() {
        sections.push(current);
    }
    sections
}

fn file_context(source: &str) -> String {
    let without_ext = source.rsplit_once('.').map_or(source, |(stem, _)| stem);
    // The docs-dir name itself carries no meaning ("docs/ci.md" → "ci").
    let mut parts: Vec<&str> = without_ext.split('/').collect();
    if parts.len() > 1 && DOCS_DIR_CANDIDATES.contains(&parts[0]) {
        parts.remove(0);
    }
    parts.join(" ")
}

fn is_prose(path: &str) -> bool {
    path.rsplit_once('.')
        .map(|(_, ext)| PROSE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Text of a prose source, or `None` when it must be left in place untouched
/// (symlink, binary, not UTF-8).
fn read_prose(root: &Path, rel: &str) -> Option<String> {
    let path = root.join(rel);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || !is_prose(rel) {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Build the merge plan.
///
/// `existing` is the base snapshot (repo-relative files): a v2 path in it is
/// never written, and only its doc paths are read as sources. Sources are read
/// from `root`.
pub fn plan_merge(
    root: &Path,
    existing: &BTreeSet<String>,
    template: &[TemplateFile],
    docs_dir: &str,
) -> Result<MergePlan, String> {
    let target_path = |rel: &str| format!("{docs_dir}/{rel}");
    let template_targets: BTreeSet<String> = template.iter().map(|t| target_path(&t.rel)).collect();

    let mut correspondence = Vec::new();
    let mut imports: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for source in source_paths(existing) {
        let Some(content) = read_prose(root, &source) else {
            correspondence.push(Correspondence {
                source,
                section: None,
                start_line: 0,
                end_line: 0,
                target: None,
                action: MappingAction::NotProse,
            });
            continue;
        };
        let context_target = classify(&file_context(&source));
        for section in split_sections(&content) {
            if !section.has_content() {
                continue;
            }
            if section.kronn_managed {
                correspondence.push(Correspondence {
                    source: source.clone(),
                    section: None,
                    start_line: section.start,
                    end_line: section.end(),
                    target: None,
                    action: MappingAction::KronnManaged,
                });
                continue;
            }
            let target = section
                .heading
                .as_deref()
                .and_then(classify)
                .or(context_target)
                .map(target_path)
                .filter(|t| template_targets.contains(t));
            let action = match &target {
                None => MappingAction::Unmapped,
                Some(t) if existing.contains(t) => MappingAction::TargetExists,
                Some(t) => {
                    let block = format!(
                        "### From `{source}` (lines {}-{})\n\n{}\n",
                        section.start,
                        section.end(),
                        section.lines.join("\n")
                    );
                    imports.entry(t.clone()).or_default().push(block);
                    MappingAction::Extracted
                }
            };
            correspondence.push(Correspondence {
                source: source.clone(),
                section: section.heading.clone(),
                start_line: section.start,
                end_line: section.end(),
                target,
                action,
            });
        }
    }

    let mut writes = Vec::new();
    let mut existing_targets = Vec::new();
    for file in template {
        let path = target_path(&file.rel);
        if existing.contains(&path) {
            existing_targets.push(path);
            continue;
        }
        let blocks = imports.remove(&path).unwrap_or_default();
        let content = if blocks.is_empty() {
            String::from_utf8(file.bytes.clone())
                .map_err(|_| format!("template {} is not UTF-8", file.rel))?
        } else {
            let base = String::from_utf8(file.bytes.clone())
                .map_err(|_| format!("template {} is not UTF-8", file.rel))?;
            render_with_imports(&base, &blocks)
        };
        writes.push(PlannedFile {
            path,
            content,
            imported_sections: blocks.len(),
        });
    }

    let mut plan = MergePlan {
        docs_dir: docs_dir.to_string(),
        correspondence,
        writes,
        existing_targets,
    };
    let report_path = target_path(REPORT_REL);
    if !existing.contains(&report_path) {
        let content = render_correspondence_markdown(&plan);
        plan.writes.push(PlannedFile {
            path: report_path,
            content,
            imported_sections: 0,
        });
        plan.writes.sort_by(|a, b| a.path.cmp(&b.path));
    }
    Ok(plan)
}

fn render_with_imports(base: &str, blocks: &[String]) -> String {
    let mut out = base.trim_end_matches('\n').to_string();
    out.push_str("\n\n## Imported from existing documentation\n\n");
    out.push_str(IMPORT_START);
    out.push_str(
        "\n> Copied verbatim by the template v2 merge; every source stays where it is. \
         Fold what is still true into the sections above, then delete this block.\n\n",
    );
    out.push_str(&blocks.join("\n"));
    out.push('\n');
    out.push_str(IMPORT_END);
    out.push('\n');
    out
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// The correspondence table, for the reviewer and the audit that follows.
pub fn render_correspondence_markdown(plan: &MergePlan) -> String {
    let mut out = String::from(
        "# Template v2 merge — correspondence table\n\n\
         Every existing doc, section by section, and the v2 file it belongs to. \
         Nothing was moved or rewritten: `extracted` sections were copied into new v2 files, \
         every other row stays only in its source.\n\n\
         | Source | Section | Lines | v2 file | Action |\n|---|---|---|---|---|\n",
    );
    for row in &plan.correspondence {
        let lines = if row.end_line == 0 {
            "—".to_string()
        } else {
            format!("{}-{}", row.start_line, row.end_line)
        };
        let action = match row.action {
            MappingAction::Extracted => "extracted",
            MappingAction::TargetExists => "kept in place (v2 file exists)",
            MappingAction::Unmapped => "unmapped, kept in place",
            MappingAction::NotProse => "not prose, kept in place",
            MappingAction::KronnManaged => "Kronn block, kept in place",
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            cell(&row.source),
            cell(row.section.as_deref().unwrap_or("(preamble)")),
            lines,
            row.target
                .as_deref()
                .map(|t| format!("`{}`", cell(t)))
                .unwrap_or_else(|| "—".into()),
            action
        ));
    }
    if !plan.existing_targets.is_empty() {
        out.push_str("\n## v2 files left as they are\n\n");
        for path in &plan.existing_targets {
            out.push_str(&format!("- `{path}`\n"));
        }
    }
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// sha256 of each listed file (lstat: a symlink hashes its target path, never its target).
pub fn fingerprint(root: &Path, paths: &[String]) -> BTreeMap<String, String> {
    paths
        .iter()
        .map(|rel| {
            let path = root.join(rel);
            let digest = match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() => std::fs::read_link(&path)
                    .map(|t| sha256_hex(t.to_string_lossy().as_bytes()))
                    .unwrap_or_else(|_| "unreadable-link".into()),
                Ok(_) => std::fs::read(&path)
                    .map(|b| sha256_hex(&b))
                    .unwrap_or_else(|_| "unreadable".into()),
                Err(_) => "missing".into(),
            };
            (rel.clone(), digest)
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOutcome {
    Created,
    /// Something is already there (a resumed merge): left untouched.
    AlreadyPresent,
}

/// Write the planned files create-only, then prove every source is byte-identical.
pub fn apply_plan(root: &Path, plan: &MergePlan) -> Result<Vec<(String, WriteOutcome)>, String> {
    let sources: Vec<String> = plan
        .correspondence
        .iter()
        .map(|row| row.source.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let before = fingerprint(root, &sources);
    let mut outcomes = Vec::new();
    for file in &plan.writes {
        let created = crate::core::fs_guard::guarded_write_new(
            root,
            &root.join(&file.path),
            file.content.as_bytes(),
        )?;
        outcomes.push((
            file.path.clone(),
            if created {
                WriteOutcome::Created
            } else {
                WriteOutcome::AlreadyPresent
            },
        ));
    }
    verify_unchanged(&before, &fingerprint(root, &sources))?;
    Ok(outcomes)
}

pub fn verify_unchanged(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Result<(), String> {
    let changed: Vec<&String> = before
        .iter()
        .filter(|(path, digest)| after.get(*path) != Some(*digest))
        .map(|(path, _)| path)
        .collect();
    if changed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "an existing doc changed during the merge, stopped: {}",
            changed
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Unified diff of a new file (every merge write is an addition).
pub fn addition_diff(path: &str, content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = format!("--- /dev/null\n+++ b/{path}\n");
    if lines.is_empty() {
        return out;
    }
    out.push_str(&format!("@@ -0,0 +1,{} @@\n", lines.len()));
    for line in lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if !content.ends_with('\n') {
        out.push_str("\\ No newline at end of file\n");
    }
    out
}

#[cfg(test)]
#[path = "docs_merge_test.rs"]
mod tests;
