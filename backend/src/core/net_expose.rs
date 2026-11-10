//! Network-exposure helpers — is this instance bound to a network-reachable
//! address (`0.0.0.0`/`::`, i.e. exposed to LAN/Tailscale peers) or to loopback
//! only? Used by the Settings "Allow connections from other devices" toggle and
//! the contacts/P2P feature.
//!
//! The toggle writes `config.server.host`, but the host is only *bound* at
//! startup — so changing it needs a restart. We record the host the process
//! actually bound at boot, so the API can tell the UI whether a restart is
//! still pending.

use std::sync::OnceLock;

/// Host the server actually bound at process startup. Set once by each binary
/// (backend `main.rs`, desktop `main.rs`) right after it resolves the bind host.
static BOUND_HOST: OnceLock<String> = OnceLock::new();

/// True when `host` is a bind-all address — i.e. the instance is reachable from
/// other machines (LAN / Tailscale), not just localhost.
pub fn is_exposed_host(host: &str) -> bool {
    host == "0.0.0.0" || host == "::"
}

/// Record the host the server bound at boot. Idempotent (first write wins).
pub fn record_bound_host(host: &str) {
    let _ = BOUND_HOST.set(host.to_string());
}

/// Whether the *running* process is exposed, if known (None before boot records
/// it — e.g. in unit tests that never start a server).
pub fn boot_exposed() -> Option<bool> {
    BOUND_HOST.get().map(|h| is_exposed_host(h))
}

/// True when the host the next boot would bind (`boot_bind(..).bind_host`, not
/// the raw configured host: a declared proxy or a plain-HTTP fallback binds
/// loopback whatever the config asks) differs from what the process actually
/// bound → a restart is required for the change to take effect.
pub fn restart_required(planned_bind_host: &str) -> bool {
    BOUND_HOST
        .get()
        .is_some_and(|booted| restart_due(booted, planned_bind_host))
}

/// [`restart_required`] without the process-global: does binding
/// `planned_bind_host` change the exposure `booted_host` has? Pure + tested.
pub fn restart_due(booted_host: &str, planned_bind_host: &str) -> bool {
    is_exposed_host(booted_host) != is_exposed_host(planned_bind_host)
}

/// The address the backend listens on: an explicit `KRONN_HOST` wins, a real
/// container binds every interface so nginx can reach it, and anything else —
/// a native install included, even one that relocated its data dir — keeps the
/// configured host (loopback by default).
pub fn resolve_bind_host(kronn_host: Option<&str>, is_docker: bool, config_host: &str) -> String {
    match kronn_host.map(str::trim) {
        Some(host) if !host.is_empty() => host.to_string(),
        _ if is_docker => "0.0.0.0".to_string(),
        _ => config_host.to_string(),
    }
}

/// True when `host` binds ONLY loopback — safe, unreachable from other machines.
/// Stricter than `!is_exposed_host`: a specific LAN IP (`192.168.x`) is NOT
/// loopback (it IS reachable), whereas the `is_exposed_host` toggle semantics
/// only flag the bind-all forms. Used by the boot security guard.
///
/// The address is parsed, not pattern-matched: brackets (`[::1]`), a trailing
/// dot (`localhost.`), the long IPv6 spelling and an IPv4-mapped address
/// (`::ffff:127.0.0.1`) are all loopback, while a name that merely starts with
/// `127.` (`127.evil.example`) is not.
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.');
    let h = h
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(h);
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.is_loopback(),
        Ok(std::net::IpAddr::V6(v6)) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        Err(_) => false,
    }
}

/// Does the operator intend LAN exposure? In Docker the backend ALWAYS binds
/// `0.0.0.0` (nginx must reach it inside the network), so the real LAN-exposure
/// lever is the *host port publish* — signalled to us by `KRONN_BIND`. Natively,
/// the resolved bind host is authoritative. Pure + tested.
pub fn lan_exposed(is_docker: bool, kronn_bind: Option<&str>, native_host: &str) -> bool {
    if is_docker {
        kronn_bind.map(|b| !is_loopback_host(b)).unwrap_or(false)
    } else {
        !is_loopback_host(native_host)
    }
}

/// Boot security guard: `Some(message)` when the instance is LAN-exposed but the
/// API is unauthenticated (no `auth_enabled`, no token) and the operator did not
/// acknowledge the risk. Secure-by-default: a fresh `docker compose up` binds
/// loopback (→ not exposed → `None`); exposing to the LAN requires either auth
/// or an explicit `KRONN_ALLOW_INSECURE_LAN` acknowledgment. Pure + tested.
pub fn insecure_lan_boot_error(
    lan_exposed: bool,
    auth_enabled: bool,
    token_configured: bool,
    ack_insecure: bool,
) -> Option<String> {
    // Actually-enforced auth = auth_enabled AND a token. Either alone is
    // open in the middleware (`auth_allows`: auth off → open; no token →
    // open) — the Docker default (auth OFF + auto-generated token) used to
    // pass this guard on `token_configured` while every endpoint stayed
    // wide open to the LAN.
    let auth_enforced = auth_enabled && token_configured;
    if ack_insecure || auth_enforced || !lan_exposed {
        return None;
    }
    Some(
        "REFUSING TO START: Kronn is exposed to the network (LAN/Tailscale) but the API \
         is unauthenticated — any peer on your network could trigger workflows (incl. Exec \
         steps) with your credentials.\n  Fix one of:\n    • set KRONN_AUTH_TOKEN=<secret> \
         (recommended — the frontend and MCP sidecar pick it up automatically), or\n    • \
         enable auth in Settings, or\n    • set KRONN_ALLOW_INSECURE_LAN=1 to keep the API \
         open on a trusted home LAN, at your own risk."
            .to_string(),
    )
}

