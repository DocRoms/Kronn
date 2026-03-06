use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::models::*;

// ─── Discussions ────────────────────────────────────────────────────────────

pub fn list_discussions(conn: &Connection) -> Result<Vec<Discussion>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, title, agent, language, participants_json,
                created_at, updated_at
         FROM discussions ORDER BY updated_at DESC"
    )?;

    let discussions: Vec<Discussion> = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let agent_str: String = row.get(3)?;
        let participants_str: String = row.get(5)?;

        Ok((id.clone(), Discussion {
            id,
            project_id: row.get(1)?,
            title: row.get(2)?,
            agent: parse_agent_type(&agent_str),
            language: row.get(4)?,
            participants: serde_json::from_str(&participants_str).unwrap_or_default(),
            messages: vec![], // loaded separately
            created_at: parse_dt(row.get::<_, String>(6)?),
            updated_at: parse_dt(row.get::<_, String>(7)?),
        }))
    })?.filter_map(|r| r.ok())
    .map(|(id, mut disc)| {
        disc.messages = list_messages(conn, &id).unwrap_or_default();
        disc
    })
    .collect();

    Ok(discussions)
}

pub fn get_discussion(conn: &Connection, id: &str) -> Result<Option<Discussion>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, title, agent, language, participants_json,
                created_at, updated_at
         FROM discussions WHERE id = ?1"
    )?;

    let disc = stmt.query_row(params![id], |row| {
        let agent_str: String = row.get(3)?;
        let participants_str: String = row.get(5)?;

        Ok(Discussion {
            id: row.get(0)?,
            project_id: row.get(1)?,
            title: row.get(2)?,
            agent: parse_agent_type(&agent_str),
            language: row.get(4)?,
            participants: serde_json::from_str(&participants_str).unwrap_or_default(),
            messages: vec![],
            created_at: parse_dt(row.get::<_, String>(6)?),
            updated_at: parse_dt(row.get::<_, String>(7)?),
        })
    }).ok();

    if let Some(mut d) = disc {
        d.messages = list_messages(conn, &d.id)?;
        Ok(Some(d))
    } else {
        Ok(None)
    }
}

pub fn insert_discussion(conn: &Connection, disc: &Discussion) -> Result<()> {
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, agent, language, participants_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            disc.id,
            disc.project_id,
            disc.title,
            format_agent_type(&disc.agent),
            disc.language,
            serde_json::to_string(&disc.participants)?,
            disc.created_at.to_rfc3339(),
            disc.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn delete_discussion(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM discussions WHERE id = ?1", params![id])?;
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

pub fn list_messages(conn: &Connection, discussion_id: &str) -> Result<Vec<DiscussionMessage>> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, agent_type, timestamp
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
        "INSERT INTO messages (id, discussion_id, role, content, agent_type, timestamp, sort_order)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            msg.id,
            discussion_id,
            format_role(&msg.role),
            msg.content,
            msg.agent_type.as_ref().map(format_agent_type),
            msg.timestamp.to_rfc3339(),
            next_order,
        ],
    )?;

    update_discussion_timestamp(conn, discussion_id)?;
    Ok(())
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
    Ok(affected > 0)
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn parse_agent_type(s: &str) -> AgentType {
    match s {
        "ClaudeCode" => AgentType::ClaudeCode,
        "Codex" => AgentType::Codex,
        "Vibe" => AgentType::Vibe,
        _ => AgentType::Custom,
    }
}

fn format_agent_type(a: &AgentType) -> String {
    match a {
        AgentType::ClaudeCode => "ClaudeCode".into(),
        AgentType::Codex => "Codex".into(),
        AgentType::Vibe => "Vibe".into(),
        AgentType::Custom => "Custom".into(),
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
