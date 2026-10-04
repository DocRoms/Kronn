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

    fn history_note(self) -> &'static str {
        match self {
            Self::Confirm => "Confirmed through an audit validation card.",
            Self::Reject => "Rejected through an audit validation card.",
            Self::AcceptDecision => {
                "Accepted as an intentional decision and recorded in docs/decisions.md."
            }
            Self::Defer => "Deferred through an audit validation card.",
        }
    }
}

const STATUS_CONFIRMED: &str = "Confirmed by user";
const STATUS_REJECTED: &str = "Rejected";
const STATUS_ACCEPTED_DECISION: &str = "Accepted decision";
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

fn valid_td_id(value: &str) -> bool {
    value.starts_with("TD-")
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
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

fn replace_current_status(lines: &mut [String], status: &str) -> Result<()> {
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

fn append_audit_history(lines: &mut Vec<String>, action: TdResolutionAction) -> Result<()> {
    let date = Utc::now().date_naive().format("%Y-%m-%d").to_string();
    let signature = format!("status: {}", action.status());
    let reviewer = "reviewer: Kronn audit validation card";
    if lines.windows(4).any(|window| {
        window
            .iter()
            .any(|line| line.trim() == format!("- date: {date}"))
            && window.iter().any(|line| line.trim() == signature)
            && window.iter().any(|line| line.trim() == reviewer)
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
    let entry = vec![
        format!("    - date: {date}"),
        format!("      status: {}", action.status()),
        "      reviewer: Kronn audit validation card".to_string(),
        format!(
            "      note: {}",
            serde_json::to_string(action.history_note())?
        ),
    ];

    if let Some(history_index) = lines[..frontmatter_end]
        .iter()
        .position(|line| line.trim() == "audit_history:")
    {
        let insert_at = lines
            .iter()
            .enumerate()
            .take(frontmatter_end)
            .skip(history_index + 1)
            .find_map(|(index, line)| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let indent = line.len() - line.trim_start().len();
                (indent <= 2).then_some(index)
            })
            .unwrap_or(frontmatter_end);
        lines.splice(insert_at..insert_at, entry);
    } else if let Some(metadata_index) = lines[..frontmatter_end]
        .iter()
        .position(|line| line.trim() == "metadata:")
    {
        let mut block = vec!["  audit_history:".to_string()];
        block.extend(entry);
        lines.splice(metadata_index + 1..metadata_index + 1, block);
    } else {
        let mut block = vec![
            "metadata:".to_string(),
            "  type: tech-debt".to_string(),
            "  audit_history:".to_string(),
        ];
        block.extend(entry);
        lines.splice(frontmatter_end..frontmatter_end, block);
    }
    Ok(())
}

fn update_td(content: &str, action: TdResolutionAction) -> Result<String> {
    let trailing_newline = content.ends_with('\n');
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    replace_current_status(&mut lines, action.status())?;
    append_audit_history(&mut lines, action)?;
    let mut updated = lines.join("\n");
    if trailing_newline {
        updated.push('\n');
    }
    Ok(updated)
}

fn escaped_table_cell(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

fn append_decision(content: &str, td_id: &str, rationale: Option<&str>) -> String {
    let source = format!("tech-debt/{td_id}.md");
    if content.lines().any(|line| line.contains(&source)) {
        return content.to_string();
    }
    let rationale = rationale
        .map(escaped_table_cell)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Accepted during audit validation.".to_string());
    let row = format!(
        "| Accept `{td_id}` as an intentional trade-off | {rationale} | Do not reopen without new evidence | [`{td_id}`]({source}) |"
    );
    let mut updated = content.trim_end().to_string();
    if !updated.contains("## Audit validation decisions") {
        if !updated.is_empty() {
            updated.push_str("\n\n");
        }
        updated.push_str(
            "## Audit validation decisions\n\n| Decision | Why chosen | What NOT to do | Source |\n|----------|------------|----------------|--------|",
        );
    }
    updated.push('\n');
    updated.push_str(&row);
    updated.push('\n');
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
    let docs_dir = canonical_child(&project_path, &project_path.join("docs"))?;
    let td_dir = canonical_child(&docs_dir, &docs_dir.join("tech-debt"))?;

    let mut seen = std::collections::HashSet::new();
    let mut updates = Vec::with_capacity(resolutions.len());
    for resolution in resolutions {
        if !valid_td_id(&resolution.td_id) || !seen.insert(resolution.td_id.as_str()) {
            return Err(anyhow!("Invalid or duplicate audit TD id"));
        }
        let path = canonical_child(&td_dir, &td_dir.join(format!("{}.md", resolution.td_id)))?;
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        updates.push((path, update_td(&content, resolution.action)?));
    }

    let accepted = resolutions
        .iter()
        .filter(|resolution| resolution.action == TdResolutionAction::AcceptDecision)
        .collect::<Vec<_>>();
    let decision_update = if accepted.is_empty() {
        None
    } else {
        let target = docs_dir.join("decisions.md");
        let target = if target.exists() {
            canonical_child(&docs_dir, &target)?
        } else {
            target
        };
        let mut content = if target.exists() {
            std::fs::read_to_string(&target)
                .with_context(|| format!("cannot read {}", target.display()))?
        } else {
            "# Architecture decisions (why, not what)\n".to_string()
        };
        for resolution in accepted {
            content = append_decision(&content, &resolution.td_id, rationale);
        }
        Some((target, content))
    };

    for (path, content) in updates {
        crate::core::mcp_scanner::atomic_write(&path, &content).map_err(|error| anyhow!(error))?;
    }
    if let Some((path, content)) = decision_update {
        crate::core::mcp_scanner::atomic_write(&path, &content).map_err(|error| anyhow!(error))?;
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
        let once = update_td(&td("Verified in source"), TdResolutionAction::Defer).unwrap();
        assert!(once.contains("- **Status**: Deferred"));
        assert!(once.contains("status: Deferred"));
        assert!(once.contains("reviewer: Kronn audit validation card"));
        assert_eq!(
            update_td(&once, TdResolutionAction::Defer).unwrap(),
            once,
            "a retry must not append a second audit-history entry"
        );
    }

    #[test]
    fn accepted_decision_adds_one_durable_decisions_row() {
        let once = append_decision(
            "# Decisions\n",
            "TD-20260928-choice",
            Some("Known trade-off"),
        );
        assert!(once.contains("## Audit validation decisions"));
        assert!(once.contains("tech-debt/TD-20260928-choice.md"));
        assert!(once.contains("Known trade-off"));
        assert_eq!(
            append_decision(&once, "TD-20260928-choice", Some("duplicate")),
            once
        );
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
            let written = update_td(&td("Verified in source"), action).unwrap();
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
        let written = update_td(sheet, TdResolutionAction::Reject).unwrap();
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
    }
}
