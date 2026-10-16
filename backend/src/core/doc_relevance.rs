//! Relevance of the AI documentation to the code as it is now (KT-1168).
//!
//! Citation fingerprints say which cited lines moved, changed or vanished.
//! They do not say whether a *section* of the docs still describes anything
//! that exists: the doc grows stale quietly and pollutes the agents' context.
//! This module gives every Markdown section a verdict, deterministically and
//! without a model, from what the section anchors itself to:
//!
//! * its `[src: file: …]` citations, read through the fingerprint report
//!   (reused, not recomputed: `citation_drift::check` stays the one source of
//!   truth for changed / moved / removed lines);
//! * the repository paths its prose names in backticks (`backend/src/x.rs`),
//!   which carry no fingerprint but can plainly stop existing. A path is only
//!   called gone when it resolves nowhere: not from the project root, not
//!   from the doc's own folder, not as the tail of any project file (docs
//!   write `lib/routes.ts` for `frontend/src/lib/routes.ts`), and is not a
//!   file Kronn generates or git ignores. A line that tells history ("the
//!   former `x.rs`") or speaks of a file's absence, a section titled as a
//!   proposal, a naming pattern (`TD-YYYYMMDD-slug.md`), an example (and the
//!   list right under it), a doc still
//!   holding the template's placeholders, and the changelog / release docs
//!   are not claims.
//!
//! Verdicts, from the section's anchors only:
//!
//! * `anchors_intact`: every anchor still resolves (a moved line counts: Kronn
//!   re-anchors it). Kronn found no contradiction; it did NOT prove the prose
//!   true: that stays for the agent pass.
//! * `outdated`: a cited line changed or fell past the end of its file, or
//!   some (not all) of the files it points at are gone.
//! * `no_longer_relevant`: every anchor points at a file that is gone: the
//!   code the section describes no longer exists.
//! * `unanchored`: no citation and no path: Kronn cannot measure it. These
//!   are listed and handed to the agent pass as unverifiable (check or anchor).
//!
//! The signal is a count, so it can be tracked over time: sections per
//! verdict, the share of anchored sections whose anchors are intact, and the
//! sections *untouched since the code they describe changed* (the section's
//! own digest is the one recorded at validation while its anchors broke).

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::citation_drift::{self, CitationDriftReport, CitationState, SectionCitations};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SectionVerdict {
    AnchorsIntact,
    Outdated,
    NoLongerRelevant,
    Unanchored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// A `[src: file: …]` citation, judged by its recorded fingerprint.
    Citation,
    /// A repository path named in prose, judged by its existence only.
    PathReference,
}

/// One anchor of a section that does not resolve any more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RelevanceEvidence {
    pub kind: EvidenceKind,
    /// The citation label or path, as the doc writes it.
    pub reference: String,
    pub state: CitationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionKind {
    /// The file is there but the claim may no longer hold: re-read it.
    Recheck,
    /// The file is gone: cite its replacement or drop the claim.
    ReplaceOrRemove,
    /// Everything the section describes is gone: drop the section.
    RemoveSection,
    /// Nothing anchors the section: check it against the code, then cite it.
    CheckOrAnchor,
}

/// A correction Kronn proposes. Nothing is applied by itself: the agent pass
/// the human launches carries it out, and the human validates the result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProposedCorrection {
    pub kind: CorrectionKind,
    pub reference: String,
    pub proposal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SectionRelevance {
    pub doc: String,
    /// The Markdown heading; empty for the text before the first heading.
    pub heading: String,
    pub verdict: SectionVerdict,
    /// The anchors that no longer resolve.
    pub evidence: Vec<RelevanceEvidence>,
    pub corrections: Vec<ProposedCorrection>,
    /// The section's text is exactly what was recorded at validation: nobody
    /// revised it although the code it describes changed.
    pub untouched_since_code_changed: bool,
}

