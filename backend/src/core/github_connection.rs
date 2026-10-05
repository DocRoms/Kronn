//! GitHub access per project (design note `agent-secret-boundary.md` §4.5, D2).
//!
//! Agents receive `GH_TOKEN`, `GITHUB_TOKEN` and `COPILOT_GITHUB_TOKEN` only
//! when their project is connected. Kronn's own GitHub calls (PR creation, git
//! status, tracker triggers) keep the backend's access and do not go through here.
//!
//! The launch path has no database handle, so the per-project grants live in a
//! process-wide map loaded at boot and updated by every API write. Token values
//! never reach logs, `Debug` output or API responses.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use zeroize::Zeroizing;

use crate::models::{GithubConnectionMode, GithubMachineTokenSource, GithubScope, GithubTokenKind};

/// The variables an agent reads a GitHub token from.
pub const GITHUB_ENV_NAMES: [&str; 3] = ["GH_TOKEN", "GITHUB_TOKEN", "COPILOT_GITHUB_TOKEN"];

/// `gh auth token` reads a file or the OS keychain; past this it is stuck.
const GH_TOKEN_TIMEOUT: Duration = Duration::from_secs(5);
/// Short, so a `gh auth logout` or `gh auth switch` reaches new launches quickly.
const MACHINE_TOKEN_TTL: Duration = Duration::from_secs(60);
const DEFAULT_API_BASE: &str = "https://api.github.com";
const MAX_LISTED_REPOSITORIES: usize = 100;
/// Longest token accepted from the UI; GitHub tokens are well below it.
pub const MAX_TOKEN_CHARS: usize = 512;

// ─── Remote detection ───────────────────────────────────────────────────────

/// `owner/name` when `url` points at a repository on github.com (HTTPS, SSH or
/// scp-like form). GitHub Enterprise hosts are not github.com and return `None`.
pub fn github_repo_slug(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "https" | "http" | "ssh" | "git" | "git+ssh"
        ) {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        let host = host.split(':').next()?;
        if !is_github_host(host) {
            return None;
        }
        path
    } else {
        // scp-like: [user@]github.com:owner/name.git
        let (authority, path) = url.split_once(':')?;
        let host = authority.rsplit('@').next()?;
        if !is_github_host(host) {
            return None;
        }
        path
    };
    let path = rest.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let name = parts.next().filter(|s| !s.is_empty())?;
    if parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

fn is_github_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "github.com" || host == "www.github.com" || host == "ssh.github.com"
}

/// Remote URLs declared in a repository's git config, worktrees included.
pub fn git_remote_urls(repo: &Path) -> Vec<String> {
    let Some(config) = git_config_path(repo) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(config) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            (key.trim().eq_ignore_ascii_case("url")).then(|| value.trim().to_string())
        })
        .collect()
}

fn git_config_path(repo: &Path) -> Option<PathBuf> {
    let dot_git = repo.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git.join("config"));
    }
    // A worktree or submodule: `.git` is a file pointing at the real git dir.
    let pointer = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = pointer.trim().strip_prefix("gitdir:")?.trim();
    let gitdir = if Path::new(gitdir).is_absolute() {
        PathBuf::from(gitdir)
    } else {
        repo.join(gitdir)
    };
    let common = std::fs::read_to_string(gitdir.join("commondir"))
        .ok()
        .map(|common| gitdir.join(common.trim()))
        .unwrap_or(gitdir);
    Some(common.join("config"))
}

/// The project's remote is on github.com, read from `repo_url` or else the git config.
pub fn project_is_on_github(path: &str, repo_url: Option<&str>) -> bool {
    if repo_url
        .filter(|url| !url.trim().is_empty())
        .is_some_and(|url| github_repo_slug(url).is_some())
    {
        return true;
    }
    let resolved = crate::core::scanner::resolve_host_path(path);
    git_remote_urls(&resolved)
        .iter()
        .any(|url| github_repo_slug(url).is_some())
}

// ─── Tokens ─────────────────────────────────────────────────────────────────

