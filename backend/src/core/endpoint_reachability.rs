//! Bounded transport probe for a configured HTTP endpoint (KT-697).
//!
//! A saved address is configuration, not connectivity: `agent_list` used to
//! report every named connection as reachable without opening a socket. This
//! probe answers one question — did something answer at that address — with a
//! classified reason when nothing did. It never sends a credential and its
//! result never carries the endpoint, so nothing it reports can leak a secret.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

/// Why an endpoint did not answer. The wire form is `as_str`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreachableReason {
    Dns,
    Refused,
    Timeout,
    Tls,
    HttpStatus,
    InvalidEndpoint,
    Connect,
}

impl UnreachableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dns => "dns",
            Self::Refused => "refused",
            Self::Timeout => "timeout",
            Self::Tls => "tls",
            Self::HttpStatus => "http_status",
            Self::InvalidEndpoint => "invalid_endpoint",
            Self::Connect => "connect",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// An HTTP response came back; any status below 500 proves the transport,
    /// including the 401 an unauthenticated probe usually gets.
    Reachable { http_status: u16 },
    Unreachable {
        reason: UnreachableReason,
        http_status: Option<u16>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointObservation {
    pub outcome: ProbeOutcome,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProbeBudget {
    pub connect: Duration,
    pub total: Duration,
}

/// Short on purpose: `agent_list` waits for the slowest probe.
pub const DEFAULT_BUDGET: ProbeBudget = ProbeBudget {
    connect: Duration::from_secs(2),
    total: Duration::from_secs(3),
};

/// One unauthenticated `GET {endpoint}/v1/models`, the route the legacy
/// LiteLLM probe reads, bounded by `budget.total` end to end.
pub async fn probe_endpoint(endpoint: &str, budget: ProbeBudget) -> ProbeOutcome {
    probe_with_resolver(endpoint, budget, system_resolves).await
}

async fn system_resolves(host: String, port: u16) -> bool {
    match tokio::net::lookup_host((host.as_str(), port)).await {
        Ok(mut addresses) => addresses.next().is_some(),
        Err(_) => false,
    }
}

async fn probe_with_resolver<F, Fut>(
    endpoint: &str,
    budget: ProbeBudget,
    resolves: F,
) -> ProbeOutcome
where
    F: FnOnce(String, u16) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    // One deadline for the request and the diagnosis that may follow it.
    let deadline = tokio::time::Instant::now() + budget.total;
    let unreachable = |reason| ProbeOutcome::Unreachable {
        reason,
        http_status: None,
    };
    let Ok(mut url) = reqwest::Url::parse(endpoint.trim()) else {
        return unreachable(UnreachableReason::InvalidEndpoint);
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return unreachable(UnreachableReason::InvalidEndpoint);
    }
    // Credentials embedded in the address are not needed to prove transport.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let host = url.host_str().unwrap_or_default().to_string();
    let port = url.port_or_known_default().unwrap_or(80);
    let https = url.scheme() == "https";
    let path = format!("{}/v1/models", url.path().trim_end_matches('/'));
    url.set_path(&path);

    let client = reqwest::Client::builder()
        .connect_timeout(budget.connect)
        .timeout(budget.total)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default();
    let response = match tokio::time::timeout_at(deadline, client.get(url).send()).await {
        Err(_) => return unreachable(UnreachableReason::Timeout),
        Ok(response) => response,
    };
    let error = match response {
        Ok(response) if response.status().is_server_error() => {
            return ProbeOutcome::Unreachable {
                reason: UnreachableReason::HttpStatus,
                http_status: Some(response.status().as_u16()),
            }
        }
        Ok(response) => {
            return ProbeOutcome::Reachable {
                http_status: response.status().as_u16(),
            }
        }
        Err(error) => error,
    };
    if error.is_timeout() {
        return unreachable(UnreachableReason::Timeout);
    }
    if let Some(reason) = classify_by_kind(&error, https) {
        return unreachable(reason);
    }
    // The client's wording is not read; the name is asked for directly, in
    // whatever time the deadline leaves.
    let lookup_deadline = deadline.min(tokio::time::Instant::now() + budget.connect);
    match tokio::time::timeout_at(lookup_deadline, resolves(host, port)).await {
        Ok(false) => unreachable(UnreachableReason::Dns),
        Ok(true) | Err(_) => unreachable(UnreachableReason::Connect),
    }
}

/// Reads error types and kinds only: the `Display` of a reqwest error carries
/// the URL, so text matching would classify by host name or path.
fn classify_by_kind(error: &reqwest::Error, https: bool) -> Option<UnreachableReason> {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        // `io::Error::source` skips the wrapped error, so nested io errors
        // are reached through `get_ref`.
        let mut io = current.downcast_ref::<std::io::Error>();
        while let Some(layer) = io {
            match layer.kind() {
                std::io::ErrorKind::ConnectionRefused => return Some(UnreachableReason::Refused),
                std::io::ErrorKind::TimedOut => return Some(UnreachableReason::Timeout),
                // tokio-rustls reports every handshake and certificate
                // failure as `InvalidData` around the rustls error.
                std::io::ErrorKind::InvalidData if https && error.is_connect() => {
                    return Some(UnreachableReason::Tls)
                }
                _ => {}
            }
            io = layer
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<std::io::Error>());
        }
        source = current.source();
    }
    None
}