/// What is measured, over every section of every doc.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RelevanceCounts {
    #[ts(type = "number")]
    pub sections: usize,
    #[ts(type = "number")]
    pub anchors_intact: usize,
    #[ts(type = "number")]
    pub outdated: usize,
    #[ts(type = "number")]
    pub no_longer_relevant: usize,
    #[ts(type = "number")]
    pub unanchored: usize,
    /// Outdated or no-longer-relevant sections nobody touched since the
    /// validation that recorded them.
    #[ts(type = "number")]
    pub untouched_stale: usize,
    /// Anchors (citations and prose paths) whose file is gone.
    #[ts(type = "number")]
    pub missing_references: usize,
    /// The part of `missing_references` that is prose only: no citation, so
    /// no fingerprint, broke: only this check sees it.
    #[ts(type = "number")]
    pub prose_missing: usize,
    /// `anchors_intact` over the anchored sections; `None` when none is
    /// anchored. Intact anchors are not proof the prose is true.
    #[ts(type = "number | null")]
    pub anchored_intact_pct: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RelevanceReport {
    pub counts: RelevanceCounts,
    /// Only the sections that need attention (`outdated`, `no_longer_relevant`).
    pub sections: Vec<SectionRelevance>,
    /// The sections Kronn cannot measure (no citation, no path): handed to
    /// the agent pass to check or anchor. They do not raise the badge.
    pub unanchored: Vec<SectionRelevance>,
}

impl RelevanceReport {
    pub fn needs_attention(&self) -> bool {
        self.counts.outdated + self.counts.no_longer_relevant > 0
    }

    /// The fingerprint badge, raised when a section is outdated or gone with
    /// no broken citation to show for it. `Unknown` stays: nothing has been
    /// validated yet, so there is no baseline to be stale against.
    pub fn escalate(&self, state: citation_drift::SourcesState) -> citation_drift::SourcesState {
        use citation_drift::SourcesState;
        match state {
            SourcesState::Clean if self.needs_attention() => SourcesState::NeedsReverification,
            other => other,
        }
    }

    /// The sections of one doc that need attention.
    pub fn of_doc<'a>(&'a self, doc: &'a str) -> impl Iterator<Item = &'a SectionRelevance> {
        self.sections.iter().filter(move |s| s.doc == doc)
    }
}

// ─── Sections ─────────────────────────────────────────────────────────────

struct Section {
    /// Unique within the doc: the heading, suffixed when it repeats.
    key: String,
    heading: String,
    text: String,
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

fn heading_of(line: &str) -> Option<String> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &t[hashes..];
    rest.starts_with(' ').then(|| rest.trim().to_string())
}

/// Flat split on Markdown headings (any level), fenced code left alone.
fn split_sections(content: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut in_fence = false;
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut current = Section {
        key: String::new(),
        heading: String::new(),
        text: String::new(),
    };
    for line in content.split('\n') {
        if is_fence(line) {
            in_fence = !in_fence;
        }
        let heading = if in_fence { None } else { heading_of(line) };
        if let Some(heading) = heading {
            if !(current.text.trim().is_empty() && current.heading.is_empty()) {
                sections.push(current);
            }
            let n = seen.entry(heading.clone()).or_insert(0);
            *n += 1;
            let key = if *n == 1 {
                heading.clone()
            } else {
                format!("{heading}#{n}")
            };
            current = Section {
                key,
                heading,
                text: String::new(),
            };
        }
        current.text.push_str(line);
        current.text.push('\n');
    }
    if !(current.text.trim().is_empty() && current.heading.is_empty()) {
        sections.push(current);
    }
    sections
}

fn digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The digest of every section, as recorded with the citation fingerprints.
pub(crate) fn section_digests(content: &str) -> BTreeMap<String, String> {
    split_sections(content)
        .into_iter()
        .map(|s| (s.key, digest(&s.text)))
        .collect()
}

// ─── Prose paths ──────────────────────────────────────────────────────────

fn strip_line_suffix(token: &str) -> &str {
    match token.rsplit_once(':') {
        Some((head, tail))
            if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit() || c == '-') =>
        {
            head
        }
        _ => token,
    }
}

