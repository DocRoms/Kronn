// « Valider cette version » (KT-1489). The docs of a validated project were
// edited outside Kronn: a person who read them as they stand vouches for them
// without an agent pass. The deterministic checks of the revalidation run
// first (no model); only when no citation is broken are the fingerprints, the
// provenance and the validation date recorded for the docs as they read now.

use std::path::Path as FsPath;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;
use ts_rs::TS;

use crate::core::citation_drift::{self, CitationStatus};
use crate::core::{doc_provenance, kronn_state, scanner};
use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

/// A citation that blocks the validation, with the doc that carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct BrokenDocCitation {
    pub doc: String,
    pub citation: CitationStatus,
}

/// What « Valider cette version » did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS, Default)]
#[ts(export)]
pub struct ValidateVersionOutcome {
    /// The current docs are now the validated version.
    pub validated: bool,
    /// The citations to correct first; empty when `validated`.
    pub broken: Vec<BrokenDocCitation>,
    /// Moved citations rewritten to their new line.
    #[ts(type = "number")]
    pub reanchored: usize,
    /// Docs recorded as validated.
    #[ts(type = "number")]
    pub docs: usize,
}

/// The citations of the current docs that no agent-free check can vouch for:
/// recorded ones whose line changed or whose file is gone, and the ones no
/// fingerprint covers (new docs, citations added by hand) whose file, line
/// range or `#symbol` does not hold. A block that only moved (same content at
/// other lines) is not broken, it is re-anchored; one whose content changed is,
/// moved or not. Sorted by doc, then citation.
pub(crate) fn broken_citations(project_path: &FsPath) -> Vec<BrokenDocCitation> {
    let report = citation_drift::check(project_path);
    let mut out: Vec<BrokenDocCitation> = Vec::new();
    // The fingerprint's own verdict, never the re-anchor suggestion's: a
    // symbol found elsewhere does not make a block whose content changed a
    // block that only moved.
    for section in report.sections.iter().filter(|s| !s.doc_missing) {
        for citation in section.citations.iter().filter(|c| c.state.is_broken()) {
            out.push(BrokenDocCitation {
                doc: section.doc.clone(),
                citation: citation.clone(),
            });
        }
    }
    out.extend(
        citation_drift::unrecorded_problems(project_path)
            .into_iter()
            .map(|(doc, citation)| BrokenDocCitation { doc, citation }),
    );
    out.sort_by(|a, b| (&a.doc, &a.citation.cite).cmp(&(&b.doc, &b.citation.cite)));
    out.dedup_by(|a, b| a.doc == b.doc && a.citation.cite == b.citation.cite);
    out
}

/// A file as it was before the validation: absent, or its bytes and mode.
enum Saved {
    Absent,
    Present(Vec<u8>, std::fs::Permissions),
}

/// Every file the validation writes, as it was, to put back when a later
/// write fails: the docs the re-anchor rewrites and the three records.
struct Snapshot(Vec<(std::path::PathBuf, Saved)>);

impl Snapshot {
    /// A file that exists but cannot be read is an error: without its bytes the
    /// rollback could only delete it.
    fn take(project_path: &FsPath) -> Result<Self, String> {
        let mut paths: Vec<std::path::PathBuf> = citation_drift::doc_files(project_path)
            .iter()
            .map(|doc| project_path.join(doc))
            .collect();
        paths.push(citation_drift::citations_path(project_path));
        paths.push(doc_provenance::provenance_path(project_path));
        paths.push(kronn_state::state_path(project_path));
        let mut saved = Vec::with_capacity(paths.len());
        for path in paths {
            let read = std::fs::read(&path).and_then(|bytes| {
                std::fs::metadata(&path).map(|meta| Saved::Present(bytes, meta.permissions()))
            });
            match read {
                Ok(file) => saved.push((path, file)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    saved.push((path, Saved::Absent))
                }
                Err(e) => {
                    return Err(format!(
                        "Cannot read {} to protect it during the validation: {e}",
                        path.display()
                    ))
                }
            }
        }
        Ok(Self(saved))
    }

    /// Put every file back; the ones that could not be, with the reason.
    fn restore(&self) -> Vec<String> {
        let mut failed = Vec::new();
        for (path, saved) in &self.0 {
            let done = match saved {
                Saved::Present(bytes, permissions) => {
                    crate::core::mcp_scanner::atomic_write_bytes(path, bytes)
                        .and_then(|()| {
                            std::fs::set_permissions(path, permissions.clone())
                                .map_err(|e| e.to_string())
                        })
                        .map_err(|e| format!("{}: {e}", path.display()))
                }
                Saved::Absent => match std::fs::remove_file(path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        Err(format!("{}: {e}", path.display()))
                    }
                    _ => Ok(()),
                },
            };
            if let Err(e) = done {
                tracing::error!("Validation rollback: cannot restore {e}");
                failed.push(e);
            }
        }
        failed
    }
}

/// Vouch for the docs as they stand, under the project's audit lease.
/// `Err` is a refusal that is not about citations (nothing validated, a
/// placeholder left in a doc, an unreadable record) or a write that failed:
/// then nothing is left half done, the docs and the records are put back (and
/// when one cannot be, the error names it).
pub(crate) fn validate_current_version(
    project_path: &FsPath,
    by: &str,
) -> Result<ValidateVersionOutcome, String> {
    validate_with(project_path, by, kronn_state::revalidate_current)
}

