// Per-citation drift: a fingerprint of every `[src: path:line]` cited by the
// Kronn docs, recorded at validation (and after each audit), then compared
// with the current code. Deterministic, no model involved.
//
// Stored as `docs/citations.json`, next to `checksums.json` and `.kronn.json`:
// it must travel with `git clone` like the docs it describes, so a teammate
// sees the same "sources clean / to re-verify" state. Only paths, line numbers
// and SHA-256 digests are written. Each cited line is hashed after secret
// redaction, so a digest never depends on a secret value.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use ts_rs::TS;

pub const CITATIONS_FILENAME: &str = "citations.json";

const CITATIONS_README: &str = "Managed by Kronn. Fingerprints of the code lines cited by \
these docs, used to detect which citations changed since validation. Paths, line numbers \
and SHA-256 digests only. Commit it with the docs.";

/// A moved line is searched this many lines above and below its recorded place.
pub const REANCHOR_WINDOW: usize = 50;
/// A cited line needs this many alphanumeric characters to be re-anchored:
/// `}` or `]` reappear everywhere and would re-anchor onto any of them.
const REANCHOR_MIN_ALNUM: usize = 3;
/// Same cap as the citation verifier: larger files are tracked by existence only.
const FILE_SIZE_CAP_BYTES: u64 = 2 * 1024 * 1024;
const MAX_DOCS: usize = 1000;
const MAX_CITATIONS_PER_DOC: usize = 5000;
/// Docs dir sub-folders whose citations are not Kronn's to track.
const SKIPPED_DOC_DIRS: &[&str] = &["legacy", "var"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RecordReason {
    Validation,
    Audit,
    Acknowledged,
    /// Re-verified by "Revalider l'audit" (KT-1168).
    Revalidation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationFingerprint {
    /// The citation as the doc names it, `path:line` or `path:start-end`.
    pub cite: String,
    pub file: String,
    /// `None`: a bare-path citation, tracked by the file's existence only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocFingerprints {
    /// Digest of the doc itself when its citations were recorded: a later
    /// edit shows as `doc_changed`.
    pub doc_sha256: String,
    pub recorded_at: String,
    pub reason: RecordReason,
    pub citations: Vec<CitationFingerprint>,
    /// Digest of each Markdown section when recorded (KT-1168): tells a
    /// section nobody revised from one a person already touched.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sections: BTreeMap<String, String>,
    /// Per section key, the gone paths its prose names that a human saw and
    /// accepted with « Vérifié, inchangé »: a deliberate mention, not a stale
    /// claim. Only an acknowledgement writes it; a validation or a
    /// revalidation carries it over and never adds to it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub known_missing: BTreeMap<String, Vec<String>>,
}

/// A doc no citation or path anchors was re-read by an agent pass (KT-1292):
/// the doc as it stood, and the sources of the step that owns it. Either
/// changing sends the doc back to its step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocReview {
    pub doc_sha256: String,
    /// Digest of the owning step's sources when the doc was re-read.
    pub sources_fingerprint: String,
    pub reviewed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CitationsFile {
    #[serde(rename = "_readme", default)]
    pub readme: String,
    /// Doc path relative to the project root (`docs/AGENTS.md`) → its citations.
    #[serde(default)]
    pub docs: BTreeMap<String, DocFingerprints>,
    /// Doc path → when an agent pass last re-read it though nothing anchors
    /// it. Only the docs sent for that reason have a stamp.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reviews: BTreeMap<String, DocReview>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CitationState {
    Unchanged,
    /// Same content found within the window: re-anchored, not stale.
    Moved,
    /// The cited line now holds other content (modified, or removed in place).
    Changed,
    /// The cited line is past the end of the file.
    Removed,
    FileMissing,
}

impl CitationState {
    pub fn is_broken(self) -> bool {
        matches!(self, Self::Changed | Self::Removed | Self::FileMissing)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Moved => "moved",
            Self::Changed => "changed",
            Self::Removed => "removed",
            Self::FileMissing => "file missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CitationStatus {
    pub cite: String,
    pub file: String,
    #[ts(type = "number | null")]
    pub line: Option<usize>,
    #[ts(type = "number | null")]
    pub end_line: Option<usize>,
    pub state: CitationState,
    /// Where a moved citation now starts.
    #[ts(type = "number | null")]
    pub current_line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SectionCitations {
    /// Doc path relative to the project root, as `ai_file` in the drift report.
    pub doc: String,
    pub recorded_at: String,
    pub reason: RecordReason,
    pub stale: bool,
    pub doc_missing: bool,
    /// The doc was edited since its citations were recorded.
    pub doc_changed: bool,
    #[ts(type = "number")]
    pub unchanged_count: usize,
    /// Broken and moved citations only; unchanged ones are counted above.
    pub citations: Vec<CitationStatus>,
    /// Citations the doc carries now that were never fingerprinted.
    pub unrecorded: Vec<String>,
}

/// "Sources clean" badge, recomputed on every read. Distinct from the dated
/// "audit complete" milestone (`validated_at` in `.kronn.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SourcesState {
    /// No fingerprint recorded yet.
    #[default]
    Unknown,
    Clean,
    NeedsReverification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct CitationDriftReport {
    pub sources_state: SourcesState,
    pub sections: Vec<SectionCitations>,
    #[ts(type = "number")]
    pub broken_count: usize,
    #[ts(type = "number")]
    pub moved_count: usize,
    #[ts(type = "number")]
    pub unchanged_count: usize,
}

impl CitationDriftReport {
    pub fn section(&self, doc: &str) -> Option<&SectionCitations> {
        self.sections.iter().find(|s| s.doc == doc)
    }
}

// ─── Hashing ──────────────────────────────────────────────────────────────

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whitespace-insensitive, secret-blind form of one line.
fn normalize_line(line: &str) -> String {
    let (redacted, _) = crate::core::redact::redact_for_audit_artifact(line);
    redacted.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn block_hash(lines: &[String]) -> String {
    sha256_hex(lines.join("\n").as_bytes())
}

fn worth_reanchoring(lines: &[String]) -> bool {
    lines
        .iter()
        .flat_map(|l| l.chars())
        .filter(|c| c.is_alphanumeric())
        .count()
        >= REANCHOR_MIN_ALNUM
}

// ─── Paths ────────────────────────────────────────────────────────────────

fn is_plain_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}

/// The cited file under the project root, or `None` when the citation is not
/// a plain relative path or resolves outside the root (symlink).
fn resolve_cited(project_path: &Path, file: &str) -> Option<PathBuf> {
    let rel = Path::new(file);
    if !is_plain_relative(rel) {
        return None;
    }
    let candidate = project_path.join(rel);
    match (candidate.canonicalize(), project_path.canonicalize()) {
        (Ok(real), Ok(root)) if !real.starts_with(&root) => None,
        _ => Some(candidate),
    }
}

enum FileLines {
    Missing,
    /// Present but not line-addressable here (too large, binary, unreadable).
    Opaque,
    Lines(Vec<String>),
}

fn read_lines(path: &Path) -> FileLines {
    let Ok(meta) = std::fs::metadata(path) else {
        return FileLines::Missing;
    };
    if !meta.is_file() || meta.len() > FILE_SIZE_CAP_BYTES {
        return FileLines::Opaque;
    }
    match std::fs::read_to_string(path) {
        Ok(text) => FileLines::Lines(text.lines().map(normalize_line).collect()),
        Err(_) => FileLines::Opaque,
    }
}

// ─── Citations of a doc ───────────────────────────────────────────────────

struct DocCitation {
    cite: String,
    file: String,
    lines: Option<(usize, usize)>,
}

pub(crate) fn cite_label(file: &str, lines: Option<(usize, usize)>) -> String {
    match lines {
        None => file.to_string(),
        Some((s, e)) if s == e => format!("{file}:{s}"),
        Some((s, e)) => format!("{file}:{s}-{e}"),
    }
}

/// The distinct file citations of a doc, in order of first appearance.
fn doc_citations(content: &str) -> Vec<DocCitation> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for raw in crate::core::anti_halluc::extract_source_markers(content) {
        let Some((file, lines)) = crate::core::anti_halluc::file_citation(&raw) else {
            continue;
        };
        if !is_plain_relative(Path::new(&file)) {
            continue;
        }
        let cite = cite_label(&file, lines);
        if seen.insert(cite.clone()) {
            out.push(DocCitation { cite, file, lines });
            if out.len() >= MAX_CITATIONS_PER_DOC {
                break;
            }
        }
    }
    out
}

fn fingerprint(project_path: &Path, citation: &DocCitation) -> Option<CitationFingerprint> {
    let path = resolve_cited(project_path, &citation.file)?;
    let mut fp = CitationFingerprint {
        cite: citation.cite.clone(),
        file: citation.file.clone(),
        line: None,
        end_line: None,
        sha256: None,
    };
    let Some((start, end)) = citation.lines else {
        return Some(fp);
    };
    fp.line = Some(start);
    fp.end_line = Some(end);
    if let FileLines::Lines(lines) = read_lines(&path) {
        if end <= lines.len() {
            fp.sha256 = Some(block_hash(&lines[start - 1..end]));
        }
    }
    Some(fp)
}

/// The accepted gone paths a new record keeps. An acknowledgement accepts what
/// the human was just shown; every other reason carries over what was accepted
/// before, as long as that section still names the path.
fn accepted_missing(
    project_path: &Path,
    doc: &str,
    content: &str,
    reason: RecordReason,
    previous: Option<&DocFingerprints>,
) -> BTreeMap<String, Vec<String>> {
    let current = crate::core::doc_relevance::missing_by_section(project_path, doc, content);
    if reason == RecordReason::Acknowledged {
        return current;
    }
    let Some(previous) = previous else {
        return BTreeMap::new();
    };
    current
        .into_iter()
        .filter_map(|(key, paths)| {
            let kept: Vec<String> = previous
                .known_missing
                .get(&key)
                .map(|accepted| {
                    paths
                        .iter()
                        .filter(|p| accepted.contains(p))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            (!kept.is_empty()).then_some((key, kept))
        })
        .collect()
}

fn fingerprint_doc(
    project_path: &Path,
    doc: &str,
    content: &str,
    reason: RecordReason,
    previous: Option<&DocFingerprints>,
) -> DocFingerprints {
    DocFingerprints {
        doc_sha256: sha256_hex(content.as_bytes()),
        recorded_at: chrono::Utc::now().to_rfc3339(),
        reason,
        citations: doc_citations(content)
            .iter()
            .filter_map(|c| fingerprint(project_path, c))
            .collect(),
        sections: crate::core::doc_relevance::section_digests(content),
        known_missing: accepted_missing(project_path, doc, content, reason, previous),
    }
}

// ─── Store ────────────────────────────────────────────────────────────────

pub fn citations_path(project_path: &Path) -> PathBuf {
    crate::core::scanner::detect_docs_dir(project_path).join(CITATIONS_FILENAME)
}

/// `Ok(None)` when missing, `Err` when unreadable or malformed: a writer must
/// not rebuild from empty and lose the other docs' fingerprints.
pub fn read_strict(project_path: &Path) -> Result<Option<CitationsFile>, String> {
    let path = citations_path(project_path);
    match std::fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data)
            .map(Some)
            .map_err(|e| format!("{} is malformed: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

fn write(project_path: &Path, file: &mut CitationsFile) -> Result<(), String> {
    file.readme = CITATIONS_README.to_string();
    let json = serde_json::to_string_pretty(file).map_err(|e| format!("JSON error: {e}"))?;
    crate::core::mcp_scanner::atomic_write(&citations_path(project_path), &json)
}

/// Markdown docs under the docs dir, as paths relative to the project root.
pub(crate) fn doc_files(project_path: &Path) -> Vec<String> {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    let mut out = Vec::new();
    let mut stack = vec![docs_dir.clone()];
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
                let skipped = dir == docs_dir
                    && SKIPPED_DOC_DIRS
                        .iter()
                        .any(|s| entry.file_name().to_str() == Some(s));
                if !skipped {
                    stack.push(path);
                }
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "md") {
                if let Ok(rel) = path.strip_prefix(project_path) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    out.sort();
    out.truncate(MAX_DOCS);
    out
}

/// Whether a project-relative file (or folder) a doc names still exists.
pub(crate) fn cited_file_exists(project_path: &Path, file: &str) -> bool {
    resolve_cited(project_path, file).is_some_and(|path| path.exists())
}

fn read_doc(project_path: &Path, doc: &str) -> Option<String> {
    std::fs::read_to_string(project_path.join(doc)).ok()
}

/// Validation: every doc's citations are re-fingerprinted from the code as it
/// is now. The human has just checked the whole set.
pub fn record_all(project_path: &Path) -> Result<(), String> {
    // A previous record is read for what a human accepted, never rebuilt:
    // a validation does not accept a vanished path nobody was shown.
    let previous = read_strict(project_path).ok().flatten().unwrap_or_default();
    // A review stamp is no fingerprint of the code: a validation keeps it, and
    // the doc hash and the sources digest decide later whether it still holds.
    let mut file = CitationsFile {
        reviews: previous.reviews.clone(),
        ..Default::default()
    };
    for doc in doc_files(project_path) {
        if let Some(content) = read_doc(project_path, &doc) {
            let entry = fingerprint_doc(
                project_path,
                &doc,
                &content,
                RecordReason::Validation,
                previous.docs.get(&doc),
            );
            file.docs.insert(doc, entry);
        }
    }
    write(project_path, &mut file)
}

/// After an audit: only the docs a step of this run proved it rewrote get
/// fresh fingerprints. Every other record stays, so a doc edited by hand keeps
/// its broken citations "to re-verify" until a human or a rewrite checks them.
pub fn record_after_audit(project_path: &Path, rewritten: &[String]) -> Result<(), String> {
    let previous = read_strict(project_path)?;
    let docs: BTreeSet<String> = doc_files(project_path).into_iter().collect();
    let mut file = previous.clone().unwrap_or_default();
    for doc in rewritten.iter().filter(|d| docs.contains(*d)) {
        if let Some(content) = read_doc(project_path, doc) {
            let entry = fingerprint_doc(
                project_path,
                doc,
                &content,
                RecordReason::Audit,
                previous.as_ref().and_then(|p| p.docs.get(doc)),
            );
            file.docs.insert(doc.clone(), entry);
        }
    }
    let unchanged = match &previous {
        Some(prev) => *prev == file,
        None => file.docs.is_empty(),
    };
    if unchanged {
        return Ok(());
    }
    write(project_path, &mut file)
}

/// The gone prose paths a human was shown, per doc then per section key: what
/// an acknowledgement from the card may accept.
pub type ShownPaths = BTreeMap<String, BTreeMap<String, Vec<String>>>;

/// "Verified, unchanged": refresh one section's fingerprints from the current
/// code. Only `citations.json` is written, never the doc. Every gone prose
/// path of the doc is accepted: the caller is a panel that lists them all.
pub fn acknowledge(project_path: &Path, doc: &str) -> Result<SectionCitations, String> {
    acknowledge_accepting(project_path, doc, None)
}

/// As `acknowledge`, accepting only the gone prose paths in `shown` (what the
/// human was shown, not what is gone at the click). `None` accepts them all.
pub fn acknowledge_accepting(
    project_path: &Path,
    doc: &str,
    shown: Option<&BTreeMap<String, Vec<String>>>,
) -> Result<SectionCitations, String> {
    let rel = Path::new(doc);
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    let inside_docs = project_path
        .join(rel)
        .strip_prefix(&docs_dir)
        .is_ok_and(|p| !p.as_os_str().is_empty());
    if !is_plain_relative(rel) || !inside_docs || rel.extension().is_none_or(|e| e != "md") {
        return Err(format!("`{doc}` is not a Markdown doc of this project"));
    }
    let content = read_doc(project_path, doc).ok_or_else(|| format!("`{doc}` not found"))?;
    let mut file = read_strict(project_path)?.unwrap_or_default();
    let previous = file.docs.get(doc).cloned();
    let mut entry = fingerprint_doc(
        project_path,
        doc,
        &content,
        RecordReason::Acknowledged,
        previous.as_ref(),
    );
    if let Some(shown) = shown {
        // What was accepted before stays accepted; of the rest, only what the
        // human saw. The record never grows past the gone paths named now.
        let before = previous.as_ref().map(|p| &p.known_missing);
        entry.known_missing = std::mem::take(&mut entry.known_missing)
            .into_iter()
            .filter_map(|(key, paths)| {
                let kept: Vec<String> = paths
                    .into_iter()
                    .filter(|p| {
                        shown.get(&key).is_some_and(|s| s.contains(p))
                            || before
                                .and_then(|b| b.get(&key))
                                .is_some_and(|b| b.contains(p))
                    })
                    .collect();
                (!kept.is_empty()).then_some((key, kept))
            })
            .collect();
    }
    file.docs.insert(doc.to_string(), entry.clone());
    write(project_path, &mut file)?;
    Ok(evaluate_section(project_path, doc, &entry))
}

/// « Vérifié, inchangé » on several sections at once, from the card. Every one
/// that is a doc of the project is acknowledged, recorded or not: the checks
/// judge every doc, so one added since the validation must be clearable too.
/// A section that is not a doc (an audit step's own name) is left to its other
/// route. Only the gone paths in `shown` are accepted.
pub fn acknowledge_docs(
    project_path: &Path,
    sections: &[String],
    shown: &ShownPaths,
) -> Result<(), String> {
    let docs: BTreeSet<String> = doc_files(project_path).into_iter().collect();
    let none = BTreeMap::new();
    for doc in sections.iter().filter(|s| docs.contains(*s)) {
        acknowledge_accepting(project_path, doc, Some(shown.get(doc).unwrap_or(&none)))?;
    }
    Ok(())
}

/// The distinct citation labels a text carries, as the store names them.
pub(crate) fn cite_labels(content: &str) -> BTreeSet<String> {
    doc_citations(content).into_iter().map(|c| c.cite).collect()
}

/// A moved citation's new place: its label and line span there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reanchor {
    pub from: String,
    pub to: String,
    pub line: usize,
    pub end_line: usize,
}

impl Reanchor {
    pub fn of(status: &CitationStatus) -> Option<Self> {
        let (start, end, now) = (status.line?, status.end_line?, status.current_line?);
        if status.state != CitationState::Moved {
            return None;
        }
        let end_now = now + (end - start);
        Some(Self {
            from: status.cite.clone(),
            to: cite_label(&status.file, Some((now, end_now))),
            line: now,
            end_line: end_now,
        })
    }
}

/// `content` with each marker named in `moves` rewritten to its new lines,
/// on the lines `editable` allows, outside code. Returns the count rewritten.
pub fn rewrite_citations(
    content: &str,
    moves: &[Reanchor],
    editable: impl Fn(usize) -> bool,
) -> (String, usize) {
    let mut out = String::with_capacity(content.len());
    let mut rewritten = 0;
    let mut in_fence = false;
    for (index, line) in content.split_inclusive('\n').enumerate() {
        let fence = line.trim_start();
        if fence.starts_with("```") || fence.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence || !editable(index) {
            out.push_str(line);
            continue;
        }
        let (text, count) = rewrite_line(line, moves);
        rewritten += count;
        out.push_str(&text);
    }
    (out, rewritten)
}

fn rewrite_line(line: &str, moves: &[Reanchor]) -> (String, usize) {
    let mut out = String::with_capacity(line.len());
    let mut rewritten = 0;
    let mut rest = line;
    let mut ticks = 0usize;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(at) = lower.find("[src:") else {
            out.push_str(rest);
            return (out, rewritten);
        };
        ticks += rest[..at].matches('`').count();
        out.push_str(&rest[..at]);
        let body = &rest[at + 5..];
        let mut depth = 1usize;
        let mut close = None;
        for (i, c) in body.char_indices() {
            match c {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else {
            out.push_str(&rest[at..]);
            return (out, rewritten);
        };
        let inner = &body[..close];
        let replaced = ticks
            .is_multiple_of(2)
            .then(|| reanchored_inner(inner, moves))
            .flatten();
        out.push_str(&rest[at..at + 5]);
        match replaced {
            Some(inner) => {
                out.push_str(&inner);
                rewritten += 1;
            }
            None => out.push_str(inner),
        }
        out.push(']');
        rest = &body[close + 1..];
    }
}

fn reanchored_inner(inner: &str, moves: &[Reanchor]) -> Option<String> {
    let (file, lines) = crate::core::anti_halluc::file_citation(inner.trim())?;
    let label = cite_label(&file, Some(lines?));
    let to = moves.iter().find(|m| m.from == label)?;
    let trimmed = inner.trim_end();
    let colon = trimmed.rfind(':')?;
    // A `#symbol` anchor follows the moved lines.
    let anchor = trimmed[colon..]
        .find('#')
        .map_or("", |at| &trimmed[colon + at..]);
    let spec = if to.line == to.end_line {
        to.line.to_string()
    } else {
        format!("{}-{}", to.line, to.end_line)
    };
    Some(format!(
        "{}{spec}{anchor}{}",
        &trimmed[..=colon],
        &inner[trimmed.len()..]
    ))
}

/// After `rewrite_citations`: the moved fingerprints follow their citation,
/// digest unchanged. A label the doc still carries (a human section) keeps
/// its old entry too. Nothing else of the doc is acknowledged.
pub fn record_reanchored(
    project_path: &Path,
    doc: &str,
    before: &str,
    after: &str,
    moves: &[Reanchor],
) -> Result<(), String> {
    let mut file = read_strict(project_path)?
        .ok_or_else(|| format!("No citation fingerprints recorded for `{doc}`"))?;
    let entry = file
        .docs
        .get_mut(doc)
        .ok_or_else(|| format!("No citation fingerprints recorded for `{doc}`"))?;
    let still_cited = cite_labels(after);
    // A label a move lands on now names the moved line, not its old one.
    let landed: BTreeSet<&str> = moves.iter().map(|m| m.to.as_str()).collect();
    let mut citations: Vec<CitationFingerprint> = Vec::new();
    fn push(fp: CitationFingerprint, out: &mut Vec<CitationFingerprint>) {
        if !out.iter().any(|c| c.cite == fp.cite) {
            out.push(fp);
        }
    }
    for fp in std::mem::take(&mut entry.citations) {
        match moves.iter().find(|m| m.from == fp.cite) {
            Some(m) => {
                let moved = CitationFingerprint {
                    cite: m.to.clone(),
                    line: Some(m.line),
                    end_line: Some(m.end_line),
                    ..fp.clone()
                };
                if still_cited.contains(&fp.cite) && !landed.contains(fp.cite.as_str()) {
                    push(fp, &mut citations);
                }
                citations.retain(|c| c.cite != moved.cite);
                citations.push(moved);
            }
            None if landed.contains(fp.cite.as_str()) => {}
            None => push(fp, &mut citations),
        }
    }
    entry.citations = citations;
    // An edit made by hand since the record stays visible as such.
    if entry.doc_sha256 == sha256_hex(before.as_bytes()) {
        entry.doc_sha256 = sha256_hex(after.as_bytes());
        if !entry.sections.is_empty() {
            entry.sections = crate::core::doc_relevance::section_digests(after);
        }
    }
    write(project_path, &mut file)
}

/// Fresh fingerprints for the docs a revalidation re-verified; every other
/// doc keeps its record.
pub fn record_docs(
    project_path: &Path,
    docs: &[String],
    reason: RecordReason,
) -> Result<(), String> {
    if docs.is_empty() {
        return Ok(());
    }
    let mut file = read_strict(project_path)?.unwrap_or_default();
    for doc in docs {
        if let Some(content) = read_doc(project_path, doc) {
            let entry = fingerprint_doc(project_path, doc, &content, reason, file.docs.get(doc));
            file.docs.insert(doc.clone(), entry);
        }
    }
    write(project_path, &mut file)
}

/// Stamp each doc an agent pass re-read although nothing anchors it, whether
/// or not the pass added anchors: `(doc, owning step's sources digest)`. The
/// doc is hashed as it stands now, after the pass.
pub fn record_reviews(project_path: &Path, reviewed: &[(String, String)]) -> Result<(), String> {
    if reviewed.is_empty() {
        return Ok(());
    }
    let mut file = read_strict(project_path)?.unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339();
    for (doc, sources_fingerprint) in reviewed {
        if let Some(content) = read_doc(project_path, doc) {
            file.reviews.insert(
                doc.clone(),
                DocReview {
                    doc_sha256: sha256_hex(content.as_bytes()),
                    sources_fingerprint: sources_fingerprint.clone(),
                    reviewed_at: now.clone(),
                },
            );
        }
    }
    write(project_path, &mut file)
}

/// The stamp of `doc` when it still holds for this content and these sources;
/// `None` when never re-read, or edited / sources changed since. The sources
/// digest is only computed once the doc itself matches.
pub fn current_review<'a>(
    file: Option<&'a CitationsFile>,
    doc: &str,
    content: &str,
    sources_fingerprint: impl FnOnce() -> String,
) -> Option<&'a DocReview> {
    let stamp = file?.reviews.get(doc)?;
    (stamp.doc_sha256 == sha256_hex(content.as_bytes())
        && stamp.sources_fingerprint == sources_fingerprint())
    .then_some(stamp)
}

// ─── Evaluation ───────────────────────────────────────────────────────────

fn find_moved(lines: &[String], start: usize, len: usize, hash: &str) -> Option<usize> {
    let fits = |s: usize| s >= 1 && s + len - 1 <= lines.len();
    let matches = |s: usize| fits(s) && block_hash(&lines[s - 1..s - 1 + len]) == hash;
    (1..=REANCHOR_WINDOW).find_map(|d| {
        [start.checked_sub(d), Some(start + d)]
            .into_iter()
            .flatten()
            .find(|&s| matches(s))
    })
}

fn evaluate_citation(project_path: &Path, fp: &CitationFingerprint) -> CitationStatus {
    let mut status = CitationStatus {
        cite: fp.cite.clone(),
        file: fp.file.clone(),
        line: fp.line,
        end_line: fp.end_line,
        state: CitationState::Unchanged,
        current_line: None,
    };
    let Some(path) = resolve_cited(project_path, &fp.file) else {
        status.state = CitationState::FileMissing;
        return status;
    };
    let lines = match read_lines(&path) {
        FileLines::Missing => {
            status.state = CitationState::FileMissing;
            return status;
        }
        FileLines::Opaque => return status,
        FileLines::Lines(lines) => lines,
    };
    let (Some(start), Some(end)) = (fp.line, fp.end_line) else {
        return status;
    };
    let Some(hash) = fp.sha256.as_deref() else {
        // Recorded past the end of the file: still past it, or now resolvable.
        if end > lines.len() {
            status.state = CitationState::Removed;
        }
        return status;
    };
    if end <= lines.len() && block_hash(&lines[start - 1..end]) == hash {
        return status;
    }
    let len = end - start + 1;
    if let Some(found) = find_moved(&lines, start, len, hash) {
        if worth_reanchoring(&lines[found - 1..found - 1 + len]) {
            status.state = CitationState::Moved;
            status.current_line = Some(found);
            return status;
        }
    }
    status.state = if end > lines.len() {
        CitationState::Removed
    } else {
        CitationState::Changed
    };
    status
}

fn evaluate_section(project_path: &Path, doc: &str, entry: &DocFingerprints) -> SectionCitations {
    let current = read_doc(project_path, doc);
    let mut section = SectionCitations {
        doc: doc.to_string(),
        recorded_at: entry.recorded_at.clone(),
        reason: entry.reason,
        stale: false,
        doc_missing: current.is_none(),
        doc_changed: false,
        unchanged_count: 0,
        citations: Vec::new(),
        unrecorded: Vec::new(),
    };
    let Some(content) = current else {
        section.stale = true;
        return section;
    };
    section.doc_changed = sha256_hex(content.as_bytes()) != entry.doc_sha256;
    let present: Vec<DocCitation> = doc_citations(&content);
    let present_cites: BTreeSet<&str> = present.iter().map(|c| c.cite.as_str()).collect();
    let recorded: BTreeSet<&str> = entry.citations.iter().map(|c| c.cite.as_str()).collect();
    for fp in &entry.citations {
        // A citation the doc no longer carries was fixed or removed by hand.
        if section.doc_changed && !present_cites.contains(fp.cite.as_str()) {
            continue;
        }
        let status = evaluate_citation(project_path, fp);
        if status.state == CitationState::Unchanged {
            section.unchanged_count += 1;
        } else {
            section.stale |= status.state.is_broken();
            section.citations.push(status);
        }
    }
    section.unrecorded = present
        .iter()
        .filter(|c| !recorded.contains(c.cite.as_str()))
        .map(|c| c.cite.clone())
        .collect();
    section
}

/// Every recorded section, evaluated against the current code.
pub fn check(project_path: &Path) -> CitationDriftReport {
    let file = match read_strict(project_path) {
        Ok(Some(file)) => file,
        Ok(None) => return CitationDriftReport::default(),
        Err(e) => {
            tracing::warn!("Citation drift unavailable: {e}");
            return CitationDriftReport::default();
        }
    };
    let mut report = CitationDriftReport::default();
    for (doc, entry) in &file.docs {
        let section = evaluate_section(project_path, doc, entry);
        report.unchanged_count += section.unchanged_count;
        for c in &section.citations {
            if c.state.is_broken() {
                report.broken_count += 1;
            } else {
                report.moved_count += 1;
            }
        }
        report.sections.push(section);
    }
    report.sources_state = if report.sections.iter().any(|s| s.stale) {
        SourcesState::NeedsReverification
    } else {
        SourcesState::Clean
    };
    report
}

/// The drift entries naming what broke in a stale section.
pub fn changed_sources(section: &SectionCitations) -> Vec<String> {
    if section.doc_missing {
        return vec![format!("{} (deleted)", section.doc)];
    }
    section
        .citations
        .iter()
        .filter(|c| c.state.is_broken())
        .map(|c| format!("{} ({})", c.cite, c.state.label()))
        .collect()
}

// ─── Scheduled re-verification ────────────────────────────────────────────

/// Last scheduled result per project path; the report itself is recomputed
/// on every read, the cron only records when it last looked and what it saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScheduledCheck {
    pub checked_at: String,
    pub sources_state: SourcesState,
    #[ts(type = "number")]
    pub broken_count: usize,
}

static SCHEDULED: std::sync::LazyLock<std::sync::Mutex<BTreeMap<PathBuf, ScheduledCheck>>> =
    std::sync::LazyLock::new(Default::default);
static LAST_TICK: std::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>> =
    std::sync::Mutex::new(None);

pub fn last_scheduled_check(project_path: &Path) -> Option<ScheduledCheck> {
    SCHEDULED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(project_path)
        .cloned()
}

/// Whether the re-verification cron has an occurrence in `(since, now]`.
/// No expression configured means off.
pub fn reverify_due(
    cron: Option<&str>,
    default_tz: chrono_tz::Tz,
    since: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let Some(schedule) = cron.map(str::trim).filter(|c| !c.is_empty()) else {
        return false;
    };
    let trigger = crate::models::WorkflowTrigger::Cron {
        schedule: schedule.to_string(),
        timezone: None,
    };
    crate::workflows::trigger::should_fire(&trigger, default_tz, since, now)
}

/// Re-verify every project that has recorded fingerprints.
pub fn reverify_projects(project_paths: &[PathBuf]) -> Vec<(PathBuf, ScheduledCheck)> {
    let mut out = Vec::new();
    for path in project_paths {
        if !citations_path(path).is_file() {
            continue;
        }
        let report = check(path);
        let result = ScheduledCheck {
            checked_at: chrono::Utc::now().to_rfc3339(),
            sources_state: report.sources_state,
            broken_count: report.broken_count,
        };
        if report.sources_state == SourcesState::NeedsReverification {
            tracing::warn!(
                project = %path.display(),
                broken = report.broken_count,
                "Cited lines changed since validation: the docs need re-verification"
            );
        }
        SCHEDULED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(path.clone(), result.clone());
        out.push((path.clone(), result));
    }
    out
}

/// One scheduler tick, called by the workflow engine loop.
pub async fn scheduled_tick(state: &crate::AppState) {
    let now = chrono::Utc::now();
    let since = {
        let mut last = LAST_TICK.lock().unwrap_or_else(|p| p.into_inner());
        last.replace(now).unwrap_or(now)
    };
    let cron = state.config.read().await.audit_reverify_cron.clone();
    if !reverify_due(
        cron.as_deref(),
        crate::core::timezone::current(),
        since,
        now,
    ) {
        return;
    }
    let projects = match state
        .db
        .with_read_conn(crate::db::projects::list_projects)
        .await
    {
        Ok(projects) => projects,
        Err(e) => {
            tracing::warn!("Citation re-verification skipped: {e}");
            return;
        }
    };
    let paths: Vec<PathBuf> = projects
        .iter()
        .map(|p| crate::core::scanner::resolve_host_path(&p.path))
        .collect();
    let _ = tokio::task::spawn_blocking(move || reverify_projects(&paths)).await;
}

#[cfg(test)]
#[path = "citation_drift_test.rs"]
mod tests;
