//! 0.8.7 — Endpoints for the anti-hallu migration of existing projects.
//!
//! - `POST /api/projects/:id/anti-hallu/inject` — invoke the deterministic
//!   STEP 0 (`audit::anti_hallu_step::apply`) on a project's `docs/AGENTS.md`,
//!   inserting or refreshing the `<!-- kronn:section name="anti-hallu" -->`
//!   block. Idempotent. The audit pipeline already calls this at the start
//!   of every audit run — this endpoint is the explicit "1-click migrate
//!   my pre-0.8.7 projects without rerunning the full audit" path.
//!
//! - `POST /api/projects/:id/redirectors/sync` — re-copy the redirector
//!   files (CLAUDE.md, GEMINI.md, AGENTS.md, .cursorrules, etc.) from the
//!   binary's `templates/` directory into the project. Idempotent. Adds
//!   a `<!-- kronn:pointer v1 -->` marker on the line above the body for
//!   future drift detection. Useful when (a) a redirector was deleted by
//!   accident, or (b) the template added a new redirector (.windsurfrules
//!   in 0.8.7 etc.) that legacy projects don't have.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::core::scanner;
use crate::models::*;
use crate::AppState;

#[derive(Debug, serde::Serialize)]
pub struct AntiHalluStatusResponse {
    /// Whether the `<!-- kronn:section name="anti-hallu" -->` marker is
    /// present in the project's `docs/AGENTS.md`.
    pub present: bool,
    /// The `audit="YYYY-MM-DD"` attribute value, if the marker is
    /// present and carries the attribute. None if the section is
    /// missing OR if the marker lacks the attribute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_date: Option<String>,
    /// Whether the file exists at all. If false, `present` is also false
    /// and the project needs `install_template` / `bootstrap` first.
    pub file_exists: bool,
}

/// GET /api/projects/:id/anti-hallu/status
///
/// Lightweight check : does this project's `docs/AGENTS.md` already
/// carry the anti-hallu canonical section ? Frontend reads this to
/// decide whether to show "✓ Anti-hallu v1" (present) vs "⚠ inject"
/// (missing). Cheap : one FS read, no agent call.
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AntiHalluStatusResponse>> {
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path = scanner::resolve_host_path(&project.path);
    let docs_dir = scanner::detect_docs_dir(&project_path);
    let agents_md = docs_dir.join("AGENTS.md");
    if !agents_md.is_file() {
        return Json(ApiResponse::ok(AntiHalluStatusResponse {
            present: false,
            audit_date: None,
            file_exists: false,
        }));
    }
    let content = match std::fs::read_to_string(&agents_md) {
        Ok(c) => c,
        Err(_) => {
            return Json(ApiResponse::ok(AntiHalluStatusResponse {
                present: false,
                audit_date: None,
                file_exists: true,
            }));
        }
    };

    // Find the opening marker line + try to extract `audit="…"`.
    const MARKER_PREFIX: &str = "<!-- kronn:section name=\"anti-hallu\"";
    let opening = content
        .lines()
        .find(|l| l.trim_start().starts_with(MARKER_PREFIX));
    let present = opening.is_some();
    let audit_date = opening.and_then(|line| {
        let idx = line.find("audit=\"")?;
        let after = &line[idx + "audit=\"".len()..];
        let end = after.find('"')?;
        Some(after[..end].to_string())
    });

    Json(ApiResponse::ok(AntiHalluStatusResponse {
        present,
        audit_date,
        file_exists: true,
    }))
}

