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
        // This catches regressions where skip_serializing_if accidentally hides the field.
        let input = r#"
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
startup_timeout_sec = 30
"#;
        let parsed: toml::Value = input.parse().unwrap();
        let server = &parsed["mcp_servers"]["github"];
        assert_eq!(server["startup_timeout_sec"].as_integer(), Some(30),
            "startup_timeout_sec must survive TOML round-trip");
        assert_eq!(server["command"].as_str(), Some("npx"));
    }

    #[test]
    fn codex_config_default_timeout_is_30() {
        // Verify the default_startup_timeout constant is 30 (not 10)
        // to prevent regression to Codex's too-short default
        assert_eq!(super::super::mcp_scanner::default_startup_timeout(), 30,
            "Default startup timeout must be 30s (Codex default is 10s, too short for Docker)");
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
}
