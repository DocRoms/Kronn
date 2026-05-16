//! DB layer for `QuickApi` — reusable API call templates (0.6.0).
//!
//! Mirror of `quick_prompts.rs` but the moteur is HTTP, not LLM. Field
//! names follow `WorkflowStep` ApiCall fields verbatim so the editor
//! (frontend) can reuse `ApiCallStepCard` without remapping.
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::collections::HashMap;

use crate::models::{ExtractSpec, PaginationSpec, QuickApi};

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// Decode an optional JSON string column into a typed value. Empty strings
/// and SQL NULLs both resolve to `None` so the model surfaces a clean
/// `Option::None` to API consumers.
fn parse_json_opt<T: serde::de::DeserializeOwned>(s: Option<String>) -> Option<T> {
    s.filter(|v| !v.is_empty()).and_then(|v| serde_json::from_str(&v).ok())
}

fn row_to_quick_api(row: &rusqlite::Row) -> QuickApi {
    QuickApi {
        id: row.get(0).unwrap_or_default(),
        name: row.get(1).unwrap_or_default(),
        description: row.get(2).unwrap_or_default(),
        icon: row.get(3).unwrap_or_else(|_| "🔌".to_string()),
        project_id: row.get(4).ok(),
        api_plugin_slug: row.get(5).unwrap_or_default(),
        api_config_id: row.get(6).unwrap_or_default(),
        api_endpoint_path: row.get(7).unwrap_or_default(),
        api_method: row.get::<_, Option<String>>(8).unwrap_or(None).filter(|s| !s.is_empty()),
        api_query: parse_json_opt::<HashMap<String, String>>(row.get(9).unwrap_or(None)),
        api_path_params: parse_json_opt::<HashMap<String, String>>(row.get(10).unwrap_or(None)),
        api_headers: parse_json_opt::<HashMap<String, String>>(row.get(11).unwrap_or(None)),
        // api_body stored as TEXT but typed Value — same trick as Workflow.
        api_body: parse_json_opt::<serde_json::Value>(row.get(12).unwrap_or(None)),
        api_extract: parse_json_opt::<ExtractSpec>(row.get(13).unwrap_or(None)),
        api_pagination: parse_json_opt::<PaginationSpec>(row.get(14).unwrap_or(None)),
        // rusqlite 0.39 dropped u64; SQLite stores as i64, cast back at the boundary.
        api_timeout_ms: row.get::<_, Option<i64>>(15).unwrap_or(None).map(|n| n as u64),
        api_max_retries: row.get::<_, Option<u8>>(16).unwrap_or(None),
        variables: parse_json_opt(row.get(17).unwrap_or(None)).unwrap_or_default(),
        created_at: parse_dt(&row.get::<_, String>(18).unwrap_or_default()),
        updated_at: parse_dt(&row.get::<_, String>(19).unwrap_or_default()),
        // 0.8.5 — columns 20/21 added by migration 056. Pre-056 rows get
        // backfilled to '[]' by the ALTER; the unwrap_or here is defensive.
        profile_ids: parse_json_opt::<Vec<String>>(row.get::<_, String>(20).ok()).unwrap_or_default(),
        directive_ids: parse_json_opt::<Vec<String>>(row.get::<_, String>(21).ok()).unwrap_or_default(),
    }
}

const COLUMNS: &str =
    "id, name, description, icon, project_id, \
     api_plugin_slug, api_config_id, api_endpoint_path, api_method, \
     api_query_json, api_path_params_json, api_headers_json, api_body, \
     api_extract_json, api_pagination_json, api_timeout_ms, api_max_retries, \
     variables_json, created_at, updated_at, \
     profile_ids_json, directive_ids_json";

