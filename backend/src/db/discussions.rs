use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::models::{*, ModelTier};

// ─── Discussions ────────────────────────────────────────────────────────────

/// Count total discussions (for pagination).
pub fn count_discussions(conn: &Connection) -> Result<u32> {
    let count: u32 = conn.query_row("SELECT COUNT(*) FROM discussions", [], |row| row.get(0))?;
    Ok(count)
}

pub fn list_discussions(conn: &Connection) -> Result<Vec<Discussion>> {
    list_discussions_paginated(conn, None, None)
}

pub fn list_discussions_paginated(conn: &Connection, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Discussion>> {
    let sql = format!(
        "SELECT d.id, d.project_id, d.title, d.agent, d.language, d.participants_json,
                d.created_at, d.updated_at, d.archived, d.skill_ids_json,
                d.message_count,
                d.profile_ids_json, d.directive_ids_json,
                d.workspace_mode, d.workspace_path, d.worktree_branch,
                d.summary_cache, d.summary_up_to_msg_idx, d.model_tier,
                d.pin_first_message,
                d.shared_id, d.shared_with_json
         FROM discussions d ORDER BY d.updated_at DESC{}",
        match (limit, offset) {
            (Some(l), Some(o)) => format!(" LIMIT {} OFFSET {}", l, o),
            (Some(l), None) => format!(" LIMIT {}", l),
            _ => String::new(),
        }
    );
    let mut stmt = conn.prepare(&sql)?;

    let discussions: Vec<Discussion> = stmt.query_map([], |row| {
        let agent_str: String = row.get(3)?;
        let participants_str: String = row.get(5)?;
        let skill_ids_str: String = row.get::<_, String>(9).unwrap_or_else(|_| "[]".into());
        let profile_ids_str: String = row.get::<_, String>(11).unwrap_or_else(|_| "[]".into());
        let directive_ids_str: String = row.get::<_, String>(12).unwrap_or_else(|_| "[]".into());

        Ok(Discussion {
            id: row.get(0)?,
            project_id: row.get(1)?,
            title: row.get(2)?,
            agent: parse_agent_type(&agent_str),
            language: row.get(4)?,
            participants: serde_json::from_str(&participants_str).unwrap_or_default(),
            messages: vec![],
            message_count: row.get::<_, u32>(10).unwrap_or(0),
            skill_ids: serde_json::from_str(&skill_ids_str).unwrap_or_default(),
            profile_ids: serde_json::from_str(&profile_ids_str).unwrap_or_default(),
            directive_ids: serde_json::from_str(&directive_ids_str).unwrap_or_default(),
            archived: row.get::<_, i32>(8).unwrap_or(0) != 0,
            workspace_mode: row.get::<_, String>(13).unwrap_or_else(|_| "Direct".into()),
            workspace_path: row.get::<_, Option<String>>(14).unwrap_or(None),
            worktree_branch: row.get::<_, Option<String>>(15).unwrap_or(None),
            tier: parse_model_tier(&row.get::<_, String>(18).unwrap_or_else(|_| "default".into())),
            pin_first_message: row.get::<_, i32>(19).unwrap_or(0) != 0,
            summary_cache: row.get::<_, Option<String>>(16).unwrap_or(None),
            summary_up_to_msg_idx: row.get::<_, Option<u32>>(17).unwrap_or(None),
            shared_id: row.get::<_, Option<String>>(20).unwrap_or(None),
            shared_with: serde_json::from_str(&row.get::<_, String>(21).unwrap_or_else(|_| "[]".into())).unwrap_or_default(),
            created_at: parse_dt(row.get::<_, String>(6)?),
            updated_at: parse_dt(row.get::<_, String>(7)?),
        })
    })?.filter_map(|r| r.ok())
    .collect();

    // Don't load messages for the list view — messages are only loaded
    // for individual discussions via get_discussion(). With 200+ discussions
    // each having 50+ messages, loading all messages here is a performance bomb.
    // message_count is populated via SQL subquery for display purposes.

    Ok(discussions)
}

/// Like list_discussions but also loads all messages (used for export).
pub fn list_discussions_with_messages(conn: &Connection) -> Result<Vec<Discussion>> {
    let mut discussions = list_discussions(conn)?;

    let all_messages = list_all_messages(conn)?;
    for disc in &mut discussions {
        if let Some(msgs) = all_messages.get(&disc.id) {
            disc.messages = msgs.clone();
            disc.message_count = disc.messages.len() as u32;
        }
    }

    Ok(discussions)
}

pub fn get_discussion(conn: &Connection, id: &str) -> Result<Option<Discussion>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, title, agent, language, participants_json,
                created_at, updated_at, archived, skill_ids_json, profile_ids_json, directive_ids_json,
                workspace_mode, workspace_path, worktree_branch,
                summary_cache, summary_up_to_msg_idx, model_tier, pin_first_message,
                shared_id, shared_with_json
         FROM discussions WHERE id = ?1"
    )?;

    let disc = stmt.query_row(params![id], |row| {
        let agent_str: String = row.get(3)?;
        let participants_str: String = row.get(5)?;
        let skill_ids_str: String = row.get::<_, String>(9).unwrap_or_else(|_| "[]".into());
        let profile_ids_str: String = row.get::<_, String>(10).unwrap_or_else(|_| "[]".into());
        let directive_ids_str: String = row.get::<_, String>(11).unwrap_or_else(|_| "[]".into());

        Ok(Discussion {
            id: row.get(0)?,
            project_id: row.get(1)?,
            title: row.get(2)?,
            agent: parse_agent_type(&agent_str),
            language: row.get(4)?,
            participants: serde_json::from_str(&participants_str).unwrap_or_default(),
            messages: vec![],
            message_count: 0,
            skill_ids: serde_json::from_str(&skill_ids_str).unwrap_or_default(),
            profile_ids: serde_json::from_str(&profile_ids_str).unwrap_or_default(),
            directive_ids: serde_json::from_str(&directive_ids_str).unwrap_or_default(),
            archived: row.get::<_, i32>(8).unwrap_or(0) != 0,
            workspace_mode: row.get::<_, String>(12).unwrap_or_else(|_| "Direct".into()),
            workspace_path: row.get::<_, Option<String>>(13).unwrap_or(None),
            worktree_branch: row.get::<_, Option<String>>(14).unwrap_or(None),
            tier: parse_model_tier(&row.get::<_, String>(17).unwrap_or_else(|_| "default".into())),
            pin_first_message: row.get::<_, i32>(18).unwrap_or(0) != 0,
            summary_cache: row.get::<_, Option<String>>(15).unwrap_or(None),
            summary_up_to_msg_idx: row.get::<_, Option<u32>>(16).unwrap_or(None),
            shared_id: row.get::<_, Option<String>>(19).unwrap_or(None),
            shared_with: serde_json::from_str(&row.get::<_, String>(20).unwrap_or_else(|_| "[]".into())).unwrap_or_default(),
            created_at: parse_dt(row.get::<_, String>(6)?),
            updated_at: parse_dt(row.get::<_, String>(7)?),
        })
    }).ok();

    if let Some(mut d) = disc {
        d.messages = list_messages(conn, &d.id)?;
        d.message_count = d.messages.len() as u32;
        Ok(Some(d))
    } else {
        Ok(None)
    }
}

