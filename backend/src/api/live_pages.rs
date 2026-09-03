use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::models::{
    ApiErrorCode, ApiResponse, CreateLivePageDataset, CreateLivePageRequest,
    LinkLivePageDiscussionRequest, LivePage, LivePageDataset, LivePageDiscussionRelation,
    LivePageRevision, LivePageWorkflowBinding, PageGateDecisionRequest, PageTriggerRequest,
    PageTriggerResponse, PublishLivePageRequest, RunStatus, UpdateLivePageHtmlRequest,
    UpdateLivePageRequest, UpsertLivePageBindingRequest,
};
use crate::AppState;

const MAX_PAGE_HTML_BYTES: usize = 1_000_000;

pub async fn capability(
    State(state): State<AppState>,
) -> Json<ApiResponse<crate::models::LivePagesCapability>> {
    match state
        .db
        .with_read_conn(crate::db::live_pages::pages_capability)
        .await
    {
        Ok(value) => Json(ApiResponse::ok(value)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to read Pages capability: {error}"),
        )),
    }
}

pub async fn list(State(state): State<AppState>) -> Json<ApiResponse<Vec<LivePage>>> {
    match state
        .db
        .with_read_conn(crate::db::live_pages::list_live_pages)
        .await
    {
        Ok(pages) => Json(ApiResponse::ok(pages)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to list Pages: {error}"),
        )),
    }
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<crate::models::LivePageDetail>> {
    match state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::get_live_page(conn, &id))
        .await
    {
        Ok(Some(page)) => Json(ApiResponse::ok(page)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to load Page: {error}"),
        )),
    }
}

pub async fn revisions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<LivePageRevision>>> {
    match state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::list_live_page_revisions(conn, &id))
        .await
    {
        Ok(revisions) if revisions.is_empty() => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Ok(revisions) => Json(ApiResponse::ok(revisions)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to load Page revisions: {error}"),
        )),
    }
}

pub async fn workflows(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::LivePageWorkflowLink>>> {
    match state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::list_live_page_workflows(conn, &id))
        .await
    {
        Ok(Some(workflows)) => Json(ApiResponse::ok(workflows)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to load Page workflows: {error}"),
        )),
    }
}

pub async fn publications(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::LivePagePublication>>> {
    match state
        .db
        .with_read_conn(move |conn| {
            crate::db::live_pages::list_live_page_publications(conn, &id, 3)
        })
        .await
    {
        Ok(Some(publications)) => Json(ApiResponse::ok(publications)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to load Page publications: {error}"),
        )),
    }
}

pub async fn discussions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::LivePageDiscussionLink>>> {
    match state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::list_live_page_discussions(conn, &id))
        .await
    {
        Ok(Some(discussions)) => Json(ApiResponse::ok(discussions)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to load Page discussions: {error}"),
        )),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(mut request): Json<UpdateLivePageRequest>,
) -> Json<ApiResponse<crate::models::LivePageDetail>> {
    if let Some(title) = request.title.take() {
        request.title = match normalize_page_title(&title) {
            Ok(title) => Some(title),
            Err(message) => {
                return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message));
            }
        };
    }
    match state
        .db
        .with_conn(move |conn| crate::db::live_pages::update_live_page(conn, &id, &request))
        .await
    {
        Ok(Some(page)) => Json(ApiResponse::ok(page)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to update Page: {error}"),
        )),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state
        .db
        .with_conn(move |conn| crate::db::live_pages::delete_live_page(conn, &id))
        .await
    {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to delete Page: {error}"),
        )),
    }
}

pub async fn link_discussion(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<LinkLivePageDiscussionRequest>,
) -> Json<ApiResponse<Vec<crate::models::LivePageDiscussionLink>>> {
    let discussion_id = request.discussion_id.trim().to_string();
    if discussion_id.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "discussion_id is required",
        ));
    }
    let relation = request
        .relation
        .unwrap_or(LivePageDiscussionRelation::Attached);
    let page_id = id.clone();
    match state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::link_live_page_discussion(conn, &id, &discussion_id, relation)
        })
        .await
    {
        Ok(true) => match state
            .db
            .with_read_conn(move |conn| {
                crate::db::live_pages::list_live_page_discussions(conn, &page_id)
            })
            .await
        {
            Ok(Some(links)) => Json(ApiResponse::ok(links)),
            Ok(None) => Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Page not found",
            )),
            Err(error) => Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("Unable to reload Page discussions: {error}"),
            )),
        },
        Ok(false) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) if error.to_string().contains("FOREIGN KEY constraint") => Json(
            ApiResponse::err_coded(ApiErrorCode::NotFound, "Discussion not found"),
        ),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to link Page discussion: {error}"),
        )),
    }
}

