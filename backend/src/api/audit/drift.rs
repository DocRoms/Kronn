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

use super::{SseStream, ANALYSIS_STEPS};

/// GET /api/projects/:id/drift
/// Check which docs/ sections are stale based on source file checksums.
/// Pure computation — no LLM tokens consumed.
pub async fn check_drift(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<DriftCheckResponse>> {
    let project = match state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path = scanner::resolve_host_path(&project.path);

    let result = tokio::task::spawn_blocking(move || {
        let placeholders = crate::api::projects::template_reinstall::placeholder_docs(
            &project_path,
            &crate::api::projects::template::resolve_templates_dir().join("docs"),
        );
        (
            crate::core::checksums::check_drift_cached(&project_path),
            placeholders,
            crate::core::template_manifest::check(
                &project_path,
                &crate::api::projects::template::resolve_templates_dir().join("docs"),
            )
            .ok(),
        )
    })
    .await;

    match result {
        Ok((drift, placeholder_docs, template_update)) => Json(ApiResponse::ok(drift_response(
            drift,
            placeholder_docs,
            template_update,
        ))),
        Err(e) => Json(ApiResponse::err(format!("Drift check failed: {}", e))),
    }
}

fn drift_response(
    drift: crate::core::checksums::DriftResult,
    placeholder_docs: Vec<crate::models::PlaceholderDoc>,
    template_update: Option<crate::core::template_manifest::TemplateUpdate>,
) -> DriftCheckResponse {
    DriftCheckResponse {
        template_update,
        audit_date: drift.audit_date,
        stale_sections: drift
            .stale_sections
            .into_iter()
            .map(|s| DriftSection {
                ai_file: s.ai_file,
                audit_step: s.audit_step,
                changed_sources: s.changed_sources,
                missing_paths: s.missing_paths,
            })
            .collect(),
        fresh_sections: drift.fresh_sections,
        total_sections: drift.total_sections,
        sources_state: drift.sources_state,
        broken_citations: drift.broken_citations,
        missing_paths: Some(drift.missing_paths),
        placeholder_docs,
    }
}

/// POST /api/projects/:id/drift/acknowledge
/// « Vérifié, inchangé » on stale sections: their baseline is refreshed
/// without rewriting the docs. A human decision, held under the audit lease.
pub async fn acknowledge_drift(
    State(state): State<AppState>,
    Path(id): Path<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Json(req): Json<AcknowledgeDriftRequest>,
) -> Json<ApiResponse<DriftCheckResponse>> {
    // Not on the bridge route list either: this guard holds if it ever is.
    if bridge.is_some() {
        return Json(ApiResponse::err(
            "Acknowledging stale sections is a human decision: refused to an agent's bridge token.",
        ));
    }
    let (project_id, project_path) = match project_path_of(&state, id).await {
        Ok(found) => found,
        Err(e) => return Json(ApiResponse::err(e)),
    };
    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &project_id).await {
            Ok(lease) => lease,
            Err(error) => return Json(ApiResponse::err(error)),
        };
    let result = lease
        .blocking(move || {
            let mut shown = crate::core::citation_drift::ShownPaths::new();
            for p in &req.shown_paths {
                shown
                    .entry(p.doc.clone())
                    .or_default()
                    .entry(p.section.clone())
                    .or_default()
                    .push(p.path.clone());
            }
            crate::core::checksums::acknowledge_stale(&project_path, &req.sections, &shown)?;
            if let Err(e) = crate::core::doc_provenance::record_validation(
                &project_path,
                &req.sections,
                crate::core::doc_provenance::ValidationReason::Acknowledged,
            ) {
                tracing::warn!("Human-validation provenance not recorded: {e}");
            }
            let placeholders = crate::api::projects::template_reinstall::placeholder_docs(
                &project_path,
                &crate::api::projects::template::resolve_templates_dir().join("docs"),
            );
            Ok(drift_response(
                crate::core::checksums::check_drift_cached(&project_path),
                placeholders,
                crate::core::template_manifest::check(
                    &project_path,
                    &crate::api::projects::template::resolve_templates_dir().join("docs"),
                )
                .ok(),
            ))
        })
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {e}")));
    match result {
        Ok(drift) => Json(ApiResponse::ok(drift)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

async fn project_path_of(
    state: &AppState,
    id: String,
) -> Result<(String, std::path::PathBuf), String> {
    match state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => {
            let path = scanner::resolve_host_path(&p.path);
            Ok((p.id, path))
        }
        Ok(None) => Err("Project not found".into()),
        Err(e) => Err(format!("DB error: {e}")),
    }
}

/// GET /api/projects/:id/citation-drift
/// Every recorded citation compared with the current code, per section.
/// Deterministic, no model: the base "Revalider l'audit" (KT-1168) reads.
pub async fn citation_drift(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<CitationDriftResponse>> {
    let project_path = match project_path_of(&state, id).await {
        Ok((_, path)) => path,
        Err(e) => return Json(ApiResponse::err(e)),
    };
    let result = tokio::task::spawn_blocking(move || CitationDriftResponse {
        report: crate::core::citation_drift::check(&project_path),
        last_scheduled_check: crate::core::citation_drift::last_scheduled_check(&project_path),
    })
    .await;
    match result {
        Ok(response) => Json(ApiResponse::ok(response)),
        Err(e) => Json(ApiResponse::err(format!("Citation drift failed: {e}"))),
    }
}

/// POST /api/projects/:id/citation-drift/acknowledge
/// "Verified, unchanged": refreshes one section's fingerprints without
/// rewriting the doc. Held under the project lease like a validation.
pub async fn acknowledge_citations(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AcknowledgeCitationsRequest>,
) -> Json<ApiResponse<crate::core::citation_drift::SectionCitations>> {
    let (project_id, project_path) = match project_path_of(&state, id).await {
        Ok(found) => found,
        Err(e) => return Json(ApiResponse::err(e)),
    };
    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &project_id).await {
            Ok(lease) => lease,
            Err(error) => return Json(ApiResponse::err(error)),
        };
    let result = lease
        .blocking(move || {
            let section = crate::core::citation_drift::acknowledge(&project_path, &req.doc)?;
            if let Err(e) = crate::core::doc_provenance::record_validation(
                &project_path,
                std::slice::from_ref(&req.doc),
                crate::core::doc_provenance::ValidationReason::Acknowledged,
            ) {
                tracing::warn!("Human-validation provenance not recorded: {e}");
            }
            let documents = crate::core::audit_documents::capture(&project_path)?;
            Ok((section, req.doc, documents))
        })
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {e}")));
    let result = match result {
        Ok((section, doc, documents)) => {
            let pid = project_id.clone();
            state
                .db
                .with_conn(move |conn| {
                    crate::db::audit_documents::confirm_paths(conn, &pid, &[doc], &documents)
                })
                .await
                .map(|()| section)
                .map_err(|e| format!("Cannot record acknowledged audit documents: {e}"))
        }
        Err(e) => Err(e),
    };
    match result {
        Ok(section) => Json(ApiResponse::ok(section)),
        Err(e) => Json(ApiResponse::err(e)),
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
    let project = state
        .db
        .with_conn({
            let id = id.clone();
            move |conn| crate::db::projects::get_project(conn, &id)
        })
        .await
        .ok()
        .flatten();

    if project.is_none() {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default()
                    .event("error")
                    .data("{\"error\":\"Project not found\"}"),
            )
        }));
        return Sse::new(stream);
    }

    // Safety: early return above guarantees project is Some
    let project = project.expect("project is Some after early return");
    let project_path = scanner::resolve_host_path(&project.path);
    if let Some(error) = crate::api::projects::template_merge::audit_launch_refusal(&project_path) {
        return super::full::sse_refusal(error);
    }
    // One root for every artefact (KT-1192); a broken override refuses.
    let docs_root = match crate::core::docs_location::DocsRoot::for_project(&project_path) {
        Ok(docs_root) => docs_root,
        Err(reason) => {
            let msg = serde_json::json!({ "error": reason });
            let stream: SseStream = Box::pin(futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
            }));
            return Sse::new(stream);
        }
    };
    if let Some(refusal) = super::legacy_layout_refusal(&project_path) {
        let stream: SseStream = Box::pin(futures::stream::once(async move {
            Ok::<_, Infallible>(Event::default().event("error").data(refusal.to_string()))
        }));
        return Sse::new(stream);
    }
    // The folder's human docs wait for the briefing's choice, as for a Full run.
    let recorded_run = super::has_recorded_full_run(&state, &project.id).await;
    if let Some(refusal) =
        super::docs_folder_choice_refusal(&project_path, &docs_root, recorded_run)
    {
        return super::full::sse_refusal(refusal);
    }

    // Validate requested step numbers against the FULL chained pipeline
    // (foundation 1..9 + chained sub-audits 10..16), not just the 9
    // foundation steps — otherwise a drift-flagged sub-audit section (Codex
    // #8) could never be refreshed, the "Mettre à jour" button would send a
    // step the endpoint rejects.
    let chain = super::steps_for(crate::models::AuditKind::Full, &docs_root);
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
            && !super::partial_selectable(&chain[step - 1], &docs_root)
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
        })
        .await
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
                            Ok::<_, Infallible>(
                                Event::default().event("error").data(msg.to_string()),
                            )
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

    run_partial_pipeline(
        state,
        project,
        resolved_steps,
        req.agent,
        req.tier,
        req.connection_id,
        None,
        req.accept_unverified_agents.unwrap_or(false),
    )
    .await
}

