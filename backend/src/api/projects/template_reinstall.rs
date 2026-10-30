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

fn publish(dst: &Path, bytes: &[u8], like: Option<&Path>) -> Result<(), String> {
    use std::io::Write;
    let parent = dst.parent().ok_or("no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    let name = dst.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = parent.join(format!(".{name}.kronn-reinstall"));
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write {}: {e}", dst.display()));
    }
    if let Some(perms) = like
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.permissions())
    {
        let _ = std::fs::set_permissions(&tmp, perms);
    }
    std::fs::rename(&tmp, dst).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("replace {}: {e}", dst.display())
    })
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
            if let Some(parent) = backup.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("backup of {}: {e}", file.rel))?;
            }
            std::fs::write(&backup, old).map_err(|e| format!("backup of {}: {e}", file.rel))?;
            if std::fs::read(&backup).map_err(|e| e.to_string())? != old {
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

    let mut written: Vec<&Prepared> = Vec::new();
    for file in prepared
        .iter()
        .filter(|p| p.action == ReinstallAction::Create || chosen(p))
    {
        fs_guard::assert_contained_no_symlink(project, &file.dst)?;
        if let Err(e) = publish(
            &file.dst,
            &file.new,
            file.old.is_some().then_some(file.dst.as_path()),
        ) {
            let mut undone = Vec::new();
            for done in written.iter().rev() {
                let restored = match &done.old {
                    Some(old) => publish(&done.dst, old, Some(&done.dst)),
                    None => std::fs::remove_file(&done.dst).map_err(|e| e.to_string()),
                };
                if restored.is_err() {
                    undone.push(done.rel.clone());
                }
            }
            return Err(if undone.is_empty() {
                format!("{e}. Files already replaced were put back.")
            } else {
                format!(
                    "{e}. These files could not be put back, restore them from {}: {}",
                    backup_dir.as_deref().unwrap_or("the backups"),
                    undone.join(", ")
                )
            });
        }
        written.push(file);
    }

    // Placeholders of the files this call wrote, scoped to those files only.
    let touched: Vec<PathBuf> = written.iter().map(|p| p.dst.clone()).collect();
    let _ = crate::core::docs_migration::prefill_files(project, &touched);
    let mut to_reconfirm = Vec::new();
    for file in &written {
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
