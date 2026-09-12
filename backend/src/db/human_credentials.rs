//! KT-619 volet B — authenticated authority for a human-published card.
//!
//! Decided 2026-09-09 (`kt619-human-credential-bootstrap` →
//! `dedicated-human-credential`).
//!
//! **What this proves:** the caller holds a credential enrolled by someone who
//! already held one, and the publication it is making is the one the server
//! issued a proof for.
//!
//! **What it does not:** physical presence. And by the decided scope, neither
//! OS-level theft of the operator's secret nor a direct write to this database
//! is defended against. Nothing here should be read as claiming otherwise.
//!
//! Every plaintext is shown once and stored only as a hash.

use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// How long a publication proof stays usable. Short on purpose: a proof is
/// meant to be spent by the request that asked for it, not carried around.
const PROOF_TTL_SECONDS: i64 = 120;

use crate::db::discussion_sessions::sha256_hex;

#[cfg(test)]
mod tests;

/// A secret in transit. Redacting `Debug`, no `Serialize`: a struct holding one
/// cannot leak it through a tracing line or an API response by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Named so every read is greppable.
    pub fn expose(&self) -> &str {
        &self.0
    }

    fn hash(&self) -> String {
        sha256_hex(&self.0)
    }
}

/// What a grant may do. Fixed at enrolment, never derived afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GrantRole {
    /// Publishes human cards, and administers credentials.
    Human,
    /// Publishes orchestrator cards. Administers nothing — that is the whole
    /// difference, and it is why an orchestrator cannot mint a human.
    Orchestrator,
}

impl GrantRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Orchestrator => "orchestrator",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "human" => Self::Human,
            "orchestrator" => Self::Orchestrator,
            _ => return None,
        })
    }
}

/// Who authorised an enrolment. There is deliberately no anonymous variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EnrolledBy {
    Admin,
    Human,
}

impl EnrolledBy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Human => "human",
        }
    }
}

/// A credential as the UI may see it. No hash, no plaintext — a list of these
/// is safe to return.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HumanCredential {
    pub id: String,
    pub label: String,
    pub role: GrantRole,
    pub enrolled_by: EnrolledBy,
    pub created_at: String,
    pub revoked_at: Option<String>,
    pub revoked_reason: Option<String>,
}

/// Why an authority check failed. Deliberately coarse at the API boundary: a
/// caller learns that it may not publish, not which guard it tripped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityError {
    /// No credential, or one that authenticates nothing.
    Unknown,
    /// Authenticated once, but revoked since.
    Revoked,
}

/// Mint the bootstrap secret, exactly once.
///
/// Returns the plaintext for the CALLER to write to operator private storage,
/// and keeps only its hash. A second call finds the row and refuses: a second
/// admin secret alongside the first would silently authorise enrolments the
/// operator never sanctioned.
pub fn create_admin_secret(conn: &Connection, delivered_to: &str) -> Result<Secret> {
    if admin_secret_exists(conn)? {
        bail!("the admin secret already exists; rotate it rather than minting a second");
    }
    let secret = Secret::new(format!("kr-admin-{}", Uuid::new_v4().simple()));
    conn.execute(
        "INSERT INTO human_admin_secret (singleton, secret_hash, delivered_to, created_at) \
         VALUES (1, ?1, ?2, ?3)",
        params![secret.hash(), delivered_to, Utc::now().to_rfc3339()],
    )?;
    Ok(secret)
}

pub fn admin_secret_exists(conn: &Connection) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM human_admin_secret WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some())
}

/// Where the operator was told to find the secret. A path, never the secret.
pub fn admin_secret_location(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT delivered_to FROM human_admin_secret WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

fn admin_secret_matches(conn: &Connection, secret: &Secret) -> Result<bool> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT secret_hash FROM human_admin_secret WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    // Absent bootstrap authenticates nothing — it must not read as "no check".
    Ok(stored.is_some_and(|hash| hash == secret.hash()))
}

/// Does this secret authenticate as the ADMIN, and only the admin?
///
/// Deliberately not [`authorise_enrolment`], which also accepts a live `human`
/// grant. Administering credentials and rotating the secret that governs them
/// are different powers: a grant the operator enrolled must not be able to
/// rotate the operator out of their own install.
pub fn admin_secret_authenticates(conn: &Connection, presented: &Secret) -> Result<bool> {
    admin_secret_matches(conn, presented)
}