/// Env var an operator sets to declare that the plain-HTTP listener is NOT the
/// transport peers see: a TLS-terminating reverse proxy on this machine carries
/// their traffic (nginx, Caddy, `tailscale serve`). The same declaration lives in
/// `server.tls_terminated_by_proxy`, settable from Settings.
pub const TLS_TERMINATED_ENV: &str = "KRONN_TLS_TERMINATED_BY_PROXY";

/// Env var carrying the proxy's public `https://` origin, for the same
/// declaration made without Settings (Docker).
pub const PUBLIC_ORIGIN_ENV: &str = "KRONN_PUBLIC_ORIGIN";

/// Parse the boolean spelling shared by the exposure env flags.
pub fn flag_enabled(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "yes"))
}

/// The proxy declaration in force: the config flag or the env flag. Pure.
pub fn proxy_declared(config_flag: bool, env_value: Option<&str>) -> bool {
    config_flag || flag_enabled(env_value)
}

/// [`proxy_declared`] for a live config, reading the process environment.
pub fn proxy_declared_for(server: &crate::models::ServerConfig) -> bool {
    proxy_declared(
        server.tls_terminated_by_proxy,
        crate::core::child_env::var(TLS_TERMINATED_ENV)
            .ok()
            .as_deref(),
    )
}

/// The proxy's public origin: the config value, else the env one. Pure.
pub fn public_origin(config_value: Option<&str>, env_value: Option<&str>) -> Option<String> {
    [config_value, env_value]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|origin| !origin.is_empty())
        .map(str::to_owned)
}

/// [`public_origin`] for a live config, reading the process environment.
pub fn public_origin_for(server: &crate::models::ServerConfig) -> Option<String> {
    public_origin(
        server.public_origin.as_deref(),
        crate::core::child_env::var(PUBLIC_ORIGIN_ENV)
            .ok()
            .as_deref(),
    )
}

/// The one browser origin a declared proxy serves Kronn's UI from, canonical
/// (`https://host[:port]`), or None without a declaration or an origin. CORS and
/// the WebSocket Origin check allow exactly this one, so a remote user behind
/// the proxy is not refused as a foreign page while any other site still is.
pub fn declared_origin(server: &crate::models::ServerConfig) -> Option<String> {
    if !proxy_declared_for(server) {
        return None;
    }
    crate::db::contacts::normalize_public_origin(&public_origin_for(server)?)
}

/// Whether a request that arrives from a loopback peer may be trusted as the
/// owner: never when a proxy on this machine forwards the network's traffic
/// (every proxied request then comes from 127.0.0.1). Docker is not exempt: a
/// proxy on the host reaches nginx from the bridge gateway (172.x.0.1), which
/// counts as local too, and nginx's `X-Real-IP` then names that gateway, not
/// the remote user. Pure + tested.
pub fn loopback_peer_is_owner(proxy_declared: bool) -> bool {
    !proxy_declared
}

/// Shortest on-machine proof ever minted or accepted.
const LOCAL_PROOF_MIN_LEN: usize = 32;

/// Header carrying the on-machine proof (see [`local_proof`]).
pub const LOCAL_PROOF_HEADER: &str = "x-kronn-local-proof";

/// File, in the data directory, holding the on-machine proof.
pub const LOCAL_PROOF_FILE: &str = "local-proof";

/// The secret that tells the owner at this machine from a stranger coming
/// through a declared proxy (KT-1596). Behind a proxy every request arrives from
/// loopback, so the peer address proves nothing; this file can only be read by
/// whoever can read the data directory (owner-only, like the keys beside it).
/// It is what lets the recovery routes work while the token cannot be decrypted
/// and no token can authenticate anyone. Minted on first use.
pub fn local_proof(dir: &std::path::Path) -> std::io::Result<String> {
    let path = dir.join(LOCAL_PROOF_FILE);
    if let Some(existing) = read_trusted_proof(&path) {
        return Ok(existing);
    }
    let proof = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let tmp = dir.join(format!(".{LOCAL_PROOF_FILE}.tmp"));
    crate::core::keyvault::write_private_atomic(&tmp, &path, proof.as_bytes())?;
    Ok(proof)
}