pub fn insert_discussion(conn: &Connection, disc: &Discussion) -> Result<()> {
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, agent, language, participants_json, created_at, updated_at, archived, skill_ids_json, profile_ids_json, directive_ids_json, workspace_mode, workspace_path, worktree_branch, model_tier, pin_first_message, shared_id, shared_with_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        params![
            disc.id,
            disc.project_id,
            disc.title,
            format_agent_type(&disc.agent),
            disc.language,
            serde_json::to_string(&disc.participants)?,
            disc.created_at.to_rfc3339(),
            disc.updated_at.to_rfc3339(),
            disc.archived as i32,
            serde_json::to_string(&disc.skill_ids)?,
            serde_json::to_string(&disc.profile_ids)?,
            serde_json::to_string(&disc.directive_ids)?,
            disc.workspace_mode,
            disc.workspace_path,
            disc.worktree_branch,
            format_model_tier(&disc.tier),
            disc.pin_first_message as i32,
            disc.shared_id,
            serde_json::to_string(&disc.shared_with)?,
        ],
    )?;
    Ok(())
}

pub fn delete_discussion(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM discussions WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

pub fn update_discussion(conn: &Connection, id: &str, title: Option<&str>, archived: Option<bool>, project_id: Option<Option<&str>>) -> Result<bool> {
    update_discussion_fields(conn, id, title, archived, None, None, None, project_id)
}

pub fn update_discussion_skill_ids(conn: &Connection, id: &str, skill_ids: &[String]) -> Result<bool> {
    update_discussion_fields(conn, id, None, None, Some(skill_ids), None, None, None)
}

pub fn update_discussion_profile_ids(conn: &Connection, id: &str, profile_ids: &[String]) -> Result<bool> {
    update_discussion_fields(conn, id, None, None, None, Some(profile_ids), None, None)
}

pub fn update_discussion_tier(conn: &Connection, id: &str, tier: &ModelTier) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE discussions SET model_tier = ?1, updated_at = ?2 WHERE id = ?3",
        params![format_model_tier(tier), Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

pub fn update_discussion_agent(conn: &Connection, id: &str, agent: &AgentType) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE discussions SET agent = ?1, updated_at = ?2 WHERE id = ?3",
        params![format_agent_type(agent), Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

pub fn update_discussion_directive_ids(conn: &Connection, id: &str, directive_ids: &[String]) -> Result<bool> {
    update_discussion_fields(conn, id, None, None, None, None, Some(directive_ids), None)
}

/// Update workspace_path and worktree_branch for a discussion (used after worktree creation).
pub fn update_discussion_workspace(conn: &Connection, id: &str, workspace_path: &str, worktree_branch: &str) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE discussions SET workspace_path = ?1, worktree_branch = ?2, updated_at = ?3 WHERE id = ?4",
        params![workspace_path, worktree_branch, Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

#[allow(clippy::too_many_arguments)]
fn update_discussion_fields(conn: &Connection, id: &str, title: Option<&str>, archived: Option<bool>, skill_ids: Option<&[String]>, profile_ids: Option<&[String]>, directive_ids: Option<&[String]>, project_id: Option<Option<&str>>) -> Result<bool> {
    let mut sets = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(t) = title {
        sets.push("title = ?");
        values.push(Box::new(t.to_string()));
    }
    if let Some(a) = archived {
        sets.push("archived = ?");
        values.push(Box::new(a as i32));
    }
    if let Some(pid) = project_id {
        sets.push("project_id = ?");
        values.push(Box::new(pid.map(|s| s.to_string())));
    }
    if let Some(s) = skill_ids {
        sets.push("skill_ids_json = ?");
        values.push(Box::new(serde_json::to_string(s).unwrap_or_else(|_| "[]".into())));
    }
    if let Some(p) = profile_ids {
        sets.push("profile_ids_json = ?");
        values.push(Box::new(serde_json::to_string(p).unwrap_or_else(|_| "[]".into())));
    }
    if let Some(d) = directive_ids {
        sets.push("directive_ids_json = ?");
        values.push(Box::new(serde_json::to_string(d).unwrap_or_else(|_| "[]".into())));
    }

    if sets.is_empty() {
        return Ok(false);
    }

    sets.push("updated_at = ?");
    values.push(Box::new(Utc::now().to_rfc3339()));

    values.push(Box::new(id.to_string()));

    let sql = format!(
        "UPDATE discussions SET {} WHERE id = ?",
        sets.join(", ")
    );

    let params: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
    let affected = conn.execute(&sql, params.as_slice())?;
    Ok(affected > 0)
}

pub fn update_discussion_timestamp(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE discussions SET updated_at = ?1 WHERE id = ?2",
        params![Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

pub fn update_discussion_participants(conn: &Connection, id: &str, participants: &[AgentType]) -> Result<()> {
    conn.execute(
        "UPDATE discussions SET participants_json = ?1, updated_at = ?2 WHERE id = ?3",
        params![serde_json::to_string(participants)?, Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

// ─── Messages ───────────────────────────────────────────────────────────────

/// Load all messages grouped by discussion_id in a single query (avoids N+1).
fn list_all_messages(conn: &Connection) -> Result<std::collections::HashMap<String, Vec<DiscussionMessage>>> {
    let mut stmt = conn.prepare(
        "SELECT discussion_id, id, role, content, agent_type, timestamp, tokens_used, auth_mode, model_tier, cost_usd
         FROM messages ORDER BY sort_order, timestamp"
    )?;

    let mut map: std::collections::HashMap<String, Vec<DiscussionMessage>> = std::collections::HashMap::new();
    let rows = stmt.query_map([], |row| {
        let disc_id: String = row.get(0)?;
        let role_str: String = row.get(2)?;
        let agent_type_str: Option<String> = row.get(4)?;

        Ok((disc_id, DiscussionMessage {
            id: row.get(1)?,
            role: parse_role(&role_str),
            content: row.get(3)?,
            agent_type: agent_type_str.map(|s| parse_agent_type(&s)),
            timestamp: parse_dt(row.get::<_, String>(5)?),
            tokens_used: row.get::<_, i64>(6).unwrap_or(0) as u64,
            auth_mode: row.get(7)?,
            model_tier: row.get::<_, Option<String>>(8).unwrap_or(None),
            cost_usd: row.get::<_, Option<f64>>(9).unwrap_or(None),
            author_pseudo: None,
            author_avatar_email: None,
        }))
    })?;

    for row in rows.filter_map(|r| r.ok()) {
        map.entry(row.0).or_default().push(row.1);
    }

    Ok(map)
}

pub fn list_messages(conn: &Connection, discussion_id: &str) -> Result<Vec<DiscussionMessage>> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, agent_type, timestamp, tokens_used, auth_mode, model_tier, cost_usd, author_pseudo, author_avatar_email
         FROM messages WHERE discussion_id = ?1
         ORDER BY sort_order, timestamp"
    )?;

    let messages = stmt.query_map(params![discussion_id], |row| {
        let role_str: String = row.get(1)?;
        let agent_type_str: Option<String> = row.get(3)?;

        Ok(DiscussionMessage {
            id: row.get(0)?,
            role: parse_role(&role_str),
            content: row.get(2)?,
            agent_type: agent_type_str.map(|s| parse_agent_type(&s)),
            timestamp: parse_dt(row.get::<_, String>(4)?),
            tokens_used: row.get::<_, i64>(5).unwrap_or(0) as u64,
            auth_mode: row.get(6)?,
            model_tier: row.get::<_, Option<String>>(7).unwrap_or(None),
            cost_usd: row.get::<_, Option<f64>>(8).unwrap_or(None),
            author_pseudo: row.get::<_, Option<String>>(9).unwrap_or(None),
            author_avatar_email: row.get::<_, Option<String>>(10).unwrap_or(None),
        })
    })?.filter_map(|r| r.ok()).collect();

    Ok(messages)
}

pub fn insert_message(conn: &Connection, discussion_id: &str, msg: &DiscussionMessage) -> Result<()> {
    // Get the next sort_order for this discussion
    let next_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM messages WHERE discussion_id = ?1",
        params![discussion_id],
        |row| row.get(0),
    )?;

    conn.execute(
        "INSERT INTO messages (id, discussion_id, role, content, agent_type, timestamp, sort_order, tokens_used, auth_mode, model_tier, cost_usd, author_pseudo, author_avatar_email)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            msg.id,
            discussion_id,
            format_role(&msg.role),
            msg.content,
            msg.agent_type.as_ref().map(format_agent_type),
            msg.timestamp.to_rfc3339(),
            next_order,
            msg.tokens_used as i64,
            msg.auth_mode,
            msg.model_tier,
            msg.cost_usd,
            msg.author_pseudo,
            msg.author_avatar_email,
        ],
    )?;

    conn.execute(
        "UPDATE discussions SET message_count = message_count + 1 WHERE id = ?1",
        params![discussion_id],
    )?;

    update_discussion_timestamp(conn, discussion_id)?;
    Ok(())
}

/// Find a discussion by its shared_id (cross-Kronn replicated ID).
pub fn find_discussion_by_shared_id(conn: &Connection, shared_id: &str) -> Result<Option<String>> {
    let id = conn.query_row(
        "SELECT id FROM discussions WHERE shared_id = ?1",
        params![shared_id],
        |row| row.get::<_, String>(0),
    ).ok();
    Ok(id)
}

/// Update shared_id and shared_with for a discussion.
pub fn update_discussion_sharing(conn: &Connection, discussion_id: &str, shared_id: &str, shared_with: &[String]) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE discussions SET shared_id = ?1, shared_with_json = ?2, updated_at = ?3 WHERE id = ?4",
        params![shared_id, serde_json::to_string(shared_with)?, Utc::now().to_rfc3339(), discussion_id],
    )?;
    Ok(affected > 0)
}

pub fn delete_last_agent_messages(conn: &Connection, discussion_id: &str) -> Result<u64> {
    // Delete trailing non-User messages (Agent + System) from the end
    let affected = conn.execute(
        "DELETE FROM messages WHERE discussion_id = ?1 AND sort_order > (
            SELECT COALESCE(MAX(sort_order), -1) FROM messages
            WHERE discussion_id = ?1 AND role = 'User'
        )",
        params![discussion_id],
    )?;

    // Recount to keep message_count accurate after bulk delete
    conn.execute(
        "UPDATE discussions SET message_count = (
            SELECT COUNT(*) FROM messages WHERE discussion_id = ?1
         ) WHERE id = ?1",
        params![discussion_id],
    )?;

    update_discussion_timestamp(conn, discussion_id)?;
    Ok(affected as u64)
}

pub fn edit_last_user_message(conn: &Connection, discussion_id: &str, content: &str) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE messages SET content = ?1, timestamp = ?2
         WHERE discussion_id = ?3 AND role = 'User'
         AND sort_order = (SELECT MAX(sort_order) FROM messages WHERE discussion_id = ?3 AND role = 'User')",
        params![content, Utc::now().to_rfc3339(), discussion_id],
    )?;

    update_discussion_timestamp(conn, discussion_id)?;
    // Invalidate cached summary since conversation content changed
    let _ = invalidate_summary_cache(conn, discussion_id);
    Ok(affected > 0)
}

