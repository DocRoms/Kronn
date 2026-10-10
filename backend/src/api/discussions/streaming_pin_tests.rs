//! KT-1096 — a BatchQuickPrompt child discussion of a pinned workflow run
//! starts on the skills, directives and profiles that run pinned, carried in
//! its prompt rather than in shared native files a live sync can rewrite.

use super::AgentExecutionOutcome;
use crate::models::{AgentType, DirectiveCategory, SkillCategory};
use std::sync::{Arc, Mutex};

/// Answers every prompt at once and records it. Before reading a prompt it
/// runs `before_read`: the agent is suspended while a live sync happens.
struct Recording(Arc<Mutex<Vec<String>>>, Box<dyn Fn() + Send + Sync>);

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
        (self.1)();
        self.0.lock().unwrap().push(prompt.to_string());
        let _ = events
            .send(crate::acp::AcpSessionEvent::TextDelta("done".into()))
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

fn state_on(db: Arc<crate::db::Database>) -> crate::AppState {
    let mut config = crate::core::config::default_config();
    config.agents.claude_code.full_access = true;
    crate::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

async fn child(db: &crate::db::Database, id: &str, skill: &str, directive: &str) {
    let (id, skill, directive) = (id.to_string(), skill.to_string(), directive.to_string());
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO discussions (id, title, agent, language, project_id, created_at, updated_at,
                                      awaiting_agent, workflow_run_id, skill_ids_json, directive_ids_json)
             VALUES (?1, ?1, 'ClaudeCode', 'fr', 'proj-pin', datetime('now'), datetime('now'), 1,
                     'batch-pin', ?2, ?3)",
            rusqlite::params![
                id,
                serde_json::to_string(&[skill])?,
                serde_json::to_string(&[directive])?
            ],
        )?;
        let user = crate::models::DiscussionMessage {
            id: format!("{id}-u"),
            role: crate::models::MessageRole::User,
            channel: crate::models::MessageChannel::Main,
            content: "item".into(),
            agent_type: None,
            timestamp: chrono::Utc::now(),
            tokens_used: 0,
            session_tokens_at_message: None,
            recovered_partial: false,
            auth_mode: None,
            model_tier: None,
            model: None,
            cost_usd: None,
            author_pseudo: None,
            author_avatar_email: None,
            author_cli_ordinal: None,
            source_msg_id: None,
            duration_ms: None,
            lint_report: None,
            target_agent: None,
            reply_to_message_id: None,
        };
        crate::db::discussions::insert_message(conn, &id, &user)?;
        Ok(())
    })
    .await
    .unwrap();
}

