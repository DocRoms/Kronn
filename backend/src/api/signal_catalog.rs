//! Discoverable contracts for structured proposals in discussion responses.
//! Read-only metadata: reading a contract never creates or launches an action.

use axum::Json;
use serde_json::{json, Value};

use crate::db::discussion_actions::DiscussionActionKind;
use crate::models::ApiResponse;

pub fn catalogue() -> Value {
    let kinds = [
        DiscussionActionKind::Workflow,
        DiscussionActionKind::QuickPrompt,
        DiscussionActionKind::QuickApi,
        DiscussionActionKind::QuickExec,
    ];
    json!({
        "schema_version": 1,
        "scope": "Propose existing automations from a discussion response. Historical Planning, audit and Quick Prompt improvement signals remain supported by their existing contracts.",
        "signals": [{
            "name": "kronn-action",
            "format": "One JSON object in a Markdown code fence named kronn-action.",
            "effect": "Kronn displays a persistent action card with the target and editable inputs. Only the human click launches it; emitting or reading this contract never runs the target.",
            "human_approval_required": true,
            "discovery": {
                "workflow": "workflow_list",
                "quick_prompt": "qp_list",
                "quick_api": "qa_list",
                "quick_exec": "qe_list"
            },
            "rules": [
                "Resolve a real existing target id with the listed discovery tool. Never invent an id or run the target to propose it.",
                "Read the target's available variable declarations before suggesting values. Omit unknown or missing inputs: the human can fill them in the card.",
                "Use agent_suggestion for an agent's suggested value. Never include secrets or resolved environment/context values; Kronn resolves those server-side.",
                "Omit project_id to use the target's project, then the discussion's project. An explicit project must match the target's own project when it has one.",
                "Emit one card per intended proposal. Re-ingesting the same message and fence position does not create another action; a new message is a new proposal.",
                "Invalid targets or payloads produce a diagnostic card. Historical signal formats keep their own parsers and are not replaced by this catalogue."
            ],
            "payload_schema": {
                "type": "object",
                "required": ["kind", "target_id"],
                "additionalProperties": false,
                "properties": {
                    "kind": {"type": "string", "enum": kinds.map(DiscussionActionKind::as_db_str)},
                    "target_id": {"type": "string", "minLength": 1},
                    "project_id": {"type": ["string", "null"]},
                    "values": {
                        "type": "array",
                        "items": {
                            "type": "object", "required": ["name"], "additionalProperties": false,
                            "properties": {
                                "name": {"type": "string"},
                                "value": {"type": ["string", "null"]},
                                "provenance": {"type": "string", "enum": ["user_input", "agent_suggestion"], "default": "user_input"},
                                "source_ref": {"type": ["string", "null"]},
                                "suggested_by": {"type": ["string", "null"]}
                            }
                        }
                    }
                }
            }
        }]
    })
}

/// GET /api/signals/catalog — the same registry consumed by MCP and native tools.
pub async fn get() -> Json<ApiResponse<Value>> {
    Json(ApiResponse::ok(catalogue()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::discussion_actions::{ActionFence, DiscussionActionValueProvenance};

    #[test]
    fn every_advertised_kind_decodes_as_a_real_proposal() {
        let catalog = catalogue();
        let signal = &catalog["signals"][0];
        assert_eq!(catalog["schema_version"], 1);
        assert_eq!(signal["human_approval_required"], true);
        let kinds = signal["payload_schema"]["properties"]["kind"]["enum"]
            .as_array()
            .unwrap();
        assert_eq!(kinds.len(), 4);
        for kind in kinds {
            let proposal: ActionFence = serde_json::from_value(json!({
                "kind":kind, "target_id":"discovered-id",
                "values":[{"name":"topic", "value":"Release", "provenance":"agent_suggestion", "suggested_by":"agent"}]
            })).unwrap();
            assert_eq!(proposal.kind.as_db_str(), kind.as_str().unwrap());
            assert_eq!(
                proposal.values[0].provenance,
                DiscussionActionValueProvenance::AgentSuggestion
            );
            assert!(signal["discovery"][kind.as_str().unwrap()].is_string());
        }
        assert!(!kinds.contains(&json!("invalid")));
        assert!(
            !signal["payload_schema"]["properties"]["values"]["items"]["properties"]["provenance"]
                ["enum"]
                .as_array()
                .unwrap()
                .contains(&json!("project_env"))
        );
    }
}
