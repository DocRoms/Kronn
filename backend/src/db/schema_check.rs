//! KT-1483 — boot-time check of the live schema against what the migrations build.
//!
//! `_migrations` only records that a migration ran. Nothing stops a table from
//! vanishing afterwards (a manual `DROP`, a restored partial backup, a crashed
//! tool) while its receipt stays, and the failure then only shows up on the code
//! path that writes the table. After the migrations, the expected schema (the one
//! every migration produces on a fresh in-memory database) is diffed against the
//! live one:
//!
//! - a missing table, index or trigger is recreated with `CREATE ... IF NOT
//!   EXISTS`, from the SQL SQLite itself stored for it;
//! - a missing column is added back when `ALTER TABLE ADD COLUMN` can express it;
//! - anything else is reported, loudly, in the logs and on `/api/health`.
//!
//! Recreated tables are empty: the rows that were in them are gone for good.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

/// Outcome of one schema check. Entries are human-readable, one per object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SchemaReport {
    /// Recreated by the check.
    pub repaired: Vec<String>,
    /// Missing and not repairable, or whose repair failed.
    pub unrepaired: Vec<String>,
}

impl SchemaReport {
    pub fn is_clean(&self) -> bool {
        self.repaired.is_empty() && self.unrepaired.is_empty()
    }

    /// `ok` (nothing missing), `repaired` (everything missing was recreated) or
    /// `degraded` (something is still missing).
    pub fn status(&self) -> &'static str {
        if !self.unrepaired.is_empty() {
            "degraded"
        } else if !self.repaired.is_empty() {
            "repaired"
        } else {
            "ok"
        }
    }
}

static LAST_REPORT: Mutex<Option<SchemaReport>> = Mutex::new(None);

/// Report of the last boot check of this process, `None` before it ran.
pub fn last_report() -> Option<SchemaReport> {
    LAST_REPORT.lock().ok().and_then(|report| report.clone())
}

pub(crate) fn record(report: SchemaReport) {
    if let Ok(mut slot) = LAST_REPORT.lock() {
        *slot = Some(report);
    }
}

struct SchemaObject {
    kind: String,
    name: String,
    sql: String,
}

struct SchemaColumn {
    name: String,
    decl_type: String,
    not_null: bool,
    default: Option<String>,
    primary_key: bool,
}

struct ExpectedSchema {
    /// Tables first, then indexes, then triggers, each in creation order.
    objects: Vec<SchemaObject>,
    columns: BTreeMap<String, Vec<SchemaColumn>>,
}

static EXPECTED: OnceLock<ExpectedSchema> = OnceLock::new();

fn expected_schema() -> Result<&'static ExpectedSchema> {
    if let Some(expected) = EXPECTED.get() {
        return Ok(expected);
    }
    let reference = Connection::open_in_memory().context("open the reference schema database")?;
    reference.execute_batch("PRAGMA foreign_keys=ON;")?;
    super::migrations::build_reference_schema(&reference)
        .context("build the reference schema from the migrations")?;
    let objects = read_objects(&reference)?;
    let mut columns = BTreeMap::new();
    for object in objects.iter().filter(|object| object.kind == "table") {
        columns.insert(object.name.clone(), read_columns(&reference, &object.name)?);
    }
    Ok(EXPECTED.get_or_init(|| ExpectedSchema { objects, columns }))
}

