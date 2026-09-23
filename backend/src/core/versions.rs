//! Bounded discovery of stable CLI releases.
//!
//! Installed versions are probed by their respective integrations. This module
//! only discovers available releases and never performs installation.

use crate::models::AgentType;
use serde::Deserialize;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const REFRESH_BUDGET: Duration = Duration::from_secs(8);
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
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
            Self::Npm(package) => format!("https://registry.npmjs.org/{package}/latest"),
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

struct ReleaseCache {
    state: Mutex<Cache>,
    completed: tokio::sync::watch::Sender<u64>,
}

impl Default for ReleaseCache {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            completed: tokio::sync::watch::channel(0).0,
        }
    }
}

impl ReleaseCache {
    fn start(
        self: &Arc<Self>,
        force: bool,
        fetch: impl Future<Output = Vec<(String, ReleaseStatus)>> + Send + 'static,
    ) -> (tokio::sync::watch::Receiver<u64>, u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Subscribe while holding the state lock so completion cannot fall
        // between reading the generation and subscribing to its notification.
        let receiver = self.completed.subscribe();
        let generation = state.refresh_generation;
        if begin_refresh(&mut state, force) {
            // The request owns only its wait, not the shared refresh. The lease
            // also clears the flag if the spawned future is dropped or panics.
            let lease = RefreshLease {
                owner: self.clone(),
                completed: false,
            };
            tokio::spawn(async move {
                let statuses = tokio::time::timeout(REFRESH_BUDGET, fetch)
                    .await
                    .unwrap_or_else(|_| failure_results("Official source timed out."));
                lease.finish(statuses);
            });
        }
        (receiver, generation)
    }

    fn finish(&self, statuses: Vec<(String, ReleaseStatus)>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        apply_results(&mut state, statuses);
        state.refreshed_at = Some(Instant::now());
        state.refreshing = false;
        state.refresh_generation = state.refresh_generation.wrapping_add(1);
        self.completed.send_replace(state.refresh_generation);
    }
}

struct RefreshLease {
    owner: Arc<ReleaseCache>,
    completed: bool,
}

impl RefreshLease {
    fn finish(mut self, statuses: Vec<(String, ReleaseStatus)>) {
        self.owner.finish(statuses);
        self.completed = true;
    }
}

impl Drop for RefreshLease {
    fn drop(&mut self) {
        if !self.completed {
            self.owner
                .finish(failure_results("Official source refresh was interrupted."));
        }
    }
}

static CACHE: LazyLock<Arc<ReleaseCache>> = LazyLock::new(Arc::default);
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
        .state
        .lock()
        .ok()
        .and_then(|cache| cache.values.get(&key_for_agent(agent)).cloned())
        .unwrap_or_else(|| unknown(agent_source(agent)))
}
pub fn rtk_status() -> ReleaseStatus {
    CACHE
        .state
        .lock()
        .ok()
        .and_then(|cache| cache.values.get(RTK_KEY).cloned())
        .unwrap_or_else(|| unknown(Some(RTK_SOURCE)))
}
pub fn ccusage_status() -> ReleaseStatus {
    CACHE
        .state
        .lock()
        .ok()
        .and_then(|cache| cache.values.get(CCUSAGE_KEY).cloned())
        .unwrap_or_else(|| unknown(Some(CCUSAGE_SOURCE)))
}

/// Starts one refresh when the snapshot is absent or expired. It returns
/// immediately, so agent detection and application startup never wait on I/O.
pub fn refresh_if_stale() {
    CACHE.start(false, refresh_all());
}

/// Explicit user-triggered recheck. Concurrent callers share the active refresh.
pub async fn refresh_now() {
    let (completed, generation) = CACHE.start(true, refresh_all());
    wait_for_refresh(completed, generation).await;
}

