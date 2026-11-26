use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use crate::models::{
    AddPlanningBlockerRequest, ApiErrorCode, ApiResponse, AssignPlanningTaskRequest,
    CreatePlanningTaskRequest, DiscussionPlan, LinkPlanningDiscussionRequest,
    PlanningDiscussionRelation, PlanningPlacement, PlanningTaskChange, PlanningTaskDetail,
    PlanningTaskListQuery, PlanningTaskListResponse, PlanningTaskStatus,
    RemovePlanningBlockerRequest, UnassignPlanningTaskRequest, UnlinkPlanningDiscussionRequest,
    UpdatePlanningDodItemRequest, UpdatePlanningTaskRequest,
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
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Query(mut query): Query<PlanningTaskListQuery>,
) -> Json<ApiResponse<PlanningTaskListResponse>> {
    // A Kronn-launched agent pages through its project's tasks, filtered in
    // the query rather than after the page is cut.
    if let Some(axum::Extension(caller)) = bridge {
        if query.project_id.is_none() {
            query.project_id = caller.project;
        }
    }
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

/// `?view=compact` asks for the projection an agent reads. The UI sends no
/// `view` and keeps the full typed answer, byte for byte.
#[derive(Debug, Default, serde::Deserialize)]
pub struct PlanningViewQuery {
    #[serde(default)]
    pub view: Option<String>,
}

impl PlanningViewQuery {
    fn is_compact(&self) -> bool {
        self.view.as_deref() == Some("compact")
    }
}

/// The same response around another projection of its data.
fn with_data<T: serde::Serialize, U: serde::Serialize>(
    response: ApiResponse<T>,
    project: impl FnOnce(&T) -> U,
) -> ApiResponse<U> {
    ApiResponse {
        success: response.success,
        data: response.data.as_ref().map(project),
        error: response.error,
        error_code: response.error_code,
        notice: response.notice,
        readiness: response.readiness,
    }
}

/// `GET /api/planning/tasks/{id}`: the full task, or with `?view=compact` the
/// one `task_get` answers by default (KT-1984).
pub async fn read_task(
    state: State<AppState>,
    path: Path<String>,
    Query(query): Query<PlanningViewQuery>,
) -> axum::response::Response {
    let Json(response) = get_task(state, path).await;
    if query.is_compact() {
        Json(with_data(response, task_compact_view)).into_response()
    } else {
        Json(response).into_response()
    }
}

/// Events `task_get` lists by default, newest first: the current round of
/// work, not a history that grows with every edit.
pub const TASK_EVENTS_BRIEF_CAP: usize = 10;

/// KT-1984 — a task as an agent reads it: everything but the bodies of its
/// history. An `updated` event carries the whole previous description, so the
/// answer grew with each edit; the event keeps who, when and which fields.
pub fn task_compact_view(task: &PlanningTaskDetail) -> serde_json::Value {
    let mut view = match serde_json::to_value(task) {
        Ok(view) => view,
        Err(error) => return serde_json::json!({ "serialization_error": error.to_string() }),
    };
    let events: Vec<serde_json::Value> = task
        .events
        .iter()
        .take(TASK_EVENTS_BRIEF_CAP)
        .map(|event| {
            let mut brief = serde_json::json!({
                "id": event.id,
                "action": event.action,
                "actor_kind": event.actor_kind,
                "actor_id": event.actor_id,
                "created_at": event.created_at,
            });
            if event.action == "updated" {
                if let Some(fields) = event.changes.as_object() {
                    brief["changed"] = fields.keys().cloned().collect();
                }
            }
            brief
        })
        .collect();
    let older = task.events.len() - events.len();
    view["events"] = serde_json::Value::Array(events);
    view["events_omitted"] = serde_json::json!(older);
    // Named explicitly: silence about a dropped field reads as "there was none".
    view["omitted"] = serde_json::json!(if older == 0 {
        "the `changes` body of each event — task_get with full_events: true returns them"
            .to_string()
    } else {
        format!(
            "the `changes` body of each event and the {older} oldest events — task_get with \
             full_events: true returns the whole history"
        )
    });
    view
}

pub async fn update_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdatePlanningTaskRequest>,
) -> Json<ApiResponse<PlanningTaskDetail>> {
    let closes = matches!(
        request.status,
        Some(crate::models::PlanningTaskStatus::Done | crate::models::PlanningTaskStatus::Archived)
    );
    match state
        .db
        .with_conn(move |connection| crate::db::planning::update_task(connection, &id, &request))
        .await
    {
        Ok(task) => {
            if closes {
                // KT-1676 — a task closed by hand never made its execution
                // terminal, so nothing released its worktrees. Git runs off the
                // request: the response does not wait for a slow disk.
                let db = state.db.clone();
                let task_id = task.summary.id.clone();
                tokio::spawn(async move {
                    crate::api::orchestration::release_closed_task_checkouts(&db, &task_id).await;
                });
            }
            Json(ApiResponse::ok(task))
        }
        Err(error) => planning_error(error),
    }
}

