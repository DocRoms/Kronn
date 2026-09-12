//! KT-619 — the enrolment surface.
//!
//! Every route here authenticates the caller and performs its mutation **inside
//! one `with_conn` closure**. That is not tidiness: a capability resolved in one
//! closure, awaited, and reused in the next would let a revocation land in
//! between, and the mutation would proceed on an authority that no longer
//! exists.
//!
//! The DB functions these routes call are `pub` so tests can reach them. They
//! are not endpoints, and nothing outside this module should treat them as
//! authorised on their own.

use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use ts_rs::TS;

use crate::db::human_credentials::{self, GrantRole, HumanCredential, Secret};
use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

/// The presented secret never appears in a URL or a query string — a path is
/// logged by every proxy in the world, and a body is not.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct EnrolRequest {
    /// The admin secret, or a live credential whose role is `human`.
    pub authority: String,
    pub role: GrantRole,
    pub label: String,
}

/// Carries the plaintext exactly once, in the response to the request that
/// created it. No route returns it a second time, and it is stored only as a
/// hash.
#[derive(serde::Serialize, TS)]
#[ts(export)]
pub struct EnrolResponse {
    pub credential: HumanCredential,
    pub secret: String,
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct RevokeRequest {
    pub authority: String,
    pub credential_id: String,
    pub reason: String,
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct RotateRequest {
    pub authority: String,
    pub credential_id: String,
}

/// One refusal for every way authority can fail.
///
/// The status carries the meaning; the coded category stays `Validation`
/// because the shared enum has no `Forbidden` and this lot is not the place to
/// widen it. Telling a caller WHICH guard it tripped would tell it what to try
/// next, so the message never says.
fn refused<T: serde::Serialize>() -> (StatusCode, Json<ApiResponse<T>>) {
    (
        StatusCode::FORBIDDEN,
        Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "This request was not authorised to administer publication credentials.",
        )),
    )
}

/// `POST /api/human-credentials/enrol`
pub async fn enrol(
    State(state): State<AppState>,
    Json(request): Json<EnrolRequest>,
) -> (StatusCode, Json<ApiResponse<EnrolResponse>>) {
    let result = state
        .db
        .with_conn(move |conn| {
            // Authorise and mutate in ONE transaction. Splitting them across an
            // await is what would let a revocation slip between the two.
            let transaction = conn.unchecked_transaction()?;
            let authority = Secret::new(request.authority);
            let Some(authority) = human_credentials::authorise_enrolment(&transaction, &authority)?
            else {
                return Ok(None);
            };
            let enrolled =
                human_credentials::enrol(&transaction, &authority, request.role, &request.label)?;
            transaction.commit()?;
            Ok(Some(enrolled))
        })
        .await;

    match result {
        Ok(Some((credential, secret))) => (
            StatusCode::OK,
            Json(ApiResponse::ok(EnrolResponse {
                credential,
                // The only place this ever travels.
                secret: secret.expose().to_string(),
            })),
        ),
        Ok(None) => refused(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                error.to_string(),
            )),
        ),
    }
}

/// `POST /api/human-credentials/revoke`
pub async fn revoke(
    State(state): State<AppState>,
    Json(request): Json<RevokeRequest>,
) -> (StatusCode, Json<ApiResponse<bool>>) {
    let result = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            let authority = Secret::new(request.authority);
            // Revocation is administration, so it needs the same authority as
            // enrolment: admin or a live `human`. An orchestrator cannot revoke
            // the credential that could revoke it.
            if human_credentials::authorise_enrolment(&transaction, &authority)?.is_none() {
                return Ok(None);
            }
            let revoked =
                human_credentials::revoke(&transaction, &request.credential_id, &request.reason)?;
            transaction.commit()?;
            Ok(Some(revoked))
        })
        .await;

    match result {
        Ok(Some(revoked)) => (StatusCode::OK, Json(ApiResponse::ok(revoked))),
        Ok(None) => refused(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}

/// `POST /api/human-credentials/rotate`
///
/// Rotation proves possession of the current secret OR carries an administering
/// authority. It never changes a role: the row keeps the one it was enrolled
/// with, so possession cannot become a path to privilege.
pub async fn rotate(
    State(state): State<AppState>,
    Json(request): Json<RotateRequest>,
) -> (StatusCode, Json<ApiResponse<String>>) {
    let result = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            let presented = Secret::new(request.authority);

            let administers =
                human_credentials::authorise_enrolment(&transaction, &presented)?.is_some();
            // Or the holder rotating its own secret, which needs no
            // administering authority — but cannot reach anyone else's.
            let owns_it = matches!(
                human_credentials::authenticate(&transaction, &presented),
                Ok((id, _role, _epoch)) if id == request.credential_id
            );
            if !administers && !owns_it {
                return Ok(None);
            }

            let rotated = human_credentials::rotate_grant(&transaction, &request.credential_id)?;
            transaction.commit()?;
            Ok(Some(rotated))
        })
        .await;

    match result {
        Ok(Some(secret)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(secret.expose().to_string())),
        ),
        Ok(None) => refused(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                error.to_string(),
            )),
        ),
    }
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct RotateAdminRequest {
    /// The CURRENT admin secret. Nothing else opens this door.
    pub authority: String,
}

