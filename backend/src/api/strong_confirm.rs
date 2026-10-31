//! KT-1496 — the gate that makes the routes returning a secret, or destroying
//! data, ask for the strong confirmation of KT-1492 when one is enrolled.
//!
//! The owner's rule is "never bridle more than before": without an enrolment
//! no request is touched. With one, a protected request (`core::strong_confirm`)
//! must carry a proof, whoever sends it and wherever it comes from, loopback
//! included. The code is the proof on every channel:
//! - the `X-Kronn-Confirm` header or a `kronn_confirm` body field (a script, a
//!   CLI, an agent the owner gave the code to);
//! - an `X-Kronn-Grant` header: a single-use grant bound to one call, minted
//!   by `POST /api/trust/grants` from a code (what the screen does after the
//!   dialog, and what a browser download needs);
//! - a bridge-token request carrying either: accepted for that one action
//!   (`BridgeCodePass`, set by the bridge gate), and only when a strong
//!   confirmation is enrolled.
//!
//! A code is single use (the TOTP step is consumed, a recovery code is spent),
//! bound to the one route and object of the call that presented it, journalled
//! with its channel, and rate limited (5 failures, then 60 s) by
//! `core::trust_proof`. A grant is bound to a method and a path, expires after
//! two minutes and is spent by the first attempt that names it.
//!
//! Routes where only some calls matter ask from the handler, through
//! [`Strong::require`]. A bridge request on a route of its own list
//! (`BRIDGE_ROUTES`) is left to that list's scoping, as before.

// The refusal is a whole response on purpose: it is returned as is.
#![allow(clippy::result_large_err)]

use axum::{
    extract::{FromRequestParts, MatchedPath, Request, State},
    http::{request::Parts, HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};

use crate::core::strong_confirm::{self as strong, Protection};
use crate::core::trust_proof::{self, Binding, Channel, ProofError};
use crate::models::{ApiErrorCode, ApiResponse, StrongGrant, StrongGrantRequest};
use crate::AppState;

/// Largest JSON body read to look for a `kronn_confirm` field.
const PEEK_LIMIT: usize = 1024 * 1024;

/// Set by the bridge gate on a bridge-token request that reached a protected
/// route outside the bridge list because it carries a code or a grant.
#[derive(Debug, Clone, Copy)]
pub struct BridgeCodePass;

/// Set by the gate once the request's proof was accepted (and spent).
#[derive(Debug, Clone, Copy)]
pub struct StrongProven;

/// What a request carries as proof.
#[derive(Debug, Clone, Default)]
pub struct Presented {
    pub code: Option<String>,
    pub grant: Option<String>,
}

impl Presented {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let read = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        Self {
            code: read(crate::api::trust::CONFIRM_HEADER),
            grant: read(strong::GRANT_HEADER),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.code.is_none() && self.grant.is_none()
    }
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .is_some_and(|mime| mime == "application/json" || mime.ends_with("+json"))
}

/// The proof of a request: its headers, else the body field. A body is read
/// only when it is JSON and declares a size under [`PEEK_LIMIT`]; it is put
/// back whole.
pub async fn presented_by(parts: &Parts, body: axum::body::Body) -> (Presented, axum::body::Body) {
    let mut presented = Presented::from_headers(&parts.headers);
    if !presented.is_empty() || !is_json(&parts.headers) {
        return (presented, body);
    }
    let declared = parts
        .headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if !declared.is_some_and(|size| size <= PEEK_LIMIT) {
        return (presented, body);
    }
    match axum::body::to_bytes(body, PEEK_LIMIT).await {
        Ok(bytes) => {
            presented.code = strong::body_code(&bytes);
            (presented, axum::body::Body::from(bytes))
        }
        // Unreadable: the handler will see the same failure.
        Err(_) => (presented, axum::body::Body::empty()),
    }
}

fn channel_of(parts: &Parts) -> Channel {
    if parts
        .extensions
        .get::<crate::core::bridge_token::BridgeCaller>()
        .is_some()
    {
        Channel::Bridge
    } else if parts.headers.contains_key(axum::http::header::ORIGIN)
        || parts.headers.contains_key("sec-fetch-site")
    {
        Channel::Ui
    } else {
        Channel::Http
    }
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

fn required(message: &str) -> Response {
    refusal(
        StatusCode::FORBIDDEN,
        ApiErrorCode::StrongConfirmationRequired,
        message,
    )
}

fn proof_refusal(error: ProofError) -> Response {
    match &error {
        ProofError::NotEnrolled | ProofError::Invalid => required(&error.to_string()),
        ProofError::Locked(_) | ProofError::NowLocked(_) | ProofError::RecoveryRefused(_) => {
            refusal(
                StatusCode::TOO_MANY_REQUESTS,
                ApiErrorCode::StrongConfirmationRequired,
                &error.to_string(),
            )
        }
        ProofError::PendingEnrolment => refusal(
            StatusCode::CONFLICT,
            ApiErrorCode::Validation,
            &error.to_string(),
        ),
        ProofError::Storage(_) => refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            &error.to_string(),
        ),
    }
}

/// The installation key and the data directory of the confirmation record.
async fn store(state: &AppState) -> Result<([u8; 32], std::path::PathBuf), Response> {
    let unavailable = || {
        refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            "the confirmation record is unavailable: the installation key is not readable",
        )
    };
    let secret = state.config.read().await.encryption_secret.clone();
    let key = secret
        .as_deref()
        .and_then(|secret| crate::core::crypto::parse_secret(secret).ok())
        .ok_or_else(unavailable)?;
    let dir = crate::core::config::config_dir().map_err(|_| unavailable())?;
    Ok((key, dir))
}

