//! A human's answer to a cross-agent dispute card (KT-1183), applied: the
//! finding's sheet, the step's index and the central tech-debt index follow it.

use std::path::Path;

use rusqlite::Connection;

use crate::core::audit_validation::{append_history_entry, replace_current_status, TdDecision};
use crate::core::cross_audit::{apply_human_decision_to_index, merge_provenance, HumanDecision};
use crate::db::audit_cross::DisputeTarget;

const REVIEWER: &str = "Kronn cross-agent dispute card";

/// The sheet after the decision: Kronn block refreshed, a dismissal rejects
/// it, a keep leaves its status and records who kept it.
pub fn decided_sheet(existing: &str, target: &DisputeTarget) -> Result<String, String> {
    let merged = merge_provenance(existing, &target.record);
    let Some(decision) = target.record.human_decision else {
        return Ok(merged);
    };
    let mut lines: Vec<String> = merged.lines().map(str::to_string).collect();
    match decision {
        HumanDecision::Dismiss => {
            let status = TdDecision::Rejected.status_label();
            replace_current_status(&mut lines, status).map_err(|e| e.to_string())?;
            // The status line is the decision; a sheet without front matter has no history.
            let _ = append_history_entry(
                &mut lines,
                status,
                REVIEWER,
                "Dismissed through a cross-agent dispute card.",
            );
        }
        HumanDecision::Keep => {
            let id = target.record.td_id.as_deref().unwrap_or_default();
            let status = crate::core::td_index::parse_sheet(id, &merged).status;
            let _ = append_history_entry(
                &mut lines,
                &status,
                REVIEWER,
                "Kept through a cross-agent dispute card.",
            );
        }
        HumanDecision::Declined => {}
    }
    let mut out = lines.join("\n");
    if merged.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

fn read_regular(root: &Path, path: &Path) -> Result<Option<String>, String> {
    crate::core::fs_guard::assert_contained_no_symlink(root, path)?;
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

/// Applies `decision` to the finding of `target`. The caller holds the
/// project's audit lease and a savepoint: on error nothing stays written.
pub fn apply(
    conn: &Connection,
    target: &mut DisputeTarget,
    decision: HumanDecision,
) -> Result<(), String> {
    let root = crate::core::scanner::resolve_host_path(&target.project_path);
    let td_id = target
        .record
        .td_id
        .clone()
        .filter(|id| crate::core::td_index::is_td_id(id))
        .ok_or("the disputed finding has no tech-debt sheet")?;
    target.record.human_decision = Some(decision);

    let docs_root = crate::core::docs_location::DocsRoot::of(&root);
    let docs = docs_root.dir(&root);
    let sheet_path = docs.join("tech-debt").join(format!("{td_id}.md"));
    let original_sheet = read_regular(&root, &sheet_path)?
        .ok_or_else(|| format!("the sheet of {td_id} is no longer on file"))?;
    let sheet = decided_sheet(&original_sheet, target)?;
    let index_path = root.join(&target.file_label);
    let index = read_regular(&root, &index_path)?
        .map(|text| (apply_human_decision_to_index(&text, &td_id, decision), text));

    crate::db::audit_cross::update_record(conn, target).map_err(|e| e.to_string())?;
    crate::core::mcp_scanner::atomic_write(&sheet_path, &sheet)?;
    if let Some((patched, original)) = &index {
        if patched != original {
            if let Err(e) = crate::core::mcp_scanner::atomic_write(&index_path, patched) {
                if let Err(restore) =
                    crate::core::mcp_scanner::atomic_write(&sheet_path, &original_sheet)
                {
                    tracing::error!("Could not restore {}: {restore}", sheet_path.display());
                }
                return Err(e);
            }
        }
    }

    if decision != HumanDecision::Declined {
        let rel = docs_root.path(&format!("tech-debt/{td_id}.md"));
        if let Err(e) = crate::core::doc_provenance::record_validation(
            &root,
            &[rel],
            crate::core::doc_provenance::ValidationReason::ValidationCard,
        ) {
            tracing::warn!("Human-validation provenance of {td_id} not recorded: {e}");
        }
    }
    let trust = crate::core::adr::Trust::CardAccepted(&[]);
    if let Err(e) = crate::core::td_index::resync(&docs, trust) {
        tracing::warn!("Tech-debt index resync after a dispute decision failed: {e}");
    }
    crate::core::td_tasks::close_inactive_logged(
        conn,
        &target.project_id,
        &docs,
        "cross-agent dispute card",
    );
    Ok(())
}

#[cfg(test)]
#[path = "cross_audit_decision_test.rs"]
mod tests;
