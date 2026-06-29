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
