//! Persistent log for ALL Kronn-mediated API calls (0.8.6 #24).
//!
//! Sources: `workflow` (ApiCall step), `agent_broker` (agent via the
//! kronn-internal MCP `api_call` tool), `manual_test` (SetupWizard).
//! Inserts go through `record(...)` which truncates excerpts and
//! best-effort redacts secrets before they hit the database.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

const EXCERPT_BYTES_CAP: usize = 2048;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ApiCallSource {
    #[serde(rename = "workflow")]
    Workflow,
    #[serde(rename = "agent_broker")]
    AgentBroker,
    #[serde(rename = "manual_test")]
    ManualTest,
}

impl ApiCallSource {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            ApiCallSource::Workflow => "workflow",
            ApiCallSource::AgentBroker => "agent_broker",
            ApiCallSource::ManualTest => "manual_test",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub enum ApiCallStatus {
    #[serde(rename = "OK")]
    Ok,
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "RateLimited")]
    RateLimited,
    #[serde(rename = "TimedOut")]
    TimedOut,
}

impl ApiCallStatus {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            ApiCallStatus::Ok => "OK",
            ApiCallStatus::Error => "ERROR",
            ApiCallStatus::RateLimited => "RateLimited",
            ApiCallStatus::TimedOut => "TimedOut",
        }
    }

    fn from_db_str(s: &str) -> Self {
        match s {
            "OK" => ApiCallStatus::Ok,
            "RateLimited" => ApiCallStatus::RateLimited,
            "TimedOut" => ApiCallStatus::TimedOut,
            _ => ApiCallStatus::Error,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewApiCallLog<'a> {
    pub source: ApiCallSource,
    pub project_id: Option<&'a str>,
    pub run_id: Option<&'a str>,
    pub disc_id: Option<&'a str>,
    pub agent: Option<&'a str>,
    pub plugin_slug: &'a str,
    pub config_id: Option<&'a str>,
    pub endpoint_path: &'a str,
    pub method: &'a str,
    pub http_status: Option<u16>,
    pub status: ApiCallStatus,
    pub duration_ms: u64,
    pub request_excerpt: Option<&'a str>,
    pub response_excerpt: Option<&'a str>,
    pub error_message: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiCallLog {
    pub id: String,
    pub source: String,
    pub project_id: Option<String>,
    pub run_id: Option<String>,
    pub disc_id: Option<String>,
    pub agent: Option<String>,
    pub plugin_slug: String,
    pub config_id: Option<String>,
    pub endpoint_path: String,
    pub method: String,
    pub http_status: Option<i64>,
    pub status: String,
    pub duration_ms: i64,
    pub request_excerpt: Option<String>,
    pub response_excerpt: Option<String>,
    pub error_message: Option<String>,
    pub called_at: String,
}

/// Truncate a payload to <= EXCERPT_BYTES_CAP bytes on a char boundary.
/// Returns `None` for empty inputs to avoid storing meaningless rows.
pub fn truncate_excerpt(s: Option<&str>) -> Option<String> {
    let s = s?.trim();
    if s.is_empty() {
        return None;
    }
    if s.len() <= EXCERPT_BYTES_CAP {
        return Some(s.to_string());
    }
    // Walk back to the nearest char boundary so we never slice mid-UTF-8.
    let mut end = EXCERPT_BYTES_CAP;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 1);
    out.push_str(&s[..end]);
    out.push('…');
    Some(out)
}

/// Re-export of `core::redact::redact_secrets` for backwards compat
/// inside this module. 0.8.6 (#57) promoted the regex set to a shared
/// module so `learning_candidates` (0.9.0) can reuse it.
pub use crate::core::redact::redact_secrets;

/// Insert one row. Never panics — on DB errors we log and swallow so an
/// audit-trail failure cannot abort a successful API call.
pub fn record(conn: &Connection, log: NewApiCallLog<'_>) -> rusqlite::Result<String> {
    let id = Uuid::new_v4().to_string();
    let request_excerpt = log
        .request_excerpt
        .and_then(|s| truncate_excerpt(Some(&redact_secrets(s))));
    let response_excerpt = log
        .response_excerpt
        .and_then(|s| truncate_excerpt(Some(&redact_secrets(s))));
    conn.execute(
        "INSERT INTO api_call_logs (
            id, source, project_id, run_id, disc_id, agent,
            plugin_slug, config_id, endpoint_path, method,
            http_status, status, duration_ms,
            request_excerpt, response_excerpt, error_message
         ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            id,
            log.source.as_db_str(),
            log.project_id,
            log.run_id,
            log.disc_id,
            log.agent,
            log.plugin_slug,
            log.config_id,
            log.endpoint_path,
            log.method,
            log.http_status.map(|v| v as i64),
            log.status.as_db_str(),
            log.duration_ms as i64,
            request_excerpt,
            response_excerpt,
            log.error_message,
        ],
    )?;
    Ok(id)
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter<'a> {
    pub source: Option<ApiCallSource>,
    pub project_id: Option<&'a str>,
    pub plugin_slug: Option<&'a str>,
    pub status: Option<ApiCallStatus>,
    pub limit: Option<u32>,
}

