//! KT-1495 — the routes that change or hand out Kronn's own secrets need a
//! human's proof, never the IP alone.
//!
//! A request from the loopback address is trusted as the owner's (the browser,
//! the owner's scripts, the desktop shell), and an agent process runs on that
//! same loopback: it only has to leave its bridge token out of the request to be
//! taken for the owner. `auth_strict_localhost` would close that door and also
//! break every local script and the browser, so it stays off by default. The
//! proof of a human is the strong confirmation instead (`core::trust_proof`: a
//! TOTP code, the same on every channel, the same single use and lockout as the
//! trust routes of KT-1492).
//!
//! The gate asks for a code on the routes that take over or destroy the vault: a
//! new operator token, the recovery passphrase, the encryption key, a whole-DB
//! import, the instance reset. It asks **only a caller that got in on its
//! loopback address**: a request with the operator token already carries a
//! credential, and a bridge token never reaches these routes (they are not on
//! its positive list).
//!
//! Nothing that works today is refused: with no enrolment, or no readable
//! record, the request passes as it always did, and the Settings > Security
//! screen says the vault routes are open until a confirmation is enrolled.

use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::core::trust_proof::{self, Binding, Channel, ProofError};
use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

/// The header that carries the code (the one the trust routes read).
pub use crate::api::trust::CONFIRM_HEADER;

/// The POST routes that take over or destroy the vault, matched by path.
pub const HUMAN_PROOF_ROUTES: &[&str] = &[
    "/api/setup/reset",
    "/api/config/import",
    "/api/config/auth-token/regenerate",
    "/api/config/recovery/set",
    "/api/config/recovery/restore",
    "/api/config/recovery/reencrypt",
    "/api/config/recovery/start-new-key",
];

/// Whether this request is one only a human may make.
pub fn needs_human(method: &Method, path: &str) -> bool {
    method == Method::POST && HUMAN_PROOF_ROUTES.contains(&path)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn refusal(status: StatusCode, code: ApiErrorCode, message: &str) -> Response {
    (
        status,
        Json(ApiResponse::<()>::err_coded(code, message.to_owned())),
    )
        .into_response()
}

/// The refusal for a request with no code.
fn code_required() -> Response {
    refusal(
        StatusCode::FORBIDDEN,
        ApiErrorCode::ConfirmRequired,
        "this action changes Kronn's own secrets: send a code of the strong confirmation in the X-Kronn-Confirm header",
    )
}

/// The verdict on a request that got in on its loopback address alone.
pub enum Verdict {
    /// Nothing is asked: not a vault route, or no confirmation is enrolled.
    Pass,
    /// A code was asked and is valid (it is spent on this route).
    Proven,
    /// Refused, with the answer to send.
    Refused(Response),
}

/// Decide a loopback-trusted request. The record and the key are read off the
/// async worker; a record that cannot be read counts as no enrolment, so the
/// route is never closed because a file is missing.
pub async fn check(state: &AppState, headers: &HeaderMap, method: &Method, path: &str) -> Verdict {
    if !needs_human(method, path) {
        return Verdict::Pass;
    }
    let Ok(dir) = crate::core::config::config_dir() else {
        return Verdict::Pass;
    };
    let enrolled_dir = dir.clone();
    let enrolled = tokio::task::spawn_blocking(move || trust_proof::is_enrolled(&enrolled_dir))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false);
    if !enrolled {
        return Verdict::Pass;
    }
    let code = headers
        .get(CONFIRM_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .map(str::to_owned);
    let Some(code) = code else {
        return Verdict::Refused(code_required());
    };
    let key = state
        .config
        .read()
        .await
        .encryption_secret
        .as_deref()
        .and_then(|secret| crate::core::crypto::parse_secret(secret).ok());
    let Some(key) = key else {
        return Verdict::Refused(refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            "the confirmation record is unavailable: the installation key is not readable",
        ));
    };
    let binding = Binding {
        channel: if headers.contains_key(axum::http::header::ORIGIN)
            || headers.contains_key("sec-fetch-site")
        {
            Channel::Ui
        } else {
            Channel::Http
        },
        route: format!("{method} {path}"),
        object: "vault".to_owned(),
    };
    let spent_on = binding.clone();
    let checked = tokio::task::spawn_blocking(move || {
        trust_proof::check_bound(&dir, &key, &code, now_secs(), &spent_on)
    })
    .await
    .unwrap_or_else(|error| Err(ProofError::Storage(error.to_string())));
    // A lockout or an unreadable record is not journalled: a flood of attempts
    // must not fill the journal.
    let note = match &checked {
        Ok(()) => Some((true, "accepted")),
        Err(ProofError::Invalid) => Some((false, "missing, wrong or already used")),
        Err(_) => None,
    };
    if let Some((accepted, reason)) = note {
        let journalled = binding.clone();
        if let Err(error) = state
            .db
            .with_conn(move |conn| {
                crate::db::trust::record_proof(conn, accepted, &journalled, reason)
            })
            .await
        {
            tracing::warn!(%error, "the confirmation could not be journalled");
        }
    }
    match checked {
        Ok(()) => Verdict::Proven,
        Err(ProofError::Locked(_)) => Verdict::Refused(refusal(
            StatusCode::TOO_MANY_REQUESTS,
            ApiErrorCode::Validation,
            "the confirmation is locked after repeated wrong codes: wait a minute",
        )),
        Err(ProofError::Storage(_)) => Verdict::Refused(refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            "the confirmation record could not be read",
        )),
        Err(_) => Verdict::Refused(refusal(
            StatusCode::FORBIDDEN,
            ApiErrorCode::ConfirmRequired,
            "the confirmation code is missing, wrong or already used",
        )),
    }
}

#[cfg(test)]
#[path = "human_gate_tests.rs"]
mod human_gate_tests;
