//! Bounded monitor reads. No transcript hydration, attachments, dispatch, or
//! writes: every query is scoped to one selected discussion's indexed id.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};

use crate::models::{DiscussionMonitorMessage, DiscussionMonitorPreview, PlanningPlanStats};

pub const MAX_MONITOR_DISCUSSIONS: usize = 12;
const MESSAGE_LIMIT: usize = 8;
const CONTENT_LIMIT: usize = 2048;
const PARTIAL_LIMIT: usize = 4096;

/// The SQL projection reads at most limit + 1 Unicode scalar values. The extra
/// character signals truncation without loading the complete text into Rust.
fn bounded_content(content: String, limit: usize, tail: bool) -> (String, bool) {
    let count = content.chars().count();
    if count <= limit {
        return (content, false);
    }
    let content = if tail {
        content.chars().skip(count - limit).collect()
    } else {
        content.chars().take(limit).collect()
    };
    (content, true)
}

pub fn preview(conn: &Connection, id: &str) -> Result<Option<DiscussionMonitorPreview>> {
    let preview = conn
        .query_row(
            "SELECT substr(d.title, 1, 500), d.shared_id, d.agent, d.no_agent,
                (SELECT display_name FROM external_api_connections WHERE id = d.connection_id),
                d.awaiting_agent,
                EXISTS(SELECT 1 FROM agent_dispatch_jobs j WHERE j.discussion_id = d.id
                       AND j.status = 'Running' AND j.agent_started_at IS NOT NULL),
                (SELECT progress_phase FROM agent_dispatch_jobs j WHERE j.discussion_id = d.id
                    AND j.status = 'Running' ORDER BY j.created_at DESC LIMIT 1),
                (SELECT COUNT(*) FROM discussion_questions q
                    WHERE q.discussion_id = d.id AND q.state = 'pending'),
                d.updated_at, d.partial_response_message_id,
                substr(d.partial_response, -?2), d.partial_response_agent_type,
                d.partial_response_model, d.partial_response_started_at
           FROM discussions d WHERE d.id = ?1",
            rusqlite::params![id, (PARTIAL_LIMIT + 1) as u32],
            |row| {
                let partial: Option<String> = row.get(11)?;
                let partial_response = partial
                    .map(|content| {
                        let (content, truncated) = bounded_content(content, PARTIAL_LIMIT, true);
                        Ok::<_, rusqlite::Error>(DiscussionMonitorMessage {
                            id: row
                                .get::<_, Option<String>>(10)?
                                .unwrap_or_else(|| format!("partial-{id}")),
                            role: "Agent".into(),
                            channel: "main".into(),
                            content,
                            truncated,
                            agent_type: row
                                .get::<_, Option<String>>(12)?
                                .map(|agent| super::discussions::parse_agent_type(&agent))
                                .transpose()?,
                            model: row.get(13)?,
                            author_pseudo: None,
                            author_cli_ordinal: None,
                            timestamp: row.get::<_, Option<String>>(14)?.unwrap_or_default(),
                        })
                    })
                    .transpose()?;
                Ok(DiscussionMonitorPreview {
                    plan: PlanningPlanStats::default(),
                    title: row.get(0)?,
                    shared_id: row.get(1)?,
                    agent: if row.get::<_, bool>(3)? {
                        None
                    } else {
                        Some(super::discussions::parse_agent_type(
                            &row.get::<_, String>(2)?,
                        )?)
                    },
                    connection_name: row.get(4)?,
                    awaiting_agent: row.get(5)?,
                    agent_running: row.get(6)?,
                    progress_phase: row.get(7)?,
                    pending_question_count: row.get(8)?,
                    updated_at: row.get(9)?,
                    messages: vec![],
                    partial_response,
                })
            },
        )
        .optional()?;
    let Some(mut preview) = preview else {
        return Ok(None);
    };
    let mut statement = conn.prepare(
        "SELECT id, role, channel, substr(content, 1, ?2), agent_type,
                model, author_pseudo, timestamp,
                (SELECT (SELECT COUNT(*) FROM discussion_sessions e
                          WHERE e.disc_id = s.disc_id AND e.agent_type = s.agent_type AND e.id <= s.id)
                   FROM message_cli_authors mca JOIN discussion_sessions s ON s.id = mca.cli_session_id
                  WHERE mca.message_id = messages.id)
           FROM messages WHERE discussion_id = ?1
          ORDER BY sort_order DESC LIMIT ?3",
    )?;
    preview.messages = statement
        .query_map(
            rusqlite::params![id, (CONTENT_LIMIT + 1) as u32, MESSAGE_LIMIT as u32],
            |row| {
                let (content, truncated) = bounded_content(row.get(3)?, CONTENT_LIMIT, false);
                Ok(DiscussionMonitorMessage {
                    id: row.get(0)?,
                    role: row.get(1)?,
                    channel: row.get(2)?,
                    content,
                    truncated,
                    agent_type: row
                        .get::<_, Option<String>>(4)?
                        .map(|agent| super::discussions::parse_agent_type(&agent))
                        .transpose()?,
                    model: row.get(5)?,
                    author_pseudo: row.get(6)?,
                    author_cli_ordinal: row.get(8)?,
                    timestamp: row.get(7)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    preview.messages.reverse();
    preview.plan = plan_stats(conn, id)?;
    Ok(Some(preview))
}

/// Aggregate in SQLite: at most six rows, without task descriptions, events,
/// DoD or dependency hydration. Keep precedence aligned with get_discussion_plan.
fn plan_stats(conn: &Connection, id: &str) -> Result<PlanningPlanStats> {
    let mut statement = conn.prepare(
        "SELECT CASE
            WHEN relation.placement = 'later' THEN 'later'
            WHEN task.status = 'done' THEN 'done'
            WHEN task.status = 'blocked' OR EXISTS (
                SELECT 1 FROM planning_task_blockers b
                JOIN planning_tasks blocker ON blocker.id = b.blocker_task_id
                WHERE b.task_id = task.id AND blocker.status NOT IN ('done', 'archived')
            ) THEN 'blocked'
            WHEN task.status = 'in_progress' THEN 'in_progress'
            WHEN task.status = 'idea' THEN 'ideas'
            ELSE 'ready' END AS bucket, COUNT(*)
         FROM planning_task_discussions relation
         JOIN planning_tasks task ON task.id = relation.task_id
         WHERE relation.discussion_id = ?1 AND task.status <> 'archived'
         GROUP BY bucket",
    )?;
    let mut stats = PlanningPlanStats::default();
    for row in statement.query_map([id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
    })? {
        let (bucket, count) = row?;
        match bucket.as_str() {
            "later" => stats.later = count,
            "done" => stats.done = count,
            "blocked" => stats.blocked = count,
            "in_progress" => stats.in_progress = count,
            "ideas" => stats.ideas = count,
            _ => stats.ready = count,
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_text_bounds_preserve_unicode_and_mark_only_real_truncation() {
        assert_eq!(
            bounded_content("é🙂".into(), 2, false),
            ("é🙂".into(), false)
        );
        assert_eq!(
            bounded_content("é🙂終".into(), 2, false),
            ("é🙂".into(), true)
        );
        assert_eq!(
            bounded_content("é🙂終".into(), 2, true),
            ("🙂終".into(), true)
        );
        assert_eq!(
            bounded_content(String::new(), 2, true),
            (String::new(), false)
        );
    }
}
