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
            mcps: vec![],  // loaded separately
            tasks: vec![], // loaded separately
            created_at: parse_dt(row.get::<_, String>(6)?),
            updated_at: parse_dt(row.get::<_, String>(7)?),
        }))
    })?.filter_map(|r| r.ok())
    .map(|(id, mut project)| {
        // Load nested tasks (MCPs now in separate mcp_configs system)
        project.mcps = vec![];
        project.tasks = list_tasks(conn, &id).unwrap_or_default();
        project
    })
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

// ─── MCPs (legacy stubs — real MCP logic in db::mcps) ───────────────────────
// The old `mcps` table has been dropped by migration 002.
// These stubs keep Project loading working while we transition.

// ─── Tasks ──────────────────────────────────────────────────────────────────

pub fn list_tasks(conn: &Connection, project_id: &str) -> Result<Vec<ScheduledTask>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, cron_expr, human_interval, agent, prompt, active,
                last_run, last_status_json, tokens_used, created_at
         FROM tasks WHERE project_id = ?1"
    )?;

    let tasks = stmt.query_map(params![project_id], |row| {
        let agent_str: String = row.get(4)?;
        let last_run_str: Option<String> = row.get(7)?;
        let last_status_str: Option<String> = row.get(8)?;

        Ok(ScheduledTask {
            id: row.get(0)?,
            name: row.get(1)?,
            cron_expr: row.get(2)?,
            human_interval: row.get(3)?,
            agent: serde_json::from_str(&format!("\"{}\"", agent_str))
                .unwrap_or(AgentType::ClaudeCode),
            prompt: row.get(5)?,
            active: row.get::<_, i32>(6)? != 0,
            last_run: last_run_str.map(parse_dt),
            last_status: last_status_str.and_then(|s| serde_json::from_str(&s).ok()),
            tokens_used: row.get(9)?,
            created_at: parse_dt(row.get::<_, String>(10)?),
        })
    })?.filter_map(|r| r.ok()).collect();

    Ok(tasks)
}

pub fn insert_task(conn: &Connection, project_id: &str, task: &ScheduledTask) -> Result<()> {
    conn.execute(
        "INSERT INTO tasks (id, project_id, name, cron_expr, human_interval, agent, prompt, active, tokens_used, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            task.id,
            project_id,
            task.name,
            task.cron_expr,
            task.human_interval,
            serde_json::to_string(&task.agent)?.trim_matches('"'),
            task.prompt,
            task.active as i32,
            task.tokens_used,
            task.created_at.to_rfc3339(),
        ],
    )?;
    update_project_timestamps(conn, project_id)?;
    Ok(())
}

pub fn delete_task(conn: &Connection, project_id: &str, task_id: &str) -> Result<bool> {
    let affected = conn.execute(
        "DELETE FROM tasks WHERE id = ?1 AND project_id = ?2",
        params![task_id, project_id],
    )?;
    update_project_timestamps(conn, project_id)?;
    Ok(affected > 0)
}

pub fn toggle_task(conn: &Connection, project_id: &str, task_id: &str) -> Result<Option<bool>> {
    let new_active: Option<bool> = conn.query_row(
        "UPDATE tasks SET active = NOT active WHERE id = ?1 AND project_id = ?2 RETURNING active",
        params![task_id, project_id],
        |row| Ok(row.get::<_, i32>(0)? != 0),
    ).ok();

    if new_active.is_some() {
        update_project_timestamps(conn, project_id)?;
    }
    Ok(new_active)
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
