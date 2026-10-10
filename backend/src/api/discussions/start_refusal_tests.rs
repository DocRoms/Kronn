use super::*;
use crate::api::discussions::streaming::AgentExecutionOutcome;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn failure(reason: ModelUnavailableReason, action: &str) -> CatalogPreflightFailure {
    CatalogPreflightFailure {
        runtime_target_id: "http:external-api-litellm".into(),
        agent_type: AgentType::LiteLlm,
        model_id: Some("vertex_ai/claude-fable-5@default".into()),
        reason,
        detail: "Access denied (HTTP 401): the proxy lists this model but refuses it".into(),
        last_checked_at: "2026-10-06T08:15:00Z".parse().unwrap(),
        recommended_action: action.into(),
        replacement: None,
    }
}

fn payload(content: &str) -> serde_json::Value {
    serde_json::from_str(
        content
            .strip_prefix("[kronn:agent-error]\n")
            .expect(content),
    )
    .unwrap()
}

#[test]
fn a_stored_access_denial_names_the_model_the_reason_and_its_date() {
    let refusal = failure(ModelUnavailableReason::AccessDenied, "choose_another_model");
    let summary = preflight_refusal_summary(&AgentType::LiteLlm, "fr", &refusal);
    assert!(
        summary.contains("vertex_ai/claude-fable-5@default"),
        "{summary}"
    );
    assert!(summary.contains("accès refusé"), "{summary}");
    assert!(summary.contains("2026-10-06 08:15 UTC"), "{summary}");
    assert!(summary.contains("Config › Agents › LiteLLM"), "{summary}");
    // The old sentence merged two causes into one guess.
    assert!(
        !summary.contains("introuvable ou accès refusé"),
        "{summary}"
    );

    let card = payload(&preflight_refusal_card(
        &summary,
        ModelTier::Default,
        &refusal,
        Some("job-1"),
    ));
    assert_eq!(card["kind"], "model_error");
    assert_eq!(card["status"], 401);
    assert_eq!(card["tier"], "default");
    assert_eq!(card["retry_dispatch_id"], "job-1");
    assert_eq!(card["summary"], summary);
    assert!(card["detail"].as_str().unwrap().contains("(access_denied)"));
}

#[test]
fn every_catalogue_refusal_reads_as_its_own_cause() {
    use ModelUnavailableReason as R;
    let reasons = [
        (R::AccessDenied, "choose_another_model"),
        (R::NotFound, "choose_another_model"),
        (R::Disappeared, "choose_replacement"),
        (R::AuthRequired, "authenticate"),
        (R::Timeout, "recheck_catalog"),
        (R::CliMissing, "install_cli"),
        (R::InvalidCatalog, "configure_manual_model"),
        (R::ProviderError, "recheck_catalog"),
        (R::Unsupported, "configure_manual_model"),
    ];
    for language in ["fr", "en", "es", "zh"] {
        let summaries: std::collections::HashSet<String> = reasons
            .iter()
            .map(|(reason, action)| {
                preflight_refusal_summary(&AgentType::Ollama, language, &failure(*reason, action))
            })
            .collect();
        assert_eq!(summaries.len(), reasons.len(), "{language}: {summaries:?}");
        for summary in &summaries {
            assert!(summary.contains("2026-10-06 08:15 UTC"), "{summary}");
            assert!(!summary.contains("LiteLLM"), "{summary}");
        }
    }
    // Only a refused model sends the user to pick another one.
    let auth = preflight_refusal_summary(
        &AgentType::ClaudeCode,
        "en",
        &failure(R::AuthRequired, "authenticate"),
    );
    assert!(!auth.contains("Choose another model"), "{auth}");
    assert!(auth.contains("Sign the agent in again"), "{auth}");
}

#[test]
fn the_vpn_card_is_typed_and_never_carries_the_address() {
    let refusal = StartRefusal {
        reason: UnreachableReason::Dns,
        checked_at: Utc::now(),
    };
    let card = payload(&unreachable_card(
        "LiteLlm",
        "fr",
        ModelTier::Economy,
        &refusal,
        Some("job-2"),
    ));
    assert_eq!(card["kind"], "agent_error");
    assert_eq!(card["status"], serde_json::Value::Null);
    let summary = card["summary"].as_str().unwrap();
    assert!(summary.contains("VPN"), "{summary}");
    assert!(summary.contains("DNS"), "{summary}");
    assert!(card["detail"].as_str().unwrap().contains("dns"));
    assert!(!card.to_string().contains("http"), "{card}");
}

