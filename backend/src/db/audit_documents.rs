//! Local final-document proof. Git metadata and human provenance cannot
//! silently replace the content a run or its linked validation checked.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::core::audit_documents::Fingerprints;

pub fn get(conn: &Connection, run_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT final_doc_fingerprints_json FROM audit_runs WHERE id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

/// Called inside the terminal transaction, after its Completed transition.
pub fn record(conn: &Connection, run_id: &str, fingerprints: &str) -> Result<()> {
    let _: Fingerprints = serde_json::from_str(fingerprints)?;
    let updated = conn.execute(
        "UPDATE audit_runs SET final_doc_fingerprints_json = ?2
         WHERE id = ?1 AND status = 'Completed'",
        params![run_id, fingerprints],
    )?;
    anyhow::ensure!(
        updated == 1,
        "Document fingerprints require a completed audit run"
    );
    Ok(())
}

/// The caller persists this proof atomically with the successful terminal
/// validation reply. An unlinked/title-only discussion certifies no run.
pub fn finish_validation(conn: &Connection, discussion_id: &str, fingerprints: &str) -> Result<()> {
    let _: Fingerprints = serde_json::from_str(fingerprints)?;
    conn.execute(
        "UPDATE audit_runs SET final_doc_fingerprints_json = ?2
         WHERE validation_discussion_id = ?1 AND status = 'Completed'",
        params![discussion_id, fingerprints],
    )?;
    Ok(())
}

/// The latest run is authoritative even when it is interrupted. Never borrow
/// proof from an older completed run after a new audit changed the docs.
pub fn latest(conn: &Connection, project_id: &str) -> Result<Option<String>> {
    let Some(run) = super::audit_runs::list_recent(conn, project_id, 1)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    if run.status != "Completed" {
        return Ok(None);
    }
    get(conn, &run.id)
}

/// An explicit local confirmation accepts only the documents the person
/// selected. Committable provenance files alone never invoke this operation.
pub fn confirm_paths(
    conn: &Connection,
    project_id: &str,
    docs: &[String],
    current: &Fingerprints,
) -> Result<()> {
    let Some(run) = super::audit_runs::list_recent(conn, project_id, 1)?
        .into_iter()
        .next()
    else {
        return Ok(());
    };
    if run.status != "Completed" {
        return Ok(());
    }
    let Some(raw) = get(conn, &run.id)? else {
        return Ok(());
    };
    let mut recorded: Fingerprints = serde_json::from_str(&raw)?;
    for doc in docs {
        if let Some(hash) = current.get(doc) {
            recorded.insert(doc.clone(), hash.clone());
        } else {
            recorded.remove(doc);
        }
    }
    record(conn, &run.id, &serde_json::to_string(&recorded)?)
}

/// Accept only this local operation's changes to documents whose BEFORE
/// fingerprint still matched the run. An unrelated pull, including one into
/// the same sheet before a TD decision, cannot be laundered by that decision.
pub fn advance_local_changes(
    conn: &Connection,
    project_id: &str,
    before: &Fingerprints,
    after: &Fingerprints,
) -> Result<()> {
    let Some(run) = super::audit_runs::list_recent(conn, project_id, 1)?
        .into_iter()
        .next()
    else {
        return Ok(());
    };
    if run.status != "Completed" {
        return Ok(());
    }
    let Some(raw) = get(conn, &run.id)? else {
        return Ok(());
    };
    let mut recorded: Fingerprints = serde_json::from_str(&raw)?;
    for path in before.keys().chain(after.keys()) {
        if before.get(path) == after.get(path) || recorded.get(path) != before.get(path) {
            continue;
        }
        if let Some(hash) = after.get(path) {
            recorded.insert(path.clone(), hash.clone());
        } else {
            recorded.remove(path);
        }
    }
    record(conn, &run.id, &serde_json::to_string(&recorded)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn final_documents_and_the_terminal_reply_commit_or_roll_back_together() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute_batch("INSERT INTO projects (id,name,path,created_at,updated_at)
                VALUES ('p','P','/fixture',datetime('now'),datetime('now'));
                INSERT INTO discussions (id,project_id,title,agent,language,created_at,updated_at)
                VALUES ('d','p','Validation audit','ClaudeCode','en',datetime('now'),datetime('now'));
                INSERT INTO discussions (id,project_id,title,agent,language,created_at,updated_at)
                VALUES ('forged','p','Validation audit','ClaudeCode','en',datetime('now'),datetime('now'));
                INSERT INTO audit_runs (id,project_id,kind,agent_type,started_at,status,validation_discussion_id)
                VALUES ('r','p','Full','ClaudeCode',datetime('now'),'Completed','d');")?;
            assert!(get(conn, "r")?.is_none(), "old runs acquire no guessed proof");
            record(conn, "r", "{}")?;
            assert!(record(conn, "missing", "{}").is_err());
            let documents = serde_json::json!({"docs/AGENTS.md": "a".repeat(64)}).to_string();
            finish_validation(conn, "forged", &documents)?;
            assert_eq!(get(conn, "r")?.as_deref(), Some("{}"));
            let mut message: crate::models::DiscussionMessage = serde_json::from_value(serde_json::json!({
                "id":"final", "role":"Agent", "content":"KRONN:VALIDATION_COMPLETE",
                "timestamp":"2026-10-13T00:00:00Z"
            }))?;
            conn.execute_batch("CREATE TRIGGER reject_reply BEFORE INSERT ON messages
                BEGIN SELECT RAISE(ABORT, 'reply write failed'); END;")?;
            assert!(super::super::discussions::insert_native_agent_message_with_audit_documents(
                conn,"d",&message,true,None,&crate::models::AgentType::ClaudeCode,&[],false,None,None,Some(&documents)).is_err());
            assert_eq!(get(conn, "r")?.as_deref(), Some("{}"), "failed reply cannot advance proof");
            conn.execute_batch("DROP TRIGGER reject_reply;")?;
            message.content = "Validation still running".into();
            assert!(super::super::discussions::insert_native_agent_message_with_audit_documents(
                conn,"d",&message,true,None,&crate::models::AgentType::ClaudeCode,&[],false,None,None,Some(&documents)).is_err());
            message.content = "KRONN:VALIDATION_COMPLETE".into();
            super::super::discussions::insert_native_agent_message_with_audit_documents(
                conn,"d",&message,true,None,&crate::models::AgentType::ClaudeCode,&[],false,None,None,Some(&documents))?;
            assert_eq!(get(conn, "r")?.as_deref(), Some(documents.as_str()));
            Ok(())
        }).await.unwrap();
    }
}
