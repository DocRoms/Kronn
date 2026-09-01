//! Persistence of accepted delivery summaries (KT-544).
//!
//! The canonical JSON is the record; the Markdown published in the discussion
//! is a projection of it. Insertion is the idempotency guard: the primary key
//! `(execution_id, attempt_no)` means a replayed approval, a double click or a
//! restart between the row and the message cannot produce a second report.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

/// A stored summary, as it was accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDeliverySummary {
    pub execution_id: String,
    pub attempt_no: u32,
    pub canonical_json: String,
    pub message_id: String,
    pub discussion_id: String,
    pub correlation_id: String,
}

/// Outcome of a publication attempt, so the caller knows whether it owns the
/// message it must now write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// This caller claimed the publication and must render the message.
    Claimed,
    /// Someone already published it; the stored row is authoritative.
    AlreadyPublished(StoredDeliverySummary),
}

/// Deterministic id of the message a summary renders into.
///
/// Derived, never random: the message and the row are written in separate
/// statements, so a crash between them must still converge on the same id
/// instead of leaving a second report behind.
pub fn message_id_for(execution_id: &str, attempt_no: u32) -> String {
    format!("delivery-summary:{execution_id}:{attempt_no}")
}

/// Claim the single publication slot for this attempt.
///
/// Returns [`Claim::AlreadyPublished`] when a row exists, including when this
/// exact caller wrote it moments ago — the caller must then publish nothing.
pub fn claim(
    conn: &Connection,
    summary: &StoredDeliverySummary,
    now: DateTime<Utc>,
) -> Result<Claim> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO delivery_summaries
             (execution_id, attempt_no, canonical_json, message_id, discussion_id,
              correlation_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            summary.execution_id,
            summary.attempt_no,
            summary.canonical_json,
            summary.message_id,
            summary.discussion_id,
            summary.correlation_id,
            now.to_rfc3339(),
        ],
    )?;
    if inserted == 1 {
        return Ok(Claim::Claimed);
    }
    let existing = get(conn, &summary.execution_id, summary.attempt_no)?
        // The insert was ignored, so a row is there. If it vanished between the
        // two statements the caller must not silently skip publication.
        .ok_or_else(|| anyhow::anyhow!("delivery summary row vanished after a conflict"))?;
    Ok(Claim::AlreadyPublished(existing))
}

pub fn get(
    conn: &Connection,
    execution_id: &str,
    attempt_no: u32,
) -> Result<Option<StoredDeliverySummary>> {
    Ok(conn
        .query_row(
            "SELECT execution_id, attempt_no, canonical_json, message_id, discussion_id,
                    correlation_id
             FROM delivery_summaries WHERE execution_id = ?1 AND attempt_no = ?2",
            params![execution_id, attempt_no],
            |row| {
                Ok(StoredDeliverySummary {
                    execution_id: row.get("execution_id")?,
                    attempt_no: row.get::<_, i64>("attempt_no")? as u32,
                    canonical_json: row.get("canonical_json")?,
                    message_id: row.get("message_id")?,
                    discussion_id: row.get("discussion_id")?,
                    correlation_id: row.get("correlation_id")?,
                })
            },
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn row(execution_id: &str) -> StoredDeliverySummary {
        StoredDeliverySummary {
            execution_id: execution_id.into(),
            attempt_no: 0,
            canonical_json: r#"{"schema_version":"delivery_summary/v1"}"#.into(),
            message_id: message_id_for(execution_id, 0),
            discussion_id: "d-parent".into(),
            correlation_id: "corr-1".into(),
        }
    }

    #[tokio::test]
    async fn the_first_claim_wins_and_every_replay_reads_the_stored_row() {
        let db = Database::open_in_memory().expect("in-memory db");
        db.with_conn(|conn| {
            let now = DateTime::parse_from_rfc3339("2026-09-01T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc);
            assert_eq!(claim(conn, &row("exec-1"), now)?, Claim::Claimed);

            // A double click, a replayed approval and a restart all land here.
            // None of them may publish a second report.
            let second = claim(conn, &row("exec-1"), now)?;
            match second {
                Claim::AlreadyPublished(stored) => {
                    assert_eq!(stored.message_id, "delivery-summary:exec-1:0");
                    assert_eq!(stored.correlation_id, "corr-1");
                }
                Claim::Claimed => panic!("a second claim must never win"),
            }

            // A different attempt is a different delivery and gets its own slot.
            let mut retry = row("exec-1");
            retry.attempt_no = 1;
            retry.message_id = message_id_for("exec-1", 1);
            assert_eq!(claim(conn, &retry, now)?, Claim::Claimed);
            Ok(())
        })
        .await
        .expect("claims");
    }

    #[tokio::test]
    async fn a_conflicting_claim_never_overwrites_the_accepted_record() {
        let db = Database::open_in_memory().expect("in-memory db");
        db.with_conn(|conn| {
            let now = DateTime::parse_from_rfc3339("2026-09-01T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc);
            claim(conn, &row("exec-2"), now)?;

            // Same slot, different payload: the audit record must be the one
            // that was accepted, not whatever a later caller carried.
            let mut tampered = row("exec-2");
            tampered.canonical_json = r#"{"summary":"rewritten after the fact"}"#.into();
            let outcome = claim(conn, &tampered, now)?;
            assert!(matches!(outcome, Claim::AlreadyPublished(_)));
            let stored = get(conn, "exec-2", 0)?.expect("row");
            assert_eq!(
                stored.canonical_json,
                r#"{"schema_version":"delivery_summary/v1"}"#
            );
            Ok(())
        })
        .await
        .expect("claims");
    }
}
