//! Pure rules of a tech-debt sheet: the human decision it records, its status
//! line and the `audit_history` front matter. Kept free of the database so the
//! index, the ADR check and the validation cards share one reading of a sheet.

use anyhow::{anyhow, Result};
use chrono::Utc;

pub const STATUS_CONFIRMED: &str = "Confirmed by user";
pub const STATUS_REJECTED: &str = "Rejected";
pub const STATUS_ACCEPTED_DECISION: &str = "Accepted decision";
pub const STATUS_DEFERRED: &str = "Deferred";

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
    pub fn status_label(self) -> &'static str {
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

pub fn replace_current_status(lines: &mut [String], status: &str) -> Result<()> {
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
pub fn comment_start(text: &str) -> Option<usize> {
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
pub fn append_history_entry(
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

pub fn is_td_id(value: &str) -> bool {
    value.len() > 3
        && value.len() <= 100
        && value.starts_with("TD-")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
