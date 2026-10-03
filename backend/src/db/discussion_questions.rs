use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DiscussionQuestionOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DiscussionQuestionItem {
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
    items: Vec<DiscussionQuestionItem>,
    #[serde(default)]
    multiple: bool,
    #[serde(default)]
    recommended_option_ids: Vec<String>,
    task_ref: Option<String>,
    #[serde(default)]
    resume: Option<QuestionResumeSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
struct QuestionResumeSpec {
    workflow_id: String,
    #[serde(default)]
    variables: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiscussionQuestionState {
    Pending,
    Answered,
    /// The human refused to arbitrate, or the question no longer applies.
    /// Recorded like an answer — same author, same reason field, same dispatch
    /// to the asker — because refusing IS a decision the agent must receive.
    /// Without it, picking an option was the only way to clear a question,
    /// including one that should never have been asked.
    Declined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiscussionQuestionResumeState {
    Pending,
    Launching,
    Launched,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionQuestionResume {
    pub workflow_id: String,
    #[ts(type = "Record<string, string>")]
    pub variables: HashMap<String, String>,
    pub state: DiscussionQuestionResumeState,
    pub run_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionQuestionWorkflowStep {
    #[serde(flatten)]
    pub identity: crate::models::WorkflowStepIdentity,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DeclineDiscussionQuestionRequest {
    pub idempotency_key: String,
    /// Why it is refused. Optional: a human owes no justification, but the
    /// agent reads it, so an empty refusal still has to be actionable.
    pub reason: Option<String>,
}

/// A remark on a question that is not a decision: it reaches the asker and
/// leaves the card waiting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CommentDiscussionQuestionRequest {
    pub idempotency_key: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionQuestionAnswer {
    pub selected_option_ids: Vec<String>,
    #[serde(default)]
    pub item_answers: Vec<DiscussionQuestionItemAnswer>,
    pub text: Option<String>,
    pub author_pseudo: String,
    pub answered_at: String,
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DiscussionQuestionItemAnswer {
    pub item_id: String,
    pub selected_option_id: String,
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
    pub items: Vec<DiscussionQuestionItem>,
    pub multiple: bool,
    pub recommended_option_ids: Vec<String>,
    pub task_ref: Option<String>,
    pub requester_workflow_step: Option<DiscussionQuestionWorkflowStep>,
    pub resume: Option<DiscussionQuestionResume>,
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
    #[serde(default)]
    pub item_answers: Vec<DiscussionQuestionItemAnswer>,
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

fn valid_resume_variable(key: &str, value: &str) -> bool {
    (1..=64).contains(&key.len())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        && value.chars().count() <= 8000
        && !value.chars().any(char::is_control)
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
    let item_ids = spec
        .items
        .iter()
        .map(|item| item.id.as_str())
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
        && spec.items.len() <= 8
        && item_ids.len() == spec.items.len()
        && spec.items.iter().all(|item| {
            stable_key(&item.id)
                && nonempty_bounded(&item.label, 250)
                && item
                    .description
                    .as_ref()
                    .is_none_or(|value| value.chars().count() <= 1000)
        })
        && (spec.items.is_empty() || !spec.options.is_empty())
        && spec
            .recommended_option_ids
            .iter()
            .all(|id| ids.contains(id.as_str()))
        && (spec.multiple || spec.recommended_option_ids.len() <= 1)
        && spec.resume.as_ref().is_none_or(|resume| {
            nonempty_bounded(&resume.workflow_id, 128)
                && resume.variables.len() <= 16
                && resume
                    .variables
                    .iter()
                    .all(|(key, value)| valid_resume_variable(key, value))
        }))
    .then_some(spec)
}

/// Drop everything inside a `kronn:context` block.
///
/// That block holds an agent's RAW output, which Kronn folds away as
/// "technical details" when a run fails. On a failure the raw output includes
/// the system prompt — and the system prompt documents the arbitration format
/// with a complete, valid `kronn-question` example. Ingesting that example
/// created a real pending arbitration reading "Which option should we use?"
/// with the example's own `decision-key`, which no human could have meant and
/// which nothing could close: the table has no path out of `pending` but a
/// human answer. Technical context is evidence, never an agent's intent.
pub(crate) fn strip_context_blocks(content: &str) -> String {
    const OPEN: &str = "<!-- kronn:context";
    const CLOSE: &str = "<!-- /kronn:context -->";
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        rest = match rest[start..].find(CLOSE) {
            Some(end) => &rest[start + end + CLOSE.len()..],
            // Unclosed block: the remainder is context to the end of the message.
            None => "",
        };
    }
    out.push_str(rest);
    out
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
    // Never ingest from folded technical context — see strip_context_blocks.
    let content = &strip_context_blocks(content);
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
    for (index, body) in question_fences(content).into_iter().take(64).enumerate() {
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
        "SELECT id, source_message_id, fence_index, payload_json, state, answer_json, created_at, updated_at,
                resume_state, resume_run_id, resume_error
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
            r.get::<_, Option<String>>(8)?,
            r.get::<_, Option<String>>(9)?,
            r.get::<_, Option<String>>(10)?,
        ))
    })?;
    let mut questions = Vec::new();
    for row in rows {
        let (
            id,
            source_message_id,
            fence_index,
            payload,
            state,
            answer,
            created_at,
            updated_at,
            resume_state,
            resume_run_id,
            resume_error,
        ) = row?;
        let spec: QuestionSpec = serde_json::from_str(&payload)?;
        let state = match state.as_str() {
            "pending" => DiscussionQuestionState::Pending,
            "answered" => DiscussionQuestionState::Answered,
            "declined" => DiscussionQuestionState::Declined,
            _ => anyhow::bail!("Invalid question state"),
        };
        let requester_workflow_step =
            super::workflow_step_rooms::message_author(conn, discussion_id, &source_message_id)?
                .map(|(identity, active)| DiscussionQuestionWorkflowStep { identity, active });
        let resume = spec
            .resume
            .map(|resume| -> Result<DiscussionQuestionResume> {
                let state = match resume_state.as_deref().unwrap_or("pending") {
                    "pending" => DiscussionQuestionResumeState::Pending,
                    "launching" => DiscussionQuestionResumeState::Launching,
                    "launched" => DiscussionQuestionResumeState::Launched,
                    "failed" => DiscussionQuestionResumeState::Failed,
                    _ => anyhow::bail!("Invalid question resume state"),
                };
                Ok(DiscussionQuestionResume {
                    workflow_id: resume.workflow_id,
                    variables: resume.variables,
                    state,
                    run_id: resume_run_id,
                    error: resume_error,
                })
            })
            .transpose()?;
        questions.push(DiscussionQuestion {
            id,
            discussion_id: discussion_id.to_string(),
            source_message_id,
            fence_index,
            key: spec.key,
            question: spec.question,
            context: spec.context,
            options: spec.options,
            items: spec.items,
            multiple: spec.multiple,
            recommended_option_ids: spec.recommended_option_ids,
            task_ref: spec.task_ref,
            requester_workflow_step,
            resume,
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

/// Publish a human resolution of `question` and route it back to whoever asked.
///
/// Shared by answering and refusing: both are decisions the asker must receive,
/// through the same target resolution (exact CLI session if the question came
/// from one, otherwise the agent plus its dispatch connection) and the same
/// dispatch enqueue. Splitting these two paths is how they would drift.
#[allow(clippy::too_many_arguments)]
fn publish_human_resolution(
    conn: &Connection,
    discussion_id: &str,
    question: &DiscussionQuestion,
    message_id: &str,
    mut content: String,
    dedupe_key: &str,
    now: chrono::DateTime<Utc>,
    author_pseudo: &str,
    author_avatar_email: Option<&str>,
    // False when the decision leaves the asker nothing to do: keeping a
    // partial answer after a ceiling must not buy one more turn.
    dispatch: bool,
) -> Result<(), AnswerError> {
    if question
        .requester_workflow_step
        .as_ref()
        .is_some_and(|source| !source.active)
    {
        content.push_str(
            "\n\nL'étape qui a posé cette question est terminée ; personne ne lira cette réponse maintenant. Elle sera disponible au prochain run.",
        );
    }
    let message = crate::models::DiscussionMessage {
        id: message_id.to_string(),
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
    if !dispatch {
        return Ok(());
    }
    let cli_target = super::discussions::message_cli_author_target(
        conn,
        discussion_id,
        &question.source_message_id,
    )?;
    if let Some(target) = cli_target {
        super::discussions::replace_message_targets(conn, &message.id, &[target])?;
        return Ok(());
    }
    let agent: Option<String> = conn
        .query_row(
            "SELECT agent_type FROM messages WHERE id=?1 AND discussion_id=?2",
            params![question.source_message_id, discussion_id],
            |r| r.get(0),
        )
        .map_err(anyhow::Error::from)?;
    let Some(agent) = agent else { return Ok(()) };
    let agent = super::discussions::parse_agent_type(&agent).map_err(anyhow::Error::from)?;
    let connection_id: Option<String> = conn
        .query_row(
            "SELECT COALESCE(j.connection_id, q.requester_connection_id)
             FROM discussion_questions q JOIN messages m ON m.id=q.source_message_id
             LEFT JOIN agent_dispatch_jobs j ON j.id=m.agent_dispatch_job_id WHERE q.id=?1",
            [question.id.as_str()],
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
            dedupe_key,
            agent_override: Some(&agent),
            chain_prompt_ids: &[],
            batch_item: None,
            group_id: None,
            group_concurrency_limit: None,
        },
        connection_id.as_deref(),
    )?;
    Ok(())
}

/// Reserved launch data for one question resolution. Claiming stores the run
/// id before any asynchronous work, so retries cannot start a second run.
#[derive(Debug, Clone)]
pub struct QuestionResumeLaunch {
    pub question_id: String,
    pub run_id: String,
    pub workflow_id: String,
    pub variables: HashMap<String, String>,
    pub source_run_id: String,
    pub discussion_id: String,
    pub project_id: Option<String>,
}

pub fn claim_resume(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
) -> Result<Option<QuestionResumeLaunch>, AnswerError> {
    let question = list(conn, discussion_id)?
        .questions
        .into_iter()
        .find(|question| question.id == question_id)
        .ok_or(AnswerError::NotFound)?;
    if question.state == DiscussionQuestionState::Pending {
        return Ok(None);
    }
    let Some(resume) = question.resume else {
        return Ok(None);
    };
    if resume.state != DiscussionQuestionResumeState::Pending {
        return Ok(None);
    }
    let Some(source) = question.requester_workflow_step else {
        conn.execute(
            "UPDATE discussion_questions SET resume_state='failed', resume_error=?2
              WHERE id=?1 AND resume_state IS NULL",
            params![
                question_id,
                "Only workflow-step questions can declare a resume"
            ],
        )
        .map_err(anyhow::Error::from)?;
        return Ok(None);
    };
    let run_id = uuid::Uuid::new_v4().to_string();
    let claimed = conn
        .execute(
            "UPDATE discussion_questions
            SET resume_state='launching', resume_run_id=?2, resume_error=NULL
          WHERE id=?1 AND resume_state IS NULL",
            params![question_id, run_id],
        )
        .map_err(anyhow::Error::from)?;
    if claimed == 0 {
        return Ok(None);
    }
    let project_id = conn
        .query_row(
            "SELECT project_id FROM discussions WHERE id=?1",
            [discussion_id],
            |row| row.get(0),
        )
        .map_err(anyhow::Error::from)?;
    Ok(Some(QuestionResumeLaunch {
        question_id: question.id,
        run_id,
        workflow_id: resume.workflow_id,
        variables: resume.variables,
        source_run_id: source.identity.run_id,
        discussion_id: discussion_id.to_string(),
        project_id,
    }))
}

pub fn finish_resume(
    conn: &Connection,
    question_id: &str,
    run_id: &str,
    result: Result<(), &str>,
) -> Result<()> {
    let (state, error) = match result {
        Ok(()) => ("launched", None),
        Err(error) => ("failed", Some(error)),
    };
    conn.execute(
        "UPDATE discussion_questions SET resume_state=?3, resume_error=?4, updated_at=?5
          WHERE id=?1 AND resume_run_id=?2 AND resume_state='launching'",
        params![question_id, run_id, state, error, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Say something about a question without deciding it: a question back, a
/// missing fact, a caveat. The asker receives it like an answer, through the
/// same targeting and dispatch, and the question stays pending.
pub fn comment(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
    request: &CommentDiscussionQuestionRequest,
    author_pseudo: &str,
    author_avatar_email: Option<&str>,
) -> Result<DiscussionQuestion, AnswerError> {
    if !nonempty_bounded(&request.idempotency_key, 128) {
        return Err(AnswerError::Invalid(
            "A bounded idempotency key is required".into(),
        ));
    }
    let text = request.text.trim();
    if text.is_empty() || text.chars().count() > 8000 {
        return Err(AnswerError::Invalid(
            "A comment needs between 1 and 8000 characters".into(),
        ));
    }
    let question = list(conn, discussion_id)?
        .questions
        .into_iter()
        .find(|q| q.id == question_id)
        .ok_or(AnswerError::NotFound)?;
    if question.state != DiscussionQuestionState::Pending {
        return Err(AnswerError::Conflict);
    }
    let message_id = format!("question-comment:{question_id}:{}", request.idempotency_key);
    let already: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1)",
            [&message_id],
            |r| r.get(0),
        )
        .map_err(anyhow::Error::from)?;
    if already {
        return Ok(question);
    }
    conn.execute_batch("SAVEPOINT comment_discussion_question")
        .map_err(anyhow::Error::from)?;
    let content = format!(
        "Commentaire sur l'arbitrage — {}\n\n{text}\n\nCe n'est pas une décision : la question reste en attente.",
        question.question
    );
    let published = publish_human_resolution(
        conn,
        discussion_id,
        &question,
        &message_id,
        content,
        &message_id,
        Utc::now(),
        author_pseudo,
        author_avatar_email,
        true,
    );
    match published {
        Ok(()) => {
            conn.execute_batch("RELEASE comment_discussion_question")
                .map_err(anyhow::Error::from)?;
            Ok(question)
        }
        Err(error) => {
            let _ = conn.execute_batch(
                "ROLLBACK TO comment_discussion_question; RELEASE comment_discussion_question",
            );
            Err(error)
        }
    }
}

/// Refuse an arbitration instead of answering it.
///
/// Recorded as a resolution, not a deletion: the row keeps its author, its
/// reason and its timestamp, and the asker receives the refusal through the
/// same dispatch as an answer. A question the human will not arbitrate is a
/// decision the agent has to act on — leaving it pending, or deleting it
/// silently, both leave the agent waiting on something that will never come.
pub fn decline(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
    request: &DeclineDiscussionQuestionRequest,
    author_pseudo: &str,
    author_avatar_email: Option<&str>,
) -> Result<DiscussionQuestion, AnswerError> {
    if !nonempty_bounded(&request.idempotency_key, 128) {
        return Err(AnswerError::Invalid(
            "A bounded idempotency key is required".into(),
        ));
    }
    conn.execute_batch("SAVEPOINT decline_discussion_question")
        .map_err(anyhow::Error::from)?;
    let outcome = decline_inner(
        conn,
        discussion_id,
        question_id,
        request,
        author_pseudo,
        author_avatar_email,
    );
    match outcome {
        Ok(question) => {
            conn.execute_batch("RELEASE decline_discussion_question")
                .map_err(anyhow::Error::from)?;
            Ok(question)
        }
        Err(error) => {
            let _ = conn.execute_batch(
                "ROLLBACK TO decline_discussion_question; RELEASE decline_discussion_question",
            );
            Err(error)
        }
    }
}

fn decline_inner(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
    request: &DeclineDiscussionQuestionRequest,
    author_pseudo: &str,
    author_avatar_email: Option<&str>,
) -> Result<DiscussionQuestion, AnswerError> {
    let mut question = list(conn, discussion_id)?
        .questions
        .into_iter()
        .find(|q| q.id == question_id)
        .ok_or(AnswerError::NotFound)?;
    let reason = request
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    if reason.as_ref().is_some_and(|r| r.chars().count() > 8000) {
        return Err(AnswerError::Invalid(
            "A refusal reason is limited to 8000 characters".into(),
        ));
    }
    match question.state {
        DiscussionQuestionState::Declined => {
            // Idempotent: the same key refusing an already-refused question is
            // the client retrying, not a conflict.
            let key: String = conn
                .query_row(
                    "SELECT answer_idempotency_key FROM discussion_questions WHERE id=?1",
                    [question_id],
                    |r| r.get(0),
                )
                .map_err(anyhow::Error::from)?;
            if key == request.idempotency_key {
                return Ok(question);
            }
            return Err(AnswerError::Conflict);
        }
        // An answered question is decided. Refusing it afterwards would rewrite
        // a decision the agent already received.
        DiscussionQuestionState::Answered => return Err(AnswerError::Conflict),
        DiscussionQuestionState::Pending => {}
    }
    let now = Utc::now();
    let message_id = format!("question-decline:{question_id}");
    let record = DiscussionQuestionAnswer {
        selected_option_ids: Vec::new(),
        item_answers: Vec::new(),
        text: reason.clone(),
        author_pseudo: author_pseudo.to_string(),
        answered_at: now.to_rfc3339(),
        message_id: message_id.clone(),
    };
    let content = format!(
        "Arbitrage refusé — {}\n\n{}",
        question.question,
        reason.as_deref().unwrap_or(
            "Aucune raison donnée. Ne bloque pas sur cette question : poursuis \
                        sans elle, ou reformule-la si la décision reste nécessaire."
        )
    );
    // Refusing a ceiling question is keeping the partial answer.
    let ceiling = super::discussion_ceiling_requests::decide(
        conn,
        discussion_id,
        &question.key,
        &[super::discussion_ceiling_requests::OPTION_STOP.to_string()],
    )?;
    publish_human_resolution(
        conn,
        discussion_id,
        &question,
        &message_id,
        content,
        &format!("question-decline:{question_id}"),
        now,
        author_pseudo,
        author_avatar_email,
        ceiling == super::discussion_ceiling_requests::Answered::NotACeiling,
    )?;
    conn.execute(
        "UPDATE discussion_questions SET state='declined', answer_json=?2, \
         answer_idempotency_key=?3, updated_at=?4 WHERE id=?1 AND state='pending'",
        params![
            question_id,
            serde_json::to_string(&record).map_err(anyhow::Error::from)?,
            request.idempotency_key,
            record.answered_at
        ],
    )
    .map_err(anyhow::Error::from)?;
    question.state = DiscussionQuestionState::Declined;
    // Mirror what the row now holds, so a replay that re-reads the database
    // returns exactly this value.
    question.updated_at = record.answered_at.clone();
    question.answer = Some(record);
    Ok(question)
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
    let mut item_answers = request.item_answers.clone();
    item_answers.sort_by(|left, right| left.item_id.cmp(&right.item_id));
    let item_ids = item_answers
        .iter()
        .map(|answer| answer.item_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if selected.windows(2).any(|w| w[0] == w[1])
        || (!question.multiple && selected.len() > 1)
        || selected
            .iter()
            .any(|id| !question.options.iter().any(|o| &o.id == id))
        || (!question.items.is_empty()
            && (item_answers.len() != question.items.len()
                || item_ids.len() != item_answers.len()
                || item_answers.iter().any(|answer| {
                    !question.items.iter().any(|item| item.id == answer.item_id)
                        || !question
                            .options
                            .iter()
                            .any(|option| option.id == answer.selected_option_id)
                })))
        || (question.items.is_empty() && !item_answers.is_empty())
        || (!question.items.is_empty() && !selected.is_empty())
        || (selected.is_empty() && item_answers.is_empty() && text.is_none())
        || text.as_ref().is_some_and(|s| s.chars().count() > 8000)
    {
        return Err(AnswerError::Invalid(
            "Select valid options or provide a non-empty answer (maximum 8000 characters)".into(),
        ));
    }
    // A refused question is decided. Answering it afterwards would publish a
    // second resolution and a second dispatch for one decision.
    if question.state == DiscussionQuestionState::Declined {
        return Err(AnswerError::Conflict);
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
            && question.answer.as_ref().is_some_and(|a| {
                a.selected_option_ids == selected
                    && a.item_answers == item_answers
                    && a.text == text
            })
        {
            return Ok(question);
        }
        return Err(AnswerError::Conflict);
    }
    let now = Utc::now();
    let answer = DiscussionQuestionAnswer {
        selected_option_ids: selected.clone(),
        item_answers: item_answers.clone(),
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
    let item_labels = item_answers
        .iter()
        .filter_map(|answer| {
            let item = question
                .items
                .iter()
                .find(|item| item.id == answer.item_id)?;
            let option = question
                .options
                .iter()
                .find(|option| option.id == answer.selected_option_id)?;
            Some(format!("- {}: {}", item.label, option.label))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let chosen = match (labels.is_empty(), item_labels.is_empty()) {
        (false, false) => format!("{labels}\n{item_labels}"),
        (false, true) => labels,
        (true, false) => item_labels,
        (true, true) => String::new(),
    };
    let mut content = format!(
        "Décision humaine — {}\n\n{}{}{}",
        question.question,
        chosen,
        if chosen.is_empty() || text.is_none() {
            ""
        } else {
            "\n\n"
        },
        text.as_deref().unwrap_or("")
    );

    let audit_resolutions = if let Some(td_id) = question
        .task_ref
        .as_deref()
        .and_then(|task_ref| task_ref.strip_prefix("audit-td:"))
    {
        let option_id = selected
            .first()
            .ok_or_else(|| AnswerError::Invalid("Choose one audit TD outcome".into()))?;
        let action = crate::core::audit_validation::TdResolutionAction::from_option_id(option_id)
            .ok_or_else(|| AnswerError::Invalid("Unknown audit TD outcome".into()))?;
        Some(vec![crate::core::audit_validation::TdResolution {
            td_id: td_id.to_string(),
            action,
        }])
    } else if question.task_ref.as_deref() == Some("audit-td-batch") {
        let mut resolutions = Vec::with_capacity(item_answers.len());
        for item_answer in &item_answers {
            let action = crate::core::audit_validation::TdResolutionAction::from_option_id(
                &item_answer.selected_option_id,
            )
            .ok_or_else(|| AnswerError::Invalid("Unknown audit TD outcome".into()))?;
            resolutions.push(crate::core::audit_validation::TdResolution {
                td_id: item_answer.item_id.clone(),
                action,
            });
        }
        Some(resolutions)
    } else {
        None
    };
    if let Some(resolutions) = audit_resolutions.as_ref() {
        crate::core::audit_validation::apply_td_resolutions(
            conn,
            discussion_id,
            resolutions,
            text.as_deref(),
        )
        .map_err(|error| AnswerError::Invalid(error.to_string()))?;
    }
    let ceiling =
        super::discussion_ceiling_requests::decide(conn, discussion_id, &question.key, &selected)?;
    if let super::discussion_ceiling_requests::Answered::Decided(
        super::discussion_ceiling_requests::Decision::Grant
        | super::discussion_ceiling_requests::Decision::Unlimited,
    ) = ceiling
    {
        content.push_str(
            "\n\nKronn a relevé le plafond pour ton prochain tour. Reprends là où ta réponse \
             précédente s'est arrêtée, en commençant par ce qu'elle dit manquer : ne refais pas \
             ce qui y est déjà établi.",
        );
    }
    let dispatch = if audit_resolutions.is_some() {
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM discussion_questions WHERE discussion_id=?1 AND state='pending'",
                [discussion_id],
                |row| row.get(0),
            )
            .map_err(anyhow::Error::from)?;
        pending == 1
    } else {
        ceiling
            != super::discussion_ceiling_requests::Answered::Decided(
                super::discussion_ceiling_requests::Decision::Stop,
            )
    };
    publish_human_resolution(
        conn,
        discussion_id,
        &question,
        &answer.message_id,
        content,
        &format!("question-answer:{question_id}"),
        now,
        author_pseudo,
        author_avatar_email,
        dispatch,
    )?;
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
