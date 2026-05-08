// `POST /api/projects/:id/full-audit` — the unified end-to-end pipeline:
// install template if needed → run the 10-step audit → create the
// validation discussion. Plus `POST /api/projects/:id/cancel-audit`
// which kills the running agent process and cleans the docs/ tree
// + redirector files so the project is back to a clean slate.

use std::convert::Infallible;

use axum::{
    extract::{Path, State},
    response::sse::{Event, Sse},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::agents::runner;
use crate::core::cmd::sync_cmd;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

use super::helpers::{
    check_ai_dir_permissions, compute_audit_info_sync, detect_issue_tracker_mcp,
    detect_project_skills, build_validation_prompt, remove_bootstrap_block,
};
use super::{SseStream, ANALYSIS_STEPS, AUDIT_REDIRECTOR_FILES, PROMPT_PREAMBLE};

/// POST /api/projects/:id/full-audit
/// Unified endpoint: install template + run 10-step audit + create validation discussion.
pub async fn full_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LaunchAuditRequest>,
) -> Sse<SseStream> {
    // Look up project
    let project = state.db.with_conn({
        let id = id.clone();
        move |conn| crate::db::projects::get_project(conn, &id)
    }).await.ok().flatten();

    if project.is_none() {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default().event("error").data("{\"error\":\"Project not found\"}")
            )
        }));
        return Sse::new(stream);
    }

    // Safety: early return above guarantees project is Some
    let project = project.expect("project is Some after early return");
    let project_id = project.id.clone();
    let project_path_str = project.path.clone();
    let project_path = scanner::resolve_host_path(&project.path);
    let project_default_skill_ids = project.default_skill_ids.clone();
    let briefing_notes = crate::api::projects::resolve_briefing_notes(&project_path, &project.briefing_notes);
    let agent_type = req.agent;

    let tokens = {
        let config = state.config.read().await;
        config.tokens.clone()
    };

    let language = {
        let config = state.config.read().await;
        config.language.clone()
    };

    let total_steps = ANALYSIS_STEPS.len();
    let db = state.db.clone();
    let audit_tracker = state.audit_tracker.clone();

    // Clear any stale cancellation flag for this project
    if let Ok(mut tracker) = audit_tracker.lock() {
        tracker.cancelled.remove(&project_id);
    }

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        // Seed live progress so GET /audit-status can report where we are
        // even when the SSE client (browser tab) went away.
        if let Ok(mut t) = audit_tracker.lock() {
            t.start_progress(&project_id, total_steps as u32, "full_audit");
            // Phase 1 starts here; advance_step will update to "auditing"
            // once the 10-step loop begins. The intermediate installing
            // phase is visible by checking step_index == 0.
            if let Some(entry) = t.progress.get_mut(&project_id) {
                entry.phase = "installing".into();
            }
        }

        // ── Phase 1: Install template if needed ──
        let status = scanner::detect_audit_status(&project_path_str);
        let template_installed = matches!(status, AiAuditStatus::NoTemplate);

        if template_installed {
            let pp = project_path_str.clone();
            let install_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
                let project_path = scanner::resolve_host_path(&pp);
                if !project_path.exists() {
                    return Err(format!("Project path not found: {}", project_path.display()));
                }

                // 0.7.1 — bootstrap to docs/ (or respect legacy ai/ when
                // present). Permission check runs on whichever the project
                // already uses, then template install lands at docs/ for
                // fresh projects (default).
                let docs_target = scanner::detect_docs_dir(&project_path);
                if docs_target.exists() {
                    if let Err(e) = check_ai_dir_permissions(&docs_target) {
                        return Err(format!("{}/ permission error: {}",
                            docs_target.file_name().and_then(|n| n.to_str()).unwrap_or("docs"), e));
                    }
                }

                let template_dir = crate::api::projects::resolve_templates_dir();
                if !template_dir.exists() {
                    return Err(format!("Templates directory not found: {}", template_dir.display()));
                }

                let docs_template = template_dir.join("docs");
                if docs_template.is_dir() {
                    crate::api::projects::copy_dir_nondestructive(&docs_template, &docs_target)?;
                }
                crate::api::projects::ensure_agent_writable_subfolders(&docs_target)?;

                for filename in &["CLAUDE.md", ".cursorrules", ".windsurfrules", ".clinerules"] {
                    let src = template_dir.join(filename);
                    let dst = project_path.join(filename);
                    if src.exists() && !dst.exists() {
                        if let Err(e) = std::fs::copy(&src, &dst) {
                            tracing::warn!("Failed to copy {}: {}", filename, e);
                        }
                    }
                }

                let index_file = project_path.join("docs/AGENTS.md");
                if index_file.exists() {
                    crate::api::projects::inject_bootstrap_prompt(&index_file);
                }

                runner::fix_file_ownership(&project_path);
                Ok(())
            }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

            if let Err(e) = install_result {
                // Install failed — drop progress so the UI stops polling with
                // a stale "installing" state.
                if let Ok(mut t) = audit_tracker.lock() { t.clear_progress(&project_id); }
                let err = serde_json::json!({ "error": e });
                yield Event::default().event("error").data(err.to_string());
                return;
            }

            crate::core::mcp_scanner::ensure_gitignore_public(&project_path_str, "docs/var/");
        }

        let tmpl_event = serde_json::json!({ "installed": template_installed });
        yield Event::default().event("template_installed").data(tmpl_event.to_string());

        // ── Phase 2: Run 10-step audit ──
        // Remove bootstrap prompt
        let index_file = project_path.join("docs/AGENTS.md");
        if index_file.exists() {
            remove_bootstrap_block(&index_file);
        }

        let start = serde_json::json!({ "total_steps": total_steps });
        yield Event::default().event("start").data(start.to_string());

        for (step_num, analysis_step) in ANALYSIS_STEPS.iter().enumerate() {
            // Check for cancellation before each step
            if audit_tracker.lock().map(|t| t.cancelled.contains(&project_id)).unwrap_or(false) {
                if let Ok(mut t) = audit_tracker.lock() { t.clear_progress(&project_id); }
                let cancelled = serde_json::json!({ "status": "cancelled" });
                yield Event::default().event("cancelled").data(cancelled.to_string());
                return;
            }

            let step = step_num + 1;
            let file_label = if analysis_step.target_file == "REVIEW" { "Final review" } else { analysis_step.target_file };

            if let Ok(mut t) = audit_tracker.lock() {
                t.advance_step(&project_id, step as u32, Some(file_label.to_string()));
            }

            let step_start = serde_json::json!({
                "step": step, "total": total_steps, "file": file_label
            });
            yield Event::default().event("step_start").data(step_start.to_string());

            let today = Utc::now().format("%Y-%m-%d").to_string();
            let today_compact = Utc::now().format("%Y%m%d").to_string();
            let mut full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, analysis_step.prompt)
                .replace("YYYYMMDD=today", &format!("YYYYMMDD={}", today_compact))
                .replace("today's date (YYYY-MM-DD)", &today)
                .replace("set to today's date", &format!("set to {}", today));

            if let Some(ref notes) = briefing_notes {
                full_prompt.push_str(&format!("\n\n## Project briefing (from the user)\n{}\n", notes));
            }

            match runner::start_agent_with_config(runner::AgentStartConfig {
                agent_type: &agent_type, project_path: &project_path_str, work_dir: None,
                prompt: &full_prompt, tokens: &tokens, full_access: true,
                skill_ids: &[], directive_ids: &[], profile_ids: &[],
                mcp_context_override: None,
                tier: crate::models::ModelTier::Reasoning, model_tiers: None, context_files_prompt: "",
                discussion_id: None,
            }).await {
                Ok(mut process) => {
                    // Register the child PID for cancellation
                    if let Some(pid) = process.child.id() {
                        if let Ok(mut tracker) = audit_tracker.lock() {
                            tracker.running_pids.insert(project_id.clone(), pid);
                        }
                    }

                    while let Some(line) = process.next_line().await {
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }
                    let status = process.child.wait().await;
                    process.fix_ownership();

                    // Unregister PID
                    if let Ok(mut tracker) = audit_tracker.lock() {
                        tracker.running_pids.remove(&project_id);
                    }

                    // Check if cancelled during this step
                    if audit_tracker.lock().map(|t| t.cancelled.contains(&project_id)).unwrap_or(false) {
                        if let Ok(mut t) = audit_tracker.lock() { t.clear_progress(&project_id); }
                        let cancelled = serde_json::json!({ "status": "cancelled" });
                        yield Event::default().event("cancelled").data(cancelled.to_string());
                        return;
                    }

                    let success = status.map(|s| s.success()).unwrap_or(false);

                    let step_done = serde_json::json!({
                        "step": step, "success": success, "file": file_label
                    });
                    yield Event::default().event("step_done").data(step_done.to_string());
                }
                Err(e) => {
                    tracing::error!("Audit step {} failed to start: {}", step, e);
                    let err = serde_json::json!({
                        "error": format!("Step {} ({}): {}", step, file_label, e),
                        "step": step
                    });
                    yield Event::default().event("step_error").data(err.to_string());
                }
            }
        }

        // ── Auto-detect project skills ──
        let detected_skill_ids = {
            let p = project_path.clone();
            tokio::task::spawn_blocking(move || detect_project_skills(&p))
                .await.unwrap_or_default()
        };
        let skill_ids_for_disc = if detected_skill_ids.is_empty() {
            project_default_skill_ids.clone()
        } else {
            // Save detected skills to DB
            let pid = project_id.clone();
            let sids = detected_skill_ids.clone();
            let _ = db.with_conn(move |conn| {
                crate::db::projects::update_project_default_skills(conn, &pid, &sids)
            }).await;
            detected_skill_ids
        };

        // ── Phase 3: Create validation discussion ──
        if let Ok(mut t) = audit_tracker.lock() { t.mark_validating(&project_id); }

        let pp = project_path_str.clone();
        let audit_info = tokio::task::spawn_blocking(move || {
            compute_audit_info_sync(&pp)
        }).await.unwrap_or_else(|_| AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] });

        // Detect if project has an issue tracker MCP (GitHub, GitLab, Jira, Linear, etc.)
        let has_issue_tracker_mcp = detect_issue_tracker_mcp(&project_path);

        let validation_prompt = build_validation_prompt(&language, &audit_info, has_issue_tracker_mcp);

        let now = Utc::now();
        let discussion_id = Uuid::new_v4().to_string();
        let initial_message = DiscussionMessage {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content: validation_prompt,
            agent_type: None,
            timestamp: now,
            tokens_used: 0,
            auth_mode: None,
            model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None,
        };

        let discussion = Discussion {
            id: discussion_id.clone(),
            project_id: Some(project_id.clone()),
            title: "Validation audit AI".to_string(),
            agent: agent_type.clone(),
            language: language.clone(),
            participants: vec![agent_type.clone()],
            messages: vec![initial_message.clone()],
            message_count: 1,
            skill_ids: skill_ids_for_disc.clone(),
            profile_ids: vec![
                "architect".into(),
                "tech-lead".into(),
                "qa-engineer".into(),
                "devils-advocate".into(),
            ],
            directive_ids: vec![],
            tier: crate::models::ModelTier::Default,
            pin_first_message: true,
            archived: false,
            pinned: false,
            workspace_mode: "Direct".into(),
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
        let disc_created = db.with_conn(move |conn| {
            crate::db::discussions::insert_discussion(conn, &disc)?;
            crate::db::discussions::insert_message(conn, &disc.id, &msg)?;
            Ok(())
        }).await;

        let disc_id = match disc_created {
            Ok(()) => {
                let ev = serde_json::json!({ "discussion_id": discussion_id });
                yield Event::default().event("validation_created").data(ev.to_string());
                Some(discussion_id)
            }
            Err(e) => {
                tracing::error!("Failed to create validation discussion: {}", e);
                let err = serde_json::json!({ "error": format!("Failed to create validation discussion: {}", e) });
                yield Event::default().event("step_error").data(err.to_string());
                None
            }
        };

        // Generate checksums for drift detection
        {
            let pp = project_path.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let mappings: Vec<crate::core::checksums::ChecksumMapping> = ANALYSIS_STEPS.iter()
                    .enumerate()
                    .filter(|(_, s)| !s.sources.is_empty())
                    .map(|(i, s)| {
                        let checksums = crate::core::checksums::compute_step_checksums(&pp, s.sources);
                        crate::core::checksums::ChecksumMapping {
                            ai_file: s.target_file.to_string(),
                            audit_step: i + 1,
                            sources: s.sources.iter().map(|p| p.to_string()).collect(),
                            checksums,
                        }
                    })
                    .collect();
                if let Err(e) = crate::core::checksums::write_checksums_file(&pp, &mappings) {
                    tracing::warn!("Failed to write checksums: {}", e);
                } else {
                    tracing::info!("Wrote docs/checksums.json with {} mappings", mappings.len());
                }
            }).await;
        }

        // Audit fully complete — drop progress so UI polling can stop and
        // `GET /audit-status` reports `None`.
        if let Ok(mut t) = audit_tracker.lock() { t.clear_progress(&project_id); }

        let done = serde_json::json!({
            "status": "complete",
            "total_steps": total_steps,
            "discussion_id": disc_id,
            "template_was_installed": template_installed
        });
        yield Event::default().event("done").data(done.to_string());
    });

    Sse::new(stream)
}