/// A backticked token that reads as a repository file path, or `None`.
fn path_token(raw: &str) -> Option<String> {
    let token = strip_line_suffix(raw.trim());
    if token.is_empty()
        || token.len() > 200
        || !token.contains('/')
        || token.starts_with(['/', '.', '~'])
        || token.ends_with('/')
        || token.contains("..")
        || token.contains("//")
    {
        return None;
    }
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '@' | '+'))
    {
        return None;
    }
    let name = token.rsplit('/').next()?;
    let ext = name.rsplit_once('.')?.1;
    let plausible_ext = !ext.is_empty()
        && ext.len() <= 5
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && ext.chars().any(|c| c.is_ascii_alphabetic());
    plausible_ext.then(|| token.to_string())
}

/// Words that make a line history ("the former `x.rs`"), not a claim about
/// the code as it is now. Matched as whole words, outside inline code.
const HISTORY_WORDS: &[&str] = &[
    "former",
    "formerly",
    "previously",
    "legacy",
    "removed",
    "deleted",
    "deprecated",
    "replaced",
    "renamed",
    "superseded",
    "ancien",
    "ancienne",
    "anciens",
    "anciennement",
    "supprimé",
    "supprimée",
    "remplacé",
    "remplacée",
    "renommé",
    "renommée",
    "obsolète",
];

/// Phrases of a line that speaks of a path's absence or a file still to be
/// made: it does not claim the path exists.
const ABSENCE_PHRASES: &[&str] = &[
    "no longer",
    "used to ",
    "n'existe plus",
    "n'existe pas",
    "does not exist",
    "doesn't exist",
    "resolved to nothing",
    "if we add",
    "we could",
    "to be created",
    "à créer",
];

fn tells_history(clause: &str) -> bool {
    let outside_code = clause.split('`').step_by(2).collect::<Vec<_>>().join(" ");
    let lower = outside_code.to_lowercase();
    if ABSENCE_PHRASES.iter().any(|p| lower.contains(p)) {
        return true;
    }
    lower
        .split(|c: char| !c.is_alphabetic())
        .any(|word| HISTORY_WORDS.contains(&word))
}

/// The sentences of a line, split on sentence punctuation outside inline
/// code. Kronn's docs write whole paragraphs on one line: a history word in
/// one sentence must not hide the present-tense claims of another.
fn clauses(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_code = false;
    let bytes = line.as_bytes();
    for (i, c) in line.char_indices() {
        match c {
            '`' => in_code = !in_code,
            '.' | ';' | '!' | '?' if !in_code => {
                let ends = bytes.get(i + 1).is_none_or(|b| b.is_ascii_whitespace());
                // "e.g. `x`" and "i.e. `x`" go on in the same sentence.
                let lower = line[start..i].to_ascii_lowercase();
                let abbreviation = c == '.' && (lower.ends_with("e.g") || lower.ends_with("i.e"));
                if ends && !abbreviation {
                    out.push(&line[start..=i]);
                    start = i + 1;
                }
            }
            _ => {}
        }
    }
    if start < line.len() {
        out.push(&line[start..]);
    }
    out
}

/// Headings that, as a whole, are a proposal list ("Ideas", "Roadmap").
const PROPOSAL_HEADINGS: &[&str] = &[
    "idea",
    "ideas",
    "proposal",
    "proposals",
    "proposition",
    "propositions",
    "suggestion",
    "suggestions",
    "todo",
    "todos",
    "roadmap",
    "future",
    "backlog",
    "pistes",
    "idees",
    "idées",
];

/// Phrases that make a heading a proposal wherever they sit in it.
const PROPOSAL_PHRASES: &[&[&str]] = &[
    &["future", "work"],
    &["future", "ideas"],
    &["future", "improvements"],
    &["future", "enhancements"],
    &["suggested"],
    &["proposed"],
    &["todo", "list"],
    &["to", "do", "list"],
    &["roadmap"],
    &["travaux", "futurs"],
    &["pistes", "d"],
];

