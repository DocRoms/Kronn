//! Guarded outbound HTTP for destinations a user, a plugin spec or an agent
//! supplies.
//!
//! Every [`SafeClient`] resolves names through [`PinnedResolver`]: the
//! addresses are classified and the connection uses exactly the addresses that
//! passed, so a DNS answer cannot change between the check and the connect.
//! Literal-IP URLs never reach a resolver, so [`SafeRequest::send`] and every
//! redirect hop classify them with the same [`is_global`] rule. Proxies are
//! disabled: through a proxy the resolver would see the proxy, not the target.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use reqwest::header::{HeaderMap, HeaderName};
use reqwest::{Method, StatusCode, Url};

/// Hop cap for followed redirects; reqwest's default is 10.
pub const MAX_REDIRECTS: usize = 5;

/// Which addresses a client may connect to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeHttpPolicy {
    /// Globally routable addresses only.
    Public,
    /// A destination the operator configured on purpose (a local MCP server):
    /// any address, but redirects still never leave the configured origin.
    Configured,
    /// Public, plus loopback so a wiremock can stand in for a public host;
    /// link-local and private targets stay refused.
    #[cfg(test)]
    PublicOrLoopbackForTests,
}

impl SafeHttpPolicy {
    pub fn allows(self, ip: IpAddr) -> bool {
        match self {
            Self::Public => is_global(ip),
            Self::Configured => true,
            #[cfg(test)]
            Self::PublicOrLoopbackForTests => is_global(ip) || canonical_ip(ip).is_loopback(),
        }
    }
}

/// IPv4-mapped (`::ffff:a.b.c.d`), IPv4-compatible (`::a.b.c.d`) and NAT64
/// (`64:ff9b::a.b.c.d`) addresses reach the embedded IPv4 host, so they are
/// classified as that host.
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(v6) = ip else {
        return ip;
    };
    if let Some(v4) = v6.to_ipv4_mapped() {
        return IpAddr::V4(v4);
    }
    let s = v6.segments();
    let embedded = || {
        let [a, b] = s[6].to_be_bytes();
        let [c, d] = s[7].to_be_bytes();
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    };
    // `::` and `::1` are not IPv4-compatible forms.
    if s[..6] == [0; 6] && !(s[6] == 0 && s[7] <= 1) {
        return embedded();
    }
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0; 4] {
        return embedded();
    }
    ip
}

/// The one classifier for "may an outbound request reach this address".
pub fn is_global(ip: IpAddr) -> bool {
    match canonical_ip(ip) {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(o[0] == 0
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_documentation()
                // 100.64.0.0/10 shared address space (carrier NAT, Tailscale).
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                // 192.0.0.0/24 IETF protocol assignments.
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                // 192.88.99.0/24 deprecated 6to4 relay anycast.
                || (o[0] == 192 && o[1] == 88 && o[2] == 99)
                // 198.18.0.0/15 benchmarking.
                || (o[0] == 198 && (o[1] & 0xfe) == 18)
                // 240.0.0.0/4 reserved.
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            if s[0] == 0x2002 {
                // 6to4 reaches the embedded IPv4 host.
                let [a, b] = s[1].to_be_bytes();
                let [c, d] = s[2].to_be_bytes();
                return is_global(IpAddr::V4(Ipv4Addr::new(a, b, c, d)));
            }
            ipv6_is_global(u128::from(v6))
        }
    }
}

/// `(prefix, length)` as a 128-bit value and its mask width.
const fn v6(prefix: u128, len: u32) -> (u128, u32) {
    (prefix, len)
}

fn in_prefix(addr: u128, (prefix, len): (u128, u32)) -> bool {
    let mask = if len == 0 {
        0
    } else {
        u128::MAX << (128 - len)
    };
    addr & mask == prefix & mask
}

/// Non-global entries of the IANA IPv6 Special-Purpose Address Registry
/// that fall inside 2000::/3 (everything outside it is refused anyway).
const IPV6_NON_GLOBAL: [(u128, u32); 5] = [
    v6(0x2001_0000 << 96, 23), // IETF protocol assignments (Teredo, 2001:2::/48 benchmarking…)
    v6(0x2001_0db8 << 96, 32), // documentation
    v6(0x3fff_0000 << 96, 20), // documentation
    v6(0x5f00_0000 << 96, 16), // SRv6 SIDs
    v6(0x2002_0000 << 96, 16), // 6to4, classified by its embedded host instead
];

