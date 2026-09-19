//! Assets > Editor: the order a discussion's clips are played in as one film,
//! and the clips set aside from it.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

/// The clips a person arranged. Read back filtered to the files the
/// discussion still has.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VideoSequence {
    /// Played, in this order.
    pub file_ids: Vec<String>,
    /// Kept by the discussion, not played.
    pub excluded_ids: Vec<String>,
}

enum Outcome {
    Missing,
    Refused(String),
    Order(crate::db::discussion_video_sequences::Sequence),
}

fn answer(outcome: anyhow::Result<Outcome>) -> Json<ApiResponse<VideoSequence>> {
    match outcome {
        Ok(Outcome::Order(stored)) => Json(ApiResponse::ok(VideoSequence {
            file_ids: stored.file_ids,
            excluded_ids: stored.excluded_ids,
        })),
        Ok(Outcome::Missing) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Discussion not found",
        )),
        Ok(Outcome::Refused(reason)) => {
            Json(ApiResponse::err_coded(ApiErrorCode::Validation, reason))
        }
        Err(error) => Json(ApiResponse::err(format!(
            "Unable to read the clip order: {error}"
        ))),
    }
}

pub async fn get(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
) -> Json<ApiResponse<VideoSequence>> {
    answer(
        state
            .db
            .with_read_conn(move |conn| {
                if crate::db::discussions::get_discussion(conn, &discussion_id)?.is_none() {
                    return Ok(Outcome::Missing);
                }
                Ok(Outcome::Order(crate::db::discussion_video_sequences::get(
                    conn,
                    &discussion_id,
                )?))
            })
            .await,
    )
}

pub async fn set(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
    Json(request): Json<VideoSequence>,
) -> Json<ApiResponse<VideoSequence>> {
    answer(
        state
            .db
            .with_conn(move |conn| {
                if crate::db::discussions::get_discussion(conn, &discussion_id)?.is_none() {
                    return Ok(Outcome::Missing);
                }
                Ok(
                    match crate::db::discussion_video_sequences::set(
                        conn,
                        &discussion_id,
                        &request.file_ids,
                        &request.excluded_ids,
                    ) {
                        Ok(stored) => Outcome::Order(stored),
                        Err(reason) => Outcome::Refused(reason.to_string()),
                    },
                )
            })
            .await,
    )
}