/// The proof stored at `path`, only when it can be trusted as ours: a regular
/// file (a symlink would let a planted link to a known file pass as the proof,
/// and a dangling or foreign target must not be followed), owner-only on Unix
/// (a group/world-accessible file was writable or readable by others), and at
/// least [`LOCAL_PROOF_MIN_LEN`] once trimmed. Anything else is None, so the
/// mint replaces it and a match is refused.
fn read_trusted_proof(path: &std::path::Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    let content = std::fs::read_to_string(path).ok()?;
    let content = content.trim();
    (content.len() >= LOCAL_PROOF_MIN_LEN).then(|| content.to_string())
}

/// Put the on-machine proof in the data directory when a proxy is declared, so
/// the owner can read it before a recovery needs it. Best-effort: a failure is
/// logged, never fatal to the boot.
pub fn arm_local_proof(server: &crate::models::ServerConfig) {
    if !proxy_declared_for(server) {
        return;
    }
    let armed = crate::core::config::config_dir()
        .map_err(|e| e.to_string())
        .and_then(|dir| local_proof(&dir).map_err(|e| e.to_string()));
    if let Err(e) = armed {
        tracing::warn!("Could not prepare the on-machine recovery proof: {e}");
    }
}

/// Whether `presented` is the on-machine proof of `dir`. A missing, empty or
/// shorter-than-minted (hand-written, truncated) proof file never matches, nor
/// does a symlink or a file others can access, and the comparison does not stop at the first differing byte. The file is only
/// created by [`local_proof`], never here, so a request cannot make it appear.
pub fn local_proof_matches(dir: &std::path::Path, presented: Option<&str>) -> bool {
    let Some(presented) = presented.map(str::trim).filter(|p| !p.is_empty()) else {
        return false;
    };
    let Some(expected) = read_trusted_proof(&dir.join(LOCAL_PROOF_FILE)) else {
        return false;
    };
    crate::core::bridge_token::operator_token_matches(Some(&expected), presented)
}

/// Who the recovery routes treat as the local owner while auth is locked.
/// Without a proxy: a loopback peer. Behind a declared proxy the peer address
/// is the proxy, so only a loopback peer that also presents the on-machine
/// proof counts. Pure + tested.
pub fn recovery_caller_is_local(
    proxy_fronted: bool,
    peer_is_local: bool,
    proof_matches: bool,
) -> bool {
    peer_is_local && (!proxy_fronted || proof_matches)
}

/// Whether plain HTTP is what a network peer would get: exposure is requested
/// (the bind the next boot is asked for is not loopback, `KRONN_HOST` included)
/// and nothing declares TLS in front. Natively that boot falls back to loopback
/// and nobody is served, but the owner must be told, and no invite may
/// advertise an address that would carry the token in clear. Docker refuses to
/// start instead. Pure + tested.
pub fn plain_http_blocked(is_docker: bool, requested_host: &str, proxy_declared: bool) -> bool {
    !is_docker && !proxy_declared && !is_loopback_host(requested_host)
}

/// [`plain_http_blocked`] for a live config and the process environment.
pub fn plain_http_blocked_for(server: &crate::models::ServerConfig) -> bool {
    let is_docker = crate::core::env::is_docker();
    let requested = resolve_bind_host(
        crate::core::child_env::var("KRONN_HOST").ok().as_deref(),
        is_docker,
        &server.host,
    );
    plain_http_blocked(is_docker, &requested, proxy_declared_for(server))
}

/// What the Settings toggle must say when asked to expose Kronn (KT-1596):
/// `Some(message)` when neither in-process TLS nor a declared proxy would carry
/// the exposure, so the Bearer token and P2P invite codes would cross the
/// network in clear. Refusing here, rather than at the next boot, is what keeps
/// the owner from writing a config that cannot start. Pure + tested.
pub fn plain_http_exposure_refusal(tls_enabled: bool, proxy_declared: bool) -> Option<String> {
    if tls_enabled || proxy_declared {
        return None;
    }
    Some(
        "Kronn cannot be exposed over plain HTTP: the access token and P2P invite codes \
         would cross your network in clear. Put a TLS-terminating reverse proxy on this \
         machine in front of Kronn (nginx, Caddy, or `tailscale serve`), then declare it \
         here with its public https:// address. TLS inside Kronn is not available yet."
            .to_string(),
    )
}

/// Boot transport guard for a runtime whose listening interface is not ours to
/// change — Docker, where the host port publish is outside the process.
/// `Some(message)` when the instance is LAN-exposed and would serve the token
/// and P2P invites over plain HTTP, with neither in-process TLS nor the proxy
/// declaration. Independent of the auth guard above:
/// `KRONN_ALLOW_INSECURE_LAN` (an unauthenticated API) does not waive it.
/// Loopback-only use is never refused. Pure + tested.
pub fn plain_http_lan_boot_error(
    lan_exposed: bool,
    tls_enabled: bool,
    tls_terminated_by_proxy: bool,
) -> Option<String> {
    if !lan_exposed || tls_enabled || tls_terminated_by_proxy {
        return None;
    }
    Some(format!(
        "REFUSING TO START: Kronn is published on the network over plain HTTP — the \
         Bearer token and P2P invite codes would travel in clear.\n  Fix one of:\n    \
         • put a TLS-terminating reverse proxy in front, set {TLS_TERMINATED_ENV}=1 and \
         {PUBLIC_ORIGIN_ENV}=https://<its address>, or\n    • publish on loopback only \
         (KRONN_BIND=127.0.0.1)."
    ))
}