/// Inside 2001::/23, the registry's globally reachable exceptions.
const IPV6_GLOBAL_IN_IETF: [(u128, u32); 7] = [
    v6(0x2001_0001_0000_0000_0000_0000_0000_0001, 128), // PCP anycast
    v6(0x2001_0001_0000_0000_0000_0000_0000_0002, 128), // TURN anycast
    v6(0x2001_0001_0000_0000_0000_0000_0000_0003, 128), // DNS-SD SRP anycast
    v6(0x2001_0003 << 96, 32),                          // AMT
    v6(0x2001_0004_0112 << 80, 48),                     // AS112-v6
    v6(0x2001_0020 << 96, 28),                          // ORCHIDv2
    v6(0x2001_0030 << 96, 28),                          // drone remote ID
];

/// Global unicast is 2000::/3; outside it every block (loopback, ULA,
/// link-local, multicast, discard-only 100::/64, the dummy prefix, the
/// local-use NAT64 64:ff9b:1::/48, reserved space) is non-global. The
/// well-known NAT64 64:ff9b::/96 and the mapped forms were already
/// reduced to their IPv4 host by [`canonical_ip`].
fn ipv6_is_global(addr: u128) -> bool {
    if !in_prefix(addr, v6(0x2000 << 112, 3)) {
        return false;
    }
    if IPV6_GLOBAL_IN_IETF.iter().any(|p| in_prefix(addr, *p)) {
        return true;
    }
    !IPV6_NON_GLOBAL.iter().any(|p| in_prefix(addr, *p))
}

/// A destination refused by policy. Kept as an error type so it can be found
/// in a reqwest error's source chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked(pub String);

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Blocked {}

/// Scheme and literal-address check, run on every URL before it is sent.
pub fn check_url(url: &Url, policy: SafeHttpPolicy) -> Result<(), Blocked> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Blocked(format!("unsupported scheme `{}`", url.scheme())));
    }
    let Some(host) = url.host_str() else {
        return Err(Blocked("URL has no host".into()));
    };
    // `host_str` keeps the brackets of an IPv6 literal.
    let literal = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .ok();
    if let Some(ip) = literal {
        if !policy.allows(ip) {
            return Err(Blocked(format!(
                "refusing to call a non-public address ({ip})"
            )));
        }
    }
    Ok(())
}

type LookupFn =
    Arc<dyn Fn(String) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>> + Send + Sync>;

fn system_lookup() -> LookupFn {
    Arc::new(|host: String| {
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .map(|socket| socket.ip())
                .collect())
        })
    })
}

/// Resolves, refuses the host if ANY answer is outside the policy (a mixed
/// answer is the rebinding shape), and hands reqwest only checked addresses.
struct PinnedResolver {
    policy: SafeHttpPolicy,
    lookup: LookupFn,
}

