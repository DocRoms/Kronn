//! KT-1750 — what the project card shows about a project's commit identity and
//! sign-off: what the repository says by itself (the default the settings fall
//! back to), where each answer comes from, and what Kronn actually commits with.
//!
//! The settings themselves are saved with the project workspace
//! (`PUT /api/projects/{id}/workspace`).

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;
use ts_rs::TS;

use crate::core::commit_policy::CommitPolicy;
use crate::models::{ApiErrorCode, ApiResponse, ProjectCommitSettings};
use crate::AppState;

/// One resolved policy, with where each half comes from.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct CommitPolicyView {
    pub name: String,
    pub email: String,
    /// `setting`, `kronn_config`, `repository`, `conditional`, `kronn_global`, `global` or
    /// `fallback`.
    pub identity_source: String,
    pub signoff: bool,
    /// `setting`, `kronn_config`, `briefing`, `dco_file`, `commit_msg_hook`,
    /// `history`, `kronn_global` or `default`.
    pub signoff_source: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ProjectCommitPolicy {
    /// What the card saved.
    pub settings: ProjectCommitSettings,
    /// What the project inherits when its card says nothing: the repository's
    /// own answers, then Settings > Workers', then the machine's git config.
    pub detected: CommitPolicyView,
    /// What Kronn commits with: the settings over the detection.
    pub effective: CommitPolicyView,
    /// The effective identity is the global git config's, which no project
    /// decided: a personal address on a repository that may not be personal.
    pub global_identity: bool,
}

impl CommitPolicyView {
    pub fn of(policy: &CommitPolicy) -> Self {
        Self {
            name: policy.identity.name.clone(),
            email: policy.identity.email.clone(),
            identity_source: policy.identity_source.as_str().to_string(),
            signoff: policy.signoff,
            signoff_source: policy.signoff_source.as_str().to_string(),
        }
    }
}

/// The card view of the project at `repo`, with `settings` laid over the
/// detection.
pub fn view_of(repo: &std::path::Path, settings: &ProjectCommitSettings) -> ProjectCommitPolicy {
    let detected = CommitPolicy::detect(repo);
    let effective = detected.clone().with_settings(settings);
    ProjectCommitPolicy {
        settings: settings.clone(),
        detected: CommitPolicyView::of(&detected),
        effective: CommitPolicyView::of(&effective),
        global_identity: effective.uses_global_identity(),
    }
}

/// GET /api/projects/{id}/commit-policy
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ProjectCommitPolicy>> {
    let project = state
        .db
        .with_read_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await;
    let project = match project {
        Ok(Some(project)) => project,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Project not found",
            ))
        }
        Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
    };
    let repo = crate::core::scanner::resolve_host_path(&project.path);
    if !repo.exists() {
        return Json(ApiResponse::err(format!(
            "Project path not found: {}",
            repo.display()
        )));
    }
    let settings = project
        .workspace
        .map(|workspace| workspace.commit)
        .unwrap_or_default();
    // Several `git` runs: off the async workers.
    match tokio::task::spawn_blocking(move || view_of(&repo, &settings)).await {
        Ok(view) => Json(ApiResponse::ok(view)),
        Err(error) => Json(ApiResponse::err(format!("commit policy failed: {error}"))),
    }
}
