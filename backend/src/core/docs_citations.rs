//! CI check of the `[src: file:]` citations in Kronn's own docs (KT-1059).
//!
//! Stricter than the runtime verifier it reuses: a doc citation must name a
//! regular file by its path from the repo root (no basename or suffix walk, no
//! absolute path) and carry a line or range that exists. Only a binary file
//! may be cited without a line. Fenced code and placeholder examples are skipped.
//! A `#symbol` anchor (KT-1074) must appear as a word inside the cited lines,
//! and open them when they contain its definition.
//!
//! KT-1195: CI scopes the gate per pull request ([`Scope`]) and
//! `make docs-reanchor` repairs moved anchors ([`reanchor_corpus`]).

mod diff_map;

use crate::core::anti_halluc::{
    anchor_miss, anchor_miss_detail, file_citation_anchored, is_binary_file, line_bounds_status,
    markers_at, partition_inline_code, resolve_in_root, scan_fences, strip_inline_code, Jailed,
    SourceStatus,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Doc trees checked, relative to the repo root.
pub const DOC_ROOTS: &[&str] = &["docs", "backend/docs"];
/// Historical release notes describe the code of their time and are not repaired.
pub const EXCLUDED: &[&str] = &["docs/releases"];

/// Path of a file listing the paths a pull request changes, one per line.
/// Set by CI on pull requests only; unset means strict.
pub const CHANGED_ENV: &str = "KRONN_DOCS_CITATIONS_CHANGED";
/// Directory where CI collects `annotations.txt` and `summary.md`.
pub const REPORT_DIR_ENV: &str = "KRONN_DOCS_CITATIONS_REPORT_DIR";
/// `all`, or the docs to re-anchor, space-separated: `make docs-reanchor`.
pub const REANCHOR_ENV: &str = "KRONN_DOCS_REANCHOR";
/// Git revision whose old line numbers the re-anchor run maps to `HEAD`.
pub const REANCHOR_BASE_ENV: &str = "KRONN_DOCS_REANCHOR_BASE";
/// A pull request that changes it is a release: every doc is checked.
const RELEASE_MARKER: &str = "VERSION";

/// One citation of a doc that does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocCitationFailure {
    /// 1-based line of the doc holding the marker's `[`.
    pub line: usize,
    /// The marker's inner text, after `src:`.
    pub citation: String,
    pub reason: String,
    /// Set when only the range is stale: the anchor still occurs in the file.
    pub moved: Option<MovedAnchor>,
}

/// A `#symbol` that left its cited range, or whose definition inside it no
/// longer opens it, but still occurs in the cited file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedAnchor {
    pub span: (usize, usize),
    /// Where a range for the symbol would start: the top of the doc comment
    /// and attributes of each definition, else every line holding it. None
    /// opens `span`.
    pub occurrences: Vec<usize>,
    /// Line count of the cited file.
    pub total: usize,
}

/// A syntax example such as `<path>:<line>` or `src/{a,b}.rs`, not a citation.
fn is_placeholder(citation: &str) -> bool {
    citation.contains(['<', '>', '{', '}', '*', '…']) || citation.contains("...")
}

/// `None` when the citation holds, else why it does not.
fn check_reference(
    path: &str,
    lines: Option<(usize, usize)>,
    anchor: Option<&str>,
    root: &Path,
) -> Option<(String, Option<MovedAnchor>)> {
    let rel = Path::new(path);
    if rel.has_root() {
        return Some((
            "absolute path: cite it relative to the repo root".into(),
            None,
        ));
    }
    let found = match resolve_in_root(root, rel) {
        Jailed::Inside(found) => found,
        Jailed::Missing => return Some((format!("file not found: {path}"), None)),
        Jailed::Escapes => return Some(("path leaves the repo root".into(), None)),
    };
    if !found.is_file() {
        return Some((format!("not a file: {path}"), None));
    }
    match lines {
        None if is_binary_file(&found) => None,
        None => Some(("no line: cite `path:line` or `path:start-end`".into(), None)),
        Some(span) => match line_bounds_status(&found, Some(span)) {
            (SourceStatus::Verified, _) => {
                let symbol = anchor?;
                let miss = anchor_miss(&found, span, symbol)?;
                let detail = anchor_miss_detail(span, symbol, &miss);
                let (elsewhere, starts) = (miss.lines, miss.starts);
                let total = std::fs::read_to_string(&found).map_or(0, |t| t.lines().count());
                let reason = match elsewhere.as_slice() {
                    [] => format!("{detail} — rewrite the claim or cite where it lives"),
                    [_] => match suggested_span(span, &starts, total) {
                        Some((start, end)) => {
                            format!(
                                "{detail} — try `{path}:{}#{symbol}`",
                                span_label(start, end)
                            )
                        }
                        None => detail,
                    },
                    // The nearest occurrence is often the wrong one: no suggestion.
                    lines => {
                        let lines: Vec<String> = lines.iter().map(usize::to_string).collect();
                        format!(
                            "{detail} — ambiguous: pick by hand among lines {}",
                            lines.join(", ")
                        )
                    }
                };
                let moved = (!elsewhere.is_empty()).then_some(MovedAnchor {
                    span,
                    occurrences: starts,
                    total,
                });
                Some((reason, moved))
            }
            (_, detail) => Some((detail, None)),
        },
    }
}

/// The occurrence nearest the old start, keeping the cited span's length
/// within the file.
fn suggested_span(
    span: (usize, usize),
    occurrences: &[usize],
    total: usize,
) -> Option<(usize, usize)> {
    let (start, end) = span;
    let line = *occurrences.iter().min_by_key(|l| l.abs_diff(start))?;
    Some((line, (line + (end - start)).min(total.max(line))))
}

fn span_label(start: usize, end: usize) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