/// A section titled as a proposal ("Suggested direction", "Future work",
/// "Ideas") names files that are still to be made: not claims about the
/// repository now. A single word inside a real title ("Ma Todo", "Workflow
/// suggestions") does not make one.
fn is_proposal_heading(heading: &str) -> bool {
    let lower = heading.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() == 1 && PROPOSAL_HEADINGS.contains(&words[0]) {
        return true;
    }
    PROPOSAL_PHRASES
        .iter()
        .any(|phrase| words.windows(phrase.len()).any(|w| w == *phrase))
}

/// A token that is a naming pattern, not a path: `TD-YYYYMMDD-slug.md`,
/// `NNNN-title.md`, `<name>.rs`.
fn is_placeholder_pattern(path: &str) -> bool {
    ["YYYY", "MMDD", "NNN", "XXX"]
        .iter()
        .any(|p| path.contains(p))
}

/// A line that presents an example ("Detail file example:", "e.g."): the
/// paths it and the list under it carry illustrate a format, they are not
/// claims about this repository.
fn tells_example(line: &str) -> bool {
    let outside_code = line.split('`').step_by(2).collect::<Vec<_>>().join(" ");
    let lower = outside_code.to_lowercase();
    lower.contains("e.g.")
        || lower.split(|c: char| !c.is_alphabetic()).any(|w| {
            matches!(
                w,
                "example" | "examples" | "exemple" | "exemples" | "sample"
            )
        })
}

fn is_list_item(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("- ")
        || t.starts_with("* ")
        || t.starts_with("+ ")
        || t.split_once(". ")
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// The changelog, release notes and history / archive docs name files that
/// are gone because that is their job: they are not claims about the code now.
pub(crate) fn is_historical_doc(doc: &str) -> bool {
    let lower = doc.to_lowercase();
    let mut parts: Vec<&str> = lower.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    let stem = name.strip_suffix(".md").unwrap_or(name);
    parts.iter().any(|dir| {
        matches!(
            *dir,
            "releases" | "changelog" | "history" | "archive" | "archives"
        )
    }) || stem.contains("changelog")
        || stem.contains("release-notes")
        || stem == "history"
        || stem.ends_with("-history")
        || stem.ends_with("-archive")
}

/// The repository paths a text names in inline code, outside fences and
/// outside the sentences that tell history. Only a path whose first folder exists at
/// the project root is taken: a bare example (`path/to/file.rs`) or another
/// repo's path is not a claim.
fn prose_paths(project: &Path, text: &str) -> Vec<String> {
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut in_fence = false;
    // An example line covers the list right under it, up to the next blank
    // line or heading.
    let mut example_block = false;
    for line in text.split('\n') {
        if is_fence(line) {
            in_fence = !in_fence;
            continue;
        }
        if line.trim().is_empty() || heading_of(line).is_some() {
            example_block = false;
        }
        if in_fence {
            continue;
        }
        let parts = clauses(line);
        // An example clause that closes a line covers the list right under
        // it. An example in the middle of a paragraph covers its own clause.
        if !is_list_item(line) && parts.last().is_some_and(|c| tells_example(c)) {
            example_block = true;
        }
        if example_block {
            continue;
        }
        for clause in parts {
            if tells_example(clause) || tells_history(clause) {
                continue;
            }
            for (i, piece) in clause.split('`').enumerate() {
                if i % 2 == 0 {
                    continue;
                }
                let Some(path) = path_token(piece) else {
                    continue;
                };
                let first = path.split('/').next().unwrap_or_default();
                if project.join(first).is_dir() {
                    found.insert(path);
                }
            }
        }
    }
    found.into_iter().collect()
}

// ─── Resolving a prose path ───────────────────────────────────────────────

/// Every project file, as the tails a doc may write it with: `a/b/c.rs`,
/// `b/c.rs`, `c.rs`.
struct ProjectIndex {
    tails: HashSet<String>,
}

const WALK_CAP: usize = 100_000;
const WALK_SKIPPED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "__pycache__",
    ".kronn",
];

