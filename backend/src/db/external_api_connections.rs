//! Persistence and one-time legacy-config backfill for named external API connections.

use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::parse_dt;
use crate::models::{
    AgentType, AppConfig, ExternalApiConnection, ExternalApiConnectionPreset, MessageTarget,
    ModelTierConfig,
};

const COLUMNS: &str = "id, display_name, mention_alias, endpoint, credential_slug, origin_preset, \
    economy_model, default_model, reasoning_model, created_at, updated_at";

const LEGACY_LITELLM_ID: &str = "external-api-litellm";
const LEGACY_NVIDIA_ID: &str = "external-api-nvidia";

/// Apply the canonical named connection to the legacy runtime slots still
/// consumed by agent selectors and the runner. Named connections are the UI
/// source of truth; these fields remain as a compatibility projection until
/// every execution path is connection-aware.
pub fn sync_runtime_config(connection: &ExternalApiConnection, config: &mut AppConfig) -> bool {
    let (agent, tiers) = match (connection.id.as_str(), connection.origin_preset) {
        (LEGACY_LITELLM_ID, ExternalApiConnectionPreset::LiteLlm) => (
            &mut config.agents.lite_llm,
            &mut config.agents.model_tiers.lite_llm,
        ),
        (LEGACY_NVIDIA_ID, ExternalApiConnectionPreset::Nvidia) => (
            &mut config.agents.nvidia,
            &mut config.agents.model_tiers.nvidia,
        ),
        _ => return false,
    };
    let changed = agent.base_url != connection.endpoint
        || tiers.economy != connection.economy_model
        || tiers.default != connection.default_model
        || tiers.reasoning != connection.reasoning_model;
    agent.base_url.clone_from(&connection.endpoint);
    tiers.economy.clone_from(&connection.economy_model);
    tiers.default.clone_from(&connection.default_model);
    tiers.reasoning.clone_from(&connection.reasoning_model);
    changed
}

/// Canonical provider rows which project into the global agent configuration.
pub fn runtime_connections(conn: &Connection) -> Result<Vec<ExternalApiConnection>> {
    let mut connections = Vec::with_capacity(2);
    for id in [LEGACY_LITELLM_ID, LEGACY_NVIDIA_ID] {
        if let Some(connection) = get(conn, id)? {
            connections.push(connection);
        }
    }
    Ok(connections)
}

fn preset_name(preset: ExternalApiConnectionPreset) -> &'static str {
    match preset {
        ExternalApiConnectionPreset::LiteLlm => "litellm",
        ExternalApiConnectionPreset::Nvidia => "nvidia",
        ExternalApiConnectionPreset::OpenRouter => "open_router",
        ExternalApiConnectionPreset::Other => "other",
    }
}

fn parse_preset(value: &str) -> ExternalApiConnectionPreset {
    match value {
        "litellm" => ExternalApiConnectionPreset::LiteLlm,
        "nvidia" => ExternalApiConnectionPreset::Nvidia,
        "open_router" => ExternalApiConnectionPreset::OpenRouter,
        _ => ExternalApiConnectionPreset::Other,
    }
}

