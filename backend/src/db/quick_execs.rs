//! Persistence for reusable shell-free CLI collectors.

use anyhow::Result;
use rusqlite::{params, Connection};

use super::parse_dt;
use crate::models::{CollectQuickExecOutputFormat, QuickExec};

const COLUMNS: &str = "id, name, description, icon, project_id, command, args_json, \
    timeout_secs, output_format, variables_json, created_at, updated_at, pinned";

fn parse_output_format(value: &str) -> CollectQuickExecOutputFormat {
    match value {
        "text" => CollectQuickExecOutputFormat::Text,
        "lines" => CollectQuickExecOutputFormat::Lines,
        "csv" => CollectQuickExecOutputFormat::Csv,
        _ => CollectQuickExecOutputFormat::Json,
    }
}

fn output_format_name(value: CollectQuickExecOutputFormat) -> &'static str {
    match value {
        CollectQuickExecOutputFormat::Json => "json",
        CollectQuickExecOutputFormat::Text => "text",
        CollectQuickExecOutputFormat::Lines => "lines",
        CollectQuickExecOutputFormat::Csv => "csv",
    }
}

fn row_to_quick_exec(row: &rusqlite::Row<'_>) -> rusqlite::Result<QuickExec> {
    let args_json: String = row.get(6)?;
    let variables_json: String = row.get(9)?;
    let output_format: String = row.get(8)?;
    Ok(QuickExec {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        icon: row.get(3)?,
        project_id: row.get(4)?,
        command: row.get(5)?,
        args: serde_json::from_str(&args_json).unwrap_or_default(),
        timeout_secs: row.get::<_, i64>(7)?.clamp(1, 1800) as u32,
        output_format: parse_output_format(&output_format),
        variables: serde_json::from_str(&variables_json).unwrap_or_default(),
        pinned: row.get::<_, i32>(12).unwrap_or(0) != 0,
        created_at: parse_dt(row.get(10)?),
        updated_at: parse_dt(row.get(11)?),
    })
}

pub fn list_quick_execs(conn: &Connection) -> Result<Vec<QuickExec>> {
    let sql = format!("SELECT {COLUMNS} FROM quick_execs ORDER BY updated_at DESC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_quick_exec)?;
    Ok(rows.filter_map(|row| row.ok()).collect())
}

pub fn get_quick_exec(conn: &Connection, id: &str) -> Result<Option<QuickExec>> {
    let sql = format!("SELECT {COLUMNS} FROM quick_execs WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], row_to_quick_exec).ok())
}

pub fn insert_quick_exec(conn: &Connection, quick_exec: &QuickExec) -> Result<()> {
    conn.execute(
        "INSERT INTO quick_execs (
            id, name, description, icon, project_id, command, args_json,
            timeout_secs, output_format, variables_json, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            quick_exec.id,
            quick_exec.name,
            quick_exec.description,
            quick_exec.icon,
            quick_exec.project_id,
            quick_exec.command,
            serde_json::to_string(&quick_exec.args)?,
            i64::from(quick_exec.timeout_secs),
            output_format_name(quick_exec.output_format),
            serde_json::to_string(&quick_exec.variables)?,
            quick_exec.created_at.to_rfc3339(),
            quick_exec.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn update_quick_exec(conn: &Connection, quick_exec: &QuickExec) -> Result<()> {
    conn.execute(
        "UPDATE quick_execs SET
            name = ?2, description = ?3, icon = ?4, project_id = ?5,
            command = ?6, args_json = ?7, timeout_secs = ?8,
            output_format = ?9, variables_json = ?10, updated_at = ?11
         WHERE id = ?1",
        params![
            quick_exec.id,
            quick_exec.name,
            quick_exec.description,
            quick_exec.icon,
            quick_exec.project_id,
            quick_exec.command,
            serde_json::to_string(&quick_exec.args)?,
            i64::from(quick_exec.timeout_secs),
            output_format_name(quick_exec.output_format),
            serde_json::to_string(&quick_exec.variables)?,
            quick_exec.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn update_quick_exec_pinned(conn: &Connection, id: &str, pinned: bool) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE quick_execs SET pinned = ?2 WHERE id = ?1",
        params![id, pinned as i32],
    )? > 0)
}

pub fn delete_quick_exec(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM quick_execs WHERE id = ?1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn roundtrip_preserves_csv_and_literal_argv() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let item = QuickExec {
            id: "qe-1".into(),
            pinned: false,
            name: "AWS inventory".into(),
            icon: "⌘".into(),
            description: "Collect instances".into(),
            project_id: None,
            command: "aws".into(),
            args: vec![
                "ec2".into(),
                "describe-instances".into(),
                "{{region}}".into(),
            ],
            timeout_secs: 90,
            output_format: CollectQuickExecOutputFormat::Csv,
            variables: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        insert_quick_exec(&conn, &item).unwrap();
        let loaded = get_quick_exec(&conn, "qe-1").unwrap().unwrap();
        assert_eq!(loaded.args, item.args);
        assert_eq!(loaded.output_format, CollectQuickExecOutputFormat::Csv);
        assert_eq!(list_quick_execs(&conn).unwrap().len(), 1);
        delete_quick_exec(&conn, "qe-1").unwrap();
        assert!(get_quick_exec(&conn, "qe-1").unwrap().is_none());
    }
}