/// Docker with a declared proxy AND the plain port published beyond loopback:
/// the proxy is declared as the only way in, yet the backend's own port still
/// answers in clear on the network, token and P2P invites included. The declared
/// proxy lives on this host and reaches Kronn through loopback, so there is no
/// reason to publish wider. `Some(message)` to refuse to start. Pure + tested.
pub fn proxy_with_plain_publish_error(lan_exposed: bool, proxy_declared: bool) -> Option<String> {
    if !(lan_exposed && proxy_declared) {
        return None;
    }
    Some(format!(
        "REFUSING TO START: {TLS_TERMINATED_ENV} declares a TLS proxy as the way in, but \
         Kronn's own port is published beyond loopback, so the token and P2P invite codes \
         are still served in clear on the network.\n  Publish on loopback only \
         (KRONN_BIND=127.0.0.1) and let the proxy on this host reach it there."
    ))
}

/// Why the backend binds the interface it binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindReason {
    /// Loopback, or an exposure that something already encrypts.
    AsRequested,
    /// A declared proxy on this machine is the only thing that faces the
    /// network: the backend itself, P2P socket included, stays on loopback.
    ProxyOnLoopback,
    /// Exposure was requested (stale config, env) with no TLS and no declared
    /// proxy: the backend starts on loopback instead of refusing to start, so
    /// the owner is never locked out of the UI that could fix it.
    PlainHttpFellBackToLoopback,
}

/// Where the process binds, and what the boot guards must know about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootBind {
    /// The address to bind.
    pub bind_host: String,
    pub reason: BindReason,
    /// The auth guard's input: the network can reach this instance, directly or
    /// through a declared proxy. False after a plain-HTTP fallback to loopback.
    pub reachable_from_network: bool,
    /// A refusal to start, for the one runtime that cannot fall back (Docker).
    pub fatal: Option<String>,
}

/// Resolve the bind of this boot (KT-1596). `requested_host` is what the owner
/// asked for (`resolve_bind_host`). A native or embedded runtime never starts
/// reachable in clear: plain HTTP falls back to loopback, a declared proxy gets
/// loopback too (it is on this machine, and the plain socket is not for the
/// network), and in both cases the owner reaches the UI that can fix the
/// config. Docker binds `0.0.0.0` inside its container whatever happens, so
/// only the publish decides and the guard can only refuse. Pure + tested.
pub fn boot_bind(
    requested_host: &str,
    is_docker: bool,
    kronn_bind: Option<&str>,
    tls_enabled: bool,
    proxy_declared: bool,
) -> BootBind {
    let requested_exposed = lan_exposed(is_docker, kronn_bind, requested_host);
    if is_docker {
        return BootBind {
            bind_host: requested_host.to_string(),
            reason: BindReason::AsRequested,
            reachable_from_network: requested_exposed || proxy_declared,
            fatal: plain_http_lan_boot_error(requested_exposed, tls_enabled, proxy_declared)
                .or_else(|| proxy_with_plain_publish_error(requested_exposed, proxy_declared)),
        };
    }
    let (bind_host, reason) = if !requested_exposed || tls_enabled {
        (requested_host.to_string(), BindReason::AsRequested)
    } else if proxy_declared {
        ("127.0.0.1".to_string(), BindReason::ProxyOnLoopback)
    } else {
        (
            "127.0.0.1".to_string(),
            BindReason::PlainHttpFellBackToLoopback,
        )
    };
    // What the network can reach is what is bound (a fallback binds loopback and
    // reaches nobody), plus whatever a declared proxy forwards.
    let reachable_from_network = !is_loopback_host(&bind_host) || proxy_declared;
    BootBind {
        bind_host,
        reason,
        reachable_from_network,
        fatal: None,
    }
}

