//! KT-1270 — a room's reach beyond its primary project. Every route here but
//! `file_request` is human-only: none is a bridge route, so an agent's token is
//! refused before the handler.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::db::discussion_scope::{self, Filed, ScopeError};
use crate::models::discussion_scope::{
    DecideScopeRequest, DiscussionProjectGrant, DiscussionScope, DiscussionScopeRequest,
    FileScopeRequest, GrantDiscussionProjectRequest, RevokeDiscussionProjectRequest, ScopeAccess,
    ScopeGrantSource, ALL_PROJECTS_WRITE_CONFIRMATION,
};
use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

type Reply<T> = (StatusCode, Json<ApiResponse<T>>);

fn failure<T: serde::Serialize>(error: anyhow::Error) -> Reply<T> {
    let (status, code) = match error.downcast_ref::<ScopeError>() {
        Some(ScopeError::Invalid(_)) => (StatusCode::BAD_REQUEST, ApiErrorCode::Validation),
        Some(ScopeError::NotFound(_)) => (StatusCode::NOT_FOUND, ApiErrorCode::NotFound),
        Some(ScopeError::Conflict(_)) => (StatusCode::CONFLICT, ApiErrorCode::Conflict),
        None => (StatusCode::INTERNAL_SERVER_ERROR, ApiErrorCode::Internal),
    };
    (
        status,
        Json(ApiResponse::err_coded(code, error.to_string())),
    )
}

fn refused<T: serde::Serialize>(message: &str) -> Reply<T> {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            message.to_owned(),
        )),
    )
}

fn human(name: Option<String>) -> String {
    name.map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "human".into())
}

/// GET /api/discussions/{id}/scope
pub async fn get_scope(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
) -> Reply<DiscussionScope> {
    match state
        .db
        .with_read_conn(move |conn| discussion_scope::scope_of(conn, &discussion_id))
        .await
    {
        Ok(Some(scope)) => (StatusCode::OK, Json(ApiResponse::ok(scope))),
        Ok(None) => failure(ScopeError::NotFound("discussion not found".into()).into()),
        Err(error) => failure(error),
    }
}

/// POST /api/discussions/{id}/scope/grants — a project, or every project.
/// Every project in write needs the typed confirmation and the fingerprint of
/// the scope the human saw: a stale screen cannot open everything.
pub async fn grant(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
    Json(request): Json<GrantDiscussionProjectRequest>,
) -> Reply<DiscussionProjectGrant> {
    let (project, source) = match (request.all, request.project_id) {
        (true, None) => (None, ScopeGrantSource::All),
        (false, Some(project)) if !project.trim().is_empty() => {
            (Some(project), ScopeGrantSource::Link)
        }
        _ => return refused("name one project_id, or set all: true"),
    };
    if source == ScopeGrantSource::All && request.access == ScopeAccess::Write {
        if request.confirm.as_deref() != Some(ALL_PROJECTS_WRITE_CONFIRMATION) {
            return refused(&format!(
                "write access to every project needs confirm: \"{ALL_PROJECTS_WRITE_CONFIRMATION}\""
            ));
        }
        if request.expected_fingerprint.is_none() {
            return refused("write access to every project needs the scope fingerprint you saw");
        }
    }
    let granted_by = human(request.granted_by);
    let expected = request.expected_fingerprint;
    match state
        .db
        .with_conn(move |conn| {
            if let Some(expected) = expected {
                let current = discussion_scope::scope_of(conn, &discussion_id)?
                    .ok_or_else(|| ScopeError::NotFound("discussion not found".into()))?;
                if current.fingerprint != expected {
                    return Err(ScopeError::Conflict(
                        "the room's access changed since you looked: reload and confirm again"
                            .into(),
                    )
                    .into());
                }
            }
            discussion_scope::grant(
                conn,
                &discussion_id,
                project.as_deref(),
                request.access,
                source,
                &granted_by,
            )
        })
        .await
    {
        Ok(grant) => (StatusCode::OK, Json(ApiResponse::ok(grant))),
        Err(error) => failure(error),
    }
}

/// POST /api/discussions/{id}/scope/grants/{grant_id}/revoke
pub async fn revoke(
    State(state): State<AppState>,
    Path((discussion_id, grant_id)): Path<(String, String)>,
    Json(request): Json<RevokeDiscussionProjectRequest>,
) -> Reply<DiscussionScope> {
    let revoked_by = human(request.revoked_by);
    match state
        .db
        .with_conn(move |conn| {
            discussion_scope::revoke(
                conn,
                &discussion_id,
                &grant_id,
                &revoked_by,
                request.reason.as_deref(),
            )?;
            discussion_scope::scope_of(conn, &discussion_id)?
                .ok_or_else(|| ScopeError::NotFound("discussion not found".into()).into())
        })
        .await
    {
        Ok(scope) => (StatusCode::OK, Json(ApiResponse::ok(scope))),
        Err(error) => failure(error),
    }
}