/// Await a fresh snapshot for a settings read without bypassing the TTL.
pub async fn refresh_if_stale_and_wait() {
    let (completed, generation) = CACHE.start(false, refresh_all());
    // A fresh idle cache did not start a refresh and needs no notification.
    let refreshing = CACHE
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .refreshing;
    if refreshing {
        wait_for_refresh(completed, generation).await;
    }
}

async fn wait_for_refresh(mut completed: tokio::sync::watch::Receiver<u64>, generation: u64) {
    let _ = tokio::time::timeout(REFRESH_BUDGET + Duration::from_secs(1), async {
        while *completed.borrow() == generation {
            if completed.changed().await.is_err() {
                return;
            }
        }
    })
    .await;
}

fn failure_results(error: &str) -> Vec<(String, ReleaseStatus)> {
    sources()
        .into_iter()
        .map(|(key, source)| (key, failed_status(source, error)))
        .collect()
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
        .redirect(reqwest::redirect::Policy::none())
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
    version: String,
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
    fetch_stable_at(client, source, &source.url()).await
}

async fn fetch_stable_at(
    client: &reqwest::Client,
    source: Source,
    request_url: &str,
) -> ReleaseStatus {
    let source_url = source.url();
    let checked_at = Some(chrono::Utc::now().to_rfc3339());
    let mut response = match client.get(request_url).send().await {
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
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return failed_status(source, "Official source response exceeded the size limit.");
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= MAX_RESPONSE_BYTES - body.len() => {
                body.extend_from_slice(&chunk);
            }
            Ok(Some(_)) => {
                return failed_status(source, "Official source response exceeded the size limit.");
            }
            Ok(None) => break,
            Err(_) => return failed_status(source, "Official source response could not be read."),
        }
    }
    let latest = parse_latest(source, &body);
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
            .map(|body| body.version),
        Source::Pypi(_) => serde_json::from_slice::<Pypi>(body)
            .ok()
            .map(|body| body.info.version),
        Source::GitHub(_) => serde_json::from_slice::<GitHubRelease>(body)
            .ok()
            .and_then(|body| (!body.prerelease && !body.draft).then_some(body.tag_name)),
    }?;
    let version = version.strip_prefix('v').unwrap_or(&version).to_string();
    is_stable_version(&version).then_some(version)
}

fn is_stable_version(version: &str) -> bool {
    !version.is_empty()
        && !version.contains('-')
        && version.split('.').all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && part.parse::<u64>().is_ok()
        })
}

/// Probe an already-resolved executable, never a package-runner install command.
/// The child and its output are bounded, including cancellation by the caller.
pub async fn installed_version(program: &std::path::Path) -> Option<String> {
    probe_installed_version(program, Duration::from_secs(3)).await
}

