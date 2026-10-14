//! KT-982 — review the audit's tech-debt sheets from the project card: list
//! them, settle them (one at a time or in bulk), turn one into a Kronn task,
//! and talk about one with the chosen agents.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::Connection;

use crate::core::audit_validation::{
    apply_td_resolutions_at, TdDecisionOrigin, TdResolution, TdResolutionAction,
};
use crate::core::docs_location::DocsRoot;
use crate::core::td_index::is_td_id;
use crate::core::{scanner, td_review, td_tasks};
use crate::models::*;
use crate::AppState;

const MAX_DECISIONS: usize = 500;
const MAX_REASON_CHARS: usize = 2000;
const MAX_TALK_MESSAGE_CHARS: usize = 8000;
const EXCERPT_MAX: usize = 1200;
const TD_MESSAGE_MAX: usize = 8000;

/// The project and its directory on this host.
async fn project_dir(state: &AppState, id: String) -> Result<(Project, PathBuf), ApiErrorBody> {
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(project)) => project,
        Ok(None) => return Err((ApiErrorCode::NotFound, "Project not found".into())),
        Err(e) => return Err((ApiErrorCode::Internal, format!("DB error: {e}"))),
    };
    let path = scanner::resolve_host_path(&project.path);
    if !path.is_dir() {
        return Err((ApiErrorCode::NotFound, "Project directory not found".into()));
    }
    Ok((project, path))
}

type ApiErrorBody = (ApiErrorCode, String);

fn err<T: serde::Serialize>((code, message): ApiErrorBody) -> Json<ApiResponse<T>> {
    Json(ApiResponse::err_coded(code, message))
}

/// The TD sheets under the project's docs root, where the audit and
/// `apply_td_resolutions_at` write them (KT-1192).
fn read_root_items(project_path: &std::path::Path) -> Vec<TdReviewItem> {
    let docs_root = DocsRoot::of(project_path);
    td_review::read_items_rel(&docs_root.dir(project_path), docs_root.rel())
}

async fn read_items_blocking(project_path: PathBuf) -> anyhow::Result<Vec<TdReviewItem>> {
    Ok(tokio::task::spawn_blocking(move || read_root_items(&project_path)).await?)
}

fn parse_timestamp(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .map(|t| t.with_timezone(&Utc))
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S").map(|t| t.and_utc()))
        .unwrap_or_default()
}

/// What the list needs from the DB, in one connection: latest run, linked
/// tasks, validation mentions and the sheets' own discussions.
struct ListContext {
    audit_date: Option<DateTime<Utc>>,
    validation_discussion_id: Option<String>,
    tasks: HashMap<String, TdLinkedTask>,
    mentions: HashMap<String, u32>,
    talks: HashMap<String, (String, u32)>,
    disputes: Vec<crate::db::audit_cross::PendingDispute>,
}

fn list_context(conn: &Connection, project_id: &str) -> anyhow::Result<ListContext> {
    let latest = crate::db::audit_runs::list_recent(conn, project_id, 1)?
        .into_iter()
        .next();
    let validation_discussion_id = latest
        .as_ref()
        .and_then(|r| r.validation_discussion_id.clone());
    let prefix = td_tasks::td_task_key(project_id, "");
    let tasks = crate::db::planning::tasks_by_idempotency_prefix(conn, &prefix)?
        .into_iter()
        .filter_map(|(key, id, reference)| {
            let td_id = key.strip_prefix(&prefix)?;
            is_td_id(td_id).then(|| (td_id.to_string(), TdLinkedTask { id, reference }))
        })
        .collect();
    let mut mentions: HashMap<String, u32> = HashMap::new();
    if let Some(disc_id) = validation_discussion_id.as_deref() {
        let mut stmt = conn.prepare(
            "SELECT content FROM messages WHERE discussion_id = ?1 AND content LIKE '%TD-%'",
        )?;
        let rows = stmt.query_map([disc_id], |row| row.get::<_, String>(0))?;
        for content in rows {
            for td_id in td_review::mentioned_td_ids(&content?) {
                *mentions.entry(td_id).or_default() += 1;
            }
        }
    }
    let mut talks: HashMap<String, (String, u32)> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT d.id, d.title,
                (SELECT COUNT(*) FROM messages m WHERE m.discussion_id = d.id)
         FROM discussions d
         WHERE d.project_id = ?1 AND d.archived = 0 AND d.title LIKE ?2
         ORDER BY d.updated_at DESC, d.rowid DESC",
    )?;
    let pattern = format!("{}TD-%", td_review::TALK_TITLE_PREFIX);
    let rows = stmt.query_map(rusqlite::params![project_id, pattern], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (id, title, count) = row?;
        if let Some(td_id) = td_review::talk_td_id(&title) {
            talks
                .entry(td_id.to_string())
                .or_insert((id, count.max(0) as u32));
        }
    }
    Ok(ListContext {
        audit_date: latest.map(|r| r.ended_at.unwrap_or(r.started_at)),
        validation_discussion_id,
        tasks,
        mentions,
        talks,
        disputes: crate::db::audit_cross::pending_disputes(conn, project_id)?,
    })
}

