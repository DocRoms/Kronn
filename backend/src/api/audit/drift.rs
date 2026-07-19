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

    // Validate requested step numbers against the FULL chained pipeline
    // (foundation 1..9 + chained sub-audits 10..16), not just the 9
    // foundation steps — otherwise a drift-flagged sub-audit section (Codex
    // #8) could never be refreshed, the "Mettre à jour" button would send a
    // step the endpoint rejects.
    let chain = super::assemble_chained_steps(crate::models::AuditKind::Full);
    let first_chained_step = ANALYSIS_STEPS.len() + 1;
    let total_steps_available = chain.len();
    // An empty selection would run nothing and still end `done complete` — a
    // no-op masquerading as a refresh. Refuse it like the bridge does.
    if req.steps.is_empty() {
        let msg = serde_json::json!({ "error": "partial-audit requires at least one step" });
        let stream: SseStream = Box::pin(futures::stream::once(async move {
            Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
        }));
        return Sse::new(stream);
    }
    for &step in &req.steps {
        // Synthetic steps (the Full pipeline's REVIEW pseudo-step) have no
        // real target file: the validator would pass them on exit code 0
        // alone and a "refresh" could end Completed having verified
        // nothing. Refused before any lease/row exists.
        if (1..=total_steps_available).contains(&step)
            && !super::partial_selectable(&chain[step - 1])
        {
            let msg = serde_json::json!({
                "error": format!("Step {step} ({}) is a synthetic step — it cannot be refreshed via partial-audit", chain[step - 1].target_file)
            });
            let stream: SseStream = Box::pin(futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
            }));
            return Sse::new(stream);
        }
        if step < 1 || step > total_steps_available {
            let msg = serde_json::json!({
                "error": format!("Invalid step {}: must be between 1 and {}", step, total_steps_available)
            });
            let stream: SseStream = Box::pin(futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
            }));
            return Sse::new(stream);
        }
    }

    // De-positionalize: a step number in the request comes from the STORED
    // baseline (the drift response echoes `audit_step` from checksums.json),
    // which may predate a chain reorder/insertion. The stable identity of a
    // section is its `ai_file` — when the stored mapping for a requested
    // number names a different file than the current chain slot, re-route to
    // the chain position that owns that file today. Positional fallback when
    // no baseline entry exists (fresh baseline, foundation steps).
    // STRICT read (Codex msg 160): a corrupt manifest or a dead blocking
    // task must refuse the run — falling back to an empty mapping list here
    // silently re-routes every requested step to its POSITIONAL slot, which
    // after a reorder can be a different section entirely.
    let stored_mappings: Vec<crate::core::checksums::ChecksumMapping> = {
        let pp = project_path.clone();
        let read = tokio::task::spawn_blocking(move || {
            crate::core::checksums::read_checksums_file_strict(&pp)
                .map(|f| f.map(|f| f.mappings).unwrap_or_default())
        }).await
            .map_err(|e| format!("baseline read task failed: {e}"))
            .and_then(|r| r);
        match read {
            Ok(mappings) => mappings,
            Err(e) => {
                let msg = serde_json::json!({
                    "error": format!("{e} — partial refresh refused; run a full audit to rebuild the baseline")
                });
                let stream: SseStream = Box::pin(futures::stream::once(async move {
                    Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
                }));
                return Sse::new(stream);
            }
        }
    };
    let mut resolved_steps: Vec<usize> = Vec::new();
    for &requested in &req.steps {
        let stored = stored_mappings.iter().find(|m| m.audit_step == requested);
        let resolved = match stored {
            Some(m) if chain[requested - 1].target_file != m.ai_file => {
                match chain.iter().position(|s| s.target_file == m.ai_file) {
                    Some(idx) => idx + 1,
                    // The baseline references a section the current pipeline
                    // no longer produces — falling back to the positional
                    // slot would silently re-run a DIFFERENT dimension.
                    None => {
                        let msg = serde_json::json!({
                            "error": format!(
                                "Baseline section '{}' (step {requested}) is no longer part of the audit pipeline — run a full audit to rebuild the baseline",
                                m.ai_file
                            )
                        });
                        let stream: SseStream = Box::pin(futures::stream::once(async move {
                            Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
                        }));
                        return Sse::new(stream);
                    }
                }
            }
            _ => requested,
        };
        // Two stale sections can resolve to the same current slot after a
        // reorder — run it once.
        if !resolved_steps.contains(&resolved) {
            resolved_steps.push(resolved);
        }
    }

    let tokens = {
        let config = state.config.read().await;
        config.tokens.clone()
    };

    let agent_type = req.agent;
    if !super::agent_can_audit(&agent_type) {
        let msg = serde_json::json!({
            "error": format!("{agent_type:?} cannot run audits: no filesystem access — the refreshed docs would never be written.")
        });
        let stream: SseStream = Box::pin(futures::stream::once(async move {
            Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
        }));
        return Sse::new(stream);
    }
    let requested_steps = resolved_steps;
    let total_requested = requested_steps.len();
    let audit_tracker = state.audit_tracker.clone();
    let guard_db = state.db.clone();
    let guard_tracker = state.audit_tracker.clone();
    let guard_project = id.clone();
    let project_id_for_progress = id.clone();

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        // Take the SAME single-project audit lease the Full/specialized paths
        // use — a partial refresh must not overlap a Full run (it would mutate
        // the same tracker + docs/checksums concurrently). Atomic under the
        // tracker mutex; refusal is an SSE `error` event.
        if !audit_tracker.lock().map(|mut t| t.try_acquire_lease(&project_id_for_progress)).unwrap_or(false) {
            yield Event::default().event("error").data(
                serde_json::json!({
                    "error": "Audit already running for this project; wait for it to finish before refreshing drift."
                }).to_string(),
            );
            return;
        }
        // Same abandonment story as the full pipeline: this stream is held
        // by the BROWSER TAB — a Ctrl+C'd backend or a navigated-away tab
        // dropped the generator before the tail (checksums merge + tracker
        // clear), leaving an eternal "3 sections obsolètes" + a phantom
        // active-audit entry. No audit_runs row on this path → run_id None.
        // The guard is built right after the lease (no await between) and
        // releases it on every exit path.
        let mut drop_guard = super::full::AuditDropGuard::new(
            guard_db,
            guard_tracker,
            None,
            guard_project,
        );
        drop_guard.hold_lease();

        // A3 — partial runs get their own audit_runs row: history and the
        // resume/validate rules must see a failed or newer partial, not
        // skip it. Mandatory, like the full pipeline.
        let audit_run_id = uuid::Uuid::new_v4().to_string();
        let inserted = {
            let run_id = audit_run_id.clone();
            let pid = project_id_for_progress.clone();
            let agent_name = format!("{:?}", agent_type);
            state.db.with_conn(move |conn| {
                crate::db::audit_runs::insert_running(
                    conn, &run_id, &pid, "Partial", &agent_name, Utc::now(),
                )
            }).await
        };
        if let Err(e) = inserted {
            yield Event::default().event("error").data(
                serde_json::json!({
                    "error": format!("Could not record the partial run (db): {e} — refresh refused.")
                }).to_string(),
            );
            return;
        }
        drop_guard.set_run_id(audit_run_id.clone());
        // CONTRACTUAL revocation (Codex A5) — AFTER the row insert, like the
        // full pipeline: if the insert fails the launch is refused with ZERO
        // mutation (Validated intact); if the revocation fails, the armed
        // guard persists this row Interrupted.
        {
            let pp = project_path.clone();
            let revoked = tokio::task::spawn_blocking(move || {
                crate::core::kronn_state::revoke_validated(&pp)
            }).await;
            let outcome = match revoked {
                Ok(r) => r,
                Err(e) => Err(format!("revocation task failed: {e}")),
            };
            if let Err(e) = outcome {
                yield Event::default().event("error").data(
                    serde_json::json!({
                        "error": format!("Could not revoke the prior validation state: {e} — refresh refused.")
                    }).to_string(),
                );
                return;
            }
        }


        if let Ok(mut t) = audit_tracker.lock() {
            t.start_progress(&project_id_for_progress, total_requested as u32, "partial");
        }

        // `requested_steps` are the CANONICAL (resolved) steps — the client
        // request may name pre-reorder positions; validating the terminal
        // partition against the raw request would refuse a legitimate
        // re-route. The done partition is defined over THIS list.
        let start = serde_json::json!({
            "total_steps": total_requested,
            "requested_steps": &requested_steps,
        });
        yield Event::default().event("start").data(start.to_string());

        // Same-inputs parity with the Full pipeline (Codex lot-3 #4/#3):
        // when step 8 is selected, its deterministic detector signals AND
        // the known-debt digests are prepared exactly like the Full run;
        // enforce mode drives the same bounded retry policy.
        let enforce_mode = crate::core::anti_halluc::current_mode()
            == crate::core::anti_halluc::AntiHallucMode::Enforce;
        let step8_selected = requested_steps.iter()
            .any(|&n| chain[n - 1].target_file.ends_with("inconsistencies-tech-debt.md"));
        let (detector_signals, prior_td_digests) = if step8_selected {
            let p = project_path.clone();
            let signals = tokio::task::spawn_blocking(move || {
                crate::core::audit_detectors::run_detectors(&p)
            }).await.unwrap_or_default();
            let digests = super::reconciliation::digest_prior_tech_debt(&project_path.join("docs"));
            (signals, digests)
        } else {
            (Vec::new(), Vec::new())
        };

        // Only steps whose agent actually SUCCEEDED get their baseline entry
        // refreshed — a failed section must stay stale (false-freshness
        // otherwise: the docs weren't regenerated but drift stops flagging).
        // A step whose target did NOT change (normalized hash equal after
        // every gate passed) is `unchanged`: exit 0 + old file is not a
        // refresh — the section stays stale too (matrix v2).
        let mut succeeded_steps: Vec<usize> = Vec::new();
        let mut unchanged_steps: Vec<usize> = Vec::new();

        for (progress_idx, &step) in requested_steps.iter().enumerate() {
            let analysis_step = &chain[step - 1];
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
            // Chained sub-audit steps (10..16) carry the relevance gate, same
            // as a full chained run — a partial re-run of a sub-audit that no
            // longer applies must still write its one-line "Not applicable".
            let gate = super::gate_for_step(step, first_chained_step);
            let mut full_prompt = format!("{}\n\n{}{}", PROMPT_PREAMBLE, gate, analysis_step.prompt)
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
            if analysis_step.target_file.ends_with("inconsistencies-tech-debt.md") {
                full_prompt.push_str(&super::step8_context_block(&detector_signals, &prior_td_digests));
            }

            // Rewrite-proof snapshot BEFORE the agent runs — compared as the
            // LAST gate after CLI/validator (and step-8/enforce gates when
            // they apply): equality only means `unchanged` when everything
            // else was green.
            let pre_snapshot = match super::validation::target_snapshot(&project_path, analysis_step.target_file) {
                Ok(snap) => snap,
                Err(reason) => {
                    // Unreadable pre-state (≠ absent): fail before burning tokens.
                    yield Event::default().event("step_warning").data(
                        serde_json::json!({
                            "step": step, "file": file_label,
                            "reason": reason, "repaired_from_template": false,
                        }).to_string()
                    );
                    yield Event::default().event("step_done").data(
                        serde_json::json!({
                            "step": step, "success": false,
                            "outcome": "failed", "file": file_label,
                        }).to_string()
                    );
                    continue;
                }
            };

            let max_attempts = if enforce_mode {
                super::anti_hallu_enforce::MAX_ATTEMPTS
            } else {
                1
            };
            let mut attempt: usize = 0;
            let mut citation_feedback: Option<String> = None;
            'attempts: loop {
            attempt += 1;
            let attempt_prompt = match &citation_feedback {
                Some(fb) => format!("{full_prompt}\n\n{fb}"),
                None => full_prompt.clone(),
            };
            match runner::start_agent_with_config(runner::AgentStartConfig {
                full_access: true,
                tier: crate::models::ModelTier::Reasoning,
                ..runner::AgentStartConfig::new(&agent_type, &project_path_str, &attempt_prompt, &tokens)
            }).await {
                Ok(mut process) => {
                    while let Some(line) = process.next_line().await {
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }

                    let status = process.child.wait().await;
                    process.fix_ownership();
                    tracing::debug!("Partial audit step {}: fix_ownership applied for {}", step, file_label);
                    let cli_success = status.map(|s| s.success()).unwrap_or(false);
                    // Gate order = Full parity (matrix v2): CLI → semantic
                    // validator → step-8 disposition → enforce lint → the
                    // rewrite-proof snapshot LAST.
                    let (mut success, validation_warning) =
                        super::validation::validate_step_output(
                            cli_success, &project_path, analysis_step.target_file,
                        );
                    if let Some(w) = validation_warning {
                        let ev = serde_json::json!({
                            "step": step, "file": file_label,
                            "reason": w.reason, "repaired_from_template": w.repaired,
                        });
                        yield Event::default().event("step_warning").data(ev.to_string());
                    }

                    if success && analysis_step.target_file.ends_with("inconsistencies-tech-debt.md") {
                        if let Some(w) = super::validation::check_detector_disposition(
                            &project_path, &detector_signals,
                        ) {
                            success = false;
                            yield Event::default().event("step_warning").data(
                                serde_json::json!({
                                    "step": step, "file": file_label,
                                    "reason": w.reason, "repaired_from_template": false,
                                }).to_string()
                            );
                        }
                    }

                    {
                        use super::anti_hallu_enforce::EnforceGateOutcome;
                        match super::anti_hallu_enforce::evaluate_enforce_gate(
                            enforce_mode, success, analysis_step.target_file,
                            &project_path, attempt, max_attempts,
                        ) {
                            EnforceGateOutcome::NotApplicable => {}
                            EnforceGateOutcome::Unreadable(reason) => {
                                success = false;
                                yield Event::default().event("step_warning").data(
                                    serde_json::json!({
                                        "step": step, "file": file_label,
                                        "reason": reason, "repaired_from_template": false,
                                    }).to_string()
                                );
                            }
                            EnforceGateOutcome::Retry { feedback, fabricated } => {
                                yield Event::default().event("step_retry").data(
                                    serde_json::json!({
                                        "step": step, "file": file_label,
                                        "attempt": attempt, "max_attempts": max_attempts,
                                        "fabricated_count": fabricated,
                                        "reason": "anti_hallu_fabricated_citations",
                                    }).to_string()
                                );
                                citation_feedback = Some(feedback);
                                continue 'attempts;
                            }
                            EnforceGateOutcome::Fail { reason } => {
                                success = false;
                                yield Event::default().event("step_warning").data(
                                    serde_json::json!({
                                        "step": step, "file": file_label,
                                        "reason": reason, "repaired_from_template": false,
                                    }).to_string()
                                );
                            }
                            EnforceGateOutcome::Pass { .. } => {
                                // No mutation here (Codex msg 150): stamping
                                // happens only after `succeeded` is proven.
                            }
                        }
                    }

                    // Final gate: the rewrite proof — only once every other
                    // gate is green; a failed step keeps its real reason.
                    let outcome = if success {
                        match super::validation::target_snapshot(&project_path, analysis_step.target_file) {
                            Err(reason) => {
                                yield Event::default().event("step_warning").data(
                                    serde_json::json!({
                                        "step": step, "file": file_label,
                                        "reason": reason, "repaired_from_template": false,
                                    }).to_string()
                                );
                                "failed"
                            }
                            Ok(post) if post == pre_snapshot => {
                                unchanged_steps.push(step);
                                yield Event::default().event("step_unchanged").data(
                                    serde_json::json!({ "step": step, "file": file_label }).to_string(),
                                );
                                "unchanged"
                            }
                            Ok(_) => {
                                succeeded_steps.push(step);
                                // Substantial rewrite PROVEN — only now may
                                // enforce stamp the curated audit dates. The
                                // step already succeeded, so any stamp failure
                                // (read or write) is a NON-terminal `warning`
                                // — the partial dispatcher shows it, never
                                // swallows it.
                                if enforce_mode {
                                    let target_path = project_path.join(analysis_step.target_file);
                                    match std::fs::read_to_string(&target_path) {
                                        Ok(written) => {
                                            let today = Utc::now().format("%Y-%m-%d").to_string();
                                            if let Some(stamped) =
                                                super::anti_hallu_enforce::stamp_curated_audit_dates(&written, &today)
                                            {
                                                if let Err(e) = std::fs::write(&target_path, &stamped) {
                                                    yield Event::default().event("warning").data(
                                                        serde_json::json!({
                                                            "message": format!(
                                                                "Step {step} ({file_label}): audit-date stamp failed: {e}"
                                                            ),
                                                        }).to_string()
                                                    );
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            yield Event::default().event("warning").data(
                                                serde_json::json!({
                                                    "message": format!(
                                                        "Step {step} ({file_label}): audit-date stamp skipped, target unreadable: {e}"
                                                    ),
                                                }).to_string()
                                            );
                                        }
                                    }
                                }
                                "succeeded"
                            }
                        }
                    } else {
                        "failed"
                    };

                    let step_done = serde_json::json!({
                        "step": step,
                        "success": success && outcome == "succeeded",
                        "outcome": outcome,
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
                    // "every step closes exactly once" holds here too.
                    yield Event::default().event("step_done").data(
                        serde_json::json!({
                            "step": step, "success": false,
                            "outcome": "failed", "file": file_label,
                        }).to_string()
                    );
                }
            }
            break 'attempts;
            } // 'attempts
        }

        // Compute the refreshed baseline — ONLY for steps whose agent
        // succeeded — but do NOT write it yet (Codex lot-2 #4): the file
        // write happens strictly AFTER the terminal DB transaction commits,
        // so a terminal failure can never leave drift green while the run
        // is Interrupted (the retry would lose its scope). A failed
        // baseline write after a committed Completed is the conservative
        // direction: sections merely stay stale.
        let merged_baseline: Option<Vec<crate::core::checksums::ChecksumMapping>> = if !succeeded_steps.is_empty() {
            let pp = project_path.clone();
            let steps_clone = succeeded_steps.clone();
            let chain_for_merge = chain.clone();
            let merge_res = tokio::task::spawn_blocking(move || {
                compute_merged_baseline(&pp, &steps_clone, &chain_for_merge)
            }).await
                // A JoinError must NOT silently downgrade the run to
                // no-change (the tx would persist Failed while the SSE lists
                // still say steps succeeded — DB/SSE divergence).
                .map_err(|e| format!("baseline merge task failed: {e}"))
                .and_then(|r| r);
            match merge_res {
                Ok(merged) => Some(merged),
                // Terminal failure (JoinError or unreadable/corrupt
                // manifest): the drop guard persists Interrupted, the stale
                // baseline keeps the scope re-runnable.
                Err(e) => {
                    tracing::error!("Baseline merge computation failed: {e}");
                    yield Event::default().event("error").data(
                        serde_json::json!({
                            "error": format!("Baseline merge computation failed: {e} — the run is NOT finished; the stale scope is preserved for the retry.")
                        }).to_string(),
                    );
                    return;
                }
            }
        } else {
            None
        };

        // ── Terminal write (Codex A5 v3): authoritative + atomic, same
        // contract as the full pipeline. A successful partial creates its
        // SCOPED validation discussion in the same transaction that stamps
        // Completed + links it + persists the structured outcomes; the
        // project stays Audited until that discussion ends on the terminal
        // signal and the user validates. Failure path: Interrupted +
        // outcomes, no discussion. A failed write = terminal `error` +
        // return with the guard armed — never a `done`.
        let no_failures = requested_steps.iter()
            .all(|s| succeeded_steps.contains(s) || unchanged_steps.contains(s));
        let partial_success = no_failures && !succeeded_steps.is_empty() && merged_baseline.is_some();
        let refreshed_files: Vec<String> = succeeded_steps.iter()
            .map(|&n| chain[n - 1].target_file.to_string())
            .collect();
        let run_td_ids: Vec<String> = {
            let pp = project_path.clone();
            let idx: Vec<String> = refreshed_files.iter()
                .filter(|f| f.contains("inconsistencies-")).cloned().collect();
            tokio::task::spawn_blocking(move || {
                idx.iter()
                    .filter_map(|f| std::fs::read_to_string(pp.join(f)).ok())
                    .flat_map(|c| super::reconciliation::parse_index_td_ids(&c))
                    .collect::<std::collections::BTreeSet<String>>()
                    .into_iter().collect::<Vec<String>>()
            }).await.unwrap_or_default()
        };
        // Exact partition (matrix v2): requested = succeeded ⊎ unchanged ⊎ failed.
        let outcomes_json = serde_json::json!({
            "requested": requested_steps,
            "succeeded": succeeded_steps,
            "unchanged": unchanged_steps,
            "failed": requested_steps.iter()
                .filter(|s| !succeeded_steps.contains(s) && !unchanged_steps.contains(s))
                .collect::<Vec<_>>(),
        }).to_string();

        let pending_validation: Option<(Discussion, DiscussionMessage)> = if partial_success {
            let language = { state.config.read().await.language.clone() };
            let prompt = super::helpers::build_partial_validation_prompt(
                &refreshed_files, &run_td_ids, &language,
            );
            let now = Utc::now();
            let disc_id = uuid::Uuid::new_v4().to_string();
            let msg = DiscussionMessage {
                model: None, lint_report: None,
                id: uuid::Uuid::new_v4().to_string(),
                role: MessageRole::User, content: prompt, agent_type: None,
                timestamp: now, tokens_used: 0, auth_mode: None,
                model_tier: None, cost_usd: None, author_pseudo: None,
                author_avatar_email: None, source_msg_id: None, duration_ms: None,
            };
            let disc = Discussion {
                awaiting_agent: false,
                id: disc_id,
                project_id: Some(project_id_for_progress.clone()),
                title: format!("Validation audit partiel ({} section{})",
                    refreshed_files.len(), if refreshed_files.len() > 1 { "s" } else { "" }),
                agent: agent_type.clone(),
                language,
                participants: vec![agent_type.clone()],
                messages: vec![msg.clone()],
                message_count: 1, non_system_message_count: 1,
                skill_ids: vec![],
                profile_ids: vec!["architect".into(), "qa-engineer".into()],
                directive_ids: vec![],
                tier: crate::models::ModelTier::Default,
                model: None,
                pin_first_message: true,
                archived: false, pinned: false,
                workspace_mode: "Direct".into(),
                workspace_path: None, worktree_branch: None,
                summary_cache: None, summary_up_to_msg_idx: None,
                summary_strategy: crate::models::SummaryStrategy::Auto,
                introspection_call_count: 0,
                shared_id: None, shared_with: vec![],
                workflow_run_id: None,
                test_mode_restore_branch: None, test_mode_stash_ref: None,
                created_at: now, updated_at: now,
            };
            Some((disc, msg))
        } else {
            None
        };

        let failed_steps: Vec<usize> = requested_steps.iter()
            .filter(|s| !succeeded_steps.contains(s) && !unchanged_steps.contains(s))
            .copied().collect();
        let terminal = if !failed_steps.is_empty() {
            PartialTerminal::Interrupted { failed_steps: failed_steps.clone() }
        } else if let Some(pending) = pending_validation.clone() {
            PartialTerminal::Success(Box::new(pending))
        } else {
            PartialTerminal::NoChange { unchanged_steps: unchanged_steps.clone() }
        };
        // Single terminal authority (Codex): the SSE `done` mirrors THIS
        // object — never a second computation that could diverge from what
        // the transaction persists.
        let done_status = done_status_of(&terminal);
        let finalize = finalize_partial_run(
            &state.db,
            audit_run_id.clone(),
            project_path.clone(),
            terminal,
            outcomes_json.clone(),
        ).await;
        if let Err(msg) = finalize {
            tracing::error!("Failed to finalize partial run {audit_run_id}: {msg}");
            // Terminal failure ⇒ terminal event (`error`, not `step_error`):
            // the stream ends here and the client must clean up exactly once.
            yield Event::default().event("error").data(
                serde_json::json!({
                    "error": format!("Could not persist the refresh's final status: {msg} — the run is NOT finished; it will be reconciled. The drift baseline was NOT touched: the stale scope is preserved for the retry.")
                }).to_string(),
            );
            return;
        }

        // Baseline write STRICTLY after the committed terminal status
        // (Codex lot-2 #4): a DB failure above never leaves drift green,
        // and a write failure here is the conservative direction (the
        // sections merely stay stale on a Completed run).
        if partial_success {
            if let Some(merged) = merged_baseline.clone() {
                let pp = project_path.clone();
                let write_res = tokio::task::spawn_blocking(move || -> Result<(), String> {
                    crate::core::checksums::write_checksums_file(&pp, &merged)?;
                    if let Err(e) = crate::core::kronn_state::record_audit(&pp, "partial") {
                        tracing::warn!("Failed to record partial audit in .kronn.json: {e}");
                    }
                    Ok(())
                }).await;
                let outcome: Result<(), String> = match write_res {
                    Ok(r) => r,
                    Err(e) => Err(format!("baseline write task failed: {e}")),
                };
                if let Err(msg) = outcome {
                    tracing::error!("Baseline write failed after commit: {msg}");
                    // Post-commit: the run IS Completed — a fatal-looking
                    // event here would contradict the `done complete` that
                    // follows. Non-terminal warning; sections merely stay
                    // stale (conservative direction).
                    yield Event::default().event("warning").data(
                        serde_json::json!({
                            "message": format!("Baseline write failed: {msg} — the refreshed sections will still show as stale; re-run the update to rewrite the baseline.")
                        }).to_string(),
                    );
                }
            }
        }

        let disc_id: Option<String> = pending_validation.as_ref().map(|(d, _)| d.id.clone());
        if let Some(vdisc) = disc_id.clone() {
            yield Event::default().event("validation_created").data(
                serde_json::json!({ "discussion_id": vdisc }).to_string(),
            );
            let vstate = state.clone();
            tokio::spawn(async move {
                crate::api::discussions::spawn_agent_run_with_chain(
                    vstate, vdisc, Vec::new(), None,
                ).await;
            });
        }

        if let Ok(mut t) = audit_tracker.lock() {
            t.clear_progress(&project_id_for_progress);
        }

        // Honest ending: status AND partition come from the same terminal
        // object the transaction persisted — no second computation.
        let done = serde_json::json!({
            "status": done_status,
            "total_steps": total_requested,
            "succeeded_steps": succeeded_steps,
            "unchanged_steps": unchanged_steps,
            "failed_steps": failed_steps,
            "discussion_id": disc_id,
            "audit_run_id": audit_run_id,
        });
        drop_guard.disarm();
        yield Event::default().event("done").data(done.to_string());
    });

    Sse::new(stream)
}

