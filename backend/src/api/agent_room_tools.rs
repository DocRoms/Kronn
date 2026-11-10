//! The room tools of an HTTP Agent step with a `room_id` (KT-1244).
//!
//! A CLI step reaches its room through the `kronn-internal` bridge it joined
//! with. An HTTP step has no bridge: the orchestrator joined it to the room at
//! activation (`workflows::step_room`) and runs these three tools in-process.
//! They act on that room only, as a member of it: the step must still be
//! running, its session still active, and a room named in the arguments must be
//! the step's own. A step without a `room_id` is never offered them.
use super::*;
use crate::models::{MessageChannel, MessageRole};
use crate::workflows::step_room::StepRoomMember;
use axum::{extract::State, Json};

/// The names the room tools are declared and dispatched under.
pub(super) const NAMES: &[&str] = &["disc_read", "disc_append", "disc_question_list"];

const QUESTION_EXAMPLE: &str = "```kronn-question\n{\"version\":1,\"key\":\"stable-key\",\"question\":\"...\",\"options\":[{\"id\":\"a\",\"label\":\"...\"}]}\n```";

pub(super) fn declarations() -> Vec<Value> {
    let append = format!(
        "Post a message in THIS step's room, on the main channel, as this step's agent. `content` is \
         Markdown. When the human must arbitrate (choice, approval, ambiguous scope), put a closed \
         kronn-question fence in `content`, never prose alone: {QUESTION_EXAMPLE} `version` is the JSON \
         number 1, not \"1\". A card that could not be recorded comes back in `invalid_cards` with its \
         reason: repost it. Then read its key back with disc_question_list."
    );
    [
        (
            "disc_read",
            "Read the most recent messages of THIS step's room (default 40, max 200); `truncated` is set \
             when the room is longer. Read-only.",
            json!({"limit": {"type": "integer", "description": "How many recent messages (1-200)."}}),
            json!([]),
        ),
        (
            "disc_append",
            append.as_str(),
            json!({"content": {"type": "string", "description": "The message, Markdown."}}),
            json!(["content"]),
        ),
        (
            "disc_question_list",
            "Read the pending human arbitrations of THIS step's room (latest 50), or the durable answer \
             for an exact `key`. Never answers for the human. After publishing a kronn-question, read \
             its exact key back to verify it was recorded; an absent row is not an answer, correct the \
             card and repost it with the same key.",
            json!({"key": {"type": "string", "description": "Exact question key, up to 100 characters."}}),
            json!([]),
        ),
    ]
    .into_iter()
    .map(|(name, description, properties, required)| {
        json!({"type": "function", "function": {
            "name": name,
            "description": description,
            "parameters": {"type": "object", "properties": properties, "required": required},
        }})
    })
    .collect()
}

impl KronnToolExecutor {
    pub(super) async fn execute_step_room_tool(&self, call: &ToolCall) -> ToolOutcome {
        let Some(member) = self.step_room.as_ref() else {
            return fail(
                call,
                format!(
                    "tool `{}` needs a step that works in a room (`room_id`)",
                    call.name
                ),
            );
        };
        let room = member.context.discussion_id.as_str();
        if let Some(refusal) = foreign_room_refusal(call, room) {
            return fail(call, refusal);
        }
        if let Some(refusal) = self.membership_refusal(member).await {
            return fail(call, refusal);
        }
        match call.name.as_str() {
            "disc_read" => self.step_room_read(call, room).await,
            "disc_append" => self.step_room_append(call, member).await,
            _ => self.step_room_questions(call, room).await,
        }
    }

    /// Why the step can no longer act in its room, if it can't: the step ended
    /// (or was replaced), or its session left.
    async fn membership_refusal(&self, member: &StepRoomMember) -> Option<String> {
        let ended = "this step no longer works in its room: its membership has ended".to_string();
        if !self.state.workflow_step_rooms.authorize(&member.context) {
            return Some(ended);
        }
        let (agent, session, room) = (
            format!("{:?}", member.agent_type),
            member.session_id.clone(),
            member.context.discussion_id.clone(),
        );
        let active = self
            .state
            .db
            .with_read_conn(move |conn| {
                Ok(
                    crate::db::discussion_sessions::find_active_session(conn, &agent, &session)?
                        .is_some_and(|row| row.disc_id == room),
                )
            })
            .await;
        match active {
            Ok(true) => None,
            Ok(false) => Some(ended),
            Err(error) => Some(format!(
                "could not check the step's room membership: {error}"
            )),
        }
    }

