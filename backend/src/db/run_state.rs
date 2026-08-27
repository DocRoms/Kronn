//! 0.11.0 (KT-317) — the shared run-state invariant, one home for two engines.
//!
//! ADR-002 §2 mitigation: extract the sticky-write CAS predicate into a small
//! primitive both the workflow engine and the TaskExecution aggregate can call,
//! so the *invariant* has one home even though there are two aggregates. This is
//! deliberately minimal — it does NOT rewire the existing `db/workflows.rs`
//! runner (that behavioural refactor is out of KT-317 scope); `db/orchestration.rs`
//! is its first caller, and workflows can adopt it later.
//!
//! Boot reconcile is intentionally NOT a primitive here: flipping a row to
//! `Interrupted` must also journal the transition and preserve its origin
//! checkpoint (ADR §3, DoD-3), which is aggregate-specific. A bulk status UPDATE
//! that erased the origin and skipped the journal is exactly the anti-pattern the
//! orchestration reconcile avoids — see `db::orchestration::reconcile_stale_task_executions`.
//!
//! The `table` argument is always a hard-coded `&'static str` from Kronn's own
//! code — never user input — and is checked against an allow-list before it is
//! interpolated, so there is no injection surface.

use anyhow::{bail, Result};
use rusqlite::{params, Connection};

/// Tables that expose a string `status` column plus `id`/`updated_at` and obey
/// the sticky-transition contract. Interpolating any other name is a bug.
const RUN_STATE_TABLES: &[&str] = &["task_executions"];

fn assert_known_table(table: &str) -> Result<()> {
    if !RUN_STATE_TABLES.contains(&table) {
        bail!("run_state: refusing to operate on unregistered table `{table}`");
    }
    Ok(())
}

/// Compare-and-swap a row's status: `UPDATE … SET status=to WHERE id=? AND
/// status=from`. Returns `true` iff exactly one row moved. A caller treats
/// `false` as "the row changed hands beneath us" (raced, already terminal, or
/// gone) and stops — the same semantics as `workflows::claim_run_status`.
///
/// This is the terminal-lock in one line: a terminal row never matches a
/// non-terminal `from`, so a zombie writer cannot resurrect it.
pub fn claim_status(
    conn: &Connection,
    table: &'static str,
    id: &str,
    from_status: &str,
    to_status: &str,
    now_rfc3339: &str,
) -> Result<bool> {
    assert_known_table(table)?;
    let sql =
        format!("UPDATE {table} SET status = ?3, updated_at = ?4 WHERE id = ?1 AND status = ?2");
    let n = conn.execute(&sql, params![id, from_status, to_status, now_rfc3339])?;
    Ok(n == 1)
}
