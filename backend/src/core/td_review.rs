//! KT-982 — the human review of the audit's tech-debt sheets: what the list
//! shows for each sheet, the counts, and the rule that lets a project be
//! validated once every Critical and High sheet is settled.

use std::collections::BTreeSet;
use std::path::Path;

use crate::core::audit_validation::TdDecision;
use crate::core::td_index::{counted_sheet_stem, docs_index_rows, is_sheet_id, is_td_id, IndexRow};
use crate::core::td_tasks::{body_lines, field_block, source_from, truncate};
use crate::models::{
    TdPointer, TdReviewItem, TdReviewState, TdSeverityCount, TdStateCounts, TdValidationGate,
};

pub const SEVERITIES: [&str; 4] = ["Critical", "High", "Medium", "Low"];
const IMPACT_MAX: usize = 600;
const SHORT_FIELD_MAX: usize = 80;
const MAX_POINTERS: usize = 12;
/// Title prefix of the discussion dedicated to one sheet: `TD · <id> — …`.
pub const TALK_TITLE_PREFIX: &str = "TD · ";

/// `Critical`, `High`, `Medium`, `Low` or `Unknown`, from a free-form value.
pub fn severity_bucket(raw: &str) -> &'static str {
    let lower = raw.trim().to_lowercase();
    SEVERITIES
        .iter()
        .find(|s| lower.starts_with(&s.to_lowercase()))
        .copied()
        .unwrap_or("Unknown")
}

pub fn state_of(decision: Option<TdDecision>) -> TdReviewState {
    match decision {
        None => TdReviewState::Pending,
        Some(TdDecision::Confirmed) => TdReviewState::Confirmed,
        Some(TdDecision::AcceptedDecision) => TdReviewState::AcceptedDecision,
        Some(TdDecision::Deferred) => TdReviewState::Deferred,
        Some(TdDecision::Rejected) => TdReviewState::Rejected,
    }
}

/// A deferral postpones the decision: the sheet still has to be settled.
pub fn is_settled(state: TdReviewState) -> bool {
    !matches!(state, TdReviewState::Pending | TdReviewState::Deferred)
}

/// Every sheet the project card counts, directly under `<docs>/tech-debt/`,
/// sorted by id, without the task and talk links (the caller adds them from
/// the DB). That includes the `CQ-*` / `PERF-*` detail files of the dimension
/// audits, which may carry no front matter: their severity and status come
/// from the index row when the file does not say, else they stay `Unknown`.
pub fn read_items(docs_dir: &Path) -> Vec<TdReviewItem> {
    let docs_name = docs_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("docs")
        .to_string();
    read_items_rel(docs_dir, &docs_name)
}

/// `read_items` with the sheets' paths under `docs_rel`, the docs root as the
/// repository names it (`packages/x/docs`, KT-1192).
pub fn read_items_rel(docs_dir: &Path, docs_name: &str) -> Vec<TdReviewItem> {
    let Ok(entries) = std::fs::read_dir(docs_dir.join("tech-debt")) else {
        return Vec::new();
    };
    let rows = docs_index_rows(docs_dir);
    let mut items: Vec<TdReviewItem> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let id = counted_sheet_stem(&name)?;
            let content = std::fs::read_to_string(e.path()).ok()?;
            let mut item = item_from(id, &content, format!("{docs_name}/tech-debt/{name}"));
            if let Some(row) = rows.get(id) {
                fill_from_index(&mut item, row);
            }
            Some(item)
        })
        .collect();
    items.sort_by(|a, b| a.id.cmp(&b.id));
    items
}

/// What the sheet does not say, its index row does.
fn fill_from_index(item: &mut TdReviewItem, row: &IndexRow) {
    let unknown = |v: &str| v.trim().is_empty() || v.eq_ignore_ascii_case("unknown");
    if item.title == item.id {
        if let Some(title) = &row.title {
            item.title = truncate(title, 160);
        }
    }
    if unknown(&item.area) {
        if let Some(area) = &row.area {
            item.area = area.clone();
        }
    }
    if item.severity == "Unknown" {
        if let Some(severity) = &row.severity {
            item.severity = severity_bucket(severity).to_string();
        }
    }
    if unknown(&item.status) {
        if let Some(status) = &row.status {
            item.status = status.clone();
        }
    }
}