/// `git ls-files` (tracked and untracked-not-ignored), `None` outside git.
fn git_files(project: &Path) -> Option<Vec<String>> {
    let out = crate::core::cmd::git_cmd()
        .arg("-C")
        .arg(project)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|f| !f.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// The fallback outside a git work tree: a bounded walk.
fn walked_files(project: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![project.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                let skipped = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| WALK_SKIPPED_DIRS.contains(&n));
                if !skipped {
                    stack.push(path);
                }
            } else if kind.is_file() {
                if let Ok(rel) = path.strip_prefix(project) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
            if out.len() >= WALK_CAP {
                return out;
            }
        }
    }
    out
}

impl ProjectIndex {
    fn build(project: &Path) -> Self {
        let files = git_files(project).unwrap_or_else(|| walked_files(project));
        let mut tails = HashSet::new();
        for file in &files {
            let mut rest = file.as_str();
            loop {
                tails.insert(rest.to_string());
                match rest.split_once('/') {
                    Some((_, tail)) if !tail.is_empty() => rest = tail,
                    _ => break,
                }
            }
        }
        Self { tails }
    }
}

/// Kronn's own outputs next to the docs: regenerated or written on demand
/// (`linked-repos.md` exists only once a repo is linked), never authored.
fn is_generated_output(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    [
        "briefing.md",
        "linked-repos.md",
        // Written when a project opts into document budgets.
        ".kronn-document-budgets.json",
        "checksums.json",
        citation_drift::CITATIONS_FILENAME,
        crate::core::doc_provenance::PROVENANCE_FILENAME,
        crate::core::kronn_state::KRONN_STATE_FILENAME,
        crate::core::audit_run_state::RUN_STATE_FILENAME,
    ]
    .contains(&name)
}

fn is_git_ignored(project: &Path, path: &str) -> bool {
    crate::core::cmd::git_cmd()
        .arg("-C")
        .arg(project)
        .args(["check-ignore", "-q", "--", path])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Decides whether a path a doc names is really gone. The file index is built
/// on the first miss only: a healthy doc set never pays for it.
struct PathResolver<'a> {
    project: &'a Path,
    index: OnceCell<ProjectIndex>,
}

impl<'a> PathResolver<'a> {
    fn new(project: &'a Path) -> Self {
        Self {
            project,
            index: OnceCell::new(),
        }
    }

    /// Gone: resolves from the project root nowhere, from the doc's folder
    /// nowhere, as the tail of no project file, and is neither generated nor
    /// git-ignored.
    fn is_missing(&self, doc: &str, path: &str) -> bool {
        if citation_drift::cited_file_exists(self.project, path) {
            return false;
        }
        if let Some(folder) = Path::new(doc)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            let beside = folder.join(path).to_string_lossy().replace('\\', "/");
            if citation_drift::cited_file_exists(self.project, &beside) {
                return false;
            }
        }
        let index = self.index.get_or_init(|| ProjectIndex::build(self.project));
        if index.tails.contains(path) {
            return false;
        }
        !(is_generated_output(path) || is_git_ignored(self.project, path))
    }
}

/// A doc that still holds raw `{{PLACEHOLDER}}` slots is the Kronn template,
/// not yet this project's: the example paths it carries are not claims.
fn is_unfilled_template(content: &str) -> bool {
    crate::api::audit::validation::count_raw_placeholders(content) > 0
}

/// The paths of one section's prose no citation of that section covers.
fn section_prose_paths(project: &Path, section: &Section) -> Vec<String> {
    if is_proposal_heading(&section.heading) {
        return Vec::new();
    }
    let labels = citation_drift::cite_labels(&section.text);
    let cited: BTreeSet<&str> = labels.iter().map(|l| file_of_label(l)).collect();
    prose_paths(project, &section.text)
        .into_iter()
        .filter(|p| !cited.contains(p.as_str()) && !is_placeholder_pattern(p))
        .collect()
}