impl BootBind {
    /// The line the boot logs for a bind that is not what was asked, if any.
    pub fn notice(&self) -> Option<String> {
        match self.reason {
            BindReason::AsRequested => None,
            BindReason::ProxyOnLoopback => Some(
                "Network exposure goes through the declared TLS proxy: Kronn itself listens \
                 on 127.0.0.1 only, so the proxy on this machine is the only way in."
                    .to_string(),
            ),
            BindReason::PlainHttpFellBackToLoopback => Some(format!(
                "Network exposure was requested but nothing carries TLS: starting on \
                 127.0.0.1 instead of serving the token and P2P invites in clear. Declare a \
                 TLS proxy and its public https:// address in Settings › Server & network \
                 access (or set {TLS_TERMINATED_ENV}=1), or turn the exposure off."
            )),
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn plain_http_guard_refuses_an_exposed_instance_without_tls_or_the_proxy_flag() {
        // Exposed, no TLS, no declaration → refuse, naming the flag to set.
        let msg = plain_http_lan_boot_error(true, false, false).expect("must refuse");
        assert!(msg.contains(TLS_TERMINATED_ENV));
        // Loopback-only use is accepted.
        assert!(plain_http_lan_boot_error(false, false, false).is_none());
        // Either way out lifts the refusal.
        assert!(plain_http_lan_boot_error(true, true, false).is_none());
        assert!(plain_http_lan_boot_error(true, false, true).is_none());
    }

    #[test]
    fn tls_flag_spelling_matches_the_other_exposure_flags() {
        assert!(flag_enabled(Some("1")));
        assert!(flag_enabled(Some(" true ")));
        assert!(flag_enabled(Some("yes")));
        assert!(!flag_enabled(Some("0")));
        assert!(!flag_enabled(Some("")));
        assert!(!flag_enabled(None));
    }

    #[test]
    fn a_native_backend_keeps_its_configured_host_whatever_its_data_dir() {
        // KRONN_DATA_DIR used to stand in for "in Docker" and moved a native
        // backend onto 0.0.0.0; only a real container does that now.
        assert_eq!(resolve_bind_host(None, false, "127.0.0.1"), "127.0.0.1");
        assert_eq!(
            resolve_bind_host(Some("  "), false, "127.0.0.1"),
            "127.0.0.1"
        );
        assert_eq!(resolve_bind_host(None, true, "127.0.0.1"), "0.0.0.0");
        assert_eq!(
            resolve_bind_host(Some("0.0.0.0"), false, "127.0.0.1"),
            "0.0.0.0"
        );
        assert_eq!(
            resolve_bind_host(Some(" 127.0.0.1 "), true, "0.0.0.0"),
            "127.0.0.1"
        );
    }

    #[test]
    fn exposed_host_detection() {
        assert!(is_exposed_host("0.0.0.0"));
        assert!(is_exposed_host("::"));
        assert!(!is_exposed_host("127.0.0.1"));
        assert!(!is_exposed_host("localhost"));
        assert!(!is_exposed_host("192.168.1.10"));
    }

    #[test]
    fn restart_required_compares_against_boot_host() {
        // Before any boot host is recorded, we can't know → never demand a restart.
        // (BOUND_HOST is a process-global OnceLock; in the test binary it may or
        // may not be set by other code, so only assert the None-path invariant
        // when it's genuinely unset.)
        if boot_exposed().is_none() {
            assert!(!restart_required("0.0.0.0"));
            assert!(!restart_required("127.0.0.1"));
        }
    }

    #[test]
    fn loopback_detection_is_strict() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("127.0.1.1"));
        assert!(is_loopback_host("  127.0.0.1 "));
        // A specific LAN IP IS reachable → NOT loopback (stricter than the toggle).
        assert!(!is_loopback_host("192.168.1.10"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("::"));
    }

    #[test]
    fn every_spelling_of_loopback_is_loopback() {
        // KT-1596 review: these were refused as "LAN" before the host was parsed.
        for host in [
            "127.0.0.1",
            "127.255.255.254",
            "localhost",
            "LOCALHOST",
            "localhost.",
            "::1",
            "[::1]",
            "0:0:0:0:0:0:0:1",
            "[0:0:0:0:0:0:0:1]",
            "::ffff:127.0.0.1",
            "[::ffff:127.0.0.1]",
            " [::1] ",
        ] {
            assert!(is_loopback_host(host), "{host:?} must be loopback");
        }
    }

    #[test]
    fn a_lan_or_lookalike_address_is_never_loopback() {
        for host in [
            "192.168.1.10",
            "10.0.0.5",
            "100.64.1.5",
            "0.0.0.0",
            "::",
            "[::]",
            "fe80::1",
            "::ffff:192.168.1.10",
            "kronn.local",
            // A NAME that starts with 127. is not the loopback address.
            "127.evil.example",
            "127.0.0.1.evil.example",
            "localhost.evil.example",
            "",
        ] {
            assert!(!is_loopback_host(host), "{host:?} must NOT be loopback");
        }
        // And the guards agree: those forms are LAN exposure, loopback ones are not.
        assert!(lan_exposed(false, None, "192.168.1.10"));
        assert!(!lan_exposed(false, None, "[::1]"));
        assert!(!lan_exposed(false, None, "::ffff:127.0.0.1"));
        assert!(!lan_exposed(true, Some("[::1]"), "0.0.0.0"));
        assert!(lan_exposed(true, Some("192.168.1.10"), "0.0.0.0"));
    }

    #[test]
    fn the_toggle_refuses_to_expose_plain_http_and_says_what_is_needed() {
        let message = plain_http_exposure_refusal(false, false).expect("must refuse");
        assert!(message.contains("proxy"), "{message}");
        assert!(message.contains("https://"), "{message}");
        assert!(plain_http_exposure_refusal(true, false).is_none());
        assert!(plain_http_exposure_refusal(false, true).is_none());
    }

    #[test]
    fn the_proxy_declaration_comes_from_the_config_or_the_environment() {
        assert!(!proxy_declared(false, None));
        assert!(!proxy_declared(false, Some("0")));
        assert!(proxy_declared(true, None));
        assert!(proxy_declared(false, Some("1")));
        assert_eq!(public_origin(None, None), None);
        assert_eq!(public_origin(Some(" "), None), None);
        assert_eq!(
            public_origin(Some("https://a.example"), Some("https://b.example")).as_deref(),
            Some("https://a.example")
        );
        assert_eq!(
            public_origin(None, Some(" https://b.example ")).as_deref(),
            Some("https://b.example")
        );
    }

    #[test]
    fn a_proxy_on_this_machine_withdraws_the_loopback_owner_trust() {
        // A declared proxy: a loopback peer (the bridge gateway in Docker, as a
        // host proxy reaches nginx from it) is the proxy, not the owner.
        assert!(!loopback_peer_is_owner(true));
        assert!(loopback_peer_is_owner(false));
    }

    #[test]
    fn a_short_or_hand_written_proof_file_never_matches() {
        let dir = std::env::temp_dir().join(format!("kronn-proof-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // Hand-written, shorter than minted: refused even when presented verbatim.
        std::fs::write(dir.join(LOCAL_PROOF_FILE), "letmein").unwrap();
        assert!(!local_proof_matches(&dir, Some("letmein")));
        // `local_proof` replaces it with a minted one that does match.
        let minted = local_proof(&dir).unwrap();
        assert!(minted.len() >= LOCAL_PROOF_MIN_LEN);
        assert!(local_proof_matches(&dir, Some(&minted)));
        assert!(!local_proof_matches(&dir, Some("letmein")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)] // creates symlinks through std::os::unix
    #[test]
    fn a_symlinked_proof_is_refused_on_read_and_replaced_on_mint() {
        let dir = tempfile::tempdir().unwrap();
        // A known file of 32+ bytes, planted as the proof.
        let known = dir.path().join("known");
        let known_content = "k".repeat(64);
        std::fs::write(&known, &known_content).unwrap();
        std::os::unix::fs::symlink(&known, dir.path().join(LOCAL_PROOF_FILE)).unwrap();
        assert!(!local_proof_matches(dir.path(), Some(&known_content)));
        let minted = local_proof(dir.path()).unwrap();
        assert_ne!(minted, known_content, "the planted link is not adopted");
        let meta = std::fs::symlink_metadata(dir.path().join(LOCAL_PROOF_FILE)).unwrap();
        assert!(meta.file_type().is_file(), "the link itself was replaced");
        assert_eq!(std::fs::read_to_string(&known).unwrap(), known_content);
        assert!(local_proof_matches(dir.path(), Some(&minted)));
        assert!(!local_proof_matches(dir.path(), Some(&known_content)));
    }

    #[cfg(unix)] // asserts POSIX mode bits
    #[test]
    fn a_proof_others_can_access_is_refused_on_read_and_replaced_on_mint() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOCAL_PROOF_FILE);
        let planted = "p".repeat(64);
        std::fs::write(&path, &planted).unwrap();
        for mode in [0o644, 0o640, 0o604] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(!local_proof_matches(dir.path(), Some(&planted)), "{mode:o}");
        }
        let minted = local_proof(dir.path()).unwrap();
        assert_ne!(minted, planted);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "owner-only, got {mode:o}");
        assert!(local_proof_matches(dir.path(), Some(&minted)));
    }

