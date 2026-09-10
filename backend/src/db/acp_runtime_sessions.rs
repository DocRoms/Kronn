use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

const MAX_CONVERSATION_ID_BYTES: usize = 512;

#[derive(Debug, Clone)]
pub struct SessionKey {
    pub discussion_id: String,
    pub agent_type: String,
    pub runtime: String,
    pub project_scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedCheckpoint {
    pub conversation_id: String,
    pub input_message_id: String,
    pub output_message_id: String,
}

#[derive(Debug, Clone)]
pub struct TurnCompletion {
    pub key: SessionKey,
    pub turn_id: String,
    pub input_message_id: String,
    pub output_message_id: String,
}

pub fn get_completed_checkpoint(
    conn: &Connection,
    key: &SessionKey,
) -> Result<Option<CompletedCheckpoint>> {
    conn.query_row(
        "SELECT conversation_id, last_seen_message_id, last_output_message_id
         FROM acp_runtime_sessions
         WHERE discussion_id = ?1 AND agent_type = ?2 AND runtime = ?3
           AND project_scope = ?4 AND active_turn_id IS NULL
           AND last_seen_message_id IS NOT NULL AND last_output_message_id IS NOT NULL",
        params![
            key.discussion_id,
            key.agent_type,
            key.runtime,
            key.project_scope
        ],
        |row| {
            Ok(CompletedCheckpoint {
                conversation_id: row.get(0)?,
                input_message_id: row.get(1)?,
                output_message_id: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// Retire old progress before a direct CLI can start processing another turn.
/// Its init event will supply the session identity; absent init is not proof.
pub fn invalidate_checkpoint(conn: &Connection, key: &SessionKey) -> Result<()> {
    conn.execute(
        "UPDATE acp_runtime_sessions
         SET last_seen_message_id = NULL, last_output_message_id = NULL,
             active_turn_id = NULL, updated_at = ?5
         WHERE discussion_id = ?1 AND agent_type = ?2 AND runtime = ?3 AND project_scope = ?4",
        params![
            key.discussion_id,
            key.agent_type,
            key.runtime,
            key.project_scope,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

/// Acquire a new per-turn identity before ACP dispatch or after CLI init.
pub fn begin_turn(
    conn: &Connection,
    key: &SessionKey,
    conversation_id: &str,
    turn_id: &str,
) -> Result<()> {
    validate_component("turn id", turn_id)?;
    let tx = conn.unchecked_transaction()?;
    upsert(
        &tx,
        &key.discussion_id,
        &key.agent_type,
        &key.runtime,
        &key.project_scope,
        conversation_id,
    )?;
    tx.execute(
        "UPDATE acp_runtime_sessions
         SET last_seen_message_id = NULL, last_output_message_id = NULL,
             active_turn_id = ?5
         WHERE discussion_id = ?1 AND agent_type = ?2 AND runtime = ?3 AND project_scope = ?4",
        params![
            key.discussion_id,
            key.agent_type,
            key.runtime,
            key.project_scope,
            turn_id
        ],
    )?;
    tx.commit()?;
    Ok(())
}

/// Adapter events may replace their provisional id. A delayed event from an
/// older turn must not replace the identity or checkpoint of a newer one.
pub fn update_turn_session(
    conn: &Connection,
    key: &SessionKey,
    turn_id: &str,
    conversation_id: &str,
) -> Result<()> {
    validate_component("conversation id", conversation_id)?;
    let changed = conn.execute(
        "UPDATE acp_runtime_sessions SET conversation_id = ?6, updated_at = ?7
         WHERE discussion_id = ?1 AND agent_type = ?2 AND runtime = ?3
           AND project_scope = ?4 AND active_turn_id = ?5",
        params![
            key.discussion_id,
            key.agent_type,
            key.runtime,
            key.project_scope,
            turn_id,
            conversation_id,
            Utc::now().to_rfc3339()
        ],
    )?;
    if changed != 1 {
        return Err(anyhow!("ACP turn no longer owns its session checkpoint"));
    }
    Ok(())
}

/// Called inside the SAME transaction as the response insertion. A stale turn
/// still has a visible response, but cannot advance a replacement's frontier.
pub fn complete_turn(conn: &Connection, completion: &TurnCompletion) -> Result<bool> {
    let key = &completion.key;
    let changed = conn.execute(
        "UPDATE acp_runtime_sessions
         SET last_seen_message_id = ?6, last_output_message_id = ?7,
             active_turn_id = NULL, updated_at = ?8
         WHERE discussion_id = ?1 AND agent_type = ?2 AND runtime = ?3
           AND project_scope = ?4 AND active_turn_id = ?5",
        params![
            key.discussion_id,
            key.agent_type,
            key.runtime,
            key.project_scope,
            completion.turn_id,
            completion.input_message_id,
            completion.output_message_id,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(changed == 1)
}

fn validate_component(label: &str, value: &str) -> Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_CONVERSATION_ID_BYTES {
        return Err(anyhow!("invalid ACP {label}"));
    }
    if trimmed != value || value.chars().any(char::is_control) {
        return Err(anyhow!("invalid ACP {label}"));
    }
    Ok(())
}

pub fn get(
    conn: &Connection,
    discussion_id: &str,
    agent_type: &str,
    runtime: &str,
    project_scope: &str,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT conversation_id
           FROM acp_runtime_sessions
          WHERE discussion_id = ?1
            AND agent_type = ?2
            AND runtime = ?3
            AND project_scope = ?4",
        params![discussion_id, agent_type, runtime, project_scope],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn upsert(
    conn: &Connection,
    discussion_id: &str,
    agent_type: &str,
    runtime: &str,
    project_scope: &str,
    conversation_id: &str,
) -> Result<()> {
    validate_component("discussion id", discussion_id)?;
    validate_component("agent type", agent_type)?;
    validate_component("runtime", runtime)?;
    validate_component("project scope", project_scope)?;
    validate_component("conversation id", conversation_id)?;
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO acp_runtime_sessions (
             discussion_id, agent_type, runtime, project_scope,
             conversation_id, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(discussion_id, agent_type, runtime) DO UPDATE SET
             project_scope = excluded.project_scope,
             conversation_id = excluded.conversation_id,
             last_seen_message_id = CASE
                 WHEN acp_runtime_sessions.project_scope = excluded.project_scope
                  AND acp_runtime_sessions.conversation_id = excluded.conversation_id
                 THEN acp_runtime_sessions.last_seen_message_id
                 ELSE NULL
             END,
             last_output_message_id = CASE
                 WHEN acp_runtime_sessions.project_scope = excluded.project_scope
                  AND acp_runtime_sessions.conversation_id = excluded.conversation_id
                 THEN acp_runtime_sessions.last_output_message_id
                 ELSE NULL
             END,
             updated_at = excluded.updated_at",
        params![
            discussion_id,
            agent_type,
            runtime,
            project_scope,
            conversation_id,
            now
        ],
    )?;
    Ok(())
}

/// The conversation id AND the last message this agent was shown.
///
/// Legacy inspection API. Production continuation requires both boundaries
/// from [`get_completed_checkpoint`], not this unqualified input cursor alone.
#[cfg(test)]
fn get_with_progress(
    conn: &Connection,
    discussion_id: &str,
    agent_type: &str,
    runtime: &str,
    project_scope: &str,
) -> Result<Option<(String, Option<String>)>> {
    conn.query_row(
        "SELECT conversation_id, last_seen_message_id
           FROM acp_runtime_sessions
          WHERE discussion_id = ?1
            AND agent_type = ?2
            AND runtime = ?3
            AND project_scope = ?4",
        params![discussion_id, agent_type, runtime, project_scope],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

/// Reproduce the legacy input-only state for migration regression tests.
/// Production must use the transactional completed-turn proof instead.
#[cfg(test)]
fn record_progress(
    conn: &Connection,
    discussion_id: &str,
    agent_type: &str,
    runtime: &str,
    project_scope: &str,
    last_seen_message_id: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE acp_runtime_sessions
            SET last_seen_message_id = ?5, updated_at = ?6
          WHERE discussion_id = ?1
            AND agent_type = ?2
            AND runtime = ?3
            AND project_scope = ?4",
        params![
            discussion_id,
            agent_type,
            runtime,
            project_scope,
            last_seen_message_id,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE discussions (id TEXT PRIMARY KEY);
             INSERT INTO discussions(id) VALUES ('disc-1');
             CREATE TABLE acp_runtime_sessions (
                 discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
                 agent_type TEXT NOT NULL,
                 runtime TEXT NOT NULL,
                 project_scope TEXT NOT NULL,
                 conversation_id TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 last_seen_message_id TEXT,
                 last_output_message_id TEXT,
                 active_turn_id TEXT,
                 PRIMARY KEY (discussion_id, agent_type, runtime)
             );",
        )
        .unwrap();
        conn
    }

    fn native_key() -> SessionKey {
        SessionKey {
            discussion_id: "disc-1".into(),
            agent_type: "OpenCode".into(),
            runtime: "opencode_acp_v1".into(),
            project_scope: "/project/a".into(),
        }
    }

    fn proof(key: &SessionKey, turn: &str, input: &str, output: &str) -> TurnCompletion {
        TurnCompletion {
            key: key.clone(),
            turn_id: turn.into(),
            input_message_id: input.into(),
            output_message_id: output.into(),
        }
    }

    #[test]
    fn completed_checkpoint_is_scoped_and_a_new_turn_invalidates_it_before_prompt() {
        let conn = connection();
        let key = native_key();
        begin_turn(&conn, &key, "session-1", "turn-1").unwrap();
        assert!(get_completed_checkpoint(&conn, &key).unwrap().is_none());
        let first = proof(&key, "turn-1", "input-1", "output-1");
        assert!(complete_turn(&conn, &first).unwrap());
        assert_eq!(
            get_completed_checkpoint(&conn, &key).unwrap(),
            Some(CompletedCheckpoint {
                conversation_id: "session-1".into(),
                input_message_id: "input-1".into(),
                output_message_id: "output-1".into(),
            })
        );
        for foreign in [
            SessionKey {
                discussion_id: "other-discussion".into(),
                ..key.clone()
            },
            SessionKey {
                agent_type: "ClaudeCode".into(),
                ..key.clone()
            },
            SessionKey {
                runtime: "other-runtime".into(),
                ..key.clone()
            },
            SessionKey {
                project_scope: "/project/b".into(),
                ..key.clone()
            },
        ] {
            assert!(get_completed_checkpoint(&conn, &foreign).unwrap().is_none());
        }

        begin_turn(&conn, &key, "session-1", "turn-2").unwrap();
        assert!(
            get_completed_checkpoint(&conn, &key).unwrap().is_none(),
            "crash/failed turn has no certified frontier"
        );
        assert!(
            !complete_turn(&conn, &first).unwrap(),
            "a late old completion cannot certify the new turn"
        );
        assert!(update_turn_session(&conn, &key, "turn-1", "stale-adapter-id").is_err());
        update_turn_session(&conn, &key, "turn-2", "resolved-adapter-id").unwrap();
        assert!(complete_turn(&conn, &proof(&key, "turn-2", "input-2", "output-2")).unwrap());
        assert_eq!(
            get_completed_checkpoint(&conn, &key)
                .unwrap()
                .unwrap()
                .conversation_id,
            "resolved-adapter-id"
        );
    }

    #[test]
    fn a_legacy_input_cursor_is_not_a_complete_checkpoint_and_replacement_clears_both_boundaries() {
        let conn = connection();
        let key = native_key();
        upsert(
            &conn,
            &key.discussion_id,
            &key.agent_type,
            &key.runtime,
            &key.project_scope,
            "legacy",
        )
        .unwrap();
        record_progress(
            &conn,
            &key.discussion_id,
            &key.agent_type,
            &key.runtime,
            &key.project_scope,
            "old-marker",
        )
        .unwrap();
        assert!(get_completed_checkpoint(&conn, &key).unwrap().is_none());
        begin_turn(&conn, &key, "complete", "turn").unwrap();
        complete_turn(&conn, &proof(&key, "turn", "input", "output")).unwrap();
        upsert(
            &conn,
            &key.discussion_id,
            &key.agent_type,
            &key.runtime,
            &key.project_scope,
            "replacement",
        )
        .unwrap();
        assert!(get_completed_checkpoint(&conn, &key).unwrap().is_none());
        let output: Option<String> = conn
            .query_row(
                "SELECT last_output_message_id FROM acp_runtime_sessions",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(output.is_none());
    }

    #[tokio::test]
    async fn native_response_and_completed_checkpoint_roll_back_together() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute("INSERT INTO discussions (id, title, agent, created_at, updated_at) VALUES ('disc-1', 'Atomic checkpoint', 'OpenCode', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')", [])?;
            let key = native_key();
            begin_turn(conn, &key, "session", "turn")?;
            let completion = proof(&key, "turn", "input", "native-output");
            let reply: crate::models::DiscussionMessage = serde_json::from_value(serde_json::json!({
                "id": "native-output", "role": "Agent", "content": "native response", "agent_type": "OpenCode",
                "timestamp": "2026-01-01T00:00:00Z"
            }))?;
            conn.execute_batch("CREATE TRIGGER fail_completed_checkpoint BEFORE UPDATE OF last_seen_message_id ON acp_runtime_sessions WHEN NEW.last_seen_message_id IS NOT NULL BEGIN SELECT RAISE(FAIL, 'checkpoint failure fixture'); END;")?;
            assert!(crate::db::discussions::insert_native_agent_message_with_checkpoint(
                conn, "disc-1", &reply, true, None, &crate::models::AgentType::OpenCode, &[], false, None, Some(&completion),
            ).is_err());
            let replies: i64 = conn.query_row("SELECT COUNT(*) FROM messages WHERE id='native-output'", [], |row| row.get(0))?;
            assert_eq!(replies, 0, "checkpoint failure must roll back the response too");
            assert!(get_completed_checkpoint(conn, &key)?.is_none());
            conn.execute_batch("DROP TRIGGER fail_completed_checkpoint;")?;
            crate::db::discussions::insert_native_agent_message_with_checkpoint(
                conn, "disc-1", &reply, true, None, &crate::models::AgentType::OpenCode, &[], false, None, Some(&completion),
            )?;
            assert_eq!(get_completed_checkpoint(conn, &key)?.unwrap().output_message_id, reply.id);
            Ok(())
        }).await.unwrap();
    }

    #[test]
    fn upsert_is_idempotent_and_scope_changes_do_not_resume_the_old_project() {
        let conn = connection();
        upsert(
            &conn,
            "disc-1",
            "Codex",
            "codex_cli_adapter_v1",
            "/project/a",
            "thread-a",
        )
        .unwrap();
        assert_eq!(
            get(
                &conn,
                "disc-1",
                "Codex",
                "codex_cli_adapter_v1",
                "/project/a"
            )
            .unwrap()
            .as_deref(),
            Some("thread-a")
        );
        assert_eq!(
            get(
                &conn,
                "disc-1",
                "Codex",
                "codex_cli_adapter_v1",
                "/project/b"
            )
            .unwrap(),
            None
        );

        upsert(
            &conn,
            "disc-1",
            "Codex",
            "codex_cli_adapter_v1",
            "/project/a",
            "thread-b",
        )
        .unwrap();
        assert_eq!(
            get(
                &conn,
                "disc-1",
                "Codex",
                "codex_cli_adapter_v1",
                "/project/a"
            )
            .unwrap()
            .as_deref(),
            Some("thread-b")
        );
    }

    #[test]
    fn invalid_or_control_character_ids_are_refused() {
        let conn = connection();
        for id in ["", " ", "thread\nsecret", " thread"] {
            assert!(upsert(
                &conn,
                "disc-1",
                "ClaudeCode",
                "claude_cli_adapter_v1",
                "/project/a",
                id,
            )
            .is_err());
        }
    }

    #[test]
    fn replacing_a_session_or_scope_clears_its_old_progress_cursor() {
        let conn = connection();
        upsert(
            &conn,
            "disc-1",
            "OpenCode",
            "opencode_acp_v1",
            "/project/a",
            "old",
        )
        .unwrap();
        record_progress(
            &conn,
            "disc-1",
            "OpenCode",
            "opencode_acp_v1",
            "/project/a",
            "m2",
        )
        .unwrap();

        upsert(
            &conn,
            "disc-1",
            "OpenCode",
            "opencode_acp_v1",
            "/project/a",
            "replacement",
        )
        .unwrap();
        assert_eq!(
            get_with_progress(&conn, "disc-1", "OpenCode", "opencode_acp_v1", "/project/a")
                .unwrap(),
            Some(("replacement".into(), None))
        );

        record_progress(
            &conn,
            "disc-1",
            "OpenCode",
            "opencode_acp_v1",
            "/project/a",
            "m3",
        )
        .unwrap();
        upsert(
            &conn,
            "disc-1",
            "OpenCode",
            "opencode_acp_v1",
            "/project/b",
            "replacement",
        )
        .unwrap();
        assert_eq!(
            get_with_progress(&conn, "disc-1", "OpenCode", "opencode_acp_v1", "/project/b")
                .unwrap(),
            Some(("replacement".into(), None))
        );
    }
}
