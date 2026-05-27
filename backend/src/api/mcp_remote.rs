//! 0.8.6 phase 4 — MCP remote control routes.
//!
//! Wraps the existing UI-facing endpoints (`POST /trigger`, `GET /runs/:id`,
//! `POST /quick-prompts/:id/batch`) with two differences :
//!
//! 1. **JSON instead of SSE** — MCP wrappers can't easily consume SSE
//!    streams. These routes do the same work but return a sync JSON
//!    body containing the `run_id` / `disc_id` + the smart-polling hint.
//!
//! 2. **`next_check` smart-polling hint** — every response carries a
//!    `next_check: {wait_seconds, reason, confidence}` so a mobile agent
//!    knows when to call back without burning tokens on tight polling.
//!    Computed via [`crate::core::run_eta`] from historical averages
//!    (`workflow_runs.total_duration_ms` / `qp_versions.
//!    avg_first_agent_duration_ms`).
//!
//! ## Use case
//! Claude Code mobile linked to a PC session → MCP `kronn-internal` →
//! these endpoints. Lets an agent launch a workflow / QP from a phone
//! and track progress without opening the desktop UI.
//!
//! ## Why not SSE
//! On mobile the round-trip per SSE event is wasted tokens — the agent
//! only acts at status transitions, not on every chunk. JSON polling
//! with `next_check` matches the agent's reasoning cadence.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::api::workflows::build_manual_trigger_obj;
use crate::core::run_eta::{
    next_check_initial, next_check_polling, NextCheck,
};
use crate::models::*;
use crate::AppState;

// ─── Helpers ────────────────────────────────────────────────────────────

/// Average `total_duration_ms` over the last `LIMIT` completed (status
/// = Success | Failed | Cancelled | StoppedByGuard) runs of a workflow.
/// Returns `(avg_ms, sample_count)` ; sample_count of 0 means no history.
///
/// Used by the smart-polling hint — only `finished_at - started_at` for
/// terminal runs counts ; pending/running ones are excluded so a long
/// in-flight run can't poison the average.
const RUN_AVG_LIMIT: u32 = 10;

fn avg_workflow_duration_ms(
    runs: &[WorkflowRun],
) -> (Option<u64>, u32) {
    let completed: Vec<u64> = runs
        .iter()
        .filter(|r| {
            matches!(
                r.status,
                RunStatus::Success
                    | RunStatus::Failed
                    | RunStatus::Cancelled
                    | RunStatus::StoppedByGuard
            )
        })
        .filter_map(|r| {
            r.finished_at.map(|fin| {
                (fin - r.started_at).num_milliseconds().max(0) as u64
            })
        })
        .take(RUN_AVG_LIMIT as usize)
        .collect();
    let samples = completed.len() as u32;
    if samples == 0 {
        return (None, 0);
    }
    let total: u64 = completed.iter().sum();
    (Some(total / samples as u64), samples)
}

// ─── workflow_trigger ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct McpTriggerWorkflowRequest {
    pub workflow_id: String,
    #[serde(default)]
    pub variables: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct McpTriggerWorkflowResponse {
    pub run_id: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub status: String,
    pub started_at: chrono::DateTime<Utc>,
    /// Average duration over the last N completed runs of this workflow,
    /// in milliseconds. `None` when no completed history yet.
    pub expected_duration_ms: Option<u64>,
    /// How many completed runs the average was computed from. `0` means
    /// `next_check.confidence` is `no_baseline`.
    pub samples: u32,
    pub next_check: NextCheck,
}