/// [`validate_current_version`] with the step that dates the validation given,
/// last in the sequence: a failure there is the one that must undo the rest.
fn validate_with(
    project_path: &FsPath,
    by: &str,
    date_validation: impl FnOnce(&FsPath, &str) -> Result<(), String>,
) -> Result<ValidateVersionOutcome, String> {
    // Every record a write below depends on is read, strictly, before the
    // first write: an unreadable one refuses the validation with nothing changed.
    let state = kronn_state::load_for_mutation(project_path)?;
    if state.validated_at.is_none() {
        return Err(
            "The documentation is not validated: there is no validation to renew".to_string(),
        );
    }
    doc_provenance::read_strict(project_path)?;
    // The fingerprints the checks compare with: `check` reads past a malformed
    // record as an empty one and `record_all` would overwrite it, so a record
    // that cannot be read refuses the validation before anything is compared.
    citation_drift::read_strict(project_path)?;
    let broken = broken_citations(project_path);
    if !broken.is_empty() {
        return Ok(ValidateVersionOutcome {
            broken,
            ..Default::default()
        });
    }
    let report = crate::core::document_optimization::analyze(project_path)?;
    let unresolved = super::validate::unresolved_unknowns(project_path, &report);
    if !unresolved.is_empty() {
        return Err(format!(
            "Unresolved unknowns remain (`{{{{...}}}}` or `<!-- TODO: ... -->`); settle them before validating this version: {}",
            unresolved.join(", ")
        ));
    }
    let snapshot = Snapshot::take(project_path)?;
    let reanchored = match record_version(project_path, by, date_validation) {
        Ok(count) => count,
        Err(e) => {
            let unrestored = snapshot.restore();
            if unrestored.is_empty() {
                return Err(e);
            }
            return Err(format!(
                "{e}. The rollback could not restore: {}; check these files before validating again",
                unrestored.join("; ")
            ));
        }
    };
    Ok(ValidateVersionOutcome {
        validated: true,
        broken: Vec::new(),
        reanchored,
        docs: citation_drift::doc_files(project_path).len(),
    })
}

/// The writes of the validation; returns the number of citations re-anchored.
fn record_version(
    project_path: &FsPath,
    by: &str,
    date_validation: impl FnOnce(&FsPath, &str) -> Result<(), String>,
) -> Result<usize, String> {
    // A moved line is rewritten first, so the fingerprints below are taken at
    // the line the claim is about rather than at the one the code moved to.
    let reanchored: usize = super::revalidate::apply_reanchors(project_path)?
        .iter()
        .map(|r| r.count)
        .sum();
    citation_drift::record_all(project_path)?;
    // The docs nothing anchors are vouched for too; a failed stamp only queues
    // them for a re-read, it never refuses the validation (as KT-1319).
    if let Err(e) = super::revalidate::stamp_validated_rereads(project_path) {
        tracing::warn!("Validation re-read stamps not recorded: {e}");
    }
    doc_provenance::accept_current(project_path, doc_provenance::ValidationReason::Confirmed)?;
    date_validation(project_path, by)?;
    Ok(reanchored)
}

/// POST /api/projects/:id/validate-current-docs
/// « Valider cette version »: human only, held under the project lease like a
/// validation. Runs no agent.
pub async fn validate_current_docs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
) -> Json<ApiResponse<ValidateVersionOutcome>> {
    if bridge.is_some() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Only a human can validate a documentation version",
        ));
    }
    let project = match state
        .db
        .with_read_conn({
            let id = id.clone();
            move |conn| crate::db::projects::get_project(conn, &id)
        })
        .await
    {
        Ok(Some(project)) => project,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {e}"))),
    };
    let path = scanner::resolve_host_path(&project.path);
    let lease = match super::docs_write::DocsWriteLease::acquire(&state, &project.id).await {
        Ok(lease) => lease,
        Err(error) => return Json(ApiResponse::err_coded(ApiErrorCode::Conflict, error)),
    };
    let by = super::validate::operator_pseudo(&state).await;
    let recorded = {
        let pid = project.id.clone();
        state
            .db
            .with_read_conn(move |conn| crate::db::audit_documents::latest(conn, &pid))
            .await
            .unwrap_or(None)
    };
    let done = lease
        .blocking(move || {
            let outcome = validate_current_version(&path, &by)?;
            if !outcome.validated {
                return Ok::<_, String>((outcome, Vec::new(), Default::default()));
            }
            // The completed run's document proof follows the docs the person
            // just validated, only for the ones that moved from it.
            let changed = match recorded.as_deref() {
                Some(recorded) => crate::core::audit_documents::changed_since(&path, recorded)?,
                None => Vec::new(),
            };
            let current = crate::core::audit_documents::capture(&path)?;
            Ok((outcome, changed, current))
        })
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {e}")));
    let (outcome, changed, current) = match done {
        Ok(done) => done,
        Err(e) => return Json(ApiResponse::err(e)),
    };
    if outcome.validated && !changed.is_empty() {
        let pid = project.id.clone();
        if let Err(e) = state
            .db
            .with_conn(move |conn| {
                crate::db::audit_documents::confirm_paths(conn, &pid, &changed, &current)
            })
            .await
        {
            return Json(ApiResponse::err(format!(
                "Validated, but the audit documents were not recorded: {e}"
            )));
        }
    }
    Json(ApiResponse::ok(outcome))
}

#[cfg(test)]
#[path = "validate_version_test.rs"]
mod tests;
