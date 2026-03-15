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
    match skills::save_custom_skill(&req.name, &req.description, &req.icon, &req.category, &req.content) {
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

    match skills::save_custom_skill(&req.name, &req.description, &req.icon, &req.category, &req.content) {
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
