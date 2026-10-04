//! GET /api/health — lightweight, unauthenticated.
//!
//! `instance` lets the desktop shell tell its own embedded backend apart from
//! another Kronn already answering on the same loopback port.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};

static INSTANCE_NONCE: OnceLock<String> = OnceLock::new();

/// Set once per process by the desktop shell before the backend starts.
/// The nonce itself is never served, only its hash.
pub fn set_instance_nonce(nonce: String) -> bool {
    INSTANCE_NONCE.set(nonce).is_ok()
}

/// Value served as `instance` for a given nonce.
pub fn instance_tag(nonce: &str) -> String {
    Sha256::digest(nonce.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub async fn health() -> axum::Json<serde_json::Value> {
    let mut body = serde_json::json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "host_os": crate::agents::detect_host_label_public(),
        // Under Docker the UI points to the host-side `kronn` CLI for installs.
        "in_docker": crate::core::env::is_docker(),
    });
    if let Some(nonce) = INSTANCE_NONCE.get() {
        body["instance"] = serde_json::Value::String(instance_tag(nonce));
    }
    axum::Json(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_is_stable_hex_and_differs_per_nonce() {
        let a = instance_tag("nonce-a");
        assert_eq!(a, instance_tag("nonce-a"));
        assert_ne!(a, instance_tag("nonce-b"));
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!a.contains("nonce-a"));
    }
}
