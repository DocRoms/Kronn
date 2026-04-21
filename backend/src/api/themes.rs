//! Unlock endpoint — swaps a user-typed code for one or more unlocks
//! (themes, profiles, …). Two sources are consulted:
//!
//!   1. `BUILT_IN_UNLOCK_HASHES` below — `(kind, name, sha256_hex)`
//!      triples committed to the repo and shipped with every release.
//!      Same code works on every self-hosted instance after update.
//!      A single code CAN appear on multiple rows (same hash repeated) —
//!      the endpoint returns ALL matching unlocks, enabling bundles like
//!      "Batman code unlocks profile AND gotham theme together".
//!   2. `config.secret_themes` — plaintext THEME-only overrides in the
//!      operator's local `~/.config/kronn/config.toml`. Kept for quick
//!      testing without a rebuild; profile unlocks go through built-ins.
//!
//! Generic `invalid code` error on miss so a brute-forcer cannot tell
//! "wrong code" from "this thing isn't configured here".
//!
//! ## Adding / rotating a code
//!
//! Pick a long-ish code (≥12 chars — dictionary resistance comes from
//! length since the hash is unsalted). Compute its hash once:
//!
//! ```sh
//! echo -n 'YourCodeHere' | sha256sum | cut -d' ' -f1
//! ```
//!
//! Paste the hex into `BUILT_IN_UNLOCK_HASHES` as `(kind, name, hash)`.
//! Repeat on multiple rows with the SAME hash to bundle unlocks under
//! one code. Remember the code in a password manager — nothing
//! recovers it from the hash; rotation = edit file + release.
//!
//! ## Kinds
//!
//! - `"theme"` — client-side only. Frontend adds to
//!   `localStorage['kronn:unlockedThemes']` and applies the theme.
//! - `"profile"` — server-side. Endpoint persists the profile id into
//!   `config.unlocked_profiles` and saves the config; `GET /api/profiles`
//!   then includes the previously-hidden secret profile.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::config;
use crate::models::ApiResponse;
use crate::AppState;

/// Built-in unlock entries. Format: `(kind, name, sha256_hex)`.
///
/// A single code (= single hash) may appear on multiple rows — every
/// matching row is unlocked in one go (e.g. `kronnBatman` below unlocks
/// BOTH the "batman" profile AND the "gotham" theme).
const BUILT_IN_UNLOCK_HASHES: &[(&str, &str, &str)] = &[
    // ── kronnMatrix ──
    ("theme",   "matrix", "4eda5940efa96ee10b1e15e17b8ef44a182adaedd905d358313cec34f37ae971"),
    // ── kronnSakura ──
    ("theme",   "sakura", "0cf0d64c6ede3ad870a872f14600e4970b3b4809a80be27aa93bbebce351ac97"),
    // ── kronnBatman (bundle: Batman profile + Gotham theme) ──
    ("profile", "batman", "2c367c31a68a254729e77cce88c9025b1f21183d2ab2675924899f52bc8296a7"),
    ("theme",   "gotham", "2c367c31a68a254729e77cce88c9025b1f21183d2ab2675924899f52bc8296a7"),
];

#[derive(Debug, Deserialize)]
pub struct UnlockRequest {
    pub code: String,
}

/// One row of the response array — identifies what kind of thing was
/// unlocked and its canonical name. The frontend dispatches per kind.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UnlockedItem {
    pub kind: String, // "theme" | "profile"
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct UnlockResponse {
    /// All unlocks triggered by the code. At least one entry on success;
    /// a bundle code returns several. Order matches the built-in array
    /// declaration order, so multi-unlock UX is deterministic.
    pub unlocks: Vec<UnlockedItem>,
}

/// Plain SHA-256 hex digest of the UTF-8 bytes of `code`. No salt —
/// stays predictable so contributors can regenerate hashes with any
/// standard tool (`sha256sum`, `shasum -a 256`, an online calculator).
fn hash_code(code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(code.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Collect EVERY built-in match for the given code (a bundle code
/// may match multiple entries that share the same hash).
fn match_built_in(code: &str) -> Vec<UnlockedItem> {
    let hashed = hash_code(code);
    BUILT_IN_UNLOCK_HASHES
        .iter()
        .filter(|(_, _, h)| *h == hashed.as_str())
        .map(|(kind, name, _)| UnlockedItem {
            kind: (*kind).to_string(),
            name: (*name).to_string(),
        })
        .collect()
}

/// POST /api/themes/unlock (legacy name, kept for URL stability)
///
/// Body: `{ "code": "<user-typed-secret>" }`
///
/// Success: `ApiResponse::ok({ unlocks: [{kind, name}, ...] })` — always
/// at least one entry, and bundles return several.
/// Miss or empty: `ApiResponse::err("invalid code")` with HTTP 200.
///
/// Side effect: when an unlock's `kind` is `"profile"`, the name is
/// persisted to `config.unlocked_profiles` and the config file is
/// re-saved so the profile becomes visible in `GET /api/profiles`
/// across restarts.
pub async fn unlock(
    State(state): State<AppState>,
    Json(req): Json<UnlockRequest>,
) -> Json<ApiResponse<UnlockResponse>> {
    let code = req.code.trim();
    if code.is_empty() {
        return Json(ApiResponse::err("invalid code".to_string()));
    }

    // 1. Built-in hashes (shared by every Kronn release).
    let mut matches = match_built_in(code);

    // 2. Operator-local plaintext theme overrides (config.toml).
    //    Profiles go through built-ins only — intentional: the config
    //    path is for quick theme testing without rebuild, profiles
    //    need server-side state so they already require edits here.
    {
        let cfg = state.config.read().await;
        for (theme, stored) in cfg.secret_themes.iter() {
            if stored == code && !matches.iter().any(|m| m.kind == "theme" && &m.name == theme) {
                matches.push(UnlockedItem {
                    kind: "theme".into(),
                    name: theme.clone(),
                });
            }
        }
    }

    if matches.is_empty() {
        return Json(ApiResponse::err("invalid code".to_string()));
    }

    // Persist profile unlocks so subsequent /api/profiles requests
    // see the now-unlocked built-ins. Dedup against existing entries
    // — replaying the same code is idempotent. If the save fails we
    // log and still return success: the UI gets the unlock confirmed,
    // and the next write will reattempt persistence.
    let mut profile_ids_to_persist: Vec<String> = matches
        .iter()
        .filter(|m| m.kind == "profile")
        .map(|m| m.name.clone())
        .collect();
    if !profile_ids_to_persist.is_empty() {
        let mut cfg = state.config.write().await;
        let mut changed = false;
        for id in profile_ids_to_persist.drain(..) {
            if !cfg.unlocked_profiles.iter().any(|p| p == &id) {
                cfg.unlocked_profiles.push(id);
                changed = true;
            }
        }
        if changed {
            if let Err(e) = config::save(&cfg).await {
                tracing::warn!("Failed to persist unlocked profile(s): {}", e);
            }
        }
    }

    Json(ApiResponse::ok(UnlockResponse { unlocks: matches }))
}