pub fn token_kind(token: &str) -> GithubTokenKind {
    if token.starts_with("github_pat_") {
        GithubTokenKind::FineGrained
    } else if token.starts_with("ghp_") {
        GithubTokenKind::Classic
    } else if token.starts_with("gho_") {
        GithubTokenKind::Oauth
    } else if token.starts_with("ghu_") || token.starts_with("ghs_") {
        GithubTokenKind::App
    } else {
        GithubTokenKind::Unknown
    }
}

/// Validates a token pasted in the UI. The error never quotes the value.
pub fn validate_pasted_token(token: &str) -> Result<String, String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("The GitHub token is empty".into());
    }
    if token.chars().count() > MAX_TOKEN_CHARS {
        return Err(format!(
            "The GitHub token is longer than {MAX_TOKEN_CHARS} characters"
        ));
    }
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("The GitHub token contains characters a GitHub token never has".into());
    }
    Ok(token.to_string())
}

/// Key of the one-entry map a stored token is kept in. Same format as MCP
/// env values, so the boot key reconcile can self-test against it.
const STORED_TOKEN_ENV_KEY: &str = "GH_TOKEN";

pub fn encrypt_stored_token(token: &str, secret: &str) -> Result<String, String> {
    let env =
        std::collections::HashMap::from([(STORED_TOKEN_ENV_KEY.to_string(), token.to_string())]);
    crate::db::mcps::encrypt_env(&env, secret)
}

pub fn decrypt_stored_token(cipher: &str, secret: &str) -> Result<Zeroizing<String>, String> {
    let mut env = crate::db::mcps::decrypt_env(cipher, secret)?;
    env.remove(STORED_TOKEN_ENV_KEY)
        .filter(|token| !token.is_empty())
        .map(Zeroizing::new)
        .ok_or_else(|| "The stored GitHub token is missing".to_string())
}

struct CachedMachineToken {
    at: Instant,
    value: Option<(Zeroizing<String>, GithubMachineTokenSource)>,
}

static MACHINE_TOKEN: LazyLock<tokio::sync::Mutex<Option<CachedMachineToken>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

/// `Some(value)` replaces the machine token lookup (tests only); `None` restores it.
static MACHINE_TOKEN_OVERRIDE: RwLock<Option<Option<String>>> = RwLock::new(None);

#[doc(hidden)]
pub fn override_machine_token_for_tests(value: Option<Option<&str>>) {
    *MACHINE_TOKEN_OVERRIDE
        .write()
        .unwrap_or_else(|e| e.into_inner()) = value.map(|token| token.map(str::to_string));
}

fn env_machine_token() -> Option<Zeroizing<String>> {
    ["GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
        .map(Zeroizing::new)
}

async fn gh_cli_token() -> Option<Zeroizing<String>> {
    let mut cmd = crate::core::cmd::async_cmd("gh");
    cmd.args(["auth", "token"])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(GH_TOKEN_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(_)) | Ok(Err(_)) => return None,
        Err(_) => {
            tracing::warn!(
                target: "kronn::github",
                "`gh auth token` did not answer within {}s; no machine GitHub token",
                GH_TOKEN_TIMEOUT.as_secs()
            );
            return None;
        }
    };
    let token = Zeroizing::new(String::from_utf8_lossy(&output.stdout).trim().to_string());
    (!token.is_empty()).then_some(token)
}

/// The machine's GitHub token: the backend's `GH_TOKEN`/`GITHUB_TOKEN`, else
/// `gh auth token` (asynchronous, bounded, cached for a minute).
pub async fn machine_token() -> Option<(Zeroizing<String>, GithubMachineTokenSource)> {
    if let Some(value) = MACHINE_TOKEN_OVERRIDE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return value.map(|token| (Zeroizing::new(token), GithubMachineTokenSource::GhCli));
    }
    if let Some(token) = env_machine_token() {
        return Some((token, GithubMachineTokenSource::Environment));
    }
    let mut cached = MACHINE_TOKEN.lock().await;
    if let Some(entry) = cached.as_ref() {
        if entry.at.elapsed() < MACHINE_TOKEN_TTL {
            return entry.value.clone();
        }
    }
    let value = gh_cli_token()
        .await
        .map(|token| (token, GithubMachineTokenSource::GhCli));
    *cached = Some(CachedMachineToken {
        at: Instant::now(),
        value: value.clone(),
    });
    value
}

