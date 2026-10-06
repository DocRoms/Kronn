//! Security guards for `StepType::ApiCall`.
//!
//! Three checks sit between a step's declared target and the wire:
//!
//! 1. **Host must match the plugin's `base_url`** — a step cannot reach an
//!    arbitrary origin, only the one the plugin registry declared. Without
//!    this, a templated `{{previous_step.data}}` that contained a URL
//!    could redirect the request anywhere.
//! 2. **Resolved IP must be public** — blocks SSRF against the backend's
//!    own loopback, the Docker network (`169.254.*` metadata endpoint,
//!    `172.17.*`), and RFC1918 private ranges. A compromised plugin
//!    server with a split-horizon DNS can't trick us into scanning the
//!    intranet.
//! 3. **`ResolvedAuth` never logs its secrets** — manual `Debug` impl
//!    redacts bearer tokens and api-key headers. Errors emitted by
//!    reqwest often include the URL (with query string); the redaction
//!    helpers below strip the auth query params before those messages
//!    reach `tracing`.

use reqwest::Url;
use std::collections::HashMap;
use std::net::IpAddr;

/// Tags returned by the guards; let the caller map them to `StepResult`
/// or surface in the test-api-call endpoint response.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SecurityError {
    #[error("Refusing to call {target_host}: plugin base URL is {expected_host}")]
    CrossHost {
        target_host: String,
        expected_host: String,
    },
    #[error("Target URL has no host component")]
    NoHost,
    #[error("Target URL cannot be parsed: {reason}")]
    InvalidUrl { reason: String },
    #[error("Refusing to call {host} — resolves to loopback/private IP {ip}")]
    PrivateOrLoopback { host: String, ip: IpAddr },
    #[error("Refusing to call {host} — DNS resolution failed: {reason}")]
    ResolutionFailed { host: String, reason: String },
}

/// Asserts the target URL shares its host with the plugin's declared
/// `base_url`. Port differences are allowed (a plugin may be behind a
/// reverse proxy); scheme must match (HTTPS stays HTTPS). Subdomains
/// do NOT count as a match — `atlassian.net` != `evil.atlassian.net`.
pub fn assert_host_matches_base(target: &Url, plugin_base_url: &str) -> Result<(), SecurityError> {
    let base = Url::parse(plugin_base_url).map_err(|e| SecurityError::InvalidUrl {
        reason: format!("plugin base_url invalid: {e}"),
    })?;
    let target_host = target
        .host_str()
        .ok_or(SecurityError::NoHost)?
        .to_ascii_lowercase();
    let base_host = base
        .host_str()
        .ok_or(SecurityError::NoHost)?
        .to_ascii_lowercase();
    if target_host != base_host {
        return Err(SecurityError::CrossHost {
            target_host,
            expected_host: base_host,
        });
    }
    if target.scheme() != base.scheme() {
        return Err(SecurityError::CrossHost {
            target_host: format!("{}://{}", target.scheme(), target_host),
            expected_host: format!("{}://{}", base.scheme(), base_host),
        });
    }
    Ok(())
}

/// Resolves the target host via the OS resolver and rejects any
/// loopback/private/link-local address. Run **after**
/// [`assert_host_matches_base`] (which catches the cheap case) and
/// **before** issuing the real request.
///
/// TOCTOU note: a DNS server could return a public IP here and a private
/// IP on reqwest's own resolution a moment later (DNS rebind). In practice
/// this matters for browsers, not for a self-hosted backend where the
/// operator controls the resolver. A harder guard would mean resolving
/// once and feeding the IP to reqwest with a `Host:` header override,
/// which is a much bigger surgery — revisit if the threat model changes.
pub async fn assert_public_ip(target: &Url) -> Result<(), SecurityError> {
    assert_ip_allowed(target, false).await
}

