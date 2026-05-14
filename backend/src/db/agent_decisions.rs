//! DB layer for `agent_decisions` — see migration
//! `051_agent_decisions.sql` for the schema and
//! `models::agent_decisions::AgentDecision` for the Rust mirror.
//!
//! Insertion is upsert-on-conflict (UNIQUE on `run_id` + `decision_id`)
//! so re-running a workflow rewrites its own rows instead of failing.
//! That's intentional — the runner ingests the manifest on every
//! triage-step completion, including after a Goto-driven retriage.

use crate::models::AgentDecision;
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

/// Insert or replace a single decision row. Idempotent on
/// `(run_id, decision_id)` thanks to the UNIQUE constraint —
/// re-running the triage step (e.g. after a Gate `RequestChanges`)
/// rewrites the row instead of duplicating.
pub fn upsert(conn: &Connection, d: &AgentDecision) -> Result<()> {
    conn.execute(
        "INSERT INTO agent_decisions (
            id, run_id, step_name, workflow_id, project_id,
            ticket_ref, category, decision_id, what,
            chosen, options_json, why,
            placeholder, strategy, revisit_when,
            needed_from, workaround,
            gate_status, override_value, code_locations,
            created_at, resolved_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, ?7, ?8, ?9,
            ?10, ?11, ?12,
            ?13, ?14, ?15,
            ?16, ?17,
            ?18, ?19, ?20,
            ?21, ?22
         )
         ON CONFLICT(run_id, decision_id) DO UPDATE SET
            step_name = excluded.step_name,
            category = excluded.category,
            what = excluded.what,
            chosen = excluded.chosen,
            options_json = excluded.options_json,
            why = excluded.why,
            placeholder = excluded.placeholder,
            strategy = excluded.strategy,
            revisit_when = excluded.revisit_when,
            needed_from = excluded.needed_from,
            workaround = excluded.workaround,
            gate_status = excluded.gate_status,
            override_value = excluded.override_value,
            code_locations = excluded.code_locations
            -- created_at intentionally NOT overwritten on conflict
            -- so re-runs preserve the first-seen timestamp.
         ",
        params![
            d.id, d.run_id, d.step_name, d.workflow_id, d.project_id,
            d.ticket_ref, d.category, d.decision_id, d.what,
            d.chosen, d.options_json, d.why,
            d.placeholder, d.strategy, d.revisit_when,
            d.needed_from, d.workaround,
            d.gate_status, d.override_value, d.code_locations,
            d.created_at.to_rfc3339(),
            d.resolved_at.map(|t| t.to_rfc3339()),
        ],
    )?;
    Ok(())
}

