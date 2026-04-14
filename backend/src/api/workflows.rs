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
        project_id: req.project_id.unwrap_or(existing.project_id),
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
        // Legacy linear runs — batch fields stay at their defaults.
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: None,
        parent_run_id: None,
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
    let state_for_run = state.clone();
    let config = state.config.clone();
    let mut run_exec = run.clone();
    tokio::spawn(async move {
        let cfg = config.read().await;
        let tokens = cfg.tokens.clone();
        let agents = cfg.agents.clone();
        drop(cfg);

        if let Err(e) = crate::workflows::runner::execute_run(
            state_for_run, &wf, &mut run_exec, &tokens, &agents, Some(tx),
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
                crate::workflows::runner::RunEvent::StepProgress { .. } => {
                    // Progress events not streamed for full workflow runs (only test-step)
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

    Sse::new(crate::core::sse_limits::bounded(stream))
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

/// Request for [`test_batch_step`].
#[derive(Debug, serde::Deserialize)]
pub struct TestBatchStepRequest {
    pub step: WorkflowStep,
    /// Mock output of the upstream step (raw text or structured JSON envelope).
    /// We feed it to the template engine so `{{steps.<name>.data.tickets}}`
    /// resolves to the same thing it would at runtime.
    #[serde(default)]
    pub mock_previous_output: Option<String>,
    /// The name of the upstream step the mock output represents — defaults
    /// to "previous" so users can use `{{previous_step.output}}` in items_from.
    #[serde(default)]
    pub previous_step_name: Option<String>,
}

/// Response for [`test_batch_step`].
#[derive(Debug, serde::Serialize)]
pub struct BatchPreview {
    /// First N items the runner would fan out (capped at 10 for display).
    pub sample_items: Vec<String>,
    pub total_items: u32,
    pub capped_at: u32,
    pub max_items_allowed: u32,
    pub quick_prompt_id: Option<String>,
    pub quick_prompt_name: Option<String>,
    pub quick_prompt_icon: Option<String>,
    pub quick_prompt_agent: Option<String>,
    pub first_variable_name: Option<String>,
    /// Prompt that would be sent for the FIRST item (after `{{var}}` substitution).
    /// Kept for backward compat — prefer `sample_rendered_prompts` for the
    /// per-item view.
    pub sample_rendered_prompt: Option<String>,
    /// Rendered prompt for EACH sample item (same length & order as
    /// `sample_items`). Lets the user spot-check the rendering on every
    /// ticket of their batch, not just the first one.
    pub sample_rendered_prompts: Vec<String>,
    pub workspace_mode: String,
    pub wait_for_completion: bool,
    /// Validation errors found during the dry-run (missing QP, empty list,
    /// unresolved template, etc.) — non-empty means the step would fail at
    /// runtime. Frontend renders these as red bullets.
    pub errors: Vec<String>,
    /// Non-blocking warnings: the dry-run could proceed, but there's a
    /// configuration smell that would bite in production (e.g. using
    /// `{{steps.X.data}}` against a FreeText step). Shown in orange,
    /// separate from the red errors block.
    pub warnings: Vec<String>,
}

/// POST /api/workflows/test-batch-step
///
/// Dry-run preview for a `BatchQuickPrompt` step. Renders the items_from
/// template against mock previous output, parses the items, loads the QP,
/// and returns what the runner would do — WITHOUT creating any discussion,
/// batch run, or worktree. Used by the wizard's per-step "Tester" button so
/// users can validate their batch configuration before triggering the real
/// workflow.
pub async fn test_batch_step(
    State(state): State<AppState>,
    Json(req): Json<TestBatchStepRequest>,
) -> Json<ApiResponse<BatchPreview>> {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // 1. Validate required fields up front — same checks as the runtime
    //    executor. Surfaces config bugs without needing to fire the workflow.
    let qp_id = match req.step.batch_quick_prompt_id.as_ref() {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            errors.push("Missing batch_quick_prompt_id".to_string());
            return Json(ApiResponse::ok(empty_preview(&req.step, errors)));
        }
    };
    let items_from = match req.step.batch_items_from.as_ref() {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            errors.push("Missing batch_items_from".to_string());
            return Json(ApiResponse::ok(empty_preview(&req.step, errors)));
        }
    };

    // 2. Render items_from against the mock previous output. We seed the
    //    template ctx with a synthetic upstream step so the same expressions
    //    that work at runtime work here.
    let mut ctx = crate::workflows::template::TemplateContext::new();
    if let Some(ref prev) = req.mock_previous_output {
        let step_name = req.previous_step_name.as_deref().unwrap_or("previous");
        ctx.set_step_output(step_name, prev);
        // Also expose under "previous" so `{{previous_step.output}}` works
        // regardless of the actual step name.
        if step_name != "previous" {
            ctx.set_step_output("previous", prev);
        }

        // Dry-run convenience: users often wire `{{steps.X.data}}` but their
        // source step is in FreeText mode → `.data` is never populated by
        // `set_step_output`, and the render leaves the template literal
        // (which then looks like a single item = silent config bug).
        //
        // Detect the smell (items_from references `.data` and the mock is
        // NOT a structured envelope) and:
        //   1. Inject a raw-text fallback for `.data` so the preview runs
        //   2. Warn the user so they know this won't work at runtime
        let uses_data = items_from.contains(".data");
        let has_envelope = crate::workflows::template::extract_step_envelope(prev).is_some();
        if uses_data && !has_envelope {
            ctx.set(format!("steps.{}.data", step_name), prev.clone());
            if step_name != "previous" {
                ctx.set("previous_step.data".to_string(), prev.clone());
            }
            warnings.push(format!(
                "Ton template utilise `{{{{steps.{name}.data}}}}` mais le step « {name} » \
                 n'est pas en mode « Structured ». En production, `.data` ne sera pas disponible — \
                 seulement `.output`. Pour corriger : coche « Structured » sur le step « {name} » \
                 (le step précédent), OU remplace `.data` par `.output` dans « Liste des items ». \
                 Ce test continue avec un fallback pour te montrer le résultat.",
                name = step_name
            ));
        }
    }

    let rendered = match ctx.render(&items_from) {
        Ok(s) => s,
        Err(e) => {
            errors.push(format!("Template render error: {}", e));
            return Json(ApiResponse::ok(empty_preview(&req.step, errors)));
        }
    };

    // Detect an unresolved template — the render engine leaves `{{foo}}`
    // in-place when a variable is unknown (no exception thrown). Without
    // this guard, the parser downstream treats the literal `{{steps.X.data}}`
    // as a single item and reports "1 item would be launched", which is
    // worse than useless: it hides the configuration bug behind a green
    // check. Catch it here and tell the user exactly what's missing.
    if rendered.contains("{{") && rendered.contains("}}") {
        errors.push(format!(
            "Le template contient une variable non résolue : {}. \
             As-tu testé le step précédent pour qu'il produise un output ? \
             Ou colle manuellement un exemple dans « Mock input » ci-dessus.",
            rendered.trim()
        ));
        return Json(ApiResponse::ok(empty_preview(&req.step, errors)));
    }

    // 3. Parse the rendered string into items. Reuses the same logic as
    //    the runtime executor (JSON array OR text split).
    let items = crate::workflows::batch_step::parse_items_for_test(&rendered);
    if items.is_empty() {
        errors.push(format!(
            "items_from resolved to an empty list. Rendered value: {:?}",
            if rendered.len() > 200 { format!("{}…", &rendered[..200]) } else { rendered.clone() }
        ));
    }

    // 4. Load the Quick Prompt.
    let qp_lookup = qp_id.clone();
    let qp = match state.db.with_conn(move |conn| {
        crate::db::quick_prompts::get_quick_prompt(conn, &qp_lookup)
    }).await {
        Ok(Some(q)) => Some(q),
        Ok(None) => {
            errors.push(format!("Quick prompt '{}' not found", qp_id));
            None
        }
        Err(e) => {
            errors.push(format!("DB error loading QP: {}", e));
            None
        }
    };

    // 5. Worktree safety check (same as runtime).
    let workspace_mode = req.step.batch_workspace_mode
        .clone()
        .unwrap_or_else(|| "Direct".to_string());
    if workspace_mode == "Isolated"
        && qp.as_ref().map(|q| q.project_id.is_none()).unwrap_or(false)
    {
        errors.push(
            "Isolated workspace mode requires a project-linked Quick Prompt"
                .to_string()
        );
    }

    // 6. Render every sample item's prompt for the user to spot-check.
    //    Was rendering only the first item — but Marie wants to see the
    //    rendering for each ticket of her batch, not just one (otherwise
    //    she can't catch per-ticket template surprises).
    let first_variable_name = qp.as_ref()
        .and_then(|q| q.variables.first())
        .map(|v| v.name.clone());
    const SAMPLE_LIMIT: usize = 10;
    let sample_rendered_prompts: Vec<String> = if let Some(qp) = &qp {
        items.iter().take(SAMPLE_LIMIT).map(|item| {
            crate::workflows::batch_step::render_qp_prompt_for_test(
                &qp.prompt_template,
                first_variable_name.as_deref(),
                item,
            )
        }).collect()
    } else {
        Vec::new()
    };
    let sample_rendered_prompt = sample_rendered_prompts.first().cloned();

    let max_items = req.step.batch_max_items.unwrap_or(50);
    let total = items.len() as u32;
    if total > max_items {
        errors.push(format!(
            "Item count {} exceeds max {} (raise `batch_max_items` if needed)",
            total, max_items
        ));
    }

    // Same SAMPLE_LIMIT as above — keep them in lockstep so sample_items
    // and sample_rendered_prompts have matching indices.
    let sample_items: Vec<String> = items.iter().take(SAMPLE_LIMIT).cloned().collect();

    Json(ApiResponse::ok(BatchPreview {
        sample_items,
        total_items: total,
        capped_at: SAMPLE_LIMIT as u32,
        max_items_allowed: max_items,
        quick_prompt_id: qp.as_ref().map(|q| q.id.clone()),
        quick_prompt_name: qp.as_ref().map(|q| q.name.clone()),
        quick_prompt_icon: qp.as_ref().map(|q| q.icon.clone()),
        quick_prompt_agent: qp.as_ref().map(|q| {
            serde_json::to_string(&q.agent).unwrap_or_default().trim_matches('"').to_string()
        }),
        first_variable_name,
        sample_rendered_prompt,
        sample_rendered_prompts,
        workspace_mode,
        wait_for_completion: req.step.batch_wait_for_completion.unwrap_or(true),
        errors,
        warnings,
    }))
}

/// Build a mostly-empty BatchPreview when validation aborts early.
fn empty_preview(step: &WorkflowStep, errors: Vec<String>) -> BatchPreview {
    BatchPreview {
        sample_items: vec![],
        total_items: 0,
        capped_at: 10,
        max_items_allowed: step.batch_max_items.unwrap_or(50),
        quick_prompt_id: step.batch_quick_prompt_id.clone(),
        quick_prompt_name: None,
        quick_prompt_icon: None,
        quick_prompt_agent: None,
        first_variable_name: None,
        sample_rendered_prompt: None,
        sample_rendered_prompts: Vec::new(),
        workspace_mode: step.batch_workspace_mode.clone().unwrap_or_else(|| "Direct".into()),
        wait_for_completion: step.batch_wait_for_completion.unwrap_or(true),
        errors,
        warnings: Vec::new(),
    }
}

/// POST /api/workflows/test-step — Test a single step with mock context (SSE)
pub async fn test_step(
    State(state): State<AppState>,
    Json(req): Json<TestStepRequest>,
) -> Sse<SseStream> {
    let cfg = state.config.read().await;
    let tokens = cfg.tokens.clone();
    let agents = cfg.agents.clone();
    drop(cfg);

    // Resolve project path (for MCP context)
    let project_path = if let Some(pid) = &req.project_id {
        let id = pid.clone();
        match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
            Ok(Some(p)) => p.path,
            _ => std::env::temp_dir().to_string_lossy().to_string(),
        }
    } else {
        std::env::temp_dir().to_string_lossy().to_string()
    };
    let work_dir = project_path.clone();

    // Build template context with mock data
    let mut ctx = crate::workflows::template::TemplateContext::new();
    if let Some(prev_output) = &req.mock_previous_output {
        ctx.set_step_output("previous", prev_output);
    }
    if let Some(vars) = &req.mock_variables {
        for (k, v) in vars {
            ctx.set(k.clone(), v.clone());
        }
    }

    // Determine full_access from agent config
    let full_access = match req.step.agent {
        AgentType::ClaudeCode => agents.claude_code.full_access,
        AgentType::Codex => agents.codex.full_access,
        AgentType::GeminiCli => agents.gemini_cli.full_access,
        AgentType::Kiro => agents.kiro.full_access,
        AgentType::Vibe => agents.vibe.full_access,
        AgentType::CopilotCli => agents.copilot_cli.full_access,
        AgentType::Ollama => agents.ollama.full_access,
        AgentType::Custom => false,
    };

    // In dry_run mode, prepend a simulation preamble to the prompt
    let mut step = req.step.clone();
    if req.dry_run {
        step.prompt_template = format!(
            "⚠️ MODE SIMULATION (dry-run) — RÈGLES STRICTES :\n\
            - Tu ne dois RIEN exécuter, RIEN modifier, RIEN écrire, RIEN créer.\n\
            - Tu ne dois PAS appeler de tool qui modifie des données (pas de create, update, delete, write, post, comment).\n\
            - Tu peux LIRE des données (get, list, search, read) pour analyser la situation.\n\
            - Tu dois DÉCRIRE précisément ce que tu FERAIS en mode réel : quelles actions, sur quels éléments, avec quel contenu.\n\
            - Formate ta réponse comme un plan d'exécution détaillé.\n\n\
            ---\n\n{}",
            step.prompt_template
        );
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::workflows::runner::RunEvent>(64);

    tokio::spawn(async move {
        let _ = tx.send(crate::workflows::runner::RunEvent::StepStart {
            step_name: step.name.clone(),
            step_index: 0,
            total_steps: 1,
        }).await;

        // Create a progress channel to stream partial output
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<String>(256);
        let tx_progress = tx.clone();
        // Forward progress chunks as StepProgress events
        tokio::spawn(async move {
            while let Some(text) = progress_rx.recv().await {
                let _ = tx_progress.send(crate::workflows::runner::RunEvent::StepProgress {
                    text,
                }).await;
            }
        });

        let outcome = crate::workflows::steps::execute_step(
            &step, &project_path, &work_dir, &tokens, full_access, &ctx,
            Some(progress_tx),
        ).await;

        let _ = tx.send(crate::workflows::runner::RunEvent::StepDone {
            step_result: outcome.result.clone(),
        }).await;

        let status = outcome.result.status.clone();
        let _ = tx.send(crate::workflows::runner::RunEvent::RunDone { status }).await;
    });

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        yield Event::default().event("run_start").data(
            serde_json::json!({ "run_id": "test", "test_mode": true }).to_string()
        );
        while let Some(evt) = rx.recv().await {
            match &evt {
                crate::workflows::runner::RunEvent::StepStart { step_name, step_index, total_steps } => {
                    yield Event::default().event("step_start").data(
                        serde_json::json!({ "step_name": step_name, "step_index": step_index, "total_steps": total_steps }).to_string()
                    );
                }
                crate::workflows::runner::RunEvent::StepProgress { text } => {
                    yield Event::default().event("step_progress").data(
                        serde_json::json!({ "text": text }).to_string()
                    );
                }
                crate::workflows::runner::RunEvent::StepDone { step_result } => {
                    yield Event::default().event("step_done").data(
                        serde_json::to_value(step_result).unwrap_or_default().to_string()
                    );
                }
                crate::workflows::runner::RunEvent::RunDone { status } => {
                    yield Event::default().event("run_done").data(
                        serde_json::json!({ "status": status }).to_string()
                    );
                }
                crate::workflows::runner::RunEvent::RunError { error } => {
                    yield Event::default().event("error").data(
                        serde_json::json!({ "error": error }).to_string()
                    );
                }
            }
        }
    });

    Sse::new(crate::core::sse_limits::bounded(stream))
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

/// GET /api/workflow-runs/batch-summaries
///
/// Returns a compact summary of every batch run, with the parent linear run
/// resolved to a human-friendly (workflow name + run sequence) label.
/// Consumed by the discussion sidebar to render a clickable pastille that
/// jumps back to the workflow that spawned a given batch.
pub async fn list_batch_run_summaries(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<BatchRunSummary>>> {
    match state.db.with_conn(crate::db::workflows::list_batch_run_summaries).await {
        Ok(summaries) => Json(ApiResponse::ok(summaries)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// Response for [`cancel_run`].
#[derive(Debug, serde::Serialize)]
pub struct CancelRunResponse {
    pub run_cancelled: bool,
    /// Number of child-batch discussions whose agent tokens were triggered.
    /// Zero means either this was a plain linear run (no batch children)
    /// or all child discs had already finished.
    pub child_discs_cancelled: u32,
}

/// POST /api/workflows/:id/runs/:run_id/cancel
///
/// Stop a Running workflow run. Triggers the run's cancellation token (so the
/// runner short-circuits to `Cancelled` status before its next step) AND
/// cascades to every active batch child: each child batch run's discussions
/// have their own agent token triggered so in-flight agents stop too.
///
/// This is the "⏹ Arrêter" button on WorkflowDetail run cards. Idempotent —
/// safe to call on an already-finished run (returns false for all).
pub async fn cancel_run(
    State(state): State<AppState>,
    Path((_workflow_id, run_id)): Path<(String, String)>,
) -> Json<ApiResponse<CancelRunResponse>> {
    // 1. Trigger the linear run's own token
    let run_cancelled = {
        let mut map = match state.cancel_registry.lock() {
            Ok(m) => m,
            Err(_) => return Json(ApiResponse::err("Cancel registry poisoned")),
        };
        if let Some(token) = map.remove(&run_id) {
            token.cancel();
            true
        } else {
            false
        }
    };

    // 2. Find child batches via parent_run_id and cascade to their disc agents.
    //    One DB call to get all child batches of this run, then another to get
    //    each batch's child discussions. For each disc, trigger its cancel
    //    token if one is registered (i.e. agent still running).
    let run_id_for_db = run_id.clone();
    let child_disc_ids: Vec<String> = match state.db.with_conn(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT d.id FROM discussions d \
             JOIN workflow_runs wr ON d.workflow_run_id = wr.id \
             WHERE wr.parent_run_id = ?1"
        )?;
        let ids: Vec<String> = stmt
            .query_map(rusqlite::params![&run_id_for_db], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }).await {
        Ok(ids) => ids,
        Err(e) => return Json(ApiResponse::err(format!("DB error finding child discs: {}", e))),
    };

    let child_discs_cancelled = {
        let mut map = match state.cancel_registry.lock() {
            Ok(m) => m,
            Err(_) => return Json(ApiResponse::err("Cancel registry poisoned")),
        };
        let mut n: u32 = 0;
        for disc_id in &child_disc_ids {
            if let Some(token) = map.remove(disc_id) {
                token.cancel();
                n += 1;
            }
        }
        n
    };

    // 3. Also mark any running child batch runs as Cancelled in the DB so the
    //    sidebar stops showing them as "in progress" even though no one is
    //    actively writing to them anymore. We don't touch discussions — they
    //    get their Cancelled/Failed status from the agent-task finally path.
    let run_id_for_db2 = run_id.clone();
    let _ = state.db.with_conn(move |conn| {
        conn.execute(
            "UPDATE workflow_runs SET status = 'Cancelled', finished_at = datetime('now') \
             WHERE parent_run_id = ?1 AND status = 'Running'",
            rusqlite::params![&run_id_for_db2],
        )?;
        Ok(())
    }).await;

    tracing::info!(
        "Cancel run {}: run_cancelled={}, {} child disc agents stopped",
        run_id, run_cancelled, child_discs_cancelled
    );

    Json(ApiResponse::ok(CancelRunResponse {
        run_cancelled,
        child_discs_cancelled,
    }))
}

/// Response for [`delete_batch_run`].
#[derive(Debug, serde::Serialize)]
pub struct DeletedBatchResponse {
    pub run_id: String,
    pub discussions_deleted: u32,
}

/// DELETE /api/workflow-runs/:run_id
///
/// Delete a batch workflow run AND all its child discussions in one
/// transaction. Refuses to act on linear runs (use the workflow run delete
/// endpoint for those — they don't own discussions to begin with).
///
/// The user-visible flow: from the sidebar batch group, "🗑 Supprimer ce
/// batch et ses N discussions" → confirm → this handler. Returns the count
/// of discussions actually deleted so the toast can show the right number
/// (a batch may have grown/shrunk between when the UI computed N and now).
pub async fn delete_batch_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Json<ApiResponse<DeletedBatchResponse>> {
    let run_id_for_db = run_id.clone();
    match state.db.with_conn(move |conn| {
        crate::db::workflows::delete_batch_run_with_discussions(conn, &run_id_for_db)
    }).await {
        Ok(summary) => Json(ApiResponse::ok(DeletedBatchResponse {
            run_id: summary.run_id,
            discussions_deleted: summary.discussions_deleted as u32,
        })),
        Err(e) => Json(ApiResponse::err(format!("Failed to delete batch: {}", e))),
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

// ═══════════════════════════════════════════════════════════════════════════════
// Workflow suggestions — MCP-based template catalogue
// ═══════════════════════════════════════════════════════════════════════════════

/// Static catalogue entry: which MCP combination triggers which suggestion.
struct CatalogueEntry {
    id: &'static str,
    title_fr: &'static str,
    title_en: &'static str,
    desc_fr: &'static str,
    desc_en: &'static str,
    required_mcps: &'static [&'static str],
    audience: &'static str,
    complexity: &'static str,
    trigger: fn() -> WorkflowTrigger,
    /// Each tuple is (step_name, prompt, is_structured). Multiple tuples = multi-step workflow.
    /// is_structured=true → engine injects format instructions + extracts JSON envelope.
    step_prompts: &'static [(&'static str, &'static str, bool)],
}

const CATALOGUE: &[CatalogueEntry] = &[
    CatalogueEntry {
        id: "orphan-prs",
        title_fr: "Détection de PRs orphelines",
        title_en: "Orphan PR detection",
        desc_fr: "Alerte quand une PR n'a pas de ticket Jira/Linear associé",
        desc_en: "Alert when a PR has no linked Jira/Linear ticket",
        required_mcps: &["github", "jira"],
        audience: "dev",
        complexity: "simple",
        trigger: || WorkflowTrigger::Cron { schedule: "0 9 * * 1-5".to_string() },
        step_prompts: &[
            ("collect-prs", "List all open pull requests on the repository. For each PR, return: title, author, url, branch_name, description (first 200 chars). data must be a JSON array of objects with these fields.", true),
            ("check-tickets", "For each PR in {{previous_step.data}}, check if the title, description, or branch_name contains a Jira ticket reference (pattern: uppercase letters followed by a dash and digits, e.g. PROJ-123). Return only the PRs that have NO ticket reference. data must be an array of {title, author, url}.", true),
        ],
    },
    CatalogueEntry {
        id: "sprint-digest",
        title_fr: "Digest de sprint hebdomadaire",
        title_en: "Weekly sprint digest",
        desc_fr: "Résume les tickets fermés et les PRs mergées chaque vendredi",
        desc_en: "Summarize closed tickets and merged PRs every Friday",
        required_mcps: &["jira", "slack"],
        audience: "pm",
        complexity: "simple",
        trigger: || WorkflowTrigger::Cron { schedule: "0 17 * * 5".to_string() },
        step_prompts: &[
            ("collect-tickets", "Query Jira for all tickets resolved or closed in the last 7 days. For each: key, summary, type (Bug/Feature/Task), assignee. data must be a JSON array of these objects.", true),
            ("format-digest", "From the tickets in {{previous_step.data}}, generate a concise sprint digest grouped by type (Bug fixes, Features, Tasks). Include counts per category and the top 3 highlights.", false),
        ],
    },
    CatalogueEntry {
        id: "changelog-release",
        title_fr: "Changelog automatique à chaque release",
        title_en: "Auto-changelog on release",
        desc_fr: "Génère un changelog depuis les commits et PRs mergées entre deux tags",
        desc_en: "Generate a changelog from commits and merged PRs between two tags",
        required_mcps: &["github"],
        audience: "dev",
        complexity: "simple",
        trigger: || WorkflowTrigger::Manual,
        step_prompts: &[
            ("collect-prs", "List all merged pull requests since the last git tag. For each: number, title, author, labels (as array). data must be a JSON array of these objects.", true),
            ("generate-changelog", "From the PRs in {{previous_step.data}}, generate a changelog in Markdown. Group by type using labels or title prefixes: feat, fix, chore, docs. Format: `- title (#number) — @author`.", false),
        ],
    },
    CatalogueEntry {
        id: "stale-prs",
        title_fr: "Notification de PRs en attente",
        title_en: "Stale PR notifications",
        desc_fr: "Détecte les PRs ouvertes depuis plus de 48h sans review",
        desc_en: "Detect PRs open for 48h+ without review",
        required_mcps: &["github", "slack"],
        audience: "dev",
        complexity: "simple",
        trigger: || WorkflowTrigger::Cron { schedule: "0 10 * * 1-5".to_string() },
        step_prompts: &[
            ("find-stale", "List all open pull requests with zero reviews AND created more than 48 hours ago. For each: title, author, created_at, url. data must be a JSON array. If none found, use status NO_RESULTS with data as empty array [].", true),
            ("notify", "From the stale PRs in {{previous_step.data}}: format a notification listing each one with title, author, and days waiting. If {{previous_step.status}} is NO_RESULTS, just output 'No stale PRs found.'", false),
        ],
    },
    CatalogueEntry {
        id: "bug-report",
        title_fr: "Rapport de bugs par priorité",
        title_en: "Bug report by priority",
        desc_fr: "Génère un rapport mensuel des bugs ouverts classés par priorité",
        desc_en: "Generate a monthly report of open bugs sorted by priority",
        required_mcps: &["jira", "confluence"],
        audience: "pm",
        complexity: "simple",
        trigger: || WorkflowTrigger::Cron { schedule: "0 9 1 * *".to_string() },
        step_prompts: &[
            ("query-bugs", "Query Jira for all open issues of type Bug. For each: key, summary, priority (Critical/High/Medium/Low), created_date, assignee. data must be a JSON array.", true),
            ("generate-report", "From the bugs in {{previous_step.data}}: count by priority, list the top 5 oldest, note trends if visible. Generate a Markdown report.", false),
        ],
    },
    CatalogueEntry {
        id: "pr-quality",
        title_fr: "Analyse qualité sur chaque PR",
        title_en: "Code quality analysis per PR",
        desc_fr: "Analyse automatique du code de chaque nouvelle PR",
        desc_en: "Automatic code analysis on each new PR",
        required_mcps: &["github"],
        audience: "dev",
        complexity: "advanced",
        trigger: || WorkflowTrigger::Tracker {
            source: TrackerSourceConfig::GitHub { owner: String::new(), repo: String::new() },
            query: String::new(),
            labels: vec!["review-needed".to_string()],
            interval: "*/10 * * * *".to_string(),
        },
        step_prompts: &[
            ("fetch-diff", "Get the full diff of PR {{issue.title}} ({{issue.url}}). List all changed files with additions/deletions count. data must be a JSON object with fields: {files: [{path, additions, deletions}], total_changes: number}.", true),
            ("analyze", "Review the code changes from {{previous_step.data}}. Check for: 1) Security (injection, secrets, auth), 2) Performance (N+1, unbounded loops), 3) Missing error handling. data must be an array of {file, line, severity, issue, suggestion}. If no issues: status NO_RESULTS, data [].", true),
            ("review-summary", "Based on {{previous_step.data}}: if {{previous_step.status}} is NO_RESULTS, output 'LGTM — no issues detected'. Otherwise, write a concise PR review listing each issue with severity and suggested fix.", false),
        ],
    },
    CatalogueEntry {
        id: "5xx-correlation",
        title_fr: "Corrélation alertes 5xx / déploiements",
        title_en: "5xx alerts / deployment correlation",
        desc_fr: "Quand une alerte 5xx survient, identifie le dernier déploiement et les changements associés",
        desc_en: "When a 5xx alert fires, identify the last deployment and associated changes",
        required_mcps: &["cloudwatch", "github"],
        audience: "ops",
        complexity: "advanced",
        trigger: || WorkflowTrigger::Cron { schedule: "*/15 * * * *".to_string() },
        step_prompts: &[
            ("check-errors", "Query CloudWatch for HTTP 5xx error count in the last 15 minutes. data must be {count: number, endpoints: [{path, count}]}. If count is 0: status NO_RESULTS, data {count: 0, endpoints: []}.", true),
            ("find-deploys", "List the last 3 merged PRs on main (recent deployments). For each: title, author, merged_at, changed_files. data must be a JSON array.", true),
            ("correlate", "5xx errors: {{steps.check-errors.data}}. Recent deploys: {{previous_step.data}}. Identify the most likely cause based on timing and files changed. Output a concise incident summary.", false),
        ],
    },
    CatalogueEntry {
        id: "sprint-brief",
        title_fr: "Brief de sprint : livré, glissé, risques",
        title_en: "Sprint brief: delivered, slipped, risks",
        desc_fr: "Rapport de fin de sprint croisant Jira, GitHub et Confluence",
        desc_en: "End-of-sprint report crossing Jira, GitHub and Confluence",
        required_mcps: &["jira", "github", "confluence"],
        audience: "pm",
        complexity: "advanced",
        trigger: || WorkflowTrigger::Cron { schedule: "0 16 * * 5".to_string() },
        step_prompts: &[
            ("collect-sprint", "Get the current active sprint from Jira. List all tickets: key, summary, status, assignee, story_points. data must be a JSON array.", true),
            ("check-prs", "For each ticket in {{previous_step.data}}, check if there is a linked GitHub PR. data must be an array of {ticket_key, pr_status: 'merged'|'open'|'none', pr_url}.", true),
            ("classify", "Cross-reference tickets and PRs from {{previous_step.data}}. Classify: DELIVERED (done + merged), SLIPPED (not done or not merged), AT_RISK (in progress, no PR). data must be {delivered: [...], slipped: [...], at_risk: [...], delivery_rate: number, points_delivered: number, points_planned: number}.", true),
            ("format-brief", "Generate a sprint brief from {{previous_step.data}}: stats, top 3 deliveries, top 3 risks with reasons, one-paragraph recommendation.", false),
        ],
    },
    CatalogueEntry {
        id: "perf-monitoring",
        title_fr: "Alerting proactif sur anomalies de performance",
        title_en: "Proactive performance anomaly alerting",
        desc_fr: "Surveille les métriques CloudWatch et alerte sur les anomalies",
        desc_en: "Monitor CloudWatch metrics and alert on anomalies",
        required_mcps: &["cloudwatch", "slack"],
        audience: "ops",
        complexity: "advanced",
        trigger: || WorkflowTrigger::Cron { schedule: "*/30 * * * *".to_string() },
        step_prompts: &[
            ("collect-metrics", "Query CloudWatch for: latency_p99_ms, error_rate_percent, cpu_percent — last 30 min. data must be {latency_p99_ms: number, error_rate_percent: number, cpu_percent: number}.", true),
            ("detect-anomalies", "Current metrics: {{previous_step.data}}. Compare against 7-day average for same time window. data must be an array of {metric, current, baseline, factor}. If all normal: status NO_RESULTS, data [].", true),
            ("alert", "Anomalies: {{previous_step.data}}. For each: format metric name, current vs baseline, deviation. Recommend action if any >5x baseline.", false),
        ],
    },
    CatalogueEntry {
        id: "doc-sync",
        title_fr: "Synchronisation documentation technique",
        title_en: "Technical documentation sync",
        desc_fr: "Détecte quand le code a changé mais la doc Confluence n'a pas été mise à jour",
        desc_en: "Detect when code changed but Confluence docs were not updated",
        required_mcps: &["github", "confluence"],
        audience: "dev",
        complexity: "advanced",
        trigger: || WorkflowTrigger::Cron { schedule: "0 10 * * 1".to_string() },
        step_prompts: &[
            ("find-api-changes", "List PRs merged in the last 7 days that modified **/routes/**, **/api/**, **/models/**, **/schema/**. For each: pr_title, changed_files. data must be a JSON array.", true),
            ("check-docs", "For each PR in {{previous_step.data}}, search Confluence for related pages. Check if updated in last 7 days. data must be [{pr_title, page_title, page_url, last_updated, is_stale: bool}].", true),
            ("report-stale", "From {{previous_step.data}}, filter is_stale=true. If none: status NO_RESULTS. Otherwise list each stale doc with page title, URL, related PR, and last update date.", false),
        ],
    },
];

/// Normalize MCP server names to catalogue keys.
/// e.g. "mcp-github" → "github", "mcp-atlassian" → "jira", "Jira" → "jira"
fn normalize_mcp_name(name: &str) -> String {
    let n = name.to_lowercase()
        .replace("mcp-", "")
        .replace("server-", "");
    // Known aliases
    if n.contains("atlassian") { return "jira".to_string(); }
    if n.contains("cloudwatch") { return "cloudwatch".to_string(); }
    n
}

/// GET /api/projects/:id/workflow-suggestions
pub async fn suggestions(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Json<ApiResponse<Vec<WorkflowSuggestion>>> {
    // 1. Get project
    let project = match state.db.with_conn({
        let pid = project_id.clone();
        move |conn| crate::db::projects::get_project(conn, &pid)
    }).await {
        Ok(Some(p)) => p,
        _ => return Json(ApiResponse::ok(vec![])),
    };

    // 2. Get MCP configs linked to this project (global + project-specific)
    let mcp_names: Vec<String> = match state.db.with_conn({
        let pid = project_id.clone();
        move |conn| {
            let configs = crate::db::mcps::list_configs_display(conn, None)?;
            let names: Vec<String> = configs.into_iter()
                .filter(|c| c.is_global || c.project_ids.contains(&pid))
                .map(|c| normalize_mcp_name(&c.server_name))
                .collect();
            Ok(names)
        }
    }).await {
        Ok(n) => n,
        Err(_) => return Json(ApiResponse::ok(vec![])),
    };

    if mcp_names.is_empty() {
        return Json(ApiResponse::ok(vec![]));
    }

    // 3. Also try to read workflow hints from ai/operations/mcp-servers.md (if audited)
    let project_path = crate::core::scanner::resolve_host_path(&project.path);
    let _hints_path = project_path.join("ai/operations/mcp-servers.md");
    // Future: parse the hints table for project-specific suggestions.
    // For now, we use only the static catalogue.

    // 4. Detect language (fr default)
    let lang = state.config.read().await.language.clone();
    let is_fr = lang.starts_with("fr");

    // 5. Match catalogue entries against available MCPs
    let mut suggestions = Vec::new();
    for entry in CATALOGUE {
        let all_present = entry.required_mcps.iter()
            .all(|req| mcp_names.iter().any(|m| m.contains(req)));
        if !all_present {
            continue;
        }

        let title = if is_fr { entry.title_fr } else { entry.title_en };
        let desc = if is_fr { entry.desc_fr } else { entry.desc_en };

        let reason = if is_fr {
            format!("Vous avez {} connectés", entry.required_mcps.join(" + "))
        } else {
            format!("You have {} connected", entry.required_mcps.join(" + "))
        };

        suggestions.push(WorkflowSuggestion {
            id: entry.id.to_string(),
            title: title.to_string(),
            description: desc.to_string(),
            reason,
            required_mcps: entry.required_mcps.iter().map(|s| s.to_string()).collect(),
            audience: entry.audience.to_string(),
            complexity: entry.complexity.to_string(),
            trigger: (entry.trigger)(),
            steps: entry.step_prompts.iter().map(|(step_name, prompt, structured)| WorkflowStep {
                name: step_name.to_string(),
                step_type: StepType::Agent,
                description: None,
                agent: AgentType::ClaudeCode,
                prompt_template: prompt.to_string(),
                mode: StepMode::Normal,
                output_format: if *structured { StepOutputFormat::Structured } else { StepOutputFormat::FreeText },
                on_result: vec![],
                agent_settings: None,
                stall_timeout_secs: None,
                retry: None,
                delay_after_secs: None,
                mcp_config_ids: vec![],
                skill_ids: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                batch_quick_prompt_id: None,
                batch_items_from: None,
                batch_wait_for_completion: None,
                batch_max_items: None,
                batch_workspace_mode: None,
                notify_config: None,
            }).collect(),
        });
    }

    Json(ApiResponse::ok(suggestions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_mcp_names() {
        assert_eq!(normalize_mcp_name("mcp-github"), "github");
        assert_eq!(normalize_mcp_name("GitHub"), "github");
        assert_eq!(normalize_mcp_name("mcp-atlassian"), "jira");
        assert_eq!(normalize_mcp_name("Atlassian"), "jira");
        assert_eq!(normalize_mcp_name("awslabs.cloudwatch-mcp-server"), "cloudwatch");
        assert_eq!(normalize_mcp_name("Slack"), "slack");
    }

    #[test]
    fn catalogue_has_entries() {
        assert!(CATALOGUE.len() >= 10, "Catalogue should have at least 10 entries");
    }

    #[test]
    fn catalogue_entries_have_valid_fields() {
        for entry in CATALOGUE {
            assert!(!entry.id.is_empty(), "Entry must have an id");
            assert!(!entry.title_fr.is_empty(), "Entry {} must have a French title", entry.id);
            assert!(!entry.title_en.is_empty(), "Entry {} must have an English title", entry.id);
            assert!(!entry.required_mcps.is_empty(), "Entry {} must require at least one MCP", entry.id);
            assert!(
                ["dev", "pm", "ops"].contains(&entry.audience),
                "Entry {} has invalid audience: {}", entry.id, entry.audience
            );
            assert!(
                ["simple", "advanced"].contains(&entry.complexity),
                "Entry {} has invalid complexity: {}", entry.id, entry.complexity
            );
            assert!(!entry.step_prompts.is_empty(), "Entry {} must have at least one step", entry.id);
            for (sname, sprompt, _structured) in entry.step_prompts {
                assert!(!sname.is_empty(), "Entry {} has a step with empty name", entry.id);
                assert!(!sprompt.is_empty(), "Entry {} step '{}' has empty prompt", entry.id, sname);
            }
        }
    }

    #[test]
    fn catalogue_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for entry in CATALOGUE {
            assert!(seen.insert(entry.id), "Duplicate catalogue id: {}", entry.id);
        }
    }

    #[test]
    fn github_only_suggestions_match() {
        let mcps = ["github".to_string()];
        let matches: Vec<&str> = CATALOGUE.iter()
            .filter(|e| e.required_mcps.iter().all(|req| mcps.iter().any(|m| m.contains(req))))
            .map(|e| e.id)
            .collect();
        assert!(matches.contains(&"changelog-release"), "GitHub alone should suggest changelog");
        assert!(matches.contains(&"pr-quality"), "GitHub alone should suggest PR quality");
        assert!(!matches.contains(&"orphan-prs"), "GitHub alone should NOT suggest orphan PRs (needs jira)");
    }

    #[test]
    fn github_jira_suggestions_match() {
        let mcps = ["github".to_string(), "jira".to_string()];
        let matches: Vec<&str> = CATALOGUE.iter()
            .filter(|e| e.required_mcps.iter().all(|req| mcps.iter().any(|m| m.contains(req))))
            .map(|e| e.id)
            .collect();
        assert!(matches.contains(&"orphan-prs"), "GitHub+Jira should suggest orphan PRs");
        assert!(matches.contains(&"changelog-release"), "GitHub+Jira should still suggest changelog");
    }
}