/// Rotate the bootstrap secret. Every credential it enrolled keeps working;
/// only the ability to enrol NEW ones moves to the new secret.
pub fn rotate_admin_secret(conn: &Connection, delivered_to: &str) -> Result<Secret> {
    if !admin_secret_exists(conn)? {
        bail!("no admin secret to rotate");
    }
    let secret = Secret::new(format!("kr-admin-{}", Uuid::new_v4().simple()));
    conn.execute(
        "UPDATE human_admin_secret \
            SET secret_hash = ?1, delivered_to = ?2, rotated_at = ?3 \
          WHERE singleton = 1",
        params![secret.hash(), delivered_to, Utc::now().to_rfc3339()],
    )?;
    Ok(secret)
}

/// Proof that the caller may enrol. Constructible only by [`authorise_enrolment`],
/// so a route cannot enrol by forgetting to check.
pub struct EnrolmentAuthority(EnrolledBy);

/// Authorise an enrolment by the admin secret, or by a live grant whose role is
/// **`Human`**.
///
/// The first version accepted any live credential. That was a defect, not a
/// simplification: it let an `orchestrator` — which exists only to publish —
/// mint credentials, including `human` ones, so the weaker role could reach the
/// stronger in one step.
///
/// There is no third way in and no "first caller" case: an install with no
/// admin secret and no human grant authorises nobody.
pub fn authorise_enrolment(
    conn: &Connection,
    presented: &Secret,
) -> Result<Option<EnrolmentAuthority>> {
    if admin_secret_matches(conn, presented)? {
        return Ok(Some(EnrolmentAuthority(EnrolledBy::Admin)));
    }
    match authenticate(conn, presented) {
        Ok((_, GrantRole::Human, _)) => Ok(Some(EnrolmentAuthority(EnrolledBy::Human))),
        // Explicitly including a live `orchestrator`: administering is not
        // among the things it may do.
        Ok((_, GrantRole::Orchestrator, _)) | Err(_) => Ok(None),
    }
}

/// Enrol a new human credential. Returns its plaintext ONCE.
pub fn enrol(
    conn: &Connection,
    authority: &EnrolmentAuthority,
    role: GrantRole,
    label: &str,
) -> Result<(HumanCredential, Secret)> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 100 {
        bail!("a credential label must be non-empty and at most 100 characters");
    }
    let secret = Secret::new(format!("kr-human-{}", Uuid::new_v4().simple()));
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO human_credentials \
             (id, label, role, secret_hash, enrolled_by, created_at, epoch) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
        params![
            id,
            label,
            role.as_str(),
            secret.hash(),
            authority.0.as_str(),
            now
        ],
    )?;
    Ok((
        HumanCredential {
            id,
            label: label.to_string(),
            role,
            enrolled_by: authority.0,
            created_at: now,
            revoked_at: None,
            revoked_reason: None,
        },
        secret,
    ))
}

/// The live grant a secret authenticates: its id, role and epoch — or why it
/// authenticates nothing.
pub fn authenticate(
    conn: &Connection,
    presented: &Secret,
) -> std::result::Result<(String, GrantRole, i64), AuthorityError> {
    if presented.expose().is_empty() {
        return Err(AuthorityError::Unknown);
    }
    let row: Option<(String, String, Option<String>, i64)> = conn
        .query_row(
            "SELECT id, role, revoked_at, epoch FROM human_credentials WHERE secret_hash = ?1",
            params![presented.hash()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|_| AuthorityError::Unknown)?;
    match row {
        None => Err(AuthorityError::Unknown),
        Some((_, _, Some(_revoked), _)) => Err(AuthorityError::Revoked),
        // An unparsable role is a contract break, not a permissive default.
        Some((id, role, None, epoch)) => match GrantRole::parse(&role) {
            Some(role) => Ok((id, role, epoch)),
            None => Err(AuthorityError::Unknown),
        },
    }
}

/// Revoke a credential and invalidate every proof it had in flight.
///
/// The epoch bump is what makes invalidation immediate: proofs carry the epoch
/// they were issued under, so none has to be found and deleted.
pub fn revoke(conn: &Connection, credential_id: &str, reason: &str) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE human_credentials \
            SET revoked_at = ?2, revoked_reason = ?3, epoch = epoch + 1 \
          WHERE id = ?1 AND revoked_at IS NULL",
        params![credential_id, Utc::now().to_rfc3339(), reason],
    )?;
    Ok(changed == 1)
}

/// Rotate a grant's secret in place, keeping its identity, role and history.
///
/// The role is deliberately absent from the UPDATE: rotation proves possession
/// of the current secret, and possession must never be a path to a role change.
/// Proofs issued under the old secret die with the epoch bump.
pub fn rotate_grant(conn: &Connection, credential_id: &str) -> Result<Secret> {
    let secret = Secret::new(format!("kr-human-{}", Uuid::new_v4().simple()));
    let changed = conn.execute(
        "UPDATE human_credentials SET secret_hash = ?2, epoch = epoch + 1 \
          WHERE id = ?1 AND revoked_at IS NULL",
        params![credential_id, secret.hash()],
    )?;
    if changed != 1 {
        bail!("no live credential to rotate");
    }
    Ok(secret)
}