pub async fn unlink_discussion(
    State(state): State<AppState>,
    Path((id, discussion_id)): Path<(String, String)>,
) -> Json<ApiResponse<()>> {
    match state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::unlink_live_page_discussion(conn, &id, &discussion_id)
        })
        .await
    {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page discussion link not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to unlink Page discussion: {error}"),
        )),
    }
}

pub async fn update_html(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateLivePageHtmlRequest>,
) -> Json<ApiResponse<LivePageRevision>> {
    if request.html.trim().is_empty() || request.html.len() > MAX_PAGE_HTML_BYTES {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Page HTML must be non-empty and at most 1 MB",
        ));
    }
    let html = request.html;
    let actor = request.created_by_agent;
    match state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::update_live_page_html(conn, &id, &html, actor.as_deref())
        })
        .await
    {
        Ok(revision) => Json(ApiResponse::ok(revision)),
        Err(error) if error.to_string().contains("Page not found") => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to update Page HTML: {error}"),
        )),
    }
}

pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateLivePageRequest>,
) -> Json<ApiResponse<crate::models::LivePageDetail>> {
    let title = match normalize_page_title(&request.title) {
        Ok(title) => title,
        Err(message) => {
            return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message));
        }
    };
    if request.html.trim().is_empty() || request.html.len() > MAX_PAGE_HTML_BYTES {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Page HTML must be non-empty and at most 1 MB",
        ));
    }
    let slug = request
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| slugify(&title));
    if !valid_slug(&slug) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Page slug must contain lowercase ASCII letters, digits and single '-' separators",
        ));
    }

    let now = Utc::now();
    let page_id = Uuid::new_v4().to_string();
    let revision_id = Uuid::new_v4().to_string();
    let page = LivePage {
        id: page_id.clone(),
        project_id: request.project_id,
        title,
        slug,
        current_revision_id: revision_id.clone(),
        data_revision: 0,
        created_at: now,
        updated_at: now,
        last_published_at: None,
        pinned: false,
        archived: false,
    };
    let revision = LivePageRevision {
        id: revision_id,
        page_id: page_id.clone(),
        revision: 1,
        html: request.html,
        created_by_agent: request.created_by_agent,
        created_at: now,
    };
    let page_for_insert = page.clone();
    let revision_for_insert = revision.clone();
    let datasets = request.datasets;
    let discussion_id = request.discussion_id;
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::create_live_page(
                conn,
                &page_for_insert,
                &revision_for_insert,
                &datasets,
                discussion_id.as_deref(),
            )
        })
        .await
    {
        let message = error.to_string();
        let code = if message.contains("UNIQUE constraint") || message.contains("Dataset names") {
            ApiErrorCode::Conflict
        } else {
            ApiErrorCode::Internal
        };
        return Json(ApiResponse::err_coded(
            code,
            format!("Unable to create Page: {error}"),
        ));
    }
    match state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::get_live_page(conn, &page_id))
        .await
    {
        Ok(Some(detail)) => Json(ApiResponse::ok(detail)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            "Page was created but could not be reloaded",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Page was created but could not be reloaded: {error}"),
        )),
    }
}

pub async fn add_dataset(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(dataset): Json<CreateLivePageDataset>,
) -> Json<ApiResponse<LivePageDataset>> {
    match state
        .db
        .with_conn(move |conn| crate::db::live_pages::add_live_page_dataset(conn, &id, &dataset))
        .await
    {
        Ok(dataset) => Json(ApiResponse::ok(dataset)),
        Err(error) if error.to_string().contains("Page not found") => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => {
            let message = error.to_string();
            let code = if message.contains("already exists with kind")
                || message.contains("UNIQUE constraint")
            {
                ApiErrorCode::Conflict
            } else if message.contains("Dataset names")
                || message.contains("max_points")
                || message.contains("max_age_days")
            {
                ApiErrorCode::Validation
            } else {
                ApiErrorCode::Internal
            };
            Json(ApiResponse::err_coded(
                code,
                format!("Unable to add dataset: {error}"),
            ))
        }
    }
}

pub async fn list_bindings(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<LivePageWorkflowBinding>>> {
    match state
        .db
        .with_conn(move |conn| crate::db::live_pages::list_live_page_bindings(conn, &id))
        .await
    {
        Ok(Some(bindings)) => Json(ApiResponse::ok(bindings)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to list bindings: {error}"),
        )),
    }
}

