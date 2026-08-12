use std::str::FromStr;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::models::{
    AddPlanningBlockerRequest, CreatePlanningDodItem, CreatePlanningTaskLink,
    CreatePlanningTaskRequest, DiscussionPlan, LinkPlanningDiscussionRequest, PlanningActor,
    PlanningActorKind, PlanningDependencySummary, PlanningDiscussionRelation, PlanningDodItem,
    PlanningPlacement, PlanningPlanStats, PlanningTaskChange, PlanningTaskDetail,
    PlanningTaskEvent, PlanningTaskLink, PlanningTaskListQuery, PlanningTaskListResponse,
    PlanningTaskPriority, PlanningTaskStatus, PlanningTaskSummary, PlanningWorkspaceSummary,
    UpdatePlanningDodItemRequest, UpdatePlanningTaskRequest,
};

fn parse_dt(value: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&value)
        .map(|date| date.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// A transaction that is nesting-aware: it runs a real `BEGIN`/`COMMIT` when the
/// connection is in autocommit, or a `SAVEPOINT`/`RELEASE` when it is already
/// inside a transaction. This lets a mutation fn (create/update/link) be called
/// standalone OR composed inside a larger transaction (e.g. the planning-proposal
/// decision applies task mutation + item state + receipt atomically) without the
/// "cannot start a transaction within a transaction" error. Rolls back on drop
/// unless `commit()` was called. Derefs to `Connection` so bodies are unchanged.
struct NestableTx<'c> {
    conn: &'c Connection,
    savepoint: bool,
    committed: bool,
}

impl<'c> NestableTx<'c> {
    fn begin(conn: &'c Connection) -> Result<Self> {
        let savepoint = !conn.is_autocommit();
        conn.execute_batch(if savepoint {
            "SAVEPOINT planning_mut"
        } else {
            "BEGIN"
        })?;
        Ok(Self {
            conn,
            savepoint,
            committed: false,
        })
    }

    fn begin_immediate(conn: &'c Connection) -> Result<Self> {
        let savepoint = !conn.is_autocommit();
        conn.execute_batch(if savepoint {
            "SAVEPOINT planning_mut"
        } else {
            "BEGIN IMMEDIATE"
        })?;
        Ok(Self {
            conn,
            savepoint,
            committed: false,
        })
    }

    fn commit(mut self) -> Result<()> {
        self.conn.execute_batch(if self.savepoint {
            "RELEASE planning_mut"
        } else {
            "COMMIT"
        })?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for NestableTx<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.conn.execute_batch(if self.savepoint {
                "ROLLBACK TO planning_mut; RELEASE planning_mut"
            } else {
                "ROLLBACK"
            });
        }
    }
}

impl std::ops::Deref for NestableTx<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn
    }
}

fn validate_actor(actor: &PlanningActor) -> Result<()> {
    if actor.kind == PlanningActorKind::Agent
        && actor.id.as_deref().is_none_or(|id| id.trim().is_empty())
    {
        bail!("Agent task changes must identify the acting agent");
    }
    Ok(())
}

fn validate_title(title: &str) -> Result<()> {
    let length = title.chars().count();
    if !(1..=240).contains(&length) {
        bail!("Task title must be 1-240 characters");
    }
    Ok(())
}

fn normalized_idempotency_key(request: &CreatePlanningTaskRequest) -> Result<Option<&str>> {
    match request.idempotency_key.as_deref() {
        Some(key) => {
            let key = key.trim();
            if key.is_empty() || key.chars().count() > 240 {
                bail!("Planning task idempotency keys must be 1-240 characters");
            }
            Ok(Some(key))
        }
        None => Ok(None),
    }
}

fn normalized_create_discussion_id(request: &CreatePlanningTaskRequest) -> Result<Option<&str>> {
    match request.discussion_id.as_deref() {
        Some(discussion_id) => {
            let discussion_id = discussion_id.trim();
            if discussion_id.is_empty() {
                bail!("Discussion ID must not be empty");
            }
            Ok(Some(discussion_id))
        }
        None => Ok(None),
    }
}

fn validate_dod(items: &[CreatePlanningDodItem]) -> Result<()> {
    if items.len() > 200 {
        bail!("A task cannot contain more than 200 Definition of Done items");
    }
    if items
        .iter()
        .any(|item| item.sentence.trim().is_empty() || item.sentence.chars().count() > 500)
    {
        bail!("Definition of Done items must be 1-500 characters");
    }
    Ok(())
}

fn validate_links(links: &[CreatePlanningTaskLink]) -> Result<()> {
    if links.len() > 100 {
        bail!("A task cannot contain more than 100 links");
    }
    for link in links {
        if link.label.trim().is_empty() || link.label.chars().count() > 200 {
            bail!("Task link labels must be 1-200 characters");
        }
        let url = link.url.trim();
        if url.len() > 2_048
            || !(url.starts_with("https://")
                || url.starts_with("http://")
                || url.starts_with("file://"))
        {
            bail!("Task links must use http://, https:// or file://");
        }
    }
    Ok(())
}

fn normalize_tags(tags: &[String]) -> Result<Vec<String>> {
    if tags.len() > 50 {
        bail!("A task cannot contain more than 50 tags");
    }
    let mut normalized = Vec::new();
    for tag in tags {
        let value = tag.trim();
        if value.is_empty() || value.chars().count() > 80 {
            bail!("Task tags must be 1-80 characters");
        }
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(value))
        {
            normalized.push(value.to_string());
        }
    }
    Ok(normalized)
}

fn create_request_fingerprint(
    request: &CreatePlanningTaskRequest,
    parent_id: Option<&str>,
    tags: &[String],
    discussion_id: Option<&str>,
) -> Result<String> {
    let mut project_ids = request.project_ids.clone();
    project_ids.sort();
    project_ids.dedup();
    let mut normalized_tags = tags
        .iter()
        .map(|tag| tag.to_lowercase())
        .collect::<Vec<_>>();
    normalized_tags.sort();
    let definition_of_done = request
        .definition_of_done
        .iter()
        .map(|item| {
            serde_json::json!({
                "sentence": item.sentence.trim(),
                "completed": item.completed,
            })
        })
        .collect::<Vec<_>>();
    let links = request
        .links
        .iter()
        .map(|link| {
            serde_json::json!({
                "label": link.label.trim(),
                "url": link.url.trim(),
            })
        })
        .collect::<Vec<_>>();
    let mut canonical_value = serde_json::json!({
        "title": request.title.trim(),
        "description": request.description,
        "status": request.status,
        "priority": request.priority,
        "parent_id": parent_id,
        "project_ids": project_ids,
        "tags": normalized_tags,
        "definition_of_done": definition_of_done,
        "links": links,
    });
    // Preserve the pre-discussion-target fingerprint byte-for-byte when the
    // optional field is absent, so retries created by older clients remain
    // valid after this additive API change.
    if let Some(discussion_id) = discussion_id {
        canonical_value
            .as_object_mut()
            .context("Planning task fingerprint must be an object")?
            .insert(
                "discussion_id".to_string(),
                serde_json::Value::String(discussion_id.to_string()),
            );
    }
    let canonical = serde_json::to_vec(&canonical_value)?;
    let digest: String = Sha256::digest(canonical)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(format!("v1:{digest}"))
}

fn ensure_task_exists(conn: &Connection, task_id: &str) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM planning_tasks WHERE id = ?1)",
        [task_id],
        |row| row.get(0),
    )?;
    if !exists {
        bail!("Planning task not found");
    }
    Ok(())
}

fn resolve_task_id(conn: &Connection, reference: &str) -> Result<String> {
    if let Some(number) = reference
        .strip_prefix("KT-")
        .or_else(|| reference.strip_prefix("kt-"))
        .and_then(|value| value.parse::<i64>().ok())
    {
        return conn
            .query_row(
                "SELECT id FROM planning_tasks WHERE task_number = ?1",
                [number],
                |row| row.get(0),
            )
            .optional()?
            .context("Planning task not found");
    }
    ensure_task_exists(conn, reference)?;
    Ok(reference.to_string())
}

fn ensure_parent_is_valid(conn: &Connection, task_id: Option<&str>, parent_id: &str) -> Result<()> {
    ensure_task_exists(conn, parent_id)?;
    if task_id == Some(parent_id) {
        bail!("A task cannot be its own parent");
    }
    if let Some(task_id) = task_id {
        let creates_cycle: bool = conn.query_row(
            "WITH RECURSIVE ancestors(id) AS (
                SELECT parent_id FROM planning_tasks WHERE id = ?1
                UNION ALL
                SELECT planning_tasks.parent_id
                FROM planning_tasks
                JOIN ancestors ON planning_tasks.id = ancestors.id
                WHERE planning_tasks.parent_id IS NOT NULL
             )
             SELECT EXISTS(SELECT 1 FROM ancestors WHERE id = ?2)",
            params![parent_id, task_id],
            |row| row.get(0),
        )?;
        if creates_cycle {
            bail!("Task hierarchy cycle detected");
        }
    }
    Ok(())
}

