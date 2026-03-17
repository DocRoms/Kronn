#[cfg(test)]
mod tests {
    use crate::agents::runner::*;
    use crate::models::AgentType;

    // ─── parse_claude_stream_line ─────────────────────────────────────────────

    #[test]
    fn parse_stream_empty_line() {
        assert!(matches!(parse_claude_stream_line(""), StreamJsonEvent::Skip));
        assert!(matches!(parse_claude_stream_line("  "), StreamJsonEvent::Skip));
    }

    #[test]
    fn parse_stream_text_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}}"#;
        match parse_claude_stream_line(line) {
            StreamJsonEvent::Text(t) => assert_eq!(t, "Hello"),
            _ => panic!("Expected Text event"),
        }
    }

    #[test]
    fn parse_stream_usage_from_message_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"message_delta","usage":{"input_tokens":100,"output_tokens":50}}}"#;
        match parse_claude_stream_line(line) {
            StreamJsonEvent::Usage { input_tokens, output_tokens } => {
                assert_eq!(input_tokens, 100);
                assert_eq!(output_tokens, 50);
            }
            _ => panic!("Expected Usage event"),
        }
    }

    #[test]
    fn parse_stream_result_with_usage() {
        let line = r#"{"type":"result","subtype":"success","cost_usd":0.01,"usage":{"input_tokens":200,"output_tokens":100}}"#;
        match parse_claude_stream_line(line) {
            StreamJsonEvent::Usage { input_tokens, output_tokens } => {
                assert_eq!(input_tokens, 200);
                assert_eq!(output_tokens, 100);
            }
            _ => panic!("Expected Usage event from result"),
        }
    }

    #[test]
    fn parse_stream_result_without_usage() {
        let line = r#"{"type":"result","subtype":"success"}"#;
        assert!(matches!(parse_claude_stream_line(line), StreamJsonEvent::Skip));
    }

    #[test]
    fn parse_stream_assistant_skipped() {
        let line = r#"{"type":"assistant","message":"full text so far"}"#;
        assert!(matches!(parse_claude_stream_line(line), StreamJsonEvent::Skip));
    }

    #[test]
    fn parse_stream_not_json() {
        // Non-JSON lines should be passed through as text
        match parse_claude_stream_line("This is plain text output") {
            StreamJsonEvent::Text(t) => assert_eq!(t, "This is plain text output"),
            _ => panic!("Expected Text passthrough"),
        }
    }

    #[test]
    fn parse_stream_unknown_type() {
        let line = r#"{"type":"init","session_id":"abc"}"#;
        assert!(matches!(parse_claude_stream_line(line), StreamJsonEvent::Skip));
    }

    #[test]
    fn parse_stream_event_without_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"message_start"}}"#;
        assert!(matches!(parse_claude_stream_line(line), StreamJsonEvent::Skip));
    }

    #[test]
    fn parse_stream_zero_usage_skipped() {
        let line = r#"{"type":"stream_event","event":{"type":"message_delta","usage":{"input_tokens":0,"output_tokens":0}}}"#;
        assert!(matches!(parse_claude_stream_line(line), StreamJsonEvent::Skip));
    }

    // ─── parse_token_usage ────────────────────────────────────────────────────

    #[test]
    fn codex_tokens_from_stderr() {
        let stderr = vec!["some info".into(), "tokens used".into(), "1,234".into()];
        let (response, tokens) = parse_token_usage(&AgentType::Codex, "response text", &stderr);
        assert_eq!(tokens, 1234);
        assert_eq!(response, "response text"); // response not modified
    }

    #[test]
    fn codex_tokens_from_stdout_fallback() {
        let response = "Hello world\ntokens used\n5,678";
        let (cleaned, tokens) = parse_token_usage(&AgentType::Codex, response, &[]);
        assert_eq!(tokens, 5678);
        assert_eq!(cleaned, "Hello world"); // token lines stripped
    }

    #[test]
    fn codex_no_tokens() {
        let (response, tokens) = parse_token_usage(&AgentType::Codex, "just a response", &[]);
        assert_eq!(tokens, 0);
        assert_eq!(response, "just a response");
    }

    #[test]
    fn claude_tokens_always_zero_from_this_fn() {
        // Claude Code tokens are parsed inline via parse_claude_stream_line
        let (_, tokens) = parse_token_usage(&AgentType::ClaudeCode, "response", &[]);
        assert_eq!(tokens, 0);
    }

    #[test]
    fn vibe_tokens_always_zero() {
        let (_, tokens) = parse_token_usage(&AgentType::Vibe, "response", &[]);
        assert_eq!(tokens, 0);
    }

    #[test]
    fn gemini_tokens_always_zero() {
        let (_, tokens) = parse_token_usage(&AgentType::GeminiCli, "response", &[]);
        assert_eq!(tokens, 0);
    }

    // ─── fix_file_ownership ──────────────────────────────────────────────────
    // super::super:: because: runner.rs > runner_test (mod) > tests (mod)

    #[test]
    fn fix_file_ownership_no_env_vars_does_not_panic() {
        // When KRONN_HOST_UID / KRONN_HOST_GID are not set, fix_file_ownership
        // should return early without error.
        std::env::remove_var("KRONN_HOST_UID");
        std::env::remove_var("KRONN_HOST_GID");
        super::super::fix_file_ownership(std::path::Path::new("/tmp"));
    }

    #[test]
    fn fix_file_ownership_with_nonexistent_dir_does_not_panic() {
        // Even with UID/GID set, chown on a nonexistent path should not panic.
        std::env::set_var("KRONN_HOST_UID", "1000");
        std::env::set_var("KRONN_HOST_GID", "1000");
        super::super::fix_file_ownership(std::path::Path::new("/nonexistent/path/for/test"));
        // Clean up
        std::env::remove_var("KRONN_HOST_UID");
        std::env::remove_var("KRONN_HOST_GID");
    }

    // ─── agent_command: full_access flags ──────────────────────────────────────

    #[test]
    fn claude_code_full_access_adds_skip_permissions() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode, "test prompt", true, "", None,
        );
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()),
            "Claude Code with full_access should include --dangerously-skip-permissions");
    }

    #[test]
    fn claude_code_no_full_access_omits_skip_permissions() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode, "test prompt", false, "", None,
        );
        assert!(!args.contains(&"--dangerously-skip-permissions".to_string()),
            "Claude Code without full_access should NOT include --dangerously-skip-permissions");
    }

    #[test]
    fn codex_full_access_uses_explicit_sandbox_only() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Codex, "test prompt", true, "", None,
        );
        assert!(!args.contains(&"--full-auto".to_string()),
            "Codex should not include --full-auto (it overrides explicit sandbox)");
    }

    #[test]
    fn codex_no_full_access_omits_full_auto() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Codex, "test prompt", false, "", None,
        );
        assert!(!args.contains(&"--full-auto".to_string()),
            "Codex without full_access should NOT include --full-auto");
    }

    #[test]
    fn gemini_full_access_adds_yolo() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::GeminiCli, "test prompt", true, "", None,
        );
        assert!(args.contains(&"--yolo".to_string()),
            "Gemini CLI with full_access should include --yolo");
    }

    // ─── agent_command: MCP/skills context injection ───────────────────────────

    #[test]
    fn claude_code_injects_context_via_append_system_prompt() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode, "prompt", false, "MCP context here", None,
        );
        let idx = args.iter().position(|a| a == "--append-system-prompt");
        assert!(idx.is_some(), "Should have --append-system-prompt flag");
        assert_eq!(args[idx.unwrap() + 1], "MCP context here");
    }

    #[test]
    fn codex_prepends_context_to_prompt() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Codex, "user prompt", false, "MCP context", None,
        );
        let last = args.last().unwrap();
        assert!(last.starts_with("MCP context"), "Context should be prepended to prompt");
        assert!(last.contains("user prompt"), "Original prompt should be in the combined prompt");
    }

    #[test]
    fn agent_command_no_context_when_empty() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode, "prompt", false, "", None,
        );
        assert!(!args.contains(&"--append-system-prompt".to_string()),
            "Should not add --append-system-prompt when context is empty");
    }

    // ─── Kiro output cleaning ──────────────────────────────────────────────────

    #[test]
    fn kiro_credits_parsing() {
        let stderr = vec!["▸ Credits: 0.05 • Time: 3s".into()];
        let (_, tokens) = parse_token_usage(&AgentType::Kiro, "response", &stderr);
        assert_eq!(tokens, 500); // 0.05 × 10000
    }

    #[test]
    fn kiro_credits_parsing_no_bullet() {
        let stderr = vec!["Credits: 1.23 • Time: 10s".into()];
        let (_, tokens) = parse_token_usage(&AgentType::Kiro, "response", &stderr);
        assert_eq!(tokens, 12300); // 1.23 × 10000
    }

    // ─── clean_kiro_line: structural pattern filtering ───────────────────────

    #[test]
    fn kiro_filters_tool_use_lines() {
        // Lines with "(using tool: X)" should be filtered regardless of language
        assert!(clean_kiro_line("Reading file: /some/path (using tool: read)").is_none());
        assert!(clean_kiro_line("Recherche de symboles (using tool: code)").is_none());
        assert!(clean_kiro_line("Buscando archivos (using tool: grep)").is_none());
        assert!(clean_kiro_line("ファイルを書いています (using tool: write)").is_none());
    }

    #[test]
    fn kiro_filters_mcp_tool_calls() {
        assert!(clean_kiro_line("Running tool jira_get_issue with params (from mcp server: atlassian)").is_none());
        assert!(clean_kiro_line("Appel de l'outil get_repos (from mcp server: github)").is_none());
    }

    #[test]
    fn kiro_filters_unicode_markers() {
        assert!(clean_kiro_line("✓ Successfully read 7951 bytes").is_none());
        assert!(clean_kiro_line("↱ Operation 1: Reading file").is_none());
        assert!(clean_kiro_line("⋮").is_none());
        assert!(clean_kiro_line("❗ No matches found for pattern: X").is_none());
    }

    #[test]
    fn kiro_filters_structured_results() {
        assert!(clean_kiro_line("- Completed in 0.39s").is_none());
        assert!(clean_kiro_line("- Summary: 2 operations processed").is_none());
        assert!(clean_kiro_line("Batch fs_read operation with 2 operations").is_none());
    }

    #[test]
    fn kiro_filters_credits_and_empty() {
        assert!(clean_kiro_line("Credits: 0.05 • Time: 3s").is_none());
        assert!(clean_kiro_line("▸ Credits: 1.23").is_none());
        assert!(clean_kiro_line("").is_none());
        assert!(clean_kiro_line("   ").is_none());
    }

    #[test]
    fn kiro_filters_shell_commands_and_symbol_lookups() {
        // Real examples from Kiro output — "I will run..." contains "(using tool: shell)"
        assert!(clean_kiro_line("I will run the following command: find /some/path -name '*.yaml' (using tool: shell)").is_none());
        assert!(clean_kiro_line("Getting symbols from: /some/file.php [top_level=true] (using tool: code)").is_none());
        // French variant
        assert!(clean_kiro_line("Je vais exécuter la commande suivante: ls -la (using tool: shell)").is_none());
    }

    #[test]
    fn kiro_keeps_real_content() {
        // Actual response text should NOT be filtered
        assert_eq!(clean_kiro_line("Voici l'analyse du problème :"), Some("Voici l'analyse du problème :".into()));
        assert_eq!(clean_kiro_line("## Architecture des redirections"), Some("## Architecture des redirections".into()));
        assert_eq!(clean_kiro_line("Layer 1 — YAML"), Some("Layer 1 — YAML".into()));
        assert_eq!(clean_kiro_line("The fix needed: preserve query params"), Some("The fix needed: preserve query params".into()));
    }

    #[test]
    fn kiro_strips_ansi_and_prefix() {
        // ANSI codes should be stripped
        assert_eq!(clean_kiro_line("\x1b[32mSome text\x1b[0m"), Some("Some text".into()));
        // "> " prefix should be stripped
        assert_eq!(clean_kiro_line("> Response text"), Some("Response text".into()));
    }
}
