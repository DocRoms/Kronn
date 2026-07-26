//! 0.9.2-H — durable Planning proposals (`kronn-plan-action` fences) with
//! itemized, human-gated validation. Agents PROPOSE; a human accepts/rejects
//! each item. Acceptance applies the underlying task mutation idempotently and
//! emits a `[kronn-planning: …]` System receipt.
//!
//! Models + DTO live here (TS-exported for the inbox UI); the ingestion hook,
//! decision transaction, queries and receipt are added in the same module.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Aggregate over a proposal's item states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProposalAggregateState {
    /// Every item still pending.
    Pending,
    /// A mix of pending and terminal items.
    Partial,
    /// No pending item and at least one accepted.
    Applied,
    /// Every item rejected.
    Dismissed,
}

/// The mutation an item requests. `open` is a local navigation, never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProposalItemAction {
    Create,
    Status,
    Complete,
    Unblock,
}

/// Per-item validation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProposalItemState {
    Pending,
    Accepted,
    Rejected,
}

/// A human's decision on one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProposalDecision {
    Accept,
    Reject,
}

/// The proposed fields for one item — a flat union covering every action
/// (create carries title/description/priority/placement/is_primary; the
/// mutations carry task_id and, for `status`, the target status).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProposalItemPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub placement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub is_primary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<String>,
}

/// One item of a proposal, with its validation state + idempotent result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningProposalItem {
    pub id: String,
    pub item_index: i64,
    pub action: ProposalItemAction,
    pub payload: ProposalItemPayload,
    pub state: ProposalItemState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rejected_reason: Option<String>,
    /// The task created/updated on acceptance (the idempotent result).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub result_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub decided_at: Option<String>,
}

/// A durable proposal: one `kronn-plan-action` fence from an Agent message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanningProposal {
    pub id: String,
    pub discussion_id: String,
    pub source_message_id: String,
    pub fence_index: i64,
    pub aggregate_state: ProposalAggregateState,
    pub items: Vec<PlanningProposalItem>,
    pub created_at: String,
    pub updated_at: String,
}

/// `GET /api/planning/proposals?discussion_id=…` — the inbox snapshot. Counts
/// drive the header chip ("N propositions · M tâches à valider").
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProposalListResponse {
    pub proposals: Vec<PlanningProposal>,
    /// Proposals with at least one pending item.
    pub pending_proposal_count: i64,
    /// Items still pending across the returned proposals.
    pub pending_item_count: i64,
}

/// `POST /api/planning/proposals/:id/items/:item_id/decision` body. The
/// `idempotency_key` makes a retry (or double-click, or a second client)
/// return the same result instead of applying twice.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProposalDecisionRequest {
    pub decision: ProposalDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
    pub idempotency_key: String,
}

/// Decision result: the updated item + the proposal's recomputed aggregate.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProposalDecisionResponse {
    pub item: PlanningProposalItem,
    pub aggregate_state: ProposalAggregateState,
}

// ─── Fence parsing (mirrors frontend lib/planningProposal.ts) ────────────────

/// One parsed, validated item ready to persist (create_many expands to N).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedProposalItem {
    pub action: ProposalItemAction,
    pub payload: ProposalItemPayload,
}

const PRIORITIES: &[&str] = &["critical", "high", "normal", "low"];
const PLACEMENTS: &[&str] = &["active", "later"];
const STATUSES: &[&str] = &["idea", "todo", "in_progress", "blocked", "done", "archived"];