#[derive(Debug, serde::Serialize)]
pub struct AntiHalluInjectResponse {
    /// "ok" on success ; "error" on FS failure.
    pub status: &'static str,
    /// "inserted" / "refreshed" / "noop" / "missing".
    pub result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// POST /api/projects/:id/anti-hallu/inject
///
/// Inserts or refreshes the anti-hallu section in `docs/AGENTS.md`. Always
/// safe to call : on already-injected projects it just refreshes the audit
/// date, on fresh-bootstrap projects the template already carries the
/// section so this is a no-op, on legacy projects without the section it
/// inserts it at the top.
pub async fn inject(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AntiHalluInjectResponse>> {
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path = scanner::resolve_host_path(&project.path);
    match crate::api::audit::anti_hallu_step::apply(&project_path) {
        Ok(result) => {
            let label = match result {
                crate::api::audit::anti_hallu_step::AntiHalluApplyResult::Inserted => "inserted",
                crate::api::audit::anti_hallu_step::AntiHalluApplyResult::Refreshed => "refreshed",
                crate::api::audit::anti_hallu_step::AntiHalluApplyResult::NoOp => "noop",
                crate::api::audit::anti_hallu_step::AntiHalluApplyResult::FileMissing => "missing",
            };
            Json(ApiResponse::ok(AntiHalluInjectResponse {
                status: "ok",
                result: label,
                error: None,
            }))
        }
        Err(e) => Json(ApiResponse::ok(AntiHalluInjectResponse {
            status: "error",
            result: "error",
            error: Some(e.to_string()),
        })),
    }
}

#[derive(Debug, serde::Serialize)]
pub struct RedirectorsSyncResponse {
    pub status: &'static str,
    pub created: Vec<String>,
    pub already_present: Vec<String>,
    pub failed: Vec<String>,
}

/// POST /api/projects/:id/redirectors/sync
///
/// Idempotent re-copy of the redirector files from the binary templates
/// directory into the project root. Already-present files are left
/// untouched (we don't overwrite user-edited redirectors). Failures are
/// collected and returned so the UI can surface them.
pub async fn sync_redirectors(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<RedirectorsSyncResponse>> {
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path = scanner::resolve_host_path(&project.path);
    let template_dir = super::template::resolve_templates_dir();
    if !template_dir.exists() {
        return Json(ApiResponse::err(format!(
            "Templates directory not found: {}",
            template_dir.display()
        )));
    }

    let mut created = Vec::new();
    let mut already_present = Vec::new();
    let mut failed = Vec::new();

    for filename in crate::api::audit::AUDIT_REDIRECTOR_FILES {
        let src = template_dir.join(filename);
        let dst = project_path.join(filename);
        if !src.exists() {
            // Template file missing in the binary — should not happen, but
            // skip rather than fail the whole call.
            continue;
        }
        if dst.exists() {
            already_present.push((*filename).to_string());
            continue;
        }
        if let Some(parent) = dst.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                failed.push(format!("{filename}: mkdir failed: {e}"));
                continue;
            }
        }
        match std::fs::copy(&src, &dst) {
            Ok(_) => created.push((*filename).to_string()),
            Err(e) => failed.push(format!("{filename}: copy failed: {e}")),
        }
    }

    // Kiro steering (nested path, kept consistent with bootstrap.rs)
    let kiro_src = template_dir.join(".kiro/steering/instructions.md");
    let kiro_dst = project_path.join(".kiro/steering/instructions.md");
    if kiro_src.exists() && !kiro_dst.exists() {
        if let Some(parent) = kiro_dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::copy(&kiro_src, &kiro_dst) {
            Ok(_) => created.push(".kiro/steering/instructions.md".to_string()),
            Err(e) => failed.push(format!(".kiro/steering/instructions.md: copy failed: {e}")),
        }
    } else if kiro_dst.exists() {
        already_present.push(".kiro/steering/instructions.md".to_string());
    }

    Json(ApiResponse::ok(RedirectorsSyncResponse {
        status: if failed.is_empty() { "ok" } else { "partial" },
        created,
        already_present,
        failed,
    }))
}

#[cfg(test)]
mod tests {
    use crate::api::audit::anti_hallu_step::{self, AntiHalluApplyResult};

    #[test]
    fn inject_into_fresh_project_creates_section() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let docs_dir = tmp.path().join("docs");
        std::fs::create_dir_all(&docs_dir).unwrap();
        std::fs::write(
            docs_dir.join("AGENTS.md"),
            "# AI agent context — Entry point\n\nSome content\n",
        )
        .unwrap();

        let result = anti_hallu_step::apply(tmp.path()).unwrap();
        assert_eq!(result, AntiHalluApplyResult::Inserted);

        let content = std::fs::read_to_string(docs_dir.join("AGENTS.md")).unwrap();
        assert!(content.contains("kronn:section name=\"anti-hallu\""));
    }

    #[test]
    fn inject_is_idempotent_after_first_call() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let docs_dir = tmp.path().join("docs");
        std::fs::create_dir_all(&docs_dir).unwrap();
        std::fs::write(
            docs_dir.join("AGENTS.md"),
            "# AI agent context — Entry point\n\nSome content\n",
        )
        .unwrap();

        let r1 = anti_hallu_step::apply(tmp.path()).unwrap();
        assert_eq!(r1, AntiHalluApplyResult::Inserted);
        // Second call : section is already there at today's date → NoOp.
        let r2 = anti_hallu_step::apply(tmp.path()).unwrap();
        assert_eq!(r2, AntiHalluApplyResult::NoOp);
    }

    #[test]
    fn inject_on_missing_agents_md_reports_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // No docs/ dir at all
        let result = anti_hallu_step::apply(tmp.path()).unwrap();
        assert_eq!(result, AntiHalluApplyResult::FileMissing);
    }
}
