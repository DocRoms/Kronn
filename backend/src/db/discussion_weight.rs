//! Ventilated storage weight per discussion.
//!
//! Two grouped aggregates cover the whole listing, so the caller never issues
//! one query per discussion. Message bytes are measured on the BLOB cast:
//! `LENGTH` over TEXT counts characters, which under-reports accented and
//! emoji content.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, params_from_iter, Connection};

use crate::models::DiscussionWeight;

/// A file kept on disk and also text-extracted occupies both places, so the
/// two sums are deliberately independent rather than exclusive.
const FILE_BYTES_SQL: &str = "SELECT discussion_id,
            SUM(CASE WHEN disk_path IS NOT NULL THEN original_size ELSE 0 END) AS disk_bytes,
            SUM(extracted_size) AS extracted_bytes
     FROM context_files";

const MESSAGE_BYTES_SQL: &str = "SELECT discussion_id,
            SUM(LENGTH(CAST(content AS BLOB))) AS message_bytes
     FROM messages";

/// `SUM` over an empty selection is NULL, and a byte count can never be
/// negative, so both cases collapse to zero. Columns are read by ALIAS: the
/// grouped form keeps `discussion_id` ahead of the sums, and an index-based
/// read silently yielded zero.
fn bytes(row: &rusqlite::Row, alias: &str) -> u64 {
    row.get::<_, Option<i64>>(alias)
        .ok()
        .flatten()
        .and_then(|v| u64::try_from(v).ok())
        .unwrap_or(0)
}

/// Largest batch accepted in one call. Keeps the `IN (...)` list well under
/// SQLite's bound-parameter ceiling and stops a caller from turning the
/// endpoint into a full-table scan.
pub const MAX_BATCH_IDS: usize = 200;

/// Weight of the requested discussions only. The result is SPARSE: a
/// discussion holding nothing is simply absent, so the caller distinguishes
/// "weighs nothing" from "not asked for". Never scans the whole table.
pub fn for_ids(conn: &Connection, ids: &[String]) -> Result<HashMap<String, DiscussionWeight>> {
    let mut out: HashMap<String, DiscussionWeight> = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let ids: Vec<&String> = ids.iter().take(MAX_BATCH_IDS).collect();
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");

    let sql =
        format!("{FILE_BYTES_SQL} WHERE discussion_id IN ({placeholders}) GROUP BY discussion_id");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(ids.iter()))?;
    while let Some(row) = rows.next()? {
        let id: String = row.get("discussion_id")?;
        let entry = out
            .entry(id.clone())
            .or_insert_with(|| DiscussionWeight::new(id));
        entry.disk_bytes = bytes(row, "disk_bytes");
        entry.extracted_text_bytes = bytes(row, "extracted_bytes");
    }

    let sql = format!(
        "{MESSAGE_BYTES_SQL} WHERE discussion_id IN ({placeholders}) GROUP BY discussion_id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(ids.iter()))?;
    while let Some(row) = rows.next()? {
        let id: String = row.get("discussion_id")?;
        let entry = out
            .entry(id.clone())
            .or_insert_with(|| DiscussionWeight::new(id));
        entry.message_bytes = bytes(row, "message_bytes");
    }

    Ok(out)
}