impl reqwest::dns::Resolve for PinnedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        let lookup = self.lookup.clone();
        let policy = self.policy;
        Box::pin(async move {
            let ips = lookup(host.clone()).await?;
            if ips.is_empty() {
                return Err(format!("{host} did not resolve").into());
            }
            if let Some(bad) = ips.iter().find(|ip| !policy.allows(**ip)) {
                return Err(Box::new(Blocked(format!(
                    "{host} resolves to a non-public address ({bad})"
                )))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            let addrs: reqwest::dns::Addrs =
                Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redirects {
    /// The caller follows redirects itself ([`send_following`]) or not at all.
    Manual,
    /// reqwest follows redirects that stay on the same origin; any other one
    /// is an error.
    SameOrigin,
}

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub redirects: Redirects,
    pub timeout: Option<Duration>,
    pub connect_timeout: Option<Duration>,
    pub user_agent: Option<String>,
}

impl ClientOptions {
    pub fn new(redirects: Redirects) -> Self {
        Self {
            redirects,
            timeout: None,
            connect_timeout: None,
            user_agent: None,
        }
    }
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self
    }
    pub fn user_agent(mut self, agent: impl Into<String>) -> Self {
        self.user_agent = Some(agent.into());
        self
    }
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host() == b.host()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// A reqwest client that can only send through [`SafeRequest`].
#[derive(Clone)]
pub struct SafeClient {
    inner: reqwest::Client,
    policy: SafeHttpPolicy,
}

pub fn client(policy: SafeHttpPolicy, options: ClientOptions) -> Result<SafeClient, String> {
    build(policy, options, system_lookup())
}

/// A contact's Kronn: its configured address may be on the LAN or Tailscale,
/// so any address is allowed; a redirect is returned, never followed.
pub fn peer_client(timeout: Duration) -> Result<SafeClient, String> {
    client(
        SafeHttpPolicy::Configured,
        ClientOptions::new(Redirects::Manual).timeout(timeout),
    )
}

#[cfg(test)]
pub(crate) fn client_with_lookup(
    policy: SafeHttpPolicy,
    options: ClientOptions,
    lookup: LookupFn,
) -> Result<SafeClient, String> {
    build(policy, options, lookup)
}

fn build(
    policy: SafeHttpPolicy,
    options: ClientOptions,
    lookup: LookupFn,
) -> Result<SafeClient, String> {
    let redirect = match options.redirects {
        Redirects::Manual => reqwest::redirect::Policy::none(),
        Redirects::SameOrigin => reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() > MAX_REDIRECTS {
                return attempt.error(Blocked(format!("more than {MAX_REDIRECTS} redirects")));
            }
            let next = attempt.url().clone();
            let leaves = attempt
                .previous()
                .last()
                .is_some_and(|previous| !same_origin(previous, &next));
            if leaves {
                return attempt.error(Blocked(format!(
                    "redirect to another origin refused ({})",
                    next.host_str().unwrap_or("?")
                )));
            }
            if let Err(blocked) = check_url(&next, policy) {
                return attempt.error(blocked);
            }
            attempt.follow()
        }),
    };
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(Arc::new(PinnedResolver { policy, lookup }))
        .redirect(redirect);
    if let Some(timeout) = options.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(timeout) = options.connect_timeout {
        builder = builder.connect_timeout(timeout);
    }
    if let Some(agent) = options.user_agent {
        builder = builder.user_agent(agent);
    }
    let inner = builder
        .build()
        .map_err(|e| format!("HTTP client build failed: {e}"))?;
    Ok(SafeClient { inner, policy })
}

impl SafeClient {
    pub fn policy(&self) -> SafeHttpPolicy {
        self.policy
    }
    pub fn request(&self, method: Method, url: Url) -> SafeRequest {
        SafeRequest {
            builder: self.inner.request(method, url),
            policy: self.policy,
        }
    }
    pub fn get(&self, url: Url) -> SafeRequest {
        self.request(Method::GET, url)
    }
    pub fn post(&self, url: Url) -> SafeRequest {
        self.request(Method::POST, url)
    }
    pub fn patch(&self, url: Url) -> SafeRequest {
        self.request(Method::PATCH, url)
    }
}

/// Request builder whose only way out is the checked [`SafeRequest::send`].
pub struct SafeRequest {
    builder: reqwest::RequestBuilder,
    policy: SafeHttpPolicy,
}

impl SafeRequest {
    pub fn header(self, key: &str, value: impl AsRef<str>) -> Self {
        self.map(|b| b.header(key, value.as_ref()))
    }
    pub fn headers(self, headers: HeaderMap) -> Self {
        self.map(|b| b.headers(headers))
    }
    pub fn bearer_auth<T: std::fmt::Display>(self, token: T) -> Self {
        self.map(|b| b.bearer_auth(token))
    }
    pub fn json<T: serde::Serialize + ?Sized>(self, json: &T) -> Self {
        self.map(|b| b.json(json))
    }
    pub fn form<T: serde::Serialize + ?Sized>(self, form: &T) -> Self {
        self.map(|b| b.form(form))
    }
    pub fn body<T: Into<reqwest::Body>>(self, body: T) -> Self {
        self.map(|b| b.body(body))
    }
    pub fn timeout(self, timeout: Duration) -> Self {
        self.map(|b| b.timeout(timeout))
    }
    fn map(self, f: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder) -> Self {
        Self {
            builder: f(self.builder),
            policy: self.policy,
        }
    }

    pub async fn send(self) -> Result<reqwest::Response, SendError> {
        let (client, request) = self.builder.build_split();
        let request = request.map_err(SendError::from_reqwest)?;
        check_url(request.url(), self.policy).map_err(|b| SendError::Blocked(b.0))?;
        client
            .execute(request)
            .await
            .map_err(SendError::from_reqwest)
    }
}

/// A refused destination is final; a transport error may be retried.
#[derive(Debug)]
pub enum SendError {
    Blocked(String),
    Transport(reqwest::Error),
}

impl SendError {
    fn from_reqwest(error: reqwest::Error) -> Self {
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
        while let Some(current) = source {
            if let Some(blocked) = current.downcast_ref::<Blocked>() {
                return Self::Blocked(blocked.0.clone());
            }
            source = current.source();
        }
        Self::Transport(error.without_url())
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Blocked(_))
    }
}

/// Never carries the URL: reqwest's own Display would print it, query and all.
impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked(reason) => write!(f, "Security: {reason}"),
            Self::Transport(error) => {
                use std::error::Error as _;
                match error.source() {
                    Some(source) => write!(f, "{error}: {source}"),
                    None => write!(f, "{error}"),
                }
            }
        }
    }
}

impl std::error::Error for SendError {}

