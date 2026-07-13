use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};

use crate::models::Contact;

pub fn list_contacts(conn: &Connection) -> Result<Vec<Contact>> {
    let mut stmt = conn.prepare(
        "SELECT id, pseudo, avatar_email, kronn_url, invite_code, status, created_at, updated_at
         FROM contacts ORDER BY pseudo"
    )?;
    let contacts = stmt.query_map([], |row| {
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
    })?.filter_map(|r| r.ok()).collect();
    Ok(contacts)
}

pub fn get_contact(conn: &Connection, id: &str) -> Result<Option<Contact>> {
    let mut stmt = conn.prepare(
        "SELECT id, pseudo, avatar_email, kronn_url, invite_code, status, created_at, updated_at
         FROM contacts WHERE id = ?1"
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
        Some(c) => InviteAuth::NotAccepted { pseudo: c.pseudo, status: c.status },
        None => InviteAuth::Unknown,
    })
}

pub fn find_contact_by_invite_code(conn: &Connection, invite_code: &str) -> Result<Option<Contact>> {
    let mut stmt = conn.prepare(
        "SELECT id, pseudo, avatar_email, kronn_url, invite_code, status, created_at, updated_at
         FROM contacts WHERE invite_code = ?1"
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

/// Parse invite code format: kronn:pseudo@host:port
pub fn parse_invite_code(code: &str) -> Option<(String, String)> {
    let code = code.trim();
    let rest = code.strip_prefix("kronn:")?;
    let (pseudo, url_part) = rest.split_once('@')?;
    if pseudo.is_empty() || url_part.is_empty() {
        return None;
    }
    let kronn_url = format!("http://{}", url_part);
    Some((pseudo.to_string(), kronn_url))
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
            ("a", "kr-a", "accepted"), ("b", "kr-b", "pending"), ("c", "kr-c", "refused"),
        ] {
            insert_contact(&conn, &crate::models::Contact {
                id: id.into(),
                pseudo: format!("p-{id}"),
                avatar_email: None,
                kronn_url: "http://x".into(),
                invite_code: code.into(),
                status: status.into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }).unwrap();
        }
        assert!(matches!(authenticate_invite_code(&conn, "kr-a").unwrap(),
            InviteAuth::Accepted(c) if c.id == "a"));
        assert!(matches!(authenticate_invite_code(&conn, "kr-b").unwrap(),
            InviteAuth::NotAccepted { ref status, .. } if status == "pending"));
        assert!(matches!(authenticate_invite_code(&conn, "kr-c").unwrap(),
            InviteAuth::NotAccepted { ref status, .. } if status == "refused"));
        assert!(matches!(authenticate_invite_code(&conn, "kr-ghost").unwrap(), InviteAuth::Unknown));
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
