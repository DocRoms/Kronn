//! KT-1492 — trust levels. None of these routes is a bridge route: the bridge
//! gate lets a bridge token reach a write here only when the request carries a
//! strong-confirmation code (`lib.rs`, `bridge_gate`), and the handler checks it.
//! A change is a destructive request (`is_destructive`), refused to a remote
//! caller without a token.
//!
//! Raising a level above Standard needs, in this order: the risk panel's
//! confirmation, an acknowledgement that names the missing vault protection
//! when the host has no sandbox (or the owner turned the protection off), and a
//! proof of the strong confirmation (`core::trust_proof`, a TOTP code or a
//! recovery code) that no request driven through the UI can produce, loopback
//! included. The code is the proof whatever the channel: the UI, an
//! `X-Kronn-Confirm` header or a body field, a bridge-token call. Lowering or
//! revoking never needs it, except on the bridge channel, which always
//! presents one. This module is the only caller of `db::trust::set_level`.

use axum::{
    extract::{FromRequestParts, Path, Query, State},
    http::{request::Parts, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::trust_proof::{self, Binding, Channel, ProofError};
use crate::db::trust::{self, TrustOrigin};
use crate::models::{
    ApiErrorCode, ApiResponse, ConfirmTrustProofRequest, EffectiveTrust, SetTrustRequest,
    TrustAuditEntry, TrustEnrolment, TrustLevel, TrustOverview, TrustProofRequest, TrustProofView,
    TrustRecoveryCodes, TrustSandboxView, TrustScope, TrustSetting,
};
use crate::AppState;

type Reply<T> = (StatusCode, Json<ApiResponse<T>>);

/// The header that carries the strong-confirmation code on any channel.
pub const CONFIRM_HEADER: &str = "x-kronn-confirm";

/// How a request reached a trust route: the channel, and the code its
/// `X-Kronn-Confirm` header carried. The bridge channel is decided by the
/// bridge gate (a live bridge token), never by what the request says; the UI
/// channel is a browser request (it carries an `Origin`), a label for the
/// journal and not a privilege.
#[derive(Debug, Clone, Default)]
pub struct Confirm {
    channel: Channel,
    header: Option<String>,
}

impl Confirm {
    pub fn on(channel: Channel) -> Self {
        Self {
            channel,
            header: None,
        }
    }

    pub fn with_header(mut self, code: &str) -> Self {
        self.header = Some(code.to_owned());
        self
    }

    #[cfg(test)]
    pub(crate) fn channel_for_tests(&self) -> Channel {
        self.channel
    }

    fn is_bridge(&self) -> bool {
        self.channel == Channel::Bridge
    }

    /// The code of the request: the body field, else the header.
    fn code(&self, body: Option<String>) -> Option<String> {
        body.map(|code| code.trim().to_owned())
            .filter(|code| !code.is_empty())
            .or_else(|| self.header.clone())
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Confirm {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        // An agent-marked request (a host-launched CLI behind the MCP bridge,
        // KT-1490) is the agent channel like a bridge token: it needs a code on
        // every write and is never handed a secret. The marker is checked before
        // `Origin`, which a script can set.
        let channel = if parts
            .extensions
            .get::<crate::core::bridge_token::BridgeCaller>()
            .is_some()
            || parts
                .extensions
                .get::<crate::core::bridge_token::AgentMarker>()
                .is_some()
        {
            Channel::Bridge
        } else if parts.headers.contains_key(axum::http::header::ORIGIN)
            || parts.headers.contains_key("sec-fetch-site")
        {
            Channel::Ui
        } else {
            Channel::Http
        };
        let header = parts
            .headers
            .get(CONFIRM_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|code| !code.is_empty())
            .map(str::to_owned);
        Ok(Self { channel, header })
    }
}

/// A scope's own setting and the level in force there.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrustScopeView {
    pub setting: TrustSetting,
    pub effective: EffectiveTrust,
}

fn refused<T: Serialize>(status: StatusCode, code: ApiErrorCode, message: &str) -> Reply<T> {
    (
        status,
        Json(ApiResponse::err_coded(code, message.to_owned())),
    )
}

fn internal<T: Serialize>(error: anyhow::Error) -> Reply<T> {
    refused(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        &error.to_string(),
    )
}

/// The routes that hand out secret material (the seed, the recovery codes)
/// never answer a bridge token: the reply would reach a model. The gate in
/// front of the router already refuses it; this is the same refusal where the
/// decision is made.
fn refuse_secret_to_agent<T: Serialize>(confirm: &Confirm) -> Option<Reply<T>> {
    confirm.is_bridge().then(|| {
        refused(
            StatusCode::FORBIDDEN,
            ApiErrorCode::Validation,
            "the authenticator seed and recovery codes are never handed to an agent: enrol from the Kronn screen",
        )
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The installation key and the data directory the confirmation lives in.
#[allow(clippy::result_large_err)]
async fn proof_store<T: Serialize>(
    state: &AppState,
) -> Result<([u8; 32], std::path::PathBuf), Reply<T>> {
    let unavailable = || {
        refused(
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

fn proof_refusal<T: Serialize>(error: ProofError) -> Reply<T> {
    let (status, code) = match &error {
        ProofError::NotEnrolled | ProofError::Invalid => {
            (StatusCode::FORBIDDEN, ApiErrorCode::Validation)
        }
        ProofError::Locked(_) | ProofError::NowLocked(_) | ProofError::RecoveryRefused(_) => {
            (StatusCode::TOO_MANY_REQUESTS, ApiErrorCode::Validation)
        }
        ProofError::PendingEnrolment => (StatusCode::CONFLICT, ApiErrorCode::Validation),
        ProofError::Storage(_) => (StatusCode::INTERNAL_SERVER_ERROR, ApiErrorCode::Internal),
    };
    refused(status, code, &error.to_string())
}

/// Check a proof of the strong confirmation, off the async worker (it reads and
/// writes the record). The code comes from the body field or the
/// `X-Kronn-Confirm` header, whatever the channel; it is spent on this route and
/// object only, and the use or the refusal is journalled with the channel.
#[allow(clippy::result_large_err)]
async fn require_proof<T: Serialize>(
    state: &AppState,
    confirm: &Confirm,
    body_proof: Option<String>,
    route: &str,
    object: &str,
) -> Result<(), Reply<T>> {
    let (key, dir) = proof_store::<T>(state).await?;
    let code = confirm.code(body_proof).unwrap_or_default();
    let binding = Binding {
        channel: confirm.channel,
        route: route.to_owned(),
        object: object.to_owned(),
    };
    let spent_on = binding.clone();
    let checked = tokio::task::spawn_blocking(move || {
        trust_proof::check_bound(&dir, &key, &code, now_secs(), &spent_on)
    })
    .await
    .map_err(|error| {
        refused(
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
        // A guess at a recovery code during a lockout is no free try: it counts
        // toward the daily cap and leaves a line.
        Err(ProofError::RecoveryRefused(_)) => Some((
            false,
            "wrong recovery code while locked (counted toward the daily cap)".to_owned(),
        )),
        Err(_) => None,
    };
    if let Some((accepted, reason)) = note {
        if let Err(error) = state
            .db
            .with_conn(move |conn| trust::record_proof(conn, accepted, &binding, &reason))
            .await
        {
            tracing::warn!(%error, "the confirmation could not be journalled");
        }
    }
    checked.map_err(proof_refusal)
}

/// The object a trust call acts on, as the proof is bound to it.
fn object_of(scope: TrustScope, scope_id: Option<&str>) -> String {
    match scope {
        TrustScope::Instance => "instance".to_owned(),
        _ => format!("{}:{}", scope.as_str(), scope_id.unwrap_or_default()),
    }
}

/// Who changes the setting, as this instance knows its human: never a name the
/// request body supplies.
async fn human(state: &AppState) -> String {
    state
        .config
        .read()
        .await
        .server
        .pseudo
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| "human".into())
}

/// GET /api/trust — the instance level and the policy table the risk panel lists.
pub async fn overview(State(state): State<AppState>) -> Reply<TrustOverview> {
    let instance = match state
        .db
        .with_read_conn(|conn| trust::get_setting(conn, TrustScope::Instance, None))
        .await
    {
        Ok(instance) => instance,
        Err(error) => return internal(error),
    };
    let proof = proof_view().await;
    (
        StatusCode::OK,
        Json(ApiResponse::ok(TrustOverview {
            instance,
            effects: crate::core::trust::effects(),
            never_covered: crate::models::NeverCovered::ALL.to_vec(),
            sandbox: sandbox_view().await,
            proof,
        })),
    )
}

/// The vault protection as the screen shows it: the host, the setting, and the
/// sentence naming what is missing.
async fn sandbox_view() -> TrustSandboxView {
    let host = tokio::task::spawn_blocking(crate::core::trust_sandbox::host_sandbox)
        .await
        .unwrap_or_else(|error| Err(error.to_string()));
    let protect_vault = crate::core::trust_sandbox::protect_vault_setting();
    let protection = match &host {
        Err(reason) if protect_vault => {
            crate::core::trust_sandbox::VaultProtection::Unavailable(reason.clone())
        }
        _ if !protect_vault => crate::core::trust_sandbox::VaultProtection::Disabled,
        _ => crate::core::trust_sandbox::VaultProtection::Active,
    };
    TrustSandboxView {
        available: host.is_ok(),
        reason: host.err(),
        protect_vault,
        protected: protection.is_active(),
        missing: protection.missing(),
    }
}

/// The confirmation's state as the screen shows it. An unreadable record reads
/// as not enrolled: nothing can then be raised.
async fn proof_view() -> TrustProofView {
    let status = tokio::task::spawn_blocking(|| {
        let dir =
            crate::core::config::config_dir().map_err(|e| ProofError::Storage(e.to_string()))?;
        trust_proof::status(&dir, now_secs())
    })
    .await
    .ok()
    .and_then(Result::ok);
    match status {
        Some(status) => TrustProofView {
            enrolled: status.enrolled,
            pending: status.pending,
            recovery_codes_left: status.recovery_codes_left as u32,
            locked_for_secs: status.locked_for_secs as u32,
        },
        None => TrustProofView {
            enrolled: false,
            pending: false,
            recovery_codes_left: 0,
            locked_for_secs: 0,
        },
    }
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub limit: Option<usize>,
}

/// GET /api/trust/audit — what the levels granted on their own, newest first.
pub async fn audit(
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
) -> Reply<Vec<TrustAuditEntry>> {
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    match state
        .db
        .with_read_conn(move |conn| trust::audit_entries(conn, limit))
        .await
    {
        Ok(entries) => (StatusCode::OK, Json(ApiResponse::ok(entries))),
        Err(error) => internal(error),
    }
}

async fn view(
    state: &AppState,
    scope: TrustScope,
    scope_id: Option<String>,
) -> Result<Option<TrustScopeView>, anyhow::Error> {
    state
        .db
        .with_read_conn(move |conn| {
            let exists = match (scope, scope_id.as_deref()) {
                (TrustScope::Instance, _) => true,
                (TrustScope::Room, Some(id)) => {
                    crate::db::discussions::get_discussion(conn, id)?.is_some()
                }
                (TrustScope::Workflow, Some(id)) => {
                    crate::db::workflows::get_workflow(conn, id)?.is_some()
                }
                _ => false,
            };
            if !exists {
                return Ok(None);
            }
            let origin = match (scope, scope_id.as_deref()) {
                (TrustScope::Room, Some(id)) => TrustOrigin::room(id),
                (TrustScope::Workflow, Some(id)) => TrustOrigin::workflow(id),
                _ => TrustOrigin::default(),
            };
            Ok(Some(TrustScopeView {
                setting: trust::get_setting(conn, scope, scope_id.as_deref())?,
                effective: trust::effective(conn, &origin)?,
            }))
        })
        .await
}

async fn get_scope(
    state: AppState,
    scope: TrustScope,
    scope_id: Option<String>,
) -> Reply<TrustScopeView> {
    match view(&state, scope, scope_id).await {
        Ok(Some(found)) => (StatusCode::OK, Json(ApiResponse::ok(found))),
        Ok(None) => refused(StatusCode::NOT_FOUND, ApiErrorCode::NotFound, "not found"),
        Err(error) => internal(error),
    }
}

/// `METHOD /pattern` of a scope's route, as a code is bound to it.
fn route_of(method: &str, scope: TrustScope) -> String {
    let pattern = match scope {
        TrustScope::Instance => "/api/trust/instance",
        TrustScope::Workflow => "/api/trust/workflows/{id}",
        TrustScope::Room => "/api/trust/rooms/{id}",
    };
    format!("{method} {pattern}")
}

async fn put_scope(
    state: AppState,
    confirm: Confirm,
    scope: TrustScope,
    scope_id: Option<String>,
    request: SetTrustRequest,
) -> Reply<TrustScopeView> {
    let in_force = match view(&state, scope, scope_id.clone()).await {
        Ok(Some(found)) => found.effective.level,
        Ok(None) => return refused(StatusCode::NOT_FOUND, ApiErrorCode::NotFound, "not found"),
        Err(error) => return internal(error),
    };
    // Choosing Trusted or Full access is what the risk panel confirms: the
    // server asks for the confirmation too, not only the screen. Lowering or
    // keeping a level asks for nothing.
    if request.level > in_force && !request.acknowledged_risks {
        return refused(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::Validation,
            "choosing Trusted or Full access needs acknowledged_risks: true — the risk panel's confirmation",
        );
    }
    let raising = request.level > in_force;
    // A host with no sandbox, or the protection turned off, does not refuse a
    // level: it asks for an acknowledgement that names what is missing.
    if raising && !request.acknowledged_vault_unprotected {
        let protection = tokio::task::spawn_blocking(crate::core::trust_sandbox::vault_protection)
            .await
            .unwrap_or_else(|error| {
                crate::core::trust_sandbox::VaultProtection::Unavailable(error.to_string())
            });
        if let Some(missing) = protection.missing() {
            return refused(
                StatusCode::BAD_REQUEST,
                ApiErrorCode::Validation,
                &format!(
                    "{missing}: choosing Trusted or Full access needs acknowledged_vault_unprotected: true"
                ),
            );
        }
    }
    // Raising needs the strong confirmation; lowering or keeping does not,
    // except on the bridge channel, which always presents a code. The proof
    // comes last: only a request that could otherwise succeed spends an attempt.
    if raising || confirm.is_bridge() {
        let object = object_of(scope, scope_id.as_deref());
        if let Err(refusal) = require_proof::<TrustScopeView>(
            &state,
            &confirm,
            request.proof.clone(),
            &route_of("PUT", scope),
            &object,
        )
        .await
        {
            return refusal;
        }
    }
    let by = human(&state).await;
    let id = scope_id.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| trust::set_level(conn, scope, id.as_deref(), request.level, &by))
        .await
    {
        return internal(error);
    }
    get_scope(state, scope, scope_id).await
}

async fn delete_scope(
    state: AppState,
    confirm: Confirm,
    scope: TrustScope,
    scope_id: Option<String>,
) -> Reply<TrustScopeView> {
    // Dropping an override falls back to the instance level. When that is higher
    // than what is in force, dropping it would raise the level without the
    // strong confirmation: it is a PUT, with the proof, not a DELETE.
    let in_force = match view(&state, scope, scope_id.clone()).await {
        Ok(Some(found)) => found.effective.level,
        Ok(None) => return refused(StatusCode::NOT_FOUND, ApiErrorCode::NotFound, "not found"),
        Err(error) => return internal(error),
    };
    let inherited = match state.db.with_read_conn(trust::instance_level).await {
        Ok(level) if scope != TrustScope::Instance => level,
        Ok(_) => TrustLevel::Standard,
        Err(error) => return internal(error),
    };
    if inherited > in_force {
        return refused(
            StatusCode::FORBIDDEN,
            ApiErrorCode::Validation,
            "following the instance level would raise this scope's level: choose it explicitly, with the strong confirmation",
        );
    }
    // Lowering needs no code, except on the bridge channel.
    if confirm.is_bridge() {
        let object = object_of(scope, scope_id.as_deref());
        if let Err(refusal) = require_proof::<TrustScopeView>(
            &state,
            &confirm,
            None,
            &route_of("DELETE", scope),
            &object,
        )
        .await
        {
            return refusal;
        }
    }
    let by = human(&state).await;
    let id = scope_id.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| trust::clear_level(conn, scope, id.as_deref(), &by).map(|_| ()))
        .await
    {
        return internal(error);
    }
    get_scope(state, scope, scope_id).await
}

/// GET /api/trust/instance
pub async fn get_instance(State(state): State<AppState>) -> Reply<TrustScopeView> {
    get_scope(state, TrustScope::Instance, None).await
}

/// PUT /api/trust/instance
pub async fn put_instance(
    State(state): State<AppState>,
    confirm: Confirm,
    Json(request): Json<SetTrustRequest>,
) -> Reply<TrustScopeView> {
    put_scope(state, confirm, TrustScope::Instance, None, request).await
}

/// GET /api/trust/workflows/{id}
pub async fn get_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Reply<TrustScopeView> {
    get_scope(state, TrustScope::Workflow, Some(id)).await
}

/// PUT /api/trust/workflows/{id}
pub async fn put_workflow(
    State(state): State<AppState>,
    confirm: Confirm,
    Path(id): Path<String>,
    Json(request): Json<SetTrustRequest>,
) -> Reply<TrustScopeView> {
    put_scope(state, confirm, TrustScope::Workflow, Some(id), request).await
}

/// DELETE /api/trust/workflows/{id} — the workflow follows the instance again.
pub async fn delete_workflow(
    State(state): State<AppState>,
    confirm: Confirm,
    Path(id): Path<String>,
) -> Reply<TrustScopeView> {
    delete_scope(state, confirm, TrustScope::Workflow, Some(id)).await
}

/// GET /api/trust/rooms/{id}
pub async fn get_room(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Reply<TrustScopeView> {
    get_scope(state, TrustScope::Room, Some(id)).await
}

/// PUT /api/trust/rooms/{id}
pub async fn put_room(
    State(state): State<AppState>,
    confirm: Confirm,
    Path(id): Path<String>,
    Json(request): Json<SetTrustRequest>,
) -> Reply<TrustScopeView> {
    put_scope(state, confirm, TrustScope::Room, Some(id), request).await
}

/// DELETE /api/trust/rooms/{id} — the room follows its workflow or the instance again.
pub async fn delete_room(
    State(state): State<AppState>,
    confirm: Confirm,
    Path(id): Path<String>,
) -> Reply<TrustScopeView> {
    delete_scope(state, confirm, TrustScope::Room, Some(id)).await
}

// ── the strong confirmation ────────────────────────────────────────────────

/// GET /api/trust/proof
pub async fn get_proof() -> Reply<TrustProofView> {
    (StatusCode::OK, Json(ApiResponse::ok(proof_view().await)))
}

/// While nothing is enrolled, a code proves nothing yet: whoever enrols owns
/// the proof. The root of trust is then the human-admin secret, a file in the
/// data directory that a level-granted command cannot read (the vault sandbox)
/// and that the owner can. An agent's token-less loopback `curl` has neither
/// the file nor, therefore, the right to enrol. Journalled when accepted; a
/// refusal is not (a flood would fill the journal).
#[allow(clippy::result_large_err)]
async fn require_admin_secret<T: Serialize>(
    state: &AppState,
    confirm: &Confirm,
    presented: Option<String>,
    route: &str,
) -> Result<(), Reply<T>> {
    let presented = presented.map(|secret| secret.trim().to_owned());
    let authenticated = match presented.filter(|secret| !secret.is_empty()) {
        Some(secret) => state
            .db
            .with_conn(move |conn| {
                crate::db::human_credentials::admin_secret_authenticates(
                    conn,
                    &crate::db::human_credentials::Secret::new(secret),
                )
            })
            .await
            .map_err(internal)?,
        None => false,
    };
    if !authenticated {
        return Err(refused(
            StatusCode::FORBIDDEN,
            ApiErrorCode::Validation,
            "the first enrolment needs the Kronn admin secret: paste the content of the human-admin-secret file from the Kronn data directory",
        ));
    }
    let (route, channel) = (route.to_owned(), confirm.channel);
    if let Err(error) = state
        .db
        .with_conn(move |conn| trust::record_enrolment_step(conn, &route, channel))
        .await
    {
        tracing::warn!(%error, "the enrolment step could not be journalled");
    }
    Ok(())
}

/// POST /api/trust/proof/enroll — issue a seed for an authenticator. It needs
/// the admin secret (nothing proves the caller yet); replacing a confirmed
/// enrolment needs a proof of the current one as well. A pending enrolment is
/// not replaced for a few minutes: whoever started it keeps it.
pub async fn enroll_proof(
    State(state): State<AppState>,
    confirm: Confirm,
    Json(request): Json<TrustProofRequest>,
) -> Reply<TrustEnrolment> {
    if let Some(refusal) = refuse_secret_to_agent(&confirm) {
        return refusal;
    }
    let (key, dir) = match proof_store::<TrustEnrolment>(&state).await {
        Ok(store) => store,
        Err(refusal) => return refusal,
    };
    if let Err(refusal) = require_admin_secret::<TrustEnrolment>(
        &state,
        &confirm,
        request.admin_secret,
        "POST /api/trust/proof/enroll",
    )
    .await
    {
        return refusal;
    }
    let enrolled = {
        let dir = dir.clone();
        tokio::task::spawn_blocking(move || trust_proof::is_enrolled(&dir))
            .await
            .unwrap_or(Ok(false))
    };
    let replacing = match enrolled {
        Ok(true) => {
            if let Err(refusal) = require_proof::<TrustEnrolment>(
                &state,
                &confirm,
                request.proof,
                "POST /api/trust/proof/enroll",
                "proof",
            )
            .await
            {
                return refusal;
            }
            true
        }
        Ok(false) => false,
        Err(error) => return proof_refusal(error),
    };
    let account = human(&state).await;
    let started = tokio::task::spawn_blocking(move || {
        trust_proof::begin_enrolment(&dir, &key, &account, now_secs(), replacing)
    })
    .await;
    match started {
        Ok(Ok(enrolment)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(TrustEnrolment {
                secret: enrolment.secret,
                uri: enrolment.uri,
            })),
        ),
        Ok(Err(error)) => proof_refusal(error),
        Err(error) => internal(error.into()),
    }
}

/// POST /api/trust/proof/confirm — the first code of the authenticator turns
/// the enrolment on and returns the ten recovery codes, once. It needs the
/// admin secret too, or an agent could confirm the owner's pending seed and
/// read the recovery codes.
pub async fn confirm_proof(
    State(state): State<AppState>,
    confirm: Confirm,
    Json(request): Json<ConfirmTrustProofRequest>,
) -> Reply<TrustRecoveryCodes> {
    if let Some(refusal) = refuse_secret_to_agent(&confirm) {
        return refusal;
    }
    let (key, dir) = match proof_store::<TrustRecoveryCodes>(&state).await {
        Ok(store) => store,
        Err(refusal) => return refusal,
    };
    if let Err(refusal) = require_admin_secret::<TrustRecoveryCodes>(
        &state,
        &confirm,
        request.admin_secret,
        "POST /api/trust/proof/confirm",
    )
    .await
    {
        return refusal;
    }
    let confirmed = tokio::task::spawn_blocking(move || {
        trust_proof::confirm_enrolment(&dir, &key, &request.code, now_secs())
    })
    .await;
    match confirmed {
        Ok(Ok(codes)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(TrustRecoveryCodes { codes })),
        ),
        Ok(Err(error)) => proof_refusal(error),
        Err(error) => internal(error.into()),
    }
}

/// POST /api/trust/proof/recovery-codes — ten new codes; the old ones stop working.
pub async fn renew_recovery_codes(
    State(state): State<AppState>,
    confirm: Confirm,
    Json(request): Json<TrustProofRequest>,
) -> Reply<TrustRecoveryCodes> {
    if let Some(refusal) = refuse_secret_to_agent(&confirm) {
        return refusal;
    }
    if let Err(refusal) = require_proof::<TrustRecoveryCodes>(
        &state,
        &confirm,
        request.proof,
        "POST /api/trust/proof/recovery-codes",
        "proof",
    )
    .await
    {
        return refusal;
    }
    let (key, dir) = match proof_store::<TrustRecoveryCodes>(&state).await {
        Ok(store) => store,
        Err(refusal) => return refusal,
    };
    match tokio::task::spawn_blocking(move || trust_proof::regenerate_recovery(&dir, &key)).await {
        Ok(Ok(codes)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(TrustRecoveryCodes { codes })),
        ),
        Ok(Err(error)) => proof_refusal(error),
        Err(error) => internal(error.into()),
    }
}

/// POST /api/trust/proof/disable — remove the enrolment. Raising a level then
/// needs a new one. A **pending** enrolment (a seed issued, no first code yet)
/// has nothing to prove with: the owner cancels it with the admin secret
/// instead of waiting for it to expire, and an agent cannot.
pub async fn disable_proof(
    State(state): State<AppState>,
    confirm: Confirm,
    Json(request): Json<TrustProofRequest>,
) -> Reply<TrustProofView> {
    let (_, dir) = match proof_store::<TrustProofView>(&state).await {
        Ok(store) => store,
        Err(refusal) => return refusal,
    };
    let enrolled = {
        let dir = dir.clone();
        tokio::task::spawn_blocking(move || trust_proof::is_enrolled(&dir))
            .await
            .unwrap_or(Ok(false))
    };
    match enrolled {
        Ok(true) => {
            // Nothing secret comes back, so a bridge call carrying a valid code is fine.
            if let Err(refusal) = require_proof::<TrustProofView>(
                &state,
                &confirm,
                request.proof,
                "POST /api/trust/proof/disable",
                "proof",
            )
            .await
            {
                return refusal;
            }
        }
        Ok(false) => {
            if let Some(refusal) = refuse_secret_to_agent(&confirm) {
                return refusal;
            }
            if let Err(refusal) = require_admin_secret::<TrustProofView>(
                &state,
                &confirm,
                request.admin_secret,
                "POST /api/trust/proof/disable",
            )
            .await
            {
                return refusal;
            }
        }
        Err(error) => return proof_refusal(error),
    }
    match tokio::task::spawn_blocking(move || trust_proof::disable(&dir)).await {
        Ok(Ok(())) => (StatusCode::OK, Json(ApiResponse::ok(proof_view().await))),
        Ok(Err(error)) => proof_refusal(error),
        Err(error) => internal(error.into()),
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "trust_gate_tests.rs"]
mod trust_gate_tests;
