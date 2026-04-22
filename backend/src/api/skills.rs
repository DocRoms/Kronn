//! Skills API — list, create, update, delete skills.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::core::skills;
use crate::models::*;
use crate::AppState;

/// GET /api/skills — list all skills (builtin + custom)
pub async fn list(_state: State<AppState>) -> Json<ApiResponse<Vec<Skill>>> {
    let all = skills::list_all_skills();
    Json(ApiResponse::ok(all))
}

/// POST /api/skills — create a custom skill
pub async fn create(
    _state: State<AppState>,
    Json(req): Json<CreateSkillRequest>,
) -> Json<ApiResponse<Skill>> {
    match skills::save_custom_skill(&req.name, &req.description, &req.icon, &req.category, &req.content, req.license.as_deref(), req.allowed_tools.as_deref()) {
        Ok(id) => {
            match skills::get_skill(&id) {
                Some(skill) => Json(ApiResponse::ok(skill)),
                None => Json(ApiResponse::err("Skill created but could not be loaded")),
            }
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// PUT /api/skills/:id — update a custom skill
pub async fn update(
    Path(id): Path<String>,
    _state: State<AppState>,
    Json(req): Json<CreateSkillRequest>,
) -> Json<ApiResponse<Skill>> {
    if !id.starts_with("custom-") {
        return Json(ApiResponse::err("Cannot modify builtin skills"));
    }

    let _ = skills::delete_custom_skill(&id);

    match skills::save_custom_skill(&req.name, &req.description, &req.icon, &req.category, &req.content, req.license.as_deref(), req.allowed_tools.as_deref()) {
        Ok(new_id) => {
            match skills::get_skill(&new_id) {
                Some(skill) => Json(ApiResponse::ok(skill)),
                None => Json(ApiResponse::err("Skill updated but could not be loaded")),
            }
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// DELETE /api/skills/:id — delete a custom skill
pub async fn delete(
    Path(id): Path<String>,
    _state: State<AppState>,
) -> Json<ApiResponse<bool>> {
    match skills::delete_custom_skill(&id) {
        Ok(deleted) => Json(ApiResponse::ok(deleted)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/skills/auto-triggers/disabled — returns the list of skill
/// IDs for which auto-activation (keyword-based) is OFF. The frontend
/// fetches this once on mount and filters its local
/// `detectTriggeredSkills()` output against it.
pub async fn list_disabled_auto(
    state: State<AppState>,
) -> Json<ApiResponse<Vec<String>>> {
    let ids = state.config.read().await.disabled_auto_skills.clone();
    Json(ApiResponse::ok(ids))
}

/// POST /api/skills/:id/auto-trigger/toggle — flips the opt-out state
/// for one skill. Returns the new `disabled` boolean (true = auto-
/// activation is OFF for this skill, false = default/enabled).
/// Idempotent: sending twice ends up where it started. Also persists
/// to config.toml so the choice survives restarts.
pub async fn toggle_auto_trigger(
    Path(id): Path<String>,
    state: State<AppState>,
) -> Json<ApiResponse<bool>> {
    let mut cfg = state.config.write().await;
    let pos = cfg.disabled_auto_skills.iter().position(|s| s == &id);
    let now_disabled = match pos {
        Some(i) => {
            cfg.disabled_auto_skills.remove(i);
            false
        }
        None => {
            cfg.disabled_auto_skills.push(id.clone());
            true
        }
    };
    if let Err(e) = crate::core::config::save(&cfg).await {
        tracing::warn!("Failed to persist disabled_auto_skills toggle: {e}");
    }
    Json(ApiResponse::ok(now_disabled))
}