async fn start(state: crate::AppState, id: &str) {
    use axum::response::IntoResponse;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let response =
        super::make_agent_stream_inner(state, id.into(), None, None, None, None, Some(tx))
            .await
            .into_response();
    let _ = axum::body::to_bytes(response.into_body(), 1024 * 1024).await;
    let outcome = rx.await.unwrap();
    assert!(
        !matches!(outcome, AgentExecutionOutcome::PreflightFailed { .. }),
        "{id} did not start: {outcome:?}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn batch_children_start_on_the_pinned_skills_and_directives_even_after_a_restart() {
    let _data_dir = crate::core::config::TestDataDir::new();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().to_str().unwrap().to_owned();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let work_dir =
        crate::agents::runner::resolve_agent_work_dir(Some(&project_path), &project_path).unwrap();
    let skill_cell: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let live_sync = {
        let (cell, project_path) = (skill_cell.clone(), project_path.clone());
        move || {
            // A normal sync of the same project, from the live catalog.
            if let Some(skill) = cell.lock().unwrap().clone() {
                let _ = crate::core::native_files::sync_project_native_files(
                    &project_path,
                    &[skill],
                    &[],
                );
            }
        }
    };
    let _route = crate::agents::runner::test_acp_routes::route(
        &work_dir,
        Arc::new(Recording(prompts.clone(), Box::new(live_sync))),
    );
    let _saved = crate::core::config::test_saved_access::set(&AgentType::ClaudeCode, true);

    let skill = crate::core::skills::save_custom_skill(
        "Pinned Child Skill",
        "desc",
        "S",
        &SkillCategory::Domain,
        "PINNED-SKILL-BODY",
        None,
        None,
        None,
    )
    .unwrap();
    let directive = crate::core::directives::save_custom_directive(
        "Pinned Child Rule",
        "desc",
        "D",
        &DirectiveCategory::Output,
        "PINNED-RULE-BODY",
        &[],
    )
    .unwrap();

    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    let project_row: crate::models::Project = serde_json::from_value(serde_json::json!({
        "id": "proj-pin", "name": "proj-pin", "path": project_path,
        "repo_url": null, "token_override": null, "ai_config": {"detected": false, "configs": []},
        "created_at": chrono::Utc::now().to_rfc3339(), "updated_at": chrono::Utc::now().to_rfc3339()
    }))
    .unwrap();
    let (pinned_skill, pinned_directive) = (skill.clone(), directive.clone());
    db.with_conn(move |conn| {
        crate::db::projects::insert_project(conn, &project_row)?;
        let workflow: crate::models::Workflow = serde_json::from_value(serde_json::json!({
            "id": "wf-pin-batch", "name": "batch", "project_id": "proj-pin",
            "trigger": {"type": "Manual"},
            "steps": [{"name": "fan", "step_type": {"type": "Agent"},
                       "skill_ids": [pinned_skill], "directive_ids": [pinned_directive]}],
            "actions": [],
            "safety": {"sandbox": false, "max_files": null, "max_lines": null, "require_approval": false},
            "workspace_config": null, "concurrency_limit": null, "enabled": true,
            "created_at": chrono::Utc::now(), "updated_at": chrono::Utc::now(),
        }))?;
        crate::db::workflows::insert_workflow(conn, &workflow)?;
        let mut run: crate::models::WorkflowRun = serde_json::from_value(serde_json::json!({
            "id": "wf-run-pin", "workflow_id": "wf-pin-batch", "status": "Running",
            "step_results": [], "tokens_used": 0, "started_at": chrono::Utc::now()
        }))?;
        crate::db::workflows::insert_run(conn, &run)?;
        crate::workflows::run_pins::pin_or_load(conn, &workflow, &run)?
            .map_err(anyhow::Error::msg)?;
        run.id = "batch-pin".into();
        run.run_type = "batch".into();
        run.parent_run_id = Some("wf-run-pin".into());
        crate::db::workflows::insert_run(conn, &run)?;
        crate::workflows::run_pins::inherit_for_batch(conn, "wf-run-pin", "batch-pin")
    })
    .await
    .unwrap();
    for id in ["child-1", "child-2", "child-3"] {
        child(&db, id, &skill, &directive).await;
    }
    *skill_cell.lock().unwrap() = Some(skill.clone());

    start(state_on(db.clone()), "child-1").await;

    // Edited between two children of the same batch.
    crate::core::skills::update_custom_skill(
        &skill,
        "Pinned Child Skill",
        "desc",
        "S",
        &SkillCategory::Domain,
        "LIVE-SKILL-BODY",
        None,
        None,
        None,
    )
    .unwrap();
    crate::core::directives::update_custom_directive(
        &directive,
        "Pinned Child Rule",
        "desc",
        "D",
        &DirectiveCategory::Output,
        "LIVE-RULE-BODY",
        &[],
    )
    .unwrap();
    start(state_on(db.clone()), "child-2").await;

    // A restart: a fresh state, nothing in memory but the database.
    start(state_on(db.clone()), "child-3").await;

    // The live sync really rewrote the shared file to the new revision...
    let shared = project
        .path()
        .join(".claude/skills")
        .join(crate::core::native_files::slug(&skill))
        .join("SKILL.md");
    assert!(std::fs::read_to_string(&shared)
        .unwrap()
        .contains("LIVE-SKILL-BODY"));
    // ...yet what each child read is its pinned revision, carried inline.
    let prompts = prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 3, "{prompts:?}");
    for prompt in &prompts {
        assert!(prompt.contains("PINNED-SKILL-BODY"), "{prompt}");
        assert!(prompt.contains("PINNED-RULE-BODY"), "{prompt}");
        assert!(!prompt.contains("LIVE-RULE-BODY") && !prompt.contains("LIVE-SKILL-BODY"));
    }
    let _ = crate::core::skills::delete_custom_skill(&skill);
    let _ = crate::core::directives::delete_custom_directive(&directive);
}
