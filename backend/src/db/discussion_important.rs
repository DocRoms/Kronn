//! KT-619 — important messages: steering cards, kept apart from worker reports.
//!
//! An important message is its own persisted object, not a formatting
//! convention. Publication mints a new message and one row here, in one
//! transaction, and no service path attaches a card to a message it did not
//! just write — so a `delivery_summary` stays a `delivery_summary` even when it
//! carries an important fact.
//!
//! The scope of that guarantee is the service. None of it constrains a
//! privileged client writing SQL directly, and nothing here should be read as
//! claiming otherwise.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[cfg(test)]
mod tests;

/// Contract version pinned by every [`ImportantSpec`]. Single source of truth:
/// bumping it moves the accepted `version` with it.
pub const IMPORTANT_SCHEMA_VERSION: u32 = 1;

/// Same ceiling as the question fence, for the same reason: refuse before
/// parsing rather than after allocating.
const MAX_SPEC_BYTES: usize = 24_000;

/// The closed set. A category is part of the contract, so a new one is a
/// deliberate migration, never a free-text label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ImportantCategory {
    Decision,
    ScopeChange,
    DodWaiver,
    BlockingAlert,
    HumanActionRequired,
    AcceptedDelivery,
}

impl ImportantCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::ScopeChange => "scope_change",
            Self::DodWaiver => "dod_waiver",
            Self::BlockingAlert => "blocking_alert",
            Self::HumanActionRequired => "human_action_required",
            Self::AcceptedDelivery => "accepted_delivery",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "decision" => Self::Decision,
            "scope_change" => Self::ScopeChange,
            "dod_waiver" => Self::DodWaiver,
            "blocking_alert" => Self::BlockingAlert,
            "human_action_required" => Self::HumanActionRequired,
            "accepted_delivery" => Self::AcceptedDelivery,
            _ => return None,
        })
    }
}

/// Who may publish. There is deliberately no worker variant: a worker cannot
/// name itself here, so the refusal does not depend on the API layer being
/// asked politely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ImportantAuthorKind {
    Orchestrator,
    Human,
}

impl ImportantAuthorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Orchestrator => "orchestrator",
            Self::Human => "human",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "orchestrator" => Self::Orchestrator,
            "human" => Self::Human,
            _ => return None,
        })
    }
}

/// Objects the card points at. Every field is optional because a card may
/// legitimately predate the object it will later concern.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ImportantReferences {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dod_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
}

/// `required: false` is the explicit "none" the contract asks for, so an
/// omitted action and a deliberate absence of action cannot be confused.
/// Validation refuses a half-filled action in either direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ImportantAction {
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
}

/// The bounded template a publisher submits.
///
/// `deny_unknown_fields` is what keeps `author`, `created_at` and
/// `schema_version` server-owned: a payload that tries to carry them fails to
/// parse instead of being silently trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ImportantSpec {
    pub version: u32,
    pub category: ImportantCategory,
    /// Stable identity of the reported fact. Replays collapse onto it.
    pub dedup_key: String,
    pub title: String,
    /// The one line that must survive being read at a glance.
    pub highlight: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub impact: String,
    pub action_required: ImportantAction,
    #[serde(default)]
    pub references: ImportantReferences,
}

/// A persisted card, as read back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportantMessage {
    pub id: String,
    pub discussion_id: String,
    pub message_id: String,
    pub category: ImportantCategory,
    pub schema_version: u32,
    pub dedup_key: String,
    pub title: String,
    pub highlight: String,
    pub context: Option<String>,
    pub impact: String,
    pub action_required: ImportantAction,
    pub references: ImportantReferences,
    pub author_kind: ImportantAuthorKind,
    pub author_label: String,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
    pub created_at: String,
    /// Transcript position, so the filter and previous/next agree with the list.
    pub sort_order: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportantMessageList {
    pub items: Vec<ImportantMessage>,
    pub total: u32,
}

fn nonempty_bounded(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max
}

fn stable_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}

fn optional_bounded(value: Option<&String>, max: usize) -> bool {
    value.is_none_or(|s| nonempty_bounded(s, max))
}