/// `allow_loopback` lets tests stand a wiremock on 127.0.0.1 in for a public
/// host while link-local and private targets stay refused.
async fn assert_ip_allowed(target: &Url, allow_loopback: bool) -> Result<(), SecurityError> {
    let refused = |ip: &IpAddr| is_disallowed_ip(ip) && !(allow_loopback && ip.is_loopback());
    let host = target.host_str().ok_or(SecurityError::NoHost)?.to_string();

    // Fast path: the URL already contains an IP literal (`127.0.0.1`,
    // `[::1]`, `169.254.169.254`, …). No DNS needed, and crucially no
    // dependency on the env having IPv6 connectivity — literal `::1`
    // must block even on IPv4-only hosts. `host_str()` includes the
    // `[…]` brackets for IPv6 literals, hence the trim.
    let host_stripped = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host_stripped.parse::<IpAddr>() {
        if refused(&ip) {
            return Err(SecurityError::PrivateOrLoopback { host, ip });
        }
        return Ok(());
    }

    // Hostname: resolve through the OS resolver and check every address.
    let port = target.port_or_known_default().unwrap_or(443);
    let addr_key = (host.clone(), port);
    let lookup = match tokio::net::lookup_host(addr_key).await {
        Ok(iter) => iter,
        Err(e) => {
            return Err(SecurityError::ResolutionFailed {
                host,
                reason: e.to_string(),
            })
        }
    };
    for sock in lookup {
        let ip = sock.ip();
        if refused(&ip) {
            return Err(SecurityError::PrivateOrLoopback { host, ip });
        }
    }
    Ok(())
}

/// Hop cap for followed redirects; reqwest's default is 10.
pub const MAX_REDIRECTS: usize = 5;

/// Validation applied to every redirect hop, so a 3xx cannot steer a request
/// past the checks the initial URL went through.
#[derive(Debug, Clone, Copy)]
pub struct RedirectGuard<'a> {
    /// `Some` when the target must stay on the plugin's host (ApiCall).
    pub base_url: Option<&'a str>,
    pub enforce_public_ip: bool,
}

impl RedirectGuard<'_> {
    pub async fn check(&self, target: &Url) -> Result<(), SecurityError> {
        if !matches!(target.scheme(), "http" | "https") {
            return Err(SecurityError::InvalidUrl {
                reason: format!("redirect to unsupported scheme `{}`", target.scheme()),
            });
        }
        if let Some(base) = self.base_url {
            assert_host_matches_base(target, base)?;
        }
        // Test mode keeps loopback reachable but still refuses metadata and
        // private ranges on a hop.
        assert_ip_allowed(target, !self.enforce_public_ip).await
    }
}

/// Failure of [`send_with_guarded_redirects`]: transport errors stay
/// retryable, a refused hop does not.
#[derive(Debug)]
pub enum GuardedSendError {
    Transport(reqwest::Error),
    Refused(String),
}

