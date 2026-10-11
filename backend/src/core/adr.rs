//! Architecture decision records in `docs/decisions.md`. An accepted ADR is a
//! choice, not debt: after the audit steps, Kronn settles every TD sheet an
//! accepted ADR covers (status `Accepted decision`, plus a `Decision record`
//! line naming the ADR), so the finding never reaches the debt index. Only an
//! ADR accepted before the run, or by a human through a validation card, may
//! settle: the audit cannot accept its own way out of a finding.

use std::collections::BTreeSet;
use std::path::Path;

use crate::core::audit_validation::{
    append_history_entry, replace_current_status, td_decision, TdDecision, STATUS_ACCEPTED_DECISION,
};

pub const DECISIONS_FILE: &str = "decisions.md";
/// Sections every ADR carries, in this order.
pub const SECTIONS: [&str; 3] = ["Context", "Decision", "Consequences"];
const RECORD_FIELD: &str = "Decision record";
const REVIEWER: &str = "Kronn ADR check";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdrStatus {
    Proposed,
    Accepted,
    Superseded,
    Unknown,
}

impl AdrStatus {
    fn parse(value: &str) -> Self {
        let v = clean(value).to_lowercase();
        if v.starts_with("accepted") {
            Self::Accepted
        } else if v.starts_with("proposed") {
            Self::Proposed
        } else if v.starts_with("superseded") {
            Self::Superseded
        } else {
            Self::Unknown
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adr {
    /// Canonical id, `ADR-0001`. Empty for a legacy table row.
    pub id: String,
    pub title: String,
    pub date: Option<String>,
    pub status: AdrStatus,
    pub supersedes: Vec<String>,
    pub superseded_by: Option<String>,
    /// TD slugs this decision explains: the TD id without `TD-<date>-`, so a
    /// finding re-discovered on another day still matches.
    pub covers: Vec<String>,
    /// Required sections (lowercase) that have content.
    pub sections: Vec<String>,
    /// A row of the table format earlier files used; it covers the TD sheets
    /// it links to.
    pub legacy: bool,
}

impl Adr {
    /// How a TD sheet names this decision.
    pub fn reference(&self) -> String {
        if self.legacy {
            format!("decisions.md row \"{}\"", self.title)
        } else {
            format!("{} — {}", self.id, self.title)
        }
    }
}

/// Canonical `ADR-NNNN` for the first ADR id in `text`.
fn find_adr_id(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(at) = rest.find("ADR-") {
        let digits: String = rest[at + 4..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(n) = digits.parse::<u32>() {
            return Some(format!("ADR-{n:04}"));
        }
        rest = &rest[at + 4..];
    }
    None
}

fn all_adr_ids(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("ADR-") {
        if let Some(id) = find_adr_id(&rest[at..]) {
            out.push(id);
        }
        rest = &rest[at + 4..];
    }
    out
}

/// `ADR-0001: Title`, `ADR-0001 — Title` or `ADR-0001 - Title`.
fn adr_heading(text: &str) -> Option<(String, String)> {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let rest = t.strip_prefix("ADR-")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let n: u32 = digits.parse().ok()?;
    let title = rest[digits.len()..]
        .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '—' | '–' | '-' | '.'))
        .trim();
    Some((format!("ADR-{n:04}"), title.to_string()))
}

/// The slug a TD id or a `Covers` entry matches on.
pub fn td_slug(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    let rest = lower.strip_prefix("td-").unwrap_or(&lower);
    let bytes = rest.as_bytes();
    if bytes.len() > 9 && bytes[..8].iter().all(u8::is_ascii_digit) && bytes[8] == b'-' {
        rest[9..].to_string()
    } else {
        rest.to_string()
    }
}

fn clean(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| matches!(c, '*' | '`' | '"' | '\''))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    let level = t.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    rest.starts_with(' ').then(|| (level, rest.trim()))
}

/// Per line: whether it is outside fenced code and HTML comments. Examples
/// and guidance never count as recorded decisions.
fn live_mask(content: &str) -> Vec<bool> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut in_comment = false;
    for line in content.lines() {
        let t = line.trim();
        if in_comment {
            in_comment = !t.contains("-->");
            out.push(false);
        } else if t.starts_with("```") {
            in_fence = !in_fence;
            out.push(false);
        } else if in_fence {
            out.push(false);
        } else if t.starts_with("<!--") {
            in_comment = !t.contains("-->");
            out.push(false);
        } else {
            out.push(true);
        }
    }
    out
}