/// Save a conversation summary cache for a discussion.
pub fn update_summary_cache(conn: &Connection, discussion_id: &str, summary: &str, up_to_msg_idx: u32) -> Result<()> {
    conn.execute(
        "UPDATE discussions SET summary_cache = ?1, summary_up_to_msg_idx = ?2 WHERE id = ?3",
        params![summary, up_to_msg_idx, discussion_id],
    )?;
    Ok(())
}

/// Invalidate summary cache (e.g., when messages are edited or deleted).
pub fn invalidate_summary_cache(conn: &Connection, discussion_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE discussions SET summary_cache = NULL, summary_up_to_msg_idx = NULL WHERE id = ?1",
        params![discussion_id],
    )?;
    Ok(())
}

pub fn update_message_tokens(conn: &Connection, message_id: &str, tokens_used: u64, auth_mode: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE messages SET tokens_used = ?1, auth_mode = ?2 WHERE id = ?3",
        params![tokens_used as i64, auth_mode, message_id],
    )?;
    Ok(())
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to parse datetime '{}': {}, using now()", s, e);
            Utc::now()
        })
}

fn parse_agent_type(s: &str) -> AgentType {
    match s {
        "ClaudeCode" => AgentType::ClaudeCode,
        "Codex" => AgentType::Codex,
        "Vibe" => AgentType::Vibe,
        "GeminiCli" => AgentType::GeminiCli,
        "Kiro" => AgentType::Kiro,
        _ => AgentType::Custom,
    }
}

