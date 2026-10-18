//! Deterministic post-audit documentation budgets and routing diagnostics.

use crate::core::run_provenance::RunProvenance;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::core::anti_halluc::parse_line_bounds;

mod mechanical_fix;
pub use mechanical_fix::{mechanical_repair, AppliedFix, Repaired};

const CONFIG_FILE: &str = "docs/.kronn-document-budgets.json";
pub use crate::core::audit_state::REPORT_FILE;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct DocumentBudgets {
    pub adapter_max_words: usize,
    pub agents_md_max_words: usize,
    pub mandatory_path_max_words: usize,
    pub initially_routed_max_documents: usize,
    pub large_inventory_min_words: usize,
    pub exception: Option<BudgetException>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BudgetException {
    pub justification: String,
    pub provenance: String,
    pub reviewed_by: String,
    pub reviewed_at: String,
}

impl Default for DocumentBudgets {
    fn default() -> Self {
        Self {
            adapter_max_words: 220,
            agents_md_max_words: 800,
            mandatory_path_max_words: 1_200,
            initially_routed_max_documents: 2,
            large_inventory_min_words: 1_200,
            exception: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DocumentMeasure {
    pub path: String,
    pub bytes: usize,
    pub words: usize,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentLoadPath {
    pub adapter: String,
    pub documents: Vec<DocumentMeasure>,
    pub bytes: usize,
    pub words: usize,
    pub estimated_tokens: usize,
    pub initially_routed_documents: usize,
    pub top_contributors: Vec<DocumentMeasure>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub path: String,
    pub message: String,
    pub blocking: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DocumentaryOptimizationReport {
    pub phase: String,
    pub budgets: DocumentBudgets,
    pub agents: Vec<AgentLoadPath>,
    pub diagnostics: Vec<Diagnostic>,
}

impl DocumentaryOptimizationReport {
    pub fn blocking_diagnostics(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.blocking)
    }
}

/// The project's docs root (KT-1192): every `docs/` path below is relative
/// to it, written for the default and relocated per project.
fn docs_root(project: &Path) -> crate::core::docs_location::DocsRoot {
    crate::core::docs_location::DocsRoot::of(project)
}

/// A `docs/…` path of this module under the project's root.
fn under_root(project: &Path, default_path: &'static str) -> &'static str {
    docs_root(project).relocate_static(default_path)
}

pub fn analyze(project: &Path) -> Result<DocumentaryOptimizationReport, String> {
    analyze_with(project, None)
}

/// [`analyze`], with the `deps:` and `url:` citations also held against what
/// the audit run read (KT-1308). Outside a run there is nothing to hold them
/// against, and they stay honestly unchecked.
pub fn analyze_with(
    project: &Path,
    run: Option<&RunProvenance>,
) -> Result<DocumentaryOptimizationReport, String> {
    let budgets = load_budgets(project)?;
    let docs = markdown_files(&docs_root(project).dir(project))?;
    let adapters = root_adapters(project)?;
    let mut diagnostics = Vec::new();
    let mut agents = Vec::new();

    for adapter in &adapters {
        let adapter_rel = relative(project, adapter);
        let content = read(adapter)?;
        let adapter_measure = measure(&adapter_rel, &content);
        if contains_placeholder(&content) {
            diagnostics.push(diag(
                "placeholder",
                &adapter_rel,
                "unresolved template or TODO placeholder".into(),
            ));
        }
        let (owned, human) =
            crate::api::audit::anti_hallu_enforce::split_human_owned_sections(&content);
        inspect_citations(project, &adapter_rel, &owned, &mut diagnostics);
        let visible = visible_markdown(&content);
        for line in visible.lines().filter(|line| mutable_line_reference(line)) {
            diagnostics.push(diag(
                "mutable_line_reference",
                &adapter_rel,
                line.trim().to_string(),
            ));
        }
        if adapter_measure.words > budgets.adapter_max_words {
            diagnostics.push(diag(
                "adapter_budget",
                &adapter_rel,
                format!(
                    "{} words exceeds adapter budget {}",
                    adapter_measure.words, budgets.adapter_max_words
                ),
            ));
        }
        let mut routed = routed_markdown(project, adapter, &owned, &mut diagnostics);
        for (name, text) in &human {
            let mut found = Vec::new();
            inspect_citations(project, &adapter_rel, text, &mut found);
            routed.extend(routed_markdown(project, adapter, text, &mut found));
            demote_human_section(&adapter_rel, name, found, &mut diagnostics);
        }
        let entry = project.join(under_root(project, "docs/AGENTS.md"));
        if entry.is_file() && adapter != &entry && !routed.contains(&entry) {
            routed.insert(0, entry);
        }
        routed.dedup();
        let mut measures = vec![adapter_measure];
        measures.extend(
            routed
                .iter()
                .filter_map(|p| read(p).ok().map(|s| measure(&relative(project, p), &s))),
        );
        let words = measures.iter().map(|m| m.words).sum();
        if words > budgets.mandatory_path_max_words {
            diagnostics.push(diag(
                "mandatory_path_budget",
                &adapter_rel,
                format!(
                    "mandatory path is {words} words; budget is {}",
                    budgets.mandatory_path_max_words
                ),
            ));
        }
        if routed.len() > budgets.initially_routed_max_documents {
            diagnostics.push(diag(
                "initial_routing_budget",
                &adapter_rel,
                format!(
                    "{} documents routed initially; budget is {}",
                    routed.len(),
                    budgets.initially_routed_max_documents
                ),
            ));
        }
        let mut top = measures.clone();
        top.sort_by_key(|m| std::cmp::Reverse(m.words));
        top.truncate(5);
        agents.push(AgentLoadPath {
            adapter: adapter_rel,
            documents: measures.clone(),
            bytes: measures.iter().map(|m| m.bytes).sum(),
            words,
            estimated_tokens: measures.iter().map(|m| m.estimated_tokens).sum(),
            initially_routed_documents: routed.len(),
            top_contributors: top,
        });
    }

    inspect_docs(project, &docs, &adapters, &budgets, &mut diagnostics);
    inspect_stack_currency(project, &mut diagnostics);
    if let Some(run) = run {
        inspect_run_provenance(project, &docs, &adapters, run, &mut diagnostics);
    }
    for diagnostic in diagnostics.iter_mut().filter(|d| d.blocking) {
        if !kronn_owns(project, &diagnostic.path) {
            diagnostic.blocking = false;
            diagnostic
                .message
                .push_str(" (human-owned document: reported, not blocking)");
        }
    }
    diagnostics.sort_by(|a, b| (&a.path, &a.code, &a.message).cmp(&(&b.path, &b.code, &b.message)));
    Ok(DocumentaryOptimizationReport {
        phase: "documentary_optimization".into(),
        budgets,
        agents,
        diagnostics,
    })
}

pub fn analyze_and_write(project: &Path) -> Result<DocumentaryOptimizationReport, String> {
    let report = analyze(project)?;
    let body = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    let target = crate::core::audit_state::state_dir(project)?.join("document-optimization.json");
    if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{REPORT_FILE} is a symlink; refusing report write"));
    }
    crate::core::mcp_scanner::atomic_write(&target, &format!("{body}\n"))
        .map_err(|e| format!("write {REPORT_FILE}: {e}"))?;
    Ok(report)
}

/// Legacy documents moved aside by the template install keep their author.
const LEGACY_DOCS_PREFIX: &str = "docs/legacy/";

/// Whether Kronn wrote `rel` and may block a run on it, or ask an agent to
/// repair it. Moved legacy docs and root instruction files with human content
/// outside Kronn's managed block belong to their authors: an audit reports
/// their dead links but never fails on them.
pub fn kronn_owns(project: &Path, rel: &str) -> bool {
    if rel.starts_with(under_root(project, LEGACY_DOCS_PREFIX)) {
        return false;
    }
    if docs_root(project).contains(rel) {
        return true;
    }
    read(&project.join(rel)).is_ok_and(|content| {
        !crate::core::root_agent_files::has_content_outside_managed_block(&content)
    })
}

/// The same bounded, symlink-filtered surface used by the final audit gate.
pub(crate) fn document_paths(project: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = markdown_files(&docs_root(project).dir(project))?;
    paths.extend(root_adapters(project)?);
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn load_budgets(project: &Path) -> Result<DocumentBudgets, String> {
    let path = project.join(under_root(project, CONFIG_FILE));
    if !path.exists() {
        return Ok(DocumentBudgets::default());
    }
    let budgets: DocumentBudgets =
        serde_json::from_str(&read(&path)?).map_err(|e| format!("invalid {CONFIG_FILE}: {e}"))?;
    validate_budgets(&budgets)?;
    Ok(budgets)
}

fn validate_budgets(budgets: &DocumentBudgets) -> Result<(), String> {
    let defaults = DocumentBudgets::default();
    let relaxed = budgets.adapter_max_words > defaults.adapter_max_words
        || budgets.agents_md_max_words > defaults.agents_md_max_words
        || budgets.mandatory_path_max_words > defaults.mandatory_path_max_words
        || budgets.initially_routed_max_documents > defaults.initially_routed_max_documents
        || budgets.large_inventory_min_words > defaults.large_inventory_min_words;
    if budgets.adapter_max_words > 800
        || budgets.agents_md_max_words > 1_200
        || budgets.mandatory_path_max_words > 2_000
        || budgets.initially_routed_max_documents > 4
        || budgets.large_inventory_min_words > 2_000
    {
        return Err(format!(
            "invalid {CONFIG_FILE}: budget exception exceeds hard safety limits"
        ));
    }
    if relaxed {
        let Some(exception) = &budgets.exception else {
            return Err(format!(
                "invalid {CONFIG_FILE}: relaxed budgets require an explicitly reviewed exception"
            ));
        };
        if [
            &exception.justification,
            &exception.provenance,
            &exception.reviewed_by,
            &exception.reviewed_at,
        ]
        .iter()
        .any(|value| value.trim().is_empty())
        {
            return Err(format!(
                "invalid {CONFIG_FILE}: exception justification, provenance, reviewer and review date are required"
            ));
        }
    }
    Ok(())
}

fn inspect_docs(
    project: &Path,
    docs: &[PathBuf],
    adapters: &[PathBuf],
    budgets: &DocumentBudgets,
    out: &mut Vec<Diagnostic>,
) {
    let mut titles: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut linked = BTreeSet::new();
    for adapter in adapters {
        if let Ok(content) = read(adapter) {
            let rel = relative(project, adapter);
            let (owned, human) =
                crate::api::audit::anti_hallu_enforce::split_human_owned_sections(&content);
            collect_valid_links(project, adapter, &owned, &mut linked, Some(out));
            for (name, text) in human {
                let mut found = Vec::new();
                collect_valid_links(project, adapter, &text, &mut linked, Some(&mut found));
                demote_human_section(&rel, &name, found, out);
            }
        }
    }
    for path in docs {
        let Ok(content) = read(path) else { continue };
        let rel = relative(project, path);
        if rel == under_root(project, "docs/AGENTS.md")
            && word_count(&content) > budgets.agents_md_max_words
        {
            out.push(diag(
                "agents_md_budget",
                &rel,
                format!(
                    "{} words exceeds budget {}",
                    word_count(&content),
                    budgets.agents_md_max_words
                ),
            ));
        }
        if contains_placeholder(&content) {
            out.push(diag(
                "placeholder",
                &rel,
                "unresolved template or TODO placeholder".into(),
            ));
        }
        let visible = visible_markdown(&content);
        for line in visible.lines() {
            if let Some(title) = line.strip_prefix("# ") {
                titles
                    .entry(title.trim().to_lowercase())
                    .or_default()
                    .push(rel.clone());
            }
            if mutable_line_reference(line) {
                out.push(diag(
                    "mutable_line_reference",
                    &rel,
                    line.trim().to_string(),
                ));
            }
        }
        inspect_references(project, path, &rel, &content, &mut linked, out);
        if word_count(&content) >= budgets.large_inventory_min_words && !search_first(&content) {
            out.push(diag(
                "large_inventory_not_search_first",
                &rel,
                "large document is not marked search-first".into(),
            ));
        }
        if content.to_lowercase().contains("obsolete")
            || content.to_lowercase().contains("deprecated guide")
        {
            out.push(diag(
                "obsolete_guide",
                &rel,
                "document declares obsolete/deprecated guidance".into(),
            ));
        }
    }
    out.sort_by(|a, b| (&a.path, &a.code, &a.message).cmp(&(&b.path, &b.code, &b.message)));
    out.dedup_by(|a, b| a.path == b.path && a.code == b.code && a.message == b.message);
    for paths in titles.values().filter(|p| p.len() > 1) {
        for path in paths {
            out.push(diag(
                "duplicate_guide",
                path,
                format!(
                    "duplicate top-level title shared by {} documents",
                    paths.len()
                ),
            ));
        }
    }
    for path in docs {
        let rel = relative(project, path);
        if rel != under_root(project, "docs/AGENTS.md")
            && !rel.starts_with(under_root(project, "docs/tech-debt/"))
            && !rel.starts_with(under_root(project, "docs/reports/"))
            && !rel.starts_with(under_root(project, LEGACY_DOCS_PREFIX))
            && !linked.contains(&normalize(path))
        {
            out.push(diag(
                "orphan_document",
                &rel,
                "document is not linked from another documentation file".into(),
            ));
        }
    }
    unindexed_tds(project, docs, out);
}

/// KT-949 — an active TD sheet must be linked from the central index, or
/// from a dimension index the central index links. Settled sheets
/// (rejected, accepted decision) leave the index on purpose.
fn unindexed_tds(project: &Path, docs: &[PathBuf], out: &mut Vec<Diagnostic>) {
    let central = docs_root(project)
        .dir(project)
        .join(crate::core::td_index::CENTRAL_INDEX);
    let mut linked = BTreeSet::new();
    if let Ok(content) = read(&central) {
        collect_valid_links(project, &central, &content, &mut linked, None);
    }
    let dimension_indexes: Vec<PathBuf> = linked
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("inconsistencies-") && n.ends_with(".md"))
        })
        .cloned()
        .collect();
    for index in dimension_indexes {
        if let Ok(content) = read(&index) {
            collect_valid_links(project, &index, &content, &mut linked, None);
        }
    }
    for path in docs {
        let rel = relative(project, path);
        let Some(id) = rel
            .strip_prefix(under_root(project, "docs/tech-debt/"))
            .and_then(|name| name.strip_suffix(".md"))
            .filter(|id| crate::core::td_index::is_td_id(id))
        else {
            continue;
        };
        let active = read(path)
            .is_ok_and(|content| crate::core::td_index::parse_sheet(id, &content).is_active());
        if active && !linked.contains(&normalize(path)) {
            out.push(diag(
                "unindexed_td",
                &rel,
                format!(
                    "tech-debt sheet is not listed in {}",
                    docs_root(project).path(crate::core::td_index::CENTRAL_INDEX)
                ),
            ));
        }
    }
}

/// Links and citations of one document. A dead one inside an `owner="human"`
/// section is reported for its owner and never blocks: human prose never
/// holds a run back (KT-1079).
fn inspect_references(
    project: &Path,
    source: &Path,
    rel: &str,
    content: &str,
    linked: &mut BTreeSet<PathBuf>,
    out: &mut Vec<Diagnostic>,
) {
    let (owned, human) = crate::api::audit::anti_hallu_enforce::split_human_owned_sections(content);
    collect_valid_links(project, source, &owned, linked, Some(out));
    inspect_citations(project, rel, &owned, out);
    for (name, text) in human {
        let mut found = Vec::new();
        collect_valid_links(project, source, &text, linked, Some(&mut found));
        inspect_citations(project, rel, &text, &mut found);
        demote_human_section(rel, &name, found, out);
    }
}

fn demote_human_section(rel: &str, name: &str, found: Vec<Diagnostic>, out: &mut Vec<Diagnostic>) {
    for d in found.into_iter().filter(|d| d.blocking) {
        let item = diag(
            HUMAN_SECTION_REFERENCE,
            rel,
            format!(
                "human-owned section `{name}`: {}; reported for its owner, never blocking",
                d.message
            ),
        );
        if !out.contains(&item) {
            out.push(item);
        }
    }
}

/// A dead link or citation inside a human-owned section: a to-do for a person.
pub const HUMAN_SECTION_REFERENCE: &str = "human_section_reference";

/// Diagnostic code of a `deps:` / `url:` citation the audit run cannot back.
pub const UNBACKED_PROVENANCE: &str = "unbacked_provenance";

/// KT-1308 — every document, holding its `deps:` and `url:` citations against
/// what the run read. A human-owned section is reported, never blocking.
fn inspect_run_provenance(
    project: &Path,
    docs: &[PathBuf],
    adapters: &[PathBuf],
    run: &RunProvenance,
    out: &mut Vec<Diagnostic>,
) {
    let mut paths: Vec<&PathBuf> = docs.iter().chain(adapters).collect();
    paths.sort();
    paths.dedup();
    for path in paths {
        let Ok(content) = read(path) else { continue };
        let rel = relative(project, path);
        let (owned, human) =
            crate::api::audit::anti_hallu_enforce::split_human_owned_sections(&content);
        for problem in run.problems(&owned) {
            out.push(diag(UNBACKED_PROVENANCE, &rel, problem));
        }
        for (name, text) in human {
            let found: Vec<_> = run
                .problems(&text)
                .into_iter()
                .map(|problem| diag(UNBACKED_PROVENANCE, &rel, problem))
                .collect();
            demote_human_section(&rel, &name, found, out);
        }
    }
}

/// What the documentary gate makes of one `[src: file: …]` marker.
enum CitationCheck {
    /// Accepted as it is (a bare path to an existing binary file).
    Skip,
    /// Refused; the message says what to write instead.
    Defect(String),
    /// A text file and a line range inside it.
    Valid {
        path_part: String,
        line_part: String,
        max_line: usize,
        start: usize,
        end: usize,
    },
}

fn inspect_citations(project: &Path, rel: &str, content: &str, out: &mut Vec<Diagnostic>) {
    let visible = visible_markdown(content);
    for (at, marker) in visible.match_indices("[src: file:") {
        let tail = &visible[at + marker.len()..];
        let Some(raw) = tail.split(']').next() else {
            continue;
        };
        match check_citation(project, rel, raw.trim()) {
            CitationCheck::Skip => {}
            CitationCheck::Defect(detail) => out.push(diag("broken_citation", rel, detail)),
            CitationCheck::Valid {
                path_part,
                line_part,
                max_line,
                start,
                end,
            } => {
                if start == 1 && end >= max_line && max_line >= WHOLE_FILE_WARN_LINES {
                    let claim = visible[..at].rfind('\n').map_or(0, |i| i + 1);
                    let claim_end = visible[at..].find('\n').map_or(visible.len(), |i| at + i);
                    if !reads_as_absence(&visible[claim..claim_end]) {
                        let warning = diag(
                            WHOLE_FILE_CITATION,
                            rel,
                            format!(
                                "`{path_part}:{line_part}` cites all {max_line} lines of the file, \
                                 which only supports a claim that something is absent from it; for \
                                 a claim that something exists, cite the line that shows it"
                            ),
                        );
                        if !out.contains(&warning) {
                            out.push(warning);
                        }
                    }
                }
            }
        }
    }
    // `(src: file: …)` is not the grammar: the verifier and the drift check only
    // read the bracketed marker, so a parenthesised one would never be checked.
    // Kronn's own partial-step banner quotes the gate's reason with its brackets
    // turned round (`step_guard::partial_banner`): that is not the agent's text.
    let body = crate::api::audit::step_guard::strip_partial_marker(content);
    let visible = body.as_deref().map_or(visible, visible_markdown);
    for (at, marker) in visible.match_indices("(src: file:") {
        let tail = &visible[at + marker.len()..];
        let Some(raw) = tail.split(')').next().map(str::trim) else {
            continue;
        };
        if raw.contains('<') || raw.contains('>') || raw.is_empty() {
            continue;
        }
        let write = match check_citation(project, rel, raw) {
            CitationCheck::Defect(detail) => format!("and as written it is also refused: {detail}"),
            _ => format!("write [src: file: {raw}]"),
        };
        out.push(diag(
            "broken_citation",
            rel,
            format!(
                "parenthesised citation `(src: file: {raw})` is never verified: a citation is \
                 written between square brackets, {write}"
            ),
        ));
    }
}

/// Judge one marker's content. Every path and line this gate proposes in a
/// message is one it accepts: the split suggestion is made only once the
/// reference alone passes, and an empty or secret-looking file gets no range.
fn check_citation(project: &Path, rel: &str, raw: &str) -> CitationCheck {
    use CitationCheck::{Defect, Skip, Valid};
    if raw.contains('<') || raw.contains('>') {
        return Skip;
    }
    // Run O7: `.gitignore:3 — only logs/ ignored, not root-level` was told to
    // split references; the fix was to move the comment out of the marker.
    if let Some((reference, comment)) = raw.split_once(char::is_whitespace) {
        if reference.contains(':') && !reference.contains(',') {
            return Defect(match check_citation(project, rel, reference) {
                Defect(why) => format!(
                    "malformed file citation `{raw}`: a marker holds only `path:line`, and \
                     `{reference}` is itself refused: {why}"
                ),
                _ => format!(
                    "malformed file citation `{raw}`: a marker holds only `path:line`. \
                     Write [src: file: {reference}] and put `{}` outside the brackets",
                    comment.trim()
                ),
            });
        }
    }
    if raw.contains(',') {
        return Defect(format!(
            "malformed file citation `{raw}`: use one [src: file: path:line] marker per reference"
        ));
    }
    // A bare path is the citation form of a binary file, which has no lines.
    let Some((path_part, line_part)) = raw.rsplit_once(':') else {
        if is_project_dir(project, raw) {
            return Defect(format!(
                "malformed file citation `{raw}`: {}",
                directory_citation_message(raw)
            ));
        }
        return Defect(match cited_file(project, raw) {
            Some(path) if crate::core::anti_halluc::is_binary_file(&path) => return Skip,
            Some(path) if crate::acp::secret_files::is_secret_file(&path) => format!(
                "malformed file citation `{raw}`: this file may hold secrets, so no line of it is \
                 cited. To state that it exists or is tracked, write [src: file: {raw}:exists] \
                 and do not quote its content; otherwise name it in backticks outside any \
                 [src:] marker"
            ),
            Some(path) => match line_count(&path) {
                0 => format!(
                    "malformed file citation `{raw}`: the file is empty, so it has no line to \
                     cite. Say it is empty and name it in backticks outside any [src:] marker"
                ),
                lines => format!(
                    "malformed file citation `{raw}`: a text file is cited with the line \
                     that supports the claim, [src: file: {raw}:<line>]. To state that \
                     something is absent from it, cite the whole file as \
                     [src: file: {raw}:1-{lines}] and write the absence outside the brackets. \
                     If the claim is only that the file exists or is tracked, write \
                     [src: file: {raw}:exists] and do not quote its content"
                ),
            },
            None if ignored_path(project, raw) => format!(
                "malformed file citation `{raw}`: {}",
                ignored_citation_message(raw)
            ),
            None => format!(
                "malformed file citation `{raw}`: expected [src: file: path:line], \
                 or a bare path for an existing binary file{}",
                did_you_mean(project, rel, raw, None)
            ),
        });
    };
    let path_part = path_part.trim();
    let Some(path) = cited_file(project, path_part) else {
        // Run O7: `helpers/hbs/:1` was reported as a path that does not exist.
        return Defect(if is_project_dir(project, path_part) {
            directory_citation_message(path_part)
        } else if ignored_path(project, path_part) {
            format!(
                "citation path does not exist `{path_part}`: {}",
                ignored_citation_message(path_part)
            )
        } else {
            format!(
                "citation path does not exist `{path_part}`{}",
                did_you_mean(project, rel, path_part, Some(line_part.trim()))
            )
        });
    };
    // A presence citation (`path:exists`) claims the file is there, not what it
    // holds: no line to bound, and none to quote from a secret file.
    if crate::core::anti_halluc::is_presence_marker(line_part) {
        if crate::core::anti_halluc::is_tracked_marker(line_part)
            && !crate::core::anti_halluc::is_git_tracked(project, path_part)
        {
            return Defect(format!(
                "`{path_part}` is not tracked by git: write [src: file: {path_part}:exists] \
                 if the claim is only that the file is there"
            ));
        }
        return Skip;
    }
    if crate::core::anti_halluc::is_binary_file(&path) {
        return Defect(format!(
            "`{path_part}` is a binary file and has no lines: cite it as \
             [src: file: {path_part}] and name the page or section outside the brackets"
        ));
    }
    let max_line = line_count(&path);
    // An editor shows a newline-terminated file with one more, empty, line.
    let last_line = last_line_of(&path);
    match parse_line_bounds(without_anchor(line_part)) {
        None => Defect(format!(
            "citation line `{}` is not a line number: write a number (`{path_part}:12`) \
             or a range (`{path_part}:12-20`)",
            line_part.trim()
        )),
        Some((start, end))
            if crate::core::anti_halluc::range_is_citable(max_line, last_line, start, end) =>
        {
            Valid {
                path_part: path_part.to_string(),
                line_part: line_part.trim().to_string(),
                max_line,
                start,
                end,
            }
        }
        Some(_) => Defect(format!(
            "citation line `{}` is outside `{path_part}`, which has {max_line} line(s){}",
            line_part.trim(),
            did_you_mean(project, rel, path_part, Some(line_part.trim()))
        )),
    }
}

/// KT-1353 — a path git ignores (a vendored dependency, a build output) is
/// absent from a fresh clone, so "does not exist" would be the clone's accident,
/// not a fault of the citation: say why it cannot be checked.
fn ignored_path(project: &Path, cited: &str) -> bool {
    !Path::new(cited).is_absolute() && crate::core::anti_halluc::is_git_ignored(project, cited)
}

fn ignored_citation_message(cited: &str) -> String {
    format!(
        "`{cited}` is git-ignored (vendored or generated), so a fresh clone does not have it \
         and the citation cannot be verified. Cite a tracked file that shows the same fact \
         (a manifest, a lockfile, your own source), or name `{cited}` in backticks outside any \
         [src:] marker"
    )
}

/// A whole-file range this long is rarely the line that proves a claim.
const WHOLE_FILE_WARN_LINES: usize = 30;

/// A citation spanning a whole long file backs a claim that is not an absence.
pub const WHOLE_FILE_CITATION: &str = "whole_file_citation";

/// The claim around a whole-file citation says something is missing or absent.
fn reads_as_absence(claim: &str) -> bool {
    const MARKERS: [&str; 15] = [
        "no ", "not ", "n't", "never", "none", "nothing", "absent", "absence", "missing",
        "without", "lack", "aucun", "pas de", "pas d'", "sans ",
    ];
    let claim = format!(" {} ", claim.to_lowercase());
    MARKERS.iter().any(|marker| claim.contains(marker))
}

/// `12-20` of a `12-20#symbol` line spec: the anchor is checked by the
/// anti-hallucination gate, which owns the symbol search.
fn without_anchor(line_part: &str) -> &str {
    match line_part.rsplit_once('#') {
        Some((head, symbol))
            if crate::core::anti_halluc::is_anchor_symbol(symbol.trim())
                && crate::core::anti_halluc::parse_line_bounds(head).is_some() =>
        {
            head.trim_end()
        }
        _ => line_part,
    }
}

fn line_count(path: &Path) -> usize {
    read(path).map(|s| s.lines().count()).unwrap_or(0)
}

/// The last line a citation may end on: one past [`line_count`] for a
/// newline-terminated file, as the verifier reads it.
fn last_line_of(path: &Path) -> usize {
    crate::core::anti_halluc::last_citable_line_of(path)
}

/// Citation paths resolve from the repository root. A model often writes the
/// path relative to the docs root or to the citing document instead: name the
/// root-relative form of that other file so the next attempt fixes the path. A
/// candidate is proposed only if it holds the cited lines.
fn did_you_mean(project: &Path, citing: &str, cited: &str, line: Option<&str>) -> String {
    let spec = line.map(|l| parse_line_bounds(without_anchor(l)));
    let hint = did_you_mean_candidates(project, citing, cited, spec)
        .into_iter()
        .next();
    match (hint, spec, line) {
        (Some(path), Some(Some(_)), Some(line)) => {
            format!(". Paths are repository-root relative; did you mean `{path}:{line}`?")
        }
        (Some(path), Some(None), _) => {
            format!(". Paths are repository-root relative; did you mean `{path}:<line>`?")
        }
        (Some(path), _, _) => {
            format!(
                ". Paths are repository-root relative; {}",
                bare_path_form(project, &path)
            )
        }
        (None, _, _) => String::new(),
    }
}

/// Every other root-relative file the cited path may have meant (docs root, then
/// the citing document's folder), distinct and in that order. With a line spec
/// only a text file that holds the lines qualifies.
fn did_you_mean_candidates(
    project: &Path,
    citing: &str,
    cited: &str,
    spec: Option<Option<(usize, usize)>>,
) -> Vec<String> {
    if Path::new(cited).is_absolute() {
        return Vec::new();
    }
    let docs_dir = docs_root(project).path("");
    let citing_dir = Path::new(citing)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let own = repo_relative(cited);
    let mut found: Vec<String> = Vec::new();
    for candidate in [docs_dir.trim_end_matches('/').to_string(), citing_dir]
        .into_iter()
        .filter(|dir| !dir.is_empty())
        .filter_map(|dir| repo_relative(&format!("{dir}/{cited}")))
        .filter(|candidate| own.as_ref() != Some(candidate))
    {
        let full = project.join(&candidate);
        let holds = full.is_file()
            && !outside_project(project, &full)
            && contained_path(project, &full)
            && !crate::acp::secret_files::is_secret_file(&full)
            && match spec {
                Some(Some((start, end))) => {
                    !crate::core::anti_halluc::is_binary_file(&full)
                        && crate::core::anti_halluc::range_is_citable(
                            line_count(&full),
                            last_line_of(&full),
                            start,
                            end,
                        )
                }
                _ => true,
            };
        if holds && !found.contains(&candidate) {
            found.push(candidate);
        }
    }
    found
}

/// How to cite `path` when the model gave it without a line: only a binary file
/// is cited by its bare path, so a text file is offered a line or the whole file
/// and an empty one is not cited at all.
fn bare_path_form(project: &Path, path: &str) -> String {
    let full = project.join(path);
    if crate::core::anti_halluc::is_binary_file(&full) {
        return format!("did you mean `{path}`?");
    }
    match line_count(&full) {
        0 => format!(
            "`{path}` is empty, so it has no line to cite: say so and name it in backticks \
             outside any [src:] marker"
        ),
        lines => format!(
            "did you mean `{path}:<line>`, the line that supports the claim (or `{path}:1-{lines}` \
             for the whole file)?"
        ),
    }
}

/// `docs//AGENTS.md` and `docs/../AGENTS.md` as the canonical `AGENTS.md`;
/// `None` when the path climbs out of the repository.
fn repo_relative(path: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn root_adapters(project: &Path) -> Result<Vec<PathBuf>, String> {
    let names = [
        "AGENTS.md",
        "CLAUDE.md",
        "GEMINI.md",
        ".cursorrules",
        ".windsurfrules",
        ".clinerules",
        ".github/copilot-instructions.md",
        ".cursor/rules/repo-instructions.mdc",
        ".kiro/steering/instructions.md",
        ".vibe/instructions.md",
    ];
    let mut found: Vec<_> = names
        .iter()
        .map(|n| project.join(n))
        .filter(|p| {
            fs::symlink_metadata(p)
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        })
        .collect();
    let entry = project.join(under_root(project, "docs/AGENTS.md"));
    if found.is_empty() && entry.is_file() {
        found.push(entry);
    }
    Ok(found)
}

fn markdown_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let project_real = root
        .parent()
        .ok_or_else(|| format!("{} has no project parent", root.display()))?
        .canonicalize()
        .map_err(|e| format!("resolve project root: {e}"))?;
    let root_real = root
        .canonicalize()
        .map_err(|e| format!("resolve {}: {e}", root.display()))?;
    if !root_real.starts_with(&project_real) {
        return Err("docs directory resolves outside the project".into());
    }
    fn walk(
        dir: &Path,
        root_real: &Path,
        visited: &mut BTreeSet<PathBuf>,
        out: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        let real = dir
            .canonicalize()
            .map_err(|e| format!("resolve {}: {e}", dir.display()))?;
        if !real.starts_with(root_real) || !visited.insert(real) {
            return Ok(());
        }
        for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|e| format!("stat {}: {e}", path.display()))?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                walk(&path, root_real, visited, out)?;
            } else if metadata.is_file()
                && path
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("md"))
                // A `TEMPLATE.md` is a gabarit meant to be copied per-instance
                // (per sequence, per MCP server), never filled in place — it
                // is not project documentation and is excluded from the scan.
                && path.file_name().and_then(|n| n.to_str()) != Some("TEMPLATE.md")
            {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, &root_real, &mut BTreeSet::new(), &mut out)?;
    out.sort();
    Ok(out)
}

fn routed_markdown(
    project: &Path,
    adapter: &Path,
    content: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<PathBuf> {
    markdown_targets(content)
        .into_iter()
        .filter_map(|target| {
            if target.starts_with("http") || target.starts_with('#') {
                return None;
            }
            let clean = target.split('#').next().unwrap_or("");
            let path = adapter.parent().unwrap_or(project).join(clean);
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                return None;
            }
            if !contained_file(project, &path) {
                diagnostics.push(diag(
                    "broken_link",
                    &relative(project, adapter),
                    format!(
                        "broken routed document `{target}`{}",
                        match root_relative_fix(project, adapter, clean) {
                            Some(fix) => format!(
                                ": `{clean}` exists from the repository root — write `{fix}`"
                            ),
                            None => link_resolution_hint(project, adapter, clean),
                        }
                    ),
                ));
                None
            } else {
                Some(normalize(&path))
            }
        })
        .collect()
}

fn markdown_targets(content: &str) -> Vec<String> {
    let visible = visible_markdown(content);
    let mut definitions = BTreeMap::new();
    for line in visible.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(split) = rest.find("]:") {
                let id = rest[..split].trim().to_ascii_lowercase();
                let target = markdown_destination(rest[split + 2..].trim());
                if !id.is_empty() && !target.is_empty() {
                    definitions.insert(id, target);
                }
            }
        }
    }
    let bytes = visible.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b'(' {
            let mut depth = 1usize;
            let mut escaped = false;
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() {
                let b = bytes[end];
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'(' {
                    depth += 1;
                } else if b == b')' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                end += 1;
            }
            if depth == 0 {
                let target = markdown_destination(&visible[start..end]);
                if !target.is_empty() {
                    out.push(target);
                }
                i = end + 1;
                continue;
            }
        } else if bytes[i] == b']' && bytes[i + 1] == b'[' {
            if let Some(close) = visible[i + 2..].find(']') {
                let explicit_id = visible[i + 2..i + 2 + close].trim();
                let id = if explicit_id.is_empty() {
                    reference_label_before(&visible, i).unwrap_or_default()
                } else {
                    explicit_id.to_ascii_lowercase()
                };
                if let Some(target) = definitions.get(&id) {
                    out.push(target.clone());
                }
                i += close + 3;
                continue;
            }
        } else if bytes[i] == b']' && bytes[i + 1] != b':' {
            if let Some(id) = reference_label_before(&visible, i) {
                if let Some(target) = definitions.get(&id) {
                    out.push(target.clone());
                }
            }
        }
        i += 1;
    }
    out
}

