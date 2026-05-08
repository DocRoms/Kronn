// `POST /api/projects/clone` plus the URL-mangling helpers it relies on:
// HTTPS → SSH conversion when SSH keys are present, and PAT injection
// from the configured MCP sources when they aren't. The cloned `.git`
// has its `origin` reset to the original (token-free) URL afterwards so
// secrets never persist on disk.

use axum::{extract::State, Json};
use chrono::Utc;
use uuid::Uuid;

use crate::core::cmd::sync_cmd;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

use super::{determine_parent_dir, enrich_audit_status};

/// Inject a token into an HTTPS git URL for authenticated cloning.
/// Returns the original URL unchanged if it's not HTTPS or no matching provider is found.
fn inject_token_into_url(url: &str, provider: &str, token: &str) -> Option<String> {
    if !url.starts_with("https://") {
        return None;
    }
    match provider {
        "github" if url.contains("github.com") => {
            Some(url.replacen("https://github.com", &format!("https://x-access-token:{}@github.com", token), 1))
        }
        "gitlab" if url.contains("gitlab") => {
            let real_token = token.split('|').next().unwrap_or(token);
            url.find("://").map(|i| {
                let after_scheme = &url[i + 3..];
                format!("https://oauth2:{}@{}", real_token, after_scheme)
            })
        }
        _ => None,
    }
}

/// Convert an HTTPS git URL to its SSH equivalent.
fn https_to_ssh(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let slash_pos = rest.find('/')?;
    let host = &rest[..slash_pos];
    let path = &rest[slash_pos + 1..];
    Some(format!("git@{}:{}", host, path))
}

/// For HTTPS clone URLs, inject a Personal Access Token into the URL so that
/// `git clone` works inside Docker where no interactive credential helper is
/// available.  Falls back to converting HTTPS → SSH when keys are mounted.
async fn inject_clone_auth(url: &str, state: &AppState) -> String {
    if !url.starts_with("https://") {
        return url.to_string();
    }

    let sources = crate::api::discover::find_all_provider_sources(state).await;

    // Try to inject a token from configured MCP sources
    for (source, token) in &sources {
        if let Some(authed_url) = inject_token_into_url(url, &source.provider, token) {
            return authed_url;
        }
    }

    // No token found — try SSH fallback if SSH keys are available
    if url.contains("github.com") || url.contains("gitlab.com") {
        let ssh_dir = std::env::var("HOME").ok().map(|h| std::path::PathBuf::from(h).join(".ssh"));
        let has_ssh_keys = ssh_dir
            .map(|d| d.join("id_rsa").exists() || d.join("id_ed25519").exists())
            .unwrap_or(false);
        if has_ssh_keys {
            if let Some(ssh_url) = https_to_ssh(url) {
                return ssh_url;
            }
        }
    }

    url.to_string()
}

