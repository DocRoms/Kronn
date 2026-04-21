//! Profiles API — list, create, update, delete agent profiles.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::core::profiles;
use crate::models::*;
use crate::AppState;

/// GET /api/profiles — list all profiles (builtin + custom), minus any
/// secret built-in that the operator hasn't unlocked.
pub async fn list(state: State<AppState>) -> Json<ApiResponse<Vec<AgentProfile>>> {
    let unlocked = state.config.read().await.unlocked_profiles.clone();
    let visible: Vec<_> = profiles::list_all_profiles()
        .into_iter()
        .filter(|p| !profiles::is_secret_profile(&p.id) || unlocked.iter().any(|u| u == &p.id))
        .collect();
    Json(ApiResponse::ok(visible))
}

/// GET /api/profiles/:id — get a single profile. Returns 404 (via
/// ApiResponse::err) if the id is a secret and not unlocked — same
/// payload shape as a truly missing profile, so an attacker probing
/// ids can't distinguish "locked" from "doesn't exist".
pub async fn get(
    Path(id): Path<String>,
    state: State<AppState>,
) -> Json<ApiResponse<AgentProfile>> {
    if profiles::is_secret_profile(&id) {
        let unlocked = state.config.read().await.unlocked_profiles.clone();
        if !unlocked.iter().any(|u| u == &id) {
            return Json(ApiResponse::err("Profile not found"));
        }
    }
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
