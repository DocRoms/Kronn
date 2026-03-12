//! Profiles API — list, create, update, delete agent profiles.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::core::profiles;
use crate::models::*;
use crate::AppState;

/// GET /api/profiles — list all profiles (builtin + custom)
pub async fn list(_state: State<AppState>) -> Json<ApiResponse<Vec<AgentProfile>>> {
    let all = profiles::list_all_profiles();
    Json(ApiResponse::ok(all))
}

/// GET /api/profiles/:id — get a single profile
pub async fn get(
    Path(id): Path<String>,
    _state: State<AppState>,
) -> Json<ApiResponse<AgentProfile>> {
    match profiles::get_profile(&id) {
        Some(profile) => Json(ApiResponse::ok(profile)),
        None => Json(ApiResponse::err("Profile not found")),
    }
}

/// POST /api/profiles — create a custom profile
pub async fn create(
    _state: State<AppState>,
    Json(req): Json<CreateProfileRequest>,
) -> Json<ApiResponse<AgentProfile>> {
    let data = profiles::CustomProfileData {
        name: &req.name,
        persona_name: &req.persona_name,
        role: &req.role,
        avatar: &req.avatar,
        color: &req.color,
        category: &req.category,
        persona_prompt: &req.persona_prompt,
        default_engine: req.default_engine.as_deref(),
    };
    match profiles::save_custom_profile(&data) {
        Ok(id) => {
            match profiles::get_profile(&id) {
                Some(profile) => Json(ApiResponse::ok(profile)),
                None => Json(ApiResponse::err("Profile created but could not be loaded")),
            }
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// PUT /api/profiles/:id — update a custom profile
pub async fn update(
    Path(id): Path<String>,
    _state: State<AppState>,
    Json(req): Json<CreateProfileRequest>,
) -> Json<ApiResponse<AgentProfile>> {
    if !id.starts_with("custom-") {
        return Json(ApiResponse::err("Cannot modify builtin profiles"));
    }

    let _ = profiles::delete_custom_profile(&id);

    let data = profiles::CustomProfileData {
        name: &req.name,
        persona_name: &req.persona_name,
        role: &req.role,
        avatar: &req.avatar,
        color: &req.color,
        category: &req.category,
        persona_prompt: &req.persona_prompt,
        default_engine: req.default_engine.as_deref(),
    };
    match profiles::save_custom_profile(&data) {
        Ok(new_id) => {
            match profiles::get_profile(&new_id) {
                Some(profile) => Json(ApiResponse::ok(profile)),
                None => Json(ApiResponse::err("Profile updated but could not be loaded")),
            }
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// PUT /api/profiles/:id/persona-name — update persona name (works for builtins too)
pub async fn update_persona_name(
    Path(id): Path<String>,
    _state: State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Json<ApiResponse<AgentProfile>> {
    let persona_name = body.get("persona_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match profiles::save_persona_override(&id, persona_name) {
        Ok(()) => {
            match profiles::get_profile(&id) {
                Some(profile) => Json(ApiResponse::ok(profile)),
                None => Json(ApiResponse::err("Profile not found")),
            }
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// DELETE /api/profiles/:id — delete a custom profile
pub async fn delete(
    Path(id): Path<String>,
    _state: State<AppState>,
) -> Json<ApiResponse<bool>> {
    match profiles::delete_custom_profile(&id) {
        Ok(deleted) => Json(ApiResponse::ok(deleted)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}
