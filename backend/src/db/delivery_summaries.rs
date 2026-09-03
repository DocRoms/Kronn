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

/// What a publication attempt actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Published {
    /// Record and message were both written by this call.
    Created,
    /// The record existed with no message — a crash or a kill between the two
    /// writes of an older code path. This call inserted the missing message
    /// from the STORED payload.
    Repaired,
    /// Record and message were already there; nothing was written.
    AlreadyComplete,
}

/// Deterministic id of the message a summary renders into.
///
/// Derived, never random: it is what lets a retry recognise its own message
/// instead of adding a second one.
pub fn message_id_for(execution_id: &str, attempt_no: u32) -> String {
    format!("delivery-summary:{execution_id}:{attempt_no}")
}

/// Publish the accepted summary and its single message, atomically.
///
/// The record and the message are written in ONE transaction, so the normal
/// path has no window between them. The repair branch exists for records left
/// by an earlier code path (or by a kill outside a transaction): finding a
/// record with no message, this inserts the message rather than concluding
/// "already published" and leaving an accepted delivery reportless — the
/// failure mode of the first version of this module.
///
/// `render` receives the payload that is actually STORED, never the caller's.
/// A retry carrying a divergent summary therefore republishes the accepted
/// record and cannot rewrite history.
pub fn publish<F>(
    conn: &Connection,
    summary: &StoredDeliverySummary,
    now: DateTime<Utc>,
    render: F,
) -> Result<Published>
where
    F: FnOnce(&StoredDeliverySummary) -> crate::models::DiscussionMessage,
{
    let tx = conn.unchecked_transaction()?;
    let inserted = tx.execute(
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

    // Authoritative from here on: whatever is stored, even if this caller
    // brought something else.
    let stored = get(&tx, &summary.execution_id, summary.attempt_no)?
        .ok_or_else(|| anyhow::anyhow!("delivery summary row vanished inside its transaction"))?;

    let message_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
        [&stored.message_id],
        |row| row.get(0),
    )?;

    let outcome = if message_exists {
        Published::AlreadyComplete
    } else {
        let message = render(&stored);
        // The id is derived, so a message written by a previous attempt would
        // have matched above; inserting here cannot duplicate.
        crate::db::discussions::insert_message(&tx, &stored.discussion_id, &message)?;
        if inserted == 1 {
            Published::Created
        } else {
            Published::Repaired
        }
    };
    tx.commit()?;
    Ok(outcome)
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
    use crate::models::{DiscussionMessage, MessageChannel, MessageRole};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-01T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn row(execution_id: &str) -> StoredDeliverySummary {
        StoredDeliverySummary {
            execution_id: execution_id.into(),
            attempt_no: 0,
            canonical_json: r#"{"summary":"accepted delivery"}"#.into(),
            message_id: message_id_for(execution_id, 0),
            discussion_id: "d-parent".into(),
            correlation_id: "corr-1".into(),
        }
    }

    /// Renders from the STORED payload, so a test can see which one was used.
    fn render(stored: &StoredDeliverySummary) -> DiscussionMessage {
        DiscussionMessage {
            id: stored.message_id.clone(),
            role: MessageRole::Agent,
            channel: MessageChannel::Main,
            content: stored.canonical_json.clone(),
            agent_type: None,
            timestamp: now(),
            tokens_used: 0,
            session_tokens_at_message: None,
            recovered_partial: false,
            auth_mode: None,
            model_tier: None,
            model: None,
            cost_usd: None,
            author_pseudo: Some("Worker".into()),
            author_avatar_email: None,
            source_msg_id: None,
            duration_ms: None,
            lint_report: None,
            target_agent: None,
            reply_to_message_id: None,
            author_cli_ordinal: None,
        }
    }

    async fn db_with_discussion() -> Database {
        let db = Database::open_in_memory().expect("in-memory db");
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at) \
                 VALUES ('d-parent', 'Parent', ?1, ?1)",
                ["2026-09-01T09:00:00Z"],
            )?;
            Ok(())
        })
        .await
        .expect("seed");
        db
    }

    async fn messages_for(db: &Database, id: &str) -> Vec<String> {
        let id = id.to_string();
        db.with_read_conn(move |conn| {
            let mut stmt = conn.prepare("SELECT content FROM messages WHERE id = ?1")?;
            let rows = stmt
                .query_map([&id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
        .expect("read messages")
    }

    #[tokio::test]
    async fn a_first_publication_writes_the_record_and_exactly_one_message() {
        let db = db_with_discussion().await;
        let outcome = db
            .with_conn(|conn| publish(conn, &row("exec-1"), now(), render))
            .await
            .expect("publish");
        assert_eq!(outcome, Published::Created);
        assert_eq!(
            messages_for(&db, "delivery-summary:exec-1:0").await.len(),
            1
        );
    }

    #[tokio::test]
    async fn a_double_click_or_a_restart_publishes_nothing_more() {
        let db = db_with_discussion().await;
        db.with_conn(|conn| publish(conn, &row("exec-1"), now(), render))
            .await
            .unwrap();
        // Second click, replayed approval, process restart: all land here.
        let again = db
            .with_conn(|conn| publish(conn, &row("exec-1"), now(), render))
            .await
            .unwrap();
        assert_eq!(again, Published::AlreadyComplete);
        assert_eq!(
            messages_for(&db, "delivery-summary:exec-1:0").await.len(),
            1,
            "a retry must never add a second report"
        );
    }

    #[tokio::test]
    async fn a_record_left_without_its_message_is_repaired_by_the_retry() {
        let db = db_with_discussion().await;
        // Simulate the crash the first version of this module could not
        // survive: the row is durable, the message was never written.
        db.with_conn(|conn| {
            let orphan = row("exec-crash");
            conn.execute(
                "INSERT INTO delivery_summaries
                     (execution_id, attempt_no, canonical_json, message_id, discussion_id,
                      correlation_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    orphan.execution_id,
                    orphan.attempt_no,
                    orphan.canonical_json,
                    orphan.message_id,
                    orphan.discussion_id,
                    orphan.correlation_id,
                    now().to_rfc3339(),
                ],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(messages_for(&db, "delivery-summary:exec-crash:0")
            .await
            .is_empty());

        let repaired = db
            .with_conn(|conn| publish(conn, &row("exec-crash"), now(), render))
            .await
            .unwrap();
        assert_eq!(
            repaired,
            Published::Repaired,
            "an accepted delivery must not stay reportless"
        );
        assert_eq!(
            messages_for(&db, "delivery-summary:exec-crash:0")
                .await
                .len(),
            1,
            "the repair must insert exactly one message"
        );
    }

    #[tokio::test]
    async fn a_retry_carrying_a_divergent_payload_republishes_the_accepted_one() {
        let db = db_with_discussion().await;
        db.with_conn(|conn| {
            let accepted = row("exec-2");
            conn.execute(
                "INSERT INTO delivery_summaries
                     (execution_id, attempt_no, canonical_json, message_id, discussion_id,
                      correlation_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    accepted.execution_id,
                    accepted.attempt_no,
                    accepted.canonical_json,
                    accepted.message_id,
                    accepted.discussion_id,
                    accepted.correlation_id,
                    now().to_rfc3339(),
                ],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // The repair path is where a divergent payload could slip in.
        let mut tampered = row("exec-2");
        tampered.canonical_json = r#"{"summary":"rewritten after the fact"}"#.into();
        db.with_conn(move |conn| publish(conn, &tampered, now(), render))
            .await
            .unwrap();

        let published = messages_for(&db, "delivery-summary:exec-2:0").await;
        assert_eq!(published.len(), 1);
        assert_eq!(
            published[0], r#"{"summary":"accepted delivery"}"#,
            "the report must render the stored record, never a later payload"
        );
        let stored = db
            .with_read_conn(|conn| get(conn, "exec-2", 0))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.canonical_json, r#"{"summary":"accepted delivery"}"#);
    }

    #[tokio::test]
    async fn a_second_attempt_of_the_same_execution_gets_its_own_report() {
        let db = db_with_discussion().await;
        db.with_conn(|conn| publish(conn, &row("exec-3"), now(), render))
            .await
            .unwrap();
        let mut retry = row("exec-3");
        retry.attempt_no = 1;
        retry.message_id = message_id_for("exec-3", 1);
        let outcome = db
            .with_conn(move |conn| publish(conn, &retry, now(), render))
            .await
            .unwrap();
        assert_eq!(outcome, Published::Created);
        assert_eq!(
            messages_for(&db, "delivery-summary:exec-3:1").await.len(),
            1
        );
    }
}