fn live_lines(content: &str) -> Vec<&str> {
    content
        .lines()
        .zip(live_mask(content))
        .filter_map(|(line, live)| live.then_some(line))
        .collect()
}

/// `- **Name**: value` or `**Name:** value`, case-insensitive.
fn field(lines: &[&str], name: &str) -> Option<String> {
    let colon_out = format!("**{name}**:");
    let colon_in = format!("**{name}:**");
    lines.iter().find_map(|line| {
        let t = line.trim_start();
        let t = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .unwrap_or(t);
        let starts = |p: &str| t.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p));
        let rest = if starts(&colon_out) {
            &t[colon_out.len()..]
        } else if starts(&colon_in) {
            &t[colon_in.len()..]
        } else {
            return None;
        };
        Some(rest.trim().to_string()).filter(|v| !v.is_empty())
    })
}

fn covers(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .filter_map(|token| {
            let token = token.trim();
            // `[TD-x](tech-debt/TD-x.md)` names the TD by its link text.
            let token = token.split("](").next().unwrap_or(token);
            let token = token.trim_matches(|c: char| matches!(c, '[' | ']' | '`' | '*' | ' '));
            let slug = td_slug(token);
            (!slug.is_empty()
                && slug
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
            .then_some(slug)
        })
        .collect()
}

fn parse_block(id: String, title: String, level: usize, block: &[&str]) -> Adr {
    let status_raw = field(block, "Status").unwrap_or_default();
    let mut superseded_by = field(block, "Superseded by").and_then(|v| find_adr_id(&v));
    let status = AdrStatus::parse(&status_raw);
    if superseded_by.is_none() && status == AdrStatus::Superseded {
        superseded_by = find_adr_id(&status_raw);
    }
    let mut sections = BTreeSet::new();
    for (i, line) in block.iter().enumerate() {
        let Some((sub_level, text)) = heading(line) else {
            continue;
        };
        let name = text.trim_end_matches(':').trim().to_lowercase();
        if sub_level <= level || !SECTIONS.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
            continue;
        }
        let has_content = block[i + 1..]
            .iter()
            .take_while(|l| heading(l).is_none())
            .any(|l| !l.trim().is_empty());
        if has_content {
            sections.insert(name);
        }
    }
    for name in SECTIONS {
        if field(block, name).is_some() {
            sections.insert(name.to_lowercase());
        }
    }
    Adr {
        id,
        title,
        date: field(block, "Date").map(|d| clean(&d)),
        status,
        supersedes: field(block, "Supersedes")
            .map(|v| all_adr_ids(&v))
            .unwrap_or_default(),
        superseded_by,
        covers: field(block, "Covers")
            .map(|v| covers(&v))
            .unwrap_or_default(),
        sections: sections.into_iter().collect(),
        legacy: false,
    }
}