pub async fn update_dod_item(
    state: State<AppState>,
    path: Path<(String, String)>,
    Json(request): Json<UpdatePlanningDodItemRequest>,
) -> Json<ApiResponse<PlanningTaskDetail>> {
    update_dod_item_as(
        state,
        None,
        path,
        Json(UpdateDodItemBody {
            request,
            source_agent: String::new(),
            source_session_id: String::new(),
            room_agent: None,
        }),
    )
    .await
}

/// A DoD write with the principal identity the bridge derives, the same fields
/// the execution tools send (KT-1954). Never part of a tool's input schema.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateDodItemBody {
    #[serde(flatten)]
    pub request: UpdatePlanningDodItemRequest,
    #[serde(default)]
    pub source_agent: String,
    #[serde(default)]
    pub source_session_id: String,
    /// Read leniently: a malformed identity proves nothing, it does not fail
    /// the write of an ordinary item.
    #[serde(default)]
    pub room_agent: Option<serde_json::Value>,
}

pub async fn update_dod_item_as(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Path((task_id, dod_id)): Path<(String, String)>,
    Json(body): Json<UpdateDodItemBody>,
) -> Json<ApiResponse<PlanningTaskDetail>> {
    let bridge = bridge.map(|axum::Extension(caller)| caller);
    match state
        .db
        .with_conn(move |connection| {
            let UpdateDodItemBody {
                mut request,
                source_agent,
                source_session_id,
                room_agent,
            } = body;
            // Only a live check asks who the principal is: an ordinary item
            // never depends on the state of the task's executions. Proven in
            // the same connection turn as the write it authorises; a proof
            // that cannot be read is no proof.
            let principal = if crate::db::planning::dod_item_is_live_check(connection, &dod_id)? {
                crate::api::orchestration::live_check_principal(
                    connection,
                    &task_id,
                    &source_agent,
                    &source_session_id,
                    room_agent.and_then(|room| serde_json::from_value(room).ok()),
                    bridge.as_ref(),
                )
                .ok()
                .flatten()
            } else {
                None
            };
            // The journal names the identity that was proven, whole.
            if let Some((agent, session)) = &principal {
                request.actor.id = Some(agent.clone());
                request.actor.session_id = Some(session.clone());
            }
            crate::db::planning::update_dod_item_as(
                connection,
                &task_id,
                &dod_id,
                &request,
                principal.is_some(),
            )
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

/// KT-594 — `DELETE /api/planning/tasks/{id}/discussions`
///
/// The counterpart of the POST above. A task could be added to a plan and then
/// only moved between "active" and "later", never taken out.
pub async fn unlink_discussion(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UnlinkPlanningDiscussionRequest>,
) -> Json<ApiResponse<DiscussionPlan>> {
    match state
        .db
        .with_conn(move |connection| {
            crate::db::planning::unlink_discussion(connection, &id, &request)
        })
        .await
    {
        Ok(plan) => Json(ApiResponse::ok(plan)),
        Err(error) => planning_error(error),
    }
}

/// KT-154 — `POST /api/planning/tasks/{id}/assignees`: put one agent on a task.
pub async fn assign_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<AssignPlanningTaskRequest>,
) -> Json<ApiResponse<PlanningTaskDetail>> {
    match state
        .db
        .with_conn(move |connection| crate::db::planning::assign_task(connection, &id, &request))
        .await
    {
        Ok(task) => Json(ApiResponse::ok(task)),
        Err(error) => planning_error(error),
    }
}

/// KT-154 — `DELETE /api/planning/tasks/{id}/assignees`: take one agent off a
/// task (identified by its typed identity, in the body like the discussion unlink).
pub async fn unassign_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UnassignPlanningTaskRequest>,
) -> Json<ApiResponse<PlanningTaskDetail>> {
    match state
        .db
        .with_conn(move |connection| crate::db::planning::unassign_task(connection, &id, &request))
        .await
    {
        Ok(task) => Json(ApiResponse::ok(task)),
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

pub async fn remove_blocker(
    State(state): State<AppState>,
    Path((id, blocker_id)): Path<(String, String)>,
    Json(request): Json<RemovePlanningBlockerRequest>,
) -> Json<ApiResponse<PlanningTaskDetail>> {
    match state
        .db
        .with_conn(move |connection| {
            crate::db::planning::remove_blocker(connection, &id, &blocker_id, &request.actor)
        })
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

/// `GET /api/discussions/{id}/plan`: the full plan, or with `?view=compact`
/// the one `plan_get` answers (KT-2110).
pub async fn read_discussion_plan(
    state: State<AppState>,
    path: Path<String>,
    Query(query): Query<PlanningViewQuery>,
) -> axum::response::Response {
    let Json(response) = get_discussion_plan(state, path).await;
    if query.is_compact() {
        Json(with_data(response, plan_compact_view)).into_response()
    } else {
        Json(response).into_response()
    }
}

/// What `plan_get` may weigh, in bytes of compact JSON: about 3 000 tokens.
/// Every agent reads the plan before acting, a small local model included.
pub const PLAN_COMPACT_MAX_BYTES: usize = 12_000;

/// Blockers a row names by reference; beyond, the count alone.
const PLAN_ROW_BLOCKER_REFS: usize = 3;

/// KT-2110 — the plan as an agent reads it: one compact row per task, the
/// most urgent first, within [`PLAN_COMPACT_MAX_BYTES`]. A room of 151 tasks
/// answered 197 KB; what does not fit is counted and `task_list` lists it.
pub fn plan_compact_view(plan: &DiscussionPlan) -> serde_json::Value {
    plan_compact_view_within(plan, PLAN_COMPACT_MAX_BYTES)
}

fn plan_compact_view_within(plan: &DiscussionPlan, max_bytes: usize) -> serde_json::Value {
    let mut relations: Vec<&PlanningDiscussionRelation> =
        plan.active.iter().chain(plan.later.iter()).collect();
    // Stable: the plan's own order is kept inside each rank.
    relations.sort_by_key(|relation| plan_row_rank(relation));
    let mut rows: Vec<serde_json::Value> = relations.iter().copied().map(plan_row).collect();

    let whole = plan_envelope(plan, rows.clone(), &[]);
    if whole.to_string().len() <= max_bytes {
        return whole;
    }
    // The envelope at its widest (every count at full width), so the rows
    // added below can only bring the answer under the budget, never over it.
    let mut widest = plan_envelope(plan, Vec::new(), &relations);
    widest["shown"] = relations.len().into();
    let mut used = widest.to_string().len();
    let mut shown = 0;
    for row in &rows {
        // One more byte for the comma that separates two rows.
        let size = row.to_string().len() + 1;
        if used + size > max_bytes {
            break;
        }
        used += size;
        shown += 1;
    }
    rows.truncate(shown);
    plan_envelope(plan, rows, &relations[shown..])
}

/// What an agent must see first: the objective, work under way, what can be
/// picked up, what waits, ideas; then later and finished work.
fn plan_row_rank(relation: &PlanningDiscussionRelation) -> u8 {
    let task = &relation.task;
    if relation.is_primary {
        return 0;
    }
    match task.status {
        PlanningTaskStatus::Done => return 7,
        // Before the placement: a started task placed "later" is still under way.
        PlanningTaskStatus::InProgress => return 1,
        _ => {}
    }
    if relation.placement == PlanningPlacement::Later {
        return 6;
    }
    match task.status {
        _ if relation.actionable => 2,
        PlanningTaskStatus::Blocked => 3,
        _ if task.blocker_count > 0 => 3,
        PlanningTaskStatus::Idea => 4,
        _ => 5,
    }
}

/// One task of the plan, by its reference: every write tool accepts it, so
/// the row carries no UUID, date, rank, project, discussion or tag.
fn plan_row(relation: &PlanningDiscussionRelation) -> serde_json::Value {
    let task = &relation.task;
    let mut row = serde_json::json!({
        "reference": task.reference,
        "title": task.title,
        "status": task.status,
        "priority": task.priority,
        "actionable": relation.actionable,
    });
    if relation.is_primary {
        row["is_primary"] = true.into();
    }
    if relation.placement == PlanningPlacement::Later {
        row["placement"] = "later".into();
    }
    if task.blocker_count > 0 {
        row["blocker_count"] = task.blocker_count.into();
        let blockers = &relation.active_blockers;
        if !blockers.is_empty() && blockers.len() <= PLAN_ROW_BLOCKER_REFS {
            row["blockers"] = blockers
                .iter()
                .map(|blocker| blocker.reference.clone())
                .collect();
        }
    }
    if !task.assignees.is_empty() {
        row["assignees"] = task
            .assignees
            .iter()
            .map(|assignee| serde_json::json!(assignee.agent_type))
            .collect();
    }
    if let Some(merge) = &task.awaiting_merge {
        row["awaiting_merge"] = merge.branch.clone().into();
    }
    if !task.live_checks.is_empty() {
        row["live_checks"] = task.live_checks.len().into();
    }
    row
}

/// The answer around its rows. A plan that leaves rows out says so: how many
/// per status, and the call that lists them.
fn plan_envelope(
    plan: &DiscussionPlan,
    shown: Vec<serde_json::Value>,
    left_out: &[&PlanningDiscussionRelation],
) -> serde_json::Value {
    let mut view = serde_json::json!({
        "discussion_id": plan.discussion_id,
        "primary_objective": plan.primary_objective.as_ref().map(|task| task.reference.as_str()),
        "stats": plan.stats,
        "total_active": plan.total_active,
        "completed_active": plan.completed_active,
        "row_defaults": "a row omits is_primary when false, placement when active, \
                         blocker_count when 0, assignees when none",
        "total": shown.len() + left_out.len(),
        "shown": shown.len(),
        "complete": left_out.is_empty(),
        "tasks": shown,
    });
    if !left_out.is_empty() {
        let mut by_status = serde_json::Map::new();
        for relation in left_out {
            let count = by_status
                .entry(relation.task.status.as_str())
                .or_insert(serde_json::Value::from(0u64));
            *count = serde_json::Value::from(count.as_u64().unwrap_or(0) + 1);
        }
        view["omitted"] = serde_json::json!({
            "count": left_out.len(),
            "by_status": by_status,
            "list_with": format!(
                "task_list({{discussion_id: \"{}\", status: \"<status>\", cursor: <next_cursor>}})",
                plan.discussion_id
            ),
        });
    }
    view
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