    async fn step_room_read(&self, call: &ToolCall, room: &str) -> ToolOutcome {
        let limit = count_arg(call, "limit").unwrap_or(40).clamp(1, 200) as usize;
        let target = room.to_string();
        let messages = self
            .state
            .db
            .with_read_conn(move |conn| crate::db::discussions::list_messages(conn, &target))
            .await;
        let messages = match messages {
            Ok(messages) => messages,
            Err(error) => return fail(call, format!("could not read the room: {error}")),
        };
        let (total, tail) = message_tail(messages, limit);
        ok(
            call,
            json!({
                "discussion_id": room,
                "total_messages": total,
                "returned": tail.len(),
                "truncated": total > tail.len(),
                "messages": tail,
            }),
        )
    }

    async fn step_room_append(&self, call: &ToolCall, member: &StepRoomMember) -> ToolOutcome {
        let Some(content) = call.arguments["content"]
            .as_str()
            .filter(|content| !content.trim().is_empty())
        else {
            return fail(call, "missing required field `content`");
        };
        let request = crate::api::disc_source::DiscAppendRequest {
            disc_id: member.context.discussion_id.clone(),
            messages: vec![crate::api::disc_source::DiscAppendMessage {
                source_msg_id: format!("step-{}", uuid::Uuid::new_v4()),
                role: MessageRole::Agent,
                channel: MessageChannel::Main,
                content: content.to_string(),
                agent_type: Some(member.agent_type.clone()),
                targets: Vec::new(),
                target_agent: None,
                reply_to_message_id: None,
            }],
            // The session the orchestrator joined for the step: it is what
            // makes the step's agent the author of the message.
            session_id: Some(member.session_id.clone()),
            since_sort_order: None,
            session_credential: None,
            publication_grant: None,
            publication_proof: None,
            room_agent: None,
        };
        let Json(res) =
            crate::api::disc_source::disc_append(State(self.state.clone()), Json(request)).await;
        unwrap_api(call, res.success, res.data, res.error)
    }

    async fn step_room_questions(&self, call: &ToolCall, room: &str) -> ToolOutcome {
        let key = match call.arguments.get("key") {
            None | Some(Value::Null) => None,
            Some(Value::String(key)) if !key.is_empty() && key.chars().count() <= 100 => {
                Some(key.as_str())
            }
            Some(_) => {
                return fail(
                    call,
                    "key must be a non-empty string of at most 100 characters",
                )
            }
        };
        let target = room.to_string();
        let listed = self
            .state
            .db
            .with_read_conn(move |conn| crate::db::discussion_questions::list(conn, &target))
            .await;
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => {
                return fail(
                    call,
                    format!("could not read the room's questions: {error}"),
                )
            }
        };
        let pending_count = listed.pending_count;
        let questions = match serde_json::to_value(listed.questions) {
            Ok(Value::Array(questions)) => questions,
            _ => return fail(call, "could not serialise the room's questions"),
        };
        let matches: Vec<Value> = questions
            .into_iter()
            .filter(|question| match key {
                Some(key) => question["key"] == key,
                None => question["state"] == "pending",
            })
            .collect();
        let shown = matches.len().saturating_sub(50);
        ok(
            call,
            json!({
                "questions": matches[shown..],
                "matching_count": matches.len(),
                "pending_count": pending_count,
                "truncated": shown > 0,
            }),
        )
    }
}

/// A discussion other than the step's own, named in the arguments, is refused
/// rather than ignored: the model must not believe it read or wrote there.
fn foreign_room_refusal(call: &ToolCall, room: &str) -> Option<String> {
    for field in ["discussion_id", "disc_id"] {
        let named = match call.arguments.get(field) {
            None | Some(Value::Null) => continue,
            Some(Value::String(id)) if id.trim().is_empty() || id.trim() == room => continue,
            Some(named) => named,
        };
        return Some(format!(
            "refused: `{field}` {named} is not this step's room. These tools act on the room of this \
             step only; omit `{field}`."
        ));
    }
    None
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "agent_room_tests.rs"]
mod tests;