/// POST /api/mcp/workflow-trigger
///
/// JSON wrapper around the existing `POST /api/workflows/:id/trigger`
/// SSE handler. Creates the run + spawns the runner exactly like the
/// UI route, but returns the run_id + smart-polling hint synchronously
/// instead of streaming events.
pub async fn workflow_trigger(
    State(state): State<AppState>,
    Json(req): Json<McpTriggerWorkflowRequest>,
) -> Json<ApiResponse<McpTriggerWorkflowResponse>> {
    let wf_id = req.workflow_id.clone();
    let wf = match state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id))
        .await
    {
        Ok(Some(wf)) => wf,
        Ok(None) => return Json(ApiResponse::err("Workflow not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    if !wf.enabled {
        return Json(ApiResponse::err(
            "Workflow is disabled — enable it in the UI before triggering",
        ));
    }

    // Required-variable validation mirrors the existing trigger route.
    for declared in &wf.variables {
        if declared.required {
            let val = req.variables.get(&declared.name).map(|s| s.trim()).unwrap_or("");
            if val.is_empty() {
                let label = if declared.label.is_empty() {
                    &declared.name
                } else {
                    &declared.label
                };
                return Json(ApiResponse::err(format!(
                    "Variable « {} » est obligatoire pour lancer ce workflow.",
                    label
                )));
            }
        }
    }
    let trigger_obj = build_manual_trigger_obj(&req.variables, Utc::now());

    // Compute the smart-polling hint BEFORE we insert the new run, so
    // the sample count reflects only history (the new pending run
    // doesn't influence its own ETA).
    let wf_id_for_avg = wf.id.clone();
    let history = state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::list_runs_paginated(
                conn,
                &wf_id_for_avg,
                Some(RUN_AVG_LIMIT),
                None,
            )
        })
        .await
        .unwrap_or_default();
    let (expected_duration_ms, samples) = avg_workflow_duration_ms(&history);
    let next_check = next_check_initial(expected_duration_ms, samples);

    let now = Utc::now();
    let run = WorkflowRun {
        id: Uuid::new_v4().to_string(),
        workflow_id: wf.id.clone(),
        status: RunStatus::Pending,
        trigger_context: Some(serde_json::Value::Object(trigger_obj)),
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: None,
        parent_run_id: None,
        state: ::std::collections::HashMap::new(),
        produced_branches: vec![],
    };

    let r = run.clone();
    let limit = wf.concurrency_limit;
    let wf_id_check = wf.id.clone();
    match state
        .db
        .with_conn(move |conn| {
            if let Some(max) = limit {
                let active = crate::db::workflows::count_active_runs(conn, &wf_id_check)?;
                if active >= max {
                    anyhow::bail!("CONCURRENCY_LIMIT:{}/{}", active, max);
                }
            }
            crate::db::workflows::insert_run(conn, &r)?;
            Ok(())
        })
        .await
    {
        Ok(()) => {}
        Err(e) => {
            let msg = e.to_string();
            if let Some(rest) = msg.strip_prefix("CONCURRENCY_LIMIT:") {
                return Json(ApiResponse::err(format!(
                    "Concurrency limit reached ({})",
                    rest
                )));
            }
            return Json(ApiResponse::err(format!("DB error: {}", msg)));
        }
    }

    tracing::info!(
        "MCP triggered workflow run {} for workflow {}",
        run.id,
        wf.name
    );

    // Background dispatch — identical to the SSE route, just without
    // an event sink. The runner persists status / step_results to the
    // run row, which the GET status route reads back.
    let state_for_run = state.clone();
    let config = state.config.clone();
    let wf_for_run = wf.clone();
    let mut run_exec = run.clone();
    tokio::spawn(async move {
        let cfg = config.read().await;
        let tokens = cfg.tokens.clone();
        let agents = cfg.agents.clone();
        drop(cfg);
        if let Err(e) = crate::workflows::runner::execute_run(
            state_for_run,
            &wf_for_run,
            &mut run_exec,
            &tokens,
            &agents,
            None,
        )
        .await
        {
            tracing::error!("Workflow run {} failed: {}", run_exec.id, e);
        }
    });

    Json(ApiResponse::ok(McpTriggerWorkflowResponse {
        run_id: run.id,
        workflow_id: wf.id,
        workflow_name: wf.name,
        status: "Pending".into(),
        started_at: now,
        expected_duration_ms,
        samples,
        next_check,
    }))
}

// ─── workflow_run_status ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct StepResultSummary {
    pub step_name: String,
    pub status: String,
    pub duration_ms: u64,
    pub tokens_used: u64,
    /// First ~200 chars of the step output (truncated for token economy).
    /// MCP callers fetch the full body via `GET /api/workflows/<id>/runs/<run_id>`
    /// only when they need it.
    pub output_excerpt: String,
    pub step_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct McpRunStatusResponse {
    pub run_id: String,
    pub workflow_id: String,
    pub status: String,
    pub started_at: chrono::DateTime<Utc>,
    pub finished_at: Option<chrono::DateTime<Utc>>,
    pub elapsed_ms: u64,
    pub current_step: Option<String>,
    pub step_count: u32,
    pub tokens_used: u64,
    pub steps: Vec<StepResultSummary>,
    /// Average duration of completed history for this workflow.
    /// Same value the trigger response carried.
    pub expected_duration_ms: Option<u64>,
    pub samples: u32,
    /// `None` when the run is terminal — no point in scheduling another
    /// poll, the caller can read `status` to decide what to do.
    pub next_check: Option<NextCheck>,
}

const OUTPUT_EXCERPT_MAX_CHARS: usize = 200;