/// The partial pipeline over canonical chain steps: lease, run row, revoked
/// validation, per-step gates, cancel, terminal write. A revalidation
/// (KT-1168) runs through it with its findings and its own restore rules.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_partial_pipeline(
    state: AppState,
    project: Project,
    requested_steps: Vec<usize>,
    agent_type: AgentType,
    tier: Option<crate::models::ModelTier>,
    connection_id: Option<String>,
    revalidation: Option<super::revalidate::RevalidationRun>,
    accept_unverified_agents: bool,
) -> Sse<SseStream> {
    let id = project.id.clone();
    let project_path_str = project.path.clone();
    let project_path = scanner::resolve_host_path(&project.path);
    let briefing_notes =
        crate::api::projects::resolve_briefing_notes(&project_path, &project.briefing_notes);
    // KT-999 — same documentation switch as the Full pipeline.
    let briefing_docs_on =
        super::briefing_docs::prompt_enabled(state.config.read().await.audit_briefing_docs);
    // KT-1193 — same rules inputs as the Full pipeline.
    let project_rules = briefing_notes
        .as_deref()
        .and_then(super::briefing_rules::parse);
    // KT-926 — the only other repos this prompt names are the ones the user
    // explicitly linked to the project. Never the list of the machine's Kronn
    // projects: the prompt goes to the model provider, and a companion list
    // would be copied into a versioned `docs/AGENTS.md`.
    let linked_repos_block =
        crate::api::projects::format_linked_repos_for_prompt(&project.linked_repos);
    let docs_root = match crate::core::docs_location::DocsRoot::for_project(&project_path) {
        Ok(docs_root) => docs_root,
        Err(reason) => {
            let msg = serde_json::json!({ "error": reason });
            let stream: SseStream = Box::pin(futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
            }));
            return Sse::new(stream);
        }
    };
    let chain = super::steps_for(crate::models::AuditKind::Full, &docs_root);
    // KT-1208 — a partial refresh runs every step single-agent; it says so
    // when the briefing asked for the cross-agent audit on a refreshed step.
    let cross_agent_skipped =
        super::cross_audit::briefing_cross_agents(briefing_notes.as_deref(), None).is_some()
            && requested_steps.iter().any(|&n| {
                chain.get(n.wrapping_sub(1)).is_some_and(|s| {
                    crate::core::cross_audit::is_cross_checked_target(s.target_file, &docs_root)
                })
            });
    let redact_targets: Vec<String> = chain.iter().map(|s| s.target_file.to_string()).collect();
    let first_chained_step = ANALYSIS_STEPS.len() + 1;

    let tokens = {
        let config = state.config.read().await;
        config.tokens.clone()
    };

    let audit_tier = tier.unwrap_or(crate::models::ModelTier::Reasoning);
    if !super::agent_can_audit(&agent_type) {
        let msg = serde_json::json!({
            "error": super::audit_refusal_message(&agent_type)
        });
        let stream: SseStream = Box::pin(futures::stream::once(async move {
            Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
        }));
        return Sse::new(stream);
    }
    // Same wiring as the Full pipeline: an HTTP agent gets file tools scoped to
    // this project (a no-op for a CLI agent).
    let agent_launcher = match super::agent_launch::AuditAgentLauncher::for_request(
        &state,
        &agent_type,
        connection_id.as_deref(),
    )
    .await
    {
        Ok(launcher) => launcher,
        Err(error) => {
            let msg = serde_json::json!({ "error": error });
            let stream: SseStream = Box::pin(futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
            }));
            return Sse::new(stream);
        }
    };
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
        if let Some(error) = crate::api::projects::template_merge::audit_launch_refusal(&project_path) {
            drop_guard.disarm();
            yield Event::default().event("error").data(error.to_string());
            return;
        }


        // A flag left by a previous run's Stop must not stop this one.
        if let Ok(mut tracker) = audit_tracker.lock() {
            tracker.cancelled.remove(&project_id_for_progress);
        }
        // Its agent sessions get the audit MCP rule (no kronn-internal, no Memory).
        let _audit_session = crate::core::audit_mcp_filter::AuditSessionGuard::enter(&project_path);

        // KT-1169 — a refresh run would supersede an audit that can still be
        // resumed: that audit is finished first.
        // KT-1312 — unless a resume would have nothing left to run: every
        // failed step is accepted, which is what the refusal asks for.
        let latest = state.db.with_conn({
            let pid = project_id_for_progress.clone();
            move |conn| {
                Ok(match crate::db::audit_runs::latest_resumable(conn, &pid)? {
                    Some(run) => {
                        let steps = crate::db::audit_runs::list_audit_steps(conn, &run.id)?;
                        super::full::resume_has_steps_left(&run, &steps).then_some(run)
                    }
                    None => None,
                })
            }
        }).await;
        let refusal = match latest {
            Ok(latest) => super::full::fresh_launch_refusal(latest.as_ref(), false)
                .map(|_| "An interrupted audit can be resumed: resume it, or accept its failed steps, before refreshing stale sections.".to_string()),
            Err(e) => Some(format!("Could not read the project's latest audit run (db): {e} — refresh refused.")),
        };
        if let Some(msg) = refusal {
            yield Event::default().event("error").data(
                serde_json::json!({ "error": msg }).to_string(),
            );
            return;
        }

        // KT-1324 — what a previous run's agent left is stopped and settled
        // before this run snapshots its targets: a survivor still writing them
        // would be snapshotted as if it were the document's own text.
        if let Err(error) = super::step_guard::settle_open_runs_before_launch(
            &state.db, &project_id_for_progress, &project_path, accept_unverified_agents,
        ).await {
            yield Event::default().event("error").data(error.event_data());
            return;
        }

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
        agent_launcher.bind_run(&audit_run_id);
        super::full::stop_validation_agents(&state, &project_id_for_progress).await;
        let provenance_writer = if revalidation.is_some() {
            crate::core::doc_provenance::DocWriter::Revalidation
        } else {
            crate::core::doc_provenance::DocWriter::PartialAudit
        };
        // A run that ends up rewriting nothing gives the validation back.
        let validated_before: Option<String> =
            crate::core::kronn_state::read(&project_path).and_then(|s| s.validated_at);
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

        // PRE-AGENT boundary: Partial agents also have full filesystem access.
        // Sweep the complete managed surface before digests, prompts or spawn.
        match super::redact_artifacts::sanitize_all(
            &project_path,
            &redact_targets,
            "partial-pre-agent",
        ) {
            Ok(r) => {
                if !r.is_empty() {
                    tracing::warn!(target: "kronn::invariant", files = r.len(),
                        "partial audit: redacted existing literals before agent read");
                }
            }
            Err(e) => {
                let run_id = audit_run_id.clone();
                match state.db.with_conn(move |conn| crate::db::audit_runs::mark_failed(
                    conn, &run_id, "secret-redaction failed before partial audit agent",
                )).await {
                    Ok(()) => drop_guard.disarm(),
                    Err(db_err) => tracing::error!("partial redaction failure could not mark run failed: {db_err}"),
                }
                if let Ok(mut t) = audit_tracker.lock() {
                    t.clear_progress(&project_id_for_progress);
                }
                yield Event::default().event("error").data(
                    serde_json::json!({
                        "error": format!("secret-redaction failed before partial audit agent: {e}"),
                        "fatal": true,
                    }).to_string(),
                );
                return;
            }
        }


        // KT-1168 — the deterministic part first: moved citations follow
        // their line. Then every agent target is snapshotted so a run that
        // rewrites nothing can give the validation back.
        if revalidation.is_some() {
            let pp = project_path.clone();
            let reanchored = tokio::task::spawn_blocking(move || super::revalidate::apply_reanchors(&pp))
                .await
                .unwrap_or_else(|e| Err(format!("re-anchor task failed: {e}")));
            match reanchored {
                Ok(docs) => {
                    for doc in docs {
                        yield Event::default().event("reanchored").data(
                            serde_json::to_string(&doc).unwrap_or_default(),
                        );
                    }
                }
                Err(e) => {
                    yield Event::default().event("warning").data(
                        serde_json::json!({ "message": format!("Moved citations not re-anchored: {e}") }).to_string(),
                    );
                }
            }
        }
        let targets: Vec<&str> = requested_steps.iter().map(|&step| chain[step - 1].target_file).collect();
        let target_snapshots = match super::helpers::validation_restore_snapshots(&project_path, &targets) {
            Ok(snapshots) => snapshots,
            Err(error) => {
                yield Event::default().event("error").data(serde_json::json!({
                    "error": format!("Could not snapshot validated documentation: {error} — refresh refused.")
                }).to_string());
                return;
            }
        };
        let restore_reason = revalidation.is_some()
            .then_some(crate::core::doc_provenance::ValidationReason::Revalidation);

        if let Ok(mut t) = audit_tracker.lock() {
            t.start_progress(&project_id_for_progress, total_requested as u32, "partial");
            t.set_auditor(
                &project_id_for_progress,
                agent_type.clone(),
                audit_tier,
                agent_launcher.connection_id(),
            );
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
        if cross_agent_skipped {
            yield Event::default().event("single_agent").data(serde_json::json!({
                "message": "This refresh runs single-agent: the cross-agent audit option applies to Full audits only.",
            }).to_string());
        }
        // KT-1199 — the refresh is a run too: its state travels with the repository.
        let run_state_writer = std::sync::Arc::new(super::run_state::RunStateWriter::new(
            project_path.clone(),
            audit_run_id.clone(),
            super::run_state::RunStateMeta::capture(
                &project_path,
                super::validate::operator_pseudo(&state).await,
                None,
            ),
            requested_steps
                .iter()
                .map(|&n| (n as u32, super::full::step_file_label(&chain[n - 1]).to_string()))
                .collect(),
        ));
        {
            let writer = run_state_writer.clone();
            let _ = state.db.with_conn(move |conn| { writer.sync(conn, false); Ok(()) }).await;
        }
        let td_decisions_before = crate::core::td_decisions::snapshot(&docs_root.dir(&project_path));

        // Same-inputs parity with the Full pipeline (Codex lot-3 #4/#3):
        // when step 8 is selected, its deterministic detector signals AND
        // the known-debt digests are prepared exactly like the Full run;
        // enforce mode drives the same bounded retry policy.
        let enforce_mode = crate::core::anti_halluc::current_mode()
            == crate::core::anti_halluc::AntiHallucMode::Enforce;
        let step8_selected = requested_steps.iter()
            .any(|&n| chain[n - 1].target_file.ends_with("inconsistencies-tech-debt.md"));
        // KT-1174 — only the ADRs in force and the TD record lines present
        // before this refresh settle a TD.
        let adr_baseline = crate::core::adr::StartOfRun::capture(&docs_root.dir(&project_path));
        let (detector_signals, ecosystems, prior_td_digests) = if step8_selected {
            let p = project_path.clone();
            let rules = project_rules.clone().unwrap_or_default().rules;
            let (signals, ecosystems) = tokio::task::spawn_blocking(move || {
                let scan = crate::core::audit_detectors::scan_project_with_rules(&p, &rules);
                (scan.signals, scan.ecosystems)
            }).await.unwrap_or_default();
            let digests = super::reconciliation::digest_prior_tech_debt(&docs_root.dir(&project_path));
            (signals, ecosystems, digests)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        let version_ctx = super::version_context::VersionContext::load(
            &state.db, &project_id_for_progress, &project_path, &project_path_str, &agent_launcher, &agent_type,
        ).await;

        // A failed section stays stale: only a done step refreshes its baseline
        // entry. An `unchanged` step passed every gate on its untouched target:
        // the section was re-verified, so it is done too (KT-1207).
        let mut succeeded_steps: Vec<usize> = Vec::new();
        let mut unchanged_steps: Vec<usize> = Vec::new();
        // KT-1388 — the advice the steps left in their documents: noted on the
        // run and in the validation prompt, never counted as a failure.
        let mut documentary_advisories: Vec<crate::core::document_optimization::Diagnostic> = Vec::new();
        // KT-1370 — the markers demoted to flagged backticked paths once a
        // step's attempts were spent, with the step that left each one.
        let mut unverified_references: Vec<(u32, crate::core::document_optimization::DemotedReference)> = Vec::new();
        // KT-927 — unknown until an agent reports usage, never 0.
        let mut run_tokens = super::agent_launch::RunTokens::default();
        // The model last written on the run row, so a step writes it only when it changes.
        let mut recorded_model: Option<String> = None;
        // KT-1357 — the sections whose cited lines and mapped sources are as the
        // last baseline recorded them. A refresh asked for such a step has
        // nothing to refresh: it ends `unchanged` without launching an agent.
        // A revalidation always runs its agents (it exists to re-check them).
        let fresh_sections: std::collections::HashSet<String> = if revalidation.is_none() {
            let path = project_path.clone();
            tokio::task::spawn_blocking(move || fresh_section_keys(&path))
                .await
                .unwrap_or_default()
        } else {
            Default::default()
        };

        for (progress_idx, &step) in requested_steps.iter().enumerate() {
            // KT-927 — a Stop between two steps. The partial pipeline had no
            // cancellation at all: `cancel-audit` set a flag nobody read and the
            // refresh ran to its end.
            if audit_tracker.lock().map(|t| t.cancelled.contains(&project_id_for_progress)).unwrap_or(false) {
                // Persist the terminal status BEFORE acknowledging (same
                // contract as the Full pipeline): on failure the guard stays
                // armed and stamps Interrupted.
                let run_id = audit_run_id.clone();
                let persisted = state.db.with_conn(move |conn| {
                    crate::db::audit_runs::mark_cancelled(conn, &run_id)
                }).await;
                match persisted {
                    Ok(()) => drop_guard.disarm(),
                    Err(e) => tracing::error!(
                        "mark_cancelled failed for {audit_run_id}: {e} — leaving the drop-guard armed"
                    ),
                }
                if super::revalidate::restore_validation_if_untouched(&project_path, validated_before.as_deref(), &target_snapshots, restore_reason) {
                    tracing::info!("Partial run stopped before rewriting anything: validation restored");
                }
                if let Ok(mut t) = audit_tracker.lock() {
                    t.clear_progress(&project_id_for_progress);
                    t.cancelled.remove(&project_id_for_progress);
                }
                yield Event::default().event("cancelled").data(
                    serde_json::json!({ "status": "cancelled" }).to_string()
                );
                return;
            }
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

            // Chained sub-audit steps (10..16) carry the relevance gate, same
            // as a full chained run — a partial re-run of a sub-audit that no
            // longer applies must still write its one-line "Not applicable".
            let gate = super::gate_for_step(step, first_chained_step);
            let reverify = super::reverifies_target(&project_path, step, analysis_step);
            let mut full_prompt = super::dated_step_prompt(gate, &super::step_prompt_body(analysis_step, reverify), Utc::now(), &docs_root);

            if let Some(ref notes) = briefing_notes {
                full_prompt.push_str(&super::briefing::briefing_prompt_section_for(
                    notes,
                    &docs_root,
                    analysis_step.target_file,
                    briefing_docs_on,
                ));
            }
            if let Some(ref block) = linked_repos_block {
                full_prompt.push_str(&format!("\n\n{}\n", block));
            }
            if analysis_step.target_file.ends_with("inconsistencies-tech-debt.md") {
                full_prompt.push_str(&super::step8_context_block(&detector_signals, &ecosystems, &prior_td_digests, &docs_root));
            }
            if let Some(block) = version_ctx.block_for(analysis_step.target_file) {
                full_prompt.push_str(&block);
            }
            if let Some(block) = revalidation.as_ref().and_then(|r| r.blocks.get(&step)) {
                full_prompt.push_str(block);
            }
            full_prompt.push_str(&super::decisions_context_block(&project_path, analysis_step, &adr_baseline.decisions, &docs_root));
            full_prompt.push_str(&super::briefing_rules::prompt_block(analysis_step.target_file, project_rules.as_ref()));

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
            // KT-1211 — the target's content before the agent runs: a stop puts
            // it back, a failure marks what the agent left as partial.
            // Kept durably too (same table as the Full pipeline): a backend
            // that dies mid-step leaves the doc half-written, and boot puts it
            // back from here — memory alone would be gone.
            let pre_content: Option<String> = if analysis_step.target_file == "REVIEW" {
                None
            } else {
                let content = super::step_guard::read_target(&project_path, analysis_step.target_file)
                    .ok()
                    .flatten();
                let (run_id, target, saved) = (
                    audit_run_id.clone(), analysis_step.target_file.to_string(), content.clone(),
                );
                if let Err(e) = state.db.with_conn(move |conn| {
                    crate::db::audit_runs::save_step_snapshot(conn, &run_id, step as u32, &target, saved.as_deref())
                }).await {
                    tracing::warn!("Could not keep the pre-step snapshot of partial step {step}: {e}");
                }
                content
            };
            // What the agent is seen writing is kept durably: a crash settles
            // the target only from content it can vouch for.
            let mut target_watch = super::step_guard::TargetWatch::new(
                &project_path, analysis_step.target_file, pre_content.as_deref(),
            );
            // KT-1360 — where HEAD and the files outside the docs root stand
            // before the agent runs; settled after every attempt.
            let step_remit = super::step_remit::StepRemit::capture(&project_path, &docs_root).await;
            let mut remit_notes: Vec<String> = Vec::new();
            // KT-1388 — the documents as they stand before the agent runs: what
            // the step then rewrites is what its advice (a stack conflict, an
            // unproven closure) is read from. The pseudo-step `REVIEW` writes
            // no document.
            let documentary_before = if analysis_step.target_file == "REVIEW" {
                None
            } else {
                match super::document_repair::snapshot(&project_path) {
                    Ok(snapshot) => Some(snapshot),
                    Err(e) => {
                        tracing::warn!("Partial audit step {step} ({file_label}): documents not read, no advice given: {e}");
                        None
                    }
                }
            };
            // KT-1370 — what this step demoted after its last attempt.
            let mut step_demotions: Vec<crate::core::document_optimization::DemotedReference> = Vec::new();
            let validated_docs_snapshot =
                match super::helpers::capture_validated_docs(&project_path) {
                    Ok(snapshot) => snapshot,
                    Err(reason) => {
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

            // KT-927 — the same per-step metrics the Full pipeline records, so
            // the recap and the run total of a partial refresh are not blank,
            // and an agent that reports no usage reads as unknown, not 0. The
            // row is opened only now: the two early exits above leave none.
            let step_started_at = std::time::Instant::now();
            {
                let run_id = audit_run_id.clone();
                let label = file_label.to_string();
                let started = Utc::now();
                if let Err(e) = state.db.with_conn(move |conn| {
                    crate::db::audit_runs::insert_audit_step_start(
                        conn, &run_id, step as u32, &label, started,
                    )
                }).await {
                    tracing::error!("Failed to persist partial audit step start for run {audit_run_id}: {e}");
                }
            }

            // KT-1357 — the section is as its last baseline recorded it and its
            // document is filled: there is nothing to refresh, so the step is
            // verified `unchanged` without an agent (a second refresh right after a
            // successful one used to rewrite the document again, apollo's glossary).
            if fresh_sections.contains(&format!("{}#step{}", analysis_step.target_file, step))
                && super::step_guard::read_target(&project_path, analysis_step.target_file)
                    .ok()
                    .flatten()
                    .is_some_and(|content| {
                        !content.trim().is_empty()
                            && !super::step_guard::has_partial_marker(&content)
                            && super::validation::count_raw_placeholders(&content) == 0
                    })
            {
                tracing::info!(
                    "Partial audit step {step} ({file_label}) unchanged: sources as recorded at the last audit"
                );
                unchanged_steps.push(step);
                yield Event::default().event("step_unchanged").data(
                    serde_json::json!({ "step": step, "file": file_label }).to_string(),
                );
                yield Event::default().event("step_done").data(
                    serde_json::json!({
                        "step": step,
                        "success": true,
                        "outcome": "unchanged",
                        "file": file_label,
                        "tokens": serde_json::Value::Null,
                        "duration_ms": step_started_at.elapsed().as_millis() as u64,
                        "total_tokens": run_tokens.total(),
                        "cost_usd_micros": serde_json::Value::Null,
                    }).to_string()
                );
                let run_id = audit_run_id.clone();
                let recorded_total = run_tokens.total();
                let duration_ms = step_started_at.elapsed().as_millis() as u64;
                let writer = run_state_writer.clone();
                if let Err(e) = state.db.with_conn(move |conn| {
                    let finalized = crate::db::audit_runs::finalize_audit_step_with_outcome(
                        conn,
                        &run_id,
                        step as u32,
                        Utc::now(),
                        duration_ms,
                        &crate::db::audit_runs::StepTokens::UNKNOWN,
                        recorded_total,
                        true,
                        None,
                        false,
                        true,
                    );
                    writer.sync(conn, false);
                    finalized?;
                    crate::db::audit_runs::delete_step_snapshot(conn, &run_id, step as u32)?;
                    crate::db::audit_runs::set_step_target_state(conn, &run_id, step as u32, None)
                }).await {
                    tracing::error!("Failed to finalize partial audit step {step} for run {audit_run_id}: {e}");
                }
                continue;
            }

            let detector_gated = analysis_step.target_file.ends_with("inconsistencies-tech-debt.md")
                && !detector_signals.is_empty();
            // The tech-debt step also owns the coverage matrix, which is given
            // back to the agent on a miss (KT-1357).
            let max_attempts = if enforce_mode
                || revalidation.is_some()
                || documentary_before.is_some()
                || detector_gated
                || analysis_step.target_file.ends_with("inconsistencies-tech-debt.md")
            {
                super::anti_hallu_enforce::MAX_ATTEMPTS
            } else {
                1
            };
            let mut attempt: usize = 0;
            // KT-1168 — the KT-1132 stale-target check: what no longer
            // resolves is named before the agent runs.
            let mut citation_feedback: Option<String> = if revalidation.is_some() {
                super::document_repair::with_stale_target(&project_path, analysis_step.target_file, Vec::new())
                    .ok()
                    .filter(|d| !d.is_empty())
                    .map(|d| super::document_repair::feedback(&d))
            } else {
                None
            };
            let mut step_cost = super::agent_launch::StepCost::default();
            'attempts: loop {
            attempt += 1;
            let mut step_usage = crate::db::audit_runs::StepTokens::UNKNOWN;
            let mut stream_turns = super::agent_launch::StreamTurns::default();
            let attempt_prompt = super::finish_step_prompt(
                &full_prompt, citation_feedback.as_deref(), step, analysis_step, &docs_root,
            );
            // Stop reaches an HTTP or ACP agent through this token and a direct
            // CLI agent through its PID — the same registration as the Full
            // pipeline, made BEFORE the start so a Stop during the handshake
            // or a cold model's first response still finds it.
            let attempt_cancel = agent_launcher.stops_with_token().then(tokio_util::sync::CancellationToken::new);
            if let Some(cancel) = &attempt_cancel {
                if let Ok(mut tracker) = audit_tracker.lock() {
                    tracker.agent_cancels.insert(project_id_for_progress.clone(), cancel.clone());
                }
            }
            let (activity_sink, activity_rx) = tokio::sync::watch::channel(None);
            match agent_launcher.start(
                &agent_type,
                audit_tier,
                &project_path,
                &project_path_str,
                &attempt_prompt,
                &tokens,
                attempt_cancel.clone(),
                analysis_step.target_file,
                Some(activity_sink),
            ).await {
                Ok(mut process) => {
                    // Only a direct CLI agent has a PID worth killing: the
                    // child of an HTTP or ACP run is a lifeline.
                    if attempt_cancel.is_none() {
                        if let Some(pid) = process.child.id() {
                            if let Ok(mut tracker) = audit_tracker.lock() {
                                tracker.running_pids.insert(project_id_for_progress.clone(), pid);
                            }
                            // KT-1211 (M5) — the group and the leader's start
                            // stamp, so a boot after a crash kills an agent the
                            // dead backend left rewriting the docs.
                            let stamp = tokio::task::spawn_blocking(move || {
                                crate::core::agent_orphans::start_stamp(pid)
                            }).await.ok().flatten();
                            let run_id = audit_run_id.clone();
                            if let Err(e) = state.db.with_conn(move |conn| {
                                crate::db::audit_runs::set_agent_process(conn, &run_id, pid as i64, stamp.as_deref())
                            }).await {
                                tracing::warn!("Could not record the partial audit agent's process group: {e}");
                            }
                        }
                    }
                    let is_stream_json = process.output_mode == runner::OutputMode::StreamJson;
                    // Same activity watch as the Full pipeline: a tool chain
                    // without prose still moves the tool chip and the tokens.
                    let mut activity = (!is_stream_json).then(|| {
                        super::agent_launch::StepActivityWatch::new(
                            super::agent_launch::AuditActivityProbe::new(
                                process.tool_activity_probe(),
                                Some(activity_rx.clone()),
                            ),
                            agent_type.clone(),
                        )
                    });
                    let mut recent = super::agent_launch::StepRecentFeed::new(
                        (!is_stream_json).then(|| process.tool_activity_probe()),
                    );
                    let mut activity_tick = tokio::time::interval(super::agent_launch::ACTIVITY_TICK);
                    activity_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    let mut stream_cost: Option<u64> = None;
                    loop {
                        let wake = tokio::select! {
                            maybe_line = process.next_line() => super::agent_launch::StepWake::Line(maybe_line),
                            _ = activity_tick.tick() => super::agent_launch::StepWake::Tick,
                        };
                        let line = match wake {
                            super::agent_launch::StepWake::Line(Some(line)) => line,
                            super::agent_launch::StepWake::Line(None) => break,
                            super::agent_launch::StepWake::Idle => continue,
                            super::agent_launch::StepWake::Tick => {
                                if let Some(snapshot) = recent.moved() {
                                    let payload = serde_json::json!({ "step": step, "recent": &snapshot });
                                    if let Ok(mut t) = audit_tracker.lock() {
                                        t.set_recent_activity(&project_id_for_progress, snapshot);
                                    }
                                    yield Event::default().event("activity").data(payload.to_string());
                                }
                                if let Some(seen) = target_watch.as_mut().and_then(|w| w.moved()) {
                                    let (run_id, step_n) = (audit_run_id.clone(), step as u32);
                                    if let Err(e) = state.db.with_conn(move |conn| {
                                        crate::db::audit_runs::record_observed_output(conn, &run_id, step_n, &seen)
                                    }).await {
                                        tracing::warn!("Could not record what partial step {step} wrote: {e}");
                                    }
                                }
                                let Some(watch) = activity.as_mut() else { continue };
                                if let Some((tool, calls)) = watch.tool_moved() {
                                    if let Ok(mut t) = audit_tracker.lock() {
                                        t.set_tool_activity(&project_id_for_progress, tool.clone(), calls);
                                    }
                                    yield Event::default().event("tool_call").data(
                                        serde_json::json!({ "step": step, "tool": tool, "calls": calls }).to_string()
                                    );
                                }
                                if let Some(reading) = watch.tokens_moved() {
                                    step_usage = reading;
                                    if let Some(step_tokens) = step_usage.total() {
                                        let cumulative = run_tokens.with(step_tokens);
                                        if let Ok(mut t) = audit_tracker.lock() {
                                            t.update_chips(&project_id_for_progress, Some(step_tokens), Some(cumulative), None);
                                            t.set_step_breakdown(&project_id_for_progress, step_usage.breakdown());
                                        }
                                        yield Event::default().event("step_progress").data(
                                            serde_json::json!({
                                                "step": step,
                                                "step_tokens": step_tokens,
                                                "total_tokens_so_far": cumulative,
                                                "breakdown": step_usage.breakdown(),
                                            }).to_string()
                                        );
                                    }
                                }
                                continue;
                            }
                        };
                        if is_stream_json {
                            let event = runner::parse_claude_stream_line(&line);
                            // The chip and its count, as in the Full pipeline:
                            // the tool's category only.
                            if let Some(category) = recent.on_stream_event(&event) {
                                let tool = category.as_str();
                                if let Ok(mut t) = audit_tracker.lock() {
                                    t.update_chips(&project_id_for_progress, None, None, Some(tool.to_owned()));
                                }
                                yield Event::default().event("tool_call").data(
                                    serde_json::json!({ "step": step, "tool": tool }).to_string()
                                );
                            }
                            let reported_cost = match &event {
                                runner::StreamJsonEvent::Usage { cost_usd, .. } => *cost_usd,
                                runner::StreamJsonEvent::TerminalError(failure) => failure.cost_usd,
                                _ => None,
                            };
                            if let Some(cost) = reported_cost.and_then(crate::agents::chat_codec::usd_to_micros) {
                                stream_cost = Some(cost);
                            }
                            if let runner::StreamJsonEvent::Usage { input_tokens, output_tokens, prompt_cache, .. } = event
                            {
                                // KT-1313 — a turn adds to the step; only the final
                                // line states the whole run, so a stopped step keeps
                                // every turn it consumed.
                                if input_tokens + output_tokens > 0 {
                                    let reading = crate::db::audit_runs::StepTokens {
                                        input: Some(input_tokens),
                                        output: Some(output_tokens),
                                        cache_read: prompt_cache.cached_prompt_tokens,
                                        cache_write: prompt_cache.cache_write_prompt_tokens,
                                        accounting: Default::default(),
                                    }.inclusive_for(&agent_type);
                                    step_usage = stream_turns.fold(step_usage, reading, runner::is_claude_result_line(&line));
                                }
                            }
                        }
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }
                    // An agent that streams text (ACP, HTTP) reports its usage on
                    // the process, often with the end of the turn: read it now.
                    if !is_stream_json {
                        let reading = crate::db::audit_runs::StepTokens::from_reported(
                            process.reported_usage_counters(),
                        ).inclusive_for(&agent_type);
                        if reading.total().is_some() {
                            step_usage = reading;
                        }
                    }
                    drop(activity);
                    // KT-997 — this attempt's cost, summed with the step's earlier ones.
                    step_cost = step_cost.with_attempt(process.reported_cost_usd_micros().or(stream_cost));

                    let status = process.child.wait().await;
                    process.fix_ownership();
                    if let Ok(mut tracker) = audit_tracker.lock() {
                        tracker.running_pids.remove(&project_id_for_progress);
                        tracker.agent_cancels.remove(&project_id_for_progress);
                    }
                    {
                        // KT-1211 (M5) — the agent is reaped: nothing to kill at boot.
                        let run_id = audit_run_id.clone();
                        if let Err(e) = state.db.with_conn(move |conn| {
                            crate::db::audit_runs::clear_agent_process(conn, &run_id)
                        }).await {
                            tracing::warn!("Could not clear the partial audit agent's process group: {e}");
                        }
                    }
                    run_tokens.add(step_usage.total());
                    tracing::debug!("Partial audit step {}: fix_ownership applied for {}", step, file_label);

                    // PER-ATTEMPT boundary: sanitize before validation can
                    // choose Retry and before another agent process can spawn.
                    match super::redact_artifacts::sanitize_all(
                        &project_path,
                        &redact_targets,
                        "partial-per-attempt",
                    ) {
                        Ok(r) => {
                            if !r.is_empty() {
                                tracing::warn!(target: "kronn::invariant", step, attempt,
                                    files = r.len(), "partial audit: redacted literals before validation/retry");
                            }
                        }
                        Err(e) => {
                            let run_id = audit_run_id.clone();
                            match state.db.with_conn(move |conn| crate::db::audit_runs::mark_failed(
                                conn, &run_id, "secret-redaction failed during partial audit",
                            )).await {
                                Ok(()) => drop_guard.disarm(),
                                Err(db_err) => tracing::error!("partial redaction failure could not mark run failed: {db_err}"),
                            }
                            if let Ok(mut t) = audit_tracker.lock() {
                                t.clear_progress(&project_id_for_progress);
                            }
                            yield Event::default().event("error").data(
                                serde_json::json!({
                                    "error": format!("secret-redaction failed after partial step {step} attempt {attempt}: {e}"),
                                    "fatal": true,
                                }).to_string(),
                            );
                            return;
                        }
                    }

                    // KT-1360 — the agent's commits are undone and its edits outside
                    // the docs root put back, before any gate reads the tree.
                    if let Some(remit) = &step_remit {
                        let report = remit.settle().await;
                        for event in super::step_remit::warning_events(step, file_label, &report) {
                            yield event;
                        }
                        remit_notes.extend(report.messages());
                    }
                    // The rules section belongs to Kronn: an agent rewrite never keeps it.
                    super::briefing_rules::sync_from_notes(&project_path, briefing_notes.as_deref());
                    // Every write of the attempt stays; the docs a human
                    // validated are marked to reconfirm (KT-1079).
                    if let Err(reason) = super::helpers::mark_validated_doc_writes(
                        &project_path,
                        &validated_docs_snapshot,
                        provenance_writer,
                    ) {
                        let reason = format!(
                            "documentation protection notice for {file_label}: {reason}"
                        );
                        yield Event::default().event("step_warning").data(
                            serde_json::json!({
                                "step": step, "file": file_label,
                                "reason": reason, "repaired_from_template": false,
                            }).to_string()
                        );
                    }

                    // KT-927 — a Stop during the step. Checked AFTER the
                    // sweeps above, like the Full pipeline, so the attempt's
                    // rewrite is already redacted and the validated docs
                    // marked; BEFORE any gate or retry, so a stopped step
                    // neither counts as a failure nor spawns another agent.
                    if audit_tracker.lock().map(|t| t.cancelled.contains(&project_id_for_progress)).unwrap_or(false) {
                        // KT-1211 — the agent was killed mid-write: the target
                        // goes back as it was before the step, never left
                        // half-written (marked partial when it cannot be put back).
                        let step_n = step as u32;
                        let outcome = super::step_guard::settle_killed(
                            &project_path,
                            analysis_step.target_file,
                            pre_content.as_deref(),
                            target_watch.as_ref().map(|w| w.last_seen()),
                            super::step_guard::StopKind::Cancelled,
                            step_n,
                        );
                        {
                            let run_id = audit_run_id.clone();
                            let reason = super::step_guard::killed_reason(step_n, &outcome);
                            let duration_ms = step_started_at.elapsed().as_millis() as u64;
                            let recorded_total = run_tokens.total();
                            let usage = step_usage;
                            let killed_cost = step_cost.usd_micros();
                            let killed_state = outcome.target_state();
                            let keep_snapshot = outcome.keeps_snapshot();
                            if let Err(e) = state.db.with_conn(move |conn| {
                                crate::db::audit_runs::finalize_audit_step(
                                    conn, &run_id, step_n, Utc::now(), duration_ms, &usage,
                                    recorded_total, false, Some(&reason), false,
                                )?;
                                if let Some(cost) = killed_cost {
                                    crate::db::audit_runs::set_step_cost(conn, &run_id, step_n, cost)?;
                                }
                                crate::db::audit_runs::set_step_target_state(conn, &run_id, step_n, killed_state)?;
                                if !keep_snapshot {
                                    crate::db::audit_runs::delete_step_snapshot(conn, &run_id, step_n)?;
                                }
                                Ok(())
                            }).await {
                                tracing::error!("Failed to record the cancelled partial step {step} for run {audit_run_id}: {e}");
                            }
                        }
                        let run_id = audit_run_id.clone();
                        let persisted = state.db.with_conn(move |conn| {
                            crate::db::audit_runs::mark_cancelled_at_step(conn, &run_id, step_n)
                                .map(|_| ())
                        }).await;
                        match persisted {
                            Ok(()) => drop_guard.disarm(),
                            Err(e) => tracing::error!(
                                "mark_cancelled failed for {audit_run_id}: {e} — leaving the drop-guard armed"
                            ),
                        }
                        if super::revalidate::restore_validation_if_untouched(&project_path, validated_before.as_deref(), &target_snapshots, restore_reason) {
                            tracing::info!("Partial run stopped before rewriting anything: validation restored");
                        }
                        if let Ok(mut t) = audit_tracker.lock() {
                            t.clear_progress(&project_id_for_progress);
                            t.cancelled.remove(&project_id_for_progress);
                        }
                        yield Event::default().event("cancelled").data(
                            serde_json::json!({ "status": "cancelled" }).to_string()
                        );
                        return;
                    }

                    let cli_success = status.map(|s| s.success()).unwrap_or(false);
                    // Gate order = Full parity (matrix v2): CLI → semantic
                    // validator → step-8 disposition → enforce lint → the
                    // rewrite-proof snapshot LAST.
                    if cli_success {
                        crate::api::audit::validation::fill_step_owned_placeholders(
                            &project_path,
                            analysis_step.target_file,
                            &chrono::Utc::now().format("%Y-%m-%d").to_string(),
                        );
                        // KT-1364 — Full parity: an outcome and its evidence merged
                        // in one cell is split here, not sent back once per row.
                        if super::validation::repair_coverage_rows(
                            &project_path, analysis_step.target_file,
                        ) {
                            tracing::info!("Partial audit step {step} ({file_label}): split merged coverage matrix rows");
                        }
                    }
                    let (mut success, validation_warning) =
                        super::validation::validate_step_output(
                            cli_success, &project_path, analysis_step.target_file,
                        );
                    // KT-1357 — Full parity: a model that finished normally with an
                    // incomplete coverage matrix gets that exact failure back within
                    // the attempt budget (apollo's revalidation lost step 8 to its
                    // first attempt). Never after a provider failure.
                    if cli_success && !success && attempt < max_attempts {
                        if let Some(feedback) = super::validation::coverage_repair_feedback(
                            &project_path, analysis_step.target_file,
                        ) {
                            yield Event::default().event("step_retry").data(
                                serde_json::json!({
                                    "step": step, "file": file_label,
                                    "attempt": attempt, "max_attempts": max_attempts,
                                    "reason": "dimension_coverage",
                                    "diagnostic": validation_warning.as_ref().map(|w| &w.reason),
                                }).to_string()
                            );
                            citation_feedback = Some(feedback);
                            continue 'attempts;
                        }
                    }
                    let validation_warning = super::validation::with_http_diagnostics(
                        success, validation_warning, &process.captured_stderr(),
                    );
                    if let Some(w) = validation_warning {
                        let ev = serde_json::json!({
                            "step": step, "file": file_label,
                            "reason": w.reason, "repaired_from_template": w.repaired,
                        });
                        yield Event::default().event("step_warning").data(ev.to_string());
                    }

                    if success && analysis_step.target_file.ends_with("inconsistencies-tech-debt.md") {
                        if let Some(gap) = super::validation::detector_disposition_gap(
                            &project_path, &detector_signals,
                        ) {
                            if attempt < max_attempts {
                                yield Event::default().event("step_retry").data(
                                    serde_json::json!({
                                        "step": step, "file": file_label,
                                        "attempt": attempt, "max_attempts": max_attempts,
                                        "undisposed_count": gap.count,
                                        "reason": "detector_disposition",
                                    }).to_string()
                                );
                                citation_feedback = Some(gap.feedback);
                                continue 'attempts;
                            }
                            // KT-1357 — the attempts are spent: Kronn lists what is
                            // left for a human to triage instead of failing the step.
                            if let Some(listed) = super::validation::settle_unaddressed_signals(
                                &project_path, &detector_signals,
                            ) {
                                tracing::warn!(
                                    "Partial audit step {step} ({file_label}): {listed} detector signal(s) listed for triage"
                                );
                                yield Event::default().event("warning").data(
                                    serde_json::json!({
                                        "message": format!(
                                            "Step {step} ({file_label}): {listed} detector signal(s) were left unaddressed by the agent; Kronn listed them under \"Unaddressed detector signals\" for triage"
                                        ),
                                    }).to_string()
                                );
                            } else {
                                success = false;
                                yield Event::default().event("step_warning").data(
                                    serde_json::json!({
                                        "step": step, "file": file_label,
                                        "reason": gap.warning.reason, "repaired_from_template": false,
                                    }).to_string()
                                );
                            }
                        }
                    }

                    {
                        use super::anti_hallu_enforce::EnforceGateOutcome;
                        match super::anti_hallu_enforce::evaluate_enforce_gate(
                            enforce_mode, success, analysis_step.target_file,
                            &project_path, attempt, max_attempts, version_ctx.snapshot.as_ref(),
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
                                // KT-1370 — the last attempt is spent: when the
                                // documentary gate sees only citation markers on the
                                // target, they are demoted to flagged backticked paths
                                // and the step goes on; anything else fails it as before.
                                let target_holds = || matches!(
                                    super::anti_hallu_enforce::evaluate_enforce_gate(
                                        enforce_mode, true, analysis_step.target_file,
                                        &project_path, attempt, max_attempts, version_ctx.snapshot.as_ref(),
                                    ),
                                    EnforceGateOutcome::NotApplicable | EnforceGateOutcome::Pass { .. }
                                );
                                let demoted = super::document_repair::with_stale_target(&project_path, analysis_step.target_file, Vec::new())
                                    .and_then(|left| super::document_repair::demote_last_attempt(&project_path, &left, None, &target_holds))
                                    .unwrap_or_else(|e| {
                                        tracing::warn!("Partial audit step {step} ({file_label}): demotion failed: {e}");
                                        Vec::new()
                                    });
                                if demoted.is_empty() {
                                    success = false;
                                    yield Event::default().event("step_warning").data(
                                        serde_json::json!({
                                            "step": step, "file": file_label,
                                            "reason": reason, "repaired_from_template": false,
                                        }).to_string()
                                    );
                                } else {
                                    yield Event::default().event("documentary_demoted").data(
                                        serde_json::json!({
                                            "step": step, "file": file_label,
                                            "attempt": attempt, "references": demoted,
                                        }).to_string()
                                    );
                                    unverified_references.extend(demoted.iter().cloned().map(|r| (step as u32, r)));
                                    step_demotions = demoted;
                                }
                            }
                            EnforceGateOutcome::Pass { .. } => {
                                // No mutation here (Codex msg 150): stamping
                                // happens only after `succeeded` is proven.
                            }
                        }
                    }

                    // KT-1168 — a revalidated target leaves nothing stale:
                    // the documentary blockers still on it retry the step.
                    if success && revalidation.is_some() {
                        match super::document_repair::with_stale_target(&project_path, analysis_step.target_file, Vec::new()) {
                            Ok(left) if !left.is_empty() => {
                                if attempt < max_attempts {
                                    yield Event::default().event("step_retry").data(
                                        serde_json::json!({
                                            "step": step, "file": file_label,
                                            "attempt": attempt, "max_attempts": max_attempts,
                                            "reason": "revalidation_stale_target",
                                        }).to_string()
                                    );
                                    citation_feedback = Some(super::document_repair::feedback(&left));
                                    continue 'attempts;
                                }
                                // KT-1370 — as above, once the attempts are spent.
                                let target_holds = || matches!(
                                    super::anti_hallu_enforce::evaluate_enforce_gate(
                                        enforce_mode, true, analysis_step.target_file,
                                        &project_path, attempt, max_attempts, version_ctx.snapshot.as_ref(),
                                    ),
                                    super::anti_hallu_enforce::EnforceGateOutcome::NotApplicable
                                        | super::anti_hallu_enforce::EnforceGateOutcome::Pass { .. }
                                );
                                let demoted = super::document_repair::demote_last_attempt(&project_path, &left, None, &target_holds)
                                    .unwrap_or_else(|e| {
                                        tracing::warn!("Revalidation step {step} ({file_label}): demotion failed: {e}");
                                        Vec::new()
                                    });
                                if !demoted.is_empty() {
                                    yield Event::default().event("documentary_demoted").data(
                                        serde_json::json!({
                                            "step": step, "file": file_label,
                                            "attempt": attempt, "references": demoted,
                                        }).to_string()
                                    );
                                    unverified_references.extend(demoted.iter().cloned().map(|r| (step as u32, r)));
                                    step_demotions = demoted;
                                } else {
                                success = false;
                                yield Event::default().event("step_warning").data(
                                    serde_json::json!({
                                        "step": step, "file": file_label,
                                        "reason": format!("{} stale reference(s) left after revalidation", left.len()),
                                        "repaired_from_template": false,
                                    }).to_string()
                                );
                                }
                            }
                            Ok(_) => {}
                            Err(e) => tracing::warn!("Revalidation step {step} ({file_label}): stale check skipped: {e}"),
                        }
                    }

                    // KT-1388 — Full parity: advice, not a blocker. A stack
                    // conflict or an unproven closure in what the step wrote is
                    // handed back while it can still correct; the last attempt
                    // leaves it as a step warning and a documentary note, and
                    // the step stays done.
                    if success {
                        if let Some(before) = &documentary_before {
                            match super::document_repair::advisories_since(&project_path, before, analysis_step.target_file) {
                                Ok(advisory) if !advisory.is_empty() && attempt < max_attempts => {
                                    citation_feedback = Some(super::document_repair::advice(&advisory));
                                    yield Event::default().event("step_retry").data(
                                        serde_json::json!({
                                            "step": step, "file": file_label,
                                            "attempt": attempt, "max_attempts": max_attempts,
                                            "reason": "documentary_advisories",
                                            "advisory_count": advisory.len(),
                                            "diagnostics": advisory,
                                        }).to_string()
                                    );
                                    continue 'attempts;
                                }
                                Ok(advisory) if !advisory.is_empty() => {
                                    remit_notes.extend(advisory.iter().map(|d| {
                                        format!("Unverified: `{}` — {}", d.path, d.message)
                                    }));
                                    yield Event::default().event("documentary_advisory").data(
                                        serde_json::json!({
                                            "step": step, "file": file_label,
                                            "diagnostics": advisory,
                                        }).to_string()
                                    );
                                    for diagnostic in advisory {
                                        if !documentary_advisories.contains(&diagnostic) {
                                            documentary_advisories.push(diagnostic);
                                        }
                                    }
                                }
                                Ok(_) => {}
                                Err(e) => tracing::warn!("Partial audit step {step} ({file_label}): documentary advice skipped: {e}"),
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
                                let target_path = project_path.join(analysis_step.target_file);
                                // Substantial rewrite PROVEN — only now may
                                // enforce stamp the curated audit dates. The
                                // step already succeeded, so any stamp failure
                                // (read or write) is a NON-terminal `warning`
                                // — the partial dispatcher shows it, never
                                // swallows it.
                                if enforce_mode {
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

                    let step_succeeded = success && outcome == "succeeded";
                    // KT-1211 — a step that failed never leaves an unmarked
                    // half-written doc: what the agent wrote stays under a
                    // visible partial marker; a success drops an earlier one.
                    let mut target_state: Option<&'static str> = None;
                    if step_succeeded {
                        if let Err(e) = super::step_guard::clear_partial_marker(&project_path, analysis_step.target_file) {
                            tracing::warn!("Partial audit step {step} ({file_label}): {e}");
                        }
                    } else if outcome == "failed" {
                        match super::step_guard::settle_failed_target(
                            &project_path, analysis_step.target_file, &pre_snapshot, step as u32,
                            "the step did not pass its checks",
                        ) {
                            Ok(state) => target_state = state,
                            Err(e) => tracing::warn!("Partial audit step {step} ({file_label}): {e}"),
                        }
                    }
                    let duration_ms = step_started_at.elapsed().as_millis() as u64;
                    if let (Some(step_tokens), Ok(mut t)) = (step_usage.total(), audit_tracker.lock()) {
                        t.update_chips(&project_id_for_progress, Some(step_tokens), run_tokens.total(), None);
                    }
                    let step_done = serde_json::json!({
                        "step": step,
                        "success": step_succeeded || outcome == "unchanged",
                        "outcome": outcome,
                        "file": file_label,
                        // `null` — not 0 — when the agent reported no usage.
                        "tokens": step_usage.total(),
                        "duration_ms": duration_ms,
                        "total_tokens": run_tokens.total(),
                        // `null` — not 0 — when an attempt's agent reported no cost.
                        "cost_usd_micros": step_cost.usd_micros(),
                        "remit": remit_notes,
                        "unverified_references": step_demotions.len(),
                    });
                    yield Event::default().event("step_done").data(step_done.to_string());
                    if !step_demotions.is_empty() {
                        // KT-1370 — the timeline's notes block reads them from the run.
                        let run_id = audit_run_id.clone();
                        let added: Vec<_> = step_demotions.iter().cloned().map(|r| (step as u32, r)).collect();
                        if let Err(error) = state.db.with_conn(move |conn| {
                            let existing = crate::db::audit_runs::get_by_id(conn, &run_id)?
                                .and_then(|run| run.documentary_notes);
                            match super::document_repair::merge_unverified_notes(existing.as_deref(), &added) {
                                Some(notes) => crate::db::audit_runs::set_documentary_notes(conn, &run_id, &notes),
                                None => Ok(()),
                            }
                        }).await {
                            tracing::warn!("Unverified references of {audit_run_id} not recorded: {error}");
                        }
                    }
                    {
                        let run_id = audit_run_id.clone();
                        let remit_warning = {
                            let mut lines = remit_notes.clone();
                            lines.extend(super::document_repair::demotion_notes(&step_demotions));
                            (!lines.is_empty()).then(|| lines.join("\n"))
                        };
                        let recorded_usage = step_usage;
                        let recorded_total = run_tokens.total();
                        let step_unchanged = outcome == "unchanged";
                        let writer = run_state_writer.clone();
                        if let Err(e) = state.db.with_conn(move |conn| {
                            let finalized = crate::db::audit_runs::finalize_audit_step_with_outcome(
                                conn,
                                &run_id,
                                step as u32,
                                Utc::now(),
                                duration_ms,
                                &recorded_usage,
                                recorded_total,
                                // An `unchanged` step is done, as in the Full
                                // pipeline: a resume carries it only so (KT-1357).
                                step_succeeded || step_unchanged,
                                remit_warning.as_deref(),
                                false,
                                step_unchanged,
                            );
                            writer.sync(conn, false);
                            finalized?;
                            // A done step has nothing left to restore; a failed
                            // one keeps its snapshot (acceptance diff).
                            if step_succeeded || step_unchanged {
                                crate::db::audit_runs::delete_step_snapshot(conn, &run_id, step as u32)?;
                            }
                            crate::db::audit_runs::set_step_target_state(conn, &run_id, step as u32, target_state)
                        }).await {
                            tracing::error!("Failed to finalize partial audit step {step} for run {audit_run_id}: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Partial audit step {} failed to start: {}", step, e);
                    if let Ok(mut tracker) = audit_tracker.lock() {
                        tracker.agent_cancels.remove(&project_id_for_progress);
                    }
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
                            "cost_usd_micros": step_cost.usd_micros(),
                        }).to_string()
                    );
                    {
                        let run_id = audit_run_id.clone();
                        let duration_ms = step_started_at.elapsed().as_millis() as u64;
                        let recorded_total = run_tokens.total();
                        let writer = run_state_writer.clone();
                        if let Err(e) = state.db.with_conn(move |conn| {
                            let finalized = crate::db::audit_runs::finalize_audit_step(
                                conn,
                                &run_id,
                                step as u32,
                                Utc::now(),
                                duration_ms,
                                &crate::db::audit_runs::StepTokens::UNKNOWN,
                                recorded_total,
                                false,
                                None,
                                false,
                            );
                            writer.sync(conn, false);
                            finalized
                        }).await {
                            tracing::error!("Failed to finalize partial audit step {step} for run {audit_run_id}: {e}");
                        }
                    }
                }
            }
            // KT-997 — the step's cost and the run's model, once the row is finalized.
            if let Some(cost) = step_cost.usd_micros() {
                let run_id = audit_run_id.clone();
                if let Err(e) = state.db.with_conn(move |conn| {
                    crate::db::audit_runs::set_step_cost(conn, &run_id, step as u32, cost)
                }).await {
                    tracing::error!("Failed to record the cost of partial audit step {step}: {e}");
                }
            }
            if let Some(model) = agent_launcher.run_model(&agent_type, audit_tier).await {
                if recorded_model.as_deref() != Some(model.as_str()) {
                    let (run_id, value) = (audit_run_id.clone(), model.clone());
                    match state.db.with_conn(move |conn| crate::db::audit_runs::set_run_model(conn, &run_id, &value)).await {
                        Ok(()) => recorded_model = Some(model),
                        Err(e) => tracing::warn!("Could not record the partial audit run's model: {e}"),
                    }
                }
            }
            break 'attempts;
            } // 'attempts
            if let Err(reason) = super::helpers::mark_validated_doc_writes(
                &project_path, &validated_docs_snapshot, provenance_writer,
            ) {
                yield Event::default().event("step_warning").data(serde_json::json!({
                    "step": step, "file": file_label, "reason": reason, "repaired_from_template": false,
                }).to_string());
            }
        }

        // PRE-PUBLICATION backstop. Baseline merge, TD extraction and
        // validation-discussion creation below must only observe clean data.
        match super::redact_artifacts::sanitize_all(
            &project_path,
            &redact_targets,
            "partial-post-loop",
        ) {
            Ok(r) => {
                if !r.is_empty() {
                    tracing::warn!(target: "kronn::invariant", files = r.len(),
                        "partial audit: redacted literals before publication");
                }
            }
            Err(e) => {
                let run_id = audit_run_id.clone();
                match state.db.with_conn(move |conn| crate::db::audit_runs::mark_failed(
                    conn, &run_id, "secret-redaction failed before partial audit publication",
                )).await {
                    Ok(()) => drop_guard.disarm(),
                    Err(db_err) => tracing::error!("partial redaction failure could not mark run failed: {db_err}"),
                }
                if let Ok(mut t) = audit_tracker.lock() {
                    t.clear_progress(&project_id_for_progress);
                }
                yield Event::default().event("error").data(
                    serde_json::json!({
                        "error": format!("secret-redaction failed before partial audit publication: {e}"),
                        "fatal": true,
                    }).to_string(),
                );
                return;
            }
        }

        // Compute the refreshed baseline — ONLY for done steps — but do NOT
        // write it yet (Codex lot-2 #4): the file write happens strictly
        // AFTER the terminal DB transaction commits, so a terminal failure
        // can never leave drift green while the run is reconciled (the
        // retry would lose its scope). A failed baseline write after the
        // commit is the conservative direction: sections merely stay stale.
        let done_steps: Vec<usize> = requested_steps.iter()
            .filter(|s| succeeded_steps.contains(s) || unchanged_steps.contains(s))
            .copied().collect();
        let merged_baseline: Option<Vec<crate::core::checksums::ChecksumMapping>> = if !done_steps.is_empty() {
            let pp = project_path.clone();
            let steps_clone = done_steps.clone();
            let chain_for_merge = chain.clone();
            let merge_res = tokio::task::spawn_blocking(move || {
                compute_merged_baseline(&pp, &steps_clone, &chain_for_merge)
            }).await
                // A JoinError must NOT silently drop the done steps (the tx
                // would persist a terminal the SSE lists contradict).
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

        // Nothing rewritten: the docs are still the validated ones, whatever
        // the run's status, so the validation comes back before the commit.
        let validation_restored = succeeded_steps.is_empty()
            && super::revalidate::restore_validation_if_untouched(&project_path, validated_before.as_deref(), &target_snapshots, restore_reason);
        if validation_restored {
            tracing::info!("Partial run rewrote nothing: validation restored");
        }

        // ── Terminal write (Codex A5 v3): authoritative + atomic, same
        // contract as the full pipeline. The done steps get their SCOPED
        // validation discussion in the same transaction that stamps the
        // status + links it + persists the structured outcomes; a project
        // whose validation came back needs none. A failed write = terminal
        // `error` + return with the guard armed — never a `done`.
        let failed_steps: Vec<usize> = requested_steps.iter()
            .filter(|s| !succeeded_steps.contains(s) && !unchanged_steps.contains(s))
            .copied().collect();
        let refreshed_files: Vec<String> = succeeded_steps.iter()
            .map(|&n| chain[n - 1].target_file.to_string())
            .collect();
        let scope_files: Vec<String> = done_steps.iter()
            .map(|&n| chain[n - 1].target_file.to_string())
            .collect();
        let run_td_ids: Vec<String> = {
            let pp = project_path.clone();
            let idx: Vec<String> = refreshed_files.iter()
                .filter(|f| f.contains("inconsistencies-")).cloned().collect();
            let baseline = adr_baseline.clone();
            tokio::task::spawn_blocking(move || {
                // A trusted ADR settles its sheets before the scope is read.
                let trust = baseline.trust();
                let docs = crate::core::docs_location::DocsRoot::of(&pp).dir(&pp);
                if let Err(e) = crate::core::adr::settle_covered_sheets(&docs, trust) {
                    tracing::warn!("ADR check after the refresh failed: {e}");
                }
                super::reconciliation::td_validation_scope(&pp, &idx).undecided
            }).await.unwrap_or_default()
        };
        // KT-1388 — what the steps left as advice is noted on the run, as the
        // Full pipeline's final gate does.
        if !documentary_advisories.is_empty() {
            let run_id = audit_run_id.clone();
            let advisories = documentary_advisories.clone();
            if let Err(error) = state.db.with_conn(move |conn| {
                let existing = crate::db::audit_runs::get_by_id(conn, &run_id)?
                    .and_then(|run| run.documentary_notes);
                match super::document_repair::merge_advisory_notes(existing.as_deref(), &advisories) {
                    Some(notes) => crate::db::audit_runs::set_documentary_notes(conn, &run_id, &notes),
                    None => Ok(()),
                }
            }).await {
                tracing::warn!("Documentary notes of {audit_run_id} not recorded: {error}");
            }
        }
        // Exact partition (matrix v2): requested = succeeded ⊎ unchanged ⊎ failed.
        let outcomes_json = serde_json::json!({
            "requested": requested_steps,
            "succeeded": succeeded_steps,
            "unchanged": unchanged_steps,
            "failed": failed_steps,
        }).to_string();

        let pending_validation: Option<(Discussion, DiscussionMessage)> = if !scope_files.is_empty() && !validation_restored {
            let language = { state.config.read().await.language.clone() };
            let total_chain = chain.len() as u32;
            let listed = |steps: &[usize]| -> Vec<(u32, String)> {
                steps.iter().map(|&n| (n as u32, chain[n - 1].target_file.to_string())).collect()
            };
            let prompt = format!(
                "{}{}{}{}",
                super::helpers::documentary_notes_block(&[], &documentary_advisories, &[], &unverified_references, &language),
                super::helpers::unchanged_steps_block(&listed(&unchanged_steps), total_chain, &language),
                super::helpers::partial_refresh_failed_block(&listed(&failed_steps), total_chain, &language),
                super::helpers::build_partial_validation_prompt(&scope_files, &run_td_ids, &language),
            );
            let prompt = docs_root.relocate(&prompt).into_owned();
            let now = Utc::now();
            let disc_id = uuid::Uuid::new_v4().to_string();
            let msg = DiscussionMessage {
                recovered_partial: false,
                session_tokens_at_message: None,
                author_cli_ordinal: None,
                session_continuity: None,
                model: None, lint_report: None,
                id: uuid::Uuid::new_v4().to_string(),
                role: MessageRole::User,
                channel: MessageChannel::Main,
                content: prompt,
                agent_type: None,
                timestamp: now, tokens_used: 0, auth_mode: None,
                model_tier: None, cost_usd: None, author_pseudo: None,
                author_avatar_email: None, source_msg_id: None, duration_ms: None,
                target_agent: None,
        reply_to_message_id: None,
            };
            let disc = Discussion {
                connection_id: None,
                awaiting_agent: false,
                agent_running: false,
                id: disc_id,
                project_id: Some(project_id_for_progress.clone()),
                title: format!("Validation audit partiel ({} section{})",
                    scope_files.len(), if scope_files.len() > 1 { "s" } else { "" }),
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
                summary_strategy: crate::models::SummaryStrategy::OnDemand,
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

        let terminal = partial_terminal(
            failed_steps.clone(),
            !succeeded_steps.is_empty(),
            pending_validation.clone(),
        );
        let completed = !matches!(terminal, PartialTerminal::Interrupted { .. });
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

        // KT-1199 — the terminal state, with the documents as they now stand.
        {
            let writer = run_state_writer.clone();
            let _ = state.db.with_conn(move |conn| { writer.sync(conn, true); Ok(()) }).await;
        }

        // KT-1122 — who opened this partial run's validation. Best-effort:
        // never fails a run whose terminal status is already committed.
        if pending_validation.is_some() {
            let by = super::validate::operator_pseudo(&state).await;
            let run_for_by = audit_run_id.clone();
            if let Err(e) = state
                .db
                .with_conn(move |conn| {
                    crate::db::audit_validation::set_started_by(conn, &run_for_by, &by)
                })
                .await
            {
                tracing::warn!("Validation starter not recorded: {e}");
            }
        }

        // Baseline write STRICTLY after the committed terminal status
        // (Codex lot-2 #4): a DB failure above never leaves drift green,
        // and a write failure here is the conservative direction (the
        // sections merely stay stale).
        if let Some(merged) = merged_baseline.clone() {
            let pp = project_path.clone();
            let rewritten = refreshed_files.clone();
            let write_res = tokio::task::spawn_blocking(move || -> Result<(), String> {
                crate::core::checksums::write_checksums_file(&pp, &merged, &rewritten)?;
                if completed {
                    if let Err(e) = crate::core::kronn_state::record_audit(&pp, "partial") {
                        tracing::warn!("Failed to record partial audit in .kronn.json: {e}");
                    }
                }
                Ok(())
            }).await;
            let outcome: Result<(), String> = match write_res {
                Ok(r) => r,
                Err(e) => Err(format!("baseline write task failed: {e}")),
            };
            if let Err(msg) = outcome {
                tracing::error!("Baseline write failed after commit: {msg}");
                // Post-commit: the terminal status IS persisted — a
                // fatal-looking event here would contradict the `done` that
                // follows. Non-terminal warning; sections merely stay stale.
                yield Event::default().event("warning").data(
                    serde_json::json!({
                        "message": format!("Baseline write failed: {msg} — the refreshed sections will still show as stale; re-run the update to rewrite the baseline.")
                    }).to_string(),
                );
            }
        }

        // A normal refresh accepts a gated unchanged step as re-verified.
        // Revalidation keeps its explicit human acknowledgement for unchanged
        // citations; only its rewritten targets receive new fingerprints.
        let citation_docs: Vec<String> = if revalidation.is_some() {
            refreshed_files.clone()
        } else {
            unchanged_steps.iter().map(|&n| chain[n - 1].target_file.to_string()).collect()
        };
        if !citation_docs.is_empty() {
            let reason = if revalidation.is_some() {
                crate::core::citation_drift::RecordReason::Revalidation
            } else {
                crate::core::citation_drift::RecordReason::Audit
            };
            let pp = project_path.clone();
            let recorded = tokio::task::spawn_blocking(move || {
                crate::core::citation_drift::record_docs(&pp, &citation_docs, reason)
            }).await.unwrap_or_else(|e| Err(format!("record task failed: {e}")));
            if let Err(e) = recorded {
                yield Event::default().event("warning").data(
                    serde_json::json!({ "message": format!("Citation fingerprints not refreshed: {e}") }).to_string(),
                );
            }
        }

        // KT-1292 — a doc sent only to be re-read is stamped once its step ran,
        // whether or not the agent anchored it: the plan does not queue it again
        // until the doc or the step's sources change.
        if let Some(run) = &revalidation {
            let reread: Vec<(usize, String)> = run.rereads.iter()
                .filter(|(step, _)| succeeded_steps.contains(step) || unchanged_steps.contains(step))
                .cloned()
                .collect();
            if !reread.is_empty() {
                let pp = project_path.clone();
                let steps = chain.clone();
                let stamped = tokio::task::spawn_blocking(move || {
                    super::revalidate::stamp_rereads(&pp, &steps, &reread)
                }).await.unwrap_or_else(|e| Err(format!("review task failed: {e}")));
                if let Err(e) = stamped {
                    yield Event::default().event("warning").data(
                        serde_json::json!({ "message": format!("Re-read stamps not recorded: {e}") }).to_string(),
                    );
                }
            }
        }

        if let Some(event) = super::td_tasks::guard_decisions(&project_path, td_decisions_before).await {
            yield event;
        }
        super::td_tasks::close_after_audit(&state.db, &project_id_for_progress, &project_path, &audit_run_id).await;

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
            "revalidation": revalidation.is_some(),
            "cross_agent_skipped": cross_agent_skipped,
            "unverified_references": unverified_references.len(),
        });
        drop_guard.disarm();
        yield Event::default().event("done").data(done.to_string());
    });

    detached_partial_audit_response(stream)
}

/// `doc#stepN` of every section built from the whole source tree, when that tree
/// is exactly the one the last baseline recorded, no cited line is stale and the
/// prose names no path that is gone (KT-1357): the sections a refresh has nothing
/// to do for.
fn fresh_section_keys(project: &std::path::Path) -> std::collections::HashSet<String> {
    crate::core::checksums::unchanged_tree_sections(project)
        .into_iter()
        .filter(|key| {
            let doc = key
                .rsplit_once("#step")
                .map_or(key.as_str(), |(doc, _)| doc);
            crate::core::doc_relevance::vanished_paths(project, doc).is_empty()
        })
        .collect()
}

fn detached_partial_audit_response(stream: SseStream) -> Sse<SseStream> {
    Sse::new(super::detach_sse_stream(stream))
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
    let fresh_mappings: Vec<crate::core::checksums::ChecksumMapping> = succeeded_steps
        .iter()
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
    let refreshed: std::collections::HashSet<&str> =
        fresh_mappings.iter().map(|m| m.ai_file.as_str()).collect();
    let mut merged: Vec<crate::core::checksums::ChecksumMapping> =
        crate::core::checksums::read_checksums_file_strict(project_path)?
            .map(|f| {
                f.mappings
                    .into_iter()
                    .filter(|m| !refreshed.contains(m.ai_file.as_str()))
                    .collect()
            })
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
        PartialTerminal::Reverified { .. } => "no_change",
    }
}

type PendingValidation = (Discussion, DiscussionMessage);

/// The terminal branch for a run's partition. `validation` is the scoped
/// discussion of the done steps, absent when their validation came back.
pub(crate) fn partial_terminal(
    failed_steps: Vec<usize>,
    rewrote: bool,
    validation: Option<PendingValidation>,
) -> PartialTerminal {
    let validation = validation.map(Box::new);
    if !failed_steps.is_empty() {
        return PartialTerminal::Interrupted {
            failed_steps,
            validation,
        };
    }
    match validation {
        Some(pending) if rewrote => PartialTerminal::Success(pending),
        validation => PartialTerminal::Reverified { validation },
    }
}

/// Terminal branch of a partial run (Codex lot-3 matrix v2, KT-1207).
pub(crate) enum PartialTerminal {
    /// failed == 0 && succeeded >= 1 — validation discussion SCOPED to the
    /// done sections, Completed + link, all in one commit.
    Success(Box<PendingValidation>),
    /// failed > 0 — Interrupted; the done steps, if any, get their scoped
    /// discussion (KT-931 convention). Validate still refuses the run.
    Interrupted {
        failed_steps: Vec<usize>,
        validation: Option<Box<PendingValidation>>,
    },
    /// failed == 0 && succeeded == 0 — every section re-verified unchanged:
    /// Completed. A discussion only when the validation did not come back.
    /// The SSE layer reports `no_change`.
    Reverified {
        validation: Option<Box<PendingValidation>>,
    },
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
                complete_partial_run(&tx, &run_id, &project_path)?;
                crate::db::audit_runs::set_validation_discussion(&tx, &run_id, &disc.id)?;
            }
            PartialTerminal::Interrupted {
                failed_steps,
                validation,
            } => {
                crate::db::audit_runs::mark_interrupted_strict(
                    &tx,
                    &run_id,
                    &format!("partial refresh incomplete (failed steps: {failed_steps:?})"),
                )?;
                if let Some(pending) = validation {
                    let (disc, msg) = pending.as_ref();
                    crate::db::discussions::insert_discussion(&tx, disc)?;
                    crate::db::discussions::insert_message(&tx, &disc.id, msg)?;
                    crate::db::audit_runs::set_validation_discussion(&tx, &run_id, &disc.id)?;
                }
            }
            PartialTerminal::Reverified { validation, .. } => {
                if let Some(pending) = validation {
                    let (disc, msg) = pending.as_ref();
                    crate::db::discussions::insert_discussion(&tx, disc)?;
                    crate::db::discussions::insert_message(&tx, &disc.id, msg)?;
                }
                complete_partial_run(&tx, &run_id, &project_path)?;
                if let Some(pending) = validation {
                    crate::db::audit_runs::set_validation_discussion(&tx, &run_id, &pending.0.id)?;
                }
            }
        }
        crate::db::audit_runs::set_step_outcomes(&tx, &run_id, &outcomes_json)?;
        tx.commit()?;
        Ok(())
    })
    .await
    .map_err(|e| format!("terminal write failed: {e}"))
}

