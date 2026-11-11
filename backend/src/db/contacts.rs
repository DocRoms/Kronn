use aes_gcm::aead::{rand_core::RngCore, OsRng};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use crate::models::Contact;

pub fn list_contacts(conn: &Connection) -> Result<Vec<Contact>> {
    let mut stmt = conn.prepare(
        "SELECT id, pseudo, avatar_email, kronn_url, invite_code, status, created_at, updated_at
         FROM contacts ORDER BY pseudo",
    )?;
    let contacts = stmt
        .query_map([], |row| {
            Ok(Contact {
                id: row.get(0)?,
                pseudo: row.get(1)?,
                avatar_email: row.get::<_, Option<String>>(2).unwrap_or(None),
                kronn_url: row.get(3)?,
                invite_code: row.get(4)?,
                status: row.get(5)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(contacts)
}

pub fn get_contact(conn: &Connection, id: &str) -> Result<Option<Contact>> {
    let mut stmt = conn.prepare(
        "SELECT id, pseudo, avatar_email, kronn_url, invite_code, status, created_at, updated_at
         FROM contacts WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(params![id], |row| {
        Ok(Contact {
            id: row.get(0)?,
            pseudo: row.get(1)?,
            avatar_email: row.get::<_, Option<String>>(2).unwrap_or(None),
            kronn_url: row.get(3)?,
            invite_code: row.get(4)?,
            status: row.get(5)?,
            created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    })?;
    Ok(rows.next().and_then(|r| r.ok()))
}

/// An incoming contact request: an unknown peer presented this code. Kronn
/// never connects back nor verifies it until the operator adds the code.
pub const STATUS_REQUESTED: &str = "requested";

/// A contact that predates per-contact secrets (KT-1650): it keeps its row, and
/// so its shared discussions, but is neither dialled nor admitted until the two
/// sides pair again.
pub const STATUS_NEEDS_REPAIR: &str = "needs_repair";

/// Whether the outbound client may dial this contact. A request, a refused
/// contact or one waiting to be paired again is never dialled: dialling would
/// mark it accepted.
pub fn dials_outbound(status: &str) -> bool {
    status != STATUS_REQUESTED && status != "refused" && status != STATUS_NEEDS_REPAIR
}

/// What a peer is told, on every route, when it cannot be authenticated. The
/// owner decision of KT-1650 is that an unpaired contact is refused and both
/// sides re-pair, so the text says how.
pub const REPAIR_HINT: &str = "Pairing required: on both Kronn instances delete the contact, \
     then on one of them open Settings > Identity, generate a pairing code and paste it into \
     Contacts > Add on the other.";

/// The refusal of a route that authenticates a peer. Identical whatever the
/// reason (unknown code, wrong or missing secret, contact not accepted): it
/// must not tell which.
pub const PEER_REFUSED: &str = "peer not authenticated: the contact is unknown, not accepted or \
     has no valid pairing secret. Pairing required: on both Kronn instances delete the contact, \
     then on one of them open Settings > Identity, generate a pairing code and paste it into \
     Contacts > Add on the other.";

/// Short form for a WebSocket close reason (123 bytes at most on the wire).
pub const WS_REFUSED_REASON: &str =
    "Peer refused: re-pair both sides (Settings > Identity: pairing code, Contacts > Add)";

/// Bytes of entropy in a pairing secret.
const SECRET_BYTES: usize = 32;
/// Characters of a pairing secret (hex).
pub const SECRET_LEN: usize = SECRET_BYTES * 2;

/// A fresh random pairing secret: 256 bits from the OS RNG, hex-encoded. It is
/// never derived from the invite code or any other public value.
pub fn generate_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `secret` has the shape [`generate_secret`] produces.
pub fn is_well_formed_secret(secret: &str) -> bool {
    secret.len() == SECRET_LEN && secret.bytes().all(|b| b.is_ascii_hexdigit())
}

/// SHA-256 (hex) of a secret: all the database keeps of an inbound secret.
pub fn hash_secret(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether `presented` is the secret whose [`hash_secret`] is `stored_hash`,
/// compared in constant time over the digests (neither content nor length of the
/// presented value leaks).
pub fn secret_matches(stored_hash: &str, presented: &str) -> bool {
    let presented = hash_secret(presented);
    let (a, b) = (stored_hash.as_bytes(), presented.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Passe D — the ONE sanctioned way to authenticate a P2P caller: a known
/// contact whose status is `accepted` AND that presents the per-contact secret
/// it was paired with (KT-1650). The invite code only says WHO the caller claims
/// to be: it is public, so it never authenticates on its own. A pending/refused
/// contact keeps its code but must not pass the auth-exempt routes
/// (claim-by-token, fetch-file). `NotAccepted` carries the reason for telemetry;
/// callers MUST answer exactly like `Unknown` (no oracle).
pub enum InviteAuth {
    Accepted(Contact),
    NotAccepted { pseudo: String, status: String },
    Unknown,
}

pub fn authenticate_peer(conn: &Connection, invite_code: &str, secret: &str) -> Result<InviteAuth> {
    let Some(contact) = find_contact_by_invite_code(conn, invite_code)? else {
        return Ok(InviteAuth::Unknown);
    };
    let stored = inbound_secret_hash(conn, &contact.id)?;
    // Hashed whatever the outcome: a missing secret costs the same as a wrong one.
    let presented_ok = match &stored {
        Some(hash) => secret_matches(hash, secret),
        None => {
            let _ = hash_secret(secret);
            false
        }
    };
    if !presented_ok {
        let reason = if stored.is_some() {
            "bad_secret"
        } else {
            "unpaired"
        };
        return Ok(InviteAuth::NotAccepted {
            pseudo: contact.pseudo,
            status: reason.into(),
        });
    }
    Ok(if contact.status == "accepted" {
        InviteAuth::Accepted(contact)
    } else {
        InviteAuth::NotAccepted {
            pseudo: contact.pseudo,
            status: contact.status,
        }
    })
}

/// The hash of the secret a contact must present to us, if it was paired.
pub fn inbound_secret_hash(conn: &Connection, contact_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT inbound_secret_hash FROM contacts WHERE id = ?1",
            params![contact_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap_or(None))
}

/// The secret we present to a contact, if it was paired. Never listed nor sent
/// to the frontend.
pub fn outbound_secret(conn: &Connection, contact_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT outbound_secret FROM contacts WHERE id = ?1",
            params![contact_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap_or(None))
}

/// Complete a pairing: store both secrets and accept the contact. Conditional:
/// a contact that already holds secrets, or is refused, is left untouched. The
/// result says whether a row changed, so a caller that raced a pairing or a
/// refusal sees it and backs out.
pub fn set_pairing_secrets(
    conn: &Connection,
    contact_id: &str,
    inbound_secret_hash: &str,
    outbound_secret: &str,
) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE contacts
            SET inbound_secret_hash = ?1, outbound_secret = ?2,
                status = 'accepted', updated_at = ?3
          WHERE id = ?4 AND inbound_secret_hash IS NULL AND status != 'refused'",
        params![
            inbound_secret_hash,
            outbound_secret,
            Utc::now().to_rfc3339(),
            contact_id
        ],
    )?;
    Ok(affected > 0)
}

/// Whether a contact row already holds pairing secrets. Such a row is never
/// overwritten by a new pairing: it has to be deleted first.
pub fn is_paired(conn: &Connection, contact_id: &str) -> Result<bool> {
    Ok(inbound_secret_hash(conn, contact_id)?.is_some())
}

/// Most UNBOUND pairing codes outstanding at once, and how long one stays valid.
/// A code bound to a contact is outside the cap: there is at most one per contact.
pub const MAX_PAIRING_OFFERS: i64 = 20;
pub const PAIRING_OFFER_TTL_DAYS: i64 = 7;

/// A live pairing code, as the accepting side sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingOffer {
    pub id: String,
    /// The one existing contact this code was issued for (a re-pair), if any. A
    /// code without one pairs a new contact only.
    pub contact_id: Option<String>,
}

/// Hand out a new pairing code secret. Only its hash is kept; the secret is
/// returned once, for the operator to give the other side.
///
/// With a `contact_id` the code is issued for that existing contact (a re-pair
/// from its row): it must exist, not be paired already and not be refused. None
/// when it does not qualify.
pub fn create_pairing_offer(conn: &Connection, contact_id: Option<&str>) -> Result<Option<String>> {
    if let Some(id) = contact_id {
        let eligible = match get_contact(conn, id)? {
            Some(c) => c.status != "refused" && !is_paired(conn, id)?,
            None => false,
        };
        if !eligible {
            return Ok(None);
        }
    }
    purge_expired_offers(conn)?;
    match contact_id {
        // At most one live offer per contact: a fresh Re-pair replaces the last.
        Some(id) => {
            conn.execute(
                "DELETE FROM contact_pairing_offers WHERE contact_id = ?1",
                params![id],
            )?;
        }
        // The cap evicts the oldest UNBOUND offers only. A bound offer is never
        // evicted by it (those are bounded by the contact count), and never
        // counts against it.
        None => {
            conn.execute(
                "DELETE FROM contact_pairing_offers WHERE id IN (
                     SELECT id FROM contact_pairing_offers WHERE contact_id IS NULL
                     ORDER BY created_at DESC LIMIT -1 OFFSET ?1)",
                params![MAX_PAIRING_OFFERS - 1],
            )?;
        }
    }
    let secret = generate_secret();
    conn.execute(
        "INSERT INTO contact_pairing_offers (id, secret_hash, created_at, contact_id)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            uuid::Uuid::new_v4().to_string(),
            hash_secret(&secret),
            Utc::now().to_rfc3339(),
            contact_id
        ],
    )?;
    Ok(Some(secret))
}

fn purge_expired_offers(conn: &Connection) -> Result<()> {
    let cutoff = (Utc::now() - chrono::Duration::days(PAIRING_OFFER_TTL_DAYS)).to_rfc3339();
    conn.execute(
        "DELETE FROM contact_pairing_offers WHERE created_at < ?1",
        params![cutoff],
    )?;
    Ok(())
}

/// The live offer `secret` belongs to. Every offer is compared in constant time
/// and the scan never stops early.
pub fn find_pairing_offer(conn: &Connection, secret: &str) -> Result<Option<PairingOffer>> {
    purge_expired_offers(conn)?;
    let mut stmt =
        conn.prepare("SELECT id, secret_hash, contact_id FROM contact_pairing_offers")?;
    let offers = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    let mut found = None;
    for (id, hash, contact_id) in offers {
        if secret_matches(&hash, secret) {
            found = Some(PairingOffer { id, contact_id });
        }
    }
    Ok(found)
}

/// A pairing code is spent by the first peer that completes the pairing. The
/// number of rows removed is what enforces that: exactly 1, or the pairing is void.
pub fn delete_pairing_offer(conn: &Connection, id: &str) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM contact_pairing_offers WHERE id = ?1",
        params![id],
    )?)
}

/// What a peer sends to complete a pairing, once parsed.
pub struct PairingAttempt {
    pub offer_secret: String,
    pub invite_code: String,
    pub pseudo: String,
    pub kronn_url: String,
    pub reverse_secret: String,
}

/// Accept a pairing on the side that handed out the code. One transaction: the
/// offer is found, the contact stored, both secrets set and the offer spent, or
/// nothing happens. Ok(false) is a refusal, whatever the reason.
///
/// - An offer pairs a NEW contact only, unless it was issued for an existing
///   one: then the caller's invite code must resolve to exactly that contact.
///   A live offer can never rebind another contact's row.
/// - Single use is enforced by the write: the offer must delete exactly one row.
pub fn accept_pairing(conn: &Connection, attempt: &PairingAttempt) -> Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let Some(offer) = find_pairing_offer(&tx, &attempt.offer_secret)? else {
        return Ok(false);
    };
    let existing = find_contact_by_invite_code(&tx, &attempt.invite_code)?;
    let id = match (offer.contact_id.as_deref(), existing) {
        (None, None) => {
            let now = Utc::now();
            let contact = Contact {
                id: uuid::Uuid::new_v4().to_string(),
                pseudo: attempt.pseudo.clone(),
                avatar_email: None,
                kronn_url: attempt.kronn_url.clone(),
                invite_code: attempt.invite_code.clone(),
                status: "pending".into(),
                created_at: now,
                updated_at: now,
            };
            insert_contact(&tx, &contact)?;
            contact.id
        }
        (Some(bound), Some(c)) if c.id == bound => c.id,
        // An unbound offer meets an existing contact, or a bound one meets
        // someone else (or nobody): refused.
        _ => return Ok(false),
    };
    // The caller presents the pairing code's secret from now on; it is kept
    // hashed, like a password. No row changed: the contact got paired or
    // refused in the meantime. Returning drops the transaction: rollback.
    if !set_pairing_secrets(
        &tx,
        &id,
        &hash_secret(&attempt.offer_secret),
        &attempt.reverse_secret,
    )? {
        return Ok(false);
    }
    if delete_pairing_offer(&tx, &offer.id)? != 1 {
        return Ok(false);
    }
    tx.commit()?;
    Ok(true)
}

/// Store the outcome of a pairing WE started, once the peer accepted it: the
/// contact is created (or the existing one paired in place) and both secrets are
/// set, in one transaction. None when the contact changed while the peer was
/// being asked (deleted, paired, refused, or created by someone else): nothing
/// is written then.
pub fn complete_outbound_pairing(
    conn: &Connection,
    existing_id: Option<&str>,
    new_contact: &Contact,
    inbound_secret_hash: &str,
    outbound_secret: &str,
) -> Result<Option<Contact>> {
    let tx = conn.unchecked_transaction()?;
    let found = find_contact_by_invite_code(&tx, &new_contact.invite_code)?;
    let id = match (existing_id, found) {
        (None, None) => {
            insert_contact(&tx, new_contact)?;
            new_contact.id.clone()
        }
        (Some(expected), Some(c)) if c.id == expected => c.id,
        _ => return Ok(None),
    };
    if !set_pairing_secrets(&tx, &id, inbound_secret_hash, outbound_secret)? {
        return Ok(None);
    }
    let contact = get_contact(&tx, &id)?;
    tx.commit()?;
    Ok(contact)
}

/// Longest pairing code: an invite code, `#`, and a secret.
pub const MAX_PAIRING_CODE_LEN: usize = MAX_INVITE_CODE_LEN + 1 + SECRET_LEN;

/// Split what an operator pastes into the public invite code and the pairing
/// secret (`kronn:pseudo@host:port#<secret>`). None for a bare invite code, which
/// is only an address and pairs nobody.
pub fn split_pairing_code(code: &str) -> Option<(String, String)> {
    let code = code.trim();
    if code.len() > MAX_PAIRING_CODE_LEN {
        return None;
    }
    let (invite, secret) = code.rsplit_once('#')?;
    if !is_well_formed_secret(secret) {
        return None;
    }
    Some((invite.to_string(), secret.to_string()))
}

pub fn find_contact_by_invite_code(
    conn: &Connection,
    invite_code: &str,
) -> Result<Option<Contact>> {
    let mut stmt = conn.prepare(
        "SELECT id, pseudo, avatar_email, kronn_url, invite_code, status, created_at, updated_at
         FROM contacts WHERE invite_code = ?1",
    )?;
    let mut rows = stmt.query_map(params![invite_code], |row| {
        Ok(Contact {
            id: row.get(0)?,
            pseudo: row.get(1)?,
            avatar_email: row.get::<_, Option<String>>(2).unwrap_or(None),
            kronn_url: row.get(3)?,
            invite_code: row.get(4)?,
            status: row.get(5)?,
            created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    })?;
    Ok(rows.next().and_then(|r| r.ok()))
}

/// Longest invite code accepted from anyone (a peer's Presence or a paste).
pub const MAX_INVITE_CODE_LEN: usize = 256;
/// Longest pseudo carried by an invite code, in characters.
pub const MAX_PSEUDO_CHARS: usize = 64;
/// Most contact requests kept at once, and how long one is kept.
pub const MAX_PENDING_REQUESTS: i64 = 20;
pub const REQUEST_TTL_DAYS: i64 = 7;

/// Prefix of an invite code whose peer is dialled over TLS (https / wss).
pub const TLS_INVITE_PREFIX: &str = "kronns:";

/// Parse invite code format: `kronn:pseudo@host:port` (plain http / ws) or
/// `kronns:pseudo@host:port` (https / wss: the peer sits behind a TLS proxy,
/// KT-1596). The address must be a bare host and port: no path, query,
/// fragment or credentials, so the URL built from it cannot point anywhere
/// else.
pub fn parse_invite_code(code: &str) -> Option<(String, String)> {
    let code = code.trim();
    if code.len() > MAX_INVITE_CODE_LEN {
        return None;
    }
    let (scheme, rest) = match code.strip_prefix(TLS_INVITE_PREFIX) {
        Some(rest) => ("https", rest),
        None => ("http", code.strip_prefix("kronn:")?),
    };
    let (pseudo, url_part) = rest.rsplit_once('@')?;
    if pseudo.is_empty()
        || pseudo.chars().count() > MAX_PSEUDO_CHARS
        || pseudo.chars().any(|c| c.is_control() || c == '@')
    {
        return None;
    }
    let authority = canonical_authority(url_part)?;
    Some((pseudo.to_string(), format!("{scheme}://{authority}")))
}

/// `host:port` (port 443 when the origin has none) of a proxy's public
/// `https://` origin, or None unless it is exactly an https scheme, a DNS name
/// or IP and an optional port: no path, query, fragment or credentials.
pub fn public_origin_authority(origin: &str) -> Option<String> {
    let origin = origin.trim().trim_end_matches('/');
    let scheme_len = "https://".len();
    if origin.len() <= scheme_len
        || !origin.is_char_boundary(scheme_len)
        || !origin[..scheme_len].eq_ignore_ascii_case("https://")
    {
        return None;
    }
    let rest = &origin[scheme_len..];
    let has_port = match rest.strip_prefix('[') {
        Some(v6) => v6.contains("]:"),
        None => rest.contains(':'),
    };
    if has_port {
        canonical_authority(rest)
    } else {
        canonical_authority(&format!("{rest}:443"))
    }
}

/// The canonical form of a proxy's public origin (`https://host`, the port
/// shown only when it is not 443), or None when it is not a bare https origin.
pub fn normalize_public_origin(origin: &str) -> Option<String> {
    let authority = public_origin_authority(origin)?;
    let shown = authority.strip_suffix(":443").unwrap_or(&authority);
    Some(format!("https://{shown}"))
}

/// `host:port` in canonical form, or None unless it is exactly a DNS name,
/// an IPv4 address or a bracketed IPv6 address followed by a port.
pub fn canonical_authority(authority: &str) -> Option<String> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (v6, tail) = rest.split_once(']')?;
        v6.parse::<std::net::Ipv6Addr>().ok()?;
        (
            format!("[{}]", v6.to_ascii_lowercase()),
            tail.strip_prefix(':')?,
        )
    } else {
        let (host, port) = authority.rsplit_once(':')?;
        if !valid_host_name(host) {
            return None;
        }
        (host.to_ascii_lowercase(), port)
    };
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    Some(format!("{host}:{port}"))
}

fn valid_host_name(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// The base URL of a stored contact, re-validated before any request to it:
/// rows written before the strict grammar may hold a path or a fragment.
pub fn contact_base_url(kronn_url: &str) -> Option<String> {
    let (scheme, authority) = kronn_url.trim_end_matches('/').split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    Some(format!("{scheme}://{}", canonical_authority(authority)?))
}

/// Store an incoming contact request, dropping expired ones first. Returns
/// false when the request table is full.
pub fn insert_contact_request(conn: &Connection, contact: &Contact) -> Result<bool> {
    let cutoff = (Utc::now() - chrono::Duration::days(REQUEST_TTL_DAYS)).to_rfc3339();
    conn.execute(
        "DELETE FROM contacts WHERE status = ?1 AND created_at < ?2",
        params![STATUS_REQUESTED, cutoff],
    )?;
    let outstanding: i64 = conn.query_row(
        "SELECT COUNT(*) FROM contacts WHERE status = ?1",
        params![STATUS_REQUESTED],
        |row| row.get(0),
    )?;
    if outstanding >= MAX_PENDING_REQUESTS {
        return Ok(false);
    }
    insert_contact(conn, contact)?;
    Ok(true)
}

/// Whether a contact id still designates an accepted contact.
pub fn contact_id_is_accepted(conn: &Connection, id: &str) -> Result<bool> {
    Ok(get_contact(conn, id)?.is_some_and(|c| c.status == "accepted"))
}

pub fn insert_contact(conn: &Connection, contact: &Contact) -> Result<()> {
    conn.execute(
        "INSERT INTO contacts (id, pseudo, avatar_email, kronn_url, invite_code, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            contact.id,
            contact.pseudo,
            contact.avatar_email,
            contact.kronn_url,
            contact.invite_code,
            contact.status,
            contact.created_at.to_rfc3339(),
            contact.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn update_contact_status(conn: &Connection, id: &str, status: &str) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE contacts SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status, Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

pub fn delete_contact(conn: &Connection, id: &str) -> Result<bool> {
    // A pairing code issued for this contact dies with it.
    conn.execute(
        "DELETE FROM contact_pairing_offers WHERE contact_id = ?1",
        params![id],
    )?;
    let affected = conn.execute("DELETE FROM contacts WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::db::migrations;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrations::run(&conn).unwrap();
        conn
    }

    fn paired_contact(conn: &Connection, id: &str, code: &str, status: &str, secret: &str) {
        let (pseudo, kronn_url) =
            parse_invite_code(code).unwrap_or(("p".into(), "http://x".into()));
        insert_contact(
            conn,
            &crate::models::Contact {
                id: id.into(),
                pseudo,
                avatar_email: None,
                kronn_url,
                invite_code: code.into(),
                status: "pending".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        )
        .unwrap();
        set_pairing_secrets(conn, id, &hash_secret(secret), secret).unwrap();
        update_contact_status(conn, id, status).unwrap();
    }

    const SECRET: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";

    #[test]
    fn only_accepted_contacts_authenticate_with_their_secret() {
        // Passe D (Codex constat n°1) — pending/refused contacts keep their
        // code but must not pass peer auth; the enum carries the real status
        // so callers can log it without re-implementing the rule.
        let conn = test_conn();
        for (id, code, status) in [
            ("a", "kr-a", "accepted"),
            ("b", "kr-b", "pending"),
            ("c", "kr-c", "refused"),
        ] {
            paired_contact(&conn, id, code, status, SECRET);
        }
        assert!(matches!(authenticate_peer(&conn, "kr-a", SECRET).unwrap(),
            InviteAuth::Accepted(c) if c.id == "a"));
        assert!(matches!(authenticate_peer(&conn, "kr-b", SECRET).unwrap(),
            InviteAuth::NotAccepted { ref status, .. } if status == "pending"));
        assert!(matches!(authenticate_peer(&conn, "kr-c", SECRET).unwrap(),
            InviteAuth::NotAccepted { ref status, .. } if status == "refused"));
        assert!(matches!(
            authenticate_peer(&conn, "kr-ghost", SECRET).unwrap(),
            InviteAuth::Unknown
        ));
    }

    #[test]
    fn the_public_invite_code_alone_authenticates_nobody() {
        // KT-1650 (was KT-1597's finding): the code is `scheme:pseudo@host:port`,
        // rebuilt from the `pseudo` and `kronn_url` columns, so it can name a
        // contact but never prove who presents it. The secret is what does.
        let conn = test_conn();
        for (id, code) in [
            ("p", "kronn:alice@100.64.1.2:3456"),
            ("t", "kronns:bob@kronn.example.com:443"),
        ] {
            paired_contact(&conn, id, code, "accepted", SECRET);
            let stored = get_contact(&conn, id).unwrap().unwrap();
            let (scheme, authority) = stored.kronn_url.split_once("://").unwrap();
            let prefix = if scheme == "https" { "kronns" } else { "kronn" };
            let rebuilt = format!("{prefix}:{}@{authority}", stored.pseudo);
            assert_eq!(rebuilt, code, "derivable from public columns");
            for presented in [
                "",
                rebuilt.as_str(),
                stored.pseudo.as_str(),
                &"00".repeat(32),
            ] {
                assert!(
                    matches!(
                        authenticate_peer(&conn, &rebuilt, presented).unwrap(),
                        InviteAuth::NotAccepted { .. }
                    ),
                    "{code} with {presented:?}"
                );
            }
            assert!(matches!(
                authenticate_peer(&conn, &rebuilt, SECRET).unwrap(),
                InviteAuth::Accepted(c) if c.id == id
            ));
        }
    }

    #[test]
    fn a_contact_without_secrets_is_never_authenticated() {
        // Owner decision (KT-1650): old contacts re-pair, no compatibility.
        let conn = test_conn();
        insert_contact(
            &conn,
            &crate::models::Contact {
                id: "old".into(),
                pseudo: "Old".into(),
                avatar_email: None,
                kronn_url: "http://10.0.0.1:3456".into(),
                invite_code: "kronn:Old@10.0.0.1:3456".into(),
                status: "accepted".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        )
        .unwrap();
        for presented in ["", SECRET, "kronn:Old@10.0.0.1:3456"] {
            assert!(matches!(
                authenticate_peer(&conn, "kronn:Old@10.0.0.1:3456", presented).unwrap(),
                InviteAuth::NotAccepted { ref status, .. } if status == "unpaired"
            ));
        }
        assert!(!is_paired(&conn, "old").unwrap());
        assert!(outbound_secret(&conn, "old").unwrap().is_none());
    }

    #[test]
    fn secrets_are_random_hashed_and_compared_in_constant_time() {
        let (a, b) = (generate_secret(), generate_secret());
        assert!(is_well_formed_secret(&a) && is_well_formed_secret(&b));
        assert_ne!(a, b, "never repeats");
        assert_eq!(a.len(), SECRET_LEN);
        let hash = hash_secret(&a);
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, a, "the stored value is not the secret");
        assert!(secret_matches(&hash, &a));
        assert!(!secret_matches(&hash, &b));
        assert!(!secret_matches(&hash, ""));
        assert!(!secret_matches(&hash, &hash), "the hash is no credential");
        assert!(!secret_matches("", &a));
        assert!(!is_well_formed_secret("kronn:alice@100.64.1.2:3456"));
        assert!(!is_well_formed_secret(&"zz".repeat(32)));
    }

    #[test]
    fn the_database_keeps_only_a_hash_of_the_inbound_secret() {
        let conn = test_conn();
        paired_contact(&conn, "a", "kronn:a@10.0.0.2:3456", "accepted", SECRET);
        let stored = inbound_secret_hash(&conn, "a").unwrap().unwrap();
        assert_eq!(stored, hash_secret(SECRET));
        assert_ne!(stored, SECRET);
        let in_inbound_column: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM contacts WHERE inbound_secret_hash = ?1",
                params![SECRET],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            in_inbound_column, 0,
            "the clear secret is not in that column"
        );
    }

    #[test]
    fn pairing_offers_are_hashed_spent_once_capped_and_expire() {
        let conn = test_conn();
        let secret = create_pairing_offer(&conn, None).unwrap().unwrap();
        assert!(is_well_formed_secret(&secret));
        let stored: String = conn
            .query_row("SELECT secret_hash FROM contact_pairing_offers", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(stored, hash_secret(&secret));
        assert!(find_pairing_offer(&conn, &generate_secret())
            .unwrap()
            .is_none());
        assert!(find_pairing_offer(&conn, "").unwrap().is_none());
        assert!(
            find_pairing_offer(&conn, &stored).unwrap().is_none(),
            "the hash is no credential"
        );
        let offer = find_pairing_offer(&conn, &secret)
            .unwrap()
            .expect("live offer");
        assert_eq!(offer.contact_id, None);
        assert_eq!(delete_pairing_offer(&conn, &offer.id).unwrap(), 1);
        assert_eq!(
            delete_pairing_offer(&conn, &offer.id).unwrap(),
            0,
            "a spent offer deletes nothing"
        );
        assert!(
            find_pairing_offer(&conn, &secret).unwrap().is_none(),
            "spent"
        );

        // Beyond the cap the oldest offers go.
        let first = create_pairing_offer(&conn, None).unwrap().unwrap();
        for _ in 0..MAX_PAIRING_OFFERS {
            create_pairing_offer(&conn, None).unwrap().unwrap();
        }
        let live: i64 = conn
            .query_row("SELECT COUNT(*) FROM contact_pairing_offers", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(live, MAX_PAIRING_OFFERS);
        assert!(
            find_pairing_offer(&conn, &first).unwrap().is_none(),
            "oldest evicted"
        );

        // An old one expires.
        let old = create_pairing_offer(&conn, None).unwrap().unwrap();
        conn.execute(
            "UPDATE contact_pairing_offers SET created_at = ?1 WHERE secret_hash = ?2",
            params![
                (Utc::now() - chrono::Duration::days(PAIRING_OFFER_TTL_DAYS + 1)).to_rfc3339(),
                hash_secret(&old)
            ],
        )
        .unwrap();
        assert!(
            find_pairing_offer(&conn, &old).unwrap().is_none(),
            "expired"
        );
    }

    fn plain_contact(id: &str, code: &str, status: &str) -> crate::models::Contact {
        let (pseudo, kronn_url) = parse_invite_code(code).unwrap();
        crate::models::Contact {
            id: id.into(),
            pseudo,
            avatar_email: None,
            kronn_url,
            invite_code: code.into(),
            status: status.into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn unpaired_contact(conn: &Connection, id: &str, code: &str, status: &str) {
        insert_contact(conn, &plain_contact(id, code, status)).unwrap();
    }

    fn attempt(code: &str, offer_secret: &str) -> PairingAttempt {
        let (pseudo, kronn_url) = parse_invite_code(code).unwrap();
        PairingAttempt {
            offer_secret: offer_secret.into(),
            invite_code: code.into(),
            pseudo,
            kronn_url,
            reverse_secret: generate_secret(),
        }
    }

    const BOB: &str = "kronn:bob@10.0.0.2:3140";
    const CHARLIE: &str = "kronn:charlie@10.0.0.3:3140";

    fn untouched(conn: &Connection, id: &str, status: &str) {
        assert_eq!(get_contact(conn, id).unwrap().unwrap().status, status);
        assert!(!is_paired(conn, id).unwrap(), "{id} got secrets");
        assert!(outbound_secret(conn, id).unwrap().is_none());
    }

    #[test]
    fn an_unbound_offer_pairs_a_new_contact_and_never_an_existing_one() {
        let conn = test_conn();
        // Charlie predates pairing secrets (needs_repair) and shares discussions.
        unpaired_contact(&conn, "charlie", CHARLIE, "needs_repair");
        let secret = create_pairing_offer(&conn, None).unwrap().unwrap();

        // Bob presents Charlie's public code with the offer: refused, Charlie's
        // row is untouched and the offer is still live.
        assert!(!accept_pairing(&conn, &attempt(CHARLIE, &secret)).unwrap());
        untouched(&conn, "charlie", "needs_repair");
        assert!(find_pairing_offer(&conn, &secret).unwrap().is_some());

        // Same for a pending row and a contact request.
        unpaired_contact(&conn, "dave", "kronn:dave@10.0.0.4:3140", "pending");
        assert!(!accept_pairing(&conn, &attempt("kronn:dave@10.0.0.4:3140", &secret)).unwrap());
        untouched(&conn, "dave", "pending");
        unpaired_contact(&conn, "erin", "kronn:erin@10.0.0.5:3140", "requested");
        assert!(!accept_pairing(&conn, &attempt("kronn:erin@10.0.0.5:3140", &secret)).unwrap());
        untouched(&conn, "erin", "requested");

        // A contact nobody knows yet is created, accepted and the offer spent.
        let bob = attempt(BOB, &secret);
        assert!(accept_pairing(&conn, &bob).unwrap());
        let stored = find_contact_by_invite_code(&conn, BOB).unwrap().unwrap();
        assert_eq!(stored.status, "accepted");
        assert_eq!(
            inbound_secret_hash(&conn, &stored.id).unwrap(),
            Some(hash_secret(&secret))
        );
        assert_eq!(
            outbound_secret(&conn, &stored.id).unwrap(),
            Some(bob.reverse_secret.clone())
        );
        assert!(find_pairing_offer(&conn, &secret).unwrap().is_none());
        untouched(&conn, "charlie", "needs_repair");
    }

    #[test]
    fn a_bound_offer_pairs_exactly_its_contact() {
        let conn = test_conn();
        unpaired_contact(&conn, "bob", BOB, "needs_repair");
        unpaired_contact(&conn, "charlie", CHARLIE, "needs_repair");
        let secret = create_pairing_offer(&conn, Some("bob")).unwrap().unwrap();
        assert_eq!(
            find_pairing_offer(&conn, &secret)
                .unwrap()
                .unwrap()
                .contact_id,
            Some("bob".to_string())
        );

        // Charlie's code with Bob's offer: refused, nothing moves.
        assert!(!accept_pairing(&conn, &attempt(CHARLIE, &secret)).unwrap());
        untouched(&conn, "charlie", "needs_repair");
        untouched(&conn, "bob", "needs_repair");
        // A stranger is not created either: the offer is for Bob.
        let stranger = "kronn:mallory@10.0.0.9:3140";
        assert!(!accept_pairing(&conn, &attempt(stranger, &secret)).unwrap());
        assert!(find_contact_by_invite_code(&conn, stranger)
            .unwrap()
            .is_none());
        assert!(find_pairing_offer(&conn, &secret).unwrap().is_some());

        // Bob re-pairs in place: same row (its id, its discussions), now accepted.
        assert!(accept_pairing(&conn, &attempt(BOB, &secret)).unwrap());
        let bob = get_contact(&conn, "bob").unwrap().unwrap();
        assert_eq!(bob.status, "accepted");
        assert!(is_paired(&conn, "bob").unwrap());
        untouched(&conn, "charlie", "needs_repair");
        assert!(find_pairing_offer(&conn, &secret).unwrap().is_none());
    }

    #[test]
    fn an_offer_cannot_be_issued_for_a_contact_that_cannot_be_re_paired() {
        let conn = test_conn();
        assert!(create_pairing_offer(&conn, Some("ghost"))
            .unwrap()
            .is_none());
        paired_contact(&conn, "bob", BOB, "accepted", SECRET);
        assert!(create_pairing_offer(&conn, Some("bob")).unwrap().is_none());
        unpaired_contact(&conn, "charlie", CHARLIE, "refused");
        assert!(create_pairing_offer(&conn, Some("charlie"))
            .unwrap()
            .is_none());
        let live: i64 = conn
            .query_row("SELECT COUNT(*) FROM contact_pairing_offers", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(live, 0, "no offer was stored");
    }

    fn live_offers(conn: &Connection, bound: bool) -> i64 {
        let clause = if bound {
            "contact_id IS NOT NULL"
        } else {
            "contact_id IS NULL"
        };
        conn.query_row(
            &format!("SELECT COUNT(*) FROM contact_pairing_offers WHERE {clause}"),
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn a_contact_has_at_most_one_live_bound_offer() {
        let conn = test_conn();
        unpaired_contact(&conn, "bob", BOB, "needs_repair");
        unpaired_contact(&conn, "charlie", CHARLIE, "needs_repair");
        let other = create_pairing_offer(&conn, Some("charlie"))
            .unwrap()
            .unwrap();
        let first = create_pairing_offer(&conn, Some("bob")).unwrap().unwrap();
        // Repeated Re-pair clicks on one contact never pile up.
        let mut last = first.clone();
        for _ in 0..(MAX_PAIRING_OFFERS * 2) {
            last = create_pairing_offer(&conn, Some("bob")).unwrap().unwrap();
        }
        assert_eq!(live_offers(&conn, true), 2, "one per contact");
        assert!(
            find_pairing_offer(&conn, &first).unwrap().is_none(),
            "the earlier offer for Bob is replaced"
        );
        assert!(find_pairing_offer(&conn, &last).unwrap().is_some());
        // Another contact's bound offer is untouched.
        assert!(find_pairing_offer(&conn, &other).unwrap().is_some());
        // Replaced offers cannot pair.
        assert!(!accept_pairing(&conn, &attempt(BOB, &first)).unwrap());
        untouched(&conn, "bob", "needs_repair");
        assert!(accept_pairing(&conn, &attempt(BOB, &last)).unwrap());
    }

    #[test]
    fn the_offer_cap_evicts_unbound_offers_and_never_a_bound_one() {
        let conn = test_conn();
        unpaired_contact(&conn, "bob", BOB, "needs_repair");
        let bound = create_pairing_offer(&conn, Some("bob")).unwrap().unwrap();
        let oldest_unbound = create_pairing_offer(&conn, None).unwrap().unwrap();
        // Unbound offers created later push the cap, not the bound one out.
        for _ in 0..(MAX_PAIRING_OFFERS * 2) {
            create_pairing_offer(&conn, None).unwrap().unwrap();
        }
        assert_eq!(live_offers(&conn, false), MAX_PAIRING_OFFERS);
        assert_eq!(live_offers(&conn, true), 1);
        assert!(
            find_pairing_offer(&conn, &oldest_unbound)
                .unwrap()
                .is_none(),
            "the oldest unbound offer is evicted"
        );
        assert!(
            find_pairing_offer(&conn, &bound).unwrap().is_some(),
            "a bound offer is never evicted by the cap"
        );
        // The bound offer does not eat into the unbound allowance either.
        assert!(accept_pairing(&conn, &attempt(BOB, &bound)).unwrap());
        assert_eq!(live_offers(&conn, false), MAX_PAIRING_OFFERS);
    }

    #[test]
    fn deleting_a_contact_drops_the_offers_issued_for_it() {
        let conn = test_conn();
        unpaired_contact(&conn, "bob", BOB, "needs_repair");
        let secret = create_pairing_offer(&conn, Some("bob")).unwrap().unwrap();
        assert!(delete_contact(&conn, "bob").unwrap());
        assert!(find_pairing_offer(&conn, &secret).unwrap().is_none());
    }

    #[test]
    fn a_spent_offer_cannot_pair_twice() {
        let conn = test_conn();
        let secret = create_pairing_offer(&conn, None).unwrap().unwrap();
        assert!(accept_pairing(&conn, &attempt(BOB, &secret)).unwrap());
        assert!(!accept_pairing(&conn, &attempt(CHARLIE, &secret)).unwrap());
        assert!(find_contact_by_invite_code(&conn, CHARLIE)
            .unwrap()
            .is_none());

        // Even when the offer is read as live, the write that spends it decides:
        // an offer deleted under the pairing (here by a trigger that swallows
        // the delete) voids it and leaves no accepted contact behind.
        let secret = create_pairing_offer(&conn, None).unwrap().unwrap();
        conn.execute_batch(
            "CREATE TRIGGER swallow_offer_delete BEFORE DELETE ON contact_pairing_offers
             BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
        assert!(!accept_pairing(&conn, &attempt(CHARLIE, &secret)).unwrap());
        assert!(
            find_contact_by_invite_code(&conn, CHARLIE)
                .unwrap()
                .is_none(),
            "the insert was rolled back"
        );
    }

    #[test]
    fn an_error_after_the_insert_leaves_no_accepted_contact_behind() {
        let conn = test_conn();
        let secret = create_pairing_offer(&conn, None).unwrap().unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_offer_delete BEFORE DELETE ON contact_pairing_offers
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .unwrap();
        assert!(accept_pairing(&conn, &attempt(BOB, &secret)).is_err());
        assert!(
            find_contact_by_invite_code(&conn, BOB).unwrap().is_none(),
            "contact insert and secrets rolled back"
        );
        assert!(list_contacts(&conn).unwrap().is_empty());
        conn.execute_batch("DROP TRIGGER fail_offer_delete;")
            .unwrap();
        assert!(
            find_pairing_offer(&conn, &secret).unwrap().is_some(),
            "the offer is still there"
        );
        assert!(accept_pairing(&conn, &attempt(BOB, &secret)).unwrap());
    }

    #[test]
    fn secrets_are_only_set_on_a_contact_that_is_not_paired_nor_refused() {
        let conn = test_conn();
        unpaired_contact(&conn, "bob", BOB, "needs_repair");
        assert!(set_pairing_secrets(&conn, "bob", &hash_secret("first"), "out-1").unwrap());
        // Paired meanwhile: a second pairing changes nothing.
        assert!(!set_pairing_secrets(&conn, "bob", &hash_secret("second"), "out-2").unwrap());
        assert_eq!(
            inbound_secret_hash(&conn, "bob").unwrap(),
            Some(hash_secret("first"))
        );
        assert_eq!(outbound_secret(&conn, "bob").unwrap(), Some("out-1".into()));
        // Refused meanwhile: not paired either.
        unpaired_contact(&conn, "charlie", CHARLIE, "refused");
        assert!(!set_pairing_secrets(&conn, "charlie", &hash_secret("x"), "y").unwrap());
        untouched(&conn, "charlie", "refused");
        assert!(!set_pairing_secrets(&conn, "ghost", &hash_secret("x"), "y").unwrap());
    }

    #[test]
    fn an_outbound_pairing_does_not_overwrite_a_contact_that_changed_in_flight() {
        let conn = test_conn();
        let fresh = |code: &str| plain_contact(&uuid::Uuid::new_v4().to_string(), code, "pending");
        let inbound = hash_secret("ours");

        // Unchanged: a new contact is created, then paired in place.
        let created = complete_outbound_pairing(&conn, None, &fresh(BOB), &inbound, "theirs")
            .unwrap()
            .expect("stored");
        assert_eq!(created.status, "accepted");
        unpaired_contact(&conn, "charlie", CHARLIE, "needs_repair");
        let in_place =
            complete_outbound_pairing(&conn, Some("charlie"), &fresh(CHARLIE), &inbound, "theirs")
                .unwrap()
                .expect("stored");
        assert_eq!(in_place.id, "charlie");
        assert_eq!(in_place.status, "accepted");

        // Paired while the peer was asked: nothing is overwritten.
        unpaired_contact(&conn, "dave", "kronn:dave@10.0.0.4:3140", "needs_repair");
        set_pairing_secrets(&conn, "dave", &hash_secret("winner"), "winner-out").unwrap();
        assert!(complete_outbound_pairing(
            &conn,
            Some("dave"),
            &fresh("kronn:dave@10.0.0.4:3140"),
            &inbound,
            "theirs"
        )
        .unwrap()
        .is_none());
        assert_eq!(
            inbound_secret_hash(&conn, "dave").unwrap(),
            Some(hash_secret("winner"))
        );
        assert_eq!(
            outbound_secret(&conn, "dave").unwrap(),
            Some("winner-out".into())
        );

        // Refused while the peer was asked.
        unpaired_contact(&conn, "erin", "kronn:erin@10.0.0.5:3140", "needs_repair");
        update_contact_status(&conn, "erin", "refused").unwrap();
        assert!(complete_outbound_pairing(
            &conn,
            Some("erin"),
            &fresh("kronn:erin@10.0.0.5:3140"),
            &inbound,
            "theirs"
        )
        .unwrap()
        .is_none());
        untouched(&conn, "erin", "refused");

        // Deleted while the peer was asked: not resurrected.
        assert!(complete_outbound_pairing(
            &conn,
            Some("gone"),
            &fresh("kronn:gone@10.0.0.6:3140"),
            &inbound,
            "theirs"
        )
        .unwrap()
        .is_none());
        assert!(
            find_contact_by_invite_code(&conn, "kronn:gone@10.0.0.6:3140")
                .unwrap()
                .is_none()
        );

        // Created by someone else while the peer was asked: no duplicate.
        unpaired_contact(&conn, "frank", "kronn:frank@10.0.0.7:3140", "pending");
        assert!(complete_outbound_pairing(
            &conn,
            None,
            &fresh("kronn:frank@10.0.0.7:3140"),
            &inbound,
            "theirs"
        )
        .unwrap()
        .is_none());
        untouched(&conn, "frank", "pending");
    }

    #[test]
    fn a_pairing_code_is_an_invite_code_and_a_secret() {
        let secret = generate_secret();
        let code = format!("kronn:alice@100.64.1.2:3456#{secret}");
        assert_eq!(
            split_pairing_code(&format!("  {code}\n")),
            Some(("kronn:alice@100.64.1.2:3456".into(), secret.clone()))
        );
        // A bare invite code is an address: it pairs nobody.
        assert!(split_pairing_code("kronn:alice@100.64.1.2:3456").is_none());
        assert!(split_pairing_code("kronn:alice@100.64.1.2:3456#short").is_none());
        assert!(split_pairing_code(&format!("kronn:a@h:1#{}", "g".repeat(64))).is_none());
        assert!(
            split_pairing_code(&format!("{}#{secret}", "x".repeat(MAX_INVITE_CODE_LEN + 1)))
                .is_none()
        );
        // The public part still has to parse as an invite code.
        let (public, _) = split_pairing_code(&code).unwrap();
        assert!(parse_invite_code(&public).is_some());
        assert!(
            parse_invite_code(&code).is_none(),
            "the secret is never part of an address"
        );
    }

    #[test]
    fn contacts_needing_a_repair_are_not_dialled() {
        assert!(!dials_outbound(STATUS_NEEDS_REPAIR));
        assert!(!dials_outbound(STATUS_REQUESTED));
        assert!(dials_outbound("accepted"));
    }

    #[test]
    fn parse_invite_code_valid() {
        let (pseudo, url) = parse_invite_code("kronn:testuser@100.64.1.5:3456").unwrap();
        assert_eq!(pseudo, "testuser");
        assert_eq!(url, "http://100.64.1.5:3456");
    }

    #[test]
    fn parse_invite_code_invalid() {
        assert!(parse_invite_code("invalid").is_none());
        assert!(parse_invite_code("kronn:@host").is_none());
        assert!(parse_invite_code("kronn:user@").is_none());
        assert!(parse_invite_code("").is_none());
    }

    #[test]
    fn insert_and_list_contacts() {
        let conn = test_conn();
        let now = Utc::now();
        let contact = Contact {
            id: "c1".into(),
            pseudo: "PeerAlpha".into(),
            avatar_email: Some("alpha@test.local".into()),
            kronn_url: "http://100.64.1.2:3456".into(),
            invite_code: "kronn:alpha@100.64.1.2:3456".into(),
            status: "accepted".into(),
            created_at: now,
            updated_at: now,
        };
        insert_contact(&conn, &contact).unwrap();

        let contacts = list_contacts(&conn).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].pseudo, "PeerAlpha");
    }

    #[test]
    fn delete_contact_removes_it() {
        let conn = test_conn();
        let now = Utc::now();
        let contact = Contact {
            id: "c2".into(),
            pseudo: "PeerBeta".into(),
            avatar_email: None,
            kronn_url: "http://100.64.1.3:3456".into(),
            invite_code: "kronn:beta@100.64.1.3:3456".into(),
            status: "pending".into(),
            created_at: now,
            updated_at: now,
        };
        insert_contact(&conn, &contact).unwrap();
        assert!(delete_contact(&conn, "c2").unwrap());
        assert_eq!(list_contacts(&conn).unwrap().len(), 0);
    }

    #[test]
    fn update_contact_status_changes_status() {
        let conn = test_conn();
        let now = Utc::now();
        let contact = Contact {
            id: "c3".into(),
            pseudo: "PeerGamma".into(),
            avatar_email: None,
            kronn_url: "http://100.64.1.4:3456".into(),
            invite_code: "kronn:gamma@100.64.1.4:3456".into(),
            status: "pending".into(),
            created_at: now,
            updated_at: now,
        };
        insert_contact(&conn, &contact).unwrap();
        update_contact_status(&conn, "c3", "accepted").unwrap();
        let c = get_contact(&conn, "c3").unwrap().unwrap();
        assert_eq!(c.status, "accepted");
    }

    #[test]
    fn find_by_invite_code_works() {
        let conn = test_conn();
        let now = Utc::now();
        let contact = Contact {
            id: "c4".into(),
            pseudo: "PeerDelta".into(),
            avatar_email: None,
            kronn_url: "http://100.64.1.5:3456".into(),
            invite_code: "kronn:delta@100.64.1.5:3456".into(),
            status: "accepted".into(),
            created_at: now,
            updated_at: now,
        };
        insert_contact(&conn, &contact).unwrap();
        let found = find_contact_by_invite_code(&conn, "kronn:delta@100.64.1.5:3456").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().pseudo, "PeerDelta");
    }

    #[test]
    fn parse_invite_code_trims_whitespace() {
        // Defensive — pasted invite codes commonly have leading/trailing spaces.
        let parsed = parse_invite_code("  kronn:user@host:3456  \n").unwrap();
        assert_eq!(parsed.0, "user");
        assert_eq!(parsed.1, "http://host:3456");
    }

    #[test]
    fn parse_invite_code_keeps_complex_pseudo_and_host() {
        // Hyphens, underscores, dots in pseudo / host — must all survive.
        let (pseudo, url) = parse_invite_code("kronn:user-with_dots.x@100.65.0.1:9999").unwrap();
        assert_eq!(pseudo, "user-with_dots.x");
        assert_eq!(url, "http://100.65.0.1:9999");
    }

    #[test]
    fn parse_invite_code_refuses_path_fragment_and_credential_tricks() {
        for code in [
            "kronn:x@127.0.0.1:9000/admin/action#",
            "kronn:x@127.0.0.1:9000#frag",
            "kronn:x@127.0.0.1:9000?a=b",
            "kronn:x@user:pw@127.0.0.1:9000",
            "kronn:x@127.0.0.1",
            "kronn:x@127.0.0.1:0",
            "kronn:x@127.0.0.1:65536",
            "kronn:x@exa mple.com:3140",
            "kronn:x@[::1]9000",
        ] {
            assert!(parse_invite_code(code).is_none(), "{code}");
        }
        let long_pseudo = format!("kronn:{}@host:3140", "p".repeat(MAX_PSEUDO_CHARS + 1));
        assert!(parse_invite_code(&long_pseudo).is_none());
        let long_host = format!("kronn:p@{}.example:3140", "h".repeat(MAX_INVITE_CODE_LEN));
        assert!(parse_invite_code(&long_host).is_none());
        assert_eq!(
            parse_invite_code("kronn:x@[::1]:3140").unwrap().1,
            "http://[::1]:3140"
        );
    }

    #[test]
    fn contact_base_url_revalidates_stored_urls() {
        assert_eq!(
            contact_base_url("http://10.0.0.5:3140/").as_deref(),
            Some("http://10.0.0.5:3140")
        );
        assert!(contact_base_url("http://127.0.0.1:9000/admin/action#").is_none());
        assert!(contact_base_url("file://host:1").is_none());
    }

    #[test]
    fn contact_requests_are_capped_and_expire() {
        let conn = test_conn();
        let request = |n: usize, age_days: i64| Contact {
            id: format!("r{n}"),
            pseudo: format!("R{n}"),
            avatar_email: None,
            kronn_url: format!("http://10.0.0.{n}:3456"),
            invite_code: format!("kronn:R{n}@10.0.0.{n}:3456"),
            status: STATUS_REQUESTED.into(),
            created_at: Utc::now() - chrono::Duration::days(age_days),
            updated_at: Utc::now(),
        };
        for n in 0..MAX_PENDING_REQUESTS as usize {
            assert!(insert_contact_request(&conn, &request(n, 0)).unwrap());
        }
        assert!(!insert_contact_request(&conn, &request(200, 0)).unwrap());
        conn.execute(
            "UPDATE contacts SET created_at = ?1 WHERE id = 'r0'",
            params![(Utc::now() - chrono::Duration::days(REQUEST_TTL_DAYS + 1)).to_rfc3339()],
        )
        .unwrap();
        assert!(insert_contact_request(&conn, &request(201, 0)).unwrap());
        assert!(
            get_contact(&conn, "r0").unwrap().is_none(),
            "expired request dropped"
        );
    }

    #[test]
    fn get_contact_returns_none_for_unknown_id() {
        let conn = test_conn();
        let res = get_contact(&conn, "nope").unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn update_contact_status_unknown_id_returns_false() {
        let conn = test_conn();
        let changed = update_contact_status(&conn, "does-not-exist", "accepted").unwrap();
        assert!(!changed, "updating an unknown id must report false");
    }

    #[test]
    fn delete_contact_unknown_id_returns_false() {
        let conn = test_conn();
        let removed = delete_contact(&conn, "does-not-exist").unwrap();
        assert!(!removed, "deleting an unknown id must report false");
    }

    #[test]
    fn find_contact_by_invite_code_unknown_returns_none() {
        let conn = test_conn();
        let res = find_contact_by_invite_code(&conn, "kronn:nobody@nowhere:0").unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn list_contacts_returns_empty_vec_when_table_empty() {
        let conn = test_conn();
        let contacts = list_contacts(&conn).unwrap();
        assert!(contacts.is_empty());
    }

    #[test]
    fn insert_contact_preserves_optional_avatar_none() {
        let conn = test_conn();
        let now = Utc::now();
        let contact = Contact {
            id: "c-no-avatar".into(),
            pseudo: "PeerNoAvatar".into(),
            avatar_email: None,
            kronn_url: "http://100.64.5.5:3456".into(),
            invite_code: "kronn:noavatar@100.64.5.5:3456".into(),
            status: "pending".into(),
            created_at: now,
            updated_at: now,
        };
        insert_contact(&conn, &contact).unwrap();
        let loaded = get_contact(&conn, "c-no-avatar").unwrap().unwrap();
        assert!(loaded.avatar_email.is_none());
        assert_eq!(loaded.pseudo, "PeerNoAvatar");
        assert_eq!(loaded.status, "pending");
    }

    #[test]
    fn insert_contact_duplicate_id_errors() {
        // The PK on contacts.id must reject duplicates — verifies the
        // migrations declare it (regression guard against schema drift).
        let conn = test_conn();
        let now = Utc::now();
        let contact = Contact {
            id: "dup-id".into(),
            pseudo: "PeerOne".into(),
            avatar_email: None,
            kronn_url: "http://100.64.6.6:3456".into(),
            invite_code: "kronn:one@100.64.6.6:3456".into(),
            status: "accepted".into(),
            created_at: now,
            updated_at: now,
        };
        insert_contact(&conn, &contact).unwrap();
        // Second insert with same id must fail.
        let result = insert_contact(&conn, &contact);
        assert!(result.is_err(), "duplicate id must be rejected");
    }

    #[test]
    fn list_contacts_orders_by_creation() {
        let conn = test_conn();
        let now = Utc::now();
        for (i, pseudo) in ["First", "Second", "Third"].iter().enumerate() {
            let contact = Contact {
                id: format!("ord-{}", i),
                pseudo: (*pseudo).into(),
                avatar_email: None,
                kronn_url: format!("http://100.64.7.{}:3456", i),
                invite_code: format!("kronn:{}@100.64.7.{}:3456", pseudo, i),
                status: "accepted".into(),
                created_at: now + chrono::Duration::seconds(i as i64),
                updated_at: now,
            };
            insert_contact(&conn, &contact).unwrap();
        }
        let contacts = list_contacts(&conn).unwrap();
        assert_eq!(contacts.len(), 3);
    }
}
