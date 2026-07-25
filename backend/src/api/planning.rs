use axum::{
    extract::{Path, Query, State},
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
