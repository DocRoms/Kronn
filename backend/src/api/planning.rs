use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use crate::models::{
    AddPlanningBlockerRequest, ApiErrorCode, ApiResponse, CreatePlanningTaskRequest,
    DiscussionPlan, LinkPlanningDiscussionRequest, PlanningTaskChange, PlanningTaskDetail,
    PlanningTaskListQuery, PlanningTaskListResponse, UpdatePlanningDodItemRequest,
    UpdatePlanningTaskRequest,
};
use crate::AppState;

fn planning_error<T: serde::Serialize>(error: anyhow::Error) -> Json<ApiResponse<T>> {
    let message = error.to_string();
    let code = if message.contains("not found") || message.contains("Unknown project") {
        ApiErrorCode::NotFound
    } else if message.contains("idempotency key was reused") {
        ApiErrorCode::Conflict
    } else if message.contains("cycle")
        || message.contains("must")
        || message.contains("cannot")
        || message.contains("more than")
    {
        ApiErrorCode::Validation
    } else {
        ApiErrorCode::Internal
    };
    Json(ApiResponse::err_coded(code, message))
}

pub async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<PlanningTaskListQuery>,
) -> Json<ApiResponse<PlanningTaskListResponse>> {
    match state
        .db
        .with_read_conn(move |connection| crate::db::planning::list_tasks(connection, &query))
        .await
    {
        Ok(tasks) => Json(ApiResponse::ok(tasks)),
        Err(error) => planning_error(error),
    }
}

pub async fn create_task(
    State(state): State<AppState>,
    Json(request): Json<CreatePlanningTaskRequest>,
) -> Json<ApiResponse<PlanningTaskDetail>> {
    match state
        .db
        .with_conn(move |connection| crate::db::planning::create_task(connection, &request))
        .await
    {
        Ok(task) => Json(ApiResponse::ok(task)),
        Err(error) => planning_error(error),
    }
}

pub async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<PlanningTaskDetail>> {
    match state
        .db
        .with_read_conn(move |connection| crate::db::planning::get_task(connection, &id))
        .await
    {
        Ok(Some(task)) => Json(ApiResponse::ok(task)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Planning task not found",
        )),
        Err(error) => planning_error(error),
    }
}

pub async fn update_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdatePlanningTaskRequest>,
) -> Json<ApiResponse<PlanningTaskDetail>> {
    match state
        .db
        .with_conn(move |connection| crate::db::planning::update_task(connection, &id, &request))
        .await
    {
        Ok(task) => Json(ApiResponse::ok(task)),
        Err(error) => planning_error(error),
    }
}

pub async fn update_dod_item(
    State(state): State<AppState>,
    Path((task_id, dod_id)): Path<(String, String)>,
    Json(request): Json<UpdatePlanningDodItemRequest>,
) -> Json<ApiResponse<PlanningTaskDetail>> {
    match state
        .db
        .with_conn(move |connection| {
            crate::db::planning::update_dod_item(connection, &task_id, &dod_id, &request)
        })
        .await
    {
        Ok(task) => Json(ApiResponse::ok(task)),
        Err(error) => planning_error(error),
    }
}

pub async fn link_discussion(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<LinkPlanningDiscussionRequest>,
) -> Json<ApiResponse<DiscussionPlan>> {
    match state
        .db
        .with_conn(move |connection| {
            crate::db::planning::link_discussion(connection, &id, &request)
        })
        .await
    {
        Ok(plan) => Json(ApiResponse::ok(plan)),
        Err(error) => planning_error(error),
    }
}

pub async fn add_blocker(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<AddPlanningBlockerRequest>,
) -> Json<ApiResponse<PlanningTaskDetail>> {
    match state
        .db
        .with_conn(move |connection| crate::db::planning::add_blocker(connection, &id, &request))
        .await
    {
        Ok(task) => Json(ApiResponse::ok(task)),
        Err(error) => planning_error(error),
    }
}

pub async fn get_discussion_plan(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<DiscussionPlan>> {
    match state
        .db
        .with_read_conn(move |connection| crate::db::planning::get_discussion_plan(connection, &id))
        .await
    {
        Ok(plan) => Json(ApiResponse::ok(plan)),
        Err(error) => planning_error(error),
    }
}

#[derive(serde::Deserialize, Default)]
pub struct PlanningChangesQuery {
    pub since: Option<String>,
}

pub async fn task_changes(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<PlanningChangesQuery>,
) -> Json<ApiResponse<Vec<PlanningTaskChange>>> {
    match state
        .db
        .with_read_conn(move |connection| {
            crate::db::planning::task_changes(connection, &id, query.since.as_deref())
        })
        .await
    {
        Ok(changes) => Json(ApiResponse::ok(changes)),
        Err(error) => planning_error(error),
    }
}

// ─── 0.9.2-H — Planning proposals (durable inbox, human-gated) ───────────────

use crate::db::planning_proposals::{
    self, DecisionError, PlanningProposal, ProposalDecisionRequest, ProposalDecisionResponse,
    ProposalListResponse,
};

fn default_pending_only() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
pub struct ProposalListQuery {
    pub discussion_id: String,
    #[serde(default = "default_pending_only")]
    pub pending_only: bool,
}

pub async fn list_proposals(
    State(state): State<AppState>,
    Query(query): Query<ProposalListQuery>,
) -> Json<ApiResponse<ProposalListResponse>> {
    match state
        .db
        .with_read_conn(move |connection| {
            planning_proposals::list_proposals(connection, &query.discussion_id, query.pending_only)
        })
        .await
    {
        Ok(list) => Json(ApiResponse::ok(list)),
        Err(error) => planning_error(error),
    }
}

pub async fn get_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<PlanningProposal>> {
    match state
        .db
        .with_read_conn(move |connection| planning_proposals::get_proposal(connection, &id))
        .await
    {
        Ok(Some(proposal)) => Json(ApiResponse::ok(proposal)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Planning proposal not found",
        )),
        Err(error) => planning_error(error),
    }
}

pub async fn decide_proposal_item(
    State(state): State<AppState>,
    Path((proposal_id, item_id)): Path<(String, String)>,
    Json(request): Json<ProposalDecisionRequest>,
) -> (StatusCode, Json<ApiResponse<ProposalDecisionResponse>>) {
    let result = state
        .db
        .with_conn(move |connection| {
            Ok(planning_proposals::decide_item(
                connection,
                &proposal_id,
                &item_id,
                request.decision,
                request.reason.as_deref(),
                &request.idempotency_key,
            ))
        })
        .await;
    match result {
        Ok(Ok(response)) => (StatusCode::OK, Json(ApiResponse::ok(response))),
        Ok(Err(DecisionError::NotFound)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Planning proposal item not found",
            )),
        ),
        Ok(Err(DecisionError::Conflict { current_state })) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                format!("This item was already decided ({current_state:?}) under another request"),
            )),
        ),
        Ok(Err(DecisionError::Invalid(message))) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
        ),
        Ok(Err(DecisionError::Failed(error))) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            planning_error::<ProposalDecisionResponse>(error),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            planning_error::<ProposalDecisionResponse>(error),
        ),
    }
}