fn assemble(mut items: Vec<TdReviewItem>, context: ListContext, validated: bool) -> TdReviewList {
    for item in &mut items {
        item.task = context.tasks.get(&item.id).cloned();
        let mentions = context.mentions.get(&item.id).copied().unwrap_or(0);
        let (talk_id, talk_count) = context
            .talks
            .get(&item.id)
            .map_or((None, 0), |(id, count)| (Some(id.clone()), *count));
        item.talk_discussion_id = talk_id;
        item.talk_count = mentions + talk_count;
    }
    let (counts, by_severity) = td_review::counts(&items);
    let open = td_review::open_disputes(&items, &context.disputes);
    TdReviewList {
        gate: td_review::with_disputes(td_review::gate(&items), open.len()),
        counts,
        by_severity,
        validated,
        audit_date: context.audit_date,
        validation_discussion_id: context.validation_discussion_id,
        dispute_discussion_id: open.first().map(|d| d.discussion_id.clone()),
        items,
    }
}

/// GET /api/projects/{id}/audit/tds
pub async fn list_tds(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<TdReviewList>> {
    let (project, project_path) = match project_dir(&state, id).await {
        Ok(found) => found,
        Err(e) => return err(e),
    };
    let raw_path = project.path.clone();
    let files = tokio::task::spawn_blocking(move || {
        let items = read_root_items(&project_path);
        let validated = scanner::detect_audit_status(&raw_path) == AiAuditStatus::Validated;
        (items, validated)
    })
    .await;
    let Ok((items, validated)) = files else {
        return Json(ApiResponse::err("Could not read the tech-debt sheets"));
    };
    let pid = project.id.clone();
    match state
        .db
        .with_read_conn(move |conn| list_context(conn, &pid))
        .await
    {
        Ok(context) => Json(ApiResponse::ok(assemble(items, context, validated))),
        Err(e) => Json(ApiResponse::err(format!("DB error: {e}"))),
    }
}

fn action_of(decision: TdReviewDecision) -> TdResolutionAction {
    match decision {
        TdReviewDecision::Confirm => TdResolutionAction::Confirm,
        TdReviewDecision::AcceptDecision => TdResolutionAction::AcceptDecision,
        TdReviewDecision::Defer => TdResolutionAction::Defer,
        TdReviewDecision::Reject => TdResolutionAction::Reject,
    }
}

/// Checks a whole batch against the sheets on disk before anything is
/// written: unknown or duplicate ids refuse the batch.
pub(crate) fn check_batch(items: &[TdDecisionItem], known: &[TdReviewItem]) -> Result<(), String> {
    if items.is_empty() {
        return Err("Choose at least one tech-debt sheet".into());
    }
    if items.len() > MAX_DECISIONS {
        return Err(format!("At most {MAX_DECISIONS} sheets per decision"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut unknown = Vec::new();
    for item in items {
        if !seen.insert(item.td_id.as_str()) {
            return Err(format!("{} is listed twice", item.td_id));
        }
        if !known.iter().any(|k| k.id == item.td_id) {
            unknown.push(item.td_id.as_str());
        }
        if item
            .reason
            .as_deref()
            .is_some_and(|r| r.chars().count() > MAX_REASON_CHARS)
        {
            return Err(format!(
                "The reason for {} exceeds {MAX_REASON_CHARS} characters",
                item.td_id
            ));
        }
    }
    if !unknown.is_empty() {
        return Err(format!("Unknown tech-debt sheet: {}", unknown.join(", ")));
    }
    Ok(())
}

/// Settles the batch on the sheets: all of them, or none.
pub(crate) fn decide_at(
    project_path: &std::path::Path,
    items: &[TdDecisionItem],
) -> Result<TdDecideOutcome, String> {
    check_batch(items, &read_root_items(project_path))?;
    let resolutions: Vec<TdResolution> = items
        .iter()
        .map(|item| TdResolution {
            td_id: item.td_id.clone(),
            action: action_of(item.decision),
        })
        .collect();
    let with_reasons: Vec<(&TdResolution, Option<&str>)> = resolutions
        .iter()
        .zip(items)
        .map(|(resolution, item)| {
            (
                resolution,
                item.reason
                    .as_deref()
                    .map(str::trim)
                    .filter(|r| !r.is_empty()),
            )
        })
        .collect();
    apply_td_resolutions_at(project_path, &with_reasons, TdDecisionOrigin::Review)
        .map_err(|e| e.to_string())?;
    let after = read_root_items(project_path);
    let states: BTreeMap<&str, TdReviewState> =
        after.iter().map(|i| (i.id.as_str(), i.state)).collect();
    let results = items
        .iter()
        .map(|item| TdDecisionResult {
            td_id: item.td_id.clone(),
            state: states
                .get(item.td_id.as_str())
                .copied()
                .unwrap_or(TdReviewState::Pending),
        })
        .collect();
    Ok(TdDecideOutcome {
        results,
        gate: td_review::gate(&after),
    })
}

/// POST /api/projects/{id}/audit/tds/decide
pub async fn decide_tds(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<TdDecideRequest>,
) -> Json<ApiResponse<TdDecideOutcome>> {
    let (project, project_path) = match project_dir(&state, id).await {
        Ok(found) => found,
        Err(e) => return err(e),
    };
    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &project.id).await {
            Ok(lease) => lease,
            Err(error) => return Json(ApiResponse::err_coded(ApiErrorCode::Conflict, error)),
        };
    let items = req.items;
    let docs_root = DocsRoot::of(&project_path);
    let docs_dir = docs_root.dir(&project_path);
    let docs_rel = docs_root.rel().to_string();
    match lease
        .blocking(move || {
            let before = crate::core::audit_documents::capture(&project_path)?;
            let outcome = decide_at(&project_path, &items)?;
            let after = crate::core::audit_documents::capture(&project_path)?;
            Ok::<_, String>((outcome, before, after))
        })
        .await
    {
        Ok(Ok((mut outcome, documents_before, documents_after))) => {
            let pid = project.id.clone();
            let after = state
                .db
                .with_conn(move |conn| {
                    crate::db::audit_documents::advance_local_changes(
                        conn,
                        &pid,
                        &documents_before,
                        &documents_after,
                    )?;
                    td_tasks::close_inactive_logged(conn, &pid, &docs_dir, "tech-debt review");
                    let pending = crate::db::audit_cross::pending_disputes(conn, &pid)?;
                    Ok((td_review::read_items_rel(&docs_dir, &docs_rel), pending))
                })
                .await;
            let (items, pending) = match after {
                Ok(result) => result,
                Err(e) => {
                    return Json(ApiResponse::err(format!(
                        "Could not record the tech-debt review: {e}"
                    )))
                }
            };
            let open = td_review::open_disputes(&items, &pending).len();
            outcome.gate = td_review::with_disputes(outcome.gate, open);
            Json(ApiResponse::ok(outcome))
        }
        Ok(Err(message)) => Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
        Err(e) => Json(ApiResponse::err(format!("Task failed: {e}"))),
    }
}

/// POST /api/projects/{id}/audit/tds/{td_id}/task — the KT-1167 conversion
/// for one sheet; a sheet already linked reports its task.
pub async fn create_td_task(
    State(state): State<AppState>,
    Path((id, td_id)): Path<(String, String)>,
) -> Json<ApiResponse<TdTasksOutcome>> {
    if !is_td_id(&td_id) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Invalid tech-debt id",
        ));
    }
    let (project, project_path) = match project_dir(&state, id).await {
        Ok(found) => found,
        Err(e) => return err(e),
    };
    let wanted = td_id.clone();
    let sources = tokio::task::spawn_blocking(move || {
        let docs_root = DocsRoot::of(&project_path);
        td_tasks::load_sources_rel(&docs_root.dir(&project_path), docs_root.rel())
            .into_iter()
            .filter(|s| s.sheet.id == wanted)
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    if sources.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            format!("Unknown tech-debt sheet: {td_id}"),
        ));
    }
    let pid = project.id;
    match state
        .db
        .with_conn(move |conn| td_tasks::convert(conn, &pid, &sources, &PlanningActor::default()))
        .await
    {
        Ok(outcome) => Json(ApiResponse::ok(outcome)),
        Err(e) => Json(ApiResponse::err(format!("Could not create the task: {e}"))),
    }
}

