//! Immutable Quick Prompt generation overrides, bound to the launch target.
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::agents::{generation_settings, runner};
use crate::models::{AgentType, Discussion, ModelTier, ModelTiersConfig, QuickPrompt};

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct HttpSettings {
    pub reasoning_effort: Option<String>,
    pub max_tokens: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct InvalidLaunchSettings(String);

pub(crate) fn capture(
    conn: &Connection,
    discussion: &Discussion,
    qp: &QuickPrompt,
    tiers: &ModelTiersConfig,
) -> Result<()> {
    if qp.agent != discussion.agent || qp.connection_id != discussion.connection_id {
        return Ok(());
    }
    let Some(settings) = qp.agent_settings.as_ref() else {
        return Ok(());
    };
    let effort = settings
        .reasoning_effort
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    generation_settings::validate(&discussion.agent, effort, settings.max_tokens)
        .map_err(InvalidLaunchSettings)?;
    if !runner::is_http_chat_agent(&discussion.agent) {
        return super::discussion_effort::capture(conn, discussion, qp, tiers);
    }
    if effort.is_none() && settings.max_tokens.is_none() {
        return Ok(());
    }
    let connection_model = if let Some(id) = discussion.connection_id.as_deref() {
        let connection = super::external_api_connections::get(conn, id)?.ok_or_else(|| {
            InvalidLaunchSettings("Quick Prompt connection is unavailable".into())
        })?;
        crate::http_transport::connection_tier_model(&connection, discussion.tier)
    } else {
        None
    };
    let model = discussion
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_owned)
        .or(connection_model)
        .or_else(|| {
            runner::effective_model_flag(None, &discussion.agent, discussion.tier, Some(tiers))
        })
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| {
            InvalidLaunchSettings(
                "Cannot capture Quick Prompt generation settings without a resolved model".into(),
            )
        })?;
    let agent = serde_json::to_value(&discussion.agent)?;
    let tier = serde_json::to_value(discussion.tier)?;
    conn.execute(
        "INSERT INTO discussion_http_settings
         (discussion_id,agent,tier,connection_id,model,reasoning_effort,max_tokens)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            discussion.id,
            agent.as_str().context("agent")?,
            tier.as_str().context("tier")?,
            discussion.connection_id.as_deref().unwrap_or_default(),
            model,
            effort,
            settings.max_tokens.map(|value| value as i64)
        ],
    )?;
    Ok(())
}

pub(crate) fn for_http_run(
    conn: &Connection,
    discussion_id: &str,
    agent: &AgentType,
    tier: ModelTier,
    connection_id: Option<&str>,
    model: Option<&str>,
) -> Result<Option<HttpSettings>> {
    let agent = serde_json::to_value(agent)?;
    let tier = serde_json::to_value(tier)?;
    conn.query_row(
        "SELECT reasoning_effort,max_tokens FROM discussion_http_settings
         WHERE discussion_id=?1 AND agent=?2 AND tier=?3 AND connection_id=?4 AND model=?5",
        params![
            discussion_id,
            agent.as_str().context("agent")?,
            tier.as_str().context("tier")?,
            connection_id.unwrap_or_default(),
            model
        ],
        |row| {
            Ok(HttpSettings {
                reasoning_effort: row.get(0)?,
                max_tokens: row.get::<_, Option<i64>>(1)?.map(|value| value as u64),
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_launch_settings_survive_restart_and_only_match_the_original_target() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("launch.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            crate::db::migrations::run(&conn).unwrap();
            conn.execute("INSERT INTO discussions (id,title,created_at,updated_at) VALUES ('d','test','now','now')", []).unwrap();
            conn.execute("INSERT INTO discussion_http_settings VALUES ('d','Custom','reasoning','connection-a','model-a','high',3210)", []).unwrap();
        }
        let conn = Connection::open(&path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let expected = HttpSettings {
            reasoning_effort: Some("high".into()),
            max_tokens: Some(3210),
        };
        assert_eq!(
            for_http_run(
                &conn,
                "d",
                &AgentType::Custom,
                ModelTier::Reasoning,
                Some("connection-a"),
                Some("model-a")
            )
            .unwrap(),
            Some(expected)
        );
        for (id, agent, tier, connection, model) in [
            (
                "other",
                AgentType::Custom,
                ModelTier::Reasoning,
                Some("connection-a"),
                Some("model-a"),
            ),
            (
                "d",
                AgentType::LiteLlm,
                ModelTier::Reasoning,
                Some("connection-a"),
                Some("model-a"),
            ),
            (
                "d",
                AgentType::Custom,
                ModelTier::Default,
                Some("connection-a"),
                Some("model-a"),
            ),
            (
                "d",
                AgentType::Custom,
                ModelTier::Reasoning,
                Some("connection-b"),
                Some("model-a"),
            ),
            (
                "d",
                AgentType::Custom,
                ModelTier::Reasoning,
                None,
                Some("model-a"),
            ),
            (
                "d",
                AgentType::Custom,
                ModelTier::Reasoning,
                Some("connection-a"),
                Some("model-b"),
            ),
            (
                "d",
                AgentType::Custom,
                ModelTier::Reasoning,
                Some("connection-a"),
                None,
            ),
        ] {
            assert_eq!(
                for_http_run(&conn, id, &agent, tier, connection, model).unwrap(),
                None
            );
        }
        conn.execute_batch("PRAGMA foreign_keys=ON; DELETE FROM discussions WHERE id='d';")
            .unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM discussion_http_settings", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
    }
}