/// Forget the cached `gh auth token`, e.g. before an explicit scope refresh.
pub async fn forget_machine_token() {
    *MACHINE_TOKEN.lock().await = None;
}

// ─── Grants used at launch ──────────────────────────────────────────────────

#[derive(Clone)]
enum Grant {
    GhLogin,
    StoredToken(Zeroizing<String>),
}

static GRANTS: LazyLock<RwLock<HashMap<String, Grant>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Records what new launches of `project_id` receive. `token` is the plaintext
/// of a stored token and is read only with `StoredToken`.
pub fn set_grant(project_id: &str, mode: GithubConnectionMode, token: Option<&str>) {
    let mut grants = GRANTS.write().unwrap_or_else(|e| e.into_inner());
    match (mode, token) {
        (GithubConnectionMode::GhLogin, _) => {
            grants.insert(project_id.to_string(), Grant::GhLogin);
        }
        (GithubConnectionMode::StoredToken, Some(token)) if !token.is_empty() => {
            grants.insert(
                project_id.to_string(),
                Grant::StoredToken(Zeroizing::new(token.to_string())),
            );
        }
        _ => {
            grants.remove(project_id);
        }
    }
}

/// Loads every project's grant at boot. A stored token that cannot be
/// decrypted leaves its project without a token rather than failing boot.
pub async fn load_grants(db: &crate::db::Database, secret: Option<&str>) -> anyhow::Result<usize> {
    let rows = db.with_conn(crate::db::github_connections::list).await?;
    let mut loaded = 0;
    for row in rows {
        let token = match (&row.mode, &row.token_encrypted) {
            (GithubConnectionMode::StoredToken, Some(cipher)) => {
                match secret.map(|secret| decrypt_stored_token(cipher, secret)) {
                    Some(Ok(plain)) => Some(plain),
                    _ => {
                        tracing::warn!(
                            target: "kronn::github",
                            project_id = %row.project_id,
                            "stored GitHub token could not be decrypted; the project's agents get no token"
                        );
                        None
                    }
                }
            }
            _ => None,
        };
        set_grant(
            &row.project_id,
            row.mode,
            token.as_deref().map(String::as_str),
        );
        if row.mode != GithubConnectionMode::NotConnected {
            loaded += 1;
        }
    }
    Ok(loaded)
}

/// The GitHub variables a launch of `project_id` receives: empty unless the
/// project is connected. Callers remove `GITHUB_ENV_NAMES` from the child's
/// inherited environment first, then apply this.
pub async fn env_for_launch(project_id: Option<&str>) -> Vec<(String, String)> {
    let Some(project_id) = project_id else {
        return Vec::new();
    };
    let grant = GRANTS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(project_id)
        .cloned();
    let token = match grant {
        None => return Vec::new(),
        Some(Grant::StoredToken(token)) => token,
        Some(Grant::GhLogin) => match machine_token().await {
            Some((token, _)) => token,
            None => return Vec::new(),
        },
    };
    GITHUB_ENV_NAMES
        .iter()
        .map(|name| (name.to_string(), token.to_string()))
        .collect()
}

/// Applies `env_for_launch`'s result to a child: GitHub variables inherited
/// from the backend are removed, values the launch set on purpose (a project
/// MCP's own `GITHUB_TOKEN`) are kept, and a connected project's token wins.
pub fn apply_launch_env(cmd: &mut std::process::Command, github_env: &[(String, String)]) {
    let explicit: Vec<String> = cmd
        .get_envs()
        .filter(|(_, value)| value.is_some())
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    for name in GITHUB_ENV_NAMES {
        if !explicit.iter().any(|set| set == name) {
            cmd.env_remove(name);
        }
    }
    cmd.envs(github_env.iter().map(|(name, value)| (name, value)));
}