/// Reject the whole payload rather than repair it. A card that reached the
/// transcript half-validated would be worse than one that never posted.
pub fn parse_spec(body: &str) -> Option<ImportantSpec> {
    if body.len() > MAX_SPEC_BYTES {
        return None;
    }
    let spec: ImportantSpec = serde_json::from_str(body).ok()?;
    validate_spec(&spec).then_some(spec)
}

pub fn validate_spec(spec: &ImportantSpec) -> bool {
    let action_ok = if spec.action_required.required {
        nonempty_bounded(spec.action_required.action.as_deref().unwrap_or(""), 500)
            && nonempty_bounded(spec.action_required.owner.as_deref().unwrap_or(""), 200)
            && optional_bounded(spec.action_required.due.as_ref(), 100)
    } else {
        // An explicit "no action" carries no action fields at all; a stray one
        // means the publisher meant something it did not say.
        spec.action_required.action.is_none()
            && spec.action_required.owner.is_none()
            && spec.action_required.due.is_none()
    };

    spec.version == IMPORTANT_SCHEMA_VERSION
        && stable_key(&spec.dedup_key)
        && nonempty_bounded(&spec.title, 200)
        && nonempty_bounded(&spec.highlight, 500)
        && spec.context.as_ref().is_none_or(|s| s.chars().count() <= 2000)
        && nonempty_bounded(&spec.impact, 1000)
        && action_ok
        && optional_bounded(spec.references.task_ref.as_ref(), 100)
        && optional_bounded(spec.references.dod_id.as_ref(), 100)
        && optional_bounded(spec.references.execution_id.as_ref(), 100)
        && optional_bounded(spec.references.agent.as_ref(), 100)
        && optional_bounded(spec.references.commit.as_ref(), 100)
        && optional_bounded(spec.references.artifact.as_ref(), 500)
}

/// Attach a card to the message just written for it, in that same transaction.
///
/// Returns the existing row when `dedup_key` was already used in this
/// discussion, so a replayed steering event is a no-op rather than a duplicate.
///
/// `pub(crate)` on purpose: no route hands this an arbitrary `message_id`, and
/// the newest-message check below refuses one anyway. Together those are what
/// keep an ordinary message from being converted — `UNIQUE(message_id)` alone
/// would only stop a second card on a message that already had one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn publish(
    conn: &Connection,
    id: &str,
    discussion_id: &str,
    message_id: &str,
    spec: &ImportantSpec,
    author_kind: ImportantAuthorKind,
    author_label: &str,
    source: Option<(&str, &str)>,
    created_at: &str,
) -> Result<String> {
    if !validate_spec(spec) {
        anyhow::bail!("important message payload failed validation");
    }
    if !is_newest_message(conn, discussion_id, message_id)? {
        anyhow::bail!("important messages attach only to the message just published");
    }
    if let Some(existing) = find_id_by_dedup_key(conn, discussion_id, &spec.dedup_key)? {
        return Ok(existing);
    }
    let payload = serde_json::to_string(spec)?;
    let (source_kind, source_id) = match source {
        Some((kind, id)) => (Some(kind), Some(id)),
        None => (None, None),
    };
    conn.execute(
        "INSERT INTO discussion_important_messages (
             id, discussion_id, message_id, category, schema_version, dedup_key,
             payload_json, author_kind, author_label, source_kind, source_id, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            id,
            discussion_id,
            message_id,
            spec.category.as_str(),
            IMPORTANT_SCHEMA_VERSION,
            spec.dedup_key,
            payload,
            author_kind.as_str(),
            author_label,
            source_kind,
            source_id,
            created_at,
        ],
    )?;
    Ok(id.to_string())
}

