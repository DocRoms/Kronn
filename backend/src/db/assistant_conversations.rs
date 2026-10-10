//! Links a configuration assistant's discussion to what it configures (KT-1111).
//!
//! The discussion itself is an ordinary one; this table only says "this is an
//! assistant conversation about X" so the UI can file it apart and reopen it.

use anyhow::Result;
use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::AgentType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AssistantKind {
    /// The Custom API plugin form (create or edit).
    CustomApi,
    /// An ApiCall step of a workflow or a Quick API.
    ApiCallStep,
}

impl AssistantKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CustomApi => "custom_api",
            Self::ApiCallStep => "api_call_step",
        }
    }

    fn parse(raw: &str) -> rusqlite::Result<Self> {
        match raw {
            "custom_api" => Ok(Self::CustomApi),
            "api_call_step" => Ok(Self::ApiCallStep),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                format!("unknown assistant kind '{other}'").into(),
            )),
        }
    }
}

/// Secret values a client sends with a request so the server can mask them.
/// Used for that request only: never stored, never logged.
#[derive(Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct TransientSecrets(Vec<String>);

impl TransientSecrets {
    pub fn values(&self) -> &[String] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|value| value.trim().is_empty())
    }
}

impl std::fmt::Debug for TransientSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TransientSecrets({} redacted)", self.0.len())
    }
}

#[cfg(test)]
impl From<Vec<String>> for TransientSecrets {
    fn from(values: Vec<String>) -> Self {
        Self(values)
    }
}

/// What a discussion is created for when it is an assistant conversation:
/// the link is written in the discussion's own transaction, and `secrets`
/// are masked out of its title and first message before anything is stored.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct AssistantContext {
    pub kind: AssistantKind,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub target_id: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub target_step: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub plugin_id: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub target_label: Option<String>,
    #[serde(default)]
    #[ts(as = "Option<Vec<String>>", optional)]
    pub secrets: TransientSecrets,
}

/// One kept assistant conversation, with the discussion fields the lists show.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AssistantConversation {
    pub discussion_id: String,
    pub kind: AssistantKind,
    /// Custom API server id, or the workflow / Quick API owning the step.
    /// `None` while the configured object is not saved yet.
    pub target_id: Option<String>,
    /// ApiCall step name; `None` for a plugin.
    pub target_step: Option<String>,
    /// The API plugin the conversation is about, when known.
    pub plugin_id: Option<String>,
    pub target_label: String,
    /// Signature of the last proposal the assistant made, if any.
    pub last_proposal_signature: Option<String>,
    /// Signature of the last proposal the user applied, if any.
    pub last_applied_signature: Option<String>,
    pub last_applied_at: Option<String>,
    pub created_at: String,
    pub title: String,
    pub agent: AgentType,
    pub archived: bool,
    pub message_count: u32,
    pub updated_at: String,
}

/// A link as stored, for the logical export.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AssistantConversationLink {
    pub discussion_id: String,
    pub kind: AssistantKind,
    pub target_id: Option<String>,
    pub target_step: Option<String>,
    pub plugin_id: Option<String>,
    pub target_label: String,
    pub last_proposal_signature: Option<String>,
    pub last_applied_signature: Option<String>,
    pub last_applied_at: Option<String>,
    pub created_at: String,
}