// ─── Scope read from GitHub ─────────────────────────────────────────────────

/// `KRONN_GITHUB_API_BASE` lets tests point at a stub; GitHub otherwise.
pub fn api_base() -> String {
    std::env::var("KRONN_GITHUB_API_BASE")
        .ok()
        .map(|base| base.trim().trim_end_matches('/').to_string())
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

fn unverified(kind: GithubTokenKind, login: Option<String>, reason: String) -> GithubScope {
    GithubScope {
        verified: false,
        token_kind: kind,
        login,
        scopes: Vec::new(),
        repositories: Vec::new(),
        repositories_truncated: false,
        broad: false,
        reason: Some(reason),
        checked_at: Utc::now(),
    }
}

/// Scopes that reach every repository the account can reach.
fn scopes_are_broad(scopes: &[String]) -> bool {
    scopes.iter().any(|scope| {
        matches!(
            scope.as_str(),
            "repo" | "public_repo" | "admin:org" | "write:org" | "delete_repo" | "workflow"
        )
    })
}

fn request(client: &reqwest::Client, url: &str, token: &str) -> reqwest::RequestBuilder {
    client
        .get(url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "Kronn")
}

/// Asks GitHub what `token` can do: `X-OAuth-Scopes` for classic and OAuth
/// tokens, the reachable repositories for fine-grained ones.
pub async fn verify_scope(api_base: &str, token: &str) -> GithubScope {
    let kind = token_kind(token);
    let client = http_client();
    let user = match request(&client, &format!("{api_base}/user"), token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            // reqwest errors name the URL, never the request headers.
            return unverified(kind, None, format!("GitHub could not be reached: {error}"));
        }
    };
    let status = user.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return unverified(kind, None, "GitHub refused the token (401)".into());
    }
    let oauth_scopes = user
        .headers()
        .get("x-oauth-scopes")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .map(|scope| scope.trim().to_string())
                .filter(|scope| !scope.is_empty())
                .collect::<Vec<_>>()
        });
    let login = if status.is_success() {
        user.json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|body| body["login"].as_str().map(str::to_string))
    } else {
        None
    };

    if kind != GithubTokenKind::FineGrained {
        if let Some(scopes) = oauth_scopes.filter(|_| status.is_success()) {
            return GithubScope {
                verified: true,
                token_kind: kind,
                login,
                broad: scopes_are_broad(&scopes),
                scopes,
                repositories: Vec::new(),
                repositories_truncated: false,
                reason: None,
                checked_at: Utc::now(),
            };
        }
    }

    let repos_url = format!("{api_base}/user/repos?per_page={MAX_LISTED_REPOSITORIES}");
    let repos = match request(&client, &repos_url, token).send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            let reason = format!(
                "GitHub did not report this token's scope (HTTP {} on /user, {} on /user/repos)",
                status.as_u16(),
                response.status().as_u16()
            );
            return unverified(kind, login, reason);
        }
        Err(error) => {
            let reason = format!("GitHub could not be reached: {error}");
            return unverified(kind, login, reason);
        }
    };
    let has_next_page = repos
        .headers()
        .get("link")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|link| link.contains("rel=\"next\""));
    let Ok(body) = repos.json::<Vec<serde_json::Value>>().await else {
        return unverified(
            kind,
            login,
            "GitHub returned an unreadable repository list".into(),
        );
    };
    let repositories: Vec<String> = body
        .iter()
        .filter_map(|repo| repo["full_name"].as_str().map(str::to_string))
        .collect();
    GithubScope {
        verified: true,
        token_kind: kind,
        login,
        scopes: Vec::new(),
        repositories_truncated: has_next_page || repositories.len() >= MAX_LISTED_REPOSITORIES,
        repositories,
        broad: false,
        reason: None,
        checked_at: Utc::now(),
    }
}

#[cfg(test)]
#[path = "github_connection_test.rs"]
mod tests;
