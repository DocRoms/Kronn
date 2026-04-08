use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::models::{AgentType, ModelTier, QuickPrompt};

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_quick_prompt(row: &rusqlite::Row) -> QuickPrompt {
    let variables_json: String = row.get(4).unwrap_or_default();
    let agent_str: String = row.get(5).unwrap_or_default();
    let skill_ids_json: String = row.get(7).unwrap_or_default();
    let tier_str: String = row.get(8).unwrap_or_default();

    QuickPrompt {
        id: row.get(0).unwrap_or_default(),
        name: row.get(1).unwrap_or_default(),
        icon: row.get(2).unwrap_or_default(),
        prompt_template: row.get(3).unwrap_or_default(),
        variables: serde_json::from_str(&variables_json).unwrap_or_default(),
        agent: serde_json::from_str(&format!("\"{}\"", agent_str)).unwrap_or(AgentType::ClaudeCode),
        project_id: row.get(6).unwrap_or(None),
        skill_ids: serde_json::from_str(&skill_ids_json).unwrap_or_default(),
        tier: serde_json::from_str(&format!("\"{}\"", tier_str)).unwrap_or(ModelTier::Default),
        created_at: parse_dt(&row.get::<_, String>(9).unwrap_or_default()),
        updated_at: parse_dt(&row.get::<_, String>(10).unwrap_or_default()),
    }
}

pub fn list_quick_prompts(conn: &Connection) -> Result<Vec<QuickPrompt>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, icon, prompt_template, variables_json, agent, project_id, skill_ids_json, tier, created_at, updated_at
         FROM quick_prompts ORDER BY updated_at DESC"
    )?;
    let items = stmt.query_map([], |row| Ok(row_to_quick_prompt(row)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

pub fn get_quick_prompt(conn: &Connection, id: &str) -> Result<Option<QuickPrompt>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, icon, prompt_template, variables_json, agent, project_id, skill_ids_json, tier, created_at, updated_at
         FROM quick_prompts WHERE id = ?1"
    )?;
    let item = stmt.query_row(params![id], |row| Ok(row_to_quick_prompt(row))).ok();
    Ok(item)
}

pub fn insert_quick_prompt(conn: &Connection, qp: &QuickPrompt) -> Result<()> {
    let agent_str = serde_json::to_string(&qp.agent)?;
    let tier_str = serde_json::to_string(&qp.tier)?;
    conn.execute(
        "INSERT INTO quick_prompts (id, name, icon, prompt_template, variables_json, agent, project_id, skill_ids_json, tier, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            qp.id,
            qp.name,
            qp.icon,
            qp.prompt_template,
            serde_json::to_string(&qp.variables)?,
            agent_str.trim_matches('"'),
            qp.project_id,
            serde_json::to_string(&qp.skill_ids)?,
            tier_str.trim_matches('"'),
            qp.created_at.to_rfc3339(),
            qp.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn update_quick_prompt(conn: &Connection, qp: &QuickPrompt) -> Result<()> {
    let agent_str = serde_json::to_string(&qp.agent)?;
    let tier_str = serde_json::to_string(&qp.tier)?;
    conn.execute(
        "UPDATE quick_prompts SET name = ?2, icon = ?3, prompt_template = ?4, variables_json = ?5,
         agent = ?6, project_id = ?7, skill_ids_json = ?8, tier = ?9, updated_at = ?10
         WHERE id = ?1",
        params![
            qp.id,
            qp.name,
            qp.icon,
            qp.prompt_template,
            serde_json::to_string(&qp.variables)?,
            agent_str.trim_matches('"'),
            qp.project_id,
            serde_json::to_string(&qp.skill_ids)?,
            tier_str.trim_matches('"'),
            qp.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn delete_quick_prompt(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM quick_prompts WHERE id = ?1", params![id])?;
    Ok(())
}
