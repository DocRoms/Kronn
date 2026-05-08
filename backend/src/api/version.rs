//! Version-check endpoint for the auto-update banner.
//!
//! Frontend calls `GET /api/version/check` on mount; backend returns
//! the running version + the latest GitHub release tag (cached 6h).
//! Frontend renders a subtle bar in the header when the latest is
//! newer than current.
//!
//! # Why backend-side
//!
//! Doing the GitHub fetch from the browser would CORS-fail and burn
//! a request from the user's IP rate budget. The backend can:
//!   - Sit behind a single rate-limited daily fetch.
//!   - Cache the result so a 100-user UI burst doesn't fan out to
//!     GitHub.
//!   - Skip the fetch entirely when offline (returns last cached, or
//!     `latest: None`).
//!
//! # Why no auto-install
//!
//! Self-hosted Kronn lives in a wide variety of installs: native
//! `.deb`, Tauri, Docker, manual `cargo install`. There's no single
//! upgrade path we could automate without breaking one of them. The
//! banner just links the user to the release page; they do `make
//! bump` / `apt upgrade` / Tauri auto-updater themselves.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use ts_rs::TS;

use crate::AppState;
use crate::models::ApiResponse;

const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600); // 6 h
const REPO: &str = "DocRoms/Kronn";

#[derive(Debug, Clone, Serialize, Deserialize, TS, utoipa::ToSchema)]
#[ts(export)]
pub struct VersionCheck {
    pub current: String,
    pub latest: Option<String>,
    pub release_url: Option<String>,
    pub up_to_date: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
}

struct Cache {
    fetched_at: Instant,
    latest: Option<String>,
    release_url: Option<String>,
}

fn cache() -> &'static Mutex<Option<Cache>> {
    static CACHE: OnceLock<Mutex<Option<Cache>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Strip a leading `v` from `v0.7.2` → `0.7.2` for clean comparison.
fn normalize_tag(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// Compare two semver strings naively (major.minor.patch). Returns
/// `true` when `current >= latest`. Falls through to string equality
/// on parse failure — safer to under-flag updates than to spam users
/// with a banner triggered by a malformed tag.
fn is_up_to_date(current: &str, latest: &str) -> bool {
    let parse = |s: &str| -> Option<(u64, u64, u64)> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() < 3 { return None; }
        Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            // Drop pre-release suffix like "-rc1" — common in release tags.
            parts[2].split('-').next()?.parse().ok()?,
        ))
    };
    match (parse(current), parse(latest)) {
        (Some(c), Some(l)) => c >= l,
        // Fallback when either side can't parse: assume up-to-date so a
        // malformed tag never spams the user with an "update available"
        // banner. Worst case = a real upgrade gets quietly hidden until
        // the next valid tag — much less annoying than a false alarm.
        _ => true,
    }
}

async fn fetch_latest() -> Option<(String, String)> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", REPO);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(concat!("Kronn/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() { return None; }
    let release: GitHubRelease = resp.json().await.ok()?;
    Some((release.tag_name, release.html_url))
}

/// `GET /api/version/check` — returns the current+latest version pair,
/// using a 6h in-memory cache to bound the GitHub API rate burn.
pub async fn check(
    State(_state): State<AppState>,
) -> Json<ApiResponse<VersionCheck>> {
    let current = env!("CARGO_PKG_VERSION").to_string();

    let mut cache_guard = cache().lock().await;
    let cached_fresh = cache_guard
        .as_ref()
        .is_some_and(|c| c.fetched_at.elapsed() < CHECK_INTERVAL);

    if !cached_fresh {
        // Fire the fetch. On failure (offline, GitHub 5xx) keep the
        // existing cache (if any) but stamp `fetched_at` so we don't
        // hammer GitHub on every request when offline.
        if let Some((tag, url)) = fetch_latest().await {
            *cache_guard = Some(Cache {
                fetched_at: Instant::now(),
                latest: Some(normalize_tag(&tag).to_string()),
                release_url: Some(url),
            });
        } else if cache_guard.is_none() {
            // Mark we tried and failed — bounded retry interval.
            *cache_guard = Some(Cache {
                fetched_at: Instant::now(),
                latest: None,
                release_url: None,
            });
        } else if let Some(ref mut c) = *cache_guard {
            c.fetched_at = Instant::now();
        }
    }

    let (latest, release_url) = match cache_guard.as_ref() {
        Some(c) => (c.latest.clone(), c.release_url.clone()),
        None => (None, None),
    };
    let up_to_date = match &latest {
        Some(l) => is_up_to_date(&current, l),
        None => true,
    };

    Json(ApiResponse::ok(VersionCheck {
        current,
        latest,
        release_url,
        up_to_date,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_v_prefix() {
        assert_eq!(normalize_tag("v0.7.2"), "0.7.2");
        assert_eq!(normalize_tag("0.7.2"), "0.7.2");
        assert_eq!(normalize_tag("v1.0"), "1.0");
    }

    #[test]
    fn up_to_date_when_current_equals_latest() {
        assert!(is_up_to_date("0.7.1", "0.7.1"));
    }

    #[test]
    fn up_to_date_when_current_ahead() {
        // Local dev build can be ahead of the public release while a
        // PR is in-flight; we don't want to nag about a "downgrade".
        assert!(is_up_to_date("0.8.0", "0.7.2"));
        assert!(is_up_to_date("1.0.0", "0.99.99"));
    }

    #[test]
    fn not_up_to_date_when_latest_is_newer() {
        assert!(!is_up_to_date("0.7.1", "0.7.2"));
        assert!(!is_up_to_date("0.7.1", "0.8.0"));
        assert!(!is_up_to_date("0.7.1", "1.0.0"));
    }

    #[test]
    fn handles_pre_release_suffix() {
        // 0.7.2-rc1 and 0.7.2 should compare as same patch — we drop
        // the suffix on the patch component. Means a user on 0.7.2
        // doesn't see "update available: 0.7.2-rc1" if a pre-release
        // happens to be tagged later.
        assert!(is_up_to_date("0.7.2", "0.7.2-rc1"));
    }

    #[test]
    fn malformed_tag_assumes_up_to_date() {
        // A garbage `latest` could come from a stray git tag or a
        // GitHub API hiccup. Assume up-to-date so we don't nag the
        // user with a banner triggered by parse failure.
        assert!(is_up_to_date("0.7.1", "garbage"));
        assert!(is_up_to_date("garbage", "0.7.1"));
        assert!(is_up_to_date("garbage", "garbage"));
    }
}
