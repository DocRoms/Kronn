//! Turns the audit's tech-debt sheets (`docs/tech-debt/TD-*.md`) into Kronn
//! planning tasks. The sheets stay the source of truth; the tasks only track
//! the work. One parent task per theme, one subtask per sheet, every task
//! keyed so a second conversion creates nothing.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;

use crate::core::audit_validation::TdDecision;
use crate::core::td_index::{is_td_id, parse_sheet, TdSheet};
use crate::models::{
    CreatePlanningDodItem, CreatePlanningTaskRequest, PlanningActor, PlanningTaskPriority,
    PlanningTaskStatus, TdTaskItem, TdTaskState, TdTaskTheme, TdTasksOutcome,
};

/// Briefing line that opts a project into the conversion after a Full audit.
pub const BRIEFING_OPTION_LINE: &str = "- Turn audit tech debt into Kronn tasks: yes";
pub const BRIEFING_OPTIONS_HEADING: &str = "## Kronn options";
pub const TECH_DEBT_TAG: &str = "tech-debt";
const PROBLEM_MAX: usize = 600;
const VERIFICATION_MAX: usize = 480;
const TITLE_MAX: usize = 240;

/// One sheet with the excerpts a task needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TdTaskSource {
    pub sheet: TdSheet,
    pub problem: Option<String>,
    pub verification: Option<String>,
    /// Repository-relative path of the sheet.
    pub source_path: String,
}

pub fn briefing_wants_td_tasks(notes: &str) -> bool {
    notes
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case(BRIEFING_OPTION_LINE))
}

/// Task key of one sheet: `audit:<project_id>:<TD-id>`.
pub fn td_task_key(project_id: &str, td_id: &str) -> String {
    format!("audit:{project_id}:{td_id}")
}

fn theme_task_key(project_id: &str, theme: &str) -> String {
    format!("audit:{project_id}:theme:{theme}")
}

/// Every `TD-*.md` directly under `<docs>/tech-debt/`, sorted by id. Symlinks
/// are skipped, as for the index.
pub fn load_sources(docs_dir: &Path) -> Vec<TdTaskSource> {
    let Ok(entries) = std::fs::read_dir(docs_dir.join("tech-debt")) else {
        return Vec::new();
    };
    let docs_name = docs_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("docs")
        .to_string();
    let mut sources: Vec<TdTaskSource> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let id = name.strip_suffix(".md")?;
            if !is_td_id(id) {
                return None;
            }
            let content = std::fs::read_to_string(e.path()).ok()?;
            Some(source_from(
                id,
                &content,
                format!("{docs_name}/tech-debt/{name}"),
            ))
        })
        .collect();
    sources.sort_by(|a, b| a.sheet.id.cmp(&b.sheet.id));
    sources
}

pub fn source_from(id: &str, content: &str, source_path: String) -> TdTaskSource {
    let body = body_lines(content);
    TdTaskSource {
        sheet: parse_sheet(id, content),
        problem: field_block(&body, &["problem"]).map(|p| truncate(&p, PROBLEM_MAX)),
        verification: field_block(
            &body,
            &[
                "verification",
                "acceptance",
                "done when",
                "definition of done",
            ],
        )
        .map(|v| truncate(&v, VERIFICATION_MAX)),
        source_path,
    }
}

/// Lines after the front matter, outside fenced code blocks.
pub(crate) fn body_lines(content: &str) -> Vec<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let start = if lines.first().is_some_and(|l| l.trim() == "---") {
        (1..lines.len())
            .find(|&i| lines[i].trim() == "---")
            .map_or(0, |end| end + 1)
    } else {
        0
    };
    let mut in_fence = false;
    let mut out = Vec::new();
    for line in &lines[start..] {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push(*line);
        }
    }
    out
}

