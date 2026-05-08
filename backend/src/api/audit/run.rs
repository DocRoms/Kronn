// `POST /api/projects/:id/ai-audit` — original 10-step audit (SSE),
// + `GET /api/projects/:id/audit-status` for the polling UI to resume
// the progress bar after navigating away.

use std::convert::Infallible;

use axum::{
    extract::{Path, State},
    response::sse::{Event, Sse},
    Json,
};
use chrono::Utc;

use crate::agents::runner;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

use super::helpers::remove_bootstrap_block;
use super::{SseStream, ANALYSIS_STEPS, PROMPT_PREAMBLE};

/// POST /api/projects/:id/ai-audit
/// Runs a 10-step AI audit, streaming progress via SSE.
pub async fn run_audit(
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
    let project_path_str = project.path.clone();
    let project_path = scanner::resolve_host_path(&project.path);
    let briefing_notes = crate::api::projects::resolve_briefing_notes(&project_path, &project.briefing_notes);

    // Remove bootstrap prompt before running audit
    let index_file = project_path.join("docs/AGENTS.md");
    if index_file.exists() {
        remove_bootstrap_block(&index_file);
    }

    let tokens = {
        let config = state.config.read().await;
        config.tokens.clone()
    };

    let agent_type = req.agent;
    let total_steps = ANALYSIS_STEPS.len();
    let audit_tracker = state.audit_tracker.clone();
    let project_id_for_progress = id.clone();

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        // Seed live progress so the UI can resume via GET /audit-status after
        // navigating away. Cleared on done / error / cancelled.
        if let Ok(mut t) = audit_tracker.lock() {
            t.start_progress(&project_id_for_progress, total_steps as u32, "full");
        }

        let start = serde_json::json!({ "total_steps": total_steps });
        yield Event::default().event("start").data(start.to_string());

        for (step_num, analysis_step) in ANALYSIS_STEPS.iter().enumerate() {
            let step = step_num + 1;
            let file_label = if analysis_step.target_file == "REVIEW" { "Final review" } else { analysis_step.target_file };

            // Mirror the step_start event into the shared tracker so a
            // polling client catches the same "step 3/10 — repo-map.md".
            if let Ok(mut t) = audit_tracker.lock() {
                t.advance_step(&project_id_for_progress, step as u32, Some(file_label.to_string()));
            }

            let step_start = serde_json::json!({
                "step": step,
                "total": total_steps,
                "file": file_label
            });
            yield Event::default().event("step_start").data(step_start.to_string());

            // Inject today's date so agents don't have to guess it
            let today = Utc::now().format("%Y-%m-%d").to_string();
            let today_compact = Utc::now().format("%Y%m%d").to_string();
            let mut full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, analysis_step.prompt)
                .replace("YYYYMMDD=today", &format!("YYYYMMDD={}", today_compact))
                .replace("today's date (YYYY-MM-DD)", &today)
                .replace("set to today's date", &format!("set to {}", today));

            // Inject user briefing notes if available
            if let Some(ref notes) = briefing_notes {
                full_prompt.push_str(&format!("\n\n## Project briefing (from the user)\n{}\n", notes));
            }

            // No profiles for audit — solo agent mode produces clean factual documentation.
            // Multi-profile debate format would pollute docs/ files with discussion artifacts.

            // Always use full_access for audit (agent needs to write files)
            match runner::start_agent_with_config(runner::AgentStartConfig {
                agent_type: &agent_type, project_path: &project_path_str, work_dir: None,
                prompt: &full_prompt, tokens: &tokens, full_access: true,
                skill_ids: &[], directive_ids: &[], profile_ids: &[],
                mcp_context_override: None,
                tier: crate::models::ModelTier::Reasoning, model_tiers: None, context_files_prompt: "",
                discussion_id: None,
            }).await {
                Ok(mut process) => {
                    while let Some(line) = process.next_line().await {
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }

                    let status = process.child.wait().await;
                    process.fix_ownership();
                    tracing::debug!("Audit step {}: fix_ownership applied for {}", step, file_label);
                    let success = status.map(|s| s.success()).unwrap_or(false);

                    let step_done = serde_json::json!({
                        "step": step,
                        "success": success,
                        "file": file_label
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
                    // Continue to next step (same behavior as CLI)
                }
            }
        }

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

        // Audit finished cleanly — drop the progress entry so the UI stops
        // polling. This runs BEFORE yielding `done` so a client racing the
        // next request sees a consistent "no audit running" state.
        if let Ok(mut t) = audit_tracker.lock() {
            t.clear_progress(&project_id_for_progress);
        }

        let done = serde_json::json!({ "status": "complete", "total_steps": total_steps });
        yield Event::default().event("done").data(done.to_string());
    });

    Sse::new(stream)
}

/// GET /api/projects/:id/audit-status
///
/// Returns the current in-flight audit progress for this project, or `None`
/// if no audit is running. The UI polls this endpoint every ~2 s while its
/// `kronn:audit:<projectId>` localStorage entry is set, so the progress bar
/// survives tab/page navigation (the server-side audit process keeps
/// running whether or not an SSE client is attached).
///
/// Progress entries are written by `run_audit`, `partial_audit`, and
/// `full_audit` as they advance, and cleared on done / cancelled / error.
pub async fn audit_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Option<AuditProgress>>> {
    let snapshot = match state.audit_tracker.lock() {
        Ok(t) => t.get_progress(&id),
        Err(_) => return Json(ApiResponse::err("audit tracker lock poisoned")),
    };
    Json(ApiResponse::ok(snapshot))
}
