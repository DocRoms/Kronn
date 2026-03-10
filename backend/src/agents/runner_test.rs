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
}