pub fn item_from(id: &str, content: &str, source_path: String) -> TdReviewItem {
    let source = source_from(id, content, source_path);
    let body = body_lines(content);
    let short = |names: &[&str]| {
        field_block(&body, names)
            .filter(|v| !v.trim().is_empty())
            .map(|v| truncate(&v, SHORT_FIELD_MAX))
    };
    TdReviewItem {
        id: source.sheet.id.clone(),
        title: source.sheet.title.clone(),
        area: source.sheet.area.clone(),
        severity: severity_bucket(&source.sheet.severity).to_string(),
        status: source.sheet.status.clone(),
        effort: short(&["effort"]),
        blast_radius: short(&["blast radius"]),
        state: state_of(source.sheet.decision),
        problem: source.problem,
        impact: field_markdown(&body, &["impact"]).map(|v| truncate(&v, IMPACT_MAX)),
        verification: source.verification,
        pointers: pointers(&body),
        source_path: source.source_path,
        task: None,
        talk_count: 0,
        talk_discussion_id: None,
    }
}

/// Lines with their common indentation removed, blank runs collapsed and the
/// blank edges dropped; `None` when nothing is left.
fn dedent(lines: &[&str]) -> Option<String> {
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out: Vec<&str> = Vec::new();
    for line in lines {
        let text = line.get(indent..).unwrap_or("").trim_end();
        if text.is_empty() && out.last().is_none_or(|last| last.is_empty()) {
            continue;
        }
        out.push(text);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

/// Like `field_block`, but keeps the line structure: the text is Markdown
/// (`**Critical**`, `- ` bullets) and the review renders it as such.
fn field_markdown(body: &[&str], names: &[&str]) -> Option<String> {
    let starts_with_name = |text: &str| {
        let lower = text.to_lowercase();
        names.iter().any(|name| lower.starts_with(name))
    };
    for (idx, line) in body.iter().enumerate() {
        let t = line.trim();
        if let Some(heading) = t.strip_prefix("## ") {
            if starts_with_name(heading.trim()) {
                let section: Vec<&str> = body[idx + 1..]
                    .iter()
                    .copied()
                    .take_while(|l| !l.trim().starts_with('#'))
                    .filter(|l| !l.trim().starts_with("<!--"))
                    .collect();
                return dedent(&section);
            }
            continue;
        }
        let item = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .unwrap_or(t);
        let Some(label) = item.strip_prefix("**") else {
            continue;
        };
        if !starts_with_name(label) {
            continue;
        }
        let Some(close) = label.find("**") else {
            continue;
        };
        let rest = label[close + 2..].trim_start();
        let first = rest.strip_prefix(':').unwrap_or(rest).trim();
        let tail: Vec<&str> = body[idx + 1..]
            .iter()
            .copied()
            .take_while(|n| {
                !n.trim().is_empty() && !n.trim().starts_with('#') && n.starts_with(' ')
            })
            .collect();
        let tail = dedent(&tail);
        return match (first.is_empty(), tail) {
            (true, tail) => tail,
            (false, None) => Some(first.to_string()),
            (false, Some(tail)) => Some(format!("{first}\n{tail}")),
        };
    }
    None
}

/// `file[:line]` references of the `## Where` section: code spans first, bare
/// tokens when a line has none.
pub fn pointers(body: &[&str]) -> Vec<TdPointer> {
    let Some(start) = body.iter().position(|l| {
        l.trim()
            .strip_prefix("## ")
            .is_some_and(|h| h.trim().to_lowercase().starts_with("where"))
    }) else {
        return Vec::new();
    };
    let mut out: Vec<TdPointer> = Vec::new();
    for line in body[start + 1..]
        .iter()
        .take_while(|l| !l.trim_start().starts_with('#'))
    {
        let spans: Vec<&str> = line.split('`').skip(1).step_by(2).collect();
        let candidates: Vec<&str> = if spans.is_empty() {
            line.split_whitespace().collect()
        } else {
            spans
        };
        for candidate in candidates {
            if let Some(pointer) = parse_pointer(candidate) {
                if !out.contains(&pointer) {
                    out.push(pointer);
                }
            }
            if out.len() >= MAX_POINTERS {
                return out;
            }
        }
    }
    out
}

fn parse_pointer(raw: &str) -> Option<TdPointer> {
    let token = raw
        .split_whitespace()
        .next()?
        .trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';' | '[' | ']' | '*' | '"'))
        .trim_end_matches('.');
    if token.contains("://") || token.starts_with('-') {
        return None;
    }
    let (path, line) = match token.split_once(['#', ':']) {
        Some((path, rest)) => {
            let digits: String = rest
                .trim_start_matches('L')
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            (path, digits.parse::<u32>().ok().filter(|n| *n > 0))
        }
        None => (token, None),
    };
    let path = path.trim_start_matches("./");
    let looks_like_path = (path.contains('/') || path.contains('.'))
        && !path.starts_with('/')
        && !path.split('/').any(|part| part == "..")
        && path.len() <= 300
        && path
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '@' | '+'));
    let has_name = path
        .rsplit('/')
        .next()
        .is_some_and(|name| name.chars().any(char::is_alphanumeric));
    (looks_like_path && has_name).then(|| TdPointer {
        path: path.to_string(),
        line,
    })
}