/// Is this the newest message in its discussion?
///
/// Inside the publishing transaction the message just inserted holds the top
/// `sort_order`, so this passes for a genuine publication and fails for any
/// attempt to attach a card to an older message. It bounds what the service
/// accepts; it is not a claim about a client writing SQL directly.
fn is_newest_message(conn: &Connection, discussion_id: &str, message_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT m.id = ?2 FROM messages m \
              WHERE m.discussion_id = ?1 \
              ORDER BY m.sort_order DESC LIMIT 1",
            params![discussion_id, message_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

pub fn find_id_by_dedup_key(
    conn: &Connection,
    discussion_id: &str,
    dedup_key: &str,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM discussion_important_messages \
              WHERE discussion_id = ?1 AND dedup_key = ?2",
            params![discussion_id, dedup_key],
            |row| row.get(0),
        )
        .optional()?)
}

const SELECT_COLUMNS: &str = "i.id, i.discussion_id, i.message_id, i.category, i.schema_version, \
                              i.dedup_key, i.payload_json, i.author_kind, i.author_label, \
                              i.source_kind, i.source_id, i.created_at, m.sort_order";

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<ImportantMessage>> {
    let category: String = row.get(3)?;
    let author_kind: String = row.get(7)?;
    let payload: String = row.get(6)?;
    // A row whose category or author no longer parses is a contract break, not
    // a card to render half-way; skip it rather than guess.
    let (Some(category), Some(author_kind), Ok(spec)) = (
        ImportantCategory::from_str(&category),
        ImportantAuthorKind::from_str(&author_kind),
        serde_json::from_str::<ImportantSpec>(&payload),
    ) else {
        return Ok(None);
    };
    Ok(Some(ImportantMessage {
        id: row.get(0)?,
        discussion_id: row.get(1)?,
        message_id: row.get(2)?,
        category,
        schema_version: row.get(4)?,
        dedup_key: row.get(5)?,
        title: spec.title,
        highlight: spec.highlight,
        context: spec.context,
        impact: spec.impact,
        action_required: spec.action_required,
        references: spec.references,
        author_kind,
        author_label: row.get(8)?,
        source_kind: row.get(9)?,
        source_id: row.get(10)?,
        created_at: row.get(11)?,
        sort_order: row.get(12)?,
    }))
}

/// Every card in a discussion, in transcript order, optionally one category.
pub fn list(
    conn: &Connection,
    discussion_id: &str,
    category: Option<ImportantCategory>,
) -> Result<ImportantMessageList> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS} \
           FROM discussion_important_messages i \
           JOIN messages m ON m.id = i.message_id \
          WHERE i.discussion_id = ?1 AND (?2 IS NULL OR i.category = ?2) \
          ORDER BY m.sort_order ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![discussion_id, category.map(|c| c.as_str())],
        |row| row_to_message(row),
    )?;
    let mut items = Vec::new();
    for row in rows {
        if let Some(item) = row? {
            items.push(item);
        }
    }
    let total = items.len() as u32;
    Ok(ImportantMessageList { items, total })
}

/// Cheap counter for the filter chip; does not build the payloads.
pub fn count(conn: &Connection, discussion_id: &str) -> Result<u32> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM discussion_important_messages WHERE discussion_id = ?1",
        [discussion_id],
        |row| row.get(0),
    )?)
}

/// Who is publishing, resolved from Kronn-attached context.
///
/// Never from a request field a caller chooses: a worker can name any room it
/// likes in `disc_id`, so authority is read from the caller's own durable room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportantPublisher {
    Orchestrator(String),
    Human(String),
    /// An active worker. It cannot publish, and is told so rather than ignored.
    Worker,
}

impl ImportantPublisher {
    fn authority(&self) -> Option<(ImportantAuthorKind, &str)> {
        match self {
            Self::Orchestrator(label) => Some((ImportantAuthorKind::Orchestrator, label)),
            Self::Human(label) => Some((ImportantAuthorKind::Human, label)),
            Self::Worker => None,
        }
    }
}