#[test]
fn the_probed_address_is_the_one_the_runner_calls() {
    let endpoints = HttpEndpoints {
        lite_llm: Some("http://litellm.internal".into()),
        nvidia: None,
    };
    assert_eq!(
        launch_endpoint(&AgentType::LiteLlm, None, &endpoints).as_deref(),
        Some("http://litellm.internal")
    );
    assert_eq!(
        launch_endpoint(&AgentType::Custom, Some(" http://named "), &endpoints).as_deref(),
        Some("http://named")
    );
    // NVIDIA's hosted service and connection-less customs are not probed.
    assert_eq!(launch_endpoint(&AgentType::Nvidia, None, &endpoints), None);
    assert_eq!(launch_endpoint(&AgentType::Custom, None, &endpoints), None);
}

/// Answers every prompt at once and records it.
struct Recording(Arc<Mutex<Vec<String>>>);

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
        crate::acp::AcpSessionTarget::new(crate::acp::AcpAgent::ClaudeCode, "kt1154")
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

struct Room {
    db: Arc<crate::db::Database>,
    _project: tempfile::TempDir,
    _route: crate::agents::runner::test_acp_routes::RouteGuard,
    prompts: Arc<Mutex<Vec<String>>>,
}

async fn room() -> Room {
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().to_str().unwrap().to_owned();
    let work_dir =
        crate::agents::runner::resolve_agent_work_dir(Some(&project_path), &project_path).unwrap();
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let route = crate::agents::runner::test_acp_routes::route(
        &work_dir,
        Arc::new(Recording(prompts.clone())),
    );
    let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
    let project_row: crate::models::Project = serde_json::from_value(serde_json::json!({
        "id": "proj-kt1154", "name": "proj-kt1154", "path": project_path,
        "repo_url": null, "token_override": null, "ai_config": {"detected": false, "configs": []},
        "created_at": Utc::now().to_rfc3339(), "updated_at": Utc::now().to_rfc3339()
    }))
    .unwrap();
    db.with_conn(move |conn| crate::db::projects::insert_project(conn, &project_row))
        .await
        .unwrap();
    Room {
        db,
        _project: project,
        _route: route,
        prompts,
    }
}

async fn discussion(
    db: &crate::db::Database,
    id: &str,
    agent: &str,
    tier: &str,
    model: Option<&str>,
) {
    let (id, agent, tier, model) = (
        id.to_string(),
        agent.to_string(),
        tier.to_string(),
        model.map(str::to_string),
    );
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO discussions (id, title, agent, language, project_id, created_at, updated_at,
                                      awaiting_agent, model_tier, model)
             VALUES (?1, ?1, ?2, 'fr', 'proj-kt1154', datetime('now'), datetime('now'), 1, ?3, ?4)",
            rusqlite::params![id, agent, tier, model],
        )?;
        let user = DiscussionMessage {
            id: format!("{id}-u"),
            role: MessageRole::User,
            channel: MessageChannel::Main,
            content: "go".into(),
            agent_type: None,
            timestamp: Utc::now(),
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

fn state_on(db: Arc<crate::db::Database>, lite_llm: &str) -> crate::AppState {
    let mut config = crate::core::config::default_config();
    config.agents.claude_code.full_access = true;
    config.agents.lite_llm.base_url = Some(lite_llm.to_string());
    crate::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(config)),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

async fn start(state: crate::AppState, id: &str) -> (AgentExecutionOutcome, Duration) {
    use axum::response::IntoResponse;
    let started = Instant::now();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let response = crate::api::discussions::streaming::make_agent_stream_inner(
        state,
        id.into(),
        None,
        None,
        None,
        None,
        Some(tx),
    )
    .await
    .into_response();
    let _ = axum::body::to_bytes(response.into_body(), 1024 * 1024).await;
    let outcome = rx.await.unwrap();
    (outcome, started.elapsed())
}

async fn system_cards(db: &crate::db::Database, id: &str) -> Vec<serde_json::Value> {
    let id = id.to_string();
    db.with_read_conn(move |conn| crate::db::discussions::list_messages(conn, &id))
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == MessageRole::System)
        .filter_map(|message| {
            message
                .content
                .strip_prefix("[kronn:agent-error]\n")
                .map(|json| serde_json::from_str(json).unwrap())
        })
        .collect()
}

