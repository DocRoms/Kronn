use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::models::{
    AgentType, ModelTier, QuickPrompt, QuickPromptVersion, QuickPromptVersionMetrics,
};

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

// 0.8.5 — column layout (read by row_to_quick_prompt + every SELECT below):
//   0  id
//   1  name
//   2  icon
//   3  prompt_template
//   4  variables_json
//   5  agent
//   6  project_id
//   7  skill_ids_json
//   8  tier
//   9  created_at
//   10 updated_at
//   11 description
//   12 profile_ids_json      ← added in 056
//   13 directive_ids_json    ← added in 056
//   14 agent_settings_json   ← added in 070 (nullable)
fn row_to_quick_prompt(row: &rusqlite::Row) -> QuickPrompt {
    let variables_json: String = row.get(4).unwrap_or_default();
    let agent_str: String = row.get(5).unwrap_or_default();
    let skill_ids_json: String = row.get(7).unwrap_or_default();
    let tier_str: String = row.get(8).unwrap_or_default();
    let description: String = row.get(11).unwrap_or_default();
    // 070 — nullable; absent/NULL/garbage → None (fall back to tier).
    let agent_settings = row
        .get::<_, Option<String>>(14)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok());
    // 0.8.5 — fall back to `[]` when the column is missing (pre-056 rows
    // were already backfilled with `'[]'` by the ALTER, but we keep the
    // unwrap_or_default safety net for completeness).
    let profile_ids_json: String = row.get::<_, String>(12).unwrap_or_else(|_| "[]".into());
    let directive_ids_json: String = row.get::<_, String>(13).unwrap_or_else(|_| "[]".into());

    QuickPrompt {
        id: row.get(0).unwrap_or_default(),
        name: row.get(1).unwrap_or_default(),
        icon: row.get(2).unwrap_or_default(),
        prompt_template: row.get(3).unwrap_or_default(),
        variables: serde_json::from_str(&variables_json).unwrap_or_default(),
        agent: serde_json::from_str(&format!("\"{}\"", agent_str)).unwrap_or(AgentType::ClaudeCode),
        project_id: row.get(6).unwrap_or(None),
        skill_ids: serde_json::from_str(&skill_ids_json).unwrap_or_default(),
        profile_ids: serde_json::from_str(&profile_ids_json).unwrap_or_default(),
        directive_ids: serde_json::from_str(&directive_ids_json).unwrap_or_default(),
        tier: serde_json::from_str(&format!("\"{}\"", tier_str)).unwrap_or(ModelTier::Default),
        agent_settings,
        description,
        created_at: parse_dt(&row.get::<_, String>(9).unwrap_or_default()),
        updated_at: parse_dt(&row.get::<_, String>(10).unwrap_or_default()),
    }
}

const SELECT_COLUMNS: &str = "id, name, icon, prompt_template, variables_json, agent, project_id, skill_ids_json, tier, created_at, updated_at, description, profile_ids_json, directive_ids_json, agent_settings_json";

