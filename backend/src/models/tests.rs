use crate::models::*;

// ─── TokensConfig ────────────────────────────────────────────────────────

#[test]
fn active_key_for_finds_active() {
    let config = TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: vec![
            ApiKey {
                id: "1".into(),
                name: "k1".into(),
                provider: "anthropic".into(),
                value: "sk-ant-123".into(),
                active: true,
            },
            ApiKey {
                id: "2".into(),
                name: "k2".into(),
                provider: "openai".into(),
                value: "sk-oai-456".into(),
                active: false,
            },
        ],
        disabled_overrides: vec![],
    };
    assert_eq!(config.active_key_for("anthropic"), Some("sk-ant-123"));
    assert_eq!(config.active_key_for("openai"), None); // not active
    assert_eq!(config.active_key_for("google"), None); // no key
}

#[test]
fn active_key_for_empty_keys() {
    let config = TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: vec![],
        disabled_overrides: vec![],
    };
    assert_eq!(config.active_key_for("anthropic"), None);
}

#[test]
fn active_key_for_multiple_same_provider() {
    let config = TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: vec![
            ApiKey {
                id: "1".into(),
                name: "old".into(),
                provider: "anthropic".into(),
                value: "old-key".into(),
                active: false,
            },
            ApiKey {
                id: "2".into(),
                name: "new".into(),
                provider: "anthropic".into(),
                value: "new-key".into(),
                active: true,
            },
        ],
        disabled_overrides: vec![],
    };
    assert_eq!(config.active_key_for("anthropic"), Some("new-key"));
}

// ─── AgentType serialization ─────────────────────────────────────────────

#[test]
fn agent_type_roundtrip() {
    let types = vec![
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::Vibe,
        AgentType::GeminiCli,
        AgentType::Kiro,
        AgentType::Custom,
    ];
    for t in &types {
        let json = serde_json::to_string(t).unwrap();
        let parsed: AgentType = serde_json::from_str(&json).unwrap();
        assert_eq!(&parsed, t);
    }
}

#[test]
fn agent_type_json_format() {
    assert_eq!(
        serde_json::to_string(&AgentType::ClaudeCode).unwrap(),
        "\"ClaudeCode\""
    );
    assert_eq!(
        serde_json::to_string(&AgentType::GeminiCli).unwrap(),
        "\"GeminiCli\""
    );
}

// ─── WorkflowTrigger serialization ───────────────────────────────────────

#[test]
fn workflow_trigger_manual_roundtrip() {
    let trigger = WorkflowTrigger::Manual;
    let json = serde_json::to_string(&trigger).unwrap();
    let parsed: WorkflowTrigger = serde_json::from_str(&json).unwrap();
    assert!(matches!(parsed, WorkflowTrigger::Manual));
}

#[test]
fn workflow_trigger_cron_roundtrip() {
    let trigger = WorkflowTrigger::Cron {
        schedule: "0 * * * *".into(),
    };
    let json = serde_json::to_string(&trigger).unwrap();
    let parsed: WorkflowTrigger = serde_json::from_str(&json).unwrap();
    match parsed {
        WorkflowTrigger::Cron { schedule } => assert_eq!(schedule, "0 * * * *"),
        _ => panic!("Expected Cron trigger"),
    }
}

// ─── StepMode serialization ──────────────────────────────────────────────

#[test]
fn step_mode_normal_roundtrip() {
    let mode = StepMode::Normal;
    let json = serde_json::to_string(&mode).unwrap();
    let parsed: StepMode = serde_json::from_str(&json).unwrap();
    assert!(matches!(parsed, StepMode::Normal));
}

// ─── StepType serialization ──────────────────────────────────────────────

#[test]
fn step_type_default_is_agent() {
    assert_eq!(StepType::default(), StepType::Agent);
}

#[test]
fn step_type_agent_roundtrip() {
    let st = StepType::Agent;
    let json = serde_json::to_string(&st).unwrap();
    assert!(json.contains("Agent"));
    let parsed: StepType = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, StepType::Agent);
}

#[test]
fn step_type_api_call_roundtrip() {
    let st = StepType::ApiCall;
    let json = serde_json::to_string(&st).unwrap();
    assert!(json.contains("ApiCall"));
    let parsed: StepType = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, StepType::ApiCall);
}

