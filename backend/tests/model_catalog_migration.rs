//! Production-library bootstrap replay in isolated SQLite files, without provider calls.
//! Serial tests protect this binary's process-local catalogue projection.
use kronn::core::{config::default_config, model_catalog};
use kronn::db::{model_catalog as store, Database};
use kronn::models::{
    AgentType, ModelProvenance, ModelTier, ModelUnavailableReason, UpsertManualModelRequest,
};
use serial_test::serial;

#[tokio::test]
#[serial]
async fn ollama_bootstrap_preserves_the_configured_default_instead_of_its_old_seed() {
    let database = Database::open_in_memory().unwrap();
    let mut config = default_config();
    config.agents.model_tiers.ollama.default = Some("operator/local:v2".into());
    model_catalog::migrate_hardcoded_catalog_once(&database, &config)
        .await
        .unwrap();
    let rows = database
        .with_read_conn(|conn| store::list_for_target(conn, "agent:ollama"))
        .await
        .unwrap();
    let configured = rows
        .iter()
        .find(|entry| entry.model_id == "operator/local:v2")
        .expect("the migrated catalogue includes the existing Ollama override");
    assert_eq!(configured.provenance, ModelProvenance::Migrated);
    assert_eq!(configured.tier_assignment, Some(ModelTier::Default));
    assert_eq!(
        model_catalog::assigned_model_for_agent(&AgentType::Ollama, ModelTier::Default).as_deref(),
        Some("operator/local:v2")
    );
    assert!(rows
        .iter()
        .filter(|entry| entry.model_id.starts_with("qwen3:"))
        .all(|entry| entry.tier_assignment.is_none()));
}

#[tokio::test]
#[serial]
async fn catalogue_bootstrap_is_durable_and_does_not_restore_removed_seeds() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalogue.db");
    let database = Database::open_path(&path).unwrap();
    let mut config = default_config();
    config.agents.model_tiers.open_code.reasoning = Some("operator/Éclair:v2".into());
    let original = serde_json::to_value(&config).unwrap();
    model_catalog::migrate_hardcoded_catalog_once(&database, &config)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&config).unwrap(), original);
    database
        .with_conn(|conn| {
            assert!(store::migration_already_ran(conn)?);
            let configured = store::get(conn, "agent:opencode", "operator/Éclair:v2")?.unwrap();
            assert_eq!(configured.provenance, ModelProvenance::Migrated);
            assert_eq!(configured.tier_assignment, Some(ModelTier::Reasoning));
            assert!(store::delete_manual(conn, "agent:ollama", "qwen3:8b")?);
            Ok(())
        })
        .await
        .unwrap();
    let before = database.with_read_conn(store::list_all).await.unwrap();
    drop(database);

    let reopened = Database::open_path(&path).unwrap();
    config.agents.model_tiers.open_code.reasoning = Some("post-bootstrap-choice".into());
    model_catalog::migrate_hardcoded_catalog_once(&reopened, &config)
        .await
        .unwrap();
    let after = reopened.with_read_conn(store::list_all).await.unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert_eq!(
        model_catalog::assigned_model_for_agent(&AgentType::Ollama, ModelTier::Default),
        None
    );
    assert_eq!(
        model_catalog::assigned_model_for_agent(&AgentType::OpenCode, ModelTier::Reasoning)
            .as_deref(),
        Some("operator/Éclair:v2")
    );
}

#[tokio::test]
#[serial]
async fn migrated_operator_tier_replaces_the_historical_assignment() {
    let database = Database::open_in_memory().unwrap();
    let mut config = default_config();
    config.agents.model_tiers.claude_code.default = Some("operator/tiny".into());
    model_catalog::migrate_hardcoded_catalog_once(&database, &config)
        .await
        .unwrap();
    let rows = database
        .with_read_conn(|conn| store::list_for_target(conn, "agent:claude-code"))
        .await
        .unwrap();
    let holders: Vec<_> = rows
        .iter()
        .filter(|entry| entry.tier_assignment == Some(ModelTier::Default))
        .map(|entry| entry.model_id.as_str())
        .collect();
    assert_eq!(
        holders,
        vec!["operator/tiny"],
        "one configured tier must not retain a competing historical assignment"
    );
    assert_eq!(
        model_catalog::assigned_model_for_agent(&AgentType::ClaudeCode, ModelTier::Default)
            .as_deref(),
        Some("operator/tiny")
    );
    assert!(
        rows.iter()
            .any(|entry| entry.model_id == "sonnet" && entry.tier_assignment.is_none()),
        "the old identity stays available without stealing the configured tier"
    );
}

