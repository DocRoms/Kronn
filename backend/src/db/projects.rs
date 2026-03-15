use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::models::*;

// ─── Projects ───────────────────────────────────────────────────────────────

pub fn list_projects(conn: &Connection) -> Result<Vec<Project>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, path, repo_url, token_override_json, ai_config_json,
                created_at, updated_at, default_skill_ids_json, default_profile_id
         FROM projects ORDER BY name"
    )?;

    let projects: Vec<Project> = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let token_override_str: Option<String> = row.get(4)?;
        let ai_config_str: String = row.get(5)?;
        let skill_ids_str: String = row.get(8)?;

        Ok((id.clone(), Project {
            id,
            name: row.get(1)?,
            path: row.get(2)?,
            repo_url: row.get(3)?,
            token_override: token_override_str
                .and_then(|s| serde_json::from_str(&s).ok()),
            ai_config: serde_json::from_str(&ai_config_str)
                .unwrap_or(AiConfigStatus { detected: false, configs: vec![] }),
            audit_status: AiAuditStatus::default(), // enriched by API layer
            ai_todo_count: 0,  // enriched by API layer
            default_skill_ids: serde_json::from_str(&skill_ids_str).unwrap_or_default(),
            default_profile_id: row.get(9)?,
            created_at: parse_dt(row.get::<_, String>(6)?),
            updated_at: parse_dt(row.get::<_, String>(7)?),
        }))
    })?.filter_map(|r| r.ok())
    .map(|(_id, project)| project)
    .collect();

    Ok(projects)
}

pub fn get_project(conn: &Connection, id: &str) -> Result<Option<Project>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, path, repo_url, token_override_json, ai_config_json,
                created_at, updated_at, default_skill_ids_json, default_profile_id
         FROM projects WHERE id = ?1"
    )?;

    let project = stmt.query_row(params![id], |row| {
        let token_override_str: Option<String> = row.get(4)?;
        let ai_config_str: String = row.get(5)?;
        let skill_ids_str: String = row.get(8)?;

        Ok(Project {
            id: row.get(0)?,
            name: row.get(1)?,
            path: row.get(2)?,
            repo_url: row.get(3)?,
            token_override: token_override_str
                .and_then(|s| serde_json::from_str(&s).ok()),
            ai_config: serde_json::from_str(&ai_config_str)
                .unwrap_or(AiConfigStatus { detected: false, configs: vec![] }),
            audit_status: AiAuditStatus::default(),
            ai_todo_count: 0,
            default_skill_ids: serde_json::from_str(&skill_ids_str).unwrap_or_default(),
            default_profile_id: row.get(9)?,
            created_at: parse_dt(row.get::<_, String>(6)?),
            updated_at: parse_dt(row.get::<_, String>(7)?),
        })
    }).ok();

    Ok(project)
}

/// Batch-load project names by IDs in one query (avoids N+1).
pub fn get_project_names(conn: &Connection) -> Result<std::collections::HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT id, name FROM projects")?;
    let mut map = std::collections::HashMap::new();
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows.filter_map(|r| r.ok()) {
        map.insert(row.0, row.1);
    }
    Ok(map)
}

pub fn insert_project(conn: &Connection, project: &Project) -> Result<()> {
    conn.execute(
        "INSERT INTO projects (id, name, path, repo_url, token_override_json, ai_config_json, created_at, updated_at, default_skill_ids_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            project.id,
            project.name,
            project.path,
            project.repo_url,
            project.token_override.as_ref().map(serde_json::to_string).transpose()?,
            serde_json::to_string(&project.ai_config)?,
            project.created_at.to_rfc3339(),
            project.updated_at.to_rfc3339(),
            serde_json::to_string(&project.default_skill_ids)?,
        ],
    )?;
    Ok(())
}

pub fn delete_project(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

#[allow(dead_code)]
pub fn update_project_timestamps(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE projects SET updated_at = ?1 WHERE id = ?2",
        params![Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

pub fn update_project_ai_config(conn: &Connection, id: &str, ai_config: &AiConfigStatus) -> Result<()> {
    conn.execute(
        "UPDATE projects SET ai_config_json = ?1, updated_at = ?2 WHERE id = ?3",
        params![serde_json::to_string(ai_config)?, Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}


pub fn update_project_default_skills(conn: &Connection, id: &str, skill_ids: &[String]) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE projects SET default_skill_ids_json = ?1, updated_at = ?2 WHERE id = ?3",
        params![serde_json::to_string(skill_ids)?, Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

pub fn update_project_default_profile(conn: &Connection, id: &str, profile_id: Option<&str>) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE projects SET default_profile_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![profile_id, Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

pub fn delete_project_discussions(conn: &Connection, project_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM discussions WHERE project_id = ?1",
        params![project_id],
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