/// A path, never a secret. The new plaintext goes to the operator's private
/// file and travels over no wire — an API that could hand back the admin secret
/// would be the hole this lot exists to close.
#[derive(serde::Serialize, TS)]
#[ts(export)]
pub struct RotateAdminResponse {
    pub path: String,
}

/// `POST /api/human-credentials/admin/rotate`
///
/// Rotation for an operator who still HOLDS the secret: no restart, no shell.
/// The one who lost it takes the other door — a `recover-admin-secret` file in
/// the private directory, honoured at boot — because proving you are the
/// operator when you can no longer authenticate is a filesystem question, not
/// an HTTP one.
pub async fn rotate_admin(
    State(state): State<AppState>,
    Json(request): Json<RotateAdminRequest>,
) -> (StatusCode, Json<ApiResponse<RotateAdminResponse>>) {
    let result = state
        .db
        .with_conn(move |conn| {
            let presented = Secret::new(request.authority);
            // Deliberately NOT `authorise_enrolment`: that also accepts a live
            // `human` grant, and a grant the operator enrolled must not be able
            // to rotate the operator out of their own install.
            if !human_credentials::admin_secret_authenticates(conn, &presented)? {
                return Ok(None);
            }
            // No outer transaction here, unlike every other route in this file:
            // `rotate` owns one, and opening a second around it is the nested
            // BEGIN that breaks the whole call. The reason the others hold one —
            // a revocation landing between the check and the mutation — has no
            // equivalent for a singleton that nothing revokes; two concurrent
            // rotations simply leave the later one's secret in the file.
            crate::core::operator_secret::rotate(conn)
                .map(|delivered| Some(delivered.path.to_string_lossy().into_owned()))
        })
        .await;

    match result {
        Ok(Some(path)) => (
            StatusCode::OK,
            Json(ApiResponse::ok(RotateAdminResponse { path })),
        ),
        Ok(None) => refused(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct ListRequest {
    pub authority: String,
}

/// `POST /api/human-credentials/list`
///
/// A POST, and authenticated, for two reasons the review made plain.
///
/// Credential metadata is an **administration surface**, not public data: which
/// grants exist, which roles they carry and which are revoked tells an attacker
/// what to aim at. And the authority travels in the body because a URL is
/// logged by every proxy between here and the client — which is why this cannot
/// be the GET it started as.
///
/// Labels, roles and dates only. No hash and no plaintext: a hash is enough to
/// verify a guess, so it does not travel either.
pub async fn list(
    State(state): State<AppState>,
    Json(request): Json<ListRequest>,
) -> (StatusCode, Json<ApiResponse<Vec<HumanCredential>>>) {
    let result = state
        .db
        .with_conn(move |conn| {
            let authority = Secret::new(request.authority);
            if human_credentials::authorise_enrolment(conn, &authority)?.is_none() {
                return Ok(None);
            }
            human_credentials::list(conn).map(Some)
        })
        .await;

    match result {
        Ok(Some(credentials)) => (StatusCode::OK, Json(ApiResponse::ok(credentials))),
        Ok(None) => refused(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct IssueProofRequest {
    pub grant: String,
    pub discussion_id: String,
    /// The exact message body about to be posted. Hashed here, so the proof
    /// cannot be issued for one card and spent on another.
    pub content: String,
}

/// `POST /api/human-credentials/proof`
///
/// A publication is two steps on purpose. Ask for a proof over the body you are
/// about to post, then post it: a captured append cannot be replayed, because
/// the proof it carried is spent, and a captured proof cannot be aimed
/// elsewhere, because it is bound to this room and this body.
pub async fn issue_proof(
    State(state): State<AppState>,
    Json(request): Json<IssueProofRequest>,
) -> (StatusCode, Json<ApiResponse<String>>) {
    let result = state
        .db
        .with_conn(move |conn| {
            let grant = Secret::new(request.grant);
            Ok(human_credentials::issue_proof(
                conn,
                &grant,
                &request.discussion_id,
                &request.content,
                chrono::Utc::now(),
            )
            .ok())
        })
        .await;

    match result {
        Ok(Some(proof)) => (StatusCode::OK, Json(ApiResponse::ok(proof))),
        // A grant that authenticates nothing, or one revoked since. Same
        // refusal either way.
        Ok(None) => refused(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}
