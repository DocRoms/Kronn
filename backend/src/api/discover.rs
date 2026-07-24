//! Remote repository discovery — GitHub/GitLab integration.

use axum::{extract::State, Json};

use crate::models::*;
use crate::AppState;

/// POST /api/projects/discover-repos
/// Discovers remote repositories from GitHub/GitLab that aren't yet tracked.
/// Accepts optional source_ids to filter which MCP configs to query.
pub async fn discover_repos(
    State(state): State<AppState>,
    Json(req): Json<DiscoverReposRequest>,
) -> Json<ApiResponse<DiscoverReposResponse>> {
    let mut all_repos: Vec<RemoteRepo> = vec![];
    let mut used_sources: Vec<String> = vec![];
    // 0.8.7 — accumulate per-source failures so the UI can surface them
    // instead of leaving the user to wonder why a configured provider
    // returned zero repos. Pre-fix this was only `tracing::warn!` and the
    // user had no signal at all (the GitLab silent-fail case).
    let mut errors: Vec<crate::models::DiscoverSourceError> = vec![];

    // Get existing projects to check "already_cloned"
    let existing = state
        .db
        .with_conn(crate::db::projects::list_projects)
        .await
        .unwrap_or_default();
    let existing_urls: std::collections::HashSet<String> = existing
        .iter()
        .filter_map(|p| p.repo_url.as_ref())
        .map(|u| normalize_repo_url(u))
        .collect();
    let existing_names: std::collections::HashSet<String> =
        existing.iter().map(|p| p.name.to_lowercase()).collect();

    // Get all available sources
    let all_sources = find_all_provider_sources(&state).await;
    let available_sources: Vec<RepoSource> = all_sources.iter().map(|(s, _)| s.clone()).collect();

    if all_sources.is_empty() {
        return Json(ApiResponse::err(
            "No GitHub or GitLab token found. Configure the GitHub or GitLab MCP with a Personal Access Token, or set GITHUB_TOKEN / GITLAB_TOKEN environment variable."
        ));
    }

    // Filter sources if specific IDs requested
    let sources_to_use: Vec<&(RepoSource, String)> = if req.source_ids.is_empty() {
        all_sources.iter().collect()
    } else {
        all_sources
            .iter()
            .filter(|(s, _)| req.source_ids.contains(&s.id))
            .collect()
    };

    tracing::info!(
        "discover_repos: requested source_ids={:?}, available={:?}, using={:?}",
        req.source_ids,
        available_sources
            .iter()
            .map(|s| format!("{}({})", s.label, s.id))
            .collect::<Vec<_>>(),
        sources_to_use
            .iter()
            .map(|(s, _)| format!("{}({})", s.label, s.id))
            .collect::<Vec<_>>(),
    );

    // Deduplicate repos by full_name (in case multiple tokens see the same repo)
    let mut seen_full_names = std::collections::HashSet::new();

    for (source, token_data) in &sources_to_use {
        match source.provider.as_str() {
            "github" => {
                let token_preview = if token_data.len() > 8 {
                    &token_data[..8]
                } else {
                    token_data
                };
                tracing::info!(
                    "discover_repos: querying GitHub source '{}' with token {}...",
                    source.label,
                    token_preview
                );
                match fetch_github_repos(token_data).await {
                    Ok(repos) => {
                        tracing::info!(
                            "discover_repos: source '{}' returned {} repos",
                            source.label,
                            repos.len()
                        );
                        used_sources.push(source.label.clone());
                        for r in repos {
                            if !seen_full_names.insert(r.full_name.clone()) {
                                continue; // skip duplicate
                            }
                            let already = existing_urls.contains(&normalize_repo_url(&r.clone_url))
                                || existing_urls.contains(&normalize_repo_url(&r.ssh_url))
                                || existing_names.contains(&r.name.to_lowercase());
                            all_repos.push(RemoteRepo {
                                already_cloned: already,
                                ..r
                            });
                        }
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        tracing::warn!(
                            "GitHub repo discovery failed for {}: {}",
                            source.label,
                            msg
                        );
                        errors.push(crate::models::DiscoverSourceError {
                            source_id: source.id.clone(),
                            source_label: source.label.clone(),
                            provider: "github".into(),
                            message: msg,
                        });
                    }
                }
            }
            "gitlab" => {
                let parts: Vec<&str> = token_data.splitn(2, '|').collect();
                let (token, api_url) = (parts[0], parts.get(1).unwrap_or(&"https://gitlab.com"));
                match fetch_gitlab_repos(token, api_url).await {
                    Ok(repos) => {
                        used_sources.push(source.label.clone());
                        for r in repos {
                            if !seen_full_names.insert(r.full_name.clone()) {
                                continue;
                            }
                            let already = existing_urls.contains(&normalize_repo_url(&r.clone_url))
                                || existing_urls.contains(&normalize_repo_url(&r.ssh_url))
                                || existing_names.contains(&r.name.to_lowercase());
                            all_repos.push(RemoteRepo {
                                already_cloned: already,
                                ..r
                            });
                        }
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        tracing::warn!(
                            "GitLab repo discovery failed for {}: {}",
                            source.label,
                            msg
                        );
                        errors.push(crate::models::DiscoverSourceError {
                            source_id: source.id.clone(),
                            source_label: source.label.clone(),
                            provider: "gitlab".into(),
                            message: msg,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    // Sort: not-cloned first, then by updated_at descending
    all_repos.sort_by(|a, b| {
        a.already_cloned
            .cmp(&b.already_cloned)
            .then(b.updated_at.cmp(&a.updated_at))
    });

    Json(ApiResponse::ok(DiscoverReposResponse {
        repos: all_repos,
        sources: used_sources,
        available_sources,
        errors,
    }))
}

/// Find all available token sources from MCP configs and env vars.
pub(crate) async fn find_all_provider_sources(state: &AppState) -> Vec<(RepoSource, String)> {
    let mut sources: Vec<(RepoSource, String)> = vec![];

    // Read encryption secret
    let config = state.config.read().await;
    let secret = config.encryption_secret.clone();
    drop(config);

    // Scan MCP configs for GitHub/GitLab tokens
    if let Some(secret) = &secret {
        let secret_clone = secret.clone();
        let configs = state
            .db
            .with_conn(move |conn| crate::db::mcps::list_configs(conn))
            .await
            .unwrap_or_default();

        for cfg in configs {
            if cfg.env_encrypted.is_empty() {
                continue;
            }
            let env = match crate::db::mcps::decrypt_env(&cfg.env_encrypted, &secret_clone) {
                Ok(e) => e,
                Err(_) => continue,
            };

            // GitHub MCP
            if cfg.server_id == "mcp-github" {
                if let Some(token) = env
                    .get("GITHUB_PERSONAL_ACCESS_TOKEN")
                    .filter(|v| !v.is_empty())
                {
                    let token_end = if token.len() > 4 {
                        &token[token.len() - 4..]
                    } else {
                        token
                    };
                    tracing::info!(
                        "discover: found GitHub MCP config '{}' (id={}) with token ...{}",
                        cfg.label,
                        cfg.id,
                        token_end
                    );
                    sources.push((
                        RepoSource {
                            id: cfg.id.clone(),
                            label: cfg.label.clone(),
                            provider: "github".into(),
                        },
                        token.clone(),
                    ));
                }
            }

            // GitLab MCP
            if cfg.server_id == "mcp-gitlab" {
                if let Some(token) = env
                    .get("GITLAB_PERSONAL_ACCESS_TOKEN")
                    .filter(|v| !v.is_empty())
                {
                    let api_url = env
                        .get("GITLAB_API_URL")
                        .filter(|v| !v.is_empty())
                        .cloned()
                        .unwrap_or_else(|| "https://gitlab.com".into());
                    // Encode the API URL in the token string with a separator
                    sources.push((
                        RepoSource {
                            id: cfg.id.clone(),
                            label: cfg.label.clone(),
                            provider: "gitlab".into(),
                        },
                        format!("{}|{}", token, api_url),
                    ));
                }
            }
        }
    }

    // Environment variable fallbacks
    if let Ok(token) = std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
        // Only add env source if there's no MCP config for GitHub already
        let has_gh = sources.iter().any(|(s, _)| s.provider == "github");
        if !has_gh {
            sources.push((
                RepoSource {
                    id: "env:github".into(),
                    label: "GitHub (env)".into(),
                    provider: "github".into(),
                },
                token,
            ));
        }
    }

    if let Ok(token) = std::env::var("GITLAB_TOKEN") {
        let has_gl = sources.iter().any(|(s, _)| s.provider == "gitlab");
        if !has_gl {
            let api_url =
                std::env::var("GITLAB_API_URL").unwrap_or_else(|_| "https://gitlab.com".into());
            sources.push((
                RepoSource {
                    id: "env:gitlab".into(),
                    label: "GitLab (env)".into(),
                    provider: "gitlab".into(),
                },
                format!("{}|{}", token, api_url),
            ));
        }
    }

    sources
}

/// Normalize a repo URL for comparison (strip .git suffix, lowercase, strip protocol prefix)
fn normalize_repo_url(url: &str) -> String {
    url.to_lowercase()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .replace("https://github.com/", "github:")
        .replace("https://gitlab.com/", "gitlab:")
        .replace("git@github.com:", "github:")
        .replace("git@gitlab.com:", "gitlab:")
        .to_string()
}

/// Fetch all repos for the authenticated GitHub user, including organization repos.
/// Bounded per request: discovery paginates in a loop inside a GET handler —
/// one stalled page (self-hosted GitLab that accepts and never answers) would
/// pin the handler forever; the UI modal has no cancel.
fn discovery_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default()
}

async fn fetch_github_repos(token: &str) -> Result<Vec<RemoteRepo>, String> {
    let client = discovery_client();
    let mut all_repos = vec![];
    let mut seen = std::collections::HashSet::new();

    // 1. User repos (owned, collaborated, org-member)
    let mut page = 1u32;
    loop {
        let url = format!(
            "https://api.github.com/user/repos?per_page=100&page={}&sort=updated&affiliation=owner,organization_member,collaborator",
            page
        );
        let repos = github_get_json_array(&client, &url, token).await?;
        if repos.is_empty() {
            break;
        }
        let done = repos.len() < 100;
        for r in &repos {
            let full_name = r["full_name"].as_str().unwrap_or("").to_string();
            if seen.insert(full_name.clone()) {
                all_repos.push(parse_github_repo(r));
            }
        }
        if done {
            break;
        }
        page += 1;
    }

    // 2. Organization repos — covers org repos the token can see but /user/repos may miss
    if let Ok(orgs) = github_get_json_array(
        &client,
        "https://api.github.com/user/orgs?per_page=100",
        token,
    )
    .await
    {
        for org in &orgs {
            let login = match org["login"].as_str() {
                Some(l) => l,
                None => continue,
            };
            tracing::info!("discover_repos: fetching org '{}' repos", login);
            let mut page = 1u32;
            loop {
                let url = format!(
                    "https://api.github.com/orgs/{}/repos?per_page=100&page={}&sort=updated",
                    login, page
                );
                let repos = match github_get_json_array(&client, &url, token).await {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(
                            "discover_repos: failed to list repos for org '{}': {}",
                            login,
                            e
                        );
                        break;
                    }
                };
                if repos.is_empty() {
                    break;
                }
                let done = repos.len() < 100;
                for r in &repos {
                    let full_name = r["full_name"].as_str().unwrap_or("").to_string();
                    if seen.insert(full_name.clone()) {
                        all_repos.push(parse_github_repo(r));
                    }
                }
                if done {
                    break;
                }
                page += 1;
            }
        }
    }

    Ok(all_repos)
}

/// Helper: GET a JSON array from GitHub API with auth headers.
async fn github_get_json_array(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Kronn/0.1")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub API error {}: {}", status, body));
    }

    resp.json()
        .await
        .map_err(|e| format!("Failed to parse GitHub response: {}", e))
}

/// Parse a GitHub repo JSON object into a RemoteRepo.
fn parse_github_repo(r: &serde_json::Value) -> RemoteRepo {
    RemoteRepo {
        name: r["name"].as_str().unwrap_or("").to_string(),
        full_name: r["full_name"].as_str().unwrap_or("").to_string(),
        clone_url: r["clone_url"].as_str().unwrap_or("").to_string(),
        ssh_url: r["ssh_url"].as_str().unwrap_or("").to_string(),
        description: r["description"].as_str().map(|s| s.to_string()),
        language: r["language"].as_str().map(|s| s.to_string()),
        stargazers_count: r["stargazers_count"].as_u64().unwrap_or(0) as u32,
        updated_at: r["updated_at"].as_str().unwrap_or("").to_string(),
        source: "github".into(),
        already_cloned: false,
    }
}

/// Fetch all repos for the authenticated GitLab user, including group repos.
async fn fetch_gitlab_repos(token: &str, api_url: &str) -> Result<Vec<RemoteRepo>, String> {
    let client = discovery_client();
    let base = api_url.trim_end_matches('/');
    let mut all_repos = vec![];
    let mut seen = std::collections::HashSet::new();

    // 1. User-owned projects
    gitlab_collect_projects(
        &client,
        token,
        &format!(
            "{}/api/v4/projects?owned=true&per_page=100&order_by=updated_at",
            base
        ),
        &mut all_repos,
        &mut seen,
    )
    .await?;

    // 2. Projects from groups the user is a member of
    if let Ok(groups) = gitlab_get_json_array(
        &client,
        &format!("{}/api/v4/groups?per_page=100&min_access_level=10", base),
        token,
    )
    .await
    {
        for g in &groups {
            let group_id = match g["id"].as_u64() {
                Some(id) => id,
                None => continue,
            };
            let group_name = g["full_path"].as_str().unwrap_or("?");
            tracing::info!(
                "discover_repos: fetching GitLab group '{}' projects",
                group_name
            );
            if let Err(e) = gitlab_collect_projects(&client, token, &format!(
                "{}/api/v4/groups/{}/projects?per_page=100&order_by=updated_at&include_subgroups=true", base, group_id
            ), &mut all_repos, &mut seen).await {
                tracing::warn!("discover_repos: failed to list projects for GitLab group '{}': {}", group_name, e);
            }
        }
    }

    Ok(all_repos)
}

/// Paginate a GitLab projects endpoint and collect results.
async fn gitlab_collect_projects(
    client: &reqwest::Client,
    token: &str,
    base_url: &str,
    out: &mut Vec<RemoteRepo>,
    seen: &mut std::collections::HashSet<String>,
) -> Result<(), String> {
    let mut page = 1u32;
    loop {
        let url = format!("{}&page={}", base_url, page);
        let repos = gitlab_get_json_array(client, &url, token).await?;
        if repos.is_empty() {
            break;
        }
        let done = repos.len() < 100;
        for r in &repos {
            let full_name = r["path_with_namespace"].as_str().unwrap_or("").to_string();
            if seen.insert(full_name.clone()) {
                out.push(parse_gitlab_repo(r));
            }
        }
        if done {
            break;
        }
        page += 1;
    }
    Ok(())
}

/// Helper: GET a JSON array from GitLab API with auth headers.
async fn gitlab_get_json_array(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let resp = client
        .get(url)
        .header("PRIVATE-TOKEN", token)
        .header("User-Agent", "Kronn/0.1")
        .send()
        .await
        .map_err(|e| format!("GitLab request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitLab API error {}: {}", status, body));
    }

    resp.json()
        .await
        .map_err(|e| format!("Failed to parse GitLab response: {}", e))
}