/// Per section key, the prose paths of a doc that are gone. Empty for a
/// historical doc.
pub(crate) fn missing_by_section(
    project: &Path,
    doc: &str,
    content: &str,
) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    if is_historical_doc(doc) || is_unfilled_template(content) {
        return out;
    }
    let resolver = PathResolver::new(project);
    for section in split_sections(content) {
        let missing: Vec<String> = section_prose_paths(project, &section)
            .into_iter()
            .filter(|p| resolver.is_missing(doc, p))
            .collect();
        if !missing.is_empty() {
            out.insert(section.key, missing);
        }
    }
    out
}

/// Per section key, the gone prose paths of `doc` that no human
/// acknowledgement accepted: what is still to be shown, and what keeps the
/// doc "to re-verify".
pub(crate) fn unaccepted_missing(
    project: &Path,
    doc: &str,
    content: &str,
) -> BTreeMap<String, Vec<String>> {
    let mut missing = missing_by_section(project, doc, content);
    if missing.is_empty() {
        return missing;
    }
    let accepted = citation_drift::read_strict(project)
        .ok()
        .flatten()
        .and_then(|f| f.docs.get(doc).map(|e| e.known_missing.clone()))
        .unwrap_or_default();
    missing.retain(|key, paths| {
        if let Some(seen) = accepted.get(key) {
            paths.retain(|p| !seen.contains(p));
        }
        !paths.is_empty()
    });
    missing
}

/// The gone prose paths of one section that no acknowledgement accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VanishedPaths {
    /// The section's key within its doc (its heading).
    pub section: String,
    pub paths: Vec<String>,
}

/// What a doc's stale block must list, so « Vérifié, inchangé » accepts only
/// what the human was shown. Empty for anything that is not a readable doc.
pub fn vanished_paths(project: &Path, doc: &str) -> Vec<VanishedPaths> {
    let Ok(content) = std::fs::read_to_string(project.join(doc)) else {
        return Vec::new();
    };
    unaccepted_missing(project, doc, &content)
        .into_iter()
        .map(|(section, paths)| VanishedPaths { section, paths })
        .collect()
}

/// Whether the prose of `doc` names a path that is gone and that no human
/// acknowledgement accepted, section by section.
pub(crate) fn has_stale_prose_paths(project: &Path, doc: &str, content: &str) -> bool {
    !unaccepted_missing(project, doc, content).is_empty()
}

// ─── Verdicts ─────────────────────────────────────────────────────────────

/// `file`, `file:12` or `file:12-20` → the file part.
fn file_of_label(label: &str) -> &str {
    strip_line_suffix(label)
}

fn citation_state(
    project: &Path,
    label: &str,
    drift: Option<&SectionCitations>,
    moved: Option<&BTreeSet<String>>,
) -> CitationState {
    let Some(drift) = drift else {
        return existence_state(project, label);
    };
    if drift.doc_missing {
        return CitationState::FileMissing;
    }
    if let Some(found) = drift.citations.iter().find(|c| c.cite == label) {
        // Mapped by the git diff past the fingerprint window: it moved.
        if found.state.is_broken() && moved.is_some_and(|m| m.contains(label)) {
            return CitationState::Moved;
        }
        return found.state;
    }
    if drift.unrecorded.iter().any(|c| c == label) {
        return existence_state(project, label);
    }
    // Recorded and not listed: its fingerprint still matches.
    CitationState::Unchanged
}

fn existence_state(project: &Path, label: &str) -> CitationState {
    if citation_drift::cited_file_exists(project, file_of_label(label)) {
        CitationState::Unchanged
    } else {
        CitationState::FileMissing
    }
}

