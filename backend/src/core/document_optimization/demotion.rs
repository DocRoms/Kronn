//! KT-1370 — what is left of a citation marker Kronn cannot repair.
//!
//! After the last attempt of a step, a `[src: file: …]` marker that neither the
//! agent nor the exact replacements of `mechanical_fix` could make acceptable
//! (docroms step 8: a text file cited without a line, three attempts running)
//! no longer interrupts the audit. It stops being a citation: the path is named
//! in backticks and flagged, so a reader never takes it for a verified
//! reference. A secret file is the one exception, nothing of it is written.
//! Inline code and fenced code are quoted examples and are never touched.

use super::{check_citation, without_anchor, CitationCheck};
use std::path::Path;

/// The flag written after a demoted path. The gate does not read it: a
/// backticked path is not a citation, so revalidation reports it as it stands.
pub const UNVERIFIED_FLAG: &str = "(unverified reference)";

/// Shown in place of the marker of a secret file, which is never repeated.
const SECRET_MARKER: &str = "[src: file: <secret file>]";

/// One marker demoted in a document.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DemotedReference {
    /// The document that held the marker.
    pub doc: String,
    /// The marker as the agent wrote it (one that names a secret file is
    /// withheld: a fixed placeholder stands for it).
    pub marker: String,
}

/// The document with every refused marker demoted.
pub struct Demotion {
    pub content: String,
    pub references: Vec<DemotedReference>,
}

/// `None` when the document holds no marker the gate refuses.
pub fn demote_unverifiable(project: &Path, rel: &str, content: &str) -> Option<Demotion> {
    let mut references = Vec::new();
    let output = rewrite_markers(content, |raw| {
        if raw.contains(['<', '>'])
            || !matches!(check_citation(project, rel, raw), CitationCheck::Defect(_))
        {
            return None;
        }
        let (text, marker) = demoted_text(project, raw);
        references.push(DemotedReference {
            doc: rel.to_string(),
            marker,
        });
        Some(text)
    });
    (!references.is_empty()).then_some(Demotion {
        content: output,
        references,
    })
}

/// The document as it may be copied to Kronn's own state: every marker that
/// names a secret file is replaced by a fixed placeholder, so a backup of the
/// text before a demotion does not keep what the demotion removes.
pub fn scrub_secret_markers(project: &Path, content: &str) -> String {
    rewrite_markers(content, |raw| {
        names_secret(project, raw).then(|| SECRET_MARKER.to_string())
    })
}

/// Rewrites each `[src: file: …]` marker `replace` answers for; inline code and
/// fenced code are quoted examples and are never offered to it.
fn rewrite_markers(content: &str, mut replace: impl FnMut(&str) -> Option<String>) -> String {
    let mask = crate::api::audit::anti_hallu_enforce::code_line_mask(content, true);
    let mut output = String::with_capacity(content.len());
    for (index, line) in content.split_inclusive('\n').enumerate() {
        if mask.in_fence.get(index).copied().unwrap_or(false) {
            output.push_str(line);
            continue;
        }
        output.push_str(&rewrite_line(line, &mut replace));
    }
    output
}