/// Text of a `## Name…` section or of a `- **Name…**: text` item with its
/// continuation lines, whitespace-collapsed.
pub(crate) fn field_block(body: &[&str], names: &[&str]) -> Option<String> {
    let starts_with_name = |text: &str| {
        let lower = text.to_lowercase();
        names.iter().any(|name| lower.starts_with(name))
    };
    for (idx, line) in body.iter().enumerate() {
        let t = line.trim();
        if let Some(heading) = t.strip_prefix("## ") {
            if starts_with_name(heading.trim()) {
                let text: Vec<&str> = body[idx + 1..]
                    .iter()
                    .map(|l| l.trim())
                    .take_while(|l| !l.starts_with('#'))
                    .filter(|l| !l.is_empty() && !l.starts_with("<!--"))
                    .collect();
                return collapse(&text.join(" "));
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
        let rest = rest.strip_prefix(':').unwrap_or(rest);
        let mut text = vec![rest.trim()];
        for next in &body[idx + 1..] {
            let n = next.trim();
            if n.is_empty() || n.starts_with('#') || !next.starts_with(' ') {
                break;
            }
            text.push(n);
        }
        return collapse(&text.join(" "));
    }
    None
}

fn collapse(value: &str) -> Option<String> {
    let out = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!out.is_empty()).then_some(out)
}

pub(crate) fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max - 1).collect();
    out.push('…');
    out
}

pub fn priority_for(severity: &str) -> PlanningTaskPriority {
    match severity.trim().to_lowercase().as_str() {
        s if s.starts_with("critical") => PlanningTaskPriority::Critical,
        s if s.starts_with("high") => PlanningTaskPriority::High,
        s if s.starts_with("low") => PlanningTaskPriority::Low,
        _ => PlanningTaskPriority::Normal,
    }
}

fn priority_rank(priority: PlanningTaskPriority) -> u8 {
    match priority {
        PlanningTaskPriority::Critical => 0,
        PlanningTaskPriority::High => 1,
        PlanningTaskPriority::Normal => 2,
        PlanningTaskPriority::Low => 3,
    }
}

/// `(slug, label)` of the sheet's theme: its sub-audit `Category`, else its
/// `Area` without the parenthesised detail.
pub fn theme_of(sheet: &TdSheet) -> (String, String) {
    let raw = sheet
        .category
        .as_deref()
        .filter(|c| !c.trim().is_empty())
        .unwrap_or(&sheet.area);
    let label = raw
        .split(['(', '|', ',', ';'])
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let mut slug = String::new();
    for c in label.to_lowercase().chars() {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(60).collect();
    if slug.is_empty() || slug == "unknown" {
        return ("other".to_string(), "Other".to_string());
    }
    (slug, label)
}

/// Why a sheet gets no task, if it gets none.
pub fn skip_reason(sheet: &TdSheet) -> Option<&'static str> {
    match sheet.decision {
        Some(TdDecision::Rejected) => Some("rejected"),
        Some(TdDecision::AcceptedDecision) => Some("accepted_decision"),
        _ => None,
    }
}

fn task_title(source: &TdTaskSource) -> String {
    let suffix = format!(" ({})", source.sheet.id);
    let room = TITLE_MAX.saturating_sub(suffix.chars().count()).max(1);
    format!("{}{suffix}", truncate(&source.sheet.title, room))
}

fn verification_of(source: &TdTaskSource) -> String {
    source.verification.clone().unwrap_or_else(|| {
        format!(
            "The problem described in {} no longer reproduces at the places it cites, and the next audit does not report it again.",
            source.source_path
        )
    })
}

fn task_description(source: &TdTaskSource, theme_label: &str) -> String {
    let sheet = &source.sheet;
    let mut out = format!(
        "Tech-debt sheet: `{}` (source of truth: record the fix there).\n\n\
         - Severity: {}\n- Theme: {}\n- Area: {}\n- Status: {}\n",
        source.source_path, sheet.severity, theme_label, sheet.area, sheet.status
    );
    if let Some(problem) = &source.problem {
        out.push_str(&format!("\n## Problem\n{problem}\n"));
    }
    out.push_str(&format!("\n## Verification\n{}\n", verification_of(source)));
    out
}

/// Common fields of every task of one theme.
fn base_request(project_id: &str, slug: &str, actor: &PlanningActor) -> CreatePlanningTaskRequest {
    CreatePlanningTaskRequest {
        title: String::new(),
        discussion_id: None,
        idempotency_key: None,
        description: String::new(),
        status: PlanningTaskStatus::Todo,
        priority: PlanningTaskPriority::Normal,
        parent_id: None,
        project_ids: vec![project_id.to_string()],
        tags: vec![TECH_DEBT_TAG.to_string(), slug.to_string()],
        definition_of_done: vec![],
        links: vec![],
        actor: actor.clone(),
    }
}

