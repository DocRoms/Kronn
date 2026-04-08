use axum::{extract::{Path, State}, Json};
use chrono::Utc;
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