fn reference_label_before(content: &str, close: usize) -> Option<String> {
    let open = content[..close].rfind('[')?;
    let label = content[open + 1..close].trim().to_ascii_lowercase();
    (!label.is_empty()).then_some(label)
}

fn markdown_destination(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('<') {
        return rest.split('>').next().unwrap_or("").to_string();
    }
    raw.split_whitespace().next().unwrap_or("").to_string()
}

fn collect_valid_links(
    project: &Path,
    source: &Path,
    content: &str,
    linked: &mut BTreeSet<PathBuf>,
    mut diagnostics: Option<&mut Vec<Diagnostic>>,
) {
    for target in markdown_targets(content) {
        if target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with('#')
        {
            continue;
        }
        let clean = target.split('#').next().unwrap_or("");
        let base = source.parent().unwrap_or(project);
        if let Some(resolved) = link_resolution(project, base, clean) {
            linked.insert(normalize(&resolved));
        } else if let Some(out) = diagnostics.as_deref_mut() {
            out.push(diag(
                "broken_link",
                &relative(project, source),
                match root_relative_fix(project, source, clean) {
                    Some(fix) => format!(
                        "broken link target `{target}`: links resolve from this document's \
                         folder, and `{clean}` exists from the repository root — write `{fix}`"
                    ),
                    None => format!(
                        "broken link target `{target}`{}",
                        link_resolution_hint(project, source, clean)
                    ),
                },
            ));
        }
    }
}

