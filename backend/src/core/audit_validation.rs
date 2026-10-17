use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TdResolutionAction {
    Confirm,
    Reject,
    AcceptDecision,
    Defer,
}

impl TdResolutionAction {
    pub(crate) fn from_option_id(value: &str) -> Option<Self> {
        match value {
            "confirm" => Some(Self::Confirm),
            "reject" => Some(Self::Reject),
            "accept_decision" => Some(Self::AcceptDecision),
            "defer" => Some(Self::Defer),
            _ => None,
        }
    }

    fn status(self) -> &'static str {
        match self {
            Self::Confirm => STATUS_CONFIRMED,
            Self::Reject => STATUS_REJECTED,
            Self::AcceptDecision => STATUS_ACCEPTED_DECISION,
            Self::Defer => STATUS_DEFERRED,
        }
    }

    fn history_note(self, origin: TdDecisionOrigin) -> &'static str {
        match (self, origin) {
            (Self::Confirm, TdDecisionOrigin::Card) => {
                "Confirmed through an audit validation card."
            }
            (Self::Reject, TdDecisionOrigin::Card) => "Rejected through an audit validation card.",
            (Self::Defer, TdDecisionOrigin::Card) => "Deferred through an audit validation card.",
            (Self::Confirm, TdDecisionOrigin::Review) => {
                "Confirmed from the tech-debt review screen."
            }
            (Self::Reject, TdDecisionOrigin::Review) => {
                "Rejected from the tech-debt review screen."
            }
            (Self::Defer, TdDecisionOrigin::Review) => "Deferred from the tech-debt review screen.",
            (Self::AcceptDecision, TdDecisionOrigin::Card) => {
                "Accepted as an intentional decision and recorded in docs/decisions.md."
            }
            (Self::AcceptDecision, TdDecisionOrigin::Review) => {
                "Accepted as an intentional decision and recorded in the decision log."
            }
        }
    }
}

/// Where a human took a TD decision. Both are human decisions: the ADR an
/// acceptance appends is trusted the same way (KT-1174).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TdDecisionOrigin {
    Card,
    Review,
}

impl TdDecisionOrigin {
    fn reviewer(self) -> &'static str {
        match self {
            Self::Card => "Kronn audit validation card",
            Self::Review => "Kronn tech-debt review",
        }
    }

    fn adr_source(self) -> &'static str {
        match self {
            Self::Card => "user (audit validation card)",
            Self::Review => "user (tech-debt review)",
        }
    }
}

const STATUS_CONFIRMED: &str = "Confirmed by user";
const STATUS_REJECTED: &str = "Rejected";
pub(crate) const STATUS_ACCEPTED_DECISION: &str = "Accepted decision";
const STATUS_DEFERRED: &str = "Deferred";

/// A human decision a validation card wrote on a TD sheet. The sheet is the
/// authority: re-audits and later validations read it back from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TdDecision {
    Confirmed,
    Rejected,
    AcceptedDecision,
    Deferred,
}

impl TdDecision {
    /// Settled TDs are never asked again. A deferred TD is: the user postponed
    /// the decision, they did not take it.
    pub fn is_settled(self) -> bool {
        !matches!(self, Self::Deferred)
    }

    /// Rejected findings and accepted trade-offs are not active debt: they
    /// leave the index and are not re-emitted.
    pub fn leaves_index(self) -> bool {
        matches!(self, Self::Rejected | Self::AcceptedDecision)
    }

    /// The `Status` value a human decision writes on a sheet.
    pub(crate) fn status_label(self) -> &'static str {
        match self {
            Self::Confirmed => STATUS_CONFIRMED,
            Self::Rejected => STATUS_REJECTED,
            Self::AcceptedDecision => STATUS_ACCEPTED_DECISION,
            Self::Deferred => STATUS_DEFERRED,
        }
    }

    fn from_status(value: &str) -> Option<Self> {
        let value = value
            .trim()
            .trim_matches(|c| c == '"' || c == '\'' || c == '`' || c == '*')
            .trim()
            .to_lowercase();
        let starts = |status: &str| value.starts_with(&status.to_lowercase());
        if starts(STATUS_CONFIRMED) || value == "confirmed" {
            Some(Self::Confirmed)
        } else if starts(STATUS_REJECTED) {
            Some(Self::Rejected)
        } else if starts(STATUS_ACCEPTED_DECISION) {
            Some(Self::AcceptedDecision)
        } else if starts(STATUS_DEFERRED) {
            Some(Self::Deferred)
        } else {
            None
        }
    }
}

/// The decision an index row's `Status` cell states, for a sheet that says
/// nothing itself (no file, or a file without a status): the human statuses,
/// and a debt the index marks resolved, which is no longer active.
pub fn index_status_decision(value: &str) -> Option<TdDecision> {
    TdDecision::from_status(value).or_else(|| {
        let lower = value
            .trim()
            .trim_matches(|c| c == '"' || c == '\'' || c == '`' || c == '*')
            .trim()
            .to_lowercase();
        ["resolved", "closed", "fixed", "done"]
            .iter()
            .any(|word| lower == *word || lower.starts_with(&format!("{word} ")))
            .then_some(TdDecision::Rejected)
    })
}

fn status_value(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    ["- **status**:", "**status**:", "status:"]
        .iter()
        .find(|prefix| lower.starts_with(*prefix))
        .map(|prefix| &trimmed[prefix.len()..])
}

/// End of the YAML front matter (index of the closing `---`), if any.
fn frontmatter_end(lines: &[&str]) -> Option<usize> {
    if lines.first().is_none_or(|line| line.trim() != "---") {
        return None;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.trim() == "---").then_some(index))
}

/// The decision recorded on a TD sheet, if any. Reads the body `Status` line
/// (outside code fences) first, then a `status:` key of the front matter, then
/// the last `audit_history` entry. Any other status (Verified in source,
/// Inferred, ...) is no decision.
pub fn td_decision(content: &str) -> Option<TdDecision> {
    let lines: Vec<&str> = content.lines().collect();
    let fm_end = frontmatter_end(&lines);
    let body_start = fm_end.map_or(0, |end| end + 1);
    let mut in_fence = false;
    for line in &lines[body_start..] {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            if let Some(value) = status_value(line) {
                return TdDecision::from_status(value);
            }
        }
    }
    let frontmatter = &lines[1..fm_end?];
    let top_level = frontmatter.iter().find_map(|line| {
        let indent = line.len() - line.trim_start().len();
        (indent <= 2).then(|| status_value(line)).flatten()
    });
    if let Some(value) = top_level {
        return TdDecision::from_status(value);
    }
    frontmatter
        .iter()
        .rev()
        .find_map(|line| status_value(line))
        .and_then(TdDecision::from_status)
}

