//! KT-1100 — whether a successful run changed anything.
//!
//! A run is `NoOp` only when every step either declared that it changed
//! nothing or provably could not change anything, and the database holds no
//! trace of an effect. Anything unknown, ambiguous or unparseable reads as
//! `Changed`: a run with an effect must never be purged as a no-op.
//!
//! Declarations a step can make:
//! - Exec: a line that is exactly `KRONN_NOOP` on stdout or stderr (exit 0);
//! - Agent: `"no_change": true` at the top of its `---STEP_OUTPUT---` envelope.
//!
//! What Kronn infers on its own: a `PublishPageData` whose content did not
//! change, an `ApiCall` (or Collect Quick API source) that was a GET or HEAD,
//! and the pure data steps (`JsonData`, `TransformData`).
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::db::run_retention::REFERENCING_COLUMNS;
use crate::models::{RunStatus, StepResult, WorkflowRun, WorkflowRunOutcome};
use crate::AppState;

/// The line an Exec step prints to declare that it changed nothing.
pub const NOOP_MARKER: &str = "KRONN_NOOP";

/// Characters of each step output a no-op run keeps; the rest goes at once.
pub const NO_OP_OUTPUT_CHARS: i64 = 2000;

/// Appended to a step output shortened because its run changed nothing.
pub const NO_OP_OUTPUT_SUFFIX: &str = "\n[… output shortened: run without changes]";

/// Only top-level linear runs are classified: a child's outputs are read by
/// its parent, and batch or compare runs carry ratings.
fn is_candidate(run: &WorkflowRun) -> bool {
    run.status == RunStatus::Success
        && run.run_type == "linear"
        && run.parent_run_id.is_none()
        && run.outcome.is_none()
}

/// The outcome the steps and run fields alone allow. `Changed` is final;
/// `NoOp` still needs [`has_recorded_effect`] to be false.
pub fn classify_steps(run: &WorkflowRun) -> WorkflowRunOutcome {
    let neutral = run.status == RunStatus::Success
        // A worktree means code work; a preserved branch holds commits.
        && run.workspace_path.is_none()
        && run.produced_branches.is_empty()
        && !run.step_results.is_empty()
        && run.step_results.iter().all(step_is_neutral);
    if neutral {
        WorkflowRunOutcome::NoOp
    } else {
        WorkflowRunOutcome::Changed
    }
}

fn step_is_neutral(step: &StepResult) -> bool {
    if step.status != RunStatus::Success
        || step.is_rollback
        || step.child_run_id.is_some()
        || step.quota_wait.is_some()
        || step.terminal_stop.is_some()
    {
        return false;
    }
    let Some(kind) = step.step_kind.as_deref() else {
        return false;
    };
    let envelope = delimited_envelope(&step.output);
    let envelope = envelope.as_ref();
    let neutral = match kind {
        "Agent" => envelope.is_some_and(|e| e.get("no_change") == Some(&Value::Bool(true))),
        "Exec" => envelope.is_some_and(exec_declares_no_op),
        "ApiCall" => envelope.is_some_and(|e| read_only_summary(e.get("summary"))),
        "CollectApiData" => envelope.is_some_and(collect_reads_only),
        "PublishPageData" => envelope.is_some_and(publish_unchanged),
        "JsonData" | "TransformData" => true,
        // A board read touches no task; every other operation writes one.
        "TaskBoard" => envelope.is_some_and(task_board_read_only),
        // DelegateSubtasks and any later step type: an effect is assumed.
        _ => false,
    };
    // Only an Agent spends tokens, and only after declaring no change.
    neutral && (kind == "Agent" || step.tokens_used == Some(0))
}

/// The JSON between the first `---STEP_OUTPUT---` and the last
/// `---END_STEP_OUTPUT---`, the shape every Kronn step writes. No fallback:
/// an output this cannot read declares nothing.
fn delimited_envelope(output: &str) -> Option<Value> {
    const START: &str = "---STEP_OUTPUT---";
    let start = output.find(START)? + START.len();
    let end = output.rfind("---END_STEP_OUTPUT---")?;
    let json = output.get(start..end)?.trim();
    serde_json::from_str::<Value>(json)
        .ok()
        .filter(Value::is_object)
}

fn exec_declares_no_op(envelope: &Value) -> bool {
    let data = &envelope["data"];
    if data["exit_code"] != 0 {
        return false;
    }
    ["stdout", "stderr"].iter().any(|stream| {
        data[*stream]
            .as_str()
            .is_some_and(|text| text.lines().any(|line| line.trim() == NOOP_MARKER))
    })
}

/// ApiCall summaries start with the method actually sent (`GET https://…`).
fn read_only_summary(summary: Option<&Value>) -> bool {
    summary
        .and_then(Value::as_str)
        .is_some_and(|text| text.starts_with("GET ") || text.starts_with("HEAD "))
}

