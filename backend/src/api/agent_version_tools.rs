//! Saved versions for HTTP agents (KT-1434), the same capability as the
//! bridge's `skill_get` / `qp_get` / `skill_create` / `skill_update`. The gate
//! has already checked the object as the route it mirrors; restoring stays human.
use super::*;

/// The skill fields an agent may write; `project_id` last (creation only).
pub(super) const SKILL_FIELDS: &[&str] = &[
    "name",
    "description",
    "icon",
    "category",
    "content",
    "license",
    "project_id",
];

/// A version list without its bodies: an agent reads one by its index.
fn version_index(
    entries: impl Iterator<Item = (u32, chrono::DateTime<chrono::Utc>, bool)>,
) -> Value {
    Value::Array(
        entries
            .map(|(index, created_at, current)| {
                json!({ "version_index": index, "created_at": created_at, "current": current })
            })
            .collect(),
    )
}

impl KronnToolExecutor {
    pub(super) async fn execute_version_tool(&self, call: &ToolCall) -> ToolOutcome {
        let project_id = self.effective_project_id().await;
        match call.name.as_str() {
            "skill_get" => skill_get(call, project_id.as_deref()),
            "qp_get" => self.qp_get(call, project_id.as_deref()).await,
            _ => fail(call, "Unknown version tool"),
        }
    }

    /// `skill_create` / `skill_update` through the HTTP handlers, as the
    /// bridge caller the gate admitted: the handler keeps its agent rules.
    pub(super) async fn execute_skill_write(
        &self,
        call: &ToolCall,
        caller: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    ) -> ToolOutcome {
        use axum::extract::{Path, State};
        use axum::Json;
        if caller.is_none() {
            return fail(call, "Kronn refused this skill write: no admitted caller");
        }
        let args = &call.arguments;
        let mut body = match call.name.as_str() {
            "skill_create" => json!({}),
            _ => {
                let Some(id) = args["skill_id"].as_str().map(str::trim) else {
                    return fail(call, "missing required field `skill_id`");
                };
                let Some(existing) = crate::core::skills::get_skill(id) else {
                    return fail(call, format!("skill `{id}` not found"));
                };
                // The stored fields this tool may change; the project is kept.
                let stored = json!(existing);
                Value::Object(
                    SKILL_FIELDS[..SKILL_FIELDS.len() - 1]
                        .iter()
                        .filter_map(|key| Some(((*key).to_owned(), stored.get(*key)?.clone())))
                        .collect(),
                )
            }
        };
        let writable: &[&str] = if call.name == "skill_create" {
            SKILL_FIELDS
        } else {
            &SKILL_FIELDS[..SKILL_FIELDS.len() - 1]
        };
        for key in writable {
            if let Some(value) = args.get(*key) {
                body[*key] = value.clone();
            }
        }
        let request: crate::models::CreateSkillRequest = match serde_json::from_value(body) {
            Ok(request) => request,
            Err(error) => return fail(call, format!("invalid skill fields: {error}")),
        };
        let Json(res) = if call.name == "skill_create" {
            crate::api::skills::create(State(self.state.clone()), caller, Json(request)).await
        } else {
            let id = args["skill_id"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_owned();
            crate::api::skills::update(Path(id), State(self.state.clone()), caller, Json(request))
                .await
        };
        unwrap_api(call, res.success, res.data, res.error)
    }

    async fn qp_get(&self, call: &ToolCall, project_id: Option<&str>) -> ToolOutcome {
        let Some(id) = call.arguments["qp_id"]
            .as_str()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            return fail(call, "missing required field `qp_id`");
        };
        let wanted = match call.arguments.get("version") {
            None | Some(Value::Null) => None,
            Some(value) => match value
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n >= 1)
            {
                Some(index) => Some(index),
                None => return fail(call, "`version` is a version_index from 1"),
            },
        };
        let lookup = id.to_owned();
        let found = self
            .state
            .db
            .with_read_conn(move |conn| {
                let saved = crate::db::quick_prompts::get_quick_prompt(conn, &lookup)?;
                let versions = crate::db::quick_prompts::list_quick_prompt_versions(conn, &lookup)?;
                Ok((saved, versions))
            })
            .await;
        let (saved, versions) = match found {
            Ok(found) => found,
            Err(error) => return fail(call, format!("DB error: {error}")),
        };
        let Some(saved) =
            saved.filter(|qp| project_is_in_scope(qp.project_id.as_deref(), project_id))
        else {
            return fail(
                call,
                "Quick Prompt is not available in this discussion's scope — call qp_list again",
            );
        };
        if let Some(index) = wanted {
            return match versions.into_iter().find(|v| v.version_index == index) {
                Some(version) => ok(call, json!(version)),
                None => fail(call, format!("this Quick Prompt has no version {index}")),
            };
        }
        let newest = versions.iter().map(|v| v.version_index).max();
        let mut body = json!(saved);
        body["versions"] = version_index(versions.iter().map(|v| {
            (
                v.version_index,
                v.created_at,
                Some(v.version_index) == newest,
            )
        }));
        ok(call, body)
    }
}

fn skill_get(call: &ToolCall, project_id: Option<&str>) -> ToolOutcome {
    let Some(id) = call.arguments["skill_id"]
        .as_str()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return fail(call, "missing required field `skill_id`");
    };
    let (base, pinned) = match crate::core::skill_versions::split_pin(id) {
        Ok(split) => split,
        Err(error) => return fail(call, error),
    };
    let Some(skill) = crate::core::skills::get_skill(base)
        .filter(|skill| project_is_in_scope(skill.project_id.as_deref(), project_id))
    else {
        return fail(
            call,
            "Skill is not available in this discussion's scope — check its id",
        );
    };
    if let Some(index) = pinned {
        return match crate::core::skill_versions::version(base, index) {
            Ok(version) => ok(call, json!(version)),
            Err(error) => fail(call, error),
        };
    }
    let mut body = json!(skill);
    if !skill.is_builtin {
        match crate::core::skill_versions::history(base) {
            Ok(versions) => {
                body["versions"] = version_index(
                    versions
                        .iter()
                        .map(|v| (v.version_index, v.created_at, v.current)),
                )
            }
            Err(error) => body["versions_unavailable"] = json!(error),
        }
    }
    ok(call, body)
}