/// Resolve who is publishing from the caller's own durable room.
///
/// Deliberately ignores both the target room and the claimed role. A worker can
/// name any `disc_id` and can label its own turn `User`; what it cannot do is
/// change which room its session sits in. That room is written by Kronn at
/// offer acceptance, so it is the one fact here the caller does not author.
///
/// A session in a room that is or was an execution room is treated as a worker.
/// Erring that way costs an orchestrator nothing — it does not sit in one.
pub fn publisher_for_author(
    conn: &Connection,
    author_cli_session_id: Option<i64>,
    role_is_human: bool,
    label: &str,
) -> Result<ImportantPublisher> {
    if let Some(session_pk) = author_cli_session_id {
        let home: Option<String> = conn
            .query_row(
                "SELECT disc_id FROM discussion_sessions WHERE id = ?1",
                params![session_pk],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(home) = home {
            if crate::db::orchestration::discussion_is_execution_room(conn, &home)? {
                return Ok(ImportantPublisher::Worker);
            }
        }
    }
    Ok(if role_is_human {
        ImportantPublisher::Human(label.to_string())
    } else {
        ImportantPublisher::Orchestrator(label.to_string())
    })
}

/// What one message's fences produced. Counts are reported back so a refused
/// publisher learns it was refused instead of assuming it succeeded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportantIngest {
    pub published: u32,
    pub deduplicated: u32,
    pub refused_worker: u32,
    pub invalid: u32,
    /// Fences past the first in one message. One card is one message, so the
    /// extras are refused rather than silently collapsed by the unique index.
    pub refused_extra: u32,
}

impl ImportantIngest {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Track every fence, including examples, so a nested example never publishes.
fn important_fences(content: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut open: Option<(char, usize, bool)> = None;
    let mut body = String::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some((marker, count, important)) = open {
            let closing_count = trimmed.chars().take_while(|c| *c == marker).count();
            if closing_count >= count && trimmed[closing_count..].trim().is_empty() {
                if important {
                    result.push(std::mem::take(&mut body));
                }
                open = None;
            } else if important {
                body.push_str(line);
                body.push('\n');
            }
        } else if line.len() - trimmed.len() <= 3 {
            let Some(marker @ ('`' | '~')) = trimmed.chars().next() else {
                continue;
            };
            let count = trimmed.chars().take_while(|c| *c == marker).count();
            if count >= 3 {
                open = Some((marker, count, trimmed[count..].trim() == "kronn-important"));
                body.clear();
            }
        }
    }
    result
}

/// Persist the important card a message carries, if it may carry one.
///
/// Runs in the caller's transaction so a card exists if and only if its message
/// does. A replayed steering event collapses onto `dedup_key`, which is why the
/// key is the fact's identity and not the message's.
pub fn ingest_message_important(
    conn: &Connection,
    discussion_id: &str,
    message_id: &str,
    content: &str,
    publisher: &ImportantPublisher,
    created_at: &str,
) -> Result<ImportantIngest> {
    let mut ingest = ImportantIngest::default();
    if !content.contains("kronn-important") {
        return Ok(ingest);
    }
    let fences = important_fences(content);
    if fences.is_empty() {
        return Ok(ingest);
    }

    let Some((author_kind, author_label)) = publisher.authority() else {
        ingest.refused_worker = fences.len() as u32;
        return Ok(ingest);
    };

    for (index, body) in fences.into_iter().enumerate() {
        if index > 0 {
            ingest.refused_extra += 1;
            continue;
        }
        let Some(spec) = parse_spec(&body) else {
            ingest.invalid += 1;
            continue;
        };
        if find_id_by_dedup_key(conn, discussion_id, &spec.dedup_key)?.is_some() {
            ingest.deduplicated += 1;
            continue;
        }
        publish(
            conn,
            &format!("important:{message_id}"),
            discussion_id,
            message_id,
            &spec,
            author_kind,
            author_label,
            None,
            created_at,
        )?;
        ingest.published += 1;
    }
    Ok(ingest)
}

/// The card carried by a given message, if that message is one.
pub fn get_by_message(conn: &Connection, message_id: &str) -> Result<Option<ImportantMessage>> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS} \
           FROM discussion_important_messages i \
           JOIN messages m ON m.id = i.message_id \
          WHERE i.message_id = ?1"
    );
    Ok(conn
        .query_row(&sql, [message_id], |row| row_to_message(row))
        .optional()?
        .flatten())
}