pub fn list_links(conn: &Connection) -> Result<Vec<AssistantConversationLink>> {
    let mut stmt = conn.prepare(
        "SELECT discussion_id, kind, target_id, target_step, plugin_id, target_label,
                last_proposal_signature, last_applied_signature, last_applied_at, created_at
           FROM assistant_conversations ORDER BY created_at",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let kind: String = row.get(1)?;
            Ok(AssistantConversationLink {
                discussion_id: row.get(0)?,
                kind: AssistantKind::parse(&kind)?,
                target_id: row.get(2)?,
                target_step: row.get(3)?,
                plugin_id: row.get(4)?,
                target_label: row.get(5)?,
                last_proposal_signature: row.get(6)?,
                last_applied_signature: row.get(7)?,
                last_applied_at: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Restores an exported link verbatim; its discussion must already exist.
pub fn insert_link_row(conn: &Connection, link: &AssistantConversationLink) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO assistant_conversations
             (discussion_id, kind, target_id, target_step, plugin_id, target_label,
              last_proposal_signature, last_applied_signature, last_applied_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            link.discussion_id,
            link.kind.as_str(),
            link.target_id,
            link.target_step,
            link.plugin_id,
            link.target_label,
            link.last_proposal_signature,
            link.last_applied_signature,
            link.last_applied_at,
            link.created_at,
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct NewLink {
    pub discussion_id: String,
    pub kind: Option<AssistantKind>,
    pub target_id: Option<String>,
    pub target_step: Option<String>,
    pub plugin_id: Option<String>,
    pub target_label: String,
}

/// Partial update: `None` leaves a column as it is.
#[derive(Debug, Clone, Default)]
pub struct LinkPatch {
    pub target_id: Option<String>,
    pub target_step: Option<String>,
    pub target_label: Option<String>,
    pub last_proposal_signature: Option<String>,
    pub last_applied_signature: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub kind: Option<AssistantKind>,
    pub target_id: Option<String>,
    /// List only links whose target is not saved yet.
    pub unattached: bool,
    /// With `target_id`, also list links not attached yet (a draft whose
    /// attachment failed) for the same step and plugin.
    pub include_unattached: bool,
    /// With `include_unattached`: unattached links this client still owes
    /// to the target (an attachment that failed), by discussion id.
    pub pending_ids: Vec<String>,
    /// With `include_unattached` for a step: the draft name its unattached
    /// links carry, as the saved step is known by its durable id.
    pub unattached_step: Option<String>,
    pub target_step: Option<String>,
    pub plugin_id: Option<String>,
}

const SELECT: &str = "SELECT a.discussion_id, a.kind, a.target_id, a.target_step, a.plugin_id,
        a.target_label, a.last_proposal_signature, a.last_applied_signature,
        a.last_applied_at, a.created_at, d.title, d.agent, d.archived,
        (SELECT COUNT(*) FROM messages m
           WHERE m.discussion_id = d.id AND m.role != 'System'),
        d.updated_at
   FROM assistant_conversations a
   JOIN discussions d ON d.id = a.discussion_id";

fn map_row(row: &rusqlite::Row) -> rusqlite::Result<AssistantConversation> {
    let kind: String = row.get(1)?;
    let agent: String = row.get(11)?;
    Ok(AssistantConversation {
        discussion_id: row.get(0)?,
        kind: AssistantKind::parse(&kind)?,
        target_id: row.get(2)?,
        target_step: row.get(3)?,
        plugin_id: row.get(4)?,
        target_label: row.get(5)?,
        last_proposal_signature: row.get(6)?,
        last_applied_signature: row.get(7)?,
        last_applied_at: row.get(8)?,
        created_at: row.get(9)?,
        title: row.get(10)?,
        agent: crate::db::discussions::parse_agent_type(&agent)?,
        archived: row.get::<_, i64>(12)? != 0,
        message_count: row.get::<_, u32>(13)?,
        updated_at: row.get(14)?,
    })
}

/// Returns `false` when the discussion does not exist. Re-linking an already
/// linked discussion keeps its first kind and creation date.
pub fn link(conn: &Connection, new: &NewLink) -> Result<bool> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM discussions WHERE id = ?1",
            params![new.discussion_id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !exists {
        return Ok(false);
    }
    let kind = new.kind.unwrap_or(AssistantKind::CustomApi);
    conn.execute(
        "INSERT INTO assistant_conversations
             (discussion_id, kind, target_id, target_step, plugin_id, target_label, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(discussion_id) DO UPDATE SET
             target_id = COALESCE(excluded.target_id, assistant_conversations.target_id),
             target_step = COALESCE(excluded.target_step, assistant_conversations.target_step),
             plugin_id = COALESCE(excluded.plugin_id, assistant_conversations.plugin_id),
             target_label = excluded.target_label",
        params![
            new.discussion_id,
            kind.as_str(),
            new.target_id,
            new.target_step,
            new.plugin_id,
            new.target_label,
            chrono::Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(true)
}

/// Returns `false` when the discussion is not an assistant conversation.
pub fn update(conn: &Connection, discussion_id: &str, patch: &LinkPatch) -> Result<bool> {
    let applied_at = patch
        .last_applied_signature
        .as_ref()
        .map(|_| chrono::Utc::now().to_rfc3339());
    let changed = conn.execute(
        "UPDATE assistant_conversations SET
             target_id = COALESCE(?2, target_id),
             target_label = COALESCE(?3, target_label),
             last_proposal_signature = COALESCE(?4, last_proposal_signature),
             last_applied_signature = COALESCE(?5, last_applied_signature),
             last_applied_at = COALESCE(?6, last_applied_at),
             target_step = COALESCE(?7, target_step)
         WHERE discussion_id = ?1",
        params![
            discussion_id,
            patch.target_id,
            patch.target_label,
            patch.last_proposal_signature,
            patch.last_applied_signature,
            applied_at,
            patch.target_step,
        ],
    )?;
    Ok(changed > 0)
}

pub fn get(conn: &Connection, discussion_id: &str) -> Result<Option<AssistantConversation>> {
    let sql = format!("{SELECT} WHERE a.discussion_id = ?1");
    Ok(conn
        .query_row(&sql, params![discussion_id], map_row)
        .optional()?)
}

/// Newest activity first.
pub fn list(conn: &Connection, filter: &ListFilter) -> Result<Vec<AssistantConversation>> {
    let sql = format!(
        "{SELECT}
         WHERE (?1 IS NULL OR a.kind = ?1)
           AND (
             -- Owed to this target by this client: shown whatever its step.
             (?6 AND a.target_id IS NULL
                 AND a.discussion_id IN (SELECT value FROM json_each(?7)))
             OR (
               (?2 IS NULL OR a.target_id = ?2
                    OR (?6 AND ?8 IS NOT NULL AND a.target_id IS NULL
                        AND a.target_step = ?8))
               AND (?3 = 0 OR a.target_id IS NULL)
               AND (?4 IS NULL OR a.target_step = ?4
                    OR (?6 AND a.target_id IS NULL AND a.target_step = ?8))
               AND (?5 IS NULL OR a.plugin_id = ?5 OR (?6 AND a.target_id IS NOT NULL))
             )
           )
         ORDER BY d.updated_at DESC, a.created_at DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(
            params![
                filter.kind.map(AssistantKind::as_str),
                filter.target_id,
                filter.unattached,
                filter.target_step,
                filter.plugin_id,
                filter.include_unattached,
                serde_json::to_string(&filter.pending_ids)?,
                filter.unattached_step,
            ],
            map_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ─── Masking at every write ─────────────────────────────────────────────────

/// Age past which a turn is checked against its dispatch: kept while the
/// dispatch is queued or running, dropped once it is terminal or gone.
const TURN_RECHECK_AFTER: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);

struct Turn {
    discussion_id: String,
    secrets: crate::core::secret_scrub::SecretSet,
    started: std::time::Instant,
}

/// Per database: the key that decrypts the plugins' stored credentials, and
/// the values a client named for a turn still running, in memory only and
/// keyed by dispatch so one turn's end never drops another's.
#[derive(Default)]
pub struct AssistantGuard {
    key: std::sync::RwLock<Option<String>>,
    turns: std::sync::Mutex<HashMap<String, Turn>>,
}

impl std::fmt::Debug for AssistantGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AssistantGuard(redacted)")
    }
}

impl AssistantGuard {
    pub fn set_key(&self, key: Option<String>) {
        if let Ok(mut slot) = self.key.write() {
            *slot = key;
        }
    }

    fn key(&self) -> Option<String> {
        self.key.read().ok().and_then(|key| key.clone())
    }

    /// Keeps `secrets` for the replies of dispatch `turn_id` in `discussion_id`.
    pub fn begin_turn(&self, turn_id: &str, discussion_id: &str, secrets: &TransientSecrets) {
        if secrets.is_empty() {
            return;
        }
        let mut set = crate::core::secret_scrub::SecretSet::new();
        for value in secrets.values() {
            set.add(value);
        }
        if let Ok(mut turns) = self.turns.lock() {
            turns.insert(
                turn_id.to_string(),
                Turn {
                    discussion_id: discussion_id.to_string(),
                    secrets: set,
                    started: std::time::Instant::now(),
                },
            );
        }
    }

    pub fn end_turn(&self, turn_id: &str) {
        if let Ok(mut turns) = self.turns.lock() {
            turns.remove(turn_id);
        }
    }

    fn transient_for(
        &self,
        conn: &Connection,
        discussion_id: &str,
    ) -> Result<crate::core::secret_scrub::SecretSet> {
        let mut set = crate::core::secret_scrub::SecretSet::new();
        let Ok(mut turns) = self.turns.lock() else {
            return Ok(set);
        };
        let aged: Vec<String> = turns
            .iter()
            .filter(|(_, turn)| turn.started.elapsed() >= TURN_RECHECK_AFTER)
            .map(|(id, _)| id.clone())
            .collect();
        for id in aged {
            // A turn whose end was missed: its dispatch tells whether it is over.
            let status: Option<String> = conn
                .query_row(
                    "SELECT status FROM agent_dispatch_jobs WHERE id = ?1",
                    [&id],
                    |row| row.get(0),
                )
                .optional()?;
            if !matches!(status.as_deref(), Some("Pending" | "Running")) {
                turns.remove(&id);
            }
        }
        for turn in turns
            .values()
            .filter(|turn| turn.discussion_id == discussion_id)
        {
            set.extend(&turn.secrets);
        }
        Ok(set)
    }

    /// Ages a turn, for the tests that cannot wait two hours.
    #[cfg(test)]
    pub fn age_turn(&self, turn_id: &str, by: std::time::Duration) {
        if let Ok(mut turns) = self.turns.lock() {
            if let Some(turn) = turns.get_mut(turn_id) {
                turn.started = turn.started.checked_sub(by).unwrap_or(turn.started);
            }
        }
    }

    #[cfg(test)]
    pub fn live_turns(&self) -> usize {
        self.turns.lock().map(|turns| turns.len()).unwrap_or(0)
    }
}

thread_local! {
    static LENT: std::cell::RefCell<Option<std::sync::Arc<AssistantGuard>>> =
        const { std::cell::RefCell::new(None) };
}

/// Restores the previous lender when dropped.
pub struct LentGuard(Option<std::sync::Arc<AssistantGuard>>);

impl Drop for LentGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        LENT.with(|slot| *slot.borrow_mut() = previous);
    }
}

/// Makes `guard` the one the writers of this thread mask with.
pub fn lend_guard(guard: std::sync::Arc<AssistantGuard>) -> LentGuard {
    LentGuard(LENT.with(|slot| slot.borrow_mut().replace(guard)))
}

/// The values of every stored config of `server_id`, in their wire forms.
pub(crate) fn stored_plugin_secrets(
    conn: &Connection,
    server_id: &str,
    key: &str,
) -> Result<crate::core::secret_scrub::SecretSet> {
    let mut set = crate::core::secret_scrub::SecretSet::new();
    let server = crate::db::mcps::list_servers(conn)?
        .into_iter()
        .find(|server| server.id == server_id);
    let mut stmt = conn.prepare("SELECT env_encrypted FROM mcp_configs WHERE server_id = ?1")?;
    let encrypted: Vec<String> = stmt
        .query_map([server_id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for blob in encrypted.iter().filter(|blob| !blob.is_empty()) {
        match crate::db::mcps::decrypt_env(blob, key) {
            Ok(env) => {
                for value in env.values() {
                    set.add(value);
                }
                if let Some(server) = &server {
                    set.extend(&crate::workflows::api_call_executor::call_secrets(
                        server, &env,
                    ));
                }
            }
            Err(_) => tracing::warn!(
                target: "kronn::assistant",
                server_id,
                "a stored config could not be decrypted to mask its values"
            ),
        }
    }
    Ok(set)
}

/// The plugin whose stored credentials an assistant conversation may meet.
pub(crate) fn plugin_of(
    kind: AssistantKind,
    target_id: Option<&str>,
    plugin_id: Option<&str>,
) -> Option<String> {
    match kind {
        AssistantKind::CustomApi => target_id.map(str::to_string),
        AssistantKind::ApiCallStep => plugin_id.map(str::to_string),
    }
}

/// What may be written into `discussion_id`: in an assistant conversation,
/// its plugin's stored credentials, the values of its live turns and common
/// token shapes are masked; elsewhere only the values of a live turn.
pub fn mask_for_discussion(conn: &Connection, discussion_id: &str, text: &str) -> Result<String> {
    let link: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT kind, target_id, plugin_id FROM assistant_conversations
              WHERE discussion_id = ?1",
            params![discussion_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let guard = LENT.with(|slot| slot.borrow().clone());
    let mut set = match guard.as_ref() {
        Some(guard) => guard.transient_for(conn, discussion_id)?,
        None => crate::core::secret_scrub::SecretSet::new(),
    };
    let Some((kind, target_id, plugin_id)) = link else {
        return Ok(if set.is_empty() {
            text.to_string()
        } else {
            set.scrub(text)
        });
    };
    let plugin = plugin_of(
        AssistantKind::parse(&kind)?,
        target_id.as_deref(),
        plugin_id.as_deref(),
    );
    if let (Some(server_id), Some(key)) = (plugin, guard.as_ref().and_then(|g| g.key())) {
        set.extend(&stored_plugin_secrets(conn, &server_id, &key)?);
    }
    Ok(crate::core::redact::redact_secrets(&set.scrub(text)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::models::{AgentType, Discussion, ModelTier};
    use chrono::Utc;

    fn discussion(id: &str) -> Discussion {
        let now = Utc::now();
        Discussion {
            connection_id: None,
            awaiting_agent: false,
            agent_running: false,
            id: id.into(),
            project_id: None,
            title: format!("Assistant {id}"),
            agent: AgentType::ClaudeCode,
            language: "en".into(),
            participants: vec![AgentType::ClaudeCode],
            messages: vec![],
            message_count: 0,
            non_system_message_count: 0,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            archived: false,
            pinned: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            tier: ModelTier::Default,
            model: None,
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::OnDemand,
            introspection_call_count: 0,
            shared_id: None,
            shared_with: vec![],
            workflow_run_id: None,
            test_mode_restore_branch: None,
            test_mode_stash_ref: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn custom_api(id: &str, target: Option<&str>) -> NewLink {
        NewLink {
            discussion_id: id.into(),
            kind: Some(AssistantKind::CustomApi),
            target_id: target.map(str::to_string),
            target_label: "Insider".into(),
            ..NewLink::default()
        }
    }

    #[tokio::test]
    async fn link_lists_by_target_and_survives_relinking() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            for id in ["a", "b", "c"] {
                crate::db::discussions::insert_discussion(conn, &discussion(id))?;
            }
            assert!(link(conn, &custom_api("a", Some("srv-1")))?);
            assert!(link(conn, &custom_api("b", None))?);
            assert!(link(
                conn,
                &NewLink {
                    discussion_id: "c".into(),
                    kind: Some(AssistantKind::ApiCallStep),
                    target_id: Some("wf-1".into()),
                    target_step: Some("fetch".into()),
                    plugin_id: Some("srv-1".into()),
                    target_label: "fetch".into(),
                }
            )?);
            assert!(!link(conn, &custom_api("missing", None))?);

            let plugin = list(
                conn,
                &ListFilter {
                    kind: Some(AssistantKind::CustomApi),
                    target_id: Some("srv-1".into()),
                    ..ListFilter::default()
                },
            )?;
            assert_eq!(plugin.len(), 1);
            assert_eq!(plugin[0].discussion_id, "a");
            assert_eq!(plugin[0].agent, AgentType::ClaudeCode);

            let unattached = list(
                conn,
                &ListFilter {
                    kind: Some(AssistantKind::CustomApi),
                    unattached: true,
                    ..ListFilter::default()
                },
            )?;
            assert_eq!(unattached.len(), 1);
            assert_eq!(unattached[0].discussion_id, "b");

            let step = list(
                conn,
                &ListFilter {
                    kind: Some(AssistantKind::ApiCallStep),
                    target_id: Some("wf-1".into()),
                    target_step: Some("fetch".into()),
                    ..ListFilter::default()
                },
            )?;
            assert_eq!(step.len(), 1);
            assert_eq!(list(conn, &ListFilter::default())?.len(), 3);

            // A re-link never changes the kind nor erases a known target.
            link(
                conn,
                &NewLink {
                    kind: Some(AssistantKind::ApiCallStep),
                    ..custom_api("a", None)
                },
            )?;
            let a = get(conn, "a")?.unwrap();
            assert_eq!(a.kind, AssistantKind::CustomApi);
            assert_eq!(a.target_id.as_deref(), Some("srv-1"));
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn update_attaches_and_records_the_applied_proposal() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            crate::db::discussions::insert_discussion(conn, &discussion("a"))?;
            link(conn, &custom_api("a", None))?;
            assert!(update(
                conn,
                "a",
                &LinkPatch {
                    last_proposal_signature: Some("sig-1".into()),
                    ..LinkPatch::default()
                }
            )?);
            let pending = get(conn, "a")?.unwrap();
            assert_eq!(pending.last_proposal_signature.as_deref(), Some("sig-1"));
            assert!(pending.last_applied_signature.is_none());
            assert!(pending.last_applied_at.is_none());

            update(
                conn,
                "a",
                &LinkPatch {
                    target_id: Some("srv-9".into()),
                    last_applied_signature: Some("sig-1".into()),
                    ..LinkPatch::default()
                },
            )?;
            let applied = get(conn, "a")?.unwrap();
            assert_eq!(applied.target_id.as_deref(), Some("srv-9"));
            assert_eq!(applied.last_applied_signature.as_deref(), Some("sig-1"));
            assert!(applied.last_applied_at.is_some());
            assert!(!update(conn, "nope", &LinkPatch::default())?);
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn deleting_the_discussion_drops_the_link() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            crate::db::discussions::insert_discussion(conn, &discussion("a"))?;
            link(conn, &custom_api("a", Some("srv-1")))?;
            assert!(crate::db::discussions::delete_discussion(conn, "a")?);
            let left: i64 =
                conn.query_row("SELECT COUNT(*) FROM assistant_conversations", [], |row| {
                    row.get(0)
                })?;
            assert_eq!(left, 0);
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn one_turn_ending_keeps_the_other_turns_values() {
        let db = Database::open_in_memory().unwrap();
        let guard = db.assistant_guard().clone();
        guard.begin_turn(
            "job-a",
            "d",
            &TransientSecrets::from(vec!["alpha-Secret-1".into()]),
        );
        guard.begin_turn(
            "job-b",
            "d",
            &TransientSecrets::from(vec!["bravo-Secret-2".into()]),
        );
        guard.begin_turn(
            "job-c",
            "other",
            &TransientSecrets::from(vec!["charlie-Secret-3".into()]),
        );
        guard.end_turn("job-a");
        let masked = db
            .with_conn(|conn| {
                mask_for_discussion(conn, "d", "alpha-Secret-1 bravo-Secret-2 charlie-Secret-3")
            })
            .await
            .unwrap();
        // Ended turn: no longer masked; live turn of this discussion: masked;
        // another discussion's turn never applies here.
        assert_eq!(masked, "alpha-Secret-1 *** charlie-Secret-3");

        // Turns begun and ended concurrently never drop each other.
        let threads: Vec<_> = (0..16)
            .map(|n| {
                let guard = guard.clone();
                std::thread::spawn(move || {
                    let id = format!("job-{n}");
                    guard.begin_turn(
                        &id,
                        "d",
                        &TransientSecrets::from(vec![format!("value-{n}-xxxxxxxx")]),
                    );
                    if n % 2 == 0 {
                        guard.end_turn(&id);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(guard.live_turns(), 2 + 8);
        let masked = db
            .with_conn(|conn| mask_for_discussion(conn, "d", "value-1-xxxxxxxx value-2-xxxxxxxx"))
            .await
            .unwrap();
        assert_eq!(masked, "*** value-2-xxxxxxxx");
        assert_eq!(format!("{guard:?}"), "AssistantGuard(redacted)");
    }

    #[tokio::test]
    async fn a_saved_step_lists_its_own_and_its_drafts_unattached_conversations() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            for id in ["saved", "draft", "other-step", "other-wf"] {
                crate::db::discussions::insert_discussion(conn, &discussion(id))?;
            }
            for (id, target, step) in [
                ("saved", Some("wf-1"), "step-uuid-1"),
                ("draft", None, "fetch"),
                ("other-step", None, "reshape"),
                ("other-wf", Some("wf-2"), "step-uuid-1"),
            ] {
                link(
                    conn,
                    &NewLink {
                        discussion_id: id.into(),
                        kind: Some(AssistantKind::ApiCallStep),
                        target_id: target.map(str::to_string),
                        target_step: Some(step.into()),
                        plugin_id: Some("srv".into()),
                        target_label: step.into(),
                    },
                )?;
            }
            // The step was renamed after its conversation: its durable id
            // still finds it, and a draft of its current name is offered too.
            let mut found: Vec<String> = list(
                conn,
                &ListFilter {
                    kind: Some(AssistantKind::ApiCallStep),
                    target_id: Some("wf-1".into()),
                    include_unattached: true,
                    target_step: Some("step-uuid-1".into()),
                    unattached_step: Some("fetch".into()),
                    plugin_id: Some("srv".into()),
                    ..ListFilter::default()
                },
            )?
            .into_iter()
            .map(|row| row.discussion_id)
            .collect();
            found.sort();
            assert_eq!(found, vec!["draft".to_string(), "saved".to_string()]);
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn an_owed_conversation_is_found_by_id_whatever_its_label() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            for (id, label) in [("owed", ""), ("stranger", "FinalAPI")] {
                crate::db::discussions::insert_discussion(conn, &discussion(id))?;
                link(
                    conn,
                    &NewLink {
                        discussion_id: id.into(),
                        kind: Some(AssistantKind::CustomApi),
                        target_label: label.into(),
                        ..NewLink::default()
                    },
                )?;
            }
            let found = list(
                conn,
                &ListFilter {
                    kind: Some(AssistantKind::CustomApi),
                    target_id: Some("custom-created".into()),
                    include_unattached: true,
                    pending_ids: vec!["owed".into()],
                    ..ListFilter::default()
                },
            )?;
            // The label proves nothing: only the id this client owes counts.
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].discussion_id, "owed");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn an_old_turn_keeps_masking_while_its_dispatch_is_active() {
        let db = Database::open_in_memory().unwrap();
        let guard = db.assistant_guard().clone();
        db.with_conn(|conn| {
            crate::db::discussions::insert_discussion(conn, &discussion("d"))?;
            for (id, status) in [("job-live", "Running"), ("job-done", "Completed")] {
                // The trigger message is not what this test is about.
                conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
                conn.execute(
                    "INSERT INTO agent_dispatch_jobs (id, discussion_id, trigger_message_id,
                         trigger_sort_order, dedupe_key, status, available_at, created_at, updated_at)
                     VALUES (?1, 'd', 'm', 0, ?1, ?2, '2026-10-09T00:00:00Z',
                         '2026-10-09T00:00:00Z', '2026-10-09T00:00:00Z')",
                    params![id, status],
                )?;
                conn.execute_batch("PRAGMA foreign_keys=ON;")?;
            }
            Ok(())
        })
        .await
        .unwrap();
        guard.begin_turn(
            "job-live",
            "d",
            &TransientSecrets::from(vec!["live-Secret-1234".into()]),
        );
        guard.begin_turn(
            "job-done",
            "d",
            &TransientSecrets::from(vec!["done-Secret-5678".into()]),
        );
        guard.begin_turn(
            "job-gone",
            "d",
            &TransientSecrets::from(vec!["gone-Secret-9012".into()]),
        );
        let three_hours = std::time::Duration::from_secs(3 * 60 * 60);
        for id in ["job-live", "job-done", "job-gone"] {
            guard.age_turn(id, three_hours);
        }
        let masked = db
            .with_conn(|conn| {
                mask_for_discussion(
                    conn,
                    "d",
                    "live-Secret-1234 done-Secret-5678 gone-Secret-9012",
                )
            })
            .await
            .unwrap();
        // Still running past the two hours: masked; terminal or missing: released.
        assert_eq!(masked, "*** done-Secret-5678 gone-Secret-9012");
        assert_eq!(guard.live_turns(), 1);
    }
}