fn row_to_connection(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExternalApiConnection> {
    Ok(ExternalApiConnection {
        id: row.get(0)?,
        display_name: row.get(1)?,
        mention_alias: row.get(2)?,
        endpoint: row.get(3)?,
        credential_slug: row.get(4)?,
        origin_preset: parse_preset(&row.get::<_, String>(5)?),
        economy_model: row.get(6)?,
        default_model: row.get(7)?,
        reasoning_model: row.get(8)?,
        created_at: parse_dt(row.get(9)?),
        updated_at: parse_dt(row.get(10)?),
    })
}

pub fn list(conn: &Connection) -> Result<Vec<ExternalApiConnection>> {
    let sql = format!("SELECT {COLUMNS} FROM external_api_connections ORDER BY display_name");
    Ok(conn
        .prepare(&sql)?
        .query_map([], row_to_connection)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<ExternalApiConnection>> {
    let sql = format!("SELECT {COLUMNS} FROM external_api_connections WHERE id = ?1");
    Ok(conn
        .query_row(&sql, params![id], row_to_connection)
        .optional()?)
}

/// Remove a connection row. The caller is responsible for clearing the linked
/// credential from the token store; the row only references it by slug.
pub fn delete(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute(
        "DELETE FROM external_api_connections WHERE id = ?1",
        params![id],
    )?;
    Ok(affected > 0)
}

pub fn connection_mention_alias(connection: &ExternalApiConnection) -> String {
    format!("@{}", connection.mention_alias.trim().to_lowercase())
}

fn ensure_mention_alias_available(
    conn: &Connection,
    mention_alias: &str,
    excluding_id: Option<&str>,
) -> Result<()> {
    let conflicting_id = conn
        .query_row(
            "SELECT id FROM external_api_connections
             WHERE mention_alias = ?1 COLLATE NOCASE
               AND (?2 IS NULL OR id <> ?2)",
            params![mention_alias.trim(), excluding_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(id) = conflicting_id {
        bail!(
            "mention alias @{} is already claimed by connection {id}",
            mention_alias.trim().to_lowercase()
        );
    }
    Ok(())
}

pub fn target_for_connection(connection: &ExternalApiConnection) -> MessageTarget {
    let agent_type = match connection.origin_preset {
        ExternalApiConnectionPreset::LiteLlm => AgentType::LiteLlm,
        ExternalApiConnectionPreset::Nvidia => AgentType::Nvidia,
        ExternalApiConnectionPreset::OpenRouter | ExternalApiConnectionPreset::Other => {
            AgentType::Custom
        }
    };
    let mut target = MessageTarget::agent(agent_type);
    target.connection_id = Some(connection.id.clone());
    target
}

pub fn resolve_connection_mentions(
    content: &str,
    connections: &[ExternalApiConnection],
) -> Vec<MessageTarget> {
    let lower = content.to_lowercase();
    let mut found = Vec::new();
    for connection in connections {
        let alias = connection_mention_alias(connection);
        for (index, _) in lower.match_indices(&alias) {
            let before = lower[..index].chars().next_back();
            let after = lower[index + alias.len()..].chars().next();
            let valid_before = before.is_none_or(|ch| !ch.is_alphanumeric());
            let valid_after = after.is_none_or(|ch| !(ch.is_alphanumeric() || ch == '-'));
            if valid_before && valid_after {
                found.push((index, target_for_connection(connection)));
                break;
            }
        }
    }
    found.sort_by_key(|(index, _)| *index);
    found.into_iter().map(|(_, target)| target).collect()
}

/// Add dynamically resolved connection targets without retaining the generic
/// static target for the same provider. The scoped connection wins because it
/// carries the runtime identity; a tier selected on the static target remains
/// attached to the replacement.
pub fn merge_connection_mention_targets(
    targets: &mut Vec<MessageTarget>,
    connection_targets: Vec<MessageTarget>,
) {
    for mut connection_target in connection_targets {
        let generic_position = targets.iter().position(|target| {
            target.kind == crate::models::MessageTargetKind::Agent
                && target.agent_type == connection_target.agent_type
                && target.connection_id.is_none()
                && target.cli_session_id.is_none()
        });
        let existing_position = targets.iter().position(|target| {
            target.kind == connection_target.kind
                && target.agent_type == connection_target.agent_type
                && target.connection_id == connection_target.connection_id
                && target.cli_session_id == connection_target.cli_session_id
        });

        match (generic_position, existing_position) {
            (Some(generic), Some(existing)) => {
                if targets[existing].tier.is_none() {
                    targets[existing].tier = targets[generic].tier;
                }
                targets.remove(generic);
            }
            (Some(generic), None) => {
                connection_target.tier = targets[generic].tier;
                targets[generic] = connection_target;
            }
            (None, Some(_)) => {}
            (None, None) => targets.push(connection_target),
        }
    }
}

pub fn insert(conn: &Connection, connection: &ExternalApiConnection) -> Result<()> {
    ensure_mention_alias_available(conn, &connection.mention_alias, None)?;
    conn.execute(
        "INSERT INTO external_api_connections (
             id, display_name, mention_alias, endpoint, credential_slug, origin_preset,
             economy_model, default_model, reasoning_model, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            connection.id,
            connection.display_name,
            connection.mention_alias.trim().to_lowercase(),
            connection.endpoint,
            connection.credential_slug,
            preset_name(connection.origin_preset),
            connection.economy_model,
            connection.default_model,
            connection.reasoning_model,
            connection.created_at.to_rfc3339(),
            connection.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn update(conn: &Connection, connection: &ExternalApiConnection) -> Result<()> {
    ensure_mention_alias_available(conn, &connection.mention_alias, Some(&connection.id))?;
    conn.execute(
        "UPDATE external_api_connections SET
             display_name = ?2, mention_alias = ?3, endpoint = ?4, credential_slug = ?5,
             origin_preset = ?6, economy_model = ?7, default_model = ?8,
             reasoning_model = ?9, updated_at = ?10
         WHERE id = ?1",
        params![
            connection.id,
            connection.display_name,
            connection.mention_alias.trim().to_lowercase(),
            connection.endpoint,
            connection.credential_slug,
            preset_name(connection.origin_preset),
            connection.economy_model,
            connection.default_model,
            connection.reasoning_model,
            connection.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Backfill the two legacy provider settings exactly once. `DO NOTHING` keeps
/// a connection created or later edited by the named-connection workflow intact.
pub fn backfill_legacy_config(conn: &Connection, config: &AppConfig) -> Result<()> {
    insert_legacy(
        conn,
        LEGACY_LITELLM_ID,
        "LiteLLM",
        "litellm",
        config.agents.lite_llm.base_url.as_deref(),
        ExternalApiConnectionPreset::LiteLlm,
        &config.agents.model_tiers.lite_llm,
    )?;
    let nvidia_endpoint =
        crate::api::nvidia::resolve_base_url_pub(config.agents.nvidia.base_url.as_deref());
    insert_legacy(
        conn,
        LEGACY_NVIDIA_ID,
        "NVIDIA",
        "nvidia",
        Some(&nvidia_endpoint),
        ExternalApiConnectionPreset::Nvidia,
        &config.agents.model_tiers.nvidia,
    )?;
    // Early versions of the named-connection migration persisted NVIDIA's
    // optional config value verbatim. With no explicit override that produced
    // a NULL endpoint, even though the provider has a real hosted default.
    // Repair only the canonical legacy row and never overwrite a user edit.
    conn.execute(
        "UPDATE external_api_connections
         SET endpoint = ?2
         WHERE id = ?1 AND (endpoint IS NULL OR trim(endpoint) = '')",
        params![LEGACY_NVIDIA_ID, nvidia_endpoint],
    )?;
    Ok(())
}

fn insert_legacy(
    conn: &Connection,
    id: &str,
    display_name: &str,
    credential_slug: &str,
    endpoint: Option<&str>,
    origin_preset: ExternalApiConnectionPreset,
    tiers: &ModelTierConfig,
) -> Result<()> {
    conn.execute(
        "INSERT INTO external_api_connections (
             id, display_name, mention_alias, endpoint, credential_slug, origin_preset,
             economy_model, default_model, reasoning_model
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(credential_slug) DO NOTHING",
        params![
            id,
            display_name,
            credential_slug,
            endpoint.map(str::trim).filter(|value| !value.is_empty()),
            credential_slug,
            preset_name(origin_preset),
            tiers.economy,
            tiers.default,
            tiers.reasoning,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::default_config;
    use crate::models::ApiKey;
    use chrono::Utc;

    fn connection(
        id: &str,
        alias: &str,
        preset: ExternalApiConnectionPreset,
    ) -> ExternalApiConnection {
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
        }
    }

    #[test]
    fn mention_alias_is_unique_for_insert_and_update_case_insensitively() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        insert(
            &conn,
            &connection("groq-primary", "Groq", ExternalApiConnectionPreset::Other),
        )
        .unwrap();

        let insert_error = insert(
            &conn,
            &connection("groq-secondary", "groq", ExternalApiConnectionPreset::Other),
        )
        .unwrap_err()
        .to_string();
        assert!(insert_error.contains("already claimed"));

        let mut other = connection("other", "other", ExternalApiConnectionPreset::Other);
        insert(&conn, &other).unwrap();
        other.mention_alias = "GROQ".into();
        let update_error = update(&conn, &other).unwrap_err().to_string();
        assert!(update_error.contains("already claimed"));
    }

    #[test]
    fn dynamic_alias_resolves_by_exact_match_to_typed_connection_target() {
        let connections = vec![
            connection("groq-primary", "groq", ExternalApiConnectionPreset::Other),
            connection(
                "nvidia-primary",
                "nvidia",
                ExternalApiConnectionPreset::Nvidia,
            ),
        ];

        let targets = resolve_connection_mentions("Ask @GROQ, then @nvidia.", &connections);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].agent_type, AgentType::Custom);
        assert_eq!(targets[0].connection_id.as_deref(), Some("groq-primary"));
        assert_eq!(targets[1].agent_type, AgentType::Nvidia);
        assert_eq!(targets[1].connection_id.as_deref(), Some("nvidia-primary"));
        assert!(
            resolve_connection_mentions("mail@groq.example or @groq-fast", &connections).is_empty()
        );
    }

    #[test]
    fn openrouter_roundtrips_and_routes_through_its_named_custom_connection() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let openrouter = connection(
            "openrouter-primary",
            "openrouter",
            ExternalApiConnectionPreset::OpenRouter,
        );
        insert(&conn, &openrouter).unwrap();

        let persisted = get(&conn, "openrouter-primary").unwrap().unwrap();
        assert_eq!(
            persisted.origin_preset,
            ExternalApiConnectionPreset::OpenRouter
        );
        let target = target_for_connection(&persisted);
        assert_eq!(target.agent_type, AgentType::Custom);
        assert_eq!(target.connection_id.as_deref(), Some("openrouter-primary"));
        assert_eq!(
            resolve_connection_mentions("Ask @openrouter.", &[persisted]),
            vec![target]
        );
    }

    #[test]
    fn dynamic_connection_target_replaces_static_provider_target_without_losing_tier() {
        let connections = vec![connection(
            "nvidia-primary",
            "nvidia",
            ExternalApiConnectionPreset::Nvidia,
        )];
        let mut targets =
            vec![MessageTarget::agent(AgentType::Nvidia)
                .with_tier(crate::models::ModelTier::Reasoning)];

        merge_connection_mention_targets(
            &mut targets,
            resolve_connection_mentions("Ask @nvidia.", &connections),
        );

        assert_eq!(
            targets,
            vec![MessageTarget::agent(AgentType::Nvidia)
                .with_connection("nvidia-primary")
                .with_tier(crate::models::ModelTier::Reasoning)]
        );
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
        };
        config.agents.model_tiers.nvidia = ModelTierConfig {
            economy: Some("nim-economy".into()),
            default: Some("nim-default".into()),
            reasoning: Some("nim-reasoning".into()),
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

    #[test]
    fn connections_keep_endpoints_and_credential_slugs_isolated() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let now = Utc::now();
        for (id, alias, endpoint, credential_slug) in [
            (
                "connection-one",
                "one",
                "https://one.example.test",
                "credential-one",
            ),
            (
                "connection-two",
                "two",
                "https://two.example.test",
                "credential-two",
            ),
        ] {
            insert(
                &conn,
                &ExternalApiConnection {
                    id: id.into(),
                    display_name: alias.into(),
                    mention_alias: alias.into(),
                    endpoint: Some(endpoint.into()),
                    credential_slug: credential_slug.into(),
                    origin_preset: ExternalApiConnectionPreset::Other,
                    economy_model: Some(format!("{alias}-economy")),
                    default_model: Some(format!("{alias}-default")),
                    reasoning_model: Some(format!("{alias}-reasoning")),
                    created_at: now,
                    updated_at: now,
                },
            )
            .unwrap();
        }
        let connections = list(&conn).unwrap();
        assert_eq!(connections.len(), 2);
        assert_ne!(connections[0].endpoint, connections[1].endpoint);
        assert_ne!(
            connections[0].credential_slug,
            connections[1].credential_slug
        );
        assert_ne!(connections[0].economy_model, connections[1].economy_model);
        assert_ne!(connections[0].default_model, connections[1].default_model);
        assert_ne!(
            connections[0].reasoning_model,
            connections[1].reasoning_model
        );
    }
}
