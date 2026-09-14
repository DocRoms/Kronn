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
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));
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
        let stale = cache
            .refreshed_at
            .is_none_or(|at| at.elapsed() >= CACHE_TTL);
        if !stale || cache.refreshing {
            false
        } else {
            cache.refreshing = true;
            true
        }
    };
    if should_spawn {
        tokio::spawn(async {
            let _ = refresh().await;
        });
    }
}

/// Explicit user-triggered recheck. Concurrent callers share the active refresh.
pub async fn refresh_now() {
    let should_run = {
        let Ok(mut cache) = CACHE.lock() else { return };
        if cache.refreshing {
            false
        } else {
            cache.refreshing = true;
            true
        }
    };
    if should_run {
        let _ = refresh().await;
    }
}

async fn refresh() -> Result<(), ()> {
    let result = tokio::time::timeout(REFRESH_BUDGET, refresh_all()).await;
    if result.is_err() {
        if let Ok(mut cache) = CACHE.lock() {
            cache.refreshed_at = Some(Instant::now());
            cache.refreshing = false;
        }
    }
    result.map_err(|_| ())?;
    Ok(())
}

async fn refresh_all() {
    let client = match reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("Kronn/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(_) => return,
    };
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
    let results = futures::future::join_all(sources.into_iter().map(|(key, source)| {
        let client = client.clone();
        async move { (key, fetch_stable(&client, source).await) }
    }))
    .await;
    if let Ok(mut cache) = CACHE.lock() {
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
        cache.refreshed_at = Some(Instant::now());
        cache.refreshing = false;
    }
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
    let latest = match source {
        Source::Npm(_) => response
            .json::<Npm>()
            .await
            .ok()
            .map(|body| body.dist_tags.latest),
        Source::Pypi(_) => response
            .json::<Pypi>()
            .await
            .ok()
            .map(|body| body.info.version),
        Source::GitHub(_) => response
            .json::<GitHubRelease>()
            .await
            .ok()
            .and_then(|body| (!body.prerelease && !body.draft).then(|| body.tag_name)),
    }
    .map(|version| version.trim_start_matches('v').to_string())
    .filter(|version| is_stable_version(version));
    ReleaseStatus {
        error: latest
            .is_none()
            .then(|| "Official source did not provide a stable numeric release.".into()),
        latest,
        checked_at,
        source_url: Some(source_url),
    }
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
}
