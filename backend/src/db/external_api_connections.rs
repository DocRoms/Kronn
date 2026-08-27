//! Persistence and one-time legacy-config backfill for named external API connections.

use anyhow::Result;
use rusqlite::{params, Connection};

use super::parse_dt;
use crate::models::{
    AppConfig, ExternalApiConnection, ExternalApiConnectionPreset, ModelTierConfig,
};

const COLUMNS: &str = "id, display_name, mention_alias, endpoint, credential_slug, origin_preset, \
    economy_model, default_model, reasoning_model, created_at, updated_at";

const LEGACY_LITELLM_ID: &str = "external-api-litellm";
const LEGACY_NVIDIA_ID: &str = "external-api-nvidia";

fn preset_name(preset: ExternalApiConnectionPreset) -> &'static str {
    match preset {
        ExternalApiConnectionPreset::LiteLlm => "litellm",
        ExternalApiConnectionPreset::Nvidia => "nvidia",
        ExternalApiConnectionPreset::Other => "other",
    }
}

fn parse_preset(value: &str) -> ExternalApiConnectionPreset {
    match value {
        "litellm" => ExternalApiConnectionPreset::LiteLlm,
        "nvidia" => ExternalApiConnectionPreset::Nvidia,
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

pub fn insert(conn: &Connection, connection: &ExternalApiConnection) -> Result<()> {
    conn.execute(
        "INSERT INTO external_api_connections (
             id, display_name, mention_alias, endpoint, credential_slug, origin_preset,
             economy_model, default_model, reasoning_model, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            connection.id,
            connection.display_name,
            connection.mention_alias,
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
    insert_legacy(
        conn,
        LEGACY_NVIDIA_ID,
        "NVIDIA",
        "nvidia",
        config.agents.nvidia.base_url.as_deref(),
        ExternalApiConnectionPreset::Nvidia,
        &config.agents.model_tiers.nvidia,
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

    #[test]
    fn legacy_litellm_and_nvidia_configurations_backfill_without_losing_keys_or_tiers() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run_through(&conn, "143_quick_items_pinned").unwrap();

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

        crate::db::migrations::run(&conn).unwrap();
        backfill_legacy_config(&conn, &config).unwrap();

        let connections = list(&conn).unwrap();
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