pub async fn upsert_binding(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpsertLivePageBindingRequest>,
) -> Json<ApiResponse<LivePageWorkflowBinding>> {
    use crate::db::live_pages::UpsertBindingError;
    // The db layer returns a typed `UpsertBindingError` (not message text), so
    // the mapping below is total and survives any reword of the error strings.
    // `with_conn` needs an `anyhow::Result`, so the typed result rides INSIDE the
    // Ok — the outer Err is reserved for infra/panic failures.
    match state
        .db
        .with_conn(move |conn| {
            Ok(crate::db::live_pages::upsert_live_page_binding(
                conn, &id, &req,
            ))
        })
        .await
    {
        Ok(Ok(binding)) => Json(ApiResponse::ok(binding)),
        Ok(Err(UpsertBindingError::PageNotFound)) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Ok(Err(UpsertBindingError::WorkflowNotFound(workflow_id))) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            format!("Workflow not found: {workflow_id}"),
        )),
        Ok(Err(UpsertBindingError::InvalidDataset(message))) => {
            Json(ApiResponse::err_coded(ApiErrorCode::Validation, message))
        }
        Ok(Err(UpsertBindingError::Db(message))) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to save binding: {message}"),
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to save binding: {error}"),
        )),
    }
}

pub async fn delete_binding(
    State(state): State<AppState>,
    Path((id, dataset)): Path<(String, String)>,
) -> Json<ApiResponse<()>> {
    match state
        .db
        .with_conn(move |conn| crate::db::live_pages::delete_live_page_binding(conn, &id, &dataset))
        .await
    {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Binding not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to delete binding: {error}"),
        )),
    }
}

/// POST /api/pages/:id/gate-decision
///
/// Decide the gate a Page's bound run is waiting on, from the Page itself. The
/// `(page, dataset)` binding is the authorization boundary: the run must belong
/// to the bound workflow, be `WaitingApproval`, and the gate it is waiting on
/// must be listed in the binding's `allowed_gate_steps`. The actual resume goes
/// through the same audited path as the workflow-UI decide endpoint.
pub async fn decide_gate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PageGateDecisionRequest>,
) -> Json<ApiResponse<crate::api::workflows::DecideRunResponse>> {
    // 1. Resolve the binding — no binding, no authority.
    let page_id = id.clone();
    let dataset = req.dataset.clone();
    let binding = match state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::get_live_page_binding(conn, &page_id, &dataset)
        })
        .await
    {
        Ok(Some(binding)) => binding,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "No workflow binding for this page/dataset",
            ))
        }
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("DB error: {error}"),
            ))
        }
    };

    // 2. Load the run and enforce it belongs to the bound workflow.
    let run_id = req.run_id.clone();
    let run = match state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, &run_id))
        .await
    {
        Ok(Some(run)) => run,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Run not found",
            ))
        }
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("DB error: {error}"),
            ))
        }
    };
    if run.workflow_id != binding.workflow_id {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Run does not belong to the workflow bound to this page",
        ));
    }

    // The client sends the run_id it actually rendered, so trusting it (rather
    // than re-resolving "the mirrored run" server-side at click time) is what the
    // page's real use case needs: a concurrent second run of the same workflow —
    // routine for a "mise en prod" — must not flip the resolution and reject the
    // human's in-flight approval of the run they saw. Authorization is still
    // bounded below: the run must belong to the bound workflow, be
    // `WaitingApproval`, and its waiting gate must be in `allowed_gate_steps`.
    if run.status != RunStatus::WaitingApproval {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            format!("Run is not waiting for approval (status: {:?})", run.status),
        ));
    }

    // 3. Only the gate the run is CURRENTLY waiting on, and only if the binding
    //    lists it, may be decided from the Page. `resume_run` resumes the
    //    TRAILING step (see runner.rs), so authorize exactly that step — not
    //    merely "some waiting step" — to keep the allowlist check and the resume
    //    target provably identical if a future step is ever recorded past a gate.
    let current_gate = match run.step_results.last() {
        Some(result) if result.status == RunStatus::WaitingApproval => result.step_name.clone(),
        _ => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                "Run has no waiting gate step",
            ))
        }
    };
    if !binding
        .allowed_gate_steps
        .iter()
        .any(|s| s == &current_gate)
    {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!("Gate `{current_gate}` is not decidable from this page"),
        ));
    }

    // 4. Load the workflow and apply the decision through the shared audited path.
    let wf_id = binding.workflow_id.clone();
    let workflow = match state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id))
        .await
    {
        Ok(Some(workflow)) => workflow,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Workflow not found",
            ))
        }
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("DB error: {error}"),
            ))
        }
    };
    let decision =
        match crate::api::workflows::parse_gate_decision(&req.decision, req.comment.clone()) {
            Ok(decision) => decision,
            Err(error) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, error)),
        };
    match crate::api::workflows::resume_with_decision(&state, run, workflow, decision).await {
        Ok(response) => Json(ApiResponse::ok(response)),
        // A concurrent decision (page vs workflow-UI, double-click) winning the
        // atomic claim is a benign race → Conflict…
        Err(crate::api::workflows::ResumeDecisionError::LostRace) => Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "Run was just decided by another caller — decision ignored (no double-resume)",
        )),
        // …but a genuine DB/internal failure must surface as Internal, not be
        // masked as a routine race.
        Err(crate::api::workflows::ResumeDecisionError::Internal(msg)) => {
            Json(ApiResponse::err_coded(ApiErrorCode::Internal, msg))
        }
    }
}