fn excerpt(s: &str) -> String {
    if s.chars().count() <= OUTPUT_EXCERPT_MAX_CHARS {
        return s.to_string();
    }
    // Char-boundary-safe truncation (matches the `feedback_rust_str_slicing` memory).
    let mut out: String = s.chars().take(OUTPUT_EXCERPT_MAX_CHARS).collect();
    out.push('…');
    out
}

fn is_terminal_status(status: &RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Success
            | RunStatus::Failed
            | RunStatus::Cancelled
            | RunStatus::StoppedByGuard
    )
}

/// GET /api/mcp/workflow-run-status/:run_id
pub async fn workflow_run_status(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Json<ApiResponse<McpRunStatusResponse>> {
    let run_id_lookup = run_id.clone();
    let run = match state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, &run_id_lookup))
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return Json(ApiResponse::err("Run not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let workflow_id = run.workflow_id.clone();
    let history = state
        .db
        .with_conn({
            let wf_id = workflow_id.clone();
            move |conn| {
                crate::db::workflows::list_runs_paginated(
                    conn,
                    &wf_id,
                    Some(RUN_AVG_LIMIT + 1),
                    None,
                )
            }
        })
        .await
        .unwrap_or_default();
    // Exclude the current run from the history — its in-flight state
    // would skew the average if it's a long-running one.
    let history: Vec<WorkflowRun> = history.into_iter().filter(|r| r.id != run.id).collect();
    let (expected_duration_ms, samples) = avg_workflow_duration_ms(&history);

    let now = Utc::now();
    let end = run.finished_at.unwrap_or(now);
    let elapsed_ms = (end - run.started_at).num_milliseconds().max(0) as u64;

    let next_check = if is_terminal_status(&run.status) {
        None
    } else {
        Some(next_check_polling(expected_duration_ms, elapsed_ms, samples))
    };

    let current_step = run.step_results.last().and_then(|s| {
        if matches!(s.status, RunStatus::Running | RunStatus::Pending) {
            Some(s.step_name.clone())
        } else {
            None
        }
    });

    let steps: Vec<StepResultSummary> = run
        .step_results
        .iter()
        .map(|s| StepResultSummary {
            step_name: s.step_name.clone(),
            status: format!("{:?}", s.status),
            duration_ms: s.duration_ms,
            tokens_used: s.tokens_used,
            output_excerpt: excerpt(&s.output),
            step_type: s.step_kind.clone(),
        })
        .collect();

    let step_count = steps.len() as u32;

    Json(ApiResponse::ok(McpRunStatusResponse {
        run_id: run.id,
        workflow_id,
        status: format!("{:?}", run.status),
        started_at: run.started_at,
        finished_at: run.finished_at,
        elapsed_ms,
        current_step,
        step_count,
        tokens_used: run.tokens_used,
        steps,
        expected_duration_ms,
        samples,
        next_check,
    }))
}

// ─── qp_run ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct McpQpRunRequest {
    pub qp_id: String,
    /// Variable values for the QP's `{{var}}` placeholders. Missing
    /// keys for required variables → 400 like the existing batch path.
    #[serde(default)]
    pub vars: HashMap<String, String>,
    /// Optional agent override — defaults to the QP's declared agent.
    #[serde(default)]
    pub agent: Option<AgentType>,
    /// Optional project override — defaults to the QP's project_id, or
    /// the disc-resolved project_id if the agent is mid-conversation.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Optional discussion title prefix. Defaults to the QP name +
    /// " — MCP run".
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct McpQpRunResponse {
    pub disc_id: String,
    pub qp_id: String,
    pub qp_name: String,
    pub agent: String,
    /// Average first-agent-reply duration across all prior launches of
    /// this QP (from `qp_versions` metrics). `None` when no history.
    pub expected_duration_ms: Option<u64>,
    pub samples: u32,
    pub next_check: NextCheck,
}

/// Render a QP template substituting `{{var}}` placeholders. Mirrors
/// the front-end's `renderTemplate` so the server-side path produces
/// the same prompt the UI would have sent.
fn render_qp_template(
    template: &str,
    vars: &HashMap<String, String>,
) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        let placeholder = format!("{{{{{}}}}}", k);
        out = out.replace(&placeholder, v);
    }
    out
}

