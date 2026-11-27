use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
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
    /// KT-1348 — the TDs a partial audit card could not record. Written by
    /// Kronn only (an agent's fence carrying it is refused), so it never mixes
    /// with the agent's `context` nor eats its length budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    td_errors: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
struct QuestionResumeSpec {
    workflow_id: String,
    #[serde(default)]
    variables: HashMap<String, String>,
}

crate::debug_without_secrets!(QuestionResumeSpec { workflow_id; masked: variables });

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
    /// KT-1955 — a human refused the card without a message (« Refuser » with
    /// an empty field): it was settled elsewhere, or no longer deserves a
    /// decision. The reader sees a refusal; unlike `Declined`, nothing reaches
    /// the asker and nothing resumes: no message, no dispatch, no workflow
    /// launch. The card carries no answer, only who closed it and when.
    Cancelled,
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionQuestionResume {
    pub workflow_id: String,
    #[ts(type = "Record<string, string>")]
    pub variables: HashMap<String, String>,
    pub state: DiscussionQuestionResumeState,
    pub run_id: Option<String>,
    pub error: Option<String>,
}

crate::debug_without_secrets!(DiscussionQuestionResume { workflow_id, state, run_id, error; masked: variables });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionQuestionWorkflowStep {
    #[serde(flatten)]
    pub identity: crate::models::WorkflowStepIdentity,
    pub active: bool,
}

/// Who refused a card without a message, and when (KT-1955).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionQuestionCancellation {
    pub author_pseudo: String,
    pub cancelled_at: String,
    /// The card was the last pending one of a message whose step had ended,
    /// and the human's answers to its other cards — which waited for it —
    /// were then dispatched to an agent, the recap naming this card as
    /// refused without a message (KT-1401). The refusal alone wakes nobody.
    #[serde(default)]
    pub relayed_answers: bool,
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
    /// The TDs a partial audit card could not record, with the reason for each;
    /// cleared once the card is fully answered.
    pub td_errors: Option<String>,
    pub options: Vec<DiscussionQuestionOption>,
    pub items: Vec<DiscussionQuestionItem>,
    pub multiple: bool,
    pub recommended_option_ids: Vec<String>,
    pub task_ref: Option<String>,
    pub requester_workflow_step: Option<DiscussionQuestionWorkflowStep>,
    pub resume: Option<DiscussionQuestionResume>,
    pub state: DiscussionQuestionState,
    pub answer: Option<DiscussionQuestionAnswer>,
    /// Set only when `state` is `cancelled`; such a card never has an answer.
    pub cancellation: Option<DiscussionQuestionCancellation>,
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

const QUESTION_MAX: usize = 1000;
const CONTEXT_MAX: usize = 4000;
const LABEL_MAX: usize = 250;
const DESCRIPTION_MAX: usize = 1000;

/// `text` cut to at most `max` characters, at a word boundary when one lies in
/// the second half of the cut, with an ellipsis saying it was cut.
fn cut_at_word(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max - 1).collect();
    let on_boundary = text.chars().nth(max - 1).is_some_and(char::is_whitespace);
    let kept = if on_boundary {
        head.as_str()
    } else {
        match head.rfind(char::is_whitespace) {
            Some(at) if at >= head.len() / 2 => &head[..at],
            _ => head.as_str(),
        }
    };
    format!("{}…", kept.trim_end())
}

/// A label too long for its bound is cut at a word and its full text moves to
/// the description, ahead of whatever the agent wrote there. An over-long
/// description has no further place to go: it is cut.
fn spill_label(label: &mut String, description: &mut Option<String>) {
    if label.chars().count() > LABEL_MAX {
        let full = label.trim().to_string();
        *label = cut_at_word(&full, LABEL_MAX);
        *description = Some(match description.take().filter(|d| !d.trim().is_empty()) {
            Some(existing) => format!("{full}\n\n{existing}"),
            None => full,
        });
    }
    if let Some(text) = description.as_mut() {
        if text.chars().count() > DESCRIPTION_MAX {
            *text = cut_at_word(text, DESCRIPTION_MAX);
        }
    }
}

/// KT-1401 — a card whose only fault is a text longer than its bound is
/// recorded with that text shortened, never dropped: a card the human sees but
/// cannot answer is worse than a shortened one. The full text of a label or a
/// question stays readable (description / context).
fn normalize_lengths(spec: &mut QuestionSpec) {
    if spec.question.chars().count() > QUESTION_MAX {
        let full = spec.question.trim().to_string();
        spec.question = cut_at_word(&full, QUESTION_MAX);
        spec.context = Some(match spec.context.take().filter(|c| !c.trim().is_empty()) {
            Some(existing) => format!("{full}\n\n{existing}"),
            None => full,
        });
    }
    if let Some(context) = spec.context.as_mut() {
        if context.chars().count() > CONTEXT_MAX {
            *context = cut_at_word(context, CONTEXT_MAX);
        }
    }
    for option in &mut spec.options {
        spill_label(&mut option.label, &mut option.description);
    }
    for item in &mut spec.items {
        spill_label(&mut item.label, &mut item.description);
    }
}

