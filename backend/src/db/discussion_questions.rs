use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DiscussionQuestionOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuestionSpec {
    version: u32,
    key: String,
    question: String,
    context: Option<String>,
    #[serde(default)]
    options: Vec<DiscussionQuestionOption>,
    #[serde(default)]
    multiple: bool,
    #[serde(default)]
    recommended_option_ids: Vec<String>,
    task_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiscussionQuestionState {
    Pending,
    Answered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionQuestionAnswer {
    pub selected_option_ids: Vec<String>,
    pub text: Option<String>,
    pub author_pseudo: String,
    pub answered_at: String,
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionQuestion {
    pub id: String,
    pub discussion_id: String,
    pub source_message_id: String,
    pub fence_index: i64,
    pub key: String,
    pub question: String,
    pub context: Option<String>,
    pub options: Vec<DiscussionQuestionOption>,
    pub multiple: bool,
    pub recommended_option_ids: Vec<String>,
    pub task_ref: Option<String>,
    pub state: DiscussionQuestionState,
    pub answer: Option<DiscussionQuestionAnswer>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionQuestionList {
    pub questions: Vec<DiscussionQuestion>,
    pub pending_count: usize,
}

/// Read projection for sidebar polling; not part of a discussion's stored settings.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionListItem {
    #[serde(flatten)]
    pub discussion: crate::models::Discussion,
    pub pending_question_count: u32,
}

pub fn with_pending_counts(
    conn: &Connection,
    discussions: Vec<crate::models::Discussion>,
) -> Result<Vec<DiscussionListItem>> {
    let mut statement = conn.prepare(
        "SELECT discussion_id, COUNT(*) FROM discussion_questions WHERE state='pending' GROUP BY discussion_id",
    )?;
    let counts = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?
        .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()?;
    Ok(discussions
        .into_iter()
        .map(|discussion| DiscussionListItem {
            pending_question_count: counts.get(&discussion.id).copied().unwrap_or(0),
            discussion,
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct AnswerDiscussionQuestionRequest {
    #[serde(default)]
    pub selected_option_ids: Vec<String>,
    pub text: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AnswerError {
    #[error("Discussion question not found")]
    NotFound,
    #[error("This question has already been answered")]
    Conflict,
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

fn nonempty_bounded(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max
}

fn stable_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn parse_spec(body: &str) -> Option<QuestionSpec> {
    if body.len() > 24_000 {
        return None;
    }
    let spec: QuestionSpec = serde_json::from_str(body).ok()?;
    let ids = spec
        .options
        .iter()
        .map(|o| o.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    (spec.version == 1
        && stable_key(&spec.key)
        && nonempty_bounded(&spec.question, 1000)
        && spec
            .context
            .as_ref()
            .is_none_or(|s| s.chars().count() <= 4000)
        && spec
            .task_ref
            .as_ref()
            .is_none_or(|s| nonempty_bounded(s, 100))
        && spec.options.len() <= 8
        && ids.len() == spec.options.len()
        && spec.options.iter().all(|o| {
            stable_key(&o.id)
                && nonempty_bounded(&o.label, 250)
                && o.description
                    .as_ref()
                    .is_none_or(|s| s.chars().count() <= 1000)
        })
        && spec
            .recommended_option_ids
            .iter()
            .all(|id| ids.contains(id.as_str()))
        && (spec.multiple || spec.recommended_option_ids.len() <= 1))
        .then_some(spec)
}

// Track every fence, including examples, so a nested example never becomes a live question.
fn question_fences(content: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut open: Option<(char, usize, bool)> = None;
    let mut body = String::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some((marker, count, question)) = open {
            let closing_count = trimmed.chars().take_while(|c| *c == marker).count();
            if closing_count >= count && trimmed[closing_count..].trim().is_empty() {
                if question {
                    result.push(std::mem::take(&mut body));
                }
                open = None;
            } else if question {
                body.push_str(line);
                body.push('\n');
            }
        } else if line.len() - trimmed.len() <= 3 {
            let Some(marker @ ('`' | '~')) = trimmed.chars().next() else {
                continue;
            };
            let count = trimmed.chars().take_while(|c| *c == marker).count();
            if count >= 3 {
                open = Some((marker, count, trimmed[count..].trim() == "kronn-question"));
                body.clear();
            }
        }
    }
    result
}

pub fn ingest_message_questions(
    conn: &Connection,
    discussion_id: &str,
    message_id: &str,
    content: &str,
) -> Result<()> {
    if !content.contains("kronn-question") {
        return Ok(());
    }
    // A peer provider must not inherit the room's unrelated default connection.
    // Dispatch provenance, attached after message insertion, takes precedence at answer time.
    let connection_id: Option<String> = conn.query_row(
        "SELECT CASE WHEN m.agent_type=d.agent THEN d.connection_id ELSE NULL END
         FROM messages m JOIN discussions d ON d.id=m.discussion_id WHERE m.id=?1 AND d.id=?2",
        params![message_id, discussion_id],
        |row| row.get(0),
    )?;
    for (index, body) in question_fences(content).into_iter().take(8).enumerate() {
        let Some(spec) = parse_spec(&body) else {
            continue;
        };
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO discussion_questions (id, discussion_id, source_message_id, fence_index,
                 question_key, payload_json, requester_connection_id, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)
             ON CONFLICT DO NOTHING",
            params![
                format!("question:{message_id}:{index}"),
                discussion_id,
                message_id,
                index as i64,
                spec.key,
                serde_json::to_string(&spec)?,
                connection_id,
                now
            ],
        )?;
    }
    Ok(())
}

pub fn list(conn: &Connection, discussion_id: &str) -> Result<DiscussionQuestionList> {
    let mut statement = conn.prepare(
        "SELECT id, source_message_id, fence_index, payload_json, state, answer_json, created_at, updated_at
         FROM discussion_questions WHERE discussion_id = ?1 ORDER BY created_at, id"
    )?;
    let rows = statement.query_map([discussion_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
        ))
    })?;
    let mut questions = Vec::new();
    for row in rows {
        let (id, source_message_id, fence_index, payload, state, answer, created_at, updated_at) =
            row?;
        let spec: QuestionSpec = serde_json::from_str(&payload)?;
        let state = match state.as_str() {
            "pending" => DiscussionQuestionState::Pending,
            "answered" => DiscussionQuestionState::Answered,
            _ => anyhow::bail!("Invalid question state"),
        };
        questions.push(DiscussionQuestion {
            id,
            discussion_id: discussion_id.to_string(),
            source_message_id,
            fence_index,
            key: spec.key,
            question: spec.question,
            context: spec.context,
            options: spec.options,
            multiple: spec.multiple,
            recommended_option_ids: spec.recommended_option_ids,
            task_ref: spec.task_ref,
            state,
            answer: answer.map(|a| serde_json::from_str(&a)).transpose()?,
            created_at,
            updated_at,
        });
    }
    let pending_count = questions
        .iter()
        .filter(|q| q.state == DiscussionQuestionState::Pending)
        .count();
    Ok(DiscussionQuestionList {
        questions,
        pending_count,
    })
}

pub fn answer(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
    request: &AnswerDiscussionQuestionRequest,
    author_pseudo: &str,
    author_avatar_email: Option<&str>,
) -> Result<DiscussionQuestion, AnswerError> {
    if !nonempty_bounded(&request.idempotency_key, 128) {
        return Err(AnswerError::Invalid(
            "A bounded idempotency key is required".into(),
        ));
    }
    conn.execute_batch("SAVEPOINT answer_discussion_question")
        .map_err(anyhow::Error::from)?;
    let outcome = answer_inner(
        conn,
        discussion_id,
        question_id,
        request,
        author_pseudo,
        author_avatar_email,
    );
    match outcome {
        Ok(question) => {
            conn.execute_batch("RELEASE answer_discussion_question")
                .map_err(anyhow::Error::from)?;
            Ok(question)
        }
        Err(error) => {
            let _ = conn.execute_batch(
                "ROLLBACK TO answer_discussion_question; RELEASE answer_discussion_question",
            );
            Err(error)
        }
    }
}

fn answer_inner(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
    request: &AnswerDiscussionQuestionRequest,
    author_pseudo: &str,
    author_avatar_email: Option<&str>,
) -> Result<DiscussionQuestion, AnswerError> {
    let mut question = list(conn, discussion_id)?
        .questions
        .into_iter()
        .find(|q| q.id == question_id)
        .ok_or(AnswerError::NotFound)?;
    let text = request
        .text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let mut selected = request.selected_option_ids.clone();
    selected.sort();
    if selected.windows(2).any(|w| w[0] == w[1])
        || (!question.multiple && selected.len() > 1)
        || selected
            .iter()
            .any(|id| !question.options.iter().any(|o| &o.id == id))
        || (selected.is_empty() && text.is_none())
        || text.as_ref().is_some_and(|s| s.chars().count() > 8000)
    {
        return Err(AnswerError::Invalid(
            "Select valid options or provide a non-empty answer (maximum 8000 characters)".into(),
        ));
    }
    if question.state == DiscussionQuestionState::Answered {
        let key: String = conn
            .query_row(
                "SELECT answer_idempotency_key FROM discussion_questions WHERE id=?1",
                [question_id],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)?;
        if key == request.idempotency_key
            && question
                .answer
                .as_ref()
                .is_some_and(|a| a.selected_option_ids == selected && a.text == text)
        {
            return Ok(question);
        }
        return Err(AnswerError::Conflict);
    }
    let now = Utc::now();
    let answer = DiscussionQuestionAnswer {
        selected_option_ids: selected.clone(),
        text: text.clone(),
        author_pseudo: author_pseudo.to_string(),
        answered_at: now.to_rfc3339(),
        message_id: format!("question-answer:{question_id}"),
    };
    let labels = question
        .options
        .iter()
        .filter(|o| selected.contains(&o.id))
        .map(|o| format!("- {}", o.label))
        .collect::<Vec<_>>()
        .join("\n");
    let content = format!(
        "Décision humaine — {}\n\n{}{}{}",
        question.question,
        labels,
        if labels.is_empty() || text.is_none() {
            ""
        } else {
            "\n\n"
        },
        text.as_deref().unwrap_or("")
    );
    let message = crate::models::DiscussionMessage {
        id: answer.message_id.clone(),
        role: crate::models::MessageRole::User,
        channel: crate::models::MessageChannel::Main,
        content,
        agent_type: None,
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: Some(author_pseudo.to_string()),
        author_avatar_email: author_avatar_email.map(str::to_string),
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: Some(question.source_message_id.clone()),
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
    };
    let sort_order = super::discussions::insert_message(conn, discussion_id, &message)?;
    let cli_target = super::discussions::message_cli_author_target(
        conn,
        discussion_id,
        &question.source_message_id,
    )?;
    if let Some(target) = cli_target {
        super::discussions::replace_message_targets(conn, &message.id, &[target])?;
    } else {
        let agent: Option<String> = conn
            .query_row(
                "SELECT agent_type FROM messages WHERE id=?1 AND discussion_id=?2",
                params![question.source_message_id, discussion_id],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)?;
        if let Some(agent) = agent {
            let agent = super::discussions::parse_agent_type(&agent);
            let connection_id: Option<String> = conn
                .query_row(
                    "SELECT COALESCE(j.connection_id, q.requester_connection_id)
                     FROM discussion_questions q JOIN messages m ON m.id=q.source_message_id
                     LEFT JOIN agent_dispatch_jobs j ON j.id=m.agent_dispatch_job_id WHERE q.id=?1",
                    [question_id],
                    |r| r.get(0),
                )
                .map_err(anyhow::Error::from)?;
            let mut target = crate::models::MessageTarget::agent(agent.clone());
            target.connection_id = connection_id.clone();
            super::discussions::replace_message_targets(conn, &message.id, &[target])?;
            super::agent_dispatch::enqueue_with_connection(
                conn,
                super::agent_dispatch::NewAgentDispatchJob {
                    id: &uuid::Uuid::new_v4().to_string(),
                    discussion_id,
                    trigger_message_id: &message.id,
                    trigger_sort_order: sort_order,
                    dedupe_key: &format!("question-answer:{question_id}"),
                    agent_override: Some(&agent),
                    chain_prompt_ids: &[],
                    batch_item: None,
                    group_id: None,
                    group_concurrency_limit: None,
                },
                connection_id.as_deref(),
            )?;
        }
    }
    conn.execute("UPDATE discussion_questions SET state='answered', answer_json=?2, answer_idempotency_key=?3, updated_at=?4 WHERE id=?1 AND state='pending'",
        params![question_id, serde_json::to_string(&answer).map_err(anyhow::Error::from)?, request.idempotency_key, answer.answered_at])
        .map_err(anyhow::Error::from)?;
    question.state = DiscussionQuestionState::Answered;
    question.updated_at = answer.answered_at.clone();
    question.answer = Some(answer);
    Ok(question)
}

#[cfg(test)]
mod tests;