/// Where a link target lands when it resolves from `base` (as written, then
/// percent-decoded); `None` when it leaves the project or does not exist.
fn link_resolution(project: &Path, base: &Path, clean: &str) -> Option<PathBuf> {
    let unresolved = |path: &Path| outside_project(project, path) || !contained_path(project, path);
    let mut resolved = base.join(clean);
    if unresolved(&resolved) {
        // Markdown encodes spaces in link targets: `My%20File.docx`.
        if let Some(decoded) = crate::core::anti_halluc::percent_decoded_relative(clean) {
            resolved = base.join(decoded);
        }
    }
    (!unresolved(&resolved)).then_some(resolved)
}

/// The link a model meant when it wrote a repository-root path from inside a
/// folder (`docs/AGENTS.md` from `docs/`): the same file, relative to `source`.
fn root_relative_fix(project: &Path, source: &Path, clean: &str) -> Option<String> {
    let target = project.join(clean.trim_start_matches("./"));
    if clean.is_empty() || outside_project(project, &target) || !target.exists() {
        return None;
    }
    let dir: Vec<_> = source
        .parent()?
        .strip_prefix(project)
        .ok()?
        .components()
        .collect();
    let target = repo_relative(clean)?;
    let parts: Vec<&str> = target.split('/').collect();
    let shared = dir
        .iter()
        .zip(&parts)
        .take_while(|(a, b)| a.as_os_str() == std::ffi::OsStr::new(**b))
        .count()
        .min(parts.len() - 1);
    let ups = "../".repeat(dir.len() - shared);
    Some(format!("{ups}{}", parts[shared..].join("/")))
}

