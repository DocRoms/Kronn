// Explicit reinstall of the AI-doc template v2 files (KT-1166). Unlike the
// install (`copy_dir_nondestructive`, which never overwrites), this replaces
// the template's own files, after copying every original to
// `.kronn/audit/template-backups/<stamp>/`. A human-validated doc or section
// it rewrites is marked to reconfirm, never kept back (KT-1079). No file
// outside the template's list is read or written. A human action: it is off
// the bridge-token route list and refused again in the handler.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as AxPath, State},
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::{audit_state, fs_guard, scanner};
use crate::models::{AiAuditStatus, ApiErrorCode, ApiResponse, PlaceholderDoc};
use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ReinstallAction {
    /// Missing in the project: written, nothing to back up.
    Create,
    /// Differs from the template: backed up, then replaced.
    Replace,
    /// Already identical to what a reinstall would write.
    Unchanged,
}

/// Why an existing file differs from the template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ReinstallKind {
    /// Empty, binary, still holding `{{placeholders}}`, or with unbalanced
    /// section markers: repaired by default.
    DamagedOrTemplate,
    /// Real content (an audit's or a human's): replaced only when selected.
    Filled,
}

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
pub struct TemplateReinstallRequest {
    /// Existing files to replace. `None`: the damaged ones only. Missing
    /// files are always created; a path the plan does not offer is refused.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ReinstallFile {
    /// Project-relative path.
    pub path: String,
    pub action: ReinstallAction,
    /// Set for `replace` only.
    pub kind: Option<ReinstallKind>,
    /// `owner="human"` sections of the current file this reinstall rewrites.
    pub human_sections: Vec<String>,
    /// The current file is human-validated: replacing it asks to reconfirm.
    pub human_validated: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct TemplateReinstallPlan {
    pub docs_dir: String,
    pub files: Vec<ReinstallFile>,
    /// An audit holds the project's lease: the reinstall would be refused.
    pub audit_running: bool,
    #[ts(optional)]
    pub template_update: Option<crate::core::template_manifest::TemplateUpdate>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ReplacedFile {
    pub path: String,
    /// Project-relative path of the copy of the original.
    pub backup: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct TemplateReinstallReport {
    /// Project-relative; `None` when nothing was replaced.
    pub backup_dir: Option<String>,
    pub replaced: Vec<ReplacedFile>,
    pub created: Vec<String>,
    pub unchanged: Vec<String>,
    /// Differing files that were not selected, left as they are.
    pub skipped: Vec<String>,
    /// Human-validated docs this reinstall rewrote, now to reconfirm.
    pub to_reconfirm: Vec<String>,
    /// Template docs still holding placeholders afterwards, with their step.
    pub placeholder_docs: Vec<PlaceholderDoc>,
    /// `TemplateInstalled` when the docs entry itself is a raw template again.
    pub audit_status: AiAuditStatus,
}

struct Prepared {
    rel: String,
    dst: PathBuf,
    action: ReinstallAction,
    kind: Option<ReinstallKind>,
    old: Option<Vec<u8>>,
    new: Vec<u8>,
    human: Vec<String>,
    validated: bool,
}

fn template_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                walk(&entry.path(), out)?;
            } else if kind.is_file() {
                out.push(entry.path());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, &mut out)?;
    out.sort();
    Ok(out)
}

/// Template docs of `project` holding raw placeholders, each mapped to the
/// Full-chain step whose target it is. Reads only the template's own file list.
pub fn placeholder_docs(project: &Path, template_docs: &Path) -> Vec<PlaceholderDoc> {
    let Ok(sources) = template_files(template_docs) else {
        return Vec::new();
    };
    let docs_dir = scanner::detect_docs_dir(project);
    let docs_root = crate::core::docs_location::DocsRoot::of(project);
    let chain = crate::api::audit::steps_for(crate::models::AuditKind::Full, &docs_root);
    let mut out = Vec::new();
    for source in sources {
        let Ok(within) = source.strip_prefix(template_docs) else {
            continue;
        };
        // A `TEMPLATE.md` is a gabarit to copy: its placeholders are by design.
        let name = within.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "TEMPLATE.md" || !name.ends_with(".md") {
            continue;
        }
        let dst = docs_dir.join(within);
        if !std::fs::symlink_metadata(&dst).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&dst) else {
            continue;
        };
        if crate::api::audit::validation::count_raw_placeholders(&content) == 0 {
            continue;
        }
        let within = within.to_string_lossy().replace('\\', "/");
        // Targets are relocated under the configured docs root, the run's own spelling.
        let target = docs_root.path(&within);
        let step = chain
            .iter()
            .position(|s| {
                s.target_file == target && crate::api::audit::partial_selectable(s, &docs_root)
            })
            .map(|i| i + 1);
        let path = dst
            .strip_prefix(project)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or(within);
        out.push(PlaceholderDoc { path, step });
    }
    out
}