/// Weight of a single discussion. Returns a zeroed entry when it holds
/// nothing, so callers never have to special-case an empty discussion.
pub fn one(conn: &Connection, discussion_id: &str) -> Result<DiscussionWeight> {
    let mut weight = DiscussionWeight::new(discussion_id);

    let sql = format!("{FILE_BYTES_SQL} WHERE discussion_id = ?1");
    conn.query_row(&sql, params![discussion_id], |row| {
        weight.disk_bytes = bytes(row, "disk_bytes");
        weight.extracted_text_bytes = bytes(row, "extracted_bytes");
        Ok(())
    })?;

    let sql = format!("{MESSAGE_BYTES_SQL} WHERE discussion_id = ?1");
    conn.query_row(&sql, params![discussion_id], |row| {
        weight.message_bytes = bytes(row, "message_bytes");
        Ok(())
    })?;

    Ok(weight)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{WeightLevel, WeightThresholds};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE context_files (
                id TEXT PRIMARY KEY,
                discussion_id TEXT NOT NULL,
                filename TEXT NOT NULL,
                mime_type TEXT NOT NULL DEFAULT '',
                original_size INTEGER NOT NULL DEFAULT 0,
                extracted_text TEXT NOT NULL DEFAULT '',
                extracted_size INTEGER NOT NULL DEFAULT 0,
                disk_path TEXT
             );
             CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                discussion_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    fn add_file(conn: &Connection, id: &str, disc: &str, size: i64, extracted: i64, on_disk: bool) {
        conn.execute(
            "INSERT INTO context_files
                (id, discussion_id, filename, original_size, extracted_size, disk_path)
             VALUES (?1, ?2, 'f', ?3, ?4, ?5)",
            params![id, disc, size, extracted, on_disk.then_some("/tmp/f")],
        )
        .unwrap();
    }

    fn add_message(conn: &Connection, id: &str, disc: &str, content: &str) {
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content)
             VALUES (?1, ?2, 'User', ?3)",
            params![id, disc, content],
        )
        .unwrap();
    }

    #[test]
    fn splits_disk_extracted_and_message_bytes() {
        let conn = db();
        add_file(&conn, "f1", "d1", 1_000, 400, true);
        add_file(&conn, "f2", "d1", 5_000, 0, false);
        add_message(&conn, "m1", "d1", "hello");

        let w = one(&conn, "d1").unwrap();
        // f2 has no disk_path, so its original_size is not disk-held; its
        // text would be counted through extracted_size instead.
        assert_eq!(w.disk_bytes, 1_000);
        assert_eq!(w.extracted_text_bytes, 400);
        assert_eq!(w.message_bytes, 5);
        assert_eq!(w.total_bytes(), 1_405);
        assert_eq!(w.reclaimable_bytes(), 1_000);
    }

    #[test]
    fn message_bytes_count_utf8_octets_not_chars() {
        let conn = db();
        // 4 chars, 7 UTF-8 bytes: a naive LENGTH() would report 4.
        add_message(&conn, "m1", "d1", "éàc✓");

        let w = one(&conn, "d1").unwrap();
        assert_eq!(w.message_bytes, "éàc✓".len() as u64);
        assert!(w.message_bytes > 4);
    }

    #[test]
    fn empty_discussion_weighs_nothing_without_erroring() {
        let conn = db();
        let w = one(&conn, "ghost").unwrap();
        assert_eq!(w.total_bytes(), 0);
        assert_eq!(w.discussion_id, "ghost");
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn for_ids_covers_each_requested_discussion() {
        let conn = db();
        add_file(&conn, "f1", "d1", 2_000, 0, true);
        add_message(&conn, "m1", "d1", "abc");
        add_message(&conn, "m2", "d2", "defgh");
        add_file(&conn, "f2", "d3", 0, 900, false);

        let map = for_ids(&conn, &ids(&["d1", "d2", "d3"])).unwrap();
        assert_eq!(map.len(), 3);
        assert_eq!(map["d1"].disk_bytes, 2_000);
        assert_eq!(map["d1"].message_bytes, 3);
        assert_eq!(map["d2"].message_bytes, 5);
        assert_eq!(map["d2"].disk_bytes, 0);
        assert_eq!(map["d3"].extracted_text_bytes, 900);
    }

    #[test]
    fn for_ids_never_returns_discussions_it_was_not_asked_for() {
        let conn = db();
        add_message(&conn, "m1", "asked", "abc");
        add_message(&conn, "m2", "other", "defgh");

        let map = for_ids(&conn, &ids(&["asked"])).unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("asked"));
        assert!(!map.contains_key("other"));
    }

    #[test]
    fn for_ids_is_sparse_and_costs_nothing_on_an_empty_request() {
        let conn = db();
        add_message(&conn, "m1", "d1", "abc");

        // Empty request short-circuits without touching the database.
        assert!(for_ids(&conn, &[]).unwrap().is_empty());
        // A requested but empty discussion is absent, not a zeroed row.
        let map = for_ids(&conn, &ids(&["d1", "ghost"])).unwrap();
        assert_eq!(map.len(), 1);
        assert!(!map.contains_key("ghost"));
    }

    #[test]
    fn for_ids_caps_the_batch_instead_of_scanning_everything() {
        let conn = db();
        for i in 0..(MAX_BATCH_IDS + 25) {
            add_message(&conn, &format!("m{i}"), &format!("d{i}"), "x");
        }
        let requested = ids(&[])
            .into_iter()
            .chain((0..(MAX_BATCH_IDS + 25)).map(|i| format!("d{i}")))
            .collect::<Vec<_>>();

        let map = for_ids(&conn, &requested).unwrap();
        assert_eq!(map.len(), MAX_BATCH_IDS);
    }

    #[test]
    fn a_file_on_disk_and_extracted_counts_in_both_masses() {
        let conn = db();
        add_file(&conn, "f1", "d1", 10_000, 3_000, true);

        let w = one(&conn, "d1").unwrap();
        assert_eq!(w.disk_bytes, 10_000);
        assert_eq!(w.extracted_text_bytes, 3_000);
        assert_eq!(w.total_bytes(), 13_000);
    }

    #[test]
    fn an_unusable_threshold_pair_falls_back_whole_instead_of_being_patched() {
        use crate::models::{DEFAULT_AMBER_BYTES, DEFAULT_RED_BYTES};
        for bad in [
            WeightThresholds {
                amber_bytes: 100,
                red_bytes: 50,
            },
            WeightThresholds {
                amber_bytes: 0,
                red_bytes: 50,
            },
            WeightThresholds {
                amber_bytes: 50,
                red_bytes: 50,
            },
        ] {
            assert!(!bad.is_valid());
            let t = bad.validated();
            assert_eq!(t.amber_bytes, DEFAULT_AMBER_BYTES);
            assert_eq!(t.red_bytes, DEFAULT_RED_BYTES);
        }
        let good = WeightThresholds {
            amber_bytes: 10,
            red_bytes: 20,
        };
        assert!(good.is_valid());
        assert_eq!(good.validated(), good);
    }

    #[test]
    fn level_grades_on_the_total_footprint() {
        let t = WeightThresholds {
            amber_bytes: 50,
            red_bytes: 100,
        };

        let mut w = DiscussionWeight::new("d1");
        w.message_bytes = 10;
        assert_eq!(w.level(&t), WeightLevel::Green);
        w.message_bytes = 60;
        assert_eq!(w.level(&t), WeightLevel::Amber);
        w.message_bytes = 500;
        assert_eq!(w.level(&t), WeightLevel::Red);
    }
}
