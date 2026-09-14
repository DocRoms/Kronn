//! Bounded discovery of stable CLI releases.
//!
//! Installed versions are probed by their respective integrations. This module
//! only discovers available releases and never performs installation.

use crate::models::AgentType;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const REFRESH_BUDGET: Duration = Duration::from_secs(8);
/// Upgrade remains an explicit user action; discovery never invokes it.
pub const RTK_UPDATE_CMD: &str =
    "curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/main/install.sh | sh";

#[derive(Clone, Debug, Default)]
pub struct ReleaseStatus {
    pub latest: Option<String>,
    pub checked_at: Option<String>,
    pub error: Option<String>,
    pub source_url: Option<String>,
}

#[derive(Clone, Copy)]
enum Source {
    Npm(&'static str),
    Pypi(&'static str),
    GitHub(&'static str),
}

impl Source {
    fn url(self) -> String {
        match self {
            Self::Npm(package) => format!("https://registry.npmjs.org/{package}"),
            Self::Pypi(package) => format!("https://pypi.org/pypi/{package}/json"),
            Self::GitHub(repo) => format!("https://api.github.com/repos/{repo}/releases/latest"),
        }
    }
}

fn sources() -> Vec<(String, Source)> {
    let mut sources: Vec<(String, Source)> = [
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::OpenCode,
        AgentType::Vibe,
        AgentType::GeminiCli,
        AgentType::CopilotCli,
        AgentType::Ollama,
        AgentType::LiteLlm,
    ]
    .into_iter()
    .filter_map(|agent| agent_source(&agent).map(|source| (key_for_agent(&agent), source)))
    .collect();
    sources.extend([
        (RTK_KEY.to_string(), RTK_SOURCE),
        (CCUSAGE_KEY.to_string(), CCUSAGE_SOURCE),
    ]);
    sources
}

fn agent_source(agent: &AgentType) -> Option<Source> {
    match agent {
        AgentType::ClaudeCode => Some(Source::Npm("@anthropic-ai/claude-code")),
        AgentType::Codex => Some(Source::Npm("@openai/codex")),
        AgentType::OpenCode => Some(Source::Npm("opencode-ai")),
        AgentType::Vibe => Some(Source::Pypi("mistral-vibe")),
        AgentType::GeminiCli => Some(Source::Npm("@google/gemini-cli")),
        AgentType::CopilotCli => Some(Source::Npm("@github/copilot")),
        AgentType::Ollama => Some(Source::GitHub("ollama/ollama")),
        AgentType::LiteLlm => Some(Source::Pypi("litellm")),
        AgentType::Kiro | AgentType::Nvidia | AgentType::Custom => None,
    }
}

const RTK_SOURCE: Source = Source::GitHub("rtk-ai/rtk");
const CCUSAGE_SOURCE: Source = Source::Npm("ccusage");

#[derive(Default)]
struct Cache {
    values: HashMap<String, ReleaseStatus>,
    refreshed_at: Option<Instant>,
    refreshing: bool,
    refresh_generation: u64,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));
static REFRESH_COMPLETE: LazyLock<tokio::sync::watch::Sender<u64>> =
    LazyLock::new(|| tokio::sync::watch::channel(0).0);
fn key_for_agent(agent: &AgentType) -> String {
    format!("agent:{agent:?}")
}
const RTK_KEY: &str = "rtk";
const CCUSAGE_KEY: &str = "ccusage";

fn unknown(source: Option<Source>) -> ReleaseStatus {
    match source {
        Some(source) => ReleaseStatus {
            source_url: Some(source.url()),
            ..ReleaseStatus::default()
        },
        None => ReleaseStatus {
            error: Some("No verified stable release source is configured for this tool.".into()),
            ..ReleaseStatus::default()
        },
    }
}

pub fn agent_status(agent: &AgentType) -> ReleaseStatus {
    CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.values.get(&key_for_agent(agent)).cloned())
        .unwrap_or_else(|| unknown(agent_source(agent)))
}
pub fn rtk_status() -> ReleaseStatus {
    CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.values.get(RTK_KEY).cloned())
        .unwrap_or_else(|| unknown(Some(RTK_SOURCE)))
}
pub fn ccusage_status() -> ReleaseStatus {
    CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.values.get(CCUSAGE_KEY).cloned())
        .unwrap_or_else(|| unknown(Some(CCUSAGE_SOURCE)))
}

