// « Revalider l'audit » (KT-1168). Deterministic first: the citation
// fingerprints recorded at validation name every cited line that changed,
// moved or disappeared, without a model. A moved line is re-anchored by
// rewriting its line number. What only an agent can settle goes to a bounded
// pass of the partial-audit pipeline (same lease, run row, cancel, resume),
// on the owning steps only, with the findings and the revalidation skill.

use std::collections::{BTreeSet, HashMap};
use std::convert::Infallible;
use std::path::Path as FsPath;

use axum::{
    extract::{Path, State},
    response::sse::{Event, Sse},
    Json,
};

use crate::core::citation_drift::{self, CitationState, CitationStatus, Reanchor, SourcesState};
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

use super::SseStream;

/// The builtin skill the agent pass receives.
pub(crate) const SKILL_ID: &str = "audit-revalidation";

/// Each `owner="human"` section's citation labels, listed for its owner.
fn human_parts(content: &str) -> Vec<(String, BTreeSet<String>)> {
    let (_, sections) = super::anti_hallu_enforce::split_human_owned_sections(content);
    sections
        .into_iter()
        .map(|(name, text)| (name, citation_drift::cite_labels(&text)))
        .collect()
}

fn owning_step(
    chain: &[super::AnalysisStep],
    doc: &str,
    docs_root: &crate::core::docs_location::DocsRoot,
) -> Option<usize> {
    chain
        .iter()
        .position(|s| s.target_file == doc && super::partial_selectable(s, docs_root))
        .map(|i| i + 1)
}

/// What "Revalider l'audit" would do, computed without writing anything.
pub fn plan(project_path: &FsPath) -> RevalidationPlan {
    let report = citation_drift::check(project_path);
    let docs_root = crate::core::docs_location::DocsRoot::of(project_path);
    let chain = super::steps_for(AuditKind::Full, &docs_root);
    let mut plan = RevalidationPlan {
        sources_state: report.sources_state,
        ..Default::default()
    };
    for section in &report.sections {
        if section.citations.is_empty() && !section.doc_missing {
            continue;
        }
        let content = std::fs::read_to_string(project_path.join(&section.doc)).unwrap_or_default();
        let human_sections = human_parts(&content);
        let mut entry = RevalidationSection {
            doc: section.doc.clone(),
            step: owning_step(&chain, &section.doc, &docs_root),
            action: RevalidationAction::Manual,
            doc_missing: section.doc_missing,
            reanchor: Vec::new(),
            broken: Vec::new(),
            human: Vec::new(),
        };
        for citation in reanchor_statuses(project_path, &content, section) {
            for (name, labels) in &human_sections {
                if labels.contains(&citation.cite) {
                    entry.human.push(HumanCitationTodo {
                        section: name.clone(),
                        citation: citation.clone(),
                    });
                }
            }
            // A citation in a human section is corrected like any other; its
            // owner then reconfirms the doc (KT-1079).
            if citation.state == CitationState::Moved {
                entry.reanchor.push(citation.clone());
            } else {
                entry.broken.push(citation.clone());
            }
        }
        entry.action = if section.doc_missing || (!entry.broken.is_empty() && entry.step.is_none())
        {
            RevalidationAction::Manual
        } else if !entry.broken.is_empty() {
            RevalidationAction::Agent
        } else if !entry.reanchor.is_empty() {
            RevalidationAction::Reanchor
        } else {
            RevalidationAction::Human
        };
        plan.reanchor_count += entry.reanchor.len();
        plan.human_count += entry.human.len();
        if entry.action == RevalidationAction::Agent {
            if let Some(step) = entry.step {
                plan.agent_steps.push(step);
            }
        }
        plan.sections.push(entry);
    }
    plan.agent_steps.sort_unstable();
    plan.agent_steps.dedup();
    plan
}

/// Citation checks alone cannot see changed prose or newly pulled docs.
/// Extend their bounded plan from the latest run's locally recorded contents.
fn plan_with_documents(
    project_path: &FsPath,
    documents: Option<&str>,
) -> Result<RevalidationPlan, String> {
    let mut planned = plan(project_path);
    let Some(documents) = documents else {
        return Ok(planned);
    };
    let docs_root = crate::core::docs_location::DocsRoot::of(project_path);
    let chain = super::steps_for(AuditKind::Full, &docs_root);
    for doc in crate::core::audit_documents::changed_since(project_path, documents)? {
        if let Some(section) = planned
            .sections
            .iter_mut()
            .find(|section| section.doc == doc)
        {
            // An ordinary move already has its deterministic action. An
            // additional prose edit is not certified by moving a citation:
            // advance_local_changes retains the old document proof for it.
            if section.doc_missing || !section.broken.is_empty() || !section.reanchor.is_empty() {
                continue;
            }
            section.action = if section.step.is_some() {
                RevalidationAction::Agent
            } else {
                RevalidationAction::Manual
            };
            if let Some(step) = section.step {
                planned.agent_steps.push(step);
            }
            continue;
        }
        let step = owning_step(&chain, &doc, &docs_root);
        let doc_missing = !project_path.join(&doc).is_file();
        planned.sections.push(RevalidationSection {
            doc_missing,
            doc,
            step,
            action: if step.is_some() && !doc_missing {
                RevalidationAction::Agent
            } else {
                RevalidationAction::Manual
            },
            reanchor: Vec::new(),
            broken: Vec::new(),
            human: Vec::new(),
        });
        if let Some(step) = step.filter(|_| !doc_missing) {
            planned.agent_steps.push(step);
        }
    }
    planned.agent_steps.sort_unstable();
    planned.agent_steps.dedup();
    Ok(planned)
}

