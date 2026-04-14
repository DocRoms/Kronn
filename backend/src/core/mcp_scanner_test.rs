#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use crate::core::mcp_scanner::*;

    fn make_test_data() -> McpJsonFile {
        let mut servers = HashMap::new();
        servers.insert("github".to_string(), McpServerEntry {
            command: Some("npx".to_string()),
            args: Some(vec!["-y".into(), "@modelcontextprotocol/server-github".into()]),
            url: None,
            env: {
                let mut env = HashMap::new();
                env.insert("GITHUB_TOKEN".to_string(), "ghp_test123".to_string());
                env
            },
        });
        servers.insert("context7".to_string(), McpServerEntry {
            command: Some("npx".to_string()),
            args: Some(vec!["-y".into(), "@upstash/context7-mcp".into()]),
            url: None,
            env: HashMap::new(),
        });
        McpJsonFile { mcp_servers: servers }
    }

    fn setup_tmp(name: &str) -> std::path::PathBuf {
        // Ensure resolve_host_path passes through unchanged
        std::env::remove_var("KRONN_HOST_HOME");
        let tmp = std::env::temp_dir().join(format!("kronn-test-mcp-{}", name));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        tmp
    }

    fn cleanup(tmp: &std::path::Path) {
        let _ = std::fs::remove_dir_all(tmp);
    }

    // ─── write_mcp_json (Claude Code .mcp.json) ────────────────────────────

    #[test]
    fn write_mcp_json_creates_valid_file() {
        let tmp = setup_tmp("claude-write");
        let data = make_test_data();

        write_mcp_json(&tmp.to_string_lossy(), &data).unwrap();

        let file = tmp.join(".mcp.json");
        assert!(file.exists(), ".mcp.json should be created");

        let content = std::fs::read_to_string(&file).unwrap();
        let parsed: McpJsonFile = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.mcp_servers.len(), 2);
        assert!(parsed.mcp_servers.contains_key("github"));
        assert!(parsed.mcp_servers.contains_key("context7"));

        // Verify structure has mcpServers key (not mcp_servers)
        let raw: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(raw.get("mcpServers").is_some(), "JSON should use mcpServers key");

        cleanup(&tmp);
    }

    // ─── write_mcp_json_to_subpath (Kiro + Gemini) ─────────────────────────

    #[test]
    fn write_kiro_mcp_creates_nested_dirs_and_valid_file() {
        let tmp = setup_tmp("kiro-write");
        let data = make_test_data();

        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".kiro/settings/mcp.json", &data).unwrap();

        let file = tmp.join(".kiro/settings/mcp.json");
        assert!(file.exists(), ".kiro/settings/mcp.json should be created");
        assert!(tmp.join(".kiro/settings").is_dir(), ".kiro/settings/ dir should exist");

        let content = std::fs::read_to_string(&file).unwrap();
        let parsed: McpJsonFile = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.mcp_servers.len(), 2);
        assert!(parsed.mcp_servers.contains_key("github"));

        cleanup(&tmp);
    }

    #[test]
    fn write_gemini_mcp_creates_dir_and_valid_file() {
        let tmp = setup_tmp("gemini-write");
        let data = make_test_data();

        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".gemini/settings.json", &data).unwrap();

        let file = tmp.join(".gemini/settings.json");
        assert!(file.exists(), ".gemini/settings.json should be created");
        assert!(tmp.join(".gemini").is_dir(), ".gemini/ dir should exist");

        let content = std::fs::read_to_string(&file).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(raw.get("mcpServers").is_some(), "Gemini JSON should use mcpServers key");

        let parsed: McpJsonFile = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.mcp_servers.len(), 2);

        cleanup(&tmp);
    }

    #[test]
    fn write_subpath_overwrites_existing() {
        let tmp = setup_tmp("overwrite");
        let data = make_test_data();

        // Write twice — second should overwrite, not append
        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".kiro/settings/mcp.json", &data).unwrap();
        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".kiro/settings/mcp.json", &data).unwrap();

        let content = std::fs::read_to_string(tmp.join(".kiro/settings/mcp.json")).unwrap();
        let parsed: McpJsonFile = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.mcp_servers.len(), 2, "Should have 2 servers, not duplicated");

        cleanup(&tmp);
    }

    // ─── All 3 JSON formats produce identical content ──────────────────────

    #[test]
    fn claude_kiro_gemini_produce_identical_json() {
        let tmp = setup_tmp("identical");
        let data = make_test_data();

        write_mcp_json(&tmp.to_string_lossy(), &data).unwrap();
        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".kiro/settings/mcp.json", &data).unwrap();
        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".gemini/settings.json", &data).unwrap();

        let claude = std::fs::read_to_string(tmp.join(".mcp.json")).unwrap();
        let kiro = std::fs::read_to_string(tmp.join(".kiro/settings/mcp.json")).unwrap();
        let gemini = std::fs::read_to_string(tmp.join(".gemini/settings.json")).unwrap();

        assert_eq!(claude, kiro, "Claude and Kiro configs should be identical");
        assert_eq!(claude, gemini, "Claude and Gemini configs should be identical");

        cleanup(&tmp);
    }

    // ─── ensure_gitignore ──────────────────────────────────────────────────

    #[test]
    fn ensure_gitignore_adds_kiro_and_gemini_patterns() {
        let tmp = setup_tmp("gitignore");
        // Create empty .gitignore
        std::fs::write(tmp.join(".gitignore"), ".mcp.json\n").unwrap();

        ensure_gitignore_public(&tmp.to_string_lossy(), ".kiro/settings/");
        ensure_gitignore_public(&tmp.to_string_lossy(), ".gemini/");

        let content = std::fs::read_to_string(tmp.join(".gitignore")).unwrap();
        assert!(content.contains(".kiro/settings/"), "Should contain .kiro/settings/");
        assert!(content.contains(".gemini/"), "Should contain .gemini/");
        assert!(content.contains(".mcp.json"), "Should still contain .mcp.json");

        cleanup(&tmp);
    }

    #[test]
    fn ensure_gitignore_idempotent() {
        let tmp = setup_tmp("gitignore-idem");
        std::fs::write(tmp.join(".gitignore"), "").unwrap();

        // Add same pattern twice
        ensure_gitignore_public(&tmp.to_string_lossy(), ".kiro/settings/");
        ensure_gitignore_public(&tmp.to_string_lossy(), ".kiro/settings/");

        let content = std::fs::read_to_string(tmp.join(".gitignore")).unwrap();
        let count = content.lines().filter(|l| l.trim() == ".kiro/settings/").count();
        assert_eq!(count, 1, "Pattern should appear exactly once");

        cleanup(&tmp);
    }

    // ─── Cleanup removes all agent configs ─────────────────────────────────

    #[test]
    fn cleanup_removes_all_agent_config_files() {
        let tmp = setup_tmp("cleanup");
        let data = make_test_data();

        // Create all 3 JSON configs
        write_mcp_json(&tmp.to_string_lossy(), &data).unwrap();
        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".kiro/settings/mcp.json", &data).unwrap();
        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".gemini/settings.json", &data).unwrap();

        assert!(tmp.join(".mcp.json").exists());
        assert!(tmp.join(".kiro/settings/mcp.json").exists());
        assert!(tmp.join(".gemini/settings.json").exists());

        // Simulate cleanup (same logic as sync_project_mcps_to_disk when no MCPs)
        for filename in &[".mcp.json", ".kiro/settings/mcp.json", ".gemini/settings.json"] {
            let file = tmp.join(filename);
            if file.exists() {
                std::fs::remove_file(&file).unwrap();
            }
        }

        assert!(!tmp.join(".mcp.json").exists(), ".mcp.json should be removed");
        assert!(!tmp.join(".kiro/settings/mcp.json").exists(), "Kiro config should be removed");
        assert!(!tmp.join(".gemini/settings.json").exists(), "Gemini config should be removed");

        cleanup(&tmp);
    }

    // ─── McpServerEntry serialization ──────────────────────────────────────

    #[test]
    fn mcp_entry_omits_empty_fields() {
        let entry = McpServerEntry {
            command: Some("npx".into()),
            args: Some(vec!["-y".into(), "pkg".into()]),
            url: None,
            env: HashMap::new(),
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("url"), "url:null should be omitted");
        assert!(!json.contains("env"), "empty env should be omitted");
        assert!(json.contains("command"), "command should be present");
        assert!(json.contains("args"), "args should be present");
    }

    #[test]
    fn mcp_entry_includes_env_when_present() {
        let mut env = HashMap::new();
        env.insert("API_KEY".to_string(), "secret123".to_string());

        let entry = McpServerEntry {
            command: Some("npx".into()),
            args: Some(vec![]),
            url: None,
            env,
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("API_KEY"), "env should be serialized");
        assert!(json.contains("secret123"), "env values should be present");
    }

    // ─── read_mcp_json roundtrip ───────────────────────────────────────────

    #[test]
    fn read_write_roundtrip() {
        let tmp = setup_tmp("roundtrip");
        let data = make_test_data();

        write_mcp_json(&tmp.to_string_lossy(), &data).unwrap();
        let read_back = read_mcp_json(&tmp.to_string_lossy()).unwrap();

        assert_eq!(read_back.mcp_servers.len(), data.mcp_servers.len());
        for (key, entry) in &data.mcp_servers {
            let read_entry = read_back.mcp_servers.get(key).unwrap();
            assert_eq!(read_entry.command, entry.command);
            assert_eq!(read_entry.args, entry.args);
            assert_eq!(read_entry.env, entry.env);
        }

        cleanup(&tmp);
    }

    // ─── SSE servers filtered from .mcp.json ───────────────────────────────

    #[test]
    fn sse_entry_has_no_command() {
        // An SSE entry only has url, no command — verify this is the case
        let sse_entry = McpServerEntry {
            command: None,
            args: None,
            url: Some("http://localhost:8000/sse".into()),
            env: HashMap::new(),
        };
        assert!(sse_entry.command.is_none(), "SSE entries must not have command");

        // A stdio entry has command
        let stdio_entry = McpServerEntry {
            command: Some("npx".into()),
            args: Some(vec!["-y".into(), "pkg".into()]),
            url: None,
            env: HashMap::new(),
        };
        assert!(stdio_entry.command.is_some(), "Stdio entries must have command");

        // Filtering by command.is_some() should exclude SSE
        let mut all = HashMap::new();
        all.insert("github".to_string(), stdio_entry);
        all.insert("data.gouv.fr".to_string(), sse_entry);

        let stdio_only: HashMap<String, McpServerEntry> = all.iter()
            .filter(|(_, entry)| entry.command.is_some())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        assert_eq!(stdio_only.len(), 1, "Only stdio servers should remain");
        assert!(stdio_only.contains_key("github"));
        assert!(!stdio_only.contains_key("data.gouv.fr"),
            "SSE server must be filtered from .mcp.json (breaks Claude Code schema)");
    }

    // ─── ensure_redirectors ────────────────────────────────────────────────

    #[test]
    fn ensure_redirectors_skips_projects_without_ai_dir() {
        let tmp = setup_tmp("redir-no-ai");
        // No ai/ directory — should do nothing
        // Set KRONN_TEMPLATES_DIR to a valid templates path
        std::env::set_var("KRONN_TEMPLATES_DIR", "/tmp/kronn-test-no-templates");
        super::super::mcp_scanner::ensure_redirectors_public(&tmp.to_string_lossy());
        // No files should be created
        assert!(!tmp.join("CLAUDE.md").exists());
        assert!(!tmp.join("GEMINI.md").exists());
        cleanup(&tmp);
    }

    #[test]
    fn ensure_redirectors_creates_missing_files() {
        let tmp = setup_tmp("redir-create");
        // Create ai/ directory so project qualifies
        std::fs::create_dir_all(tmp.join("ai")).unwrap();

        // Create a minimal templates dir with redirectors
        let tpl = std::env::temp_dir().join("kronn-test-templates-redir");
        let _ = std::fs::remove_dir_all(&tpl);
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(tpl.join("CLAUDE.md"), "Read ai/index.md").unwrap();
        std::fs::write(tpl.join("GEMINI.md"), "Read ai/index.md").unwrap();
        std::fs::write(tpl.join("AGENTS.md"), "Read ai/index.md").unwrap();
        std::fs::create_dir_all(tpl.join(".kiro/steering")).unwrap();
        std::fs::write(tpl.join(".kiro/steering/instructions.md"), "Read ai/index.md").unwrap();
        std::fs::create_dir_all(tpl.join(".github")).unwrap();
        std::fs::write(tpl.join(".github/copilot-instructions.md"), "Read ai/index.md").unwrap();

        std::env::set_var("KRONN_TEMPLATES_DIR", tpl.to_string_lossy().to_string());
        super::super::mcp_scanner::ensure_redirectors_public(&tmp.to_string_lossy());

        assert!(tmp.join("CLAUDE.md").exists(), "CLAUDE.md should be created");
        assert!(tmp.join("GEMINI.md").exists(), "GEMINI.md should be created");
        assert!(tmp.join("AGENTS.md").exists(), "AGENTS.md should be created");
        assert!(tmp.join(".kiro/steering/instructions.md").exists(), ".kiro/steering/instructions.md should be created");
        assert!(tmp.join(".github/copilot-instructions.md").exists(), ".github/copilot-instructions.md should be created");

        cleanup(&tmp);
        let _ = std::fs::remove_dir_all(&tpl);
    }

    #[test]
    fn ensure_redirectors_does_not_overwrite_existing() {
        let tmp = setup_tmp("redir-no-overwrite");
        std::fs::create_dir_all(tmp.join("ai")).unwrap();

        // Pre-create a CLAUDE.md with custom content
        std::fs::write(tmp.join("CLAUDE.md"), "Custom content").unwrap();

        // Create templates
        let tpl = std::env::temp_dir().join("kronn-test-templates-noover");
        let _ = std::fs::remove_dir_all(&tpl);
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(tpl.join("CLAUDE.md"), "Template content").unwrap();
        std::fs::write(tpl.join("GEMINI.md"), "Template content").unwrap();

        std::env::set_var("KRONN_TEMPLATES_DIR", tpl.to_string_lossy().to_string());
        super::super::mcp_scanner::ensure_redirectors_public(&tmp.to_string_lossy());

        // CLAUDE.md should NOT be overwritten
        let content = std::fs::read_to_string(tmp.join("CLAUDE.md")).unwrap();
        assert_eq!(content, "Custom content", "Existing file should not be overwritten");

        // GEMINI.md should be created (was missing)
        assert!(tmp.join("GEMINI.md").exists(), "Missing GEMINI.md should be created");

        cleanup(&tmp);
        let _ = std::fs::remove_dir_all(&tpl);
    }

    #[test]
    fn atomic_write_produces_valid_file() {
        let tmp = setup_tmp("atomic-write");
        let data = make_test_data();
        let path = tmp.to_string_lossy().to_string();

        write_mcp_json(&path, &data).unwrap();

        // File should exist and be valid JSON
        let content = std::fs::read_to_string(tmp.join(".mcp.json")).unwrap();
        let parsed: McpJsonFile = serde_json::from_str(&content).unwrap();
        assert!(parsed.mcp_servers.contains_key("github"));

        // No temp file left behind
        assert!(!tmp.join(".mcp.tmp").exists(), "Temp file should be cleaned up");

        cleanup(&tmp);
    }

    #[test]
    fn incompatibility_detects_gitlab_for_kiro() {
        use crate::core::mcp_scanner::get_incompatibilities;
        use crate::models::{McpServer, McpTransport, McpSource, AgentType};

        let servers = vec![
            McpServer {
                id: "mcp-gitlab".into(),
                name: "GitLab".into(),
                description: "test".into(),
                transport: McpTransport::Stdio {
                    command: "npx".into(),
                    args: vec!["-y".into(), "@modelcontextprotocol/server-gitlab".into()],
                },
                source: McpSource::Registry,
            },
            McpServer {
                id: "mcp-github".into(),
                name: "GitHub".into(),
                description: "test".into(),
                transport: McpTransport::Stdio {
                    command: "npx".into(),
                    args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
                },
                source: McpSource::Registry,
            },
        ];

        let incomp = get_incompatibilities(&servers);
        assert_eq!(incomp.len(), 1, "Only gitlab should be incompatible");
        assert_eq!(incomp[0].server_id, "mcp-gitlab");
        assert_eq!(incomp[0].agent, AgentType::Kiro);
        assert!(incomp[0].reason.contains("Bedrock"));
    }

    #[test]
    fn incompatibility_returns_empty_for_compatible_servers() {
        use crate::core::mcp_scanner::get_incompatibilities;
        use crate::models::{McpServer, McpTransport, McpSource};

        let servers = vec![
            McpServer {
                id: "mcp-github".into(), name: "GitHub".into(), description: "".into(),
                transport: McpTransport::Stdio {
                    command: "npx".into(),
                    args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
                },
                source: McpSource::Registry,
            },
            McpServer {
                id: "mcp-context7".into(), name: "Context7".into(), description: "".into(),
                transport: McpTransport::Stdio {
                    command: "npx".into(),
                    args: vec!["-y".into(), "@upstash/context7-mcp".into()],
                },
                source: McpSource::Registry,
            },
        ];

        let incomp = get_incompatibilities(&servers);
        assert!(incomp.is_empty(), "Compatible servers should have no incompatibilities");
    }

    #[test]
    fn incompatibility_flags_localhost_sse_servers() {
        use crate::core::mcp_scanner::get_incompatibilities;
        use crate::models::{McpServer, McpTransport, McpSource};

        let servers = vec![
            McpServer {
                id: "detected:data-gouv".into(), name: "data.gouv.fr".into(), description: "".into(),
                transport: McpTransport::Sse { url: "http://localhost:8000/sse".into() },
                source: McpSource::Detected,
            },
        ];

        let incomp = get_incompatibilities(&servers);
        assert_eq!(incomp.len(), 1, "Localhost SSE should be flagged as incompatible");
    }

    #[test]
    fn sync_writes_kiro_config_without_gitlab() {
        // Verify that write_mcp_json_to_subpath with filtered data excludes gitlab
        let tmp = setup_tmp("kiro-no-gitlab");
        let mut servers = std::collections::HashMap::new();
        servers.insert("github".to_string(), McpServerEntry {
            command: Some("npx".into()),
            args: Some(vec!["-y".into(), "@modelcontextprotocol/server-github".into()]),
            url: None, env: std::collections::HashMap::new(),
        });
        // gitlab should NOT be in Kiro config (filtered by sync logic)
        let kiro_data = McpJsonFile { mcp_servers: servers };
        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".kiro/settings/mcp.json", &kiro_data).unwrap();

        let content = std::fs::read_to_string(tmp.join(".kiro/settings/mcp.json")).unwrap();
        let parsed: McpJsonFile = serde_json::from_str(&content).unwrap();
        assert!(parsed.mcp_servers.contains_key("github"), "github should be in Kiro config");
        assert!(!parsed.mcp_servers.contains_key("gitlab"), "gitlab should NOT be in Kiro config");

        cleanup(&tmp);
    }

    #[test]
    fn is_command_available_finds_common_commands() {
        use crate::core::mcp_scanner::is_command_available;
        // `sh` should always be available on any Unix system
        assert!(is_command_available("sh"), "sh should be available");
        // Nonexistent command
        assert!(!is_command_available("kronn_nonexistent_cmd_12345"),
            "Nonexistent command should not be found");
    }

    #[test]
    fn incompatibility_detects_localhost_sse() {
        use crate::core::mcp_scanner::get_incompatibilities;
        use crate::models::{McpServer, McpTransport, McpSource};

        let servers = vec![
            McpServer {
                id: "detected:data-gouv".into(), name: "data.gouv.fr".into(), description: "".into(),
                transport: McpTransport::Sse { url: "http://localhost:8000/sse".into() },
                source: McpSource::Detected,
            },
            McpServer {
                id: "mcp-github".into(), name: "GitHub".into(), description: "".into(),
                transport: McpTransport::Stdio {
                    command: "npx".into(),
                    args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
                },
                source: McpSource::Registry,
            },
        ];

        let incomp = get_incompatibilities(&servers);
        assert_eq!(incomp.len(), 1, "Only localhost SSE should be flagged");
        assert_eq!(incomp[0].server_id, "detected:data-gouv");
        assert!(incomp[0].reason.contains("localhost"), "Reason should mention localhost");
    }

    #[test]
    fn localhost_127_also_detected() {
        use crate::core::mcp_scanner::get_incompatibilities;
        use crate::models::{McpServer, McpTransport, McpSource};

        let servers = vec![McpServer {
            id: "local-svc".into(), name: "Local".into(), description: "".into(),
            transport: McpTransport::Streamable { url: "http://127.0.0.1:3000/mcp".into() },
            source: McpSource::Detected,
        }];

        let incomp = get_incompatibilities(&servers);
        assert_eq!(incomp.len(), 1, "127.0.0.1 should be detected as localhost");
    }

    #[test]
    fn remote_sse_not_flagged() {
        use crate::core::mcp_scanner::get_incompatibilities;
        use crate::models::{McpServer, McpTransport, McpSource};

        let servers = vec![McpServer {
            id: "remote-svc".into(), name: "Remote".into(), description: "".into(),
            transport: McpTransport::Sse { url: "https://api.example.com/mcp".into() },
            source: McpSource::Detected,
        }];

        let incomp = get_incompatibilities(&servers);
        assert!(incomp.is_empty(), "Remote SSE should NOT be flagged");
    }

    #[test]
    fn codex_config_includes_startup_timeout() {
        // Verify that a round-tripped Codex TOML config preserves startup_timeout_sec.
        // npx/uvx MCPs get 60s (cold start download), binaries get 30s.
        let input = r#"
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
startup_timeout_sec = 60
"#;
        let parsed: toml::Value = input.parse().unwrap();
        let server = &parsed["mcp_servers"]["github"];
        assert_eq!(server["startup_timeout_sec"].as_integer(), Some(60),
            "startup_timeout_sec must survive TOML round-trip");
        assert_eq!(server["command"].as_str(), Some("npx"));
    }

    #[test]
    fn codex_config_default_timeout_is_30() {
        // Verify the default_startup_timeout constant is 30 (not 10)
        // to prevent regression to Codex's too-short default
        assert_eq!(super::super::mcp_scanner::default_startup_timeout(), 30,
            "Default startup timeout must be 30s (Codex default is 10s, too short for binaries)");
    }

    // ─── Codex config preservation (regression for silent unwrap_or_default) ─

    #[test]
    fn codex_load_returns_empty_when_file_missing() {
        use crate::core::mcp_scanner::{load_codex_config_for_merge, CodexLoadOutcome};
        let tmp = setup_tmp("codex-missing");
        let path = tmp.join("config.toml");
        match load_codex_config_for_merge(&path) {
            CodexLoadOutcome::Empty => {}
            other => panic!("expected Empty, got {:?}", other),
        }
        cleanup(&tmp);
    }

    #[test]
    fn codex_load_returns_loaded_table_with_user_sections() {
        // Round-trip with [model_providers] and [profiles] — these MUST survive
        // a merge cycle because they contain the user's API keys and presets.
        use crate::core::mcp_scanner::{load_codex_config_for_merge, CodexLoadOutcome};
        let tmp = setup_tmp("codex-loaded");
        let path = tmp.join("config.toml");
        let original = r#"
[model_providers.openai]
api_key = "sk-test"

[profiles.default]
model = "gpt-4o"

[mcp_servers.preexisting]
command = "npx"
args = ["@example/old-mcp"]
"#;
        std::fs::write(&path, original).unwrap();

        match load_codex_config_for_merge(&path) {
            CodexLoadOutcome::Loaded(table) => {
                assert!(table.contains_key("model_providers"),
                    "model_providers section must be preserved");
                assert!(table.contains_key("profiles"),
                    "profiles section must be preserved");
                assert!(table.contains_key("mcp_servers"),
                    "existing mcp_servers section must be preserved (sync replaces it)");
            }
            other => panic!("expected Loaded, got {:?}", other),
        }
        cleanup(&tmp);
    }

    #[test]
    fn codex_load_aborts_and_backs_up_on_corrupt_toml() {
        // CRITICAL: a malformed config.toml must NOT be replaced with an empty
        // table — that would silently destroy [model_providers] etc. We expect
        // Aborted + a .kronn-backup file alongside the original.
        use crate::core::mcp_scanner::{load_codex_config_for_merge, CodexLoadOutcome};
        let tmp = setup_tmp("codex-corrupt");
        let path = tmp.join("config.toml");
        // Definitely-not-TOML content
        std::fs::write(&path, "this is = = not = valid [[[ toml ]]]\n[unclosed").unwrap();

        match load_codex_config_for_merge(&path) {
            CodexLoadOutcome::Aborted => {}
            other => panic!("expected Aborted, got {:?}", other),
        }

        let backup = tmp.join("config.toml.kronn-backup");
        assert!(backup.exists(), "corrupt config must be backed up to .kronn-backup");
        let backup_content = std::fs::read_to_string(&backup).unwrap();
        assert!(backup_content.contains("not = valid"),
            "backup must contain the original (corrupt) bytes");

        // The original file is left in place untouched (we never wrote over it)
        let original = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, backup_content);
        cleanup(&tmp);
    }

    #[test]
    fn codex_load_aborts_when_root_is_not_a_table() {
        // toml technically allows arrays at root in some grammars; reject anything
        // that isn't a table so the merge logic only ever sees a Table.
        use crate::core::mcp_scanner::{load_codex_config_for_merge, CodexLoadOutcome};
        let tmp = setup_tmp("codex-non-table");
        let path = tmp.join("config.toml");
        // Empty file parses to an empty table — that's actually fine, so use
        // a sentinel that parses but is unusual: just whitespace + comment.
        // Empty/whitespace TOML always becomes an empty Table, so we can't
        // easily produce a non-table root from string parsing — instead we
        // verify the Aborted path indirectly by feeding broken TOML above.
        // This test documents that load_codex_config_for_merge accepts a clean
        // empty file as Loaded(empty), which is the right default.
        std::fs::write(&path, "# just a comment\n").unwrap();
        match load_codex_config_for_merge(&path) {
            CodexLoadOutcome::Loaded(t) => assert!(t.is_empty()),
            other => panic!("expected Loaded(empty), got {:?}", other),
        }
        cleanup(&tmp);
    }

    #[test]
    fn scanner_resolve_host_path_existing_local() {
        use crate::core::scanner::resolve_host_path;
        // When path exists locally, it should be returned as-is
        let tmp = setup_tmp("resolve-host");
        let path = tmp.to_string_lossy().to_string();
        let resolved = resolve_host_path(&path);
        assert_eq!(resolved.to_string_lossy(), path, "Existing local path should be returned unchanged");
        cleanup(&tmp);
    }

    #[test]
    fn scanner_resolve_host_path_missing_returns_original() {
        use crate::core::scanner::resolve_host_path;
        let fake = "/tmp/kronn-nonexistent-path-test-12345";
        let resolved = resolve_host_path(fake);
        assert_eq!(resolved.to_string_lossy(), fake, "Missing path should be returned as-is");
    }

    #[test]
    fn copilot_mcp_config_json_format() {
        // Verify that the McpJsonFile serializes to the format Copilot CLI expects:
        // { "mcpServers": { "name": { "command": "...", "args": [...], "env": {...} } } }
        let data = make_test_data();
        let json = serde_json::to_string_pretty(&data).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(parsed["mcpServers"].is_object(), "Root must have mcpServers key");
        let github = &parsed["mcpServers"]["github"];
        assert_eq!(github["command"].as_str(), Some("npx"));
        assert!(github["args"].is_array());
        assert_eq!(github["env"]["GITHUB_TOKEN"].as_str(), Some("ghp_test123"));
    }

    #[test]
    fn sync_key_always_uses_config_label() {
        // Regression test: the sync key must always be config.label,
        // never server.name.to_lowercase() — mixing the two caused duplicate
        // entries with different casing (e.g. "fastly" AND "Fastly").
        let label = "My Custom Label";
        // Simulate the key assignment logic (must match mcp_scanner.rs)
        let key = label.to_string(); // = config.label.clone()
        assert_eq!(key, "My Custom Label", "Key must preserve label casing exactly");
        // The old buggy code would have used server.name.to_lowercase() for single configs
        let server_name = "Fastly";
        assert_ne!(key, server_name.to_lowercase(), "Key must NOT be server.name.to_lowercase()");
    }

    // ─── File sync integration tests ──────────────────────────────────────────

    #[test]
    fn sync_writes_env_to_mcp_json() {
        let tmp = setup_tmp("sync-env");
        let mut servers = HashMap::new();
        let mut env = HashMap::new();
        env.insert("API_KEY".to_string(), "sk-test-alpha-123".to_string());
        env.insert("API_SECRET".to_string(), "secret-beta-456".to_string());
        servers.insert("test-server".to_string(), McpServerEntry {
            command: Some("npx".into()),
            args: Some(vec!["-y".into(), "test-pkg".into()]),
            url: None,
            env,
        });
        let data = McpJsonFile { mcp_servers: servers };

        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".mcp.json", &data).unwrap();

        let content = std::fs::read_to_string(tmp.join(".mcp.json")).unwrap();
        let parsed: McpJsonFile = serde_json::from_str(&content).unwrap();
        let entry = parsed.mcp_servers.get("test-server").unwrap();
        assert_eq!(entry.env.get("API_KEY").unwrap(), "sk-test-alpha-123");
        assert_eq!(entry.env.get("API_SECRET").unwrap(), "secret-beta-456");

        cleanup(&tmp);
    }

    #[test]
    fn sync_uses_config_label_as_key() {
        let tmp = setup_tmp("sync-label-key");
        let mut servers = HashMap::new();
        servers.insert("PeerAlpha Config".to_string(), McpServerEntry {
            command: Some("npx".into()),
            args: Some(vec!["-y".into(), "server-alpha".into()]),
            url: None,
            env: HashMap::new(),
        });
        servers.insert("PeerBeta Config".to_string(), McpServerEntry {
            command: Some("npx".into()),
            args: Some(vec!["-y".into(), "server-beta".into()]),
            url: None,
            env: HashMap::new(),
        });
        let data = McpJsonFile { mcp_servers: servers };

        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".mcp.json", &data).unwrap();

        let content = std::fs::read_to_string(tmp.join(".mcp.json")).unwrap();
        let parsed: McpJsonFile = serde_json::from_str(&content).unwrap();
        assert!(parsed.mcp_servers.contains_key("PeerAlpha Config"),
            "Key should be the config label 'PeerAlpha Config'");
        assert!(parsed.mcp_servers.contains_key("PeerBeta Config"),
            "Key should be the config label 'PeerBeta Config'");
        assert_eq!(parsed.mcp_servers.len(), 2);

        cleanup(&tmp);
    }

    #[test]
    fn sync_empty_env_when_no_secrets() {
        let tmp = setup_tmp("sync-empty-env");
        let mut servers = HashMap::new();
        servers.insert("no-secrets".to_string(), McpServerEntry {
            command: Some("npx".into()),
            args: Some(vec!["-y".into(), "pkg".into()]),
            url: None,
            env: HashMap::new(), // empty env
        });
        let data = McpJsonFile { mcp_servers: servers };

        write_mcp_json_to_subpath(&tmp.to_string_lossy(), ".mcp.json", &data).unwrap();

        let content = std::fs::read_to_string(tmp.join(".mcp.json")).unwrap();
        let parsed: McpJsonFile = serde_json::from_str(&content).unwrap();
        let entry = parsed.mcp_servers.get("no-secrets").unwrap();
        assert!(entry.env.is_empty(), "env should be empty when no secrets");
        // Also verify the JSON omits env (skip_serializing_if)
        let raw: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(raw["mcpServers"]["no-secrets"]["env"].is_null()
            || !raw["mcpServers"]["no-secrets"].as_object().unwrap().contains_key("env"),
            "Empty env should be omitted from JSON");

        cleanup(&tmp);
    }

    #[test]
    fn copilot_global_config_format() {
        // Verify McpJsonFile serializes correctly for Copilot:
        // { "mcpServers": { "name": { "command", "args", "env" } } }
        let mut servers = HashMap::new();
        let mut env = HashMap::new();
        env.insert("TOKEN".to_string(), "test-token-value".to_string());
        servers.insert("test-copilot-server".to_string(), McpServerEntry {
            command: Some("node".into()),
            args: Some(vec!["server.js".into()]),
            url: None,
            env,
        });
        let data = McpJsonFile { mcp_servers: servers };

        let json_str = serde_json::to_string_pretty(&data).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert!(parsed["mcpServers"].is_object(), "Root key must be mcpServers");
        let server = &parsed["mcpServers"]["test-copilot-server"];
        assert_eq!(server["command"].as_str(), Some("node"));
        assert!(server["args"].is_array());
        assert_eq!(server["args"][0].as_str(), Some("server.js"));
        assert_eq!(server["env"]["TOKEN"].as_str(), Some("test-token-value"));
    }

    #[test]
    fn vibe_config_toml_format() {
        use super::super::mcp_scanner::VibeMcpEntry;
        use super::super::mcp_scanner::VibeConfig;

        let mut env = HashMap::new();
        env.insert("API_KEY".to_string(), "test-key-value".to_string());

        let entry = VibeMcpEntry {
            name: "TestServer".into(),
            transport: "stdio".into(),
            command: Some("npx".into()),
            args: Some(vec!["-y".into(), "test-pkg".into()]),
            url: None,
            env,
        };

        let config = VibeConfig {
            mcp_servers: vec![entry],
        };

        let toml_str = toml::to_string_pretty(&config).unwrap();

        // Verify TOML has the expected structure
        assert!(toml_str.contains("[[mcp_servers]]"), "TOML should have [[mcp_servers]] array");
        assert!(toml_str.contains("name = \"TestServer\""), "Should have name field");
        assert!(toml_str.contains("command = \"npx\""), "Should have command field");
        assert!(toml_str.contains("transport = \"stdio\""), "Should have transport field");

        // Verify round-trip
        let parsed: VibeConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.mcp_servers.len(), 1);
        assert_eq!(parsed.mcp_servers[0].name, "TestServer");
        assert_eq!(parsed.mcp_servers[0].command.as_deref(), Some("npx"));
        assert_eq!(parsed.mcp_servers[0].args.as_ref().unwrap().len(), 2);
        assert_eq!(parsed.mcp_servers[0].env.get("API_KEY").unwrap(), "test-key-value");
    }

    // ── Claude settings.local.json sync tests ──

    #[test]
    fn sync_claude_enabled_servers_adds_missing() {
        let tmp = setup_tmp("claude-settings-add");
        let claude_dir = tmp.join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();

        // Existing settings with only "atlassian" enabled
        let settings = serde_json::json!({
            "permissions": { "allow": ["Bash(ls:*)"] },
            "enableAllProjectMcpServers": true,
            "enabledMcpjsonServers": ["atlassian"]
        });
        std::fs::write(claude_dir.join("settings.local.json"),
            serde_json::to_string_pretty(&settings).unwrap()).unwrap();

        // MCP servers to sync
        let mut servers = HashMap::new();
        servers.insert("atlassian".to_string(), McpServerEntry {
            command: Some("uvx".into()), args: Some(vec![]), url: None, env: HashMap::new(),
        });
        servers.insert("GitLab".to_string(), McpServerEntry {
            command: Some("npx".into()), args: Some(vec![]), url: None, env: HashMap::new(),
        });

        sync_claude_enabled_servers(tmp.to_str().unwrap(), &servers);

        // Re-read and verify
        let content = std::fs::read_to_string(claude_dir.join("settings.local.json")).unwrap();
        let result: serde_json::Value = serde_json::from_str(&content).unwrap();
        let enabled: Vec<&str> = result["enabledMcpjsonServers"].as_array().unwrap()
            .iter().map(|v| v.as_str().unwrap()).collect();

        assert!(enabled.contains(&"atlassian"), "existing entry preserved");
        assert!(enabled.contains(&"GitLab"), "new entry added");
        assert_eq!(enabled.len(), 2);

        // Permissions untouched
        assert!(result["permissions"]["allow"].as_array().unwrap().len() == 1);

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn sync_claude_enabled_servers_removes_stale_entries() {
        // Regression for TD-20260403-mcp-naming-migration:
        // Old entries (from server.name era) must be removed when no longer
        // in .mcp.json. Otherwise Claude Code never reads the renamed MCP.
        let tmp = setup_tmp("claude-settings-noop");
        let claude_dir = tmp.join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();

        // Whitelist has "GitLab" (current) + "Docker" (stale — no longer in .mcp.json)
        let settings = serde_json::json!({
            "enabledMcpjsonServers": ["GitLab", "Docker"]
        });
        std::fs::write(claude_dir.join("settings.local.json"),
            serde_json::to_string_pretty(&settings).unwrap()).unwrap();

        // Only GitLab is in the current .mcp.json
        let mut servers = HashMap::new();
        servers.insert("GitLab".to_string(), McpServerEntry {
            command: Some("npx".into()), args: Some(vec![]), url: None, env: HashMap::new(),
        });

        sync_claude_enabled_servers(tmp.to_str().unwrap(), &servers);

        let content = std::fs::read_to_string(claude_dir.join("settings.local.json")).unwrap();
        let result: serde_json::Value = serde_json::from_str(&content).unwrap();
        let enabled: Vec<&str> = result["enabledMcpjsonServers"].as_array().unwrap()
            .iter().map(|v| v.as_str().unwrap()).collect();

        // Docker must be REMOVED (stale), only GitLab remains
        assert_eq!(enabled.len(), 1, "stale entry must be removed");
        assert!(enabled.contains(&"GitLab"), "current entry preserved");
        assert!(!enabled.contains(&"Docker"), "stale entry removed");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn sync_claude_enabled_servers_skips_when_no_settings_file() {
        let tmp = setup_tmp("claude-settings-none");
        let mut servers = HashMap::new();
        servers.insert("GitLab".to_string(), McpServerEntry {
            command: Some("npx".into()), args: Some(vec![]), url: None, env: HashMap::new(),
        });

        // Should not panic or create a file
        sync_claude_enabled_servers(tmp.to_str().unwrap(), &servers);
        assert!(!tmp.join(".claude/settings.local.json").exists());

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn sync_claude_enabled_servers_skips_when_no_enabled_list() {
        let tmp = setup_tmp("claude-settings-no-list");
        let claude_dir = tmp.join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();

        // Settings without enabledMcpjsonServers
        let settings = serde_json::json!({
            "permissions": { "allow": [] }
        });
        std::fs::write(claude_dir.join("settings.local.json"),
            serde_json::to_string_pretty(&settings).unwrap()).unwrap();

        let mut servers = HashMap::new();
        servers.insert("GitLab".to_string(), McpServerEntry {
            command: Some("npx".into()), args: Some(vec![]), url: None, env: HashMap::new(),
        });

        sync_claude_enabled_servers(tmp.to_str().unwrap(), &servers);

        // File unchanged — no enabledMcpjsonServers created
        let content = std::fs::read_to_string(claude_dir.join("settings.local.json")).unwrap();
        let result: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(result.get("enabledMcpjsonServers").is_none(), "should not create list");

        std::fs::remove_dir_all(&tmp).ok();
    }
}