/// Starts one refresh when the snapshot is absent or expired. It returns
/// immediately, so agent detection and application startup never wait on I/O.
pub fn refresh_if_stale() {
    let should_spawn = {
        let Ok(mut cache) = CACHE.lock() else { return };
        begin_refresh(&mut cache, false)
    };
    if should_spawn {
        tokio::spawn(async {
            let _ = refresh().await;
        });
    }
}

/// Explicit user-triggered recheck. Concurrent callers share the active refresh.
pub async fn refresh_now() {
    let mut completed = REFRESH_COMPLETE.subscribe();
    let (should_run, generation) = {
        let Ok(mut cache) = CACHE.lock() else { return };
        (begin_refresh(&mut cache, true), cache.refresh_generation)
    };
    if should_run {
        let _ = refresh().await;
    } else {
        while *completed.borrow() == generation {
            if completed.changed().await.is_err() {
                return;
            }
        }
    }
}

async fn refresh() -> Result<(), ()> {
    let result = tokio::time::timeout(REFRESH_BUDGET, refresh_all()).await;
    let timed_out = result.is_err();
    let statuses = match result {
        Ok(statuses) => statuses,
        Err(_) => sources()
            .into_iter()
            .map(|(key, source)| (key, failed_status(source, "Official source timed out.")))
            .collect(),
    };
    if let Ok(mut cache) = CACHE.lock() {
        apply_results(&mut cache, statuses);
        cache.refreshed_at = Some(Instant::now());
        cache.refreshing = false;
        cache.refresh_generation += 1;
        REFRESH_COMPLETE.send_replace(cache.refresh_generation);
    }
    if timed_out { Err(()) } else { Ok(()) }
}

fn begin_refresh(cache: &mut Cache, force: bool) -> bool {
    let stale = cache
        .refreshed_at
        .is_none_or(|at| at.elapsed() >= CACHE_TTL);
    if cache.refreshing || (!force && !stale) {
        false
    } else {
        cache.refreshing = true;
        true
    }
}

fn apply_results(cache: &mut Cache, results: Vec<(String, ReleaseStatus)>) {
    for (key, status) in results {
        let previous = cache.values.get(&key).cloned();
        cache.values.insert(
            key,
            match (previous, status.latest.is_none()) {
                (Some(mut previous), true) => {
                    previous.error = status.error;
                    previous.source_url = status.source_url;
                    previous.checked_at = status.checked_at;
                    previous
                }
                (_, _) => status,
            },
        );
    }
}

fn failed_status(source: Source, error: &str) -> ReleaseStatus {
    ReleaseStatus {
        checked_at: Some(chrono::Utc::now().to_rfc3339()),
        error: Some(error.to_string()),
        source_url: Some(source.url()),
        latest: None,
    }
}

async fn refresh_all() -> Vec<(String, ReleaseStatus)> {
    let client = match reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("Kronn/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            return sources()
                .into_iter()
                .map(|(key, source)| {
                    (
                        key,
                        failed_status(source, "Official source client could not be created."),
                    )
                })
                .collect()
        }
    };
    futures::future::join_all(sources().into_iter().map(|(key, source)| {
        let client = client.clone();
        async move { (key, fetch_stable(&client, source).await) }
    }))
    .await
}

#[derive(Deserialize)]
struct Npm {
    #[serde(rename = "dist-tags")]
    dist_tags: DistTags,
}
#[derive(Deserialize)]
struct DistTags {
    latest: String,
}
#[derive(Deserialize)]
struct Pypi {
    info: PypiInfo,
}
#[derive(Deserialize)]
struct PypiInfo {
    version: String,
}
#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    prerelease: bool,
    draft: bool,
}

async fn fetch_stable(client: &reqwest::Client, source: Source) -> ReleaseStatus {
    let source_url = source.url();
    let checked_at = Some(chrono::Utc::now().to_rfc3339());
    let response = match client.get(&source_url).send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            return ReleaseStatus {
                checked_at,
                source_url: Some(source_url),
                error: Some(format!(
                    "Official source returned HTTP {}.",
                    response.status()
                )),
                latest: None,
            }
        }
        Err(_) => {
            return ReleaseStatus {
                checked_at,
                source_url: Some(source_url),
                error: Some("Official source could not be reached.".into()),
                latest: None,
            }
        }
    };
    let latest = response
        .bytes()
        .await
        .ok()
        .and_then(|body| parse_latest(source, &body));
    ReleaseStatus {
        error: latest
            .is_none()
            .then(|| "Official source did not provide a stable numeric release.".into()),
        latest,
        checked_at,
        source_url: Some(source_url),
    }
}