pub fn counts(items: &[TdReviewItem]) -> (TdStateCounts, Vec<TdSeverityCount>) {
    let mut counts = TdStateCounts::default();
    for item in items {
        match item.state {
            TdReviewState::Pending => counts.pending += 1,
            TdReviewState::Confirmed => counts.confirmed += 1,
            TdReviewState::AcceptedDecision => counts.accepted_decision += 1,
            TdReviewState::Deferred => counts.deferred += 1,
            TdReviewState::Rejected => counts.rejected += 1,
        }
    }
    let by_severity = SEVERITIES
        .iter()
        .chain(std::iter::once(&"Unknown"))
        .filter_map(|severity| {
            let group: Vec<&TdReviewItem> =
                items.iter().filter(|i| i.severity == *severity).collect();
            (!group.is_empty() || *severity != "Unknown").then(|| TdSeverityCount {
                severity: severity.to_string(),
                total: group.len() as u32,
                remaining: group.iter().filter(|i| !is_settled(i.state)).count() as u32,
            })
        })
        .collect();
    (counts, by_severity)
}

/// Met when the audit left at least one sheet and every Critical and High
/// sheet is confirmed, rejected or kept on purpose.
pub fn gate(items: &[TdReviewItem]) -> TdValidationGate {
    let blocking: Vec<&TdReviewItem> = items
        .iter()
        .filter(|i| matches!(i.severity.as_str(), "Critical" | "High"))
        .collect();
    let settled = blocking.iter().filter(|i| is_settled(i.state)).count() as u32;
    TdValidationGate {
        blocking_total: blocking.len() as u32,
        blocking_settled: settled,
        met: !items.is_empty() && settled as usize == blocking.len(),
        open_disputes: 0,
    }
}

/// Reads the sheets where the audit writes them: the project's docs root.
pub fn validation_items(project_path: &Path) -> Vec<TdReviewItem> {
    let docs_root = crate::core::docs_location::DocsRoot::of(project_path);
    read_items_rel(&docs_root.dir(project_path), docs_root.rel())
}