/// POST /api/projects/:id/cancel-audit
/// Cancel a running audit and remove ALL files created by the audit.
pub async fn cancel_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    // Look up project
    let project = match state.db.with_conn({
        let id = id.clone();
        move |conn| crate::db::projects::get_project(conn, &id)
    }).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_id = project.id.clone();

    // 1. Signal cancellation and kill any running agent process
    {
        let Ok(mut tracker) = state.audit_tracker.lock() else {
            return Json(ApiResponse::err("Internal error: audit tracker lock poisoned"));
        };
        tracker.cancelled.insert(project_id.clone());
        // Drop live progress so GET /audit-status stops reporting "running"
        // even if the SSE stream is slow to notice the cancellation flag.
        tracker.clear_progress(&project_id);
        if let Some(pid) = tracker.running_pids.remove(&project_id) {
            tracing::info!("Killing audit agent process (PID {}) for project {}", pid, project_id);
            // Kill the process tree: first try killing the process group, then the process itself
            let _ = sync_cmd("kill")
                .args(["-9", &format!("-{}", pid)]) // negative PID = process group
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let _ = sync_cmd("kill")
                .args(["-9", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    // Small delay to let the SSE stream detect the cancellation
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // 2. Delete all audit-created files
    let project_path_str = project.path.clone();
    let cleanup_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        if !project_path.exists() {
            return Err(format!("Project path not found: {}", project_path.display()));
        }

        // Remove the project's docs folder entirely. Cleanup hits both
        // post-pivot `docs/` AND legacy `ai/` if both happen to be on
        // disk (e.g. half-finished migration), so the operator gets a
        // clean slate.
        for folder in ["docs", "doc", "ai"] {
            let dir = project_path.join(folder);
            if dir.exists() && dir.is_dir() && !dir.is_symlink() {
                std::fs::remove_dir_all(&dir)
                    .map_err(|e| format!("Failed to remove {}/: {}", folder, e))?;
                tracing::info!("Removed {}/ directory from {}", folder, project_path.display());
            }
        }
        // Drop a `ai` symlink if one was left over from a migration.
        let ai_link = project_path.join("ai");
        if ai_link.is_symlink() {
            let _ = std::fs::remove_file(&ai_link);
        }

        // Remove redirector files (CLAUDE.md, .cursorrules, etc.)
        for filename in AUDIT_REDIRECTOR_FILES {
            let file = project_path.join(filename);
            if file.exists() {
                if let Err(e) = std::fs::remove_file(&file) {
                    tracing::warn!("Failed to remove {}: {}", filename, e);
                } else {
                    tracing::info!("Removed {} from {}", filename, project_path.display());
                }
            }
        }

        Ok(())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = cleanup_result {
        // Clear cancellation flag before returning error
        if let Ok(mut tracker) = state.audit_tracker.lock() {
            tracker.cancelled.remove(&project_id);
        }
        return Json(ApiResponse::err(e));
    }

    // 3. Delete any validation discussion for this project
    let pid = project_id.clone();
    if let Err(e) = state.db.with_conn(move |conn| {
        // Find and delete validation discussions for this project
        let discussions = crate::db::discussions::list_discussions(conn)?;
        for disc in discussions {
            if disc.project_id.as_deref() == Some(&pid) && disc.title == "Validation audit AI" {
                crate::db::discussions::delete_discussion(conn, &disc.id)?;
                tracing::info!("Deleted validation discussion {} for project {}", disc.id, pid);
            }
        }
        Ok(())
    }).await {
        tracing::error!("Failed to delete validation discussions for project: {e}");
    }

    // 4. Clear cancellation flag
    if let Ok(mut tracker) = state.audit_tracker.lock() {
        tracker.cancelled.remove(&project_id);
    }

    // Return updated status (should be NoTemplate now)
    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}
