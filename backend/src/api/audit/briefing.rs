// Pre-audit briefing: a conversational discussion that asks the user
// 6 product/team questions and writes the answers to `docs/briefing.md`.
// The audit later reads `docs/briefing.md` (via
// `projects::resolve_briefing_notes`) and injects the user's answers
// into every step prompt.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

use super::helpers::build_briefing_prompt;

/// GET /api/projects/:id/briefing
/// Returns the briefing notes for a project.
pub async fn get_briefing(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Option<String>>> {
    match state.db.with_conn(move |conn| {
        crate::db::projects::get_project_briefing_notes(conn, &id)
    }).await {
        Ok(notes) => Json(ApiResponse::ok(notes)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/projects/:id/briefing
/// Sets or clears the briefing notes for a project.
pub async fn set_briefing(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SetBriefingRequest>,
) -> Json<ApiResponse<bool>> {
    let notes = req.notes;
    match state.db.with_conn(move |conn| {
        crate::db::projects::update_project_briefing_notes(conn, &id, notes.as_deref())
    }).await {
        Ok(true) => Json(ApiResponse::ok(true)),
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/projects/:id/start-briefing
/// Creates a conversational briefing discussion for a project.
pub async fn start_briefing(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LaunchAuditRequest>,
) -> Json<ApiResponse<StartBriefingResponse>> {
    // 1. Look up the project
    let pid = id.clone();
    let project = state.db.with_conn(move |conn| {
        crate::db::projects::get_project(conn, &pid)
    }).await.ok().flatten();

    let Some(project) = project else {
        return Json(ApiResponse::err("Project not found"));
    };

    // 2. Get language
    let language = {
        let config = state.config.read().await;
        config.language.clone()
    };

    // 3. Build briefing prompt. If the user already submitted the form
    //    (`docs/briefing.md` exists with content OR DB notes are set),
    //    switch to the SHORT review prompt — the agent reads back the
    //    answers, asks at most 2-3 targeted clarifications, then
    //    finalizes. Without this guard the agent re-asks the 6 questions
    //    the user just answered (the original UX confusion).
    let project_path = crate::core::scanner::resolve_host_path(&project.path);
    let prefilled = crate::api::projects::resolve_briefing_notes(
        &project_path,
        &project.briefing_notes,
    );
    let briefing_prompt = build_briefing_prompt(&language, prefilled.as_deref());

    // 4. Create discussion
    let now = Utc::now();
    let discussion_id = Uuid::new_v4().to_string();
    let agent_type = req.agent;

    let initial_message = DiscussionMessage {
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: briefing_prompt,
        agent_type: None,
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None, source_msg_id: None,
    };

    let title = match language.as_str() {
        "en" => "Project Briefing".to_string(),
        "es" => "Briefing del proyecto".to_string(),
        _ => "Briefing projet".to_string(),
    };

    let discussion = Discussion {
        id: discussion_id.clone(),
        project_id: Some(project.id.clone()),
        title,
        agent: agent_type.clone(),
        language: language.clone(),
        participants: vec![agent_type],
        messages: vec![initial_message.clone()],
        message_count: 1,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
            pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        tier: crate::models::ModelTier::Default,
        pin_first_message: true,
        worktree_branch: None,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: crate::models::SummaryStrategy::Auto,
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };

    let disc = discussion.clone();
    let msg = initial_message;
    if let Err(e) = state.db.with_conn(move |conn| {
        crate::db::discussions::insert_discussion(conn, &disc)?;
        crate::db::discussions::insert_message(conn, &disc.id, &msg)?;
        Ok(())
    }).await {
        return Json(ApiResponse::err(format!("Failed to create discussion: {}", e)));
    }

    Json(ApiResponse::ok(StartBriefingResponse { discussion_id }))
}

/// 0.8.4 (#285) — désagentified briefing : 6 réponses utilisateur →
/// `docs/briefing.md` + `projects.briefing_notes` directement, sans
/// passer par une discussion LLM. Token cost = 0, instantané, et le
/// user peut éditer son briefing sans avoir à relancer une conv.
///
/// Le format markdown produit ici est BYTE-FOR-BYTE compatible avec
/// celui généré par l'ancien flow conversationnel — l'audit consomme
/// le même fichier via `projects::resolve_briefing_notes` (audit/full.rs
/// Phase 1 + chaque step prompt). Aucun changement nécessaire en aval.
#[derive(Debug, serde::Deserialize)]
pub struct BriefingFormRequest {
    pub purpose: String,
    pub team: String,
    pub maturity: String,
    pub dependencies: String,
    pub traps: String,
    pub additional: String,
}

pub async fn save_briefing_form(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(form): Json<BriefingFormRequest>,
) -> Json<ApiResponse<bool>> {
    // 1. Validate: Q1-Q5 are mandatory (matches the conversational flow).
    if form.purpose.trim().is_empty()
        || form.team.trim().is_empty()
        || form.maturity.trim().is_empty()
        || form.dependencies.trim().is_empty()
        || form.traps.trim().is_empty()
    {
        return Json(ApiResponse::err(
            "Q1-Q5 are mandatory (purpose, team, maturity, dependencies, traps). \
             Q6 (additional) is optional. Use \"Not provided\" or \"None\" if you \
             want to skip a mandatory question explicitly.",
        ));
    }

    // 2. Look up project for the on-disk path.
    let pid = id.clone();
    let project = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
        .ok()
        .flatten();
    let Some(project) = project else {
        return Json(ApiResponse::err("Project not found"));
    };

    // 3. Format the briefing.md body — same shape as
    // `build_briefing_prompt`'s STEP 2 instructions (EN spelling
    // because that's the canonical format the audit reads).
    let trap_lines = form.traps.trim();
    let traps_rendered = if trap_lines.is_empty() {
        "Not provided".to_string()
    } else {
        trap_lines.to_string()
    };
    let additional_rendered = if form.additional.trim().is_empty() {
        "None.".to_string()
    } else {
        form.additional.trim().to_string()
    };

    let body = format!(
        "# Project Briefing\n\
         > Auto-generated by Kronn briefing form (no AI call). Source: user answers.\n\n\
         ## Purpose\n{}\n\n\
         ## Team\n{}\n\n\
         ## Maturity\n{}\n\n\
         ## External Dependencies\n{}\n\n\
         ## Traps & Fragile Areas\n{}\n\n\
         ## Additional Context\n{}\n",
        form.purpose.trim(),
        form.team.trim(),
        form.maturity.trim(),
        form.dependencies.trim(),
        traps_rendered,
        additional_rendered,
    );

    // 4. Write `docs/briefing.md` on disk + persist briefing_notes in DB.
    // The on-disk file is what the audit reads at Phase 1; the DB
    // column is what the ProjectCard / API surfaces back to the UI.
    let project_path = crate::core::scanner::resolve_host_path(&project.path);
    let docs_dir = crate::core::scanner::detect_docs_dir(&project_path);
    if !docs_dir.is_dir() {
        // Pre-bootstrap projects don't have docs/ yet — store in DB
        // only; the audit's Phase 1 install will pick it up + emit
        // the file via the existing `resolve_briefing_notes` path.
        let notes = body.clone();
        match state
            .db
            .with_conn(move |conn| crate::db::projects::update_project_briefing_notes(conn, &id, Some(&notes)))
            .await
        {
            Ok(true) => return Json(ApiResponse::ok(true)),
            Ok(false) => return Json(ApiResponse::err("Project not found")),
            Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
        }
    }
    let target = docs_dir.join("briefing.md");
    if let Err(e) = std::fs::write(&target, &body) {
        return Json(ApiResponse::err(format!("Failed to write {}: {}", target.display(), e)));
    }
    let notes = body.clone();
    match state
        .db
        .with_conn(move |conn| crate::db::projects::update_project_briefing_notes(conn, &id, Some(&notes)))
        .await
    {
        Ok(true) => Json(ApiResponse::ok(true)),
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}