pub fn list(conn: &Connection) -> Result<Vec<HumanCredential>> {
    let mut statement = conn.prepare(
        "SELECT id, label, role, enrolled_by, created_at, revoked_at, revoked_reason \
           FROM human_credentials ORDER BY created_at, id",
    )?;
    let rows = statement.query_map([], |row| {
        let role: String = row.get(2)?;
        let enrolled: String = row.get(3)?;
        Ok(HumanCredential {
            id: row.get(0)?,
            label: row.get(1)?,
            role: GrantRole::parse(&role).unwrap_or(GrantRole::Orchestrator),
            enrolled_by: match enrolled.as_str() {
                "admin" => EnrolledBy::Admin,
                _ => EnrolledBy::Human,
            },
            created_at: row.get(4)?,
            revoked_at: row.get(5)?,
            revoked_reason: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Issue a single-use proof for one card in one room.
///
/// The content hash is taken here rather than trusted from the caller, so the
/// proof cannot be issued for one body and spent on another.
pub fn issue_proof(
    conn: &Connection,
    presented: &Secret,
    discussion_id: &str,
    content: &str,
    now: DateTime<Utc>,
) -> std::result::Result<String, AuthorityError> {
    let (credential_id, _role, epoch) = authenticate(conn, presented)?;
    let id = format!("proof-{}", Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO human_publication_proofs \
             (id, credential_id, discussion_id, content_hash, issued_at, expires_at, credential_epoch) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            credential_id,
            discussion_id,
            sha256_hex(content),
            now.to_rfc3339(),
            (now + Duration::seconds(PROOF_TTL_SECONDS)).to_rfc3339(),
            epoch,
        ],
    )
    .map_err(|_| AuthorityError::Unknown)?;
    Ok(id)
}

/// Why a proof was not accepted. Kept apart from [`AuthorityError`] because
/// "you may not publish" and "this proof is spent" are different facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofError {
    Unknown,
    Expired,
    AlreadyUsed,
    WrongDiscussion,
    ContentChanged,
    CredentialChanged,
}

/// Spend a proof for exactly this room and this body.
///
/// The consumption is an `UPDATE … WHERE consumed_at IS NULL` believed only
/// when it reports one changed row: two concurrent callers holding the same
/// proof must not both publish, and a read-then-write would let them.
pub fn consume_proof(
    conn: &Connection,
    proof_id: &str,
    presented: &Secret,
    discussion_id: &str,
    content: &str,
    now: DateTime<Utc>,
) -> std::result::Result<String, ProofError> {
    let (credential_id, _role, epoch) =
        authenticate(conn, presented).map_err(|_| ProofError::Unknown)?;

    let row: Option<(String, String, String, String, Option<String>, i64)> = conn
        .query_row(
            "SELECT credential_id, discussion_id, content_hash, expires_at, consumed_at, \
                    credential_epoch \
               FROM human_publication_proofs WHERE id = ?1",
            params![proof_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|_| ProofError::Unknown)?;

    let Some((owner, room, content_hash, expires_at, consumed_at, proof_epoch)) = row else {
        return Err(ProofError::Unknown);
    };
    // A proof belonging to someone else is `Unknown`, not `WrongDiscussion`:
    // the caller learns nothing about a credential that is not its own.
    if owner != credential_id {
        return Err(ProofError::Unknown);
    }
    if consumed_at.is_some() {
        return Err(ProofError::AlreadyUsed);
    }
    if proof_epoch != epoch {
        return Err(ProofError::CredentialChanged);
    }
    if room != discussion_id {
        return Err(ProofError::WrongDiscussion);
    }
    if content_hash != sha256_hex(content) {
        return Err(ProofError::ContentChanged);
    }
    match DateTime::parse_from_rfc3339(&expires_at) {
        Ok(expiry) if now <= expiry.with_timezone(&Utc) => {}
        // An unparsable expiry is a broken row, not an open door.
        _ => return Err(ProofError::Expired),
    }

    let spent = conn
        .execute(
            "UPDATE human_publication_proofs SET consumed_at = ?2 \
              WHERE id = ?1 AND consumed_at IS NULL",
            params![proof_id, now.to_rfc3339()],
        )
        .map_err(|_| ProofError::Unknown)?;
    if spent != 1 {
        // Another caller spent it between the read above and this write.
        return Err(ProofError::AlreadyUsed);
    }
    Ok(credential_id)
}