/// Where a dead relative link lands, and the link that exists. A model writes
/// `../tech-debt/TD-1.md` from `docs/` when the file is `docs/tech-debt/TD-1.md`:
/// naming the resolved path shows the extra `../`, and a link that exists is
/// proposed by dropping leading `../` and trying the document's folder and the
/// docs root.
fn link_resolution_hint(project: &Path, source: &Path, clean: &str) -> String {
    let source_rel = relative(project, source);
    let dir = source_rel.rsplit_once('/').map_or("", |(dir, _)| dir);
    let join = |base: &str, path: &str| {
        if base.is_empty() {
            path.to_string()
        } else {
            format!("{base}/{path}")
        }
    };
    let landing = match repo_relative(&join(dir, clean)) {
        Some(path) => format!("`{path}` (repository root)"),
        None => "a path outside the repository".to_string(),
    };
    let mut hint = format!(". From `{source_rel}` it resolves to {landing}, which does not exist");
    let tail: Vec<&str> = clean
        .split('/')
        .skip_while(|part| matches!(*part, ".." | "." | ""))
        .collect();
    if tail.is_empty() {
        return hint;
    }
    let tail = tail.join("/");
    let docs_dir = docs_root(project).path("");
    let found = [dir.to_string(), docs_dir.trim_end_matches('/').to_string()]
        .into_iter()
        .filter_map(|base| repo_relative(&join(&base, &tail)))
        .find(|candidate| {
            let full = project.join(candidate);
            full.exists()
                && !outside_project(project, &full)
                && contained_path(project, &full)
                && !crate::acp::secret_files::is_secret_file(&full)
        })
        .and_then(|candidate| root_relative_fix(project, source, &candidate));
    if let Some(fix) = found {
        hint.push_str(&format!("; did you mean `{fix}`?"));
    }
    hint
}

/// The project file a citation names, as written or percent-decoded; both
/// forms stay inside the project root.
fn cited_file(project: &Path, cited: &str) -> Option<PathBuf> {
    let candidates = std::iter::once(cited.to_string())
        .chain(crate::core::anti_halluc::percent_decoded_relative(cited));
    candidates
        .map(|c| project.join(c))
        .find(|path| !outside_project(project, path) && path.is_file())
}

fn is_project_dir(project: &Path, path: &str) -> bool {
    let path = project.join(path);
    !outside_project(project, &path) && path.is_dir()
}

fn directory_citation_message(path: &str) -> String {
    format!(
        "`{path}` is a directory, and a [src: file:] marker needs a file and a line. Cite \
         one file inside it as [src: file: path:line], or name the directory in backticks \
         without a marker"
    )
}

fn measure(path: &str, content: &str) -> DocumentMeasure {
    let words = word_count(content);
    DocumentMeasure {
        path: path.into(),
        bytes: content.len(),
        words,
        estimated_tokens: content.chars().count().div_ceil(4),
    }
}
/// Raw words, HTML comments included: the agent reads the file as written, so
/// a comment costs it exactly what visible text does.
fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}
pub(crate) fn contains_placeholder(s: &str) -> bool {
    crate::api::audit::validation::count_raw_placeholders(s) > 0
        || visible_markdown(s).lines().any(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("<!-- TODO:") || trimmed == "PLACEHOLDER"
        })
}
fn visible_markdown(s: &str) -> String {
    crate::core::anti_halluc::strip_inline_code(&crate::core::anti_halluc::strip_fenced_code(s))
}
fn mutable_line_reference(s: &str) -> bool {
    let s = crate::core::anti_halluc::normalize_range_dashes(s);
    s.contains("[src: file:")
        && s.contains('-')
        && s.split("[src: file:").skip(1).any(|p| {
            p.split(']')
                .next()
                .is_some_and(|c| c.rsplit(':').next().is_some_and(|n| n.contains('-')))
        })
}
fn search_first(s: &str) -> bool {
    let l = s.to_lowercase();
    l.contains("search-first") || l.contains("search first") || l.contains("use `rg`")
}
fn read(p: &Path) -> Result<String, String> {
    fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))
}
fn relative(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(c.as_os_str()),
        }
    }
    out
}
fn outside_project(root: &Path, p: &Path) -> bool {
    !normalize(p).starts_with(normalize(root).as_path())
}
fn contained_path(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    path.canonicalize().is_ok_and(|path| path.starts_with(root))
}
fn contained_file(root: &Path, path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_file() && !metadata.file_type().is_symlink() && contained_path(root, path)
    })
}
/// Diagnostic code of a Stack row without its currency facts.
pub const STACK_CURRENCY: &str = "stack_currency";

/// KT-819 DoD 8 — every row of the entry file's Stack table says what is in
/// place, the latest stable version and when support ends, each with its source
/// and date. A warning, never a blocker: a run with no live source honestly
/// writes `not verified: no source` and that passes.
fn inspect_stack_currency(project: &Path, diagnostics: &mut Vec<Diagnostic>) {
    let rel = under_root(project, "docs/AGENTS.md");
    let Ok(content) = read(&project.join(rel)) else {
        return;
    };
    for problem in stack_currency_problems(&content) {
        diagnostics.push(diag(STACK_CURRENCY, rel, problem));
    }
}

fn table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn has_iso_date(text: &str) -> bool {
    text.as_bytes().windows(10).any(|w| {
        w.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        })
    })
}

/// What the first table under the first "Stack" heading lacks. Empty when
/// there is no such table yet (the placeholder diagnostic covers an unfilled
/// one) or when every row is complete.
fn stack_currency_problems(content: &str) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    let is_heading = |line: &str| line.trim_start().starts_with('#');
    let Some(heading) = lines.iter().position(|line| {
        is_heading(line)
            && line
                .trim_start()
                .trim_start_matches('#')
                .to_ascii_lowercase()
                .contains("stack")
    }) else {
        return Vec::new();
    };
    let mut table: Vec<&str> = Vec::new();
    for line in &lines[heading + 1..] {
        if is_heading(line) {
            break;
        }
        if line.trim_start().starts_with('|') {
            table.push(line);
        } else if !table.is_empty() {
            break;
        }
    }
    if table.len() < 2 || table[0].contains("{{") {
        return Vec::new();
    }
    let header: Vec<String> = table_cells(table[0])
        .into_iter()
        .map(|cell| cell.to_ascii_lowercase())
        .collect();
    let column = |names: &[&str]| {
        header
            .iter()
            .position(|cell| names.iter().any(|name| cell.contains(name)))
    };
    let (Some(latest), Some(support)) = (
        column(&["latest"]),
        column(&["support", "eol", "end of life"]),
    ) else {
        return vec![
            "the Stack table has no `Latest stable` / `Support end` column: each row needs both, \
             with source and date, or `not verified: no source`"
                .into(),
        ];
    };
    let mut problems = Vec::new();
    for row in &table[1..] {
        let cells = table_cells(row);
        if cells
            .iter()
            .all(|c| c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
        {
            continue;
        }
        let name = cells.first().map(String::as_str).unwrap_or("");
        for (index, label) in [(latest, "Latest stable"), (support, "Support end")] {
            let cell = cells.get(index).map(String::as_str).unwrap_or("");
            let lowered = cell.to_ascii_lowercase();
            let honest = lowered.contains("not verified");
            let sourced = lowered.contains("[src:") && has_iso_date(cell);
            if !(honest || sourced) {
                problems.push(format!(
                    "Stack row `{name}`: `{label}` lacks its source and date (or `not verified: no source`)"
                ));
            }
        }
    }
    problems
}

fn diag(code: &str, path: &str, message: String) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        path: path.into(),
        message,
        blocking: is_blocking_code(code),
    }
}

