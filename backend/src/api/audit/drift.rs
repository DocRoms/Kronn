// Drift detection + drift-triggered partial re-audit.
// `check_drift` is pure-checksum (no LLM tokens); `partial_audit` re-runs
// only the requested step indices and merges their checksums into the
// existing manifest so unrelated sections stay marked fresh.

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

use super::{SseStream, ANALYSIS_STEPS, PROMPT_PREAMBLE};

/// GET /api/projects/:id/drift
/// Check which docs/ sections are stale based on source file checksums.
/// Pure computation — no LLM tokens consumed.
pub async fn check_drift(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<DriftCheckResponse>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path = scanner::resolve_host_path(&project.path);

    let result = tokio::task::spawn_blocking(move || {
        crate::core::checksums::check_drift(&project_path)
    }).await;

    match result {
        Ok(drift) => {
            let response = DriftCheckResponse {
                audit_date: drift.audit_date,
                stale_sections: drift.stale_sections.into_iter().map(|s| DriftSection {
                    ai_file: s.ai_file,
                    audit_step: s.audit_step,
                    changed_sources: s.changed_sources,
                }).collect(),
                fresh_sections: drift.fresh_sections,
                total_sections: drift.total_sections,
            };
            Json(ApiResponse::ok(response))
        }
        Err(e) => Json(ApiResponse::err(format!("Drift check failed: {}", e))),
    }
}

/// POST /api/projects/:id/partial-audit
/// Re-run only specific audit steps and update checksums (merge, not overwrite).
pub async fn partial_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PartialAuditRequest>,
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
    let linked_repos_block = crate::api::projects::format_linked_repos_for_prompt(&project.linked_repos);
    let pid_for_universe = project.id.clone();
    let kronn_projects_universe_block = match state
        .db
        .with_conn(crate::db::projects::list_projects)
        .await
    {
        Ok(all) => crate::api::projects::format_kronn_projects_universe_for_prompt(&all, &pid_for_universe),
        Err(_) => None,
    };

    // Validate requested step numbers
    let total_analysis_steps = ANALYSIS_STEPS.len();
    for &step in &req.steps {
        if step < 1 || step > total_analysis_steps {
            let msg = serde_json::json!({
                "error": format!("Invalid step {}: must be between 1 and {}", step, total_analysis_steps)
            });
            let stream: SseStream = Box::pin(futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
            }));
            return Sse::new(stream);
        }
    }

    let tokens = {
        let config = state.config.read().await;
        config.tokens.clone()
    };

    let agent_type = req.agent;
    let requested_steps = req.steps;
    let total_requested = requested_steps.len();
    let audit_tracker = state.audit_tracker.clone();
    let project_id_for_progress = id.clone();

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        if let Ok(mut t) = audit_tracker.lock() {
            t.start_progress(&project_id_for_progress, total_requested as u32, "partial");
        }

        let start = serde_json::json!({ "total_steps": total_requested });
        yield Event::default().event("start").data(start.to_string());

        for (progress_idx, &step) in requested_steps.iter().enumerate() {
            let analysis_step = &ANALYSIS_STEPS[step - 1];
            let file_label = if analysis_step.target_file == "REVIEW" { "Final review" } else { analysis_step.target_file };

            // Progress counter is 1-based position within the requested subset,
            // not the absolute audit step number — matches what the SSE reports.
            if let Ok(mut t) = audit_tracker.lock() {
                t.advance_step(&project_id_for_progress, (progress_idx + 1) as u32, Some(file_label.to_string()));
                t.clear_step_chips(&project_id_for_progress);
            }

            let step_start = serde_json::json!({
                "step": step,
                "progress": progress_idx + 1,
                "total": total_requested,
                "file": file_label
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
            if let Some(ref block) = linked_repos_block {
                full_prompt.push_str(&format!("\n\n{}\n", block));
            }
            if let Some(ref block) = kronn_projects_universe_block {
                full_prompt.push_str(&format!("\n\n{}\n", block));
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
                    while let Some(line) = process.next_line().await {
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }

                    let status = process.child.wait().await;
                    process.fix_ownership();
                    tracing::debug!("Partial audit step {}: fix_ownership applied for {}", step, file_label);
                    let success = status.map(|s| s.success()).unwrap_or(false);

                    let step_done = serde_json::json!({
                        "step": step,
                        "success": success,
                        "file": file_label
                    });
                    yield Event::default().event("step_done").data(step_done.to_string());
                }
                Err(e) => {
                    tracing::error!("Partial audit step {} failed to start: {}", step, e);
                    let err = serde_json::json!({
                        "error": format!("Step {} ({}): {}", step, file_label, e),
                        "step": step
                    });
                    yield Event::default().event("step_error").data(err.to_string());
                }
            }
        }

        // Merge checksums: read existing, update only re-run steps, write back
        {
            let pp = project_path.clone();
            let steps_clone = requested_steps.clone();
            let _ = tokio::task::spawn_blocking(move || {
                // Build fresh checksums for re-run steps
                let fresh_mappings: Vec<crate::core::checksums::ChecksumMapping> = steps_clone.iter()
                    .filter_map(|&step_num| {
                        let s = &ANALYSIS_STEPS[step_num - 1];
                        if s.sources.is_empty() {
                            return None;
                        }
                        let checksums = crate::core::checksums::compute_step_checksums(&pp, s.sources);
                        Some(crate::core::checksums::ChecksumMapping {
                            ai_file: s.target_file.to_string(),
                            audit_step: step_num,
                            sources: s.sources.iter().map(|p| p.to_string()).collect(),
                            checksums,
                        })
                    })
                    .collect();

                // Read existing checksums and merge
                let mut merged: Vec<crate::core::checksums::ChecksumMapping> =
                    if let Some(existing) = crate::core::checksums::read_checksums_file(&pp) {
                        // Keep mappings for steps NOT re-run
                        let rerun_steps: std::collections::HashSet<usize> = steps_clone.iter().copied().collect();
                        existing.mappings.into_iter()
                            .filter(|m| !rerun_steps.contains(&m.audit_step))
                            .collect()
                    } else {
                        Vec::new()
                    };

                // Add fresh mappings
                merged.extend(fresh_mappings);
                // Sort by step number for consistency
                merged.sort_by_key(|m| m.audit_step);

                if let Err(e) = crate::core::checksums::write_checksums_file(&pp, &merged) {
                    tracing::warn!("Failed to write checksums after partial audit: {}", e);
                } else {
                    tracing::info!("Updated docs/checksums.json for {} re-run steps", steps_clone.len());
                }
            }).await;
        }

        if let Ok(mut t) = audit_tracker.lock() {
            t.clear_progress(&project_id_for_progress);
        }

        let done = serde_json::json!({ "status": "complete", "total_steps": total_requested });
        yield Event::default().event("done").data(done.to_string());
    });

    Sse::new(stream)
}