/// Parse a GitLab project JSON object into a RemoteRepo.
fn parse_gitlab_repo(r: &serde_json::Value) -> RemoteRepo {
    RemoteRepo {
        name: r["name"].as_str().unwrap_or("").to_string(),
        full_name: r["path_with_namespace"].as_str().unwrap_or("").to_string(),
        clone_url: r["http_url_to_repo"].as_str().unwrap_or("").to_string(),
        ssh_url: r["ssh_url_to_repo"].as_str().unwrap_or("").to_string(),
        description: r["description"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        language: None, // GitLab doesn't include language in list endpoint
        stargazers_count: r["star_count"].as_u64().unwrap_or(0) as u32,
        updated_at: r["last_activity_at"].as_str().unwrap_or("").to_string(),
        source: "gitlab".into(),
        already_cloned: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_repo_url_strips_https_github_prefix() {
        assert_eq!(
            normalize_repo_url("https://github.com/Org/Repo"),
            "github:org/repo"
        );
    }

    #[test]
    fn normalize_repo_url_strips_trailing_slash_and_dot_git() {
        assert_eq!(
            normalize_repo_url("https://github.com/Org/Repo.git/"),
            "github:org/repo"
        );
    }

    #[test]
    fn normalize_repo_url_strips_ssh_github_prefix() {
        assert_eq!(
            normalize_repo_url("git@github.com:Org/Repo.git"),
            "github:org/repo"
        );
    }

    #[test]
    fn normalize_repo_url_handles_gitlab_https() {
        assert_eq!(
            normalize_repo_url("https://gitlab.com/Group/Project"),
            "gitlab:group/project"
        );
    }

    #[test]
    fn normalize_repo_url_handles_gitlab_ssh() {
        assert_eq!(
            normalize_repo_url("git@gitlab.com:Group/Project.git"),
            "gitlab:group/project"
        );
    }

    #[test]
    fn normalize_repo_url_passes_through_unknown_hosts() {
        // Self-hosted bitbucket etc. — host stays as-is.
        let out = normalize_repo_url("https://bitbucket.example.com/Foo/Bar.git");
        assert!(out.contains("bitbucket.example.com"));
        assert!(!out.ends_with(".git"));
    }

    #[test]
    fn parse_github_repo_extracts_all_fields() {
        let v = serde_json::json!({
            "name": "kronn",
            "full_name": "docroms/kronn",
            "clone_url": "https://github.com/docroms/kronn.git",
            "ssh_url": "git@github.com:docroms/kronn.git",
            "description": "An agent orchestration tool",
            "language": "Rust",
            "stargazers_count": 42,
            "updated_at": "2026-05-28T10:00:00Z",
        });
        let parsed = parse_github_repo(&v);
        assert_eq!(parsed.name, "kronn");
        assert_eq!(parsed.full_name, "docroms/kronn");
        assert_eq!(parsed.clone_url, "https://github.com/docroms/kronn.git");
        assert_eq!(parsed.ssh_url, "git@github.com:docroms/kronn.git");
        assert_eq!(
            parsed.description.as_deref(),
            Some("An agent orchestration tool")
        );
        assert_eq!(parsed.language.as_deref(), Some("Rust"));
        assert_eq!(parsed.stargazers_count, 42);
        assert_eq!(parsed.updated_at, "2026-05-28T10:00:00Z");
        assert_eq!(parsed.source, "github");
        assert!(!parsed.already_cloned);
    }

    #[test]
    fn parse_github_repo_tolerates_missing_optional_fields() {
        // null description and missing language must not panic.
        let v = serde_json::json!({
            "name": "x",
            "full_name": "u/x",
            "clone_url": "",
            "ssh_url": "",
            "description": null,
            "language": null,
            "stargazers_count": 0,
            "updated_at": "",
        });
        let parsed = parse_github_repo(&v);
        assert_eq!(parsed.name, "x");
        assert!(parsed.description.is_none());
        assert!(parsed.language.is_none());
        assert_eq!(parsed.stargazers_count, 0);
    }

    #[test]
    fn parse_github_repo_handles_empty_object() {
        let v = serde_json::json!({});
        let parsed = parse_github_repo(&v);
        assert_eq!(parsed.name, "");
        assert_eq!(parsed.stargazers_count, 0);
        assert!(parsed.description.is_none());
    }

    #[test]
    fn parse_gitlab_repo_uses_path_with_namespace() {
        let v = serde_json::json!({
            "name": "infra",
            "path_with_namespace": "group/sub/infra",
            "http_url_to_repo": "https://gitlab.com/group/sub/infra.git",
            "ssh_url_to_repo": "git@gitlab.com:group/sub/infra.git",
            "description": "Terraform modules",
            "star_count": 7,
            "last_activity_at": "2026-05-27T12:00:00Z",
        });
        let parsed = parse_gitlab_repo(&v);
        assert_eq!(parsed.name, "infra");
        assert_eq!(parsed.full_name, "group/sub/infra");
        assert_eq!(parsed.clone_url, "https://gitlab.com/group/sub/infra.git");
        assert_eq!(parsed.ssh_url, "git@gitlab.com:group/sub/infra.git");
        assert_eq!(parsed.description.as_deref(), Some("Terraform modules"));
        // GitLab list endpoint never includes language.
        assert!(parsed.language.is_none());
        assert_eq!(parsed.stargazers_count, 7);
        assert_eq!(parsed.source, "gitlab");
    }

    #[test]
    fn parse_gitlab_repo_empty_description_filtered_to_none() {
        // GitLab returns "" for missing description ; we want None, not Some("").
        let v = serde_json::json!({
            "name": "y",
            "path_with_namespace": "g/y",
            "http_url_to_repo": "",
            "ssh_url_to_repo": "",
            "description": "",
            "star_count": 0,
            "last_activity_at": "",
        });
        let parsed = parse_gitlab_repo(&v);
        assert!(
            parsed.description.is_none(),
            "empty string must be filtered to None"
        );
    }

    #[test]
    fn parse_gitlab_repo_tolerates_missing_fields() {
        let v = serde_json::json!({});
        let parsed = parse_gitlab_repo(&v);
        assert_eq!(parsed.name, "");
        assert_eq!(parsed.full_name, "");
        assert_eq!(parsed.source, "gitlab");
        assert!(parsed.language.is_none());
    }
}