/// Only an invented path — a link or citation pointing at something that
/// does not exist — is worth losing a whole audit run over. Volume,
/// structure (orphans/duplicates), citation ranges and "obsolete" markers
/// are real findings but surface as warnings instead: `full.rs` gates
/// validation-discussion creation and the drift baseline on
/// `blocking_diagnostics()` being empty (KT-840).
fn is_blocking_code(code: &str) -> bool {
    matches!(
        code,
        "broken_link" | "broken_citation" | UNBACKED_PROVENANCE
    )
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn a_dead_reference_inside_a_human_section_is_a_to_do_not_a_blocker() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("src/kept.rs"), "fn main() {}\n");
        write(
            &root.join("docs/AGENTS.md"),
            "# Projet\n\nAgent claim [src: file: src/gone.rs:1]\n\n\
             <!-- kronn:section name=\"team\" owner=\"human\" -->\n\
             Human claim [src: file: src/retired.rs:2] and [notes](missing.md).\n\
             Still true [src: file: src/kept.rs:1]\n\
             <!-- kronn:section:end -->\n",
        );
        let report = analyze(root).unwrap();
        let blocking: Vec<_> = report.blocking_diagnostics().collect();
        assert_eq!(blocking.len(), 1, "{blocking:?}");
        assert!(blocking[0].message.contains("src/gone.rs"), "{blocking:?}");
        let human: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == HUMAN_SECTION_REFERENCE)
            .collect();
        // The link is both a dead target and a dead routed document.
        assert_eq!(human.len(), 3, "{human:?}");
        assert!(human
            .iter()
            .all(|d| !d.blocking && d.path == "docs/AGENTS.md"));
        assert!(human
            .iter()
            .all(|d| d.message.starts_with("human-owned section `team`: ")));
        assert!(human.iter().any(|d| d.message.contains("`src/retired.rs`")));
        assert!(human.iter().any(|d| d.message.contains("missing.md")));
    }

    #[test]
    fn dead_links_in_human_documents_are_reported_but_never_block() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("docs/AGENTS.md"),
            "# Projet\n\nSee [guide](legacy/guide.md).\n",
        );
        // A hand-written guide whose relative link was valid before its move.
        write(
            &root.join("docs/legacy/guide.md"),
            "# Guide é\n\n[code](../src/x.rs)\n",
        );
        write(
            &root.join("CLAUDE.md"),
            "# Team rules\n\n[old](docs/missing.md)\n\n<!-- KRONN-MANAGED-BLOCK:START -->\nx\n<!-- KRONN-MANAGED-BLOCK:END -->\n",
        );
        let report = analyze(root).unwrap();
        let legacy = report
            .diagnostics
            .iter()
            .find(|d| d.path == "docs/legacy/guide.md" && d.code == "broken_link")
            .expect("the dead link is still reported");
        assert!(!legacy.blocking);
        assert!(legacy.message.contains("not blocking"));
        let adapter = report
            .diagnostics
            .iter()
            .find(|d| d.path == "CLAUDE.md" && d.code == "broken_link")
            .expect("adapter dead link reported");
        assert!(!adapter.blocking);
        assert_eq!(report.blocking_diagnostics().count(), 0);
        assert!(!report
            .diagnostics
            .iter()
            .any(|d| d.code == "orphan_document" && d.path.starts_with("docs/legacy/")));

        // The same dead link in a document Kronn owns still blocks.
        write(
            &root.join("docs/glossary.md"),
            "# Glossary\n\n[x](../src/x.rs)\n",
        );
        let report = analyze(root).unwrap();
        assert!(report
            .blocking_diagnostics()
            .any(|d| d.path == "docs/glossary.md"));
    }

    #[test]
    fn ownership_follows_the_managed_block() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("AGENTS.md"),
            "<!-- KRONN-MANAGED-BLOCK:START -->\nx\n<!-- KRONN-MANAGED-BLOCK:END -->\n",
        );
        write(&root.join("CLAUDE.md"), "Human rules only\n");
        assert!(kronn_owns(root, "AGENTS.md"));
        assert!(!kronn_owns(root, "CLAUDE.md"));
        assert!(kronn_owns(root, "docs/repo-map.md"));
        assert!(!kronn_owns(root, "docs/legacy/repo-map.md"));
        assert!(!kronn_owns(root, "GEMINI.md"), "unreadable is not owned");
    }

    #[test]
    fn a_root_relative_link_says_which_link_to_write() {
        // Run O7, step 1: `[..](docs/AGENTS.md)` inside docs/AGENTS.md was
        // reported as a bare "broken link target" through two retries.
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("docs/AGENTS.md"), "x\n");
        write(&tmp.path().join("package.json"), "{}\n");
        let source = tmp.path().join("docs/AGENTS.md");
        let mut out = Vec::new();
        let mut linked = BTreeSet::new();
        collect_valid_links(
            tmp.path(),
            &source,
            "[a](docs/AGENTS.md) [b](package.json) [c](nowhere.md)",
            &mut linked,
            Some(&mut out),
        );
        let messages: Vec<_> = out.iter().map(|d| d.message.clone()).collect();
        assert_eq!(messages.len(), 3, "{messages:#?}");
        assert!(
            messages[0].ends_with("write `AGENTS.md`"),
            "{}",
            messages[0]
        );
        assert!(
            messages[1].ends_with("write `../package.json`"),
            "{}",
            messages[1]
        );
        assert!(
            messages[2].starts_with(
                "broken link target `nowhere.md`. From `docs/AGENTS.md` it resolves to \
                 `docs/nowhere.md`"
            ),
            "{}",
            messages[2]
        );
    }

    #[test]
    fn a_directory_citation_says_what_to_do_instead() {
        // Run O6: "malformed file citation `helpers/`" sent qwen3.6:35b
        // re-reading sources for two attempts instead of dropping the marker.
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("helpers/a.js"), "x\n");
        let mut out = Vec::new();
        inspect_citations(
            tmp.path(),
            "docs/AGENTS.md",
            "Helpers [src: file: helpers/] and [src: file: nowhere] and [src: file: helpers/:3]",
            &mut out,
        );
        assert_eq!(out.len(), 3, "{out:#?}");
        assert!(
            out[2].message.contains("is a directory"),
            "{}",
            out[2].message
        );
        assert!(
            out[0].message.contains("is a directory"),
            "{}",
            out[0].message
        );
        assert!(out[0].message.contains("in backticks without a marker"));
        assert!(
            !out[1].message.contains("is a directory"),
            "{}",
            out[1].message
        );
        assert!(out[1].message.contains("expected [src: file: path:line]"));

        let mut out = Vec::new();
        inspect_citations(
            tmp.path(),
            "docs/x.md",
            "[src: file: helpers/a.js:1 — only logs/ ignored, not root] [src: file: a.js:1, b.js:2]",
            &mut out,
        );
        assert_eq!(out.len(), 2, "{out:#?}");
        assert!(
            out[0].message.contains(
                "Write [src: file: helpers/a.js:1] and put `— only logs/ ignored, not root` outside"
            ),
            "{}",
            out[0].message
        );
        assert!(out[1]
            .message
            .contains("one [src: file: path:line] marker per reference"));
    }

    #[test]
    fn small_context_onboarding_stays_within_default_budgets() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("AGENTS.md"),
            "Read [docs/AGENTS.md](docs/AGENTS.md).\n",
        );
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\n\nSearch-first.\n",
        );
        let report = analyze(tmp.path()).unwrap();
        assert_eq!(report.agents[0].initially_routed_documents, 1);
        assert!(
            report.blocking_diagnostics().next().is_none(),
            "{:#?}",
            report.diagnostics
        );
    }

    #[test]
    fn africanews_baseline_accepts_613_word_router_and_755_word_path() {
        let tmp = tempfile::tempdir().unwrap();
        let router = std::iter::repeat_n("router", 612)
            .collect::<Vec<_>>()
            .join(" ")
            + " [entry](docs/AGENTS.md)";
        let entry = std::iter::repeat_n("entry", 139)
            .collect::<Vec<_>>()
            .join(" ");
        write(&tmp.path().join("AGENTS.md"), &router);
        write(
            &tmp.path().join("docs/AGENTS.md"),
            &format!("# Entry\nSearch-first {entry}"),
        );
        write(
            &tmp.path().join("docs/.kronn-document-budgets.json"),
            r#"{
                "adapter_max_words": 620,
                "exception": {
                    "justification": "Reviewed Africanews baseline",
                    "provenance": "KT-524 fixture",
                    "reviewed_by": "test reviewer",
                    "reviewed_at": "2026-08-30"
                }
            }"#,
        );
        let report = analyze(tmp.path()).unwrap();
        assert_eq!(report.agents[0].documents[0].words, 613);
        assert_eq!(report.agents[0].words, 755);
        assert!(
            report.blocking_diagnostics().next().is_none(),
            "{:#?}",
            report.diagnostics
        );
    }

    #[test]
    fn only_invented_paths_block_the_rest_warns() {
        let tmp = tempfile::tempdir().unwrap();
        // Inside Kronn's managed block: Kronn owns this adapter.
        write(
            &tmp.path().join("AGENTS.md"),
            "<!-- KRONN-MANAGED-BLOCK:START -->\n[entry](docs/AGENTS.md) [missing](docs/no.md)\n<!-- KRONN-MANAGED-BLOCK:END -->\n",
        );
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\n{{TODO}} [bad](missing.md) [src: file: src/a.rs:1-2]",
        );
        write(
            &tmp.path().join("docs/orphan.md"),
            "# Orphan\nobsolete guide",
        );
        write(
            &tmp.path().join("docs/duplicate.md"),
            "# Orphan\nNot search-first. ",
        );
        let inventory = std::iter::repeat_n("inventory", 1_201)
            .collect::<Vec<_>>()
            .join(" ");
        write(
            &tmp.path().join("docs/inventory.md"),
            &format!("# Inventory\n{inventory}"),
        );
        write(
            &tmp.path().join("docs/.kronn-document-budgets.json"),
            r#"{"adapter_max_words":1,"agents_md_max_words":2,"mandatory_path_max_words":3,"initially_routed_max_documents":0}"#,
        );
        let diagnostics = analyze(tmp.path()).unwrap().diagnostics;
        // (code, expected blocking) — only an invented path (a link or
        // citation to something that does not exist) halts the run; every
        // other finding here is real but surfaces as a warning (KT-840).
        for (code, expect_blocking) in [
            ("adapter_budget", false),
            ("agents_md_budget", false),
            ("mandatory_path_budget", false),
            ("initial_routing_budget", false),
            ("placeholder", false),
            ("broken_link", true),
            ("broken_citation", true),
            ("orphan_document", false),
            ("obsolete_guide", false),
            ("mutable_line_reference", false),
            ("duplicate_guide", false),
            ("large_inventory_not_search_first", false),
        ] {
            let matches: Vec<_> = diagnostics.iter().filter(|d| d.code == code).collect();
            assert!(!matches.is_empty(), "missing {code}: {diagnostics:#?}");
            for d in matches {
                assert_eq!(d.blocking, expect_blocking, "{code}: {d:#?}");
            }
        }
    }

    #[test]
    fn grammar_examples_and_kronn_docs_do_not_create_false_diagnostics() {
        let template = include_str!("../../../templates/docs/AGENTS.md");
        assert!(contains_placeholder(template));
        assert!(!contains_placeholder(
            "Twig {{ asset('app.css') }} and prose `{{EXAMPLE_SLOT}}`"
        ));
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut template_citations = Vec::new();
        inspect_citations(
            project,
            "templates/docs/AGENTS.md",
            template,
            &mut template_citations,
        );
        assert!(template_citations.is_empty(), "{template_citations:?}");
        assert!(!visible_markdown(template)
            .lines()
            .any(mutable_line_reference));

        let actual = fs::read_to_string(project.join("docs/AGENTS.md")).unwrap();
        assert!(!contains_placeholder(&actual));
        let mut actual_citations = Vec::new();
        inspect_citations(project, "docs/AGENTS.md", &actual, &mut actual_citations);
        assert!(actual_citations.is_empty(), "{actual_citations:?}");
    }

    fn copy_dir_recursive(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let target = dst.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir_recursive(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    /// Builds a fresh project carrying only the shipped skeleton: the whole
    /// `templates/docs/` tree plus the anti-hallu spec that project bootstrap
    /// installs into `docs/conventions/` (`api/projects/template.rs:101-110`).
    fn skeleton_project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let templates_docs = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("templates/docs");
        copy_dir_recursive(&templates_docs, &tmp.path().join("docs"));
        write(
            &tmp.path().join("docs/conventions/agents-md-format-v1.md"),
            crate::core::anti_halluc::SPEC_AGENTS_MD_V1,
        );
        tmp
    }

    #[test]
    fn full_audit_reaches_completion_on_the_provided_skeleton() {
        // DoD #1 (KT-840): a Full audit on a blank repo that only carries
        // the provided skeleton (unfilled `{{...}}` placeholders and all)
        // must be able to reach Completed. `full.rs` gates
        // validation-discussion creation and the drift baseline write on
        // `blocking_diagnostics()` being empty (full.rs:1723-1798) — this
        // is that precondition.
        let tmp = skeleton_project();
        let report = analyze(tmp.path()).unwrap();
        assert!(
            report.blocking_diagnostics().next().is_none(),
            "{:#?}",
            report.diagnostics
        );
    }

    #[test]
    fn word_budgets_count_html_comments_because_the_agent_reads_them() {
        assert_eq!(word_count("one <!-- two three --> four"), 6);
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("<!--\n  été  naïve\n-->"), 4);
    }

    #[test]
    fn provided_skeleton_fits_its_word_budget_html_comments_included() {
        // The budget is measured in raw words, HTML comments included, because
        // that is what the agent reads and pays for (KT-934, replacing the
        // KT-840 visible-words rule).
        let tmp = skeleton_project();
        let content = fs::read_to_string(tmp.path().join("docs/AGENTS.md")).unwrap();
        let words = word_count(&content);
        let default_budget = DocumentBudgets::default().agents_md_max_words;
        assert!(
            words <= default_budget,
            "skeleton is {words} words; budget is {default_budget}"
        );
        let report = analyze(tmp.path()).unwrap();
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|d| d.code == "agents_md_budget"),
            "{:#?}",
            report.diagnostics
        );
    }

    fn stack_diagnostics(body: &str) -> Vec<Diagnostic> {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("docs/AGENTS.md"), body);
        analyze(tmp.path())
            .unwrap()
            .diagnostics
            .into_iter()
            .filter(|d| d.code == STACK_CURRENCY)
            .collect()
    }

    const STACK_HEAD: &str = "# Project\n\n## 6. Stack\n\n\
        | Technology | In place | Latest stable | Support end | Role |\n\
        |---|---|---|---|---|\n";

    #[test]
    fn a_stack_row_carries_in_place_latest_and_support_end_with_source_and_date() {
        // KT-819 DoD 8.
        let complete = format!(
            "{STACK_HEAD}\
             | PHP | 8.3.1 [src: file: composer.lock:3] | 8.4.1 [src: url: https://www.php.net/releases/] (read 2026-10-15) | 2026-12-31 [src: url: https://www.php.net/supported-versions.php] (read 2026-10-15) | API |\n\
             | Symfony | 7.3.6 [src: file: composer.lock:9] | 7.4.0 [src: deps: composer/symfony/symfony@2026-10-04] | not verified: no source | Framework |\n"
        );
        assert!(stack_diagnostics(&complete).is_empty());

        // A row whose cell has neither a dated source nor an honest "not verified".
        let missing = format!(
            "{STACK_HEAD}\
             | PHP | 8.3.1 [src: file: composer.lock:3] | 8.4 | 2026-12-31 | API |\n\
             | Redis | 7.2 [src: file: docker-compose.yml:4] | not verified: no source | not verified: no source | Cache |\n"
        );
        let found = stack_diagnostics(&missing);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found.iter().all(|d| d.message.contains("`PHP`")),
            "{found:?}"
        );
        assert!(found.iter().any(|d| d.message.contains("`Latest stable`")));
        assert!(found.iter().any(|d| d.message.contains("`Support end`")));
        // A date without a source is a claim, not a fact.
        assert!(found.iter().all(|d| !d.blocking));

        // A table with no currency column at all is one finding.
        let old_shape = "# Project\n\n## 6. Stack\n\n| Technology | Version | Role |\n|---|---|---|\n| PHP | 8.3 | API |\n";
        let found = stack_diagnostics(old_shape);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].message.contains("no `Latest stable`"));
    }

    #[test]
    fn an_unfilled_stack_block_raises_no_currency_finding() {
        // The placeholder diagnostic owns an unfilled block.
        assert!(stack_diagnostics("# Project\n\n## 6. Stack\n\n{{STACK}}\n").is_empty());
        let skeleton = skeleton_project();
        let report = analyze(skeleton.path()).unwrap();
        assert!(report.diagnostics.iter().all(|d| d.code != STACK_CURRENCY));
    }

    #[test]
    fn template_gabarit_files_are_excluded_from_the_scan() {
        // Objective (KT-840): `TEMPLATE.md` gabarits (per-flow sequence,
        // per-MCP server context) are copied per-instance, never filled in
        // place, and must not be scanned as project documentation.
        let tmp = skeleton_project();
        let docs = markdown_files(&tmp.path().join("docs")).unwrap();
        assert!(
            !docs
                .iter()
                .any(|p| p.file_name().and_then(|n| n.to_str()) == Some("TEMPLATE.md")),
            "{docs:?}"
        );
    }

    #[test]
    fn dated_reports_are_excluded_from_routing_diagnostics() {
        let tmp = skeleton_project();
        write(
            &tmp.path().join("docs/reports/2026-09-27-audit-note.md"),
            "# Audit note\n\nPoint-in-time result.\n",
        );
        let report = analyze(tmp.path()).unwrap();
        assert!(
            !report.diagnostics.iter().any(|d| {
                d.code == "orphan_document" && d.path == "docs/reports/2026-09-27-audit-note.md"
            }),
            "dated reports are intentionally outside T0-T2 routing: {:#?}",
            report.diagnostics
        );
    }

    #[test]
    fn relaxed_budgets_require_review_metadata_and_have_hard_caps() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first\n",
        );
        write(
            &tmp.path().join(CONFIG_FILE),
            r#"{"adapter_max_words": 620}"#,
        );
        assert!(analyze(tmp.path())
            .unwrap_err()
            .contains("explicitly reviewed"));
        write(
            &tmp.path().join(CONFIG_FILE),
            r#"{"adapter_max_words":18446744073709551615,"exception":{"justification":"x","provenance":"KT-524","reviewed_by":"reviewer","reviewed_at":"2026-08-30"}}"#,
        );
        assert!(analyze(tmp.path())
            .unwrap_err()
            .contains("hard safety limits"));
    }

    #[test]
    fn root_reference_links_with_titles_and_parentheses_prevent_orphans() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("AGENTS.md"),
            "[entry](docs/AGENTS.md \"entry title\") [guide][guide-ref]\n[guide-ref]: <docs/guide_(v1).md> \"Guide title\"\n",
        );
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first. [nested](guide_(v1).md#usage \"Nested title\")\n",
        );
        write(&tmp.path().join("docs/guide_(v1).md"), "# Guide\nSmall.\n");
        let report = analyze(tmp.path()).unwrap();
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|d| d.code == "broken_link" || d.code == "orphan_document"),
            "{:#?}",
            report.diagnostics
        );
    }

    #[test]
    fn collapsed_and_shortcut_reference_links_prevent_orphans() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("AGENTS.md"),
            "[entry](docs/AGENTS.md) [collapsed][] [shortcut]\n[collapsed]: docs/collapsed.md\n[shortcut]: docs/shortcut.md\n[ordinary text]\n",
        );
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first.\n",
        );
        write(
            &tmp.path().join("docs/collapsed.md"),
            "# Collapsed\nSmall.\n",
        );
        write(&tmp.path().join("docs/shortcut.md"), "# Shortcut\nSmall.\n");

        let report = analyze(tmp.path()).unwrap();
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|d| d.code == "broken_link" || d.code == "orphan_document"),
            "{:#?}",
            report.diagnostics
        );
        assert_eq!(
            markdown_targets("[ordinary text]\n[guide]: docs/guide.md\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn root_adapter_reference_links_report_broken_targets() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("AGENTS.md"),
            "[entry](docs/AGENTS.md) [missing][missing-ref]\n[missing-ref]: docs/missing_(v1).md \"Missing title\"\n",
        );
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first.\n```text\n[src: file: src/example.rs:10-20]\n```\n",
        );
        let report = analyze(tmp.path()).unwrap();
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "broken_link" && diagnostic.path == "AGENTS.md"
        }));
        assert!(!report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "mutable_line_reference"));
    }

    #[cfg(unix)]
    #[test]
    fn report_write_refuses_an_external_symlink_target() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let external = tempfile::NamedTempFile::new().unwrap();
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first\n",
        );
        fs::create_dir_all(tmp.path().join(crate::core::audit_state::STATE_DIR)).unwrap();
        symlink(external.path(), tmp.path().join(REPORT_FILE)).unwrap();
        let before = fs::read_to_string(external.path()).unwrap();
        let error = analyze_and_write(tmp.path()).unwrap_err();
        assert!(error.contains("is a symlink"), "{error}");
        assert_eq!(fs::read_to_string(external.path()).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn docs_traversal_does_not_follow_external_links_or_loops() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("docs/AGENTS.md"),
            "# Entry\nSearch-first\n",
        );
        write(
            &external.path().join("outside.md"),
            "# Outside\nPLACEHOLDER\n",
        );
        symlink(external.path(), tmp.path().join("docs/external")).unwrap();
        symlink(tmp.path().join("docs"), tmp.path().join("docs/loop")).unwrap();
        let files = markdown_files(&tmp.path().join("docs")).unwrap();
        assert_eq!(files, vec![tmp.path().join("docs/AGENTS.md")]);
        let report = analyze(tmp.path()).unwrap();
        assert!(!report
            .diagnostics
            .iter()
            .any(|d| d.path.contains("outside")));
    }

    /// A real .docx (a ZIP archive) and an all-ASCII PDF: one is caught by its
    /// bytes, the other by its header, neither by its extension.
    fn document_project() -> tempfile::TempDir {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        let mut docx = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        docx.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        docx.write_all(b"<w:document><w:body><w:p>Budget</w:p></w:body></w:document>")
            .unwrap();
        let docx = docx.finish().unwrap().into_inner();
        fs::create_dir_all(project.join("docs")).unwrap();
        fs::write(project.join("Budget 2026.docx"), docx).unwrap();
        fs::write(
            project.join("brief.pdf"),
            "%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n",
        )
        .unwrap();
        write(&project.join("notes.md"), "one\ntwo\n");
        write(&project.join("fake.docx"), "plain text in disguise\n");
        fs::write(tmp.path().join("secret.bin"), b"\x00\x01binary outside").unwrap();
        tmp
    }

    fn citation_messages(project: &Path, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        inspect_citations(project, "docs/AGENTS.md", text, &mut out);
        out.into_iter().map(|d| d.message).collect()
    }

    #[test]
    fn a_presence_citation_of_a_secret_file_needs_no_line() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        fs::create_dir_all(project.join(".ssh")).unwrap();
        write(
            &project.join(".ssh/deploy-key"),
            "-----BEGIN OPENSSH PRIVATE KEY-----\nsecret\n-----END OPENSSH PRIVATE KEY-----\n",
        );
        write(&project.join("yarn-error.log"), "Error: boom\n");
        let messages = citation_messages(
            &project,
            "key [src: file: .ssh/deploy-key:exists] log [src: file: yarn-error.log:exists] \
             and binary [src: file: brief.pdf:exists].",
        );
        assert!(messages.is_empty(), "{messages:#?}");
        // A bare secret path is still refused, and the refusal names the presence form.
        let messages = citation_messages(&project, "[src: file: .ssh/deploy-key]");
        assert_eq!(messages.len(), 1, "{messages:#?}");
        assert!(
            messages[0].contains("may hold secrets")
                && messages[0].contains("[src: file: .ssh/deploy-key:exists]"),
            "{}",
            messages[0]
        );
    }

    #[test]
    fn a_tracked_citation_is_checked_against_the_git_index() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        write(&project.join("yarn-error.log"), "Error: boom\n");
        write(&project.join("local.log"), "Error: local\n");
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=Doc Test",
                    "-c",
                    "user.email=doc@example.invalid",
                ])
                .args(args)
                .current_dir(&project)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["add", "yarn-error.log"]);
        let text = "[src: file: yarn-error.log:tracked] [src: file: local.log:tracked]";
        let messages = citation_messages(&project, text);
        assert_eq!(messages.len(), 1, "{messages:#?}");
        assert!(
            messages[0].contains("local.log") && messages[0].contains("not tracked by git"),
            "{}",
            messages[0]
        );
        // The same file, claimed only to exist, needs no index entry.
        assert!(citation_messages(&project, "[src: file: local.log:exists]").is_empty());
    }

    #[test]
    fn a_content_claim_with_a_bare_path_is_still_refused_and_the_message_names_the_presence_form() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        let messages = citation_messages(&project, "[src: file: notes.md]");
        assert_eq!(messages.len(), 1, "{messages:#?}");
        assert!(
            messages[0].contains("[src: file: notes.md:<line>]")
                && messages[0].contains("[src: file: notes.md:exists]"),
            "{}",
            messages[0]
        );
    }

    #[test]
    fn a_citation_into_a_git_ignored_path_is_told_why_it_cannot_be_checked() {
        // KT-1353 (africanews): committed sheets cite `application/vendor/...`,
        // which git ignores and a clone does not have.
        let tmp = document_project();
        let project = tmp.path().join("proj");
        write(&project.join(".gitignore"), "application/vendor/\n");
        write(&project.join("notes.md"), "one\ntwo\n");
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&project)
            .status()
            .unwrap();
        assert!(status.success());
        let vendor = "application/vendor/symfony/symfony/src/Route.php";
        let messages = citation_messages(&project, &format!("[src: file: {vendor}:12]"));
        assert_eq!(messages.len(), 1, "{messages:#?}");
        assert!(
            messages[0].contains(&format!("citation path does not exist `{vendor}`"))
                && messages[0].contains("git-ignored")
                && messages[0].contains("fresh clone"),
            "{}",
            messages[0]
        );
        // The bare form gets the same verdict, and a path git does not ignore is
        // still the plain "does not exist".
        let bare = citation_messages(&project, &format!("[src: file: {vendor}]"));
        assert!(bare[0].contains("git-ignored"), "{bare:#?}");
        let missing = citation_messages(&project, "[src: file: application/src/Nope.php:3]");
        assert_eq!(missing.len(), 1, "{missing:#?}");
        assert!(!missing[0].contains("git-ignored"), "{}", missing[0]);
        // Present in the author's checkout, the ignored file still resolves:
        // the verdict on the line stays the one the citation always got.
        write(&project.join(vendor), "a\nb\nc\n");
        assert!(citation_messages(&project, &format!("[src: file: {vendor}:2]")).is_empty());
    }

    #[test]
    fn a_presence_citation_still_needs_the_file_to_exist_inside_the_project() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        let messages = citation_messages(
            &project,
            "[src: file: missing.key:exists] [src: file: ../secret.bin:exists] \
             [src: file: docs:exists]",
        );
        assert_eq!(messages.len(), 3, "{messages:#?}");
    }

    #[test]
    fn a_binary_file_is_cited_by_bare_path() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        let messages = citation_messages(
            &project,
            "Budget [src: file: Budget 2026.docx], encoded [src: file: Budget%202026.docx] \
             and brief [src: file: brief.pdf].",
        );
        assert!(messages.is_empty(), "{messages:#?}");
    }

    #[test]
    fn a_line_on_a_binary_file_says_to_cite_the_bare_path() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        let messages = citation_messages(
            &project,
            "[src: file: Budget 2026.docx:1] [src: file: brief.pdf:2]",
        );
        assert_eq!(messages.len(), 2, "{messages:#?}");
        for (message, path) in messages.iter().zip(["Budget 2026.docx", "brief.pdf"]) {
            assert!(
                message.contains("is a binary file and has no lines")
                    && message.contains(&format!("[src: file: {path}]")),
                "{message}"
            );
        }
    }

    #[test]
    fn a_text_file_still_needs_a_line() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        // `fake.docx` is text whatever its extension says.
        let messages = citation_messages(&project, "[src: file: notes.md] [src: file: fake.docx]");
        assert_eq!(messages.len(), 2, "{messages:#?}");
        assert!(
            messages[0].contains("a text file is cited with the line")
                && messages[0].contains("[src: file: notes.md:<line>]"),
            "{}",
            messages[0]
        );
        assert!(messages[1].contains("[src: file: fake.docx:<line>]"));
        assert!(
            citation_messages(&project, "[src: file: notes.md:2] [src: file: fake.docx:1]")
                .is_empty()
        );
    }

    #[test]
    fn a_doc_relative_citation_names_the_repo_root_path() {
        // KT-1307: `AGENTS.md:94` meant `docs/AGENTS.md`; `repo-map.md:1` was
        // relative to the citing document.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        write(&project.join("AGENTS.md"), "pointer\n");
        write(&project.join("docs/AGENTS.md"), &"line\n".repeat(100));
        write(&project.join("docs/architecture/repo-map.md"), "a\nb\n");
        let mut out = Vec::new();
        inspect_citations(
            project,
            "docs/architecture/x.md",
            "[src: file: AGENTS.md:94] [src: file: repo-map.md:1] [src: file: map.md:1] \
             [src: file: repo-map.md]",
            &mut out,
        );
        let messages: Vec<_> = out.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(messages.len(), 4, "{messages:#?}");
        assert!(
            messages[0].contains("which has 1 line(s)")
                && messages[0].contains("did you mean `docs/AGENTS.md:94`?"),
            "{}",
            messages[0]
        );
        assert!(
            messages[1].contains("does not exist")
                && messages[1].contains("repository-root relative")
                && messages[1].contains("did you mean `docs/architecture/repo-map.md:1`?"),
            "{}",
            messages[1]
        );
        assert!(!messages[2].contains("did you mean"), "{}", messages[2]);
        assert!(
            messages[3].contains("did you mean `docs/architecture/repo-map.md:<line>`"),
            "{}",
            messages[3]
        );
    }

    #[test]
    fn a_hint_never_points_outside_the_project() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        let messages = citation_messages(&project, "[src: file: ../secret.bin:1]");
        assert_eq!(messages.len(), 1, "{messages:#?}");
        assert!(!messages[0].contains("did you mean"), "{}", messages[0]);
    }

    #[test]
    fn dash_ranges_accept_en_and_em_dash_and_non_numeric_says_so() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        write(&project.join("long.md"), &"x\n".repeat(30));
        assert!(citation_messages(
            &project,
            "None of it is stale: [src: file: long.md:17\u{2013}23] [src: file: long.md:1\u{2014}30] \
             [src: file: long.md:5-6]"
        )
        .is_empty());
        let messages = citation_messages(
            &project,
            "[src: file: long.md:extends] [src: file: long.md:5-] [src: file: long.md:17\u{2013}32] \
             [src: file: long.md:0] [src: file: long.md:9-3]",
        );
        assert_eq!(messages.len(), 5, "{messages:#?}");
        assert!(messages[0].contains("not a line number"), "{}", messages[0]);
        assert!(messages[1].contains("not a line number"), "{}", messages[1]);
        for outside in &messages[2..] {
            assert!(
                outside.contains("is outside `long.md`, which has 30 line(s)"),
                "{outside}"
            );
        }
    }

    #[test]
    fn a_hint_is_a_canonical_path_that_holds_the_cited_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        write(&project.join("AGENTS.md"), "pointer\n");
        write(&project.join("docs/AGENTS.md"), &"line\n".repeat(100));
        write(&project.join("docs/architecture/repo-map.md"), "a\nb\n");
        let hint = |cited: &str| {
            let mut out = Vec::new();
            inspect_citations(
                project,
                "docs/architecture/x.md",
                &format!("[src: file: {cited}]"),
                &mut out,
            );
            out.remove(0).message
        };
        // `docs/../AGENTS.md` is the 1-line root pointer: it does not hold
        // line 90, so the hint skips it for the file that does.
        let climbing = hint("../AGENTS.md:90");
        assert!(
            climbing.contains("did you mean `docs/AGENTS.md:90`?"),
            "{climbing}"
        );
        let doubled = hint("architecture//repo-map.md:1");
        assert!(
            doubled.contains("did you mean `docs/architecture/repo-map.md:1`?"),
            "{doubled}"
        );
        // A line past the end of the only candidate: no hint.
        assert!(!hint("repo-map.md:7").contains("did you mean"));
        // A non-numeric line still names the file, never the bad line.
        assert!(hint("repo-map.md:extends")
            .contains("did you mean `docs/architecture/repo-map.md:<line>`?"));
        // The hint is never a path that climbs out of the repository.
        assert!(!hint("../../../etc/hosts:1").contains("did you mean"));
    }

    #[test]
    fn a_symbol_anchor_follows_the_line_spec_like_in_the_verifier() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        write(&project.join("long.md"), &"x\n".repeat(30));
        assert!(citation_messages(
            &project,
            "[src: file: long.md:5-6#retry_once] [src: file: long.md:7#Type.method] \
             [src: file: long.md:5\u{2013}6#retry_once]"
        )
        .is_empty());
        let messages = citation_messages(
            &project,
            "[src: file: long.md:32#retry_once] [src: file: long.md:#retry_once]",
        );
        assert_eq!(messages.len(), 2, "{messages:#?}");
        assert!(
            messages[0].contains("is outside `long.md`"),
            "{}",
            messages[0]
        );
        assert!(messages[1].contains("not a line number"), "{}", messages[1]);
    }

    #[test]
    fn a_whole_file_range_backing_a_positive_claim_warns_without_blocking() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        write(&project.join("long.md"), &"x\n".repeat(60));
        write(&project.join("short.md"), "a\nb\n");
        let run = |text: &str| {
            let mut out = Vec::new();
            inspect_citations(&project, "docs/AGENTS.md", text, &mut out);
            out
        };
        let warned = run("The cache is shared [src: file: long.md:1-60]");
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert_eq!(warned[0].code, WHOLE_FILE_CITATION);
        assert!(!warned[0].blocking);
        assert!(
            warned[0].message.contains("all 60 lines"),
            "{}",
            warned[0].message
        );
        // An absence is what a whole-file range is for.
        assert!(run("No audit step exists in it [src: file: long.md:1\u{2013}60]").is_empty());
        assert!(run("The workflow never audits [src: file: long.md:1-60]").is_empty());
        // A short file, or a real range, is an ordinary citation.
        assert!(run("Two lines [src: file: short.md:1-2]").is_empty());
        assert!(run("The cache is shared [src: file: long.md:10-20]").is_empty());
    }

    #[test]
    fn a_dash_range_is_a_mutable_line_reference() {
        assert!(mutable_line_reference("see [src: file: a.rs:2-4]"));
        assert!(mutable_line_reference("see [src: file: a.rs:2\u{2013}4]"));
        assert!(mutable_line_reference("see [src: file: a.rs:2\u{2014}4]"));
        assert!(!mutable_line_reference("see [src: file: a.rs:2]"));
    }

    #[test]
    fn an_absence_is_cited_as_the_whole_file_range() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        let messages = citation_messages(&project, "[src: file: notes.md]");
        assert_eq!(messages.len(), 1, "{messages:#?}");
        assert!(
            messages[0].contains("[src: file: notes.md:1-2]")
                && messages[0].contains("absent")
                && !messages[0].contains("remove"),
            "{}",
            messages[0]
        );
        // The proposed form is accepted by the gate.
        assert!(citation_messages(&project, "no audit step [src: file: notes.md:1-2]").is_empty());
    }

    #[test]
    fn traversal_and_absolute_citations_stay_refused() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        let outside = tmp.path().join("secret.bin");
        let text = format!(
            "[src: file: ../secret.bin] [src: file: ..%2Fsecret.bin] [src: file: {}] \
             [src: file: %2F{}] [src: file: ../secret.bin:1] [src: file: %2E%2E/secret.bin:1]",
            outside.display(),
            outside.display().to_string().trim_start_matches('/'),
        );
        let messages = citation_messages(&project, &text);
        assert_eq!(messages.len(), 6, "{messages:#?}");
    }

    #[test]
    fn percent_encoded_links_resolve_inside_the_project_only() {
        let tmp = document_project();
        let project = tmp.path().join("proj");
        let source = project.join("docs/AGENTS.md");
        write(&source, "x\n");
        let mut out = Vec::new();
        let mut linked = BTreeSet::new();
        collect_valid_links(
            &project,
            &source,
            "[budget](../Budget%202026.docx) [brief](<../brief.pdf>) \
             [up](..%2F..%2Fsecret.bin) [abs](%2Fetc%2Fhosts) [raw](../../secret.bin)",
            &mut linked,
            Some(&mut out),
        );
        assert!(linked.contains(&normalize(&project.join("Budget 2026.docx"))));
        assert!(linked.contains(&normalize(&project.join("brief.pdf"))));
        let messages: Vec<_> = out.iter().map(|d| d.message.clone()).collect();
        assert_eq!(messages.len(), 3, "{messages:#?}");
        assert!(messages.iter().all(|m| m.starts_with("broken link target")));
    }

    #[test]
    fn a_td_sheet_missing_from_the_central_index_is_reported_until_resync() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("docs/AGENTS.md"),
            "# Projet\n\n[debt](inconsistencies-tech-debt.md)\n",
        );
        write(
            &root.join("docs/inconsistencies-tech-debt.md"),
            "# Debt\n\n## Current list\n\n| ID | Problem | Area | Severity |\n|---|---|---|---|\n| TD-20261001-listed | [x](tech-debt/TD-20261001-listed.md) | Backend | High |\n| TD-20261002-via-dim | see [security](inconsistencies-security.md) | Security | High |\n",
        );
        write(
            &root.join("docs/inconsistencies-security.md"),
            "| ID |\n|---|\n| [TD-20261002-via-dim](tech-debt/TD-20261002-via-dim.md) |\n",
        );
        for id in [
            "TD-20261001-listed",
            "TD-20261002-via-dim",
            "TD-20261003-missing",
        ] {
            write(
                &root.join(format!("docs/tech-debt/{id}.md")),
                "# T\n- **Status**: Inferred\n",
            );
        }
        write(
            &root.join("docs/tech-debt/TD-20261004-rejected.md"),
            "# T\n- **Status**: Rejected\n",
        );
        let unindexed = |root: &Path| -> Vec<String> {
            analyze(root)
                .unwrap()
                .diagnostics
                .into_iter()
                .filter(|d| d.code == "unindexed_td")
                .inspect(|d| assert!(!d.blocking, "a gap in the index is a warning"))
                .map(|d| d.path)
                .collect()
        };
        assert_eq!(
            unindexed(root),
            vec!["docs/tech-debt/TD-20261003-missing.md"]
        );
        crate::core::td_index::resync(
            &root.join("docs"),
            crate::core::adr::Trust::RunBaseline {
                decisions: "",
                records: "",
            },
        )
        .unwrap();
        assert!(unindexed(root).is_empty());
    }

    /// Every `[src: file: …]` the gate writes into a message, in order.
    fn proposed_markers(message: &str) -> Vec<String> {
        message
            .split("[src: file: ")
            .skip(1)
            .filter_map(|tail| tail.split(']').next())
            .filter(|raw| !raw.contains('<') && !raw.contains('>'))
            .map(str::to_string)
            .collect()
    }

    fn accepted(project: &Path, raw: &str) -> bool {
        !matches!(
            check_citation(project, "docs/AGENTS.md", raw),
            CitationCheck::Defect(_)
        )
    }

    #[test]
    fn the_empty_line_after_a_final_newline_is_a_citable_end_bound() {
        // Claude Code's Read tool numbers a 3-line, newline-terminated file up to 4.
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("ended.txt"), "a\nb\nc\n");
        write(&tmp.path().join("open.txt"), "a\nb\nc");
        let messages = citation_messages(
            tmp.path(),
            "[src: file: ended.txt:1-4] [src: file: ended.txt:3-4] [src: file: ended.txt:1-5] \
             [src: file: ended.txt:5] [src: file: open.txt:1-3] [src: file: open.txt:1-4]",
        );
        assert_eq!(messages.len(), 3, "{messages:#?}");
        assert!(messages[0].contains("`1-5` is outside `ended.txt`, which has 3 line(s)"));
        assert!(messages[1].contains("`5` is outside `ended.txt`"));
        assert!(messages[2].contains("`1-4` is outside `open.txt`, which has 3 line(s)"));
    }

    #[test]
    fn a_range_starting_on_the_empty_line_cites_nothing_and_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("ended.txt"), "a\nb\nc\n");
        write(&tmp.path().join("empty.txt"), "");
        write(&tmp.path().join("blank.txt"), "\n");
        assert!(accepted(tmp.path(), "ended.txt:3-4"));
        assert!(!accepted(tmp.path(), "ended.txt:4"));
        assert!(!accepted(tmp.path(), "ended.txt:4-4"));
        assert!(!accepted(tmp.path(), "empty.txt:1"));
        assert!(!accepted(tmp.path(), "empty.txt:1-1"));
        assert!(accepted(tmp.path(), "blank.txt:1-2"));
        assert!(!accepted(tmp.path(), "blank.txt:2"));
    }

    #[test]
    fn a_missing_line_never_gets_a_bare_path_the_gate_refuses_as_a_did_you_mean() {
        // `docs/` holds the file the model meant; without a line, a bare path is
        // only valid for a binary file, so a text file is offered a line.
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("docs/notes.md"), "a\nb\nc\n");
        write(&tmp.path().join("docs/empty.md"), "");
        write(&tmp.path().join("docs/.npmrc"), "registry=x\n");
        let messages = citation_messages(
            tmp.path(),
            "[src: file: notes.md] [src: file: empty.md] [src: file: .npmrc]",
        );
        assert_eq!(messages.len(), 3, "{messages:#?}");
        assert!(
            messages[0].contains("did you mean `docs/notes.md:<line>`"),
            "{}",
            messages[0]
        );
        assert!(!messages[0].contains("`docs/notes.md`?"), "{}", messages[0]);
        assert!(accepted(tmp.path(), "docs/notes.md:1-3"));
        assert!(
            messages[0].contains("`docs/notes.md:1-3`"),
            "{}",
            messages[0]
        );
        assert!(
            messages[1].contains("`docs/empty.md` is empty"),
            "{}",
            messages[1]
        );
        assert!(!messages[1].contains("`docs/empty.md`?"), "{}", messages[1]);
        assert!(!messages[2].contains("docs/.npmrc"), "{}", messages[2]);
    }

    #[test]
    fn the_verifier_and_the_gate_agree_on_the_end_bound() {
        use crate::core::anti_halluc::{verify_source_marker, SourceStatus};
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("ended.txt"), "a\nb\nc\n");
        write(&tmp.path().join("open.txt"), "a\nb\nc");
        let status = |raw: &str| verify_source_marker(raw, Some(tmp.path())).status;
        assert_eq!(status("file: ended.txt:1-4"), SourceStatus::Verified);
        assert_eq!(status("file: ended.txt:1-5"), SourceStatus::OutOfBounds);
        assert_eq!(status("file: open.txt:1-3"), SourceStatus::Verified);
        assert_eq!(status("file: open.txt:1-4"), SourceStatus::OutOfBounds);
        // A range that starts on the empty line N+1 cites nothing; an empty file
        // has no line at all (the verifier used to round it up to one).
        write(&tmp.path().join("empty.txt"), "");
        assert_eq!(status("file: ended.txt:3-4"), SourceStatus::Verified);
        assert_eq!(status("file: ended.txt:4"), SourceStatus::OutOfBounds);
        assert_eq!(status("file: empty.txt:1"), SourceStatus::OutOfBounds);
        assert_eq!(status("file: empty.txt"), SourceStatus::Verified);
    }

    #[test]
    fn a_comment_after_a_non_numeric_line_is_not_split_into_a_citation_the_gate_refuses() {
        // Apollo step 12: told to write `[src: file: docs/AGENTS.md:Focused]`, then
        // refused with "Focused is not a line number".
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("docs/AGENTS.md"), "a\nb\n");
        write(&tmp.path().join("helpers/a.js"), "x\n");
        let messages = citation_messages(
            tmp.path(),
            "[src: file: docs/AGENTS.md:Focused on routing] [src: file: nowhere.md:3 — gone] \
             [src: file: helpers/a.js:1 — only logs ignored]",
        );
        assert_eq!(messages.len(), 3, "{messages:#?}");
        assert!(messages[0].contains("is itself refused"), "{}", messages[0]);
        assert!(
            messages[0].contains("is not a line number"),
            "{}",
            messages[0]
        );
        assert!(messages[1].contains("is itself refused"), "{}", messages[1]);
        assert!(messages[1].contains("does not exist"), "{}", messages[1]);
        assert!(messages[2].contains("Write [src: file: helpers/a.js:1]"));
        for message in &messages {
            for marker in proposed_markers(message) {
                if message.contains(&format!("Write [src: file: {marker}]")) {
                    assert!(accepted(tmp.path(), &marker), "{marker}: {message}");
                }
            }
        }
    }

    #[test]
    fn every_range_the_gate_proposes_for_a_bare_text_file_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("ended.txt"), "a\nb\nc\n");
        write(&tmp.path().join("open.txt"), "a\nb");
        write(&tmp.path().join("blank.txt"), "\n");
        for bare in ["ended.txt", "open.txt", "blank.txt"] {
            let messages = citation_messages(tmp.path(), &format!("[src: file: {bare}]"));
            assert_eq!(messages.len(), 1, "{messages:#?}");
            let proposals = proposed_markers(&messages[0]);
            assert!(!proposals.is_empty(), "{}", messages[0]);
            for marker in proposals {
                assert!(accepted(tmp.path(), &marker), "{marker}: {}", messages[0]);
            }
        }
    }

    #[test]
    fn an_empty_file_is_not_offered_a_range_it_would_refuse() {
        // Docroms step 3: `1-1` was suggested for an empty file, then refused
        // ("0 line(s)").
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("empty.txt"), "");
        let messages = citation_messages(tmp.path(), "[src: file: empty.txt]");
        assert_eq!(messages.len(), 1, "{messages:#?}");
        assert!(messages[0].contains("the file is empty"), "{}", messages[0]);
        assert!(!messages[0].contains(":1-"), "{}", messages[0]);
        assert!(proposed_markers(&messages[0])
            .iter()
            .all(|marker| accepted(tmp.path(), marker)));
    }

    #[test]
    fn a_secret_looking_file_is_never_offered_a_range() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join(".ssh/id_deploy"), "k1\nk2\n");
        write(&tmp.path().join(".env"), "TOKEN=x\n");
        let messages =
            citation_messages(tmp.path(), "[src: file: .ssh/id_deploy] [src: file: .env]");
        assert_eq!(messages.len(), 2, "{messages:#?}");
        for message in &messages {
            assert!(message.contains("may hold secrets"), "{message}");
            assert!(!message.contains(":1-"), "{message}");
        }
        // The did-you-mean never points at a secret-looking file either.
        write(&tmp.path().join("docs/.env"), "TOKEN=x\nOTHER=y\n");
        let messages = citation_messages(tmp.path(), "[src: file: .env.nope:1-2]");
        assert!(
            messages.iter().all(|m| !m.contains("did you mean")),
            "{messages:#?}"
        );
    }

    #[test]
    fn the_partial_banner_quoting_a_gate_reason_is_not_a_parenthesised_citation() {
        // The banner turns the reason's brackets round and cuts it at a limit, so
        // a quoted `[src: file: path:line]` ends as `(src: file: path:star)`.
        let tmp = tempfile::tempdir().unwrap();
        let banner = crate::api::audit::step_guard::partial_banner(
            4,
            "Use ONE [src: file: path:line] or [src: file: path:start-end] per reference",
        );
        assert!(banner.contains("(src: file: path:line)"), "{banner}");
        let messages = citation_messages(tmp.path(), &format!("{banner}# Rules\n"));
        assert!(messages.is_empty(), "{messages:#?}");
        let messages = citation_messages(
            tmp.path(),
            &format!("{banner}# Rules\nBad (src: file: nope.rs:1)\n"),
        );
        assert_eq!(messages.len(), 1, "{messages:#?}");
    }

    #[test]
    fn a_parenthesised_citation_is_refused_with_the_bracketed_form() {
        // Docroms wrote 90 of them; the verifier only reads `[src:`, so they
        // were never checked (2 were out of range).
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("code.rs"), "a\nb\n");
        let messages = citation_messages(
            tmp.path(),
            "Good (src: file: code.rs:1-2) bad (src: file: code.rs:9) gone (src: file: nope.rs:1) \
             fine [src: file: code.rs:2] and `(src: file: code.rs:1)` in code",
        );
        assert_eq!(messages.len(), 3, "{messages:#?}");
        assert!(
            messages[0].contains("write [src: file: code.rs:1-2]"),
            "{}",
            messages[0]
        );
        assert!(messages[1].contains("also refused"), "{}", messages[1]);
        assert!(
            messages[1].contains("is outside `code.rs`"),
            "{}",
            messages[1]
        );
        assert!(messages[2].contains("does not exist"), "{}", messages[2]);
        assert!(accepted(tmp.path(), "code.rs:1-2"));
    }

    #[test]
    fn a_broken_relative_link_names_where_it_lands_and_the_link_that_exists() {
        // Apollo step 11: `../tech-debt/TD-1.md` from docs/ failed three times.
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("docs/AGENTS.md"), "x\n");
        write(&tmp.path().join("docs/tech-debt/TD-1.md"), "# t\n");
        write(&tmp.path().join("docs/architecture/overview.md"), "# o\n");
        let source = tmp.path().join("docs/architecture/overview.md");
        let mut out = Vec::new();
        let mut linked = BTreeSet::new();
        collect_valid_links(
            tmp.path(),
            &source,
            "[a](../tech-debt/TD-9.md) [b](../../tech-debt/TD-1.md) [c](../../../out.md)",
            &mut linked,
            Some(&mut out),
        );
        let messages: Vec<_> = out.iter().map(|d| d.message.clone()).collect();
        assert_eq!(messages.len(), 3, "{messages:#?}");
        assert!(
            messages[0].contains(
                "resolves to `docs/tech-debt/TD-9.md` (repository root), which does not exist"
            ),
            "{}",
            messages[0]
        );
        assert!(!messages[0].contains("did you mean"), "{}", messages[0]);
        assert!(
            messages[1].contains("resolves to `tech-debt/TD-1.md` (repository root)")
                && messages[1].ends_with("did you mean `../tech-debt/TD-1.md`?"),
            "{}",
            messages[1]
        );
        assert!(
            messages[2].contains("a path outside the repository"),
            "{}",
            messages[2]
        );
    }
}