/// `td_decision` of `<td_dir>/<id>.md`; `None` when the sheet is unreadable.
pub(crate) fn read_td_decision(td_dir: &Path, td_id: &str) -> Option<TdDecision> {
    if !valid_td_id(td_id) {
        return None;
    }
    std::fs::read_to_string(td_dir.join(format!("{td_id}.md")))
        .ok()
        .and_then(|content| td_decision(&content))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TdResolution {
    pub(crate) td_id: String,
    pub(crate) action: TdResolutionAction,
}

/// A sheet the review lists (`TD-*`, and the `CQ-*` / `PERF-*` detail files of
/// the dimension audits): no path separator, no way out of `tech-debt/`.
fn valid_td_id(value: &str) -> bool {
    crate::core::td_index::is_sheet_id(value)
}

fn linked_project_path(conn: &Connection, discussion_id: &str) -> Result<PathBuf> {
    // `Interrupted` too (KT-931): a Full run with failed steps links the
    // validation discussion of its successful steps in the same transaction as
    // its Interrupted status — its TD cards must be answerable like any other.
    let linked: Option<(String, String)> = conn
        .query_row(
            "SELECT p.path, r.id
             FROM discussions d
             JOIN projects p ON p.id = d.project_id
             JOIN audit_runs r ON r.validation_discussion_id = d.id
             WHERE d.id = ?1 AND d.archived = 0 AND r.status IN ('Completed', 'Interrupted')
             ORDER BY r.started_at DESC, r.rowid DESC
             LIMIT 1",
            [discussion_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((path, _)) = linked {
        return Ok(crate::core::scanner::resolve_host_path(&path));
    }

    // Archiving a validation room is recoverable: the project card creates a
    // new validation room, and its first audit card atomically takes over the
    // durable link from the archived room. Restrict this escape hatch to the
    // latest completed run and to the validation title family; an arbitrary
    // project discussion must never gain permission to mutate TD files.
    let replacement: Option<(String, String)> = conn
        .query_row(
            "SELECT p.path, r.id
             FROM discussions d
             JOIN projects p ON p.id = d.project_id
             JOIN audit_runs r ON r.project_id = d.project_id
             JOIN discussions previous ON previous.id = r.validation_discussion_id
             WHERE d.id = ?1
               AND d.archived = 0
               AND d.title LIKE 'Validation audit%'
               AND r.status = 'Completed'
               AND previous.archived = 1
               AND r.rowid = (
                   SELECT latest.rowid FROM audit_runs latest
                   WHERE latest.project_id = d.project_id
                   ORDER BY latest.started_at DESC, latest.rowid DESC
                   LIMIT 1
               )
             LIMIT 1",
            [discussion_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((path, run_id)) = replacement else {
        return Err(anyhow!(
            "Audit TD cards are accepted only in the linked validation discussion"
        ));
    };
    crate::db::audit_runs::set_validation_discussion(conn, &run_id, discussion_id)?;
    Ok(crate::core::scanner::resolve_host_path(&path))
}

fn canonical_child(parent: &Path, child: &Path) -> Result<PathBuf> {
    let parent = parent
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", parent.display()))?;
    let child = child
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", child.display()))?;
    if !child.starts_with(&parent) {
        return Err(anyhow!("{} escapes {}", child.display(), parent.display()));
    }
    Ok(child)
}

pub(crate) fn replace_current_status(lines: &mut [String], status: &str) -> Result<()> {
    let body_start = if lines.first().is_some_and(|line| line.trim() == "---") {
        lines
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(index, line)| (line.trim() == "---").then_some(index + 1))
            .ok_or_else(|| anyhow!("TD detail has unterminated YAML frontmatter"))?
    } else {
        0
    };
    for line in &mut lines[body_start..] {
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("- **status**:")
            || lower.starts_with("**status**:")
            || lower.starts_with("status:")
        {
            let indent_len = line.len() - trimmed.len();
            let colon = trimmed
                .find(':')
                .ok_or_else(|| anyhow!("malformed TD status line"))?;
            *line = format!("{}{} {}", &line[..indent_len], &trimmed[..=colon], status);
            return Ok(());
        }
    }
    // A sheet whose only current status is a front matter key.
    for line in &mut lines[..body_start.saturating_sub(1)] {
        let trimmed = line.trim_start();
        let indent_len = line.len() - trimmed.len();
        if indent_len <= 2 && trimmed.to_ascii_lowercase().starts_with("status:") {
            *line = format!("{}status: {}", &line[..indent_len], status);
            return Ok(());
        }
    }
    Err(anyhow!("TD detail has no current Status field"))
}

/// The reason a person typed, trimmed; `None` when there is none.
fn typed_reason(reason: Option<&str>) -> Option<&str> {
    reason.map(str::trim).filter(|r| !r.is_empty())
}

fn append_audit_history(
    lines: &mut Vec<String>,
    action: TdResolutionAction,
    origin: TdDecisionOrigin,
    reason: Option<&str>,
) -> Result<()> {
    // The typed reason is the "why"; the fixed note only stands in for it.
    let note = typed_reason(reason).unwrap_or_else(|| action.history_note(origin));
    append_history_entry(lines, action.status(), origin.reviewer(), note)
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed == "-" || trimmed.starts_with("- ")
}

/// A quote opens a quoted scalar only at the start of a value: an apostrophe
/// inside a plain scalar (`don't`) is just a character.
fn opens_quote(prev: Option<char>) -> bool {
    prev.is_none_or(|p| p.is_whitespace() || "{[,:".contains(p))
}

/// Where the trailing comment of a line starts: a `#` outside quotes, at the
/// start or after whitespace.
fn comment_start(text: &str) -> Option<usize> {
    let mut quote = None::<char>;
    let mut prev: Option<char> = None;
    let mut chars = text.char_indices();
    while let Some((at, c)) = chars.next() {
        match quote {
            Some('"') if c == '\\' => {
                chars.next();
            }
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if matches!(c, '"' | '\'') && opens_quote(prev) => quote = Some(c),
            None if c == '#' && prev.is_none_or(char::is_whitespace) => return Some(at),
            None => {}
        }
        prev = Some(c);
    }
    None
}

/// The value of a `key:` line, without its trailing comment (`""` for a
/// block key); `None` when the line is another key.
fn key_value(line: &str, key: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix(key)?.strip_prefix(':')?;
    let value = comment_start(rest).map_or(rest, |at| &rest[..at]);
    Some(value.trim().to_string())
}

/// The trailing ` # comment` of a `key:` line, with its leading space; empty
/// when there is none.
fn comment_of(line: &str) -> String {
    comment_start(line).map_or_else(String::new, |at| format!(" {}", &line[at..]))
}

/// Splits the inside of a flow mapping on its top-level commas.
fn split_flow(inner: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let (mut depth, mut quote) = (0usize, None::<char>);
    let mut prev: Option<char> = None;
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('"'), '\\') => {
                current.push(c);
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
                prev = Some('\\');
                continue;
            }
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') if opens_quote(prev) => quote = Some(c),
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => depth = depth.saturating_sub(1),
            (None, ',') if depth == 0 => {
                parts.push(std::mem::take(&mut current));
                prev = Some(c);
                continue;
            }
            _ => {}
        }
        current.push(c);
        prev = Some(c);
    }
    parts.push(current);
    parts
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

/// Appends one `audit_history` entry, unless the same status by the same
/// reviewer with the same note was already recorded today. A new note on the
/// same status is a new entry: the reason a person types is never dropped.
pub(crate) fn append_history_entry(
    lines: &mut Vec<String>,
    status: &str,
    reviewer: &str,
    note: &str,
) -> Result<()> {
    let date = Utc::now().date_naive().format("%Y-%m-%d").to_string();
    let signature = format!("status: {status}");
    let reviewer_line = format!("reviewer: {reviewer}");
    let note = serde_json::to_string(note)?;
    let note_line = format!("note: {note}");
    if lines.windows(4).any(|window| {
        window
            .iter()
            .any(|line| line.trim() == format!("- date: {date}"))
            && window.iter().any(|line| line.trim() == signature)
            && window.iter().any(|line| line.trim() == reviewer_line)
            && window.iter().any(|line| line.trim() == note_line)
    }) {
        return Ok(());
    }

    if lines.first().is_none_or(|line| line.trim() != "---") {
        return Err(anyhow!("TD detail has no YAML frontmatter"));
    }
    let frontmatter_end = lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.trim() == "---").then_some(index))
        .ok_or_else(|| anyhow!("TD detail has unterminated YAML frontmatter"))?;
    // A flow-style empty list is not a block to append to. Its trailing
    // comment stays.
    if let Some(flow) = lines[..frontmatter_end]
        .iter()
        .position(|line| key_value(line, "audit_history") == Some("[]".to_string()))
    {
        let comment = comment_of(&lines[flow]);
        lines[flow] = format!(
            "{}audit_history:{comment}",
            " ".repeat(indent_of(&lines[flow]))
        );
    }
    if lines[..frontmatter_end]
        .iter()
        .any(|line| key_value(line, "audit_history").is_some_and(|value| !value.is_empty()))
    {
        return Err(anyhow!(
            "TD detail has an audit_history that is not a block list"
        ));
    }
    // An entry has the shape of the list it joins: standard YAML parsers reject
    // a sequence whose items sit at two different indents (KT-1309).
    let entry = |item_indent: usize, key_col: usize| {
        vec![
            format!("{}- date: {date}", " ".repeat(item_indent)),
            format!("{}status: {status}", " ".repeat(key_col)),
            format!("{}{reviewer_line}", " ".repeat(key_col)),
            format!("{}note: {note}", " ".repeat(key_col)),
        ]
    };

    if let Some(history_index) = lines[..frontmatter_end]
        .iter()
        .position(|line| key_value(line, "audit_history").is_some())
    {
        let key_indent = indent_of(&lines[history_index]);
        let mut last = history_index;
        let mut shape: Option<(usize, usize)> = None;
        for (index, line) in lines
            .iter()
            .enumerate()
            .take(frontmatter_end)
            .skip(history_index + 1)
        {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let indent = indent_of(line);
            let item = is_list_item(line);
            if indent < key_indent || (indent == key_indent && !item) {
                break;
            }
            last = index;
            if shape.is_none() && item {
                let after_dash = &line.trim_start()[1..];
                let gap = after_dash.len() - after_dash.trim_start().len();
                shape = Some((indent, indent + 1 + gap.max(1)));
            }
        }
        let (item_indent, key_col) = shape.unwrap_or((key_indent + 2, key_indent + 4));
        lines.splice(last + 1..last + 1, entry(item_indent, key_col));
    } else if let Some(metadata_index) = lines[..frontmatter_end]
        .iter()
        .position(|line| key_value(line, "metadata").is_some())
    {
        let metadata_indent = indent_of(&lines[metadata_index]);
        let value = key_value(&lines[metadata_index], "metadata").unwrap_or_default();
        if value.starts_with('{') {
            // A flow mapping is rewritten as a block, its pairs kept as written.
            // An `audit_history` pair it already holds (empty) is the one the
            // entry joins: the key is written once.
            let inner = value
                .strip_prefix('{')
                .and_then(|v| v.strip_suffix('}'))
                .ok_or_else(|| anyhow!("TD detail has a metadata flow mapping on several lines"))?;
            let child_indent = metadata_indent + 2;
            let mut block = vec![format!(
                "{}metadata:{}",
                " ".repeat(metadata_indent),
                comment_of(&lines[metadata_index])
            )];
            for pair in split_flow(inner) {
                let (key, pair_value) = pair.split_once(':').unwrap_or((pair.as_str(), ""));
                if key.trim().trim_matches(['"', '\'']) == "audit_history" {
                    if !matches!(pair_value.trim(), "" | "[]" | "~" | "null") {
                        return Err(anyhow!(
                            "TD detail has an audit_history that is not a block list"
                        ));
                    }
                    continue;
                }
                block.push(format!("{}{pair}", " ".repeat(child_indent)));
            }
            block.push(format!("{}audit_history:", " ".repeat(child_indent)));
            block.extend(entry(child_indent + 2, child_indent + 4));
            lines.splice(metadata_index..=metadata_index, block);
            return Ok(());
        }
        if !value.is_empty() {
            return Err(anyhow!("TD detail has a metadata that is not a mapping"));
        }
        let child_indent = lines
            .iter()
            .take(frontmatter_end)
            .skip(metadata_index + 1)
            .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
            .map(|line| indent_of(line))
            .filter(|indent| *indent > metadata_indent)
            .unwrap_or(metadata_indent + 2);
        let mut block = vec![format!("{}audit_history:", " ".repeat(child_indent))];
        block.extend(entry(child_indent + 2, child_indent + 4));
        lines.splice(metadata_index + 1..metadata_index + 1, block);
    } else {
        let mut block = vec![
            "metadata:".to_string(),
            "  type: tech-debt".to_string(),
            "  audit_history:".to_string(),
        ];
        block.extend(entry(4, 6));
        lines.splice(frontmatter_end..frontmatter_end, block);
    }
    Ok(())
}