fn format_agent_type(a: &AgentType) -> String {
    match a {
        AgentType::ClaudeCode => "ClaudeCode".into(),
        AgentType::Codex => "Codex".into(),
        AgentType::Vibe => "Vibe".into(),
        AgentType::GeminiCli => "GeminiCli".into(),
        AgentType::Kiro => "Kiro".into(),
        AgentType::Custom => "Custom".into(),
    }
}

fn parse_model_tier(s: &str) -> ModelTier {
    match s {
        "economy" => ModelTier::Economy,
        "reasoning" => ModelTier::Reasoning,
        _ => ModelTier::Default,
    }
}

fn format_model_tier(t: &ModelTier) -> &'static str {
    match t {
        ModelTier::Economy => "economy",
        ModelTier::Default => "default",
        ModelTier::Reasoning => "reasoning",
    }
}

fn parse_role(s: &str) -> MessageRole {
    match s {
        "User" => MessageRole::User,
        "Agent" => MessageRole::Agent,
        "System" => MessageRole::System,
        _ => MessageRole::System,
    }
}

fn format_role(r: &MessageRole) -> &'static str {
    match r {
        MessageRole::User => "User",
        MessageRole::Agent => "Agent",
        MessageRole::System => "System",
    }
}

#[cfg(test)]
#[path = "discussions_test.rs"]
mod discussions_test;