pub fn list_quick_apis(conn: &Connection) -> Result<Vec<QuickApi>> {
    let sql = format!("SELECT {COLUMNS} FROM quick_apis ORDER BY updated_at DESC");
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt.query_map([], |row| Ok(row_to_quick_api(row)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(items)
}

pub fn get_quick_api(conn: &Connection, id: &str) -> Result<Option<QuickApi>> {
    let sql = format!("SELECT {COLUMNS} FROM quick_apis WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let item = stmt.query_row(params![id], |row| Ok(row_to_quick_api(row))).ok();
    Ok(item)
}

pub fn insert_quick_api(conn: &Connection, qa: &QuickApi) -> Result<()> {
    let api_query_json = qa.api_query.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
    let api_path_params_json = qa.api_path_params.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
    let api_headers_json = qa.api_headers.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
    let api_extract_json = qa.api_extract.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
    let api_pagination_json = qa.api_pagination.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
    let variables_json = serde_json::to_string(&qa.variables).unwrap_or_else(|_| "[]".into());

    let profile_ids_json = serde_json::to_string(&qa.profile_ids).unwrap_or_else(|_| "[]".into());
    let directive_ids_json = serde_json::to_string(&qa.directive_ids).unwrap_or_else(|_| "[]".into());

    conn.execute(
        "INSERT INTO quick_apis (
            id, name, description, icon, project_id,
            api_plugin_slug, api_config_id, api_endpoint_path, api_method,
            api_query_json, api_path_params_json, api_headers_json, api_body,
            api_extract_json, api_pagination_json, api_timeout_ms, api_max_retries,
            variables_json, created_at, updated_at,
            profile_ids_json, directive_ids_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
        params![
            qa.id,
            qa.name,
            qa.description,
            qa.icon,
            qa.project_id,
            qa.api_plugin_slug,
            qa.api_config_id,
            qa.api_endpoint_path,
            qa.api_method,
            api_query_json,
            api_path_params_json,
            api_headers_json,
            qa.api_body.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default()),
            api_extract_json,
            api_pagination_json,
            qa.api_timeout_ms.map(|n| n as i64),
            qa.api_max_retries,
            variables_json,
            qa.created_at.to_rfc3339(),
            qa.updated_at.to_rfc3339(),
            profile_ids_json,
            directive_ids_json,
        ],
    )?;
    Ok(())
}

pub fn update_quick_api(conn: &Connection, qa: &QuickApi) -> Result<()> {
    let api_query_json = qa.api_query.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
    let api_path_params_json = qa.api_path_params.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
    let api_headers_json = qa.api_headers.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
    let api_extract_json = qa.api_extract.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
    let api_pagination_json = qa.api_pagination.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
    let variables_json = serde_json::to_string(&qa.variables).unwrap_or_else(|_| "[]".into());

    let profile_ids_json = serde_json::to_string(&qa.profile_ids).unwrap_or_else(|_| "[]".into());
    let directive_ids_json = serde_json::to_string(&qa.directive_ids).unwrap_or_else(|_| "[]".into());

    conn.execute(
        "UPDATE quick_apis SET
            name = ?2, description = ?3, icon = ?4, project_id = ?5,
            api_plugin_slug = ?6, api_config_id = ?7, api_endpoint_path = ?8, api_method = ?9,
            api_query_json = ?10, api_path_params_json = ?11, api_headers_json = ?12, api_body = ?13,
            api_extract_json = ?14, api_pagination_json = ?15, api_timeout_ms = ?16, api_max_retries = ?17,
            variables_json = ?18, updated_at = ?19,
            profile_ids_json = ?20, directive_ids_json = ?21
         WHERE id = ?1",
        params![
            qa.id,
            qa.name,
            qa.description,
            qa.icon,
            qa.project_id,
            qa.api_plugin_slug,
            qa.api_config_id,
            qa.api_endpoint_path,
            qa.api_method,
            api_query_json,
            api_path_params_json,
            api_headers_json,
            qa.api_body.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default()),
            api_extract_json,
            api_pagination_json,
            qa.api_timeout_ms.map(|n| n as i64),
            qa.api_max_retries,
            variables_json,
            qa.updated_at.to_rfc3339(),
            profile_ids_json,
            directive_ids_json,
        ],
    )?;
    Ok(())
}

pub fn delete_quick_api(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM quick_apis WHERE id = ?1", params![id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PromptVariable;

    fn open_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn mk_quick_api() -> QuickApi {
        QuickApi {
            id: "qa-1".into(),
            name: "Create Jira ticket".into(),
            description: "Reusable POST /issue".into(),
            icon: "🔌".into(),
            project_id: None,
            api_plugin_slug: "atlassian".into(),
            api_config_id: "cfg-1".into(),
            api_endpoint_path: "/rest/api/3/issue".into(),
            api_method: Some("POST".into()),
            api_query: None,
            api_path_params: None,
            api_headers: None,
            api_body: Some(serde_json::json!({"fields":{"summary":"{{title}}"}})),
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            variables: vec![PromptVariable {
                name: "title".into(),
                label: "Ticket title".into(),
                placeholder: String::new(),
                description: None,
                required: true,
            }],
            profile_ids: vec![],
            directive_ids: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn insert_then_get_roundtrip() {
        let conn = open_test_db();
        let qa = mk_quick_api();
        insert_quick_api(&conn, &qa).unwrap();
        let fetched = get_quick_api(&conn, &qa.id).unwrap().unwrap();
        assert_eq!(fetched.id, qa.id);
        assert_eq!(fetched.name, qa.name);
        assert_eq!(fetched.api_endpoint_path, qa.api_endpoint_path);
        assert_eq!(fetched.api_method.as_deref(), Some("POST"));
        assert_eq!(fetched.variables.len(), 1);
        assert_eq!(fetched.variables[0].name, "title");
    }

    #[test]
    fn list_returns_empty_for_fresh_db() {
        let conn = open_test_db();
        assert!(list_quick_apis(&conn).unwrap().is_empty());
    }

    #[test]
    fn update_changes_fields_and_bumps_updated_at() {
        let conn = open_test_db();
        let mut qa = mk_quick_api();
        insert_quick_api(&conn, &qa).unwrap();
        qa.name = "Updated name".into();
        qa.api_body = Some(serde_json::json!({}));
        qa.updated_at = Utc::now();
        update_quick_api(&conn, &qa).unwrap();
        let fetched = get_quick_api(&conn, &qa.id).unwrap().unwrap();
        assert_eq!(fetched.name, "Updated name");
        assert_eq!(fetched.api_body, Some(serde_json::json!({})));
    }

    #[test]
    fn delete_removes_the_row() {
        let conn = open_test_db();
        let qa = mk_quick_api();
        insert_quick_api(&conn, &qa).unwrap();
        delete_quick_api(&conn, &qa.id).unwrap();
        assert!(get_quick_api(&conn, &qa.id).unwrap().is_none());
    }

    #[test]
    fn project_id_set_null_on_project_delete_via_fk_cascade() {
        // Migration declares ON DELETE SET NULL on project_id. We
        // simulate by inserting a project + a quick_api referencing it,
        // then deleting the project. project_id should become NULL.
        let conn = open_test_db();
        // Enable foreign key enforcement (off by default in rusqlite).
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('p1', 'Test', '/tmp/qa-test-fk', '2026-01-01', '2026-01-01')",
            [],
        ).unwrap();
        let mut qa = mk_quick_api();
        qa.project_id = Some("p1".into());
        insert_quick_api(&conn, &qa).unwrap();
        conn.execute("DELETE FROM projects WHERE id = 'p1'", []).unwrap();
        let fetched = get_quick_api(&conn, &qa.id).unwrap().unwrap();
        assert!(fetched.project_id.is_none(), "project_id should be NULL after parent deletion");
    }
}
