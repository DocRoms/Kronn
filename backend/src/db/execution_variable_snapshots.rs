use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::core::{crypto, execution_variables::VariableProvenance};

#[derive(Debug, serde::Serialize)]
pub struct SnapshotMetadata {
    pub id: String,
    pub resolved_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub provenance: Vec<VariableProvenance>,
    pub purged: bool,
}

pub fn snapshot_id_for_run(
    conn: &Connection,
    run_kind: &str,
    run_id: &str,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM execution_variable_snapshots WHERE run_kind=?1 AND run_id=?2",
        params![run_kind, run_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Authorize access to a snapshot for the single-operator boundary already
/// enforced by `auth_middleware`. An opaque run id is not authorization on its
/// own: the snapshot must still map to a live durable owner AND its recorded
/// project scope must match that owner's current project. This is what refuses
/// a cross-project / cross-discussion reveal built from a mismatched id pair.
///
/// Deliberately separate from decryption so API handlers cannot reveal by
/// guessing an otherwise valid snapshot/run id.
pub fn has_live_owner(conn: &Connection, run_kind: &str, run_id: &str) -> Result<bool> {
    let snapshot_project: Option<Option<String>> = conn
        .query_row(
            "SELECT project_id FROM execution_variable_snapshots WHERE run_kind=?1 AND run_id=?2",
            params![run_kind, run_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(snapshot_project) = snapshot_project else {
        return Ok(false);
    };
    let snapshot_project = snapshot_project.as_deref();
    let authorized = match run_kind {
        "quick_prompt" | "quick_prompt_batch_item" => {
            // run_id is the owning discussion. Its project must still match the
            // project scope recorded on the snapshot.
            match owner_project(
                conn,
                "SELECT project_id FROM discussions WHERE id=?1",
                run_id,
            )? {
                Some(owner_project) => owner_project.as_deref() == snapshot_project,
                None => false,
            }
        }
        "workflow" | "quick_prompt_compare" => {
            // run_id is a workflow_run; the project lives on the parent workflow.
            match owner_project(
                conn,
                "SELECT w.project_id FROM workflow_runs r JOIN workflows w ON w.id=r.workflow_id WHERE r.id=?1",
                run_id,
            )? {
                Some(owner_project) => owner_project.as_deref() == snapshot_project,
                None => false,
            }
        }
        // QA/QE calls have no separate run table. A project-backed execution
        // stays tied to that live project; an intentionally projectless
        // execution is still inspectable by the authenticated local operator
        // through its opaque execution id rather than becoming an orphan.
        "quick_api" | "quick_api_batch" | "quick_exec" => match snapshot_project {
            Some(id) => conn
                .query_row("SELECT 1 FROM projects WHERE id=?1", [id], |_| Ok(()))
                .optional()?
                .is_some(),
            None => true,
        },
        // A preview is an authenticated, opaque and short-lived inspection
        // surface. It is scoped to the selected project exactly like a QA/QE
        // run, but is never a durable execution owner.
        "preview" => match snapshot_project {
            Some(id) => conn
                .query_row("SELECT 1 FROM projects WHERE id=?1", [id], |_| Ok(()))
                .optional()?
                .is_some(),
            None => true,
        },
        _ => false,
    };
    Ok(authorized)
}

/// Fetch the (nullable) project id of an owning entity. Returns `None` when the
/// owning row itself does not exist (a deleted discussion/run), and
/// `Some(None)` when it exists but is intentionally projectless.
fn owner_project(conn: &Connection, sql: &str, run_id: &str) -> Result<Option<Option<String>>> {
    conn.query_row(sql, [run_id], |row| row.get::<_, Option<String>>(0))
        .optional()
        .map_err(Into::into)
}

pub struct NewSnapshot<'a> {
    pub run_kind: &'a str,
    pub run_id: &'a str,
    pub project_id: Option<&'a str>,
    pub environment_ref: &'a str,
    pub resolved_at: DateTime<Utc>,
    pub retention_days: u32,
    pub expires_at: Option<DateTime<Utc>>,
    pub values: &'a HashMap<String, String>,
    pub provenance: &'a [VariableProvenance],
}

pub fn insert(conn: &Connection, snapshot: NewSnapshot<'_>, key: &[u8; 32]) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let plaintext = serde_json::to_string(snapshot.values)?;
    let encrypted = crypto::encrypt(&plaintext, key).map_err(anyhow::Error::msg)?;
    let fingerprint = crypto::keyed_digest(
        key,
        b"kronn.execution-variable-snapshot.v1",
        plaintext.as_bytes(),
    );
    let provenance = serde_json::to_string(snapshot.provenance)?;
    conn.execute("INSERT INTO execution_variable_snapshots (id, run_kind, run_id, project_id, environment_ref, resolved_at, retention_days, expires_at, values_encrypted, fingerprint, provenance_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![id, snapshot.run_kind, snapshot.run_id, snapshot.project_id, snapshot.environment_ref, snapshot.resolved_at.to_rfc3339(), snapshot.retention_days, snapshot.expires_at.map(|v| v.to_rfc3339()), encrypted, fingerprint, provenance])?;
    Ok(id)
}

pub fn metadata(
    conn: &Connection,
    run_kind: &str,
    run_id: &str,
) -> Result<Option<SnapshotMetadata>> {
    conn.query_row("SELECT id,resolved_at,expires_at,provenance_json,values_encrypted IS NULL FROM execution_variable_snapshots WHERE run_kind=?1 AND run_id=?2", params![run_kind, run_id], |row| {
        let resolved: String = row.get(1)?; let expires: Option<String> = row.get(2)?; let provenance: String = row.get(3)?;
        Ok(SnapshotMetadata { id: row.get(0)?, resolved_at: DateTime::parse_from_rfc3339(&resolved).map(|v| v.with_timezone(&Utc)).map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?, expires_at: expires.and_then(|v| DateTime::parse_from_rfc3339(&v).ok()).map(|v| v.with_timezone(&Utc)), provenance: serde_json::from_str(&provenance).unwrap_or_default(), purged: row.get(4)? })
    }).optional().map_err(Into::into)
}

/// Persist a value-free execution context card in the owning discussion.
/// The serialized payload contains only names, references and provenance.
pub fn insert_execution_context_message(
    conn: &Connection,
    discussion_id: &str,
    run_kind: &str,
    run_id: &str,
) -> Result<bool> {
    let Some(metadata) = metadata(conn, run_kind, run_id)? else {
        return Ok(false);
    };
    let content = format!(
        "execution_context:{}",
        serde_json::to_string(&serde_json::json!({
            "run_kind": run_kind,
            "run_id": run_id,
            "snapshot_id": metadata.id,
            "resolved_at": metadata.resolved_at,
            "expires_at": metadata.expires_at,
            "variables": metadata.provenance,
            "purged": metadata.purged,
        }))?
    );
    let message = crate::models::DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: Uuid::new_v4().to_string(),
        role: crate::models::MessageRole::System,
        channel: crate::models::MessageChannel::Main,
        content,
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };
    crate::db::discussions::insert_message(conn, discussion_id, &message)?;
    Ok(true)
}

pub fn reveal(
    conn: &Connection,
    snapshot_id: &str,
    variable: &str,
    actor: &str,
    key: &[u8; 32],
    now: DateTime<Utc>,
) -> Result<Option<String>> {
    let encrypted: Option<Option<String>> = conn.query_row("SELECT values_encrypted FROM execution_variable_snapshots WHERE id=?1 AND (expires_at IS NULL OR expires_at>?2)", params![snapshot_id, now.to_rfc3339()], |row| row.get(0)).optional()?;
    let Some(Some(encrypted)) = encrypted else {
        return Ok(None);
    };
    let plaintext = crypto::decrypt(&encrypted, key).map_err(anyhow::Error::msg)?;
    let values: HashMap<String, String> =
        serde_json::from_str(&plaintext).context("invalid snapshot payload")?;
    let value = values.get(variable).cloned();
    if value.is_some() {
        conn.execute("INSERT INTO execution_variable_reveal_audit (id,snapshot_id,variable_name,actor,revealed_at) VALUES (?1,?2,?3,?4,?5)", params![Uuid::new_v4().to_string(), snapshot_id, variable, actor, now.to_rfc3339()])?;
    }
    Ok(value)
}

/// Decrypt the immutable values for execution. Unlike `reveal`, this internal
/// path does not create a human reveal audit row and never returns metadata to
/// an API response.
pub fn load_values(
    conn: &Connection,
    run_kind: &str,
    run_id: &str,
    key: &[u8; 32],
    now: DateTime<Utc>,
) -> Result<Option<HashMap<String, String>>> {
    let encrypted: Option<Option<String>> = conn
        .query_row(
            "SELECT values_encrypted FROM execution_variable_snapshots WHERE run_kind=?1 AND run_id=?2 AND (expires_at IS NULL OR expires_at>?3)",
            params![run_kind, run_id, now.to_rfc3339()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(Some(encrypted)) = encrypted else {
        return Ok(None);
    };
    let plaintext = crypto::decrypt(&encrypted, key).map_err(anyhow::Error::msg)?;
    serde_json::from_str(&plaintext)
        .context("invalid snapshot payload")
        .map(Some)
}

pub fn purge_expired(conn: &Connection, now: DateTime<Utc>) -> Result<usize> {
    Ok(conn.execute("UPDATE execution_variable_snapshots SET values_encrypted=NULL,purged_at=?1 WHERE values_encrypted IS NOT NULL AND expires_at IS NOT NULL AND expires_at<=?1", [now.to_rfc3339()])?)
}

/// Bound a launch-preview snapshot to a short window. Preview ciphertext is
/// diagnostic UI state, not execution history, and can never inherit the
/// normal 30-day retention policy.
pub fn set_preview_expiry(
    conn: &Connection,
    snapshot_id: &str,
    expires_at: DateTime<Utc>,
) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE execution_variable_snapshots
         SET retention_days=0, expires_at=?2
         WHERE id=?1 AND run_kind='preview'",
        params![snapshot_id, expires_at.to_rfc3339()],
    )? > 0)
}

/// Purge snapshots configured with retention=0 once their owning execution
/// reaches a terminal state. Metadata and keyed fingerprint remain.
pub fn purge_run_lifetime_snapshot(
    conn: &Connection,
    run_kind: &str,
    run_id: &str,
    now: DateTime<Utc>,
) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE execution_variable_snapshots SET values_encrypted=NULL,purged_at=?3 WHERE run_kind=?1 AND run_id=?2 AND retention_days=0 AND values_encrypted IS NOT NULL",
        params![run_kind, run_id, now.to_rfc3339()],
    )?)
}