/// Merge the freshly-refreshed step mappings into the stored baseline.
/// Identity is the `ai_file` (stable across chain reorders) — a refreshed
/// section evicts its old mapping even under a different index from an
/// older baseline; every mapping OUTSIDE the refreshed scope is preserved.
/// STRICT on the manifest (Codex msg 160): a corrupt/unreadable file is an
/// `Err` (the caller refuses the run), never an empty baseline that would
/// silently drop the non-selected scope. A missing file stays legitimate
/// (fresh project, foundation-only baseline).
pub(crate) fn compute_merged_baseline(
    project_path: &std::path::Path,
    succeeded_steps: &[usize],
    chain: &[super::AnalysisStep],
) -> Result<Vec<crate::core::checksums::ChecksumMapping>, String> {
    let fresh_mappings: Vec<crate::core::checksums::ChecksumMapping> = succeeded_steps.iter()
        .filter_map(|&step_num| {
            let s = &chain[step_num - 1];
            if s.sources.is_empty() {
                return None;
            }
            let checksums = crate::core::checksums::compute_step_checksums(project_path, s.sources);
            Some(crate::core::checksums::ChecksumMapping {
                ai_file: s.target_file.to_string(),
                audit_step: step_num,
                sources: s.sources.iter().map(|p| p.to_string()).collect(),
                checksums,
            })
        })
        .collect();
    let refreshed: std::collections::HashSet<&str> = fresh_mappings
        .iter().map(|m| m.ai_file.as_str()).collect();
    let mut merged: Vec<crate::core::checksums::ChecksumMapping> =
        crate::core::checksums::read_checksums_file_strict(project_path)?
            .map(|f| f.mappings.into_iter()
                .filter(|m| !refreshed.contains(m.ai_file.as_str()))
                .collect())
            .unwrap_or_default();
    merged.extend(fresh_mappings);
    merged.sort_by_key(|m| m.audit_step);
    Ok(merged)
}