/// Creates the missing tasks in one transaction. A sheet whose key already
/// names a task is reported as linked, never recreated nor rewritten.
pub fn convert(
    conn: &Connection,
    project_id: &str,
    sources: &[TdTaskSource],
    actor: &PlanningActor,
) -> Result<TdTasksOutcome> {
    use crate::db::planning::{create_task, task_by_idempotency_key};

    let mut outcome = TdTasksOutcome::default();
    let mut groups: BTreeMap<String, (String, Vec<&TdTaskSource>)> = BTreeMap::new();
    for source in sources {
        let (slug, label) = theme_of(&source.sheet);
        if let Some(reason) = skip_reason(&source.sheet) {
            outcome.skipped += 1;
            outcome.items.push(TdTaskItem {
                td_id: source.sheet.id.clone(),
                theme: slug,
                state: TdTaskState::Skipped,
                task_reference: None,
                reason: Some(reason.to_string()),
            });
            continue;
        }
        groups
            .entry(slug)
            .or_insert_with(|| (label, Vec::new()))
            .1
            .push(source);
    }

    let tx = conn.unchecked_transaction()?;
    for (slug, (label, members)) in &groups {
        let mut linked = Vec::new();
        let mut pending = Vec::new();
        for source in members {
            match task_by_idempotency_key(&tx, &td_task_key(project_id, &source.sheet.id))? {
                Some((_, reference)) => linked.push((*source, reference)),
                None => pending.push(*source),
            }
        }
        let theme_key = theme_task_key(project_id, slug);
        let mut parent = task_by_idempotency_key(&tx, &theme_key)?;
        let mut parent_created = false;
        if parent.is_none() && !pending.is_empty() {
            let priority = pending
                .iter()
                .map(|s| priority_for(&s.sheet.severity))
                .min_by_key(|p| priority_rank(*p))
                .unwrap_or_default();
            let created = create_task(
                &tx,
                &CreatePlanningTaskRequest {
                    title: truncate(&format!("Tech debt: {label}"), TITLE_MAX),
                    description: format!(
                        "Audit tech debt of the theme `{slug}`, one subtask per sheet under \
                         `docs/tech-debt/`. The sheets stay the source of truth."
                    ),
                    idempotency_key: Some(theme_key),
                    priority,
                    ..base_request(project_id, slug, actor)
                },
            )?;
            parent = Some((created.summary.id, created.summary.reference));
            parent_created = true;
        }
        for (source, reference) in linked {
            outcome.already_linked += 1;
            outcome.items.push(TdTaskItem {
                td_id: source.sheet.id.clone(),
                theme: slug.clone(),
                state: TdTaskState::AlreadyLinked,
                task_reference: Some(reference),
                reason: None,
            });
        }
        for source in pending {
            let created = create_task(
                &tx,
                &CreatePlanningTaskRequest {
                    title: task_title(source),
                    description: task_description(source, label),
                    idempotency_key: Some(td_task_key(project_id, &source.sheet.id)),
                    priority: priority_for(&source.sheet.severity),
                    parent_id: parent.as_ref().map(|(id, _)| id.clone()),
                    definition_of_done: vec![CreatePlanningDodItem {
                        id: None,
                        sentence: verification_of(source),
                        completed: false,
                    }],
                    ..base_request(project_id, slug, actor)
                },
            )?;
            outcome.created += 1;
            outcome.items.push(TdTaskItem {
                td_id: source.sheet.id.clone(),
                theme: slug.clone(),
                state: TdTaskState::Created,
                task_reference: Some(created.summary.reference),
                reason: None,
            });
        }
        if let Some((_, reference)) = parent {
            outcome.themes.push(TdTaskTheme {
                theme: slug.clone(),
                label: label.clone(),
                task_reference: reference,
                created: parent_created,
            });
        }
    }
    tx.commit()?;
    Ok(outcome)
}

#[cfg(test)]
#[path = "td_tasks_test.rs"]
mod tests;