/// All decisions for a run, oldest first (insertion order matches
/// manifest order — useful for rendering).
pub fn list_for_run(conn: &Connection, run_id: &str) -> Result<Vec<AgentDecision>> {
    let mut stmt = conn.prepare(
        "SELECT id, run_id, step_name, workflow_id, project_id,
                ticket_ref, category, decision_id, what,
                chosen, options_json, why,
                placeholder, strategy, revisit_when,
                needed_from, workaround,
                gate_status, override_value, code_locations,
                created_at, resolved_at
         FROM agent_decisions
         WHERE run_id = ?1
         ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![run_id], row_to_decision)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Recent decisions for a project, newest first. Drives the
/// Decision-log page.
pub fn list_recent_for_project(
    conn: &Connection,
    project_id: &str,
    limit: u32,
) -> Result<Vec<AgentDecision>> {
    let mut stmt = conn.prepare(
        "SELECT id, run_id, step_name, workflow_id, project_id,
                ticket_ref, category, decision_id, what,
                chosen, options_json, why,
                placeholder, strategy, revisit_when,
                needed_from, workaround,
                gate_status, override_value, code_locations,
                created_at, resolved_at
         FROM agent_decisions
         WHERE project_id = ?1
         ORDER BY created_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![project_id, limit], row_to_decision)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Flip a single row's `gate_status` (e.g. on Gate approval) and
/// optionally capture an override value.
pub fn update_status(
    conn: &Connection,
    id: &str,
    new_status: &str,
    override_value: Option<&str>,
) -> Result<()> {
    let resolved_at = if new_status == crate::models::agent_decisions::STATUS_RESOLVED {
        Some(Utc::now().to_rfc3339())
    } else {
        None
    };
    conn.execute(
        "UPDATE agent_decisions
         SET gate_status = ?2,
             override_value = COALESCE(?3, override_value),
             resolved_at = COALESCE(?4, resolved_at)
         WHERE id = ?1",
        params![id, new_status, override_value, resolved_at],
    )?;
    Ok(())
}

fn row_to_decision(row: &rusqlite::Row) -> rusqlite::Result<AgentDecision> {
    let created_str: String = row.get(20)?;
    let resolved_str: Option<String> = row.get(21)?;
    Ok(AgentDecision {
        id: row.get(0)?,
        run_id: row.get(1)?,
        step_name: row.get(2)?,
        workflow_id: row.get(3)?,
        project_id: row.get(4)?,
        ticket_ref: row.get(5)?,
        category: row.get(6)?,
        decision_id: row.get(7)?,
        what: row.get(8)?,
        chosen: row.get(9)?,
        options_json: row.get(10)?,
        why: row.get(11)?,
        placeholder: row.get(12)?,
        strategy: row.get(13)?,
        revisit_when: row.get(14)?,
        needed_from: row.get(15)?,
        workaround: row.get(16)?,
        gate_status: row.get(17)?,
        override_value: row.get(18)?,
        code_locations: row.get(19)?,
        created_at: parse_dt(&created_str),
        resolved_at: resolved_str.as_deref().map(parse_dt),
    })
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;
    use crate::models::agent_decisions::{
        CATEGORY_BLOCKED, CATEGORY_DECIDED, CATEGORY_MOCKED,
        STATUS_AUTO_APPROVED, STATUS_HUMAN_APPROVED, STATUS_RESOLVED,
    };

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrations::run(&conn).unwrap();
        conn
    }

    fn mk_decision(id: &str, run_id: &str, category: &str, decision_id: &str) -> AgentDecision {
        AgentDecision {
            id: id.into(),
            run_id: run_id.into(),
            step_name: "triage".into(),
            workflow_id: "wf_test".into(),
            project_id: Some("proj_test".into()),
            ticket_ref: Some("TEST-1".into()),
            category: category.into(),
            decision_id: decision_id.into(),
            what: "test entry".into(),
            chosen: if category == CATEGORY_DECIDED { Some("optA".into()) } else { None },
            options_json: None,
            why: if category == CATEGORY_DECIDED { Some("test why".into()) } else { None },
            placeholder: if category == CATEGORY_MOCKED { Some("env var".into()) } else { None },
            strategy: None,
            revisit_when: None,
            needed_from: if category == CATEGORY_BLOCKED { Some("data team".into()) } else { None },
            workaround: None,
            gate_status: "pending".into(),
            override_value: None,
            code_locations: None,
            created_at: Utc::now(),
            resolved_at: None,
        }
    }

    #[test]
    fn upsert_and_list_for_run() {
        let conn = fresh_conn();
        let d1 = mk_decision("d1", "run_a", CATEGORY_DECIDED, "brand-impl");
        let d2 = mk_decision("d2", "run_a", CATEGORY_MOCKED, "adobe-dtm");
        let d3 = mk_decision("d3", "run_b", CATEGORY_BLOCKED, "visitor-ns");
        upsert(&conn, &d1).unwrap();
        upsert(&conn, &d2).unwrap();
        upsert(&conn, &d3).unwrap();

        let list_a = list_for_run(&conn, "run_a").unwrap();
        assert_eq!(list_a.len(), 2);
        assert!(list_a.iter().any(|d| d.decision_id == "brand-impl"));
        assert!(list_a.iter().any(|d| d.decision_id == "adobe-dtm"));

        let list_b = list_for_run(&conn, "run_b").unwrap();
        assert_eq!(list_b.len(), 1);
        assert_eq!(list_b[0].decision_id, "visitor-ns");
    }

    #[test]
    fn upsert_replaces_on_conflict() {
        let conn = fresh_conn();
        let mut d = mk_decision("d1", "run_a", CATEGORY_DECIDED, "brand-impl");
        upsert(&conn, &d).unwrap();

        // Same run_id + decision_id, different chosen value → row is
        // updated, not duplicated.
        d.id = "d1-bis".into(); // even with a different PK
        d.chosen = Some("optB".into());
        upsert(&conn, &d).unwrap();

        let list = list_for_run(&conn, "run_a").unwrap();
        assert_eq!(list.len(), 1, "expected 1 row after upsert, got {}", list.len());
        assert_eq!(list[0].chosen.as_deref(), Some("optB"));
    }

    #[test]
    fn list_recent_for_project_orders_by_recency() {
        let conn = fresh_conn();
        let mut older = mk_decision("d1", "run_a", CATEGORY_DECIDED, "x");
        older.created_at = Utc::now() - chrono::Duration::hours(2);
        let newer = mk_decision("d2", "run_b", CATEGORY_DECIDED, "y");
        upsert(&conn, &older).unwrap();
        upsert(&conn, &newer).unwrap();

        let list = list_recent_for_project(&conn, "proj_test", 10).unwrap();
        assert_eq!(list.len(), 2);
        // Newer first.
        assert_eq!(list[0].decision_id, "y");
        assert_eq!(list[1].decision_id, "x");
    }

    #[test]
    fn update_status_flips_and_stamps_resolved_at() {
        let conn = fresh_conn();
        let d = mk_decision("d1", "run_a", CATEGORY_MOCKED, "adobe-dtm");
        upsert(&conn, &d).unwrap();

        // auto_approved → no resolved_at
        update_status(&conn, "d1", STATUS_AUTO_APPROVED, None).unwrap();
        let after = list_for_run(&conn, "run_a").unwrap();
        assert_eq!(after[0].gate_status, STATUS_AUTO_APPROVED);
        assert!(after[0].resolved_at.is_none());

        // resolved → stamps resolved_at
        update_status(&conn, "d1", STATUS_RESOLVED, None).unwrap();
        let after = list_for_run(&conn, "run_a").unwrap();
        assert_eq!(after[0].gate_status, STATUS_RESOLVED);
        assert!(after[0].resolved_at.is_some());
    }

    #[test]
    fn update_status_captures_override_value() {
        let conn = fresh_conn();
        let d = mk_decision("d1", "run_a", CATEGORY_DECIDED, "brand-impl");
        upsert(&conn, &d).unwrap();

        update_status(&conn, "d1", STATUS_HUMAN_APPROVED, Some("optB-overridden")).unwrap();
        let after = list_for_run(&conn, "run_a").unwrap();
        assert_eq!(after[0].override_value.as_deref(), Some("optB-overridden"));
    }
}