/// POST /api/projects/clone
pub async fn clone_project(
    State(state): State<AppState>,
    Json(req): Json<CloneProjectRequest>,
) -> Json<ApiResponse<CloneProjectResponse>> {
    let url = req.url.trim().to_string();
    if url.is_empty() {
        return Json(ApiResponse::err("Repository URL is required"));
    }

    // Extract name from URL: last segment, remove .git suffix
    let repo_name = req.name.as_deref()
        .filter(|n| !n.trim().is_empty())
        .map(|n| n.trim().to_string())
        .unwrap_or_else(|| {
            url.trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("repo")
                .trim_end_matches(".git")
                .to_string()
        });

    if repo_name.is_empty() {
        return Json(ApiResponse::err("Could not determine repository name from URL"));
    }

    // Determine parent directory (same logic as bootstrap)
    let parent_dir = determine_parent_dir(&state).await;
    let parent_dir = match parent_dir {
        Ok(d) => d,
        Err(e) => return Json(ApiResponse::err(e)),
    };

    // Sanitize name for directory (kebab-case)
    let dir_name: String = repo_name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    let project_path = format!("{}/{}", parent_dir, dir_name);
    // Resolve the *parent* directory (which exists) to get the correct Docker mount path,
    // then append the dir name. resolve_host_path on the full path would fail because
    // the target doesn't exist yet and the exists() check would fall through to the raw host path.
    let resolved_parent = scanner::resolve_host_path(&parent_dir);
    let host_path = resolved_parent.join(&dir_name);

    if host_path.exists() {
        return Json(ApiResponse::err(format!("Directory already exists: {}", project_path)));
    }

    // Git clone — inject auth token for HTTPS URLs when available
    let clone_url = inject_clone_auth(&url, &state).await;
    let original_url = url.clone();
    let clone_path = host_path.clone();
    let clone_path2 = host_path.clone();
    let clone_result = tokio::task::spawn_blocking(move || {
        sync_cmd("git")
            .args(["clone", &clone_url, &clone_path.to_string_lossy()])
            .output()
    }).await;

    match clone_result {
        Ok(Ok(output)) if !output.status.success() => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Json(ApiResponse::err(format!("git clone failed: {}", stderr.trim())));
        }
        Ok(Err(e)) => return Json(ApiResponse::err(format!("Failed to run git: {}", e))),
        Err(e) => return Json(ApiResponse::err(format!("Task failed: {}", e))),
        _ => {} // success
    }

    // Reset the remote URL to the original (without embedded token) so that
    // secrets don't persist in .git/config and don't leak via git remote scans.
    let _ = tokio::task::spawn_blocking(move || {
        sync_cmd("git")
            .args(["remote", "set-url", "origin", &original_url])
            .current_dir(&clone_path2)
            .output()
    }).await;

    // Create project in DB
    let project_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let mut project = Project {
        id: project_id.clone(),
        name: repo_name.clone(),
        path: project_path.clone(),
        repo_url: Some(url),
        token_override: None,
        ai_config: AiConfigStatus {
            detected: false,
            configs: vec![],
        },
        audit_status: crate::models::AiAuditStatus::default(),
        ai_todo_count: 0,
        needs_docs_migration: false,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        created_at: now,
        updated_at: now,
    };
    enrich_audit_status(&mut project);

    let p = project.clone();
    if let Err(e) = state.db.with_conn(move |conn| crate::db::projects::insert_project(conn, &p)).await {
        return Json(ApiResponse::err(format!("DB error: {}", e)));
    }

    // Auto-detect skills
    let detected = crate::api::audit::detect_project_skills(&host_path);
    if !detected.is_empty() {
        let pid = project_id.clone();
        let skills = detected.clone();
        if let Err(e) = state.db.with_conn(move |conn| {
            crate::db::projects::update_project_default_skills(conn, &pid, &skills)
        }).await {
            tracing::error!("Failed to update project default skills: {e}");
        }
    }

    Json(ApiResponse::ok(CloneProjectResponse {
        project_id,
        discussion_id: None,
    }))
}

#[cfg(test)]
mod clone_auth_tests {
    use super::*;

    #[test]
    fn inject_github_token() {
        let url = "https://github.com/org/repo.git";
        let result = inject_token_into_url(url, "github", "ghp_abc123").unwrap();
        assert_eq!(result, "https://x-access-token:ghp_abc123@github.com/org/repo.git");
    }

    #[test]
    fn inject_gitlab_token() {
        let url = "https://gitlab.com/org/repo.git";
        let result = inject_token_into_url(url, "gitlab", "glpat-xyz|https://gitlab.com").unwrap();
        assert_eq!(result, "https://oauth2:glpat-xyz@gitlab.com/org/repo.git");
    }

    #[test]
    fn inject_gitlab_token_no_pipe() {
        let url = "https://gitlab.example.com/org/repo.git";
        let result = inject_token_into_url(url, "gitlab", "glpat-xyz").unwrap();
        assert_eq!(result, "https://oauth2:glpat-xyz@gitlab.example.com/org/repo.git");
    }

    #[test]
    fn inject_wrong_provider_returns_none() {
        let url = "https://github.com/org/repo.git";
        assert!(inject_token_into_url(url, "gitlab", "token").is_none());
    }

    #[test]
    fn inject_ssh_url_returns_none() {
        let url = "git@github.com:org/repo.git";
        assert!(inject_token_into_url(url, "github", "token").is_none());
    }

    #[test]
    fn https_to_ssh_github() {
        let url = "https://github.com/org/repo.git";
        assert_eq!(https_to_ssh(url).unwrap(), "git@github.com:org/repo.git");
    }

    #[test]
    fn https_to_ssh_gitlab() {
        let url = "https://gitlab.com/group/subgroup/repo.git";
        assert_eq!(https_to_ssh(url).unwrap(), "git@gitlab.com:group/subgroup/repo.git");
    }

    #[test]
    fn https_to_ssh_not_https() {
        assert!(https_to_ssh("git@github.com:org/repo.git").is_none());
    }
}