/// Port 1 is closed on any developer machine or CI runner, and dialing it
/// needs no bind.
const CLOSED_ENDPOINT: &str = "http://127.0.0.1:1";

#[tokio::test]
#[serial_test::serial]
async fn an_unreachable_endpoint_fails_its_agent_fast_and_the_others_still_start() {
    let _data_dir = crate::core::config::TestDataDir::new();
    let _saved = crate::core::config::test_saved_access::set(&AgentType::ClaudeCode, true);
    let room = room().await;
    discussion(&room.db, "d-lite", "LiteLlm", "default", None).await;
    discussion(&room.db, "d-claude", "ClaudeCode", "default", None).await;
    let state = state_on(room.db.clone(), CLOSED_ENDPOINT);

    let ((lite, lite_elapsed), (claude, _)) = tokio::join!(
        start(state.clone(), "d-lite"),
        start(state.clone(), "d-claude")
    );

    let AgentExecutionOutcome::PreflightFailed { diagnostic } = lite else {
        panic!("an unreachable endpoint refuses the start: {lite:?}");
    };
    assert!(diagnostic.contains("refused"), "{diagnostic}");
    assert!(lite_elapsed < Duration::from_secs(1), "{lite_elapsed:?}");
    let cards = system_cards(&room.db, "d-lite").await;
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(cards[0]["kind"], "agent_error");
    let summary = cards[0]["summary"].as_str().unwrap();
    assert!(
        summary.contains("injoignable") && summary.contains("VPN"),
        "{summary}"
    );

    assert!(
        !matches!(claude, AgentExecutionOutcome::PreflightFailed { .. }),
        "the sibling agent did not start: {claude:?}"
    );
    assert_eq!(room.prompts.lock().unwrap().len(), 1);
    assert!(system_cards(&room.db, "d-claude").await.is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn a_catalogue_refusal_leaves_a_dated_system_card() {
    let _data_dir = crate::core::config::TestDataDir::new();
    let _saved = crate::core::config::test_saved_access::set(&AgentType::ClaudeCode, true);
    let room = room().await;
    room.db
        .with_conn(|conn| {
            let target = crate::db::model_catalog::agent_runtime_target_id(&AgentType::ClaudeCode);
            let listed = |id: &str| crate::db::model_catalog::DiscoveredModel {
                model_id: id.into(),
                display_name: id.into(),
                resolved_model: None,
                description: None,
                capabilities: vec!["chat".into()],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
            };
            crate::db::model_catalog::reconcile_live(
                conn,
                &target,
                &AgentType::ClaudeCode,
                &[listed("claude-fable-5[1m]"), listed("sonnet")],
            )?;
            crate::db::model_catalog::reconcile_live(
                conn,
                &target,
                &AgentType::ClaudeCode,
                &[listed("sonnet")],
            )
        })
        .await
        .unwrap();
    discussion(
        &room.db,
        "d-refused",
        "ClaudeCode",
        "reasoning",
        Some("claude-fable-5[1m]"),
    )
    .await;
    let state = state_on(room.db.clone(), CLOSED_ENDPOINT);

    let (outcome, _) = start(state, "d-refused").await;

    assert!(
        matches!(outcome, AgentExecutionOutcome::PreflightFailed { .. }),
        "{outcome:?}"
    );
    let cards = system_cards(&room.db, "d-refused").await;
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(cards[0]["kind"], "model_error");
    assert_eq!(cards[0]["tier"], "reasoning");
    let summary = cards[0]["summary"].as_str().unwrap();
    assert!(summary.contains("claude-fable-5[1m]"), "{summary}");
    assert!(summary.contains("constaté le 20"), "{summary}");
    assert!(room.prompts.lock().unwrap().is_empty());
}