/// One doc whose moved citations were re-anchored.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Reanchored {
    pub doc: String,
    pub count: usize,
}

/// Rewrite every moved citation to its new line and move its fingerprint with
/// it. A human-validated doc it rewrites is marked to reconfirm. Callers hold
/// the project's audit lease.
pub(crate) fn apply_reanchors(project_path: &FsPath) -> Result<Vec<Reanchored>, String> {
    let mut done = Vec::new();
    for section in plan(project_path).sections {
        if section.reanchor.is_empty() || section.doc_missing {
            continue;
        }
        let path = project_path.join(&section.doc);
        let before = std::fs::read_to_string(&path)
            .map_err(|e| format!("Cannot read {}: {e}", section.doc))?;
        let moves = reanchors_to_apply(project_path, &before, &section.doc, &section.reanchor);
        if moves.is_empty() {
            continue;
        }
        let (after, count) = citation_drift::rewrite_citations(&before, &moves, |_| true);
        if count == 0 {
            continue;
        }
        crate::core::mcp_scanner::atomic_write(&path, &after)?;
        citation_drift::record_reanchored(project_path, &section.doc, &before, &after, &moves)?;
        if let Err(e) = crate::core::doc_provenance::note_write(
            project_path,
            &section.doc,
            &before,
            &after,
            crate::core::doc_provenance::DocWriter::Revalidation,
        ) {
            tracing::warn!(
                "Human-validation provenance of {} not recorded: {e}",
                section.doc
            );
        }
        done.push(Reanchored {
            doc: section.doc,
            count,
        });
    }
    Ok(done)
}

fn reanchor_statuses(
    project_path: &FsPath,
    content: &str,
    section: &citation_drift::SectionCitations,
) -> Vec<CitationStatus> {
    let moves = reanchor_suggestions(project_path, content, &section.doc);
    section
        .citations
        .iter()
        .cloned()
        .map(|mut citation| {
            if citation.state != CitationState::Moved {
                if let Some(mapped) = moves.iter().find(|mapped| mapped.from == citation.cite) {
                    citation.state = CitationState::Moved;
                    citation.current_line = Some(mapped.line);
                }
            }
            citation
        })
        .collect()
}

fn reanchor_suggestions(project_path: &FsPath, content: &str, doc: &str) -> Vec<Reanchor> {
    let base = crate::core::docs_citations::default_reanchor_base(project_path)
        .inspect_err(|error| tracing::warn!("Citation diff mapping unavailable: {error}"))
        .ok()
        .flatten();
    crate::core::docs_citations::suggested_reanchors(content, project_path, base.as_deref())
        .unwrap_or_else(|error| {
            tracing::warn!(doc, "Citation diff mapping unavailable: {error}");
            Vec::new()
        })
}

fn reanchors_to_apply(
    project_path: &FsPath,
    content: &str,
    doc: &str,
    statuses: &[CitationStatus],
) -> Vec<Reanchor> {
    let suggestions = reanchor_suggestions(project_path, content, doc);
    statuses
        .iter()
        .filter_map(|status| {
            let fallback = Reanchor::of(status)?;
            Some(
                suggestions
                    .iter()
                    .find(|mapped| mapped.from == fallback.from && mapped.line == fallback.line)
                    .cloned()
                    .unwrap_or(fallback),
            )
        })
        .collect()
}

fn finding_line(c: &CitationStatus) -> String {
    let what = match c.state {
        CitationState::Changed => "changed: the cited line now holds other content".to_string(),
        CitationState::Removed => {
            "removed: the file is now shorter than the cited line".to_string()
        }
        CitationState::FileMissing => "file missing".to_string(),
        CitationState::Moved => match c.current_line {
            Some(line) => format!("moved to line {line}, same content"),
            None => "moved".to_string(),
        },
        CitationState::Unchanged => "unchanged".to_string(),
    };
    format!("- `[src: file: {}]` — {what}\n", c.cite)
}