/// Every source must be a Quick API read; a CLI source can do anything.
fn collect_reads_only(envelope: &Value) -> bool {
    envelope["status"] == "OK"
        && envelope["data"]["meta"]["sources"]
            .as_array()
            .is_some_and(|sources| {
                !sources.is_empty()
                    && sources.iter().all(|source| {
                        source["kind"] == "quick_api"
                            && source["error"].is_null()
                            && read_only_summary(source.get("summary"))
                    })
            })
}

/// A `read` returns `task: null`; add, toggle, move, edit and discuss name it.
fn task_board_read_only(envelope: &Value) -> bool {
    envelope["status"] == "OK"
        && envelope["data"].get("task") == Some(&Value::Null)
        && envelope["data"]["rows"].is_array()
}

fn publish_unchanged(envelope: &Value) -> bool {
    let data = &envelope["data"];
    data["content_changed"] == Value::Bool(false)
        && data["changed_datasets"]
            .as_array()
            .is_some_and(|changed| changed.is_empty())
        && data["points_added"] == 0
        && data["points_removed"] == 0
}

/// Whether the database records an effect of `run_id`: a dataset point or a
/// publication that changed content, a non-GET call, or any other row that
/// points at the run (a discussion, a child run, a question, a room message…).
pub fn has_recorded_effect(conn: &Connection, run_id: &str) -> Result<bool> {
    let mut checks = vec![
        "SELECT 1 FROM live_page_publications WHERE workflow_run_id = ?1
            AND (points_added > 0 OR points_removed > 0
                 OR COALESCE(changed_datasets_json, '[]') <> '[]')"
            .to_string(),
        "SELECT 1 FROM api_call_logs WHERE run_id = ?1
            AND UPPER(TRIM(method)) NOT IN ('GET', 'HEAD')"
            .to_string(),
    ];
    for (table, column) in REFERENCING_COLUMNS {
        // An unchanged publication is the inferred no-op itself.
        if *table == "live_page_publications" {
            continue;
        }
        checks.push(format!("SELECT 1 FROM {table} WHERE {column} = ?1"));
    }
    for sql in checks {
        let found = conn
            .query_row(&format!("{sql} LIMIT 1"), params![run_id], |_| Ok(()))
            .optional()?;
        if found.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The outcome of a finished run, from its steps and the database.
pub fn classify(conn: &Connection, run: &WorkflowRun) -> Result<WorkflowRunOutcome> {
    if classify_steps(run) == WorkflowRunOutcome::Changed || has_recorded_effect(conn, &run.id)? {
        return Ok(WorkflowRunOutcome::Changed);
    }
    Ok(WorkflowRunOutcome::NoOp)
}

/// Store the outcome of a successful run, once. A no-op run keeps only the
/// head of each step output: every step succeeded, so no error is cut.
pub fn store(conn: &Connection, run_id: &str, outcome: WorkflowRunOutcome) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE workflow_runs
            SET outcome = ?2,
                step_results_json = CASE
                    WHEN ?2 = 'no_op' AND json_valid(step_results_json)
                    THEN (SELECT json_group_array(
                              CASE WHEN json_extract(value, '$.status') = 'Success'
                                    AND length(COALESCE(json_extract(value, '$.output'), '')) > ?3
                                   THEN json_set(value, '$.output',
                                        substr(json_extract(value, '$.output'), 1, ?3) || ?4)
                                   ELSE json(value) END)
                            FROM json_each(step_results_json))
                    ELSE step_results_json END
          WHERE id = ?1 AND status = 'Success' AND outcome IS NULL",
        params![
            run_id,
            outcome.as_db_str(),
            NO_OP_OUTPUT_CHARS,
            NO_OP_OUTPUT_SUFFIX
        ],
    )?;
    Ok(changed > 0)
}

/// Classify a run that just finished and store the result. Best effort: a
/// failure leaves the run unclassified, which the purge treats as an effect.
pub async fn record(state: &AppState, run: &mut WorkflowRun) {
    if !is_candidate(run) {
        return;
    }
    let snapshot = run.clone();
    let stored = state
        .db
        .with_conn(move |conn| {
            let outcome = classify(conn, &snapshot)?;
            Ok(store(conn, &snapshot.id, outcome)?.then_some(outcome))
        })
        .await;
    match stored {
        Ok(Some(outcome)) => {
            run.outcome = Some(outcome);
            if outcome == WorkflowRunOutcome::NoOp {
                // The list folds this run, so it must refresh past the final event.
                let _ = state
                    .ws_broadcast
                    .send(crate::models::WsMessage::WorkflowRunUpdated {
                        run_id: run.id.clone(),
                        workflow_id: run.workflow_id.clone(),
                        status: format!("{:?}", run.status),
                        step_index: run.step_results.len() as i32 - 1,
                        total_steps: run.step_results.len() as u32,
                        current_step: None,
                    });
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(run_id = %run.id, "run outcome not recorded: {error}");
        }
    }
}

#[cfg(test)]
mod tests;