/// Every failing `file:` citation of one doc, prose and backticked alike.
pub fn check_doc(text: &str, root: &Path) -> Vec<DocCitationFailure> {
    let fences = scan_fences(text, &['`'], false);
    let visible: Vec<&str> = text
        .lines()
        .zip(&fences.in_fence)
        .map(|(line, fenced)| if *fenced { "" } else { line })
        .collect();

    let mut markers: Vec<(usize, String)> = Vec::new();
    // Inline-code stripping keeps one output line per input line.
    let prose = strip_inline_code(&visible.join("\n"));
    let mut line_of = Vec::with_capacity(prose.len());
    let mut line = 1usize;
    for ch in prose.chars() {
        line_of.push(line);
        if ch == '\n' {
            line += 1;
        }
    }
    for (offset, inner) in markers_at(&prose) {
        markers.push((line_of[offset], inner));
    }
    for (index, text_line) in visible.iter().enumerate() {
        if text_line.contains('`') {
            let quoted = partition_inline_code(text_line).1;
            markers.extend(markers_at(&quoted).into_iter().map(|(_, m)| (index + 1, m)));
        }
    }
    markers.sort_by_key(|(line, _)| *line);

    markers
        .into_iter()
        .filter_map(|(line, citation)| {
            // On the raw text: the path cleanup peels `<`/`>` off `<REAL_FILE>`.
            if is_placeholder(&citation) {
                return None;
            }
            let (path, lines, anchor) = file_citation_anchored(&citation)?;
            let (reason, moved) = check_reference(&path, lines, anchor.as_deref(), root)?;
            Some(DocCitationFailure {
                line,
                citation,
                reason,
                moved,
            })
        })
        .collect()
}

fn collect_docs(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if EXCLUDED.iter().any(|ex| rel == Path::new(ex)) {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect_docs(&path, root, out),
            Ok(t) if t.is_file() && path.extension().is_some_and(|e| e == "md") => {
                out.push(rel.to_path_buf())
            }
            _ => {}
        }
    }
}

/// The checked docs, relative to `root`, sorted.
fn corpus(root: &Path) -> Vec<PathBuf> {
    let mut docs = Vec::new();
    for dir in DOC_ROOTS {
        collect_docs(&root.join(dir), root, &mut docs);
    }
    docs.sort();
    docs
}

/// Every failing citation of the checked doc trees, as `(doc, failure)`.
pub fn check_corpus(root: &Path) -> Vec<(PathBuf, DocCitationFailure)> {
    let mut failures = Vec::new();
    for doc in corpus(root) {
        let Ok(text) = std::fs::read_to_string(root.join(&doc)) else {
            continue;
        };
        for failure in check_doc(&text, root) {
            failures.push((doc.clone(), failure));
        }
    }
    failures
}

/// One line per failure: `doc:line  [src: …]  reason`.
pub fn report(failures: &[(PathBuf, DocCitationFailure)]) -> String {
    failures
        .iter()
        .map(|(doc, f)| {
            format!(
                "{}:{}  [src: {}]  {}\n",
                doc.display(),
                f.line,
                f.citation,
                f.reason
            )
        })
        .collect()
}

/// A repo-relative path the gate reads: a `.md` under a doc root, outside
/// the historical release notes.
pub fn is_checked_doc(path: &str) -> bool {
    let under = |dir: &str| path.strip_prefix(dir).is_some_and(|r| r.starts_with('/'));
    path.ends_with(".md")
        && DOC_ROOTS.iter().any(|dir| under(dir))
        && !EXCLUDED.iter().any(|dir| under(dir))
}

/// Which failures fail the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every failure blocks: local runs, releases, manual runs.
    Strict,
    /// Nothing blocks: a pull request that changes no doc.
    Warn,
    /// Only failures in these docs block, the docs a pull request changes.
    Files(BTreeSet<PathBuf>),
}

impl Scope {
    /// The scope of a pull request from the paths it changes, one per line.
    pub fn for_changes(changed: &str) -> Scope {
        let paths: Vec<&str> = changed
            .lines()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if paths.contains(&RELEASE_MARKER) {
            return Scope::Strict;
        }
        let docs: BTreeSet<PathBuf> = paths
            .into_iter()
            .filter(|p| is_checked_doc(p))
            .map(PathBuf::from)
            .collect();
        if docs.is_empty() {
            Scope::Warn
        } else {
            Scope::Files(docs)
        }
    }

    pub fn blocks(&self, doc: &Path) -> bool {
        match self {
            Scope::Strict => true,
            Scope::Warn => false,
            Scope::Files(docs) => docs.contains(doc),
        }
    }

    fn label(&self) -> String {
        match self {
            Scope::Strict => "strict, every doc".into(),
            Scope::Warn => "warning only, this pull request changes no doc".into(),
            Scope::Files(docs) => format!(
                "strict on the {} doc(s) this pull request changes",
                docs.len()
            ),
        }
    }
}

/// Failures split by the scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub scope: Scope,
    pub blocking: Vec<(PathBuf, DocCitationFailure)>,
    pub warnings: Vec<(PathBuf, DocCitationFailure)>,
}

/// `%`, CR and LF are the escapes of a workflow command's message.
fn escape_data(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Properties also escape their `:` and `,` separators.
fn escape_property(s: &str) -> String {
    escape_data(s).replace(':', "%3A").replace(',', "%2C")
}

impl Verdict {
    pub fn new(failures: Vec<(PathBuf, DocCitationFailure)>, scope: Scope) -> Self {
        let (blocking, warnings) = failures.into_iter().partition(|(doc, _)| scope.blocks(doc));
        Verdict {
            scope,
            blocking,
            warnings,
        }
    }

    /// The command that repairs what blocks, limited to the blocking docs
    /// when the scope is.
    pub fn repair_command(&self) -> String {
        match self.scope {
            Scope::Files(_) => {
                let docs: BTreeSet<String> = self
                    .blocking
                    .iter()
                    .map(|(doc, _)| doc.display().to_string())
                    .collect();
                let docs: Vec<String> = docs.into_iter().collect();
                format!("make docs-reanchor DOCS=\"{}\"", docs.join(" "))
            }
            _ => "make docs-reanchor".into(),
        }
    }

    /// One GitHub workflow command per failure: errors block, warnings do not.
    pub fn annotations(&self) -> String {
        let blocking = self.blocking.iter().map(|f| ("error", f));
        let warnings = self.warnings.iter().map(|f| ("warning", f));
        blocking
            .chain(warnings)
            .map(|(level, (doc, f))| {
                format!(
                    "::{level} file={},line={},title={}::{}\n",
                    escape_property(&doc.display().to_string()),
                    f.line,
                    escape_property("Doc citation"),
                    escape_data(&format!("[src: {}] {}", f.citation, f.reason))
                )
            })
            .collect()
    }

    /// The job summary: counts, scope, repair command and every failure.
    pub fn summary(&self) -> String {
        let total = self.blocking.len() + self.warnings.len();
        let mut out = String::from("### Doc citations\n\n");
        if total == 0 {
            out.push_str("Every doc citation resolves.\n");
            return out;
        }
        out.push_str(&format!(
            "**{total}** unresolved: {} blocking, {} warning(s). Scope: {}.\n\n\
             Repair: `{}` re-anchors a range by a unique `#symbol` or a verified \
             Git diff mapping; fix the rest by hand.\n\n",
            self.blocking.len(),
            self.warnings.len(),
            self.scope.label(),
            self.repair_command()
        ));
        let rows = self.blocking.iter().map(|f| ("blocking", f));
        let rows = rows.chain(self.warnings.iter().map(|f| ("warning", f)));
        for (level, (doc, f)) in rows {
            out.push_str(&format!(
                "- {level} `{}:{}` `[src: {}]` {}\n",
                doc.display(),
                f.line,
                f.citation,
                f.reason
            ));
        }
        out
    }

    /// The panic message of a blocking run.
    pub fn failure_message(&self) -> String {
        let mut out = format!(
            "{} doc citation(s) do not resolve ({}):\n{}\nRepair: `{}` re-anchors a range \
             by a unique #symbol or a verified Git diff mapping; fix the rest by hand (path, \
             line, or the claim).\n",
            self.blocking.len(),
            self.scope.label(),
            report(&self.blocking),
            self.repair_command()
        );
        if !self.warnings.is_empty() {
            out.push_str(&format!(
                "{} more outside the scope (warnings):\n{}",
                self.warnings.len(),
                report(&self.warnings)
            ));
        }
        out
    }

    fn write_report(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        // Overwritten, not appended: a rerun of the test repeats nothing.
        std::fs::write(dir.join("annotations.txt"), self.annotations())?;
        std::fs::write(dir.join("summary.md"), self.summary())
    }
}

/// The docs a re-anchor run may rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReanchorTarget {
    All,
    Docs(BTreeSet<PathBuf>),
}

