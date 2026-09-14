//! Immutable launch-time QP overrides, independent of QP/version retention.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::agents::runner;
use crate::models::{AgentType, Discussion, ModelTier, ModelTiersConfig, QuickPrompt};

pub fn capture(
    conn: &Connection,
    discussion: &Discussion,
    qp: &QuickPrompt,
    tiers: &ModelTiersConfig,
) -> Result<()> {
    if qp.agent != discussion.agent
        || qp.connection_id != discussion.connection_id
        || discussion.connection_id.is_some()
        || !runner::agent_supports_reasoning_effort(&discussion.agent)
    {
        return Ok(());
    }
    let Some(effort) = qp
        .agent_settings
        .as_ref()
        .and_then(|settings| settings.reasoning_effort.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let model = runner::effective_model_flag(
        discussion.model.as_deref(), &discussion.agent, discussion.tier, Some(tiers),
    ).context("Cannot apply Quick Prompt effort without a resolved model; choose a model or clear the effort")?;
    let agent = serde_json::to_value(&discussion.agent)?;
    let tier = serde_json::to_value(discussion.tier)?;
    conn.execute(
        "INSERT INTO discussion_effort_snapshots (discussion_id, agent, tier, model, effort)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![discussion.id, agent.as_str(), tier.as_str(), model, effort],
    )?;
    Ok(())
}

/// A changed runtime/model/tier gets its own preset, never this QP's override.
pub fn for_run(
    conn: &Connection,
    discussion_id: &str,
    agent: &AgentType,
    tier: ModelTier,
    model: Option<&str>,
) -> Result<Option<String>> {
    let agent = serde_json::to_value(agent)?;
    let tier = serde_json::to_value(tier)?;
    conn.query_row(
        "SELECT effort FROM discussion_effort_snapshots
         WHERE discussion_id = ?1 AND agent = ?2 AND tier = ?3 AND model = ?4",
        params![discussion_id, agent.as_str(), tier.as_str(), model],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_survives_restart_and_is_bound_to_agent_model_tier_and_discussion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            crate::db::migrations::run(&conn).unwrap();
            conn.execute("INSERT INTO discussions (id,title,created_at,updated_at) VALUES ('d','test','now','now')", []).unwrap();
            conn.execute("INSERT INTO discussion_effort_snapshots VALUES ('d','ClaudeCode','default','fable','low')", []).unwrap();
        }
        let conn = Connection::open(&path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        assert_eq!(
            for_run(
                &conn,
                "d",
                &AgentType::ClaudeCode,
                ModelTier::Default,
                Some("fable")
            )
            .unwrap()
            .as_deref(),
            Some("low")
        );
        for (id, agent, tier, model) in [
            ("d", AgentType::Codex, ModelTier::Default, Some("fable")),
            (
                "d",
                AgentType::ClaudeCode,
                ModelTier::Economy,
                Some("fable"),
            ),
            (
                "d",
                AgentType::ClaudeCode,
                ModelTier::Default,
                Some("other"),
            ),
            ("d", AgentType::ClaudeCode, ModelTier::Default, None),
            (
                "legacy",
                AgentType::ClaudeCode,
                ModelTier::Default,
                Some("fable"),
            ),
        ] {
            assert_eq!(for_run(&conn, id, &agent, tier, model).unwrap(), None);
        }
        conn.execute_batch("PRAGMA foreign_keys=ON; DELETE FROM discussions WHERE id='d';")
            .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM discussion_effort_snapshots",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}
