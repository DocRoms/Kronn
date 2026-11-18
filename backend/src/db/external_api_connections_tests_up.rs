
use super::*;

use crate::core::config::default_config;

use crate::models::ApiKey;

use chrono::Utc;

fn connection(id: &str, alias: &str, preset: ExternalApiConnectionPreset) -> ExternalApiConnection {
    let now = Utc::now();
    ExternalApiConnection {
        id: id.into(),
        display_name: id.into(),
        mention_alias: alias.into(),
        endpoint: Some("https://api.example.test".into()),
        credential_slug: format!("credential-{id}"),
        origin_preset: preset,
        economy_model: None,
        default_model: Some("model".into()),
        reasoning_model: None,
        created_at: now,
        updated_at: now,
        image_model: None,
        video_model: None,
        decision_model: None,
        tools_enabled: None,
        media_endpoint: None,
    }
}

#[tokio::test]
async fn shared_bootstrap_backfills_legacy_connections_idempotently_without_secrets() {
    let database = crate::db::Database::open_in_memory().unwrap();

    let mut config = default_config();
    config.agents.lite_llm.base_url = Some("https://proxy.example.test".into());
    config.agents.nvidia.base_url = Some("https://nim.example.test".into());
    config.agents.model_tiers.lite_llm = ModelTierConfig {
        economy: Some("lite-economy".into()),
        default: Some("lite-default".into()),
        reasoning: Some("lite-reasoning".into()),
        ..Default::default()
    };
    config.agents.model_tiers.nvidia = ModelTierConfig {
        economy: Some("nim-economy".into()),
        default: Some("nim-default".into()),
        reasoning: Some("nim-reasoning".into()),
        ..Default::default()
    };
    config.tokens.keys = vec![
        ApiKey {
            id: "key-lite".into(),
            name: "Lite".into(),
            provider: "litellm".into(),
            value: "lite-secret".into(),
            active: true,
        },
        ApiKey {
            id: "key-nvidia".into(),
            name: "NVIDIA".into(),
            provider: "nvidia".into(),
            value: "nvidia-secret".into(),
            active: true,
        },
    ];

    crate::bootstrap_external_api_connections(&database, &mut config)
        .await
        .unwrap();
    crate::bootstrap_external_api_connections(&database, &mut config)
        .await
        .unwrap();

    let connections = database.with_conn(list).await.unwrap();
    assert_eq!(connections.len(), 2);
    let lite = connections
        .iter()
        .find(|item| item.credential_slug == "litellm")
        .unwrap();
    assert_eq!(lite.endpoint.as_deref(), Some("https://proxy.example.test"));
    assert_eq!(lite.origin_preset, ExternalApiConnectionPreset::LiteLlm);
    assert_eq!(lite.economy_model.as_deref(), Some("lite-economy"));
    assert_eq!(lite.default_model.as_deref(), Some("lite-default"));
    assert_eq!(lite.reasoning_model.as_deref(), Some("lite-reasoning"));
    assert_eq!(
        config.tokens.active_key_for(&lite.credential_slug),
        Some("lite-secret")
    );
    let nvidia = connections
        .iter()
        .find(|item| item.credential_slug == "nvidia")
        .unwrap();
    assert_eq!(nvidia.endpoint.as_deref(), Some("https://nim.example.test"));
    assert_eq!(nvidia.origin_preset, ExternalApiConnectionPreset::Nvidia);
    assert_eq!(nvidia.economy_model.as_deref(), Some("nim-economy"));
    assert_eq!(nvidia.default_model.as_deref(), Some("nim-default"));
    assert_eq!(nvidia.reasoning_model.as_deref(), Some("nim-reasoning"));
    assert_eq!(
        config.tokens.active_key_for(&nvidia.credential_slug),
        Some("nvidia-secret")
    );
    let persisted_secret_count = database
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM external_api_connections
                     WHERE display_name IN (?1, ?2)
                        OR mention_alias IN (?1, ?2)
                        OR endpoint IN (?1, ?2)
                        OR credential_slug IN (?1, ?2)
                        OR economy_model IN (?1, ?2)
                        OR default_model IN (?1, ?2)
                        OR reasoning_model IN (?1, ?2)",
                params!["lite-secret", "nvidia-secret"],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(persisted_secret_count, 0);
}