impl ReanchorTarget {
    /// `None` for an unset or blank value.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        match value {
            "" => None,
            "all" | "1" => Some(ReanchorTarget::All),
            docs => Some(ReanchorTarget::Docs(
                docs.split_whitespace().map(PathBuf::from).collect(),
            )),
        }
    }

    fn includes(&self, doc: &Path) -> bool {
        match self {
            ReanchorTarget::All => true,
            ReanchorTarget::Docs(docs) => docs.contains(doc),
        }
    }
}

/// What a re-anchor run did, per doc and line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReanchorOutcome {
    /// `(doc, line, old citation, new citation)`.
    pub rewritten: Vec<(PathBuf, usize, String, String)>,
    /// A moved anchor whose symbol occurs on several lines: a human picks one.
    pub ambiguous: Vec<(PathBuf, usize, String, Vec<usize>)>,
    /// A rewritable citation the doc text could not be edited for (a marker
    /// split over two lines).
    pub skipped: Vec<(PathBuf, usize, String)>,
    /// Requested docs the gate does not read.
    pub unknown: Vec<PathBuf>,
}

impl ReanchorOutcome {
    pub fn report(&self) -> String {
        let mut out = format!(
            "docs-reanchor: {} rewritten, {} ambiguous, {} skipped\n",
            self.rewritten.len(),
            self.ambiguous.len(),
            self.skipped.len()
        );
        for (doc, line, old, new) in &self.rewritten {
            out.push_str(&format!(
                "  rewritten {}:{line}  [src: {old}] -> [src: {new}]\n",
                doc.display()
            ));
        }
        for (doc, line, citation, lines) in &self.ambiguous {
            let lines: Vec<String> = lines.iter().map(usize::to_string).collect();
            out.push_str(&format!(
                "  ambiguous {}:{line}  [src: {citation}]  the symbol occurs at lines {}: pick one by hand\n",
                doc.display(),
                lines.join(", ")
            ));
        }
        for (doc, line, citation) in &self.skipped {
            out.push_str(&format!(
                "  skipped {}:{line}  [src: {citation}]  the marker spans lines: edit it by hand\n",
                doc.display()
            ));
        }
        for doc in &self.unknown {
            out.push_str(&format!("  not a checked doc: {}\n", doc.display()));
        }
        out
    }
}

/// The citation with its line spec replaced, the path and `#symbol` kept
/// byte for byte. `None` unless it parses back to the same path and anchor.
fn respan(citation: &str, span: (usize, usize)) -> Option<String> {
    let hash = citation.rfind('#')?;
    let head = &citation[..hash];
    let colon = head.rfind(':')?;
    let spec = &head[colon + 1..];
    let lead = spec.len() - spec.trim_start().len();
    let trail = spec.len() - spec.trim_end().len();
    let new = format!(
        "{}{}{}{}",
        &citation[..colon + 1 + lead],
        span_label(span.0, span.1),
        &head[head.len() - trail..],
        &citation[hash..]
    );
    let (path, _, anchor) = file_citation_anchored(citation)?;
    let parsed = file_citation_anchored(&new)?;
    (parsed == (path, Some(span), anchor.clone()) && anchor.is_some()).then_some(new)
}

/// Byte range of 1-based `line` in `text`, without its newline.
fn line_range(text: &str, line: usize) -> Option<std::ops::Range<usize>> {
    let mut start = 0usize;
    for (index, content) in text.split('\n').enumerate() {
        if index + 1 == line {
            return Some(start..start + content.len());
        }
        start += content.len() + 1;
    }
    None
}

/// Byte offset in `line` of the first `[src: …]` marker whose whole inner
/// text is `citation`: plain text or a longer marker holding it never matches.
/// Identical markers fail identically, so the first one left is the right one.
fn marker_inner_at(line: &str, citation: &str) -> Option<usize> {
    markers_at(line).into_iter().find_map(|(chars, inner)| {
        if inner != citation {
            return None;
        }
        let open = line.char_indices().nth(chars)?.0 + "[src:".len();
        Some(open + line[open..].find(citation)?)
    })
}

fn reanchor_span(
    failure: &DocCitationFailure,
    root: &Path,
    diff: &mut Option<diff_map::DiffMap<'_>>,
) -> Result<Option<(usize, usize)>, String> {
    let Some(moved) = &failure.moved else {
        return Ok(None);
    };
    // This remains the first rule: a unique symbol needs no Git history.
    if moved.occurrences.len() == 1 {
        return Ok(suggested_span(moved.span, &moved.occurrences, moved.total));
    }
    let Some((path, Some(old), Some(anchor))) = file_citation_anchored(&failure.citation) else {
        return Ok(None);
    };
    let Some(mapped) = diff
        .as_mut()
        .map(|mapper| mapper.span(Path::new(&path), old))
        .transpose()?
        .flatten()
    else {
        return Ok(None);
    };
    // Mapping positions is not enough: the mapped range must still name the
    // cited whole-word anchor under the verifier's exact rule.
    Ok(anchor_miss(&root.join(path), mapped, &anchor)
        .is_none()
        .then_some(mapped))
}