/// The block an agent step receives: the skill, then Kronn's findings for
/// that step's doc.
pub(crate) fn prompt_blocks(plan: &RevalidationPlan) -> HashMap<usize, String> {
    let skill = crate::core::skills::get_skill(SKILL_ID)
        .map(|s| s.content)
        .unwrap_or_default();
    let mut blocks = HashMap::new();
    for section in &plan.sections {
        let Some(step) = section
            .step
            .filter(|_| section.action == RevalidationAction::Agent)
        else {
            continue;
        };
        let mut block = format!(
            "\n\n{skill}\n\n## KRONN FINDINGS — `{}`\n\
             Kronn checked citation fingerprints and the documents recorded with the completed audit. \
             Review these findings against the actual repository:\n",
            section.doc
        );
        for c in &section.broken {
            block.push_str(&finding_line(c));
        }
        if section.broken.is_empty() && section.reanchor.is_empty() {
            block.push_str("The document's content changed since the completed audit or validation. Re-read this document and its actual sources, including uncited prose, and verify the changed claims. Keep accurate human wording; do not invent citations.\n");
        }
        if !section.reanchor.is_empty() {
            block.push_str(&format!(
                "Kronn already re-anchored {} moved citation(s) with unchanged content: leave them.\n",
                section.reanchor.len()
            ));
        }
        if !section.human.is_empty() {
            block.push_str(
                "These citations sit in `owner=\"human\"` sections: correct them like the others, keep the rest of the human wording; Kronn asks their owner to reconfirm.\n",
            );
            for h in &section.human {
                block.push_str(&format!(
                    "- section `{}`: {}",
                    h.section,
                    finding_line(&h.citation).trim_start_matches("- ")
                ));
            }
        }
        blocks.insert(step, block);
    }
    blocks
}

/// The agent pass's extras, handed to the partial-audit pipeline.
pub(crate) struct RevalidationRun {
    pub blocks: HashMap<usize, String>,
}

/// After a partial run that rewrote nothing — every agent step unchanged,
/// or stopped before writing — the validation it revoked at launch comes
/// back: the docs are byte-identical to what the human validated. Only a
/// `provenance` reason re-dates each doc's own validation.
pub(crate) fn restore_validation_if_untouched(
    project_path: &FsPath,
    validated_at: Option<&str>,
    snapshots: &[(String, super::validation::TargetSnapshot)],
    provenance: Option<crate::core::doc_provenance::ValidationReason>,
) -> bool {
    let Some(date) = validated_at else {
        return false;
    };
    let untouched = snapshots.iter().all(|(target, before)| {
        super::validation::target_snapshot(project_path, target).is_ok_and(|now| &now == before)
    });
    if !(untouched && crate::core::kronn_state::restore_validated(project_path, date).is_ok()) {
        return false;
    }
    let Some(reason) = provenance else {
        return true;
    };
    let docs: Vec<String> = snapshots.iter().map(|(target, _)| target.clone()).collect();
    if let Err(e) = crate::core::doc_provenance::refresh_validation(project_path, &docs, reason) {
        tracing::warn!("Human-validation provenance not recorded: {e}");
    }
    true
}

async fn project_of(state: &AppState, id: &str) -> Result<Project, String> {
    let id = id.to_string();
    match state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(project)) => Ok(project),
        Ok(None) => Err("Project not found".into()),
        Err(e) => Err(format!("DB error: {e}")),
    }
}

async fn latest_documents(state: &AppState, id: &str) -> Result<Option<String>, String> {
    let id = id.to_string();
    state
        .db
        .with_read_conn(move |conn| crate::db::audit_documents::latest(conn, &id))
        .await
        .map_err(|e| format!("Cannot read final audit documents: {e}"))
}