#[test]
fn workflow_step_without_step_type_defaults_to_agent() {
    // Simulates existing JSON from DB that doesn't have step_type
    let json = r#"{"name":"test","agent":"ClaudeCode","prompt_template":"do stuff","mode":{"type":"Normal"}}"#;
    let step: WorkflowStep = serde_json::from_str(json).unwrap();
    assert_eq!(step.step_type, StepType::Agent);
    assert!(step.description.is_none());
}

// ─── ConditionAction serialization ───────────────────────────────────────

#[test]
fn condition_action_goto_roundtrip() {
    let action = ConditionAction::Goto {
        step_name: "step2".into(),
        max_iterations: None,
    };
    let json = serde_json::to_string(&action).unwrap();
    assert!(json.contains("Goto"));
    let parsed: ConditionAction = serde_json::from_str(&json).unwrap();
    match parsed {
        ConditionAction::Goto {
            step_name,
            max_iterations,
        } => {
            assert_eq!(step_name, "step2");
            assert_eq!(max_iterations, None);
        }
        _ => panic!("Expected Goto"),
    }
}

#[test]
fn condition_action_goto_with_max_iterations_roundtrip() {
    let action = ConditionAction::Goto {
        step_name: "implement".into(),
        max_iterations: Some(5),
    };
    let json = serde_json::to_string(&action).unwrap();
    assert!(json.contains("max_iterations"));
    assert!(json.contains("5"));
    let parsed: ConditionAction = serde_json::from_str(&json).unwrap();
    match parsed {
        ConditionAction::Goto {
            step_name,
            max_iterations,
        } => {
            assert_eq!(step_name, "implement");
            assert_eq!(max_iterations, Some(5));
        }
        _ => panic!("Expected Goto"),
    }
}

#[test]
fn condition_action_goto_back_compat_no_max() {
    // Existing workflows pre-Phase-6 serialised Goto without
    // `max_iterations`. They must still parse cleanly with the new
    // field absent → None.
    let json = r#"{"type":"Goto","step_name":"implement"}"#;
    let parsed: ConditionAction = serde_json::from_str(json).unwrap();
    match parsed {
        ConditionAction::Goto {
            step_name,
            max_iterations,
        } => {
            assert_eq!(step_name, "implement");
            assert_eq!(max_iterations, None);
        }
        _ => panic!("Expected Goto"),
    }
}

// ─── ApiResponse ─────────────────────────────────────────────────────────

#[test]
fn api_response_ok() {
    let resp = ApiResponse::ok("hello");
    assert!(resp.success);
    assert_eq!(resp.data, Some("hello"));
    assert!(resp.error.is_none());
}

#[test]
fn api_response_err() {
    let resp = ApiResponse::<String>::err("something went wrong");
    assert!(!resp.success);
    assert!(resp.data.is_none());
    assert_eq!(resp.error, Some("something went wrong".into()));
}

// ─── RunStatus ───────────────────────────────────────────────────────────

#[test]
fn run_status_roundtrip() {
    let statuses = vec![
        RunStatus::Pending,
        RunStatus::Running,
        RunStatus::Success,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::WaitingApproval,
    ];
    for s in &statuses {
        let json = serde_json::to_string(s).unwrap();
        let parsed: RunStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(&parsed, s);
    }
}

// ─── AiAuditStatus ──────────────────────────────────────────────────────

#[test]
fn ai_audit_status_default() {
    assert_eq!(AiAuditStatus::default(), AiAuditStatus::NoTemplate);
}

// ─── Config TOML serialization ──────────────────────────────────────────

#[test]
fn tokens_config_legacy_fields_not_serialized() {
    let config = TokensConfig {
        anthropic: Some("sk-old".into()),
        openai: None,
        google: None,
        keys: vec![],
        disabled_overrides: vec![],
    };
    let toml_str = toml::to_string(&config).unwrap();
    // Legacy fields have skip_serializing, should NOT appear in output
    assert!(
        !toml_str.contains("anthropic"),
        "Legacy field 'anthropic' should not be serialized: {}",
        toml_str
    );
}

#[test]
fn tokens_config_legacy_fields_deserialized() {
    let toml_str = r#"
anthropic = "sk-old-key"
"#;
    let config: TokensConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.anthropic, Some("sk-old-key".into()));
    assert!(config.keys.is_empty());
}