/// SSE `done.status` for a terminal branch — the ONLY mapping from the
/// persisted terminal to the wire status, so DB and SSE cannot diverge.
pub(crate) fn done_status_of(terminal: &PartialTerminal) -> &'static str {
    match terminal {
        PartialTerminal::Success(_) => "complete",
        PartialTerminal::Interrupted { .. } => "interrupted",
        PartialTerminal::NoChange { .. } => "no_change",
    }
}

/// Terminal branch of a partial run (Codex lot-3 matrix v2).
pub(crate) enum PartialTerminal {
    /// failed == 0 && succeeded >= 1 — validation discussion SCOPED to the
    /// succeeded sections, Completed + link, all in one commit.
    Success(Box<(Discussion, DiscussionMessage)>),
    /// failed > 0 — Interrupted, no discussion.
    Interrupted { failed_steps: Vec<usize> },
    /// failed == 0 && succeeded == 0 && unchanged >= 1 — the refresh
    /// postcondition was not reached: Failed (NEVER Completed — the health
    /// snapshot and the 076 validation contract stay pure), no discussion,
    /// baseline untouched. The SSE layer reports `no_change`.
    NoChange { unchanged_steps: Vec<usize> },
}

/// Terminal write of a partial run — authoritative and ATOMIC: every
/// branch persists its status AND the structured outcomes in ONE
/// transaction, with STRICT row transitions (a Running row must really
/// have moved — a 0-row update aborts the tx so the server never reports
/// a terminal status the DB does not carry). The caller disarms the
/// drop-guard only after this returns Ok.
pub(crate) async fn finalize_partial_run(
    db: &std::sync::Arc<crate::db::Database>,
    run_id: String,
    project_path: std::path::PathBuf,
    terminal: PartialTerminal,
    outcomes_json: String,
) -> Result<(), String> {
    db.with_conn(move |conn| {
        let tx = conn.unchecked_transaction()?;
        match &terminal {
            PartialTerminal::Success(pending) => {
                let (disc, msg) = pending.as_ref();
                crate::db::discussions::insert_discussion(&tx, disc)?;
                crate::db::discussions::insert_message(&tx, &disc.id, msg)?;
                let td_dir = project_path.join("docs/tech-debt");
                let counts = super::full::count_td_severities(&td_dir);
                let score = crate::models::compute_health_score(
                    counts.critical, counts.high, counts.medium, counts.low,
                );
                crate::db::audit_runs::complete(
                    &tx, &run_id, chrono::Utc::now(), "Completed",
                    counts.critical, counts.high, counts.medium, counts.low,
                    0, 0, 0, score, None, None,
                )?;
                crate::db::audit_runs::set_validation_discussion(&tx, &run_id, &disc.id)?;
            }
            PartialTerminal::Interrupted { failed_steps } => {
                crate::db::audit_runs::mark_interrupted_strict(
                    &tx, &run_id,
                    &format!("partial refresh incomplete (failed steps: {failed_steps:?})"),
                )?;
            }
            PartialTerminal::NoChange { unchanged_steps } => {
                crate::db::audit_runs::mark_failed_strict(
                    &tx, &run_id,
                    &format!("partial refresh wrote nothing (unchanged steps: {unchanged_steps:?}) — sections stay stale"),
                )?;
            }
        }
        crate::db::audit_runs::set_step_outcomes(&tx, &run_id, &outcomes_json)?;
        tx.commit()?;
        Ok(())
    }).await.map_err(|e| format!("terminal write failed: {e}"))
}

