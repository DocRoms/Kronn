//! Briefing completion must be durable before its discussion is archived.
use crate::models::AgentType;
use std::sync::Arc;

struct Recording(std::path::PathBuf, Option<&'static str>);

#[async_trait::async_trait]
impl crate::acp::AcpTransport for Recording {
    async fn initialize(
        &self,
        _: crate::acp::AcpInitialize,
    ) -> Result<crate::acp::AcpNegotiatedCapabilities, crate::acp::AcpError> {
        Ok(crate::acp::AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities: std::collections::BTreeSet::from([
                crate::acp::AcpCapability::Sessions,
                crate::acp::AcpCapability::Streaming,
                crate::acp::AcpCapability::Cancellation,
                crate::acp::AcpCapability::McpInjection,
            ]),
        })
    }
    async fn create_session(&self) -> Result<crate::acp::AcpSessionTarget, crate::acp::AcpError> {
        crate::acp::AcpSessionTarget::new(crate::acp::AcpAgent::ClaudeCode, "pinned-child")
    }
    async fn config_options(&self) -> Vec<crate::acp::AcpConfigOption> {
        Vec::new()
    }
    async fn set_config_option(
        &self,
        _: &crate::acp::AcpSessionTarget,
        _: &str,
        _: &str,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn resume_session(
        &self,
        _: &crate::acp::AcpSessionTarget,
    ) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn prompt(
        &self,
        _: &crate::acp::AcpSessionTarget,
        prompt: &str,
        events: tokio::sync::mpsc::Sender<crate::acp::AcpSessionEvent>,
    ) -> Result<(), crate::acp::AcpError> {
        let _ = prompt;
        if let Some(text) = self.1 {
            std::fs::write(&self.0, text).unwrap();
        } else {
            std::fs::remove_file(&self.0).unwrap();
        }
        let _ = events
            .send(crate::acp::AcpSessionEvent::TextDelta(
                "Saved.\nKRONN:BRIEFING_COMPLETE".into(),
            ))
            .await;
        let _ = events.send(crate::acp::AcpSessionEvent::Completed).await;
        Ok(())
    }
    async fn cancel(&self, _: &crate::acp::AcpSessionTarget) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), crate::acp::AcpError> {
        Ok(())
    }
}

#[tokio::test]
#[serial_test::serial]
async fn briefing_completion_requires_stored_text_before_archiving() {
    use axum::response::IntoResponse;
    let _data_dir = crate::core::config::TestDataDir::new();
    for (stored, reject_save) in [
        (None, false),
        (Some("   "), false),
        (Some("# Briefing\nClarified answers.\n"), false),
        (Some("# Briefing\nClarified answers.\n"), true),
    ] {
        let project = tempfile::tempdir().unwrap();
        let project_path = project.path().to_string_lossy().into_owned();
        std::fs::create_dir_all(project.path().join("docs")).unwrap();
        let file = project.path().join("docs/briefing.md");
        let previous =
            "# Briefing\nSaved answers.\n\n## Kronn options\n- Keep this user option: yes\n";
        std::fs::write(&file, previous).unwrap();
        let work_dir =
            crate::agents::runner::resolve_agent_work_dir(Some(&project_path), &project_path)
                .unwrap();
        let _route = crate::agents::runner::test_acp_routes::route(
            &work_dir,
            Arc::new(Recording(file.clone(), stored)),
        );
        let _saved = crate::core::config::test_saved_access::set(&AgentType::ClaudeCode, true);
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        db.with_conn(move |conn| {
            conn.execute("INSERT INTO projects (id, name, path, briefing_notes, created_at, updated_at) VALUES ('p', 'P', ?1, ?2, 'now', 'now')", rusqlite::params![project_path, "# Stale DB\n\n## Kronn options\n- Stale option: no\n"])?;
            Ok(())
        }).await.unwrap();
        let mut config = crate::core::config::default_config();
        config.agents.claude_code.full_access = true;
        let state = crate::AppState::new_defaults(
            Arc::new(tokio::sync::RwLock::new(config)),
            db.clone(),
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let axum::Json(started) = crate::api::audit::briefing::start_briefing(
            axum::extract::State(state.clone()),
            axum::extract::Path("p".into()),
            axum::Json(serde_json::from_value(serde_json::json!({"agent": "ClaudeCode"})).unwrap()),
        )
        .await;
        assert!(started.success, "{:?}", started.error);
        let discussion_id = started.data.unwrap().discussion_id;
        if reject_save {
            db.with_conn(|conn| {
                conn.execute_batch("CREATE TRIGGER reject_briefing_save BEFORE UPDATE OF briefing_notes ON projects BEGIN SELECT RAISE(ABORT, 'briefing storage unavailable'); END;")?;
                Ok(())
            }).await.unwrap();
        }
        let response = super::make_agent_stream_inner(
            state,
            discussion_id.clone(),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&body);
        let (archived, content, notes) = db.with_conn(move |conn| {
            let archived = conn.query_row("SELECT archived FROM discussions WHERE id = ?1", [&discussion_id], |row| row.get::<_, bool>(0))?;
            let content = conn.query_row("SELECT content FROM messages WHERE discussion_id = ?1 AND role = 'Agent' ORDER BY sort_order DESC LIMIT 1", [&discussion_id], |row| row.get::<_, String>(0))?;
            let notes = crate::db::projects::get_project_briefing_notes(conn, "p")?
                .ok_or_else(|| anyhow::anyhow!("expected stored briefing notes"))?;
            Ok((archived, content, notes))
        }).await.unwrap();
        let expected_success = !reject_save && stored.is_some_and(|text| !text.trim().is_empty());
        assert_eq!(archived, expected_success, "{body}");
        if expected_success {
            assert!(content.contains("KRONN:BRIEFING_COMPLETE"));
            assert!(notes.contains("Clarified answers."));
            assert!(notes.contains("Keep this user option: yes"));
            assert!(!notes.contains("Stale option"));
            assert_eq!(std::fs::read_to_string(&file).unwrap(), notes);
        } else {
            assert!(
                content.contains("Briefing could not be completed"),
                "{content}"
            );
            assert!(!content.contains("KRONN:BRIEFING_COMPLETE"));
            assert_eq!(notes, previous);
            assert!(body.contains("\"success\":false"), "{body}");
        }
    }
}