#[test]
fn tokens_config_keys_toml_roundtrip() {
    let config = TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: vec![ApiKey {
            id: "1".into(),
            name: "My Key".into(),
            provider: "openai".into(),
            value: "sk-test-123".into(),
            active: true,
        }],
        disabled_overrides: vec!["google".into()],
    };
    let toml_str = toml::to_string_pretty(&config).unwrap();
    eprintln!("TOML output:\n{}", toml_str);
    let parsed: TokensConfig = toml::from_str(&toml_str).unwrap();
    assert_eq!(parsed.keys.len(), 1);
    assert_eq!(parsed.keys[0].value, "sk-test-123");
    assert_eq!(parsed.keys[0].provider, "openai");
    assert!(parsed.keys[0].active);
    assert_eq!(parsed.disabled_overrides, vec!["google".to_string()]);
}

// ─── McpTransport serialization ──────────────────────────────────────────

#[test]
fn mcp_transport_stdio_roundtrip() {
    let t = McpTransport::Stdio {
        command: "npx".into(),
        args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
    };
    let json = serde_json::to_string(&t).unwrap();
    let parsed: McpTransport = serde_json::from_str(&json).unwrap();
    match parsed {
        McpTransport::Stdio { command, args } => {
            assert_eq!(command, "npx");
            assert_eq!(args.len(), 2);
        }
        _ => panic!("Expected Stdio"),
    }
}

#[test]
fn mcp_transport_sse_roundtrip() {
    let t = McpTransport::Sse {
        url: "https://mcp.linear.app/sse".into(),
    };
    let json = serde_json::to_string(&t).unwrap();
    let parsed: McpTransport = serde_json::from_str(&json).unwrap();
    match parsed {
        McpTransport::Sse { url } => assert_eq!(url, "https://mcp.linear.app/sse"),
        _ => panic!("Expected Sse"),
    }
}

// ─── AgentsConfig::full_access_for ─────────────────────────────────────

#[test]
fn full_access_for_returns_per_agent_setting() {
    let config = AgentsConfig {
        claude_code: AgentConfig {
            full_access: true,
            ..Default::default()
        },
        codex: AgentConfig {
            full_access: false,
            ..Default::default()
        },
        gemini_cli: AgentConfig {
            full_access: true,
            ..Default::default()
        },
        kiro: AgentConfig {
            full_access: false,
            ..Default::default()
        },
        vibe: AgentConfig {
            full_access: true,
            ..Default::default()
        },
        copilot_cli: AgentConfig {
            full_access: false,
            ..Default::default()
        },
        ollama: AgentConfig {
            full_access: true,
            ..Default::default()
        },
        model_tiers: Default::default(),
    };
    assert!(config.full_access_for(&AgentType::ClaudeCode));
    assert!(!config.full_access_for(&AgentType::Codex));
    assert!(config.full_access_for(&AgentType::GeminiCli));
    assert!(config.full_access_for(&AgentType::Vibe));
    assert!(config.full_access_for(&AgentType::Ollama));
    assert!(!config.full_access_for(&AgentType::CopilotCli));
    assert!(!config.full_access_for(&AgentType::Custom));
}

// ─── WsMessage ──────────────────────────────────────────────────────────