fn insert_event(
    conn: &Connection,
    task_id: &str,
    action: &str,
    actor: &PlanningActor,
    changes: serde_json::Value,
) -> Result<()> {
    validate_actor(actor)?;
    conn.execute(
        "INSERT INTO planning_task_events
         (id, task_id, action, actor_kind, actor_id, changes_json, source_message_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            Uuid::new_v4().to_string(),
            task_id,
            action,
            actor.kind.as_str(),
            actor.id,
            serde_json::to_string(&changes)?,
            actor.source_message_id,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Compact event payload for an update: only the fields the caller actually
/// supplied, without the null-for-untouched noise or a copy of the actor
/// (already stored in dedicated columns). Keeps `task_changes` lean.
fn compact_update_changes(request: &UpdatePlanningTaskRequest) -> Result<serde_json::Value> {
    let mut changes = serde_json::Map::new();
    if let Some(value) = request.title.as_ref() {
        changes.insert("title".into(), serde_json::to_value(value)?);
    }
    if let Some(value) = request.description.as_ref() {
        changes.insert("description".into(), serde_json::to_value(value)?);
    }
    if let Some(value) = request.status.as_ref() {
        changes.insert("status".into(), serde_json::to_value(value)?);
    }
    if let Some(value) = request.priority.as_ref() {
        changes.insert("priority".into(), serde_json::to_value(value)?);
    }
    // Option<Option<_>>: an outer Some means the field was provided; the inner
    // value serializes to null when the caller clears it.
    if let Some(value) = request.parent_id.as_ref() {
        changes.insert("parent_id".into(), serde_json::to_value(value)?);
    }
    if let Some(value) = request.blocked_reason.as_ref() {
        changes.insert("blocked_reason".into(), serde_json::to_value(value)?);
    }
    if let Some(value) = request.rank.as_ref() {
        changes.insert("rank".into(), serde_json::to_value(value)?);
    }
    if let Some(value) = request.project_ids.as_ref() {
        changes.insert("project_ids".into(), serde_json::to_value(value)?);
    }
    if let Some(value) = request.tags.as_ref() {
        changes.insert("tags".into(), serde_json::to_value(value)?);
    }
    if let Some(value) = request.definition_of_done.as_ref() {
        changes.insert("definition_of_done".into(), serde_json::to_value(value)?);
    }
    if let Some(value) = request.links.as_ref() {
        changes.insert("links".into(), serde_json::to_value(value)?);
    }
    Ok(serde_json::Value::Object(changes))
}

fn rebalance_priority_band(
    conn: &Connection,
    priority: PlanningTaskPriority,
    moved_task_id: &str,
) -> Result<()> {
    let ids = {
        let mut statement = conn.prepare(
            "SELECT id
             FROM planning_tasks
             WHERE priority = ?1 AND status NOT IN ('done', 'archived')
             ORDER BY rank,
                      CASE WHEN id = ?2 THEN 1 ELSE 0 END,
                      task_number",
        )?;
        let rows = statement
            .query_map(params![priority.as_str(), moved_task_id], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (index, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE planning_tasks SET rank = ?2 WHERE id = ?1",
            params![id, (index as i64 + 1) * 1024],
        )?;
    }
    Ok(())
}

fn replace_projects(conn: &Connection, task_id: &str, project_ids: &[String]) -> Result<()> {
    conn.execute(
        "DELETE FROM planning_task_projects WHERE task_id = ?1",
        [task_id],
    )?;
    for project_id in project_ids {
        conn.execute(
            "INSERT INTO planning_task_projects (task_id, project_id) VALUES (?1, ?2)",
            params![task_id, project_id],
        )
        .with_context(|| format!("Unknown project: {project_id}"))?;
    }
    Ok(())
}

fn replace_tags(conn: &Connection, task_id: &str, tags: &[String]) -> Result<()> {
    conn.execute(
        "DELETE FROM planning_task_tags WHERE task_id = ?1",
        [task_id],
    )?;
    for tag in tags {
        conn.execute(
            "INSERT INTO planning_task_tags (task_id, tag) VALUES (?1, ?2)",
            params![task_id, tag],
        )?;
    }
    Ok(())
}

fn replace_dod(conn: &Connection, task_id: &str, items: &[CreatePlanningDodItem]) -> Result<()> {
    let existing_ids = load_string_list(
        conn,
        "SELECT id FROM planning_task_dod_items
         WHERE task_id = ?1 ORDER BY position, id",
        task_id,
    )?;
    let existing_set = existing_ids
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let mut retained_ids = std::collections::HashSet::new();
    let now = Utc::now().to_rfc3339();
    for (position, item) in items.iter().enumerate() {
        let requested_id = item
            .id
            .as_deref()
            .filter(|id| existing_set.contains(id) && !retained_ids.contains(*id));
        let fallback_id = existing_ids
            .get(position)
            .map(String::as_str)
            .filter(|id| !retained_ids.contains(*id));
        if let Some(existing_id) = requested_id.or(fallback_id) {
            retained_ids.insert(existing_id.to_string());
            conn.execute(
                "UPDATE planning_task_dod_items
                 SET sentence = ?3, completed = ?4, position = ?5, updated_at = ?6
                 WHERE id = ?1 AND task_id = ?2",
                params![
                    existing_id,
                    task_id,
                    item.sentence.trim(),
                    item.completed,
                    position as i64,
                    now,
                ],
            )?;
        } else {
            conn.execute(
                "INSERT INTO planning_task_dod_items
                 (id, task_id, sentence, completed, position, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    Uuid::new_v4().to_string(),
                    task_id,
                    item.sentence.trim(),
                    item.completed,
                    position as i64,
                    now,
                ],
            )?;
        }
    }
    for obsolete_id in existing_ids
        .iter()
        .filter(|id| !retained_ids.contains(id.as_str()))
    {
        conn.execute(
            "DELETE FROM planning_task_dod_items WHERE id = ?1 AND task_id = ?2",
            params![obsolete_id, task_id],
        )?;
    }
    Ok(())
}

fn replace_links(conn: &Connection, task_id: &str, links: &[CreatePlanningTaskLink]) -> Result<()> {
    conn.execute(
        "DELETE FROM planning_task_links WHERE task_id = ?1",
        [task_id],
    )?;
    for (position, link) in links.iter().enumerate() {
        conn.execute(
            "INSERT INTO planning_task_links (id, task_id, label, url, position)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                Uuid::new_v4().to_string(),
                task_id,
                link.label.trim(),
                link.url.trim(),
                position as i64,
            ],
        )?;
    }
    Ok(())
}

fn load_string_list(conn: &Connection, sql: &str, id: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map([id], |row| row.get(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn get_summary(conn: &Connection, task_id: &str) -> Result<Option<PlanningTaskSummary>> {
    let row = conn
        .query_row(
            "SELECT t.id, t.task_number, t.parent_id, p.task_number, p.title,
                    t.title, t.status, t.priority, t.rank, t.created_at, t.updated_at,
                    COUNT(c.id),
                    COALESCE(SUM(CASE WHEN c.status = 'done' THEN 1 ELSE 0 END), 0),
                    (SELECT COUNT(*)
                     FROM planning_task_blockers b
                     JOIN planning_tasks blocker ON blocker.id = b.blocker_task_id
                     WHERE b.task_id = t.id
                       AND blocker.status NOT IN ('done', 'archived'))
             FROM planning_tasks t
             LEFT JOIN planning_tasks p ON p.id = t.parent_id
             LEFT JOIN planning_tasks c ON c.parent_id = t.id
             WHERE t.id = ?1
             GROUP BY t.id",
            [task_id],
            |row| {
                let status: String = row.get(6)?;
                let priority: String = row.get(7)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    status,
                    priority,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, i64>(13)?,
                ))
            },
        )
        .optional()?;

    let Some((
        id,
        task_number,
        parent_id,
        parent_task_number,
        parent_title,
        title,
        status,
        priority,
        rank,
        created_at,
        updated_at,
        total_subtasks,
        completed_subtasks,
        blocker_count,
    )) = row
    else {
        return Ok(None);
    };

    Ok(Some(PlanningTaskSummary {
        project_ids: load_string_list(
            conn,
            "SELECT project_id
             FROM planning_task_projects
             WHERE task_id = ?1
             UNION
             SELECT discussions.project_id
             FROM planning_task_discussions
             JOIN discussions
               ON discussions.id = planning_task_discussions.discussion_id
             WHERE planning_task_discussions.task_id = ?1
               AND discussions.project_id IS NOT NULL
             ORDER BY project_id",
            &id,
        )?,
        discussion_ids: load_string_list(
            conn,
            "SELECT discussion_id FROM planning_task_discussions WHERE task_id = ?1 ORDER BY discussion_id",
            &id,
        )?,
        tags: load_string_list(
            conn,
            "SELECT tag FROM planning_task_tags WHERE task_id = ?1 ORDER BY tag COLLATE NOCASE",
            &id,
        )?,
        id,
        reference: format!("KT-{task_number}"),
        parent_id,
        parent_reference: parent_task_number.map(|number| format!("KT-{number}")),
        parent_title,
        title,
        status: PlanningTaskStatus::from_str(&status)?,
        priority: PlanningTaskPriority::from_str(&priority)?,
        rank,
        completed_subtasks: completed_subtasks as u32,
        total_subtasks: total_subtasks as u32,
        blocker_count: blocker_count as u32,
        created_at: parse_dt(created_at),
        updated_at: parse_dt(updated_at),
    }))
}

pub fn create_task(
    conn: &Connection,
    request: &CreatePlanningTaskRequest,
) -> Result<PlanningTaskDetail> {
    validate_title(request.title.trim())?;
    validate_actor(&request.actor)?;
    validate_dod(&request.definition_of_done)?;
    validate_links(&request.links)?;
    let idempotency_key = normalized_idempotency_key(request)?;
    let discussion_id = normalized_create_discussion_id(request)?;
    if let Some(discussion_id) = discussion_id {
        let discussion_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM discussions WHERE id = ?1)",
            [discussion_id],
            |row| row.get(0),
        )?;
        if !discussion_exists {
            bail!("Discussion not found");
        }
    }
    let tags = normalize_tags(&request.tags)?;
    let parent_id = request
        .parent_id
        .as_deref()
        .map(|reference| resolve_task_id(conn, reference))
        .transpose()?;
    if let Some(parent_id) = parent_id.as_deref() {
        ensure_parent_is_valid(conn, None, parent_id)?;
    }
    let idempotency_fingerprint = idempotency_key
        .map(|_| create_request_fingerprint(request, parent_id.as_deref(), &tags, discussion_id))
        .transpose()?;

    // A direct create may race with the retry of the same MCP call. Acquiring
    // the writer lock before the lookup makes the unique-key decision atomic
    // even when two backend connections target the same database.
    let transaction = if idempotency_key.is_some() {
        NestableTx::begin_immediate(conn)?
    } else {
        NestableTx::begin(conn)?
    };
    if let (Some(key), Some(fingerprint)) = (idempotency_key, idempotency_fingerprint.as_deref()) {
        let existing = transaction
            .query_row(
                "SELECT id, idempotency_fingerprint
                 FROM planning_tasks
                 WHERE idempotency_key = ?1",
                [key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((existing_id, existing_fingerprint)) = existing {
            if existing_fingerprint != fingerprint {
                bail!("Planning task idempotency key was reused with different content");
            }
            transaction.commit()?;
            return get_task(conn, &existing_id)?.context("Idempotent planning task disappeared");
        }
    }

    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let task_number: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(task_number), 0) + 1 FROM planning_tasks",
        [],
        |row| row.get(0),
    )?;
    let rank: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(rank), 0) + 1024 FROM planning_tasks WHERE priority = ?1",
        [request.priority.as_str()],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO planning_tasks
         (id, task_number, parent_id, title, description, status, priority, rank,
          idempotency_key, idempotency_fingerprint, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            id,
            task_number,
            parent_id,
            request.title.trim(),
            request.description,
            request.status.as_str(),
            request.priority.as_str(),
            rank,
            idempotency_key,
            idempotency_fingerprint,
            now,
        ],
    )?;
    replace_projects(&transaction, &id, &request.project_ids)?;
    replace_tags(&transaction, &id, &tags)?;
    replace_dod(&transaction, &id, &request.definition_of_done)?;
    replace_links(&transaction, &id, &request.links)?;
    insert_event(
        &transaction,
        &id,
        "created",
        &request.actor,
        serde_json::json!({
            "status": request.status,
            "priority": request.priority,
        }),
    )?;
    if let Some(discussion_id) = discussion_id {
        let position: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1
             FROM planning_task_discussions
             WHERE discussion_id = ?1 AND placement = 'active'",
            [discussion_id],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO planning_task_discussions
             (task_id, discussion_id, placement, is_primary, position, created_at)
             VALUES (?1, ?2, 'active', 0, ?3, ?4)",
            params![id, discussion_id, position, now],
        )?;
        insert_event(
            &transaction,
            &id,
            "discussion_linked",
            &request.actor,
            serde_json::json!({
                "discussion_id": discussion_id,
                "placement": PlanningPlacement::Active,
                "is_primary": false,
                "position": position,
            }),
        )?;
    }
    transaction.commit()?;
    get_task(conn, &id)?.context("Created planning task disappeared")
}

