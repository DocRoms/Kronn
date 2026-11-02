//! HTTP API for saved shell-free CLI data collectors.

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::db::trust::TrustOrigin;
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

/// One saved Quick Exec whose line waits for a human's approval (KT-1398).
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct QuickExecPendingApproval {
    pub id: String,
    /// The program that receives run values, e.g. `aws`.
    pub program: String,
    pub message: String,
    /// A human refused to activate this very line (KT-1490); it stays inactive.
    pub refused: bool,
}

/// GET /api/quick-execs/pending-approvals — the Quick Execs a run would refuse
/// until a human approves them, for the library badges and the workflow page.
pub async fn pending_approvals(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<QuickExecPendingApproval>>> {
    match state
        .db
        .with_read_conn(|conn| {
            Ok((
                crate::db::quick_execs::list_quick_execs(conn)?,
                crate::db::quick_execs::refused_activation_ids(conn)?,
            ))
        })
        .await
    {
        Ok((items, refused)) => Json(ApiResponse::ok(
            items
                .into_iter()
                .filter_map(|exec| {
                    let (program, message) = crate::core::inline_code::quick_exec_approval_needed(
                        &exec.name,
                        &exec.command,
                        &exec.args,
                        exec.unmodelled_args_approved == Some(true),
                        exec.agent_written == Some(true),
                    )?;
                    Some(QuickExecPendingApproval {
                        refused: refused.contains(&exec.id),
                        id: exec.id,
                        program,
                        message,
                    })
                })
                .collect(),
        )),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

pub async fn create(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    marker: crate::core::bridge_token::AgentMark,
    Json(request): Json<CreateQuickExecRequest>,
) -> Json<ApiResponse<QuickExec>> {
    let by_agent = crate::core::bridge_token::agent_authored(&bridge, &marker);
    let origin = origin_of(&bridge);
    let response = create_as_in(state.clone(), request, by_agent, origin).await;
    announce_from_agent(&state, bridge.as_ref(), marker.as_ref(), &response.0).await;
    response
}

/// The launch behind a bridge token, for the trust level it runs under.
fn origin_of(
    bridge: &Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
) -> TrustOrigin {
    bridge
        .as_ref()
        .map(|axum::Extension(caller)| TrustOrigin::from_caller(caller))
        .unwrap_or_default()
}

/// A level's approval of a Quick Exec, not yet journalled: it is granted in the
/// very transaction that saves the line ([`store_quick_exec`]).
pub(crate) struct PendingGrant {
    pub(crate) origin: TrustOrigin,
    pub(crate) subject: String,
    pub(crate) project_id: Option<String>,
}

/// Why a save that counted on a level was refused.
pub(crate) const TRUST_CHANGED: &str =
    "The trust level changed while the Quick Exec was being saved: it \
     was not saved. Save it again.";

/// KT-1492 — under Trusted, a line an agent wrote that passes values to a CLI is
/// active at once. Under Standard, or when the setting cannot be read, it waits
/// for a human as before. This only reads the level, so the line can be
/// validated as approved; the grant is journalled by [`store_quick_exec`] in the
/// transaction that saves the line, and the level is read again there.
async fn approve_by_trust(
    state: &AppState,
    name: &str,
    project_id: Option<&str>,
    agent_written: Option<bool>,
    approved: &mut Option<bool>,
    by_trust: &mut Option<bool>,
    origin: &TrustOrigin,
) -> Option<PendingGrant> {
    if agent_written != Some(true) || *approved == Some(true) {
        return None;
    }
    let level_origin = origin.clone();
    let granted = state
        .db
        .with_conn(move |conn| {
            crate::db::trust::level_grants(conn, &level_origin, TrustApproval::QuickExecLine)
        })
        .await;
    match granted {
        Ok(Some(_)) => {
            // The line is active at once, and runs under the level's sandbox.
            *approved = Some(true);
            *by_trust = Some(true);
            Some(PendingGrant {
                origin: origin.clone(),
                subject: format!("Quick Exec \u{ab} {} \u{bb}", name.trim()),
                project_id: project_id.map(str::to_owned),
            })
        }
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(%error, "trust level unreadable: the Quick Exec waits for a human");
            None
        }
    }
}

/// An update that keeps the approval a level gave an unchanged line copies it
/// from the row as it was read, before the save. Checked here against the row
/// as it is stored now, on the write connection: if the level took the approval
/// back meanwhile (or the line changed), the stale mark is not written back.
/// A human's own approval of that very line, given since, is kept.
fn keep_only_a_live_trust_approval(conn: &rusqlite::Connection, saved: &mut QuickExec) {
    if saved.approved_by_trust != Some(true) {
        return;
    }
    // A row that cannot be read counts as gone: the approval is not kept.
    let current = crate::db::quick_execs::get_quick_exec(conn, &saved.id)
        .ok()
        .flatten()
        .filter(|row| row.command == saved.command && row.args == saved.args);
    if current
        .as_ref()
        .is_some_and(|row| row.approved_by_trust == Some(true))
    {
        return;
    }
    saved.approved_by_trust = None;
    saved.unmodelled_args_approved = current
        .and_then(|row| row.unmodelled_args_approved)
        .filter(|approved| *approved);
}

/// Save a Quick Exec. With a `pending` grant, the level is consulted, the grant
/// journalled and the line written as one unit on the one database connection:
/// a level lowered meanwhile either comes first (the save is refused, nothing is
/// approved) or after (it withdraws the approval this save gave). An update
/// that only retains an earlier approval re-reads it on that same connection
/// first ([`keep_only_a_live_trust_approval`]).
pub(crate) async fn store_quick_exec(
    state: &AppState,
    item: &QuickExec,
    insert: bool,
    pending: Option<PendingGrant>,
) -> Result<(), String> {
    let mut saved = item.clone();
    let stored = state
        .db
        .with_conn(move |conn| {
            if !insert && pending.is_none() {
                keep_only_a_live_trust_approval(conn, &mut saved);
            }
            let write = |conn: &rusqlite::Connection| {
                if insert {
                    crate::db::quick_execs::insert_quick_exec(conn, &saved)
                } else {
                    crate::db::quick_execs::update_quick_exec(conn, &saved)
                }
            };
            match pending {
                None => write(conn).map(|()| true),
                Some(grant) => crate::db::trust::grant_and_write(
                    conn,
                    &grant.origin,
                    TrustApproval::QuickExecLine,
                    &grant.subject,
                    grant.project_id.as_deref(),
                    crate::db::trust::TrustObject::quick_exec(&saved.id),
                    write,
                ),
            }
        })
        .await;
    match stored {
        Ok(true) => Ok(()),
        Ok(false) => Err(TRUST_CHANGED.to_string()),
        Err(error) => Err(format!("DB error: {error}")),
    }
}

/// Create a Quick Exec. `by_agent` (bridge token or Kronn agent tools) drops
/// any approval of unmodelled programs: only a human gives one (KT-1017).
#[cfg(test)]
pub(crate) async fn create_as(
    state: AppState,
    request: CreateQuickExecRequest,
    by_agent: bool,
) -> Json<ApiResponse<QuickExec>> {
    create_as_in(state, request, by_agent, TrustOrigin::default()).await
}

/// [`create_as`] for an agent launched from the rooms and run of `origin`.
pub(crate) async fn create_as_in(
    state: AppState,
    mut request: CreateQuickExecRequest,
    by_agent: bool,
    origin: TrustOrigin,
) -> Json<ApiResponse<QuickExec>> {
    if by_agent {
        request.unmodelled_args_approved = None;
    }
    if let Err(error) = validate_request(&request) {
        return Json(ApiResponse::err(error));
    }
    let agent_written =
        (by_agent && needs_agent_approval(&request.command, &request.args)).then_some(true);
    drop_stale_approval(&mut request, agent_written);
    let id = Uuid::new_v4().to_string();
    let mut approved_by_trust = None;
    let mut pending = None;
    if by_agent {
        pending = approve_by_trust(
            &state,
            &request.name,
            request.project_id.as_deref(),
            agent_written,
            &mut request.unmodelled_args_approved,
            &mut approved_by_trust,
            &origin,
        )
        .await;
    }
    if let Some(error) = inline_code_error(&request, agent_written) {
        return Json(ApiResponse::err(error));
    }
    let now = Utc::now();
    let item = QuickExec {
        id,
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
        unmodelled_args_approved: request
            .unmodelled_args_approved
            .filter(|approved| *approved),
        agent_written,
        approved_by_trust,
        created_at: now,
        updated_at: now,
    };
    match store_quick_exec(&state, &item, true, pending).await {
        Ok(()) => {
            let notice = awaiting_approval_notice(&item);
            Json(ApiResponse::ok(item).with_notice(notice))
        }
        Err(error) => Json(ApiResponse::err(error)),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    marker: crate::core::bridge_token::AgentMark,
    Json(request): Json<CreateQuickExecRequest>,
) -> Json<ApiResponse<QuickExec>> {
    let by_agent = crate::core::bridge_token::agent_authored(&bridge, &marker);
    let origin = origin_of(&bridge);
    let response = update_as_in(state.clone(), id, request, by_agent, origin).await;
    announce_from_agent(&state, bridge.as_ref(), marker.as_ref(), &response.0).await;
    response
}

/// Update a Quick Exec; see [`create_as`] for `by_agent`.
#[cfg(test)]
pub(crate) async fn update_as(
    state: AppState,
    id: String,
    request: CreateQuickExecRequest,
    by_agent: bool,
) -> Json<ApiResponse<QuickExec>> {
    update_as_in(state, id, request, by_agent, TrustOrigin::default()).await
}

/// [`update_as`] for an agent launched from the rooms and run of `origin`.
pub(crate) async fn update_as_in(
    state: AppState,
    id: String,
    mut request: CreateQuickExecRequest,
    by_agent: bool,
    origin: TrustOrigin,
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
    // An agent's save keeps the stored approval only for the very line a
    // human approved; any other line loses it (KT-1017).
    let same_line = existing.command == request.command.trim() && existing.args == request.args;
    if by_agent {
        request.unmodelled_args_approved = existing.unmodelled_args_approved.filter(|_| same_line);
    }
    // An unchanged line keeps its writer; a changed one is this save's.
    let agent_written = if same_line {
        existing.agent_written
    } else {
        (by_agent && needs_agent_approval(&request.command, &request.args)).then_some(true)
    };
    drop_stale_approval(&mut request, agent_written);
    // KT-1492: the "approved by a trust level" mark is the server's, never the
    // request's: it stays only on the very line a level approved, still approved.
    let mut approved_by_trust = existing
        .approved_by_trust
        .filter(|_| same_line && request.unmodelled_args_approved == Some(true));
    let mut pending = None;
    if by_agent {
        pending = approve_by_trust(
            &state,
            &request.name,
            request.project_id.as_deref(),
            agent_written,
            &mut request.unmodelled_args_approved,
            &mut approved_by_trust,
            &origin,
        )
        .await;
    }
    // An unchanged stored line stays saveable (it is still refused at run
    // time); a new or changed one must be safe.
    let unchanged = existing.name == request.name.trim()
        && existing.command == request.command.trim()
        && existing.args == request.args
        && existing.unmodelled_args_approved
            == request
                .unmodelled_args_approved
                .filter(|approved| *approved);
    if !unchanged {
        if let Some(error) = inline_code_error(&request, agent_written) {
            return Json(ApiResponse::err(error));
        }
    }
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
        unmodelled_args_approved: request
            .unmodelled_args_approved
            .filter(|approved| *approved),
        agent_written,
        approved_by_trust,
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };
    match store_quick_exec(&state, &updated, false, pending).await {
        Ok(()) => {
            // Answer with the line as stored: a retained approval the level
            // took back meanwhile is not reported as still given.
            let stored_id = updated.id.clone();
            let updated = state
                .db
                .with_conn(move |conn| crate::db::quick_execs::get_quick_exec(conn, &stored_id))
                .await
                .ok()
                .flatten()
                .unwrap_or(updated);
            let notice = awaiting_approval_notice(&updated);
            Json(ApiResponse::ok(updated).with_notice(notice))
        }
        Err(error) => Json(ApiResponse::err(error)),
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

/// The Exec step a saved Quick Exec runs as. It carries every approval of the
/// saved line, **the trust level's included**: `exec_approved_by_trust` is what
/// puts a level-approved line under the vault sandbox
/// (`workflows::exec_step::confined_for_level`); a step built without it runs
/// unconfined, and its output reaches a model.
fn run_step(item: &QuickExec) -> WorkflowStep {
    WorkflowStep {
        name: item.name.clone(),
        step_type: StepType::Exec,
        exec_command: Some(item.command.clone()),
        exec_args: item.args.clone(),
        exec_timeout_secs: Some(item.timeout_secs),
        exec_unmodelled_args_approved: item.unmodelled_args_approved,
        exec_agent_written: item.agent_written,
        exec_approved_by_trust: item.approved_by_trust,
        ..WorkflowStep::default()
    }
}

pub async fn run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(request): Json<RunQuickExecRequest>,
) -> Json<ApiResponse<RunQuickExecResponse>> {
    let mut launch = request.launch.clone().unwrap_or_default();
    // A Kronn-launched agent runs a shared Quick Exec for its own project only.
    if let Some(axum::Extension(caller)) = &bridge {
        launch.project_id = caller.project.clone();
    }
    let run_id = uuid::Uuid::new_v4().to_string();
    let created_at = Utc::now();
    let queued = crate::models::SharedRun {
        exec_details: None,
        id: run_id.clone(),
        kind: crate::models::SharedRunKind::QuickExec,
        source_id: id.clone(),
        project_id: None,
        discussion_id: launch.discussion_id.clone(),
        status: crate::models::SharedRunStatus::Queued,
        started_at: None,
        finished_at: None,
        duration_ms: None,
        result: None,
        diagnostic: None,
        created_at,
        updated_at: created_at,
    };
    if let Err(error) = crate::api::shared_runs::persist_and_broadcast(&state, queued).await {
        return Json(ApiResponse::err(format!("DB error: {error}")));
    }

    let lookup_id = id.clone();
    let item = match state
        .db
        .with_conn(move |conn| crate::db::quick_execs::get_quick_exec(conn, &lookup_id))
        .await
    {
        Ok(Some(item)) => item,
        Ok(None) => {
            let response = RunQuickExecResponse {
                exit_code: None,
                run_id: run_id.clone(),
                success: false,
                duration_ms: 0,
                data: None,
                stdout: None,
                stderr: None,
                error: Some("Quick Exec not found".into()),
            };
            if let Err(error) = persist_quick_exec_terminal(
                &state,
                &id,
                None,
                launch.discussion_id.clone(),
                created_at,
                &response,
                crate::models::SharedRunStatus::PreflightFailed,
            )
            .await
            {
                return Json(ApiResponse::err(format!("DB error: {error}")));
            }
            return Json(ApiResponse::ok(response));
        }
        Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
    };
    let approval_item = item.clone();
    let approval = state
        .db
        .with_read_conn(move |conn| {
            crate::core::repository_resources::ensure_quick_exec_execution_approved(
                conn,
                &approval_item,
            )
            .map_err(anyhow::Error::msg)
        })
        .await;
    if let Err(error) = approval {
        let response = RunQuickExecResponse {
            exit_code: None,
            run_id: run_id.clone(),
            success: false,
            duration_ms: 0,
            data: None,
            stdout: None,
            stderr: None,
            error: Some(error.to_string()),
        };
        let _ = persist_quick_exec_terminal(
            &state,
            &id,
            item.project_id.clone(),
            launch.discussion_id.clone(),
            created_at,
            &response,
            crate::models::SharedRunStatus::PreflightFailed,
        )
        .await;
        return Json(ApiResponse::ok(response));
    }
    // A GLOBAL Quick Exec launched from a project-scoped discussion resolves
    // that project's environment/worktree exactly like one declared on the
    // project directly (KT-476 LaunchContext).
    let project_id = launch
        .effective_project_id(item.project_id.as_deref())
        .map(str::to_owned);
    let (secret, retention_days) = {
        let config = state.config.read().await;
        let Some(secret) = config.encryption_secret.clone() else {
            let response = RunQuickExecResponse {
                exit_code: None,
                run_id: run_id.clone(),
                success: false,
                duration_ms: 0,
                data: None,
                stdout: None,
                stderr: None,
                error: Some("Variable preflight unavailable: encryption key missing".into()),
            };
            let _ = persist_quick_exec_terminal(
                &state,
                &id,
                project_id.clone(),
                launch.discussion_id.clone(),
                created_at,
                &response,
                crate::models::SharedRunStatus::PreflightFailed,
            )
            .await;
            return Json(ApiResponse::ok(response));
        };
        (secret, config.server.execution_variable_retention_days)
    };
    let declarations = item.variables.clone();
    let supplied = request.variables.clone();
    let selected_project = project_id.clone();
    let launch_context = launch.context.clone();
    let launch_discussion_id = launch.discussion_id.clone();
    let snapshot_run_id = run_id.clone();
    let prepared = state
        .db
        .with_conn(move |conn| {
            crate::core::execution_variables::prepare(
                conn,
                crate::core::execution_variables::PrepareRequest {
                    declarations: &declarations,
                    supplied: &supplied,
                    context: &launch_context,
                    project_id: selected_project.as_deref(),
                    discussion_id: launch_discussion_id.as_deref(),
                    environment_ref: "project_mcp_configs",
                    run_kind: "quick_exec",
                    run_id: &snapshot_run_id,
                    encryption_secret: &secret,
                    retention_days,
                },
            )
        })
        .await;
    let resolved = match prepared {
        Ok(Ok(prepared)) => prepared.resolved,
        Ok(Err(failures)) => {
            let response = RunQuickExecResponse {
                exit_code: None,
                run_id: run_id.clone(),
                success: false,
                duration_ms: 0,
                data: None,
                stdout: None,
                stderr: None,
                error: Some(format!(
                    "preflight_failed:{}",
                    serde_json::to_string(&failures).unwrap_or_default()
                )),
            };
            if let Err(error) = persist_quick_exec_terminal(
                &state,
                &id,
                project_id.clone(),
                launch.discussion_id.clone(),
                created_at,
                &response,
                crate::models::SharedRunStatus::PreflightFailed,
            )
            .await
            {
                return Json(ApiResponse::err(format!("DB error: {error}")));
            }
            return Json(ApiResponse::ok(response));
        }
        Err(error) => {
            return Json(ApiResponse::err(format!(
                "Variable preflight failed: {error}"
            )))
        }
    };
    let work_dir = if let Some(pid) = project_id.clone() {
        match state
            .db
            .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
            .await
        {
            Ok(Some(project)) => project.path,
            Ok(None) => {
                let response = RunQuickExecResponse {
                    exit_code: None,
                    run_id: run_id.clone(),
                    success: false,
                    duration_ms: 0,
                    data: None,
                    stdout: None,
                    stderr: None,
                    error: Some("Quick Exec project not found".into()),
                };
                if let Err(error) = persist_quick_exec_terminal(
                    &state,
                    &id,
                    project_id.clone(),
                    launch.discussion_id.clone(),
                    created_at,
                    &response,
                    crate::models::SharedRunStatus::PreflightFailed,
                )
                .await
                {
                    return Json(ApiResponse::err(format!("DB error: {error}")));
                }
                return Json(ApiResponse::ok(response));
            }
            Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
        }
    } else {
        std::env::temp_dir().to_string_lossy().into_owned()
    };

    let mut context = TemplateContext::new();
    for (name, value) in resolved.values {
        context.set(name, value);
    }
    // Refused before the program starts, naming where to approve it (KT-1398).
    if let Some((_, message)) = crate::core::inline_code::quick_exec_approval_needed(
        &item.name,
        &item.command,
        &item.args,
        item.unmodelled_args_approved == Some(true),
        item.agent_written == Some(true),
    ) {
        let response = RunQuickExecResponse {
            exit_code: None,
            run_id: run_id.clone(),
            success: false,
            duration_ms: 0,
            data: None,
            stdout: None,
            stderr: None,
            error: Some(message),
        };
        if let Err(error) = persist_quick_exec_terminal(
            &state,
            &id,
            project_id.clone(),
            launch.discussion_id.clone(),
            created_at,
            &response,
            crate::models::SharedRunStatus::PreflightFailed,
        )
        .await
        {
            return Json(ApiResponse::err(format!("DB error: {error}")));
        }
        return Json(ApiResponse::ok(response));
    }
    let step = run_step(&item);
    let running_at = Utc::now();
    let running = crate::models::SharedRun {
        exec_details: None,
        id: run_id.clone(),
        kind: crate::models::SharedRunKind::QuickExec,
        source_id: id.clone(),
        project_id: project_id.clone(),
        discussion_id: launch.discussion_id.clone(),
        status: crate::models::SharedRunStatus::Running,
        started_at: Some(running_at),
        finished_at: None,
        duration_ms: None,
        result: None,
        diagnostic: None,
        created_at,
        updated_at: running_at,
    };
    if let Err(error) = crate::api::shared_runs::persist_and_broadcast(&state, running).await {
        return Json(ApiResponse::err(format!("DB error: {error}")));
    }
    let outcome = crate::workflows::exec_step::execute_exec_step_with_output_limit(
        &step,
        std::slice::from_ref(&item.command),
        &work_dir,
        &context,
        crate::workflows::exec_step::MAX_COLLECT_OUTPUT_BYTES,
        project_id.as_deref(),
    )
    .await;
    let terminal_snapshot_run_id = run_id.clone();
    let _ = state
        .db
        .with_conn(move |conn| {
            crate::db::execution_variable_snapshots::purge_run_lifetime_snapshot(
                conn,
                "quick_exec",
                &terminal_snapshot_run_id,
                Utc::now(),
            )
        })
        .await;
    // Process metadata is useful on failure too, including exit 0 followed by
    // an output-format error. Read it before branching on the run's verdict.
    let envelope = extract_step_envelope(&outcome.result.output);
    let raw_result = envelope
        .as_ref()
        .ok_or_else(|| "Quick Exec returned no structured result".to_string())
        .and_then(|envelope| {
            serde_json::from_str::<serde_json::Value>(&envelope.data_json)
                .map_err(|error| format!("Invalid Quick Exec result: {error}"))
        });
    let raw = raw_result.as_ref().ok();
    let exit_code = raw
        .and_then(|value| value.get("exit_code"))
        .and_then(serde_json::Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    let stdout = raw
        .and_then(|value| value.get("stdout"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let stderr = raw
        .and_then(|value| value.get("stderr"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let (data, error, status) = if outcome.result.status != RunStatus::Success {
        let status = if outcome.result.output.contains("timed out after") {
            crate::models::SharedRunStatus::Timeout
        } else {
            crate::models::SharedRunStatus::Failed
        };
        // Keep the original diagnostic: a failed command may explain itself
        // on stdout alone, and the saved stderr cannot replace that evidence.
        (None, Some(outcome.result.output), status)
    } else {
        match raw_result.and_then(|raw| {
            crate::workflows::collect_api_data_step::quick_exec_value(&raw, item.output_format)
        }) {
            Ok(data) => (Some(data), None, crate::models::SharedRunStatus::Success),
            Err(error) => (None, Some(error), crate::models::SharedRunStatus::Failed),
        }
    };
    let response = RunQuickExecResponse {
        run_id: run_id.clone(),
        success: error.is_none(),
        duration_ms: outcome.result.duration_ms,
        exit_code,
        data,
        stdout,
        stderr,
        error,
    };
    if let Err(error) = persist_quick_exec_terminal(
        &state,
        &id,
        project_id,
        launch.discussion_id.clone(),
        created_at,
        &response,
        status,
    )
    .await
    {
        return Json(ApiResponse::err(format!("DB error: {error}")));
    }
    Json(ApiResponse::ok(response))
}

async fn persist_quick_exec_terminal(
    state: &AppState,
    source_id: &str,
    project_id: Option<String>,
    discussion_id: Option<String>,
    created_at: chrono::DateTime<Utc>,
    response: &RunQuickExecResponse,
    status: crate::models::SharedRunStatus,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    let run = crate::models::SharedRun {
        exec_details: (response.exit_code.is_some() || response.stderr.is_some()).then(|| {
            crate::models::QuickExecDiagnostics {
                exit_code: response.exit_code,
                stderr: response.stderr.clone(),
            }
        }),
        id: response.run_id.clone(),
        kind: crate::models::SharedRunKind::QuickExec,
        source_id: source_id.to_owned(),
        project_id,
        discussion_id,
        status,
        started_at: (response.duration_ms > 0)
            .then_some(now - chrono::Duration::milliseconds(response.duration_ms as i64)),
        finished_at: Some(now),
        duration_ms: Some(response.duration_ms),
        result: response.data.clone(),
        diagnostic: response.error.clone(),
        created_at,
        updated_at: now,
    };
    crate::api::shared_runs::persist_and_broadcast(state, run).await
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
    let mut item = item;
    let mut redacted_fields = Vec::new();
    crate::core::export_secrets::redact_quick_exec(&mut item, &mut redacted_fields);
    let body = serde_json::to_string_pretty(&QuickExecExportEnvelope {
        kind: EXPORT_KIND.to_string(),
        version: EXPORT_VERSION,
        exported_at: Utc::now(),
        quick_exec: item,
        redacted_fields,
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
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    marker: crate::core::bridge_token::AgentMark,
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
    // An import never carries an approval: a human approves in the editor.
    // Through an agent's bridge (token or marker), the line is the agent's (KT-1017).
    item.unmodelled_args_approved = None;
    item.agent_written = (crate::core::bridge_token::agent_authored(&bridge, &marker)
        && needs_agent_approval(&item.command, &item.args))
    .then_some(true);
    item.approved_by_trust = None;
    let new_id = Uuid::new_v4().to_string();
    let pending = approve_by_trust(
        &state,
        &item.name.clone(),
        request.project_id.as_deref(),
        item.agent_written,
        &mut item.unmodelled_args_approved,
        &mut item.approved_by_trust,
        &origin_of(&bridge),
    )
    .await;
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
        unmodelled_args_approved: item.unmodelled_args_approved,
    };
    if let Err(error) = validate_request(&validation) {
        return Json(ApiResponse::err(error));
    }
    if let Some(error) = inline_code_error(&validation, item.agent_written) {
        return Json(ApiResponse::err(error));
    }
    let now = Utc::now();
    item.id = new_id;
    item.project_id = request.project_id;
    item.created_at = now;
    item.updated_at = now;
    match store_quick_exec(&state, &item, true, pending).await {
        Ok(()) => {
            let notice = awaiting_approval_notice(&item);
            Json(ApiResponse::ok(item).with_notice(notice))
        }
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// Inline code (`python3 -c`, `node -e`…) that interpolates a value: refused
/// with the same rule as a workflow Exec step (KT-1017).
fn inline_code_error(
    request: &CreateQuickExecRequest,
    agent_written: Option<bool>,
) -> Option<String> {
    crate::core::inline_code::quick_exec_validation_error(
        request.name.trim(),
        request.command.trim(),
        &request.args,
        crate::core::argv_roles::Trust {
            approved: request.unmodelled_args_approved == Some(true),
            agent_written: agent_written == Some(true),
            declared_scripts: Vec::new(),
        },
    )
}

/// An approval the line does not need is dropped, so it never covers what
/// the line becomes later (KT-1017).
fn drop_stale_approval(request: &mut CreateQuickExecRequest, agent_written: Option<bool>) {
    // The need is read from the classifier, not the save-time refusal, which
    // lets an agent's waiting line through and would call its approval stale.
    let needed = matches!(
        crate::core::inline_code::first_unsafe_placeholder_with(
            request.command.trim(),
            &request.args,
            crate::core::argv_roles::Trust {
                agent_written: agent_written == Some(true),
                ..Default::default()
            },
        ),
        Some(crate::core::inline_code::InlineFinding::UnmodelledProgram(
            ..
        ))
    );
    if !needed {
        request.unmodelled_args_approved = None;
    }
}

/// Whether a line, written by an agent, would wait for a human's approval:
/// only such a line carries the agent's mark (KT-1017).
pub(crate) fn needs_agent_approval(command: &str, args: &[String]) -> bool {
    matches!(
        crate::core::inline_code::first_unsafe_placeholder_with(
            command.trim(),
            args,
            crate::core::argv_roles::Trust {
                agent_written: true,
                ..Default::default()
            },
        ),
        Some(crate::core::inline_code::InlineFinding::UnmodelledProgram(
            ..
        ))
    )
}

/// The notice for a saved Quick Exec an agent wrote that waits for a human:
/// it is saved and inactive, and the agent carries on (KT-1490).
fn awaiting_approval_notice(item: &QuickExec) -> Option<String> {
    (item.agent_written == Some(true) && item.unmodelled_args_approved != Some(true)).then(|| {
        format!(
            "Quick Exec « {} » enregistrée, inactive : elle passe des valeurs du run à un \
             programme que Kronn ne vérifie pas seul, donc elle ne tourne pas tant qu'un humain \
             ne l'a pas activée (bouton « Activer » de la carte postée dans cette room, panneau \
             « À valider », ou Automatisations → Quick Execs). Continue ton travail \
             (Page, workflow) : référence-la normalement et dis à l'utilisateur qu'elle attend \
             son activation. Ne lui demande pas de la réécrire à la main. Un agent ne peut pas \
             l'activer.",
            item.name
        )
    })
}

/// A Quick Exec an agent saved that now waits for a human: the program, when
/// the line passes run values to one Kronn does not check alone.
fn waiting_program(item: &QuickExec) -> Option<String> {
    if item.agent_written != Some(true) {
        return None;
    }
    crate::core::inline_code::quick_exec_approval_needed(
        &item.name,
        &item.command,
        &item.args,
        item.unmodelled_args_approved == Some(true),
        true,
    )
    .map(|(program, _)| program)
}

/// The activation card, as the room reads it: a System message holding one
/// JSON object after the `[kronn-activate:` marker.
pub(crate) fn activation_card_content(item: &QuickExec, program: &str) -> String {
    format!(
        "[kronn-activate: {}]",
        serde_json::json!({
            "quick_exec_id": item.id,
            "name": item.name,
            "program": program,
        })
    )
}

/// Posts the one-click activation card in the room an agent saved a waiting
/// Quick Exec from. One card per line: saving the same line again adds none,
/// a changed line adds a new one. The save never fails for the card.
pub(crate) async fn announce_activation(
    state: &AppState,
    discussion_id: Option<&str>,
    item: &QuickExec,
) {
    let (Some(discussion_id), Some(program)) = (discussion_id, waiting_program(item)) else {
        return;
    };
    use sha2::{Digest, Sha256};
    let digest =
        Sha256::digest(serde_json::to_vec(&(&item.command, &item.args)).unwrap_or_default());
    let line: String = digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let message = DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        session_continuity: None,
        kind: Default::default(),
        model: None,
        lint_report: None,
        id: format!("qe-activation:{}:{line}", item.id),
        role: MessageRole::System,
        channel: MessageChannel::Main,
        content: activation_card_content(item, &program),
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };
    let discussion_id = discussion_id.to_string();
    let result = state
        .db
        .with_conn(move |conn| {
            let known: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
                    [&message.id],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !known {
                crate::db::discussions::insert_message(conn, &discussion_id, &message)?;
            }
            Ok(())
        })
        .await;
    if let Err(error) = result {
        tracing::warn!("Quick Exec activation card not posted: {error}");
    }
}

/// The card for an agent's save: the launch's one discussion for a bridge
/// token, else the room a host session says it joined (its marker).
async fn announce_from_agent(
    state: &AppState,
    bridge: Option<&axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    marker: Option<&axum::Extension<crate::core::bridge_token::AgentMarker>>,
    response: &ApiResponse<QuickExec>,
) {
    let Some(item) = response.data.as_ref() else {
        return;
    };
    let room = match (bridge, marker) {
        (Some(axum::Extension(caller)), _) => match caller.own_discussions.as_slice() {
            [discussion_id] => Some(discussion_id.clone()),
            _ => None,
        },
        (None, Some(axum::Extension(marker))) => marker.discussion_id.clone(),
        (None, None) => None,
    };
    announce_activation(state, room.as_deref(), item).await;
}

/// The line a human read and activates; a line changed since is refused.
#[derive(Debug, Clone, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ActivateQuickExecRequest {
    pub command: String,
    pub args: Vec<String>,
}

/// Whether the stored line is still the one a human read and still waits for an
/// activation. Run inside the transaction that records the decision.
fn check_waiting_line(
    existing: &QuickExec,
    request: &ActivateQuickExecRequest,
) -> Result<(), String> {
    if existing.command != request.command.trim() || existing.args != request.args {
        return Err(
            "La ligne de cette Quick Exec a changé depuis que tu l'as lue : relis-la avant de l'activer."
                .into(),
        );
    }
    let waits = matches!(
        crate::core::inline_code::first_unsafe_placeholder_with(
            &existing.command,
            &existing.args,
            crate::core::argv_roles::Trust {
                agent_written: existing.agent_written == Some(true),
                ..Default::default()
            },
        ),
        Some(crate::core::inline_code::InlineFinding::UnmodelledProgram(
            ..
        ))
    );
    if !waits {
        return Err("Cette Quick Exec n'attend aucune activation de ce type".into());
    }
    Ok(())
}

/// Records a human's decision on the line they read: check and write are one
/// atomic step (see [`crate::db::quick_execs::settle_waiting_line`]).
async fn settle_waiting_line(
    state: &AppState,
    id: &str,
    request: ActivateQuickExecRequest,
    activate: bool,
) -> Result<QuickExec, String> {
    use crate::db::quick_execs::{settle_waiting_line, WaitingLineOutcome};
    let id = id.to_owned();
    match state
        .db
        .with_conn(move |conn| {
            settle_waiting_line(conn, &id, activate, |existing| {
                check_waiting_line(existing, &request)
            })
        })
        .await
    {
        Ok(WaitingLineOutcome::Settled(item)) => Ok(*item),
        Ok(WaitingLineOutcome::Refused(why)) => Err(why),
        Ok(WaitingLineOutcome::Gone) => Err("Quick Exec not found".into()),
        Err(error) => Err(format!("DB error: {error}")),
    }
}

/// POST /api/quick-execs/:id/activate — a human activates a Quick Exec that
/// waits (KT-1490). The route is absent from the bridge-token table and the
/// handler refuses a bridge caller or an agent-marked request, an operator
/// token included: an agent never activates.
pub async fn activate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    marker: crate::core::bridge_token::AgentMark,
    Json(request): Json<ActivateQuickExecRequest>,
) -> Json<ApiResponse<QuickExec>> {
    if crate::core::bridge_token::agent_authored(&bridge, &marker) {
        return Json(ApiResponse::err(
            "Only a human activates a Quick Exec: an agent cannot",
        ));
    }
    match settle_waiting_line(&state, &id, request, true).await {
        Ok(item) => Json(ApiResponse::ok(item)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// POST /api/quick-execs/:id/refuse-activation — a human declines the line an
/// agent saved (KT-1490). The Quick Exec stays inactive; the refusal is kept
/// for the line that was read, so the room's card says « Refusée » after a
/// reload. Human only, exactly like [`activate`]: absent from the bridge-token
/// table, refused to a bridge caller and to an agent-marked request.
pub async fn refuse_activation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    marker: crate::core::bridge_token::AgentMark,
    Json(request): Json<ActivateQuickExecRequest>,
) -> Json<ApiResponse<bool>> {
    if crate::core::bridge_token::agent_authored(&bridge, &marker) {
        return Json(ApiResponse::err(
            "Only a human refuses a Quick Exec's activation: an agent cannot",
        ));
    }
    match settle_waiting_line(&state, &id, request, false).await {
        Ok(_) => Json(ApiResponse::ok(true)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// The [`Trust`](crate::core::argv_roles::Trust) of a saved Quick Exec: it
/// pins no script, so a script shape needs the approval.
pub(crate) fn quick_exec_trust(item: &QuickExec) -> crate::core::argv_roles::Trust {
    crate::core::argv_roles::Trust {
        approved: item.unmodelled_args_approved == Some(true),
        agent_written: item.agent_written == Some(true),
        declared_scripts: Vec::new(),
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
    crate::models::validate_prompt_variables(&request.variables)?;
    Ok(())
}

pub(crate) fn validate_variables(
    declarations: &[PromptVariable],
    values: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    for variable in declarations {
        variable.validate_source()?;
        if !variable.requires_user_input() {
            continue;
        }
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

#[cfg(all(test, feature = "full-tests"))]
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
            unmodelled_args_approved: None,
        }
    }

    #[test]
    fn inline_code_interpolation_is_refused_with_the_recipe() {
        let mut unsafe_request = request("python3");
        unsafe_request.args = vec!["-c".into(), "print('{{ticket}}')".into()];
        let error = inline_code_error(&unsafe_request, None).unwrap();
        assert!(error.contains("Quick Exec « Collect AWS »"), "{error}");
        assert!(
            error.contains("{{ticket}}") && error.contains("sys.argv"),
            "{error}"
        );
        assert!(error.contains("Arguments proposés"), "{error}");
        for args in [
            vec!["-cprint('{{ticket}}')"],
            vec!["--command=print('{{ticket}}')"],
        ] {
            unsafe_request.args = args.iter().map(|arg| arg.to_string()).collect();
            assert!(
                inline_code_error(&unsafe_request, None).is_some(),
                "{args:?}"
            );
        }

        let mut safe = request("python3");
        safe.args = vec![
            "-c".into(),
            "import sys; print(sys.argv[1])".into(),
            "{{ticket}}".into(),
        ];
        assert!(inline_code_error(&safe, None).is_none());
        let mut plain = request("aws");
        plain.args = vec!["s3".into(), "ls".into(), "{{bucket}}".into()];
        // `aws` is not modelled: a value reaches it only with a human's approval.
        assert!(inline_code_error(&plain, None).unwrap().contains("aws"));
        plain.unmodelled_args_approved = Some(true);
        assert!(inline_code_error(&plain, None).is_none());
    }

    /// KT-1492: the run's step carries the trust level's mark, so
    /// `confined_for_level` wraps a level-approved line in the vault sandbox.
    /// Without a real sandbox in the test host this is what pins the wiring
    /// (the end-to-end read of the key is in `api_tests`).
    #[test]
    #[serial_test::serial]
    fn a_level_approved_quick_exec_runs_as_a_step_the_sandbox_wraps() {
        let item = |by_trust: bool| QuickExec {
            approved_by_trust: by_trust.then_some(true),
            id: "q".into(),
            name: "Read".into(),
            icon: String::new(),
            description: String::new(),
            project_id: None,
            command: "cat".into(),
            args: vec!["/data/encryption_key".into()],
            timeout_secs: 10,
            output_format: CollectQuickExecOutputFormat::Text,
            variables: vec![],
            pinned: false,
            unmodelled_args_approved: Some(true),
            agent_written: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let by_level = run_step(&item(true));
        assert_eq!(by_level.exec_approved_by_trust, Some(true));
        assert_eq!(by_level.exec_unmodelled_args_approved, Some(true));
        let by_human = run_step(&item(false));
        assert_eq!(by_human.exec_approved_by_trust, None);
        // The step the run builds is what `confined_for_level` decides on.
        let args = by_level.exec_args.clone();
        let (program, launch) =
            crate::workflows::exec_step::confined_for_level(&by_level, "cat", &args);
        assert_ne!(program, "cat", "a level-approved line was not wrapped");
        assert_eq!(launch.last(), args.last());
        let (program, _) = crate::workflows::exec_step::confined_for_level(&by_human, "cat", &args);
        assert_eq!(program, "cat");
    }

    /// R5-05 and B: an imported Quick Exec never carries an approval, and
    /// one imported or saved by an agent needs a human for any value.
    #[tokio::test]
    async fn quick_exec_imports_and_agent_saves_need_a_human_for_values() {
        let state = state();
        let mut item = QuickExec {
            approved_by_trust: None,
            id: "src".into(),
            name: "Greet".into(),
            icon: String::new(),
            description: String::new(),
            project_id: None,
            command: "python3".into(),
            args: vec![
                "-c".into(),
                "import sys; print(sys.argv[1])".into(),
                "{{x}}".into(),
            ],
            timeout_secs: 10,
            output_format: CollectQuickExecOutputFormat::Text,
            variables: vec![],
            pinned: false,
            unmodelled_args_approved: None,
            agent_written: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let import_as = |item: QuickExec, by_agent: bool| {
            let state = state.clone();
            async move {
                let content = serde_json::to_string(&QuickExecExportEnvelope {
                    kind: EXPORT_KIND.into(),
                    version: EXPORT_VERSION,
                    exported_at: Utc::now(),
                    quick_exec: item,
                    redacted_fields: vec![],
                })
                .unwrap();
                let bridge = by_agent.then(|| {
                    axum::Extension(crate::core::bridge_token::BridgeCaller {
                        token_id: "t".into(),
                        project: None,
                        own_discussions: vec![],
                        own_run: None,
                        agent: None,
                    })
                });
                import(
                    State(state),
                    bridge,
                    None,
                    Json(ImportQuickExecRequest {
                        content,
                        project_id: None,
                    }),
                )
                .await
                .0
            }
        };
        assert!(
            import_as(item.clone(), false).await.success,
            "a human's shape is trusted"
        );
        let by_agent = import_as(item.clone(), true).await;
        assert!(
            by_agent.notice.is_some(),
            "an agent's line waits for a human"
        );
        assert_eq!(by_agent.data.unwrap().agent_written, Some(true));
        item.command = "aws".into();
        item.args = vec!["s3".into(), "ls".into(), "{{x}}".into()];
        item.unmodelled_args_approved = Some(true);
        assert!(
            !import_as(item, false).await.success,
            "the file's approval is dropped"
        );

        let mut shape = request("python3");
        shape.args = vec![
            "-c".into(),
            "import sys; print(sys.argv[1])".into(),
            "{{x}}".into(),
        ];
        let by_agent = create_as(state.clone(), shape.clone(), true).await.0;
        assert_eq!(by_agent.data.unwrap().agent_written, Some(true));
        let human = create_as(state.clone(), shape, false).await.0;
        assert!(human.success, "{:?}", human.error);
        assert_eq!(human.data.unwrap().agent_written, None);
    }

    /// R6-07: a human approves an agent-written Quick Exec; the line stays
    /// the agent's and runs.
    #[tokio::test]
    async fn a_human_approves_an_agent_written_quick_exec() {
        let state = state();
        let mut line = request("aws");
        line.args = vec!["s3".into(), "ls".into(), "{{bucket}}".into()];
        let created = create_as(state.clone(), line.clone(), true)
            .await
            .0
            .data
            .unwrap();
        assert_eq!(created.agent_written, Some(true));
        line.unmodelled_args_approved = Some(true);
        let approved = update_as(state.clone(), created.id.clone(), line, false)
            .await
            .0;
        assert!(approved.success, "{:?}", approved.error);
        assert!(approved.notice.is_none());
        let stored = state
            .db
            .with_conn(move |conn| crate::db::quick_execs::get_quick_exec(conn, &created.id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.unmodelled_args_approved, Some(true));
        assert_eq!(
            stored.agent_written,
            Some(true),
            "the line is still the agent's"
        );
        let step = WorkflowStep {
            name: stored.name.clone(),
            step_type: StepType::Exec,
            exec_command: Some(stored.command.clone()),
            exec_args: stored.args.clone(),
            exec_unmodelled_args_approved: stored.unmodelled_args_approved,
            exec_agent_written: stored.agent_written,
            ..Default::default()
        };
        assert_eq!(crate::core::inline_code::runtime_refusal(&step), None);
    }

    fn state() -> crate::AppState {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    fn bridge() -> Option<axum::Extension<crate::core::bridge_token::BridgeCaller>> {
        Some(axum::Extension(crate::core::bridge_token::BridgeCaller {
            token_id: "t".into(),
            project: None,
            own_discussions: vec![],
            own_run: None,
            agent: None,
        }))
    }

    /// Only a human approves an unmodelled program: the bridge and the Kronn
    /// agent tools lose the flag, on create and on a changed line.
    #[tokio::test]
    async fn only_a_human_approves_an_unmodelled_program() {
        let state = state();
        let mut approved = request("aws");
        approved.args = vec!["s3".into(), "ls".into(), "{{bucket}}".into()];
        approved.unmodelled_args_approved = Some(true);
        let clone = |r: &CreateQuickExecRequest| -> CreateQuickExecRequest {
            serde_json::from_value(serde_json::json!({
                "name": r.name, "description": "", "project_id": null, "command": r.command,
                "args": r.args, "timeout_secs": 60, "output_format": "json", "variables": [],
                "unmodelled_args_approved": r.unmodelled_args_approved,
            }))
            .unwrap()
        };
        // An agent's save goes through, unapproved and named as waiting.
        let Json(by_bridge) =
            create(State(state.clone()), bridge(), None, Json(clone(&approved))).await;
        let Json(by_tools) = create_as(state.clone(), clone(&approved), true).await;
        for saved in [by_bridge, by_tools] {
            assert!(saved.success, "{:?}", saved.error);
            assert!(saved.notice.is_some(), "the agent is told it waits");
            let item = saved.data.unwrap();
            assert_eq!(
                item.unmodelled_args_approved, None,
                "an agent cannot approve"
            );
            assert_eq!(item.agent_written, Some(true));
        }
        let Json(by_human) = create(State(state.clone()), None, None, Json(clone(&approved))).await;
        assert!(by_human.success, "{:?}", by_human.error);
        let id = by_human.data.unwrap().id;

        // The agent keeps the approval only on the very line a human approved.
        let mut same = clone(&approved);
        same.unmodelled_args_approved = None;
        same.description = "agent edit".into();
        let Json(kept) = update_as(state.clone(), id.clone(), same, true).await;
        assert_eq!(kept.data.unwrap().unmodelled_args_approved, Some(true));
        let mut changed = clone(&approved);
        changed.args = vec!["s3".into(), "rm".into(), "{{bucket}}".into()];
        let Json(dropped) = update(
            State(state.clone()),
            Path(id),
            bridge(),
            None,
            Json(changed),
        )
        .await;
        let dropped = dropped.data.expect("saved, waiting for a human");
        assert_eq!(
            dropped.unmodelled_args_approved, None,
            "a changed line loses the approval"
        );
        assert_eq!(dropped.agent_written, Some(true));
    }

    fn purge_line() -> CreateQuickExecRequest {
        let mut line = request("fastly");
        line.name = "Purge CDN".into();
        line.args = vec!["purge".into(), "--key".into(), "{{surrogate_key}}".into()];
        line.variables = serde_json::from_value(serde_json::json!([
            {"name": "surrogate_key", "label": "Key", "placeholder": "", "required": true}
        ]))
        .unwrap();
        line
    }

    fn bridge_in(
        discussion: &str,
    ) -> Option<axum::Extension<crate::core::bridge_token::BridgeCaller>> {
        Some(axum::Extension(crate::core::bridge_token::BridgeCaller {
            token_id: "t".into(),
            project: None,
            own_discussions: vec![discussion.into()],
            own_run: None,
            agent: None,
        }))
    }

    async fn room(state: &crate::AppState, id: &str) {
        let id = id.to_string();
        state
            .db
            .with_conn(move |conn| {
                let now = Utc::now();
                crate::db::discussions::insert_discussion(
                    conn,
                    &Discussion {
                        connection_id: None,
                        awaiting_agent: false,
                        agent_running: false,
                        id,
                        project_id: None,
                        title: "room".into(),
                        agent: AgentType::ClaudeCode,
                        language: "en".into(),
                        participants: vec![AgentType::ClaudeCode],
                        message_count: 0,
                        non_system_message_count: 0,
                        messages: vec![],
                        skill_ids: vec![],
                        profile_ids: vec![],
                        directive_ids: vec![],
                        archived: false,
                        pinned: false,
                        workspace_mode: "Direct".into(),
                        workspace_path: None,
                        worktree_branch: None,
                        tier: ModelTier::Default,
                        model: None,
                        pin_first_message: false,
                        summary_cache: None,
                        summary_up_to_msg_idx: None,
                        summary_strategy: SummaryStrategy::OnDemand,
                        introspection_call_count: 0,
                        shared_id: None,
                        shared_with: vec![],
                        workflow_run_id: None,
                        test_mode_restore_branch: None,
                        test_mode_stash_ref: None,
                        created_at: now,
                        updated_at: now,
                    },
                )
            })
            .await
            .unwrap();
    }

    async fn cards_in(state: &crate::AppState, id: &str) -> Vec<String> {
        let id = id.to_string();
        state
            .db
            .with_conn(move |conn| {
                let mut statement = conn.prepare(
                    "SELECT content FROM messages WHERE discussion_id = ?1 \
                     AND content LIKE '[kronn-activate:%'",
                )?;
                let rows = statement.query_map([&id], |row| row.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await
            .unwrap()
    }

    async fn stored(state: &crate::AppState, id: &str) -> QuickExec {
        let id = id.to_string();
        state
            .db
            .with_conn(move |conn| crate::db::quick_execs::get_quick_exec(conn, &id))
            .await
            .unwrap()
            .unwrap()
    }

    /// KT-1490: an agent saves a line that passes run values to a program
    /// Kronn does not model (`fastly purge --key {{surrogate_key}}`) on every
    /// path; it is stored inactive and the response says how it is activated.
    #[tokio::test]
    async fn an_agent_saves_an_unmodelled_cli_inactive_on_every_path() {
        let state = state();
        room(&state, "room-1").await;
        let Json(by_bridge) = create(
            State(state.clone()),
            bridge_in("room-1"),
            None,
            Json(purge_line()),
        )
        .await;
        let Json(by_tools) = create_as(state.clone(), purge_line(), true).await;
        let Json(seed) = create_as(state.clone(), request("fastly"), true).await;
        let seed = seed.data.expect("a line without a value is saved");
        let mut edited = purge_line();
        edited.name = "Purge CDN (edit)".into();
        let Json(by_update) = update_as(state.clone(), seed.id, edited, true).await;
        for saved in [by_bridge, by_tools, by_update] {
            assert!(saved.success, "{:?}", saved.error);
            let notice = saved.notice.clone().expect("the agent is told it waits");
            assert!(
                notice.contains("inactive")
                    && notice.contains("Activer")
                    && notice.contains("humain"),
                "{notice}"
            );
            let item = saved.data.unwrap();
            assert_eq!(item.unmodelled_args_approved, None, "stored inactive");
            assert_eq!(item.agent_written, Some(true));
        }
        // The room where the agent saved it receives one activation card.
        let cards = cards_in(&state, "room-1").await;
        assert_eq!(cards.len(), 1, "{cards:?}");
        assert!(cards[0].contains("\"program\":\"fastly\"") && cards[0].contains("Purge CDN"));
        // The same line announced again adds no second card.
        let id = serde_json::from_str::<serde_json::Value>(
            cards[0]
                .trim_start_matches("[kronn-activate: ")
                .trim_end_matches(']'),
        )
        .unwrap()["quick_exec_id"]
            .as_str()
            .unwrap()
            .to_string();
        announce_activation(&state, Some("room-1"), &stored(&state, &id).await).await;
        assert_eq!(
            cards_in(&state, "room-1").await.len(),
            1,
            "same line, same card"
        );
    }

    /// KT-1490: the run refuses an inactive line with the activation hint;
    /// one human activation lets it through; a bridge token cannot activate.
    #[tokio::test]
    async fn an_inactive_line_is_refused_until_a_human_activates_it() {
        let state = state();
        let Json(saved) = create_as(state.clone(), purge_line(), true).await;
        let item = saved.data.unwrap();
        let (_, hint) = crate::core::inline_code::quick_exec_approval_needed(
            &item.name,
            &item.command,
            &item.args,
            false,
            true,
        )
        .expect("waits");
        assert!(
            hint.contains("en attente d'activation") && hint.contains("Activer"),
            "{hint}"
        );

        let line = ActivateQuickExecRequest {
            command: item.command.clone(),
            args: item.args.clone(),
        };
        // No bridge token activates: the handler refuses and the route is not
        // in the bridge table.
        let Json(by_agent) = activate(
            State(state.clone()),
            Path(item.id.clone()),
            bridge(),
            None,
            Json(line.clone()),
        )
        .await;
        assert!(!by_agent.success);
        assert!(
            crate::core::bridge_token::route_for("POST", "/api/quick-execs/{id}/activate")
                .is_none()
        );
        assert_eq!(
            stored(&state, &item.id).await.unmodelled_args_approved,
            None
        );
        // An agent's own save cannot carry the approval either.
        let mut forged = purge_line();
        forged.unmodelled_args_approved = Some(true);
        let Json(forged) = update_as(state.clone(), item.id.clone(), forged, true).await;
        assert_eq!(forged.data.unwrap().unmodelled_args_approved, None);

        // A line that changed since the human read it is not activated.
        let Json(stale) = activate(
            State(state.clone()),
            Path(item.id.clone()),
            None,
            None,
            Json(ActivateQuickExecRequest {
                command: "fastly".into(),
                args: vec!["purge".into(), "--all".into()],
            }),
        )
        .await;
        assert!(!stale.success);

        let step = |item: &QuickExec| WorkflowStep {
            name: item.name.clone(),
            step_type: StepType::Exec,
            exec_command: Some(item.command.clone()),
            exec_args: item.args.clone(),
            exec_unmodelled_args_approved: item.unmodelled_args_approved,
            exec_agent_written: item.agent_written,
            ..Default::default()
        };
        assert!(crate::core::inline_code::runtime_refusal(&step(&item)).is_some());
        let Json(pending) = pending_approvals(State(state.clone())).await;
        assert_eq!(pending.data.unwrap().len(), 1);
        let Json(activated) = activate(
            State(state.clone()),
            Path(item.id.clone()),
            None,
            None,
            Json(line),
        )
        .await;
        assert!(activated.success, "{:?}", activated.error);
        let activated = stored(&state, &item.id).await;
        assert_eq!(activated.unmodelled_args_approved, Some(true));
        assert_eq!(activated.agent_written, Some(true));
        assert_eq!(
            crate::core::inline_code::runtime_refusal(&step(&activated)),
            None
        );
        let Json(pending) = pending_approvals(State(state.clone())).await;
        assert!(pending.data.unwrap().is_empty());
        // A later agent edit of the line loses the activation.
        let mut edited = purge_line();
        edited.args.push("--soft".into());
        let Json(edited) = update_as(state.clone(), item.id.clone(), edited, true).await;
        assert_eq!(edited.data.unwrap().unmodelled_args_approved, None);
    }

    /// KT-1490: the human activates the line they read. A line whose program
    /// alone changed since is refused, though its args are the ones read.
    #[tokio::test]
    async fn activation_refuses_a_line_whose_command_alone_changed() {
        let state = state();
        let Json(saved) = create_as(state.clone(), purge_line(), true).await;
        let item = saved.data.unwrap();
        let other = ActivateQuickExecRequest {
            command: "gcloud".into(),
            args: item.args.clone(),
        };
        let Json(activated) = activate(
            State(state.clone()),
            Path(item.id.clone()),
            None,
            None,
            Json(other.clone()),
        )
        .await;
        let Json(refused) = refuse_activation(
            State(state.clone()),
            Path(item.id.clone()),
            None,
            None,
            Json(other),
        )
        .await;
        assert!(!activated.success && !refused.success);
        for error in [activated.error, refused.error] {
            assert!(
                error.unwrap_or_default().contains("a changé"),
                "the read line is no longer the stored one"
            );
        }
        assert_eq!(
            stored(&state, &item.id).await.unmodelled_args_approved,
            None
        );
        // The stored line itself, with the padding a form leaves, still goes through.
        let Json(padded) = activate(
            State(state.clone()),
            Path(item.id.clone()),
            None,
            None,
            Json(ActivateQuickExecRequest {
                command: " fastly ".into(),
                args: item.args.clone(),
            }),
        )
        .await;
        assert!(padded.success, "{:?}", padded.error);
    }

    /// KT-1490: a line that waits for no activation (it passes no run value to
    /// an unmodelled program) is neither activated nor refused: there is
    /// nothing for a human to decide, and no approval is written for it.
    #[tokio::test]
    async fn a_line_that_waits_for_nothing_is_not_activated() {
        let state = state();
        let mut plain = purge_line();
        plain.name = "Purge all".into();
        plain.args = vec!["purge".into(), "--all".into()];
        plain.variables = Vec::new();
        let Json(saved) = create_as(state.clone(), plain, true).await;
        let item = saved.data.expect("an agent can save a line without values");
        let line = ActivateQuickExecRequest {
            command: item.command.clone(),
            args: item.args.clone(),
        };
        let Json(activated) = activate(
            State(state.clone()),
            Path(item.id.clone()),
            None,
            None,
            Json(line.clone()),
        )
        .await;
        assert!(!activated.success);
        assert!(activated
            .error
            .unwrap_or_default()
            .contains("n'attend aucune activation"));
        let Json(refused) = refuse_activation(
            State(state.clone()),
            Path(item.id.clone()),
            None,
            None,
            Json(line),
        )
        .await;
        assert!(!refused.success);
        assert_eq!(
            stored(&state, &item.id).await.unmodelled_args_approved,
            None,
            "no approval written for a line that needed none"
        );
    }

    /// KT-1490 (owner review): the read, the check and the write of an
    /// activation are one step. An agent edit landing inside it is not undone by
    /// the write (the old line, with its stale fields, never comes back), and a
    /// delete there is a conflict, not a success.
    #[tokio::test]
    async fn activation_is_atomic_against_a_concurrent_edit_or_delete() {
        use crate::db::quick_execs::{settle_waiting_line, WaitingLineOutcome};
        let state = state();
        let Json(saved) = create_as(state.clone(), purge_line(), true).await;
        let item = saved.data.unwrap();

        // The line changes after the check read it: a conflict, nothing approved.
        let id = item.id.clone();
        let outcome = state
            .db
            .with_conn(move |conn| {
                settle_waiting_line(conn, &id, true, |_| {
                    conn.execute(
                        "UPDATE quick_execs SET args_json = '[\"service\",\"delete\"]' WHERE id = ?1",
                        [&id],
                    ).unwrap();
                    Ok(())
                })
            })
            .await
            .unwrap();
        assert!(matches!(outcome, WaitingLineOutcome::Refused(why) if why.contains("a changé")));
        assert_eq!(
            stored(&state, &item.id).await.unmodelled_args_approved,
            None
        );

        // A field outside the line changes: the activation lands and does not
        // rewrite it back to what was read.
        let Json(saved) = create_as(state.clone(), purge_line(), true).await;
        let other = saved.data.unwrap();
        let id = other.id.clone();
        let outcome = state
            .db
            .with_conn(move |conn| {
                settle_waiting_line(conn, &id, true, |_| {
                    conn.execute(
                        "UPDATE quick_execs SET timeout_secs = 7, name = 'Renamed' WHERE id = ?1",
                        [&id],
                    )
                    .unwrap();
                    Ok(())
                })
            })
            .await
            .unwrap();
        assert!(matches!(outcome, WaitingLineOutcome::Settled(_)));
        let after = stored(&state, &other.id).await;
        assert_eq!(after.unmodelled_args_approved, Some(true));
        assert_eq!(
            after.timeout_secs, 7,
            "an unrelated edit is not overwritten"
        );
        assert_eq!(after.name, "Renamed");

        // The row is deleted inside the step: a conflict, never a success.
        let Json(saved) = create_as(state.clone(), purge_line(), true).await;
        let gone = saved.data.unwrap();
        let id = gone.id.clone();
        let outcome = state
            .db
            .with_conn(move |conn| {
                settle_waiting_line(conn, &id, true, |_| {
                    conn.execute("DELETE FROM quick_execs WHERE id = ?1", [&id])
                        .unwrap();
                    Ok(())
                })
            })
            .await
            .unwrap();
        assert!(matches!(outcome, WaitingLineOutcome::Refused(_)));
    }

    /// KT-1492: a human activation is a human approval. A line that still
    /// carried a level's mark loses it, so lowering the level later cannot take
    /// back what a person chose to activate.
    #[tokio::test]
    async fn a_human_activation_clears_a_stale_trust_mark() {
        use crate::db::quick_execs::{settle_waiting_line, WaitingLineOutcome};
        let state = state();
        let Json(saved) = create_as(state.clone(), purge_line(), true).await;
        let item = saved.data.unwrap();
        let id = item.id.clone();
        let outcome = state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE quick_execs SET approved_by_trust = 1 WHERE id = ?1",
                    [&id],
                )?;
                settle_waiting_line(conn, &id, true, |_| Ok(()))
            })
            .await
            .unwrap();
        let WaitingLineOutcome::Settled(settled) = outcome else {
            panic!("the activation must land");
        };
        assert_eq!(settled.approved_by_trust, None);
        let after = stored(&state, &item.id).await;
        assert_eq!(after.unmodelled_args_approved, Some(true));
        assert_eq!(after.approved_by_trust, None);
    }

    /// KT-1490: « Refuser » is recorded server-side by a human only, names the
    /// line that was read, leaves the line inactive, and is asked again once
    /// the line changes.
    #[tokio::test]
    async fn a_refusal_is_recorded_by_a_human_only_and_the_line_stays_inactive() {
        let state = state();
        let Json(saved) = create_as(state.clone(), purge_line(), true).await;
        let item = saved.data.unwrap();
        let line = ActivateQuickExecRequest {
            command: item.command.clone(),
            args: item.args.clone(),
        };
        let refused_flag = |state: AppState| async move {
            let Json(pending) = pending_approvals(State(state)).await;
            pending.data.unwrap()[0].refused
        };
        assert!(!refused_flag(state.clone()).await);

        // Neither a bridge token nor a marked request refuses on a human's behalf.
        let Json(by_token) = refuse_activation(
            State(state.clone()),
            Path(item.id.clone()),
            bridge(),
            None,
            Json(line.clone()),
        )
        .await;
        assert!(!by_token.success);
        let Json(by_marker) = refuse_activation(
            State(state.clone()),
            Path(item.id.clone()),
            None,
            Some(axum::Extension(
                crate::core::bridge_token::AgentMarker::default(),
            )),
            Json(line.clone()),
        )
        .await;
        assert!(!by_marker.success);
        assert!(!refused_flag(state.clone()).await);
        assert!(crate::core::bridge_token::route_for(
            "POST",
            "/api/quick-execs/{id}/refuse-activation"
        )
        .is_none());

        // A line other than the one stored is not refused.
        let Json(stale) = refuse_activation(
            State(state.clone()),
            Path(item.id.clone()),
            None,
            None,
            Json(ActivateQuickExecRequest {
                command: "fastly".into(),
                args: vec!["purge".into(), "--all".into()],
            }),
        )
        .await;
        assert!(!stale.success);

        let Json(refused) = refuse_activation(
            State(state.clone()),
            Path(item.id.clone()),
            None,
            None,
            Json(line.clone()),
        )
        .await;
        assert!(refused.success, "{:?}", refused.error);
        assert!(
            refused_flag(state.clone()).await,
            "still listed, now refused"
        );
        let kept = stored(&state, &item.id).await;
        assert_eq!(
            kept.unmodelled_args_approved, None,
            "the line stays inactive"
        );

        // Editing the line asks again.
        let mut edited = purge_line();
        edited.args.push("--soft".into());
        let _ = update_as(state.clone(), item.id.clone(), edited, true).await;
        assert!(!refused_flag(state.clone()).await);
    }

    /// KT-1490: shells, paths, wrappers and inline-code interpolation stay
    /// refused at save for an agent.
    #[tokio::test]
    async fn an_agent_save_still_refuses_what_is_not_a_waiting_line() {
        let state = state();
        let mut shell = request("bash");
        shell.args = vec!["-c".into(), "echo {{x}}".into()];
        assert!(!create_as(state.clone(), shell, true).await.0.success);
        let mut path = request("/usr/bin/fastly");
        path.args = vec!["purge".into(), "{{x}}".into()];
        assert!(!create_as(state.clone(), path, true).await.0.success);
        let mut inline = request("python3");
        inline.args = vec!["-c".into(), "print('{{x}}')".into()];
        assert!(!create_as(state.clone(), inline, true).await.0.success);
        let mut wrapper = request("env");
        wrapper.args = vec!["{{x}}".into()];
        assert!(!create_as(state.clone(), wrapper, true).await.0.success);
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
