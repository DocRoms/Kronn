use axum::{
    extract::{Path, State},
    Json,
};

use crate::{
    db::id_resolver::ResolvedId,
    models::{ApiErrorCode, ApiResponse},
    AppState,
};

/// Resolve an opaque Kronn object ID without making the caller probe every
/// object-specific endpoint.
pub async fn resolve(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ResolvedId>> {
    match state
        .db
        .with_read_conn(move |connection| crate::db::id_resolver::resolve_id(connection, &id))
        .await
    {
        Ok(Some(resolved)) => Json(ApiResponse::ok(resolved)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Kronn object not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            error.to_string(),
        )),
    }
}
