//! KT-1353 — the gate's own exact replacements, applied by Kronn.
//!
//! Some refusals of the documentary gate name the one thing to write instead: a
//! link relative to the document's folder, the presence form of a secret file,
//! the single file a citation meant, a range one line past the end. An agent
//! that was told so three times and still left them failed its step over a
//! mechanical edit. Each fix here re-runs the gate's own judgement on the
//! marker or link, so it is only offered where the message carries the exact
//! text; an ambiguous suggestion (several candidate paths, a text file cited
//! without a line, a range far outside the file) is never applied.

use super::{
    cited_file, did_you_mean_candidates, is_project_dir, last_line_of, line_count, link_resolution,
    markdown_targets, root_relative_fix, under_root, visible_markdown, without_anchor,
};
use std::collections::BTreeMap;
use std::path::Path;

/// One edit Kronn made on a document, for the run log.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AppliedFix {
    pub path: String,
    pub kind: &'static str,
    pub from: String,
    pub to: String,
}

/// The document with every exact replacement applied, and what was edited.
pub struct Repaired {
    pub content: String,
    pub fixes: Vec<AppliedFix>,
}

/// `None` when the gate suggests no exact replacement in `content`. Fenced code
/// and inline code are quoted examples: never touched.
pub fn mechanical_repair(project: &Path, rel: &str, content: &str) -> Option<Repaired> {
    let visible = visible_markdown(content);
    let mut citations: BTreeMap<String, (&'static str, String)> = BTreeMap::new();
    for (at, marker) in visible.match_indices("[src: file:") {
        let Some(raw) = visible[at + marker.len()..].split(']').next() else {
            continue;
        };
        let raw = raw.trim();
        if let Some(fix) = citation_fix(project, rel, raw) {
            citations.insert(raw.to_string(), fix);
        }
    }
    let source = project.join(rel);
    let mut links: BTreeMap<String, String> = BTreeMap::new();
    for target in markdown_targets(content) {
        if let Some(fixed) = link_fix(project, &source, &target) {
            links.insert(target, fixed);
        }
    }
    if citations.is_empty() && links.is_empty() {
        return None;
    }
    let mask = crate::api::audit::anti_hallu_enforce::code_line_mask(content, true);
    let mut fixes = Vec::new();
    let mut output = String::with_capacity(content.len());
    for (index, line) in content.split_inclusive('\n').enumerate() {
        if mask.in_fence.get(index).copied().unwrap_or(false) {
            output.push_str(line);
            continue;
        }
        let line = fix_citations(line, &citations, rel, &mut fixes);
        output.push_str(&fix_links(&line, &links, rel, &mut fixes));
    }
    (!fixes.is_empty()).then_some(Repaired {
        content: output,
        fixes,
    })
}

/// A marker the line holds outside inline code, rewritten when it is in `fixes`.
fn fix_citations(
    line: &str,
    citations: &BTreeMap<String, (&'static str, String)>,
    rel: &str,
    applied: &mut Vec<AppliedFix>,
) -> String {
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
        match citations.get(raw) {
            Some((kind, to)) if !in_code => {
                output.push_str(to);
                applied.push(AppliedFix {
                    path: rel.to_string(),
                    kind,
                    from: raw.to_string(),
                    to: to.clone(),
                });
            }
            _ => output.push_str(&rest[start..=end]),
        }
        consumed += end + 1;
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    output
}

/// An inline link `](target)`, `](<target>)` or `](target "title")` outside
/// inline code, pointed at the fixed target.
fn fix_links(
    line: &str,
    links: &BTreeMap<String, String>,
    rel: &str,
    applied: &mut Vec<AppliedFix>,
) -> String {
    let mut line = line.to_string();
    for (target, fixed) in links {
        for (from, to) in [
            (format!("]({target})"), format!("]({fixed})")),
            (format!("](<{target}>)"), format!("](<{fixed}>)")),
            (format!("]({target} "), format!("]({fixed} ")),
        ] {
            let mut output = String::new();
            let mut rest = line.as_str();
            let mut consumed = 0;
            while let Some(at) = rest.find(&from) {
                let in_code = line[..consumed + at].matches('`').count() % 2 == 1;
                output.push_str(&rest[..at]);
                if in_code {
                    output.push_str(&from);
                } else {
                    output.push_str(&to);
                    applied.push(AppliedFix {
                        path: rel.to_string(),
                        kind: "link_base",
                        from: target.clone(),
                        to: fixed.clone(),
                    });
                }
                consumed += at + from.len();
                rest = &rest[at + from.len()..];
            }
            output.push_str(rest);
            line = output;
        }
    }
    line
}

/// The link relative to the document's folder, when the target is a repository
/// path and that link resolves (the gate's "write `…`" suggestion).
fn link_fix(project: &Path, source: &Path, target: &str) -> Option<String> {
    if target.starts_with("http://") || target.starts_with("https://") || target.starts_with('#') {
        return None;
    }
    let clean = target.split('#').next().unwrap_or("");
    let base = source.parent().unwrap_or(project);
    if link_resolution(project, base, clean).is_some() {
        return None;
    }
    let fix = root_relative_fix(project, source, clean)?;
    link_resolution(project, base, &fix)?;
    Some(format!("{fix}{}", &target[clean.len()..]))
}

/// The text the gate would accept in place of the whole marker
/// `[src: file: raw]`, with the kind of fix. Only exact cases: no list (the
/// bundle split owns those), no placeholder, and a comment only when it trails
/// a `path:line` the gate accepts as written (KT-1357).
fn citation_fix(project: &Path, rel: &str, raw: &str) -> Option<(&'static str, String)> {
    if raw.is_empty() || raw.contains(['<', '>', ',']) {
        return None;
    }
    if raw.contains(char::is_whitespace) {
        return prose_fix(project, rel, raw);
    }
    if let Some(fix) = empty_file_fix(project, raw) {
        return Some(fix);
    }
    if let Some(fix) = secret_line_fix(project, raw) {
        return Some(fix);
    }
    if let Some(fix) = directory_fix(project, raw) {
        return Some(fix);
    }
    if let Some(fix) = td_extension_fix(project, rel, raw) {
        return Some(fix);
    }
    let (kind, content) = marker_fix(project, rel, raw)?;
    Some((kind, format!("[src: file: {content}]")))
}

/// `path:12: never edit generated files` — a marker holds only `path:line`
/// (docroms step 3). The reference stays in the marker, the comment follows it
/// in parentheses. A reference the gate refuses as written, and a comment
/// holding a bracket, a parenthesis or a backtick, are left alone.
fn prose_fix(project: &Path, rel: &str, raw: &str) -> Option<(&'static str, String)> {
    let (token, comment) = raw.split_once(char::is_whitespace)?;
    let reference = token.trim_end_matches([':', ';', '—', '–']);
    let comment = comment
        .trim()
        .trim_start_matches(['—', '–', '-', ':'])
        .trim();
    let (_, line_part) = reference.rsplit_once(':')?;
    if comment.is_empty()
        || comment.contains(['`', '[', ']', '(', ')'])
        || crate::core::anti_halluc::parse_line_bounds(line_part).is_none()
    {
        return None;
    }
    matches!(
        super::check_citation(project, rel, reference),
        super::CitationCheck::Valid { .. }
    )
    .then(|| {
        (
            "marker_prose",
            format!("[src: file: {reference}] ({comment})"),
        )
    })
}

/// A text file with no line at all cannot be cited: the gate says to name it in
/// backticks outside any marker, which is also all the claim can mean.
fn empty_file_fix(project: &Path, raw: &str) -> Option<(&'static str, String)> {
    if is_project_dir(project, raw) {
        return None;
    }
    let path = cited_file(project, raw)?;
    if crate::core::anti_halluc::is_binary_file(&path)
        || crate::acp::secret_files::is_secret_path(&path)
        || line_count(&path) > 0
        || last_line_of(&path) > 0
    {
        return None;
    }
    Some(("empty_file", format!("`{raw}` (empty)")))
}

/// A line of a secret file (`.ssh/key:1`) can only mean the file is there: the
/// presence form, never a pointer into it.
fn secret_line_fix(project: &Path, raw: &str) -> Option<(&'static str, String)> {
    let (path, line) = raw.rsplit_once(':')?;
    if crate::core::anti_halluc::is_presence_marker(line) {
        return None;
    }
    let file = cited_file(project, path.trim())?;
    crate::acp::secret_files::is_secret_path(&file).then(|| {
        (
            "secret_presence",
            format!("[src: file: {}:exists]", path.trim()),
        )
    })
}

/// A directory cannot be cited (`docs/tech-debt/`): the gate says to name it in
/// backticks without a marker, which is all the claim can mean (africanews
/// step 8). A line given to a directory (`tests:1`, docroms step 5) points at
/// nothing, so it goes too.
fn directory_fix(project: &Path, raw: &str) -> Option<(&'static str, String)> {
    let dir = match raw.rsplit_once(':') {
        Some((dir, _)) if is_project_dir(project, dir.trim()) => dir.trim(),
        _ => raw,
    };
    is_project_dir(project, dir).then(|| ("directory_backticks", format!("`{dir}`")))
}

/// A tech-debt sheet cited without its `.md` (docroms step 8:
/// `docs/tech-debt/TD-20260719-here-apikey-hardcoded:exists`). The one file the
/// citation can mean is `<path>.md`; the form (`:exists`, a line, none) is kept
/// and the result is applied only once the gate accepts it.
fn td_extension_fix(project: &Path, rel: &str, raw: &str) -> Option<(&'static str, String)> {
    let (path, tail) = match raw.rsplit_once(':') {
        Some((path, tail)) => (path.trim(), Some(tail.trim())),
        None => (raw, None),
    };
    if path.ends_with(".md") || cited_file(project, path).is_some() {
        return None;
    }
    let sheet = format!("{path}.md");
    if !is_td_sheet(project, &sheet) || cited_file(project, &sheet).is_none() {
        return None;
    }
    let fixed = match tail {
        Some(tail) => format!("{sheet}:{tail}"),
        None => {
            let (_, content) = marker_fix(project, rel, &sheet)?;
            return Some(("td_sheet_extension", format!("[src: file: {content}]")));
        }
    };
    match super::check_citation(project, rel, &fixed) {
        super::CitationCheck::Valid { .. } | super::CitationCheck::Skip => {
            Some(("td_sheet_extension", format!("[src: file: {fixed}]")))
        }
        super::CitationCheck::Defect(_) => None,
    }
}

/// The marker content the gate would accept in place of `raw` (no comment).
fn marker_fix(project: &Path, rel: &str, raw: &str) -> Option<(&'static str, String)> {
    let Some((path_part, line_part)) = raw.rsplit_once(':') else {
        return bare_fix(project, raw);
    };
    let (path_part, line_part) = (path_part.trim(), line_part.trim());
    let Some(path) = cited_file(project, path_part) else {
        if is_project_dir(project, path_part) || without_anchor(line_part) != line_part {
            return None;
        }
        let spec = crate::core::anti_halluc::parse_line_bounds(line_part)?;
        // The gate names the first candidate; Kronn applies it only when it is
        // the only one.
        let mut candidates =
            did_you_mean_candidates(project, rel, path_part, Some(Some(spec))).into_iter();
        return match (candidates.next(), candidates.next()) {
            (Some(only), None) => Some(("did_you_mean_path", format!("{only}:{line_part}"))),
            _ => None,
        };
    };
    if crate::core::anti_halluc::is_presence_marker(line_part)
        || crate::core::anti_halluc::is_binary_file(&path)
    {
        return None;
    }
    let (start, end) = crate::core::anti_halluc::parse_line_bounds(line_part)?;
    let lines = line_count(&path);
    if crate::core::anti_halluc::range_is_citable(lines, last_line_of(&path), start, end) {
        return None;
    }
    // One line past the end of a file with no closing newline: the count the
    // message states is the line the range was meant to stop at.
    if end == lines + 1 && (1..=lines).contains(&start) {
        let range = if start == lines {
            lines.to_string()
        } else {
            format!("{start}-{lines}")
        };
        return Some(("line_range_clamp", format!("{path_part}:{range}")));
    }
    None
}

/// A text file cited without a line, where the only reading is "it is there": a
/// secret file (no line of it may be cited) and a tech-debt sheet (Kronn's own
/// artefact, a pointer to it).
fn bare_fix(project: &Path, raw: &str) -> Option<(&'static str, String)> {
    if is_project_dir(project, raw) {
        return None;
    }
    let path = cited_file(project, raw)?;
    if crate::core::anti_halluc::is_binary_file(&path) {
        return None;
    }
    if crate::acp::secret_files::is_secret_path(&path) {
        return Some(("secret_presence", format!("{raw}:exists")));
    }
    (is_td_sheet(project, raw) && line_count(&path) > 0)
        .then(|| ("td_sheet_presence", format!("{raw}:exists")))
}

fn is_td_sheet(project: &Path, cited: &str) -> bool {
    cited
        .strip_prefix(under_root(project, "docs/tech-debt/"))
        .and_then(|name| name.strip_suffix(".md"))
        .is_some_and(|id| !id.contains('/') && crate::core::td_index::is_td_id(id))
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "mechanical_fix_tests.rs"]
mod tests;
