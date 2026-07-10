// `POST /api/projects/clone` plus the URL-mangling helpers it relies on:
// HTTPS → SSH conversion when SSH keys are present, and PAT injection
// from the configured MCP sources when they aren't. The cloned `.git`
// has its `origin` reset to the original (token-free) URL afterwards so
// secrets never persist on disk.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::core::cmd::sync_cmd;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

use super::{determine_parent_dir, enrich_audit_status, find_common_parent, resync_project_assets};

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

/// Mask `scheme://userinfo@host` credentials in a string so a PAT embedded in a
/// clone URL never leaks into an error message or a log line. Conservative:
/// collapses the characters between `://` and the next `@` (when there's no `/`
/// in between, i.e. it's really userinfo) to `***`.
fn redact_url_credentials(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find("://") {
        out.push_str(&rest[..pos + 3]);
        let after = &rest[pos + 3..];
        // The userinfo terminator `@` must come before any `/` (path start).
        let at = after.find('@');
        let slash = after.find('/');
        match (at, slash) {
            (Some(a), maybe_slash) if maybe_slash.map(|sl| a < sl).unwrap_or(true) => {
                out.push_str("***");
                rest = &after[a..]; // keep `@host...`
            }
            _ => {
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Build an ordered list of clone URLs to try, most-likely-to-succeed first.
///
/// On a native install the user's own git is already authenticated for these
/// repos (macOS keychain / `gh` credential helper / SSH config), so the BARE
/// URL frequently works where an injected PAT does NOT — a personal-account
/// token returns 403 ("Write access to repository not granted") on a work org
/// it can't see. We therefore try, in order:
///   1. the original URL — let git's own credential helper / SSH config resolve it,
///   2. each configured provider token injected into the URL (Docker / headless),
///   3. the SSH form when SSH keys are present.
///
/// De-duplicated, order preserved.
async fn candidate_clone_urls(url: &str, state: &AppState) -> Vec<String> {
    // Resolve the two runtime inputs (configured provider tokens + whether SSH
    // keys are mounted), then delegate the ordering/injection/dedup to the pure
    // `build_clone_candidates` so that logic is unit-testable without an
    // AppState or network.
    let token_sources: Vec<(String, String)> = if url.starts_with("https://") {
        crate::api::discover::find_all_provider_sources(state)
            .await
            .into_iter()
            .map(|(source, token)| (source.provider, token))
            .collect()
    } else {
        Vec::new()
    };
    let has_ssh_keys = std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".ssh"))
        .map(|d| d.join("id_rsa").exists() || d.join("id_ed25519").exists())
        .unwrap_or(false);
    build_clone_candidates(url, &token_sources, has_ssh_keys)
}

/// Pure core of [`candidate_clone_urls`]: build the ordered, de-duplicated
/// candidate list from already-resolved inputs (`(provider, token)` pairs +
/// whether SSH keys are present). Order: bare URL → each token-injected HTTPS
/// URL → SSH form (github/gitlab + keys). Non-HTTPS URLs yield only themselves.
fn build_clone_candidates(
    url: &str,
    token_sources: &[(String, String)],
    has_ssh_keys: bool,
) -> Vec<String> {
    let mut candidates: Vec<String> = vec![url.to_string()];

    if url.starts_with("https://") {
        for (provider, token) in token_sources {
            if let Some(authed) = inject_token_into_url(url, provider, token) {
                candidates.push(authed);
            }
        }
        if (url.contains("github.com") || url.contains("gitlab.com")) && has_ssh_keys {
            if let Some(ssh_url) = https_to_ssh(url) {
                candidates.push(ssh_url);
            }
        }
    }

    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| seen.insert(c.clone()));
    candidates
}

/// Clone `url` into `dest`, trying each auth strategy from
/// [`candidate_clone_urls`] until one succeeds. Returns the winning URL on
/// success (so the caller can decide whether to scrub a token from
/// `.git/config`), or the last/most-informative error otherwise.
///
/// Runs git non-interactively (`GIT_TERMINAL_PROMPT=0`): a strategy that would
/// otherwise block on a username/password prompt fails fast so we can move to
/// the next candidate instead of hanging. Any partial clone directory left by
/// a failed attempt is removed before the next try.
async fn clone_with_fallbacks(
    url: &str,
    dest: &std::path::Path,
    state: &AppState,
) -> Result<String, String> {
    let candidates = candidate_clone_urls(url, state).await;
    let mut last_err = String::from("no clone strategy available");

    for (i, cand) in candidates.iter().enumerate() {
        let cand_owned = cand.clone();
        let dest_buf = dest.to_path_buf();
        let res = tokio::task::spawn_blocking(move || {
            sync_cmd("git")
                .env("GIT_TERMINAL_PROMPT", "0")
                // Abort a clone stalled under 1 KB/s for 60s instead of
                // pinning the blocking thread on a dead network forever.
                .env("GIT_HTTP_LOW_SPEED_LIMIT", "1024")
                .env("GIT_HTTP_LOW_SPEED_TIME", "60")
                .args(["clone", &cand_owned, &dest_buf.to_string_lossy()])
                .output()
        })
        .await;

        match res {
            Ok(Ok(output)) if output.status.success() => return Ok(cand.clone()),
            Ok(Ok(output)) => {
                last_err = redact_url_credentials(String::from_utf8_lossy(&output.stderr).trim());
                tracing::warn!(
                    "clone attempt {}/{} failed: {}",
                    i + 1,
                    candidates.len(),
                    last_err
                );
            }
            Ok(Err(e)) => last_err = format!("failed to run git: {e}"),
            Err(e) => last_err = format!("clone task failed: {e}"),
        }

        // Remove any partial clone before the next attempt so the dir is free.
        if dest.exists() {
            let _ = std::fs::remove_dir_all(dest);
        }
    }

    Err(last_err)
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

    // Git clone — try the bare URL (native credential helper) first, then any
    // configured PAT, then SSH. See `clone_with_fallbacks`.
    let winning_url = match clone_with_fallbacks(&url, &host_path, &state).await {
        Ok(w) => w,
        Err(e) => return Json(ApiResponse::err(format!("git clone failed: {e}"))),
    };

    // If we cloned via a token-injected HTTPS URL, reset origin to the original
    // (token-free) URL so the secret never persists in .git/config. A bare or
    // SSH clone already has a clean, correct origin — leave it untouched.
    if winning_url != url && winning_url.starts_with("https://") {
        let original_url = url.clone();
        let clone_path2 = host_path.clone();
        let _ = tokio::task::spawn_blocking(move || {
            sync_cmd("git")
                .args(["remote", "set-url", "origin", &original_url])
                .current_dir(&clone_path2)
                .output()
        }).await;
    }

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
            tech_debt_count: 0,
        needs_docs_migration: false,
        path_exists: true,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
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

/// Sanitize an arbitrary repo name / URL tail into a kebab-case directory
/// name (mirrors the rule `clone_project` uses). Empty input → "repo".
fn kebab_dir_name(raw: &str) -> String {
    let cleaned: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if cleaned.is_empty() { "repo".to_string() } else { cleaned }
}

/// Pick a parent directory to clone into that ACTUALLY EXISTS on this host.
///
/// The project being recovered has an unusable path (that's why we're
/// cloning), and after a cross-OS import the common parent of all projects
/// may itself be a foreign path (e.g. WSL `/home/...` on macOS). So we try a
/// list of candidates and return the first one that resolves to a real
/// directory:
///   1. an explicit `requested` parent (from the request body),
///   2. the common parent of projects whose dir currently resolves on disk,
///   3. `KRONN_REPOS_DIR`,
///   4. each configured scan path.
async fn resolve_existing_clone_parent(
    state: &AppState,
    requested: Option<&str>,
) -> Result<String, String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(r) = requested.map(str::trim).filter(|r| !r.is_empty()) {
        candidates.push(r.to_string());
    }
    // Common parent of ON-DISK projects only — ignore imported projects whose
    // foreign paths don't resolve here, so we don't propose a dead directory.
    let existing = state
        .db
        .with_conn(crate::db::projects::list_projects)
        .await
        .unwrap_or_default();
    let on_disk: Vec<Project> = existing
        .into_iter()
        .filter(|p| scanner::resolve_host_path(&p.path).is_dir())
        .collect();
    if let Some(common) = find_common_parent(&on_disk) {
        candidates.push(common);
    }
    if let Ok(repos_dir) = std::env::var("KRONN_REPOS_DIR") {
        candidates.push(repos_dir);
    }
    {
        let config = state.config.read().await;
        for p in &config.scan.paths {
            candidates.push(p.clone());
        }
    }

    for cand in candidates {
        if scanner::contains_parent_dir(&cand) {
            continue;
        }
        if scanner::resolve_host_path(&cand).is_dir() {
            return Ok(cand);
        }
    }
    Err("No existing directory to clone into. Configure a scan path in Settings, or pass an explicit parent directory.".to_string())
}

/// POST /api/projects/:id/clone-and-remap
///
/// Recover a project whose directory no longer resolves on disk (typical
/// after a cross-machine DB import: WSL `/home/...` paths on macOS). Clones
/// the project's `repo_url` afresh into a local directory and re-points the
/// EXISTING project at the clone — no duplicate project row. Reuses the same
/// git-auth injection as `clone_project` (PAT from the linked MCP sources,
/// SSH fallback). After the path is updated, re-syncs the project's MCP
/// plugins + native skill/profile files to the new directory.
pub async fn clone_and_remap(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CloneAndRemapRequest>,
) -> Json<ApiResponse<CloneAndRemapResponse>> {
    // Load the project + its repo_url.
    let pid = id.clone();
    let project = match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {e}"))),
    };
    let url = match project
        .repo_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        Some(u) => u.to_string(),
        None => {
            return Json(ApiResponse::err(
                "This project has no repository URL to clone. Remap it to an existing local folder instead.",
            ))
        }
    };

    // Resolve a parent directory that exists on this host.
    let parent_dir = match resolve_existing_clone_parent(&state, req.parent_dir.as_deref()).await {
        Ok(d) => d,
        Err(e) => return Json(ApiResponse::err(e)),
    };
    if scanner::contains_parent_dir(&parent_dir) {
        return Json(ApiResponse::err("Parent directory may not contain '..' components"));
    }

    // Directory name: prefer the project name, fall back to the URL tail.
    let mut dir_name = kebab_dir_name(&project.name);
    if dir_name == "repo" {
        let url_tail = url
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("repo")
            .trim_end_matches(".git");
        dir_name = kebab_dir_name(url_tail);
    }

    let project_path = format!("{}/{}", parent_dir.trim_end_matches('/'), dir_name);
    // Resolve the *parent* (which exists) for the correct Docker mount path,
    // then append the dir name — same reasoning as `clone_project`.
    let resolved_parent = scanner::resolve_host_path(&parent_dir);
    let host_path = resolved_parent.join(&dir_name);
    if host_path.exists() {
        return Json(ApiResponse::err(format!(
            "Directory already exists: {project_path}. Remap to it directly instead of cloning."
        )));
    }

    // Git clone — bare URL (native credential helper) first, then any
    // configured PAT, then SSH. This is what fixes the 403 the user hit on a
    // work-org repo: their macOS keychain / SSH already has access, but a
    // personal-account PAT does not — so we must try the native auth first.
    let winning_url = match clone_with_fallbacks(&url, &host_path, &state).await {
        Ok(w) => w,
        Err(e) => return Json(ApiResponse::err(format!("git clone failed: {e}"))),
    };
    // Scrub the token from .git/config only when we cloned via a token-injected
    // HTTPS URL; a bare/SSH clone already has a clean origin.
    if winning_url != url && winning_url.starts_with("https://") {
        let original_url = url.clone();
        let clone_path2 = host_path.clone();
        let _ = tokio::task::spawn_blocking(move || {
            sync_cmd("git")
                .args(["remote", "set-url", "origin", &original_url])
                .current_dir(&clone_path2)
                .output()
        })
        .await;
    }

    // Re-point the EXISTING project at the freshly cloned directory.
    let pid2 = id.clone();
    let np = project_path.clone();
    match state
        .db
        .with_conn(move |conn| crate::db::projects::update_project_path(conn, &pid2, &np))
        .await
    {
        Ok(true) => {}
        Ok(false) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {e}"))),
    }

    // Sync plugins (MCP) + native skills/profiles to the new path.
    resync_project_assets(&state, &id).await;

    Json(ApiResponse::ok(CloneAndRemapResponse {
        project_id: id,
        new_path: project_path,
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

    #[test]
    fn kebab_dir_name_basic() {
        assert_eq!(kebab_dir_name("My Cool Project"), "my-cool-project");
    }

    #[test]
    fn kebab_dir_name_trims_and_collapses_non_alnum() {
        // Leading/trailing separators are trimmed; interior runs of
        // non-alphanumerics each become a single '-' is NOT collapsed
        // (matches clone_project's char-map rule), but edges are clean.
        assert_eq!(kebab_dir_name("  _Foo.Bar_  "), "foo-bar");
        assert_eq!(kebab_dir_name("repo.git"), "repo-git");
    }

    #[test]
    fn kebab_dir_name_empty_or_all_separators_falls_back_to_repo() {
        assert_eq!(kebab_dir_name(""), "repo");
        assert_eq!(kebab_dir_name("---"), "repo");
        assert_eq!(kebab_dir_name("...___"), "repo");
    }

    #[test]
    fn redact_masks_token_in_https_url() {
        let s = "fatal: unable to access 'https://x-access-token:ghp_SECRET123@github.com/org/repo.git/'";
        let red = redact_url_credentials(s);
        assert!(!red.contains("ghp_SECRET123"), "token must be masked: {red}");
        assert!(red.contains("https://***@github.com/org/repo.git"), "got: {red}");
    }

    #[test]
    fn redact_leaves_clean_urls_untouched() {
        // No userinfo → unchanged (the @ in an SSH-ish or path context is not
        // before the first '/').
        let s = "Cloning into 'https://github.com/org/repo.git'...";
        assert_eq!(redact_url_credentials(s), s);
    }

    #[test]
    fn redact_handles_gitlab_oauth_form() {
        let s = "error: https://oauth2:glpat-abc@gitlab.com/g/r.git failed";
        let red = redact_url_credentials(s);
        assert!(!red.contains("glpat-abc"));
        assert!(red.contains("https://***@gitlab.com/g/r.git"));
    }

    #[test]
    fn redact_no_scheme_is_unchanged() {
        // No `://` anywhere → the loop never enters; string passes through.
        let s = "fatal: could not read Username for 'github'";
        assert_eq!(redact_url_credentials(s), s);
    }

    #[test]
    fn redact_userinfo_without_path() {
        // Userinfo `@` with NO `/` after the host → exercises the
        // `maybe_slash … unwrap_or(true)` arm (mask, keep `@host`).
        let red = redact_url_credentials("https://x-access-token:ghp_S3cret@github.com");
        assert!(!red.contains("ghp_S3cret"));
        assert_eq!(red, "https://***@github.com");
    }

    #[test]
    fn redact_masks_each_of_multiple_urls() {
        // Two credentialed URLs in one line → both masked (loop runs twice).
        let s = "https://a:tok1@h1.com/x and https://b:tok2@h2.com/y";
        let red = redact_url_credentials(s);
        assert!(!red.contains("tok1") && !red.contains("tok2"), "got: {red}");
        assert!(red.contains("https://***@h1.com/x"));
        assert!(red.contains("https://***@h2.com/y"));
    }

    // ── build_clone_candidates (pure core of candidate_clone_urls) ──────────

    fn gh(token: &str) -> (String, String) { ("github".into(), token.into()) }

    #[test]
    fn candidates_non_https_yields_only_the_bare_url() {
        // SSH / git@ URLs: nothing to inject, no SSH conversion — just itself.
        let out = build_clone_candidates("git@github.com:org/repo.git", &[gh("ghp_x")], true);
        assert_eq!(out, vec!["git@github.com:org/repo.git".to_string()]);
    }

    #[test]
    fn candidates_bare_url_is_always_first() {
        // The bare URL must be tried first (native credential helper / SSH cfg),
        // before any injected PAT — the whole point of the 403-on-work-org fix.
        let out = build_clone_candidates("https://github.com/org/repo.git", &[gh("ghp_x")], false);
        assert_eq!(out[0], "https://github.com/org/repo.git");
        assert_eq!(out[1], "https://x-access-token:ghp_x@github.com/org/repo.git");
    }

    #[test]
    fn candidates_appends_ssh_form_when_keys_present() {
        let out = build_clone_candidates("https://github.com/org/repo.git", &[], true);
        assert_eq!(out, vec![
            "https://github.com/org/repo.git".to_string(),
            "git@github.com:org/repo.git".to_string(),
        ]);
    }

    #[test]
    fn candidates_no_ssh_form_without_keys() {
        let out = build_clone_candidates("https://github.com/org/repo.git", &[], false);
        assert_eq!(out, vec!["https://github.com/org/repo.git".to_string()]);
    }

    #[test]
    fn candidates_full_order_token_then_ssh() {
        let out = build_clone_candidates("https://github.com/org/repo.git", &[gh("ghp_x")], true);
        assert_eq!(out, vec![
            "https://github.com/org/repo.git".to_string(),
            "https://x-access-token:ghp_x@github.com/org/repo.git".to_string(),
            "git@github.com:org/repo.git".to_string(),
        ]);
    }

    #[test]
    fn candidates_dedup_preserves_first_occurrence() {
        // Two identical github sources → the injected URL must appear once.
        let out = build_clone_candidates(
            "https://github.com/org/repo.git",
            &[gh("ghp_x"), gh("ghp_x")],
            false,
        );
        assert_eq!(out, vec![
            "https://github.com/org/repo.git".to_string(),
            "https://x-access-token:ghp_x@github.com/org/repo.git".to_string(),
        ]);
    }

    #[test]
    fn candidates_skips_token_for_non_matching_provider() {
        // A gitlab token on a github URL injects nothing → only bare (+SSH off).
        let out = build_clone_candidates(
            "https://github.com/org/repo.git",
            &[("gitlab".into(), "glpat-x".into())],
            false,
        );
        assert_eq!(out, vec!["https://github.com/org/repo.git".to_string()]);
    }

    #[test]
    fn candidates_no_ssh_for_non_github_gitlab_host() {
        // SSH form only for github/gitlab; a self-hosted https host with keys
        // present still gets no SSH candidate.
        let out = build_clone_candidates("https://git.example.com/org/repo.git", &[], true);
        assert_eq!(out, vec!["https://git.example.com/org/repo.git".to_string()]);
    }
}
