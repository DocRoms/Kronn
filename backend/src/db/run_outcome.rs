//! What a launched run produced, told the way a card can show it.
//!
//! A workflow's own result says which steps ran; the work a human cares about
//! is often elsewhere — in the discussion a `BatchQuickPrompt` step opened,
//! where an agent framed the ticket. This projection gathers those
//! discussions for one run (its whole tree) or for one discussion, with the
//! agent's state and the beginning of its latest answer.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use ts_rs::TS;

use crate::models::{AgentType, Discussion};

/// Enough for a card; the rest is one "open" away.
const MAX_DISCUSSIONS: usize = 12;
const EXCERPT_CHARS: usize = 700;
const DIAGNOSTIC_CHARS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcomeAgentStatus {
    /// Queued or running: the answer is still to come.
    Working,
    Answered,
    /// The agent's turn failed; `diagnostic` says why.
    Failed,
    /// Stopped on purpose; whatever it said before still shows.
    Cancelled,
    /// Nothing asked of an agent, or nothing answered yet.
    Idle,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RunOutcomeDiscussion {
    pub id: String,
    pub title: String,
    pub agent: AgentType,
    pub agent_status: RunOutcomeAgentStatus,
    /// The beginning of the agent's latest answer: agents lead with their
    /// verdict, so this is the part a card has room for.
    pub answer_excerpt: Option<String>,
    pub answer_truncated: bool,
    pub answered_at: Option<String>,
    /// Why the agent failed, when it did.
    pub diagnostic: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RunOutcome {
    /// Every discussion produced, even beyond the ones listed.
    pub discussion_count: u32,
    pub discussions: Vec<RunOutcomeDiscussion>,
}

pub fn for_run(conn: &Connection, run_id: &str) -> Result<RunOutcome> {
    let discussions = crate::db::discussions::list_discussions_by_run_tree(conn, run_id)?;
    project(conn, discussions)
}

pub fn for_discussion(conn: &Connection, discussion_id: &str) -> Result<RunOutcome> {
    let discussion = crate::db::discussions::get_discussion(conn, discussion_id)?;
    project(conn, discussion.into_iter().collect())
}

fn project(conn: &Connection, discussions: Vec<Discussion>) -> Result<RunOutcome> {
    let discussion_count = discussions.len() as u32;
    let discussions = discussions
        .into_iter()
        .take(MAX_DISCUSSIONS)
        .map(|discussion| describe(conn, discussion))
        .collect::<Result<Vec<_>>>()?;
    Ok(RunOutcome {
        discussion_count,
        discussions,
    })
}

fn describe(conn: &Connection, discussion: Discussion) -> Result<RunOutcomeDiscussion> {
    let answer: Option<(String, String)> = conn
        .query_row(
            "SELECT content, timestamp FROM messages
             WHERE discussion_id = ?1 AND role = 'Agent'
             ORDER BY sort_order DESC LIMIT 1",
            params![discussion.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let dispatch: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT status, last_error FROM agent_dispatch_jobs
             WHERE discussion_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1",
            params![discussion.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    use crate::db::agent_dispatch::DispatchStatus;
    let latest = dispatch
        .as_ref()
        .map(|(status, _)| DispatchStatus::parse(status))
        .transpose()?;
    let agent_status = match latest {
        Some(DispatchStatus::Pending | DispatchStatus::Running) => RunOutcomeAgentStatus::Working,
        Some(DispatchStatus::Failed) => RunOutcomeAgentStatus::Failed,
        Some(DispatchStatus::Cancelled) => RunOutcomeAgentStatus::Cancelled,
        _ if answer.is_some() => RunOutcomeAgentStatus::Answered,
        _ => RunOutcomeAgentStatus::Idle,
    };
    let diagnostic = (agent_status == RunOutcomeAgentStatus::Failed)
        .then(|| dispatch.and_then(|(_, error)| error))
        .flatten()
        .map(|error| truncate(&error, DIAGNOSTIC_CHARS).0);
    let (answer_excerpt, answer_truncated, answered_at) = match answer {
        Some((content, at)) => {
            let (excerpt, truncated) = truncate(content.trim(), EXCERPT_CHARS);
            (Some(excerpt), truncated, Some(at))
        }
        None => (None, false, None),
    };
    Ok(RunOutcomeDiscussion {
        id: discussion.id,
        title: discussion.title,
        agent: discussion.agent,
        agent_status,
        answer_excerpt,
        answer_truncated,
        answered_at,
        diagnostic,
        updated_at: discussion.updated_at.to_rfc3339(),
    })
}

/// Cut on a character, never inside one.
fn truncate(text: &str, max_chars: usize) -> (String, bool) {
    match text.char_indices().nth(max_chars) {
        Some((byte, _)) => (text[..byte].trim_end().to_string(), true),
        None => (text.to_string(), false),
    }
}