/// A table row of the earlier format that links TD sheets, as the audit
/// validation card wrote them: an accepted trade-off covering those TDs.
fn legacy_row(line: &str) -> Option<Adr> {
    let t = line.trim();
    if !t.starts_with('|') || !t.contains("tech-debt/TD-") {
        return None;
    }
    let mut ids = Vec::new();
    let mut rest = t;
    while let Some(at) = rest.find("tech-debt/TD-") {
        let id: String = rest[at + "tech-debt/".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        ids.push(td_slug(&id));
        rest = &rest[at + "tech-debt/".len()..];
    }
    let title = t.split('|').nth(1).map(clean).unwrap_or_default();
    Some(Adr {
        id: String::new(),
        title,
        date: None,
        status: AdrStatus::Accepted,
        supersedes: Vec::new(),
        superseded_by: None,
        covers: ids,
        sections: Vec::new(),
        legacy: true,
    })
}

/// Every ADR of a `decisions.md`, in file order.
pub fn parse(content: &str) -> Vec<Adr> {
    let live = live_lines(content);
    let mut adrs = Vec::new();
    let mut i = 0;
    while i < live.len() {
        if let Some((level, text)) = heading(live[i]) {
            if let Some((id, title)) = adr_heading(text) {
                let end = (i + 1..live.len())
                    .find(|&j| heading(live[j]).is_some_and(|(l, _)| l <= level))
                    .unwrap_or(live.len());
                adrs.push(parse_block(id, title, level, &live[i + 1..end]));
                i = end;
                continue;
            }
        }
        if let Some(row) = legacy_row(live[i]) {
            adrs.push(row);
        }
        i += 1;
    }
    adrs
}

/// Decisions in force: accepted, and replaced by no accepted ADR.
pub fn in_force(adrs: &[Adr]) -> Vec<&Adr> {
    let replaced: BTreeSet<&str> = adrs
        .iter()
        .filter(|a| a.status == AdrStatus::Accepted)
        .flat_map(|a| a.supersedes.iter().map(String::as_str))
        .collect();
    adrs.iter()
        .filter(|a| {
            a.status == AdrStatus::Accepted
                && a.superseded_by.is_none()
                && !replaced.contains(a.id.as_str())
        })
        .collect()
}

fn is_iso_date(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// What makes each ADR of `content` incomplete, one line per problem.
pub fn problems(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for adr in parse(content).iter().filter(|a| !a.legacy) {
        if !seen.insert(adr.id.clone()) {
            out.push(format!("{}: duplicate id", adr.id));
        }
        if adr.title.is_empty() {
            out.push(format!("{}: no title", adr.id));
        }
        if !adr.date.as_deref().is_some_and(is_iso_date) {
            out.push(format!("{}: date is not YYYY-MM-DD", adr.id));
        }
        if adr.status == AdrStatus::Unknown {
            out.push(format!(
                "{}: status is not proposed, accepted or superseded",
                adr.id
            ));
        }
        for name in SECTIONS {
            if !adr.sections.iter().any(|s| s == &name.to_lowercase()) {
                out.push(format!("{}: no {name} section", adr.id));
            }
        }
    }
    out
}

/// Body lines of a TD sheet outside its front matter and code fences.
fn sheet_body(content: &str) -> Vec<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let start = if lines.first().is_some_and(|l| l.trim() == "---") {
        (1..lines.len())
            .find(|&i| lines[i].trim() == "---")
            .map_or(0, |end| end + 1)
    } else {
        0
    };
    let mut in_fence = false;
    lines[start..]
        .iter()
        .filter(|l| {
            if l.trim_start().starts_with("```") {
                in_fence = !in_fence;
                return false;
            }
            !in_fence
        })
        .copied()
        .collect()
}

/// The ADR a TD sheet names in its `Decision record` line.
pub fn decision_record(content: &str) -> Option<String> {
    field(&sheet_body(content), RECORD_FIELD).and_then(|v| find_adr_id(&v))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Coverage<'a> {
    /// An ADR in force covers the finding: it is a decision, not debt.
    Covered(&'a Adr),
    /// The sheet names an ADR that is not in force: it stays debt.
    NotInForce(String),
    None,
}

/// Whether one of the `trusted` decisions covers the TD sheet `td_id`: the
/// sheet names it in `Decision record`, or the ADR lists the TD's slug in
/// `Covers`. Callers pass only the ADRs allowed to settle (see [`Trust`]).
pub fn coverage<'a>(td_id: &str, content: &str, trusted: &'a [Adr]) -> Coverage<'a> {
    let force = trusted;
    let named = decision_record(content);
    if let Some(adr) = named
        .as_deref()
        .and_then(|id| force.iter().find(|a| !a.legacy && a.id == id))
    {
        return Coverage::Covered(adr);
    }
    let slug = td_slug(td_id);
    if let Some(adr) = force.iter().find(|a| a.covers.contains(&slug)) {
        return Coverage::Covered(adr);
    }
    named.map_or(Coverage::None, Coverage::NotInForce)
}

fn record_line(adr: &Adr) -> String {
    format!(
        "- **{RECORD_FIELD}**: {} ([decisions](../{DECISIONS_FILE}))",
        adr.reference()
    )
}

fn is_record_line(line: &str) -> bool {
    field(&[line], RECORD_FIELD).is_some()
}

