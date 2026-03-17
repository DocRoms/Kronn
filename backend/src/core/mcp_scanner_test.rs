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
}