/// A sheet without a `Status` field gets one, under its title, so a decision
/// has somewhere to land. `false` when the front matter never closes.
fn insert_status(lines: &mut Vec<String>, status: &str) -> bool {
    let body_start = if lines.first().is_some_and(|line| line.trim() == "---") {
        match lines
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(index, line)| (line.trim() == "---").then_some(index + 1))
        {
            Some(start) => start,
            None => return false,
        }
    } else {
        0
    };
    let field = format!("- **Status**: {status}");
    let mut in_fence = false;
    let heading = lines
        .iter()
        .enumerate()
        .skip(body_start)
        .find_map(|(index, line)| {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
            }
            (!in_fence && line.starts_with("# ")).then_some(index)
        });
    match heading {
        Some(index) => {
            let next = lines.get(index + 1).map(String::as_str).unwrap_or("");
            let block = if next.trim().is_empty() {
                vec![String::new(), field]
            } else if next.trim_start().starts_with("- **") {
                vec![field]
            } else {
                vec![String::new(), field, String::new()]
            };
            lines.splice(index + 1..index + 1, block);
        }
        None => {
            lines.splice(body_start..body_start, [field, String::new()]);
        }
    }
    true
}

/// The sheet a decision writes for a debt only the index lists: its row's
/// title, area and severity under a heading. `None` without a row.
fn sheet_from_row(id: &str, row: Option<&crate::core::td_index::IndexRow>) -> Option<String> {
    let row = row?;
    let mut out = format!("# {id}: {}\n\n", row.title.as_deref().unwrap_or(id));
    for (label, value) in [("Severity", &row.severity), ("Area", &row.area)] {
        if let Some(value) = value {
            out.push_str(&format!("- **{label}**: {value}\n"));
        }
    }
    out.push_str(&format!(
        "- **Status**: {}\n",
        row.status.as_deref().unwrap_or("Open")
    ));
    Some(out)
}