/// Read-only: what a reinstall would do, and the first reason it could not.
fn prepare(project: &Path, template_docs: &Path) -> Result<(PathBuf, Vec<Prepared>), String> {
    if !template_docs.is_dir() {
        return Err(format!(
            "Templates directory not found: {}",
            template_docs.display()
        ));
    }
    // The project's docs root (KT-1192); unconfigured, the detected folder
    // (`docs/` for a fresh project), as before.
    let docs_root = crate::core::docs_location::DocsRoot::for_project(project)?;
    let docs_dir = if docs_root.is_default() {
        scanner::detect_docs_dir(project)
    } else {
        docs_root.dir(project)
    };
    let validated: Vec<String> = crate::core::doc_provenance::report(project, false)
        .map(|r| {
            r.docs
                .into_iter()
                .filter(|d| d.state == crate::core::doc_provenance::ProvenanceState::HumanValidated)
                .map(|d| d.doc)
                .collect()
        })
        .unwrap_or_default();
    let mut prepared = Vec::new();
    for source in template_files(template_docs)? {
        let within = source
            .strip_prefix(template_docs)
            .map_err(|e| e.to_string())?;
        let dst = docs_dir.join(within);
        fs_guard::assert_contained_no_symlink(project, &dst)?;
        let rel = dst
            .strip_prefix(project)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let mut new =
            std::fs::read(&source).map_err(|e| format!("read {}: {e}", source.display()))?;
        if !docs_root.is_default() && source.extension().is_some_and(|ext| ext == "md") {
            if let Ok(text) = std::str::from_utf8(&new) {
                new = docs_root.relocate(text).into_owned().into_bytes();
            }
        }
        let (old, action, kind, new, human) = match std::fs::symlink_metadata(&dst) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                (None, ReinstallAction::Create, None, new, Vec::new())
            }
            Err(e) => return Err(format!("{rel}: {e}")),
            Ok(meta) if !meta.is_file() => {
                return Err(format!("{rel} is not a regular file: nothing was touched"));
            }
            Ok(_) => {
                let old = std::fs::read(&dst).map_err(|e| format!("read {rel}: {e}"))?;
                let human = rewritten_human_sections(&old, &new);
                let (action, kind) = if old == new {
                    (ReinstallAction::Unchanged, None)
                } else {
                    (ReinstallAction::Replace, Some(classify(&old)))
                };
                (Some(old), action, kind, new, human)
            }
        };
        prepared.push(Prepared {
            validated: validated.contains(&rel),
            rel,
            dst,
            action,
            kind,
            old,
            new,
            human,
        });
    }
    Ok((docs_dir, prepared))
}

/// A file is damaged when nothing of an audit could live in it: empty,
/// not text, an unfilled `{{placeholder}}`, or section markers that do not pair.
fn classify(old: &[u8]) -> ReinstallKind {
    let Ok(text) = std::str::from_utf8(old) else {
        return ReinstallKind::DamagedOrTemplate;
    };
    // A doc that documents placeholder or marker syntax in a code block or a
    // code span is not unfilled: only the prose outside code counts.
    let prose = without_inline_code(&crate::core::anti_halluc::strip_fenced_code(text));
    let placeholder = scanner::has_unfilled_placeholder(&prose);
    let opens = prose.matches("<!-- kronn:section ").count();
    let closes = prose.matches("<!-- kronn:section:end").count();
    if text.trim().is_empty() || placeholder || opens != closes {
        ReinstallKind::DamagedOrTemplate
    } else {
        ReinstallKind::Filled
    }
}