fn parse_latest(source: Source, body: &[u8]) -> Option<String> {
    let version = match source {
        Source::Npm(_) => serde_json::from_slice::<Npm>(body)
            .ok()
            .map(|body| body.dist_tags.latest),
        Source::Pypi(_) => serde_json::from_slice::<Pypi>(body)
            .ok()
            .map(|body| body.info.version),
        Source::GitHub(_) => serde_json::from_slice::<GitHubRelease>(body)
            .ok()
            .and_then(|body| (!body.prerelease && !body.draft).then(|| body.tag_name)),
    }?;
    let version = version.trim_start_matches('v').to_string();
    is_stable_version(&version).then_some(version)
}

fn is_stable_version(version: &str) -> bool {
    !version.is_empty()
        && !version.contains('-')
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.parse::<u64>().is_ok())
}

/// Lenient comparison used for the installed-versus-available display.
pub fn update_available(installed: &str, latest: &str) -> bool {
    fn parse(v: &str) -> Option<Vec<u64>> {
        v.trim()
            .trim_start_matches('v')
            .split(['-', '+'])
            .next()?
            .split('.')
            .map(str::parse)
            .collect::<Result<Vec<u64>, _>>()
            .ok()
    }
    let (Some(installed), Some(latest)) = (parse(installed), parse(latest)) else {
        return false;
    };
    for index in 0..installed.len().max(latest.len()) {
        match installed
            .get(index)
            .copied()
            .unwrap_or(0)
            .cmp(&latest.get(index).copied().unwrap_or(0))
        {
            std::cmp::Ordering::Less => return true,
            std::cmp::Ordering::Greater => return false,
            std::cmp::Ordering::Equal => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_versions_reject_prereleases_and_invalid_values() {
        assert!(is_stable_version("1.2.3"));
        assert!(!is_stable_version("1.2.3-rc.1"));
        assert!(!is_stable_version("latest"));
    }
    #[test]
    fn update_comparison_is_lenient() {
        assert!(update_available("v1.2.3", "1.2.4"));
        assert!(!update_available("1.2.3-rc1", "1.2.3"));
        assert!(!update_available("dev", "1.2.3"));
    }
    #[test]
    fn unsupported_tools_are_explicitly_unknown() {
        assert!(agent_status(&AgentType::Kiro).latest.is_none());
        assert!(agent_status(&AgentType::Kiro).error.is_some());
    }
    #[test]
    fn official_payloads_select_only_stable_releases() {
        assert_eq!(
            parse_latest(Source::Npm("x"), br#"{"dist-tags": {"latest": "2.3.4"}}"#),
            Some("2.3.4".into())
        );
        assert_eq!(
            parse_latest(Source::Pypi("x"), br#"{"info": {"version": "2.3.4rc1"}}"#),
            None
        );
        assert_eq!(
            parse_latest(
                Source::GitHub("x/y"),
                br#"{"tag_name":"v2.3.4-rc.1","prerelease":true,"draft":false}"#
            ),
            None
        );
    }
    #[test]
    fn failed_refresh_keeps_last_good_release_and_records_error() {
        let mut cache = Cache::default();
        cache.values.insert(
            RTK_KEY.into(),
            ReleaseStatus {
                latest: Some("1.2.3".into()),
                ..ReleaseStatus::default()
            },
        );
        apply_results(
            &mut cache,
            vec![(
                RTK_KEY.into(),
                failed_status(RTK_SOURCE, "Official source timed out."),
            )],
        );
        let status = &cache.values[RTK_KEY];
        assert_eq!(status.latest.as_deref(), Some("1.2.3"));
        assert_eq!(status.error.as_deref(), Some("Official source timed out."));
    }
    #[test]
    fn refresh_start_is_deduplicated_and_respects_ttl() {
        let mut cache = Cache::default();
        assert!(begin_refresh(&mut cache, false));
        assert!(!begin_refresh(&mut cache, true));
        cache.refreshing = false;
        cache.refreshed_at = Some(Instant::now());
        assert!(!begin_refresh(&mut cache, false));
        assert!(begin_refresh(&mut cache, true));
    }
}
