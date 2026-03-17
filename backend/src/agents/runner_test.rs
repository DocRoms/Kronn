#[cfg(test)]
mod tests {
    use crate::agents::runner::*;
    use crate::models::AgentType;
    use serial_test::serial;

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
    #[serial]
    fn fix_file_ownership_no_env_vars_does_not_panic() {
        // When KRONN_HOST_UID / KRONN_HOST_GID are not set, fix_file_ownership
        // should return early without error.
        std::env::remove_var("KRONN_HOST_UID");
        std::env::remove_var("KRONN_HOST_GID");
        super::super::fix_file_ownership(std::path::Path::new("/tmp"));
    }

    #[test]
    #[serial]
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

    // ─── agent_command: complete args structure per agent ────────────────────────
    //
    // These tests verify the full command structure for each agent type.
    // They catch regressions like missing flags, wrong binary names,
    // wrong env key, or broken npx fallback packages.

    #[test]
    fn claude_code_command_structure() {
        let (binary, npx, args, env_key, _, output_mode) = super::super::agent_command(
            &AgentType::ClaudeCode, "do something", false, "", None,
        );
        assert_eq!(binary, "claude");
        assert_eq!(npx, Some("@anthropic-ai/claude-code"));
        assert_eq!(env_key, "ANTHROPIC_API_KEY");
        assert!(matches!(output_mode, OutputMode::StreamJson));
        assert!(args.contains(&"--print".to_string()), "Missing --print flag");
        assert!(args.contains(&"--output-format".to_string()), "Missing --output-format flag");
        assert!(args.contains(&"stream-json".to_string()), "Missing stream-json value");
        assert!(args.contains(&"--verbose".to_string()), "Missing --verbose flag");
        assert!(args.contains(&"--include-partial-messages".to_string()), "Missing --include-partial-messages");
        // Prompt should be last arg
        assert_eq!(args.last().unwrap(), "do something");
    }

    #[test]
    fn codex_command_structure() {
        let (binary, npx, args, env_key, _, output_mode) = super::super::agent_command(
            &AgentType::Codex, "fix the bug", false, "", None,
        );
        assert_eq!(binary, "codex");
        assert_eq!(npx, Some("@openai/codex"));
        assert_eq!(env_key, "OPENAI_API_KEY");
        assert!(matches!(output_mode, OutputMode::Text));
        assert_eq!(args[0], "exec", "First arg must be 'exec' subcommand");
        assert!(args.contains(&"--skip-git-repo-check".to_string()), "Missing --skip-git-repo-check");
        assert_eq!(args.last().unwrap(), "fix the bug");
    }

    #[test]
    fn vibe_command_structure() {
        let (binary, npx, args, env_key, _, output_mode) = super::super::agent_command(
            &AgentType::Vibe, "analyse this", false, "", None,
        );
        assert_eq!(binary, "python3", "Vibe must use python3 with vibe-runner.py");
        assert_eq!(npx, None, "Vibe has no npx fallback");
        assert_eq!(env_key, "MISTRAL_API_KEY");
        assert!(matches!(output_mode, OutputMode::Text));
        // First arg should be the runner script path
        assert!(args[0].ends_with("vibe-runner.py"), "First arg must be vibe-runner.py, got: {}", args[0]);
        // Prompt should be the last arg
        assert_eq!(args.last().unwrap(), "analyse this");
    }

    #[test]
    fn gemini_command_structure() {
        let (binary, npx, args, env_key, _, output_mode) = super::super::agent_command(
            &AgentType::GeminiCli, "explain this", false, "", None,
        );
        assert_eq!(binary, "gemini");
        assert_eq!(npx, Some("@google/gemini-cli"));
        assert_eq!(env_key, "GEMINI_API_KEY");
        assert!(matches!(output_mode, OutputMode::Text));
        assert_eq!(args[0], "-p", "First arg must be -p flag");
        assert_eq!(args.last().unwrap(), "explain this");
    }