/// Probe results kept for `ttl`, so repeated `agent_list` calls do not each
/// pay the network round trip, and a recovered endpoint is seen within `ttl`.
pub struct ReachabilityCache {
    ttl: Duration,
    entries: Mutex<HashMap<String, (Instant, EndpointObservation)>>,
}

impl ReachabilityCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub async fn observe(&self, endpoint: &str, budget: ProbeBudget) -> EndpointObservation {
        let key = endpoint.trim().to_string();
        if let Ok(entries) = self.entries.lock() {
            if let Some((at, observation)) = entries.get(&key) {
                if at.elapsed() < self.ttl {
                    return *observation;
                }
            }
        }
        let observation = EndpointObservation {
            outcome: probe_endpoint(&key, budget).await,
            checked_at: Utc::now(),
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, (at, _)| at.elapsed() < self.ttl);
            entries.insert(key, (Instant::now(), observation));
        }
        observation
    }
}

/// How long a cached observation is served.
pub const CACHE_TTL: Duration = Duration::from_secs(30);

/// An observation older than this is reported as unverified, never as fresh.
pub const MAX_OBSERVATION_AGE: Duration = Duration::from_secs(60);

pub static SHARED: LazyLock<ReachabilityCache> =
    LazyLock::new(|| ReachabilityCache::new(CACHE_TTL));

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const FAST: ProbeBudget = ProbeBudget {
        connect: Duration::from_millis(500),
        total: Duration::from_millis(800),
    };

    /// Answers every connection with `status` and records the request heads.
    async fn stub(
        status: u16,
    ) -> (
        String,
        std::sync::Arc<Mutex<Vec<String>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let heads = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen = heads.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = vec![0u8; 4096];
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buffer[..read]).to_string());
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        (format!("http://{address}"), heads, task)
    }

    #[tokio::test]
    async fn an_unresolvable_host_is_a_dns_failure() {
        let outcome = probe_endpoint("http://kt697-no-such-host.invalid", FAST).await;
        assert_eq!(
            outcome,
            ProbeOutcome::Unreachable {
                reason: UnreachableReason::Dns,
                http_status: None
            }
        );
    }

    /// Accepts, holds the connection for `hold`, then closes it unanswered.
    async fn closing_stub(hold: Duration) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    tokio::time::sleep(hold).await;
                    drop(socket);
                });
            }
        });
        (format!("http://{address}"), task)
    }

    /// The reqwest error's text carries the URL: a host or path naming TLS
    /// must not be classified as a TLS failure.
    #[tokio::test]
    async fn a_name_mentioning_tls_is_still_a_dns_failure() {
        for endpoint in [
            "http://kt697-tls-no-such-host.invalid",
            "https://kt697-certificate-handshake.invalid",
            "http://kt697-rustls.invalid/certificate",
        ] {
            assert_eq!(
                probe_endpoint(endpoint, FAST).await,
                ProbeOutcome::Unreachable {
                    reason: UnreachableReason::Dns,
                    http_status: None
                },
                "{endpoint}"
            );
        }
    }

    #[tokio::test]
    async fn a_path_naming_certificate_is_not_a_tls_failure() {
        let (endpoint, task) = closing_stub(Duration::ZERO).await;
        let outcome = probe_endpoint(&format!("{endpoint}/certificate/tls"), FAST).await;
        task.abort();
        assert_eq!(
            outcome,
            ProbeOutcome::Unreachable {
                reason: UnreachableReason::Connect,
                http_status: None
            }
        );
    }

    #[tokio::test]
    async fn a_failed_handshake_is_a_tls_failure() {
        let (endpoint, _, task) = stub(200).await;
        let outcome = probe_endpoint(&endpoint.replace("http://", "https://"), FAST).await;
        task.abort();
        assert_eq!(
            outcome,
            ProbeOutcome::Unreachable {
                reason: UnreachableReason::Tls,
                http_status: None
            }
        );
    }

    /// A late transport error followed by a resolver that never answers: the
    /// diagnosis only gets what is left of the total budget.
    #[tokio::test]
    async fn the_diagnosis_stays_inside_the_total_budget() {
        let budget = ProbeBudget {
            connect: Duration::from_millis(900),
            total: Duration::from_millis(1000),
        };
        let (endpoint, task) = closing_stub(Duration::from_millis(700)).await;
        let started = Instant::now();
        let outcome = probe_with_resolver(&endpoint, budget, |_, _| async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            true
        })
        .await;
        let elapsed = started.elapsed();
        task.abort();
        assert_eq!(
            outcome,
            ProbeOutcome::Unreachable {
                reason: UnreachableReason::Connect,
                http_status: None
            }
        );
        assert!(elapsed < Duration::from_millis(1150), "{elapsed:?}");
    }

    #[tokio::test]
    async fn a_closed_port_is_refused() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let outcome = probe_endpoint(&format!("http://127.0.0.1:{port}"), FAST).await;
        assert_eq!(
            outcome,
            ProbeOutcome::Unreachable {
                reason: UnreachableReason::Refused,
                http_status: None
            }
        );
    }

    #[tokio::test]
    async fn an_answering_stub_is_reachable_even_when_it_refuses_auth() {
        let (endpoint, heads, task) = stub(401).await;
        let outcome = probe_endpoint(&endpoint, FAST).await;
        task.abort();
        assert_eq!(outcome, ProbeOutcome::Reachable { http_status: 401 });
        let heads = heads.lock().unwrap();
        assert!(heads[0].starts_with("GET /v1/models "), "{heads:?}");
    }

    #[tokio::test]
    async fn a_server_error_is_reported_with_its_status() {
        let (endpoint, _, task) = stub(502).await;
        let outcome = probe_endpoint(&endpoint, FAST).await;
        task.abort();
        assert_eq!(
            outcome,
            ProbeOutcome::Unreachable {
                reason: UnreachableReason::HttpStatus,
                http_status: Some(502)
            }
        );
    }

    #[tokio::test]
    async fn a_silent_server_times_out_within_the_budget() {
        // Accepts at the kernel level, never answers.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let started = Instant::now();
        let outcome = probe_endpoint(&endpoint, FAST).await;
        drop(listener);
        assert_eq!(
            outcome,
            ProbeOutcome::Unreachable {
                reason: UnreachableReason::Timeout,
                http_status: None
            }
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn credentials_in_the_address_are_never_sent() {
        let (endpoint, heads, task) = stub(200).await;
        let with_secret = endpoint.replace("http://", "http://user:sk-secret-697@");
        let outcome = probe_endpoint(&format!("{with_secret}/base/"), FAST).await;
        task.abort();
        assert_eq!(outcome, ProbeOutcome::Reachable { http_status: 200 });
        let heads = heads.lock().unwrap();
        assert!(heads[0].starts_with("GET /base/v1/models "), "{heads:?}");
        assert!(!heads[0].to_ascii_lowercase().contains("authorization"));
    }

    #[tokio::test]
    async fn the_cache_serves_within_ttl_and_sees_recovery_after_it() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let endpoint = format!("http://{address}");
        let cache = ReachabilityCache::new(Duration::from_millis(300));

        let down = cache.observe(&endpoint, FAST).await;
        assert!(matches!(down.outcome, ProbeOutcome::Unreachable { .. }));

        let listener = tokio::net::TcpListener::bind(address).await.unwrap();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = [0u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await;
            }
        });
        // Still inside the TTL: the cached failure is served, no new probe.
        assert_eq!(cache.observe(&endpoint, FAST).await, down);

        tokio::time::sleep(Duration::from_millis(350)).await;
        let up = cache.observe(&endpoint, FAST).await;
        task.abort();
        assert_eq!(up.outcome, ProbeOutcome::Reachable { http_status: 200 });
        assert!(up.checked_at > down.checked_at);
    }

    #[tokio::test]
    async fn a_non_http_address_is_invalid_without_any_network() {
        for endpoint in ["", "not a url", "ftp://host/x", "http://"] {
            assert_eq!(
                probe_endpoint(endpoint, FAST).await,
                ProbeOutcome::Unreachable {
                    reason: UnreachableReason::InvalidEndpoint,
                    http_status: None
                },
                "{endpoint:?}"
            );
        }
    }
}
