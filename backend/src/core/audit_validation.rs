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

fn append_audit_history(
    lines: &mut Vec<String>,
    action: TdResolutionAction,
    origin: TdDecisionOrigin,
) -> Result<()> {
    append_history_entry(
        lines,
        action.status(),
        origin.reviewer(),
        action.history_note(origin),
    )
}

/// Appends one `audit_history` entry, unless the same status by the same
/// reviewer was already recorded today.
pub(crate) fn append_history_entry(
    lines: &mut Vec<String>,
    status: &str,
    reviewer: &str,
    note: &str,
) -> Result<()> {
    let date = Utc::now().date_naive().format("%Y-%m-%d").to_string();
    let signature = format!("status: {status}");
    let reviewer_line = format!("reviewer: {reviewer}");
    if lines.windows(4).any(|window| {
        window
            .iter()
            .any(|line| line.trim() == format!("- date: {date}"))
            && window.iter().any(|line| line.trim() == signature)
            && window.iter().any(|line| line.trim() == reviewer_line)
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
        format!("      status: {status}"),
        format!("      {reviewer_line}"),
        format!("      note: {}", serde_json::to_string(note)?),
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

fn update_td(
    content: &str,
    action: TdResolutionAction,
    origin: TdDecisionOrigin,
) -> Result<String> {
    let trailing_newline = content.ends_with('\n');
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    replace_current_status(&mut lines, action.status())?;
    append_audit_history(&mut lines, action, origin)?;
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
    apply_td_resolutions_at(&project_path, &items, TdDecisionOrigin::Card)
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
    let td_dir = canonical_child(&docs_dir, &docs_dir.join("tech-debt"))?;

    let mut seen = std::collections::HashSet::new();
    let mut updates = Vec::with_capacity(resolutions.len());
    let mut titles = std::collections::HashMap::new();
    for (resolution, _) in resolutions {
        if !valid_td_id(&resolution.td_id) || !seen.insert(resolution.td_id.as_str()) {
            return Err(anyhow!("Invalid or duplicate audit TD id"));
        }
        let path = canonical_child(&td_dir, &td_dir.join(format!("{}.md", resolution.td_id)))?;
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let title = crate::core::td_index::parse_sheet(&resolution.td_id, &content).title;
        titles.insert(resolution.td_id.as_str(), title);
        let mut updated = update_td(&content, resolution.action, origin)?;
        // The note quotes the project's docs root (KT-1192).
        if !docs_root.is_default() {
            updated = updated.replacen(
                "recorded in docs/decisions.md.",
                &format!("recorded in {}.", docs_root.path("decisions.md")),
                1,
            );
        }
        updates.push((path, content, updated));
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
    for ((path, original, content), (resolution, _)) in updates.iter().zip(resolutions.iter()) {
        if let Err(error) = crate::core::mcp_scanner::atomic_write(path, content) {
            outcome = Err(anyhow!(error));
            break;
        }
        written.push((path.as_path(), Some(original.as_str())));
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
        )
        .unwrap();
        assert!(once.contains("- **Status**: Deferred"));
        assert!(once.contains("status: Deferred"));
        assert!(once.contains("reviewer: Kronn audit validation card"));
        assert_eq!(
            update_td(&once, TdResolutionAction::Defer, TdDecisionOrigin::Card).unwrap(),
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
            let written =
                update_td(&td("Verified in source"), action, TdDecisionOrigin::Card).unwrap();
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
        let written = update_td(sheet, TdResolutionAction::Reject, TdDecisionOrigin::Card).unwrap();
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
