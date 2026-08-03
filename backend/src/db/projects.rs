use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use super::parse_dt;
use crate::models::*;

pub const DEFAULT_DEPENDENCY_MONITORING_INTERVAL_DAYS: u16 = 7;

#[derive(Debug, Clone)]
pub struct DependencyMonitoringRecord {
    pub interval_days: Option<u16>,
    pub manifest_fingerprint: Option<u64>,
    pub summary: Option<DependencyUpdateSummary>,
}

// ─── Projects ───────────────────────────────────────────────────────────────

pub fn list_projects(conn: &Connection) -> Result<Vec<Project>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, path, repo_url, token_override_json, ai_config_json,
                created_at, updated_at, default_skill_ids_json, default_profile_id,
                briefing_notes, linked_repos_json
         FROM projects ORDER BY name",
    )?;

    let projects: Vec<Project> = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let token_override_str: Option<String> = row.get(4)?;
            let ai_config_str: String = row.get(5)?;
            let skill_ids_str: String = row.get(8)?;
            let linked_repos_str: String = row.get(11).unwrap_or_else(|_| "[]".into());

            Ok((
                id.clone(),
                Project {
                    id,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    repo_url: row.get(3)?,
                    token_override: token_override_str.and_then(|s| serde_json::from_str(&s).ok()),
                    ai_config: serde_json::from_str(&ai_config_str).unwrap_or(AiConfigStatus {
                        detected: false,
                        configs: vec![],
                    }),
                    audit_status: AiAuditStatus::default(), // enriched by API layer
                    ai_todo_count: 0,                       // enriched by API layer
                    tech_debt_count: 0,
                    needs_docs_migration: false, // enriched by API layer
                    path_exists: true,
                    default_skill_ids: serde_json::from_str(&skill_ids_str).unwrap_or_default(),
                    default_profile_id: row.get(9)?,
                    briefing_notes: row.get(10)?,
                    linked_repos: serde_json::from_str(&linked_repos_str).unwrap_or_default(),
                    created_at: parse_dt(row.get::<_, String>(6)?),
                    updated_at: parse_dt(row.get::<_, String>(7)?),
                },
            ))
        })?
        .filter_map(|r| r.ok())
        .map(|(_id, project)| project)
        .collect();

    Ok(projects)
}

pub fn get_project(conn: &Connection, id: &str) -> Result<Option<Project>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, path, repo_url, token_override_json, ai_config_json,
                created_at, updated_at, default_skill_ids_json, default_profile_id,
                briefing_notes, linked_repos_json
         FROM projects WHERE id = ?1",
    )?;

    let project = stmt
        .query_row(params![id], |row| {
            let token_override_str: Option<String> = row.get(4)?;
            let ai_config_str: String = row.get(5)?;
            let skill_ids_str: String = row.get(8)?;
            let linked_repos_str: String = row.get(11).unwrap_or_else(|_| "[]".into());

            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                repo_url: row.get(3)?,
                token_override: token_override_str.and_then(|s| serde_json::from_str(&s).ok()),
                ai_config: serde_json::from_str(&ai_config_str).unwrap_or(AiConfigStatus {
                    detected: false,
                    configs: vec![],
                }),
                audit_status: AiAuditStatus::default(),
                ai_todo_count: 0,
                tech_debt_count: 0,
                needs_docs_migration: false,
                path_exists: true,
                default_skill_ids: serde_json::from_str(&skill_ids_str).unwrap_or_default(),
                default_profile_id: row.get(9)?,
                briefing_notes: row.get(10)?,
                linked_repos: serde_json::from_str(&linked_repos_str).unwrap_or_default(),
                created_at: parse_dt(row.get::<_, String>(6)?),
                updated_at: parse_dt(row.get::<_, String>(7)?),
            })
        })
        .ok();

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
        "INSERT INTO projects (id, name, path, repo_url, token_override_json, ai_config_json, created_at, updated_at, default_skill_ids_json, briefing_notes, linked_repos_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
            project.briefing_notes,
            serde_json::to_string(&project.linked_repos)?,
        ],
    )?;
    Ok(())
}