/// Safe line moves in one doc, using the unique-symbol rule first and a Git
/// diff only for the repeated-symbol cases it cannot settle.
pub(crate) fn suggested_reanchors(
    text: &str,
    root: &Path,
    base: Option<&str>,
) -> Result<Vec<crate::core::citation_drift::Reanchor>, String> {
    let mut diff = base.map(|base| diff_map::DiffMap::new(root, base));
    let mut moves = Vec::new();
    let mut conflicts = BTreeSet::new();
    for failure in check_doc(text, root) {
        let Some(span) = reanchor_span(&failure, root, &mut diff)? else {
            continue;
        };
        let Some((path, Some(from), Some(_))) = file_citation_anchored(&failure.citation) else {
            continue;
        };
        let from = crate::core::citation_drift::cite_label(&path, Some(from));
        if conflicts.contains(&from) {
            continue;
        }
        if let Some(existing) = moves
            .iter()
            .position(|existing: &crate::core::citation_drift::Reanchor| existing.from == from)
        {
            if (moves[existing].line, moves[existing].end_line) != span {
                moves.remove(existing);
                conflicts.insert(from);
            }
            continue;
        }
        moves.push(crate::core::citation_drift::Reanchor {
            from,
            to: crate::core::citation_drift::cite_label(&path, Some(span)),
            line: span.0,
            end_line: span.1,
        });
    }
    Ok(moves)
}

/// Default base for audited-project revalidation.
pub(crate) fn default_reanchor_base(root: &Path) -> Result<Option<String>, String> {
    diff_map::default_merge_base(root)
}

/// `text` with every safely located moved anchor re-pointed at it.
fn reanchor_doc(
    doc: &Path,
    text: &str,
    root: &Path,
    base: Option<&str>,
    outcome: &mut ReanchorOutcome,
) -> Result<String, String> {
    let mut out = text.to_string();
    let mut diff = base.map(|base| diff_map::DiffMap::new(root, base));
    for failure in check_doc(text, root) {
        let Some(moved) = &failure.moved else {
            continue;
        };
        let Some(span) = reanchor_span(&failure, root, &mut diff)? else {
            // Several candidates and no proof from the diff: rewriting would
            // guess which one the claim means.
            outcome.ambiguous.push((
                doc.to_path_buf(),
                failure.line,
                failure.citation.clone(),
                moved.occurrences.clone(),
            ));
            continue;
        };
        let edited = respan(&failure.citation, span).and_then(|new| {
            let range = line_range(&out, failure.line)?;
            let at = marker_inner_at(&out[range.clone()], &failure.citation)? + range.start;
            out.replace_range(at..at + failure.citation.len(), &new);
            Some(new)
        });
        match edited {
            Some(new) => {
                outcome
                    .rewritten
                    .push((doc.to_path_buf(), failure.line, failure.citation, new))
            }
            None => outcome
                .skipped
                .push((doc.to_path_buf(), failure.line, failure.citation)),
        }
    }
    Ok(out)
}

/// Re-anchors the targeted docs of the corpus in place. Only files the gate
/// reads are ever written.
pub fn reanchor_corpus(
    root: &Path,
    target: &ReanchorTarget,
    base: Option<&str>,
) -> Result<ReanchorOutcome, String> {
    let mut outcome = ReanchorOutcome::default();
    let docs = corpus(root);
    if let ReanchorTarget::Docs(wanted) = target {
        outcome.unknown = wanted
            .iter()
            .filter(|d| !docs.contains(d))
            .cloned()
            .collect();
    }
    for doc in docs.iter().filter(|d| target.includes(d)) {
        let path = root.join(doc);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", doc.display()))?;
        let new = reanchor_doc(doc, &text, root, base, &mut outcome)?;
        if new != text {
            std::fs::write(&path, new)
                .map_err(|error| format!("cannot write {}: {error}", doc.display()))?;
        }
    }
    Ok(outcome)
}

/// The gate's result plus what a re-anchor run printed.
#[derive(Debug)]
pub struct GateRun {
    pub verdict: Verdict,
    pub notes: String,
}

