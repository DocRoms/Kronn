//! Versioned, self-contained discussion export/import bundles.
//!
//! The wire format contains transcript data, author metadata, attachment
//! contents available to Kronn, message-revision audit events and the tasks
//! directly attached to the discussion plan. Runtime credentials, source CLI
//! session ownership, local workspace paths and sharing identifiers are never
//! exported.

use std::collections::{HashMap, HashSet};

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    models::{
        AddPlanningBlockerRequest, ApiErrorCode, ApiResponse, CreatePlanningDodItem,
        CreatePlanningTaskLink, CreatePlanningTaskRequest, Discussion, DiscussionMessage,
        LinkPlanningDiscussionRequest, PlanningActor, PlanningActorKind,
        PlanningDiscussionRelation, PlanningPlacement, PlanningTaskDetail,
        UpdatePlanningTaskRequest,
    },
    AppState,
};

const DISCUSSION_EXPORT_KIND: &str = "kronn.discussion";
const DISCUSSION_EXPORT_VERSION: u32 = 1;
const SECRET_POLICY: &str =
    "conversation_and_attachment_content_included; runtime_credentials_and_local_bindings_excluded";
const TOUR_DEMO_SOURCE_ID: &str = "kronn-guided-tour-demo-v1";
const TOUR_DEMO_REQUEST_SOURCE_ID: &str = "kronn-guided-tour-demo-request";
const TOUR_DEMO_PREVIEW_SOURCE_ID: &str = "kronn-guided-tour-demo-preview";

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PortableDiscussionAttachment {
    pub source_id: String,
    pub message_id: Option<String>,
    pub filename: String,
    pub mime_type: String,
    pub original_size: u64,
    pub extracted_text: String,
    /// Raw bytes are available for disk-backed attachments. Office documents
    /// historically stored only extracted text, so this is null for them.
    pub data_base64: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PortableDiscussionRevisionEvent {
    pub target_message_id: String,
    pub previous_content_hash: String,
    pub expected_revision: String,
    pub revision: String,
    pub content: String,
    pub target_agent_json: Option<String>,
    pub idempotency_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PortableDiscussionPlanItem {
    pub placement: PlanningPlacement,
    pub is_primary: bool,
    pub position: i64,
    pub task: PlanningTaskDetail,
}

/// KT-74 — the human who exported the bundle, so the receiving Kronn can say
/// "imported from Romu" instead of attributing the discussion to a CLI it was
/// merely bound to. Both fields are optional: an instance that never set a
/// pseudo has nothing to declare, and no field may be forged to fill the gap.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PortableExporterIdentity {
    #[serde(default)]
    pub pseudo: Option<String>,
    #[serde(default)]
    pub avatar_email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionExportEnvelope {
    pub kind: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub secret_policy: String,
    pub source_discussion_id: String,
    pub discussion: Discussion,
    pub messages: Vec<DiscussionMessage>,
    pub attachments: Vec<PortableDiscussionAttachment>,
    pub revision_events: Vec<PortableDiscussionRevisionEvent>,
    pub plan: Vec<PortableDiscussionPlanItem>,
    /// Absent from every bundle exported before KT-74, hence `default` on the
    /// way in: a v1 file must keep importing untouched. `skip_serializing_if`
    /// on the way out, so an instance with no identity omits the key instead of
    /// writing `"exported_by": null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_by: Option<PortableExporterIdentity>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ImportDiscussionRequest {
    pub content: String,
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportDiscussionReport {
    pub discussion_id: String,
    pub source_discussion_id: String,
    pub already_imported: bool,
    pub imported_messages: u32,
    pub imported_attachments: u32,
    pub imported_revision_events: u32,
    pub imported_tasks: u32,
    pub imported_task_events: u32,
    pub warnings: Vec<String>,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TourDemoDiscussionResponse {
    pub discussion_id: String,
    pub created: bool,
    /// Exact first message stored in the demo. The tour types this value into
    /// the launcher so the simulated request can never drift from the result.
    pub prompt: String,
}

#[derive(Debug)]
struct AttachmentRow {
    source_id: String,
    message_id: Option<String>,
    filename: String,
    mime_type: String,
    original_size: u64,
    extracted_text: String,
    disk_path: Option<String>,
    created_at: String,
}

#[derive(Default)]
struct ImportedFilesGuard {
    paths: Vec<String>,
    committed: bool,
}

impl ImportedFilesGuard {
    fn track(&mut self, path: &Option<String>) {
        if let Some(path) = path {
            self.paths.push(path.clone());
        }
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for ImportedFilesGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for path in &self.paths {
            crate::core::context_files::delete_image_from_disk(path);
        }
    }
}

fn filename_for(title: &str) -> String {
    let safe: String = title
        .chars()
        .map(|value| {
            if value.is_alphanumeric() || value == '-' || value == '_' {
                value
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_matches('-');
    let safe = if safe.is_empty() { "discussion" } else { safe };
    format!("{safe}.kronn-discussion.json")
}

/// Identifies the CONTENT, so neither `exported_at` nor `exported_by` belongs
/// here: the same discussion exported twice, or exported by two different
/// people, is the same discussion. Hashing the exporter would turn a colleague's
/// copy of an already-imported bundle into a bogus IMPORT_CONFLICT.
fn content_fingerprint(envelope: &DiscussionExportEnvelope) -> anyhow::Result<String> {
    let canonical = serde_json::to_vec(&(
        &envelope.kind,
        envelope.version,
        &envelope.secret_policy,
        &envelope.source_discussion_id,
        &envelope.discussion,
        &envelope.messages,
        &envelope.attachments,
        &envelope.revision_events,
        &envelope.plan,
    ))?;
    let digest = Sha256::digest(canonical);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn export_attachments(
    conn: &rusqlite::Connection,
    discussion_id: &str,
) -> anyhow::Result<Vec<PortableDiscussionAttachment>> {
    let mut statement = conn.prepare(
        "SELECT id, message_id, filename, mime_type, original_size,
                extracted_text, disk_path, created_at
         FROM context_files
         WHERE discussion_id = ?1
         ORDER BY created_at, rowid",
    )?;
    let rows = statement
        .query_map([discussion_id], |row| {
            Ok(AttachmentRow {
                source_id: row.get(0)?,
                message_id: row.get(1)?,
                filename: row.get(2)?,
                mime_type: row.get(3)?,
                original_size: row.get::<_, i64>(4)?.max(0) as u64,
                extracted_text: row.get(5)?,
                disk_path: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let data_base64 = row
                .disk_path
                .as_deref()
                .and_then(|path| std::fs::read(path).ok())
                .map(|bytes| B64.encode(bytes));
            PortableDiscussionAttachment {
                source_id: row.source_id,
                message_id: row.message_id,
                filename: row.filename,
                mime_type: row.mime_type,
                original_size: row.original_size,
                extracted_text: row.extracted_text,
                data_base64,
                created_at: row.created_at,
            }
        })
        .collect())
}

fn export_revision_events(
    conn: &rusqlite::Connection,
    discussion_id: &str,
) -> anyhow::Result<Vec<PortableDiscussionRevisionEvent>> {
    let mut statement = conn.prepare(
        "SELECT target_message_id, previous_content_hash, expected_revision,
                revision, content, target_agent_json, idempotency_key, created_at
         FROM message_revision_events
         WHERE discussion_id = ?1
         ORDER BY sort_order, rowid",
    )?;
    let events = statement
        .query_map([discussion_id], |row| {
            Ok(PortableDiscussionRevisionEvent {
                target_message_id: row.get(0)?,
                previous_content_hash: row.get(1)?,
                expected_revision: row.get(2)?,
                revision: row.get(3)?,
                content: row.get(4)?,
                target_agent_json: row.get(5)?,
                idempotency_key: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(events)
}

fn relation_to_portable(
    conn: &rusqlite::Connection,
    relation: PlanningDiscussionRelation,
) -> anyhow::Result<PortableDiscussionPlanItem> {
    let task = crate::db::planning::get_task(conn, &relation.task.id)?
        .ok_or_else(|| anyhow::anyhow!("Planning task disappeared during export"))?;
    Ok(PortableDiscussionPlanItem {
        placement: relation.placement,
        is_primary: relation.is_primary,
        position: relation.position,
        task,
    })
}

fn export_plan(
    conn: &rusqlite::Connection,
    discussion_id: &str,
) -> anyhow::Result<Vec<PortableDiscussionPlanItem>> {
    let plan = crate::db::planning::get_discussion_plan(conn, discussion_id)?;
    plan.active
        .into_iter()
        .chain(plan.later)
        .map(|relation| relation_to_portable(conn, relation))
        .collect()
}

fn build_export_envelope(
    conn: &rusqlite::Connection,
    source_id: &str,
    exported_by: Option<PortableExporterIdentity>,
) -> anyhow::Result<DiscussionExportEnvelope> {
    let mut discussion = crate::db::discussions::get_discussion(conn, source_id)?
        .ok_or_else(|| anyhow::anyhow!("Discussion not found"))?;
    let messages = std::mem::take(&mut discussion.messages);
    discussion.message_count = messages.len() as u32;
    discussion.non_system_message_count = messages
        .iter()
        .filter(|message| !matches!(message.role, crate::models::MessageRole::System))
        .count() as u32;

    // Local execution/sharing state is deliberately non-portable.
    discussion.workspace_mode = "Direct".into();
    discussion.workspace_path = None;
    discussion.worktree_branch = None;
    discussion.test_mode_restore_branch = None;
    discussion.test_mode_stash_ref = None;
    discussion.shared_id = None;
    discussion.shared_with.clear();
    discussion.workflow_run_id = None;
    discussion.awaiting_agent = false;
    discussion.summary_cache = None;
    discussion.summary_up_to_msg_idx = None;

    Ok(DiscussionExportEnvelope {
        kind: DISCUSSION_EXPORT_KIND.into(),
        version: DISCUSSION_EXPORT_VERSION,
        exported_at: Utc::now(),
        secret_policy: SECRET_POLICY.into(),
        source_discussion_id: source_id.to_string(),
        attachments: export_attachments(conn, source_id)?,
        revision_events: export_revision_events(conn, source_id)?,
        plan: export_plan(conn, source_id)?,
        exported_by,
        discussion,
        messages,
    })
}

/// The local identity as the envelope should carry it: `None` when nothing is
/// configured, so the field is omitted instead of shipping an empty author.
fn exporter_identity(
    pseudo: Option<String>,
    avatar_email: Option<String>,
) -> Option<PortableExporterIdentity> {
    let pseudo = pseudo.filter(|value| !value.trim().is_empty());
    let avatar_email = avatar_email.filter(|value| !value.trim().is_empty());
    if pseudo.is_none() && avatar_email.is_none() {
        return None;
    }
    Some(PortableExporterIdentity {
        pseudo,
        avatar_email,
    })
}

/// GET /api/discussions/{id}/export
pub async fn export_discussion(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let source_id = id.clone();
    let exported_by = {
        let config = state.config.read().await;
        exporter_identity(
            config.server.pseudo.clone(),
            config.server.avatar_email.clone(),
        )
    };
    let result = state
        .db
        .with_read_conn(move |conn| build_export_envelope(conn, &source_id, exported_by))
        .await;

    let envelope = match result {
        Ok(value) => value,
        Err(error) if error.to_string() == "Discussion not found" => {
            return (StatusCode::NOT_FOUND, "Discussion not found").into_response();
        }
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("DB error: {error}"),
            )
                .into_response();
        }
    };
    let filename = filename_for(&envelope.discussion.title);
    match serde_json::to_string_pretty(&envelope) {
        Ok(body) => (
            [
                (header::CONTENT_TYPE, "application/json".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{filename}\""),
                ),
            ],
            body,
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Serialization error: {error}"),
        )
            .into_response(),
    }
}

fn existing_project(
    conn: &rusqlite::Connection,
    requested: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let Some(project_id) = requested else {
        return Ok(None);
    };
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
        [project_id],
        |row| row.get(0),
    )?;
    Ok(exists.then(|| project_id.to_string()))
}

fn imported_attachment_path(
    attachment_id: &str,
    filename: &str,
    data_base64: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let Some(encoded) = data_base64 else {
        return Ok(None);
    };
    let bytes = B64
        .decode(encoded)
        .map_err(|_| anyhow::anyhow!("Attachment `{filename}` has invalid base64 data"))?;
    crate::core::context_files::save_file_to_disk(attachment_id, filename, &bytes).map(Some)
}

fn portable_task_request(
    task: &PlanningTaskDetail,
    project_ids: Vec<String>,
) -> CreatePlanningTaskRequest {
    CreatePlanningTaskRequest {
        title: task.summary.title.clone(),
        discussion_id: None,
        idempotency_key: None,
        description: task.description.clone(),
        status: task.summary.status,
        priority: task.summary.priority,
        parent_id: None,
        project_ids,
        tags: task.summary.tags.clone(),
        definition_of_done: task
            .definition_of_done
            .iter()
            .map(|item| CreatePlanningDodItem {
                id: None,
                sentence: item.sentence.clone(),
                completed: item.completed,
            })
            .collect(),
        links: task
            .links
            .iter()
            .map(|link| CreatePlanningTaskLink {
                label: link.label.clone(),
                url: link.url.clone(),
            })
            .collect(),
        actor: PlanningActor {
            kind: PlanningActorKind::Agent,
            id: Some("discussion-import".into()),
            source_message_id: None,
        },
    }
}

fn import_bundle(
    conn: &rusqlite::Connection,
    envelope: DiscussionExportEnvelope,
    requested_project_id: Option<String>,
    fingerprint: String,
) -> anyhow::Result<ImportDiscussionReport> {
    if let Some((existing_hash, imported_discussion_id)) = conn
        .query_row(
            "SELECT content_sha256, imported_discussion_id
             FROM discussion_imports WHERE source_discussion_id = ?1",
            [&envelope.source_discussion_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    {
        if existing_hash == fingerprint {
            return Ok(ImportDiscussionReport {
                discussion_id: imported_discussion_id,
                source_discussion_id: envelope.source_discussion_id,
                already_imported: true,
                imported_messages: 0,
                imported_attachments: 0,
                imported_revision_events: 0,
                imported_tasks: 0,
                imported_task_events: 0,
                warnings: Vec::new(),
                conflicts: Vec::new(),
            });
        }
        anyhow::bail!(
            "IMPORT_CONFLICT: source discussion {} was already imported from different content",
            envelope.source_discussion_id
        );
    }

    let transaction = conn.unchecked_transaction()?;
    let mut warnings = Vec::new();
    let mut conflicts = Vec::new();
    let target_project = existing_project(
        &transaction,
        requested_project_id
            .as_deref()
            .or(envelope.discussion.project_id.as_deref()),
    )?;
    if requested_project_id.is_some() && target_project.is_none() {
        warnings.push("Requested project was not found; discussion imported globally".into());
    } else if requested_project_id.is_none()
        && envelope.discussion.project_id.is_some()
        && target_project.is_none()
    {
        warnings.push("Source project was not found; discussion imported globally".into());
    }

    let new_discussion_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let mut discussion = envelope.discussion.clone();
    discussion.id = new_discussion_id.clone();
    discussion.project_id = target_project;
    discussion.messages.clear();
    discussion.message_count = 0;
    discussion.non_system_message_count = 0;
    discussion.workspace_mode = "Direct".into();
    discussion.workspace_path = None;
    discussion.worktree_branch = None;
    discussion.test_mode_restore_branch = None;
    discussion.test_mode_stash_ref = None;
    discussion.shared_id = None;
    discussion.shared_with.clear();
    discussion.workflow_run_id = None;
    discussion.awaiting_agent = false;
    discussion.pinned = false;
    discussion.archived = false;
    discussion.summary_cache = None;
    discussion.summary_up_to_msg_idx = None;
    discussion.created_at = now;
    discussion.updated_at = now;
    crate::db::discussions::insert_discussion(&transaction, &discussion)?;

    let message_ids: HashMap<String, String> = envelope
        .messages
        .iter()
        .map(|message| (message.id.clone(), Uuid::new_v4().to_string()))
        .collect();
    for mut message in envelope.messages {
        let source_id = message.id.clone();
        message.id = message_ids
            .get(&source_id)
            .cloned()
            .expect("message ids were preallocated");
        if message.source_msg_id.is_none() {
            message.source_msg_id = Some(source_id.clone());
        }
        if let Some(source_reply_id) = message.reply_to_message_id.take() {
            match message_ids.get(&source_reply_id) {
                Some(imported_reply_id) => {
                    message.reply_to_message_id = Some(imported_reply_id.clone());
                }
                None => conflicts.push(format!(
                    "Message `{source_id}` referenced missing reply target `{source_reply_id}`"
                )),
            }
        }
        crate::db::discussions::insert_message(&transaction, &new_discussion_id, &message)?;
    }

    let mut imported_attachments = 0_u32;
    let mut imported_files = ImportedFilesGuard::default();
    for attachment in envelope.attachments {
        let attachment_id = Uuid::new_v4().to_string();
        let message_id = attachment
            .message_id
            .as_ref()
            .and_then(|source| message_ids.get(source))
            .cloned();
        if attachment.message_id.is_some() && message_id.is_none() {
            conflicts.push(format!(
                "Attachment `{}` referenced a missing source message and was kept discussion-wide",
                attachment.filename
            ));
        }
        let disk_path = imported_attachment_path(
            &attachment_id,
            &attachment.filename,
            attachment.data_base64.as_deref(),
        )?;
        imported_files.track(&disk_path);
        let extracted_size = attachment.extracted_text.len() as i64;
        transaction.execute(
            "INSERT INTO context_files
             (id, discussion_id, filename, mime_type, original_size,
              extracted_text, extracted_size, disk_path, message_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                attachment_id,
                new_discussion_id,
                attachment.filename,
                attachment.mime_type,
                attachment.original_size as i64,
                attachment.extracted_text,
                extracted_size,
                disk_path,
                message_id,
                attachment.created_at,
            ],
        )?;
        imported_attachments += 1;
    }

    let mut imported_revision_events = 0_u32;
    let mut next_revision_order: i64 = transaction.query_row(
        "SELECT next_message_seq FROM discussions WHERE id = ?1",
        [&new_discussion_id],
        |row| row.get(0),
    )?;
    for event in envelope.revision_events {
        let Some(target_message_id) = message_ids.get(&event.target_message_id) else {
            conflicts.push(format!(
                "Revision event `{}` referenced a missing source message and was skipped",
                event.idempotency_key
            ));
            continue;
        };
        transaction.execute(
            "INSERT INTO message_revision_events
             (id, discussion_id, target_message_id, previous_content_hash,
              expected_revision, revision, content, target_agent_json,
              idempotency_key, sort_order, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                Uuid::new_v4().to_string(),
                new_discussion_id,
                target_message_id,
                event.previous_content_hash,
                event.expected_revision,
                event.revision,
                event.content,
                event.target_agent_json,
                format!(
                    "import:{}:{}",
                    envelope.source_discussion_id, event.idempotency_key
                ),
                next_revision_order,
                event.created_at,
            ],
        )?;
        next_revision_order += 1;
        imported_revision_events += 1;
    }
    transaction.execute(
        "UPDATE discussions SET next_message_seq = ?2 WHERE id = ?1",
        params![new_discussion_id, next_revision_order],
    )?;

    let known_projects: HashSet<String> = {
        let mut statement = transaction.prepare("SELECT id FROM projects")?;
        let projects = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        projects
    };
    let actor = PlanningActor {
        kind: PlanningActorKind::Agent,
        id: Some("discussion-import".into()),
        source_message_id: None,
    };
    let mut task_ids = HashMap::new();
    for item in &envelope.plan {
        let project_ids = item
            .task
            .summary
            .project_ids
            .iter()
            .filter(|id| known_projects.contains(*id))
            .cloned()
            .collect();
        let created = crate::db::planning::create_task(
            &transaction,
            &portable_task_request(&item.task, project_ids),
        )?;
        task_ids.insert(item.task.summary.id.clone(), created.summary.id);
    }

    let mut imported_task_events = 0_u32;
    for item in &envelope.plan {
        let source_task = &item.task;
        let imported_task_id = task_ids.get(&source_task.summary.id).ok_or_else(|| {
            anyhow::anyhow!(
                "Imported task mapping is missing for `{}`",
                source_task.summary.reference
            )
        })?;
        let mapped_parent = source_task
            .summary
            .parent_id
            .as_ref()
            .and_then(|source| task_ids.get(source))
            .cloned();
        if source_task.summary.parent_id.is_some() && mapped_parent.is_none() {
            warnings.push(format!(
                "{}: parent outside the exported discussion plan was not copied",
                source_task.summary.reference
            ));
        }
        crate::db::planning::update_task(
            &transaction,
            imported_task_id,
            &UpdatePlanningTaskRequest {
                title: None,
                description: None,
                status: None,
                priority: None,
                parent_id: Some(mapped_parent),
                blocked_reason: Some(source_task.blocked_reason.clone()),
                rank: Some(source_task.summary.rank),
                project_ids: None,
                tags: None,
                definition_of_done: None,
                links: None,
                actor: actor.clone(),
            },
        )?;
        crate::db::planning::link_discussion(
            &transaction,
            imported_task_id,
            &LinkPlanningDiscussionRequest {
                discussion_id: new_discussion_id.clone(),
                placement: item.placement,
                is_primary: item.is_primary,
                position: Some(item.position),
                actor: actor.clone(),
            },
        )?;

        for blocker in &source_task.blockers {
            if let Some(imported_blocker_id) = task_ids.get(&blocker.id) {
                crate::db::planning::add_blocker(
                    &transaction,
                    imported_task_id,
                    &AddPlanningBlockerRequest {
                        blocker_task_id: imported_blocker_id.clone(),
                        actor: actor.clone(),
                    },
                )?;
            } else {
                warnings.push(format!(
                    "{}: blocker {} outside the exported plan was not copied",
                    source_task.summary.reference, blocker.reference
                ));
            }
        }

        for event in source_task.events.iter().rev() {
            let source_message_id = event
                .source_message_id
                .as_ref()
                .and_then(|source| message_ids.get(source));
            transaction.execute(
                "INSERT INTO planning_task_events
                 (id, task_id, action, actor_kind, actor_id, changes_json,
                  source_message_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    Uuid::new_v4().to_string(),
                    imported_task_id,
                    format!("imported_source:{}", event.action),
                    event.actor_kind.as_str(),
                    event.actor_id,
                    serde_json::to_string(&event.changes)?,
                    source_message_id,
                    event.created_at.to_rfc3339(),
                ],
            )?;
            imported_task_events += 1;
        }
    }

    // A portable bundle, explicitly — `agent_transcript` is reserved for the
    // import route that does not exist yet (096).
    let exporter = envelope.exported_by.as_ref();
    transaction.execute(
        "INSERT INTO discussion_imports
         (source_discussion_id, content_sha256, imported_discussion_id, imported_at,
          provenance_kind, imported_by_pseudo, imported_by_avatar_email)
         VALUES (?1, ?2, ?3, ?4, 'portable_bundle', ?5, ?6)",
        params![
            envelope.source_discussion_id,
            fingerprint,
            new_discussion_id,
            Utc::now().to_rfc3339(),
            exporter.and_then(|identity| identity.pseudo.clone()),
            exporter.and_then(|identity| identity.avatar_email.clone()),
        ],
    )?;
    transaction.commit()?;
    imported_files.commit();

    Ok(ImportDiscussionReport {
        discussion_id: new_discussion_id,
        source_discussion_id: envelope.source_discussion_id,
        already_imported: false,
        imported_messages: message_ids.len() as u32,
        imported_attachments,
        imported_revision_events,
        imported_tasks: task_ids.len() as u32,
        imported_task_events,
        warnings,
        conflicts,
    })
}

fn tour_demo_envelope(ui_language: &str) -> DiscussionExportEnvelope {
    let created_at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .expect("guided-tour timestamp is a valid constant")
        .with_timezone(&Utc);
    let user_message_id = TOUR_DEMO_REQUEST_SOURCE_ID.to_string();
    let preview_message_id = TOUR_DEMO_PREVIEW_SOURCE_ID.to_string();
    let (language, prompt, preview_intro, tour_label, preview_title, preview_description) = match ui_language {
        "en" => (
            "en",
            "Create a short HTML page presenting Kronn in the document viewer.",
            "A deterministic document prepared for the Kronn guided tour.",
            "Kronn · Guided tour",
            "Live document preview",
            "This HTML is rendered directly in the discussion and can be exported without asking an agent to regenerate it.",
        ),
        "es" => (
            "es",
            "Crea una breve página HTML sobre Kronn en el visor de documentos.",
            "Un documento determinista preparado para la visita guiada de Kronn.",
            "Kronn · Visita guiada",
            "Vista previa del documento",
            "Este HTML se muestra directamente en la conversación y puede exportarse sin pedir a un agente que lo genere de nuevo.",
        ),
        "zh" => (
            "zh",
            "在文档查看器中创建一个介绍 Kronn 的简短 HTML 页面。",
            "为 Kronn 引导教程准备的确定性文档。",
            "Kronn · 引导教程",
            "实时文档预览",
            "此 HTML 会直接显示在讨论中，无需让代理重新生成即可导出。",
        ),
        _ => (
            "fr",
            "Crée une courte page HTML présentant Kronn dans le viewer de documents.",
            "Un document déterministe préparé pour la visite guidée de Kronn.",
            "Kronn · Visite guidée",
            "Aperçu du document",
            "Ce HTML est affiché directement dans la discussion et peut être exporté sans demander à un agent de le générer à nouveau.",
        ),
    };
    let preview_content = format!(
        r#"{preview_intro}

```kronn-doc-preview
<!doctype html>
<html lang="{language}">
<head>
  <meta charset="utf-8">
  <style>
    body {{ margin: 0; padding: 40px; font: 16px/1.55 system-ui, sans-serif; color: #172033; background: #f7f8fb; }}
    main {{ max-width: 720px; margin: auto; padding: 36px; border-radius: 18px; background: white; box-shadow: 0 12px 36px #18203a18; }}
    h1 {{ margin: 0 0 8px; color: #6d5dfc; }}
    .meta {{ color: #697089; }}
    .capabilities {{ display: flex; gap: 10px; margin-top: 24px; }}
    .capabilities span {{ padding: 7px 12px; border-radius: 999px; color: #5143d9; background: #efedff; font-weight: 650; }}
  </style>
</head>
<body>
  <main>
    <p class="meta">{tour_label}</p>
    <h1>{preview_title}</h1>
    <p>{preview_description}</p>
    <div class="capabilities"><span>HTML</span><span>PDF</span><span>DOCX</span></div>
  </main>
</body>
</html>
```
"#,
    );

    DiscussionExportEnvelope {
        kind: DISCUSSION_EXPORT_KIND.into(),
        version: DISCUSSION_EXPORT_VERSION,
        exported_at: created_at,
        secret_policy: SECRET_POLICY.into(),
        source_discussion_id: TOUR_DEMO_SOURCE_ID.into(),
        discussion: Discussion {
            id: TOUR_DEMO_SOURCE_ID.into(),
            project_id: None,
            title: "Kronn · Demo".into(),
            agent: crate::models::AgentType::ClaudeCode,
            language: language.into(),
            participants: vec![],
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
            tier: crate::models::ModelTier::Default,
            model: None,
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::Off,
            introspection_call_count: 0,
            shared_id: None,
            shared_with: vec![],
            workflow_run_id: None,
            awaiting_agent: false,
            test_mode_restore_branch: None,
            test_mode_stash_ref: None,
            created_at,
            updated_at: created_at,
        },
        messages: vec![
            DiscussionMessage {
                id: user_message_id.clone(),
                role: crate::models::MessageRole::User,
                channel: crate::models::MessageChannel::Main,
                content: prompt.into(),
                agent_type: None,
                timestamp: created_at,
                tokens_used: 0,
                auth_mode: None,
                model_tier: None,
                model: None,
                cost_usd: None,
                author_pseudo: None,
                author_avatar_email: None,
                source_msg_id: None,
                duration_ms: None,
                lint_report: None,
                target_agent: None,
                reply_to_message_id: None,
            },
            DiscussionMessage {
                id: preview_message_id,
                role: crate::models::MessageRole::Agent,
                channel: crate::models::MessageChannel::Main,
                content: preview_content,
                agent_type: Some(crate::models::AgentType::ClaudeCode),
                timestamp: created_at + chrono::Duration::seconds(1),
                tokens_used: 0,
                auth_mode: None,
                model_tier: None,
                model: None,
                cost_usd: None,
                author_pseudo: None,
                author_avatar_email: None,
                source_msg_id: None,
                duration_ms: Some(0),
                lint_report: None,
                target_agent: None,
                reply_to_message_id: Some(user_message_id),
            },
        ],
        attachments: vec![],
        revision_events: vec![],
        plan: vec![],
        exported_by: None,
    }
}

/// POST /api/tour/demo-discussion
///
/// Seed the deterministic, agentless discussion used by the guided tour.
/// Import provenance provides idempotence across reloads and repeated tours.
pub async fn ensure_tour_demo_discussion(
    State(state): State<AppState>,
) -> Json<ApiResponse<TourDemoDiscussionResponse>> {
    let ui_language = state.config.read().await.ui_language.clone();
    let envelope = tour_demo_envelope(&ui_language);
    let seeded_prompt = envelope.messages[0].content.clone();
    let seeded_preview = envelope.messages[1].content.clone();
    let seeded_language = envelope.discussion.language.clone();
    let fingerprint = match content_fingerprint(&envelope) {
        Ok(value) => value,
        Err(error) => return Json(ApiResponse::err(format!("Fingerprint error: {error}"))),
    };
    match state
        .db
        .with_conn(move |conn| {
            // A user can change the UI language and replay the tour. Keep one
            // durable demo instead of importing one per locale, and return the
            // prompt actually stored in it so the typing animation stays true.
            if let Some(discussion_id) = conn
                .query_row(
                    "SELECT imported_discussion_id
                     FROM discussion_imports
                     WHERE source_discussion_id = ?1",
                    [TOUR_DEMO_SOURCE_ID],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
            {
                // A replay after changing the UI language keeps the same durable
                // demo discussion but refreshes its deterministic localized copy.
                conn.execute(
                    "UPDATE messages
                     SET content = ?1
                     WHERE discussion_id = ?2 AND source_msg_id = ?3",
                    params![seeded_prompt, discussion_id, TOUR_DEMO_REQUEST_SOURCE_ID],
                )?;
                conn.execute(
                    "UPDATE messages
                     SET content = ?1
                     WHERE discussion_id = ?2 AND source_msg_id = ?3",
                    params![seeded_preview, discussion_id, TOUR_DEMO_PREVIEW_SOURCE_ID],
                )?;
                conn.execute(
                    "UPDATE discussions SET language = ?1 WHERE id = ?2",
                    params![seeded_language, discussion_id],
                )?;
                crate::db::discussions::set_disc_no_agent(conn, &discussion_id, true)?;
                crate::db::discussions::update_discussion(
                    conn,
                    &discussion_id,
                    None,
                    Some(false),
                    None,
                    None,
                )?;
                return Ok(TourDemoDiscussionResponse {
                    discussion_id,
                    created: false,
                    prompt: seeded_prompt,
                });
            }

            let report = import_bundle(conn, envelope, None, fingerprint)?;
            // Reuse the portable-import ledger for idempotence without making
            // the seeded demo look like a user-imported conversation in the
            // sidebar (which intentionally badges only `portable_bundle`).
            conn.execute(
                "UPDATE discussion_imports
                 SET provenance_kind = 'guided_tour_demo'
                 WHERE source_discussion_id = ?1",
                [TOUR_DEMO_SOURCE_ID],
            )?;
            crate::db::discussions::set_disc_no_agent(conn, &report.discussion_id, true)?;
            crate::db::discussions::update_discussion(
                conn,
                &report.discussion_id,
                None,
                Some(false),
                None,
                None,
            )?;
            Ok(TourDemoDiscussionResponse {
                discussion_id: report.discussion_id,
                created: !report.already_imported,
                prompt: seeded_prompt,
            })
        })
        .await
    {
        Ok(response) => Json(ApiResponse::ok(response)),
        Err(error) => Json(ApiResponse::err(format!(
            "Guided-tour discussion setup failed: {error}"
        ))),
    }
}

/// KT-74 — provenance of an imported discussion, as the sidebar needs it.
/// `imported_by_*` stay `None` for bundles exported before the envelope carried
/// an identity: the row is still a real import, just an anonymous one.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionImportProvenance {
    pub disc_id: String,
    /// `portable_bundle` today; `agent_transcript` is reserved for the import
    /// route that does not exist yet.
    pub provenance_kind: String,
    pub imported_by_pseudo: Option<String>,
    pub imported_by_avatar_email: Option<String>,
    pub imported_at: String,
}

fn list_import_provenance(
    conn: &rusqlite::Connection,
) -> anyhow::Result<Vec<DiscussionImportProvenance>> {
    let mut statement = conn.prepare(
        "SELECT imported_discussion_id, provenance_kind,
                imported_by_pseudo, imported_by_avatar_email, imported_at
         FROM discussion_imports
         ORDER BY imported_at DESC",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(DiscussionImportProvenance {
                disc_id: row.get(0)?,
                provenance_kind: row.get(1)?,
                imported_by_pseudo: row.get(2)?,
                imported_by_avatar_email: row.get(3)?,
                imported_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// GET /api/disc/imports
pub async fn list_discussion_imports(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<DiscussionImportProvenance>>> {
    match state.db.with_read_conn(list_import_provenance).await {
        Ok(rows) => Json(ApiResponse::ok(rows)),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

/// POST /api/discussions/import
pub async fn import_discussion(
    State(state): State<AppState>,
    Json(request): Json<ImportDiscussionRequest>,
) -> Json<ApiResponse<ImportDiscussionReport>> {
    let envelope: DiscussionExportEnvelope = match serde_json::from_str(&request.content) {
        Ok(value) => value,
        Err(error) => return Json(ApiResponse::err(format!("Invalid JSON: {error}"))),
    };
    if envelope.kind != DISCUSSION_EXPORT_KIND {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!(
                "Wrong bundle kind: expected `{DISCUSSION_EXPORT_KIND}`, got `{}`",
                envelope.kind
            ),
        ));
    }
    if envelope.version > DISCUSSION_EXPORT_VERSION {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!(
                "Unsupported discussion export version {} (maximum {})",
                envelope.version, DISCUSSION_EXPORT_VERSION
            ),
        ));
    }
    if envelope.source_discussion_id.trim().is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Source discussion id is required",
        ));
    }
    let fingerprint = match content_fingerprint(&envelope) {
        Ok(value) => value,
        Err(error) => return Json(ApiResponse::err(format!("Fingerprint error: {error}"))),
    };
    match state
        .db
        .with_conn(move |conn| import_bundle(conn, envelope, request.project_id, fingerprint))
        .await
    {
        Ok(report) => Json(ApiResponse::ok(report)),
        Err(error) if error.to_string().starts_with("IMPORT_CONFLICT:") => {
            Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                error.to_string().trim_start_matches("IMPORT_CONFLICT: "),
            ))
        }
        Err(error) => Json(ApiResponse::err(format!(
            "Discussion import failed: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_discussion(id: &str) -> Discussion {
        Discussion {
            id: id.into(),
            project_id: None,
            title: "Portable".into(),
            agent: crate::models::AgentType::Codex,
            language: "en".into(),
            participants: vec![crate::models::AgentType::Codex],
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
            tier: crate::models::ModelTier::Default,
            model: None,
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::Off,
            introspection_call_count: 0,
            shared_id: None,
            shared_with: vec![],
            workflow_run_id: None,
            awaiting_agent: false,
            test_mode_restore_branch: None,
            test_mode_stash_ref: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn filename_is_sanitized() {
        assert_eq!(
            filename_for("Plan / prod: été"),
            "Plan---prod--été.kronn-discussion.json"
        );
        assert_eq!(filename_for("///"), "discussion.kronn-discussion.json");
    }

    #[test]
    fn imported_file_guard_removes_files_until_commit() {
        let directory = tempfile::tempdir().unwrap();
        let rolled_back = directory.path().join("rolled-back.txt");
        std::fs::write(&rolled_back, "temporary").unwrap();
        {
            let mut guard = ImportedFilesGuard::default();
            guard.track(&Some(rolled_back.to_string_lossy().into_owned()));
        }
        assert!(!rolled_back.exists());

        let committed = directory.path().join("committed.txt");
        std::fs::write(&committed, "kept").unwrap();
        {
            let mut guard = ImportedFilesGuard::default();
            guard.track(&Some(committed.to_string_lossy().into_owned()));
            guard.commit();
        }
        assert!(committed.exists());
    }

    #[test]
    fn fingerprint_ignores_export_timestamp() {
        let base = DiscussionExportEnvelope {
            kind: DISCUSSION_EXPORT_KIND.into(),
            version: DISCUSSION_EXPORT_VERSION,
            exported_at: Utc::now(),
            secret_policy: SECRET_POLICY.into(),
            source_discussion_id: "disc-source".into(),
            discussion: test_discussion("disc-source"),
            messages: vec![],
            attachments: vec![],
            revision_events: vec![],
            plan: vec![],
            exported_by: None,
        };
        let mut later = base.clone();
        later.exported_at = Utc::now() + chrono::Duration::days(1);
        assert_eq!(
            content_fingerprint(&base).unwrap(),
            content_fingerprint(&later).unwrap()
        );

        // KT-74 — same discussion, other exporter: still the same content, or
        // a colleague's copy would be rejected as a conflicting bundle.
        let mut by_someone_else = base.clone();
        by_someone_else.exported_by = Some(PortableExporterIdentity {
            pseudo: Some("Romu".into()),
            avatar_email: Some("romu@example.test".into()),
        });
        assert_eq!(
            content_fingerprint(&base).unwrap(),
            content_fingerprint(&by_someone_else).unwrap()
        );
    }

    #[test]
    fn exporter_identity_is_omitted_rather_than_empty() {
        assert!(exporter_identity(None, None).is_none());
        assert!(exporter_identity(Some("   ".into()), Some(String::new())).is_none());
        let only_avatar = exporter_identity(None, Some("romu@example.test".into())).unwrap();
        assert_eq!(only_avatar.pseudo, None);
        assert_eq!(
            only_avatar.avatar_email.as_deref(),
            Some("romu@example.test")
        );
    }

    /// A bundle written before KT-74 has no `exported_by` key at all. Deleting
    /// the field must not turn into a deserialization error on a real file.
    #[test]
    fn a_legacy_bundle_without_exporter_still_deserializes() {
        let legacy = serde_json::json!({
            "kind": DISCUSSION_EXPORT_KIND,
            "version": DISCUSSION_EXPORT_VERSION,
            "exported_at": "2026-07-01T10:00:00Z",
            "secret_policy": SECRET_POLICY,
            "source_discussion_id": "legacy-source",
            "discussion": test_discussion("legacy-source"),
            "messages": [],
            "attachments": [],
            "revision_events": [],
            "plan": []
        });
        let envelope: DiscussionExportEnvelope = serde_json::from_value(legacy).unwrap();
        assert!(envelope.exported_by.is_none());

        // And round-trips back out WITHOUT the key: `serde(default)` alone would
        // have written `"exported_by": null` into every bundle.
        let written = serde_json::to_value(&envelope).unwrap();
        assert!(
            written.get("exported_by").is_none(),
            "an instance with no identity must omit the key, not export a null author"
        );
    }

    #[tokio::test]
    async fn rich_bundle_round_trips_idempotently_and_reports_conflict() {
        let database = crate::db::Database::open_in_memory().unwrap();
        database
            .with_conn(|conn| {
                let source_id = "portable-source";
                crate::db::discussions::insert_discussion(conn, &test_discussion(source_id))?;
                let source_message = DiscussionMessage {
                    id: "portable-message".into(),
                    role: crate::models::MessageRole::User,
                    channel: crate::models::MessageChannel::Main,
                    content: "Bonjour avec pièce jointe".into(),
                    agent_type: None,
                    timestamp: Utc::now(),
                    tokens_used: 12,
                    auth_mode: Some("subscription".into()),
                    model_tier: None,
                    model: None,
                    cost_usd: None,
                    author_pseudo: Some("Romuald".into()),
                    author_avatar_email: Some("avatar@example.test".into()),
                    source_msg_id: None,
                    duration_ms: None,
                    lint_report: None,
                    target_agent: None,
                    reply_to_message_id: None,
                };
                crate::db::discussions::insert_message(conn, source_id, &source_message)?;
                let mut source_reply = source_message.clone();
                source_reply.id = "portable-reply".into();
                source_reply.role = crate::models::MessageRole::Agent;
                source_reply.content = "Je m'en occupe".into();
                source_reply.agent_type = Some(crate::models::AgentType::Codex);
                source_reply.author_pseudo = None;
                source_reply.author_avatar_email = None;
                source_reply.reply_to_message_id = Some(source_message.id.clone());
                crate::db::discussions::insert_message(conn, source_id, &source_reply)?;
                conn.execute(
                    "INSERT INTO context_files
                     (id, discussion_id, filename, mime_type, original_size,
                      extracted_text, extracted_size, message_id)
                     VALUES ('portable-file', ?1, 'notes.md', 'text/markdown',
                             12, 'hello export', 12, ?2)",
                    params![source_id, source_message.id],
                )?;
                conn.execute(
                    "INSERT INTO message_revision_events
                     (id, discussion_id, target_message_id, previous_content_hash,
                      expected_revision, revision, content, idempotency_key,
                      sort_order, created_at)
                     VALUES ('portable-revision', ?1, ?2, 'before', 'r0', 'r1',
                             'edited', 'portable-revision-key', 1, ?3)",
                    params![source_id, source_message.id, Utc::now().to_rfc3339()],
                )?;
                conn.execute(
                    "UPDATE discussions SET next_message_seq = 2 WHERE id = ?1",
                    [source_id],
                )?;

                let task = crate::db::planning::create_task(
                    conn,
                    &CreatePlanningTaskRequest {
                        title: "Portable task".into(),
                        discussion_id: None,
                        idempotency_key: None,
                        description: "Task body".into(),
                        status: crate::models::PlanningTaskStatus::InProgress,
                        priority: crate::models::PlanningTaskPriority::High,
                        parent_id: None,
                        project_ids: vec![],
                        tags: vec!["portable".into()],
                        definition_of_done: vec![CreatePlanningDodItem {
                            id: None,
                            sentence: "Round-trip works".into(),
                            completed: true,
                        }],
                        links: vec![CreatePlanningTaskLink {
                            label: "Spec".into(),
                            url: "https://example.test/spec".into(),
                        }],
                        actor: PlanningActor {
                            kind: PlanningActorKind::Human,
                            id: Some("tester".into()),
                            source_message_id: Some(source_message.id.clone()),
                        },
                    },
                )?;
                crate::db::planning::link_discussion(
                    conn,
                    &task.summary.id,
                    &LinkPlanningDiscussionRequest {
                        discussion_id: source_id.into(),
                        placement: PlanningPlacement::Active,
                        is_primary: true,
                        position: Some(0),
                        actor: PlanningActor::default(),
                    },
                )?;

                let envelope = build_export_envelope(
                    conn,
                    source_id,
                    exporter_identity(Some("Romu".into()), Some("romu@example.test".into())),
                )?;
                assert_eq!(envelope.messages.len(), 2);
                assert_eq!(envelope.attachments.len(), 1);
                assert_eq!(envelope.revision_events.len(), 1);
                assert_eq!(envelope.plan.len(), 1);
                let fingerprint = content_fingerprint(&envelope)?;
                let report = import_bundle(conn, envelope.clone(), None, fingerprint.clone())?;
                assert!(!report.already_imported);
                assert_eq!(report.imported_messages, 2);
                assert_eq!(report.imported_attachments, 1);
                assert_eq!(report.imported_revision_events, 1);
                assert_eq!(report.imported_tasks, 1);

                let imported = crate::db::discussions::get_discussion(conn, &report.discussion_id)?
                    .expect("imported discussion");
                assert_eq!(imported.messages.len(), 2);
                assert_eq!(
                    imported.messages[0].author_pseudo.as_deref(),
                    Some("Romuald")
                );
                assert_eq!(
                    imported.messages[0].source_msg_id.as_deref(),
                    Some("portable-message")
                );
                assert_eq!(
                    imported.messages[1].source_msg_id.as_deref(),
                    Some("portable-reply")
                );
                assert_eq!(
                    imported.messages[1].reply_to_message_id.as_deref(),
                    Some(imported.messages[0].id.as_str())
                );
                let extracted: String = conn.query_row(
                    "SELECT extracted_text FROM context_files WHERE discussion_id = ?1",
                    [&report.discussion_id],
                    |row| row.get(0),
                )?;
                assert_eq!(extracted, "hello export");
                let imported_plan =
                    crate::db::planning::get_discussion_plan(conn, &report.discussion_id)?;
                assert_eq!(imported_plan.active.len(), 1);
                assert!(imported_plan.active[0].is_primary);
                let imported_task =
                    crate::db::planning::get_task(conn, &imported_plan.active[0].task.id)?
                        .expect("imported task");
                assert!(imported_task.definition_of_done[0].completed);

                // KT-74 — the ledger keeps the exporter, tagged as a portable
                // bundle and never confused with the CLI binding.
                let provenance: (String, Option<String>, Option<String>) = conn.query_row(
                    "SELECT provenance_kind, imported_by_pseudo, imported_by_avatar_email
                     FROM discussion_imports WHERE imported_discussion_id = ?1",
                    [&report.discussion_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                assert_eq!(provenance.0, "portable_bundle");
                assert_eq!(provenance.1.as_deref(), Some("Romu"));
                assert_eq!(provenance.2.as_deref(), Some("romu@example.test"));
                // The read path the sidebar actually calls.
                let exposed = list_import_provenance(conn)?;
                assert_eq!(exposed.len(), 1);
                assert_eq!(exposed[0].disc_id, report.discussion_id);
                assert_eq!(exposed[0].provenance_kind, "portable_bundle");
                assert_eq!(exposed[0].imported_by_pseudo.as_deref(), Some("Romu"));

                let bindings = crate::db::disc_source::list_all_source_bindings(conn)?;
                assert!(
                    !bindings.iter().any(|b| b.disc_id == report.discussion_id),
                    "an imported bundle must not masquerade as a bound CLI session"
                );

                let replay = import_bundle(conn, envelope.clone(), None, fingerprint)?;
                assert!(replay.already_imported);
                assert_eq!(replay.discussion_id, report.discussion_id);

                let mut changed = envelope;
                changed.discussion.title = "Different content".into();
                let changed_fingerprint = content_fingerprint(&changed)?;
                let conflict = import_bundle(conn, changed, None, changed_fingerprint).unwrap_err();
                assert!(conflict.to_string().starts_with("IMPORT_CONFLICT:"));
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
    }
}
