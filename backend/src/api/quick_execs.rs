//! HTTP API for saved shell-free CLI data collectors.

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::models::*;
use crate::workflows::template::{extract_step_envelope, TemplateContext};
use crate::AppState;

const EXPORT_KIND: &str = "kronn.quick_exec";
const EXPORT_VERSION: u32 = 1;
const DEFAULT_TIMEOUT_SECS: u32 = 60;

pub async fn list(State(state): State<AppState>) -> Json<ApiResponse<Vec<QuickExec>>> {
    match state
        .db
        .with_conn(crate::db::quick_execs::list_quick_execs)
        .await
    {
        Ok(items) => Json(ApiResponse::ok(items)),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateQuickExecRequest>,
) -> Json<ApiResponse<QuickExec>> {
    if let Err(error) = validate_request(&request) {
        return Json(ApiResponse::err(error));
    }
    let now = Utc::now();
    let item = QuickExec {
        id: Uuid::new_v4().to_string(),
        name: request.name.trim().to_string(),
        icon: request.icon.unwrap_or_else(|| "⌘".to_string()),
        description: request.description,
        project_id: request.project_id,
        command: request.command.trim().to_string(),
        args: request.args,
        timeout_secs: request.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
        output_format: request.output_format,
        variables: request.variables,
        pinned: false,
        created_at: now,
        updated_at: now,
    };
    let saved = item.clone();
    match state
        .db
        .with_conn(move |conn| crate::db::quick_execs::insert_quick_exec(conn, &saved))
        .await
    {
        Ok(()) => Json(ApiResponse::ok(item)),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CreateQuickExecRequest>,
) -> Json<ApiResponse<QuickExec>> {
    if let Err(error) = validate_request(&request) {
        return Json(ApiResponse::err(error));
    }
    let lookup_id = id.clone();
    let existing = match state
        .db
        .with_conn(move |conn| crate::db::quick_execs::get_quick_exec(conn, &lookup_id))
        .await
    {
        Ok(Some(item)) => item,
        Ok(None) => return Json(ApiResponse::err("Quick Exec not found")),
        Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
    };
    let updated = QuickExec {
        id: existing.id,
        name: request.name.trim().to_string(),
        icon: request.icon.unwrap_or(existing.icon),
        description: request.description,
        project_id: request.project_id,
        command: request.command.trim().to_string(),
        args: request.args,
        timeout_secs: request.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
        output_format: request.output_format,
        variables: request.variables,
        pinned: existing.pinned,
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };
    let saved = updated.clone();
    match state
        .db
        .with_conn(move |conn| crate::db::quick_execs::update_quick_exec(conn, &saved))
        .await
    {
        Ok(()) => Json(ApiResponse::ok(updated)),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

/// PATCH /api/quick-execs/:id — favorite-only partial update.
pub async fn update_pinned(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateQuickFavoriteRequest>,
) -> Json<ApiResponse<QuickExec>> {
    let update_id = id.clone();
    match state
        .db
        .with_conn(move |conn| {
            if !crate::db::quick_execs::update_quick_exec_pinned(conn, &update_id, request.pinned)?
            {
                return Ok(None);
            }
            crate::db::quick_execs::get_quick_exec(conn, &update_id)
        })
        .await
    {
        Ok(Some(item)) => Json(ApiResponse::ok(item)),
        Ok(None) => Json(ApiResponse::err("Quick Exec not found")),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state
        .db
        .with_conn(move |conn| crate::db::quick_execs::delete_quick_exec(conn, &id))
        .await
    {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

pub async fn run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<RunQuickExecRequest>,
) -> Json<ApiResponse<RunQuickExecResponse>> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let lookup_id = id.clone();
    let item = match state
        .db
        .with_conn(move |conn| crate::db::quick_execs::get_quick_exec(conn, &lookup_id))
        .await
    {
        Ok(Some(item)) => item,
        Ok(None) => return Json(ApiResponse::err("Quick Exec not found")),
        Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
    };
    if let Err(error) = validate_variables(&item.variables, &request.variables) {
        return Json(ApiResponse::err(error));
    }
    let project_id = item.project_id.clone();
    let work_dir = if let Some(project_id) = project_id {
        match state
            .db
            .with_conn(move |conn| crate::db::projects::get_project(conn, &project_id))
            .await
        {
            Ok(Some(project)) => project.path,
            Ok(None) => return Json(ApiResponse::err("Quick Exec project not found")),
            Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
        }
    } else {
        std::env::temp_dir().to_string_lossy().into_owned()
    };

    let mut context = TemplateContext::new();
    for variable in &item.variables {
        if let Some(value) = request.variables.get(&variable.name) {
            context.set(variable.name.clone(), value.clone());
        }
    }
    let step = WorkflowStep {
        name: item.name.clone(),
        step_type: StepType::Exec,
        exec_command: Some(item.command.clone()),
        exec_args: item.args.clone(),
        exec_timeout_secs: Some(item.timeout_secs),
        ..WorkflowStep::default()
    };
    let outcome = crate::workflows::exec_step::execute_exec_step_with_output_limit(
        &step,
        std::slice::from_ref(&item.command),
        &work_dir,
        &context,
        crate::workflows::exec_step::MAX_COLLECT_OUTPUT_BYTES,
    )
    .await;
    if outcome.result.status != RunStatus::Success {
        let response = RunQuickExecResponse {
            run_id: run_id.clone(),
            success: false,
            duration_ms: outcome.result.duration_ms,
            data: None,
            stdout: None,
            stderr: None,
            error: Some(outcome.result.output),
        };
        persist_quick_exec_run(&state, &id, &response).await;
        return Json(ApiResponse::ok(response));
    }
    let Some(envelope) = extract_step_envelope(&outcome.result.output) else {
        return Json(ApiResponse::err("Quick Exec returned no structured result"));
    };
    let raw: serde_json::Value = match serde_json::from_str(&envelope.data_json) {
        Ok(value) => value,
        Err(error) => {
            return Json(ApiResponse::err(format!(
                "Invalid Quick Exec result: {error}"
            )))
        }
    };
    let stdout = raw
        .get("stdout")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let stderr = raw
        .get("stderr")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let response =
        match crate::workflows::collect_api_data_step::quick_exec_value(&raw, item.output_format) {
            Ok(data) => RunQuickExecResponse {
                run_id,
                success: true,
                duration_ms: outcome.result.duration_ms,
                data: Some(data),
                stdout,
                stderr,
                error: None,
            },
            Err(error) => RunQuickExecResponse {
                run_id,
                success: false,
                duration_ms: outcome.result.duration_ms,
                data: None,
                stdout,
                stderr,
                error: Some(error),
            },
        };
    persist_quick_exec_run(&state, &id, &response).await;
    Json(ApiResponse::ok(response))
}

async fn persist_quick_exec_run(
    state: &AppState,
    source_id: &str,
    response: &RunQuickExecResponse,
) {
    let now = chrono::Utc::now();
    let run = crate::models::SharedRun {
        id: response.run_id.clone(),
        kind: crate::models::SharedRunKind::QuickExec,
        source_id: source_id.to_owned(),
        discussion_id: None,
        status: if response.success {
            crate::models::SharedRunStatus::Success
        } else {
            crate::models::SharedRunStatus::Failed
        },
        started_at: Some(now - chrono::Duration::milliseconds(response.duration_ms as i64)),
        finished_at: Some(now),
        duration_ms: Some(response.duration_ms),
        result: response.data.clone(),
        diagnostic: response.error.clone(),
        created_at: now,
        updated_at: now,
    };
    let saved = run.clone();
    if state
        .db
        .with_conn(move |conn| crate::db::shared_runs::upsert(conn, &saved))
        .await
        .is_ok()
    {
        let _ = state
            .ws_broadcast
            .send(crate::models::WsMessage::SharedRunUpdated { run_id: run.id });
    }
}

pub async fn export(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let lookup_id = id.clone();
    let item = match state
        .db
        .with_conn(move |conn| crate::db::quick_execs::get_quick_exec(conn, &lookup_id))
        .await
    {
        Ok(Some(item)) => item,
        Ok(None) => return (StatusCode::NOT_FOUND, "Quick Exec not found").into_response(),
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
        }
    };
    let safe_name: String = item
        .name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    let body = serde_json::to_string_pretty(&QuickExecExportEnvelope {
        kind: EXPORT_KIND.to_string(),
        version: EXPORT_VERSION,
        exported_at: Utc::now(),
        quick_exec: item,
    })
    .unwrap_or_default();
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{safe_name}.kronn-quick-exec.json\""),
            ),
        ],
        body,
    )
        .into_response()
}