/// Whether a strong confirmation is enrolled. An unreadable record is an
/// error, never "not enrolled": a protected route does not open on a failure.
async fn enrolled() -> Result<bool, Response> {
    let read = tokio::task::spawn_blocking(|| {
        let dir =
            crate::core::config::config_dir().map_err(|e| ProofError::Storage(e.to_string()))?;
        trust_proof::is_enrolled(&dir)
    })
    .await;
    match read {
        Ok(Ok(enrolled)) => Ok(enrolled),
        Ok(Err(error)) => Err(proof_refusal(error)),
        Err(error) => Err(refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            &error.to_string(),
        )),
    }
}

async fn journal(state: &AppState, accepted: bool, binding: Binding, reason: &str) {
    let reason = reason.to_owned();
    if let Err(error) = state
        .db
        .with_conn(move |conn| crate::db::trust::record_proof(conn, accepted, &binding, &reason))
        .await
    {
        tracing::warn!(%error, "the confirmation could not be journalled");
    }
}

/// Check a proof for one call and spend it. `binding` names the route, the
/// object and the channel the journal records.
async fn spend(
    state: &AppState,
    presented: &Presented,
    binding: Binding,
    target: &str,
) -> Result<(), Response> {
    if let Some(grant) = presented.grant.as_deref() {
        let spent = strong::consume(grant, target, now_secs());
        let reason = if spent {
            "accepted (grant)"
        } else {
            "refused (grant unknown, expired or for another call)"
        };
        journal(state, spent, binding, reason).await;
        return if spent {
            Ok(())
        } else {
            Err(required(
                "the grant is unknown, expired or for another action: enter the confirmation code again",
            ))
        };
    }
    // No proof at all is the first request of a dialog, not a failed guess: it
    // neither counts toward the lockout nor fills the journal.
    let Some(code) = presented.code.clone() else {
        return Err(required(
            "this action needs the strong confirmation: send the current code of your authenticator in the X-Kronn-Confirm header, or confirm it on the Kronn screen",
        ));
    };
    let (key, dir) = store(state).await?;
    let spent_on = binding.clone();
    let checked = tokio::task::spawn_blocking(move || {
        trust_proof::check_bound(&dir, &key, &code, now_secs(), &spent_on)
    })
    .await
    .map_err(|error| {
        refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            &error.to_string(),
        )
    })?;
    // A lockout or an unreadable record is not journalled: a flood of attempts
    // must not fill the journal.
    let note = match &checked {
        Ok(()) => Some((true, "accepted".to_owned())),
        Err(ProofError::Invalid) => Some((false, "missing, wrong or already used".to_owned())),
        Err(ProofError::NotEnrolled) => Some((false, "refused: no enrolment".to_owned())),
        // The failure that starts a lockout is the one line of a flood that is
        // always written: an alert the owner reads in the journal.
        Err(ProofError::NowLocked(secs)) => Some((
            false,
            format!("wrong — ALERT: repeated failures, the confirmation is locked for {secs} s"),
        )),
        Err(ProofError::RecoveryRefused(_)) => Some((
            false,
            "wrong recovery code while locked (counted toward the daily cap)".to_owned(),
        )),
        Err(_) => None,
    };
    if let Some((accepted, reason)) = note {
        journal(state, accepted, binding, &reason).await;
    }
    checked.map_err(proof_refusal)
}

