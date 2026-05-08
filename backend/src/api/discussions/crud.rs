// CRUD endpoints for the `Discussion` resource: list / get / create
// (with optional Isolated worktree spin-up) / update (title, archive,
// pin, skill/profile/directive bindings, project move, tier change,
// agent switch with system message) / delete (with worktree cleanup) /
// share (with peer broadcast) / delete-last / edit-last.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

use super::{MAX_CONTENT_LEN, MAX_TITLE_LEN};

pub async fn list(
    State(state): State<AppState>,
    Query(pq): Query<PaginationQuery>,
) -> Json<ApiResponse<Vec<Discussion>>> {
    // page > 0 → paginated response; page == 0 (default) → return all
    // (backward compat for frontend polling). See PaginationQuery doc.
    if pq.page > 0 {
        let page = pq.page;
        let per_page = pq.per_page.min(200);
        let offset = (page - 1) * per_page;
        return match state.db.with_conn(move |conn| {
            crate::db::discussions::list_discussions_paginated(conn, Some(per_page), Some(offset))
        }).await {
            Ok(discussions) => Json(ApiResponse::ok(discussions)),
            Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
        };
    }
    match state.db.with_conn(crate::db::discussions::list_discussions).await {
        Ok(discussions) => Json(ApiResponse::ok(discussions)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// GET /api/discussions/:id
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Discussion>> {
    match state.db.with_conn(move |conn| crate::db::discussions::get_discussion(conn, &id)).await {
        Ok(Some(d)) => Json(ApiResponse::ok(d)),
        Ok(None) => Json(ApiResponse::err("Discussion not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/discussions
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateDiscussionRequest>,
) -> Json<ApiResponse<Discussion>> {
    // Input validation
    if req.title.len() > MAX_TITLE_LEN {
        return Json(ApiResponse::err(format!("Title too long ({} chars, max {})", req.title.len(), MAX_TITLE_LEN)));
    }
    if req.initial_prompt.len() > MAX_CONTENT_LEN {
        return Json(ApiResponse::err(format!("Prompt too long ({} bytes, max {})", req.initial_prompt.len(), MAX_CONTENT_LEN)));
    }

    // Reject conflicting directives (eco: avoids injecting contradictory instructions that waste tokens)
    if !req.directive_ids.is_empty() {
        let conflicts = crate::core::directives::validate_no_conflicts(&req.directive_ids);
        if !conflicts.is_empty() {
            let pairs: Vec<String> = conflicts.iter().map(|(a, b)| format!("{} <> {}", a, b)).collect();
            return Json(ApiResponse::err(format!("Conflicting directives: {}", pairs.join(", "))));
        }
    }

    // Validate project exists (if specified)
    if let Some(ref pid) = req.project_id {
        let pid = pid.clone();
        let project_exists = state.db.with_conn({
            move |conn| {
                let p = crate::db::projects::get_project(conn, &pid)?;
                Ok(p.is_some())
            }
        }).await.unwrap_or(false);

        if !project_exists {
            return Json(ApiResponse::err("Project not found"));
        }
    }

    let language = if req.language.is_empty() {
        let config = state.config.read().await;
        config.language.clone()
    } else {
        req.language
    };

    // Read user identity for first message attribution
    let (author_pseudo, author_avatar_email) = {
        let config = state.config.read().await;
        (config.server.pseudo.clone(), config.server.avatar_email.clone())
    };

    let now = Utc::now();
    let initial_message = DiscussionMessage {
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: req.initial_prompt,
        agent_type: None,
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None, cost_usd: None, author_pseudo, author_avatar_email,
    };

    let workspace_mode = req.workspace_mode.unwrap_or_else(|| "Direct".into());
    let base_branch = req.base_branch;

    let discussion = Discussion {
        id: Uuid::new_v4().to_string(),
        project_id: req.project_id,
        title: req.title,
        agent: req.agent.clone(),
        language,
        participants: vec![req.agent.clone()],
        messages: vec![initial_message.clone()],
        message_count: 1,
        skill_ids: req.skill_ids,
        profile_ids: req.profile_ids,
        directive_ids: req.directive_ids,
        tier: req.tier,
        pin_first_message: false,
        archived: false,
            pinned: false,
        workspace_mode: workspace_mode.clone(),
        workspace_path: None,
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
    match state.db.with_conn(move |conn| {
        crate::db::discussions::insert_discussion(conn, &disc)?;
        crate::db::discussions::insert_message(conn, &disc.id, &msg)?;
        Ok(())
    }).await {
        Ok(()) => {
            // If workspace_mode is "Isolated", create a worktree
            if workspace_mode == "Isolated" {
                if let Some(ref pid) = discussion.project_id {
                    let pid = pid.clone();
                    let project = state.db.with_conn(move |conn| {
                        crate::db::projects::get_project(conn, &pid)
                    }).await.ok().flatten();

                    if let Some(project) = project {
                        let resolved = crate::core::scanner::resolve_host_path(&project.path);
                        let repo_path = std::path::Path::new(&resolved);

                        let project_slug = &project.name;
                        let discussion_slug = &discussion.title;
                        let branch = base_branch.as_deref().unwrap_or("main");

                        match crate::core::worktree::create_discussion_worktree(
                            repo_path, project_slug, discussion_slug, branch,
                        ) {
                            Ok(info) => {
                                let disc_id = discussion.id.clone();
                                let wp = info.path.clone();
                                let wb = info.branch.clone();
                                if let Err(e) = state.db.with_conn(move |conn| {
                                    crate::db::discussions::update_discussion_workspace(conn, &disc_id, &wp, &wb)
                                }).await {
                                    tracing::error!("Failed to update discussion workspace: {e}");
                                }

                                // Return the updated discussion
                                let disc_id = discussion.id.clone();
                                if let Ok(Some(updated)) = state.db.with_conn(move |conn| {
                                    crate::db::discussions::get_discussion(conn, &disc_id)
                                }).await {
                                    return Json(ApiResponse::ok(updated));
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to create worktree for discussion {}: {}", discussion.id, e);
                                // Discussion is still created in Direct mode — don't fail the whole request
                            }
                        }
                    }
                }
            }

            Json(ApiResponse::ok(discussion))
        },
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PATCH /api/discussions/:id
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDiscussionRequest>,
) -> Json<ApiResponse<()>> {
    let title = req.title;
    let archived = req.archived;
    let pinned = req.pinned;
    let skill_ids = req.skill_ids;
    let profile_ids = req.profile_ids;
    let directive_ids = req.directive_ids;
    let project_id = req.project_id;
    let tier = req.tier;
    let new_agent = req.agent;
    let summary_strategy = req.summary_strategy;

    // Reject conflicting directives on update
    if let Some(ref ids) = directive_ids {
        if !ids.is_empty() {
            let conflicts = crate::core::directives::validate_no_conflicts(ids);
            if !conflicts.is_empty() {
                let pairs: Vec<String> = conflicts.iter().map(|(a, b)| format!("{} <> {}", a, b)).collect();
                return Json(ApiResponse::err(format!("Conflicting directives: {}", pairs.join(", "))));
            }
        }
    }

    // Agent switch: fetch old agent name for the system message
    let old_agent_name = if new_agent.is_some() {
        let did = id.clone();
        state.db.with_conn(move |conn| {
            crate::db::discussions::get_discussion(conn, &did)
        }).await.ok().flatten().map(|d| format!("{:?}", d.agent))
    } else {
        None
    };

    match state.db.with_conn(move |conn| {
        // project_id: None = don't change, Some(None)/Some(Some("")) = unset, Some(Some("id")) = set
        // Note: serde can't distinguish JSON null from absent for Option<Option<T>>,
        // so the frontend sends "" instead of null to mean "unset".
        let pid_update = project_id.as_ref().map(|p| {
            match p.as_deref() {
                Some("") | None => None,    // "" or null = unset project
                Some(id) => Some(id),       // real id = set project
            }
        });
        let mut updated = crate::db::discussions::update_discussion(conn, &id, title.as_deref(), archived, pinned, pid_update)?;
        if let Some(ref ids) = skill_ids {
            updated = crate::db::discussions::update_discussion_skill_ids(conn, &id, ids)? || updated;
        }
        if let Some(ref ids) = profile_ids {
            updated = crate::db::discussions::update_discussion_profile_ids(conn, &id, ids)? || updated;
        }
        if let Some(ref ids) = directive_ids {
            updated = crate::db::discussions::update_discussion_directive_ids(conn, &id, ids)? || updated;
        }
        if let Some(ref t) = tier {
            updated = crate::db::discussions::update_discussion_tier(conn, &id, t)? || updated;
        }
        if let Some(strategy) = summary_strategy {
            updated = crate::db::discussions::update_discussion_summary_strategy(conn, &id, strategy)? || updated;
        }
        if let Some(ref agent) = new_agent {
            updated = crate::db::discussions::update_discussion_agent(conn, &id, agent)? || updated;
            // Invalidate summary — new agent has different budget/context
            crate::db::discussions::invalidate_summary_cache(conn, &id)?;
            // Insert a User message so the new agent sees the switch context
            // (System messages are filtered from the agent prompt)
            let old_name = old_agent_name.as_deref().unwrap_or("?");
            let switch_msg = crate::models::DiscussionMessage {
                id: uuid::Uuid::new_v4().to_string(),
                role: crate::models::MessageRole::User,
                content: format!(
                    "[Agent switch: {} → {:?}] You are now the primary agent for this conversation. \
                    Briefly acknowledge the switch and summarize what has been discussed so far, \
                    then ask how you can help.",
                    old_name, agent
                ),
                agent_type: None,
                timestamp: chrono::Utc::now(),
                tokens_used: 0,
                auth_mode: None,
                model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None,
            };
            crate::db::discussions::insert_message(conn, &id, &switch_msg)?;
        }
        Ok(updated)
    }).await {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err("Discussion not found or no fields to update")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/discussions/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    // Fetch discussion to check for worktree before deleting
    let disc = state.db.with_conn({
        let did = id.clone();
        move |conn| crate::db::discussions::get_discussion(conn, &did)
    }).await.ok().flatten();

    // Clean up worktree if present
    if let Some(ref d) = disc {
        if let Some(ref wp) = d.workspace_path {
            if let Some(ref pid) = d.project_id {
                let pid = pid.clone();
                let project_path = state.db.with_conn(move |conn| {
                    let p = crate::db::projects::get_project(conn, &pid)?;
                    Ok(p.map(|p| p.path).unwrap_or_default())
                }).await.unwrap_or_default();

                if !project_path.is_empty() {
                    let resolved = crate::core::scanner::resolve_host_path(&project_path);
                    let repo_path = std::path::Path::new(&resolved);
                    if let Err(e) = crate::core::worktree::remove_discussion_worktree(repo_path, wp, true) {
                        tracing::warn!("Failed to remove worktree for discussion {}: {}", id, e);
                    }
                }
            }
        }
    }

    match state.db.with_conn(move |conn| crate::db::discussions::delete_discussion(conn, &id)).await {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err("Discussion not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/discussions/:id/share — share a discussion with contacts
pub async fn share(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ShareDiscussionRequest>,
) -> Json<ApiResponse<String>> {
    let disc_id = id.clone();
    let contact_ids = req.contact_ids.clone();

    // Get or create shared_id
    let result = state.db.with_conn(move |conn| {
        let disc = crate::db::discussions::get_discussion(conn, &disc_id)?
            .ok_or_else(|| anyhow::anyhow!("Discussion not found"))?;

        let shared_id = disc.shared_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let mut all_shared = disc.shared_with;
        for cid in &contact_ids {
            if !all_shared.contains(cid) {
                all_shared.push(cid.clone());
            }
        }
        crate::db::discussions::update_discussion_sharing(conn, &disc.id, &shared_id, &all_shared)?;
        Ok((shared_id, disc.title))
    }).await;

    match result {
        Ok((shared_id, title)) => {
            // Send DiscussionInvite to peers via WS broadcast
            let config = state.config.read().await;
            let pseudo = config.server.pseudo.clone().unwrap_or_default();
            let host = crate::api::contacts::advertised_host_async(&config.server).await;
            let port = config.server.port;
            drop(config);
            let invite_code = format!("kronn:{}@{}:{}", pseudo, host, port);

            let _ = state.ws_broadcast.send(WsMessage::DiscussionInvite {
                shared_discussion_id: shared_id.clone(),
                title,
                from_pseudo: pseudo,
                from_invite_code: invite_code,
            });

            Json(ApiResponse::ok(shared_id))
        }
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// DELETE /api/discussions/:id/messages/last
pub async fn delete_last_agent_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    let id_clone = id.clone();
    match state.db.with_conn(move |conn| crate::db::discussions::delete_last_agent_messages(conn, &id)).await {
        Ok(_) => {
            // Invalidate summary cache since messages were deleted
            if let Err(e) = state.db.with_conn(move |conn| crate::db::discussions::invalidate_summary_cache(conn, &id_clone)).await {
                tracing::error!("Failed to invalidate summary cache after delete: {e}");
            }
            Json(ApiResponse::ok(()))
        }
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PATCH /api/discussions/:id/messages/last
pub async fn edit_last_user_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> Json<ApiResponse<()>> {
    let content = req.content;
    match state.db.with_conn(move |conn| crate::db::discussions::edit_last_user_message(conn, &id, &content)).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}