#[cfg(test)]
mod partial_finalize_tests {
    use super::*;

    fn mini_disc(id: &str, project: &str) -> (Discussion, DiscussionMessage) {
        let now = chrono::Utc::now();
        let msg = DiscussionMessage {
            model: None, lint_report: None, id: format!("{id}-m"),
            role: MessageRole::User, content: "validate".into(), agent_type: None,
            timestamp: now, tokens_used: 0, auth_mode: None,
            model_tier: None, cost_usd: None, author_pseudo: None,
            author_avatar_email: None, source_msg_id: None, duration_ms: None,
        };
        let disc = Discussion {
            awaiting_agent: false, id: id.into(), project_id: Some(project.into()),
            title: "Validation audit partiel (1 section)".into(),
            agent: crate::models::AgentType::ClaudeCode, language: "fr".into(),
            participants: vec![crate::models::AgentType::ClaudeCode],
            messages: vec![msg.clone()], message_count: 1, non_system_message_count: 1,
            skill_ids: vec![], profile_ids: vec![], directive_ids: vec![],
            tier: crate::models::ModelTier::Default, model: None,
            pin_first_message: true, archived: false, pinned: false,
            workspace_mode: "Direct".into(), workspace_path: None, worktree_branch: None,
            summary_cache: None, summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::Auto,
            introspection_call_count: 0, shared_id: None, shared_with: vec![],
            workflow_run_id: None, test_mode_restore_branch: None, test_mode_stash_ref: None,
            created_at: now, updated_at: now,
        };
        (disc, msg)
    }