/// Each line without its closed backtick spans; an unmatched backtick is text.
fn without_inline_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let parts: Vec<&str> = line.split('`').collect();
        let unmatched_tail = parts.len().is_multiple_of(2);
        for (i, part) in parts.iter().enumerate() {
            let in_span = i % 2 == 1 && !(unmatched_tail && i == parts.len() - 1);
            if !in_span {
                out.push_str(part);
            }
        }
        out.push('\n');
    }
    out
}

/// The `owner="human"` sections of `old` the template's bytes would change.
fn rewritten_human_sections(old: &[u8], new: &[u8]) -> Vec<String> {
    let (Ok(old_text), Ok(new_text)) = (std::str::from_utf8(old), std::str::from_utf8(new)) else {
        return Vec::new();
    };
    crate::api::audit::anti_hallu_enforce::enforce_human_owned_sections(old_text, new_text)
        .map(|(_, diffs)| diffs.into_iter().map(|d| d.name).collect())
        .unwrap_or_default()
}

pub fn plan_reinstall(
    project: &Path,
    template_docs: &Path,
    audit_running: bool,
) -> Result<TemplateReinstallPlan, String> {
    let (docs_dir, prepared) = prepare(project, template_docs)?;
    Ok(TemplateReinstallPlan {
        docs_dir: docs_dir
            .strip_prefix(project)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        files: prepared
            .into_iter()
            .map(|p| ReinstallFile {
                path: p.rel,
                action: p.action,
                kind: p.kind,
                human_sections: p.human,
                human_validated: p.validated,
            })
            .collect(),
        audit_running,
        template_update: Some(crate::core::template_manifest::check(
            project,
            template_docs,
        )?),
    })
}

/// Puts `bytes` at `dst`, replacing the file there (its mode is kept).
fn publish(project: &Path, dst: &Path, bytes: &[u8]) -> Result<(), String> {
    fs_guard::guarded_replace_like(project, dst, bytes)
}

/// Writes one planned file: a `Create` is create-only (a file that appeared
/// since the plan is never overwritten, nor deleted by a rollback) and says who
/// the created file is, so a rollback removes only that file; a `Replace`
/// swaps the file in place.
fn write_planned(project: &Path, file: &Prepared) -> Result<Option<fs_guard::FileId>, String> {
    if file.action != ReinstallAction::Create {
        publish(project, &file.dst, &file.new)?;
        return Ok(None);
    }
    match fs_guard::guarded_write_new_identified(project, &file.dst, &file.new)? {
        Some(id) => Ok(Some(id)),
        None => Err(format!(
            "{} appeared since the plan: nothing was overwritten",
            file.rel
        )),
    }
}

/// Puts back what a failed reinstall already wrote, newest first, and says
/// what happened. A created file is removed only while its name still is the
/// file we created; a replaced one gets its original back, the failing file
/// too when it was a replacement (a failure can come after the swap) unless it
/// still holds its original bytes. A failing create is never touched: what sits
/// at the name is not proven to be ours.
fn roll_back(
    project: &Path,
    failing: &Prepared,
    written: &[(&Prepared, Option<fs_guard::FileId>)],
    backup_dir: Option<&str>,
    error: String,
) -> String {
    let mut undone = Vec::new();
    let mut restored_any = false;
    // Read through the rooted primitive: a link planted at the name is not
    // the original, whatever it points at.
    let failing_replace = (failing.action == ReinstallAction::Replace)
        .then_some((failing, None))
        .filter(|(f, _)| {
            fs_guard::guarded_read(project, &f.dst)
                .ok()
                .flatten()
                .as_deref()
                != f.old.as_deref()
        });
    for (done, created) in failing_replace
        .into_iter()
        .chain(written.iter().rev().cloned())
    {
        let restored = match (&done.old, created) {
            (Some(old), _) => publish(project, &done.dst, old),
            (None, Some(id)) => fs_guard::guarded_remove_file_if_ours(project, &done.dst, &id)
                .map_err(|e| e.to_string()),
            // A create with no identity cannot be proven ours: left alone.
            (None, None) => Err("not proven to be the file we created".to_string()),
        };
        match restored {
            Ok(()) => restored_any = true,
            Err(_) => undone.push(done.rel.clone()),
        }
    }
    if !undone.is_empty() {
        format!(
            "{error}. These files could not be put back, restore them from {}: {}",
            backup_dir.unwrap_or("the backups"),
            undone.join(", ")
        )
    } else if restored_any {
        format!("{error}. Files already written were put back.")
    } else {
        error
    }
}