/// Parse and validate one `kronn-question` fence. The error is the reason the
/// card cannot be recorded, in words the agent can act on.
fn parse_spec(body: &str) -> Result<QuestionSpec, String> {
    if body.len() > 24_000 {
        return Err("the fence is longer than 24000 characters".into());
    }
    let mut spec: QuestionSpec = serde_json::from_str(body)
        .map_err(|e| cut_at_word(&format!("the fence is not a valid question: {e}"), 300))?;
    normalize_lengths(&mut spec);
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
    let check = |ok: bool, reason: &str| ok.then_some(()).ok_or_else(|| reason.to_string());
    check(spec.version == 1, "`version` must be the number 1")?;
    check(
        spec.td_errors.is_none(),
        "`td_errors` is written by Kronn only",
    )?;
    check(
        stable_key(&spec.key),
        "`key` must be 1-100 characters among letters, digits, `-`, `_` and `.`",
    )?;
    check(
        !spec.question.trim().is_empty(),
        "`question` must not be empty",
    )?;
    check(
        spec.task_ref
            .as_ref()
            .is_none_or(|s| nonempty_bounded(s, 100)),
        "`task_ref` must be 1-100 characters",
    )?;
    check(spec.options.len() <= 8, "at most 8 options are allowed")?;
    check(ids.len() == spec.options.len(), "option ids must be unique")?;
    check(
        spec.options
            .iter()
            .all(|o| stable_key(&o.id) && !o.label.trim().is_empty()),
        "every option needs a stable `id` (letters, digits, `-`, `_`, `.`) and a non-empty `label`",
    )?;
    check(spec.items.len() <= 8, "at most 8 items are allowed")?;
    check(
        item_ids.len() == spec.items.len(),
        "item ids must be unique",
    )?;
    check(
        spec.items
            .iter()
            .all(|item| stable_key(&item.id) && !item.label.trim().is_empty()),
        "every item needs a stable `id` (letters, digits, `-`, `_`, `.`) and a non-empty `label`",
    )?;
    check(
        spec.items.is_empty() || !spec.options.is_empty(),
        "items need options to choose from",
    )?;
    check(
        spec.recommended_option_ids
            .iter()
            .all(|id| ids.contains(id.as_str())),
        "`recommended_option_ids` must name existing options",
    )?;
    check(
        spec.multiple || spec.recommended_option_ids.len() <= 1,
        "only one option can be recommended unless `multiple` is true",
    )?;
    check(
        spec.resume.as_ref().is_none_or(|resume| {
            nonempty_bounded(&resume.workflow_id, 128)
                && resume.variables.len() <= 16
                && resume
                    .variables
                    .iter()
                    .all(|(key, value)| valid_resume_variable(key, value))
        }),
        "`resume` is malformed",
    )?;
    Ok(spec)
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
pub fn strip_context_blocks(content: &str) -> String {
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

/// KT-2085 — round n ≥ 2 of a key is stored as `key~n` (no schema change);
/// `stable_key` refuses `~`, so no key an agent writes can take that form.
const ROUND_SEPARATOR: char = '~';

fn round_of(stored_key: &str) -> u32 {
    stored_key
        .rsplit_once(ROUND_SEPARATOR)
        .and_then(|(_, round)| round.parse().ok())
        .unwrap_or(1)
}

/// The latest round of `key` in the discussion: its number, state and message.
fn latest_round(
    conn: &Connection,
    discussion_id: &str,
    key: &str,
) -> Result<Option<(u32, String, String)>> {
    let mut statement = conn.prepare(
        "SELECT question_key, state, source_message_id FROM discussion_questions
          WHERE discussion_id=?1
            AND (question_key=?2 OR substr(question_key, 1, length(?2) + 1)=?2 || ?3)",
    )?;
    let separator = ROUND_SEPARATOR.to_string();
    let rows = statement.query_map(params![discussion_id, key, separator], |row| {
        Ok((
            round_of(&row.get::<_, String>(0)?),
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut latest: Option<(u32, String, String)> = None;
    for row in rows {
        let row = row?;
        if latest.as_ref().is_none_or(|(round, ..)| row.0 > *round) {
            latest = Some(row);
        }
    }
    Ok(latest)
}

/// The states a round is over in: only then may its key be asked again.
/// `cancelled` is the state the 0.15.4 UI adds.
const FINISHED_STATES: [&str; 3] = ["answered", "declined", "cancelled"];

/// Why `spec` cannot open a round of its key (the first, or the one after
/// `latest`: round, state, message); `None` when it can.
fn new_round_refusal(
    conn: &Connection,
    discussion_id: &str,
    spec: &QuestionSpec,
    latest: Option<&(u32, String, String)>,
    from_kronn_elevation: bool,
) -> Result<Option<String>> {
    let key = &spec.key;
    // A ceiling card rides the agent's own message: Kronn's is the one it recorded first.
    let prefix = super::discussion_ceiling_requests::QUESTION_KEY_PREFIX;
    if key.starts_with(prefix) && !ceiling_recorded(conn, discussion_id, key)? {
        return Ok(Some(format!(
            "keys starting with `{prefix}` are Kronn's ceiling cards; use another key"
        )));
    }
    // An elevation answer grants only on Kronn's message: no other holds the key, round 1 included.
    let owner = if key.starts_with(crate::db::orchestration::DOCKER_ELEVATION_KEY_PREFIX)
        && !from_kronn_elevation
    {
        Some(ELEVATION_OWNED)
    } else if latest.is_some() {
        kronn_card_owner(conn, discussion_id, spec)?
    } else {
        None
    };
    let Some((_, state, source)) = latest else {
        return Ok(owner.map(|owner| format!("key `{key}` {owner}")));
    };
    let state = state.as_str();
    let waiting = format!(
        "key `{key}` is still waiting for an answer in message {source}: wait for the human to \
         answer, decline or cancel it"
    );
    // A Kronn card is never sent to a new key: nothing would read that answer.
    Ok(match owner {
        Some(owner) if state == "pending" => Some(format!("{waiting}; it {owner}")),
        Some(BRIEFING_OWNED) if state == "answered" => Some(format!(
            "key `{key}` was answered in message {source}; a briefing card is not asked again; \
             do not repost it under any key"
        )),
        // A declined card cannot be answered: the form or a new launch is the way on.
        Some(BRIEFING_OWNED) if matches!(state, "declined" | "cancelled") => Some(format!(
            "key `{key}` was {state} in message {source}, and a {state} briefing card is not \
             asked again: the human fills in the briefing form, or a new audit launch gives a \
             new set; do not repost it under any key"
        )),
        Some(owner) => Some(format!("key `{key}` {owner}")),
        None if FINISHED_STATES.contains(&state) => None,
        None if state == "pending" => Some(format!("{waiting}, or use a new key")),
        None => Some(format!(
            "key `{key}` is `{state}` in message {source}, neither answered, declined nor \
             cancelled: it cannot be asked again; use a new key"
        )),
    })
}

// Whose card a refused key is, read after "key `…` " or "it ".
const CEILING_OWNED: &str = "belongs to a ceiling card Kronn posted, the only card that moves \
     the budget. Kronn posts this card itself and asks again when needed: do not repost it under \
     any key";
const DISPUTE_OWNED: &str = "belongs to a cross-agent dispute card Kronn posted, the only card \
     that decides the finding. Kronn posts this card itself and asks again when needed: do not \
     repost it under any key";
const ELEVATION_OWNED: &str = "is reserved to the Docker elevation cards Kronn posts, the only \
     cards that grant Docker. Kronn posts this card itself and asks again when needed: do not \
     post it under any key";
const BRIEFING_OWNED: &str = "belongs to a briefing card, and a briefing set is read once: the \
     human answers through the set they already have, so do not start a new set to remind them; \
     do not repost it under any key";

/// Whether Kronn recorded the ceiling question `key` of `discussion_id` (KT-2085).
fn ceiling_recorded(conn: &Connection, discussion_id: &str, key: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM discussion_ceiling_requests
                        WHERE discussion_id=?1 AND question_key=?2)",
        params![discussion_id, key],
        |row| row.get(0),
    )?)
}

/// A card Kronn posted binds its effect to round 1 (a ceiling budget, a
/// disputed finding, a briefing set read once): a later round would be an
/// answer that moves nothing, so its key is never asked again.
fn kronn_card_owner(
    conn: &Connection,
    discussion_id: &str,
    spec: &QuestionSpec,
) -> Result<Option<&'static str>> {
    let key = &spec.key;
    if key.starts_with(super::discussion_ceiling_requests::QUESTION_KEY_PREFIX)
        && ceiling_recorded(conn, discussion_id, key)?
    {
        return Ok(Some(CEILING_OWNED));
    }
    if key.starts_with(crate::core::cross_audit::QUESTION_KEY_PREFIX)
        && super::audit_cross::dispute_target(conn, discussion_id, key)?.is_some()
    {
        return Ok(Some(DISPUTE_OWNED));
    }
    // Round 1 can be gone with its message while a later round stays.
    let first_task_ref: Option<String> = conn
        .query_row(
            "SELECT json_extract(payload_json, '$.task_ref') FROM discussion_questions
              WHERE discussion_id=?1 AND question_key=?2",
            params![discussion_id, key],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let briefing = |task_ref: Option<&str>| {
        task_ref.is_some_and(|r| r.starts_with(crate::api::audit::briefing_cards::TASK_REF_PREFIX))
    };
    if briefing(first_task_ref.as_deref()) || briefing(spec.task_ref.as_deref()) {
        return Ok(Some(BRIEFING_OWNED));
    }
    Ok(None)
}

/// Record the cards of a message's `kronn-question` fences. A fence that
/// cannot be recorded is returned with its reason — never skipped silently
/// (KT-1401) — for the caller to report on the message.
pub fn ingest_message_questions(
    conn: &Connection,
    discussion_id: &str,
    message_id: &str,
    content: &str,
) -> Result<Vec<crate::core::anti_halluc::InvalidCard>> {
    let mut invalid = Vec::new();
    if !content.contains("kronn-question") {
        return Ok(invalid);
    }
    // Never ingest from folded technical context — see strip_context_blocks.
    let content = &strip_context_blocks(content);
    if !content.contains("kronn-question") {
        return Ok(invalid);
    }
    // A peer provider must not inherit the room's unrelated default connection.
    // Dispatch provenance, attached after message insertion, takes precedence at answer time.
    let connection_id: Option<String> = conn.query_row(
        "SELECT CASE WHEN m.agent_type=d.agent THEN d.connection_id ELSE NULL END
         FROM messages m JOIN discussions d ON d.id=m.discussion_id WHERE m.id=?1 AND d.id=?2",
        params![message_id, discussion_id],
        |row| row.get(0),
    )?;
    let from_kronn_elevation =
        crate::db::orchestration::is_kronn_docker_elevation_message(conn, message_id)?;
    let mut keys_in_message = std::collections::HashSet::new();
    for (index, body) in question_fences(content).into_iter().take(64).enumerate() {
        let spec = match parse_spec(&body) {
            Ok(spec) => spec,
            Err(reason) => {
                let key = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("key")?.as_str().map(|k| cut_at_word(k, 100)));
                invalid.push(crate::core::anti_halluc::InvalidCard { key, reason });
                continue;
            }
        };
        // One card per key and message, checked before the replay test so a
        // replay refuses the same fences as the first ingest.
        if !keys_in_message.insert(spec.key.clone()) {
            invalid.push(crate::core::anti_halluc::InvalidCard {
                key: Some(spec.key.clone()),
                reason: format!(
                    "key `{}` appears twice in this message: one card per key, give the other \
                     card its own key",
                    spec.key
                ),
            });
            continue;
        }
        // A replayed message: this fence already has its card.
        let recorded: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM discussion_questions
                            WHERE source_message_id=?1 AND fence_index=?2)",
            params![message_id, index as i64],
            |row| row.get(0),
        )?;
        if recorded {
            continue;
        }
        // A key asked again once its round is over opens a new round; while a
        // round waits, the same key would be a second card nobody can tell apart.
        let latest = latest_round(conn, discussion_id, &spec.key)?;
        if let Some(reason) = new_round_refusal(
            conn,
            discussion_id,
            &spec,
            latest.as_ref(),
            from_kronn_elevation,
        )? {
            invalid.push(crate::core::anti_halluc::InvalidCard {
                key: Some(spec.key.clone()),
                reason,
            });
            continue;
        }
        let stored_key = match latest {
            None => spec.key.clone(),
            Some((round, ..)) => format!("{}{ROUND_SEPARATOR}{}", spec.key, round + 1),
        };
        let now = Utc::now().to_rfc3339();
        let inserted = conn.execute(
            "INSERT INTO discussion_questions (id, discussion_id, source_message_id, fence_index,
                 question_key, payload_json, requester_connection_id, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)
             ON CONFLICT DO NOTHING",
            params![
                format!("question:{message_id}:{index}"),
                discussion_id,
                message_id,
                index as i64,
                stored_key,
                serde_json::to_string(&spec)?,
                connection_id,
                now
            ],
        )?;
        if inserted == 0 {
            invalid.push(crate::core::anti_halluc::InvalidCard {
                key: Some(spec.key.clone()),
                reason: format!("key `{}` is already taken in this discussion", spec.key),
            });
        }
    }
    Ok(invalid)
}