async fn decide(
    state: AppState,
    discussion_id: String,
    request_id: String,
    approve: Option<ScopeAccess>,
    decided_by: Option<String>,
) -> Reply<DiscussionScope> {
    let decided_by = human(decided_by);
    match state
        .db
        .with_conn(move |conn| {
            discussion_scope::decide(conn, &discussion_id, &request_id, approve, &decided_by)?;
            discussion_scope::scope_of(conn, &discussion_id)?
                .ok_or_else(|| ScopeError::NotFound("discussion not found".into()).into())
        })
        .await
    {
        Ok(scope) => (StatusCode::OK, Json(ApiResponse::ok(scope))),
        Err(error) => failure(error),
    }
}

/// POST /api/discussions/{id}/scope/requests/{request_id}/approve — at most
/// the access asked for; `access` narrows it.
pub async fn approve(
    State(state): State<AppState>,
    Path((discussion_id, request_id)): Path<(String, String)>,
    Json(request): Json<DecideScopeRequest>,
) -> Reply<DiscussionScope> {
    let asked = {
        let (discussion, id) = (discussion_id.clone(), request_id.clone());
        state
            .db
            .with_read_conn(move |conn| {
                Ok(
                    discussion_scope::scope_of(conn, &discussion)?.and_then(|scope| {
                        scope
                            .requests
                            .into_iter()
                            .find(|candidate| candidate.id == id)
                            .map(|candidate| candidate.access)
                    }),
                )
            })
            .await
    };
    let asked = match asked {
        Ok(Some(asked)) => asked,
        Ok(None) => return failure(ScopeError::NotFound("scope request not found".into()).into()),
        Err(error) => return failure(error),
    };
    decide(
        state,
        discussion_id,
        request_id,
        Some(request.access.unwrap_or(asked)),
        request.decided_by,
    )
    .await
}

/// POST /api/discussions/{id}/scope/requests/{request_id}/decline
pub async fn decline(
    State(state): State<AppState>,
    Path((discussion_id, request_id)): Path<(String, String)>,
    Json(request): Json<DecideScopeRequest>,
) -> Reply<DiscussionScope> {
    decide(state, discussion_id, request_id, None, request.decided_by).await
}

/// What an agent gets back from a request: never a grant, only where it
/// stands. The project is named, not identified: the token cannot read it yet.
#[derive(Debug, serde::Serialize)]
pub struct FiledScopeRequest {
    /// `pending`, `already_pending` or `already_granted`.
    pub status: &'static str,
    pub request_ref: Option<String>,
    pub project_name: Option<String>,
    pub access: Option<ScopeAccess>,
    pub next: &'static str,
}

fn filed(
    status: &'static str,
    request: Option<DiscussionScopeRequest>,
    next: &'static str,
) -> FiledScopeRequest {
    FiledScopeRequest {
        status,
        request_ref: request.as_ref().map(|request| request.id.clone()),
        project_name: request
            .as_ref()
            .and_then(|request| request.project_name.clone()),
        access: request.map(|request| request.access),
        next,
    }
}

/// POST /api/discussions/{id}/scope/requests — the one bridge route: an agent
/// of the room files a request; nothing is granted until a human decides.
pub async fn file_request(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Path(discussion_id): Path<String>,
    Json(request): Json<FileScopeRequest>,
) -> Reply<FiledScopeRequest> {
    let requested_by = bridge
        .as_ref()
        .and_then(|axum::Extension(caller)| caller.agent.as_ref())
        .map(|agent| format!("@{:?}", agent.agent_type))
        .or(request.requested_by.clone())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "@agent".into());
    match state
        .db
        .with_conn(move |conn| {
            let Some(project_id) = discussion_scope::resolve_project(conn, &request.project)?
            else {
                return Err(ScopeError::NotFound(format!(
                    "no single project is named or identified by `{}`",
                    request.project
                ))
                .into());
            };
            discussion_scope::file_request(
                conn,
                &discussion_id,
                &project_id,
                request.access,
                &request.reason,
                &requested_by,
            )
        })
        .await
    {
        Ok(outcome) => {
            let reply = match outcome {
                Filed::New(request) => filed(
                    "pending",
                    Some(request),
                    "A human decides in the room. Do not wait or retry: continue with what you can do; the decision shows up in the room.",
                ),
                Filed::AlreadyPending(request) => filed(
                    "already_pending",
                    Some(request),
                    "This request already waits for a human. Do not file it again.",
                ),
                Filed::AlreadyGranted => filed(
                    "already_granted",
                    None,
                    "The room already reaches this project at this level.",
                ),
            };
            (StatusCode::OK, Json(ApiResponse::ok(reply)))
        }
        Err(error) => failure(error),
    }
}