/// POST /api/pages/:id/trigger
///
/// Trigger the workflow a Page is bound to, from the Page itself (Phase 4). The
/// `(page, dataset)` binding is the authorization boundary: it must carry a
/// `trigger_variable_allowlist` (a `None` binding is a read/gate-only mirror and
/// is not triggerable), and every launch variable the Page passes must be listed
/// in that allowlist. The workflow's own declared-variable validation still runs
/// on top (required vars enforced, unknown vars dropped). The run is dispatched
/// detached — the Page's auto-refresh/mirror surfaces its progress.
pub async fn trigger_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PageTriggerRequest>,
) -> Json<ApiResponse<PageTriggerResponse>> {
    // 1. Resolve the binding — no binding, no authority.
    let page_id = id.clone();
    let dataset = req.dataset.clone();
    let binding = match state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::get_live_page_binding(conn, &page_id, &dataset)
        })
        .await
    {
        Ok(Some(binding)) => binding,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "No workflow binding for this page/dataset",
            ))
        }
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("DB error: {error}"),
            ))
        }
    };

    // 2. The binding must opt into triggering, and bound the launch variables.
    let Some(allowlist) = binding.trigger_variable_allowlist.as_ref() else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "This page is not allowed to trigger its bound workflow",
        ));
    };
    if let Some(unknown) = req.variables.keys().find(|k| !allowlist.contains(k)) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!("Variable `{unknown}` is not allowed for this page trigger"),
        ));
    }

    // 3. Load the bound workflow and enforce it is enabled.
    let wf_id = binding.workflow_id.clone();
    let workflow = match state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id))
        .await
    {
        Ok(Some(workflow)) => workflow,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Workflow not found",
            ))
        }
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("DB error: {error}"),
            ))
        }
    };
    if !workflow.enabled {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "Workflow is disabled",
        ));
    }

    // 4. Dispatch the run through the shared detached-spawn path.
    match crate::api::workflows::spawn_detached_run(&state, workflow, req.variables).await {
        Ok(run_id) => Json(ApiResponse::ok(PageTriggerResponse { run_id })),
        Err(error) => {
            let code = if error.starts_with("Concurrency limit") {
                ApiErrorCode::Conflict
            } else if error.starts_with("DB error") {
                ApiErrorCode::Internal
            } else {
                // validate_launch_variables surfaces missing-required-var messages.
                ApiErrorCode::Validation
            };
            Json(ApiResponse::err_coded(code, error))
        }
    }
}

pub async fn publish(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PublishLivePageRequest>,
) -> Json<ApiResponse<crate::models::PublishLivePageResult>> {
    let result = state
        .db
        .with_conn(move |conn| crate::db::live_pages::publish_live_page(conn, &id, &request))
        .await;
    match result {
        Ok(publication) => Json(ApiResponse::ok(publication)),
        Err(error) => {
            let message = error.to_string();
            let code = if message == "Page not found" {
                ApiErrorCode::NotFound
            } else if message.contains("requires")
                || message.contains("Unknown dataset")
                || message.contains("must be")
                || message.contains("cannot be")
                || message.contains("missing key_field")
            {
                ApiErrorCode::Validation
            } else {
                ApiErrorCode::Internal
            };
            Json(ApiResponse::err_coded(code, message))
        }
    }
}

fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for value in title.to_lowercase().chars() {
        if value.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            slug.push(value);
        } else {
            separator = true;
        }
    }
    if slug.is_empty() {
        format!("page-{}", &Uuid::new_v4().simple().to_string()[..8])
    } else {
        slug
    }
}

fn normalize_page_title(title: &str) -> Result<String, &'static str> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        Err("Page title must contain 1-200 characters")
    } else {
        Ok(title.to_string())
    }
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 100
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--")
        && slug
            .chars()
            .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit() || value == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_is_stable_and_url_safe() {
        assert_eq!(
            slugify("Adobe — Tests & Indicateurs"),
            "adobe-tests-indicateurs"
        );
        assert!(valid_slug("adobe-tests-indicateurs"));
        assert!(!valid_slug("Adobe--tests"));
    }

    #[test]
    fn page_title_is_trimmed_and_limited_to_two_hundred_characters() {
        assert_eq!(
            normalize_page_title("  Production health  ").unwrap(),
            "Production health"
        );
        assert!(normalize_page_title("   ").is_err());
        assert!(normalize_page_title(&"é".repeat(200)).is_ok());
        assert!(normalize_page_title(&"é".repeat(201)).is_err());
    }
}