/// A sheet an audit wrote without front matter gets the minimal block that
/// can carry its decision history.
fn ensure_frontmatter(lines: &mut Vec<String>) {
    if lines.first().is_some_and(|line| line.trim() == "---") {
        return;
    }
    let block = ["---", "metadata:", "  type: tech-debt", "---", ""];
    lines.splice(0..0, block.map(str::to_string));
}

fn update_td(
    content: &str,
    action: TdResolutionAction,
    origin: TdDecisionOrigin,
    reason: Option<&str>,
) -> Result<String> {
    let trailing_newline = content.ends_with('\n');
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    if let Err(error) = replace_current_status(&mut lines, action.status()) {
        if !insert_status(&mut lines, action.status()) {
            return Err(error);
        }
    }
    ensure_frontmatter(&mut lines);
    append_audit_history(&mut lines, action, origin, reason)?;
    let mut updated = lines.join("\n");
    if trailing_newline {
        updated.push('\n');
    }
    Ok(updated)
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Appends an accepted ADR for a TD the user accepted as a trade-off. The ADR
/// covers the TD, so the next audit settles the sheet again if it re-emits it.
fn append_decision(
    content: &str,
    td_id: &str,
    title: &str,
    rationale: Option<&str>,
    date: &str,
    origin: TdDecisionOrigin,
) -> String {
    let source = format!("tech-debt/{td_id}.md");
    if content.lines().any(|line| line.contains(&source)) {
        return content.to_string();
    }
    let next = crate::core::adr::parse(content)
        .iter()
        .filter_map(|adr| adr.id.strip_prefix("ADR-")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    let rationale = rationale
        .map(one_line)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Accepted during audit validation.".to_string());
    let title = one_line(title);
    let source_label = origin.adr_source();
    let entry = format!(
        "## ADR-{next:04}: Accept `{td_id}` as an intentional trade-off\n\n\
- **Status**: accepted\n\
- **Date**: {date}\n\
- **Covers**: {td_id}\n\
- **Source**: {source_label}\n\
- **Links**: [`{td_id}`]({source})\n\n\
### Context\n\n\
The audit reported `{td_id}`: {title}\n\n\
### Decision\n\n\
{rationale}\n\n\
### Consequences\n\n\
The audit records this finding as a decision, not debt. Reopen it only with new evidence, by superseding this ADR.\n"
    );
    let mut updated = content.trim_end().to_string();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    updated.push_str(&entry);
    updated
}

pub(crate) fn apply_td_resolutions(
    conn: &Connection,
    discussion_id: &str,
    resolutions: &[TdResolution],
    rationale: Option<&str>,
) -> Result<()> {
    if resolutions.is_empty() {
        return Err(anyhow!("An audit TD card must resolve at least one TD"));
    }
    let project_path = linked_project_path(conn, discussion_id)?;
    let items: Vec<(&TdResolution, Option<&str>)> =
        resolutions.iter().map(|r| (r, rationale)).collect();
    apply_td_resolutions_at(&project_path, &items, TdDecisionOrigin::Card)?;
    let project_id: Option<String> = conn
        .query_row(
            "SELECT project_id FROM discussions WHERE id = ?1",
            [discussion_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if let Some(project_id) = project_id {
        crate::core::td_tasks::close_inactive_logged(
            conn,
            &project_id,
            &crate::core::docs_location::DocsRoot::of(&project_path).dir(&project_path),
            "audit validation card",
        );
    }
    Ok(())
}

/// Writes human TD decisions on the sheets of `project_path`. Every id is
/// checked and every new content computed before the first write; a failed
/// write restores the files already written.
pub(crate) fn apply_td_resolutions_at(
    project_path: &Path,
    resolutions: &[(&TdResolution, Option<&str>)],
    origin: TdDecisionOrigin,
) -> Result<()> {
    if resolutions.is_empty() {
        return Err(anyhow!("At least one TD must be resolved"));
    }
    let docs_root = crate::core::docs_location::DocsRoot::of(project_path);
    let docs_dir = canonical_child(project_path, &docs_root.dir(project_path))?;
    // A project with only the central index has no `tech-debt/` yet: the first
    // decision creates it. One that exists is resolved, so a symlink out of
    // the docs root is still refused.
    let td_path = docs_dir.join("tech-debt");
    let td_missing = td_path.symlink_metadata().is_err();
    let td_dir = if td_missing {
        td_path
    } else {
        canonical_child(&docs_dir, &td_path)?
    };

    let mut seen = std::collections::HashSet::new();
    let mut updates = Vec::with_capacity(resolutions.len());
    let mut titles = std::collections::HashMap::new();
    for (resolution, reason) in resolutions {
        if !valid_td_id(&resolution.td_id) || !seen.insert(resolution.td_id.as_str()) {
            return Err(anyhow!("Invalid or duplicate audit TD id"));
        }
        let candidate = td_dir.join(format!("{}.md", resolution.td_id));
        // A debt only the index lists has no file yet: the decision writes it.
        let (path, content, existing) = if candidate.symlink_metadata().is_err() {
            let rows = crate::core::td_index::docs_index_rows(&docs_dir);
            let content = sheet_from_row(&resolution.td_id, rows.get(&resolution.td_id))
                .ok_or_else(|| anyhow!("{} has no sheet and no index row", resolution.td_id))?;
            (candidate, content, false)
        } else {
            let path = canonical_child(&td_dir, &candidate)?;
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            (path, content, true)
        };
        let title = crate::core::td_index::parse_sheet(&resolution.td_id, &content).title;
        titles.insert(resolution.td_id.as_str(), title);
        let mut updated = update_td(&content, resolution.action, origin, *reason)?;
        // The fixed note quotes the project's docs root (KT-1192); a typed
        // reason is kept as written.
        if !docs_root.is_default() && typed_reason(*reason).is_none() {
            updated = updated.replacen(
                "recorded in docs/decisions.md.",
                &format!("recorded in {}.", docs_root.path("decisions.md")),
                1,
            );
        }
        updates.push((path, existing.then_some(content), updated));
    }

    let accepted = resolutions
        .iter()
        .filter(|(resolution, _)| resolution.action == TdResolutionAction::AcceptDecision)
        .collect::<Vec<_>>();
    let mut card_adrs: Vec<String> = Vec::new();
    let decision_update = if accepted.is_empty() {
        None
    } else {
        let target = docs_dir.join("decisions.md");
        let target = if target.exists() {
            canonical_child(&docs_dir, &target)?
        } else {
            target
        };
        let original = if target.exists() {
            Some(
                std::fs::read_to_string(&target)
                    .with_context(|| format!("cannot read {}", target.display()))?,
            )
        } else {
            None
        };
        let mut content = original
            .clone()
            .unwrap_or_else(|| "# Architecture decisions (why, not what)\n".to_string());
        let date = Utc::now().date_naive().format("%Y-%m-%d").to_string();
        let before: Vec<String> = crate::core::adr::parse(&content)
            .into_iter()
            .map(|adr| adr.id)
            .collect();
        for (resolution, rationale) in accepted {
            let title = titles
                .get(resolution.td_id.as_str())
                .map_or(resolution.td_id.as_str(), String::as_str);
            content = append_decision(
                &content,
                &resolution.td_id,
                title,
                *rationale,
                &date,
                origin,
            );
        }
        card_adrs = crate::core::adr::parse(&content)
            .into_iter()
            .map(|adr| adr.id)
            .filter(|id| !id.is_empty() && !before.contains(id))
            .collect();
        Some((target, original, content))
    };

    // A decided sheet now carries a person's decision: human-validated (KT-1079).
    let root = project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf());
    let mut decided = Vec::new();
    let mut written: Vec<(&Path, Option<&str>)> = Vec::new();
    let mut outcome: Result<()> = Ok(());
    // Created only now that every decision computed: a refused one leaves no
    // empty folder behind. `docs_dir` is canonical, so the folder lands in it.
    let mut created_td_dir = false;
    if td_missing {
        match std::fs::create_dir(&td_dir) {
            Ok(()) => created_td_dir = true,
            Err(error) => {
                outcome = Err(anyhow!(error).context(format!("cannot create {}", td_dir.display())))
            }
        }
    }
    for ((path, original, content), (resolution, _)) in updates.iter().zip(resolutions.iter()) {
        if outcome.is_err() {
            break;
        }
        if let Err(error) = crate::core::mcp_scanner::atomic_write(path, content) {
            outcome = Err(anyhow!(error));
            break;
        }
        written.push((path.as_path(), original.as_deref()));
        if resolution.action != TdResolutionAction::Defer {
            if let Ok(rel) = path.strip_prefix(&root) {
                decided.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    if outcome.is_ok() {
        if let Some((path, original, content)) = &decision_update {
            match crate::core::mcp_scanner::atomic_write(path, content) {
                Ok(()) => written.push((path.as_path(), original.as_deref())),
                Err(error) => outcome = Err(anyhow!(error)),
            }
        }
    }
    if let Err(error) = outcome {
        for (path, original) in written {
            let restored = match original {
                Some(original) => crate::core::mcp_scanner::atomic_write(path, original),
                None => std::fs::remove_file(path).map_err(|e| e.to_string()),
            };
            if let Err(restore) = restored {
                tracing::error!("Could not restore {}: {restore}", path.display());
            }
        }
        if created_td_dir {
            // Empty again once its sheets are gone; a failure leaves it be.
            let _ = std::fs::remove_dir(&td_dir);
        }
        return Err(error);
    }
    if let Err(error) = crate::core::doc_provenance::record_validation(
        &root,
        &decided,
        crate::core::doc_provenance::ValidationReason::ValidationCard,
    ) {
        tracing::warn!("Human-validation provenance of the TD cards not recorded: {error}");
    }
    // The decision is on the sheets; the index status column follows them.
    // Only the ADRs this decision appended settle here: the human accepted them.
    let trust = crate::core::adr::Trust::CardAccepted(&card_adrs);
    if let Err(error) = crate::core::td_index::resync(&docs_dir, trust) {
        tracing::warn!("Tech-debt index resync after a TD decision failed: {error}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn td(status: &str) -> String {
        format!(
            "---\nname: td-test\nmetadata:\n  type: tech-debt\n  audit_history:\n    - date: 2026-09-01\n      status: {status}\n      reviewer: Full audit\n---\n\n# TD-test\n\n- **Severity**: High\n- **Status**: {status}\n"
        )
    }

    #[test]
    fn status_and_history_are_updated_idempotently() {
        let once = update_td(
            &td("Verified in source"),
            TdResolutionAction::Defer,
            TdDecisionOrigin::Card,
            None,
        )
        .unwrap();
        assert!(once.contains("- **Status**: Deferred"));
        assert!(once.contains("status: Deferred"));
        assert!(once.contains("reviewer: Kronn audit validation card"));
        assert_eq!(
            update_td(
                &once,
                TdResolutionAction::Defer,
                TdDecisionOrigin::Card,
                None
            )
            .unwrap(),
            once,
            "a retry must not append a second audit-history entry"
        );
    }

    #[test]
    fn accepted_decision_adds_one_durable_adr_covering_the_td() {
        let once = append_decision(
            "# Decisions\n",
            "TD-20260928-choice",
            "A deliberate | odd choice",
            Some("Known trade-off"),
            "2026-10-11",
            TdDecisionOrigin::Card,
        );
        assert!(once.contains("tech-debt/TD-20260928-choice.md"));
        assert!(once.contains("Known trade-off"));
        assert_eq!(
            append_decision(
                &once,
                "TD-20260928-choice",
                "t",
                Some("duplicate"),
                "2026-10-11",
                TdDecisionOrigin::Card,
            ),
            once
        );
        let adrs = crate::core::adr::parse(&once);
        assert_eq!(adrs.len(), 1);
        assert_eq!(adrs[0].id, "ADR-0001");
        assert_eq!(crate::core::adr::in_force(&adrs).len(), 1);
        assert_eq!(adrs[0].covers, vec!["choice".to_string()]);
        assert!(crate::core::adr::problems(&once).is_empty(), "{once}");
        // The next accepted TD gets the next id.
        let twice = append_decision(
            &once,
            "TD-20260929-other",
            "t",
            None,
            "2026-10-11",
            TdDecisionOrigin::Card,
        );
        assert!(twice.contains("## ADR-0002: Accept `TD-20260929-other`"));
        assert!(crate::core::adr::problems(&twice).is_empty());
    }

    #[test]
    fn every_status_a_card_writes_reads_back_as_its_decision() {
        let cases = [
            (TdResolutionAction::Confirm, TdDecision::Confirmed),
            (TdResolutionAction::Reject, TdDecision::Rejected),
            (
                TdResolutionAction::AcceptDecision,
                TdDecision::AcceptedDecision,
            ),
            (TdResolutionAction::Defer, TdDecision::Deferred),
        ];
        for (action, decision) in cases {
            let written = update_td(
                &td("Verified in source"),
                action,
                TdDecisionOrigin::Card,
                None,
            )
            .unwrap();
            assert_eq!(td_decision(&written), Some(decision), "{action:?}");
        }
        assert_eq!(td_decision(&td("Verified in source")), None);
        assert_eq!(td_decision(&td("Inferred")), None);
        assert!(!TdDecision::Deferred.is_settled());
        assert!(TdDecision::Confirmed.is_settled() && !TdDecision::Confirmed.leaves_index());
        assert!(TdDecision::Rejected.leaves_index());
        assert!(TdDecision::AcceptedDecision.leaves_index());
    }

    #[test]
    fn the_current_body_status_wins_over_the_history() {
        // An older rejection in the history does not outlive a later confirmation.
        let content = "---\nmetadata:\n  audit_history:\n    - date: 2026-08-01\n      status: Rejected\n    - date: 2026-09-01\n      status: Confirmed by user\n---\n\n# TD\n\n- **Status**: Confirmed by user\n";
        assert_eq!(td_decision(content), Some(TdDecision::Confirmed));
        // A status quoted inside a code fence is an example, not the sheet's status.
        let fenced = "# TD\n\n```\nStatus: Rejected\n```\n\n- **Status**: Inferred\n";
        assert_eq!(td_decision(fenced), None);
    }

    #[test]
    fn a_status_only_in_front_matter_is_read_and_written() {
        let sheet = "---\nname: td-x\nstatus: Open\nmetadata:\n  type: tech-debt\n---\n\n# TD-x — café ☕\n";
        assert_eq!(td_decision(sheet), None);
        let written = update_td(
            sheet,
            TdResolutionAction::Reject,
            TdDecisionOrigin::Card,
            None,
        )
        .unwrap();
        assert!(written.contains("\nstatus: Rejected\n"));
        assert_eq!(td_decision(&written), Some(TdDecision::Rejected));
        // Only history: its last entry is the current status.
        let history = "---\nmetadata:\n  audit_history:\n    - date: 2026-09-01\n      status: Verified in source\n    - date: 2026-09-02\n      status: Deferred\n---\n# TD\n";
        assert_eq!(td_decision(history), Some(TdDecision::Deferred));
        assert_eq!(td_decision(""), None);
    }

    #[test]
    fn td_ids_cannot_escape_the_tech_debt_directory() {
        assert!(valid_td_id("TD-20260928-safe-slug"));
        assert!(!valid_td_id("../decisions"));
        assert!(!valid_td_id("TD-safe/../../decisions"));
        // The dimension audits' detail files are sheets too (KT-1309).
        assert!(valid_td_id("CQ-001"));
        assert!(valid_td_id("PERF-20261016-deep-clone-pattern"));
        assert!(!valid_td_id(".hidden"));
        assert!(!valid_td_id("CQ-001/../x"));
    }

    /// The front matter of a sheet, through a strict YAML parser: Kronn's own
    /// reader is lenient, a standard parser (Ruby, serde_yaml) is not.
    fn strict_front_matter(content: &str) -> serde_yaml::Value {
        let rest = content
            .strip_prefix("---\n")
            .unwrap_or_else(|| panic!("no front matter:\n{content}"));
        let end = rest
            .find("\n---")
            .unwrap_or_else(|| panic!("unterminated front matter:\n{content}"));
        serde_yaml::from_str(&rest[..end])
            .unwrap_or_else(|error| panic!("strict YAML parse failed: {error}\n{content}"))
    }

    /// `(status, note)` of each `audit_history` entry, in file order.
    fn strict_history(content: &str) -> Vec<(String, String)> {
        let front = strict_front_matter(content);
        let entries = front
            .get("audit_history")
            .or_else(|| front.get("metadata").and_then(|m| m.get("audit_history")))
            .and_then(serde_yaml::Value::as_sequence)
            .unwrap_or_else(|| panic!("no audit_history list:\n{content}"));
        entries
            .iter()
            .map(|entry| {
                let text = |key: &str| {
                    entry
                        .get(key)
                        .and_then(serde_yaml::Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                (text("status"), text("note"))
            })
            .collect()
    }

    /// Existing history shapes: key indent, item indent, key column.
    fn sheet_with_history(key_indent: usize, item_indent: usize, key_col: usize) -> String {
        let sp = |n: usize| " ".repeat(n);
        let (meta, wrapper) = if key_indent == 0 {
            (String::new(), String::new())
        } else {
            (
                "metadata:\n".to_string(),
                format!("{}type: tech-debt\n", sp(key_indent)),
            )
        };
        format!(
            "---\nname: td-test\n{meta}{wrapper}{k}audit_history:\n{i}- date: 2026-09-01\n{c}status: Verified in source\n{c}reviewer: Full audit\n{c}note: \"first\"\n{o}other: kept\n---\n\n# TD-test\n\n- **Severity**: High\n- **Status**: Verified in source\n",
            k = sp(key_indent),
            i = sp(item_indent),
            c = sp(key_col),
            o = sp(key_indent),
        )
    }

    #[test]
    fn a_saved_decision_keeps_the_front_matter_valid_whatever_the_indentation() {
        // (key indent, item indent, key column): 2-space, 4-space, an
        // indentless sequence, a top-level key, and a deep one.
        let shapes = [
            (2, 4, 6),
            (4, 8, 10),
            (4, 6, 8),
            (2, 2, 4),
            (0, 2, 4),
            (0, 0, 2),
            (6, 8, 10),
        ];
        for (key_indent, item_indent, key_col) in shapes {
            let input = sheet_with_history(key_indent, item_indent, key_col);
            strict_front_matter(&input);
            for action in [
                TdResolutionAction::Confirm,
                TdResolutionAction::Defer,
                TdResolutionAction::Reject,
                TdResolutionAction::AcceptDecision,
            ] {
                let written =
                    update_td(&input, action, TdDecisionOrigin::Review, Some("because")).unwrap();
                let history = strict_history(&written);
                assert_eq!(
                    history.len(),
                    2,
                    "shape {key_indent}/{item_indent}/{key_col} {action:?}:\n{written}"
                );
                assert_eq!(history[0].1, "first");
                assert_eq!(
                    history[1],
                    (action.status().to_string(), "because".to_string())
                );
                // The key after the list is still its sibling, not an entry field.
                assert!(written.contains("other: kept"));
            }
        }
    }

    #[test]
    fn a_second_decision_joins_the_list_the_first_one_wrote() {
        let input = sheet_with_history(4, 8, 10);
        let first = update_td(
            &input,
            TdResolutionAction::Defer,
            TdDecisionOrigin::Review,
            Some("later"),
        )
        .unwrap();
        let second = update_td(
            &first,
            TdResolutionAction::Confirm,
            TdDecisionOrigin::Review,
            Some("real"),
        )
        .unwrap();
        let history = strict_history(&second);
        assert_eq!(history.len(), 3);
        assert_eq!(history[2], ("Confirmed by user".into(), "real".into()));
        assert_eq!(td_decision(&second), Some(TdDecision::Confirmed));
    }

    #[test]
    fn front_matter_without_a_history_gets_one_that_parses() {
        for front in [
            "name: td-x\nmetadata:\n  type: tech-debt",
            "name: td-x\nmetadata:\n    type: tech-debt",
            "name: td-x",
            "name: td-x\nmetadata:\n  type: tech-debt\n  audit_history: []",
        ] {
            let input = format!("---\n{front}\n---\n\n# TD-x\n\n- **Status**: Inferred\n");
            let written = update_td(
                &input,
                TdResolutionAction::Reject,
                TdDecisionOrigin::Card,
                None,
            )
            .unwrap();
            let history = strict_history(&written);
            assert_eq!(history.len(), 1, "{front}\n{written}");
            assert_eq!(history[0].0, "Rejected");
        }
    }

    #[test]
    fn commented_keys_and_flow_mappings_get_no_duplicate_key() {
        for front in [
            // A trailing comment on the history key.
            "name: td-x\nmetadata:\n  type: tech-debt\n  audit_history: # log\n    - date: 2026-09-01\n      status: Inferred\n      reviewer: Full audit\n      note: \"first\"",
            "name: td-x\nmetadata: # meta\n  type: tech-debt\n  audit_history:   # log\n    - date: 2026-09-01\n      status: Inferred\n      reviewer: Full audit\n      note: \"first\"",
            // A comment line inside the list.
            "name: td-x\nmetadata:\n  type: tech-debt\n  audit_history:\n    # earlier\n    - date: 2026-09-01\n      status: Inferred\n      reviewer: Full audit\n      note: \"first\"\n    # trailing",
            // A flow-style empty list with a comment.
            "name: td-x\nmetadata:\n  type: tech-debt\n  audit_history: [] # none yet",
            // Flow mappings.
            "name: td-x\nmetadata: {type: tech-debt}",
            "name: td-x\nmetadata: {type: tech-debt, owner: \"a, b\", tags: [x, y]} # inline",
            "name: td-x\nmetadata: {}",
            "name: td-x\nmetadata: # meta only",
            // A flow mapping that already holds an empty history: it is the
            // key the entry joins, not one to repeat.
            "name: td-x\nmetadata: {type: tech-debt, audit_history: []}",
            "name: td-x\nmetadata: {audit_history: [], type: tech-debt} # inline",
            "name: td-x\nmetadata: {\"audit_history\": ~, type: tech-debt}",
            "name: td-x\nmetadata: {audit_history: }",
            // A `#` inside quotes is text, not a comment.
            "name: td-x\nmetadata: {type: tech-debt, note: \"see #12\", by: don't} # real",
            "name: td-x\nmetadata:\n  audit_history: [] # \"quoted # inside\"",
        ] {
            let input = format!("---\n{front}\n---\n\n# TD-x\n\n- **Status**: Inferred\n");
            strict_front_matter(&input);
            let written = update_td(
                &input,
                TdResolutionAction::Confirm,
                TdDecisionOrigin::Review,
                Some("why"),
            )
            .unwrap();
            // The strict parser rejects a duplicated key: parsing is the proof.
            let history = strict_history(&written);
            assert_eq!(history.last().unwrap(), &("Confirmed by user".into(), "why".into()));
            assert_eq!(written.matches("audit_history:").count(), 1, "{written}");
            assert_eq!(written.matches("\nmetadata:").count(), 1, "{written}");
            assert!(!written.contains("metadata: {"), "{written}");
            // A second decision joins the same list.
            let again = update_td(
                &written,
                TdResolutionAction::Defer,
                TdDecisionOrigin::Review,
                Some("later"),
            )
            .unwrap();
            assert_eq!(strict_history(&again).len(), history.len() + 1, "{again}");
        }
        // The pairs of a flow mapping are kept.
        let input = "---\nname: td-x\nmetadata: {type: tech-debt, owner: \"a, b\"}\n---\n\n# TD-x\n\n- **Status**: Inferred\n";
        let written = update_td(
            input,
            TdResolutionAction::Confirm,
            TdDecisionOrigin::Review,
            None,
        )
        .unwrap();
        let front = strict_front_matter(&written);
        assert_eq!(front["metadata"]["type"].as_str(), Some("tech-debt"));
        assert_eq!(front["metadata"]["owner"].as_str(), Some("a, b"));
    }

    #[test]
    fn a_flow_mapping_keeps_its_pairs_and_its_history_key_once() {
        let input = "---\nname: td-x\nmetadata: {type: tech-debt, audit_history: [], note: \"see #12, ok\", by: don't} # inline\n---\n\n# TD-x\n\n- **Status**: Inferred\n";
        strict_front_matter(input);
        let written = update_td(
            input,
            TdResolutionAction::Reject,
            TdDecisionOrigin::Review,
            Some("not real"),
        )
        .unwrap();
        // Strict parse: a duplicate `audit_history` key would be an error.
        let front = strict_front_matter(&written);
        let meta = &front["metadata"];
        assert_eq!(meta["type"].as_str(), Some("tech-debt"));
        assert_eq!(meta["note"].as_str(), Some("see #12, ok"));
        assert_eq!(meta["by"].as_str(), Some("don't"));
        assert_eq!(
            strict_history(&written),
            [("Rejected".to_string(), "not real".to_string())]
        );
        assert_eq!(written.matches("audit_history").count(), 1, "{written}");
        assert!(written.contains("# inline"), "{written}");
    }

    #[test]
    fn a_hash_inside_quotes_is_not_taken_for_a_comment() {
        // Was refused as "a metadata flow mapping on several lines".
        let input = "---\nname: td-x\nmetadata: {type: tech-debt, note: \"a #b\"}\n---\n\n# TD-x\n\n- **Status**: Inferred\n";
        let written = update_td(
            input,
            TdResolutionAction::Confirm,
            TdDecisionOrigin::Review,
            None,
        )
        .unwrap();
        assert_eq!(
            strict_front_matter(&written)["metadata"]["note"].as_str(),
            Some("a #b")
        );
        assert_eq!(comment_start("a: \"x # y\" # z"), Some(11));
        assert_eq!(comment_start("a: 'x # y'"), None);
        assert_eq!(comment_start("a: it's # z"), Some(8));
        assert_eq!(comment_start("a: b#c"), None);
    }

    #[test]
    fn a_history_that_is_not_a_block_list_is_refused_not_duplicated() {
        for input in [
            "---\nname: td-x\nmetadata:\n  audit_history: [{date: 2026-09-01}]\n---\n\n# TD-x\n\n- **Status**: Inferred\n",
            "---\nname: td-x\nmetadata: {audit_history: [{date: 2026-09-01}]}\n---\n\n# TD-x\n\n- **Status**: Inferred\n",
            "---\nname: td-x\nmetadata: {audit_history: none}\n---\n\n# TD-x\n\n- **Status**: Inferred\n",
        ] {
            assert!(
                update_td(
                    input,
                    TdResolutionAction::Confirm,
                    TdDecisionOrigin::Review,
                    None
                )
                .is_err(),
                "{input}"
            );
        }
    }

    #[test]
    fn a_missing_tech_debt_folder_is_created_by_the_first_decision() {
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(
            docs.join("inconsistencies-tech-debt.md"),
            "# Index\n\n## Current list\n\n| ID | Problem | Area | Severity | Status |\n|----|---------|------|----------|--------|\n| TD-20261016-rowonly | Row only | API | High | Open |\n",
        )
        .unwrap();
        assert!(!docs.join("tech-debt").exists());
        let resolution = TdResolution {
            td_id: "TD-20261016-rowonly".into(),
            action: TdResolutionAction::Confirm,
        };
        apply_td_resolutions_at(
            tmp.path(),
            &[(&resolution, Some("Seen in prod."))],
            TdDecisionOrigin::Review,
        )
        .unwrap();
        let sheet = std::fs::read_to_string(docs.join("tech-debt/TD-20261016-rowonly.md")).unwrap();
        assert_eq!(
            strict_history(&sheet),
            [("Confirmed by user".to_string(), "Seen in prod.".to_string())]
        );
        assert!(sheet.contains("- **Severity**: High"), "{sheet}");

        // A decision that cannot be written leaves no folder behind.
        let bare = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(bare.path().join("docs")).unwrap();
        let ghost = TdResolution {
            td_id: "TD-20261016-ghost".into(),
            action: TdResolutionAction::Confirm,
        };
        assert!(
            apply_td_resolutions_at(bare.path(), &[(&ghost, None)], TdDecisionOrigin::Review)
                .is_err()
        );
        assert!(!bare.path().join("docs/tech-debt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_tech_debt_folder_that_leaves_the_docs_root_is_still_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::os::unix::fs::symlink(outside.path(), docs.join("tech-debt")).unwrap();
        let resolution = TdResolution {
            td_id: "TD-20261016-x".into(),
            action: TdResolutionAction::Confirm,
        };
        let error =
            apply_td_resolutions_at(tmp.path(), &[(&resolution, None)], TdDecisionOrigin::Review)
                .unwrap_err();
        assert!(error.to_string().contains("escapes"), "{error}");
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_new_reason_the_same_day_is_kept_and_a_retry_is_not_repeated() {
        let first = update_td(
            &td("Inferred"),
            TdResolutionAction::Confirm,
            TdDecisionOrigin::Review,
            Some("first reason"),
        )
        .unwrap();
        // Same status, same reviewer, same day, a new reason: not dropped.
        let second = update_td(
            &first,
            TdResolutionAction::Confirm,
            TdDecisionOrigin::Review,
            Some("second reason"),
        )
        .unwrap();
        let history = strict_history(&second);
        assert_eq!(history.len(), 3, "{second}");
        assert_eq!(history[1].1, "first reason");
        assert_eq!(history[2].1, "second reason");
        // The same reason again is a retry.
        let retry = update_td(
            &second,
            TdResolutionAction::Confirm,
            TdDecisionOrigin::Review,
            Some("second reason"),
        )
        .unwrap();
        assert_eq!(retry, second);
    }

    #[test]
    fn every_decision_stores_the_reason_that_was_typed() {
        let reason = "Intentional: \"legacy\" API\nsee RFC #12 \u{2014} caf\u{e9} \u{2615}";
        let cases = [
            TdResolutionAction::Confirm,
            TdResolutionAction::Defer,
            TdResolutionAction::Reject,
            TdResolutionAction::AcceptDecision,
        ];
        for action in cases {
            for origin in [TdDecisionOrigin::Card, TdDecisionOrigin::Review] {
                let written = update_td(&td("Inferred"), action, origin, Some(reason)).unwrap();
                let history = strict_history(&written);
                assert_eq!(history.last().unwrap().1, reason, "{action:?} {origin:?}");
                // Padding around the reason is not part of it.
                let padded =
                    update_td(&td("Inferred"), action, origin, Some("  why not \n")).unwrap();
                assert_eq!(strict_history(&padded).last().unwrap().1, "why not");
            }
        }
    }

    #[test]
    fn the_fixed_note_only_stands_in_for_an_empty_reason() {
        for reason in [None, Some(""), Some("   \n\t")] {
            let written = update_td(
                &td("Inferred"),
                TdResolutionAction::Defer,
                TdDecisionOrigin::Review,
                reason,
            )
            .unwrap();
            assert_eq!(
                strict_history(&written).last().unwrap().1,
                "Deferred from the tech-debt review screen."
            );
        }
    }

    #[test]
    fn a_sheet_without_front_matter_or_status_can_still_be_decided() {
        // The shape the dimension audits write for CQ-* / PERF-* (KT-1309).
        let bare = "# CQ-001: Oversized module\n\n- **Severity**: High\n- **Area**: src/app\n\n## Problem\n\nIt grew.\n";
        let written = update_td(
            bare,
            TdResolutionAction::Confirm,
            TdDecisionOrigin::Review,
            Some("agreed"),
        )
        .unwrap();
        assert_eq!(td_decision(&written), Some(TdDecision::Confirmed));
        assert_eq!(
            strict_history(&written),
            vec![("Confirmed by user".to_string(), "agreed".to_string())]
        );
        assert!(written.contains("- **Severity**: High"), "{written}");
        assert!(written.contains("## Problem\n\nIt grew."), "{written}");
        // Deciding again changes the status and does not duplicate the field.
        let rejected = update_td(
            &written,
            TdResolutionAction::Reject,
            TdDecisionOrigin::Review,
            None,
        )
        .unwrap();
        assert_eq!(rejected.matches("**Status**").count(), 1, "{rejected}");
        assert_eq!(td_decision(&rejected), Some(TdDecision::Rejected));
        assert_eq!(strict_history(&rejected).len(), 2);
    }

    #[test]
    fn a_sheet_without_a_title_gets_its_status_first() {
        let written = update_td(
            "just text\n",
            TdResolutionAction::Defer,
            TdDecisionOrigin::Card,
            None,
        )
        .unwrap();
        assert_eq!(td_decision(&written), Some(TdDecision::Deferred));
        strict_history(&written);
        // Front matter that never closes is refused, not rewritten.
        assert!(update_td(
            "---\nname: x\n",
            TdResolutionAction::Defer,
            TdDecisionOrigin::Card,
            None
        )
        .is_err());
    }
}
