//! Skills API — list, create, update, delete skills.

use axum::{
    extract::{Path, Query, State},
    Json,
};

use crate::core::skills;
use crate::models::*;
use crate::AppState;

/// GET /api/skills — list all skills (builtin + custom)
pub async fn list(_state: State<AppState>) -> Json<ApiResponse<Vec<Skill>>> {
    let all = skills::list_all_skills();
    Json(ApiResponse::ok(all))
}

#[derive(serde::Deserialize, Default)]
pub struct GuideQuery {
    #[serde(default)]
    pub section: Option<String>,
}

/// GET /api/skills/{id}/guide — a built-in skill read by section (KT-1341):
/// its contents first, then the sections an agent asks for.
pub async fn guide(
    Path(id): Path<String>,
    Query(query): Query<GuideQuery>,
) -> Json<ApiResponse<serde_json::Value>> {
    match crate::core::skill_guide::guide(&id, query.section.as_deref()) {
        Some(guide) => Json(ApiResponse::ok(guide)),
        None => Json(ApiResponse::err(format!("no built-in skill `{id}`"))),
    }
}

/// A skill may only name a project Kronn knows.
async fn check_project(
    state: &AppState,
    project_id: Option<&Option<String>>,
) -> Result<(), String> {
    let Some(Some(id)) = project_id else {
        return Ok(());
    };
    if id.is_empty() {
        return Ok(());
    }
    let id = id.clone();
    match state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err("Project not found".into()),
        Err(error) => Err(format!("Cannot read the project: {error}")),
    }
}

type Bridge = Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>;

/// `allowed-tools` pre-approves tools in the agents' native skill files: a
/// bridge token never sets or changes it, only keeps what a human wrote.
const ALLOWED_TOOLS_ARE_HUMAN: &str =
    "a skill's allowed_tools pre-approve agent tools: only a human sets or changes them";

fn tools_value(tools: Option<&str>) -> Option<&str> {
    tools.map(str::trim).filter(|tools| !tools.is_empty())
}

/// Forget the rooms a skill id was created from: a slug can be reused once
/// its skill is gone, and must not hand the new skill to an old room.
async fn forget_origin(state: &AppState, id: &str) -> Result<(), String> {
    let id = id.to_owned();
    state
        .db
        .with_conn(move |conn| {
            crate::db::discussion_scope::forget_general_origin(conn, "skill", &id)
        })
        .await
        .map_err(|error| format!("Cannot record the skill's origin: {error}"))
}

