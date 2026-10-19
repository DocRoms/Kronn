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

/// Whether a marker names, or resolves to, a secret file ANYWHERE in it: any
/// comma piece, any word of a piece (a comment may quote one), in the written,
/// the percent-decoded and the line-less form, judged by the names the path
/// walks and every link it passes through (`is_secret_path`).
fn names_secret(project: &Path, raw: &str) -> bool {
    raw.split(',').any(|piece| {
        piece.split_whitespace().any(|word| {
            let word = word.trim_matches(['`', '"', '\'', '(', ')', ';', '—', '–']);
            let word = word.strip_prefix("file:").unwrap_or(word);
            word_forms(word)
                .iter()
                .any(|form| path_is_secret(project, form))
        })
    })
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
/// listed. A bundle (`a:1, b:2`) becomes one flagged list; a comment the agent
/// put inside the marker follows the flag instead of vanishing with it.
///
/// A marker that names a secret file anywhere is removed entirely: no path, no
/// comment, no remainder of its bundle, and only a fixed placeholder is listed.
fn demoted_text(project: &Path, raw: &str) -> (String, String) {
    if names_secret(project, raw) {
        return (String::new(), SECRET_MARKER.to_string());
    }
    let mut paths: Vec<String> = Vec::new();
    let mut comment = None;
    for piece in raw.split(',') {
        let piece = piece.trim();
        let piece = piece.strip_prefix("file:").map(str::trim).unwrap_or(piece);
        let (token, rest) = match piece.split_once(char::is_whitespace) {
            Some((token, rest)) => (token, Some(rest.trim())),
            None => (piece, None),
        };
        let path = cited_path(token.trim_end_matches([':', ';', '—', '–']));
        if path.is_empty() || path.contains('`') {
            continue;
        }
        if let Some(rest) = rest {
            let rest = rest.trim_start_matches(['—', '–', '-', ':']).trim();
            if !rest.is_empty() && !rest.contains(['`', '[', ']']) && comment.is_none() {
                comment = Some(rest.to_string());
            }
        }
        if !paths.iter().any(|known| known == path) {
            paths.push(path.to_string());
        }
    }
    let marker = format!("[src: file: {raw}]");
    if paths.is_empty() {
        return (String::new(), marker);
    }
    let list = paths
        .iter()
        .map(|path| format!("`{path}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let text = match comment {
        Some(comment) => format!("{list} {UNVERIFIED_FLAG} — {comment}"),
        None => format!("{list} {UNVERIFIED_FLAG}"),
    };
    (text, marker)
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