/// POST /api/mcp/qp-run
///
/// One-shot Quick Prompt launch :
///   1. Loads the QP, renders the template with the agent-supplied vars.
///   2. Creates a single-item batch (= 1 disc) via `create_batch_run`.
///   3. Returns disc_id + ETA hint. The agent then `disc_load_other`s
///      to read the result once `next_check.wait_seconds` elapsed.
///
/// **Note on agent kickoff** : the actual agent run is NOT started by
/// this route — mirrors the existing `/quick-prompts/:id/batch` contract.
/// The MCP wrapper triggers `POST /api/discussions/:id/run` immediately
/// after this call returns (fire-and-forget — the backend spawns the
/// agent task in tokio, the wrapper doesn't need to wait for SSE).
pub async fn qp_run(
    State(state): State<AppState>,
    Json(req): Json<McpQpRunRequest>,
) -> Json<ApiResponse<McpQpRunResponse>> {
    if req.qp_id.is_empty() {
        return Json(ApiResponse::err("qp_id is required"));
    }

    let qp_lookup = req.qp_id.clone();
    let qp_loaded = state
        .db
        .with_conn(move |conn| crate::db::quick_prompts::get_quick_prompt(conn, &qp_lookup))
        .await;
    let mut qp = match qp_loaded {
        Ok(Some(q)) => q,
        Ok(None) => return Json(ApiResponse::err("Quick prompt not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    // Validate required vars
    for declared in &qp.variables {
        if declared.required {
            let val = req.vars.get(&declared.name).map(|s| s.trim()).unwrap_or("");
            if val.is_empty() {
                let label = if declared.label.is_empty() {
                    &declared.name
                } else {
                    &declared.label
                };
                return Json(ApiResponse::err(format!(
                    "Variable « {} » est obligatoire pour cette Quick Prompt.",
                    label
                )));
            }
        }
    }

    // Agent override (mutates the QP clone so create_batch_run carries it)
    if let Some(a) = req.agent {
        qp.agent = a;
    }

    let rendered_prompt = render_qp_template(&qp.prompt_template, &req.vars);
    let title = req
        .title
        .clone()
        .unwrap_or_else(|| format!("{} — MCP run", qp.name));
    let batch_name = format!("MCP · {} · {}", qp.name, Utc::now().format("%H:%M:%S"));

    let (author_pseudo, author_avatar_email) = {
        let config = state.config.read().await;
        (
            config.server.pseudo.clone(),
            config.server.avatar_email.clone(),
        )
    };

    let project_id = req.project_id.clone().or_else(|| qp.project_id.clone());
    let items = vec![crate::db::workflows::BatchItemInput {
        title: title.clone(),
        prompt: rendered_prompt,
        agent_override: None,
    }];
    let qp_for_create = qp.clone();
    let outcome = match state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::create_batch_run(
                conn,
                crate::db::workflows::CreateBatchRunInput {
                    quick_prompt: &qp_for_create,
                    items,
                    batch_name: Some(batch_name),
                    project_id,
                    parent_run_id: None,
                    author_pseudo,
                    author_avatar_email,
                    language: "fr".into(),
                    workspace_mode: "Direct".into(),
                },
            )
        })
        .await
    {
        Ok(o) => o,
        Err(e) => {
            return Json(ApiResponse::err(format!(
                "Failed to create QP run: {}",
                e
            )))
        }
    };

    let disc_id = match outcome.discussion_ids.first().cloned() {
        Some(id) => id,
        None => {
            return Json(ApiResponse::err(
                "Batch creation returned no discussion id — internal error",
            ))
        }
    };

    // Fire-and-forget agent kickoff. We spawn a task that builds the SSE
    // stream and immediately drops it ; the internal agent task uses
    // `let _ = tx.send(...)` so the dropped receiver does not cancel
    // the run. The agent's reply still lands in the DB ; the MCP caller
    // reads it via `disc_load_other(disc_id)` once `next_check` elapsed.
    let kickoff_state = state.clone();
    let kickoff_disc_id = disc_id.clone();
    tokio::spawn(async move {
        let _sse = crate::api::discussions::streaming::make_agent_stream(
            kickoff_state,
            kickoff_disc_id,
            None,
        )
        .await;
        // Dropping `_sse` here drops the SSE receiver ; the spawned
        // agent task inside `make_agent_stream` keeps running and
        // persists its result to DB regardless.
    });

    // Smart polling — pull the avg from qp_versions metrics, take the
    // sum-across-all-versions as the dominant signal (the user typically
    // updates the QP body without breaking timing). Versions with fewer
    // than 3 launches don't show up in the metrics aggregator at all so
    // we get clean data.
    let qp_id_for_metrics = req.qp_id.clone();
    let metrics = state
        .db
        .with_conn(move |conn| {
            crate::db::quick_prompts::list_quick_prompt_version_metrics(
                conn,
                &qp_id_for_metrics,
            )
        })
        .await
        .unwrap_or_default();

    let total_launches: u32 = metrics.iter().map(|m| m.launches).sum();
    let weighted_ms_sum: u64 = metrics
        .iter()
        .filter_map(|m| m.avg_duration_ms.map(|d| d * m.launches as u64))
        .sum();
    let expected_duration_ms = if total_launches > 0 && weighted_ms_sum > 0 {
        Some(weighted_ms_sum / total_launches as u64)
    } else {
        None
    };
    let next_check = next_check_initial(expected_duration_ms, total_launches);

    tracing::info!(
        "MCP qp_run created disc {} for QP {} ({})",
        disc_id,
        qp.id,
        qp.name
    );

    Json(ApiResponse::ok(McpQpRunResponse {
        disc_id,
        qp_id: qp.id,
        qp_name: qp.name,
        agent: format!("{:?}", qp.agent),
        expected_duration_ms,
        samples: total_launches,
        next_check,
    }))
}

// ─── qp_batch_run (PR2) ─────────────────────────────────────────────────

const MCP_MAX_BATCH_SIZE: usize = 50;

#[derive(Debug, Deserialize)]
pub struct McpBatchItem {
    /// Optional discussion title. Defaults to `<qp_name> #<n>`.
    #[serde(default)]
    pub title: Option<String>,
    /// Per-item variable values rendered into the QP template independently.
    #[serde(default)]
    pub vars: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct McpQpBatchRunRequest {
    pub qp_id: String,
    pub items: Vec<McpBatchItem>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub batch_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct McpQpBatchRunResponse {
    /// The parent batch run id — poll it via `workflow_run_status` or list
    /// the children via `workflow_run_discussions`.
    pub run_id: String,
    pub qp_id: String,
    pub qp_name: String,
    pub disc_ids: Vec<String>,
    pub batch_total: u32,
    /// Per-item baseline (avg single-launch duration of this QP). The batch
    /// finishes when all items do — treat this as a floor, not the total.
    pub expected_duration_ms: Option<u64>,
    pub samples: u32,
    pub next_check: NextCheck,
}

/// Default a batch item's discussion title.
fn default_batch_item_title(qp_name: &str, idx: usize, provided: Option<&str>) -> String {
    match provided {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => format!("{} #{}", qp_name, idx + 1),
    }
}

/// POST /api/mcp/qp-batch-run
///
/// Fan a Quick Prompt out to N discussions in one call (the deagentified
/// twin of the UI batch flow). Each item renders the QP template with its
/// own `vars`; all children link under one batch run for tracking. Agents
/// are kicked off server-side (fire-and-forget, throttled by the runner's
/// agent semaphore) — the MCP caller reads results via `workflow_run_discussions`.
pub async fn qp_batch_run(
    State(state): State<AppState>,
    Json(req): Json<McpQpBatchRunRequest>,
) -> Json<ApiResponse<McpQpBatchRunResponse>> {
    if req.qp_id.is_empty() {
        return Json(ApiResponse::err("qp_id is required"));
    }
    if req.items.is_empty() {
        return Json(ApiResponse::err("Batch must contain at least 1 item"));
    }
    if req.items.len() > MCP_MAX_BATCH_SIZE {
        return Json(ApiResponse::err(format!(
            "Batch too large: {} items (max {})",
            req.items.len(),
            MCP_MAX_BATCH_SIZE
        )));
    }

    let qp_lookup = req.qp_id.clone();
    let qp = match state
        .db
        .with_conn(move |conn| crate::db::quick_prompts::get_quick_prompt(conn, &qp_lookup))
        .await
    {
        Ok(Some(q)) => q,
        Ok(None) => return Json(ApiResponse::err("Quick prompt not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    // Validate required vars against every item, render each prompt.
    let mut items: Vec<crate::db::workflows::BatchItemInput> = Vec::with_capacity(req.items.len());
    for (idx, item) in req.items.iter().enumerate() {
        for declared in &qp.variables {
            if declared.required {
                let val = item.vars.get(&declared.name).map(|s| s.trim()).unwrap_or("");
                if val.is_empty() {
                    let label = if declared.label.is_empty() {
                        &declared.name
                    } else {
                        &declared.label
                    };
                    return Json(ApiResponse::err(format!(
                        "Item #{} : variable « {} » est obligatoire pour cette Quick Prompt.",
                        idx + 1,
                        label
                    )));
                }
            }
        }
        items.push(crate::db::workflows::BatchItemInput {
            title: default_batch_item_title(&qp.name, idx, item.title.as_deref()),
            prompt: render_qp_template(&qp.prompt_template, &item.vars),
            agent_override: None,
        });
    }

    let (author_pseudo, author_avatar_email) = {
        let config = state.config.read().await;
        (config.server.pseudo.clone(), config.server.avatar_email.clone())
    };
    let project_id = req.project_id.clone().or_else(|| qp.project_id.clone());
    let batch_name = req.batch_name.clone().unwrap_or_else(|| {
        format!("MCP batch · {} · {}", qp.name, Utc::now().format("%H:%M:%S"))
    });

    let qp_for_create = qp.clone();
    let outcome = match state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::create_batch_run(
                conn,
                crate::db::workflows::CreateBatchRunInput {
                    quick_prompt: &qp_for_create,
                    items,
                    batch_name: Some(batch_name),
                    project_id,
                    parent_run_id: None,
                    author_pseudo,
                    author_avatar_email,
                    language: "fr".into(),
                    workspace_mode: "Direct".into(),
                },
            )
        })
        .await
    {
        Ok(o) => o,
        Err(e) => return Json(ApiResponse::err(format!("Failed to create batch: {}", e))),
    };

    // Kick off every child agent fire-and-forget (semaphore-throttled in the
    // runner). The MCP caller doesn't await SSE — results land in the DB.
    for disc_id in &outcome.discussion_ids {
        let kickoff_state = state.clone();
        let kickoff_disc_id = disc_id.clone();
        tokio::spawn(async move {
            let _sse = crate::api::discussions::streaming::make_agent_stream(
                kickoff_state,
                kickoff_disc_id,
                None,
            )
            .await;
        });
    }

    // ETA baseline from QP version metrics (per-item single launch).
    let qp_id_for_metrics = req.qp_id.clone();
    let metrics = state
        .db
        .with_conn(move |conn| {
            crate::db::quick_prompts::list_quick_prompt_version_metrics(conn, &qp_id_for_metrics)
        })
        .await
        .unwrap_or_default();
    let total_launches: u32 = metrics.iter().map(|m| m.launches).sum();
    let weighted_ms_sum: u64 = metrics
        .iter()
        .filter_map(|m| m.avg_duration_ms.map(|d| d * m.launches as u64))
        .sum();
    let expected_duration_ms = if total_launches > 0 && weighted_ms_sum > 0 {
        Some(weighted_ms_sum / total_launches as u64)
    } else {
        None
    };
    let next_check = next_check_initial(expected_duration_ms, total_launches);

    tracing::info!(
        "MCP qp_batch_run created run {} with {} discs for QP {}",
        outcome.run_id,
        outcome.batch_total,
        qp.name
    );

    Json(ApiResponse::ok(McpQpBatchRunResponse {
        run_id: outcome.run_id,
        qp_id: qp.id,
        qp_name: qp.name,
        disc_ids: outcome.discussion_ids,
        batch_total: outcome.batch_total,
        expected_duration_ms,
        samples: total_launches,
        next_check,
    }))
}

// ─── workflow_run_discussions (PR2) ─────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct McpRunDiscussionItem {
    pub disc_id: String,
    pub title: String,
    pub agent: String,
    pub message_count: u32,
    pub archived: bool,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct McpRunDiscussionsResponse {
    pub run_id: String,
    pub disc_count: u32,
    pub discussions: Vec<McpRunDiscussionItem>,
}

/// GET /api/mcp/workflow-run-discussions/:run_id
///
/// List the discussions a run spawned (batch children or workflow
/// BatchQuickPrompt fan-out). Empty for a pure linear workflow. The agent
/// then `disc_load_other`s the ones it cares about.
pub async fn workflow_run_discussions(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Json<ApiResponse<McpRunDiscussionsResponse>> {
    let run_id_lookup = run_id.clone();
    let discs = match state
        .db
        .with_conn(move |conn| crate::db::discussions::list_discussions_by_run(conn, &run_id_lookup))
        .await
    {
        Ok(d) => d,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let discussions: Vec<McpRunDiscussionItem> = discs
        .into_iter()
        .map(|d| McpRunDiscussionItem {
            disc_id: d.id,
            title: d.title,
            agent: format!("{:?}", d.agent),
            message_count: d.message_count,
            archived: d.archived,
            created_at: d.created_at,
        })
        .collect();

    Json(ApiResponse::ok(McpRunDiscussionsResponse {
        run_id,
        disc_count: discussions.len() as u32,
        discussions,
    }))
}

// ─── workflow_wait_for_completion (PR3) ─────────────────────────────────

const WAIT_POLL_INTERVAL_MS: u64 = 1500;
const WAIT_MIN_TIMEOUT_S: u64 = 1;
const WAIT_MAX_TIMEOUT_S: u64 = 60;

/// Clamp the caller-requested long-poll timeout to a safe server window.
fn clamp_wait_timeout(requested: Option<u64>) -> u64 {
    requested
        .unwrap_or(WAIT_MAX_TIMEOUT_S)
        .clamp(WAIT_MIN_TIMEOUT_S, WAIT_MAX_TIMEOUT_S)
}

#[derive(Debug, Deserialize)]
pub struct McpWaitRequest {
    pub run_id: String,
    /// Max seconds to hold the connection waiting for a terminal status.
    /// Clamped server-side to [1, 60]. On timeout the current (non-terminal)
    /// status is returned with `timed_out = true` + a `next_check` hint.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct McpWaitResponse {
    pub run_id: String,
    pub workflow_id: String,
    pub status: String,
    pub finished_at: Option<chrono::DateTime<Utc>>,
    pub elapsed_ms: u64,
    pub tokens_used: u64,
    /// True when we returned because the timeout elapsed, not because the
    /// run reached a terminal status.
    pub timed_out: bool,
    /// `None` when terminal; otherwise a polling hint for the next call.
    pub next_check: Option<NextCheck>,
}

/// POST /api/mcp/workflow-wait-for-completion
///
/// Long-poll a run until it reaches a terminal status or `timeout_s`
/// elapses. Saves the agent the back-and-forth of repeated status polls
/// for short runs — one call blocks (up to 60s) and returns the verdict.
pub async fn workflow_wait_for_completion(
    State(state): State<AppState>,
    Json(req): Json<McpWaitRequest>,
) -> Json<ApiResponse<McpWaitResponse>> {
    if req.run_id.is_empty() {
        return Json(ApiResponse::err("run_id is required"));
    }
    let timeout_s = clamp_wait_timeout(req.timeout_s);
    let deadline = Utc::now() + chrono::Duration::seconds(timeout_s as i64);

    loop {
        let run_id_lookup = req.run_id.clone();
        let run = match state
            .db
            .with_conn(move |conn| crate::db::workflows::get_run(conn, &run_id_lookup))
            .await
        {
            Ok(Some(r)) => r,
            Ok(None) => return Json(ApiResponse::err("Run not found")),
            Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
        };

        let now = Utc::now();
        let end = run.finished_at.unwrap_or(now);
        let elapsed_ms = (end - run.started_at).num_milliseconds().max(0) as u64;

        if is_terminal_status(&run.status) {
            return Json(ApiResponse::ok(McpWaitResponse {
                run_id: run.id,
                workflow_id: run.workflow_id,
                status: format!("{:?}", run.status),
                finished_at: run.finished_at,
                elapsed_ms,
                tokens_used: run.tokens_used,
                timed_out: false,
                next_check: None,
            }));
        }

        if now >= deadline {
            // Timed out still in-flight — hand back a polling hint computed
            // from this workflow's completed history (current run excluded).
            let wf_id = run.workflow_id.clone();
            let run_id_excl = run.id.clone();
            let history = state
                .db
                .with_conn(move |conn| {
                    crate::db::workflows::list_runs_paginated(conn, &wf_id, Some(RUN_AVG_LIMIT + 1), None)
                })
                .await
                .unwrap_or_default();
            let history: Vec<WorkflowRun> =
                history.into_iter().filter(|r| r.id != run_id_excl).collect();
            let (expected_duration_ms, samples) = avg_workflow_duration_ms(&history);
            let next_check = next_check_polling(expected_duration_ms, elapsed_ms, samples);

            return Json(ApiResponse::ok(McpWaitResponse {
                run_id: run.id,
                workflow_id: run.workflow_id,
                status: format!("{:?}", run.status),
                finished_at: run.finished_at,
                elapsed_ms,
                tokens_used: run.tokens_used,
                timed_out: true,
                next_check: Some(next_check),
            }));
        }

        tokio::time::sleep(std::time::Duration::from_millis(WAIT_POLL_INTERVAL_MS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_run(status: RunStatus, started: chrono::DateTime<Utc>, finished: Option<chrono::DateTime<Utc>>) -> WorkflowRun {
        WorkflowRun {
            id: Uuid::new_v4().to_string(),
            workflow_id: "wf-1".into(),
            status,
            trigger_context: None,
            step_results: vec![],
            tokens_used: 0,
            workspace_path: None,
            started_at: started,
            finished_at: finished,
            run_type: "linear".into(),
            batch_total: 0,
            batch_completed: 0,
            batch_failed: 0,
            batch_name: None,
            parent_run_id: None,
            state: ::std::collections::HashMap::new(),
            produced_branches: vec![],
        }
    }

    #[test]
    fn avg_ignores_pending_and_running_runs() {
        let base = Utc::now();
        let runs = vec![
            make_run(RunStatus::Success, base, Some(base + Duration::seconds(60))),
            // Running — should be excluded
            make_run(RunStatus::Running, base, None),
            // Pending — should be excluded
            make_run(RunStatus::Pending, base, None),
            make_run(RunStatus::Failed, base, Some(base + Duration::seconds(120))),
        ];
        let (avg, n) = avg_workflow_duration_ms(&runs);
        // (60_000 + 120_000) / 2 = 90_000
        assert_eq!(avg, Some(90_000));
        assert_eq!(n, 2);
    }

    #[test]
    fn avg_returns_none_when_no_completed_runs() {
        let base = Utc::now();
        let runs = vec![
            make_run(RunStatus::Running, base, None),
            make_run(RunStatus::Pending, base, None),
        ];
        let (avg, n) = avg_workflow_duration_ms(&runs);
        assert_eq!(avg, None);
        assert_eq!(n, 0);
    }

    #[test]
    fn avg_counts_stopped_by_guard_as_completed() {
        // StoppedByGuard is a terminal state — should be included so
        // pathological-run averages naturally pull the next ETA up.
        let base = Utc::now();
        let runs = vec![
            make_run(RunStatus::StoppedByGuard, base, Some(base + Duration::seconds(45))),
            make_run(RunStatus::Cancelled, base, Some(base + Duration::seconds(15))),
        ];
        let (_, n) = avg_workflow_duration_ms(&runs);
        assert_eq!(n, 2);
    }

    #[test]
    fn render_qp_template_substitutes_double_braces() {
        let mut vars = HashMap::new();
        vars.insert("name".into(), "PeerAlpha".into());
        vars.insert("count".into(), "3".into());
        let out = render_qp_template("Hi {{name}}, you have {{count}} items", &vars);
        assert_eq!(out, "Hi PeerAlpha, you have 3 items");
    }

    #[test]
    fn render_qp_template_leaves_undeclared_placeholders_untouched() {
        let vars = HashMap::new();
        let out = render_qp_template("Hi {{name}}", &vars);
        // Undeclared placeholders pass through — matches the front-end
        // behaviour, the user sees them in the disc.
        assert_eq!(out, "Hi {{name}}");
    }

    #[test]
    fn excerpt_short_string_returned_verbatim() {
        let out = excerpt("hello");
        assert_eq!(out, "hello");
    }

    #[test]
    fn excerpt_long_string_truncated_with_ellipsis() {
        let s = "a".repeat(250);
        let out = excerpt(&s);
        // OUTPUT_EXCERPT_MAX_CHARS chars + 1 ellipsis = 201 chars
        assert_eq!(out.chars().count(), OUTPUT_EXCERPT_MAX_CHARS + 1);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn excerpt_is_char_boundary_safe_with_multibyte() {
        // 250 « é » chars × 2 bytes each — naive [..200] slicing
        // would panic on a non-UTF8 boundary. .chars() iteration is
        // boundary-safe by definition.
        let s = "é".repeat(250);
        let out = excerpt(&s);
        assert_eq!(out.chars().count(), OUTPUT_EXCERPT_MAX_CHARS + 1);
    }

    #[test]
    fn is_terminal_status_matches_terminal_states() {
        assert!(is_terminal_status(&RunStatus::Success));
        assert!(is_terminal_status(&RunStatus::Failed));
        assert!(is_terminal_status(&RunStatus::Cancelled));
        assert!(is_terminal_status(&RunStatus::StoppedByGuard));
        // Non-terminal
        assert!(!is_terminal_status(&RunStatus::Pending));
        assert!(!is_terminal_status(&RunStatus::Running));
        assert!(!is_terminal_status(&RunStatus::WaitingApproval));
    }

    #[test]
    fn default_batch_item_title_uses_provided_when_present() {
        assert_eq!(default_batch_item_title("Audit QP", 0, Some("Custom")), "Custom");
        assert_eq!(default_batch_item_title("Audit QP", 0, Some("  Trimmed  ")), "Trimmed");
    }

    #[test]
    fn default_batch_item_title_falls_back_to_indexed_name() {
        assert_eq!(default_batch_item_title("Audit QP", 0, None), "Audit QP #1");
        assert_eq!(default_batch_item_title("Audit QP", 4, None), "Audit QP #5");
        // Blank provided title is treated as absent.
        assert_eq!(default_batch_item_title("Audit QP", 2, Some("   ")), "Audit QP #3");
    }

    #[test]
    fn clamp_wait_timeout_defaults_and_bounds() {
        assert_eq!(clamp_wait_timeout(None), WAIT_MAX_TIMEOUT_S);
        assert_eq!(clamp_wait_timeout(Some(0)), WAIT_MIN_TIMEOUT_S);
        assert_eq!(clamp_wait_timeout(Some(5)), 5);
        assert_eq!(clamp_wait_timeout(Some(9999)), WAIT_MAX_TIMEOUT_S);
    }
}
