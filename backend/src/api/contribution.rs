//! `GET /api/contribution/preflight` (KT-1643): whether this machine can
//! contribute to a repository through the autoCode Issue workflow, and how the
//! branch will be published (straight, or through the user's fork).
//!
//! The facts are read here (`gh`, the project list, the installed agents); the
//! decisions are `core::contribution::evaluate`, which is pure and tested.

use std::time::Duration;

use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;

use crate::core::contribution::{
    self, Facts, GhAuth, PreflightReport, RepoProject, RepoProjectFact,
};
use crate::{
    models::{ApiErrorCode, ApiResponse},
    AppState,
};

/// `gh` talks to the network: bounded, so a stuck call is an answer.
const GH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
pub struct PreflightQuery {
    /// `owner/name` of the repository the pull request targets (the
    /// workflow's `repo` setting).
    pub repo: String,
    /// The branch the pull request targets; default `main`.
    #[serde(default)]
    pub target_branch: Option<String>,
    /// Review rounds per task; default 10.
    #[serde(default)]
    pub max_review_rounds: Option<u32>,
    /// The branch about to be published, for the `--head` the plan names.
    #[serde(default)]
    pub branch: Option<String>,
}

/// A branch name safe to hand to `git` and `gh` as one argument.
fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && branch.len() <= 200
        && !branch.starts_with('-')
        && !branch.contains("..")
        && branch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
}

/// `(success, stdout + stderr)` of one `gh` call; `None` when `gh` cannot run
/// (not installed, or no answer within the bound).
async fn gh(args: &[&str]) -> Option<(bool, String)> {
    let mut cmd = crate::core::cmd::async_cmd("gh", crate::core::child_env::ChildRoute::GitHost);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(GH_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Some((output.status.success(), text))
}

/// `(gh installed, login, repository push right, fork exists)`.
async fn gh_facts(repo: &str) -> (bool, Option<GhAuth>, Option<bool>, bool) {
    if gh(&["--version"]).await.is_none() {
        return (false, None, None, false);
    }
    let status = gh(&["auth", "status"]).await;
    if !status.as_ref().is_some_and(|(ok, _)| *ok) {
        return (true, None, None, false);
    }
    let scopes = status.and_then(|(_, text)| contribution::parse_token_scopes(&text));
    let login = gh(&["api", "user", "--jq", ".login"])
        .await
        .filter(|(ok, _)| *ok)
        .map(|(_, text)| text.trim().to_string())
        .filter(|login| !login.is_empty());
    let can_push = gh(&["api", &format!("repos/{repo}"), "--jq", ".permissions.push"])
        .await
        .filter(|(ok, _)| *ok)
        .and_then(|(_, text)| match text.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        });
    // Only a contributor without push rights needs the fork.
    let fork_exists = match (&login, can_push) {
        (Some(login), Some(false)) => {
            let name = repo.rsplit('/').next().unwrap_or(repo);
            gh(&["api", &format!("repos/{login}/{name}"), "--jq", ".fork"])
                .await
                .is_some_and(|(ok, text)| ok && text.trim() == "true")
        }
        _ => false,
    };
    (true, Some(GhAuth { login, scopes }), can_push, fork_exists)
}

pub async fn preflight(
    State(state): State<AppState>,
    Query(query): Query<PreflightQuery>,
) -> Json<ApiResponse<PreflightReport>> {
    if contribution::normalize_slug(&query.repo).is_none() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!(
                "`{}` is not a repository: write `owner/name`",
                query.repo.trim()
            ),
        ));
    }
    let repo = query.repo.trim().trim_matches('/').to_string();
    let target_branch = query
        .target_branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or(contribution::DEFAULT_TARGET_BRANCH)
        .to_string();
    let branch = query
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or("kronn/contribution")
        .to_string();
    if !valid_branch(&target_branch) || !valid_branch(&branch) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "a branch name holds letters, digits and `/ - _ .` only".to_string(),
        ));
    }
    let max_review_rounds = query
        .max_review_rounds
        .unwrap_or(contribution::DEFAULT_MAX_REVIEW_ROUNDS)
        .clamp(1, crate::models::MAX_REVIEW_ROUNDS_INTEGER);

    let (gh_installed, gh_auth, can_push, fork_exists) = gh_facts(&repo).await;

    let project_repo = repo.clone();
    let project = state
        .db
        .with_read_conn(move |conn| {
            let projects = crate::db::projects::list_projects(conn)?;
            let remotes = |project: &crate::models::Project| {
                let path = crate::core::scanner::resolve_host_path(&project.path);
                crate::core::github_connection::git_remote_urls(&path)
            };
            Ok(
                match contribution::find_project_by_repo(&projects, &project_repo, &remotes) {
                    RepoProject::Found(project) => RepoProjectFact::Linked {
                        path_exists: crate::core::scanner::resolve_host_path(&project.path)
                            .exists(),
                        id: project.id,
                        name: project.name,
                    },
                    RepoProject::Ambiguous(names) => RepoProjectFact::Ambiguous(names),
                    RepoProject::Missing => RepoProjectFact::Missing,
                },
            )
        })
        .await
        .unwrap_or(RepoProjectFact::Missing);

    let disabled = state.config.read().await.disabled_agents.clone();
    let local_workers = crate::agents::detect_all_cached(false)
        .await
        .into_iter()
        .filter(|agent| {
            agent.installed
                && !disabled.contains(&agent.agent_type)
                && agent.auth_ready != Some(false)
                && !contribution::is_remote_agent(&agent.agent_type)
        })
        .map(|agent| agent.agent_type)
        .collect();

    let facts = Facts {
        gh_installed,
        gh_auth,
        can_push,
        fork_exists,
        project: Some(project),
        local_workers,
    };
    Json(ApiResponse::ok(contribution::evaluate(
        &repo,
        &target_branch,
        max_review_rounds,
        &branch,
        &facts,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    async fn refused(query: PreflightQuery) -> Option<String> {
        let Json(response) = preflight(State(state()), Query(query)).await;
        assert!(!response.success);
        assert_eq!(response.error_code.as_deref(), Some("validation"));
        response.error
    }

    #[tokio::test]
    async fn a_repository_that_is_not_a_slug_is_refused_before_any_gh_call() {
        for repo in ["", "not-a-slug", "a/b/c", "a/b;rm -rf", "a b/c"] {
            let error = refused(PreflightQuery {
                repo: repo.into(),
                target_branch: None,
                max_review_rounds: None,
                branch: None,
            })
            .await;
            assert!(error.unwrap().contains("owner/name"), "{repo:?}");
        }
    }

    #[tokio::test]
    async fn a_branch_that_could_pass_as_an_option_is_refused() {
        for (target, branch) in [
            (Some("--delete"), None),
            (Some("main"), Some("a..b")),
            (None, Some("x y")),
        ] {
            let error = refused(PreflightQuery {
                repo: "acme/widgets".into(),
                target_branch: target.map(String::from),
                max_review_rounds: None,
                branch: branch.map(String::from),
            })
            .await;
            assert!(
                error.unwrap().contains("branch name"),
                "{target:?} {branch:?}"
            );
        }
    }

    #[test]
    fn a_branch_name_is_one_safe_argument() {
        for good in ["main", "autocode/kt-1643", "release/0.15.3", "a_b-c.d"] {
            assert!(valid_branch(good), "{good}");
        }
        for bad in ["", "-x", "a..b", "a b", "a;b", "a\nb", "$(id)"] {
            assert!(!valid_branch(bad), "{bad:?}");
        }
    }
}