/// One request followed through redirects by hand, for clients built with
/// [`Redirects::Manual`] that carry credentials.
pub struct Outbound<'a> {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    /// Headers holding a credential, whatever their name (even `User-Agent`).
    pub secret_headers: &'a [HeaderName],
    /// Query keys holding a credential.
    pub secret_query_keys: &'a [String],
    pub attach_body: &'a (dyn Fn(SafeRequest) -> SafeRequest + Sync),
    /// Whether `attach_body` adds a body: a hop to another origin that would
    /// carry it is refused.
    pub has_body: bool,
    /// Every hop must keep this host and scheme (ApiCall's plugin base).
    pub pinned_base: Option<&'a Url>,
    /// Checks each redirect hop's method and URL before it is sent (an API
    /// access policy); `Err` refuses the hop.
    pub hop_guard: Option<&'a HopGuard<'a>>,
}

/// A per-hop check on the method and URL a redirect would send.
pub type HopGuard<'a> = dyn Fn(&Method, &Url) -> Result<(), String> + Sync + 'a;

/// Headers that may cross to another origin, unless declared secret.
const CROSS_ORIGIN_HEADERS: [HeaderName; 3] = [
    reqwest::header::CONTENT_TYPE,
    reqwest::header::ACCEPT,
    reqwest::header::USER_AGENT,
];

/// Follows up to [`MAX_REDIRECTS`] redirects, re-checking each hop. Refused:
/// an https→http downgrade, a hop off `pinned_base`, a cross-origin 307/308
/// (it would replay the body). A cross-origin hop keeps only non-secret
/// `Content-Type`/`Accept`/`User-Agent` and loses the secret query keys.
/// 301/302 turn a POST into a bodyless GET and 303 does so for anything but
/// HEAD, as reqwest does.
pub async fn send_following(
    client: &SafeClient,
    outbound: Outbound<'_>,
) -> Result<reqwest::Response, SendError> {
    let Outbound {
        mut method,
        mut url,
        mut headers,
        secret_headers,
        secret_query_keys,
        attach_body,
        has_body,
        pinned_base,
        hop_guard,
    } = outbound;
    let refuse = |reason: String| SendError::Blocked(reason);
    let mut with_body = true;
    for hop in 0..=MAX_REDIRECTS {
        let mut request = client
            .request(method.clone(), url.clone())
            .headers(headers.clone());
        if with_body {
            request = attach_body(request);
        }
        let response = request.send().await?;
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
            break;
        }
        let mut next = url
            .join(location)
            .map_err(|e| refuse(format!("invalid redirect target: {e}")))?;
        let cross_origin = validate_hop(&url, &next, status, client.policy, pinned_base)
            .map_err(|reason| refuse(format!("redirect refused: {reason}")))?;
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
        if let Some(guard) = hop_guard {
            guard(&method, &next)
                .map_err(|reason| refuse(format!("redirect refused: {reason}")))?;
        }
        if cross_origin && with_body && has_body {
            return Err(refuse(format!(
                "redirect refused: a {} to another origin would resend the {method} body",
                status.as_u16()
            )));
        }
        if cross_origin {
            let mut kept = HeaderMap::new();
            for name in CROSS_ORIGIN_HEADERS
                .iter()
                .filter(|name| !secret_headers.contains(name))
            {
                for value in headers.get_all(name) {
                    kept.append(name.clone(), value.clone());
                }
            }
            headers = kept;
            strip_query_keys(&mut next, secret_query_keys);
        }
        url = next;
    }
    Err(refuse(format!("more than {MAX_REDIRECTS} redirects")))
}

/// Checks one redirect hop; returns whether it changes origin.
fn validate_hop(
    current: &Url,
    next: &Url,
    status: StatusCode,
    policy: SafeHttpPolicy,
    pinned_base: Option<&Url>,
) -> Result<bool, String> {
    check_url(next, policy).map_err(|b| b.0)?;
    if current.scheme() == "https" && next.scheme() != "https" {
        return Err("https → http downgrade".into());
    }
    if let Some(base) = pinned_base {
        let host = |u: &Url| u.host_str().map(str::to_ascii_lowercase);
        if host(next) != host(base) || next.scheme() != base.scheme() {
            return Err(format!(
                "{} is not the plugin host {}",
                next.host_str().unwrap_or("?"),
                base.host_str().unwrap_or("?")
            ));
        }
    }
    let cross_origin = !same_origin(current, next);
    if cross_origin
        && matches!(
            status,
            StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT
        )
    {
        return Err("a 307/308 to another origin would replay the request body".into());
    }
    Ok(cross_origin)
}

fn strip_query_keys(url: &mut Url, keys: &[String]) {
    if keys.is_empty() || url.query().is_none() {
        return;
    }
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !keys.iter().any(|key| key.eq_ignore_ascii_case(k)))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(kept);
    }
}

#[cfg(test)]
#[path = "safe_http_tests.rs"]
mod tests;
