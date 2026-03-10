use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::models::*;

// ─── Projects ───────────────────────────────────────────────────────────────

pub fn list_projects(conn: &Connection) -> Result<Vec<Project>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, path, repo_url, token_override_json, ai_config_json,
                created_at, updated_at
         FROM projects ORDER BY name"
    )?;

    let projects: Vec<Project> = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let token_override_str: Option<String> = row.get(4)?;
        let ai_config_str: String = row.get(5)?;

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
            created_at: parse_dt(row.get::<_, String>(6)?),
            updated_at: parse_dt(row.get::<_, String>(7)?),
        }))
    })?.filter_map(|r| r.ok())
    .map(|(_id, project)| project)
    .collect();

    Ok(projects)
}

pub fn get_project(conn: &Connection, id: &str) -> Result<Option<Project>> {
    let projects = list_projects(conn)?;
    Ok(projects.into_iter().find(|p| p.id == id))
}

pub fn insert_project(conn: &Connection, project: &Project) -> Result<()> {
    conn.execute(
        "INSERT INTO projects (id, name, path, repo_url, token_override_json, ai_config_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            project.id,
            project.name,
            project.path,
            project.repo_url,
            project.token_override.as_ref().map(|t| serde_json::to_string(t).unwrap()),
            serde_json::to_string(&project.ai_config)?,
            project.created_at.to_rfc3339(),
            project.updated_at.to_rfc3339(),
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


// ─── Helpers ────────────────────────────────────────────────────────────────

fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
