use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::db::discussion_questions::{DeclineDiscussionQuestionRequest,
    self, AnswerDiscussionQuestionRequest, AnswerError, DiscussionQuestion, DiscussionQuestionList,
};
use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

pub async fn list(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<DiscussionQuestionList>>) {
    let result = state
        .db
        .with_read_conn(move |conn| {
            // This endpoint is polled: checking existence must not load the transcript.
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM discussions WHERE id=?1)",
                [&discussion_id],
                |row| row.get(0),
            )?;
            if !exists {
                return Ok(None);
            }
            discussion_questions::list(conn, &discussion_id).map(Some)
        })
        .await;
    match result {
        Ok(Some(list)) => (StatusCode::OK, Json(ApiResponse::ok(list))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Discussion not found",
            )),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}

pub async fn answer(
    State(state): State<AppState>,
    Path((discussion_id, question_id)): Path<(String, String)>,
    Json(request): Json<AnswerDiscussionQuestionRequest>,
) -> (StatusCode, Json<ApiResponse<DiscussionQuestion>>) {
    let (pseudo, email) = {
        let config = state.config.read().await;
        (
            config
                .server
                .pseudo
                .clone()
                .unwrap_or_else(|| "Human".into()),
            config.server.avatar_email.clone(),
        )
    };
    let result = state
        .db
        .with_conn(move |conn| {
            Ok(discussion_questions::answer(
                conn,
                &discussion_id,
                &question_id,
                &request,
                &pseudo,
                email.as_deref(),
            ))
        })
        .await;
    match result {
        Ok(Ok(question)) => {
            // Wake the durable queue promptly; replay never enqueues a second job.
            state.agent_dispatch_notify.notify_one();
            (StatusCode::OK, Json(ApiResponse::ok(question)))
        }
        Ok(Err(AnswerError::NotFound)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Discussion question not found",
            )),
        ),
        Ok(Err(AnswerError::Conflict)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                "This question has already been answered",
            )),
        ),
        Ok(Err(AnswerError::Invalid(message))) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
        ),
        Ok(Err(AnswerError::Failed(error))) | Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}

/// Refuse an arbitration. Same shape as `answer`: the refusal is a decision,
/// so it is published, routed to the asker and dispatched the same way.
pub async fn decline(
    State(state): State<AppState>,
    Path((discussion_id, question_id)): Path<(String, String)>,
    Json(request): Json<DeclineDiscussionQuestionRequest>,
) -> (StatusCode, Json<ApiResponse<DiscussionQuestion>>) {
    let (pseudo, email) = {
        let config = state.config.read().await;
        (
            config
                .server
                .pseudo
                .clone()
                .unwrap_or_else(|| "Human".into()),
            config.server.avatar_email.clone(),
        )
    };
    let result = state
        .db
        .with_conn(move |conn| {
            Ok(discussion_questions::decline(
                conn,
                &discussion_id,
                &question_id,
                &request,
                &pseudo,
                email.as_deref(),
            ))
        })
        .await;
    match result {
        Ok(Ok(question)) => {
            state.agent_dispatch_notify.notify_one();
            (StatusCode::OK, Json(ApiResponse::ok(question)))
        }
        Ok(Err(AnswerError::NotFound)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Discussion question not found",
            )),
        ),
        Ok(Err(AnswerError::Conflict)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                "This question has already been decided",
            )),
        ),
        Ok(Err(AnswerError::Invalid(message))) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
        ),
        Ok(Err(AnswerError::Failed(error))) | Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}