/// Put the fences `ingest_message_questions` refused on the message's lint
/// report, where the UI shows them as an invalid card instead of a dead one.
pub fn report_invalid_cards(
    conn: &Connection,
    message_id: &str,
    existing: Option<&crate::core::anti_halluc::LintReport>,
    invalid: Vec<crate::core::anti_halluc::InvalidCard>,
) -> Result<()> {
    if invalid.is_empty() {
        return Ok(());
    }
    let mut report = existing
        .cloned()
        .unwrap_or_else(crate::core::anti_halluc::LintReport::empty);
    report.invalid_cards = invalid;
    conn.execute(
        "UPDATE messages SET lint_report=?2 WHERE id=?1",
        params![message_id, serde_json::to_string(&report)?],
    )?;
    Ok(())
}

pub fn list(conn: &Connection, discussion_id: &str) -> Result<DiscussionQuestionList> {
    let mut statement = conn.prepare(
        "SELECT id, source_message_id, fence_index, payload_json, state, answer_json, created_at, updated_at,
                resume_state, resume_run_id, resume_error, cancelled_by, cancelled_at
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
            r.get::<_, Option<String>>(11)?,
            r.get::<_, Option<String>>(12)?,
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
            cancelled_by,
            cancelled_at,
        ) = row?;
        let spec: QuestionSpec = serde_json::from_str(&payload)?;
        let state = match state.as_str() {
            "pending" => DiscussionQuestionState::Pending,
            "answered" => DiscussionQuestionState::Answered,
            "declined" => DiscussionQuestionState::Declined,
            "cancelled" => DiscussionQuestionState::Cancelled,
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
        let cancellation = match (state, cancelled_by, cancelled_at) {
            (DiscussionQuestionState::Cancelled, Some(author_pseudo), Some(cancelled_at)) => {
                Some(DiscussionQuestionCancellation {
                    author_pseudo,
                    cancelled_at,
                    relayed_answers: conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM agent_dispatch_jobs WHERE trigger_message_id=?1)",
                        [relay_message_id(&id)],
                        |r| r.get(0),
                    )?,
                })
            }
            _ => None,
        };
        questions.push(DiscussionQuestion {
            id,
            discussion_id: discussion_id.to_string(),
            source_message_id,
            fence_index,
            key: spec.key,
            question: spec.question,
            context: spec.context,
            td_errors: spec.td_errors,
            options: spec.options,
            items: spec.items,
            multiple: spec.multiple,
            recommended_option_ids: spec.recommended_option_ids,
            task_ref: spec.task_ref,
            requester_workflow_step,
            resume,
            state,
            answer: answer.map(|a| serde_json::from_str(&a)).transpose()?,
            cancellation,
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

/// Whether the room's own agent takes a resolution when the step that asked has
/// ended, instead of nobody reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoomAgentWake {
    Never,
    /// A comment asks for an answer now; the question stays pending.
    ForComment,
    /// The last pending card of the ended step's message was answered: the
    /// asker will never read the set, the room's agent does (KT-1401).
    ForAnswer,
    /// An earlier card of that message was answered while others are pending:
    /// nothing wakes now, the last answer carries this one to the room's agent.
    Deferred,
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
    // When the step that asked has ended, who reads this resolution now.
    wake: RoomAgentWake,
) -> Result<(), AnswerError> {
    let step_ended = question
        .requester_workflow_step
        .as_ref()
        .is_some_and(|source| !source.active);
    let room_agent = if step_ended && wake != RoomAgentWake::Never && dispatch {
        room_native_agent(conn, discussion_id)?
    } else {
        None
    };
    if step_ended {
        content.push_str(match (room_agent.is_some(), wake) {
            (true, RoomAgentWake::ForAnswer) => {
                "\n\nL'étape qui a posé cette question est terminée : l'agent de la room reprend avec les réponses de ce message."
            }
            (true, RoomAgentWake::Deferred) => {
                "\n\nL'étape qui a posé cette question est terminée : cette réponse sera transmise à l'agent de la room avec la dernière réponse de ce message."
            }
            (true, _) => {
                "\n\nL'étape qui a posé cette question est terminée : l'agent de la room répond. La question reste en attente pour le prochain run."
            }
            (false, _) => {
                "\n\nL'étape qui a posé cette question est terminée ; personne ne lira cette réponse maintenant. Elle sera disponible au prochain run."
            }
        });
    }
    // A deferred answer only takes the notice: the wake belongs to the last one.
    let room_agent = room_agent.filter(|_| wake != RoomAgentWake::Deferred);
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
        session_continuity: None,
        kind: Default::default(),
        model: None,
        lint_report: None,
    };
    let sort_order = super::discussions::insert_message(conn, discussion_id, &message)?;
    if !dispatch {
        return Ok(());
    }
    if let Some((agent, connection_id)) = room_agent {
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

/// One resolved card of a message, as the room's agent reads it with the
/// others: the question, then what the human chose or said.
fn recap_line(question: &DiscussionQuestion) -> String {
    if question.state == DiscussionQuestionState::Cancelled {
        return format!(
            "- {} → refusée sans message par un humain",
            question.question
        );
    }
    let Some(answer) = question.answer.as_ref() else {
        return String::new();
    };
    let label_of = |id: &'_ str| -> String {
        question
            .options
            .iter()
            .find(|o| o.id == id)
            .map_or(id, |o| o.label.as_str())
            .to_string()
    };
    let mut parts = answer
        .selected_option_ids
        .iter()
        .map(|id| label_of(id))
        .collect::<Vec<_>>();
    parts.extend(answer.item_answers.iter().map(|a| {
        let item = question
            .items
            .iter()
            .find(|i| i.id == a.item_id)
            .map_or(a.item_id.as_str(), |i| i.label.as_str());
        format!("{item}: {}", label_of(&a.selected_option_id))
    }));
    if let Some(text) = answer.text.as_deref() {
        parts.push(text.to_string());
    }
    let outcome = if question.state == DiscussionQuestionState::Declined {
        format!(
            "refusée{}",
            if parts.is_empty() {
                String::new()
            } else {
                format!(" — {}", parts.join(" ; "))
            }
        )
    } else {
        parts.join(" ; ")
    };
    format!("- {} → {outcome}", question.question)
}

/// The room's own native agent and its connection, unless the room has none.
fn room_native_agent(
    conn: &Connection,
    discussion_id: &str,
) -> Result<Option<(crate::models::AgentType, Option<String>)>, AnswerError> {
    if super::discussions::disc_is_no_agent(conn, discussion_id)? {
        return Ok(None);
    }
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT agent, connection_id FROM discussions WHERE id=?1",
            [discussion_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(anyhow::Error::from)?;
    let Some((agent, connection_id)) = row else {
        return Ok(None);
    };
    let agent = super::discussions::parse_agent_type(&agent).map_err(anyhow::Error::from)?;
    Ok(Some((agent, connection_id)))
}

/// Reserved launch data for one question resolution. Claiming stores the run
/// id before any asynchronous work, so retries cannot start a second run.
#[derive(Clone)]
pub struct QuestionResumeLaunch {
    pub question_id: String,
    pub run_id: String,
    pub workflow_id: String,
    pub variables: HashMap<String, String>,
    pub source_run_id: String,
    pub discussion_id: String,
    pub project_id: Option<String>,
}

crate::debug_without_secrets!(QuestionResumeLaunch { question_id, run_id, workflow_id, source_run_id, discussion_id, project_id; masked: variables });

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
    // A card refused without a message decided nothing (KT-1955): what it
    // would resume stays parked on it, never launched as if it had been
    // answered.
    if matches!(
        question.state,
        DiscussionQuestionState::Pending | DiscussionQuestionState::Cancelled
    ) {
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
        RoomAgentWake::ForComment,
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
/// With a message, the refusal is recorded as a resolution, not a deletion:
/// the row keeps its author, its reason and its timestamp, and the asker
/// receives the refusal through the same dispatch as an answer.
///
/// Without one — `reason` absent, empty or only whitespace — the card is
/// closed and nothing else happens (KT-1955): waking an agent for a refusal
/// that says nothing is useless. See [`close_without_message`].
pub fn decline(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
    request: &DeclineDiscussionQuestionRequest,
    author_pseudo: &str,
    author_avatar_email: Option<&str>,
) -> Result<DiscussionQuestion, AnswerError> {
    decline_reporting_relay(
        conn,
        discussion_id,
        question_id,
        request,
        author_pseudo,
        author_avatar_email,
    )
    .map(|(question, _)| question)
}

/// [`decline`], also saying whether a refusal without a message just relayed
/// the earlier answers of its message to the room's agent — the one case
/// where it enqueues a dispatch the caller must notify.
pub fn decline_reporting_relay(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
    request: &DeclineDiscussionQuestionRequest,
    author_pseudo: &str,
    author_avatar_email: Option<&str>,
) -> Result<(DiscussionQuestion, bool), AnswerError> {
    if !nonempty_bounded(&request.idempotency_key, 128) {
        return Err(AnswerError::Invalid(
            "A bounded idempotency key is required".into(),
        ));
    }
    if is_blank_refusal(request) {
        return close_without_message(
            conn,
            discussion_id,
            question_id,
            author_pseudo,
            author_avatar_email,
        );
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
            Ok((question, false))
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
        // a decision the agent already received. One refused without a
        // message is closed.
        DiscussionQuestionState::Answered | DiscussionQuestionState::Cancelled => {
            return Err(AnswerError::Conflict)
        }
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
    // A blank refusal never reaches here: `decline` closes it silently.
    let content = format!(
        "Arbitrage refusé — {}\n\n{}",
        question.question,
        reason.as_deref().unwrap_or_default()
    );
    let kronn_key = stored_key(conn, question_id)?;
    if let Some(mut target) = cross_dispute_of(conn, discussion_id, &kronn_key)? {
        crate::core::cross_audit_decision::apply(
            conn,
            &mut target,
            crate::core::cross_audit::HumanDecision::Declined,
        )
        .map_err(AnswerError::Invalid)?;
    }
    // Refusing a ceiling question is keeping the partial answer.
    let ceiling = super::discussion_ceiling_requests::decide(
        conn,
        discussion_id,
        &kronn_key,
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
        RoomAgentWake::Never,
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

/// The key a card is stored under. Kronn posts a ceiling or dispute card once,
/// as round 1, and ingest refuses a later round of it; one stored anyway moves
/// neither a budget nor a finding.
fn stored_key(conn: &Connection, question_id: &str) -> Result<String, AnswerError> {
    Ok(conn
        .query_row(
            "SELECT question_key FROM discussion_questions WHERE id=?1",
            [question_id],
            |row| row.get(0),
        )
        .map_err(anyhow::Error::from)?)
}

/// A refusal that says nothing: `reason` absent, empty or only whitespace.
pub fn is_blank_refusal(request: &DeclineDiscussionQuestionRequest) -> bool {
    request
        .reason
        .as_deref()
        .is_none_or(|reason| reason.trim().is_empty())
}

/// A human refused a pending card without a message (KT-1955): close it and
/// do nothing else.
///
/// The refusal is told to nobody: no message about it, no target, no
/// dispatch, no ceiling or dispute decision, no workflow resume. Whatever was
/// parked on the card stays parked, and an agent reading the key gets
/// `cancelled` with no answer. Only the human UI reaches this: `decline` is on
/// no bridge-token list and no agent tool leads here.
///
/// One exception, owed to earlier answers rather than to this refusal: see
/// [`relay_earlier_answers`]. The returned flag says it happened.
///
/// Repeating it on a closed card returns that card unchanged (a double click,
/// a second tab) and relays nothing again; a card already answered or refused
/// with a message is a conflict.
fn close_without_message(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
    author_pseudo: &str,
    author_avatar_email: Option<&str>,
) -> Result<(DiscussionQuestion, bool), AnswerError> {
    let find = || -> Result<DiscussionQuestion, AnswerError> {
        list(conn, discussion_id)?
            .questions
            .into_iter()
            .find(|q| q.id == question_id)
            .ok_or(AnswerError::NotFound)
    };
    let question = find()?;
    match question.state {
        DiscussionQuestionState::Cancelled => return Ok((question, false)),
        DiscussionQuestionState::Answered | DiscussionQuestionState::Declined => {
            return Err(AnswerError::Conflict)
        }
        DiscussionQuestionState::Pending => {}
    }
    conn.execute_batch("SAVEPOINT close_discussion_question")
        .map_err(anyhow::Error::from)?;
    let outcome = (|| -> Result<(DiscussionQuestion, bool), AnswerError> {
        let now = Utc::now().to_rfc3339();
        let changed = conn
            .execute(
                "UPDATE discussion_questions SET state='cancelled', cancelled_by=?2, \
                 cancelled_at=?3, updated_at=?3 WHERE id=?1 AND discussion_id=?4 AND state='pending'",
                params![question_id, author_pseudo, now, discussion_id],
            )
            .map_err(anyhow::Error::from)?;
        let question = find()?;
        if changed == 0 {
            // Lost a race to an answer or a refusal written meanwhile, or to
            // the same blank refusal from another tab, which relayed already.
            return match question.state {
                DiscussionQuestionState::Cancelled => Ok((question, false)),
                _ => Err(AnswerError::Conflict),
            };
        }
        let relayed = relay_earlier_answers(
            conn,
            discussion_id,
            &question,
            author_pseudo,
            author_avatar_email,
        )?;
        Ok((if relayed { find()? } else { question }, relayed))
    })();
    match outcome {
        Ok(closed) => {
            conn.execute_batch("RELEASE close_discussion_question")
                .map_err(anyhow::Error::from)?;
            Ok(closed)
        }
        Err(error) => {
            let _ = conn.execute_batch(
                "ROLLBACK TO close_discussion_question; RELEASE close_discussion_question",
            );
            Err(error)
        }
    }
}

/// Where a blank refusal relays the earlier answers of its message.
fn relay_message_id(question_id: &str) -> String {
    format!("question-relay:{question_id}")
}

/// KT-1401 — when the step that asked has ended, the answers to the earlier
/// cards of its message are only recorded: the last pending card carries them
/// to the room's agent. A blank refusal of that last card (KT-1955) must not
/// lose them. So, when `question` — just closed — was the last pending card of
/// such a message and at least one other card of it was answered or refused
/// with a message, those decisions are published once to the room's agent,
/// exactly as an answer on this card would have carried them: the same
/// recap, this card listed as refused without a message, one dispatch.
///
/// The wake is owed to the earlier answers, not to the refusal: with none to
/// carry, or while the step still runs (each answer already reached it),
/// nothing is published. Returns whether a message was published.
fn relay_earlier_answers(
    conn: &Connection,
    discussion_id: &str,
    question: &DiscussionQuestion,
    author_pseudo: &str,
    author_avatar_email: Option<&str>,
) -> Result<bool, AnswerError> {
    let step_ended = question
        .requester_workflow_step
        .as_ref()
        .is_some_and(|source| !source.active);
    if !step_ended {
        return Ok(false);
    }
    let mut cards = list(conn, discussion_id)?
        .questions
        .into_iter()
        .filter(|q| q.source_message_id == question.source_message_id)
        .collect::<Vec<_>>();
    let still_pending = cards
        .iter()
        .any(|q| q.state == DiscussionQuestionState::Pending);
    let decided = cards.iter().any(|q| {
        q.id != question.id
            && matches!(
                q.state,
                DiscussionQuestionState::Answered | DiscussionQuestionState::Declined
            )
    });
    if still_pending || !decided {
        return Ok(false);
    }
    cards.sort_by_key(|q| q.fence_index);
    let recap = cards
        .iter()
        .map(recap_line)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let content = format!(
        "Arbitrage refusé sans message — {}\n\nCe refus ne décide rien. Il fermait la dernière \
         question en attente de ce message ; voici les décisions humaines sur ses questions :\n{recap}",
        question.question
    );
    let message_id = relay_message_id(&question.id);
    publish_human_resolution(
        conn,
        discussion_id,
        question,
        &message_id,
        content,
        &message_id,
        Utc::now(),
        author_pseudo,
        author_avatar_email,
        true,
        RoomAgentWake::ForAnswer,
    )?;
    Ok(true)
}

/// The cross-agent finding a card decides, when the card is a dispute card
/// of the dispute discussion Kronn recorded.
fn cross_dispute_of(
    conn: &Connection,
    discussion_id: &str,
    key: &str,
) -> Result<Option<crate::db::audit_cross::DisputeTarget>, AnswerError> {
    if !key.starts_with(crate::core::cross_audit::QUESTION_KEY_PREFIX) {
        return Ok(None);
    }
    Ok(crate::db::audit_cross::dispute_target(
        conn,
        discussion_id,
        key,
    )?)
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
    let mut briefing_after = None;
    let outcome = answer_inner(
        conn,
        discussion_id,
        question_id,
        request,
        author_pseudo,
        author_avatar_email,
        &mut briefing_after,
    );
    match outcome {
        Ok(question) => {
            conn.execute_batch("RELEASE answer_discussion_question")
                .map_err(anyhow::Error::from)?;
            // A briefing's folder move and file run only once its rows are
            // committed; a failure leaves the folder as it was (KT-1192).
            if let Some(after) = briefing_after {
                if let Err(e) = after.apply() {
                    tracing::warn!("briefing saved, folder left as it was: {}", e.message());
                }
            }
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

/// Bound on the failure note a partial TD card carries.
const TD_ERRORS_LIMIT: usize = 8000;

/// Keeps only the failed items on a pending card and writes the reasons in its
/// own `td_errors` field, replacing the previous partial's; `context` is the
/// agent's and stays as it was. `note: None` clears the field (the card is
/// complete). Returns the card's new `td_errors`.
fn narrow_to_failed(
    conn: &Connection,
    question_id: &str,
    failed_ids: &[&str],
    note: Option<&str>,
) -> Result<Option<String>, AnswerError> {
    let payload: String = conn
        .query_row(
            "SELECT payload_json FROM discussion_questions WHERE id=?1",
            [question_id],
            |row| row.get(0),
        )
        .map_err(anyhow::Error::from)?;
    let mut spec: QuestionSpec = serde_json::from_str(&payload).map_err(anyhow::Error::from)?;
    spec.items
        .retain(|item| failed_ids.contains(&item.id.as_str()));
    spec.td_errors = note.map(|note| {
        if note.chars().count() > TD_ERRORS_LIMIT {
            note.chars().take(TD_ERRORS_LIMIT - 1).collect::<String>() + "…"
        } else {
            note.to_string()
        }
    });
    conn.execute(
        "UPDATE discussion_questions SET payload_json=?2 WHERE id=?1 AND state='pending'",
        params![
            question_id,
            serde_json::to_string(&spec).map_err(anyhow::Error::from)?
        ],
    )
    .map_err(anyhow::Error::from)?;
    Ok(spec.td_errors)
}

fn answer_inner(
    conn: &Connection,
    discussion_id: &str,
    question_id: &str,
    request: &AnswerDiscussionQuestionRequest,
    author_pseudo: &str,
    author_avatar_email: Option<&str>,
    briefing_after: &mut Option<crate::api::audit::briefing::BriefingAfterCommit>,
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
    // One refused without a message is closed: reopening it by an answer is
    // not offered.
    if matches!(
        question.state,
        DiscussionQuestionState::Declined | DiscussionQuestionState::Cancelled
    ) {
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

    // KT-1192 — a briefing card: saved with the others once all are answered.
    if let Some(task_ref) = question
        .task_ref
        .as_deref()
        .filter(|r| r.starts_with(crate::api::audit::briefing_cards::TASK_REF_PREFIX))
    {
        *briefing_after = crate::api::audit::briefing_cards::on_card_answered(
            conn,
            discussion_id,
            task_ref,
            &question,
            selected.first().map(String::as_str),
            text.as_deref(),
        )
        .map_err(AnswerError::Invalid)?;
    }
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
    let kronn_key = stored_key(conn, question_id)?;
    let cross_dispute = cross_dispute_of(conn, discussion_id, &kronn_key)?;
    if let Some(mut target) = cross_dispute.clone() {
        let decision = selected
            .first()
            .and_then(|id| crate::core::cross_audit::HumanDecision::from_option_id(id))
            .ok_or_else(|| {
                AnswerError::Invalid("Choose keep or dismiss for a cross-agent dispute".into())
            })?;
        crate::core::cross_audit_decision::apply(conn, &mut target, decision)
            .map_err(AnswerError::Invalid)?;
    }
    if let Some(resolutions) = audit_resolutions.as_ref() {
        let outcome = crate::core::audit_validation::apply_td_resolutions(
            conn,
            discussion_id,
            resolutions,
            text.as_deref(),
        )
        .map_err(|error| AnswerError::Invalid(error.to_string()))?;
        // KT-1348 — a TD that could not be written does not undo the others,
        // and it is not closed with them: the card stays pending, narrowed to
        // the failed TDs with the reason on it, so a retry after the repair
        // applies only those and the validation resumes once none is left.
        let note = outcome.failure_note();
        if !note.is_empty() {
            tracing::warn!("Audit TD card partially recorded: {note}");
            let applied_labels = item_answers
                .iter()
                .filter(|answer| outcome.applied.contains(&answer.item_id))
                .filter_map(|answer| {
                    let item = question.items.iter().find(|i| i.id == answer.item_id)?;
                    let option = question
                        .options
                        .iter()
                        .find(|o| o.id == answer.selected_option_id)?;
                    Some(format!("- {}: {}", item.label, option.label))
                })
                .collect::<Vec<_>>()
                .join("\n");
            let partial = format!(
                "Décision humaine — {}\n\n{}\n\n{}{}",
                question.question,
                applied_labels,
                text.as_deref()
                    .map(|t| format!("{t}\n\n"))
                    .unwrap_or_default(),
                note
            );
            let failed_ids = outcome
                .failed
                .iter()
                .map(|(td_id, _)| td_id.as_str())
                .collect::<Vec<_>>();
            question.td_errors = narrow_to_failed(conn, question_id, &failed_ids, Some(&note))?;
            question
                .items
                .retain(|item| failed_ids.contains(&item.id.as_str()));
            publish_human_resolution(
                conn,
                discussion_id,
                &question,
                &format!(
                    "question-answer:{question_id}:partial:{}",
                    now.timestamp_micros()
                ),
                partial,
                &format!("question-answer:{question_id}"),
                now,
                author_pseudo,
                author_avatar_email,
                false,
                RoomAgentWake::Never,
            )?;
            conn.execute(
                "UPDATE discussion_questions SET updated_at=?2 WHERE id=?1 AND state='pending'",
                params![question_id, answer.answered_at],
            )
            .map_err(anyhow::Error::from)?;
            question.updated_at = answer.answered_at;
            return Ok(question);
        }
    }
    // The card is complete: a failure note left by an earlier partial is
    // resolved, so the answered card must not keep alerting about it (its
    // history stays in the partial receipts).
    if question.td_errors.is_some() {
        let item_ids = question
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>();
        narrow_to_failed(conn, question_id, &item_ids, None)?;
        question.td_errors = None;
    }
    let ceiling =
        super::discussion_ceiling_requests::decide(conn, discussion_id, &kronn_key, &selected)?;
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
    // The decision is applied by Kronn: no agent turn follows a dispute card.
    let dispatch = if cross_dispute.is_some() {
        false
    } else if audit_resolutions.is_some() {
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
    // KT-1401 — the step that asked has ended, so its agent will never read
    // the answers; the last pending card of its message carries them all to
    // the room's agent, once. Earlier cards only record theirs.
    let mut wake = RoomAgentWake::Never;
    let step_ended = question
        .requester_workflow_step
        .as_ref()
        .is_some_and(|source| !source.active);
    if step_ended && dispatch {
        let siblings = list(conn, discussion_id)?
            .questions
            .into_iter()
            .filter(|q| q.source_message_id == question.source_message_id && q.id != question.id)
            .collect::<Vec<_>>();
        wake = RoomAgentWake::Deferred;
        if siblings
            .iter()
            .all(|q| q.state != DiscussionQuestionState::Pending)
        {
            wake = RoomAgentWake::ForAnswer;
            let recap = siblings
                .iter()
                .map(recap_line)
                .collect::<Vec<_>>()
                .join("\n");
            if !recap.is_empty() {
                content.push_str(&format!(
                    "\n\nRéponses aux autres questions du même message :\n{recap}"
                ));
            }
        }
    }
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
        wake,
    )?;
    conn.execute("UPDATE discussion_questions SET state='answered', answer_json=?2, answer_idempotency_key=?3, updated_at=?4 WHERE id=?1 AND state='pending'",
        params![question_id, serde_json::to_string(&answer).map_err(anyhow::Error::from)?, request.idempotency_key, answer.answered_at])
        .map_err(anyhow::Error::from)?;
    question.state = DiscussionQuestionState::Answered;
    question.updated_at = answer.answered_at.clone();
    question.answer = Some(answer);
    Ok(question)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests;

#[cfg(all(test, feature = "full-tests"))]
mod question_cancel_tests;
