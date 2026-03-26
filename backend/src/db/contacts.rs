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
}
