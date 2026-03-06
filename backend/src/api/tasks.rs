use axum::{extract::{Path, State}, Json};
use chrono::Utc;
use std::str::FromStr;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

/// GET /api/projects/:project_id/tasks
pub async fn list(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Json<ApiResponse<Vec<ScheduledTask>>> {
    match state.db.with_conn(move |conn| crate::db::projects::list_tasks(conn, &project_id)).await {
        Ok(tasks) => Json(ApiResponse::ok(tasks)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/projects/:project_id/tasks
pub async fn create(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(req): Json<CreateTaskRequest>,
) -> Json<ApiResponse<ScheduledTask>> {
    let cron_str = format!("0 {}", &req.cron_expr);
    if cron::Schedule::from_str(&cron_str).is_err() {
        return Json(ApiResponse::err("Invalid cron expression"));
    }

    let task = ScheduledTask {
        id: Uuid::new_v4().to_string(),
        name: req.name,
        cron_expr: req.cron_expr,
        human_interval: req.human_interval,
        agent: req.agent,
        prompt: req.prompt,
        active: false,
        last_run: None,
        last_status: None,
        tokens_used: 0,
        created_at: Utc::now(),
    };

    state.scheduler.register(task.clone()).await;

    let t = task.clone();
    let pid = project_id.clone();
    match state.db.with_conn(move |conn| crate::db::projects::insert_task(conn, &pid, &t)).await {
        Ok(()) => Json(ApiResponse::ok(task)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/projects/:project_id/tasks/:task_id
pub async fn delete(
    State(state): State<AppState>,
    Path((project_id, task_id)): Path<(String, String)>,
) -> Json<ApiResponse<()>> {
    state.scheduler.unregister(&task_id).await;

    match state.db.with_conn(move |conn| crate::db::projects::delete_task(conn, &project_id, &task_id)).await {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err("Not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PATCH /api/projects/:project_id/tasks/:task_id/toggle
pub async fn toggle(
    State(state): State<AppState>,
    Path((project_id, task_id)): Path<(String, String)>,
) -> Json<ApiResponse<bool>> {
    let tid = task_id.clone();
    match state.db.with_conn(move |conn| crate::db::projects::toggle_task(conn, &project_id, &tid)).await {
        Ok(Some(active)) => {
            state.scheduler.set_active(&task_id, active).await;
            Json(ApiResponse::ok(active))
        }
        Ok(None) => Json(ApiResponse::err("Not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}
