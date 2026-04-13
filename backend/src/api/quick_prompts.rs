use axum::{extract::{Path, State}, Json};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

/// GET /api/quick-prompts
pub async fn list(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<QuickPrompt>>> {
    match state.db.with_conn(crate::db::quick_prompts::list_quick_prompts).await {
        Ok(items) => Json(ApiResponse::ok(items)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/quick-prompts
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateQuickPromptRequest>,
) -> Json<ApiResponse<QuickPrompt>> {
    if req.name.is_empty() || req.name.len() > 200 {
        return Json(ApiResponse::err("Name must be 1-200 characters"));
    }
    if req.prompt_template.is_empty() {
        return Json(ApiResponse::err("Prompt template cannot be empty"));
    }

    let now = Utc::now();
    let qp = QuickPrompt {
        id: Uuid::new_v4().to_string(),
        name: req.name,
        icon: req.icon.unwrap_or_else(|| "⚡".into()),
        prompt_template: req.prompt_template,
        variables: req.variables,
        agent: req.agent.unwrap_or(AgentType::ClaudeCode),
        project_id: req.project_id,
        skill_ids: req.skill_ids,
        tier: req.tier,
        description: req.description,
        created_at: now,
        updated_at: now,
    };

    let q = qp.clone();
    match state.db.with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &q)).await {
        Ok(()) => Json(ApiResponse::ok(qp)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/quick-prompts/:id
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateQuickPromptRequest>,
) -> Json<ApiResponse<QuickPrompt>> {
    let qp_id = id.clone();
    let existing = match state.db.with_conn(move |conn| crate::db::quick_prompts::get_quick_prompt(conn, &qp_id)).await {
        Ok(Some(qp)) => qp,
        Ok(None) => return Json(ApiResponse::err("Quick prompt not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let updated = QuickPrompt {
        id: existing.id,
        name: if req.name.is_empty() { existing.name } else { req.name },
        icon: req.icon.unwrap_or(existing.icon),
        prompt_template: if req.prompt_template.is_empty() { existing.prompt_template } else { req.prompt_template },
        variables: req.variables,
        agent: req.agent.unwrap_or(existing.agent),
        project_id: req.project_id,
        skill_ids: req.skill_ids,
        tier: req.tier,
        // Description is always taken from the request, even if empty —
        // that's how the user clears it.
        description: req.description,
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };

    let q = updated.clone();
    match state.db.with_conn(move |conn| crate::db::quick_prompts::update_quick_prompt(conn, &q)).await {
        Ok(()) => Json(ApiResponse::ok(updated)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/quick-prompts/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::quick_prompts::delete_quick_prompt(conn, &id)).await {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Batch execution — fan out a Quick Prompt to N discussions in parallel
// ═══════════════════════════════════════════════════════════════════════════════

/// One item in the batch list: the title for the discussion and the fully
/// rendered user prompt. The frontend does the template rendering (it already
/// has `renderTemplate` from the QP launch flow) so the backend just receives
/// a list of already-filled prompts.
#[derive(Debug, Deserialize)]
pub struct BatchItem {
    pub title: String,
    pub prompt: String,
}

#[derive(Debug, Deserialize)]
pub struct BatchRunRequest {
    pub items: Vec<BatchItem>,
    /// Display name for the batch group in the sidebar.
    /// Example: "Cadrage to-Frame — 10 avr 14:00"
    pub batch_name: String,
    /// Optional project ID to attach all child discussions to.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Workspace mode for each child discussion: `"Direct"` (default) or
    /// `"Isolated"` for per-disc git worktrees. Isolated is required when
    /// the agents will write code in parallel.
    #[serde(default)]
    pub workspace_mode: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct BatchRunResponse {
    pub run_id: String,
    pub discussion_ids: Vec<String>,
    pub batch_total: u32,
}

/// POST /api/quick-prompts/:id/batch
///
/// Create N child discussions from a Quick Prompt + list of pre-rendered
/// prompts. All discussions are linked to a single batch WorkflowRun so the
/// frontend can group them in the sidebar and track progress live.
///
/// The actual agent runs are NOT started here — the frontend walks the
/// returned `discussion_ids` list and hits `POST /api/discussions/:id/run`
/// on each, honoring the existing `agent_semaphore` for parallelism control.
/// This keeps the backend simple and reuses the per-disc streaming pipeline
/// unchanged.
pub async fn batch_run(
    State(state): State<AppState>,
    Path(qp_id): Path<String>,
    Json(req): Json<BatchRunRequest>,
) -> Json<ApiResponse<BatchRunResponse>> {
    // Hard cap to prevent accidental megabatches
    const MAX_BATCH_SIZE: usize = 50;
    if req.items.is_empty() {
        return Json(ApiResponse::err("Batch must contain at least 1 item"));
    }
    if req.items.len() > MAX_BATCH_SIZE {
        return Json(ApiResponse::err(format!(
            "Batch too large: {} items (max {})", req.items.len(), MAX_BATCH_SIZE
        )));
    }
    if req.batch_name.trim().is_empty() {
        return Json(ApiResponse::err("batch_name is required"));
    }

    // Load the QP to get agent + skill_ids + tier
    let qp_lookup = qp_id.clone();
    let qp = match state.db.with_conn(move |conn| {
        crate::db::quick_prompts::get_quick_prompt(conn, &qp_lookup)
    }).await {
        Ok(Some(q)) => q,
        Ok(None) => return Json(ApiResponse::err("Quick prompt not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    // Read user identity for message attribution
    let (author_pseudo, author_avatar_email) = {
        let config = state.config.read().await;
        (config.server.pseudo.clone(), config.server.avatar_email.clone())
    };

    // Delegate to the shared pure fn — same logic as the workflow step executor.
    let batch_name_for_log = req.batch_name.clone();
    let qp_name_for_log = qp.name.clone();
    let items: Vec<(String, String)> = req.items.into_iter().map(|i| (i.title, i.prompt)).collect();
    let workspace_mode = req.workspace_mode.unwrap_or_else(|| "Direct".into());

    // Safety: Isolated mode needs a project (git repo) to worktree against.
    // Check the effective project_id (request override OR QP default).
    if workspace_mode == "Isolated"
        && req.project_id.is_none()
        && qp.project_id.is_none()
    {
        return Json(ApiResponse::err(
            "Isolated workspace mode requires a project_id (the Quick Prompt or the batch request must target a git-backed project)"
        ));
    }

    let outcome = match state.db.with_conn(move |conn| {
        crate::db::workflows::create_batch_run(conn, crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp,
            items,
            batch_name: Some(req.batch_name),
            project_id: req.project_id,
            parent_run_id: None,
            author_pseudo,
            author_avatar_email,
            language: "fr".into(),
            workspace_mode,
        })
    }).await {
        Ok(o) => o,
        Err(e) => return Json(ApiResponse::err(format!("Failed to create batch: {}", e))),
    };

    tracing::info!(
        "Created batch run {} with {} discussions (QP: {}, name: {})",
        outcome.run_id, outcome.batch_total, qp_name_for_log, batch_name_for_log
    );

    Json(ApiResponse::ok(BatchRunResponse {
        run_id: outcome.run_id,
        discussion_ids: outcome.discussion_ids,
        batch_total: outcome.batch_total,
    }))
}