/// Every original is copied (and read back) before the first file is replaced;
/// a failure while replacing puts the already-replaced files back.
pub fn reinstall(
    project: &Path,
    template_docs: &Path,
    stamp: &str,
    selection: Option<&[String]>,
) -> Result<TemplateReinstallReport, String> {
    let (_, prepared) = prepare(project, template_docs)?;
    if let Some(paths) = selection {
        for path in paths {
            let offered = prepared
                .iter()
                .any(|p| p.action == ReinstallAction::Replace && &p.rel == path);
            if !offered {
                return Err(format!(
                    "{path} is not a file this reinstall offers to replace: nothing was touched"
                ));
            }
        }
    }
    let creating: Vec<String> = prepared
        .iter()
        .filter(|p| p.action == ReinstallAction::Create)
        .map(|p| p.rel.clone())
        .collect();
    let provenance_before = crate::core::doc_provenance::snapshot_tracked(project)?;
    crate::core::template_manifest::apply_renames_in(project, template_docs, Some(&creating))?;
    let (_, prepared) = prepare(project, template_docs)?;
    let chosen = |p: &Prepared| match (p.action, selection) {
        (ReinstallAction::Replace, Some(paths)) => paths.contains(&p.rel),
        (ReinstallAction::Replace, None) => p.kind == Some(ReinstallKind::DamagedOrTemplate),
        _ => false,
    };
    let root = project
        .canonicalize()
        .map_err(|e| format!("resolve project root: {e}"))?;
    let changing: Vec<&Prepared> = prepared.iter().filter(|p| chosen(p)).collect();

    let mut backup_dir = None;
    let mut replaced = Vec::new();
    if !changing.is_empty() {
        let dir = audit_state::new_template_backup_dir(project, stamp)?;
        for file in &changing {
            let old = file.old.as_deref().unwrap_or_default();
            let backup = dir.join(&file.rel);
            fs_guard::guarded_replace(&root, &backup, old)
                .map_err(|e| format!("backup of {}: {e}", file.rel))?;
            let read_back = fs_guard::guarded_read(&root, &backup).map_err(|e| e.to_string())?;
            if read_back.as_deref() != Some(old) {
                return Err(format!(
                    "backup of {} does not match the original: nothing was replaced",
                    file.rel
                ));
            }
            replaced.push(ReplacedFile {
                path: file.rel.clone(),
                backup: backup
                    .strip_prefix(&root)
                    .unwrap_or(&backup)
                    .to_string_lossy()
                    .replace('\\', "/"),
            });
        }
        backup_dir = Some(
            dir.strip_prefix(&root)
                .unwrap_or(&dir)
                .to_string_lossy()
                .replace('\\', "/"),
        );
    }

    let mut written: Vec<(&Prepared, Option<fs_guard::FileId>)> = Vec::new();
    for file in prepared
        .iter()
        .filter(|p| p.action == ReinstallAction::Create || chosen(p))
    {
        match write_planned(project, file) {
            Ok(created) => written.push((file, created)),
            Err(e) => return Err(roll_back(project, file, &written, backup_dir.as_deref(), e)),
        }
    }

    // Placeholders of the files this call wrote, scoped to those files only.
    let touched: Vec<PathBuf> = written.iter().map(|(p, _)| p.dst.clone()).collect();
    let _ = crate::core::docs_migration::prefill_files(project, &touched);
    let mut to_reconfirm = Vec::new();
    for (file, _) in &written {
        let (Some(old), Ok(after)) = (
            file.old
                .as_deref()
                .and_then(|b| std::str::from_utf8(b).ok()),
            std::fs::read_to_string(&file.dst),
        ) else {
            continue;
        };
        match crate::core::doc_provenance::note_write(
            project,
            &file.rel,
            old,
            &after,
            crate::core::doc_provenance::DocWriter::TemplateReinstall,
        ) {
            Ok(true) => to_reconfirm.push(file.rel.clone()),
            Ok(false) => {}
            Err(e) => tracing::warn!(
                "Human-validation provenance of {} not recorded: {e}",
                file.rel
            ),
        }
    }

    crate::core::template_manifest::record_install(project, template_docs)?;
    crate::core::doc_provenance::note_snapshot_writes(
        project,
        &provenance_before,
        crate::core::doc_provenance::DocWriter::TemplateReinstall,
    )
    .map_err(|e| {
        format!("Template files reinstalled, but provenance could not be recorded: {e}")
    })?;
    let mut by_action: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for file in &prepared {
        let key = match file.action {
            ReinstallAction::Create => "created",
            ReinstallAction::Unchanged => "unchanged",
            ReinstallAction::Replace => continue,
        };
        by_action.entry(key).or_default().push(file.rel.clone());
    }
    Ok(TemplateReinstallReport {
        backup_dir,
        replaced,
        created: by_action.remove("created").unwrap_or_default(),
        unchanged: by_action.remove("unchanged").unwrap_or_default(),
        skipped: prepared
            .iter()
            .filter(|p| p.action == ReinstallAction::Replace && !chosen(p))
            .map(|p| p.rel.clone())
            .collect(),
        to_reconfirm,
        placeholder_docs: placeholder_docs(project, template_docs),
        audit_status: scanner::detect_audit_status(&project.to_string_lossy()),
    })
}