pub fn list_tasks(
    conn: &Connection,
    query: &PlanningTaskListQuery,
) -> Result<PlanningTaskListResponse> {
    let mut sql = String::from(
        "SELECT DISTINCT t.id
         FROM planning_tasks t
         WHERE 1 = 1",
    );
    let mut values: Vec<Value> = Vec::new();

    if let Some(search) = query
        .search
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        sql.push_str(
            " AND (t.title LIKE ? OR t.description LIKE ? OR ('KT-' || t.task_number) LIKE ?)",
        );
        let needle = format!("%{}%", search.trim());
        values.extend([
            Value::Text(needle.clone()),
            Value::Text(needle.clone()),
            Value::Text(needle),
        ]);
    }
    if let Some(status) = query.status {
        sql.push_str(" AND t.status = ?");
        values.push(Value::Text(status.as_str().to_string()));
    } else {
        sql.push_str(" AND t.status <> 'archived'");
    }
    if let Some(priority) = query.priority {
        sql.push_str(" AND t.priority = ?");
        values.push(Value::Text(priority.as_str().to_string()));
    }
    if let Some(project_id) = query.project_id.as_ref() {
        sql.push_str(
            " AND (
                EXISTS (
                    SELECT 1 FROM planning_task_projects p
                    WHERE p.task_id = t.id AND p.project_id = ?
                )
                OR EXISTS (
                    SELECT 1
                    FROM planning_task_discussions td
                    JOIN discussions d ON d.id = td.discussion_id
                    WHERE td.task_id = t.id AND d.project_id = ?
                )
            )",
        );
        values.push(Value::Text(project_id.clone()));
        values.push(Value::Text(project_id.clone()));
    }
    if let Some(discussion_id) = query.discussion_id.as_ref() {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM planning_task_discussions d WHERE d.task_id = t.id AND d.discussion_id = ?)",
        );
        values.push(Value::Text(discussion_id.clone()));
    }
    if let Some(tag) = query
        .tag
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM planning_task_tags tt WHERE tt.task_id = t.id AND tt.tag = ? COLLATE NOCASE)",
        );
        values.push(Value::Text(tag.trim().to_string()));
    }
    if let Some(with_discussion) = query.with_discussion {
        sql.push_str(if with_discussion {
            " AND EXISTS (SELECT 1 FROM planning_task_discussions d WHERE d.task_id = t.id)"
        } else {
            " AND NOT EXISTS (SELECT 1 FROM planning_task_discussions d WHERE d.task_id = t.id)"
        });
    }

    let offset = query.cursor.unwrap_or(0).max(0);
    let limit = query.limit.clamp(1, 100);
    sql.push_str(
        " ORDER BY CASE t.priority
             WHEN 'critical' THEN 0 WHEN 'high' THEN 1
             WHEN 'normal' THEN 2 ELSE 3 END,
           t.rank, t.task_number
         LIMIT ? OFFSET ?",
    );
    values.push(Value::Integer((limit + 1) as i64));
    values.push(Value::Integer(offset));

    let mut statement = conn.prepare(&sql)?;
    let ids = statement
        .query_map(params_from_iter(values), |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = ids.len() > limit as usize;
    let mut items = Vec::with_capacity(ids.len().min(limit as usize));
    for id in ids.into_iter().take(limit as usize) {
        if let Some(summary) = get_summary(conn, &id)? {
            items.push(summary);
        }
    }
    Ok(PlanningTaskListResponse {
        next_cursor: has_more.then_some(offset + limit as i64),
        items,
    })
}

