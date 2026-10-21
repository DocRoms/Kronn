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

fn owning_step(chain: &[super::AnalysisStep], doc: &str) -> Option<usize> {
    chain
        .iter()
        .position(|s| s.target_file == doc && super::partial_selectable(s))
        .map(|i| i + 1)
}

/// What "Revalider l'audit" would do, computed without writing anything.
pub fn plan(project_path: &FsPath) -> RevalidationPlan {
    let report = citation_drift::check(project_path);
    let chain = super::assemble_chained_steps(AuditKind::Full);
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
            step: owning_step(&chain, &section.doc),
            action: RevalidationAction::Manual,
            doc_missing: section.doc_missing,
            reanchor: Vec::new(),
            broken: Vec::new(),
            human: Vec::new(),
        };
        for citation in &section.citations {
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
        let moves: Vec<Reanchor> = section.reanchor.iter().filter_map(Reanchor::of).collect();
        if moves.is_empty() || section.doc_missing {
            continue;
        }
        let path = project_path.join(&section.doc);
        let before = std::fs::read_to_string(&path)
            .map_err(|e| format!("Cannot read {}: {e}", section.doc))?;
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
             Kronn compared each citation of this file with the fingerprint of the cited lines \
             recorded at validation, deterministically and without a model. These no longer match:\n",
            section.doc
        );
        for c in &section.broken {
            block.push_str(&finding_line(c));
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

/// After a revalidation that rewrote nothing — every agent step unchanged,
/// or stopped before writing — the validation it revoked at launch comes
/// back: the docs are byte-identical to what the human validated.
pub(crate) fn restore_validation_if_untouched(
    project_path: &FsPath,
    validated_at: Option<&str>,
    snapshots: &[(String, super::validation::TargetSnapshot)],
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
    let docs: Vec<String> = snapshots.iter().map(|(target, _)| target.clone()).collect();
    if let Err(e) = crate::core::doc_provenance::refresh_validation(
        project_path,
        &docs,
        crate::core::doc_provenance::ValidationReason::Revalidation,
    ) {
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
    match tokio::task::spawn_blocking(move || plan(&path)).await {
        Ok(plan) => Json(ApiResponse::ok(plan)),
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
    let planned = {
        let path = project_path.clone();
        match tokio::task::spawn_blocking(move || plan(&path)).await {
            Ok(plan) => plan,
            Err(e) => return single_error(format!("Revalidation plan failed: {e}")),
        }
    };
    if planned.sources_state == SourcesState::Unknown {
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
            let reanchored = apply_reanchors(&path)?;
            Ok::<_, String>((reanchored, plan(&path)))
        })
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {e}")));
        if let Ok(mut t) = tracker.lock() {
            t.release_lease(&project_id);
        }
        match applied {
            Ok((reanchored, after)) => {
                for doc in &reanchored {
                    yield Event::default().event("reanchored").data(serde_json::to_string(doc).unwrap_or_default());
                }
                let status = if after.sources_state == SourcesState::Clean { "clean" } else { "todo" };
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
