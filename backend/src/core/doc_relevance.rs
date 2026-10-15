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
//!   which carry no fingerprint but can plainly stop existing.
//!
//! Verdicts, from the section's anchors only:
//!
//! * `still_true`: every anchor still resolves (a moved line counts: Kronn
//!   re-anchors it). Kronn found no contradiction; whether a claim is true in
//!   substance stays for the agent pass.
//! * `outdated`: a cited line changed or fell past the end of its file, or
//!   some (not all) of the files it points at are gone.
//! * `no_longer_relevant`: every anchor points at a file that is gone: the
//!   code the section describes no longer exists.
//! * `unanchored`: no citation and no path: Kronn cannot measure it.
//!
//! The signal is a count, so it can be tracked over time: sections per
//! verdict, the share of anchored sections that are still true, and the
//! sections *untouched since the code they describe changed* (the section's
//! own digest is the one recorded at validation while its anchors broke).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::citation_drift::{self, CitationDriftReport, CitationState, SectionCitations};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SectionVerdict {
    StillTrue,
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
    pub still_true: usize,
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
    /// `still_true` over the anchored sections; `None` when none is anchored.
    #[ts(type = "number | null")]
    pub relevance_pct: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RelevanceReport {
    pub counts: RelevanceCounts,
    /// Only the sections that need attention (`outdated`, `no_longer_relevant`).
    pub sections: Vec<SectionRelevance>,
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

/// The repository paths a text names in inline code, outside fences. Only a
/// path whose first folder exists at the project root is taken: a bare
/// example (`path/to/file.rs`) or another repo's path is not a claim.
fn prose_paths(project: &Path, text: &str) -> Vec<String> {
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut in_fence = false;
    for line in text.split('\n') {
        if is_fence(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for (i, piece) in line.split('`').enumerate() {
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
    found.into_iter().collect()
}

/// Prose paths of a doc whose file is gone. Recorded at validation or
/// acknowledgement: a human who signed the doc off accepted them as written
/// (a historical mention), so they stop counting.
pub(crate) fn missing_prose_paths(project: &Path, content: &str) -> Vec<String> {
    prose_paths(project, content)
        .into_iter()
        .filter(|p| !citation_drift::cited_file_exists(project, p))
        .collect()
}

/// Whether the prose of `doc` names a path that is gone and that no human
/// validation or acknowledgement accepted.
pub(crate) fn has_stale_prose_paths(project: &Path, doc: &str, content: &str) -> bool {
    let missing = missing_prose_paths(project, content);
    if missing.is_empty() {
        return false;
    }
    let accepted: BTreeSet<String> = citation_drift::read_strict(project)
        .ok()
        .flatten()
        .and_then(|f| {
            f.docs
                .get(doc)
                .map(|e| e.known_missing.iter().cloned().collect())
        })
        .unwrap_or_default();
    missing.iter().any(|p| !accepted.contains(p))
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
    drift: Option<&'a SectionCitations>,
    recorded_digests: Option<&'a BTreeMap<String, String>>,
    /// The doc is byte-identical to its record.
    doc_untouched: bool,
    known_missing: BTreeSet<&'a str>,
    /// Labels of this doc a diff mapping re-anchors although the fingerprint
    /// window did not find them.
    moved: Option<&'a BTreeSet<String>>,
}

fn judge(
    ctx: &DocContext<'_>,
    section: &Section,
    counts: &mut RelevanceCounts,
) -> Option<SectionRelevance> {
    let labels = citation_drift::cite_labels(&section.text);
    let cited_files: BTreeSet<&str> = labels.iter().map(|l| file_of_label(l)).collect();
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
    for path in prose_paths(ctx.project, &section.text) {
        if cited_files.contains(path.as_str()) || ctx.known_missing.contains(path.as_str()) {
            continue;
        }
        anchors += 1;
        if !citation_drift::cited_file_exists(ctx.project, &path) {
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
        return None;
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
        counts.still_true += 1;
        return None;
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
    Some(SectionRelevance {
        doc: ctx.doc.to_string(),
        heading: section.heading.clone(),
        corrections: corrections_for(&evidence, verdict),
        verdict,
        evidence,
        untouched_since_code_changed: untouched,
    })
}

impl RelevanceCounts {
    fn finish(&mut self) {
        let anchored = self.still_true + self.outdated + self.no_longer_relevant;
        self.relevance_pct = (anchored > 0).then(|| (self.still_true * 100 / anchored) as u32);
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
    let mut out = RelevanceReport::default();
    for doc in citation_drift::doc_files(project) {
        let Ok(content) = std::fs::read_to_string(project.join(&doc)) else {
            continue;
        };
        let entry = recorded.as_ref().and_then(|f| f.docs.get(&doc));
        let ctx = DocContext {
            project,
            doc: &doc,
            drift: report.section(&doc),
            recorded_digests: entry.map(|e| &e.sections).filter(|d| !d.is_empty()),
            doc_untouched: entry.is_some_and(|e| !report_doc_changed(report, &doc, &content, e)),
            known_missing: entry
                .map(|e| e.known_missing.iter().map(String::as_str).collect())
                .unwrap_or_default(),
            moved: moved.get(&doc),
        };
        for section in split_sections(&content) {
            if let Some(found) = judge(&ctx, &section, &mut out.counts) {
                out.sections.push(found);
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