fn corrections_for(
    evidence: &[RelevanceEvidence],
    verdict: SectionVerdict,
) -> Vec<ProposedCorrection> {
    let mut out: Vec<ProposedCorrection> = evidence
        .iter()
        .map(|e| match e.state {
            CitationState::Changed => ProposedCorrection {
                kind: CorrectionKind::Recheck,
                reference: e.reference.clone(),
                proposal: format!(
                    "The cited lines of `{}` now hold other content: re-read them, then correct the claim or cite the line that supports it now.",
                    e.reference
                ),
            },
            CitationState::Removed => ProposedCorrection {
                kind: CorrectionKind::Recheck,
                reference: e.reference.clone(),
                proposal: format!(
                    "`{}` is now shorter than the cited line: find where the claim lives now and cite it, or remove the claim.",
                    e.reference
                ),
            },
            _ => ProposedCorrection {
                kind: CorrectionKind::ReplaceOrRemove,
                reference: e.reference.clone(),
                proposal: format!(
                    "`{}` no longer exists: cite the file that replaces it, or remove the claim.",
                    e.reference
                ),
            },
        })
        .collect();
    if verdict == SectionVerdict::NoLongerRelevant {
        out.push(ProposedCorrection {
            kind: CorrectionKind::RemoveSection,
            reference: String::new(),
            proposal: "Everything this section points at is gone: remove the section unless it still describes something that exists.".into(),
        });
    }
    out
}

struct DocContext<'a> {
    project: &'a Path,
    doc: &'a str,
    resolver: &'a PathResolver<'a>,
    /// What the prose names counts as claims: false for a doc still holding
    /// the template's placeholders, whose example paths are not this project's.
    prose_claims: bool,
    drift: Option<&'a SectionCitations>,
    recorded_digests: Option<&'a BTreeMap<String, String>>,
    /// The doc is byte-identical to its record.
    doc_untouched: bool,
    /// Per section key: the gone paths a human accepted as written.
    known_missing: Option<&'a BTreeMap<String, Vec<String>>>,
    /// Labels of this doc a diff mapping re-anchors although the fingerprint
    /// window did not find them.
    moved: Option<&'a BTreeSet<String>>,
}

fn verdict_row(
    ctx: &DocContext<'_>,
    section: &Section,
    verdict: SectionVerdict,
    evidence: Vec<RelevanceEvidence>,
    corrections: Vec<ProposedCorrection>,
    untouched: bool,
) -> SectionRelevance {
    SectionRelevance {
        doc: ctx.doc.to_string(),
        heading: section.heading.clone(),
        verdict,
        evidence,
        corrections,
        untouched_since_code_changed: untouched,
    }
}

/// The verdict of one section, `None` when it has no body to judge.
fn judge(
    ctx: &DocContext<'_>,
    section: &Section,
    counts: &mut RelevanceCounts,
) -> Option<SectionRelevance> {
    // A heading with nothing under it says nothing: not a section to judge.
    let body_empty = section
        .text
        .lines()
        .filter(|l| heading_of(l).is_none())
        .all(|l| l.trim().is_empty());
    if body_empty {
        return None;
    }
    let labels = citation_drift::cite_labels(&section.text);
    let accepted = ctx.known_missing.and_then(|m| m.get(&section.key));
    let mut anchors = 0usize;
    let mut evidence: Vec<RelevanceEvidence> = Vec::new();
    for label in &labels {
        anchors += 1;
        match citation_state(ctx.project, label, ctx.drift, ctx.moved) {
            // A moved line resolves: Kronn re-anchors it, nothing is lost.
            CitationState::Unchanged | CitationState::Moved => {}
            state => evidence.push(RelevanceEvidence {
                kind: EvidenceKind::Citation,
                reference: label.clone(),
                state,
            }),
        }
    }
    // A path both cited and named in prose is the citation: counted once.
    let prose = if ctx.prose_claims {
        section_prose_paths(ctx.project, section)
    } else {
        Vec::new()
    };
    for path in prose {
        if !ctx.resolver.is_missing(ctx.doc, &path) {
            anchors += 1;
        } else if accepted.is_some_and(|a| a.contains(&path)) {
            // A person accepted this mention as written.
        } else {
            anchors += 1;
            evidence.push(RelevanceEvidence {
                kind: EvidenceKind::PathReference,
                reference: path,
                state: CitationState::FileMissing,
            });
        }
    }
    counts.sections += 1;
    if anchors == 0 {
        counts.unanchored += 1;
        let correction = ProposedCorrection {
            kind: CorrectionKind::CheckOrAnchor,
            reference: String::new(),
            proposal: "Kronn cannot verify this section: it has no citation and names no repository path. Check it against the code, then anchor it with a `[src: file: path:line]` citation, or remove it if it no longer applies.".into(),
        };
        return Some(verdict_row(
            ctx,
            section,
            SectionVerdict::Unanchored,
            Vec::new(),
            vec![correction],
            false,
        ));
    }
    let gone = evidence
        .iter()
        .filter(|e| e.state == CitationState::FileMissing)
        .count();
    counts.missing_references += gone;
    counts.prose_missing += evidence
        .iter()
        .filter(|e| e.kind == EvidenceKind::PathReference)
        .count();
    let verdict = if evidence.is_empty() {
        counts.anchors_intact += 1;
        return Some(verdict_row(
            ctx,
            section,
            SectionVerdict::AnchorsIntact,
            Vec::new(),
            Vec::new(),
            false,
        ));
    } else if gone == anchors {
        counts.no_longer_relevant += 1;
        SectionVerdict::NoLongerRelevant
    } else {
        counts.outdated += 1;
        SectionVerdict::Outdated
    };
    let untouched = match ctx.recorded_digests.and_then(|d| d.get(&section.key)) {
        Some(recorded) => *recorded == digest(&section.text),
        None => ctx.doc_untouched,
    };
    if untouched {
        counts.untouched_stale += 1;
    }
    let corrections = corrections_for(&evidence, verdict);
    Some(verdict_row(
        ctx,
        section,
        verdict,
        evidence,
        corrections,
        untouched,
    ))
}

