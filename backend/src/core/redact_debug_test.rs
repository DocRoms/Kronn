//! KT-2043 — the session and server tokens KT-2039 left out never print
//! through `Debug`. Lives under `core::redact*` so the gate
//! `cargo test --lib core::redact` runs it; this crate sees every layer.

use serde::de::DeserializeOwned;
use serde_json::json;

/// Every fake secret of this file shares this prefix: no `Debug` output may
/// hold it.
const FAKE: &str = "Fake2043";

fn parse<T: DeserializeOwned>(value: serde_json::Value) -> T {
    serde_json::from_value(value).expect("fixture parses")
}

/// One row of the table: the type's name, its `{:?}` and `{:#?}`, and the
/// non-secret values that must still print.
struct Row {
    name: &'static str,
    compact: String,
    pretty: String,
    kept: &'static [&'static str],
}

fn row<T: std::fmt::Debug>(name: &'static str, value: &T, kept: &'static [&'static str]) -> Row {
    Row {
        name,
        compact: format!("{value:?}"),
        pretty: format!("{value:#?}"),
        kept,
    }
}

fn rows() -> Vec<Row> {
    use crate::api::{contacts, disc_invite, discover, plugin_portability, setup, themes, ws};
    vec![
        row(
            "InviteTokenIssued",
            &parse::<crate::db::discussion_sessions::InviteTokenIssued>(json!({
                "token": "Fake2043InviteToken",
                "disc_id": "disc-1",
                "expires_at": "2026-11-24T00:00:00Z",
            })),
            &["disc-1", "2026-11-24T00:00:00Z"],
        ),
        row(
            "JoinViaTokenResult",
            &parse::<crate::db::discussion_sessions::JoinViaTokenResult>(json!({
                "disc_id": "disc-1",
                "session_pk": 7,
                "resume_token": "Fake2043ResumeToken",
            })),
            &["disc-1", "7"],
        ),
        row(
            "InviteResponse",
            &parse::<disc_invite::InviteResponse>(json!({
                "token": "Fake2043InviteToken",
                "disc_id": "disc-1",
                "expires_at": "2026-11-24T00:00:00Z",
                "ttl_seconds": 600,
                "instruction_text": "Call disc_join with Fake2043InviteToken",
                "instruction_text_minimal": "disc_join Fake2043InviteToken",
            })),
            &["disc-1", "600"],
        ),
        row(
            "PeerJoinRequest",
            &parse::<disc_invite::PeerJoinRequest>(json!({
                "token": "Fake2043InviteToken",
                "agent_type": "ClaudeCode",
                "session_id": "sess-1",
            })),
            &["ClaudeCode", "sess-1"],
        ),
        row(
            "PeerJoinResponse",
            &parse::<disc_invite::PeerJoinResponse>(json!({
                "disc_id": "disc-1",
                "session_pk": 7,
                "resume_token": "Fake2043ResumeToken",
                "peer_count": 2,
                "disc_title": "Room title",
                "note_count": 0,
                "recent_messages": [],
                "plan_snapshot": {
                    "current": [],
                    "recently_completed": [],
                    "current_total": 0,
                    "completed_total": 0,
                    "later_total": 0,
                },
                "next_steps": "use disc_append",
            })),
            &["disc-1", "Room title", "use disc_append"],
        ),
        row(
            "PeerResumeRequest",
            &parse::<disc_invite::PeerResumeRequest>(json!({
                "agent_type": "Codex",
                "session_id": "sess-1",
                "resume_token": "Fake2043ResumeToken",
                "next_resume_token": "Fake2043NextResumeToken",
                "expected_disc_id": "disc-1",
            })),
            &["Codex", "sess-1", "disc-1"],
        ),
        row(
            "PeerResumeResponse",
            &parse::<disc_invite::PeerResumeResponse>(json!({
                "disc_id": "disc-1",
                "session_pk": 7,
                "resume_token": "Fake2043ResumeToken",
            })),
            &["disc-1", "7"],
        ),
        row(
            "OrchestratorReturnResumeRequest",
            &parse::<disc_invite::OrchestratorReturnResumeRequest>(json!({
                "agent_type": "Codex",
                "session_id": "sess-1",
                "resume_token": "Fake2043ResumeToken",
                "next_resume_token": "Fake2043NextResumeToken",
                "expected_child_disc_id": "child-1",
            })),
            &["Codex", "sess-1", "child-1"],
        ),
        row(
            "ClaimByTokenRequest",
            &parse::<disc_invite::ClaimByTokenRequest>(json!({
                "token": "Fake2043InviteToken",
                "from_invite_code": "invite-public-1",
                "secret": "Fake2043PairingSecret",
            })),
            &["invite-public-1"],
        ),
        row(
            "FetchFileRequest",
            &parse::<disc_invite::FetchFileRequest>(json!({
                "file_id": "file-1",
                "from_invite_code": "invite-public-1",
                "secret": "Fake2043PairingSecret",
            })),
            &["file-1", "invite-public-1"],
        ),
        row(
            "PairRequest",
            &parse::<contacts::PairRequest>(json!({
                "from_invite_code": "invite-public-1",
                "offer_secret": "Fake2043OfferSecret",
                "reverse_secret": "Fake2043ReverseSecret",
            })),
            &["invite-public-1"],
        ),
        row(
            "WsAuthQuery",
            &parse::<ws::WsAuthQuery>(json!({ "token": "Fake2043WsToken" })),
            &["Some("],
        ),
        row(
            "NetworkExposure",
            &setup::NetworkExposure {
                exposed: true,
                restart_required: false,
                in_docker: false,
                kronn_bind: None,
                port: 3140,
                reachable_ips: Vec::new(),
                auth_token_set: true,
                tls_terminated_by_proxy: true,
                tls_declared_by_env: false,
                public_origin: Some("https://kronn.example".into()),
                plain_http_blocked: false,
                issued_auth_token: Some("Fake2043ServerAuthToken".into()),
            },
            &["3140", "https://kronn.example", "auth_token_set: true"],
        ),
        row(
            "ExportPluginBundleRequest",
            &parse::<plugin_portability::ExportPluginBundleRequest>(json!({
                "config_ids": ["cfg-1"],
                "include_values": true,
                "passphrase": "Fake2043Passphrase",
                "confirmation": "EXPORT",
            })),
            &["cfg-1", "EXPORT"],
        ),
        row(
            "ImportPluginBundleRequest",
            &parse::<plugin_portability::ImportPluginBundleRequest>(json!({
                "content": "bundle-body",
                "passphrase": "Fake2043Passphrase",
                "accept_args_for": ["cfg-1"],
                "placeholder_values": { "cfg-1": { "API_KEY": "Fake2043PlaceholderValue" } },
            })),
            &["bundle-body", "cfg-1", "API_KEY"],
        ),
        row(
            "StrongGrantRequest",
            &parse::<crate::models::strong_confirm::StrongGrantRequest>(json!({
                "method": "DELETE",
                "path": "/api/x",
                "code": "Fake2043Code",
            })),
            &["DELETE", "/api/x"],
        ),
        row(
            "StrongGrant",
            &parse::<crate::models::strong_confirm::StrongGrant>(json!({
                "grant": "Fake2043Grant",
                "expires_in_secs": 60,
            })),
            &["60"],
        ),
        row(
            "Presented",
            &crate::api::strong_confirm::Presented {
                code: Some("Fake2043Code".into()),
                grant: Some("Fake2043Grant".into()),
            },
            &["Some("],
        ),
        row(
            "SetTrustRequest",
            &parse::<crate::models::trust::SetTrustRequest>(json!({
                "level": "trusted",
                "acknowledged_risks": true,
                "proof": "Fake2043Code",
            })),
            &["Trusted", "acknowledged_risks: true"],
        ),
        row(
            "UnlockRequest",
            &parse::<themes::UnlockRequest>(json!({ "code": "Fake2043UnlockCode" })),
            &["UnlockRequest"],
        ),
        row(
            "RepoSourceAuth::Token",
            &discover::RepoSourceAuth::Token {
                token: "Fake2043RepoToken".into(),
                api_url: Some("https://gitlab.example/api/v4".into()),
            },
            &["https://gitlab.example/api/v4"],
        ),
        row(
            "RepoSourceAuth::GitLabCli",
            &discover::RepoSourceAuth::GitLabCli {
                host: Some("gitlab.example".into()),
            },
            &["gitlab.example"],
        ),
    ]
    .into_iter()
    .chain(env_header_and_variable_rows())
    .collect()
}

/// KT-2043 review — secrets that live in a map or a list whose field name says
/// nothing (`env`, `headers`, `variables`, `value`…): an MCP server's env
/// values, a request's headers, run variables (one may be the manual override
/// of a project env credential), recovery codes, the pairing secret.
fn env_header_and_variable_rows() -> Vec<Row> {
    use crate::api::{agent_api, mcp_remote, plugin_portability, quick_prompts, trust};
    use crate::db::{discussion_actions, discussion_questions, live_page_actions};
    use crate::models::{agent_jobs, discussions, mcp, multiuser, quick, workflows};
    vec![
        row(
            "CreateMcpConfigRequest",
            &parse::<mcp::CreateMcpConfigRequest>(json!({
                "server_id": "srv-1",
                "label": "GitHub work",
                "env": { "GITHUB_TOKEN": "Fake2043EnvValue" },
                "args_override": null,
                "is_global": false,
                "project_ids": ["proj-1"],
                "custom_spec": {
                    "name": "Custom",
                    "description": "d",
                    "base_url": "https://api.example",
                    "auth": "None",
                    "fields": [{ "label": "X-Api-Key", "value": "Fake2043FieldValue" }],
                },
            })),
            &[
                "srv-1",
                "GitHub work",
                "GITHUB_TOKEN",
                "proj-1",
                "X-Api-Key",
            ],
        ),
        row(
            "UpdateMcpConfigRequest",
            &parse::<mcp::UpdateMcpConfigRequest>(json!({
                "label": "GitHub work",
                "env": { "GITHUB_TOKEN": "Fake2043EnvValue" },
            })),
            &["GitHub work", "GITHUB_TOKEN"],
        ),
        row(
            "CustomApiField",
            &parse::<mcp::CustomApiField>(json!({
                "label": "X-Api-Key",
                "value": "Fake2043FieldValue",
            })),
            &["X-Api-Key"],
        ),
        row(
            "ApiDefaultHeader",
            &parse::<mcp::ApiDefaultHeader>(json!({
                "name": "X-Api-Key",
                "value": "Fake2043HeaderValue",
            })),
            &["X-Api-Key"],
        ),
        // KT-2053 — an OAuth2 extra header's template is a literal or an
        // `{ENV_KEY}` template, as `ApiDefaultHeader.value`; it prints nested in
        // an API spec's `ApiAuthKind::OAuth2ClientCredentials`.
        row(
            "OAuth2ExtraHeader",
            &parse::<mcp::OAuth2ExtraHeader>(json!({
                "name": "x-api-key",
                "value_template": "Fake2043ExtraHeaderValue",
            })),
            &["x-api-key"],
        ),
        row(
            "ApiAuthKind::OAuth2ClientCredentials",
            &parse::<mcp::ApiAuthKind>(json!({
                "OAuth2ClientCredentials": {
                    "token_url": "https://ims.example/token",
                    "client_id_env": "CLIENT_ID",
                    "client_secret_env": "CLIENT_SECRET",
                    "scope": "openid",
                    "extra_headers": [
                        { "name": "x-api-key", "value_template": "Fake2043ExtraHeaderValue" },
                    ],
                },
            })),
            &["https://ims.example/token", "CLIENT_SECRET", "x-api-key"],
        ),
        row(
            "VibeMcpEntry",
            &parse::<crate::core::mcp_scanner::VibeMcpEntry>(json!({
                "name": "github",
                "transport": "stdio",
                "env": { "GITHUB_TOKEN": "Fake2043EnvValue" },
            })),
            &["github", "GITHUB_TOKEN"],
        ),
        row(
            "PortablePluginConfig",
            &parse::<plugin_portability::PortablePluginConfig>(json!({
                "source_config_id": "cfg-1",
                "server": {
                    "id": "srv-1",
                    "name": "Srv",
                    "description": "d",
                    "transport": "ApiOnly",
                    "source": "Manual",
                },
                "label": "GitHub work",
                "env_keys": ["GITHUB_TOKEN"],
                "values": { "GITHUB_TOKEN": "Fake2043EnvValue" },
                "args_override": null,
                "was_global": false,
                "include_general": true,
                "preferred_interface": "mcp",
            })),
            &["cfg-1", "srv-1", "GitHub work", "GITHUB_TOKEN"],
        ),
        row(
            "QuickApi",
            &parse::<quick::QuickApi>(json!({
                "id": "qa-1",
                "name": "List issues",
                "icon": "x",
                "project_id": null,
                "api_plugin_slug": "github",
                "api_config_id": "cfg-1",
                "api_endpoint_path": "/issues",
                "api_query": { "state": "open" },
                "api_headers": { "Authorization": "Bearer Fake2043HeaderValue" },
                "variables": [],
                "created_at": "2026-11-24T00:00:00Z",
                "updated_at": "2026-11-24T00:00:00Z",
            })),
            &["qa-1", "/issues", "open", "Authorization"],
        ),
        row(
            "CreateQuickApiRequest",
            &parse::<quick::CreateQuickApiRequest>(json!({
                "name": "List issues",
                "icon": null,
                "project_id": null,
                "api_plugin_slug": "github",
                "api_config_id": "cfg-1",
                "api_endpoint_path": "/issues",
                "api_method": null,
                "api_query": null,
                "api_path_params": null,
                "api_headers": { "Authorization": "Bearer Fake2043HeaderValue" },
                "api_body": null,
                "api_extract": null,
                "api_pagination": null,
                "api_timeout_ms": null,
                "api_max_retries": null,
            })),
            &["List issues", "/issues", "Authorization"],
        ),
        row(
            "RunQuickApiRequest",
            &parse::<quick::RunQuickApiRequest>(json!({
                "variables": { "token": "Fake2043VariableValue" },
            })),
            &["\"token\""],
        ),
        row(
            "RunQuickExecRequest",
            &parse::<quick::RunQuickExecRequest>(json!({
                "variables": { "token": "Fake2043VariableValue" },
            })),
            &["\"token\""],
        ),
        row(
            "WorkflowStep",
            &workflows::WorkflowStep {
                name: "call-api".into(),
                api_headers: Some(
                    [(
                        "Authorization".to_string(),
                        "Fake2043HeaderValue".to_string(),
                    )]
                    .into(),
                ),
                quick_prompt_variables: [(
                    "token".to_string(),
                    "Fake2043VariableValue".to_string(),
                )]
                .into(),
                sub_workflow_variables: [(
                    "token".to_string(),
                    "Fake2043VariableValue".to_string(),
                )]
                .into(),
                notify_config: Some(workflows::NotifyConfig {
                    url: "https://hooks.example/x".into(),
                    method: "POST".into(),
                    headers: [(
                        "Authorization".to_string(),
                        "Fake2043HeaderValue".to_string(),
                    )]
                    .into(),
                    body_template: "{}".into(),
                }),
                ..Default::default()
            },
            &["call-api", "Authorization", "https://hooks.example/x"],
        ),
        row(
            "NotifyConfig",
            &parse::<workflows::NotifyConfig>(json!({
                "url": "https://hooks.example/x",
                "headers": { "Authorization": "Bearer Fake2043HeaderValue" },
            })),
            &["https://hooks.example/x", "POST", "Authorization"],
        ),
        row(
            "CollectApiDataSource",
            &parse::<workflows::CollectApiDataSource>(json!({
                "alias": "issues",
                "quick_api_id": "qa-1",
                "variables": { "token": "Fake2043VariableValue" },
            })),
            &["issues", "qa-1", "\"token\""],
        ),
        row(
            "TriggerWorkflowRequest",
            &parse::<workflows::TriggerWorkflowRequest>(json!({
                "variables": { "token": "Fake2043VariableValue" },
                "state": { "ticket": "EW-1" },
            })),
            &["\"token\"", "EW-1"],
        ),
        row(
            "TestStepRequest",
            &parse::<workflows::TestStepRequest>(json!({
                "step": { "name": "s1", "step_type": { "type": "Agent" } },
                "project_id": null,
                "mock_variables": { "token": "Fake2043VariableValue" },
            })),
            &["s1", "\"token\""],
        ),
        row(
            "CreateDiscussionRequest",
            &parse::<discussions::CreateDiscussionRequest>(json!({
                "project_id": null,
                "title": "Room title",
                "agent": "ClaudeCode",
                "language": "en",
                "initial_prompt": "hello",
                "launch_variables": { "token": "Fake2043VariableValue" },
            })),
            &["Room title", "hello", "\"token\""],
        ),
        row(
            "SendMessageRequest",
            &parse::<discussions::SendMessageRequest>(json!({
                "content": "hello",
                "publication_grant": "Fake2043PublicationGrant",
                "publication_proof": "proof-id-1",
            })),
            &["hello", "proof-id-1"],
        ),
        row(
            "StartAgentBackgroundJobRequest",
            &parse::<agent_jobs::StartAgentBackgroundJobRequest>(json!({
                "quick_exec_id": "qe-1",
                "variables": { "token": "Fake2043VariableValue" },
                "reason": "build",
                "dedupe_key": "k-1",
                "task_execution_id": null,
            })),
            &["qe-1", "build", "\"token\""],
        ),
        row(
            "TrustRecoveryCodes",
            &parse::<crate::models::trust::TrustRecoveryCodes>(json!({
                "codes": ["Fake2043RecoveryCode1", "Fake2043RecoveryCode2"],
            })),
            &["codes"],
        ),
        row(
            "TemplateContext",
            &{
                let mut context = crate::workflows::template::TemplateContext::new();
                context.set("token", "Fake2043VariableValue");
                context.set_builtin("run.id", "run-1");
                context
            },
            &["\"token\"", "run-1"],
        ),
        row(
            "ResolvedVariables",
            &crate::core::execution_variables::ResolvedVariables {
                values: [("token".to_string(), "Fake2043VariableValue".to_string())].into(),
                provenance: Vec::new(),
                resolved_at: chrono::DateTime::UNIX_EPOCH,
            },
            &["\"token\"", "1970"],
        ),
        row(
            "AgentApiCallRequest",
            &parse::<agent_api::AgentApiCallRequest>(json!({
                "api_plugin_slug": "github",
                "endpoint_path": "/issues",
                "query": { "state": "open" },
                "headers": { "Authorization": "Bearer Fake2043HeaderValue" },
            })),
            &["github", "/issues", "open", "Authorization"],
        ),
        row(
            "BatchItem",
            &parse::<quick_prompts::BatchItem>(json!({
                "title": "item 1",
                "variables": { "token": "Fake2043VariableValue" },
            })),
            &["item 1", "\"token\""],
        ),
        row(
            "CompareAgentsRequest",
            &parse::<quick_prompts::CompareAgentsRequest>(json!({
                "prompt": "hello",
                "variables": { "token": "Fake2043VariableValue" },
                "batch_name": "Compare 1",
            })),
            &["hello", "\"token\""],
        ),
        row(
            "McpTriggerWorkflowRequest",
            &parse::<mcp_remote::McpTriggerWorkflowRequest>(json!({
                "workflow_id": "wf-1",
                "variables": { "token": "Fake2043VariableValue" },
            })),
            &["wf-1", "\"token\""],
        ),
        row(
            "McpQpRunRequest",
            &parse::<mcp_remote::McpQpRunRequest>(json!({
                "qp_id": "qp-1",
                "vars": { "token": "Fake2043VariableValue" },
            })),
            &["qp-1", "\"token\""],
        ),
        row(
            "McpBatchItem",
            &parse::<mcp_remote::McpBatchItem>(json!({
                "title": "item 1",
                "vars": { "token": "Fake2043VariableValue" },
            })),
            &["item 1", "\"token\""],
        ),
        row(
            "Confirm",
            &trust::Confirm::on(crate::core::trust_proof::Channel::Bridge)
                .with_header("Fake2043ConfirmCode"),
            &["Bridge"],
        ),
        row(
            "LaunchLivePageActionRequest",
            &parse::<live_page_actions::LaunchLivePageActionRequest>(json!({
                "variables": { "token": "Fake2043VariableValue" },
                "bindings": { "row": "row-1" },
            })),
            &["\"token\"", "row-1"],
        ),
        row(
            "LaunchDiscussionActionRequest",
            &parse::<discussion_actions::LaunchDiscussionActionRequest>(json!({
                "variables": { "token": "Fake2043VariableValue" },
            })),
            &["\"token\""],
        ),
        row(
            "DiscussionQuestionResume",
            &parse::<discussion_questions::DiscussionQuestionResume>(json!({
                "workflow_id": "wf-1",
                "variables": { "token": "Fake2043VariableValue" },
                "state": "pending",
                "run_id": null,
                "error": null,
            })),
            &["wf-1", "\"token\""],
        ),
        row(
            "QuestionResumeLaunch",
            &discussion_questions::QuestionResumeLaunch {
                question_id: "q-1".into(),
                run_id: "run-1".into(),
                workflow_id: "wf-1".into(),
                variables: [("token".to_string(), "Fake2043VariableValue".to_string())].into(),
                source_run_id: "run-0".into(),
                discussion_id: "disc-1".into(),
                project_id: None,
            },
            &["q-1", "wf-1", "\"token\""],
        ),
        row(
            "WsMessage::Presence",
            &parse::<multiuser::WsMessage>(json!({
                "type": "presence",
                "from_pseudo": "alice",
                "from_invite_code": "invite-public-1",
                "from_secret": "Fake2043PairingSecret",
                "online": true,
            })),
            &["alice", "invite-public-1", "online: true"],
        ),
    ]
}

/// The pairing secret travels on the wire exactly as before: a Presence frame
/// read and written back is byte-identical, with and without the secret.
#[test]
fn a_presence_frame_round_trips_byte_identical() {
    use crate::models::multiuser::WsMessage;
    for frame in [
        r#"{"type":"presence","from_pseudo":"alice","from_invite_code":"invite-public-1","from_secret":"Fake2043PairingSecret","online":true}"#,
        r#"{"type":"presence","from_pseudo":"alice","from_invite_code":"invite-public-1","online":false}"#,
    ] {
        let message: WsMessage = serde_json::from_str(frame).expect("frame parses");
        assert_eq!(serde_json::to_string(&message).unwrap(), frame);
    }
}

/// The TypeScript type the frontend reads does not change: the secret stays
/// out of it, and the Presence variant keeps its exact shape.
#[test]
fn the_presence_ts_binding_is_unchanged() {
    use ts_rs::TS;
    let declared = crate::models::multiuser::WsMessage::decl(&ts_rs::Config::default());
    assert!(!declared.contains("from_secret"), "{declared}");
    assert!(
        declared.contains(concat!(
            r#"type WsMessage = { "type": "presence", from_pseudo: string, "#,
            "\n/**\n * The sender's public address. It names the contact; it proves nothing.\n */\n",
            r#"from_invite_code: string, online: boolean, } | { "type": "ping","#,
        )),
        "{declared}"
    );
}

#[test]
fn session_and_server_tokens_do_not_print_through_debug() {
    let mut leaks = Vec::new();
    for row in rows() {
        for printed in [&row.compact, &row.pretty] {
            if printed.contains(FAKE) {
                leaks.push(format!("{}: {printed}", row.name));
            }
            for kept in row.kept {
                assert!(
                    printed.contains(kept),
                    "{}: {kept} lost: {printed}",
                    row.name
                );
            }
        }
    }
    assert!(leaks.is_empty(), "secrets printed:\n{}", leaks.join("\n"));
}

#[test]
fn a_masked_secret_still_reads_as_set() {
    for row in rows() {
        if row.name == "RepoSourceAuth::GitLabCli" {
            continue;
        }
        assert!(
            row.compact.contains("***REDACTED***"),
            "{}: nothing masked: {}",
            row.name,
            row.compact
        );
    }
}