fn talk_messages(conn: &Connection, project_id: &str, td_id: &str) -> anyhow::Result<TdTalk> {
    let context = list_context(conn, project_id)?;
    let discussion_id = context.talks.get(td_id).map(|(id, _)| id.clone());
    let mut messages = Vec::new();
    let mut read = |disc_id: &str, source: TdTalkSource| -> anyhow::Result<()> {
        let mut stmt = conn.prepare(
            "SELECT id, role, agent_type, author_pseudo, content, timestamp
             FROM messages WHERE discussion_id = ?1 AND (?2 = '' OR content LIKE ?2)
             ORDER BY sort_order, timestamp",
        )?;
        let filter = match source {
            TdTalkSource::Validation => format!("%{td_id}%"),
            TdTalkSource::Td => String::new(),
        };
        let rows = stmt.query_map(rusqlite::params![disc_id, filter], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        for row in rows {
            let (id, role, agent_type, author, content, timestamp) = row?;
            let (content, excerpt) = match source {
                TdTalkSource::Validation => {
                    if !td_review::mentioned_td_ids(&content).contains(td_id) {
                        continue;
                    }
                    td_review::mention_excerpt(&content, td_id, EXCERPT_MAX)
                }
                TdTalkSource::Td => {
                    let cut = content.chars().count() > TD_MESSAGE_MAX;
                    (td_tasks::truncate(&content, TD_MESSAGE_MAX), cut)
                }
            };
            messages.push(TdTalkMessage {
                id,
                discussion_id: disc_id.to_string(),
                source,
                role,
                agent_type,
                author,
                content,
                excerpt,
                timestamp: parse_timestamp(&timestamp),
            });
        }
        Ok(())
    };
    if let Some(disc_id) = context.validation_discussion_id.as_deref() {
        read(disc_id, TdTalkSource::Validation)?;
    }
    if let Some(disc_id) = discussion_id.as_deref() {
        read(disc_id, TdTalkSource::Td)?;
    }
    Ok(TdTalk {
        validation_discussion_id: context.validation_discussion_id,
        discussion_id,
        messages,
    })
}

/// GET /api/projects/{id}/audit/tds/{td_id}/talk — validation messages that
/// name the sheet, then the sheet's own discussion.
pub async fn td_talk(
    State(state): State<AppState>,
    Path((id, td_id)): Path<(String, String)>,
) -> Json<ApiResponse<TdTalk>> {
    if !is_td_id(&td_id) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Invalid tech-debt id",
        ));
    }
    let pid = id.clone();
    let exists = state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await;
    if !matches!(exists, Ok(Some(_))) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Project not found",
        ));
    }
    match state
        .db
        .with_read_conn(move |conn| talk_messages(conn, &id, &td_id))
        .await
    {
        Ok(talk) => Json(ApiResponse::ok(talk)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {e}"))),
    }
}