impl RelevanceCounts {
    fn finish(&mut self) {
        let anchored = self.anchors_intact + self.outdated + self.no_longer_relevant;
        self.anchored_intact_pct =
            (anchored > 0).then(|| (self.anchors_intact * 100 / anchored) as u32);
    }
}

/// Every doc of the project, judged section by section against the code as it
/// is now. `report` is the citation check already computed by the caller.
pub fn analyze(project: &Path, report: &CitationDriftReport) -> RelevanceReport {
    analyze_with_moves(project, report, &BTreeMap::new())
}

/// As `analyze`, where `moved` names, per doc, the citations a diff mapping
/// re-anchors: Kronn rewrites them, so they are not stale.
pub fn analyze_with_moves(
    project: &Path,
    report: &CitationDriftReport,
    moved: &BTreeMap<String, BTreeSet<String>>,
) -> RelevanceReport {
    let recorded = citation_drift::read_strict(project).ok().flatten();
    let resolver = PathResolver::new(project);
    let mut out = RelevanceReport::default();
    for doc in citation_drift::doc_files(project) {
        // History is not a claim about the code as it is now.
        if is_historical_doc(&doc) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(project.join(&doc)) else {
            continue;
        };
        let entry = recorded.as_ref().and_then(|f| f.docs.get(&doc));
        let ctx = DocContext {
            project,
            doc: &doc,
            resolver: &resolver,
            prose_claims: !is_unfilled_template(&content),
            drift: report.section(&doc),
            recorded_digests: entry.map(|e| &e.sections).filter(|d| !d.is_empty()),
            doc_untouched: entry.is_some_and(|e| !report_doc_changed(report, &doc, &content, e)),
            known_missing: entry.map(|e| &e.known_missing),
            moved: moved.get(&doc),
        };
        for section in split_sections(&content) {
            let Some(found) = judge(&ctx, &section, &mut out.counts) else {
                continue;
            };
            match found.verdict {
                SectionVerdict::AnchorsIntact => {}
                SectionVerdict::Unanchored => out.unanchored.push(found),
                _ => out.sections.push(found),
            }
        }
    }
    out.counts.finish();
    out
}

/// Whether the doc was edited since its fingerprints were recorded.
fn report_doc_changed(
    report: &CitationDriftReport,
    doc: &str,
    content: &str,
    entry: &citation_drift::DocFingerprints,
) -> bool {
    match report.section(doc) {
        Some(section) => section.doc_changed,
        None => digest(content) != entry.doc_sha256,
    }
}

#[cfg(test)]
#[path = "doc_relevance_test.rs"]
mod tests;