#[tokio::test]
#[serial]
async fn historical_bootstrap_does_not_compete_with_an_existing_manual_tier() {
    let database = Database::open_in_memory().unwrap();
    let request: UpsertManualModelRequest = serde_json::from_value(serde_json::json!({
        "runtime_target_id": "agent:claude-code", "agent_type": "ClaudeCode",
        "model_id": "operator/known", "display_name": "Operator alias", "tier_assignment": "default",
        "capabilities": ["chat"], "reasoning_modes": ["high"], "privacy_note": "Keep this note"
    })).unwrap();
    let existing = database
        .with_conn(move |conn| Ok(store::create_manual(conn, &request)?.unwrap()))
        .await
        .unwrap();
    model_catalog::migrate_hardcoded_catalog_once(&database, &default_config())
        .await
        .unwrap();
    let rows = database
        .with_read_conn(|conn| store::list_for_target(conn, "agent:claude-code"))
        .await
        .unwrap();
    let holders: Vec<_> = rows
        .iter()
        .filter(|entry| entry.tier_assignment == Some(ModelTier::Default))
        .map(|entry| entry.model_id.as_str())
        .collect();
    assert_eq!(holders, vec!["operator/known"]);
    assert_eq!(
        serde_json::to_value(
            rows.iter()
                .find(|entry| entry.model_id == "operator/known")
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(existing).unwrap()
    );
}

#[tokio::test]
#[serial]
async fn an_existing_historical_identity_keeps_its_other_tier_and_explicit_configuration() {
    let database = Database::open_in_memory().unwrap();
    let mut config = default_config();
    config.agents.model_tiers.claude_code.default = Some("haiku".into());
    let original = serde_json::to_value(&config).unwrap();
    model_catalog::migrate_hardcoded_catalog_once(&database, &config)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&config).unwrap(), original);
    let rows = database
        .with_read_conn(|conn| store::list_for_target(conn, "agent:claude-code"))
        .await
        .unwrap();
    assert!(rows
        .iter()
        .all(|entry| entry.model_id != "sonnet" || entry.tier_assignment.is_none()));
    assert_eq!(
        model_catalog::assigned_model_for_agent(&AgentType::ClaudeCode, ModelTier::Economy)
            .as_deref(),
        Some("haiku")
    );
    database
        .with_conn(|conn| {
            store::mark_unavailable(
                conn,
                "agent:claude-code",
                "haiku",
                ModelUnavailableReason::Unsupported,
                Some("fixture only"),
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let refusal = model_catalog::preflight_check(
        &database,
        None,
        AgentType::ClaudeCode,
        ModelTier::Default,
        None,
        Some(&config.agents.model_tiers),
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(refusal).unwrap()["model_id"], "haiku");
}

#[tokio::test]
#[serial]
async fn one_configured_identity_can_still_resolve_multiple_explicit_tiers() {
    let database = Database::open_in_memory().unwrap();
    let mut config = default_config();
    config.agents.model_tiers.claude_code.economy = Some("operator/shared".into());
    config.agents.model_tiers.claude_code.default = Some("operator/shared".into());
    let original = serde_json::to_value(&config).unwrap();
    model_catalog::migrate_hardcoded_catalog_once(&database, &config)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&config).unwrap(), original);
    let rows = database
        .with_read_conn(|conn| store::list_for_target(conn, "agent:claude-code"))
        .await
        .unwrap();
    assert!(rows
        .iter()
        .filter(|entry| ["haiku", "sonnet"].contains(&entry.model_id.as_str()))
        .all(|entry| entry.tier_assignment.is_none()));
    assert_eq!(
        rows.iter()
            .filter(|entry| entry.model_id == "operator/shared")
            .count(),
        1
    );
    database
        .with_conn(|conn| {
            store::mark_unavailable(
                conn,
                "agent:claude-code",
                "operator/shared",
                ModelUnavailableReason::Unsupported,
                Some("fixture only"),
            )?;
            Ok(())
        })
        .await
        .unwrap();
    for tier in [ModelTier::Economy, ModelTier::Default] {
        let refusal = model_catalog::preflight_check(
            &database,
            None,
            AgentType::ClaudeCode,
            tier,
            None,
            Some(&config.agents.model_tiers),
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(refusal).unwrap()["model_id"],
            "operator/shared"
        );
    }
}