/// GET /api/projects/:id/revalidation
pub async fn revalidation_plan(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<RevalidationPlan>> {
    let project = match project_of(&state, &id).await {
        Ok(project) => project,
        Err(e) => return Json(ApiResponse::err(e)),
    };
    let path = scanner::resolve_host_path(&project.path);
    let documents = match latest_documents(&state, &id).await {
        Ok(documents) => documents,
        Err(e) => return Json(ApiResponse::err(e)),
    };
    match tokio::task::spawn_blocking(move || plan_with_documents(&path, documents.as_deref()))
        .await
    {
        Ok(Ok(plan)) => Json(ApiResponse::ok(plan)),
        Ok(Err(e)) => Json(ApiResponse::err(e)),
        Err(e) => Json(ApiResponse::err(format!("Revalidation plan failed: {e}"))),
    }
}

fn single_error(message: String) -> Sse<SseStream> {
    let data = serde_json::json!({ "error": message }).to_string();
    let stream: SseStream = Box::pin(futures::stream::once(async move {
        Ok::<_, Infallible>(Event::default().event("error").data(data))
    }));
    Sse::new(stream)
}

/// POST /api/projects/:id/revalidate-audit
/// Re-anchors the moved citations, then runs the owning audit steps of the
/// sections with broken citations, each told what Kronn found. Relaunching
/// it resumes: the plan is recomputed from the fingerprints every time.
pub async fn revalidate_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RevalidateAuditRequest>,
) -> Sse<SseStream> {
    let project = match project_of(&state, &id).await {
        Ok(project) => project,
        Err(e) => return single_error(e),
    };
    let project_path = scanner::resolve_host_path(&project.path);
    // A configured folder failing containment refuses the run (KT-1192).
    if let Err(reason) = crate::core::docs_location::DocsRoot::for_project(&project_path) {
        return single_error(reason);
    }
    let documents = match latest_documents(&state, &id).await {
        Ok(documents) => documents,
        Err(e) => return single_error(e),
    };
    let planned = {
        let path = project_path.clone();
        match tokio::task::spawn_blocking(move || plan_with_documents(&path, documents.as_deref()))
            .await
        {
            Ok(Ok(plan)) => plan,
            Ok(Err(e)) => return single_error(e),
            Err(e) => return single_error(format!("Revalidation plan failed: {e}")),
        }
    };
    // A free re-anchor action must never dispatch an agent merely because
    // other sections in the same plan still need one. Recompute what remains
    // under the lease and return it to the caller for an explicit next choice.
    if req.deterministic_only.unwrap_or(false) {
        return deterministic_only(state, project.id, project_path);
    }
    if planned.sources_state == SourcesState::Unknown && planned.agent_steps.is_empty() {
        return single_error(
            "No citation fingerprints recorded yet: validate the audit first, then revalidate it."
                .into(),
        );
    }
    if !planned.agent_steps.is_empty() {
        let run = RevalidationRun {
            blocks: prompt_blocks(&planned),
        };
        return super::drift::run_partial_pipeline(
            state,
            project,
            planned.agent_steps.clone(),
            req.agent,
            req.tier,
            req.connection_id,
            Some(run),
        )
        .await;
    }
    deterministic_only(state, project.id, project_path)
}

/// No section needs an agent: re-anchor under the lease and report.
fn deterministic_only(
    state: AppState,
    project_id: String,
    project_path: std::path::PathBuf,
) -> Sse<SseStream> {
    let stream: SseStream = Box::pin(async_stream::try_stream! {
        let tracker = state.audit_tracker.clone();
        if !tracker.lock().map(|mut t| t.try_acquire_lease(&project_id)).unwrap_or(false) {
            yield Event::default().event("error").data(serde_json::json!({
                "error": "An audit is currently running on this project; revalidate once it has finished.",
            }).to_string());
            return;
        }
        let path = project_path.clone();
        let applied = tokio::task::spawn_blocking(move || {
            let before = crate::core::audit_documents::capture(&path)?;
            let reanchored = apply_reanchors(&path)?;
            let after = crate::core::audit_documents::capture(&path)?;
            Ok::<_, String>((reanchored, before, after))
        })
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {e}")));
        let applied = match applied {
            Ok((reanchored, before, after)) => {
                let pid = project_id.clone();
                match state.db.with_conn(move |conn| {
                    crate::db::audit_documents::advance_local_changes(conn, &pid, &before, &after)?;
                    crate::db::audit_documents::latest(conn, &pid)
                }).await {
                    Ok(documents) => {
                        let path = project_path.clone();
                        tokio::task::spawn_blocking(move || {
                            plan_with_documents(&path, documents.as_deref()).map(|plan| (reanchored, plan))
                        }).await.unwrap_or_else(|e| Err(format!("Task failed: {e}")))
                    }
                    Err(e) => Err(format!("Cannot record reanchored documents: {e}")),
                }
            }
            Err(e) => Err(e),
        };
        if let Ok(mut t) = tracker.lock() {
            t.release_lease(&project_id);
        }
        match applied {
            Ok((reanchored, after)) => {
                for doc in &reanchored {
                    yield Event::default().event("reanchored").data(serde_json::to_string(doc).unwrap_or_default());
                }
                let status = if after.sources_state == SourcesState::Clean && after.sections.is_empty() { "clean" } else { "todo" };
                yield Event::default().event("done").data(serde_json::json!({
                    "status": status,
                    "revalidation": true,
                    "reanchored": reanchored.iter().map(|r| r.count).sum::<usize>(),
                    "plan": after,
                }).to_string());
            }
            Err(e) => {
                yield Event::default().event("error").data(serde_json::json!({
                    "error": format!("Revalidation failed: {e}"),
                }).to_string());
            }
        }
    });
    Sse::new(super::detach_sse_stream(stream))
}

#[cfg(test)]
#[path = "revalidate_test.rs"]
mod tests;