/// Extract the JSON body of each ```kronn-plan-action``` fence, in order.
pub fn extract_plan_action_fences(content: &str) -> Vec<String> {
    let mut fences = Vec::new();
    let mut in_fence = false;
    let mut buf = String::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if !in_fence
            && trimmed.starts_with("```")
            && trimmed.trim_start_matches('`').trim() == "kronn-plan-action"
        {
            in_fence = true;
            buf.clear();
            continue;
        }
        if in_fence && trimmed.starts_with("```") {
            fences.push(std::mem::take(&mut buf));
            in_fence = false;
            continue;
        }
        if in_fence {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    fences
}

fn non_empty(value: &serde_json::Value, key: &str) -> Option<String> {
    let s = value.get(key)?.as_str()?;
    (!s.trim().is_empty()).then(|| s.to_string())
}

fn validated(value: &serde_json::Value, key: &str, allowed: &[&str]) -> Option<Option<String>> {
    match value.get(key) {
        None => Some(None),
        Some(v) => {
            let s = v.as_str()?;
            allowed.contains(&s).then(|| Some(s.to_string()))
        }
    }
}

fn parse_create(value: &serde_json::Value) -> Option<ParsedProposalItem> {
    let title = non_empty(value, "title")?;
    // description, when present, must be a string.
    let description = match value.get("description") {
        None => None,
        Some(v) => Some(v.as_str()?.to_string()),
    };
    let priority = validated(value, "priority", PRIORITIES)?;
    let placement = validated(value, "placement", PLACEMENTS)?;
    let is_primary = match value.get("is_primary") {
        None => None,
        Some(v) => Some(v.as_bool()?),
    };
    Some(ParsedProposalItem {
        action: ProposalItemAction::Create,
        payload: ProposalItemPayload {
            title: Some(title),
            description,
            priority,
            placement,
            is_primary,
            task_id: None,
            status: None,
        },
    })
}

/// Parse one fence's JSON into persistable items. Returns `None` for an invalid
/// fence or an `open` action (navigation, never persisted) — the message is
/// then kept raw with no proposal.
pub fn parse_proposal_items(json: &str) -> Option<Vec<ParsedProposalItem>> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    match value.get("action")?.as_str()? {
        "open" => None,
        "create" => Some(vec![parse_create(&value)?]),
        "create_many" => {
            let tasks = value.get("tasks")?.as_array()?;
            if tasks.is_empty() {
                return None;
            }
            tasks.iter().map(parse_create).collect()
        }
        "status" => {
            let task_id = non_empty(&value, "task_id")?;
            let status = value.get("status")?.as_str()?;
            if !STATUSES.contains(&status) {
                return None;
            }
            Some(vec![ParsedProposalItem {
                action: ProposalItemAction::Status,
                payload: ProposalItemPayload {
                    task_id: Some(task_id),
                    status: Some(status.to_string()),
                    ..Default::default()
                },
            }])
        }
        action @ ("complete" | "unblock") => {
            let task_id = non_empty(&value, "task_id")?;
            let act = if action == "complete" {
                ProposalItemAction::Complete
            } else {
                ProposalItemAction::Unblock
            };
            Some(vec![ParsedProposalItem {
                action: act,
                payload: ProposalItemPayload {
                    task_id: Some(task_id),
                    ..Default::default()
                },
            }])
        }
        _ => None,
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn extracts_and_parses_create_many_into_n_items() {
        let content = "prose\n\n```kronn-plan-action\n{\"action\":\"create_many\",\"tasks\":[{\"title\":\"A\"},{\"title\":\"B\",\"priority\":\"high\"}]}\n```\nmore prose";
        let fences = extract_plan_action_fences(content);
        assert_eq!(fences.len(), 1);
        let items = parse_proposal_items(&fences[0]).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].action, ProposalItemAction::Create);
        assert_eq!(items[0].payload.title.as_deref(), Some("A"));
        assert_eq!(items[1].payload.priority.as_deref(), Some("high"));
    }

    #[test]
    fn open_and_invalid_are_dropped() {
        assert!(parse_proposal_items("{\"action\":\"open\"}").is_none());
        assert!(parse_proposal_items("{\"action\":\"create\"}").is_none()); // no title
        assert!(parse_proposal_items(
            "{\"action\":\"status\",\"task_id\":\"KT-1\",\"status\":\"bogus\"}"
        )
        .is_none());
        assert!(parse_proposal_items("not json").is_none());
    }

    #[test]
    fn mutations_parse_task_id() {
        let complete =
            parse_proposal_items("{\"action\":\"complete\",\"task_id\":\"KT-9\"}").unwrap();
        assert_eq!(complete[0].action, ProposalItemAction::Complete);
        assert_eq!(complete[0].payload.task_id.as_deref(), Some("KT-9"));
        let status = parse_proposal_items(
            "{\"action\":\"status\",\"task_id\":\"KT-9\",\"status\":\"in_progress\"}",
        )
        .unwrap();
        assert_eq!(status[0].payload.status.as_deref(), Some("in_progress"));
    }
}

// ─── Ingestion (called from insert_message, same transaction) ────────────────

impl ProposalItemAction {
    pub(crate) fn as_db_str(self) -> &'static str {
        match self {
            ProposalItemAction::Create => "create",
            ProposalItemAction::Status => "status",
            ProposalItemAction::Complete => "complete",
            ProposalItemAction::Unblock => "unblock",
        }
    }

    pub(crate) fn from_db_str(s: &str) -> Option<ProposalItemAction> {
        match s {
            "create" => Some(ProposalItemAction::Create),
            "status" => Some(ProposalItemAction::Status),
            "complete" => Some(ProposalItemAction::Complete),
            "unblock" => Some(ProposalItemAction::Unblock),
            _ => None,
        }
    }
}