    #[test]
    fn a_stale_exposed_config_boots_on_loopback_instead_of_refusing() {
        // KT-1596 review: the config says 0.0.0.0, nothing carries TLS. Refusing
        // to start would strand the owner with no UI; loopback keeps it reachable.
        for requested in ["0.0.0.0", "::", "192.168.1.10"] {
            let bind = boot_bind(requested, false, None, false, false);
            assert_eq!(bind.bind_host, "127.0.0.1", "{requested}");
            assert_eq!(bind.reason, BindReason::PlainHttpFellBackToLoopback);
            assert!(bind.fatal.is_none(), "native never refuses to start");
            assert!(!bind.reachable_from_network, "nobody can reach loopback");
            let notice = bind.notice().expect("the owner is told why");
            assert!(notice.contains(TLS_TERMINATED_ENV), "{notice}");
        }
    }

    #[test]
    fn a_declared_proxy_binds_loopback_and_still_counts_as_reachable() {
        let bind = boot_bind("0.0.0.0", false, None, false, true);
        assert_eq!(bind.bind_host, "127.0.0.1");
        assert_eq!(bind.reason, BindReason::ProxyOnLoopback);
        // The auth guard must still see the network behind the proxy.
        assert!(bind.reachable_from_network);
        assert!(bind.fatal.is_none());
        assert!(bind.notice().is_some());
    }

    #[test]
    fn loopback_use_and_tls_are_bound_as_asked() {
        for host in ["127.0.0.1", "[::1]", "localhost"] {
            let bind = boot_bind(host, false, None, false, false);
            assert_eq!(bind.bind_host, host);
            assert_eq!(bind.reason, BindReason::AsRequested);
            assert!(!bind.reachable_from_network);
            assert!(bind.notice().is_none());
        }
        // Whatever encrypts the exposure itself keeps the requested bind.
        let bind = boot_bind("0.0.0.0", false, None, true, false);
        assert_eq!(bind.bind_host, "0.0.0.0");
        assert!(bind.reachable_from_network);
    }

