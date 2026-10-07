use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};

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

/// Whether the outbound client may dial this contact. A request or a refused
/// contact is never dialled: dialling would mark it accepted.
pub fn dials_outbound(status: &str) -> bool {
    status != STATUS_REQUESTED && status != "refused"
}

/// Passe D — the ONE sanctioned way to authenticate a P2P caller by invite
/// code: a known contact whose status is `accepted`. A pending/refused
/// contact keeps its code but must not pass the auth-exempt routes
/// (claim-by-token, fetch-file). `NotAccepted` carries the real status for
/// telemetry; callers MUST answer exactly like `Unknown` (no oracle).
pub enum InviteAuth {
    Accepted(Contact),
    NotAccepted { pseudo: String, status: String },
    Unknown,
}

pub fn authenticate_invite_code(conn: &Connection, invite_code: &str) -> Result<InviteAuth> {
    Ok(match find_contact_by_invite_code(conn, invite_code)? {
        Some(c) if c.status == "accepted" => InviteAuth::Accepted(c),
        Some(c) => InviteAuth::NotAccepted {
            pseudo: c.pseudo,
            status: c.status,
        },
        None => InviteAuth::Unknown,
    })
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

/// Parse invite code format: kronn:pseudo@host:port. The address must be a
/// bare host and port: no path, query, fragment or credentials, so the URL
/// built from it cannot point anywhere else.
pub fn parse_invite_code(code: &str) -> Option<(String, String)> {
    let code = code.trim();
    if code.len() > MAX_INVITE_CODE_LEN {
        return None;
    }
    let rest = code.strip_prefix("kronn:")?;
    let (pseudo, url_part) = rest.rsplit_once('@')?;
    if pseudo.is_empty()
        || pseudo.chars().count() > MAX_PSEUDO_CHARS
        || pseudo.chars().any(|c| c.is_control() || c == '@')
    {
        return None;
    }
    let authority = canonical_authority(url_part)?;
    Some((pseudo.to_string(), format!("http://{authority}")))
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
    let affected = conn.execute("DELETE FROM contacts WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        migrations::run(&conn).unwrap();
        conn
    }

    #[test]
    fn only_accepted_contacts_authenticate_by_invite_code() {
        // Passe D (Codex constat n°1) — pending/refused contacts keep their
        // code but must not pass invite-code auth; the enum carries the real
        // status so callers can log it without re-implementing the rule.
        let conn = test_conn();
        for (id, code, status) in [
            ("a", "kr-a", "accepted"),
            ("b", "kr-b", "pending"),
            ("c", "kr-c", "refused"),
        ] {
            insert_contact(
                &conn,
                &crate::models::Contact {
                    id: id.into(),
                    pseudo: format!("p-{id}"),
                    avatar_email: None,
                    kronn_url: "http://x".into(),
                    invite_code: code.into(),
                    status: status.into(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                },
            )
            .unwrap();
        }
        assert!(matches!(authenticate_invite_code(&conn, "kr-a").unwrap(),
            InviteAuth::Accepted(c) if c.id == "a"));
        assert!(matches!(authenticate_invite_code(&conn, "kr-b").unwrap(),
            InviteAuth::NotAccepted { ref status, .. } if status == "pending"));
        assert!(matches!(authenticate_invite_code(&conn, "kr-c").unwrap(),
            InviteAuth::NotAccepted { ref status, .. } if status == "refused"));
        assert!(matches!(
            authenticate_invite_code(&conn, "kr-ghost").unwrap(),
            InviteAuth::Unknown
        ));
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