/// Stamps a Running partial row Completed with the TD counts on disk.
fn complete_partial_run(
    tx: &rusqlite::Connection,
    run_id: &str,
    project_path: &std::path::Path,
) -> anyhow::Result<()> {
    let documents =
        crate::core::audit_documents::capture_json(project_path).map_err(anyhow::Error::msg)?;
    let td_dir =
        crate::core::docs_location::DocsRoot::of(project_path).file(project_path, "tech-debt");
    let counts = super::full::count_td_severities(&td_dir);
    let score = crate::models::compute_health_score(
        counts.critical,
        counts.high,
        counts.medium,
        counts.low,
    );
    crate::db::audit_runs::complete(
        tx,
        run_id,
        chrono::Utc::now(),
        "Completed",
        counts.critical,
        counts.high,
        counts.medium,
        counts.low,
        0,
        0,
        0,
        score,
        None,
        None,
    )?;
    crate::db::audit_documents::record(tx, run_id, &documents)?;
    Ok(())
}

#[cfg(all(test, feature = "full-tests"))]
mod partial_finalize_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn partial_audit_continues_after_its_subscriber_is_dropped() {
        let progress = Arc::new(AtomicUsize::new(0));
        let producer_progress = progress.clone();
        let producer: SseStream = Box::pin(async_stream::try_stream! {
            for _ in 0..3 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                producer_progress.fetch_add(1, Ordering::SeqCst);
                yield Event::default().event("progress").data("{}");
            }
        });

        drop(detached_partial_audit_response(producer));

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while progress.load(Ordering::SeqCst) < 3 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("partial audit must finish after its subscriber is dropped");
        assert_eq!(progress.load(Ordering::SeqCst), 3);
    }

    fn mini_disc(id: &str, project: &str) -> (Discussion, DiscussionMessage) {
        let now = chrono::Utc::now();
        let msg = DiscussionMessage {
            recovered_partial: false,
            session_tokens_at_message: None,
            author_cli_ordinal: None,
            session_continuity: None,
            model: None,
            lint_report: None,
            id: format!("{id}-m"),
            role: MessageRole::User,
            channel: MessageChannel::Main,
            content: "validate".into(),
            agent_type: None,
            timestamp: now,
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
        let disc = Discussion {
            connection_id: None,
            awaiting_agent: false,
            agent_running: false,
            id: id.into(),
            project_id: Some(project.into()),
            title: "Validation audit partiel (1 section)".into(),
            agent: crate::models::AgentType::ClaudeCode,
            language: "fr".into(),
            participants: vec![crate::models::AgentType::ClaudeCode],
            messages: vec![msg.clone()],
            message_count: 1,
            non_system_message_count: 1,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            tier: crate::models::ModelTier::Default,
            model: None,
            pin_first_message: true,
            archived: false,
            pinned: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::OnDemand,
            introspection_call_count: 0,
            shared_id: None,
            shared_with: vec![],
            workflow_run_id: None,
            test_mode_restore_branch: None,
            test_mode_stash_ref: None,
            created_at: now,
            updated_at: now,
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
                conn,
                "run-p",
                "p1",
                "Partial",
                "ClaudeCode",
                chrono::Utc::now(),
            )
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn success_persists_completed_link_and_outcomes_atomically() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("docs")).unwrap();
        finalize_partial_run(
            &db,
            "run-p".into(),
            tmp.path().to_path_buf(),
            PartialTerminal::Success(Box::new(mini_disc("d-scoped", "p1"))),
            r#"{"requested":[3],"succeeded":[3],"failed":[],"unchanged":[]}"#.into(),
        )
        .await
        .unwrap();
        let run = db
            .with_conn(|conn| Ok(crate::db::audit_runs::get_by_id(conn, "run-p")?.unwrap()))
            .await
            .unwrap();
        assert_eq!(run.status, "Completed");
        assert_eq!(run.validation_discussion_id.as_deref(), Some("d-scoped"));
        assert!(run
            .step_outcomes_json
            .unwrap()
            .contains("\"succeeded\":[3]"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn step_failure_marks_interrupted_without_any_discussion() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        let tmp = tempfile::TempDir::new().unwrap();
        finalize_partial_run(
            &db,
            "run-p".into(),
            tmp.path().to_path_buf(),
            PartialTerminal::Interrupted {
                failed_steps: vec![8],
                validation: None,
            },
            r#"{"requested":[3,8],"succeeded":[3],"failed":[8],"unchanged":[]}"#.into(),
        )
        .await
        .unwrap();
        let (run, disc_count) = db
            .with_conn(|conn| {
                let run = crate::db::audit_runs::get_by_id(conn, "run-p")?.unwrap();
                let n: i64 =
                    conn.query_row("SELECT COUNT(*) FROM discussions", [], |r| r.get(0))?;
                Ok((run, n))
            })
            .await
            .unwrap();
        assert_eq!(run.status, "Interrupted");
        assert!(run.validation_discussion_id.is_none());
        assert_eq!(
            disc_count, 0,
            "an interrupted partial creates no discussion"
        );
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
            &db,
            "run-ghost".into(),
            tmp.path().to_path_buf(),
            PartialTerminal::Success(Box::new(mini_disc("d-x", "p1"))),
            "{}".into(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("terminal write failed"), "{err}");
        assert!(
            !tmp.path().join("docs/checksums.json").exists(),
            "no baseline may exist after a failed terminal write"
        );
        let orphan: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM discussions", [], |r| r.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(
            orphan, 0,
            "the scoped discussion rolls back with the failure"
        );
    }

    async fn run_and_discussions(
        db: &std::sync::Arc<crate::db::Database>,
    ) -> (crate::models::AuditRun, i64) {
        db.with_conn(|conn| {
            let run = crate::db::audit_runs::get_by_id(conn, "run-p")?.unwrap();
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM discussions", [], |r| r.get(0))?;
            Ok((run, n))
        })
        .await
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn all_unchanged_completes_and_needs_no_discussion_when_validation_came_back() {
        // KT-1207 — re-verified unchanged is done: Completed, so validate and
        // every later refresh see a finished run instead of a Failed loop.
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("docs")).unwrap();
        finalize_partial_run(
            &db,
            "run-p".into(),
            tmp.path().to_path_buf(),
            PartialTerminal::Reverified { validation: None },
            r#"{"requested":[3,8],"succeeded":[],"failed":[],"unchanged":[3,8]}"#.into(),
        )
        .await
        .unwrap();
        let (run, disc_count) = run_and_discussions(&db).await;
        assert_eq!(run.status, "Completed");
        assert!(run.validation_discussion_id.is_none());
        assert_eq!(disc_count, 0);
        assert!(run
            .step_outcomes_json
            .unwrap()
            .contains("\"unchanged\":[3,8]"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn all_unchanged_links_its_scoped_discussion_when_validation_did_not_come_back() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("docs")).unwrap();
        finalize_partial_run(
            &db,
            "run-p".into(),
            tmp.path().to_path_buf(),
            PartialTerminal::Reverified {
                validation: Some(Box::new(mini_disc("d-reverified", "p1"))),
            },
            r#"{"requested":[3],"succeeded":[],"failed":[],"unchanged":[3]}"#.into(),
        )
        .await
        .unwrap();
        let (run, disc_count) = run_and_discussions(&db).await;
        assert_eq!(run.status, "Completed");
        assert_eq!(
            run.validation_discussion_id.as_deref(),
            Some("d-reverified")
        );
        assert_eq!(disc_count, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn interrupted_with_done_steps_links_their_scoped_discussion() {
        // KT-1207 M17 — the KT-931 convention: the run stays Interrupted
        // (validate refuses it) and its done steps get their discussion.
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        let tmp = tempfile::TempDir::new().unwrap();
        finalize_partial_run(
            &db,
            "run-p".into(),
            tmp.path().to_path_buf(),
            PartialTerminal::Interrupted {
                failed_steps: vec![8],
                validation: Some(Box::new(mini_disc("d-done", "p1"))),
            },
            r#"{"requested":[3,8],"succeeded":[3],"failed":[8],"unchanged":[]}"#.into(),
        )
        .await
        .unwrap();
        let (run, disc_count) = run_and_discussions(&db).await;
        assert_eq!(run.status, "Interrupted");
        assert_eq!(run.validation_discussion_id.as_deref(), Some("d-done"));
        assert_eq!(disc_count, 1);
    }

    #[test]
    fn the_terminal_follows_the_partition() {
        let disc = || Some(mini_disc("d", "p"));
        assert!(matches!(
            partial_terminal(vec![], true, disc()),
            PartialTerminal::Success(_)
        ));
        assert!(matches!(
            partial_terminal(vec![], false, None),
            PartialTerminal::Reverified {
                validation: None,
                ..
            }
        ));
        assert!(matches!(
            partial_terminal(vec![], false, disc()),
            PartialTerminal::Reverified {
                validation: Some(_),
                ..
            }
        ));
        assert!(matches!(
            partial_terminal(vec![4], true, disc()),
            PartialTerminal::Interrupted {
                validation: Some(_),
                ..
            }
        ));
        assert!(matches!(
            partial_terminal(vec![4], false, None),
            PartialTerminal::Interrupted {
                validation: None,
                ..
            }
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn already_terminal_row_refuses_every_branch_and_rolls_back_outcomes() {
        // Codex msg 148 — a Running row must REALLY have transitioned: on an
        // already-terminal row the strict marks abort the tx, so neither the
        // status nor the outcomes change and the server cannot report a
        // terminal the DB does not carry.
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        db.with_conn(|conn| crate::db::audit_runs::mark_cancelled(conn, "run-p"))
            .await
            .unwrap();
        for terminal in [
            PartialTerminal::Success(Box::new(mini_disc("d-refused", "p1"))),
            PartialTerminal::Interrupted {
                failed_steps: vec![1],
                validation: None,
            },
            PartialTerminal::Reverified { validation: None },
        ] {
            let err = finalize_partial_run(
                &db,
                "run-p".into(),
                std::path::PathBuf::from("/tmp"),
                terminal,
                r#"{"marker":"must-not-persist"}"#.into(),
            )
            .await
            .unwrap_err();
            assert!(err.contains("terminal write failed"), "{err}");
        }
        let (run, disc_count) = db
            .with_conn(|conn| {
                let run = crate::db::audit_runs::get_by_id(conn, "run-p")?.unwrap();
                let n: i64 =
                    conn.query_row("SELECT COUNT(*) FROM discussions", [], |r| r.get(0))?;
                Ok((run, n))
            })
            .await
            .unwrap();
        assert_eq!(
            run.status, "Cancelled",
            "the terminal status must be untouched"
        );
        assert!(
            run.step_outcomes_json.is_none(),
            "outcomes must roll back with the refused transition"
        );
        assert_eq!(
            disc_count, 0,
            "the scoped discussion rolls back with the refused Success"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn same_status_transition_is_still_refused() {
        // The sneakiest weak-guard case: a re-read based check would accept
        // Interrupted→Interrupted (or Completed→Completed) since the row already
        // reads as expected. The strict marks refuse it — a terminal row is
        // written exactly once.
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db).await;
        db.with_conn(|conn| crate::db::audit_runs::mark_interrupted_strict(conn, "run-p", "first"))
            .await
            .unwrap();
        let err = finalize_partial_run(
            &db,
            "run-p".into(),
            std::path::PathBuf::from("/tmp"),
            PartialTerminal::Interrupted {
                failed_steps: vec![2],
                validation: None,
            },
            r#"{"marker":"second-write"}"#.into(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("terminal write failed"), "{err}");

        let db2 = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        seed_run(&db2).await;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("docs")).unwrap();
        finalize_partial_run(
            &db2,
            "run-p".into(),
            tmp.path().to_path_buf(),
            PartialTerminal::Reverified { validation: None },
            r#"{"marker":"first-write"}"#.into(),
        )
        .await
        .unwrap();
        let err = finalize_partial_run(
            &db2,
            "run-p".into(),
            tmp.path().to_path_buf(),
            PartialTerminal::Reverified { validation: None },
            r#"{"marker":"second-write"}"#.into(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("terminal write failed"), "{err}");
        for (d, outcomes) in [(&db, None), (&db2, Some(r#"{"marker":"first-write"}"#))] {
            let run = d
                .with_conn(|conn| Ok(crate::db::audit_runs::get_by_id(conn, "run-p")?.unwrap()))
                .await
                .unwrap();
            assert_eq!(
                run.step_outcomes_json.as_deref(),
                outcomes,
                "the second write must leave no trace"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_row_refuses_every_branch() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        for terminal in [
            PartialTerminal::Success(Box::new(mini_disc("d-ghost", "p1"))),
            PartialTerminal::Interrupted {
                failed_steps: vec![1],
                validation: None,
            },
            PartialTerminal::Reverified { validation: None },
        ] {
            let err = finalize_partial_run(
                &db,
                "run-ghost".into(),
                std::path::PathBuf::from("/tmp"),
                terminal,
                "{}".into(),
            )
            .await
            .unwrap_err();
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
        crate::core::checksums::write_checksums_file(tmp.path(), &[untargeted], &[]).unwrap();

        static SOURCES: &[&str] = &["src.rs"];
        let chain = vec![super::super::AnalysisStep {
            target_file: "docs/repo-map.md",
            prompt: "",
            sources: SOURCES,
        }];
        let merged = compute_merged_baseline(tmp.path(), &[1], &chain).unwrap();
        let files: Vec<&str> = merged.iter().map(|m| m.ai_file.as_str()).collect();
        assert!(
            files.contains(&"docs/repo-map.md"),
            "the refreshed mapping is present"
        );
        assert!(
            files.contains(&"docs/other-section.md"),
            "a mapping outside the refreshed scope must survive the merge"
        );
    }

    #[test]
    fn merge_refuses_a_malformed_manifest_instead_of_erasing_the_scope() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("src.rs"), "fn main() {}").unwrap();
        std::fs::write(tmp.path().join("docs/checksums.json"), "{ not json").unwrap();

        static SOURCES: &[&str] = &["src.rs"];
        let chain = vec![super::super::AnalysisStep {
            target_file: "docs/repo-map.md",
            prompt: "",
            sources: SOURCES,
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
        assert!(
            crate::core::checksums::read_checksums_file_strict(tmp.path())
                .unwrap()
                .is_none(),
            "missing manifest is a legitimate None"
        );
        std::fs::write(tmp.path().join("docs/checksums.json"), "[broken").unwrap();
        assert!(
            crate::core::checksums::read_checksums_file_strict(tmp.path()).is_err(),
            "a corrupt manifest must be an error, never an empty baseline"
        );
    }

    #[test]
    fn done_status_mirrors_the_persisted_terminal_exactly() {
        // Codex P0 (msg 158): the SSE done must come from the SAME object
        // the transaction persists: a run that rewrote nothing never reads
        // as `complete` on the wire.
        assert_eq!(
            done_status_of(&PartialTerminal::Success(Box::new(mini_disc("d", "p")))),
            "complete"
        );
        assert_eq!(
            done_status_of(&PartialTerminal::Interrupted {
                failed_steps: vec![1],
                validation: None,
            }),
            "interrupted"
        );
        assert_eq!(
            done_status_of(&PartialTerminal::Reverified { validation: None }),
            "no_change"
        );
    }

    #[test]
    fn review_pseudo_step_is_not_partial_selectable() {
        // Codex lot-2 #3 — a synthetic step passes the validator on exit
        // code 0 alone; it must be refused at selection time.
        let review = super::super::AnalysisStep {
            target_file: "REVIEW",
            prompt: "",
            sources: &[],
        };
        assert!(!super::super::partial_selectable(
            &review,
            &crate::core::docs_location::DocsRoot::default()
        ));
        let real = super::super::AnalysisStep {
            target_file: "docs/repo-map.md",
            prompt: "",
            sources: &[],
        };
        assert!(super::super::partial_selectable(
            &real,
            &crate::core::docs_location::DocsRoot::default()
        ));
    }
}
