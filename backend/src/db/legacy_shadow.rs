//! Migration 296: the workflow-level shadow becomes per-step shadows. Kept in `db`
//! so the migration runner does not depend on the workflow engine.

/// Whether a stored `shadow_json` is a workflow-level shadow (KT-1449 or its
/// lanes) rather than the map from step id to shadows.
fn is_workflow_level(stored: &serde_json::Value) -> bool {
    stored.as_object().is_some_and(|object| {
        object
            .get("enabled")
            .is_some_and(serde_json::Value::is_boolean)
    })
}

/// The shadow one lane of a workflow-level shadow becomes on one step: the
/// lane's overrides, with the agent and the prompt the lane named for that step.
fn lane_to_step_shadow(
    lane: &serde_json::Value,
    enabled: bool,
    step: &serde_json::Value,
) -> Option<crate::models::StepShadow> {
    let lane = lane.as_object()?;
    let names = [step.get("id"), step.get("name")]
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    let mut out = serde_json::Map::new();
    out.insert("id".into(), lane.get("id")?.clone());
    for key in [
        "label",
        "project_ids",
        "sample_every",
        "skills",
        "quick_prompt_versions",
        "budget",
    ] {
        if let Some(value) = lane.get(key).filter(|v| !v.is_null()) {
            out.insert(key.into(), value.clone());
        }
    }
    out.insert("enabled".into(), enabled.into());
    let named_agent = lane
        .get("step_agents")
        .and_then(|agents| agents.as_object())
        .and_then(|agents| names.iter().find_map(|name| agents.get(*name)));
    if let Some(agent) = named_agent
        .or_else(|| lane.get("agent"))
        .filter(|v| !v.is_null())
    {
        out.insert("agent".into(), agent.clone());
    }
    let targeted = lane
        .get("prompt_steps")
        .and_then(|steps| steps.as_array())
        .is_none_or(|steps| {
            steps.is_empty()
                || steps
                    .iter()
                    .filter_map(|s| s.as_str())
                    .any(|s| names.contains(&s))
        });
    if let Some(prompt) = lane.get("prompt").filter(|v| !v.is_null() && targeted) {
        out.insert("prompt".into(), prompt.clone());
    }
    serde_json::from_value(serde_json::Value::Object(out)).ok()
}

/// The shadows a workflow-level shadow becomes: each of its lanes (the flat
/// KT-1449 shape is one lane named `shadow`) on every Agent step that does not
/// act outside Kronn, with the same overrides, keyed by step id.
pub fn legacy_shadow_to_steps(
    legacy: &serde_json::Value,
    steps: &[serde_json::Value],
) -> std::collections::BTreeMap<String, Vec<crate::models::StepShadow>> {
    let enabled = legacy
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let lanes: Vec<serde_json::Value> = match legacy.get("lanes").and_then(|l| l.as_array()) {
        Some(lanes) => lanes.clone(),
        None => {
            let mut flat = serde_json::Map::new();
            flat.insert("id".into(), "shadow".into());
            for key in ["project_ids", "skills", "quick_prompt_versions"] {
                if let Some(value) = legacy.get(key) {
                    flat.insert(key.into(), value.clone());
                }
            }
            vec![serde_json::Value::Object(flat)]
        }
    };
    let mut map = std::collections::BTreeMap::new();
    for step in steps {
        let is_agent = step
            .pointer("/step_type/type")
            .and_then(|t| t.as_str())
            .is_none_or(|t| t == "Agent");
        let external = step
            .get("external_effect")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let Some(id) = step.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if !is_agent || external {
            continue;
        }
        let shadows: Vec<crate::models::StepShadow> = lanes
            .iter()
            .filter_map(|lane| lane_to_step_shadow(lane, enabled, step))
            .collect();
        if !shadows.is_empty() {
            map.insert(id.to_string(), shadows);
        }
    }
    map
}

/// Moves each workflow-level shadow onto the Agent steps of its workflow
/// (migration 296). Only `shadow_json` is written: the steps column, which the
/// trust checks compare, is left as it is.
pub fn migrate_workflow_shadows(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT id, steps_json, on_failure, shadow_json FROM workflows WHERE shadow_json IS NOT NULL",
    )?;
    let rows: Vec<(String, String, Option<String>, String)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<_, _>>()?;
    drop(stmt);
    for (id, steps, on_failure, stored) in rows {
        let Ok(stored) = serde_json::from_str::<serde_json::Value>(&stored) else {
            continue;
        };
        if !is_workflow_level(&stored) {
            continue;
        }
        let mut all: Vec<serde_json::Value> = serde_json::from_str(&steps).unwrap_or_default();
        all.extend(
            on_failure
                .and_then(|raw| serde_json::from_str::<Vec<serde_json::Value>>(&raw).ok())
                .unwrap_or_default(),
        );
        let map = legacy_shadow_to_steps(&stored, &all);
        let column = if map.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&map)?)
        };
        conn.execute(
            "UPDATE workflows SET shadow_json = ?2 WHERE id = ?1",
            rusqlite::params![id, column],
        )?;
    }
    Ok(())
}