/// Runs the gate under the variables CI and `make docs-reanchor` set, read
/// through `env`. `Err` on a variable that cannot be honoured.
pub fn gate(root: &Path, env: &dyn Fn(&str) -> Option<String>) -> Result<GateRun, String> {
    let mut scope = match env(CHANGED_ENV) {
        None => Scope::Strict,
        Some(list) => {
            let text = std::fs::read_to_string(&list)
                .map_err(|e| format!("{CHANGED_ENV}={list}: cannot read the changed paths: {e}"))?;
            Scope::for_changes(&text)
        }
    };
    let mut notes = String::new();
    if let Some(target) = env(REANCHOR_ENV).as_deref().and_then(ReanchorTarget::parse) {
        let base = env(REANCHOR_BASE_ENV).filter(|base| !base.trim().is_empty());
        let outcome = reanchor_corpus(root, &target, base.as_deref())
            .map_err(|e| format!("docs-reanchor: {e}"))?;
        notes = outcome.report();
        if let ReanchorTarget::Docs(docs) = target {
            scope = Scope::Files(docs);
        }
    }
    let verdict = Verdict::new(check_corpus(root), scope);
    if let Some(dir) = env(REPORT_DIR_ENV) {
        verdict
            .write_report(Path::new(&dir))
            .map_err(|e| format!("{REPORT_DIR_ENV}={dir}: {e}"))?;
    }
    Ok(GateRun { verdict, notes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::process::Command;

    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/sub")).unwrap();
        std::fs::write(root.join("src/a.rs"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(root.join("logo.png"), b"\x89PNG\r\n\x1a\n\0\0body").unwrap();
        tmp
    }

    fn reasons(doc: &str, root: &Path) -> Vec<String> {
        check_doc(doc, root).into_iter().map(|f| f.reason).collect()
    }

    #[test]
    fn resolving_citations_pass() {
        let tmp = repo();
        let doc = "A [src: file: src/a.rs:2] B `[src: file: src/a.rs:1-3]`\n\
                   C [src: file: logo.png] D [src: url: https://x.y] E [src: user: 2026-10-11: ok]\n";
        assert_eq!(check_doc(doc, tmp.path()), vec![]);
    }

    #[test]
    fn missing_file_out_of_bounds_and_lineless_fail() {
        let tmp = repo();
        let root = tmp.path();
        assert_eq!(
            reasons("[src: file: src/gone.rs:1]", root),
            ["file not found: src/gone.rs"]
        );
        assert_eq!(
            reasons("[src: file: src/a.rs:2-4]", root),
            ["line 4 beyond file length 3"]
        );
        assert_eq!(
            reasons("[src: file: src/a.rs]", root),
            ["no line: cite `path:line` or `path:start-end`"]
        );
    }

    #[test]
    fn the_last_line_passes_and_the_next_one_fails() {
        let tmp = repo();
        assert!(reasons("[src: file: src/a.rs:3]", tmp.path()).is_empty());
        assert_eq!(reasons("[src: file: src/a.rs:4]", tmp.path()).len(), 1);
    }

    #[test]
    fn directories_absolute_paths_escapes_and_bare_names_fail() {
        let tmp = repo();
        let root = tmp.path();
        assert_eq!(
            reasons("[src: file: src/sub:1]", root),
            ["not a file: src/sub"]
        );
        let abs = format!("[src: file: {}:1]", root.join("src/a.rs").display());
        assert_eq!(
            reasons(&abs, root),
            ["absolute path: cite it relative to the repo root"]
        );
        assert_eq!(
            reasons("[src: file: ../a.rs:1]", root),
            ["path leaves the repo root"]
        );
        // The runtime verifier walks for a unique basename; docs must not rely on it.
        assert_eq!(
            reasons("[src: file: a.rs:1]", root),
            ["file not found: a.rs"]
        );
    }

    #[test]
    fn a_backticked_citation_is_checked() {
        let tmp = repo();
        let failures = check_doc("x\n`[src: file: src/a.rs:9]`\n", tmp.path());
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].line, 2);
    }

    #[test]
    fn fenced_blocks_and_placeholders_are_skipped() {
        let tmp = repo();
        let doc = "```md\n[src: file: src/gone.rs:1]\n```\n\
                   `[src: file: <path>:<line>]` [src: file: <path>:<start-end>]\n\
                   `[src: file: src/{a,b}.rs:1]` `[src: file: .../x.rs:1]` [src: file: <REAL_FILE>:1]\n";
        assert_eq!(check_doc(doc, tmp.path()), vec![]);
    }

    #[test]
    fn failures_carry_the_doc_line_and_the_marker() {
        let tmp = repo();
        let doc = "intro\n\n```\ncode\n```\nsee [src:\nfile: src/a.rs:7] and `x`\n";
        let failures = check_doc(doc, tmp.path());
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].line, 6);
        assert_eq!(failures[0].citation, "file: src/a.rs:7");
        let text = report(&[(PathBuf::from("docs/x.md"), failures[0].clone())]);
        assert_eq!(
            text,
            "docs/x.md:6  [src: file: src/a.rs:7]  line 7 beyond file length 3\n"
        );
    }

    #[test]
    fn the_corpus_walk_skips_release_notes() {
        let tmp = repo();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs/releases")).unwrap();
        std::fs::create_dir_all(root.join("backend/docs/conventions")).unwrap();
        std::fs::write(root.join("docs/releases/0.1.md"), "[src: file: gone.rs:1]").unwrap();
        std::fs::write(root.join("docs/a.md"), "[src: file: src/a.rs:1]").unwrap();
        std::fs::write(
            root.join("backend/docs/conventions/b.md"),
            "ok\n[src: file: src/gone.rs:1]",
        )
        .unwrap();
        let failures = check_corpus(root);
        assert_eq!(failures.len(), 1);
        assert_eq!(
            failures[0].0,
            PathBuf::from("backend/docs/conventions/b.md")
        );
        assert_eq!(failures[0].1.line, 2);
    }

    #[test]
    fn an_anchor_inside_the_range_passes() {
        let tmp = repo();
        let doc = "[src: file: src/a.rs:1-2#two] `[src: file: src/a.rs:3#three]`";
        assert_eq!(check_doc(doc, tmp.path()), vec![]);
    }

    #[test]
    fn a_moved_anchor_fails_with_the_new_range() {
        let tmp = repo();
        let root = tmp.path();
        std::fs::write(root.join("src/u.rs"), "a\nb\nc\nd\nfn moved() {}\nf\n").unwrap();
        // A symbol that occurs once: the span keeps its length from it.
        assert_eq!(
            reasons("[src: file: src/u.rs:1-2#moved]", root),
            ["symbol `moved` not in lines 1-2; it appears at line 5 — try `src/u.rs:5-6#moved`"]
        );
        assert_eq!(
            reasons("[src: file: src/u.rs:6#moved]", root),
            ["symbol `moved` not in line 6; it appears at line 5 — try `src/u.rs:5#moved`"]
        );
        // Clamped to the file's last line.
        assert_eq!(
            reasons("[src: file: src/u.rs:1-3#moved]", root),
            ["symbol `moved` not in lines 1-3; it appears at line 5 — try `src/u.rs:5-6#moved`"]
        );
    }

    #[test]
    fn a_repeated_moved_anchor_lists_every_candidate_without_a_suggestion() {
        let tmp = repo();
        let root = tmp.path();
        let text = "x\nmoved\nmoved\nmoved\ny\nmoved\nmoved\nmoved\nmoved\n";
        std::fs::write(root.join("src/m.rs"), text).unwrap();
        let reasons = reasons("[src: file: src/m.rs:1#moved]", root);
        assert_eq!(
            reasons,
            ["symbol `moved` not in line 1; it appears at lines 2, 3, 4, 6, 7 — ambiguous: pick by hand among lines 2, 3, 4, 6, 7, 8, 9"]
        );
        assert!(!reasons[0].contains("try"));
    }

    #[test]
    fn a_definition_anchor_must_open_its_range_and_reanchor_proposes_that_range() {
        let tmp = repo();
        let root = tmp.path();
        let mut file = "x\n".repeat(12);
        file.push_str("/// Docs.\n#[inline]\nfn run() {\n    step();\n}\nfn step() {}\nz\n");
        std::fs::write(root.join("src/d.rs"), file).unwrap();
        // The doc comment and the attribute open the range; so do up to 5
        // sibling lines above them.
        for ok in ["13-17", "14-17", "15", "15-17", "8-17"] {
            let doc = format!("[src: file: src/d.rs:{ok}#run]");
            assert_eq!(check_doc(&doc, root), vec![], "{ok}");
        }
        // A call site stays citable although the file defines `step`.
        assert_eq!(check_doc("[src: file: src/d.rs:15-17#step]", root), vec![]);
        // The definition sits well inside the range.
        assert_eq!(
            reasons("[src: file: src/d.rs:1-17#run]", root),
            ["symbol `run` is defined at line 15, not at the start of lines 1-17: a definition anchor must start the range — try `src/d.rs:13-19#run`"]
        );
        assert_eq!(reasons("[src: file: src/d.rs:7-17#run]", root).len(), 1);
        // The range holds neither the definition nor a call.
        assert_eq!(
            reasons("[src: file: src/d.rs:1#run]", root),
            ["symbol `run` not in line 1; it appears at line 15 — try `src/d.rs:13#run`"]
        );
        let (new, outcome) = reanchor(
            root,
            "docs/a.md",
            "[src: file: src/d.rs:1-3#run] `[src: file: src/d.rs:1-17#run]`\n",
        );
        assert_eq!(
            new,
            "[src: file: src/d.rs:13-15#run] `[src: file: src/d.rs:13-19#run]`\n"
        );
        assert_eq!(outcome.rewritten.len(), 2);
        assert!(check_corpus(root).is_empty());
    }

    #[test]
    fn a_vanished_anchor_fails_without_a_suggestion() {
        let tmp = repo();
        assert_eq!(
            reasons("[src: file: src/a.rs:1-3#gone]", tmp.path()),
            ["symbol `gone` not in lines 1-3, nor anywhere in the file — rewrite the claim or cite where it lives"]
        );
    }

    /// The CI check and the runtime verifier (and so the badge) share the
    /// anchor rule: no tolerance window on either side of the range.
    #[test]
    fn the_ci_check_and_the_runtime_verifier_apply_one_anchor_rule() {
        use crate::core::anti_halluc::verify_source_marker;
        let tmp = repo();
        let root = tmp.path();
        let mut file = "a\n".repeat(8);
        file.push_str("fn edge() {}\nd\ne\n");
        std::fs::write(root.join("src/w.rs"), file).unwrap();
        for (citation, holds) in [
            ("file: src/w.rs:9#edge", true),
            ("file: src/w.rs:9-11#edge", true),
            // The sibling allowance: 5 lines above the definition.
            ("file: src/w.rs:4-11#edge", true),
            // Past it the definition is merely inside the range.
            ("file: src/w.rs:3-11#edge", false),
            ("file: src/w.rs:1-9#edge", false),
            ("file: src/w.rs:8#edge", false),
            ("file: src/w.rs:10-11#edge", false),
            ("file: src/w.rs:1-8#edge", false),
        ] {
            let ci = check_doc(&format!("[src: {citation}]"), root).is_empty();
            let runtime =
                verify_source_marker(citation, Some(root)).status == SourceStatus::Verified;
            assert_eq!((ci, runtime), (holds, holds), "{citation}");
        }
    }

    fn broken_repo() -> tempfile::TempDir {
        let tmp = repo();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs/sub")).unwrap();
        std::fs::create_dir_all(root.join("backend/docs")).unwrap();
        std::fs::write(root.join("docs/a.md"), "ok\n[src: file: src/a.rs:9]\n").unwrap();
        std::fs::write(root.join("docs/sub/b.md"), "[src: file: src/a.rs:1]\n").unwrap();
        std::fs::write(
            root.join("backend/docs/c.md"),
            "[src: file: src/gone.rs:1]\n",
        )
        .unwrap();
        tmp
    }

    fn run_gate(root: &Path, vars: &[(&str, String)]) -> Result<GateRun, String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        gate(root, &|name: &str| vars.get(name).cloned())
    }

    fn changed(root: &Path, paths: &str) -> (&'static str, String) {
        let list = root.join("changed.txt");
        std::fs::write(&list, paths).unwrap();
        (CHANGED_ENV, list.display().to_string())
    }

    #[test]
    fn the_scope_follows_the_paths_a_pull_request_changes() {
        assert_eq!(
            Scope::for_changes("backend/src/agents/runner.rs\n"),
            Scope::Warn
        );
        assert_eq!(Scope::for_changes(""), Scope::Warn);
        // Release notes and non-Markdown files under docs are not read.
        assert_eq!(
            Scope::for_changes("docs/releases/0.15.0.md\ndocs/img.png\ndocsy/x.md\n"),
            Scope::Warn
        );
        assert_eq!(
            Scope::for_changes("src/x.rs\n docs/a.md \nbackend/docs/c.md\n\n"),
            Scope::Files(
                [
                    PathBuf::from("docs/a.md"),
                    PathBuf::from("backend/docs/c.md")
                ]
                .into_iter()
                .collect()
            )
        );
        assert_eq!(
            Scope::for_changes("docs/été.md\n"),
            Scope::Files([PathBuf::from("docs/été.md")].into_iter().collect())
        );
        // A release bumps VERSION: every doc is checked.
        assert_eq!(Scope::for_changes("VERSION\ndocs/a.md\n"), Scope::Strict);
        assert!(!is_checked_doc("backend/docsx/a.md"));
        assert!(is_checked_doc("backend/docs/a/b.md"));
    }

    #[test]
    fn a_code_only_pull_request_counts_warnings_and_never_fails() {
        let tmp = broken_repo();
        let root = tmp.path();
        let dir = root.join("report");
        let run = run_gate(
            root,
            &[
                changed(root, "backend/src/agents/runner.rs\n"),
                (REPORT_DIR_ENV, dir.display().to_string()),
            ],
        )
        .unwrap();
        assert_eq!(run.verdict.scope, Scope::Warn);
        assert!(run.verdict.blocking.is_empty());
        assert_eq!(run.verdict.warnings.len(), 2);
        let annotations = std::fs::read_to_string(dir.join("annotations.txt")).unwrap();
        assert_eq!(annotations.lines().count(), 2);
        assert!(annotations.starts_with(
            "::warning file=backend/docs/c.md,line=1,title=Doc citation::[src: file: src/gone.rs:1] file not found: src/gone.rs\n"
        ), "{annotations}");
        assert!(!annotations.contains("::error"));
        let summary = std::fs::read_to_string(dir.join("summary.md")).unwrap();
        assert!(
            summary.contains("**2** unresolved: 0 blocking, 2 warning(s)"),
            "{summary}"
        );
        assert!(summary.contains("Repair: `make docs-reanchor`"));
    }

    #[test]
    fn a_docs_pull_request_blocks_only_on_the_docs_it_changes() {
        let tmp = broken_repo();
        let root = tmp.path();
        let run = run_gate(root, &[changed(root, "docs/a.md\ndocs/sub/b.md\n")]).unwrap();
        assert_eq!(run.verdict.blocking.len(), 1);
        assert_eq!(run.verdict.blocking[0].0, PathBuf::from("docs/a.md"));
        assert_eq!(run.verdict.warnings.len(), 1);
        let message = run.verdict.failure_message();
        assert!(
            message.starts_with("1 doc citation(s) do not resolve (strict on the 2 doc(s)"),
            "{message}"
        );
        assert!(message.contains("docs/a.md:2  [src: file: src/a.rs:9]"));
        assert!(
            message.contains("`make docs-reanchor DOCS=\"docs/a.md\"`"),
            "{message}"
        );
        assert!(message.contains("1 more outside the scope (warnings):\nbackend/docs/c.md:1"));
        let annotations = run.verdict.annotations();
        assert!(
            annotations.starts_with("::error file=docs/a.md,line=2,"),
            "{annotations}"
        );
        // A clean touched doc does not block on another doc's drift.
        let run = run_gate(root, &[changed(root, "docs/sub/b.md\n")]).unwrap();
        assert!(run.verdict.blocking.is_empty());
        assert_eq!(run.verdict.warnings.len(), 2);
    }

    #[test]
    fn local_release_and_manual_runs_are_strict() {
        let tmp = broken_repo();
        let root = tmp.path();
        let run = run_gate(root, &[]).unwrap();
        assert_eq!(run.verdict.scope, Scope::Strict);
        assert_eq!(run.verdict.blocking.len(), 2);
        assert!(run
            .verdict
            .failure_message()
            .contains("Repair: `make docs-reanchor` "));
        let run = run_gate(root, &[changed(root, "VERSION\nCHANGELOG.md\n")]).unwrap();
        assert_eq!(run.verdict.blocking.len(), 2);
        // A list CI meant to pass but cannot be read never degrades to a warning.
        let missing = root.join("nope.txt").display().to_string();
        let err = run_gate(root, &[(CHANGED_ENV, missing)]).unwrap_err();
        assert!(err.contains("cannot read the changed paths"), "{err}");
    }

    #[test]
    fn workflow_command_values_are_escaped() {
        let failure = DocCitationFailure {
            line: 3,
            citation: "file: a,b.rs:1".into(),
            reason: "50% gone\nnext".into(),
            moved: None,
        };
        let verdict = Verdict::new(vec![(PathBuf::from("docs/x:y.md"), failure)], Scope::Strict);
        assert_eq!(
            verdict.annotations(),
            "::error file=docs/x%3Ay.md,line=3,title=Doc citation::[src: file: a,b.rs:1] 50%25 gone%0Anext\n"
        );
        let clean = Verdict::new(vec![], Scope::Warn);
        assert_eq!(
            clean.summary(),
            "### Doc citations\n\nEvery doc citation resolves.\n"
        );
        assert_eq!(clean.annotations(), "");
    }

    /// Code where `once` sits on line 6 only and `twice` on lines 2 and 9.
    fn moved_repo() -> tempfile::TempDir {
        let tmp = repo();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(
            tmp.path().join("src/m.rs"),
            "a\nfn twice() {}\nc\nd\ne\nfn once() {}\ng\nh\nlet t = twice();\n",
        )
        .unwrap();
        tmp
    }

    fn reanchor(root: &Path, doc: &str, text: &str) -> (String, ReanchorOutcome) {
        let path = root.join(doc);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        let outcome = reanchor_corpus(root, &ReanchorTarget::All, None).unwrap();
        (std::fs::read_to_string(&path).unwrap(), outcome)
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_commit(root: &Path, message: &str, paths: &[&str]) {
        let mut add = vec!["add", "--"];
        add.extend_from_slice(paths);
        git(root, &add);
        git(
            root,
            &[
                "-c",
                "user.name=Kronn Test",
                "-c",
                "user.email=kronn@example.invalid",
                "commit",
                "-m",
                message,
            ],
        );
    }

    fn repeated_anchor_git_repo(change_endpoint: bool) -> tempfile::TempDir {
        let tmp = repo();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(
            root.join("src/m.rs"),
            "fn target() {}\n\nfn helper() {}\nfn target() {\n    let first = 1;\n    let second = 2;\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("docs/a.md"),
            "Target [src: file: src/m.rs:4-7#target].\n",
        )
        .unwrap();
        git(root, &["init", "-b", "main"]);
        git_commit(root, "baseline", &["src/m.rs", "docs/a.md"]);
        git(root, &["switch", "-c", "feature"]);

        let mut moved = String::from("fn target() {}\n\nfn helper() {}\n");
        for index in 0..60 {
            moved.push_str(&format!("let inserted_{index} = {index};\n"));
        }
        moved.push_str(if change_endpoint {
            "pub fn target() {\n"
        } else {
            "fn target() {\n"
        });
        moved.push_str("    let first = 1;\n    let second = 2;\n}\n");
        std::fs::write(root.join("src/m.rs"), moved).unwrap();
        git_commit(root, "move target", &["src/m.rs"]);
        tmp
    }

    #[test]
    fn reanchoring_moves_a_unique_symbol_keeping_the_span_and_the_anchor() {
        let tmp = moved_repo();
        let root = tmp.path();
        let text =
            "Intro.\nSee [src: file: src/m.rs:1-2#once] and `[src: file: src/m.rs:3#once]`.\n";
        let (new, outcome) = reanchor(root, "docs/a.md", text);
        assert_eq!(
            new,
            "Intro.\nSee [src: file: src/m.rs:6-7#once] and `[src: file: src/m.rs:6#once]`.\n"
        );
        assert_eq!(outcome.rewritten.len(), 2);
        assert_eq!(
            outcome.rewritten[0],
            (
                PathBuf::from("docs/a.md"),
                2,
                "file: src/m.rs:1-2#once".into(),
                "file: src/m.rs:6-7#once".into()
            )
        );
        assert!(check_corpus(root).is_empty());
        // Clamped to the last line; spacing around the spec is kept.
        let (new, _) = reanchor(root, "docs/b.md", "[src: file: src/m.rs: 1-5 #once]\n");
        assert_eq!(new, "[src: file: src/m.rs: 6-9 #once]\n");
    }

    #[test]
    fn reanchoring_edits_the_failing_marker_not_an_earlier_copy_of_its_text() {
        let tmp = moved_repo();
        let root = tmp.path();
        let text = "é `file: src/m.rs:1#once` [src: file: src/m.rs:1#onceX] [src: file: src/m.rs:1#once]\n";
        let (new, outcome) = reanchor(root, "docs/a.md", text);
        assert_eq!(
            new,
            "é `file: src/m.rs:1#once` [src: file: src/m.rs:1#onceX] [src: file: src/m.rs:6#once]\n"
        );
        assert_eq!(outcome.rewritten.len(), 1);
        assert_eq!(marker_inner_at("[SRC: a] [src:  a ]", "a"), Some(6));
        assert_eq!(marker_inner_at("a [src: ab]", "a"), None);
    }

    #[test]
    fn reanchoring_lists_the_candidates_of_a_repeated_symbol_and_rewrites_nothing() {
        let tmp = moved_repo();
        let root = tmp.path();
        let text = "[src: file: src/m.rs:4-5#twice]\n";
        let (new, outcome) = reanchor(root, "docs/a.md", text);
        assert_eq!(new, text);
        assert!(outcome.rewritten.is_empty());
        assert_eq!(
            outcome.ambiguous,
            vec![(
                PathBuf::from("docs/a.md"),
                1,
                "file: src/m.rs:4-5#twice".into(),
                vec![2, 9]
            )]
        );
        assert!(outcome
            .report()
            .contains("the symbol occurs at lines 2, 9: pick one by hand"));
        // The strict check still fails on it.
        assert_eq!(check_corpus(root).len(), 1);
    }

    #[test]
    fn reanchoring_uses_the_diff_to_settle_a_repeated_symbol() {
        let tmp = repeated_anchor_git_repo(false);
        let root = tmp.path();
        let run = run_gate(
            root,
            &[
                (REANCHOR_ENV, "all".into()),
                (REANCHOR_BASE_ENV, "main".into()),
            ],
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("docs/a.md")).unwrap(),
            "Target [src: file: src/m.rs:64-67#target].\n"
        );
        assert!(
            run.notes.contains("1 rewritten, 0 ambiguous"),
            "{}",
            run.notes
        );
        assert!(run.verdict.blocking.is_empty());
    }

    #[test]
    fn reanchoring_keeps_changed_hunk_endpoints_ambiguous() {
        let tmp = repeated_anchor_git_repo(true);
        let root = tmp.path();
        let original = std::fs::read_to_string(root.join("docs/a.md")).unwrap();
        let run = run_gate(
            root,
            &[
                (REANCHOR_ENV, "all".into()),
                (REANCHOR_BASE_ENV, "main".into()),
            ],
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("docs/a.md")).unwrap(),
            original
        );
        assert!(
            run.notes.contains("0 rewritten, 1 ambiguous"),
            "{}",
            run.notes
        );
        assert!(
            run.notes
                .contains("the symbol occurs at lines 1, 64: pick one by hand"),
            "{}",
            run.notes
        );
        assert_eq!(run.verdict.blocking.len(), 1);
    }

    #[test]
    fn reanchoring_never_touches_other_failures_or_files_outside_the_docs() {
        let tmp = moved_repo();
        let root = tmp.path();
        let text = "[src: file: src/m.rs:20#once] [src: file: src/gone.rs:1#once]\n\
                    [src: file: src/m.rs] [src: file: src/m.rs:1#vanished] [src: file: src/m.rs:1-2]\n";
        let (new, outcome) = reanchor(root, "docs/a.md", text);
        assert_eq!(new, text);
        assert_eq!(outcome, ReanchorOutcome::default());
        // Release notes, root Markdown and a split marker stay as they are.
        let moved = "[src: file: src/m.rs:1#once]\n";
        for doc in ["docs/releases/0.1.md", "README.md", "backend/src/x.md"] {
            let (new, _) = reanchor(root, doc, moved);
            assert_eq!(new, moved, "{doc}");
        }
        let split = "see [src:\nfile: src/m.rs:1#once]\n";
        let (new, outcome) = reanchor(root, "docs/c.md", split);
        assert_eq!(new, split);
        assert_eq!(outcome.skipped.len(), 1);
    }

    #[test]
    fn a_targeted_reanchor_rewrites_only_the_listed_docs_and_scopes_the_check() {
        let tmp = moved_repo();
        let root = tmp.path();
        let moved = "[src: file: src/m.rs:1#once]\n";
        std::fs::write(root.join("docs/a.md"), moved).unwrap();
        std::fs::write(root.join("docs/b.md"), moved).unwrap();
        let run = run_gate(root, &[(REANCHOR_ENV, "docs/a.md docs/nope.md".into())]).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("docs/a.md")).unwrap(),
            "[src: file: src/m.rs:6#once]\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("docs/b.md")).unwrap(),
            moved
        );
        assert!(
            run.notes.contains("not a checked doc: docs/nope.md"),
            "{}",
            run.notes
        );
        assert!(run.verdict.blocking.is_empty());
        assert_eq!(run.verdict.warnings.len(), 1);
        assert_eq!(ReanchorTarget::parse("  "), None);
        assert_eq!(ReanchorTarget::parse("1"), Some(ReanchorTarget::All));
    }

    /// The gate itself: every citation in the repo's docs resolves. CI sets
    /// the scope variables; `make docs-reanchor` sets the re-anchor one.
    #[test]
    fn docs_citations_resolve_in_the_repo() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let run =
            gate(root, &|name: &str| std::env::var(name).ok()).unwrap_or_else(|e| panic!("{e}"));
        if run.verdict.blocking.is_empty() {
            eprint!("{}", run.notes);
            if !run.verdict.warnings.is_empty() {
                eprint!("{}", run.verdict.summary());
            }
        }
        assert!(
            run.verdict.blocking.is_empty(),
            "{}{}",
            run.notes,
            run.verdict.failure_message()
        );
    }
}