pub fn get_task(conn: &Connection, task_id: &str) -> Result<Option<PlanningTaskDetail>> {
    let task_id = match resolve_task_id(conn, task_id) {
        Ok(id) => id,
        Err(error) if error.to_string().contains("not found") => return Ok(None),
        Err(error) => return Err(error),
    };
    let task_id = task_id.as_str();
    let Some(summary) = get_summary(conn, task_id)? else {
        return Ok(None);
    };
    let (description, blocked_reason) = conn.query_row(
        "SELECT description, blocked_reason FROM planning_tasks WHERE id = ?1",
        [task_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    )?;

    let definition_of_done = {
        let mut statement = conn.prepare(
            "SELECT id, sentence, completed, position
             FROM planning_task_dod_items WHERE task_id = ?1 ORDER BY position",
        )?;
        let items = statement
            .query_map([task_id], |row| {
                Ok(PlanningDodItem {
                    id: row.get(0)?,
                    sentence: row.get(1)?,
                    completed: row.get(2)?,
                    position: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        items
    };
    let links = {
        let mut statement = conn.prepare(
            "SELECT id, label, url, position
             FROM planning_task_links WHERE task_id = ?1 ORDER BY position",
        )?;
        let items = statement
            .query_map([task_id], |row| {
                Ok(PlanningTaskLink {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    url: row.get(2)?,
                    position: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        items
    };
    let blocker_ids = load_string_list(
        conn,
        "SELECT blocker_task_id FROM planning_task_blockers WHERE task_id = ?1 ORDER BY created_at",
        task_id,
    )?;
    let blocking_ids = load_string_list(
        conn,
        "SELECT task_id FROM planning_task_blockers WHERE blocker_task_id = ?1 ORDER BY created_at",
        task_id,
    )?;
    let mut blockers = Vec::new();
    for id in blocker_ids {
        if let Some(item) = get_summary(conn, &id)? {
            blockers.push(item);
        }
    }
    let mut blocking = Vec::new();
    for id in blocking_ids {
        if let Some(item) = get_summary(conn, &id)? {
            blocking.push(item);
        }
    }
    let subtask_ids = load_string_list(
        conn,
        "SELECT id FROM planning_tasks WHERE parent_id = ?1
         ORDER BY CASE priority
             WHEN 'critical' THEN 0 WHEN 'high' THEN 1
             WHEN 'normal' THEN 2 ELSE 3 END,
           rank, task_number",
        task_id,
    )?;
    let mut subtasks = Vec::new();
    for id in subtask_ids {
        if let Some(item) = get_summary(conn, &id)? {
            subtasks.push(item);
        }
    }
    let events = {
        let mut statement = conn.prepare(
            "SELECT id, action, actor_kind, actor_id, changes_json, source_message_id, created_at
             FROM planning_task_events WHERE task_id = ?1 ORDER BY created_at DESC, rowid DESC",
        )?;
        let items = statement
            .query_map([task_id], |row| {
                let actor_kind: String = row.get(2)?;
                let changes: String = row.get(4)?;
                Ok(PlanningTaskEvent {
                    id: row.get(0)?,
                    action: row.get(1)?,
                    actor_kind: if actor_kind == "agent" {
                        PlanningActorKind::Agent
                    } else {
                        PlanningActorKind::Human
                    },
                    actor_id: row.get(3)?,
                    changes: serde_json::from_str(&changes).unwrap_or(serde_json::Value::Null),
                    source_message_id: row.get(5)?,
                    created_at: parse_dt(row.get(6)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        items
    };
    let workspaces = {
        let mut statement = conn.prepare(
            "SELECT dw.id, dw.disc_id, dw.branch, dw.state, dw.ownership,
                    ds.agent_type
               FROM discussion_workspaces dw
               LEFT JOIN discussion_sessions ds ON ds.id = dw.session_pk
              WHERE dw.task_id = ?1
              ORDER BY dw.updated_at DESC",
        )?;
        let rows = statement
            .query_map([task_id], |row| {
                Ok(PlanningWorkspaceSummary {
                    id: row.get(0)?,
                    disc_id: row.get(1)?,
                    branch: row.get(2)?,
                    state: row.get(3)?,
                    ownership: row.get(4)?,
                    session_agent_type: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };

    Ok(Some(PlanningTaskDetail {
        summary,
        subtasks,
        description,
        blocked_reason,
        definition_of_done,
        links,
        blockers,
        blocking,
        workspaces,
        events,
    }))
}

pub fn update_task(
    conn: &Connection,
    task_reference: &str,
    request: &UpdatePlanningTaskRequest,
) -> Result<PlanningTaskDetail> {
    let task_id = resolve_task_id(conn, task_reference)?;
    let task_id = task_id.as_str();
    validate_actor(&request.actor)?;
    if let Some(title) = request.title.as_deref() {
        validate_title(title.trim())?;
    }
    let resolved_parent_id = match request.parent_id.as_ref() {
        Some(Some(reference)) => Some(Some(resolve_task_id(conn, reference)?)),
        Some(None) => Some(None),
        None => None,
    };
    if let Some(parent_id) = resolved_parent_id
        .as_ref()
        .and_then(|value| value.as_deref())
    {
        ensure_parent_is_valid(conn, Some(task_id), parent_id)?;
    }
    if let Some(items) = request.definition_of_done.as_ref() {
        validate_dod(items)?;
    }
    if let Some(links) = request.links.as_ref() {
        validate_links(links)?;
    }
    let tags = request
        .tags
        .as_ref()
        .map(|tags| normalize_tags(tags))
        .transpose()?;

    let transaction = NestableTx::begin(conn)?;
    let existing = get_task(&transaction, task_id)?.context("Planning task not found")?;
    let should_rebalance = request.rank.is_some()
        || request
            .priority
            .is_some_and(|value| value != existing.summary.priority);
    let title = request
        .title
        .as_deref()
        .unwrap_or(&existing.summary.title)
        .trim();
    let description = request
        .description
        .as_deref()
        .unwrap_or(&existing.description);
    let status = request.status.unwrap_or(existing.summary.status);
    let priority = request.priority.unwrap_or(existing.summary.priority);
    let parent_id = resolved_parent_id.unwrap_or(existing.summary.parent_id.clone());
    let blocked_reason = request
        .blocked_reason
        .clone()
        .unwrap_or(existing.blocked_reason.clone());
    let rank = request.rank.unwrap_or(existing.summary.rank);
    transaction.execute(
        "UPDATE planning_tasks
         SET parent_id = ?2, title = ?3, description = ?4, status = ?5,
             priority = ?6, rank = ?7, blocked_reason = ?8, updated_at = ?9
         WHERE id = ?1",
        params![
            task_id,
            parent_id,
            title,
            description,
            status.as_str(),
            priority.as_str(),
            rank,
            blocked_reason,
            Utc::now().to_rfc3339(),
        ],
    )?;
    if let Some(project_ids) = request.project_ids.as_ref() {
        replace_projects(&transaction, task_id, project_ids)?;
    }
    if let Some(tags) = tags.as_ref() {
        replace_tags(&transaction, task_id, tags)?;
    }
    if let Some(items) = request.definition_of_done.as_ref() {
        replace_dod(&transaction, task_id, items)?;
    }
    if let Some(links) = request.links.as_ref() {
        replace_links(&transaction, task_id, links)?;
    }
    if should_rebalance {
        rebalance_priority_band(&transaction, priority, task_id)?;
    }
    insert_event(
        &transaction,
        task_id,
        "updated",
        &request.actor,
        compact_update_changes(request)?,
    )?;
    transaction.commit()?;
    get_task(conn, task_id)?.context("Updated planning task disappeared")
}

pub fn update_dod_item(
    conn: &Connection,
    task_reference: &str,
    dod_id: &str,
    request: &UpdatePlanningDodItemRequest,
) -> Result<PlanningTaskDetail> {
    let task_id = resolve_task_id(conn, task_reference)?;
    validate_actor(&request.actor)?;
    let transaction = NestableTx::begin(conn)?;
    let changed = transaction.execute(
        "UPDATE planning_task_dod_items
         SET completed = ?3, updated_at = ?4
         WHERE id = ?1 AND task_id = ?2",
        params![dod_id, task_id, request.completed, Utc::now().to_rfc3339(),],
    )?;
    if changed == 0 {
        bail!("Definition of Done item not found");
    }
    insert_event(
        &transaction,
        &task_id,
        "dod_updated",
        &request.actor,
        serde_json::json!({
            "dod_id": dod_id,
            "completed": request.completed,
        }),
    )?;
    transaction.commit()?;
    get_task(conn, &task_id)?.context("Updated planning task disappeared")
}

pub fn link_discussion(
    conn: &Connection,
    task_reference: &str,
    request: &LinkPlanningDiscussionRequest,
) -> Result<DiscussionPlan> {
    let task_id = resolve_task_id(conn, task_reference)?;
    let task_id = task_id.as_str();
    validate_actor(&request.actor)?;
    let discussion_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM discussions WHERE id = ?1)",
        [&request.discussion_id],
        |row| row.get(0),
    )?;
    if !discussion_exists {
        bail!("Discussion not found");
    }

    let transaction = NestableTx::begin(conn)?;
    let is_primary = request.is_primary && request.placement == PlanningPlacement::Active;
    if is_primary {
        transaction.execute(
            "UPDATE planning_task_discussions
             SET is_primary = 0 WHERE discussion_id = ?1",
            [&request.discussion_id],
        )?;
    }
    let position = match request.position {
        Some(position) => position,
        None => transaction.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1
             FROM planning_task_discussions
             WHERE discussion_id = ?1 AND placement = ?2",
            params![request.discussion_id, request.placement.as_str()],
            |row| row.get(0),
        )?,
    };
    transaction.execute(
        "INSERT INTO planning_task_discussions
         (task_id, discussion_id, placement, is_primary, position, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(task_id, discussion_id) DO UPDATE SET
             placement = excluded.placement,
             is_primary = excluded.is_primary,
             position = excluded.position",
        params![
            task_id,
            request.discussion_id,
            request.placement.as_str(),
            is_primary,
            position,
            Utc::now().to_rfc3339(),
        ],
    )?;
    insert_event(
        &transaction,
        task_id,
        "discussion_linked",
        &request.actor,
        serde_json::json!({
            "discussion_id": request.discussion_id,
            "placement": request.placement,
            "is_primary": is_primary,
            "position": position,
        }),
    )?;
    transaction.commit()?;
    get_discussion_plan(conn, &request.discussion_id)
}

pub fn add_blocker(
    conn: &Connection,
    task_reference: &str,
    request: &AddPlanningBlockerRequest,
) -> Result<PlanningTaskDetail> {
    let task_id = resolve_task_id(conn, task_reference)?;
    let blocker_task_id = resolve_task_id(conn, &request.blocker_task_id)?;
    let task_id = task_id.as_str();
    validate_actor(&request.actor)?;
    if task_id == blocker_task_id {
        bail!("A task cannot block itself");
    }
    let creates_cycle: bool = conn.query_row(
        "WITH RECURSIVE chain(id) AS (
             SELECT blocker_task_id FROM planning_task_blockers WHERE task_id = ?1
             UNION
             SELECT b.blocker_task_id
             FROM planning_task_blockers b
             JOIN chain ON b.task_id = chain.id
         )
         SELECT EXISTS(SELECT 1 FROM chain WHERE id = ?2)",
        params![blocker_task_id, task_id],
        |row| row.get(0),
    )?;
    if creates_cycle {
        bail!("Task dependency cycle detected");
    }

    let transaction = NestableTx::begin(conn)?;
    transaction.execute(
        "INSERT OR IGNORE INTO planning_task_blockers
         (task_id, blocker_task_id, created_at) VALUES (?1, ?2, ?3)",
        params![task_id, blocker_task_id, Utc::now().to_rfc3339(),],
    )?;
    insert_event(
        &transaction,
        task_id,
        "blocker_added",
        &request.actor,
        serde_json::json!({"blocker_task_id": blocker_task_id}),
    )?;
    transaction.commit()?;
    get_task(conn, task_id)?.context("Planning task disappeared")
}

/// Remove one explicit dependency edge without changing either task's status.
///
/// The operation is intentionally idempotent: MCP clients can safely retry a
/// request after losing the HTTP response. We only append an audit event when
/// a relation was actually removed, so retries do not invent history.
pub fn remove_blocker(
    conn: &Connection,
    task_reference: &str,
    blocker_reference: &str,
    actor: &PlanningActor,
) -> Result<PlanningTaskDetail> {
    let task_id = resolve_task_id(conn, task_reference)?;
    let blocker_task_id = resolve_task_id(conn, blocker_reference)?;
    validate_actor(actor)?;

    let transaction = NestableTx::begin(conn)?;
    let removed = transaction.execute(
        "DELETE FROM planning_task_blockers
         WHERE task_id = ?1 AND blocker_task_id = ?2",
        params![task_id, blocker_task_id],
    )?;
    if removed > 0 {
        insert_event(
            &transaction,
            &task_id,
            "blocker_removed",
            actor,
            serde_json::json!({"blocker_task_id": blocker_task_id}),
        )?;
    }
    transaction.commit()?;
    get_task(conn, &task_id)?.context("Planning task disappeared")
}

pub fn task_changes(
    conn: &Connection,
    discussion_id: &str,
    since: Option<&str>,
) -> Result<Vec<PlanningTaskChange>> {
    let discussion_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM discussions WHERE id = ?1)",
        [discussion_id],
        |row| row.get(0),
    )?;
    if !discussion_exists {
        bail!("Discussion not found");
    }
    let since = since.unwrap_or("1970-01-01T00:00:00Z");
    DateTime::parse_from_rfc3339(since).context("since must be an RFC 3339 timestamp")?;
    let mut statement = conn.prepare(
        "SELECT e.id, e.task_id, t.task_number, t.title, e.action, e.actor_kind,
                e.actor_id, e.changes_json, e.source_message_id, e.created_at
         FROM planning_task_events e
         JOIN planning_tasks t ON t.id = e.task_id
         JOIN planning_task_discussions d ON d.task_id = e.task_id
         WHERE d.discussion_id = ?1
           AND unixepoch(e.created_at, 'subsec') > unixepoch(?2, 'subsec')
         ORDER BY e.created_at, e.rowid
         LIMIT 200",
    )?;
    let rows = statement.query_map(params![discussion_id, since], |row| {
        let task_number: i64 = row.get(2)?;
        let actor_kind: String = row.get(5)?;
        let changes: String = row.get(7)?;
        Ok(PlanningTaskChange {
            task_id: row.get(1)?,
            task_reference: format!("KT-{task_number}"),
            task_title: row.get(3)?,
            event: PlanningTaskEvent {
                id: row.get(0)?,
                action: row.get(4)?,
                actor_kind: if actor_kind == "agent" {
                    PlanningActorKind::Agent
                } else {
                    PlanningActorKind::Human
                },
                actor_id: row.get(6)?,
                changes: serde_json::from_str(&changes).unwrap_or(serde_json::Value::Null),
                source_message_id: row.get(8)?,
                created_at: parse_dt(row.get(9)?),
            },
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn change_count_since_last_agent(conn: &Connection, discussion_id: &str) -> Result<u32> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM planning_task_events e
         JOIN planning_task_discussions d ON d.task_id = e.task_id
         WHERE d.discussion_id = ?1
           AND unixepoch(e.created_at, 'subsec') > COALESCE(
             (SELECT MAX(unixepoch(timestamp, 'subsec')) FROM messages
              WHERE discussion_id = ?1 AND role = 'Agent'),
             unixepoch('1970-01-01T00:00:00Z', 'subsec')
           )",
        [discussion_id],
        |row| row.get(0),
    )?;
    Ok(count as u32)
}

pub fn get_discussion_plan(conn: &Connection, discussion_id: &str) -> Result<DiscussionPlan> {
    let discussion_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM discussions WHERE id = ?1)",
        [discussion_id],
        |row| row.get(0),
    )?;
    if !discussion_exists {
        bail!("Discussion not found");
    }

    let mut statement = conn.prepare(
        "SELECT relation.task_id, relation.placement, relation.is_primary, relation.position
         FROM planning_task_discussions relation
         JOIN planning_tasks task ON task.id = relation.task_id
         WHERE relation.discussion_id = ?1
           AND task.status <> 'archived'
         ORDER BY relation.placement = 'later', relation.position, relation.created_at,
                  task.task_number",
    )?;
    let rows = statement
        .query_map([discussion_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // KT-30 — batch the projection instead of per-relation `get_summary`
    // (which fired 4 queries per task → ~4N+1). Both loads run a FIXED number
    // of query families regardless of the number of relations.
    let summaries = load_summaries_for_discussion(conn, discussion_id)?;
    let mut dependencies = load_active_dependencies_for_discussion(conn, discussion_id)?;

    let mut primary_objective = None;
    let mut active = Vec::new();
    let mut later = Vec::new();
    let mut stats = PlanningPlanStats::default();
    for (task_id, placement, is_primary, position) in rows {
        let Some(task) = summaries.get(&task_id).cloned() else {
            continue;
        };
        if is_primary {
            primary_objective = Some(task.clone());
        }
        let placement = PlanningPlacement::from_str(&placement)?;
        let active_blockers = dependencies.remove(&task_id).unwrap_or_default();
        let is_active = placement == PlanningPlacement::Active;
        // Active-only bucket, strict precedence done > blocked > in_progress >
        // ideas > ready. `blocked` = an explicit Blocked status OR any active
        // blocker (`blocker_count` already counts only non done/archived
        // blockers). A nascent `Idea` has its own bucket so `ready` stays equal
        // to the actionable count. `ready` is the residual = Todo, no active
        // blocker. See PlanningPlanStats.
        let has_active_blocker = task.blocker_count > 0;
        if is_active {
            match task.status {
                PlanningTaskStatus::Done => stats.done += 1,
                PlanningTaskStatus::Blocked => stats.blocked += 1,
                _ if has_active_blocker => stats.blocked += 1,
                PlanningTaskStatus::InProgress => stats.in_progress += 1,
                PlanningTaskStatus::Idea => stats.ideas += 1,
                _ => stats.ready += 1,
            }
        } else {
            stats.later += 1;
        }
        // Actionable: strictly a Todo with no active blocker, and only on the
        // Active side. A Later relation, an Idea, or anything started/blocked is
        // never actionable.
        let actionable =
            is_active && task.status == PlanningTaskStatus::Todo && !has_active_blocker;
        let relation = PlanningDiscussionRelation {
            placement,
            is_primary,
            position,
            task,
            active_blockers,
            actionable,
        };
        if is_active {
            active.push(relation);
        } else {
            later.push(relation);
        }
    }
    Ok(DiscussionPlan {
        discussion_id: discussion_id.to_string(),
        primary_objective,
        // Retained aliases: total = the five Active buckets, completed = done.
        total_active: active.len() as u32,
        completed_active: stats.done,
        active,
        later,
        stats,
    })
}

/// KT-30 — load every linked task's [`PlanningTaskSummary`] for a discussion in
/// a FIXED number of queries (main aggregate + projects + discussions + tags),
/// grouping the multi-valued columns in Rust. Filters through
/// `planning_task_discussions` by `discussion_id` so there is no per-task query
/// and no unbounded `IN (…)` parameter list.
fn load_summaries_for_discussion(
    conn: &Connection,
    discussion_id: &str,
) -> Result<std::collections::HashMap<String, PlanningTaskSummary>> {
    let projects = load_grouped_by_task(
        conn,
        "SELECT ptd.task_id, ptp.project_id
           FROM planning_task_discussions ptd
           JOIN planning_task_projects ptp ON ptp.task_id = ptd.task_id
          WHERE ptd.discussion_id = ?1
         UNION
         SELECT ptd.task_id, d.project_id
           FROM planning_task_discussions ptd
           JOIN planning_task_discussions link ON link.task_id = ptd.task_id
           JOIN discussions d ON d.id = link.discussion_id
          WHERE ptd.discussion_id = ?1 AND d.project_id IS NOT NULL
          ORDER BY 1, 2",
        discussion_id,
    )?;
    let discussions = load_grouped_by_task(
        conn,
        "SELECT ptd.task_id, link.discussion_id
           FROM planning_task_discussions ptd
           JOIN planning_task_discussions link ON link.task_id = ptd.task_id
          WHERE ptd.discussion_id = ?1
          ORDER BY 1, 2",
        discussion_id,
    )?;
    let tags = load_grouped_by_task(
        conn,
        "SELECT ptd.task_id, tg.tag
           FROM planning_task_discussions ptd
           JOIN planning_task_tags tg ON tg.task_id = ptd.task_id
          WHERE ptd.discussion_id = ?1
          ORDER BY ptd.task_id, tg.tag COLLATE NOCASE",
        discussion_id,
    )?;

    let mut stmt = conn.prepare(
        "SELECT t.id, t.task_number, t.parent_id, p.task_number, p.title,
                t.title, t.status, t.priority, t.rank, t.created_at, t.updated_at,
                COUNT(c.id),
                COALESCE(SUM(CASE WHEN c.status = 'done' THEN 1 ELSE 0 END), 0),
                (SELECT COUNT(*)
                 FROM planning_task_blockers b
                 JOIN planning_tasks blocker ON blocker.id = b.blocker_task_id
                 WHERE b.task_id = t.id
                   AND blocker.status NOT IN ('done', 'archived'))
         FROM planning_task_discussions ptd
         JOIN planning_tasks t ON t.id = ptd.task_id
         LEFT JOIN planning_tasks p ON p.id = t.parent_id
         LEFT JOIN planning_tasks c ON c.parent_id = t.id
         WHERE ptd.discussion_id = ?1
         GROUP BY t.id",
    )?;
    // Collect raw rows first, then parse the enums OUTSIDE the rusqlite closure
    // so a corrupt status/priority propagates an error (`?`) instead of silently
    // defaulting — matching the old per-task `get_summary`.
    type RawSummary = (
        String,         // id
        i64,            // task_number
        Option<String>, // parent_id
        Option<i64>,    // parent_task_number
        Option<String>, // parent_title
        String,         // title
        String,         // status
        String,         // priority
        i64,            // rank
        String,         // created_at
        String,         // updated_at
        i64,            // total_subtasks
        i64,            // completed_subtasks
        i64,            // blocker_count
    );
    let raw_rows: Vec<RawSummary> = stmt
        .query_map([discussion_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
                row.get(12)?,
                row.get(13)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut summaries = std::collections::HashMap::new();
    for raw in raw_rows {
        let (
            id,
            task_number,
            parent_id,
            parent_task_number,
            parent_title,
            title,
            status,
            priority,
            rank,
            created_at,
            updated_at,
            total_subtasks,
            completed_subtasks,
            blocker_count,
        ) = raw;
        let summary = PlanningTaskSummary {
            project_ids: projects.get(&id).cloned().unwrap_or_default(),
            discussion_ids: discussions.get(&id).cloned().unwrap_or_default(),
            tags: tags.get(&id).cloned().unwrap_or_default(),
            reference: format!("KT-{task_number}"),
            parent_id,
            parent_reference: parent_task_number.map(|n| format!("KT-{n}")),
            parent_title,
            title,
            status: PlanningTaskStatus::from_str(&status)?,
            priority: PlanningTaskPriority::from_str(&priority)?,
            rank,
            completed_subtasks: completed_subtasks as u32,
            total_subtasks: total_subtasks as u32,
            blocker_count: blocker_count as u32,
            created_at: parse_dt(created_at),
            updated_at: parse_dt(updated_at),
            id,
        };
        summaries.insert(summary.id.clone(), summary);
    }
    Ok(summaries)
}

/// KT-30 — load every linked task's ACTIVE blockers (status not done/archived)
/// as minimal, non-recursive [`PlanningDependencySummary`] rows, grouped by the
/// blocked task. Three fixed queries: the blocker cores, then the blockers'
/// projects and discussions (grouped in Rust) — never one query per task.
fn load_active_dependencies_for_discussion(
    conn: &Connection,
    discussion_id: &str,
) -> Result<std::collections::HashMap<String, Vec<PlanningDependencySummary>>> {
    // (blocked_task_id, blocker_id, blocker_number, title, status), plan order.
    let mut stmt = conn.prepare(
        "SELECT ptd.task_id, blocker.id, blocker.task_number, blocker.title, blocker.status
           FROM planning_task_discussions ptd
           JOIN planning_task_blockers b ON b.task_id = ptd.task_id
           JOIN planning_tasks blocker ON blocker.id = b.blocker_task_id
          WHERE ptd.discussion_id = ?1
            AND blocker.status NOT IN ('done', 'archived')
          ORDER BY ptd.task_id, blocker.task_number",
    )?;
    let rows = stmt
        .query_map([discussion_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // The blockers' own project/discussion ids, batched over the distinct
    // blocker set (one query each; the blocker's own blockers are NOT expanded).
    let blocker_ids: Vec<Value> = {
        let mut seen = std::collections::HashSet::new();
        rows.iter()
            .filter(|(_, blocker_id, ..)| seen.insert(blocker_id.clone()))
            .map(|(_, blocker_id, ..)| Value::Text(blocker_id.clone()))
            .collect()
    };
    let placeholders = vec!["?"; blocker_ids.len()].join(",");
    // The UNION references the id set in BOTH arms, so bind it twice, in order.
    let blocker_ids_twice: Vec<Value> = blocker_ids
        .iter()
        .cloned()
        .chain(blocker_ids.iter().cloned())
        .collect();
    let blocker_projects = load_grouped_by_task(
        conn,
        &format!(
            "SELECT task_id, project_id
               FROM planning_task_projects WHERE task_id IN ({placeholders})
             UNION
             SELECT link.task_id, d.project_id
               FROM planning_task_discussions link
               JOIN discussions d ON d.id = link.discussion_id
              WHERE link.task_id IN ({placeholders}) AND d.project_id IS NOT NULL
              ORDER BY 1, 2"
        ),
        BindTasks::Ids(&blocker_ids_twice),
    )?;
    let blocker_discussions = load_grouped_by_task(
        conn,
        &format!(
            "SELECT task_id, discussion_id FROM planning_task_discussions
              WHERE task_id IN ({placeholders}) ORDER BY task_id, discussion_id"
        ),
        BindTasks::Ids(&blocker_ids),
    )?;

    let mut by_task: std::collections::HashMap<String, Vec<PlanningDependencySummary>> =
        std::collections::HashMap::new();
    for (blocked_id, blocker_id, blocker_number, title, status) in rows {
        by_task
            .entry(blocked_id)
            .or_default()
            .push(PlanningDependencySummary {
                project_ids: blocker_projects
                    .get(&blocker_id)
                    .cloned()
                    .unwrap_or_default(),
                discussion_ids: blocker_discussions
                    .get(&blocker_id)
                    .cloned()
                    .unwrap_or_default(),
                reference: format!("KT-{blocker_number}"),
                title,
                status: PlanningTaskStatus::from_str(&status)?,
                id: blocker_id,
            });
    }
    Ok(by_task)
}

/// Binding strategy for [`load_grouped_by_task`]: either a single discussion id
/// (the query filters through `planning_task_discussions`) or an explicit list
/// of task ids bound to an `IN (…)` placeholder set the caller already built.
enum BindTasks<'a> {
    Discussion(&'a str),
    Ids(&'a [Value]),
}

impl<'a> From<&'a str> for BindTasks<'a> {
    fn from(discussion_id: &'a str) -> Self {
        BindTasks::Discussion(discussion_id)
    }
}

/// Run a `(task_id, value)` query and group the values per task, preserving the
/// query's row order within each task. One statement, whatever the row count.
fn load_grouped_by_task<'a>(
    conn: &Connection,
    sql: &str,
    bind: impl Into<BindTasks<'a>>,
) -> Result<std::collections::HashMap<String, Vec<String>>> {
    let mut stmt = conn.prepare(sql)?;
    let map_row = |row: &rusqlite::Row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?));
    let pairs: Vec<(String, String)> = match bind.into() {
        BindTasks::Discussion(id) => stmt
            .query_map([id], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        BindTasks::Ids(ids) => stmt
            .query_map(params_from_iter(ids.iter()), map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?,
    };
    let mut grouped: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for (task_id, value) in pairs {
        grouped.entry(task_id).or_default().push(value);
    }
    Ok(grouped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrations::run(&connection).unwrap();
        connection
    }

    fn request(title: &str) -> CreatePlanningTaskRequest {
        CreatePlanningTaskRequest {
            title: title.into(),
            discussion_id: None,
            idempotency_key: None,
            description: String::new(),
            status: PlanningTaskStatus::Todo,
            priority: PlanningTaskPriority::Normal,
            parent_id: None,
            project_ids: Vec::new(),
            tags: Vec::new(),
            definition_of_done: Vec::new(),
            links: Vec::new(),
            actor: PlanningActor::default(),
        }
    }

    fn update_request() -> UpdatePlanningTaskRequest {
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
            actor: PlanningActor::default(),
        }
    }

    #[test]
    fn creates_stable_references_and_compact_summaries() {
        let connection = connection();
        let mut first_request = request("First");
        first_request.tags = vec!["platform".into()];
        let first = create_task(&connection, &first_request).unwrap();
        let second = create_task(&connection, &request("Second")).unwrap();
        assert_eq!(first.summary.reference, "KT-1");
        assert_eq!(second.summary.reference, "KT-2");

        let listed = list_tasks(&connection, &PlanningTaskListQuery::default()).unwrap();
        assert_eq!(listed.items.len(), 2);
        assert_eq!(listed.items[0].title, "First");
        assert!(listed.items[0].discussion_ids.is_empty());

        let tagged = list_tasks(
            &connection,
            &PlanningTaskListQuery {
                tag: Some("PLATFORM".into()),
                ..PlanningTaskListQuery::default()
            },
        )
        .unwrap();
        assert_eq!(tagged.items.len(), 1);
        assert_eq!(tagged.items[0].id, first.summary.id);
    }

    #[test]
    fn create_fingerprint_stays_compatible_when_discussion_target_is_absent() {
        let create = request("Legacy retry");
        let fingerprint = create_request_fingerprint(&create, None, &[], None).unwrap();
        let legacy_canonical = serde_json::to_vec(&serde_json::json!({
            "title": "Legacy retry",
            "description": "",
            "status": PlanningTaskStatus::Todo,
            "priority": PlanningTaskPriority::Normal,
            "parent_id": serde_json::Value::Null,
            "project_ids": Vec::<String>::new(),
            "tags": Vec::<String>::new(),
            "definition_of_done": Vec::<serde_json::Value>::new(),
            "links": Vec::<serde_json::Value>::new(),
        }))
        .unwrap();
        let legacy_digest: String = Sha256::digest(legacy_canonical)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();

        assert_eq!(fingerprint, format!("v1:{legacy_digest}"));
        assert_ne!(
            fingerprint,
            create_request_fingerprint(&create, None, &[], Some("disc-target")).unwrap()
        );
    }

    #[test]
    fn create_task_can_atomically_link_an_existing_discussion() {
        let connection = connection();
        seed_disc(&connection, "disc-target");
        let mut create = request("Plan in target");
        create.discussion_id = Some(" disc-target ".into());

        let task = create_task(&connection, &create).unwrap();

        assert_eq!(task.summary.discussion_ids, vec!["disc-target"]);
        let plan = get_discussion_plan(&connection, "disc-target").unwrap();
        assert_eq!(plan.active.len(), 1);
        assert_eq!(plan.active[0].task.id, task.summary.id);
        assert!(task
            .events
            .iter()
            .any(|event| event.action == "discussion_linked"));
    }

    #[test]
    fn create_task_rejects_an_unknown_discussion_without_leaving_an_orphan() {
        let connection = connection();
        let mut create = request("Must not survive");
        create.discussion_id = Some("missing-discussion".into());

        let error = create_task(&connection, &create).unwrap_err();

        assert!(error.to_string().contains("Discussion not found"));
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM planning_tasks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn task_detail_exposes_linked_discussion_workspaces() {
        let connection = connection();
        let now = Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p-workspace', 'Workspace', '/repo', ?1, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO discussions (
                     id, project_id, title, created_at, updated_at
                 ) VALUES ('d-workspace', 'p-workspace', 'Room', ?1, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO discussion_sessions (
                     id, disc_id, agent_type, session_id, role, status, joined_at
                 ) VALUES (
                     41, 'd-workspace', 'Codex', 'sess-workspace',
                     'peer', 'active', ?1
                 )",
                [&now],
            )
            .unwrap();
        let mut create = request("Workspace-aware task");
        create.project_ids = vec!["p-workspace".into()];
        let task = create_task(&connection, &create).unwrap();
        crate::db::discussion_workspaces::upsert_external(
            &connection,
            "d-workspace",
            41,
            Some(&task.summary.id),
            "p-workspace",
            "/repo-worktree",
            "/repo-worktree",
            "feature/workspace",
            "abc123",
        )
        .unwrap();

        let detail = get_task(&connection, &task.summary.reference)
            .unwrap()
            .unwrap();
        assert_eq!(detail.workspaces.len(), 1);
        assert_eq!(detail.workspaces[0].disc_id, "d-workspace");
        assert_eq!(
            detail.workspaces[0].session_agent_type.as_deref(),
            Some("Codex")
        );
    }

    #[test]
    fn rejects_parent_and_blocker_cycles() {
        let connection = connection();
        let first = create_task(&connection, &request("First")).unwrap();
        let mut child_request = request("Child");
        child_request.parent_id = Some(first.summary.reference.clone());
        let child = create_task(&connection, &child_request).unwrap();
        let parent = get_task(&connection, &first.summary.id).unwrap().unwrap();
        assert_eq!(parent.subtasks.len(), 1);
        assert_eq!(parent.subtasks[0].id, child.summary.id);
        assert_eq!(
            child.summary.parent_reference.as_deref(),
            Some(first.summary.reference.as_str())
        );

        let update = UpdatePlanningTaskRequest {
            title: None,
            description: None,
            status: None,
            priority: None,
            parent_id: Some(Some(child.summary.id.clone())),
            blocked_reason: None,
            rank: None,
            project_ids: None,
            tags: None,
            definition_of_done: None,
            links: None,
            actor: PlanningActor::default(),
        };
        assert!(update_task(&connection, &first.summary.id, &update)
            .unwrap_err()
            .to_string()
            .contains("cycle"));

        add_blocker(
            &connection,
            &child.summary.id,
            &AddPlanningBlockerRequest {
                blocker_task_id: first.summary.id.clone(),
                actor: PlanningActor::default(),
            },
        )
        .unwrap();
        assert!(add_blocker(
            &connection,
            &first.summary.id,
            &AddPlanningBlockerRequest {
                blocker_task_id: child.summary.id,
                actor: PlanningActor::default(),
            },
        )
        .unwrap_err()
        .to_string()
        .contains("cycle"));
    }

    #[test]
    fn blocker_removal_accepts_references_and_is_idempotent() {
        let connection = connection();
        let task = create_task(&connection, &request("Blocked task")).unwrap();
        let blocker = create_task(&connection, &request("Dependency")).unwrap();
        let actor = PlanningActor {
            kind: PlanningActorKind::Agent,
            id: Some("Codex".into()),
            source_message_id: None,
        };
        add_blocker(
            &connection,
            &task.summary.reference,
            &AddPlanningBlockerRequest {
                blocker_task_id: blocker.summary.reference.clone(),
                actor: actor.clone(),
            },
        )
        .unwrap();

        let removed = remove_blocker(
            &connection,
            &task.summary.reference,
            &blocker.summary.reference,
            &actor,
        )
        .unwrap();
        assert!(removed.blockers.is_empty());
        assert_eq!(removed.summary.blocker_count, 0);
        assert_eq!(removed.events[0].action, "blocker_removed");
        assert_eq!(removed.events[0].actor_id.as_deref(), Some("Codex"));

        let retried =
            remove_blocker(&connection, &task.summary.id, &blocker.summary.id, &actor).unwrap();
        assert_eq!(retried.events.len(), removed.events.len());
        assert_eq!(retried.events[0].action, "blocker_removed");
    }

    #[test]
    fn discussion_has_only_one_primary_objective() {
        let connection = connection();
        connection
            .execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('disc-1', 'Plan', ?1, ?1)",
                [Utc::now().to_rfc3339()],
            )
            .unwrap();
        let first = create_task(&connection, &request("First")).unwrap();
        let second = create_task(&connection, &request("Second")).unwrap();
        for task_id in [&first.summary.id, &second.summary.id] {
            link_discussion(
                &connection,
                task_id,
                &LinkPlanningDiscussionRequest {
                    discussion_id: "disc-1".into(),
                    placement: PlanningPlacement::Active,
                    is_primary: true,
                    position: None,
                    actor: PlanningActor::default(),
                },
            )
            .unwrap();
        }
        let plan = get_discussion_plan(&connection, "disc-1").unwrap();
        assert_eq!(plan.primary_objective.unwrap().title, "Second");
        assert_eq!(
            plan.active
                .iter()
                .filter(|relation| relation.is_primary)
                .count(),
            1
        );
        assert_eq!(
            change_count_since_last_agent(&connection, "disc-1").unwrap(),
            4
        );

        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order)
                 VALUES ('agent-plan', 'disc-1', 'Agent', 'ack', ?1, 1)",
                [(Utc::now() + chrono::Duration::seconds(1)).to_rfc3339()],
            )
            .unwrap();
        assert_eq!(
            change_count_since_last_agent(&connection, "disc-1").unwrap(),
            0
        );
    }

    #[test]
    fn discussion_plan_excludes_archived_tasks() {
        let connection = connection();
        connection
            .execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('disc-archived-plan', 'Plan', ?1, ?1)",
                [Utc::now().to_rfc3339()],
            )
            .unwrap();
        let task = create_task(&connection, &request("Archived")).unwrap();
        link_discussion(
            &connection,
            &task.summary.id,
            &LinkPlanningDiscussionRequest {
                discussion_id: "disc-archived-plan".into(),
                placement: PlanningPlacement::Later,
                is_primary: false,
                position: None,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();
        update_task(
            &connection,
            &task.summary.id,
            &UpdatePlanningTaskRequest {
                title: None,
                description: None,
                status: Some(PlanningTaskStatus::Archived),
                priority: None,
                parent_id: None,
                blocked_reason: None,
                rank: None,
                project_ids: None,
                tags: None,
                definition_of_done: None,
                links: None,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();

        let plan = get_discussion_plan(&connection, "disc-archived-plan").unwrap();
        assert!(plan.active.is_empty());
        assert!(plan.later.is_empty());
    }

    #[test]
    fn discussion_tasks_inherit_the_discussion_project_for_filters_and_badges() {
        let connection = connection();
        let now = Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('project-1', 'Project', '/tmp/project-1', ?1, ?1)",
                [&now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO discussions
                 (id, project_id, title, created_at, updated_at)
                 VALUES ('disc-project', 'project-1', 'Plan', ?1, ?1)",
                [&now],
            )
            .unwrap();
        let task = create_task(&connection, &request("Inherited project")).unwrap();
        link_discussion(
            &connection,
            &task.summary.id,
            &LinkPlanningDiscussionRequest {
                discussion_id: "disc-project".into(),
                placement: PlanningPlacement::Active,
                is_primary: false,
                position: None,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();

        let detail = get_task(&connection, &task.summary.id).unwrap().unwrap();
        assert_eq!(detail.summary.project_ids, vec!["project-1".to_string()]);

        let project_tasks = list_tasks(
            &connection,
            &PlanningTaskListQuery {
                project_id: Some("project-1".into()),
                ..PlanningTaskListQuery::default()
            },
        )
        .unwrap();
        assert_eq!(project_tasks.items.len(), 1);
        assert_eq!(project_tasks.items[0].id, task.summary.id);
        assert_eq!(
            project_tasks.items[0].project_ids,
            vec!["project-1".to_string()]
        );
    }

    #[test]
    fn update_event_records_only_changed_fields() {
        let connection = connection();
        let task = create_task(&connection, &request("First")).unwrap();
        let update = UpdatePlanningTaskRequest {
            title: None,
            description: None,
            status: Some(PlanningTaskStatus::InProgress),
            priority: None,
            parent_id: None,
            blocked_reason: None,
            rank: None,
            project_ids: None,
            tags: None,
            definition_of_done: None,
            links: None,
            actor: PlanningActor::default(),
        };
        let updated = update_task(&connection, &task.summary.id, &update).unwrap();
        let event = updated
            .events
            .iter()
            .find(|event| event.action == "updated")
            .expect("an updated event is recorded");
        let object = event.changes.as_object().expect("changes is a JSON object");
        assert_eq!(object.len(), 1, "only the changed field is recorded");
        assert_eq!(
            object.get("status").and_then(|value| value.as_str()),
            Some("in_progress")
        );
        assert!(
            !object.contains_key("actor"),
            "actor must not leak into changes"
        );
        assert!(
            !object.contains_key("title"),
            "untouched fields are omitted"
        );
    }

    #[test]
    fn task_change_timestamps_compare_as_instants() {
        let connection = connection();
        connection
            .execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('disc-time', 'Plan', ?1, ?1)",
                [Utc::now().to_rfc3339()],
            )
            .unwrap();
        let task = create_task(&connection, &request("Timestamp")).unwrap();
        link_discussion(
            &connection,
            &task.summary.id,
            &LinkPlanningDiscussionRequest {
                discussion_id: "disc-time".into(),
                placement: PlanningPlacement::Active,
                is_primary: false,
                position: None,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();
        connection
            .execute(
                "UPDATE planning_task_events
                 SET created_at = '2026-07-25T10:00:00.805927+00:00'
                 WHERE task_id = ?1",
                [&task.summary.id],
            )
            .unwrap();

        assert_eq!(
            task_changes(&connection, "disc-time", Some("2026-07-25T10:00:00.8Z"),)
                .unwrap()
                .len(),
            2,
            "offset and fractional precision differences must not hide newer events",
        );
        assert!(
            task_changes(&connection, "disc-time", Some("2026-07-25T10:00:00.806Z"),)
                .unwrap()
                .is_empty()
        );

        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order)
                 VALUES ('agent-time', 'disc-time', 'Agent', 'ack',
                         '2026-07-25T10:00:00.8Z', 1)",
                [],
            )
            .unwrap();
        assert_eq!(
            change_count_since_last_agent(&connection, "disc-time").unwrap(),
            2,
        );
        connection
            .execute(
                "UPDATE messages SET timestamp = '2026-07-25T10:00:00.806Z'
                 WHERE id = 'agent-time'",
                [],
            )
            .unwrap();
        assert_eq!(
            change_count_since_last_agent(&connection, "disc-time").unwrap(),
            0,
        );
    }

    #[test]
    fn later_tasks_cannot_remain_primary() {
        let connection = connection();
        connection
            .execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('disc-later', 'Plan', ?1, ?1)",
                [Utc::now().to_rfc3339()],
            )
            .unwrap();
        let primary = create_task(&connection, &request("Primary")).unwrap();
        let later = create_task(&connection, &request("Later")).unwrap();
        link_discussion(
            &connection,
            &primary.summary.id,
            &LinkPlanningDiscussionRequest {
                discussion_id: "disc-later".into(),
                placement: PlanningPlacement::Active,
                is_primary: true,
                position: None,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();
        let plan = link_discussion(
            &connection,
            &later.summary.id,
            &LinkPlanningDiscussionRequest {
                discussion_id: "disc-later".into(),
                placement: PlanningPlacement::Later,
                is_primary: true,
                position: None,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();
        assert_eq!(
            plan.primary_objective.as_ref().map(|task| task.id.as_str()),
            Some(primary.summary.id.as_str()),
            "an invalid later-primary request must not erase the existing objective",
        );
        assert_eq!(plan.later.len(), 1);
        assert!(!plan.later[0].is_primary);
    }

    #[test]
    fn archived_blockers_are_satisfied_but_remain_visible() {
        let connection = connection();
        let blocker = create_task(&connection, &request("Blocker")).unwrap();
        let blocked = create_task(&connection, &request("Blocked")).unwrap();
        let linked = add_blocker(
            &connection,
            &blocked.summary.id,
            &AddPlanningBlockerRequest {
                blocker_task_id: blocker.summary.id.clone(),
                actor: PlanningActor::default(),
            },
        )
        .unwrap();
        assert_eq!(linked.summary.blocker_count, 1);

        let mut archive = update_request();
        archive.status = Some(PlanningTaskStatus::Archived);
        update_task(&connection, &blocker.summary.id, &archive).unwrap();
        let refreshed = get_task(&connection, &blocked.summary.id).unwrap().unwrap();
        assert_eq!(refreshed.summary.blocker_count, 0);
        assert_eq!(refreshed.blockers.len(), 1);
        assert_eq!(refreshed.blockers[0].status, PlanningTaskStatus::Archived);
    }

    #[test]
    fn rank_updates_rebalance_the_priority_band() {
        let connection = connection();
        let _first = create_task(&connection, &request("First")).unwrap();
        let second = create_task(&connection, &request("Second")).unwrap();
        let third = create_task(&connection, &request("Third")).unwrap();
        connection
            .execute(
                "UPDATE planning_tasks SET rank = 1025 WHERE id = ?1",
                [&second.summary.id],
            )
            .unwrap();

        let mut move_third = update_request();
        move_third.rank = Some(1024);
        update_task(&connection, &third.summary.id, &move_third).unwrap();
        let ordered = list_tasks(&connection, &PlanningTaskListQuery::default())
            .unwrap()
            .items;
        assert_eq!(
            ordered
                .iter()
                .map(|task| task.title.as_str())
                .collect::<Vec<_>>(),
            vec!["First", "Third", "Second"],
        );
        assert_eq!(
            ordered.iter().map(|task| task.rank).collect::<Vec<_>>(),
            vec![1024, 2048, 3072],
        );
    }

    #[test]
    fn dod_ids_are_stable_and_individual_updates_do_not_clobber_siblings() {
        let connection = connection();
        let mut create = request("DoD");
        create.definition_of_done = vec![
            CreatePlanningDodItem {
                id: None,
                sentence: "First".into(),
                completed: false,
            },
            CreatePlanningDodItem {
                id: None,
                sentence: "Second".into(),
                completed: false,
            },
        ];
        let task = create_task(&connection, &create).unwrap();
        let first_id = task.definition_of_done[0].id.clone();
        let second_id = task.definition_of_done[1].id.clone();

        let mut replace = update_request();
        replace.definition_of_done = Some(vec![
            CreatePlanningDodItem {
                id: Some(second_id.clone()),
                sentence: "Second".into(),
                completed: false,
            },
            CreatePlanningDodItem {
                id: Some(first_id.clone()),
                sentence: "First renamed".into(),
                completed: false,
            },
        ]);
        let replaced = update_task(&connection, &task.summary.id, &replace).unwrap();
        assert_eq!(replaced.definition_of_done[0].id, second_id);
        assert_eq!(replaced.definition_of_done[1].id, first_id);

        update_dod_item(
            &connection,
            &task.summary.id,
            &first_id,
            &UpdatePlanningDodItemRequest {
                completed: true,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();
        let updated = update_dod_item(
            &connection,
            &task.summary.id,
            &second_id,
            &UpdatePlanningDodItemRequest {
                completed: true,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();
        assert!(updated.definition_of_done.iter().all(|item| item.completed));
    }

    // ── KT-30A — compact bounded plan projection ────────────────────────────
    fn seed_disc(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at) VALUES (?1, 'Plan', ?2, ?2)",
            rusqlite::params![id, Utc::now().to_rfc3339()],
        )
        .unwrap();
    }

    fn linked_task(
        conn: &Connection,
        disc: &str,
        title: &str,
        status: PlanningTaskStatus,
        placement: PlanningPlacement,
    ) -> String {
        let mut req = request(title);
        req.status = status;
        let task = create_task(conn, &req).unwrap();
        link_discussion(
            conn,
            &task.summary.id,
            &LinkPlanningDiscussionRequest {
                discussion_id: disc.into(),
                placement,
                is_primary: false,
                position: None,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();
        task.summary.id
    }

    #[test]
    fn discussion_plan_stats_partition_active_by_strict_precedence() {
        let conn = connection();
        seed_disc(&conn, "d");
        linked_task(
            &conn,
            "d",
            "ready",
            PlanningTaskStatus::Todo,
            PlanningPlacement::Active,
        );
        linked_task(
            &conn,
            "d",
            "wip",
            PlanningTaskStatus::InProgress,
            PlanningPlacement::Active,
        );
        linked_task(
            &conn,
            "d",
            "done",
            PlanningTaskStatus::Done,
            PlanningPlacement::Active,
        );
        linked_task(
            &conn,
            "d",
            "blk",
            PlanningTaskStatus::Blocked,
            PlanningPlacement::Active,
        );
        // An Idea counts as `ready` (not-yet-started residual) but is NOT actionable.
        linked_task(
            &conn,
            "d",
            "idea",
            PlanningTaskStatus::Idea,
            PlanningPlacement::Active,
        );
        // in_progress WITH an active blocker → precedence classifies it `blocked`.
        let wip_blocked = linked_task(
            &conn,
            "d",
            "wip-blocked",
            PlanningTaskStatus::InProgress,
            PlanningPlacement::Active,
        );
        let blocker = create_task(&conn, &request("blocker")).unwrap();
        add_blocker(
            &conn,
            &wip_blocked,
            &AddPlanningBlockerRequest {
                blocker_task_id: blocker.summary.id,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();
        linked_task(
            &conn,
            "d",
            "later",
            PlanningTaskStatus::Todo,
            PlanningPlacement::Later,
        );

        let plan = get_discussion_plan(&conn, "d").unwrap();
        assert_eq!(plan.stats.ready, 1, "only the pure Todo-no-blocker");
        assert_eq!(
            plan.stats.ideas, 1,
            "the Idea has its own bucket, never counted as ready"
        );
        assert_eq!(plan.stats.in_progress, 1);
        assert_eq!(plan.stats.done, 1);
        assert_eq!(
            plan.stats.blocked, 2,
            "explicit Blocked + in_progress-with-active-blocker"
        );
        assert_eq!(plan.stats.later, 1);
        // Retained aliases stay consistent with the partition.
        assert_eq!(
            plan.total_active,
            plan.stats.ready
                + plan.stats.blocked
                + plan.stats.in_progress
                + plan.stats.ideas
                + plan.stats.done
        );
        assert_eq!(plan.total_active, 6);
        assert_eq!(plan.completed_active, plan.stats.done);
        // Only the pure ready Todo is actionable (not idea, not later, not blocked).
        let actionable: Vec<&str> = plan
            .active
            .iter()
            .filter(|r| r.actionable)
            .map(|r| r.task.title.as_str())
            .collect();
        assert_eq!(actionable, vec!["ready"]);
        // The `ready` bucket is exactly the actionable count — never inflated.
        assert_eq!(plan.stats.ready, actionable.len() as u32);
        assert!(
            plan.later.iter().all(|r| !r.actionable),
            "Later is never actionable"
        );
    }

    #[test]
    fn discussion_plan_surfaces_active_blockers_and_excludes_satisfied() {
        let conn = connection();
        seed_disc(&conn, "d");
        let blocked = linked_task(
            &conn,
            "d",
            "blocked",
            PlanningTaskStatus::Todo,
            PlanningPlacement::Active,
        );
        let active_blocker = create_task(&conn, &request("active-blocker")).unwrap();
        let done_blocker = {
            let mut r = request("done-blocker");
            r.status = PlanningTaskStatus::Done;
            create_task(&conn, &r).unwrap()
        };
        let archived_blocker = {
            let mut r = request("archived-blocker");
            r.status = PlanningTaskStatus::Archived;
            create_task(&conn, &r).unwrap()
        };
        for b in [&active_blocker, &done_blocker, &archived_blocker] {
            add_blocker(
                &conn,
                &blocked,
                &AddPlanningBlockerRequest {
                    blocker_task_id: b.summary.id.clone(),
                    actor: PlanningActor::default(),
                },
            )
            .unwrap();
        }

        let plan = get_discussion_plan(&conn, "d").unwrap();
        let rel = plan
            .active
            .iter()
            .find(|r| r.task.title == "blocked")
            .unwrap();
        assert_eq!(
            rel.active_blockers.len(),
            1,
            "both done AND archived blockers are satisfied → only the active one surfaces"
        );
        assert_eq!(rel.active_blockers[0].title, "active-blocker");
        assert_eq!(rel.active_blockers[0].status, PlanningTaskStatus::Todo);
        assert!(!rel.actionable, "an active blocker makes it non-actionable");
        assert_eq!(plan.stats.blocked, 1);
    }

    #[test]
    fn discussion_plan_preserves_plan_position_order_not_priority() {
        let conn = connection();
        seed_disc(&conn, "d");
        let low = {
            let mut r = request("A-low");
            r.priority = PlanningTaskPriority::Low;
            create_task(&conn, &r).unwrap()
        };
        let high = {
            let mut r = request("B-high");
            r.priority = PlanningTaskPriority::Critical;
            create_task(&conn, &r).unwrap()
        };
        // Link low at position 0, high at position 1 — plan order must ignore the
        // higher global priority of B.
        for (task, pos) in [(&low, 0_i64), (&high, 1_i64)] {
            link_discussion(
                &conn,
                &task.summary.id,
                &LinkPlanningDiscussionRequest {
                    discussion_id: "d".into(),
                    placement: PlanningPlacement::Active,
                    is_primary: false,
                    position: Some(pos),
                    actor: PlanningActor::default(),
                },
            )
            .unwrap();
        }
        let plan = get_discussion_plan(&conn, "d").unwrap();
        let order: Vec<&str> = plan.active.iter().map(|r| r.task.title.as_str()).collect();
        assert_eq!(
            order,
            vec!["A-low", "B-high"],
            "plan position wins over global priority"
        );
    }

    #[test]
    fn dependency_summary_carries_external_navigation_ids() {
        let conn = connection();
        seed_disc(&conn, "d");
        seed_disc(&conn, "other");
        let blocked = linked_task(
            &conn,
            "d",
            "blocked",
            PlanningTaskStatus::Todo,
            PlanningPlacement::Active,
        );
        // A blocker with its own project AND linked to ANOTHER discussion → both
        // its project_ids and discussion_ids surface so the UI can offer real
        // links to it (a ghost row), never an invented one.
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('proj-ext', 'Ext', '/tmp/ext', ?1, ?1)",
            [Utc::now().to_rfc3339()],
        )
        .unwrap();
        let blocker = {
            let mut r = request("ext-blocker");
            r.project_ids = vec!["proj-ext".into()];
            create_task(&conn, &r).unwrap()
        };
        link_discussion(
            &conn,
            &blocker.summary.id,
            &LinkPlanningDiscussionRequest {
                discussion_id: "other".into(),
                placement: PlanningPlacement::Active,
                is_primary: false,
                position: None,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();
        add_blocker(
            &conn,
            &blocked,
            &AddPlanningBlockerRequest {
                blocker_task_id: blocker.summary.id,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();

        let plan = get_discussion_plan(&conn, "d").unwrap();
        let rel = plan
            .active
            .iter()
            .find(|r| r.task.title == "blocked")
            .unwrap();
        assert_eq!(rel.active_blockers.len(), 1);
        assert!(
            rel.active_blockers[0]
                .discussion_ids
                .contains(&"other".to_string()),
            "external blocker's discussion is navigable from the dependency summary"
        );
        assert!(
            rel.active_blockers[0]
                .project_ids
                .contains(&"proj-ext".to_string()),
            "external blocker's project is navigable too"
        );
    }

    #[test]
    fn discussion_plan_propagates_a_corrupt_task_enum_instead_of_defaulting() {
        // KT-30A.2 — a corrupt status/priority in the DB must surface as an
        // explicit error, never be silently read as Idea/Normal by the batched
        // loaders (the guard against `unwrap_or_default` creeping back).
        let conn = connection();
        seed_disc(&conn, "d");
        let id = linked_task(
            &conn,
            "d",
            "t",
            PlanningTaskStatus::Todo,
            PlanningPlacement::Active,
        );
        conn.execute(
            "UPDATE planning_tasks SET status = 'bogus-status' WHERE id = ?1",
            [&id],
        )
        .unwrap();
        let err = get_discussion_plan(&conn, "d").unwrap_err();
        assert!(
            err.to_string().contains("Unknown planning task status"),
            "a corrupt status must propagate, got: {err}"
        );
    }

    static STMT_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    fn count_stmt(event: rusqlite::trace::TraceEvent<'_>) {
        if matches!(event, rusqlite::trace::TraceEvent::Stmt(..)) {
            STMT_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[test]
    fn get_discussion_plan_statement_count_is_invariant_to_plan_size() {
        use std::sync::atomic::Ordering;
        let conn = connection();

        // Small plan (2 relations) WITH an active blocker so every query family fires.
        seed_disc(&conn, "small");
        let s1 = linked_task(
            &conn,
            "small",
            "s1",
            PlanningTaskStatus::Todo,
            PlanningPlacement::Active,
        );
        linked_task(
            &conn,
            "small",
            "s2",
            PlanningTaskStatus::InProgress,
            PlanningPlacement::Active,
        );
        let sb = create_task(&conn, &request("s-blocker")).unwrap();
        add_blocker(
            &conn,
            &s1,
            &AddPlanningBlockerRequest {
                blocker_task_id: sb.summary.id,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();

        // Large plan (1 000 relations) also WITH a blocker.
        seed_disc(&conn, "large");
        let first = linked_task(
            &conn,
            "large",
            "L0",
            PlanningTaskStatus::Todo,
            PlanningPlacement::Active,
        );
        for i in 1..1000 {
            linked_task(
                &conn,
                "large",
                &format!("L{i}"),
                PlanningTaskStatus::Todo,
                PlanningPlacement::Active,
            );
        }
        let lb = create_task(&conn, &request("l-blocker")).unwrap();
        add_blocker(
            &conn,
            &first,
            &AddPlanningBlockerRequest {
                blocker_task_id: lb.summary.id,
                actor: PlanningActor::default(),
            },
        )
        .unwrap();

        conn.trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
            Some(count_stmt),
        );
        STMT_COUNT.store(0, Ordering::Relaxed);
        let small = get_discussion_plan(&conn, "small").unwrap();
        let small_stmts = STMT_COUNT.load(Ordering::Relaxed);
        STMT_COUNT.store(0, Ordering::Relaxed);
        let large = get_discussion_plan(&conn, "large").unwrap();
        let large_stmts = STMT_COUNT.load(Ordering::Relaxed);
        conn.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);

        // Correctness at scale: all 1 000 relations present, plan order preserved,
        // nothing truncated.
        assert_eq!(large.active.len(), 1000);
        assert_eq!(large.active.first().unwrap().task.title, "L0");
        assert_eq!(large.active.last().unwrap().task.title, "L999");
        assert_eq!(small.active.len(), 2);
        // The projection issues a FIXED number of SQL statements regardless of N —
        // the architectural proof there is no N+1, with no fragile timing.
        assert_eq!(
            small_stmts, large_stmts,
            "statement count must not grow with plan size (N+1 guard): small={small_stmts} large={large_stmts}"
        );
    }
}