/// Headers that may cross to another origin; everything else may carry a
/// credential (custom auth headers have arbitrary names).
const CROSS_ORIGIN_HEADERS: [reqwest::header::HeaderName; 3] = [
    reqwest::header::CONTENT_TYPE,
    reqwest::header::ACCEPT,
    reqwest::header::USER_AGENT,
];

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host() == b.host()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// Sends `method url` on a client built with `redirect::Policy::none()` and
/// follows redirects itself, validating each hop with `guard`. 301/302 turn a
/// POST into a bodyless GET and 303 turns anything but HEAD into one, as
/// reqwest does; 307/308 keep method and body.
pub async fn send_with_guarded_redirects(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: Url,
    headers: reqwest::header::HeaderMap,
    attach_body: &(dyn Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder + Sync),
    guard: &RedirectGuard<'_>,
) -> Result<reqwest::Response, GuardedSendError> {
    use reqwest::{Method, StatusCode};
    let mut method = method;
    let mut url = url;
    let mut headers = headers;
    let mut with_body = true;
    for hop in 0..=MAX_REDIRECTS {
        let mut req = client
            .request(method.clone(), url.clone())
            .headers(headers.clone());
        if with_body {
            req = attach_body(req);
        }
        let response = req.send().await.map_err(GuardedSendError::Transport)?;
        let status = response.status();
        if !status.is_redirection() {
            return Ok(response);
        }
        let Some(location) = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
        else {
            return Ok(response);
        };
        if hop == MAX_REDIRECTS {
            return Err(GuardedSendError::Refused(format!(
                "Security: more than {MAX_REDIRECTS} redirects"
            )));
        }
        let next = url.join(location).map_err(|e| {
            GuardedSendError::Refused(format!("Security: invalid redirect target: {e}"))
        })?;
        guard
            .check(&next)
            .await
            .map_err(|e| GuardedSendError::Refused(format!("Security: redirect refused: {e}")))?;
        match status {
            StatusCode::SEE_OTHER if method != Method::HEAD => {
                method = Method::GET;
                with_body = false;
            }
            StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND if method == Method::POST => {
                method = Method::GET;
                with_body = false;
            }
            _ => {}
        }
        if !same_origin(&url, &next) {
            let mut kept = reqwest::header::HeaderMap::new();
            for name in &CROSS_ORIGIN_HEADERS {
                for value in headers.get_all(name) {
                    kept.append(name.clone(), value.clone());
                }
            }
            headers = kept;
        }
        url = next;
    }
    Err(GuardedSendError::Refused(format!(
        "Security: more than {MAX_REDIRECTS} redirects"
    )))
}

/// Rejects the RFC 1918, loopback, link-local, multicast and
/// unspecified ranges. Leaves only globally-routable addresses through.
fn is_disallowed_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()     // 169.254.* — Docker / AWS metadata
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_documentation() // 192.0.2.*, 198.51.100.*, 203.0.113.*
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                // Unique-local (fc00::/7) + link-local (fe80::/10). The
                // stdlib does not expose these helpers on stable, so we
                // check the prefix bytes.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Resolved auth material ready to be attached to a reqwest::RequestBuilder.
/// The `Debug` impl is hand-rolled to redact every field — never derive it.
///
/// Typical flow: `resolve_auth(&plugin.api_spec.auth, &env) -> ResolvedAuth`,
/// then `req.headers(auth.headers()).query(&auth.query_params())`.
pub struct ResolvedAuth {
    /// `Authorization: Bearer <token>` or `Authorization: Basic <base64>`.
    pub bearer: Option<String>,
    /// Headers to attach (e.g. `X-Api-Key: …`, custom OAuth2 extras).
    pub headers: HashMap<String, String>,
    /// Query params to merge (e.g. `apikey=…` for Chartbeat / Google Search).
    pub query: HashMap<String, String>,
}

impl std::fmt::Debug for ResolvedAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bearer_view = self
            .bearer
            .as_deref()
            .map(mask_secret)
            .unwrap_or_else(|| "None".into());
        let header_view: HashMap<_, _> = self
            .headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str(),
                    if looks_like_secret_key(k) {
                        mask_secret(v)
                    } else {
                        v.clone()
                    },
                )
            })
            .collect();
        let query_view: HashMap<_, _> = self
            .query
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str(),
                    if looks_like_secret_key(k) {
                        mask_secret(v)
                    } else {
                        v.clone()
                    },
                )
            })
            .collect();
        f.debug_struct("ResolvedAuth")
            .field("bearer", &bearer_view)
            .field("headers", &header_view)
            .field("query", &query_view)
            .finish()
    }
}

/// Heuristic: a header / query key that's likely a secret container.
/// Used ONLY for redaction decisions — false positives are fine (we
/// redact more than strictly needed), false negatives leak tokens.
pub fn looks_like_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("key")
        || k.contains("token")
        || k.contains("secret")
        || k.contains("auth")
        || k == "apikey"
        || k == "api_key"
        || k == "x-api-key"
        || k == "x-cb-ak"          // Chartbeat
        || k == "x-auth-email"     // Cloudflare legacy
        || k == "x-auth-key" // Cloudflare legacy
}