/// The sheet settled by `adr`: status `Accepted decision`, a `Decision record`
/// line naming the ADR, and an `audit_history` entry when there is one.
pub fn settle_sheet(content: &str, adr: &Adr) -> String {
    let trailing_newline = content.ends_with('\n');
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let body_start = if lines.first().is_some_and(|l| l.trim() == "---") {
        (1..lines.len())
            .find(|&i| lines[i].trim() == "---")
            .map_or(0, |end| end + 1)
    } else {
        0
    };
    let record = record_line(adr);
    let status_line = format!("- **Status**: {STATUS_ACCEPTED_DECISION}");
    if replace_current_status(&mut lines, STATUS_ACCEPTED_DECISION).is_err() {
        let at = (body_start..lines.len())
            .find(|&i| lines[i].trim_start().starts_with("# "))
            .map_or(body_start, |h| h + 1);
        lines.insert(at, status_line);
    }
    if let Some(existing) = (body_start..lines.len()).find(|&i| is_record_line(&lines[i])) {
        lines[existing] = record;
    } else {
        let status_at = (body_start..lines.len())
            .find(|&i| {
                let l = lines[i].trim_start().to_ascii_lowercase();
                l.starts_with("- **status**:") || l.starts_with("**status**:")
            })
            .map_or(body_start, |i| i + 1);
        lines.insert(status_at, record);
    }
    // A sheet without front matter has no history to extend; its status line
    // already carries the decision.
    let _ = append_history_entry(
        &mut lines,
        STATUS_ACCEPTED_DECISION,
        REVIEWER,
        &format!(
            "Covered by {} in docs/{DECISIONS_FILE}: a decision, not debt.",
            adr.reference()
        ),
    );
    let mut out = lines.join("\n");
    if trailing_newline {
        out.push('\n');
    }
    out
}

