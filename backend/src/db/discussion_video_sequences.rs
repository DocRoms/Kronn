//! The order a discussion's clips are played in as one film, and the clips
//! set aside from it.
//!
//! Only what a person arranged is stored. Reading returns it filtered to the
//! files the discussion still has, so a deleted clip drops out without anyone
//! editing the order; which of those files are videos, and where a clip
//! generated since goes, is decided where the kind of a file is known.

use anyhow::{bail, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;

/// Enough for any film a discussion produces, small enough that a request
/// cannot turn the row into a store of its own.
const MAX_FILES: usize = 500;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Sequence {
    /// Played, in this order.
    pub file_ids: Vec<String>,
    /// Kept by the discussion, not played.
    pub excluded_ids: Vec<String>,
}

pub fn get(conn: &Connection, discussion_id: &str) -> Result<Sequence> {
    let stored: Option<(String, String)> = conn
        .query_row(
            "SELECT file_ids_json, excluded_file_ids_json FROM discussion_video_sequences \
             WHERE discussion_id = ?1",
            params![discussion_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((played, excluded)) = stored else {
        return Ok(Sequence::default());
    };
    let present = files_of(conn, discussion_id)?;
    let still_present = |json: &str| -> Result<Vec<String>> {
        let ids: Vec<String> = serde_json::from_str(json)?;
        Ok(ids.into_iter().filter(|id| present.contains(id)).collect())
    };
    Ok(Sequence {
        file_ids: still_present(&played)?,
        excluded_ids: still_present(&excluded)?,
    })
}

/// Store an order. Every id must be a file of this discussion, and appear
/// once across both lists: an order naming another room's clip would play it
/// here, and a clip both played and set aside means nothing.
pub fn set(
    conn: &Connection,
    discussion_id: &str,
    file_ids: &[String],
    excluded_ids: &[String],
) -> Result<Sequence> {
    if file_ids.len() + excluded_ids.len() > MAX_FILES {
        bail!("an order holds at most {MAX_FILES} clips");
    }
    let present = files_of(conn, discussion_id)?;
    let mut seen = HashSet::new();
    for id in file_ids.iter().chain(excluded_ids) {
        if !seen.insert(id.as_str()) {
            bail!("clip `{id}` appears twice in the order");
        }
        if !present.contains(id) {
            bail!("`{id}` is not a file of this discussion");
        }
    }
    conn.execute(
        "INSERT INTO discussion_video_sequences \
             (discussion_id, file_ids_json, excluded_file_ids_json, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(discussion_id) DO UPDATE SET
             file_ids_json = excluded.file_ids_json,
             excluded_file_ids_json = excluded.excluded_file_ids_json,
             updated_at = excluded.updated_at",
        params![
            discussion_id,
            serde_json::to_string(file_ids)?,
            serde_json::to_string(excluded_ids)?,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(Sequence {
        file_ids: file_ids.to_vec(),
        excluded_ids: excluded_ids.to_vec(),
    })
}

fn files_of(conn: &Connection, discussion_id: &str) -> Result<HashSet<String>> {
    let mut statement = conn.prepare("SELECT id FROM context_files WHERE discussion_id = ?1")?;
    let ids = statement
        .query_map(params![discussion_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        for discussion in ["disc-film", "disc-other"] {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at) \
                 VALUES (?1, 'Film', '2026-09-19T00:00:00Z', '2026-09-19T00:00:00Z')",
                [discussion],
            )
            .unwrap();
        }
        for (id, discussion) in [
            ("clip-1", "disc-film"),
            ("clip-2", "disc-film"),
            ("clip-3", "disc-film"),
            ("elsewhere", "disc-other"),
        ] {
            conn.execute(
                "INSERT INTO context_files (id, discussion_id, filename, mime_type, original_size, \
                 extracted_text, extracted_size, created_at) \
                 VALUES (?1, ?2, ?1, 'video/mp4', 1, '', 0, '2026-09-19T00:00:00Z')",
                params![id, discussion],
            )
            .unwrap();
        }
        conn
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn sequence(played: &[&str], excluded: &[&str]) -> Sequence {
        Sequence {
            file_ids: ids(played),
            excluded_ids: ids(excluded),
        }
    }

    #[test]
    fn an_arranged_order_is_kept_and_a_deleted_clip_drops_out_of_it() {
        let conn = connection();
        assert_eq!(
            get(&conn, "disc-film").unwrap(),
            Sequence::default(),
            "nothing arranged yet"
        );

        set(
            &conn,
            "disc-film",
            &ids(&["clip-3", "clip-1", "clip-2"]),
            &[],
        )
        .unwrap();
        assert_eq!(
            get(&conn, "disc-film").unwrap(),
            sequence(&["clip-3", "clip-1", "clip-2"], &[])
        );

        conn.execute("DELETE FROM context_files WHERE id = 'clip-1'", [])
            .unwrap();
        assert_eq!(
            get(&conn, "disc-film").unwrap(),
            sequence(&["clip-3", "clip-2"], &[])
        );
    }

    #[test]
    fn a_clip_set_aside_stays_aside_until_it_is_deleted() {
        let conn = connection();
        set(
            &conn,
            "disc-film",
            &ids(&["clip-2", "clip-1"]),
            &ids(&["clip-3"]),
        )
        .unwrap();
        assert_eq!(
            get(&conn, "disc-film").unwrap(),
            sequence(&["clip-2", "clip-1"], &["clip-3"])
        );

        conn.execute("DELETE FROM context_files WHERE id = 'clip-3'", [])
            .unwrap();
        assert_eq!(
            get(&conn, "disc-film").unwrap(),
            sequence(&["clip-2", "clip-1"], &[])
        );
    }

    #[test]
    fn an_order_naming_another_rooms_clip_or_a_clip_twice_is_refused() {
        let conn = connection();
        let foreign = set(&conn, "disc-film", &ids(&["clip-1"]), &ids(&["elsewhere"])).unwrap_err();
        assert!(
            foreign
                .to_string()
                .contains("not a file of this discussion"),
            "{foreign}"
        );
        let twice = set(&conn, "disc-film", &ids(&["clip-1", "clip-1"]), &[]).unwrap_err();
        assert!(twice.to_string().contains("twice"), "{twice}");
        let both = set(&conn, "disc-film", &ids(&["clip-1"]), &ids(&["clip-1"])).unwrap_err();
        assert!(both.to_string().contains("twice"), "{both}");
        assert_eq!(
            get(&conn, "disc-film").unwrap(),
            Sequence::default(),
            "a refused order stores nothing"
        );
    }
}
