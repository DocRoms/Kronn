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
    FileScopeRequest, GrantDiscussionProjectRequest, RevokeDiscussionProjectRequest,
    RevokeWorkflowScopeRequest, ScopeAccess, ScopeGrantSource, SetWorkflowScopeRequest,
    WorkflowScope, ALL_PROJECTS_WRITE_CONFIRMATION, GENERAL_WRITE_CONFIRMATION,
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

/// Who decides, as this instance knows its human: never a name the request
/// body supplies, which anything on loopback could forge.
async fn human(state: &AppState) -> String {
    state
        .config
        .read()
        .await
        .server
        .pseudo
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
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
    let (project, source) = match (request.all, request.general, request.project_id) {
        (true, false, None) => (None, ScopeGrantSource::All),
        (false, true, None) => (None, ScopeGrantSource::General),
        (false, false, Some(project)) if !project.trim().is_empty() => {
            (Some(project), ScopeGrantSource::Link)
        }
        _ => return refused("name one project_id, or set all: true, or general: true"),
    };
    // The general resources are seen by every project: writing them is the
    // same kind of decision as every project in write.
    if source == ScopeGrantSource::General {
        if request.access != ScopeAccess::Write {
            return refused("a room without a project already reads the general resources");
        }
        if request.confirm.as_deref() != Some(GENERAL_WRITE_CONFIRMATION) {
            return refused(&format!(
                "writing the general resources needs confirm: \"{GENERAL_WRITE_CONFIRMATION}\""
            ));
        }
        if request.expected_fingerprint.is_none() {
            return refused("writing the general resources needs the scope fingerprint you saw");
        }
    }
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
    let granted_by = human(&state).await;
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
    let revoked_by = human(&state).await;
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
) -> Reply<DiscussionScope> {
    let decided_by = human(&state).await;
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
    )
    .await
}

/// POST /api/discussions/{id}/scope/requests/{request_id}/decline
pub async fn decline(
    State(state): State<AppState>,
    Path((discussion_id, request_id)): Path<(String, String)>,
    Json(request): Json<DecideScopeRequest>,
) -> Reply<DiscussionScope> {
    let _ = request;
    decide(state, discussion_id, request_id, None).await
}

/// What an agent gets back from a request: never a grant, only where it
/// stands, and nothing about whether the named project exists.
#[derive(Debug, serde::Serialize)]
pub struct FiledScopeRequest {
    /// `pending`, `already_pending` or `already_granted`.
    pub status: &'static str,
    pub request_ref: Option<String>,
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
        .unwrap_or_else(|| "@agent".into());
    match state
        .db
        .with_conn(move |conn| {
            // An unknown name is filed like a known one: the reply never says
            // whether a project the token cannot read exists.
            let project_id = discussion_scope::resolve_project(conn, &request.project)?;
            discussion_scope::file_request_with_trust(
                conn,
                &discussion_id,
                project_id.as_deref(),
                &request.project,
                request.access,
                &request.reason,
                &requested_by,
            )
        })
        .await
    {
        Ok((Filed::New(request), Some(_))) => {
            // KT-1492: the room's trust level granted it; the journal says so.
            let reply = filed(
                "granted_by_trust",
                Some(request),
                "Granted by this room's trust level and journalled. The grant shows in the room; continue.",
            );
            (StatusCode::OK, Json(ApiResponse::ok(reply)))
        }
        Ok((outcome, _)) => {
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

/// GET /api/workflows/{id}/scope — the projects this workflow's runs reach,
/// and whether a human's validation still holds.
pub async fn get_workflow_scope(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
) -> Reply<WorkflowScope> {
    match state
        .db
        .with_read_conn(move |conn| discussion_scope::workflow_scope(conn, &workflow_id))
        .await
    {
        Ok(Some(scope)) => (StatusCode::OK, Json(ApiResponse::ok(scope))),
        Ok(None) => failure(ScopeError::NotFound("workflow not found".into()).into()),
        Err(error) => failure(error),
    }
}

/// PUT /api/workflows/{id}/scope — a human validates the scope against the
/// workflow as it is now; any later change to the workflow suspends it.
pub async fn set_workflow_scope(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
    Json(request): Json<SetWorkflowScopeRequest>,
) -> Reply<WorkflowScope> {
    let set_by = human(&state).await;
    // Every project in write, as for a room: the typed confirmation and the
    // scope the human saw, so a stale screen cannot open everything.
    if request.all == Some(ScopeAccess::Write) {
        if request.confirm.as_deref() != Some(ALL_PROJECTS_WRITE_CONFIRMATION) {
            return refused(&format!(
                "write access to every project needs confirm: \"{ALL_PROJECTS_WRITE_CONFIRMATION}\""
            ));
        }
        if request.expected_fingerprint.is_none() {
            return refused("write access to every project needs the scope fingerprint you saw");
        }
    }
    let (all, expected) = (request.all, request.expected_fingerprint.clone());
    let entries: Vec<(String, ScopeAccess)> = request
        .entries
        .into_iter()
        .map(|entry| (entry.project_id, entry.access))
        .collect();
    match state
        .db
        .with_conn(move |conn| {
            if let Some(expected) = expected {
                let current = discussion_scope::workflow_scope(conn, &workflow_id)?
                    .ok_or_else(|| ScopeError::NotFound("workflow not found".into()))?;
                if current.fingerprint != expected {
                    return Err(ScopeError::Conflict(
                        "the workflow's scope changed since you looked: reload and confirm again"
                            .into(),
                    )
                    .into());
                }
            }
            discussion_scope::set_workflow_scope(conn, &workflow_id, &entries, all, &set_by)
        })
        .await
    {
        Ok(scope) => (StatusCode::OK, Json(ApiResponse::ok(scope))),
        Err(error) => failure(error),
    }
}

/// POST /api/workflows/{id}/scope/revoke — withdraw one project or all of
/// them. Always available, even once the workflow can no longer carry a scope:
/// a human never loses the way to close what they opened.
pub async fn revoke_workflow_scope(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
    Json(request): Json<RevokeWorkflowScopeRequest>,
) -> Reply<WorkflowScope> {
    let id = workflow_id.clone();
    match state
        .db
        .with_conn(move |conn| {
            discussion_scope::revoke_workflow_scope(
                conn,
                &id,
                request.project_id.as_deref(),
                request.all,
            )
        })
        .await
    {
        Ok(Some(scope)) => (StatusCode::OK, Json(ApiResponse::ok(scope))),
        // The workflow is gone: its scope rows are gone too.
        Ok(None) => (
            StatusCode::OK,
            Json(ApiResponse::ok(WorkflowScope {
                workflow_id,
                supported: false,
                entries: Vec::new(),
                all: None,
                fingerprint: String::new(),
                valid: false,
                validated_by: None,
                validated_at: None,
            })),
        ),
        Err(error) => failure(error),
    }
}
