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

/// True when the configured exposure differs from what the process actually
/// bound at boot → a restart is required for the change to take effect.
pub fn restart_required(configured_host: &str) -> bool {
    match boot_exposed() {
        Some(booted) => booted != is_exposed_host(configured_host),
        None => false,
    }
}

/// True when `host` binds ONLY loopback — safe, unreachable from other machines.
/// Stricter than `!is_exposed_host`: a specific LAN IP (`192.168.x`) is NOT
/// loopback (it IS reachable), whereas the `is_exposed_host` toggle semantics
/// only flag the bind-all forms. Used by the boot security guard.
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim();
    h == "127.0.0.1" || h == "localhost" || h == "::1" || h.starts_with("127.")
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

#[cfg(test)]
mod tests {
    use super::*;

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