pub fn extend_retention(
    conn: &Connection,
    snapshot_id: &str,
    days: u32,
    actor: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    let previous: Option<Option<String>> = conn
        .query_row(
            "SELECT expires_at FROM execution_variable_snapshots WHERE id=?1 AND values_encrypted IS NOT NULL",
            [snapshot_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(previous) = previous else {
        return Ok(false);
    };
    let requested_expiry = now + chrono::Duration::days(i64::from(days));
    let previous_expiry = previous
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc));
    // An explicit extension is monotone: it can preserve or lengthen the
    // existing retention window, never shorten it as a side effect.
    let new_expiry = previous_expiry
        .filter(|previous| *previous > requested_expiry)
        .unwrap_or(requested_expiry);
    conn.execute(
        "UPDATE execution_variable_snapshots SET expires_at=?2,retention_days=?3 WHERE id=?1",
        params![snapshot_id, new_expiry.to_rfc3339(), days],
    )?;
    conn.execute(
        "INSERT INTO execution_variable_retention_audit (id,snapshot_id,actor,previous_expires_at,new_expires_at,extended_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![Uuid::new_v4().to_string(), snapshot_id, actor, previous, new_expiry.to_rfc3339(), now.to_rfc3339()],
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use sha2::Digest;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn sample<'a>(values: &'a HashMap<String, String>, now: DateTime<Utc>) -> NewSnapshot<'a> {
        NewSnapshot {
            run_kind: "workflow",
            run_id: "run-1",
            project_id: None,
            environment_ref: "project_mcp_configs",
            resolved_at: now,
            retention_days: 30,
            expires_at: Some(now + Duration::days(30)),
            values,
            provenance: &[],
        }
    }

    #[test]
    fn encrypted_snapshot_reveals_with_audit_then_purges_without_leaking_value() {
        let conn = conn();
        let now = Utc::now();
        let values = HashMap::from([("token".into(), "small-secret".into())]);
        let key = [7u8; 32];
        let id = insert(&conn, sample(&values, now), &key).unwrap();
        let (ciphertext, fingerprint): (String, String) = conn
            .query_row(
                "SELECT values_encrypted,fingerprint FROM execution_variable_snapshots WHERE id=?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(!ciphertext.contains("small-secret"));
        let bare_hash = sha2::Sha256::digest(b"{\"token\":\"small-secret\"}")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_ne!(fingerprint, bare_hash);
        assert_eq!(
            reveal(&conn, &id, "token", "tester", &key, now)
                .unwrap()
                .as_deref(),
            Some("small-secret")
        );
        assert_eq!(
            load_values(&conn, "workflow", "run-1", &key, now)
                .unwrap()
                .unwrap()["token"],
            "small-secret"
        );
        let audit: String = conn.query_row(
            "SELECT variable_name || ':' || actor FROM execution_variable_reveal_audit WHERE snapshot_id=?1",
            [&id], |row| row.get(0),
        ).unwrap();
        assert_eq!(audit, "token:tester");
        assert!(!audit.contains("small-secret"));
        assert_eq!(purge_expired(&conn, now + Duration::days(31)).unwrap(), 1);
        assert!(reveal(
            &conn,
            &id,
            "token",
            "tester",
            &key,
            now + Duration::days(31)
        )
        .unwrap()
        .is_none());
        assert!(
            metadata(&conn, "workflow", "run-1")
                .unwrap()
                .unwrap()
                .purged
        );
    }

    #[test]
    fn fingerprint_changes_with_active_key_policy() {
        let now = Utc::now();
        let values = HashMap::from([("token".into(), "guessable".into())]);
        let first = conn();
        insert(&first, sample(&values, now), &[1u8; 32]).unwrap();
        let a: String = first
            .query_row(
                "SELECT fingerprint FROM execution_variable_snapshots",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let second = conn();
        insert(&second, sample(&values, now), &[2u8; 32]).unwrap();
        let b: String = second
            .query_row(
                "SELECT fingerprint FROM execution_variable_snapshots",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn retention_extension_is_explicit_audited_and_value_free() {
        let conn = conn();
        let now = Utc::now();
        let values = HashMap::from([("token".into(), "never-in-audit".into())]);
        let id = insert(&conn, sample(&values, now), &[3u8; 32]).unwrap();
        assert!(extend_retention(&conn, &id, 60, "operator-1", now).unwrap());
        let audit: String = conn
            .query_row(
                "SELECT actor || ':' || new_expires_at FROM execution_variable_retention_audit WHERE snapshot_id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(audit.starts_with("operator-1:"));
        assert!(!audit.contains("never-in-audit"));
    }

    #[test]
    fn retention_extension_never_shortens_an_existing_window() {
        let conn = conn();
        let now = Utc::now();
        let values = HashMap::from([("token".into(), "retained".into())]);
        let id = insert(&conn, sample(&values, now), &[9u8; 32]).unwrap();
        // The original expiry is 30 days away. A one-day extension is an
        // explicit audit event but must not accidentally reduce retention.
        assert!(extend_retention(&conn, &id, 1, "operator-1", now).unwrap());
        let expires: String = conn
            .query_row(
                "SELECT expires_at FROM execution_variable_snapshots WHERE id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        let expires = DateTime::parse_from_rfc3339(&expires)
            .unwrap()
            .with_timezone(&Utc);
        assert!(expires >= now + Duration::days(30));
    }

    #[test]
    fn zero_retention_is_purged_at_run_lifetime_end() {
        let conn = conn();
        let now = Utc::now();
        let values = HashMap::from([("token".into(), "ephemeral".into())]);
        let mut snapshot = sample(&values, now);
        snapshot.retention_days = 0;
        snapshot.expires_at = None;
        insert(&conn, snapshot, &[4u8; 32]).unwrap();
        assert!(load_values(&conn, "workflow", "run-1", &[4u8; 32], now)
            .unwrap()
            .is_some());
        assert_eq!(
            purge_run_lifetime_snapshot(&conn, "workflow", "run-1", now).unwrap(),
            1
        );
        assert!(load_values(&conn, "workflow", "run-1", &[4u8; 32], now)
            .unwrap()
            .is_none());
    }

    fn project_snapshot<'a>(
        run_kind: &'a str,
        run_id: &'a str,
        project_id: Option<&'a str>,
        values: &'a HashMap<String, String>,
        now: DateTime<Utc>,
    ) -> NewSnapshot<'a> {
        NewSnapshot {
            run_kind,
            run_id,
            project_id,
            environment_ref: "project_mcp_configs",
            resolved_at: now,
            retention_days: 30,
            expires_at: Some(now + Duration::days(30)),
            values,
            provenance: &[],
        }
    }

    #[test]
    fn has_live_owner_refuses_cross_project_and_cross_discussion_ids() {
        let conn = conn();
        let now = Utc::now();
        conn.execute(
            "INSERT INTO projects (id,name,path,created_at,updated_at) VALUES ('p1','p1','/tmp/p1',?1,?1),('p2','p2','/tmp/p2',?1,?1)",
            [now.to_rfc3339()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussions (id,title,agent,language,created_at,updated_at,project_id) VALUES ('d1','d','Codex','en',?1,?1,'p1')",
            [now.to_rfc3339()],
        )
        .unwrap();
        let values = HashMap::from([("token".into(), "secret".into())]);
        // Snapshot recorded for project p2 but the owning discussion belongs to
        // p1 — a mismatched (run_id, project) pair must not authorize access.
        insert(
            &conn,
            project_snapshot("quick_prompt", "d1", Some("p2"), &values, now),
            &[1u8; 32],
        )
        .unwrap();
        assert!(!has_live_owner(&conn, "quick_prompt", "d1").unwrap());
        // A snapshot whose owning discussion does not exist at all is refused.
        insert(
            &conn,
            project_snapshot("quick_prompt", "ghost", Some("p1"), &values, now),
            &[1u8; 32],
        )
        .unwrap();
        assert!(!has_live_owner(&conn, "quick_prompt", "ghost").unwrap());
    }

    #[test]
    fn has_live_owner_authorizes_matching_project_and_projectless_qa_qe() {
        let conn = conn();
        let now = Utc::now();
        conn.execute(
            "INSERT INTO projects (id,name,path,created_at,updated_at) VALUES ('p1','p1','/tmp/p1',?1,?1)",
            [now.to_rfc3339()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussions (id,title,agent,language,created_at,updated_at,project_id) VALUES ('d1','d','Codex','en',?1,?1,'p1')",
            [now.to_rfc3339()],
        )
        .unwrap();
        let values = HashMap::from([("token".into(), "secret".into())]);
        insert(
            &conn,
            project_snapshot("quick_prompt", "d1", Some("p1"), &values, now),
            &[1u8; 32],
        )
        .unwrap();
        assert!(has_live_owner(&conn, "quick_prompt", "d1").unwrap());
        // A projectless QA/QE execution stays inspectable by the authenticated
        // local operator through its opaque execution id.
        insert(
            &conn,
            project_snapshot("quick_exec", "qe-run-1", None, &values, now),
            &[1u8; 32],
        )
        .unwrap();
        assert!(has_live_owner(&conn, "quick_exec", "qe-run-1").unwrap());
    }

    #[test]
    fn execution_context_card_contains_metadata_but_never_plaintext() {
        let conn = conn();
        let now = Utc::now();
        conn.execute(
            "INSERT INTO discussions (id,title,agent,language,created_at,updated_at) VALUES ('run-1','run','Codex','en',?1,?1)",
            [now.to_rfc3339()],
        )
        .unwrap();
        let values = HashMap::from([("token".into(), "card-secret".into())]);
        insert(&conn, sample(&values, now), &[5u8; 32]).unwrap();
        assert!(insert_execution_context_message(&conn, "run-1", "workflow", "run-1").unwrap());
        let content: String = conn
            .query_row(
                "SELECT content FROM messages WHERE discussion_id='run-1' AND role='System'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(content.starts_with("execution_context:"));
        assert!(!content.contains("card-secret"));
    }
}