/// Which ADRs may settle a TD sheet. An agent must never settle its own
/// finding by writing an accepted ADR during the run.
#[derive(Debug, Clone, Copy)]
pub enum Trust<'a> {
    /// An audit run: only the decisions already in force in `decisions.md`
    /// when the run started (this text), in that version. ADRs accepted since
    /// are downgraded to `proposed` and left to a human.
    RunBaseline(&'a str),
    /// A validation card: only the ADRs the card itself just appended.
    CardAccepted(&'a [String]),
}

/// The ADRs allowed to settle under `trust`, and the ids of ADRs accepted
/// during the run that must go back to `proposed`.
pub fn trusted(current: &[Adr], trust: Trust) -> (Vec<Adr>, Vec<String>) {
    let now = in_force(current);
    match trust {
        Trust::RunBaseline(baseline) => {
            let base = parse(baseline);
            let before = in_force(&base);
            let kept = before
                .iter()
                .filter(|b| {
                    now.iter().any(|a| {
                        if b.legacy {
                            a.legacy && a.covers == b.covers
                        } else {
                            !a.legacy && a.id == b.id
                        }
                    })
                })
                .map(|b| (*b).clone())
                .collect();
            let late = now
                .iter()
                .filter(|a| !a.legacy && !before.iter().any(|b| !b.legacy && b.id == a.id))
                .map(|a| a.id.clone())
                .collect();
            (kept, late)
        }
        Trust::CardAccepted(ids) => (
            now.iter()
                .filter(|a| !a.legacy && ids.contains(&a.id))
                .map(|a| (*a).clone())
                .collect(),
            Vec::new(),
        ),
    }
}

/// `content` with the `Status` of every ADR in `ids` set back to `proposed`.
pub fn downgrade(content: &str, ids: &[String], date: &str) -> String {
    let mask = live_mask(content);
    let mut out = Vec::new();
    let mut current: Option<(usize, String)> = None;
    let mut done: BTreeSet<String> = BTreeSet::new();
    for (line, live) in content.lines().zip(mask) {
        if live {
            if let Some((level, text)) = heading(line) {
                if current.as_ref().is_some_and(|(l, _)| level <= *l) {
                    current = None;
                }
                if let Some((id, _)) = adr_heading(text) {
                    current = ids.contains(&id).then_some((level, id));
                }
            } else if let Some((_, id)) = &current {
                if !done.contains(id) && field(&[line], "Status").is_some() {
                    out.push(format!(
                        "- **Status**: proposed (written as accepted during the audit of {date}; only a human accepts an ADR)"
                    ));
                    done.insert(id.clone());
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    let mut text = out.join("\n");
    if content.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// The text of `<docs>/decisions.md`, empty when absent or not a regular file.
pub fn read_decisions(docs_dir: &Path) -> String {
    let path = docs_dir.join(DECISIONS_FILE);
    if regular_file(&path) {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AdrSettleOutcome {
    /// `TD-x → ADR-y — title`, one per sheet settled by this pass.
    pub settled: Vec<String>,
    /// `TD-x → ADR-y`: the sheet names an ADR that is not in force.
    pub not_in_force: Vec<String>,
    /// Incomplete ADRs (see [`problems`]).
    pub problems: Vec<String>,
    /// ADRs accepted during the run, set back to `proposed` for a human.
    pub downgraded: Vec<String>,
}

fn regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

/// Settles every `<docs>/tech-debt/TD-*.md` sheet an ADR in force covers.
/// Rejected sheets stay as they are; a sheet already settled by the same ADR
/// is not rewritten.
pub fn settle_covered_sheets(docs_dir: &Path, trust: Trust) -> Result<AdrSettleOutcome, String> {
    let decisions_path = docs_dir.join(DECISIONS_FILE);
    let decisions = if regular_file(&decisions_path) {
        std::fs::read_to_string(&decisions_path)
            .map_err(|e| format!("read {}: {e}", decisions_path.display()))?
    } else {
        String::new()
    };
    let current = parse(&decisions);
    let (adrs, late) = trusted(&current, trust);
    let mut outcome = AdrSettleOutcome {
        problems: problems(&decisions),
        ..AdrSettleOutcome::default()
    };
    if !late.is_empty() {
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let lowered = downgrade(&decisions, &late, &date);
        crate::core::mcp_scanner::atomic_write(&decisions_path, &lowered)?;
        outcome.downgraded = current
            .iter()
            .filter(|a| late.contains(&a.id))
            .map(Adr::reference)
            .collect();
    }
    let Ok(entries) = std::fs::read_dir(docs_dir.join("tech-debt")) else {
        return Ok(outcome);
    };
    let mut sheets: Vec<(String, std::path::PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let id = name.strip_suffix(".md")?.to_string();
            crate::core::td_index::is_td_id(&id).then(|| (id, e.path()))
        })
        .collect();
    sheets.sort();
    for (id, path) in sheets {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        match coverage(&id, &content, &adrs) {
            Coverage::Covered(adr) => {
                let decision = td_decision(&content);
                if decision == Some(TdDecision::Rejected) {
                    continue;
                }
                let record = record_line(adr);
                let already = decision == Some(TdDecision::AcceptedDecision)
                    && sheet_body(&content).iter().any(|l| l.trim() == record);
                if already {
                    continue;
                }
                let settled = settle_sheet(&content, adr);
                crate::core::mcp_scanner::atomic_write(&path, &settled)?;
                outcome.settled.push(format!("{id} → {}", adr.reference()));
            }
            Coverage::NotInForce(adr_id) => outcome.not_in_force.push(format!("{id} → {adr_id}")),
            Coverage::None => {}
        }
    }
    Ok(outcome)
}

/// The decisions that may settle a TD in this run (in force since before it
/// started), for the prompt of a step that writes TDs. Empty when there is none.
pub fn in_force_prompt_block(docs_dir: &Path, baseline: &str) -> String {
    let current = parse(&read_decisions(docs_dir));
    let (force, _) = trusted(&current, Trust::RunBaseline(baseline));
    if force.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## Decisions in force (docs/decisions.md) — not debt\n\
These ADRs were accepted before this run. A finding one of them explains is a recorded choice: do NOT write a TD for it. \
An existing sheet it explains gets `- **Decision record**: <ADR id>` under its Status line instead. \
Evidence that a decision now causes harm goes in the index `## Coverage gaps` section, naming the ADR, so the validation asks whether to supersede it. \
No other ADR covers anything in this run: one accepted during the run goes back to `proposed` for a human. \
After the steps, Kronn settles every TD sheet these ADRs cover (by `Decision record`, or by a TD slug in `Covers`) as `Accepted decision`.\n",
    );
    for adr in &force {
        let covers = if adr.covers.is_empty() {
            String::new()
        } else {
            format!(" (covers: {})", adr.covers.join(", "))
        };
        out.push_str(&format!("- {}{covers}\n", adr.reference()));
    }
    out
}

#[cfg(test)]
#[path = "adr_test.rs"]
mod tests;