    async fn seed_run(db: &std::sync::Arc<crate::db::Database>) {
        db.with_conn(|conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p1', 'P', '/tmp', ?1, ?1)",
                rusqlite::params![now],
            )?;
            crate::db::audit_runs::insert_running(
                conn, "run-p", "p1", "Partial", "ClaudeCode", chrono::Utc::now(),
            )
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn success_persists_completed_link_and_outcomes_atomically() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        let tmp = tempfile::TempDir::new().unwrap();
        finalize_partial_run(
            &db, "run-p".into(), tmp.path().to_path_buf(),
            PartialTerminal::Success(Box::new(mini_disc("d-scoped", "p1"))),
            r#"{"requested":[3],"succeeded":[3],"failed":[],"unchanged":[]}"#.into(),
        ).await.unwrap();
        let run = db.with_conn(|conn| Ok(crate::db::audit_runs::get_by_id(conn, "run-p")?.unwrap())).await.unwrap();
        assert_eq!(run.status, "Completed");
        assert_eq!(run.validation_discussion_id.as_deref(), Some("d-scoped"));
        assert!(run.step_outcomes_json.unwrap().contains("\"succeeded\":[3]"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn step_failure_marks_interrupted_without_any_discussion() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        let tmp = tempfile::TempDir::new().unwrap();
        finalize_partial_run(
            &db, "run-p".into(), tmp.path().to_path_buf(),
            PartialTerminal::Interrupted { failed_steps: vec![8] },
            r#"{"requested":[3,8],"succeeded":[3],"failed":[8],"unchanged":[]}"#.into(),
        ).await.unwrap();
        let (run, disc_count) = db.with_conn(|conn| {
            let run = crate::db::audit_runs::get_by_id(conn, "run-p")?.unwrap();
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM discussions", [], |r| r.get(0))?;
            Ok((run, n))
        }).await.unwrap();
        assert_eq!(run.status, "Interrupted");
        assert!(run.validation_discussion_id.is_none());
        assert_eq!(disc_count, 0, "an interrupted partial creates no discussion");
        assert!(run.report_path.unwrap().contains("failed steps: [8]"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_db_failure_leaves_the_baseline_stale() {
        // The write ordering invariant: the checksums manifest is only
        // written AFTER finalize returns Ok. A failing terminal write
        // (unknown run row here) must therefore leave the project's drift
        // scope untouched — nothing rolls the stale sections green.
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        // NO seed: the run row does not exist → complete() 0-row → Err.
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let err = finalize_partial_run(
            &db, "run-ghost".into(), tmp.path().to_path_buf(),
            PartialTerminal::Success(Box::new(mini_disc("d-x", "p1"))),
            "{}".into(),
        ).await.unwrap_err();
        assert!(err.contains("terminal write failed"), "{err}");
        assert!(!tmp.path().join("docs/checksums.json").exists(),
            "no baseline may exist after a failed terminal write");
        let orphan: i64 = db.with_conn(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM discussions", [], |r| r.get(0))?)
        }).await.unwrap();
        assert_eq!(orphan, 0, "the scoped discussion rolls back with the failure");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn all_unchanged_marks_failed_no_change_without_touching_anything() {
        // Matrix v2: refresh postcondition not reached → Failed (NEVER
        // Completed — latest_completed/health stay pure), no discussion.
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        let tmp = tempfile::TempDir::new().unwrap();
        finalize_partial_run(
            &db, "run-p".into(), tmp.path().to_path_buf(),
            PartialTerminal::NoChange { unchanged_steps: vec![3, 8] },
            r#"{"requested":[3,8],"succeeded":[],"failed":[],"unchanged":[3,8]}"#.into(),
        ).await.unwrap();
        let (run, disc_count) = db.with_conn(|conn| {
            let run = crate::db::audit_runs::get_by_id(conn, "run-p")?.unwrap();
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM discussions", [], |r| r.get(0))?;
            Ok((run, n))
        }).await.unwrap();
        assert_eq!(run.status, "Failed");
        assert!(run.report_path.unwrap().contains("wrote nothing"));
        assert_eq!(disc_count, 0);
        assert!(run.step_outcomes_json.unwrap().contains("\"unchanged\":[3,8]"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn already_terminal_row_refuses_every_branch_and_rolls_back_outcomes() {
        // Codex msg 148 — a Running row must REALLY have transitioned: on an
        // already-terminal row the strict marks abort the tx, so neither the
        // status nor the outcomes change and the server cannot report a
        // terminal the DB does not carry.
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        db.with_conn(|conn| {
            crate::db::audit_runs::mark_cancelled(conn, "run-p")
        }).await.unwrap();
        for terminal in [
            PartialTerminal::Success(Box::new(mini_disc("d-refused", "p1"))),
            PartialTerminal::Interrupted { failed_steps: vec![1] },
            PartialTerminal::NoChange { unchanged_steps: vec![1] },
        ] {
            let err = finalize_partial_run(
                &db, "run-p".into(), std::path::PathBuf::from("/tmp"),
                terminal, r#"{"marker":"must-not-persist"}"#.into(),
            ).await.unwrap_err();
            assert!(err.contains("terminal write failed"), "{err}");
        }
        let (run, disc_count) = db.with_conn(|conn| {
            let run = crate::db::audit_runs::get_by_id(conn, "run-p")?.unwrap();
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM discussions", [], |r| r.get(0))?;
            Ok((run, n))
        }).await.unwrap();
        assert_eq!(run.status, "Cancelled", "the terminal status must be untouched");
        assert!(run.step_outcomes_json.is_none(), "outcomes must roll back with the refused transition");
        assert_eq!(disc_count, 0, "the scoped discussion rolls back with the refused Success");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn same_status_transition_is_still_refused() {
        // The sneakiest weak-guard case: a re-read based check would accept
        // Interrupted→Interrupted (or Failed→Failed) since the row already
        // reads as expected. The strict marks refuse it — a terminal row is
        // written exactly once.
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        db.with_conn(|conn| {
            crate::db::audit_runs::mark_interrupted_strict(conn, "run-p", "first")
        }).await.unwrap();
        let err = finalize_partial_run(
            &db, "run-p".into(), std::path::PathBuf::from("/tmp"),
            PartialTerminal::Interrupted { failed_steps: vec![2] },
            r#"{"marker":"second-write"}"#.into(),
        ).await.unwrap_err();
        assert!(err.contains("terminal write failed"), "{err}");

        let db2 = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db2).await;
        db2.with_conn(|conn| {
            crate::db::audit_runs::mark_failed_strict(conn, "run-p", "first no-change")
        }).await.unwrap();
        let err = finalize_partial_run(
            &db2, "run-p".into(), std::path::PathBuf::from("/tmp"),
            PartialTerminal::NoChange { unchanged_steps: vec![1] },
            r#"{"marker":"second-write"}"#.into(),
        ).await.unwrap_err();
        assert!(err.contains("terminal write failed"), "{err}");
        for d in [&db, &db2] {
            let run = d.with_conn(|conn| Ok(crate::db::audit_runs::get_by_id(conn, "run-p")?.unwrap())).await.unwrap();
            assert!(run.step_outcomes_json.is_none(), "the second write must leave no trace");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_row_refuses_every_branch() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        for terminal in [
            PartialTerminal::Success(Box::new(mini_disc("d-ghost", "p1"))),
            PartialTerminal::Interrupted { failed_steps: vec![1] },
            PartialTerminal::NoChange { unchanged_steps: vec![1] },
        ] {
            let err = finalize_partial_run(
                &db, "run-ghost".into(), std::path::PathBuf::from("/tmp"),
                terminal, "{}".into(),
            ).await.unwrap_err();
            assert!(err.contains("terminal write failed"), "{err}");
        }
    }

    #[test]
    fn merge_preserves_untargeted_mappings_from_a_valid_baseline() {
        // Codex msg 160 — the merge must never lose the non-selected scope.
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("src.rs"), "fn main() {}").unwrap();
        let untargeted = crate::core::checksums::ChecksumMapping {
            ai_file: "docs/other-section.md".into(),
            audit_step: 9,
            sources: vec!["src.rs".into()],
            checksums: std::collections::BTreeMap::new(),
        };
        crate::core::checksums::write_checksums_file(tmp.path(), &[untargeted]).unwrap();

        static SOURCES: &[&str] = &["src.rs"];
        let chain = vec![super::super::AnalysisStep {
            target_file: "docs/repo-map.md", prompt: "", sources: SOURCES,
        }];
        let merged = compute_merged_baseline(tmp.path(), &[1], &chain).unwrap();
        let files: Vec<&str> = merged.iter().map(|m| m.ai_file.as_str()).collect();
        assert!(files.contains(&"docs/repo-map.md"), "the refreshed mapping is present");
        assert!(files.contains(&"docs/other-section.md"),
            "a mapping outside the refreshed scope must survive the merge");
    }

    #[test]
    fn merge_refuses_a_malformed_manifest_instead_of_erasing_the_scope() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("src.rs"), "fn main() {}").unwrap();
        std::fs::write(tmp.path().join("docs/checksums.json"), "{ not json").unwrap();

        static SOURCES: &[&str] = &["src.rs"];
        let chain = vec![super::super::AnalysisStep {
            target_file: "docs/repo-map.md", prompt: "", sources: SOURCES,
        }];
        let err = compute_merged_baseline(tmp.path(), &[1], &chain).unwrap_err();
        assert!(err.contains("malformed"), "{err}");
        // And the corrupt file is untouched — nothing was rewritten.
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("docs/checksums.json")).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn strict_read_distinguishes_missing_from_corrupt() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        assert!(crate::core::checksums::read_checksums_file_strict(tmp.path())
            .unwrap().is_none(), "missing manifest is a legitimate None");
        std::fs::write(tmp.path().join("docs/checksums.json"), "[broken").unwrap();
        assert!(crate::core::checksums::read_checksums_file_strict(tmp.path()).is_err(),
            "a corrupt manifest must be an error, never an empty baseline");
    }

    #[test]
    fn done_status_mirrors_the_persisted_terminal_exactly() {
        // Codex P0 (msg 158): the SSE done must come from the SAME object
        // the transaction persists. In particular a NoChange terminal (the
        // branch a swallowed baseline failure used to fall into) can never
        // read as `complete` on the wire.
        assert_eq!(
            done_status_of(&PartialTerminal::Success(Box::new(mini_disc("d", "p")))),
            "complete"
        );
        assert_eq!(
            done_status_of(&PartialTerminal::Interrupted { failed_steps: vec![1] }),
            "interrupted"
        );
        assert_eq!(
            done_status_of(&PartialTerminal::NoChange { unchanged_steps: vec![1] }),
            "no_change"
        );
    }

    #[test]
    fn review_pseudo_step_is_not_partial_selectable() {
        // Codex lot-2 #3 — a synthetic step passes the validator on exit
        // code 0 alone; it must be refused at selection time.
        let review = super::super::AnalysisStep {
            target_file: "REVIEW", prompt: "", sources: &[],
        };
        assert!(!super::super::partial_selectable(&review));
        let real = super::super::AnalysisStep {
            target_file: "docs/repo-map.md", prompt: "", sources: &[],
        };
        assert!(super::super::partial_selectable(&real));
    }
}