/// POST /api/skills — create a custom skill
pub async fn create(
    State(state): State<AppState>,
    bridge: Bridge,
    marker: crate::core::bridge_token::AgentMark,
    Json(req): Json<CreateSkillRequest>,
) -> Json<ApiResponse<Skill>> {
    if crate::core::bridge_token::agent_authored(&bridge, &marker)
        && tools_value(req.allowed_tools.as_deref()).is_some()
    {
        return Json(ApiResponse::err(ALLOWED_TOOLS_ARE_HUMAN));
    }
    if let Err(error) = check_project(&state, req.project_id.as_ref()).await {
        return Json(ApiResponse::err(error));
    }
    match skills::save_custom_skill(
        &req.name,
        &req.description,
        &req.icon,
        &req.category,
        &req.content,
        req.license.as_deref(),
        req.allowed_tools.as_deref(),
        req.project_id.as_ref().and_then(|p| p.as_deref()),
    ) {
        Ok(id) => {
            if let Err(error) = forget_origin(&state, &id).await {
                return Json(ApiResponse::err(error));
            }
            match skills::get_skill(&id) {
                Some(skill) => Json(ApiResponse::ok(skill)),
                None => Json(ApiResponse::err("Skill created but could not be loaded")),
            }
        }
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// What an agent's update would change in the stored skill.
fn changes(existing: &Skill, req: &CreateSkillRequest) -> bool {
    let project = match &req.project_id {
        Some(next) => next.as_deref().filter(|p| !p.is_empty()),
        None => existing.project_id.as_deref(),
    };
    existing.name != req.name
        || existing.description != req.description
        || existing.icon != req.icon
        || existing.category != req.category
        || existing.content != req.content
        || tools_value(existing.license.as_deref()) != tools_value(req.license.as_deref())
        || existing.project_id.as_deref() != project
}

/// PUT /api/skills/:id — update a custom skill in place. The id never
/// changes, even when the skill is renamed.
pub async fn update(
    Path(id): Path<String>,
    State(state): State<AppState>,
    bridge: Bridge,
    marker: crate::core::bridge_token::AgentMark,
    Json(mut req): Json<CreateSkillRequest>,
) -> Json<ApiResponse<Skill>> {
    if !id.starts_with("custom-") {
        return Json(ApiResponse::err("Cannot modify builtin skills"));
    }
    if let Err(error) = check_project(&state, req.project_id.as_ref()).await {
        return Json(ApiResponse::err(error));
    }
    if crate::core::bridge_token::agent_authored(&bridge, &marker) {
        let Some(existing) = skills::get_skill(&id) else {
            return Json(ApiResponse::err(format!("Skill '{id}' not found")));
        };
        let stored = tools_value(existing.allowed_tools.as_deref());
        match tools_value(req.allowed_tools.as_deref()) {
            None if req.allowed_tools.is_none() => {
                req.allowed_tools = stored.map(str::to_owned);
            }
            sent if sent == stored => {}
            _ => return Json(ApiResponse::err(ALLOWED_TOOLS_ARE_HUMAN)),
        }
        // Like a Quick Prompt (KT-1037): a workflow loading this skill
        // unpinned would run the agent's edit under a human's activation.
        if changes(&existing, &req) {
            let agent = crate::api::workflows::agent_label(&state, &bridge, &marker)
                .await
                .unwrap_or_default();
            let summary = format!("Skill « {} » edited by {agent}", existing.name);
            let skill_id = id.clone();
            if let Err(error) = state
                .db
                .with_conn(move |conn| {
                    crate::core::resource_refs::disable_workflows_loading_skill(
                        conn, &skill_id, &agent, &summary,
                    )
                    .map(|_| ())
                })
                .await
            {
                return Json(ApiResponse::err(format!(
                    "Cannot disable the workflows using this skill: {error}"
                )));
            }
        }
    }

    match skills::update_custom_skill(
        &id,
        &req.name,
        &req.description,
        &req.icon,
        &req.category,
        &req.content,
        req.license.as_deref(),
        req.allowed_tools.as_deref(),
        req.project_id.as_ref().map(|p| p.as_deref()),
    ) {
        Ok(updated_id) => match skills::get_skill(&updated_id) {
            Some(skill) => Json(ApiResponse::ok(skill)),
            None => Json(ApiResponse::err("Skill updated but could not be loaded")),
        },
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/skills/:id/history — every version of a custom skill, newest
/// first (KT-1434). Builtin skills have none.
pub async fn history(Path(id): Path<String>) -> Json<ApiResponse<Vec<SkillVersion>>> {
    match crate::core::skill_versions::history(&id) {
        Ok(versions) => Json(ApiResponse::ok(versions)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/skills/:id/versions/:version — one version of a custom skill.
pub async fn version(Path((id, index)): Path<(String, u32)>) -> Json<ApiResponse<SkillVersion>> {
    match crate::core::skill_versions::version(&id, index) {
        Ok(version) => Json(ApiResponse::ok(version)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// POST /api/skills/:id/versions/:version/restore — that version becomes the
/// skill's content again, recorded as a new version. Human only.
pub async fn restore(Path((id, index)): Path<(String, u32)>) -> Json<ApiResponse<SkillVersion>> {
    match crate::core::skill_versions::restore(&id, index) {
        Ok(version) => Json(ApiResponse::ok(version)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// DELETE /api/skills/:id — delete a custom skill
pub async fn delete(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Json<ApiResponse<bool>> {
    match skills::delete_custom_skill(&id) {
        Ok(deleted) => match forget_origin(&state, &id).await {
            Ok(()) => Json(ApiResponse::ok(deleted)),
            Err(error) => Json(ApiResponse::err(error)),
        },
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/skills/auto-triggers/disabled — returns the list of skill
/// IDs for which auto-activation (keyword-based) is OFF. The frontend
/// fetches this once on mount and filters its local
/// `detectTriggeredSkills()` output against it.
pub async fn list_disabled_auto(state: State<AppState>) -> Json<ApiResponse<Vec<String>>> {
    let ids = state.config.read().await.disabled_auto_skills.clone();
    Json(ApiResponse::ok(ids))
}

/// POST /api/skills/:id/auto-trigger/toggle — flips the opt-out state
/// for one skill. Returns the new `disabled` boolean (true = auto-
/// activation is OFF for this skill, false = default/enabled).
/// Idempotent: sending twice ends up where it started. Also persists
/// to config.toml so the choice survives restarts.
pub async fn toggle_auto_trigger(
    Path(id): Path<String>,
    state: State<AppState>,
) -> Json<ApiResponse<bool>> {
    let mut cfg = state.config.write().await;
    let pos = cfg.disabled_auto_skills.iter().position(|s| s == &id);
    let now_disabled = match pos {
        Some(i) => {
            cfg.disabled_auto_skills.remove(i);
            false
        }
        None => {
            cfg.disabled_auto_skills.push(id.clone());
            true
        }
    };
    if let Err(e) = crate::core::config::save(&cfg).await {
        tracing::warn!("Failed to persist disabled_auto_skills toggle: {e}");
    }
    Json(ApiResponse::ok(now_disabled))
}