fn rewrite_line(line: &str, replace: &mut impl FnMut(&str) -> Option<String>) -> String {
    const OPEN: &str = "[src: file:";
    let mut output = String::new();
    let mut rest = line;
    let mut consumed = 0;
    while let Some(start) = rest.find(OPEN) {
        let Some(end) = rest[start..].find(']').map(|i| start + i) else {
            break;
        };
        let raw = rest[start + OPEN.len()..end].trim();
        let in_code = line[..consumed + start].matches('`').count() % 2 == 1;
        output.push_str(&rest[..start]);
        match (!in_code).then(|| replace(raw)).flatten() {
            Some(text) => output.push_str(&text),
            None => output.push_str(&rest[start..=end]),
        }
        consumed += end + 1;
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    output
}

/// What surrounds a path in prose without being part of it.
const DELIMITERS: [char; 14] = [
    '`', '"', '\'', '(', ')', '[', ']', '<', '>', '{', '}', ';', '—', '–',
];

/// More candidate starts than this and the marker is not a citation anyone
/// wrote by hand: it is judged secret rather than scanned.
const MAX_CANDIDATE_STARTS: usize = 48;

/// Whether a marker names, or resolves to, a secret file ANYWHERE in it. Every
/// stretch of the marker that starts at its beginning or after a space, a comma
/// or a colon and ends at a space, a comma, a colon or its end is a candidate
/// path (spaces included: `docs/key view.txt` may be a link to a secret; quotes,
/// brackets and the like stripped, as for any word of a comment), judged in the
/// written, the percent-decoded and the line-less form by the names the path
/// walks and every link it passes through (`is_secret_path`).
fn names_secret(project: &Path, raw: &str) -> bool {
    let is_boundary = |c: char| c.is_whitespace() || c == ',' || c == ':';
    let starts: Vec<usize> = std::iter::once(0)
        .chain(
            raw.char_indices()
                .filter(|(_, c)| is_boundary(*c))
                .map(|(index, c)| index + c.len_utf8()),
        )
        .collect();
    if starts.len() > MAX_CANDIDATE_STARTS {
        return true;
    }
    let ends: Vec<usize> = raw
        .char_indices()
        .filter(|(_, c)| is_boundary(*c))
        .map(|(index, _)| index)
        .chain(std::iter::once(raw.len()))
        .collect();
    starts.iter().any(|&start| {
        ends.iter()
            .filter(|&&end| end > start)
            .any(|&end| candidate_is_secret(project, &raw[start..end]))
    })
}

fn candidate_is_secret(project: &Path, candidate: &str) -> bool {
    let candidate = candidate.trim().trim_matches(DELIMITERS);
    let candidate = candidate.strip_prefix("file:").unwrap_or(candidate);
    let candidate = candidate.trim().trim_matches(DELIMITERS);
    !candidate.is_empty()
        && word_forms(candidate)
            .iter()
            .any(|form| path_is_secret(project, form))
}

/// What a word may stand for as a path: itself and without its line, then each
/// round of percent-decoding (`%252e` takes two), trailing colon dropped.
fn word_forms(word: &str) -> Vec<String> {
    let mut forms = Vec::new();
    let mut current = word.to_string();
    for _ in 0..4 {
        let clean = current.trim_end_matches([':', ';']).to_string();
        for form in [clean.clone(), cited_path(&clean).to_string()] {
            if !form.is_empty() && !forms.contains(&form) {
                forms.push(form);
            }
        }
        match crate::core::message_file_links::percent_decode(&current) {
            Some(decoded) if decoded != current => current = decoded,
            _ => break,
        }
    }
    forms
}

fn path_is_secret(project: &Path, path: &str) -> bool {
    let path = Path::new(path);
    crate::acp::secret_files::is_secret_file(path)
        || crate::acp::secret_files::is_secret_path(&project.join(path))
}

/// What replaces the marker whose content is `raw`, and how the marker is
/// listed. A bundle (`a:1, b:2`) becomes one flagged list. Whatever else the
/// agent wrote inside the marker (a comment, a quote of a file) is dropped:
/// a demoted marker carries its path(s) and nothing else, in the text and in
/// the list alike.
///
/// A marker that names a secret file anywhere is removed entirely: no path, no
/// comment, no remainder of its bundle, and only a fixed placeholder is listed.
fn demoted_text(project: &Path, raw: &str) -> (String, String) {
    if names_secret(project, raw) {
        return (String::new(), SECRET_MARKER.to_string());
    }
    let mut paths: Vec<String> = Vec::new();
    for (index, piece) in raw.split(',').enumerate() {
        let piece = piece.trim();
        let piece = piece.strip_prefix("file:").map(str::trim).unwrap_or(piece);
        let token = match piece.chars().next() {
            Some(quote @ ('"' | '\'')) => piece[1..].split(quote).next().unwrap_or_default(),
            _ => piece.split_whitespace().next().unwrap_or_default(),
        };
        let token = token.trim_end_matches([':', ';', '—', '–']);
        let cited = cited_path(token);
        let path = cited.trim_matches(DELIMITERS);
        // Past the first piece, a word is a path only when it is cited like one
        // (a line, a range, `:exists`) or is a file of the project: the rest is
        // the agent's comment.
        let cited_like_a_path = cited != token || project.join(path).exists();
        if path.is_empty() || (index > 0 && !cited_like_a_path) {
            continue;
        }
        if !paths.iter().any(|known| known == path) {
            paths.push(path.to_string());
        }
    }
    if paths.is_empty() {
        return (String::new(), "[src: file: <no path>]".to_string());
    }
    let marker = format!("[src: file: {}]", paths.join(", "));
    let list = paths
        .iter()
        .map(|path| format!("`{path}`"))
        .collect::<Vec<_>>()
        .join(", ");
    (format!("{list} {UNVERIFIED_FLAG}"), marker)
}

/// The path a reference names: without its line, range, anchor or `:exists`.
fn cited_path(reference: &str) -> &str {
    match reference.rsplit_once(':') {
        Some((path, tail))
            if crate::core::anti_halluc::is_presence_marker(tail)
                || crate::core::anti_halluc::parse_line_bounds(without_anchor(tail)).is_some() =>
        {
            path.trim()
        }
        _ => reference,
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "demotion_tests.rs"]
mod tests;
