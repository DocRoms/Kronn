use std::convert::Infallible;
use std::pin::Pin;
use axum::{
    extract::{Path, State},
    response::sse::{Event, Sse},
    Json,
};
use chrono::Utc;
use futures::Stream;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// GET /api/workflows
pub async fn list(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<WorkflowSummary>>> {
    match state.db.with_conn(|conn| {
        let workflows = crate::db::workflows::list_workflows(conn)?;
        // Batch-load last runs and project names (avoids N+1 queries)
        let last_runs = crate::db::workflows::get_last_runs_all(conn)?;
        let project_names = crate::db::projects::get_project_names(conn)?;

        let summaries = workflows.into_iter().map(|wf| {
            let last_run = last_runs.get(&wf.id)
                .map(|r| WorkflowRunSummary {
                    id: r.id.clone(),
                    status: r.status.clone(),
                    started_at: r.started_at,
                    finished_at: r.finished_at,
                    tokens_used: r.tokens_used,
                });

            let project_name = wf.project_id.as_ref()
                .and_then(|pid| project_names.get(pid).cloned());

            let trigger_type = match &wf.trigger {
                WorkflowTrigger::Cron { .. } => "cron",
                WorkflowTrigger::Tracker { .. } => "tracker",
                WorkflowTrigger::Manual => "manual",
            }.to_string();

            WorkflowSummary {
                id: wf.id,
                name: wf.name,
                project_id: wf.project_id,
                project_name,
                trigger_type,
                step_count: wf.steps.len() as u32,
                enabled: wf.enabled,
                last_run,
                created_at: wf.created_at,
            }
        }).collect();

        Ok(summaries)
    }).await {
        Ok(summaries) => Json(ApiResponse::ok(summaries)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// GET /api/workflows/:id
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Workflow>> {
    match state.db.with_conn(move |conn| crate::db::workflows::get_workflow(conn, &id)).await {
        Ok(Some(wf)) => Json(ApiResponse::ok(wf)),
        Ok(None) => Json(ApiResponse::err("Workflow not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/workflows
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateWorkflowRequest>,
) -> Json<ApiResponse<Workflow>> {
    if req.steps.is_empty() {
        return Json(ApiResponse::err("Workflow must have at least one step"));
    }
    if req.steps.len() > 20 {
        return Json(ApiResponse::err(format!("Too many steps ({}, max 20)", req.steps.len())));
    }
    if req.name.len() > 200 {
        return Json(ApiResponse::err("Workflow name too long (max 200 chars)"));
    }

    let now = Utc::now();
    let wf = Workflow {
        id: Uuid::new_v4().to_string(),
        name: req.name,
        project_id: req.project_id,
        trigger: req.trigger,
        steps: req.steps,
        actions: req.actions,
        safety: req.safety.unwrap_or(WorkflowSafety {
            sandbox: false,
            max_files: None,
            max_lines: None,
            require_approval: false,
        }),
        workspace_config: req.workspace_config,
        concurrency_limit: req.concurrency_limit,
        enabled: true,
        created_at: now,
        updated_at: now,
    };

    let w = wf.clone();
    match state.db.with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &w)).await {
        Ok(()) => Json(ApiResponse::ok(wf)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/workflows/:id
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateWorkflowRequest>,
) -> Json<ApiResponse<Workflow>> {
    let wf_id = id.clone();
    let existing = match state.db.with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id)).await {
        Ok(Some(wf)) => wf,
        Ok(None) => return Json(ApiResponse::err("Workflow not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    if let Some(ref steps) = req.steps {
        if steps.len() > 20 {
            return Json(ApiResponse::err(format!("Too many steps ({}, max 20)", steps.len())));
        }
    }
    if let Some(ref name) = req.name {
        if name.len() > 200 {
            return Json(ApiResponse::err("Workflow name too long (max 200 chars)"));
        }
    }

    let updated = Workflow {
        id: existing.id,
        name: req.name.unwrap_or(existing.name),
        project_id: existing.project_id,
        trigger: req.trigger.unwrap_or(existing.trigger),
        steps: req.steps.unwrap_or(existing.steps),
        actions: req.actions.unwrap_or(existing.actions),
        safety: req.safety.unwrap_or(existing.safety),
        workspace_config: req.workspace_config.or(existing.workspace_config),
        concurrency_limit: req.concurrency_limit.or(existing.concurrency_limit),
        enabled: req.enabled.unwrap_or(existing.enabled),
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };

    let w = updated.clone();
    match state.db.with_conn(move |conn| crate::db::workflows::update_workflow(conn, &w)).await {
        Ok(()) => Json(ApiResponse::ok(updated)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/workflows/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::workflows::delete_workflow(conn, &id)).await {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/workflows/:id/trigger — Manual trigger with SSE streaming
pub async fn trigger(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Sse<SseStream> {
    let wf_id = id.clone();
    let wf = match state.db.with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id)).await {
        Ok(Some(wf)) => wf,
        Ok(None) => {
            return sse_error("Workflow not found");
        }
        Err(e) => {
            return sse_error(format!("DB error: {}", e));
        }
    };

    if !wf.enabled {
        return sse_error("Workflow is disabled");
    }

    // Atomic concurrency check + insert in a single transaction (avoids TOCTOU race)
    let now = Utc::now();
    let run = WorkflowRun {
        id: Uuid::new_v4().to_string(),
        workflow_id: wf.id.clone(),
        status: RunStatus::Pending,
        trigger_context: Some(serde_json::json!({ "type": "manual", "triggered_at": now.to_rfc3339() })),
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
    };

    let r = run.clone();
    let limit = wf.concurrency_limit;
    let wf_id_check = wf.id.clone();
    match state.db.with_conn(move |conn| {
        // Single transaction: check + insert atomically
        if let Some(max) = limit {
            let active = crate::db::workflows::count_active_runs(conn, &wf_id_check)?;
            if active >= max {
                anyhow::bail!("CONCURRENCY_LIMIT:{}/{}", active, max);
            }
        }
        crate::db::workflows::insert_run(conn, &r)?;
        Ok(())
    }).await {
        Ok(()) => {}
        Err(e) => {
            let msg = e.to_string();
            if let Some(rest) = msg.strip_prefix("CONCURRENCY_LIMIT:") {
                return sse_error(format!("Concurrency limit reached ({})", rest));
            }
            return sse_error(format!("DB error: {}", msg));
        }
    }

    tracing::info!("Workflow run created: {} for workflow {}", run.id, wf.name);

    // Create event channel for real-time streaming
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::workflows::runner::RunEvent>(32);

    // Dispatch execution in background with the sender
    let db = state.db.clone();
    let config = state.config.clone();
    let mut run_exec = run.clone();
    tokio::spawn(async move {
        let cfg = config.read().await;
        let tokens = cfg.tokens.clone();
        let agents = cfg.agents.clone();
        drop(cfg);

        if let Err(e) = crate::workflows::runner::execute_run(
            db, &wf, &mut run_exec, &tokens, &agents, Some(tx),
        ).await {
            tracing::error!("Workflow run {} failed: {}", run_exec.id, e);
        }
    });

    // Stream events as SSE
    let run_id = run.id.clone();
    let stream: SseStream = Box::pin(async_stream::try_stream! {
        // Send initial run info
        let start = serde_json::json!({ "run_id": run_id });
        yield Event::default().event("run_start").data(start.to_string());

        // Forward events from the runner
        while let Some(evt) = rx.recv().await {
            match &evt {
                crate::workflows::runner::RunEvent::StepStart { step_name, step_index, total_steps } => {
                    let data = serde_json::json!({
                        "step_name": step_name,
                        "step_index": step_index,
                        "total_steps": total_steps,
                    });
                    yield Event::default().event("step_start").data(data.to_string());
                }
                crate::workflows::runner::RunEvent::StepDone { step_result } => {
                    let data = serde_json::to_value(step_result).unwrap_or_default();
                    yield Event::default().event("step_done").data(data.to_string());
                }
                crate::workflows::runner::RunEvent::RunDone { status } => {
                    let data = serde_json::json!({ "status": status });
                    yield Event::default().event("run_done").data(data.to_string());
                }
                crate::workflows::runner::RunEvent::RunError { error } => {
                    let data = serde_json::json!({ "error": error });
                    yield Event::default().event("error").data(data.to_string());
                }
            }
        }
    });

    Sse::new(stream)
}

fn sse_error(msg: impl Into<String>) -> Sse<SseStream> {
    let msg = msg.into();
    let stream: SseStream = Box::pin(futures::stream::once(async move {
        Ok::<_, Infallible>(
            Event::default().event("error").data(
                serde_json::json!({ "error": msg }).to_string()
            )
        )
    }));
    Sse::new(stream)
}

/// GET /api/workflows/:id/runs
pub async fn list_runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<WorkflowRun>>> {
    match state.db.with_conn(move |conn| crate::db::workflows::list_runs(conn, &id)).await {
        Ok(runs) => Json(ApiResponse::ok(runs)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// GET /api/workflows/:id/runs/:run_id
pub async fn get_run(
    State(state): State<AppState>,
    Path((_id, run_id)): Path<(String, String)>,
) -> Json<ApiResponse<WorkflowRun>> {
    match state.db.with_conn(move |conn| crate::db::workflows::get_run(conn, &run_id)).await {
        Ok(Some(run)) => Json(ApiResponse::ok(run)),
        Ok(None) => Json(ApiResponse::err("Run not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/workflows/:id/runs — Delete all runs for a workflow
pub async fn delete_all_runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::workflows::delete_all_runs(conn, &id)).await {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/workflows/:id/runs/:run_id — Delete a single run
pub async fn delete_run(
    State(state): State<AppState>,
    Path((_id, run_id)): Path<(String, String)>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::workflows::delete_run(conn, &run_id)).await {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}