    #[test]
    fn docker_cannot_fall_back_so_a_plain_publish_still_refuses() {
        // The publish is outside the process: refuse, naming both variables.
        let bind = boot_bind("0.0.0.0", true, Some("0.0.0.0"), false, false);
        let fatal = bind.fatal.expect("must refuse");
        assert!(fatal.contains(TLS_TERMINATED_ENV) && fatal.contains(PUBLIC_ORIGIN_ENV));
        assert_eq!(bind.bind_host, "0.0.0.0", "the container still binds all");
        // Loopback publish, or a declared proxy, boots.
        assert!(boot_bind("0.0.0.0", true, Some("127.0.0.1"), false, false)
            .fatal
            .is_none());
        assert!(boot_bind("0.0.0.0", true, None, false, false)
            .fatal
            .is_none());
        // A declared proxy with a loopback publish boots: the proxy is the way in.
        assert!(boot_bind("0.0.0.0", true, Some("127.0.0.1"), false, true)
            .fatal
            .is_none());
    }

    #[test]
    fn docker_with_a_declared_proxy_refuses_a_plain_port_published_on_the_network() {
        // KT-1596 review: the declaration says the proxy is the only way in, yet
        // KRONN_BIND=0.0.0.0 leaves Kronn's own plain port open on the LAN.
        let bind = boot_bind("0.0.0.0", true, Some("0.0.0.0"), false, true);
        let fatal = bind.fatal.expect("must refuse");
        assert!(fatal.contains("KRONN_BIND=127.0.0.1"), "{fatal}");
        assert!(proxy_with_plain_publish_error(true, true).is_some());
        assert!(proxy_with_plain_publish_error(false, true).is_none());
        assert!(proxy_with_plain_publish_error(true, false).is_none());
        // Native never reaches it: the declared proxy binds loopback there.
        assert!(boot_bind("0.0.0.0", false, None, false, true)
            .fatal
            .is_none());
    }

    #[test]
    fn plain_http_is_blocked_on_the_effective_bind_request() {
        // The requested bind, KRONN_HOST included, decides: not only the config.
        let from_env = resolve_bind_host(Some("0.0.0.0"), false, "127.0.0.1");
        assert!(plain_http_blocked(false, &from_env, false));
        assert!(plain_http_blocked(false, "192.168.1.10", false));
        assert!(
            !plain_http_blocked(false, &from_env, true),
            "declared proxy"
        );
        assert!(!plain_http_blocked(false, "127.0.0.1", false), "loopback");
        assert!(!plain_http_blocked(false, "[::1]", false));
        assert!(
            !plain_http_blocked(true, "0.0.0.0", false),
            "Docker refuses to start instead"
        );
    }

    #[test]
    fn only_the_declared_public_origin_is_a_proxy_origin() {
        let mut server = crate::core::config::default_config().server;
        server.public_origin = Some("https://Kronn.Example.com:443/".into());
        // An origin without the declaration allows nothing.
        assert_eq!(declared_origin(&server), None);
        server.tls_terminated_by_proxy = true;
        assert_eq!(
            declared_origin(&server).as_deref(),
            Some("https://kronn.example.com")
        );
        server.public_origin = Some("https://kronn.example.com:8443".into());
        assert_eq!(
            declared_origin(&server).as_deref(),
            Some("https://kronn.example.com:8443")
        );
        // Declared, but no usable origin: nothing to allow.
        server.public_origin = Some("http://kronn.example.com".into());
        assert_eq!(declared_origin(&server), None);
        server.public_origin = None;
        assert_eq!(declared_origin(&server), None);
    }

    #[test]
    fn recovery_behind_a_proxy_needs_the_machine_proof_not_just_a_loopback_peer() {
        // No proxy: a loopback peer is the owner, as before.
        assert!(recovery_caller_is_local(false, true, false));
        assert!(!recovery_caller_is_local(false, false, false));
        // Behind a proxy: the proxy is the loopback peer, so the proof decides.
        assert!(!recovery_caller_is_local(true, true, false));
        assert!(recovery_caller_is_local(true, true, true));
        // A proof from a non-loopback peer is still refused.
        assert!(!recovery_caller_is_local(true, false, true));
    }

