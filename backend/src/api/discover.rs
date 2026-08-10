//! Remote repository discovery — GitHub/GitLab integration.

use crate::models::*;
use crate::AppState;
use axum::{extract::State, Json};

#[derive(Clone, Debug)]
pub(crate) enum RepoSourceAuth {
    Token {
        token: String,
        api_url: Option<String>,
    },
    GitLabCli {
        host: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct AuthenticatedRepoSource {
    pub source: RepoSource,
    pub auth: RepoSourceAuth,
}

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
    let available_sources: Vec<RepoSource> = all_sources
        .iter()
        .map(|entry| entry.source.clone())
        .collect();

    if all_sources.is_empty() {
        return Json(ApiResponse::err(
            "No GitHub or GitLab connection found. Configure a provider plugin, authenticate its local CLI, or provide an API token."
        ));
    }

    // Filter sources if specific IDs requested
    let sources_to_use: Vec<&AuthenticatedRepoSource> = if req.source_ids.is_empty() {
        all_sources.iter().collect()
    } else {
        all_sources
            .iter()
            .filter(|entry| req.source_ids.contains(&entry.source.id))
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
            .map(|entry| format!("{}({})", entry.source.label, entry.source.id))
            .collect::<Vec<_>>(),
    );

    // Deduplicate repos by full_name (in case multiple tokens see the same repo)
    let mut seen_full_names = std::collections::HashSet::new();

    for entry in &sources_to_use {
        let source = &entry.source;
        match source.provider.as_str() {
            "github" => {
                let RepoSourceAuth::Token { token, .. } = &entry.auth else {
                    continue;
                };
                let token_preview = if token.len() > 8 { &token[..8] } else { token };
                tracing::info!(
                    "discover_repos: querying GitHub source '{}' with token {}...",
                    source.label,
                    token_preview
                );
                match fetch_github_repos(token).await {
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
                let result = match &entry.auth {
                    RepoSourceAuth::Token { token, api_url } => {
                        let api_url = api_url.as_deref().unwrap_or("https://gitlab.com");
                        match fetch_gitlab_repos(token, api_url).await {
                            Ok(repos) => Ok(repos),
                            Err(api_error) => match fetch_gitlab_repos_via_cli(api_url).await {
                                Ok(repos) => Ok(repos),
                                Err(cli_error) => Err(format!(
                                    "{}; local glab fallback also failed: {}",
                                    api_error, cli_error
                                )),
                            },
                        }
                    }
                    RepoSourceAuth::GitLabCli { host } => {
                        fetch_gitlab_repos_via_cli(host.as_deref().unwrap_or("https://gitlab.com"))
                            .await
                    }
                };
                match result {
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

/// Find all repository authentication sources from plugin configs and env vars.
/// GitLab can use either a saved PAT or the local `glab auth login` session.
pub(crate) async fn find_all_provider_sources(state: &AppState) -> Vec<AuthenticatedRepoSource> {
    let mut sources: Vec<AuthenticatedRepoSource> = vec![];

    // Read encryption secret
    let config = state.config.read().await;
    let secret = config.encryption_secret.clone();
    drop(config);

    // Scan plugin configs. A tokenless GitLab config remains a valid source:
    // discovery can use the local `glab auth login` session.
    let configs = state
        .db
        .with_conn(crate::db::mcps::list_configs)
        .await
        .unwrap_or_default();

    for cfg in configs {
        let env = match (&secret, cfg.env_encrypted.is_empty()) {
            (_, true) | (None, false) => std::collections::HashMap::new(),
            (Some(secret), false) => {
                match crate::db::mcps::decrypt_env(&cfg.env_encrypted, secret) {
                    Ok(env) => env,
                    Err(_) => continue,
                }
            }
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
                sources.push(AuthenticatedRepoSource {
                    source: RepoSource {
                        id: cfg.id.clone(),
                        label: cfg.label.clone(),
                        provider: "github".into(),
                    },
                    auth: RepoSourceAuth::Token {
                        token: token.clone(),
                        api_url: None,
                    },
                });
            }
        }

        // GitLab MCP — current names first, then backwards-compatible aliases.
        if cfg.server_id == "mcp-gitlab" {
            let (configured_token, configured_host) = gitlab_credentials(&env);
            let token = configured_token.or_else(gitlab_token_from_process_env);
            let host = configured_host.or_else(gitlab_host_from_process_env);
            let auth = match token {
                Some(token) => RepoSourceAuth::Token {
                    token,
                    api_url: host,
                },
                None => RepoSourceAuth::GitLabCli { host },
            };
            sources.push(AuthenticatedRepoSource {
                source: RepoSource {
                    id: cfg.id.clone(),
                    label: cfg.label.clone(),
                    provider: "gitlab".into(),
                },
                auth,
            });
        }
    }

    // Environment variable fallbacks
    if let Ok(token) = std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
        // Only add env source if there's no MCP config for GitHub already
        let has_gh = sources
            .iter()
            .any(|entry| entry.source.provider == "github");
        if !has_gh {
            sources.push(AuthenticatedRepoSource {
                source: RepoSource {
                    id: "env:github".into(),
                    label: "GitHub (env)".into(),
                    provider: "github".into(),
                },
                auth: RepoSourceAuth::Token {
                    token,
                    api_url: None,
                },
            });
        }
    }

    if let Some(token) = gitlab_token_from_process_env() {
        let has_gl = sources
            .iter()
            .any(|entry| entry.source.provider == "gitlab");
        if !has_gl {
            let api_url = gitlab_host_from_process_env();
            sources.push(AuthenticatedRepoSource {
                source: RepoSource {
                    id: "env:gitlab".into(),
                    label: "GitLab (env)".into(),
                    provider: "gitlab".into(),
                },
                auth: RepoSourceAuth::Token { token, api_url },
            });
        }
    }

    sources
}

fn gitlab_credentials(
    env: &std::collections::HashMap<String, String>,
) -> (Option<String>, Option<String>) {
    let first_non_blank = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| {
                env.get(*key)
                    .map(String::as_str)
                    .filter(|value| !value.trim().is_empty())
            })
            .map(str::to_owned)
    };
    (
        first_non_blank(&["GITLAB_TOKEN", "GITLAB_PERSONAL_ACCESS_TOKEN"]),
        first_non_blank(&["GITLAB_HOST", "GL_HOST", "GITLAB_API_URL"]),
    )
}

fn gitlab_token_from_process_env() -> Option<String> {
    ["GITLAB_TOKEN", "GITLAB_PERSONAL_ACCESS_TOKEN"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .filter(|value| !value.trim().is_empty())
}

fn gitlab_host_from_process_env() -> Option<String> {
    ["GITLAB_HOST", "GL_HOST", "GITLAB_API_URL"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .filter(|value| !value.trim().is_empty())
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
    let base = normalize_gitlab_base_url(api_url)?;
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

fn normalize_gitlab_base_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    let candidate = if value.contains("://") {
        value.to_string()
    } else {
        format!("https://{}", value)
    };
    let mut url = reqwest::Url::parse(&candidate)
        .map_err(|error| format!("Invalid GitLab host '{}': {}", value, error))?;
    let path = url.path().trim_end_matches('/');
    let root_path = path.strip_suffix("/api/v4").unwrap_or(path).to_string();
    url.set_path(&root_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn gitlab_cli_hostname(value: &str) -> Result<String, String> {
    let base = normalize_gitlab_base_url(value)?;
    let url = reqwest::Url::parse(&base)
        .map_err(|error| format!("Invalid GitLab host '{}': {}", value, error))?;
    let host = url
        .host_str()
        .ok_or_else(|| format!("Invalid GitLab host '{}'", value))?;
    Ok(match url.port() {
        Some(port) => format!("{}:{}", host, port),
        None => host.to_string(),
    })
}

/// Use the user's local `glab auth login` session without extracting or
/// persisting its token. This is also the recovery path when a saved PAT is
/// stale. Environment token overrides are deliberately removed because glab
/// gives them precedence over its stored credentials.
async fn fetch_gitlab_repos_via_cli(host: &str) -> Result<Vec<RemoteRepo>, String> {
    let hostname = gitlab_cli_hostname(host)?;
    let mut command = crate::core::cmd::async_cmd("glab");
    command
        .args([
            "api",
            "projects?membership=true&per_page=100&order_by=last_activity_at&sort=desc",
            "--paginate",
            "--output",
            "json",
            "--hostname",
            &hostname,
        ])
        .env_remove("GITLAB_TOKEN")
        .env_remove("GITLAB_ACCESS_TOKEN")
        .env_remove("OAUTH_TOKEN")
        .env_remove("GITLAB_HOST")
        .env_remove("GL_HOST")
        .kill_on_drop(true);

    let output = tokio::time::timeout(std::time::Duration::from_secs(20), command.output())
        .await
        .map_err(|_| "glab API request timed out after 20 seconds".to_string())?
        .map_err(|error| format!("Unable to run glab: {}", error))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail: String = detail.chars().take(500).collect();
        return Err(if detail.is_empty() {
            format!("glab exited with {}", output.status)
        } else {
            detail
        });
    }

    let projects: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Unable to parse glab response: {}", error))?;
    Ok(projects.iter().map(parse_gitlab_repo).collect())
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
    fn gitlab_credentials_prefers_current_cli_variable_names() {
        let env = std::collections::HashMap::from([
            ("GITLAB_TOKEN".to_string(), "current-token".to_string()),
            (
                "GITLAB_PERSONAL_ACCESS_TOKEN".to_string(),
                "legacy-token".to_string(),
            ),
            (
                "GITLAB_HOST".to_string(),
                "https://gitlab.example.com".to_string(),
            ),
            (
                "GITLAB_API_URL".to_string(),
                "https://legacy.example.com/api/v4".to_string(),
            ),
        ]);

        assert_eq!(
            gitlab_credentials(&env),
            (
                Some("current-token".to_string()),
                Some("https://gitlab.example.com".to_string()),
            )
        );
    }

    #[test]
    fn gitlab_credentials_keeps_legacy_plugin_configs_working() {
        let env = std::collections::HashMap::from([
            (
                "GITLAB_PERSONAL_ACCESS_TOKEN".to_string(),
                "legacy-token".to_string(),
            ),
            (
                "GITLAB_API_URL".to_string(),
                "https://gitlab.example.com/api/v4".to_string(),
            ),
        ]);

        assert_eq!(
            gitlab_credentials(&env),
            (
                Some("legacy-token".to_string()),
                Some("https://gitlab.example.com/api/v4".to_string()),
            )
        );
    }

    #[test]
    fn normalize_gitlab_base_url_accepts_host_and_existing_api_suffix() {
        assert_eq!(
            normalize_gitlab_base_url("gitlab.example.com").unwrap(),
            "https://gitlab.example.com"
        );
        assert_eq!(
            normalize_gitlab_base_url("https://gitlab.example.com/api/v4/").unwrap(),
            "https://gitlab.example.com"
        );
        assert_eq!(
            normalize_gitlab_base_url("https://example.com/gitlab/api/v4").unwrap(),
            "https://example.com/gitlab"
        );
    }

    #[test]
    fn gitlab_cli_hostname_strips_scheme_path_and_keeps_port() {
        assert_eq!(
            gitlab_cli_hostname("https://gitlab.example.com:8443/api/v4").unwrap(),
            "gitlab.example.com:8443"
        );
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