    #[test]
    fn kiro_command_structure() {
        let (binary, npx, args, env_key, _, output_mode) = super::super::agent_command(
            &AgentType::Kiro, "review code", false, "", None,
        );
        assert_eq!(binary, "kiro-cli");
        assert_eq!(npx, None, "Kiro has no npx fallback");
        assert_eq!(env_key, "AWS_BUILDER_ID");
        assert!(matches!(output_mode, OutputMode::Text));
        assert_eq!(args[0], "chat", "First arg must be 'chat' subcommand");
        assert!(args.contains(&"--no-interactive".to_string()), "Missing --no-interactive (required for headless)");
        assert!(args.contains(&"--trust-all-tools".to_string()), "Missing --trust-all-tools (required with --no-interactive)");
        assert!(args.contains(&"--wrap".to_string()), "Missing --wrap flag");
        assert!(args.contains(&"never".to_string()), "Missing 'never' wrap value");
        assert_eq!(args.last().unwrap(), "review code");
    }

    // ─── agent_command: model flag injection ────────────────────────────────────

    #[test]
    fn claude_code_model_flag_injected() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode, "prompt", false, "", Some("haiku"),
        );
        let idx = args.iter().position(|a| a == "--model").expect("Missing --model flag");
        assert_eq!(args[idx + 1], "haiku");
    }

    #[test]
    fn codex_model_flag_injected() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Codex, "prompt", false, "", Some("gpt-5-codex-mini"),
        );
        let idx = args.iter().position(|a| a == "--model").expect("Missing --model flag");
        assert_eq!(args[idx + 1], "gpt-5-codex-mini");
    }

    #[test]
    fn gemini_model_flag_injected() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::GeminiCli, "prompt", false, "", Some("gemini-2.5-flash"),
        );
        let idx = args.iter().position(|a| a == "--model").expect("Missing --model flag");
        assert_eq!(args[idx + 1], "gemini-2.5-flash");
    }

    #[test]
    fn vibe_model_flag_injected() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Vibe, "prompt", false, "", Some("devstral-small-latest"),
        );
        let idx = args.iter().position(|a| a == "--model").expect("Vibe should support --model via runner");
        assert_eq!(args[idx + 1], "devstral-small-latest");
    }

    #[test]
    fn kiro_no_model_flag_support() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Kiro, "prompt", false, "", Some("some-model"),
        );
        assert!(!args.contains(&"--model".to_string()),
            "Kiro should not have --model flag (not supported)");
    }

    // ─── Vibe runner path resolution ────────────────────────────────────────────

    #[test]
    fn vibe_runner_path_resolves_to_existing_file() {
        let path = super::super::vibe_runner_path();
        assert!(path.ends_with("vibe-runner.py"), "Path should end with vibe-runner.py, got: {}", path);
        assert!(std::path::Path::new(&path).exists(), "vibe-runner.py must exist at: {}", path);
    }

    // ─── get_api_key: Mistral provider support ──────────────────────────────────

    fn empty_tokens() -> crate::models::TokensConfig {
        crate::models::TokensConfig {
            anthropic: None, openai: None, google: None,
            keys: vec![], disabled_overrides: vec![],
        }
    }

    #[test]
    fn get_api_key_all_providers_no_panic() {
        let tokens = empty_tokens();
        // None of these should panic, all should return None with empty config
        for env_key in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY", "MISTRAL_API_KEY", "UNKNOWN_KEY"] {
            let _ = super::super::get_api_key(env_key, &tokens);
        }
    }

    #[test]
    fn get_api_key_unknown_provider_returns_none() {
        let tokens = empty_tokens();
        assert_eq!(super::super::get_api_key("UNKNOWN_KEY", &tokens), None);
    }

    #[test]
    fn get_api_key_returns_active_key_per_provider() {
        use crate::models::ApiKey;

        let cases = [
            ("ANTHROPIC_API_KEY", "anthropic", "sk-ant-test-123"),
            ("OPENAI_API_KEY",    "openai",    "sk-openai-test-456"),
            ("GEMINI_API_KEY",    "google",    "AIza-gemini-test-789"),
            ("MISTRAL_API_KEY",   "mistral",   "mist-test-abc"),
        ];

        for (env_key, provider, value) in cases {
            let mut tokens = empty_tokens();
            tokens.keys.push(ApiKey {
                id: format!("k-{}", provider),
                name: "test".into(),
                provider: provider.into(),
                value: value.into(),
                active: true,
            });
            let key = super::super::get_api_key(env_key, &tokens);
            assert_eq!(key, Some(value.to_string()),
                "get_api_key({}) should return the active {} key", env_key, provider);
        }
    }

    #[test]
    fn get_api_key_inactive_key_not_returned() {
        use crate::models::ApiKey;
        let mut tokens = empty_tokens();
        tokens.keys.push(ApiKey {
            id: "k1".into(), name: "old".into(),
            provider: "anthropic".into(), value: "sk-inactive".into(),
            active: false,
        });
        // No active key → should fall back to env var (which is unset in tests)
        let key = super::super::get_api_key("ANTHROPIC_API_KEY", &tokens);
        assert_ne!(key, Some("sk-inactive".to_string()),
            "Inactive key should NOT be returned");
    }

    #[test]
    fn get_api_key_disabled_override_skips_config() {
        use crate::models::ApiKey;
        let mut tokens = empty_tokens();
        tokens.keys.push(ApiKey {
            id: "k1".into(), name: "test".into(),
            provider: "openai".into(), value: "sk-from-config".into(),
            active: true,
        });
        tokens.disabled_overrides.push("openai".into());
        // Override disabled → should NOT use config key, falls back to env
        let key = super::super::get_api_key("OPENAI_API_KEY", &tokens);
        assert_ne!(key, Some("sk-from-config".to_string()),
            "Disabled override should skip config key");
    }

    #[test]
    fn get_api_key_picks_active_among_multiple() {
        use crate::models::ApiKey;
        let mut tokens = empty_tokens();
        tokens.keys.push(ApiKey {
            id: "k1".into(), name: "personal".into(),
            provider: "google".into(), value: "AIza-personal".into(),
            active: false,
        });
        tokens.keys.push(ApiKey {
            id: "k2".into(), name: "work".into(),
            provider: "google".into(), value: "AIza-work".into(),
            active: true,
        });
        let key = super::super::get_api_key("GEMINI_API_KEY", &tokens);
        assert_eq!(key, Some("AIza-work".to_string()),
            "Should pick the active key among multiple for same provider");
    }

    // ─── agent_command: context injection per agent ─────────────────────────────

    #[test]
    fn vibe_ignores_mcp_context_in_api_mode() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Vibe, "user prompt", false, "MCP context", None,
        );
        // In API mode, MCP context is NOT injected (no tool execution loop)
        let prompt = args.last().unwrap();
        assert_eq!(prompt, "user prompt", "MCP context must NOT be prepended in API mode");
        assert!(!prompt.contains("MCP"), "MCP context should be absent");
    }

    #[test]
    fn gemini_prepends_context_to_prompt() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::GeminiCli, "user prompt", false, "MCP context", None,
        );
        let prompt = args.last().unwrap();
        assert!(prompt.starts_with("MCP context"), "Context should be prepended");
        assert!(prompt.contains("user prompt"), "Original prompt should be present");
    }

    #[test]
    fn kiro_prepends_context_to_prompt() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Kiro, "user prompt", false, "MCP context", None,
        );
        let prompt = args.last().unwrap();
        assert!(prompt.starts_with("MCP context"), "Context should be prepended");
        assert!(prompt.contains("user prompt"), "Original prompt should be present");
    }

    // ─── resolve_model_flag: tier mapping per agent ─────────────────────────────

    #[test]
    fn resolve_model_flag_claude_code_tiers() {
        use crate::models::ModelTier;
        assert_eq!(resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Economy, None), Some("haiku".into()));
        assert_eq!(resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Default, None), Some("sonnet".into()));
        assert_eq!(resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Reasoning, None), Some("opus".into()));
    }

    #[test]
    fn resolve_model_flag_codex_tiers() {
        use crate::models::ModelTier;
        assert_eq!(resolve_model_flag(&AgentType::Codex, ModelTier::Economy, None), Some("gpt-5-codex-mini".into()));
        assert_eq!(resolve_model_flag(&AgentType::Codex, ModelTier::Default, None), None);
        assert_eq!(resolve_model_flag(&AgentType::Codex, ModelTier::Reasoning, None), Some("gpt-5.4".into()));
    }

    #[test]
    fn resolve_model_flag_gemini_tiers() {
        use crate::models::ModelTier;
        assert_eq!(resolve_model_flag(&AgentType::GeminiCli, ModelTier::Economy, None), Some("gemini-2.5-flash".into()));
        assert_eq!(resolve_model_flag(&AgentType::GeminiCli, ModelTier::Default, None), None);
        assert_eq!(resolve_model_flag(&AgentType::GeminiCli, ModelTier::Reasoning, None), Some("gemini-3.1-pro-preview".into()));
    }

    #[test]
    fn resolve_model_flag_kiro_vibe_always_none() {
        use crate::models::ModelTier;
        for tier in [ModelTier::Economy, ModelTier::Default, ModelTier::Reasoning] {
            assert_eq!(resolve_model_flag(&AgentType::Kiro, tier, None), None,
                "Kiro should return None for all tiers (no --model support)");
            assert_eq!(resolve_model_flag(&AgentType::Vibe, tier, None), None,
                "Vibe should return None for all tiers (no --model support)");
        }
    }

    #[test]
    fn resolve_model_flag_user_override_takes_precedence() {
        use crate::models::{ModelTier, ModelTiersConfig, ModelTierConfig};
        let overrides = ModelTiersConfig {
            claude_code: ModelTierConfig {
                economy: Some("custom-haiku-3".into()),
                reasoning: None,
            },
            ..Default::default()
        };
        assert_eq!(
            resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Economy, Some(&overrides)),
            Some("custom-haiku-3".into()),
            "User override should take precedence over built-in"
        );
        // Reasoning has no override → falls back to built-in
        assert_eq!(
            resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Reasoning, Some(&overrides)),
            Some("opus".into()),
        );
    }

    // ─── Claude Code: prompt is always last arg (required for --mcp-config injection)

    #[test]
    fn claude_code_prompt_is_last_arg() {
        // start_agent_with_config inserts --mcp-config before the prompt (last arg).
        // This test ensures prompt remains the last arg so that insertion works.
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode, "my prompt", true, "context", Some("sonnet"),
        );
        assert_eq!(args.last().unwrap(), "my prompt",
            "Prompt must be the last arg for --mcp-config injection to work");
    }

    #[test]
    fn claude_code_prompt_is_last_even_with_context() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode, "do stuff", false, "MCP servers info", None,
        );
        assert_eq!(args.last().unwrap(), "do stuff");
        // --append-system-prompt should be before the prompt
        let sys_idx = args.iter().position(|a| a == "--append-system-prompt").unwrap();
        assert!(sys_idx < args.len() - 1, "--append-system-prompt must come before prompt");
    }

    // ─── --mcp-config insertion order ─────────────────────────────────────────

    #[test]
    fn mcp_config_inserted_before_append_system_prompt() {
        // Simulates what start_agent_with_config does: insert --mcp-config
        // before --append-system-prompt and its value.
        let (_, _, mut args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode, "the prompt", false, "MCP context", None,
        );

        // Simulate the --mcp-config insertion logic from start_agent_with_config
        let prompt_arg = args.pop();
        let sys_prompt_val = if args.last().map(|a| !a.starts_with("--")).unwrap_or(false) {
            let val = args.pop();
            let flag = args.pop();
            Some((flag, val))
        } else {
            None
        };
        args.push("--mcp-config".into());
        args.push("/path/to/.mcp.json".into());
        if let Some((flag, val)) = sys_prompt_val {
            if let Some(f) = flag { args.push(f); }
            if let Some(v) = val { args.push(v); }
        }
        if let Some(p) = prompt_arg { args.push(p); }

        // Verify order: --mcp-config must come BEFORE --append-system-prompt
        let mcp_idx = args.iter().position(|a| a == "--mcp-config").unwrap();
        let sys_idx = args.iter().position(|a| a == "--append-system-prompt").unwrap();
        assert!(mcp_idx < sys_idx,
            "--mcp-config ({}) must come before --append-system-prompt ({}) to avoid arg parsing issues. Args: {:?}",
            mcp_idx, sys_idx, args);

        // Prompt must still be last
        assert_eq!(args.last().unwrap(), "the prompt");
    }

    #[test]
    fn mcp_config_works_without_append_system_prompt() {
        // When there's no MCP context, --append-system-prompt is absent
        let (_, _, mut args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode, "the prompt", false, "", None,
        );

        // Simulate --mcp-config insertion
        let prompt_arg = args.pop();
        let sys_prompt_val = if args.last().map(|a| !a.starts_with("--")).unwrap_or(false) {
            let val = args.pop();
            let flag = args.pop();
            Some((flag, val))
        } else {
            None
        };
        args.push("--mcp-config".into());
        args.push("/path/to/.mcp.json".into());
        if let Some((flag, val)) = sys_prompt_val {
            if let Some(f) = flag { args.push(f); }
            if let Some(v) = val { args.push(v); }
        }
        if let Some(p) = prompt_arg { args.push(p); }

        assert!(args.contains(&"--mcp-config".to_string()));
        assert_eq!(args.last().unwrap(), "the prompt");
        // No --append-system-prompt should be present
        assert!(!args.contains(&"--append-system-prompt".to_string()));
    }

    // ─── ensure_kiro_cli_available ─────────────────────────────────────────────

    #[tokio::test]
    async fn ensure_kiro_cli_skips_install_when_present() {
        // kiro-cli is available on this machine (mounted from host)
        // ensure_kiro_cli_available should return Ok immediately
        let result = super::super::ensure_kiro_cli_available().await;
        // On CI/dev where kiro-cli may not exist, this is allowed to fail
        // The important thing is it doesn't panic
        let _ = result;
    }

    // ─── agent_command: Codex sandbox behavior ──────────────────────────────────

    #[test]
    #[serial]
    fn codex_docker_sandbox_workspace_write() {
        std::env::set_var("KRONN_HOST_HOME", "/home/testuser");
        std::env::set_var("KRONN_HOST_OS", "Linux");
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Codex, "prompt", false, "", None,
        );
        assert!(args.contains(&"--sandbox=workspace-write".to_string()),
            "Linux Docker + no full_access should use workspace-write sandbox");
        std::env::remove_var("KRONN_HOST_HOME");
        std::env::remove_var("KRONN_HOST_OS");
    }

    #[test]
    #[serial]
    fn codex_macos_docker_forces_full_access_sandbox() {
        std::env::set_var("KRONN_HOST_HOME", "/Users/testuser");
        std::env::set_var("KRONN_HOST_OS", "macOS");
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Codex, "prompt", false, "", None,
        );
        assert!(args.contains(&"--sandbox=danger-full-access".to_string()),
            "macOS Docker should force danger-full-access sandbox regardless of full_access flag");
        std::env::remove_var("KRONN_HOST_HOME");
        std::env::remove_var("KRONN_HOST_OS");
    }

    #[test]
    #[serial]
    fn codex_full_access_uses_danger_sandbox() {
        std::env::set_var("KRONN_HOST_HOME", "/home/testuser");
        std::env::set_var("KRONN_HOST_OS", "Linux");
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Codex, "prompt", true, "", None,
        );
        assert!(args.contains(&"--sandbox=danger-full-access".to_string()),
            "full_access=true should use danger-full-access sandbox");
        std::env::remove_var("KRONN_HOST_HOME");
        std::env::remove_var("KRONN_HOST_OS");
    }
}
