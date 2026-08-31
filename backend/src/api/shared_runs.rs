use crate::{
    models::{ApiResponse, SharedRun},
    AppState,
};
use axum::{
    extract::{Path, State},
    Json,
};

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<SharedRun>> {
    match state
        .db
        .with_conn(move |conn| crate::db::shared_runs::get(conn, &id))
        .await
    {
        Ok(Some(run)) => Json(ApiResponse::ok(run)),
        Ok(None) => Json(ApiResponse::err("Run not found")),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}
