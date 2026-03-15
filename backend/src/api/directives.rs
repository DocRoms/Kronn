//! Directives API — list, create, update, delete directives.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::core::directives;
use crate::models::*;
use crate::AppState;

/// GET /api/directives — list all directives (builtin + custom)
pub async fn list(_state: State<AppState>) -> Json<ApiResponse<Vec<Directive>>> {
    let all = directives::list_all_directives();
    Json(ApiResponse::ok(all))
}

/// POST /api/directives — create a custom directive
pub async fn create(
    _state: State<AppState>,
    Json(req): Json<CreateDirectiveRequest>,
) -> Json<ApiResponse<Directive>> {
    match directives::save_custom_directive(&req.name, &req.description, &req.icon, &req.category, &req.content, &req.conflicts) {
        Ok(id) => {
            match directives::get_directive(&id) {
                Some(directive) => Json(ApiResponse::ok(directive)),
                None => Json(ApiResponse::err("Directive created but could not be loaded")),
            }
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// PUT /api/directives/:id — update a custom directive
pub async fn update(
    Path(id): Path<String>,
    _state: State<AppState>,
    Json(req): Json<CreateDirectiveRequest>,
) -> Json<ApiResponse<Directive>> {
    if !id.starts_with("custom-") {
        return Json(ApiResponse::err("Cannot modify builtin directives"));
    }

    let _ = directives::delete_custom_directive(&id);

    match directives::save_custom_directive(&req.name, &req.description, &req.icon, &req.category, &req.content, &req.conflicts) {
        Ok(new_id) => {
            match directives::get_directive(&new_id) {
                Some(directive) => Json(ApiResponse::ok(directive)),
                None => Json(ApiResponse::err("Directive updated but could not be loaded")),
            }
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// DELETE /api/directives/:id — delete a custom directive
pub async fn delete(
    Path(id): Path<String>,
    _state: State<AppState>,
) -> Json<ApiResponse<bool>> {
    match directives::delete_custom_directive(&id) {
        Ok(deleted) => Json(ApiResponse::ok(deleted)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}