/// Masks a secret to `"***({len} chars)"` — length enough to spot a
/// truncation bug, zero content disclosure. Empty strings surface as
/// `"<empty>"` so we notice when auth resolution silently failed.
pub fn mask_secret(raw: &str) -> String {
    if raw.is_empty() {
        return "<empty>".into();
    }
    format!("***({} chars)", raw.chars().count())
}

/// Returns a copy of `url` with every query param whose key matches
/// [`looks_like_secret_key`] replaced by `"***"`. Call this before
/// logging a URL or echoing it back to the frontend. Preserves the
/// rest of the URL verbatim so the caller can still see what endpoint
/// was hit.
pub fn redact_url_query(url: &Url) -> String {
    let mut clone = url.clone();
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let key = k.into_owned();
            let value = if looks_like_secret_key(&key) {
                "***".into()
            } else {
                v.into_owned()
            };
            (key, value)
        })
        .collect();
    clone.query_pairs_mut().clear();
    for (k, v) in &pairs {
        clone.query_pairs_mut().append_pair(k, v);
    }
    // If no query, `query_pairs_mut().clear()` leaves a trailing `?` —
    // strip it for cleanliness.
    let s = clone.to_string();
    s.trim_end_matches('?').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── assert_host_matches_base ───────────────────────────────────

    #[test]
    fn host_match_exact_passes() {
        let target = Url::parse("https://api.jira.com/rest/api/3/search").unwrap();
        assert!(assert_host_matches_base(&target, "https://api.jira.com").is_ok());
    }

    #[test]
    fn host_match_different_port_passes() {
        // Same host, different port = still the same plugin (reverse proxies).
        let target = Url::parse("https://api.jira.com:8443/v1").unwrap();
        assert!(assert_host_matches_base(&target, "https://api.jira.com:443").is_ok());
    }

    #[test]
    fn host_match_subdomain_fails() {
        // Regression guard: `evil.atlassian.net` must NOT slip through a
        // plugin declaring `atlassian.net` as its base.
        let target = Url::parse("https://evil.atlassian.net/").unwrap();
        let err = assert_host_matches_base(&target, "https://atlassian.net").unwrap_err();
        assert!(matches!(err, SecurityError::CrossHost { .. }));
    }

    #[test]
    fn host_match_different_host_fails() {
        let target = Url::parse("https://attacker.com/api").unwrap();
        let err = assert_host_matches_base(&target, "https://api.jira.com").unwrap_err();
        match err {
            SecurityError::CrossHost {
                target_host,
                expected_host,
            } => {
                assert_eq!(target_host, "attacker.com");
                assert_eq!(expected_host, "api.jira.com");
            }
            other => panic!("expected CrossHost, got {other:?}"),
        }
    }

    #[test]
    fn host_match_scheme_downgrade_fails() {
        // https → http must fail; otherwise an attacker redirects to
        // plaintext and sniffs the bearer in transit.
        let target = Url::parse("http://api.jira.com/").unwrap();
        let err = assert_host_matches_base(&target, "https://api.jira.com").unwrap_err();
        assert!(matches!(err, SecurityError::CrossHost { .. }));
    }

    #[test]
    fn host_match_case_insensitive() {
        let target = Url::parse("https://API.Jira.com/").unwrap();
        assert!(assert_host_matches_base(&target, "https://api.jira.com").is_ok());
    }

    // ─── RedirectGuard ──────────────────────────────────────────────

    #[tokio::test]
    async fn production_guard_refuses_loopback_metadata_and_other_schemes() {
        let guard = RedirectGuard {
            base_url: None,
            enforce_public_ip: true,
        };
        for target in [
            "http://127.0.0.1:3140/api/config",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "file:///etc/passwd",
        ] {
            let url = Url::parse(target).unwrap();
            assert!(guard.check(&url).await.is_err(), "{target} must be refused");
        }
    }

    #[tokio::test]
    async fn test_guard_allows_loopback_only() {
        let guard = RedirectGuard {
            base_url: None,
            enforce_public_ip: false,
        };
        let loopback = Url::parse("http://127.0.0.1:9/").unwrap();
        assert!(guard.check(&loopback).await.is_ok());
        let metadata = Url::parse("http://169.254.169.254/").unwrap();
        assert!(guard.check(&metadata).await.is_err());
        let private = Url::parse("http://10.0.0.1/").unwrap();
        assert!(guard.check(&private).await.is_err());
    }

    #[tokio::test]
    async fn guard_with_base_refuses_another_host() {
        let guard = RedirectGuard {
            base_url: Some("https://api.example.com"),
            enforce_public_ip: false,
        };
        let other = Url::parse("https://127.0.0.1/").unwrap();
        assert!(matches!(
            guard.check(&other).await,
            Err(SecurityError::CrossHost { .. })
        ));
    }

    // ─── assert_public_ip ───────────────────────────────────────────

    #[tokio::test]
    async fn assert_public_ip_rejects_localhost() {
        let target = Url::parse("https://localhost/").unwrap();
        let err = assert_public_ip(&target).await.unwrap_err();
        assert!(matches!(err, SecurityError::PrivateOrLoopback { .. }));
    }

    #[tokio::test]
    async fn assert_public_ip_rejects_loopback_literal_v4() {
        let target = Url::parse("https://127.0.0.1/").unwrap();
        let err = assert_public_ip(&target).await.unwrap_err();
        assert!(matches!(err, SecurityError::PrivateOrLoopback { .. }));
    }

    #[tokio::test]
    async fn assert_public_ip_rejects_loopback_literal_v6() {
        let target = Url::parse("https://[::1]/").unwrap();
        let err = assert_public_ip(&target).await.unwrap_err();
        assert!(matches!(err, SecurityError::PrivateOrLoopback { .. }));
    }

    #[tokio::test]
    async fn assert_public_ip_rejects_rfc1918() {
        for host in ["10.0.0.1", "172.17.0.1", "192.168.1.1"] {
            let target = Url::parse(&format!("https://{host}/")).unwrap();
            let err = assert_public_ip(&target).await.unwrap_err();
            assert!(
                matches!(err, SecurityError::PrivateOrLoopback { .. }),
                "host {host} should be blocked, got {err:?}",
            );
        }
    }

    #[tokio::test]
    async fn assert_public_ip_rejects_aws_metadata_link_local() {
        // The canonical SSRF target. Blocking 169.254.* stops it.
        let target = Url::parse("https://169.254.169.254/latest/meta-data/").unwrap();
        let err = assert_public_ip(&target).await.unwrap_err();
        assert!(matches!(err, SecurityError::PrivateOrLoopback { .. }));
    }

    #[tokio::test]
    async fn assert_public_ip_rejects_fc00_ula() {
        let target = Url::parse("https://[fc00::1]/").unwrap();
        let err = assert_public_ip(&target).await.unwrap_err();
        assert!(matches!(err, SecurityError::PrivateOrLoopback { .. }));
    }

    // ─── is_disallowed_ip (pure) ────────────────────────────────────

    #[test]
    fn is_disallowed_ip_covers_v4_ranges() {
        for blocked in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "0.0.0.0",
        ] {
            let ip: IpAddr = blocked.parse().unwrap();
            assert!(is_disallowed_ip(&ip), "expected {blocked} to be disallowed");
        }
    }

    #[test]
    fn is_disallowed_ip_allows_public_v4() {
        for ok in ["1.1.1.1", "8.8.8.8", "140.82.112.4"] {
            let ip: IpAddr = ok.parse().unwrap();
            assert!(!is_disallowed_ip(&ip), "expected {ok} to be allowed");
        }
    }

    // ─── ResolvedAuth Debug redaction ───────────────────────────────

    #[test]
    fn resolved_auth_debug_masks_bearer() {
        let auth = ResolvedAuth {
            bearer: Some("sk-live-verysecret".into()),
            headers: HashMap::new(),
            query: HashMap::new(),
        };
        let dbg = format!("{auth:?}");
        assert!(dbg.contains("***"), "expected *** in {dbg}");
        assert!(!dbg.contains("verysecret"), "bearer leaked in {dbg}");
    }

    #[test]
    fn resolved_auth_debug_masks_apikey_in_query_and_headers() {
        let mut headers = HashMap::new();
        headers.insert("X-Cb-Ak".into(), "chartbeat-secret-key".into());
        headers.insert("User-Agent".into(), "Kronn/0.5.2".into());
        let mut query = HashMap::new();
        query.insert("apikey".into(), "google-secret-api-key".into());
        query.insert("q".into(), "cats".into());
        let auth = ResolvedAuth {
            bearer: None,
            headers,
            query,
        };
        let dbg = format!("{auth:?}");
        assert!(
            !dbg.contains("chartbeat-secret-key"),
            "header secret leaked: {dbg}"
        );
        assert!(
            !dbg.contains("google-secret-api-key"),
            "query secret leaked: {dbg}"
        );
        // Non-secret fields must survive intact to keep debug useful.
        assert!(dbg.contains("Kronn/0.5.2"));
        assert!(dbg.contains("cats"));
    }

    #[test]
    fn mask_secret_counts_unicode_chars_not_bytes() {
        // Regression: `.chars().count()` prevents misleading "32 chars"
        // on a 16-character-but-32-bytes emoji token.
        let raw = "token🔒🔒🔒";
        let masked = mask_secret(raw);
        assert!(masked.contains("(8 chars)"), "got {masked}");
    }

    #[test]
    fn mask_secret_handles_empty_string() {
        // An empty secret usually means resolution silently failed —
        // flag it visibly so we don't send an auth-less request.
        assert_eq!(mask_secret(""), "<empty>");
    }

    // ─── looks_like_secret_key heuristic ────────────────────────────

    #[test]
    fn looks_like_secret_key_catches_common_names() {
        for key in [
            "Authorization",
            "X-Api-Key",
            "api_key",
            "APIKEY",
            "access_token",
            "client_secret",
        ] {
            assert!(
                looks_like_secret_key(key),
                "{key} should look like a secret"
            );
        }
    }

    #[test]
    fn looks_like_secret_key_leaves_benign_keys_alone() {
        for key in ["User-Agent", "Content-Type", "Accept", "q", "limit"] {
            assert!(
                !looks_like_secret_key(key),
                "{key} should be considered benign"
            );
        }
    }

    // ─── redact_url_query ────────────────────────────────────────────

    #[test]
    fn redact_url_query_masks_apikey_preserves_rest() {
        let url = Url::parse(
            "https://api.chartbeat.com/live/toppages/v4?apikey=supersecret&host=euronews.com&limit=5",
        ).unwrap();
        let redacted = redact_url_query(&url);
        assert!(
            !redacted.contains("supersecret"),
            "apikey leaked: {redacted}"
        );
        assert!(redacted.contains("apikey=***"));
        assert!(redacted.contains("host=euronews.com"));
        assert!(redacted.contains("limit=5"));
    }

    #[test]
    fn redact_url_query_noop_when_no_query() {
        let url = Url::parse("https://api.cloudflare.com/client/v4/zones").unwrap();
        let redacted = redact_url_query(&url);
        assert_eq!(redacted, "https://api.cloudflare.com/client/v4/zones");
    }
}