#[tokio::test]
async fn bootstrap_repairs_only_a_missing_legacy_nvidia_endpoint() {
    let database = crate::db::Database::open_in_memory().unwrap();
    database
        .with_conn(|conn| {
            insert_legacy(
                conn,
                LEGACY_NVIDIA_ID,
                "NVIDIA",
                "nvidia",
                None,
                ExternalApiConnectionPreset::Nvidia,
                &ModelTierConfig::default(),
            )
        })
        .await
        .unwrap();

    let mut config = default_config();
    crate::bootstrap_external_api_connections(&database, &mut config)
        .await
        .unwrap();
    let repaired = database
        .with_conn(|conn| get(conn, LEGACY_NVIDIA_ID))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        repaired.endpoint.as_deref(),
        Some("https://integrate.api.nvidia.com")
    );

    database
        .with_conn(|conn| {
            conn.execute(
                "UPDATE external_api_connections SET endpoint = ?2 WHERE id = ?1",
                params![LEGACY_NVIDIA_ID, "https://nim.example.test"],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    crate::bootstrap_external_api_connections(&database, &mut config)
        .await
        .unwrap();
    let preserved = database
        .with_conn(|conn| get(conn, LEGACY_NVIDIA_ID))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        preserved.endpoint.as_deref(),
        Some("https://nim.example.test")
    );
}

#[tokio::test]
async fn bootstrap_projects_saved_nvidia_connection_into_selector_and_runtime_config() {
    let database = crate::db::Database::open_in_memory().unwrap();
    let mut config = default_config();
    crate::bootstrap_external_api_connections(&database, &mut config)
        .await
        .unwrap();

    database
        .with_conn(|conn| {
            let mut nvidia = get(conn, LEGACY_NVIDIA_ID)?.unwrap();
            nvidia.endpoint = Some("https://nvidia.example.test".into());
            nvidia.economy_model = Some("nvidia/new-low".into());
            nvidia.default_model = Some("nvidia/new-standard".into());
            nvidia.reasoning_model = Some("nvidia/new-reasoning".into());
            update(conn, &nvidia)
        })
        .await
        .unwrap();

    assert!(
        crate::bootstrap_external_api_connections(&database, &mut config)
            .await
            .unwrap()
    );
    assert_eq!(
        config.agents.nvidia.base_url.as_deref(),
        Some("https://nvidia.example.test")
    );
    assert_eq!(
        config.agents.model_tiers.nvidia.economy.as_deref(),
        Some("nvidia/new-low")
    );
    assert_eq!(
        config.agents.model_tiers.nvidia.default.as_deref(),
        Some("nvidia/new-standard")
    );
    assert_eq!(
        config.agents.model_tiers.nvidia.reasoning.as_deref(),
        Some("nvidia/new-reasoning")
    );
    assert!(
        !crate::bootstrap_external_api_connections(&database, &mut config)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn migrated_legacy_connections_coexist_with_unique_live_aliases_end_to_end() {
    let database = crate::db::Database::open_in_memory().unwrap();
    let mut config = default_config();
    config.agents.lite_llm.base_url = Some("https://proxy.example.test".into());
    config.agents.nvidia.base_url = Some("https://nim.example.test".into());
    config.agents.model_tiers.lite_llm.default = Some("lite-default".into());
    config.agents.model_tiers.nvidia.default = Some("nim-default".into());

    // Replay the startup migration exactly as two consecutive boots would.
    crate::bootstrap_external_api_connections(&database, &mut config)
        .await
        .unwrap();
    crate::bootstrap_external_api_connections(&database, &mut config)
        .await
        .unwrap();

    database
        .with_conn(|conn| {
            let connections = list(conn)?;
            assert_eq!(connections.len(), 2);

            let targets = resolve_connection_mentions(
                "Compare @litellm with @NVIDIA in the same turn.",
                &connections,
            );
            assert_eq!(
                targets,
                vec![
                    MessageTarget::agent(AgentType::LiteLlm).with_connection(LEGACY_LITELLM_ID),
                    MessageTarget::agent(AgentType::Nvidia).with_connection(LEGACY_NVIDIA_ID),
                ]
            );

            let duplicate = connection(
                "duplicate-litellm",
                "LITELLM",
                ExternalApiConnectionPreset::Other,
            );
            let error = insert(conn, &duplicate).unwrap_err().to_string();
            assert!(error.contains("already claimed"));
            assert_eq!(list(conn)?.len(), 2);
            Ok(())
        })
        .await
        .unwrap();
}