pub fn update_project_briefing_notes(
    conn: &Connection,
    id: &str,
    notes: Option<&str>,
) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE projects SET briefing_notes = ?1, updated_at = ?2 WHERE id = ?3",
        params![notes, Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

/// Folders omitted by the read-only source browser and full-text search.
pub fn get_source_exclusions(conn: &Connection, project_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM project_source_exclusions
         WHERE project_id = ?1 ORDER BY path COLLATE NOCASE",
    )?;
    let paths = stmt
        .query_map([project_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(paths)
}

/// Atomically replace one project's source-browser folder exclusions.
pub fn replace_source_exclusions(
    conn: &Connection,
    project_id: &str,
    paths: &[String],
) -> Result<bool> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
        [project_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Ok(false);
    }

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM project_source_exclusions WHERE project_id = ?1",
        [project_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO project_source_exclusions (project_id, path)
             VALUES (?1, ?2)",
        )?;
        for path in paths {
            stmt.execute(params![project_id, path])?;
        }
    }
    tx.commit()?;
    Ok(true)
}

/// Load the durable dependency-monitoring state for one project.
///
/// A project without a row uses the safe default: an opportunistic weekly
/// check when its overview is opened. `interval_days = None` is reserved for
/// an explicit manual-only choice.
pub fn get_dependency_monitoring(
    conn: &Connection,
    project_id: &str,
) -> Result<DependencyMonitoringRecord> {
    let row = conn
        .query_row(
            "SELECT interval_days, manifest_fingerprint, summary_json
             FROM project_dependency_monitoring
             WHERE project_id = ?1",
            [project_id],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;

    let Some((interval_days, manifest_fingerprint, summary_json)) = row else {
        return Ok(DependencyMonitoringRecord {
            interval_days: Some(DEFAULT_DEPENDENCY_MONITORING_INTERVAL_DAYS),
            manifest_fingerprint: None,
            summary: None,
        });
    };

    Ok(DependencyMonitoringRecord {
        interval_days: interval_days.and_then(|days| u16::try_from(days).ok()),
        manifest_fingerprint: manifest_fingerprint.and_then(|value| value.parse().ok()),
        summary: summary_json.and_then(|value| serde_json::from_str(&value).ok()),
    })
}

/// Persist the result of a read-only dependency scan without altering the
/// configured cadence.
pub fn save_dependency_scan(
    conn: &Connection,
    project_id: &str,
    manifest_fingerprint: u64,
    summary: &DependencyUpdateSummary,
) -> Result<bool> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
        [project_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Ok(false);
    }

    conn.execute(
        "INSERT INTO project_dependency_monitoring
             (project_id, interval_days, manifest_fingerprint, summary_json, checked_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(project_id) DO UPDATE SET
             manifest_fingerprint = excluded.manifest_fingerprint,
             summary_json = excluded.summary_json,
             checked_at = excluded.checked_at,
             updated_at = excluded.updated_at",
        params![
            project_id,
            i64::from(DEFAULT_DEPENDENCY_MONITORING_INTERVAL_DAYS),
            manifest_fingerprint.to_string(),
            serde_json::to_string(summary)?,
            summary.checked_at.to_rfc3339(),
        ],
    )?;
    Ok(true)
}

/// Configure periodic dependency checks. `None` means manual checks only.
pub fn set_dependency_monitoring_interval(
    conn: &Connection,
    project_id: &str,
    interval_days: Option<u16>,
) -> Result<bool> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
        [project_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Ok(false);
    }

    conn.execute(
        "INSERT INTO project_dependency_monitoring
             (project_id, interval_days, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(project_id) DO UPDATE SET
             interval_days = excluded.interval_days,
             updated_at = excluded.updated_at",
        params![
            project_id,
            interval_days.map(i64::from),
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(true)
}

/// 0.8.3 — Replace the linked_repos list for a project.
pub fn update_project_linked_repos(
    conn: &Connection,
    id: &str,
    linked_repos: &[LinkedRepo],
) -> Result<bool> {
    let json = serde_json::to_string(linked_repos)?;
    let affected = conn.execute(
        "UPDATE projects SET linked_repos_json = ?1, updated_at = ?2 WHERE id = ?3",
        params![json, Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

pub fn get_project_briefing_notes(conn: &Connection, id: &str) -> Result<Option<String>> {
    let notes = conn
        .query_row(
            "SELECT briefing_notes FROM projects WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .ok()
        .flatten();
    Ok(notes)
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

pub fn update_project_ai_config(
    conn: &Connection,
    id: &str,
    ai_config: &AiConfigStatus,
) -> Result<()> {
    conn.execute(
        "UPDATE projects SET ai_config_json = ?1, updated_at = ?2 WHERE id = ?3",
        params![
            serde_json::to_string(ai_config)?,
            Utc::now().to_rfc3339(),
            id
        ],
    )?;
    Ok(())
}

pub fn update_project_default_skills(
    conn: &Connection,
    id: &str,
    skill_ids: &[String],
) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE projects SET default_skill_ids_json = ?1, updated_at = ?2 WHERE id = ?3",
        params![
            serde_json::to_string(skill_ids)?,
            Utc::now().to_rfc3339(),
            id
        ],
    )?;
    Ok(affected > 0)
}

pub fn update_project_path(conn: &Connection, id: &str, new_path: &str) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE projects SET path = ?1, updated_at = ?2 WHERE id = ?3",
        params![new_path, Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

pub fn update_project_default_profile(
    conn: &Connection,
    id: &str,
    profile_id: Option<&str>,
) -> Result<bool> {
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

#[cfg(test)]
mod dependency_monitoring_tests {
    use super::*;
    use crate::db::migrations;
    use chrono::TimeZone;

    fn database_with_project() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory database");
        migrations::run(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('project-1', 'Project', '/tmp/project-1', ?1, ?1)",
            [Utc::now().to_rfc3339()],
        )
        .expect("project");
        conn
    }

    #[test]
    fn dependency_monitoring_defaults_to_weekly_and_persists_manual_choice() {
        let conn = database_with_project();
        let initial = get_dependency_monitoring(&conn, "project-1").expect("initial state");
        assert_eq!(
            initial.interval_days,
            Some(DEFAULT_DEPENDENCY_MONITORING_INTERVAL_DAYS)
        );
        assert!(initial.summary.is_none());

        assert!(
            set_dependency_monitoring_interval(&conn, "project-1", None).expect("set manual-only")
        );
        let manual = get_dependency_monitoring(&conn, "project-1").expect("manual state");
        assert_eq!(manual.interval_days, None);
    }

    #[test]
    fn saving_scan_preserves_cadence_and_round_trips_result() {
        let conn = database_with_project();
        set_dependency_monitoring_interval(&conn, "project-1", Some(14)).expect("set cadence");
        let checked_at = Utc
            .with_ymd_and_hms(2026, 7, 30, 10, 15, 0)
            .single()
            .expect("timestamp");
        let summary = DependencyUpdateSummary {
            managers: Vec::new(),
            total_outdated: 0,
            total_major: 0,
            checked_at,
            cached: false,
            monitoring_interval_days: Some(14),
            next_check_at: Some(checked_at + chrono::Duration::days(14)),
        };

        assert!(save_dependency_scan(&conn, "project-1", u64::MAX, &summary).expect("save scan"));
        let stored = get_dependency_monitoring(&conn, "project-1").expect("stored scan");
        assert_eq!(stored.interval_days, Some(14));
        assert_eq!(stored.manifest_fingerprint, Some(u64::MAX));
        let stored_summary = stored.summary.expect("summary");
        assert_eq!(stored_summary.checked_at, checked_at);
        assert_eq!(stored_summary.total_outdated, 0);
    }

    #[test]
    fn dependency_monitoring_rejects_unknown_project() {
        let conn = database_with_project();
        assert!(
            !set_dependency_monitoring_interval(&conn, "missing", Some(7))
                .expect("unknown project")
        );
        let summary = DependencyUpdateSummary {
            managers: Vec::new(),
            total_outdated: 0,
            total_major: 0,
            checked_at: Utc::now(),
            cached: false,
            monitoring_interval_days: Some(7),
            next_check_at: None,
        };
        assert!(!save_dependency_scan(&conn, "missing", 1, &summary).expect("unknown project"));
    }
}