async fn probe_installed_version(program: &std::path::Path, timeout: Duration) -> Option<String> {
    use tokio::io::AsyncReadExt;
    const MAX_OUTPUT_BYTES: usize = 8192;
    let mut child = crate::core::cmd::async_cmd(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let result = tokio::time::timeout(timeout, async {
        let mut output = Vec::new();
        child
            .stdout
            .take()?
            .take((MAX_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut output)
            .await
            .ok()?;
        if output.len() > MAX_OUTPUT_BYTES || !child.wait().await.ok()?.success() {
            return None;
        }
        parse_installed_version(&output)
    })
    .await
    .ok()
    .flatten();
    // kill() also waits/reaps; a child which already exited needs no action.
    if result.is_none() {
        let _ = child.kill().await;
    }
    result
}

fn parse_installed_version(output: &[u8]) -> Option<String> {
    std::str::from_utf8(output)
        .ok()?
        .split_whitespace()
        .map(|token| token.strip_prefix('v').unwrap_or(token))
        .find(|token| token.contains('.') && is_stable_version(token))
        .map(str::to_string)
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
    fn installed_version_requires_numeric_components_not_a_bare_counter() {
        for text in ["ccusage 20.1.2\n", "v20.1.2\n", "rtk 0.37.2"] {
            assert!(parse_installed_version(text.as_bytes()).is_some());
        }
        for text in [
            "42",
            "ccusage +20.1.2",
            "vv20.1.2",
            "20.1.2-rc.1",
            "not installed",
        ] {
            assert!(parse_installed_version(text.as_bytes()).is_none(), "{text}");
        }
        assert!(parse_installed_version(&[0xff, 0xfe]).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installed_probe_only_requests_version_and_bounds_process_and_output() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let program = temp.path().join("fake-cli");
        // Give non-timeout cases enough time to spawn on loaded hosts. The output bound
        // is MAX_OUTPUT_BYTES; only the timeout case needs a short deadline.
        for (body, timeout, expected) in [
            (
                "[ \"$#\" = 1 ] && [ \"$1\" = --version ] || exit 91\nprintf 'ccusage v20.1.2\\n'",
                Duration::from_secs(10),
                Some("20.1.2"),
            ),
            ("printf '20.1.2\\n'; exit 1", Duration::from_secs(10), None),
            ("exec /usr/bin/yes x", Duration::from_secs(10), None),
            ("exec /bin/sleep 30", Duration::from_millis(250), None),
        ] {
            std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
            let started = std::time::Instant::now();
            let result = probe_installed_version(&program, timeout).await;
            assert_eq!(result.as_deref(), expected, "{body}");
            // A generous budget must not become a slow test: anything that
            // actually waited for it would mean the bound under test is the
            // clock rather than the one we meant to exercise.
            assert!(
                started.elapsed() < Duration::from_secs(9),
                "{body} leaned on the timeout instead of its own bound"
            );
        }
        assert!(installed_version(&temp.path().join("missing"))
            .await
            .is_none());
    }

    fn successful_results(version: &str) -> Vec<(String, ReleaseStatus)> {
        vec![(
            RTK_KEY.into(),
            ReleaseStatus {
                latest: Some(version.into()),
                checked_at: Some("2026-09-14T00:00:00Z".into()),
                source_url: Some(RTK_SOURCE.url()),
                error: None,
            },
        )]
    }

    #[tokio::test]
    async fn concurrent_callers_share_refresh_even_after_first_waiter_is_cancelled() {
        let cache = Arc::new(ReleaseCache::default());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let (first, generation) = cache.start(true, async move {
            started_tx.send(()).unwrap();
            finish_rx.await.unwrap()
        });
        let waiter = tokio::spawn(wait_for_refresh(first, generation));
        started_rx.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        let (second, same_generation) = cache.start(true, async {
            panic!("an overlapping caller must not start another fetch")
        });
        assert_eq!(same_generation, generation);
        finish_tx.send(successful_results("1.2.3")).unwrap();
        wait_for_refresh(second, same_generation).await;
        {
            let state = cache.state.lock().unwrap();
            assert!(!state.refreshing);
            assert_eq!(state.refresh_generation, 1);
            assert_eq!(state.values[RTK_KEY].latest.as_deref(), Some("1.2.3"));
        }
        let (next, generation) = cache.start(true, async { successful_results("1.2.4") });
        wait_for_refresh(next, generation).await;
        let state = cache.state.lock().unwrap();
        assert_eq!(state.refresh_generation, 2);
        assert_eq!(state.values[RTK_KEY].latest.as_deref(), Some("1.2.4"));
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_preserves_last_good_value_and_allows_a_later_explicit_retry() {
        let cache = Arc::new(ReleaseCache::default());
        cache.finish(successful_results("1.2.3"));
        let (receiver, generation) = cache.start(true, std::future::pending());
        wait_for_refresh(receiver, generation).await;
        {
            let state = cache.state.lock().unwrap();
            assert!(!state.refreshing);
            assert_eq!(state.values[RTK_KEY].latest.as_deref(), Some("1.2.3"));
            assert_eq!(
                state.values[RTK_KEY].error.as_deref(),
                Some("Official source timed out.")
            );
            assert_eq!(state.refresh_generation, generation + 1);
        }
        let (receiver, generation) = cache.start(true, async { successful_results("1.2.4") });
        wait_for_refresh(receiver, generation).await;
        assert!(cache.state.lock().unwrap().values[RTK_KEY].error.is_none());
    }

    #[test]
    fn shutting_down_the_runtime_clears_the_inflight_flag() {
        let cache = Arc::new(ReleaseCache::default());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            cache.start(true, std::future::pending());
            assert!(cache.state.lock().unwrap().refreshing);
        });
        drop(runtime);
        let state = cache.state.lock().unwrap();
        assert!(!state.refreshing);
        assert_eq!(state.refresh_generation, 1);
        assert_eq!(
            state.values[RTK_KEY].error.as_deref(),
            Some("Official source refresh was interrupted.")
        );
    }

    #[tokio::test]
    async fn http_responses_are_bounded_and_errors_remain_explicit() {
        use axum::{
            body::Body,
            http::{Response, StatusCode},
            routing::get,
            Router,
        };
        let app = Router::new()
            .route("/ok", get(|| async { r#"{"version":"20.1.2"}"# }))
            .route("/malformed", get(|| async { "not json" }))
            .route("/limited", get(|| async { StatusCode::TOO_MANY_REQUESTS }))
            .route(
                "/large",
                get(|| async { "x".repeat(MAX_RESPONSE_BYTES + 1) }),
            )
            .route(
                "/chunked",
                get(|| async {
                    let chunks = (0..65).map(|_| Ok::<_, std::io::Error>(vec![b'x'; 64 * 1024]));
                    Response::new(Body::from_stream(futures::stream::iter(chunks)))
                }),
            )
            .route(
                "/slow",
                get(|| async { std::future::pending::<String>().await }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(250))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .unwrap();
        for (path, error) in [
            ("ok", None),
            ("malformed", Some("stable numeric release")),
            ("limited", Some("HTTP 429")),
            ("large", Some("size limit")),
            ("chunked", Some("size limit")),
            ("slow", Some("could not be reached")),
        ] {
            let result =
                fetch_stable_at(&client, CCUSAGE_SOURCE, &format!("http://{address}/{path}")).await;
            assert_eq!(
                result.source_url.as_deref(),
                Some(CCUSAGE_SOURCE.url().as_str())
            );
            assert!(result.checked_at.is_some());
            match error {
                None => {
                    assert_eq!(result.latest.as_deref(), Some("20.1.2"));
                    assert!(result.error.is_none());
                }
                Some(expected) => {
                    assert!(result.latest.is_none(), "{path}");
                    assert!(
                        result.error.as_deref().unwrap().contains(expected),
                        "{path}: {:?}",
                        result.error
                    );
                }
            }
        }
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }
    #[test]
    fn npm_reads_the_bounded_latest_version_document() {
        assert_eq!(
            Source::Npm("@openai/codex").url(),
            "https://registry.npmjs.org/@openai/codex/latest"
        );
        assert_eq!(
            parse_latest(
                Source::Npm("ccusage"),
                br#"{"name":"ccusage","version":"20.1.2"}"#
            ),
            Some("20.1.2".into())
        );
    }

    #[test]
    fn stable_numeric_releases_reject_signs_and_repeated_prefixes() {
        for version in [
            "+1.2.3",
            "1.+2.3",
            "vv1.2.3",
            "1..3",
            "1.2.3\n",
            "18446744073709551616.0.0",
        ] {
            assert!(
                !is_stable_version(version),
                "invalid numeric release {version:?}"
            );
        }
        assert_eq!(
            parse_latest(
                Source::GitHub("x/y"),
                br#"{"tag_name":"vv1.2.3","prerelease":false,"draft":false}"#
            ),
            None
        );
    }
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
            parse_latest(Source::Npm("x"), br#"{"version": "2.3.4"}"#),
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
