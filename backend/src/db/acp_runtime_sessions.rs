use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

const MAX_CONVERSATION_ID_BYTES: usize = 512;

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
                 PRIMARY KEY (discussion_id, agent_type, runtime)
             );",
        )
        .unwrap();
        conn
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
}