#[test]
fn ws_message_presence_round_trip() {
    let msg = WsMessage::Presence {
        from_pseudo: "PeerAlpha".into(),
        from_invite_code: "kronn:PeerAlpha@100.64.1.5:3456".into(),
        online: true,
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"presence""#));
    assert!(json.contains("PeerAlpha"));
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    match parsed {
        WsMessage::Presence {
            from_pseudo,
            online,
            ..
        } => {
            assert_eq!(from_pseudo, "PeerAlpha");
            assert!(online);
        }
        _ => panic!("Expected Presence variant"),
    }
}

#[test]
fn ws_message_ping_pong_round_trip() {
    let ping = WsMessage::Ping {
        timestamp: 1711000000,
    };
    let json = serde_json::to_string(&ping).unwrap();
    assert!(json.contains(r#""type":"ping""#));
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    match parsed {
        WsMessage::Ping { timestamp } => assert_eq!(timestamp, 1711000000),
        _ => panic!("Expected Ping variant"),
    }

    let pong = WsMessage::Pong {
        timestamp: 1711000001,
    };
    let json = serde_json::to_string(&pong).unwrap();
    assert!(json.contains(r#""type":"pong""#));
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    match parsed {
        WsMessage::Pong { timestamp } => assert_eq!(timestamp, 1711000001),
        _ => panic!("Expected Pong variant"),
    }
}

#[test]
fn ws_message_presence_offline() {
    let msg = WsMessage::Presence {
        from_pseudo: "PeerBeta".into(),
        from_invite_code: "kronn:PeerBeta@10.0.0.2:3456".into(),
        online: false,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    match parsed {
        WsMessage::Presence {
            from_pseudo,
            online,
            ..
        } => {
            assert_eq!(from_pseudo, "PeerBeta");
            assert!(!online);
        }
        _ => panic!("Expected Presence variant"),
    }
}

#[test]
fn ws_message_workflow_run_updated_round_trip() {
    // 0.8.2 — TD #247 — guard the wire shape so the frontend WS handler
    // in WorkflowsPage stays in sync with the backend broadcast.
    let msg = WsMessage::WorkflowRunUpdated {
        run_id: "run_test_001".into(),
        workflow_id: "wf_test_001".into(),
        status: "WaitingApproval".into(),
        step_index: 2,
        total_steps: 5,
        current_step: Some("gate_review".into()),
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"workflow_run_updated""#));
    assert!(json.contains(r#""status":"WaitingApproval""#));
    assert!(json.contains(r#""step_index":2"#));
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    match parsed {
        WsMessage::WorkflowRunUpdated {
            run_id,
            workflow_id,
            status,
            step_index,
            total_steps,
            current_step,
        } => {
            assert_eq!(run_id, "run_test_001");
            assert_eq!(workflow_id, "wf_test_001");
            assert_eq!(status, "WaitingApproval");
            assert_eq!(step_index, 2);
            assert_eq!(total_steps, 5);
            assert_eq!(current_step.as_deref(), Some("gate_review"));
        }
        _ => panic!("Expected WorkflowRunUpdated variant"),
    }
}

#[test]
fn ws_message_workflow_run_updated_between_steps() {
    // current_step=None mirrors the "between steps" StepDone broadcast.
    let msg = WsMessage::WorkflowRunUpdated {
        run_id: "run_test_002".into(),
        workflow_id: "wf_test_002".into(),
        status: "Running".into(),
        step_index: 0,
        total_steps: 3,
        current_step: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    match parsed {
        WsMessage::WorkflowRunUpdated { current_step, .. } => assert!(current_step.is_none()),
        _ => panic!("Expected WorkflowRunUpdated variant"),
    }
}

// ─── ApiCall step models (désagentification) ─────────────────────────────
//
// Guards the serde contract — a drift here cascades into every workflow
// stored in the DB and into the TypeScript bindings consumed by the React
// wizard. Every new field on `ExtractSpec` / `PaginationSpec` should add
// a matching case here.

#[test]
fn extract_spec_roundtrip_full() {
    let spec = ExtractSpec {
        path: "$.issues[*].key".into(),
        fallback: Some(serde_json::json!([])),
        fail_on_empty: true,
    };
    let json = serde_json::to_string(&spec).unwrap();
    let parsed: ExtractSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, spec);
}

#[test]
fn extract_spec_roundtrip_minimal_omits_fallback() {
    // `fallback: None` should serialize as absent (skip_serializing_if),
    // not as explicit `null`. Keeps the wire format clean for users.
    let spec = ExtractSpec {
        path: "$.total".into(),
        fallback: None,
        fail_on_empty: false,
    };
    let json = serde_json::to_string(&spec).unwrap();
    assert!(
        !json.contains("fallback"),
        "expected fallback absent from output, got {json}"
    );
    let parsed: ExtractSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, spec);
}

#[test]
fn extract_spec_deserializes_when_fail_on_empty_absent() {
    // 2026-06-11 — the default flipped to `true`: a spec that OMITS the
    // field now opts INTO the `NO_RESULTS` signal on empty extraction
    // (safer — a silent empty result was a footgun). Rows that serialized
    // `false` explicitly are unaffected (verified below). Status stays
    // Success either way; this only changes the emitted signal.
    let parsed: ExtractSpec = serde_json::from_str(r#"{"path":"$.items"}"#).unwrap();
    assert_eq!(parsed.path, "$.items");
    assert!(parsed.fail_on_empty, "absent field now defaults to true");
    assert!(parsed.fallback.is_none());
    // Explicit `false` is still honoured (no behaviour change for old rows).
    let explicit: ExtractSpec =
        serde_json::from_str(r#"{"path":"$.items","fail_on_empty":false}"#).unwrap();
    assert!(!explicit.fail_on_empty, "explicit false must be preserved");
}

#[test]
fn pagination_spec_none_and_auto_roundtrip() {
    for (spec, expected_substring) in [
        (PaginationSpec::None, r#""type":"None""#),
        (
            PaginationSpec::Auto {
                max_pages: Some(25),
            },
            r#""type":"Auto""#,
        ),
        (PaginationSpec::Auto { max_pages: None }, r#""type":"Auto""#),
    ] {
        let json = serde_json::to_string(&spec).unwrap();
        assert!(
            json.contains(expected_substring),
            "serialized {spec:?} as {json}"
        );
        let parsed: PaginationSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, spec);
    }
}

#[test]
fn pagination_spec_offset_cursor_page_roundtrip() {
    let variants = vec![
        PaginationSpec::Offset {
            start_param: "startAt".into(),
            limit_param: "maxResults".into(),
            limit: 50,
            total_path: "$.total".into(),
            max_pages: Some(20),
        },
        PaginationSpec::Cursor {
            cursor_param: "after".into(),
            next_path: "$.pageInfo.endCursor".into(),
            max_pages: None,
        },
        PaginationSpec::Page {
            page_param: "page".into(),
            page_size_param: "per_page".into(),
            page_size: 100,
            has_more_path: "$.has_more".into(),
            max_pages: Some(10),
        },
    ];
    for spec in variants {
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: PaginationSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, spec, "roundtrip mismatch for json={json}");
    }
}

#[test]
fn workflow_step_deserializes_without_api_fields_backcompat() {
    // Regression guard: a workflow JSON row written before the ApiCall
    // fields existed must deserialize cleanly, with every `api_*` field
    // defaulting to `None`. Losing this means every stored workflow in
    // every user's DB becomes unreadable at upgrade. `#[serde(default)]`
    // on each Option enforces it.
    let legacy = r#"{
      "name": "Old agent step",
      "step_type": {"type": "Agent"},
      "agent": "ClaudeCode",
      "prompt_template": "hello",
      "mode": {"type": "Normal"}
    }"#;
    let step: WorkflowStep = serde_json::from_str(legacy).unwrap();
    assert_eq!(step.name, "Old agent step");
    assert!(step.api_plugin_slug.is_none());
    assert!(step.api_config_id.is_none());
    assert!(step.api_extract.is_none());
    assert!(step.api_pagination.is_none());
    assert!(step.api_timeout_ms.is_none());
    assert!(step.api_max_retries.is_none());
    assert!(step.api_output_var.is_none());
}

#[test]
fn workflow_step_api_call_roundtrip() {
    let mut query = std::collections::HashMap::new();
    query.insert("jql".into(), "project = KRONN".into());

    let step = WorkflowStep {
        name: "fetch_issues".into(),
        step_type: StepType::ApiCall,
        description: Some("Pull open bugs".into()),
        agent: AgentType::ClaudeCode,
        prompt_template: String::new(),
        mode: StepMode::Normal,
        output_format: StepOutputFormat::Structured,
        mcp_config_ids: vec![],
        agent_settings: None,
        on_result: vec![],
        on_timeout: None,
        stall_timeout_secs: None,
        retry: None,
        delay_after_secs: None,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        batch_quick_prompt_id: None,
        batch_items_from: None,
        batch_wait_for_completion: None,
        batch_max_items: None,
        batch_workspace_mode: None,
        batch_chain_prompt_ids: vec![],
        batch_concurrent_limit: None,
        quick_api_id: None,
        notify_config: None,
        api_plugin_slug: Some("jira".into()),
        api_config_id: Some("cfg-123".into()),
        api_endpoint_path: Some("/rest/api/3/search".into()),
        api_method: Some("GET".into()),
        api_path_params: None,
        api_query: Some(query),
        api_headers: None,
        api_body: None,
        api_extract: Some(ExtractSpec {
            path: "$.issues[*].key".into(),
            fallback: Some(serde_json::json!([])),
            fail_on_empty: false,
        }),
        api_pagination: Some(PaginationSpec::Auto { max_pages: Some(5) }),
        api_timeout_ms: Some(15_000),
        api_max_retries: Some(3),
        api_output_var: Some("issues".into()),
        gate_message: None,
        gate_request_changes_target: None,
        gate_notify_url: None,
        gate_checkpoint_before: None,
        gate_auto_approve_after_secs: None,
        exec_command: None,
        exec_args: vec![],
        exec_timeout_secs: None,
        exec_setup_command: None,
        exec_setup_args: vec![],
        exec_stdin: None,
        quick_prompt_id: None,
        json_data_payload: None,
        sub_workflow_id: None,
        sub_workflow_foreach_file: None,
        multi_agent_review: None,
    };
    let json = serde_json::to_string(&step).unwrap();
    let parsed: WorkflowStep = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.api_plugin_slug.as_deref(), Some("jira"));
    assert_eq!(
        parsed.api_endpoint_path.as_deref(),
        Some("/rest/api/3/search")
    );
    assert_eq!(parsed.api_output_var.as_deref(), Some("issues"));
    let extract = parsed.api_extract.unwrap();
    assert_eq!(extract.path, "$.issues[*].key");
    match parsed.api_pagination.unwrap() {
        PaginationSpec::Auto { max_pages } => assert_eq!(max_pages, Some(5)),
        other => panic!("expected Auto, got {other:?}"),
    }
}

// ─── 0.8.3 — TypedSchema.on_invalid (Feasibility-Gated triage) ──────────

#[test]
fn typed_schema_defaults_on_invalid_to_continue() {
    // Pre-0.8.3 workflows in DB don't carry `on_invalid`. They MUST
    // parse with `Continue` (the 0.7.0 behavior) — any other default
    // would silently fail every existing TypedSchema workflow on
    // upgrade.
    let json = serde_json::json!({
        "type": "TypedSchema",
        "schema": { "type": "object" }
    });
    let parsed: StepOutputFormat = serde_json::from_value(json).unwrap();
    match parsed {
        StepOutputFormat::TypedSchema { on_invalid, .. } => {
            assert_eq!(on_invalid, OnInvalid::Continue);
        }
        other => panic!("expected TypedSchema, got {other:?}"),
    }
}

#[test]
fn typed_schema_round_trips_on_invalid_fail() {
    let original = StepOutputFormat::TypedSchema {
        schema: serde_json::json!({ "type": "object" }),
        on_invalid: OnInvalid::Fail,
    };
    let json = serde_json::to_string(&original).unwrap();
    assert!(
        json.contains("\"on_invalid\":\"Fail\""),
        "expected on_invalid=Fail in serialized output, got: {json}"
    );
    let parsed: StepOutputFormat = serde_json::from_str(&json).unwrap();
    match parsed {
        StepOutputFormat::TypedSchema { on_invalid, .. } => {
            assert_eq!(on_invalid, OnInvalid::Fail);
        }
        other => panic!("expected TypedSchema, got {other:?}"),
    }
}

#[test]
fn typed_schema_round_trips_explicit_continue() {
    // Explicit `Continue` should round-trip too — frontend may emit it
    // for clarity even though it's the default.
    let original = StepOutputFormat::TypedSchema {
        schema: serde_json::json!({ "type": "string" }),
        on_invalid: OnInvalid::Continue,
    };
    let json = serde_json::to_string(&original).unwrap();
    let parsed: StepOutputFormat = serde_json::from_str(&json).unwrap();
    match parsed {
        StepOutputFormat::TypedSchema { on_invalid, .. } => {
            assert_eq!(on_invalid, OnInvalid::Continue);
        }
        other => panic!("expected TypedSchema, got {other:?}"),
    }
}

// ─── D11 (0.8.11) — typed API error codes ─────────────────────────────
#[test]
fn api_error_code_strings_are_stable_snake_case() {
    use super::ApiErrorCode::*;
    assert_eq!(NotFound.as_str(), "not_found");
    assert_eq!(Validation.as_str(), "validation");
    assert_eq!(Conflict.as_str(), "conflict");
    assert_eq!(Internal.as_str(), "internal");
}

#[test]
fn api_response_err_coded_serializes_error_code_and_plain_err_omits_it() {
    use super::{ApiErrorCode, ApiResponse};
    let coded = ApiResponse::<()>::err_coded(ApiErrorCode::NotFound, "Workflow not found");
    let j = serde_json::to_value(&coded).unwrap();
    assert_eq!(j["success"], false);
    assert_eq!(j["error"], "Workflow not found");
    assert_eq!(j["error_code"], "not_found");

    // Legacy plain err() must NOT emit error_code (back-compatible wire).
    let plain = ApiResponse::<()>::err("boom");
    let jp = serde_json::to_value(&plain).unwrap();
    assert!(
        jp.get("error_code").is_none(),
        "plain err omits error_code, got: {jp}"
    );

    // ok() likewise has no error_code.
    let ok = ApiResponse::ok(42);
    let jo = serde_json::to_value(&ok).unwrap();
    assert!(jo.get("error_code").is_none());
}