    #[test]
    fn the_local_proof_is_minted_once_kept_private_and_compared_exactly() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing to match before it exists, and matching never creates it.
        assert!(!local_proof_matches(dir.path(), Some("anything")));
        assert!(!dir.path().join(LOCAL_PROOF_FILE).exists());
        let proof = local_proof(dir.path()).unwrap();
        assert!(proof.len() >= 32);
        assert_eq!(
            local_proof(dir.path()).unwrap(),
            proof,
            "stable once minted"
        );
        assert!(local_proof_matches(dir.path(), Some(&proof)));
        assert!(local_proof_matches(
            dir.path(),
            Some(&format!(" {proof}\n"))
        ));
        assert!(!local_proof_matches(dir.path(), Some("wrong")));
        assert!(!local_proof_matches(dir.path(), Some("")));
        assert!(!local_proof_matches(dir.path(), None));
        // An empty file is not a proof that an empty header could match.
        std::fs::write(dir.path().join(LOCAL_PROOF_FILE), "").unwrap();
        assert!(!local_proof_matches(dir.path(), Some("")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let fresh = tempfile::tempdir().unwrap();
            local_proof(fresh.path()).unwrap();
            let mode = std::fs::metadata(fresh.path().join(LOCAL_PROOF_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "owner-only, got {mode:o}");
        }
    }

    #[test]
    fn toggle_then_restart_binds_what_the_toggle_could_store() {
        // The toggle only ever stores 0.0.0.0 next to a declared proxy (the API
        // refuses it otherwise, see `plain_http_exposure_refusal`), and that boot
        // binds loopback: no restart is due, and the exposure is the proxy's.
        let stored_by_the_toggle = "0.0.0.0";
        let next_boot = boot_bind(stored_by_the_toggle, false, None, false, true);
        assert_eq!(next_boot.bind_host, "127.0.0.1");
        assert!(!restart_due("127.0.0.1", &next_boot.bind_host));
        // Direct exposure (a future in-process TLS) is the case that needs one.
        let tls_boot = boot_bind(stored_by_the_toggle, false, None, true, false);
        assert!(restart_due("127.0.0.1", &tls_boot.bind_host));
        assert!(!restart_due("0.0.0.0", &tls_boot.bind_host));
        // A stale config falls back to loopback: nothing to restart for either.
        let stale = boot_bind(stored_by_the_toggle, false, None, false, false);
        assert!(!restart_due("127.0.0.1", &stale.bind_host));
    }

    #[test]
    fn both_boot_paths_bind_what_boot_bind_decided() {
        // The standalone backend and the embedded desktop server must feed the
        // shared plan, bind its host, and drop the old refuse-to-start call.
        let backend = include_str!("../main.rs");
        let desktop = include_str!("../../../desktop/src-tauri/src/main.rs");
        for (name, source) in [("backend", backend), ("desktop", desktop)] {
            assert!(
                source.contains("net_expose::boot_bind(") || source.contains("embedded_boot_bind("),
                "{name} must resolve its bind through net_expose::boot_bind"
            );
            assert!(
                !source.contains("plain_http_lan_boot_error("),
                "{name} must not refuse to start natively: the plan falls back to loopback"
            );
            assert!(
                source.contains("boot_bind.bind_host"),
                "{name} must bind the planned host"
            );
            assert!(
                source.contains("reachable_from_network"),
                "{name} must feed the auth guard from the plan"
            );
        }
        assert!(
            backend.contains("boot_bind.fatal"),
            "Docker's refusal must stay wired in the backend"
        );
        assert!(desktop.contains("fn embedded_boot_bind"));
    }

    #[test]
    fn lan_exposed_is_docker_aware() {
        // Docker: internal bind is always 0.0.0.0 — only KRONN_BIND (host publish) counts.
        assert!(
            !lan_exposed(true, Some("127.0.0.1"), "0.0.0.0"),
            "docker loopback publish = safe"
        );
        assert!(
            !lan_exposed(true, None, "0.0.0.0"),
            "docker no KRONN_BIND = default safe"
        );
        assert!(
            lan_exposed(true, Some("0.0.0.0"), "0.0.0.0"),
            "docker opted into LAN publish"
        );
        // Native: the resolved bind host is authoritative.
        assert!(!lan_exposed(false, None, "127.0.0.1"));
        assert!(lan_exposed(false, None, "0.0.0.0"));
        assert!(lan_exposed(false, None, "192.168.1.10"));
    }

    #[test]
    fn insecure_lan_guard_only_fires_when_exposed_and_unauthenticated() {
        // Safe: not exposed → never blocks, regardless of auth.
        assert!(insecure_lan_boot_error(false, false, false, false).is_none());
        // Exposed + unauthenticated + not acknowledged → BLOCK.
        assert!(insecure_lan_boot_error(true, false, false, false).is_some());
        // Exposed + auth ENFORCED (enabled AND token) → fine.
        assert!(insecure_lan_boot_error(true, true, true, false).is_none());
        // Token WITHOUT auth_enabled is a middleware no-op (auth off → every
        // endpoint open) — the Docker default (auth off + auto-generated
        // token) must NOT pass the guard on the strength of the dead token.
        assert!(insecure_lan_boot_error(true, false, true, false).is_some());
        // auth_enabled WITHOUT a token is equally open (no token → open).
        assert!(insecure_lan_boot_error(true, true, false, false).is_some());
        // Exposed + unauthenticated but explicitly acknowledged → allowed.
        assert!(insecure_lan_boot_error(true, false, false, true).is_none());
    }

    #[test]
    fn record_bound_host_then_restart_required_reflects_diff() {
        // First write wins; assert the logic via boot_exposed once set. We don't
        // assume ordering vs other tests — only that AFTER a known value the
        // diff logic holds for that value.
        record_bound_host("127.0.0.1");
        let booted = boot_exposed().expect("recorded");
        // restart_required is true exactly when the configured exposure flips.
        assert_eq!(restart_required("0.0.0.0"), !booted);
        assert_eq!(restart_required("127.0.0.1"), booted);
    }
}