fn read_objects(conn: &Connection) -> Result<Vec<SchemaObject>> {
    let mut statement = conn.prepare(
        "SELECT type, name, sql FROM sqlite_schema \
         WHERE type IN ('table', 'index', 'trigger') \
           AND sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
         ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 ELSE 2 END, rowid",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(SchemaObject {
            kind: row.get(0)?,
            name: row.get(1)?,
            sql: row.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn read_columns(conn: &Connection, table: &str) -> Result<Vec<SchemaColumn>> {
    let mut statement =
        conn.prepare("SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?1)")?;
    let rows = statement.query_map([table], |row| {
        Ok(SchemaColumn {
            name: row.get(0)?,
            decl_type: row.get(1)?,
            not_null: row.get::<_, i64>(2)? != 0,
            default: row.get(3)?,
            primary_key: row.get::<_, i64>(4)? != 0,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// SQLite stores the statement as `CREATE <KIND> <name> ...`, whatever the
/// original spelling was; the same statement with `IF NOT EXISTS` is idempotent.
fn idempotent_create(object: &SchemaObject) -> Option<String> {
    let prefix = match object.kind.as_str() {
        "table" => "CREATE TABLE ",
        "trigger" => "CREATE TRIGGER ",
        "index" if object.sql.starts_with("CREATE UNIQUE INDEX ") => "CREATE UNIQUE INDEX ",
        "index" => "CREATE INDEX ",
        _ => return None,
    };
    let rest = object.sql.strip_prefix(prefix)?;
    Some(format!("{prefix}IF NOT EXISTS {rest}"))
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn add_column_sql(table: &str, column: &SchemaColumn) -> Option<String> {
    // SQLite refuses these: a new column cannot be a key, nor NOT NULL without
    // a default to fill the existing rows with.
    if column.primary_key || (column.not_null && column.default.is_none()) {
        return None;
    }
    let mut sql = format!(
        "ALTER TABLE {} ADD COLUMN {}",
        quote(table),
        quote(&column.name)
    );
    if !column.decl_type.is_empty() {
        sql.push(' ');
        sql.push_str(&column.decl_type);
    }
    if column.not_null {
        sql.push_str(" NOT NULL");
    }
    if let Some(default) = &column.default {
        sql.push_str(" DEFAULT ");
        sql.push_str(default);
    }
    Some(sql)
}

fn apply(conn: &Connection, sql: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(sql)?;
    tx.commit()?;
    Ok(())
}

/// Diff the live schema against the migrations' and repair what can be repaired.
/// Meant to run right after the migrations, so every receipt is in place.
pub fn verify_and_repair(conn: &Connection) -> Result<SchemaReport> {
    let expected = expected_schema()?;
    let mut report = SchemaReport::default();

    let live: BTreeSet<(String, String)> = read_objects(conn)?
        .into_iter()
        .map(|object| (object.kind, object.name))
        .collect();
    let mut recreated_tables = BTreeSet::new();

    for object in &expected.objects {
        if live.contains(&(object.kind.clone(), object.name.clone())) {
            continue;
        }
        let label = format!("{} {}", object.kind, object.name);
        let Some(sql) = idempotent_create(object) else {
            report
                .unrepaired
                .push(format!("{label}: no idempotent statement can recreate it"));
            continue;
        };
        match apply(conn, &sql) {
            Ok(()) => {
                if object.kind == "table" {
                    recreated_tables.insert(object.name.clone());
                    report
                        .repaired
                        .push(format!("{label}: recreated empty, its rows are lost"));
                } else {
                    report.repaired.push(format!("{label}: recreated"));
                }
            }
            Err(error) => report.unrepaired.push(format!("{label}: {error}")),
        }
    }

    for (table, columns) in &expected.columns {
        if recreated_tables.contains(table) {
            continue;
        }
        let present: BTreeSet<String> = read_columns(conn, table)?
            .into_iter()
            .map(|column| column.name)
            .collect();
        if present.is_empty() {
            // The table itself is missing and was already reported above.
            continue;
        }
        for column in columns.iter().filter(|c| !present.contains(&c.name)) {
            let label = format!("column {table}.{}", column.name);
            let Some(sql) = add_column_sql(table, column) else {
                report
                    .unrepaired
                    .push(format!("{label}: cannot be added back by ALTER TABLE"));
                continue;
            };
            match apply(conn, &sql) {
                Ok(()) => report.repaired.push(format!(
                    "{label}: added back; CHECK/REFERENCES clauses are not restored"
                )),
                Err(error) => report.unrepaired.push(format!("{label}: {error}")),
            }
        }
    }

    Ok(report)
}

/// Boot entry point: verify, log, and keep the report for `/api/health`. Never
/// fails the boot: a schema that cannot be checked is itself reported.
pub(crate) fn check_at_boot(conn: &Connection) {
    let report = match verify_and_repair(conn) {
        Ok(report) => report,
        Err(error) => SchemaReport {
            repaired: Vec::new(),
            unrepaired: vec![format!("schema check could not run: {error:#}")],
        },
    };
    for entry in &report.repaired {
        tracing::warn!("schema repaired at boot: {entry}");
    }
    for entry in &report.unrepaired {
        tracing::error!("schema problem at boot, NOT repaired: {entry}");
    }
    record(report);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn exists(conn: &Connection, kind: &str, name: &str) -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = ?1 AND name = ?2)",
            [kind, name],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn a_fully_migrated_database_is_clean() {
        let conn = migrated();
        let report = verify_and_repair(&conn).unwrap();
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(report.status(), "ok");
    }

    #[test]
    fn a_dropped_table_is_recreated_with_its_index() {
        let conn = migrated();
        conn.execute_batch("DROP TABLE orchestration_run_events;")
            .unwrap();
        assert!(!exists(&conn, "index", "idx_orchestration_run_events_run"));

        let report = verify_and_repair(&conn).unwrap();
        assert!(report.unrepaired.is_empty(), "{report:?}");
        assert!(report
            .repaired
            .iter()
            .any(|entry| entry.starts_with("table orchestration_run_events")));
        assert_eq!(report.status(), "repaired");
        assert!(exists(&conn, "table", "orchestration_run_events"));
        assert!(exists(&conn, "index", "idx_orchestration_run_events_run"));
        // Back to the expected shape: a second pass has nothing left to do.
        assert!(verify_and_repair(&conn).unwrap().is_clean());
    }

    #[test]
    fn a_dropped_index_and_a_dropped_trigger_are_recreated() {
        let conn = migrated();
        let trigger: String = conn
            .query_row(
                "SELECT name FROM sqlite_schema WHERE type = 'trigger' ORDER BY rowid LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute_batch(&format!(
            "DROP INDEX idx_task_executions_run_active; DROP TRIGGER {trigger};"
        ))
        .unwrap();

        let report = verify_and_repair(&conn).unwrap();
        assert!(report.unrepaired.is_empty(), "{report:?}");
        assert!(exists(&conn, "index", "idx_task_executions_run_active"));
        assert!(exists(&conn, "trigger", &trigger));
    }

    #[test]
    fn a_dropped_column_with_a_default_is_added_back() {
        let conn = migrated();
        conn.execute_batch("ALTER TABLE orchestration_runs DROP COLUMN auto_continue;")
            .unwrap();

        let report = verify_and_repair(&conn).unwrap();
        assert!(report.unrepaired.is_empty(), "{report:?}");
        assert!(report
            .repaired
            .iter()
            .any(|entry| entry.starts_with("column orchestration_runs.auto_continue")));
        let present: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('orchestration_runs') \
                 WHERE name = 'auto_continue')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(present);
    }

    #[test]
    fn a_column_that_cannot_be_added_back_is_reported_not_hidden() {
        let column = SchemaColumn {
            name: "required".into(),
            decl_type: "TEXT".into(),
            not_null: true,
            default: None,
            primary_key: false,
        };
        assert!(add_column_sql("t", &column).is_none());
    }

    #[tokio::test]
    async fn boot_repairs_a_database_that_lost_a_table_and_campaign_control_works_again() {
        use crate::models::{
            OrchestrationActor, OrchestrationControlState, OrchestrationRunInput, PlanningActorKind,
        };

        fn actor() -> OrchestrationActor {
            OrchestrationActor {
                kind: PlanningActorKind::Backend,
                id: Some("orchestrator".into()),
                session_id: None,
                source_message_id: None,
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kronn.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at) \
             VALUES ('disc-1', 'D', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let run = crate::db::orchestration::create_orchestration_run(
            &conn,
            &OrchestrationRunInput::single_task("disc-1"),
        )
        .unwrap();
        conn.execute_batch("DROP TABLE orchestration_run_events;")
            .unwrap();
        // The failure of the issue: the write path hits the missing table.
        let broken = crate::db::orchestration::set_orchestration_control_state(
            &conn,
            &run.id,
            OrchestrationControlState::Cancelled,
            Some("cancel"),
            &actor(),
        );
        assert!(format!("{:#}", broken.unwrap_err()).contains("no such table"));
        drop(conn);

        // A restart: the boot check recreates the table.
        let db = crate::db::Database::open_path(&path).unwrap();
        let report = last_report().expect("the boot check recorded a report");
        assert!(report
            .repaired
            .iter()
            .any(|entry| entry.starts_with("table orchestration_run_events")));
        assert!(report.unrepaired.is_empty(), "{report:?}");

        let run_id = run.id.clone();
        let cancelled = db
            .with_conn(move |conn| {
                crate::db::orchestration::set_orchestration_control_state(
                    conn,
                    &run_id,
                    OrchestrationControlState::Cancelled,
                    Some("cancel"),
                    &actor(),
                )
            })
            .await
            .unwrap();
        assert_eq!(
            cancelled.control_state,
            OrchestrationControlState::Cancelled
        );
    }
}