pub async fn import(
    State(state): State<AppState>,
    Json(request): Json<ImportQuickExecRequest>,
) -> Json<ApiResponse<QuickExec>> {
    let envelope: QuickExecExportEnvelope = match serde_json::from_str(&request.content) {
        Ok(value) => value,
        Err(error) => return Json(ApiResponse::err(format!("Invalid JSON: {error}"))),
    };
    if envelope.kind != EXPORT_KIND || envelope.version > EXPORT_VERSION {
        return Json(ApiResponse::err("Unsupported Quick Exec export"));
    }
    let mut item = envelope.quick_exec;
    let validation = CreateQuickExecRequest {
        name: item.name.clone(),
        icon: Some(item.icon.clone()),
        description: item.description.clone(),
        project_id: request.project_id.clone(),
        command: item.command.clone(),
        args: item.args.clone(),
        timeout_secs: Some(item.timeout_secs),
        output_format: item.output_format,
        variables: item.variables.clone(),
    };
    if let Err(error) = validate_request(&validation) {
        return Json(ApiResponse::err(error));
    }
    let now = Utc::now();
    item.id = Uuid::new_v4().to_string();
    item.project_id = request.project_id;
    item.created_at = now;
    item.updated_at = now;
    let saved = item.clone();
    match state
        .db
        .with_conn(move |conn| crate::db::quick_execs::insert_quick_exec(conn, &saved))
        .await
    {
        Ok(()) => Json(ApiResponse::ok(item)),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

fn validate_request(request: &CreateQuickExecRequest) -> Result<(), String> {
    if request.name.trim().is_empty() || request.name.chars().count() > 200 {
        return Err("Name must be 1-200 characters".to_string());
    }
    let command = request.command.trim();
    if command.is_empty()
        || command.contains('/')
        || command.contains('\\')
        || command.contains(char::is_whitespace)
        || command.contains("..")
    {
        return Err("Quick Exec command must be one bare binary name".to_string());
    }
    let normalized = command.to_ascii_lowercase();
    if crate::core::quick_exec::DENIED_BINARIES.contains(&normalized.as_str()) {
        return Err("Quick Exec cannot invoke a shell or command wrapper".to_string());
    }
    if request.args.len() > 64 || request.args.iter().any(|argument| argument.contains('\0')) {
        return Err("Quick Exec accepts at most 64 NUL-free arguments".to_string());
    }
    if matches!(request.timeout_secs, Some(0 | 1801..)) {
        return Err("Quick Exec timeout must be between 1 and 1800 seconds".to_string());
    }
    let mut names = std::collections::HashSet::new();
    for variable in &request.variables {
        if variable.name.trim().is_empty() || !names.insert(variable.name.trim()) {
            return Err("Quick Exec variable names must be non-empty and unique".to_string());
        }
    }
    Ok(())
}

pub(crate) fn validate_variables(
    declarations: &[PromptVariable],
    values: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    for variable in declarations {
        let value = values.get(&variable.name).map(String::as_str).unwrap_or("");
        if variable.required && value.trim().is_empty() {
            return Err(format!("Missing required variable `{}`", variable.name));
        }
        if let Some(pattern) = &variable.pattern {
            if !value.is_empty() {
                let regex = regex_lite::Regex::new(&format!("^(?:{pattern})$"))
                    .map_err(|_| format!("Invalid pattern for variable `{}`", variable.name))?;
                if !regex.is_match(value) {
                    return Err(format!(
                        "Variable `{}` does not match its pattern",
                        variable.name
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command: &str) -> CreateQuickExecRequest {
        CreateQuickExecRequest {
            name: "Collect AWS".into(),
            icon: None,
            description: String::new(),
            project_id: None,
            command: command.into(),
            args: vec![],
            timeout_secs: Some(60),
            output_format: CollectQuickExecOutputFormat::Json,
            variables: vec![],
        }
    }

    #[test]
    fn command_validation_rejects_shells_paths_and_wrappers() {
        assert!(validate_request(&request("aws")).is_ok());
        for command in ["bash", "sh", "/usr/bin/aws", "aws cli", "../aws"] {
            assert!(
                validate_request(&request(command)).is_err(),
                "accepted {command}"
            );
        }
    }
}