/// The pending dispute cards whose sheet a human has not settled another
/// way, one per sheet, oldest first.
pub fn open_disputes<'a>(
    items: &[TdReviewItem],
    pending: &'a [crate::db::audit_cross::PendingDispute],
) -> Vec<&'a crate::db::audit_cross::PendingDispute> {
    let mut seen = BTreeSet::new();
    pending
        .iter()
        .filter(|d| {
            items
                .iter()
                .any(|i| i.id == d.td_id && !is_settled(i.state))
        })
        .filter(|d| seen.insert(d.td_id.as_str()))
        .collect()
}

/// The gate once open disputes are counted: any one of them blocks it.
pub fn with_disputes(mut gate: TdValidationGate, open: usize) -> TdValidationGate {
    gate.open_disputes = open as u32;
    gate.met = gate.met && open == 0;
    gate
}

/// TD ids a text names, as whole tokens.
pub fn mentioned_td_ids(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(at) = rest.find("TD-") {
        let before_ok = rest[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-'));
        let token: String = rest[at..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let token = token.trim_end_matches('-');
        if before_ok && is_td_id(token) {
            out.insert(token.to_string());
        }
        rest = &rest[at + 3..];
    }
    out
}

/// The lines around each mention of `td_id`, joined; `excerpt` is false when
/// that is the whole message.
pub fn mention_excerpt(content: &str, td_id: &str, max: usize) -> (String, bool) {
    let lines: Vec<&str> = content.lines().collect();
    let mut keep = vec![false; lines.len()];
    for (index, line) in lines.iter().enumerate() {
        if mentioned_td_ids(line).contains(td_id) {
            let from = index.saturating_sub(1);
            let to = (index + 1).min(lines.len() - 1);
            for flag in &mut keep[from..=to] {
                *flag = true;
            }
        }
    }
    if keep.iter().all(|k| *k) && content.chars().count() <= max {
        return (content.to_string(), false);
    }
    let mut parts: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for (line, kept) in lines.iter().zip(&keep) {
        if *kept {
            current.push(line);
        } else if !current.is_empty() {
            parts.push(current.join("\n"));
            current.clear();
        }
    }
    if !current.is_empty() {
        parts.push(current.join("\n"));
    }
    let joined = if parts.is_empty() {
        content.to_string()
    } else {
        parts.join("\n…\n")
    };
    (truncate(&joined, max), true)
}

/// The sheet id a dedicated discussion title names, if it is one.
pub fn talk_td_id(title: &str) -> Option<&str> {
    let rest = title.strip_prefix(TALK_TITLE_PREFIX)?;
    let id = rest.split_whitespace().next()?;
    is_sheet_id(id).then_some(id)
}

pub fn talk_title(item: &TdReviewItem) -> String {
    let title = truncate(&item.title, 90);
    format!("{TALK_TITLE_PREFIX}{} — {title}", item.id)
}

/// First turn of a sheet's discussion: the sheet in short, then the user's
/// message.
pub fn talk_prompt(item: &TdReviewItem, message: &str) -> String {
    let mut out = format!(
        "Tech-debt sheet `{}` ({}, {}): {}\nSheet: `{}` (source of truth).\n",
        item.id, item.severity, item.area, item.title, item.source_path
    );
    if let Some(problem) = &item.problem {
        out.push_str(&format!("Problem: {problem}\n"));
    }
    if !item.pointers.is_empty() {
        let where_list: Vec<String> = item
            .pointers
            .iter()
            .map(|p| match p.line {
                Some(line) => format!("{}:{line}", p.path),
                None => p.path.clone(),
            })
            .collect();
        out.push_str(&format!("Where: {}\n", where_list.join(", ")));
    }
    if let Some(verification) = &item.verification {
        out.push_str(&format!("Verification: {verification}\n"));
    }
    out.push_str(
        "\nThe user is reviewing this sheet before deciding whether it is real debt, \
         an intentional choice, something to defer, or a false positive. Answer about \
         this sheet only, citing the code you read.\n\n---\n\n",
    );
    out.push_str(message.trim());
    out
}

#[cfg(test)]
#[path = "td_review_test.rs"]
mod tests;