/// The gate. A protected route of an instance with no enrolment is untouched.
pub async fn strong_gate(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_owned();
    let Some(pattern) = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
    else {
        return next.run(request).await;
    };
    let protection = strong::protection(&method, &pattern);
    if protection == Protection::Open {
        return next.run(request).await;
    }
    let bridge_pass = request.extensions().get::<BridgeCodePass>().is_some();
    // A bridge request admitted on its own list keeps that list's scoping.
    let on_bridge_list = request
        .extensions()
        .get::<crate::core::bridge_token::BridgeCaller>()
        .is_some()
        && !bridge_pass;
    if on_bridge_list {
        return next.run(request).await;
    }

    let path = request.uri().path().to_owned();
    let (mut parts, body) = request.into_parts();
    let (presented, body) = presented_by(&parts, body).await;
    let enrolled = match enrolled().await {
        Ok(enrolled) => enrolled,
        Err(refused) => return refused,
    };
    if !enrolled {
        // An agent reaches a protected route outside its list only with a code
        // the owner gave it: with nothing enrolled there is none, so it stays
        // refused as before.
        if bridge_pass {
            return required(
                "this route is not available to a Kronn-launched agent's bridge token unless a strong confirmation is enrolled and its code is sent",
            );
        }
        parts.extensions.insert(presented);
        return next.run(Request::from_parts(parts, body)).await;
    }
    if protection == Protection::Conditional && !bridge_pass {
        // The handler decides whether this call needs the proof.
        parts.extensions.insert(presented);
        return next.run(Request::from_parts(parts, body)).await;
    }

    let binding = Binding {
        channel: channel_of(&parts),
        route: strong::route_label(&method, &pattern),
        object: strong::object_of(&pattern, &path),
    };
    let target = strong::target_of(&method, &path);
    if let Err(refused) = spend(&state, &presented, binding, &target).await {
        return refused;
    }
    parts.extensions.insert(presented);
    parts.extensions.insert(StrongProven);
    next.run(Request::from_parts(parts, body)).await
}

/// What a handler needs to ask for the proof of a call it decided is sensitive.
#[derive(Debug, Clone)]
pub struct Strong {
    presented: Presented,
    proven: bool,
    channel: Channel,
    method: String,
    pattern: String,
    path: String,
}

impl<S: Send + Sync> FromRequestParts<S> for Strong {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self {
            presented: parts
                .extensions
                .get::<Presented>()
                .cloned()
                .unwrap_or_else(|| Presented::from_headers(&parts.headers)),
            proven: parts.extensions.get::<StrongProven>().is_some(),
            channel: channel_of(parts),
            method: parts.method.as_str().to_owned(),
            pattern: parts
                .extensions
                .get::<MatchedPath>()
                .map(|matched| matched.as_str().to_owned())
                .unwrap_or_default(),
            path: parts.uri.path().to_owned(),
        })
    }
}

impl Strong {
    /// Ask for the proof of this call. Nothing is asked when no strong
    /// confirmation is enrolled, or when the gate already spent a proof.
    pub async fn require(&self, state: &AppState) -> Result<(), Response> {
        if self.proven || !enrolled().await? {
            return Ok(());
        }
        let binding = Binding {
            channel: self.channel,
            route: strong::route_label(&self.method, &self.pattern),
            object: strong::object_of(&self.pattern, &self.path),
        };
        let target = strong::target_of(&self.method, &self.path);
        spend(state, &self.presented, binding, &target).await
    }
}

/// POST /api/trust/grants — exchange a code for a single-use grant bound to one
/// call. This is what the screen does once the owner typed the code; the grant
/// then travels in `X-Kronn-Grant`, and a browser download that cannot carry a
/// code header uses it too. An agent without the code cannot obtain one.
pub async fn mint_grant(
    State(state): State<AppState>,
    parts: StrongMint,
    Json(request): Json<StrongGrantRequest>,
) -> Response {
    let method = request.method.trim().to_ascii_uppercase();
    let path = request
        .path
        .trim()
        .split('?')
        .next()
        .unwrap_or("")
        .to_owned();
    if method.parse::<axum::http::Method>().is_err() || !path.starts_with("/api/") {
        return refusal(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::Validation,
            "a grant names the method and the /api path of the call it is for",
        );
    }
    match enrolled().await {
        Ok(true) => {}
        Ok(false) => {
            return refusal(
                StatusCode::CONFLICT,
                ApiErrorCode::Validation,
                "no strong confirmation is enrolled: nothing asks for a grant",
            )
        }
        Err(refused) => return refused,
    }
    let presented = Presented {
        code: request
            .code
            .map(|code| code.trim().to_owned())
            .filter(|code| !code.is_empty())
            .or(parts.0.code),
        grant: None,
    };
    let binding = Binding {
        channel: parts.1,
        route: format!("{method} {path}"),
        object: "grant".to_owned(),
    };
    let target = strong::target_of(&method, &path);
    if let Err(refused) = spend(&state, &presented, binding, &target).await {
        return refused;
    }
    let grant = strong::mint(&target, now_secs());
    (
        StatusCode::OK,
        Json(ApiResponse::ok(StrongGrant {
            grant,
            expires_in_secs: strong::GRANT_TTL_SECS as u32,
        })),
    )
        .into_response()
}

/// The code a mint request carries in its header, and the channel it came by.
pub struct StrongMint(Presented, Channel);

impl<S: Send + Sync> FromRequestParts<S> for StrongMint {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            Presented::from_headers(&parts.headers),
            channel_of(parts),
        ))
    }
}