/// Persist the durable proposals for one Agent message's `kronn-plan-action`
/// fences, in the CALLER's transaction (so a proposal exists iff its message
/// exists). Deterministic IDs (`proposal:<msg>:<fence>` / `…:<item>`) +
/// `INSERT OR IGNORE` make re-ingestion (bulk import, replay) a no-op — never a
/// duplicate. A message with no valid fence is a no-op; `open` is not persisted.
pub fn ingest_message_proposals(
    conn: &Connection,
    discussion_id: &str,
    message_id: &str,
    content: &str,
) -> Result<()> {
    // Cheap early-out: the vast majority of messages carry no fence.
    if !content.contains("kronn-plan-action") {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    for (fence_index, fence) in extract_plan_action_fences(content).into_iter().enumerate() {
        let Some(items) = parse_proposal_items(&fence) else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        let proposal_id = format!("proposal:{message_id}:{fence_index}");
        conn.execute(
            "INSERT OR IGNORE INTO planning_proposals
                 (id, discussion_id, source_message_id, fence_index,
                  aggregate_state, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?5)",
            params![
                proposal_id,
                discussion_id,
                message_id,
                fence_index as i64,
                now
            ],
        )?;
        for (item_index, item) in items.into_iter().enumerate() {
            let item_id = format!("{proposal_id}:{item_index}");
            let payload = serde_json::to_string(&item.payload)?;
            conn.execute(
                "INSERT OR IGNORE INTO planning_proposal_items
                     (id, proposal_id, item_index, action, payload_json, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
                params![
                    item_id,
                    proposal_id,
                    item_index as i64,
                    item.action.as_db_str(),
                    payload
                ],
            )?;
        }
    }
    Ok(())
}

// ─── Reads (inbox snapshot; source of truth for the UI) ──────────────────────

impl ProposalItemState {
    /// Fail closed: an unknown value is a corrupt row, never silently `Pending`.
    fn from_db_str(s: &str) -> Option<ProposalItemState> {
        match s {
            "pending" => Some(ProposalItemState::Pending),
            "accepted" => Some(ProposalItemState::Accepted),
            "rejected" => Some(ProposalItemState::Rejected),
            _ => None,
        }
    }
}

impl ProposalAggregateState {
    /// Fail closed: an unknown value is a corrupt row, never silently `Pending`.
    fn from_db_str(s: &str) -> Option<ProposalAggregateState> {
        match s {
            "pending" => Some(ProposalAggregateState::Pending),
            "partial" => Some(ProposalAggregateState::Partial),
            "applied" => Some(ProposalAggregateState::Applied),
            "dismissed" => Some(ProposalAggregateState::Dismissed),
            _ => None,
        }
    }
}

fn load_items(conn: &Connection, proposal_id: &str) -> Result<Vec<PlanningProposalItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, item_index, action, payload_json, state, rejected_reason,
                result_task_id, decided_at
           FROM planning_proposal_items
          WHERE proposal_id = ?1
          ORDER BY item_index ASC",
    )?;
    let items = stmt
        .query_map(params![proposal_id], |row| {
            // Fail closed on a corrupt row — never silently coerce a bad action
            // into `create` or a bad payload into empty (that would turn
            // corruption into an unintended task mutation at the apply endpoint).
            let action_raw = row.get::<_, String>(2)?;
            let action = ProposalItemAction::from_db_str(&action_raw).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    format!("invalid proposal item action `{action_raw}`").into(),
                )
            })?;
            let payload: ProposalItemPayload = serde_json::from_str(&row.get::<_, String>(3)?)
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
            let state_raw = row.get::<_, String>(4)?;
            let state = ProposalItemState::from_db_str(&state_raw).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    format!("invalid proposal item state `{state_raw}`").into(),
                )
            })?;
            Ok(PlanningProposalItem {
                id: row.get(0)?,
                item_index: row.get(1)?,
                action,
                payload,
                state,
                rejected_reason: row.get(5)?,
                result_task_id: row.get(6)?,
                decided_at: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
}

/// Full proposal (with its items), or `None` if unknown.
pub fn get_proposal(conn: &Connection, proposal_id: &str) -> Result<Option<PlanningProposal>> {
    let row = conn
        .query_row(
            "SELECT id, discussion_id, source_message_id, fence_index,
                    aggregate_state, created_at, updated_at
               FROM planning_proposals WHERE id = ?1",
            params![proposal_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((id, discussion_id, source_message_id, fence_index, aggregate, created, updated)) =
        row
    else {
        return Ok(None);
    };
    Ok(Some(PlanningProposal {
        items: load_items(conn, &id)?,
        id,
        discussion_id,
        source_message_id,
        fence_index,
        aggregate_state: ProposalAggregateState::from_db_str(&aggregate)
            .ok_or_else(|| anyhow::anyhow!("invalid proposal aggregate state `{aggregate}`"))?,
        created_at: created,
        updated_at: updated,
    }))
}

/// Inbox snapshot for a discussion. `pending_only` keeps proposals that still
/// have work to validate (`pending`/`partial`); counters drive the header chip.
pub fn list_proposals(
    conn: &Connection,
    discussion_id: &str,
    pending_only: bool,
) -> Result<ProposalListResponse> {
    let sql = if pending_only {
        "SELECT id FROM planning_proposals
          WHERE discussion_id = ?1 AND aggregate_state IN ('pending', 'partial')
          ORDER BY created_at ASC, id ASC"
    } else {
        "SELECT id FROM planning_proposals
          WHERE discussion_id = ?1
          ORDER BY created_at ASC, id ASC"
    };
    let mut stmt = conn.prepare(sql)?;
    let ids: Vec<String> = stmt
        .query_map(params![discussion_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut proposals = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(p) = get_proposal(conn, &id)? {
            proposals.push(p);
        }
    }
    let pending_item_count = proposals
        .iter()
        .flat_map(|p| &p.items)
        .filter(|i| i.state == ProposalItemState::Pending)
        .count() as i64;
    let pending_proposal_count = proposals
        .iter()
        .filter(|p| {
            p.items
                .iter()
                .any(|i| i.state == ProposalItemState::Pending)
        })
        .count() as i64;
    Ok(ProposalListResponse {
        proposals,
        pending_proposal_count,
        pending_item_count,
    })
}

#[cfg(test)]
mod ingest_tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES ('p1','P','/tmp',?1,?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussions (id, project_id, title, created_at, updated_at) VALUES ('d1','p1','D',?1,?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order) VALUES ('msg1','d1','Agent','x',?1,0)",
            params![now],
        )
        .unwrap();
        conn
    }

    #[test]
    fn ingest_persists_items_and_is_idempotent() {
        let conn = db();
        let content = "```kronn-plan-action\n{\"action\":\"create_many\",\"tasks\":[{\"title\":\"A\"},{\"title\":\"B\",\"priority\":\"high\"}]}\n```";
        ingest_message_proposals(&conn, "d1", "msg1", content).unwrap();

        let list = list_proposals(&conn, "d1", true).unwrap();
        assert_eq!(list.proposals.len(), 1);
        assert_eq!(list.proposals[0].items.len(), 2);
        assert_eq!(
            list.proposals[0].aggregate_state,
            ProposalAggregateState::Pending
        );
        assert_eq!(list.pending_item_count, 2);
        assert_eq!(list.pending_proposal_count, 1);
        assert_eq!(
            list.proposals[0].items[1].payload.priority.as_deref(),
            Some("high")
        );

        // Re-ingest the same message → idempotent, zero duplication.
        ingest_message_proposals(&conn, "d1", "msg1", content).unwrap();
        let again = list_proposals(&conn, "d1", true).unwrap();
        assert_eq!(
            again.proposals.len(),
            1,
            "re-ingest must not duplicate the proposal"
        );
        assert_eq!(
            again.proposals[0].items.len(),
            2,
            "re-ingest must not duplicate items"
        );
    }

    #[test]
    fn message_without_a_valid_fence_yields_no_proposal() {
        let conn = db();
        ingest_message_proposals(&conn, "d1", "msg1", "just prose, no fence").unwrap();
        assert!(list_proposals(&conn, "d1", false)
            .unwrap()
            .proposals
            .is_empty());
        // An `open` fence is navigation, not an inbox proposal.
        ingest_message_proposals(
            &conn,
            "d1",
            "msg1",
            "```kronn-plan-action\n{\"action\":\"open\"}\n```",
        )
        .unwrap();
        assert!(list_proposals(&conn, "d1", false)
            .unwrap()
            .proposals
            .is_empty());
    }

    #[test]
    fn ingestion_failure_rolls_back_the_whole_message_insert() {
        // The SAVEPOINT in insert_message must make "message + proposals" atomic
        // even on a bare &Connection (autocommit). Break the items table so
        // ingestion fails; the whole insert — message row, message_count,
        // next_message_seq — must roll back.
        let conn = db();
        conn.execute("DROP TABLE planning_proposal_items", [])
            .unwrap();
        let seq_before: i64 = conn
            .query_row(
                "SELECT next_message_seq FROM discussions WHERE id = 'd1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let count_before: i64 = conn
            .query_row(
                "SELECT message_count FROM discussions WHERE id = 'd1'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let msg = crate::models::DiscussionMessage {
            model: None,
            lint_report: None,
            id: "am1".to_string(),
            role: crate::models::MessageRole::Agent,
            content: "```kronn-plan-action\n{\"action\":\"create\",\"title\":\"X\"}\n```"
                .to_string(),
            agent_type: Some(crate::models::AgentType::Codex),
            timestamp: Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: None,
            author_avatar_email: None,
            source_msg_id: None,
            duration_ms: None,
        };
        let result = crate::db::discussions::insert_message(&conn, "d1", &msg);
        assert!(result.is_err(), "ingestion failure must fail the insert");

        let msgs: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages WHERE id = 'am1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(msgs, 0, "the message row must not persist");
        let seq_after: i64 = conn
            .query_row(
                "SELECT next_message_seq FROM discussions WHERE id = 'd1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(seq_after, seq_before, "next_message_seq must be unchanged");
        let count_after: i64 = conn
            .query_row(
                "SELECT message_count FROM discussions WHERE id = 'd1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count_after, count_before, "message_count must be unchanged");
    }
}

// ─── Decision (idempotent apply + in-transaction receipt) ────────────────────

use crate::models::{
    CreatePlanningTaskRequest, LinkPlanningDiscussionRequest, PlanningPlacement,
    PlanningTaskPriority, PlanningTaskStatus, UpdatePlanningTaskRequest,
};

/// Why a decision could not be applied.
#[derive(Debug)]
pub enum DecisionError {
    /// No such proposal/item.
    NotFound,
    /// The item is already terminal under a DIFFERENT idempotency key — a
    /// contradictory decision, never a silent re-mutation.
    Conflict { current_state: ProposalItemState },
    /// The request itself is invalid (e.g. an empty/oversized idempotency key) —
    /// rejected before any work, mapped to 400 by the API.
    Invalid(String),
    /// The underlying task mutation (or persistence) failed; the whole decision
    /// rolls back.
    Failed(anyhow::Error),
}

impl From<rusqlite::Error> for DecisionError {
    fn from(e: rusqlite::Error) -> Self {
        DecisionError::Failed(e.into())
    }
}
impl From<anyhow::Error> for DecisionError {
    fn from(e: anyhow::Error) -> Self {
        DecisionError::Failed(e)
    }
}

// The payload was validated at ingestion, so these fail closed on any invalid
// PRESENT value (defence-in-depth against a corrupt row); `None` keeps the
// genuine default only where the field is truly optional. `status` is required
// for a `status` action.
fn status_from_payload(s: Option<&str>) -> Result<PlanningTaskStatus, DecisionError> {
    match s {
        Some("idea") => Ok(PlanningTaskStatus::Idea),
        Some("todo") => Ok(PlanningTaskStatus::Todo),
        Some("in_progress") => Ok(PlanningTaskStatus::InProgress),
        Some("blocked") => Ok(PlanningTaskStatus::Blocked),
        Some("done") => Ok(PlanningTaskStatus::Done),
        Some("archived") => Ok(PlanningTaskStatus::Archived),
        other => Err(DecisionError::Failed(anyhow::anyhow!(
            "status action has an invalid/missing status: {other:?}"
        ))),
    }
}

fn priority_from_payload(s: Option<&str>) -> Result<PlanningTaskPriority, DecisionError> {
    match s {
        None => Ok(PlanningTaskPriority::Normal),
        Some("critical") => Ok(PlanningTaskPriority::Critical),
        Some("high") => Ok(PlanningTaskPriority::High),
        Some("normal") => Ok(PlanningTaskPriority::Normal),
        Some("low") => Ok(PlanningTaskPriority::Low),
        Some(other) => Err(DecisionError::Failed(anyhow::anyhow!(
            "invalid priority `{other}`"
        ))),
    }
}

fn placement_from_payload(s: Option<&str>) -> Result<PlanningPlacement, DecisionError> {
    match s {
        None => Ok(PlanningPlacement::Active),
        Some("active") => Ok(PlanningPlacement::Active),
        Some("later") => Ok(PlanningPlacement::Later),
        Some(other) => Err(DecisionError::Failed(anyhow::anyhow!(
            "invalid placement `{other}`"
        ))),
    }
}

/// Apply an accepted item's underlying task mutation, returning the affected
/// task id. Runs inside the caller's decision transaction.
fn apply_accepted(
    conn: &Connection,
    discussion_id: &str,
    item: &PlanningProposalItem,
) -> Result<String, DecisionError> {
    let p = &item.payload;
    match item.action {
        ProposalItemAction::Create => {
            let created = crate::db::planning::create_task(
                conn,
                &CreatePlanningTaskRequest {
                    title: p.title.clone().unwrap_or_default(),
                    description: p.description.clone().unwrap_or_default(),
                    status: PlanningTaskStatus::Todo,
                    priority: priority_from_payload(p.priority.as_deref())?,
                    parent_id: None,
                    project_ids: vec![],
                    tags: vec![],
                    definition_of_done: vec![],
                    links: vec![],
                    actor: Default::default(),
                },
            )?;
            crate::db::planning::link_discussion(
                conn,
                &created.summary.id,
                &LinkPlanningDiscussionRequest {
                    discussion_id: discussion_id.to_string(),
                    placement: placement_from_payload(p.placement.as_deref())?,
                    is_primary: p.is_primary.unwrap_or(false),
                    position: None,
                    actor: Default::default(),
                },
            )?;
            Ok(created.summary.id)
        }
        ProposalItemAction::Status | ProposalItemAction::Complete | ProposalItemAction::Unblock => {
            let task_id = p
                .task_id
                .clone()
                .ok_or_else(|| DecisionError::Failed(anyhow::anyhow!("item has no task_id")))?;
            let update = match item.action {
                ProposalItemAction::Complete => UpdatePlanningTaskRequest {
                    status: Some(PlanningTaskStatus::Done),
                    ..empty_update()
                },
                ProposalItemAction::Unblock => UpdatePlanningTaskRequest {
                    status: Some(PlanningTaskStatus::Todo),
                    blocked_reason: Some(None),
                    ..empty_update()
                },
                _ => UpdatePlanningTaskRequest {
                    status: Some(status_from_payload(p.status.as_deref())?),
                    ..empty_update()
                },
            };
            let updated = crate::db::planning::update_task(conn, &task_id, &update)?;
            Ok(updated.summary.id)
        }
    }
}

fn empty_update() -> UpdatePlanningTaskRequest {
    UpdatePlanningTaskRequest {
        title: None,
        description: None,
        status: None,
        priority: None,
        parent_id: None,
        blocked_reason: None,
        rank: None,
        project_ids: None,
        tags: None,
        definition_of_done: None,
        links: None,
        actor: Default::default(),
    }
}

fn recompute_aggregate(conn: &Connection, proposal_id: &str) -> Result<ProposalAggregateState> {
    let mut pending = 0i64;
    let mut accepted = 0i64;
    let mut rejected = 0i64;
    let mut stmt =
        conn.prepare("SELECT state FROM planning_proposal_items WHERE proposal_id = ?1")?;
    let states = stmt
        .query_map(params![proposal_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for s in &states {
        match s.as_str() {
            "accepted" => accepted += 1,
            "rejected" => rejected += 1,
            _ => pending += 1,
        }
    }
    let aggregate = if pending == states.len() as i64 {
        ProposalAggregateState::Pending
    } else if pending > 0 {
        ProposalAggregateState::Partial
    } else if accepted > 0 {
        ProposalAggregateState::Applied
    } else {
        let _ = rejected;
        ProposalAggregateState::Dismissed
    };
    let db_value = match aggregate {
        ProposalAggregateState::Pending => "pending",
        ProposalAggregateState::Partial => "partial",
        ProposalAggregateState::Applied => "applied",
        ProposalAggregateState::Dismissed => "dismissed",
    };
    conn.execute(
        "UPDATE planning_proposals SET aggregate_state = ?2, updated_at = ?3 WHERE id = ?1",
        params![proposal_id, db_value, Utc::now().to_rfc3339()],
    )?;
    Ok(aggregate)
}

/// Decide one item: human-gated accept/reject, applied ATOMICALLY with the item
/// state, the `[kronn-planning:…]` System receipt and the recomputed aggregate.
/// Idempotent on `idempotency_key` (a replay returns the same result + receipt);
/// a terminal item under a different key is a `Conflict`.
pub fn decide_item(
    conn: &Connection,
    proposal_id: &str,
    item_id: &str,
    decision: ProposalDecision,
    reason: Option<&str>,
    idempotency_key: &str,
) -> Result<ProposalDecisionResponse, DecisionError> {
    // Validate before any work: an empty key would become a single global key
    // via the unique index and collide across items; bound both fields so the
    // API is safe on its own, not only behind a well-behaved UI.
    let key = idempotency_key.trim();
    if key.is_empty() || key.len() > 128 {
        return Err(DecisionError::Invalid(
            "idempotency_key must be 1..=128 characters".into(),
        ));
    }
    if reason.is_some_and(|r| r.len() > 500) {
        return Err(DecisionError::Invalid(
            "reason must be at most 500 characters".into(),
        ));
    }

    conn.execute_batch("SAVEPOINT decide_item")?;
    let outcome = decide_item_inner(conn, proposal_id, item_id, decision, reason, key);
    match &outcome {
        Ok(_) => {
            conn.execute_batch("RELEASE decide_item")?;
        }
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK TO decide_item; RELEASE decide_item");
        }
    }
    outcome
}

fn decide_item_inner(
    conn: &Connection,
    proposal_id: &str,
    item_id: &str,
    decision: ProposalDecision,
    reason: Option<&str>,
    idempotency_key: &str,
) -> Result<ProposalDecisionResponse, DecisionError> {
    // Discussion + current item snapshot.
    let discussion_id: String = conn
        .query_row(
            "SELECT discussion_id FROM planning_proposals WHERE id = ?1",
            params![proposal_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(DecisionError::NotFound)?;

    let current: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT state, decision_idempotency_key FROM planning_proposal_items
              WHERE id = ?1 AND proposal_id = ?2",
            params![item_id, proposal_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    let Some((state_str, existing_key)) = current else {
        return Err(DecisionError::NotFound);
    };
    let state = ProposalItemState::from_db_str(&state_str).ok_or_else(|| {
        DecisionError::Failed(anyhow::anyhow!("corrupt item state `{state_str}`"))
    })?;

    // Idempotent replay: same key → return the already-recorded result.
    if existing_key.as_deref() == Some(idempotency_key) {
        let item = load_single_item(conn, item_id)?;
        let aggregate = current_aggregate(conn, proposal_id)?;
        return Ok(ProposalDecisionResponse {
            item,
            aggregate_state: aggregate,
        });
    }
    // A terminal item under a different (or no) matching key → conflict.
    if state != ProposalItemState::Pending {
        return Err(DecisionError::Conflict {
            current_state: state,
        });
    }

    let now = Utc::now().to_rfc3339();
    let (new_state, result_task_id, receipt_body) = match decision {
        ProposalDecision::Reject => {
            let item = load_single_item(conn, item_id)?;
            let label = item.payload.title.clone().or(item.payload.task_id.clone());
            (
                "rejected",
                None,
                format!(
                    "rejected {}{}",
                    label.map(|l| format!("\"{l}\" ")).unwrap_or_default(),
                    reason.map(|r| format!("— {r}")).unwrap_or_default()
                ),
            )
        }
        ProposalDecision::Accept => {
            let item = load_single_item(conn, item_id)?;
            let task_id = apply_accepted(conn, &discussion_id, &item)?;
            (
                "accepted",
                Some(task_id.clone()),
                format!("accepted → {task_id}"),
            )
        }
    };

    // Receipt: a non-dispatching System message, inserted IN THIS transaction so
    // the decision and its receipt are all-or-nothing. Deterministic id → a
    // replay never creates a second one (and returns early above anyway).
    let receipt_id = format!(
        "planning-receipt:{proposal_id}:{item_id}:{}",
        match decision {
            ProposalDecision::Accept => "accept",
            ProposalDecision::Reject => "reject",
        }
    );
    let receipt = crate::models::DiscussionMessage {
        model: None,
        lint_report: None,
        id: receipt_id.clone(),
        role: crate::models::MessageRole::System,
        content: format!("[kronn-planning: {receipt_body}]"),
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
    };
    crate::db::discussions::insert_message(conn, &discussion_id, &receipt)
        .map_err(DecisionError::Failed)?;

    // A reason belongs to a REJECT only — never leave one on an accepted item.
    let reason_for_db = match decision {
        ProposalDecision::Reject => reason,
        ProposalDecision::Accept => None,
    };
    conn.execute(
        "UPDATE planning_proposal_items
            SET state = ?3, rejected_reason = ?4, result_task_id = ?5,
                decision_idempotency_key = ?6, receipt_message_id = ?7, decided_at = ?8
          WHERE id = ?1 AND proposal_id = ?2",
        params![
            item_id,
            proposal_id,
            new_state,
            reason_for_db,
            result_task_id,
            idempotency_key,
            receipt_id,
            now
        ],
    )?;

    let aggregate = recompute_aggregate(conn, proposal_id)?;
    let item = load_single_item(conn, item_id)?;
    Ok(ProposalDecisionResponse {
        item,
        aggregate_state: aggregate,
    })
}

fn load_single_item(conn: &Connection, item_id: &str) -> Result<PlanningProposalItem> {
    let proposal_id: String = conn.query_row(
        "SELECT proposal_id FROM planning_proposal_items WHERE id = ?1",
        params![item_id],
        |r| r.get(0),
    )?;
    load_items(conn, &proposal_id)?
        .into_iter()
        .find(|i| i.id == item_id)
        .ok_or_else(|| anyhow::anyhow!("item vanished"))
}

fn current_aggregate(conn: &Connection, proposal_id: &str) -> Result<ProposalAggregateState> {
    let s: String = conn.query_row(
        "SELECT aggregate_state FROM planning_proposals WHERE id = ?1",
        params![proposal_id],
        |r| r.get(0),
    )?;
    ProposalAggregateState::from_db_str(&s)
        .ok_or_else(|| anyhow::anyhow!("invalid proposal aggregate state `{s}`"))
}

#[cfg(test)]
mod decision_tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES ('p1','P','/tmp',?1,?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussions (id, project_id, title, created_at, updated_at) VALUES ('d1','p1','D',?1,?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order) VALUES ('msg1','d1','Agent','x',?1,0)",
            params![now],
        )
        .unwrap();
        conn
    }

    fn receipt_count(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE discussion_id='d1' AND content LIKE '[kronn-planning:%'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn accept_applies_creates_receipt_and_is_idempotent() {
        let conn = db();
        ingest_message_proposals(
            &conn,
            "d1",
            "msg1",
            "```kronn-plan-action\n{\"action\":\"create_many\",\"tasks\":[{\"title\":\"Alpha\"},{\"title\":\"Beta\"}]}\n```",
        )
        .unwrap();
        let proposal = "proposal:msg1:0";
        let item0 = "proposal:msg1:0:0";

        let resp = decide_item(
            &conn,
            proposal,
            item0,
            ProposalDecision::Accept,
            None,
            "key-1",
        )
        .unwrap();
        assert_eq!(resp.item.state, ProposalItemState::Accepted);
        assert!(
            resp.item.result_task_id.is_some(),
            "acceptance created a task"
        );
        // 1 accepted + 1 still pending → partial.
        assert_eq!(resp.aggregate_state, ProposalAggregateState::Partial);
        assert_eq!(receipt_count(&conn), 1, "one receipt inserted in-tx");

        // Idempotent replay: same key → same result, no second receipt.
        let again = decide_item(
            &conn,
            proposal,
            item0,
            ProposalDecision::Accept,
            None,
            "key-1",
        )
        .unwrap();
        assert_eq!(again.item.result_task_id, resp.item.result_task_id);
        assert_eq!(
            receipt_count(&conn),
            1,
            "replay must not create a second receipt"
        );

        // A DIFFERENT key on the now-terminal item → conflict.
        let conflict = decide_item(
            &conn,
            proposal,
            item0,
            ProposalDecision::Reject,
            Some("changed mind"),
            "key-2",
        );
        assert!(matches!(conflict, Err(DecisionError::Conflict { .. })));
    }

    #[test]
    fn reject_records_reason_and_creates_no_task() {
        let conn = db();
        ingest_message_proposals(
            &conn,
            "d1",
            "msg1",
            "```kronn-plan-action\n{\"action\":\"create\",\"title\":\"X\"}\n```",
        )
        .unwrap();
        let resp = decide_item(
            &conn,
            "proposal:msg1:0",
            "proposal:msg1:0:0",
            ProposalDecision::Reject,
            Some("not now"),
            "rk",
        )
        .unwrap();
        assert_eq!(resp.item.state, ProposalItemState::Rejected);
        assert_eq!(resp.item.rejected_reason.as_deref(), Some("not now"));
        assert!(resp.item.result_task_id.is_none());
        // Single item, rejected → dismissed.
        assert_eq!(resp.aggregate_state, ProposalAggregateState::Dismissed);
        assert_eq!(receipt_count(&conn), 1);
    }

    #[test]
    fn unknown_item_is_not_found() {
        let conn = db();
        let r = decide_item(&conn, "nope", "nope:0", ProposalDecision::Accept, None, "k");
        assert!(matches!(r, Err(DecisionError::NotFound)));
    }

    #[test]
    fn accept_rolls_back_fully_when_the_receipt_insert_fails() {
        // The whole decision — task create+link AND receipt — is one SAVEPOINT.
        // If the in-tx receipt insert fails, the created task must NOT persist and
        // the item must stay pending (retryable), never a half-applied decision.
        let conn = db();
        ingest_message_proposals(
            &conn,
            "d1",
            "msg1",
            "```kronn-plan-action\n{\"action\":\"create\",\"title\":\"Rollme\"}\n```",
        )
        .unwrap();
        let tasks_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM planning_tasks", [], |r| r.get(0))
            .unwrap();
        // Break the messages table so the in-tx receipt insert fails AFTER the
        // task create+link have run inside the same savepoint.
        conn.execute("ALTER TABLE messages RENAME TO messages_bak", [])
            .unwrap();
        let outcome = decide_item(
            &conn,
            "proposal:msg1:0",
            "proposal:msg1:0:0",
            ProposalDecision::Accept,
            None,
            "k",
        );
        conn.execute("ALTER TABLE messages_bak RENAME TO messages", [])
            .unwrap();
        assert!(outcome.is_err(), "a failed receipt must fail the decision");

        let tasks_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM planning_tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            tasks_after, tasks_before,
            "the created task must be rolled back"
        );
        let item = get_proposal(&conn, "proposal:msg1:0")
            .unwrap()
            .unwrap()
            .items[0]
            .clone();
        assert_eq!(
            item.state,
            ProposalItemState::Pending,
            "item stays pending, retryable"
        );
    }

    #[test]
    fn a_corrupt_item_state_fails_closed() {
        // Even if a row is corrupted past the CHECK constraint, reads and the
        // decision must ERROR — never silently treat it as pending (which could
        // re-mutate an already-decided item).
        let conn = db();
        ingest_message_proposals(
            &conn,
            "d1",
            "msg1",
            "```kronn-plan-action\n{\"action\":\"create\",\"title\":\"X\"}\n```",
        )
        .unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints = ON")
            .unwrap();
        conn.execute(
            "UPDATE planning_proposal_items SET state = 'bogus' WHERE id = 'proposal:msg1:0:0'",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints = OFF")
            .unwrap();

        assert!(
            list_proposals(&conn, "d1", false).is_err(),
            "reads fail closed"
        );
        let r = decide_item(
            &conn,
            "proposal:msg1:0",
            "proposal:msg1:0:0",
            ProposalDecision::Accept,
            None,
            "k",
        );
        assert!(r.is_err(), "decision on a corrupt item fails closed");
        assert_eq!(
            receipt_count(&conn),
            0,
            "no receipt on a fail-closed decision"
        );
    }

    #[test]
    fn empty_or_oversized_idempotency_key_is_rejected() {
        let conn = db();
        ingest_message_proposals(
            &conn,
            "d1",
            "msg1",
            "```kronn-plan-action\n{\"action\":\"create\",\"title\":\"X\"}\n```",
        )
        .unwrap();
        let empty = decide_item(
            &conn,
            "proposal:msg1:0",
            "proposal:msg1:0:0",
            ProposalDecision::Accept,
            None,
            "   ",
        );
        assert!(matches!(empty, Err(DecisionError::Invalid(_))));
        let huge = "x".repeat(200);
        let oversized = decide_item(
            &conn,
            "proposal:msg1:0",
            "proposal:msg1:0:0",
            ProposalDecision::Accept,
            None,
            &huge,
        );
        assert!(matches!(oversized, Err(DecisionError::Invalid(_))));
        assert_eq!(
            receipt_count(&conn),
            0,
            "a rejected request applied nothing"
        );
    }
}
