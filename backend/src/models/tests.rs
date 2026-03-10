use crate::models::*;

// ─── TokensConfig ────────────────────────────────────────────────────────

#[test]
fn active_key_for_finds_active() {
    let config = TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: vec![
            ApiKey { id: "1".into(), name: "k1".into(), provider: "anthropic".into(), value: "sk-ant-123".into(), active: true },
            ApiKey { id: "2".into(), name: "k2".into(), provider: "openai".into(), value: "sk-oai-456".into(), active: false },
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
        anthropic: None, openai: None, google: None,
        keys: vec![],
        disabled_overrides: vec![],
    };
    assert_eq!(config.active_key_for("anthropic"), None);
}

#[test]
fn active_key_for_multiple_same_provider() {
    let config = TokensConfig {
        anthropic: None, openai: None, google: None,
        keys: vec![
            ApiKey { id: "1".into(), name: "old".into(), provider: "anthropic".into(), value: "old-key".into(), active: false },
            ApiKey { id: "2".into(), name: "new".into(), provider: "anthropic".into(), value: "new-key".into(), active: true },
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
    assert_eq!(serde_json::to_string(&AgentType::ClaudeCode).unwrap(), "\"ClaudeCode\"");
    assert_eq!(serde_json::to_string(&AgentType::GeminiCli).unwrap(), "\"GeminiCli\"");
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
    let trigger = WorkflowTrigger::Cron { schedule: "0 * * * *".into() };
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

// ─── ConditionAction serialization ───────────────────────────────────────

#[test]
fn condition_action_goto_roundtrip() {
    let action = ConditionAction::Goto { step_name: "step2".into() };
    let json = serde_json::to_string(&action).unwrap();
    assert!(json.contains("Goto"));
    let parsed: ConditionAction = serde_json::from_str(&json).unwrap();
    match parsed {
        ConditionAction::Goto { step_name } => assert_eq!(step_name, "step2"),
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
        RunStatus::Pending, RunStatus::Running, RunStatus::Success,
        RunStatus::Failed, RunStatus::Cancelled, RunStatus::WaitingApproval,
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
    assert!(!toml_str.contains("anthropic"), "Legacy field 'anthropic' should not be serialized: {}", toml_str);
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
        keys: vec![
            ApiKey { id: "1".into(), name: "My Key".into(), provider: "openai".into(), value: "sk-test-123".into(), active: true },
        ],
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
    let t = McpTransport::Sse { url: "https://mcp.linear.app/sse".into() };
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
        claude_code: AgentConfig { full_access: true, ..Default::default() },
        codex: AgentConfig { full_access: false, ..Default::default() },
        gemini_cli: AgentConfig { full_access: true, ..Default::default() },
        kiro: AgentConfig { full_access: false, ..Default::default() },
        vibe: AgentConfig { full_access: true, ..Default::default() },
    };
    assert!(config.full_access_for(&AgentType::ClaudeCode));
    assert!(!config.full_access_for(&AgentType::Codex));
    assert!(config.full_access_for(&AgentType::GeminiCli));
    assert!(config.full_access_for(&AgentType::Vibe));
    assert!(!config.full_access_for(&AgentType::Custom));
}