async fn project_root(state: &AppState, id: String) -> Result<(PathBuf, String), String> {
    let project = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
        .map_err(|e| format!("DB error: {e}"))?
        .ok_or("Project not found")?;
    let root = scanner::resolve_host_path(&project.path);
    if !root.exists() {
        return Err(format!("Project path not found: {}", root.display()));
    }
    Ok((root, project.id))
}

fn bridge_refusal<T: Serialize>() -> Json<ApiResponse<T>> {
    Json(ApiResponse::err_coded(
        ApiErrorCode::Validation,
        "Only a human can reinstall the template files",
    ))
}

pub(crate) async fn plan_for(
    state: &AppState,
    id: String,
    template_docs: PathBuf,
) -> Json<ApiResponse<TemplateReinstallPlan>> {
    let (root, project_id) = match project_root(state, id).await {
        Ok(found) => found,
        Err(e) => return Json(ApiResponse::err(e)),
    };
    let running = state
        .audit_tracker
        .lock()
        .map(|t| t.leased.contains(&project_id))
        .unwrap_or(true);
    let result =
        tokio::task::spawn_blocking(move || plan_reinstall(&root, &template_docs, running))
            .await
            .unwrap_or_else(|e| Err(format!("Task failed: {e}")));
    match result {
        Ok(plan) => Json(ApiResponse::ok(plan)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

pub(crate) async fn reinstall_for(
    state: &AppState,
    id: String,
    template_docs: PathBuf,
    selection: Option<Vec<String>>,
) -> Json<ApiResponse<TemplateReinstallReport>> {
    let (root, project_id) = match project_root(state, id).await {
        Ok(found) => found,
        Err(e) => return Json(ApiResponse::err(e)),
    };
    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::acquire(state, &project_id).await {
            Ok(lease) => lease,
            Err(error) => {
                return Json(ApiResponse::err_coded(
                    crate::models::ApiErrorCode::Conflict,
                    error,
                ))
            }
        };
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let result = lease
        .blocking(move || {
            let report = reinstall(&root, &template_docs, &stamp, selection.as_deref())?;
            crate::agents::runner::fix_file_ownership(&root);
            Ok(report)
        })
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {e}")));
    match result {
        Ok(report) => Json(ApiResponse::ok(report)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/projects/:id/template-reinstall/plan — what a reinstall would replace.
pub async fn template_reinstall_plan(
    State(state): State<AppState>,
    AxPath(id): AxPath<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    marker: crate::core::bridge_token::AgentMark,
) -> Json<ApiResponse<TemplateReinstallPlan>> {
    if crate::core::bridge_token::agent_authored(&bridge, &marker) {
        return bridge_refusal();
    }
    plan_for(
        &state,
        id,
        super::template::resolve_templates_dir().join("docs"),
    )
    .await
}

/// POST /api/projects/:id/template-reinstall — back up, then replace the template files.
pub async fn template_reinstall(
    State(state): State<AppState>,
    AxPath(id): AxPath<String>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    marker: crate::core::bridge_token::AgentMark,
    body: Option<Json<TemplateReinstallRequest>>,
) -> Json<ApiResponse<TemplateReinstallReport>> {
    if crate::core::bridge_token::agent_authored(&bridge, &marker) {
        return bridge_refusal();
    }
    reinstall_for(
        &state,
        id,
        super::template::resolve_templates_dir().join("docs"),
        body.and_then(|Json(b)| b.paths),
    )
    .await
}

#[cfg(test)]
#[path = "template_reinstall_test.rs"]
mod tests;

#[cfg(all(test, unix, feature = "full-tests"))] // swaps a folder for a symlink (std::os::unix)
mod swap_tests {
    use super::*;
    use crate::core::fs_guard::swap_probe;
    use std::os::unix::fs::PermissionsExt;

    /// A two-file template (`AGENTS.md`, then `architecture/overview.md`: the
    /// reinstall writes them in that order) over a project whose two files
    /// are damaged, so both are replaced by default.
    fn damaged_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let template = tmp.path().join("templates/docs");
        std::fs::create_dir_all(template.join("architecture")).unwrap();
        std::fs::write(template.join("AGENTS.md"), "# Template entry\n").unwrap();
        std::fs::write(
            template.join("architecture/overview.md"),
            "# Template overview\n",
        )
        .unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join("docs/architecture")).unwrap();
        std::fs::write(project.join("docs/AGENTS.md"), "damaged {{ENTRY}}").unwrap();
        std::fs::write(
            project.join("docs/architecture/overview.md"),
            "damaged {{OVERVIEW}}",
        )
        .unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        (tmp, project, template, external)
    }

    #[test]
    fn a_failure_on_the_second_file_puts_the_first_back_exactly() {
        let (_tmp, project, template, external) = damaged_project();
        let folder = project.join("docs/architecture");
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        // As root a read-only folder still takes writes: nothing to prove here.
        let writable = std::fs::write(folder.join("probe"), "").is_ok();
        if writable {
            std::fs::remove_file(folder.join("probe")).unwrap();
        }
        let result = reinstall(&project, &template, "rollback", None);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        if writable {
            return;
        }
        let error = result.unwrap_err();
        assert!(error.contains("were put back"), "{error}");
        assert_eq!(
            std::fs::read_to_string(project.join("docs/AGENTS.md")).unwrap(),
            "damaged {{ENTRY}}",
            "the first file is back to its original bytes"
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("overview.md")).unwrap(),
            "damaged {{OVERVIEW}}"
        );
        assert!(std::fs::read_dir(&external).unwrap().next().is_none());
    }

    #[test]
    fn a_docs_folder_swapped_after_the_first_write_never_receives_the_rest_nor_the_rollback() {
        let (_tmp, project, template, external) = damaged_project();
        std::fs::write(external.join("AGENTS.md"), "KEEP").unwrap();
        // The folder is swapped for a link the moment the first file is
        // written through it (held as an fd): that write stays inside the
        // root, the second file and the rollback are refused.
        swap_probe::arm(&project.join("docs"), &external);
        let error = reinstall(&project, &template, "swap", None).unwrap_err();
        assert!(swap_probe::fired(), "the swap really happened");
        assert!(
            error.contains("could not be put back"),
            "the message must not claim a rollback that did not happen: {error}"
        );
        assert!(!error.contains("were put back"), "{error}");
        assert_eq!(
            std::fs::read_to_string(external.join("AGENTS.md")).unwrap(),
            "KEEP",
            "an outside file of the same name is never touched"
        );
        assert_eq!(std::fs::read_dir(&external).unwrap().count(), 1);
    }

    #[test]
    fn a_create_never_overwrites_a_file_that_appeared_since_the_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join("docs")).unwrap();
        let file = Prepared {
            rel: "docs/new.md".into(),
            dst: project.join("docs/new.md"),
            action: ReinstallAction::Create,
            kind: None,
            old: None,
            new: b"template".to_vec(),
            human: Vec::new(),
            validated: false,
        };
        std::fs::write(&file.dst, "someone else").unwrap();
        let error = write_planned(&project, &file).unwrap_err();
        assert!(error.contains("appeared since the plan"), "{error}");
        assert_eq!(std::fs::read_to_string(&file.dst).unwrap(), "someone else");
        // Free name: created.
        std::fs::remove_file(&file.dst).unwrap();
        write_planned(&project, &file).unwrap();
        assert_eq!(std::fs::read_to_string(&file.dst).unwrap(), "template");
    }

    fn prepared(
        project: &Path,
        rel: &str,
        action: ReinstallAction,
        old: Option<&[u8]>,
    ) -> Prepared {
        Prepared {
            rel: rel.into(),
            dst: project.join(rel),
            action,
            kind: None,
            old: old.map(<[u8]>::to_vec),
            new: b"template".to_vec(),
            human: Vec::new(),
            validated: false,
        }
    }

    #[test]
    fn a_rollback_removes_a_created_file_only_while_it_is_the_one_we_created() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join("docs")).unwrap();
        let mine = prepared(&project, "docs/mine.md", ReinstallAction::Create, None);
        let theirs = prepared(&project, "docs/theirs.md", ReinstallAction::Create, None);
        let failing = prepared(&project, "docs/failing.md", ReinstallAction::Create, None);
        let written = vec![
            (&mine, write_planned(&project, &mine).unwrap()),
            (&theirs, write_planned(&project, &theirs).unwrap()),
        ];
        assert!(written.iter().all(|(_, id)| id.is_some()));
        // A concurrent writer replaces one of the two before the rollback.
        std::fs::remove_file(&theirs.dst).unwrap();
        // Same size as ours: only the identity can tell them apart.
        std::fs::write(&theirs.dst, "THEIRS!!").unwrap();
        let message = roll_back(&project, &failing, &written, Some("backups"), "boom".into());
        assert!(!mine.dst.exists(), "our own file is removed");
        assert_eq!(
            std::fs::read_to_string(&theirs.dst).unwrap(),
            "THEIRS!!",
            "a file that is no longer ours is never deleted"
        );
        assert!(
            message.contains("could not be put back") && message.contains("docs/theirs.md"),
            "{message}"
        );
        assert!(!message.contains("docs/mine.md"), "{message}");
    }

    #[test]
    fn a_rollback_decision_never_reads_through_a_link_planted_at_the_name() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join("docs")).unwrap();
        let external = tmp.path().join("external");
        std::fs::create_dir_all(&external).unwrap();
        // The outside file holds exactly the original bytes: a path read would
        // call the name "unchanged" and skip the restore.
        std::fs::write(external.join("copy.md"), "ORIGINAL").unwrap();
        let failing = prepared(
            &project,
            "docs/a.md",
            ReinstallAction::Replace,
            Some(b"ORIGINAL"),
        );
        std::os::unix::fs::symlink(external.join("copy.md"), &failing.dst).unwrap();
        let message = roll_back(&project, &failing, &[], None, "boom".into());
        assert!(message.contains("could not be put back"), "{message}");
        assert_eq!(
            std::fs::read_to_string(external.join("copy.md")).unwrap(),
            "ORIGINAL",
            "the outside file is untouched"
        );
        assert!(std::fs::symlink_metadata(&failing.dst)
            .unwrap()
            .is_symlink());
    }
}