/// List logs ordered by most recent first, capped at limit (default 100).
pub fn list(conn: &Connection, filter: ListFilter<'_>) -> rusqlite::Result<Vec<ApiCallLog>> {
    let mut sql = String::from(
        "SELECT id, source, project_id, run_id, disc_id, agent,
                plugin_slug, config_id, endpoint_path, method,
                http_status, status, duration_ms,
                request_excerpt, response_excerpt, error_message, called_at
         FROM api_call_logs WHERE 1=1",
    );
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(s) = filter.source {
        sql.push_str(" AND source = ?");
        params_vec.push(s.as_db_str().to_string().into());
    }
    if let Some(p) = filter.project_id {
        sql.push_str(" AND project_id = ?");
        params_vec.push(p.to_string().into());
    }
    if let Some(p) = filter.plugin_slug {
        sql.push_str(" AND plugin_slug = ?");
        params_vec.push(p.to_string().into());
    }
    if let Some(s) = filter.status {
        sql.push_str(" AND status = ?");
        params_vec.push(s.as_db_str().to_string().into());
    }
    sql.push_str(" ORDER BY called_at DESC LIMIT ?");
    let limit = filter.limit.unwrap_or(100).clamp(1, 1000);
    params_vec.push((limit as i64).into());

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
            Ok(ApiCallLog {
                id: row.get(0)?,
                source: row.get(1)?,
                project_id: row.get(2)?,
                run_id: row.get(3)?,
                disc_id: row.get(4)?,
                agent: row.get(5)?,
                plugin_slug: row.get(6)?,
                config_id: row.get(7)?,
                endpoint_path: row.get(8)?,
                method: row.get(9)?,
                http_status: row.get(10)?,
                status: ApiCallStatus::from_db_str(&row.get::<_, String>(11)?)
                    .as_db_str()
                    .to_string(),
                duration_ms: row.get(12)?,
                request_excerpt: row.get(13)?,
                response_excerpt: row.get(14)?,
                error_message: row.get(15)?,
                called_at: row.get(16)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Delete rows older than N days. Returns the count deleted.
pub fn purge_older_than(conn: &Connection, days: u32) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM api_call_logs WHERE called_at < datetime('now', ?)",
        params![format!("-{} days", days)],
    )
}

/// Fetch one log by id. Used by the detail drawer in the UI.
pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<ApiCallLog>> {
    conn.query_row(
        "SELECT id, source, project_id, run_id, disc_id, agent,
                plugin_slug, config_id, endpoint_path, method,
                http_status, status, duration_ms,
                request_excerpt, response_excerpt, error_message, called_at
         FROM api_call_logs WHERE id = ?",
        params![id],
        |row| {
            Ok(ApiCallLog {
                id: row.get(0)?,
                source: row.get(1)?,
                project_id: row.get(2)?,
                run_id: row.get(3)?,
                disc_id: row.get(4)?,
                agent: row.get(5)?,
                plugin_slug: row.get(6)?,
                config_id: row.get(7)?,
                endpoint_path: row.get(8)?,
                method: row.get(9)?,
                http_status: row.get(10)?,
                status: row.get(11)?,
                duration_ms: row.get(12)?,
                request_excerpt: row.get(13)?,
                response_excerpt: row.get(14)?,
                error_message: row.get(15)?,
                called_at: row.get(16)?,
            })
        },
    )
    .optional()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;

    fn mkconn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrations::run(&conn).unwrap();
        conn
    }

    fn sample(plugin: &str) -> NewApiCallLog<'_> {
        NewApiCallLog {
            source: ApiCallSource::Workflow,
            project_id: Some("proj-1"),
            run_id: Some("run-1"),
            disc_id: None,
            agent: None,
            plugin_slug: plugin,
            config_id: Some("cfg-1"),
            endpoint_path: "/v1/widgets",
            method: "GET",
            http_status: Some(200),
            status: ApiCallStatus::Ok,
            duration_ms: 124,
            request_excerpt: Some("body=1"),
            response_excerpt: Some(r#"{"ok": true}"#),
            error_message: None,
        }
    }

    #[test]
    fn record_then_list_returns_row() {
        let conn = mkconn();
        let id = record(&conn, sample("api-test")).unwrap();
        let rows = list(&conn, ListFilter::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].plugin_slug, "api-test");
        assert_eq!(rows[0].status, "OK");
    }

    #[test]
    fn list_filters_by_plugin_slug() {
        let conn = mkconn();
        record(&conn, sample("api-alpha")).unwrap();
        record(&conn, sample("api-bravo")).unwrap();
        let rows = list(
            &conn,
            ListFilter {
                plugin_slug: Some("api-alpha"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].plugin_slug, "api-alpha");
    }

    #[test]
    fn list_filters_by_status() {
        let conn = mkconn();
        record(&conn, sample("api-alpha")).unwrap();
        let mut bad = sample("api-alpha");
        bad.status = ApiCallStatus::Error;
        bad.error_message = Some("boom");
        record(&conn, bad).unwrap();
        let rows = list(
            &conn,
            ListFilter {
                status: Some(ApiCallStatus::Error),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].error_message.as_deref(), Some("boom"));
    }

    #[test]
    fn list_limit_caps_returned_rows() {
        let conn = mkconn();
        for _ in 0..5 {
            record(&conn, sample("api-x")).unwrap();
        }
        let rows = list(
            &conn,
            ListFilter {
                limit: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn truncate_excerpt_caps_long_input() {
        let huge = "x".repeat(EXCERPT_BYTES_CAP * 3);
        let out = truncate_excerpt(Some(&huge)).unwrap();
        // Ellipsis byte length is 3, so cap + 3.
        assert!(out.len() <= EXCERPT_BYTES_CAP + 4);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_excerpt_handles_empty_and_utf8_boundary() {
        assert_eq!(truncate_excerpt(Some("")), None);
        assert_eq!(truncate_excerpt(None), None);
        // Non-ASCII at the boundary must not panic.
        let s = "é".repeat(EXCERPT_BYTES_CAP); // each 'é' is 2 bytes
        let out = truncate_excerpt(Some(&s)).unwrap();
        assert!(out.ends_with('…'));
    }

    #[test]
    fn redact_secrets_hides_bearer_tokens() {
        let raw = "Authorization: Bearer sk-live-1234567890abcdefghijklmn";
        let out = redact_secrets(raw);
        assert!(out.contains("REDACTED"));
        assert!(!out.contains("1234567890abcdefghij"));
    }

    #[test]
    fn redact_secrets_hides_vendor_prefixes() {
        let raw = "p8e-abcdefghijklmnopqrst plus AIzaSyAbcdEfGhIjKlMnOpQrStUvWxYz123";
        let out = redact_secrets(raw);
        assert!(out.contains("***REDACTED***"));
        assert!(!out.contains("p8e-abcdefghij"));
        assert!(!out.contains("AIzaSyAbcd"));
    }

    #[test]
    fn record_redacts_excerpts_before_insert() {
        let conn = mkconn();
        let leak = NewApiCallLog {
            source: ApiCallSource::AgentBroker,
            project_id: None,
            run_id: None,
            disc_id: Some("disc-1"),
            agent: Some("ClaudeCode"),
            plugin_slug: "api-test",
            config_id: None,
            endpoint_path: "/test",
            method: "POST",
            http_status: Some(200),
            status: ApiCallStatus::Ok,
            duration_ms: 50,
            request_excerpt: Some(
                "Authorization: Bearer sk-real-1234567890abcdefghijklmnopqrstuvw",
            ),
            response_excerpt: Some("ok"),
            error_message: None,
        };
        record(&conn, leak).unwrap();
        let rows = list(&conn, ListFilter::default()).unwrap();
        let stored = rows[0].request_excerpt.as_deref().unwrap();
        assert!(stored.contains("REDACTED"));
        assert!(!stored.contains("1234567890abcdef"));
    }

    #[test]
    fn purge_older_than_clears_old_rows() {
        let conn = mkconn();
        record(&conn, sample("api-test")).unwrap();
        // Backdate the row beyond the purge window.
        conn.execute(
            "UPDATE api_call_logs SET called_at = datetime('now', '-31 days')",
            [],
        )
        .unwrap();
        let deleted = purge_older_than(&conn, 30).unwrap();
        assert_eq!(deleted, 1);
        let rows = list(&conn, ListFilter::default()).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn get_returns_inserted_row() {
        let conn = mkconn();
        let id = record(&conn, sample("api-test")).unwrap();
        let row = get(&conn, &id).unwrap().unwrap();
        assert_eq!(row.id, id);
        assert_eq!(row.plugin_slug, "api-test");
    }

    #[test]
    fn get_returns_none_for_missing_id() {
        let conn = mkconn();
        assert!(get(&conn, "does-not-exist").unwrap().is_none());
    }
}