/// The creation request of a sheet's discussion: the first target leads, the
/// others answer the same first turn.
pub(crate) fn talk_discussion_request(
    project_id: &str,
    item: &TdReviewItem,
    message: &str,
    targets: &[MessageTarget],
    language: String,
) -> Option<CreateDiscussionRequest> {
    let lead = targets.first()?;
    let initial_targets = targets
        .iter()
        .enumerate()
        .map(|(index, target)| MessageTarget {
            kind: if index == 0 {
                MessageTargetKind::DiscussionAgent
            } else {
                MessageTargetKind::Agent
            },
            cli_session_id: None,
            ..target.clone()
        })
        .collect();
    Some(CreateDiscussionRequest {
        project_id: Some(project_id.to_string()),
        title: td_review::talk_title(item),
        agent: lead.agent_type.clone(),
        connection_id: lead.connection_id.clone(),
        language,
        initial_prompt: td_review::talk_prompt(item, message),
        initial_targets,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        workspace_mode: None,
        base_branch: None,
        tier: lead.tier.unwrap_or_default(),
        originating_qp_id: None,
        launch_variables: HashMap::new(),
        no_agent: false,
        assistant: None,
    })
}

/// POST /api/projects/{id}/audit/tds/{td_id}/talk — the sheet's discussion,
/// created with this message as its first turn when it does not exist. The
/// client then runs it (`/run`) or sends the message to it, as anywhere else.
pub async fn start_td_talk(
    State(state): State<AppState>,
    Path((id, td_id)): Path<(String, String)>,
    Json(req): Json<TdTalkRequest>,
) -> Json<ApiResponse<TdTalkStart>> {
    if !is_td_id(&td_id) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Invalid tech-debt id",
        ));
    }
    let message = req.message.trim().to_string();
    if message.is_empty() || message.chars().count() > MAX_TALK_MESSAGE_CHARS {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Write a message (at most 8000 characters)",
        ));
    }
    if req.targets.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Choose at least one agent",
        ));
    }
    let (project, project_path) = match project_dir(&state, id).await {
        Ok(found) => found,
        Err(e) => return err(e),
    };
    let items = match read_items_blocking(project_path).await {
        Ok(items) => items,
        Err(e) => return Json(ApiResponse::err(format!("Task failed: {e}"))),
    };
    let Some(item) = items.into_iter().find(|i| i.id == td_id) else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            format!("Unknown tech-debt sheet: {td_id}"),
        ));
    };
    let (pid, wanted) = (project.id.clone(), td_id.clone());
    let existing = state
        .db
        .with_read_conn(move |conn| Ok(list_context(conn, &pid)?.talks.remove(&wanted)))
        .await;
    match existing {
        Ok(Some((discussion_id, _))) => {
            return Json(ApiResponse::ok(TdTalkStart {
                discussion_id,
                created: false,
            }))
        }
        Ok(None) => {}
        Err(e) => return Json(ApiResponse::err(format!("DB error: {e}"))),
    }
    let language = state.config.read().await.language.clone();
    let Some(request) =
        talk_discussion_request(&project.id, &item, &message, &req.targets, language)
    else {
        return Json(ApiResponse::err("Choose at least one agent"));
    };
    let created = crate::api::discussions::create(State(state.clone()), Json(request))
        .await
        .0;
    match (created.success, created.data) {
        (true, Some(discussion)) => Json(ApiResponse::ok(TdTalkStart {
            discussion_id: discussion.id,
            created: true,
        })),
        _ => Json(ApiResponse::err(
            created
                .error
                .unwrap_or_else(|| "Could not create the discussion".into()),
        )),
    }
}

#[cfg(test)]
#[path = "td_review_tests.rs"]
mod tests;