pub fn list_quick_prompts(conn: &Connection) -> Result<Vec<QuickPrompt>> {
    let sql = format!(
        "SELECT {} FROM quick_prompts ORDER BY updated_at DESC",
        SELECT_COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map([], |row| Ok(row_to_quick_prompt(row)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

pub fn get_quick_prompt(conn: &Connection, id: &str) -> Result<Option<QuickPrompt>> {
    let sql = format!("SELECT {} FROM quick_prompts WHERE id = ?1", SELECT_COLUMNS);
    let mut stmt = conn.prepare(&sql)?;
    let item = stmt
        .query_row(params![id], |row| Ok(row_to_quick_prompt(row)))
        .ok();
    Ok(item)
}

pub fn insert_quick_prompt(conn: &Connection, qp: &QuickPrompt) -> Result<()> {
    let agent_str = serde_json::to_string(&qp.agent)?;
    let tier_str = serde_json::to_string(&qp.tier)?;
    conn.execute(
        "INSERT INTO quick_prompts (id, name, icon, prompt_template, variables_json, agent, project_id, skill_ids_json, tier, created_at, updated_at, description, profile_ids_json, directive_ids_json, agent_settings_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
            qp.description,
            serde_json::to_string(&qp.profile_ids)?,
            serde_json::to_string(&qp.directive_ids)?,
            qp.agent_settings.as_ref().map(serde_json::to_string).transpose()?,
        ],
    )?;
    // 0.8.5 — seed the version history with v1 = initial state.
    snapshot_quick_prompt_version(conn, qp)?;
    Ok(())
}

pub fn update_quick_prompt(conn: &Connection, qp: &QuickPrompt) -> Result<()> {
    // 0.8.5 — snapshot BEFORE the UPDATE so the history table carries
    // every state the QP has ever had. Order matters: snapshot first
    // writes vN+1 with the NEW values (we want the post-update body
    // to be the latest version). If we ran it AFTER, a panic between
    // UPDATE and snapshot would lose the version.
    snapshot_quick_prompt_version(conn, qp)?;

    let agent_str = serde_json::to_string(&qp.agent)?;
    let tier_str = serde_json::to_string(&qp.tier)?;
    conn.execute(
        "UPDATE quick_prompts SET name = ?2, icon = ?3, prompt_template = ?4, variables_json = ?5,
         agent = ?6, project_id = ?7, skill_ids_json = ?8, tier = ?9, updated_at = ?10, description = ?11,
         profile_ids_json = ?12, directive_ids_json = ?13, agent_settings_json = ?14
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
            qp.description,
            serde_json::to_string(&qp.profile_ids)?,
            serde_json::to_string(&qp.directive_ids)?,
            qp.agent_settings.as_ref().map(serde_json::to_string).transpose()?,
        ],
    )?;
    Ok(())
}

/// Append a new version snapshot for the given QP. `version_index` is
/// computed as `MAX(version_index) + 1` per QP id — v1 = first INSERT,
/// v2+ = each UPDATE. Safe to call inside the same transaction as the
/// underlying mutation.
pub fn snapshot_quick_prompt_version(conn: &Connection, qp: &QuickPrompt) -> Result<u32> {
    let next: u32 = conn.query_row(
        "SELECT COALESCE(MAX(version_index), 0) + 1 FROM quick_prompt_versions WHERE quick_prompt_id = ?1",
        params![qp.id],
        |row| row.get::<_, i64>(0).map(|v| v as u32),
    )?;
    let agent_str = serde_json::to_string(&qp.agent)?;
    let tier_str = serde_json::to_string(&qp.tier)?;
    conn.execute(
        "INSERT INTO quick_prompt_versions (
            id, quick_prompt_id, version_index, name, icon, prompt_template, variables_json,
            agent, project_id, skill_ids_json, profile_ids_json, directive_ids_json,
            tier, description, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            uuid::Uuid::new_v4().to_string(),
            qp.id,
            next as i64,
            qp.name,
            qp.icon,
            qp.prompt_template,
            serde_json::to_string(&qp.variables)?,
            agent_str.trim_matches('"'),
            qp.project_id,
            serde_json::to_string(&qp.skill_ids)?,
            serde_json::to_string(&qp.profile_ids)?,
            serde_json::to_string(&qp.directive_ids)?,
            tier_str.trim_matches('"'),
            qp.description,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(next)
}

/// Passe D (export v5) — every version row of every QP, for DB export.
pub fn list_all_quick_prompt_versions(conn: &Connection) -> Result<Vec<QuickPromptVersion>> {
    let mut ids = Vec::new();
    {
        let mut stmt =
            conn.prepare("SELECT DISTINCT quick_prompt_id FROM quick_prompt_versions")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for r in rows {
            ids.push(r?);
        }
    }
    let mut all = Vec::new();
    for id in ids {
        all.extend(list_quick_prompt_versions(conn, &id)?);
    }
    Ok(all)
}

/// Passe D (import v5) — insert a version row VERBATIM (id, version_index and
/// created_at preserved), unlike `snapshot_quick_prompt_version` which mints
/// the next index. Import-only.
pub fn insert_quick_prompt_version_row(conn: &Connection, v: &QuickPromptVersion) -> Result<()> {
    let agent_str = serde_json::to_string(&v.agent)?;
    let tier_str = serde_json::to_string(&v.tier)?;
    conn.execute(
        "INSERT INTO quick_prompt_versions (
            id, quick_prompt_id, version_index, name, icon, prompt_template, variables_json,
            agent, project_id, skill_ids_json, profile_ids_json, directive_ids_json,
            tier, description, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            v.id,
            v.quick_prompt_id,
            v.version_index as i64,
            v.name,
            v.icon,
            v.prompt_template,
            serde_json::to_string(&v.variables)?,
            agent_str.trim_matches('"'),
            v.project_id,
            serde_json::to_string(&v.skill_ids)?,
            serde_json::to_string(&v.profile_ids)?,
            serde_json::to_string(&v.directive_ids)?,
            tier_str.trim_matches('"'),
            v.description,
            v.created_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Return all stored versions for a QP, newest first.
pub fn list_quick_prompt_versions(
    conn: &Connection,
    qp_id: &str,
) -> Result<Vec<QuickPromptVersion>> {
    let mut stmt = conn.prepare(
        "SELECT id, quick_prompt_id, version_index, name, icon, prompt_template, variables_json,
                agent, project_id, skill_ids_json, profile_ids_json, directive_ids_json,
                tier, description, created_at
         FROM quick_prompt_versions
         WHERE quick_prompt_id = ?1
         ORDER BY version_index DESC",
    )?;
    let rows = stmt.query_map(params![qp_id], |row| {
        let agent_str: String = row.get::<_, String>(7).unwrap_or_default();
        let tier_str: String = row.get::<_, String>(12).unwrap_or_default();
        let variables_json: String = row.get::<_, String>(6).unwrap_or_default();
        let skill_ids_json: String = row.get::<_, String>(9).unwrap_or_default();
        let profile_ids_json: String = row.get::<_, String>(10).unwrap_or_default();
        let directive_ids_json: String = row.get::<_, String>(11).unwrap_or_default();
        Ok(QuickPromptVersion {
            id: row.get(0)?,
            quick_prompt_id: row.get(1)?,
            version_index: row.get::<_, i64>(2)? as u32,
            name: row.get(3)?,
            icon: row.get(4)?,
            prompt_template: row.get(5)?,
            variables: serde_json::from_str(&variables_json).unwrap_or_default(),
            agent: serde_json::from_str(&format!("\"{}\"", agent_str))
                .unwrap_or(AgentType::ClaudeCode),
            project_id: row.get(8)?,
            skill_ids: serde_json::from_str(&skill_ids_json).unwrap_or_default(),
            profile_ids: serde_json::from_str(&profile_ids_json).unwrap_or_default(),
            directive_ids: serde_json::from_str(&directive_ids_json).unwrap_or_default(),
            tier: serde_json::from_str(&format!("\"{}\"", tier_str)).unwrap_or(ModelTier::Default),
            description: row.get(13).unwrap_or_default(),
            created_at: parse_dt(&row.get::<_, String>(14).unwrap_or_default()),
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Aggregate per-version metrics across all discussions whose
/// `originating_qp_id` matches and that have at least one Agent
/// message. Only the FIRST Agent message per discussion is counted
/// (the QP's first reply). Returns one row per version_index that has
/// at least one launch; versions with zero launches are NOT returned
/// (the frontend renders them with N/A — saves a row roundtrip).
pub fn list_quick_prompt_version_metrics(
    conn: &Connection,
    qp_id: &str,
) -> Result<Vec<QuickPromptVersionMetrics>> {
    // Inner query: pick the first Agent message per discussion (by
    // sort_order, falling back to timestamp). Aggregate the chosen
    // rows grouped by `originating_qp_version` on the parent disc.
    let mut stmt = conn.prepare(
        "SELECT d.originating_qp_version,
                COUNT(*) as n,
                CAST(AVG(m.tokens_used) AS INTEGER) as avg_tokens,
                CAST(AVG(m.duration_ms) AS INTEGER) as avg_duration_ms,
                AVG(m.cost_usd) as avg_cost_usd
         FROM discussions d
         JOIN messages m ON m.discussion_id = d.id
         WHERE d.originating_qp_id = ?1
           AND d.originating_qp_version IS NOT NULL
           AND m.role = 'Agent'
           AND m.id = (
               SELECT id FROM messages
               WHERE discussion_id = d.id AND role = 'Agent'
               ORDER BY sort_order, timestamp
               LIMIT 1
           )
         GROUP BY d.originating_qp_version
         ORDER BY d.originating_qp_version DESC",
    )?;
    let rows = stmt.query_map(params![qp_id], |row| {
        Ok(QuickPromptVersionMetrics {
            version_index: row.get::<_, i64>(0)? as u32,
            launches: row.get::<_, i64>(1)? as u32,
            avg_tokens: row.get::<_, Option<i64>>(2)?.unwrap_or(0) as u64,
            avg_duration_ms: row.get::<_, Option<i64>>(3)?.map(|v| v as u64),
            avg_cost_usd: row.get::<_, Option<f64>>(4)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Delete a single archived version from the snapshot table. Refuses
/// to delete the CURRENT version (= `MAX(version_index)` for the QP)
/// because that's the anchor between the live QP body and the
/// history timeline — without it, every future update would orphan
/// the "current state" from the drawer.
///
/// Side effect: any discussion that had its lineage stamped with
/// `(qp_id, version_index)` gets `originating_qp_id` + version
/// cleared so the metrics aggregator naturally excludes those
/// launches (the disc itself stays — only the QP attribution is
/// dropped). Returns:
///   - `Ok(true)`  : version found and deleted, lineage cleared.
///   - `Ok(false)` : version_index doesn't exist for this QP.
///   - `Err`       : the version is the current one OR a SQL error.
pub fn delete_quick_prompt_version(
    conn: &Connection,
    qp_id: &str,
    version_index: u32,
) -> Result<bool> {
    let current: Option<u32> = current_version_index(conn, qp_id)?;
    if current == Some(version_index) {
        anyhow::bail!(
            "Cannot delete the current version v{} of QP {} — it's the anchor for the live QP body. \
             Edit the QP to create a new version, then delete the old one.",
            version_index, qp_id
        );
    }
    // Clear lineage on any disc that referenced this version so the
    // metrics aggregator stops counting those launches under this QP.
    // The disc itself stays — only the QP attribution drops.
    conn.execute(
        "UPDATE discussions SET originating_qp_id = NULL, originating_qp_version = NULL
         WHERE originating_qp_id = ?1 AND originating_qp_version = ?2",
        params![qp_id, version_index as i64],
    )?;
    let deleted = conn.execute(
        "DELETE FROM quick_prompt_versions WHERE quick_prompt_id = ?1 AND version_index = ?2",
        params![qp_id, version_index as i64],
    )?;
    Ok(deleted > 0)
}

/// Resolve the CURRENT version_index of a QP (the highest version_index
/// in the snapshot table). Used by the QP-launch path to stamp
/// `discussions.originating_qp_version` with the version the user
/// actually triggered. Returns `None` when the QP exists but has no
/// snapshot yet (defensive — shouldn't happen post-058 since
/// insert_quick_prompt seeds v1).
pub fn current_version_index(conn: &Connection, qp_id: &str) -> Result<Option<u32>> {
    let v: Option<i64> = conn.query_row(
        "SELECT MAX(version_index) FROM quick_prompt_versions WHERE quick_prompt_id = ?1",
        params![qp_id],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    Ok(v.map(|n| n as u32))
}

pub fn delete_quick_prompt(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM quick_prompts WHERE id = ?1", params![id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AgentType, ModelTier, PromptVariable};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn mk_qp(id: &str, name: &str) -> QuickPrompt {
        QuickPrompt {
            id: id.into(),
            name: name.into(),
            icon: "✨".into(),
            prompt_template: format!("template-for-{name}"),
            variables: vec![PromptVariable {
                name: "var".into(),
                label: "Var".into(),
                placeholder: String::new(),
                description: None,
                required: true,
                pattern: None,
            }],
            agent: AgentType::ClaudeCode,
            project_id: None,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            tier: ModelTier::Default,
            agent_settings: None,
            description: String::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn insert_seeds_version_one() {
        let conn = test_conn();
        let qp = mk_qp("qp-1", "first");
        insert_quick_prompt(&conn, &qp).unwrap();
        let cur = current_version_index(&conn, "qp-1").unwrap();
        assert_eq!(
            cur,
            Some(1),
            "insert must seed v1 automatically (058 contract)"
        );
    }

    #[test]
    fn current_version_index_returns_none_for_unknown_qp() {
        let conn = test_conn();
        let cur = current_version_index(&conn, "nope").unwrap();
        assert!(cur.is_none());
    }

    #[test]
    fn list_quick_prompts_empty_returns_empty_vec() {
        let conn = test_conn();
        let list = list_quick_prompts(&conn).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn list_quick_prompts_returns_inserted_ones() {
        let conn = test_conn();
        insert_quick_prompt(&conn, &mk_qp("qp-A", "Alpha")).unwrap();
        insert_quick_prompt(&conn, &mk_qp("qp-B", "Beta")).unwrap();
        let list = list_quick_prompts(&conn).unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn get_quick_prompt_unknown_returns_none() {
        let conn = test_conn();
        assert!(get_quick_prompt(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn get_quick_prompt_returns_inserted_qp() {
        let conn = test_conn();
        insert_quick_prompt(&conn, &mk_qp("qp-X", "ToFind")).unwrap();
        let found = get_quick_prompt(&conn, "qp-X").unwrap().unwrap();
        assert_eq!(found.name, "ToFind");
    }

    #[test]
    fn delete_quick_prompt_unknown_id_is_silent() {
        let conn = test_conn();
        // Pre-058 contract : DELETE on unknown ID succeeds (no-op).
        // The handler reports `success: true` either way — we'd break
        // the UI flow if this returned an error.
        delete_quick_prompt(&conn, "nope").unwrap();
    }

    #[test]
    fn delete_quick_prompt_removes_row() {
        let conn = test_conn();
        insert_quick_prompt(&conn, &mk_qp("qp-D", "Doomed")).unwrap();
        delete_quick_prompt(&conn, "qp-D").unwrap();
        assert!(get_quick_prompt(&conn, "qp-D").unwrap().is_none());
    }

    #[test]
    fn snapshot_creates_versions_in_ascending_order() {
        let conn = test_conn();
        let mut qp = mk_qp("qp-V", "Versioned");
        insert_quick_prompt(&conn, &qp).unwrap();
        // v1 seeded.

        qp.prompt_template = "v2 body".into();
        let v2 = snapshot_quick_prompt_version(&conn, &qp).unwrap();
        assert_eq!(v2, 2);

        qp.prompt_template = "v3 body".into();
        let v3 = snapshot_quick_prompt_version(&conn, &qp).unwrap();
        assert_eq!(v3, 3);

        // Listing returns 3 entries.
        let versions = list_quick_prompt_versions(&conn, "qp-V").unwrap();
        assert_eq!(versions.len(), 3);
        assert_eq!(current_version_index(&conn, "qp-V").unwrap(), Some(3));
    }

    #[test]
    fn delete_quick_prompt_version_refuses_to_delete_current() {
        let conn = test_conn();
        let qp = mk_qp("qp-LATEST", "Latest");
        insert_quick_prompt(&conn, &qp).unwrap();
        // v1 = current — must refuse.
        let result = delete_quick_prompt_version(&conn, "qp-LATEST", 1);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Cannot delete the current version"),
            "expected anchor-protection error, got: {msg}"
        );
    }

    #[test]
    fn delete_quick_prompt_version_removes_old_version() {
        let conn = test_conn();
        let mut qp = mk_qp("qp-OLD", "Old");
        insert_quick_prompt(&conn, &qp).unwrap();
        qp.prompt_template = "newer body".into();
        snapshot_quick_prompt_version(&conn, &qp).unwrap(); // v2

        let deleted = delete_quick_prompt_version(&conn, "qp-OLD", 1).unwrap();
        assert!(deleted);

        // v1 is gone, v2 remains.
        let versions = list_quick_prompt_versions(&conn, "qp-OLD").unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version_index, 2);
    }

    #[test]
    fn list_quick_prompt_version_metrics_empty_when_no_launches() {
        let conn = test_conn();
        insert_quick_prompt(&conn, &mk_qp("qp-NoLaunch", "NoLaunch")).unwrap();
        let metrics = list_quick_prompt_version_metrics(&conn, "qp-NoLaunch").unwrap();
        assert!(
            metrics.is_empty(),
            "metrics must be empty for a QP that hasn't been launched yet"
        );
    }

    #[test]
    fn update_quick_prompt_modifies_fields() {
        let conn = test_conn();
        insert_quick_prompt(&conn, &mk_qp("qp-U", "Original")).unwrap();
        let mut qp = get_quick_prompt(&conn, "qp-U").unwrap().unwrap();
        qp.name = "Updated".into();
        qp.prompt_template = "new template".into();
        update_quick_prompt(&conn, &qp).unwrap();

        let reloaded = get_quick_prompt(&conn, "qp-U").unwrap().unwrap();
        assert_eq!(reloaded.name, "Updated");
        assert_eq!(reloaded.prompt_template, "new template");
    }
}
