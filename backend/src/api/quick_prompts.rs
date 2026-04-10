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

    let now = Utc::now();
    let run_id = Uuid::new_v4().to_string();
    let batch_total = req.items.len() as u32;

    // Read user identity for message attribution
    let (author_pseudo, author_avatar_email) = {
        let config = state.config.read().await;
        (config.server.pseudo.clone(), config.server.avatar_email.clone())
    };

    // Build the WorkflowRun + all child Discussion rows ahead of time so the
    // whole insert happens in a single transaction. If anything fails, no
    // partial state is left behind.
    let run = WorkflowRun {
        id: run_id.clone(),
        // Batch runs are not tied to a saved Workflow — use the QP id as the
        // "virtual workflow id" so the existing list_runs(workflow_id) query
        // still works and users can view all runs for a QP in one place.
        workflow_id: format!("qp:{}", qp.id),
        status: RunStatus::Running,
        trigger_context: Some(serde_json::json!({
            "type": "batch",
            "quick_prompt_id": qp.id,
            "quick_prompt_name": qp.name,
            "batch_size": batch_total,
        })),
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "batch".into(),
        batch_total,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: Some(req.batch_name.clone()),
    };

    // Build each child discussion with the fully-rendered prompt as its
    // initial message, all linked back to the run via workflow_run_id.
    let discussions: Vec<(Discussion, DiscussionMessage)> = req.items.iter().map(|item| {
        let disc_id = Uuid::new_v4().to_string();
        let initial_message = DiscussionMessage {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content: item.prompt.clone(),
            agent_type: None,
            timestamp: now,
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: author_pseudo.clone(),
            author_avatar_email: author_avatar_email.clone(),
        };
        let discussion = Discussion {
            id: disc_id,
            project_id: req.project_id.clone().or_else(|| qp.project_id.clone()),
            title: item.title.clone(),
            agent: qp.agent.clone(),
            language: "fr".into(), // Batch disc inherits the server default language
            participants: vec![qp.agent.clone()],
            messages: vec![initial_message.clone()],
            message_count: 1,
            skill_ids: qp.skill_ids.clone(),
            profile_ids: vec![],
            directive_ids: vec![],
            archived: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            tier: qp.tier,
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            shared_id: None,
            shared_with: vec![],
            workflow_run_id: Some(run_id.clone()),
            created_at: now,
            updated_at: now,
        };
        (discussion, initial_message)
    }).collect();

    let discussion_ids: Vec<String> = discussions.iter().map(|(d, _)| d.id.clone()).collect();

    // Single transaction: ensure the batch placeholder workflow exists,
    // insert the run, then all discussions and their initial messages.
    // Using a placeholder keeps the workflow_runs FK intact without needing
    // a schema change — the placeholder row is filtered out of the
    // user-facing workflows list (see BATCH_WORKFLOW_PREFIX).
    let run_for_db = run.clone();
    let discs_for_db = discussions;
    let qp_id_for_tx = qp.id.clone();
    let qp_name_for_tx = qp.name.clone();
    let qp_project_for_tx = qp.project_id.clone();
    if let Err(e) = state.db.with_conn(move |conn| {
        conn.execute_batch("BEGIN")?;
        let tx_result: anyhow::Result<()> = (|| {
            crate::db::workflows::ensure_batch_placeholder_workflow(
                conn, &qp_id_for_tx, &qp_name_for_tx, qp_project_for_tx.as_deref(),
            )?;
            crate::db::workflows::insert_run(conn, &run_for_db)?;
            for (disc, msg) in &discs_for_db {
                crate::db::discussions::insert_discussion(conn, disc)?;
                crate::db::discussions::insert_message(conn, &disc.id, msg)?;
            }
            Ok(())
        })();
        if let Err(e) = tx_result {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }
        conn.execute_batch("COMMIT")?;
        Ok(())
    }).await {
        return Json(ApiResponse::err(format!("Failed to create batch: {}", e)));
    }

    tracing::info!(
        "Created batch run {} with {} discussions (QP: {}, name: {})",
        run_id, batch_total, qp.name, req.batch_name
    );

    Json(ApiResponse::ok(BatchRunResponse {
        run_id,
        discussion_ids,
        batch_total,
    }))
}
