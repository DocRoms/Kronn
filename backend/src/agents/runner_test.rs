#[cfg(test)]
mod tests {
    use crate::acp::{
        AcpAgent, AcpCapability, AcpConfigOption, AcpError, AcpInitialize,
        AcpNegotiatedCapabilities, AcpSessionEvent, AcpSessionTarget, AcpTransport,
    };
    use crate::agents::runner::*;
    use crate::models::AgentType;
    use serial_test::serial;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    struct NativeRouteFixture {
        created: std::sync::atomic::AtomicUsize,
        resumed: std::sync::atomic::AtomicUsize,
        prompts: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl AcpTransport for NativeRouteFixture {
        async fn initialize(
            &self,
            _: AcpInitialize,
        ) -> Result<AcpNegotiatedCapabilities, AcpError> {
            Ok(AcpNegotiatedCapabilities {
                protocol_version: 1,
                capabilities: BTreeSet::from([
                    AcpCapability::Sessions,
                    AcpCapability::Resume,
                    AcpCapability::Streaming,
                    AcpCapability::Cancellation,
                    AcpCapability::McpInjection,
                ]),
            })
        }

        async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
            self.created
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            AcpSessionTarget::new(AcpAgent::OpenCode, "native-route-session")
        }

        async fn config_options(&self) -> Vec<AcpConfigOption> {
            Vec::new()
        }

        async fn set_config_option(
            &self,
            _: &AcpSessionTarget,
            _: &str,
            _: &str,
        ) -> Result<(), AcpError> {
            Ok(())
        }

        async fn resume_session(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
            self.resumed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        async fn prompt(
            &self,
            _: &AcpSessionTarget,
            prompt: &str,
            events: tokio::sync::mpsc::Sender<AcpSessionEvent>,
        ) -> Result<(), AcpError> {
            self.prompts.lock().unwrap().push(prompt.to_owned());
            events
                .send(AcpSessionEvent::TextDelta("reply".into()))
                .await
                .unwrap();
            events.send(AcpSessionEvent::Completed).await.unwrap();
            Ok(())
        }

        async fn cancel(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
            Ok(())
        }

        async fn shutdown(&self) -> Result<(), AcpError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn start_agent_with_config_native_route_uses_only_the_explicit_resume_delta() {
        let fixture = Arc::new(NativeRouteFixture {
            created: std::sync::atomic::AtomicUsize::new(0),
            resumed: std::sync::atomic::AtomicUsize::new(0),
            prompts: Mutex::new(Vec::new()),
        });
        let project = tempfile::tempdir().unwrap();
        let tokens = crate::models::setup::TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: Vec::new(),
            disabled_overrides: Vec::new(),
        };
        let agent = AgentType::OpenCode;
        let mut first = start_agent_with_config(AgentStartConfig {
            test_acp_transport: Some(fixture.clone()),
            ..AgentStartConfig::new(
                &agent,
                project.path().to_str().unwrap(),
                "full history",
                &tokens,
            )
        })
        .await
        .unwrap();
        while first.next_line().await.is_some() {}
        assert!(first.child.wait().await.unwrap().success());

        let mut second = start_agent_with_config(AgentStartConfig {
            cli_resume_id: Some("native-route-session"),
            native_acp_full_prompt: Some("full history"),
            test_acp_transport: Some(fixture.clone()),
            ..AgentStartConfig::new(
                &agent,
                project.path().to_str().unwrap(),
                "new peer input",
                &tokens,
            )
        })
        .await
        .unwrap();
        while second.next_line().await.is_some() {}
        assert!(second.child.wait().await.unwrap().success());
        assert_eq!(fixture.created.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(fixture.resumed.load(std::sync::atomic::Ordering::SeqCst), 1);
        let prompts = fixture.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 2);
        // Every real turn carries the system context ahead of the caller's
        // prompt (memory prelude, tool notices, …) — this fixture does not
        // try to reproduce that prefix, only that each turn's own payload is
        // exactly what the dispatcher decided to send, nothing more.
        assert!(
            prompts[0].ends_with("full history"),
            "first turn (no store yet) must carry the full transcript, got: {}",
            prompts[0]
        );
        assert!(
            prompts[1].ends_with("new peer input"),
            "the resumed turn must carry only the unseen delta, got: {}",
            prompts[1]
        );
        assert!(
            !prompts[1].contains("full history"),
            "the production NativeAcp route must not resume when the dispatcher did not supply an id, and \
             must never resend the transcript it already resumed: got {}",
            prompts[1]
        );
    }

    #[test]
    fn acp_mcp_registry_uses_only_command_entries_without_environment_values() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{
                "mcpServers": {
                    "safe": {"command": "safe-server", "args": ["--project"]},
                    "credentialed": {"command": "private-server", "env": {"API_KEY": "secret"}},
                    "remote": {"url": "https://example.invalid/mcp"}
                }
            }"#,
        )
        .unwrap();

        let servers = acp_project_mcp_servers(project.path().to_str().unwrap());
        // Kronn's own bridge rides along and is asserted on its own below;
        // what this test guards is which PROJECT servers survive the filter.
        let project_servers: Vec<_> = servers
            .iter()
            .filter(|server| server.id != "kronn-internal")
            .collect();

        assert_eq!(project_servers.len(), 1);
        assert_eq!(project_servers[0].id, "safe");
        assert_eq!(project_servers[0].command, "safe-server");
        assert_eq!(project_servers[0].args, vec!["--project"]);
        assert!(project_servers[0].allowed_tools.is_empty());
    }

    #[test]
    fn acp_mcp_registry_drops_a_server_whose_args_embed_a_credential_directly() {
        // KT-542 review: the no-secret promise must cover `args`, not only
        // `env`. A server with an empty `env` map but a secret passed as a
        // CLI flag value must never reach the ACP payload either.
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{
                "mcpServers": {
                    "safe": {"command": "safe-server", "args": ["--project"]},
                    "leaky": {"command": "leaky-server", "args": ["--token", "sk-super-secret-do-not-leak"]}
                }
            }"#,
        )
        .unwrap();

        let servers = acp_project_mcp_servers(project.path().to_str().unwrap());
        let project_servers: Vec<_> = servers
            .iter()
            .filter(|server| server.id != "kronn-internal")
            .collect();

        assert_eq!(
            project_servers.len(),
            1,
            "the leaky server must be dropped wholesale"
        );
        assert_eq!(project_servers[0].id, "safe");
        assert!(servers.iter().all(|server| !server
            .args
            .iter()
            .any(|arg| arg.contains("sk-super-secret"))));
    }

    /// KT-543 — an ACP agent that cannot reach the bridge is mute in the room.
    ///
    /// Claude receives `kronn-internal` through `--mcp-config` and Codex
    /// through its TOML override. The native ACP route had no equivalent, so
    /// OpenCode joined discussions it could not answer in. The registry now
    /// carries it, and carries it even with no project attached — the bridge
    /// is about the room, not about a checkout.
    #[test]
    #[serial]
    fn acp_mcp_registry_carries_the_kronn_bridge_with_no_credential_in_the_payload() {
        let script = tempfile::NamedTempFile::new().unwrap();
        let previous = std::env::var("KRONN_DISC_INTROSPECTION_MCP").ok();
        std::env::set_var("KRONN_DISC_INTROSPECTION_MCP", script.path());

        let with_no_project = acp_project_mcp_servers("");
        let bridge = with_no_project
            .iter()
            .find(|server| server.id == "kronn-internal")
            .expect("the bridge does not depend on a project being attached");
        assert_eq!(bridge.command, "python3");
        assert_eq!(
            bridge.args,
            vec![script.path().to_string_lossy().to_string()]
        );

        // The whole point of passing it this way: the protocol carries the
        // command and nothing else. The bridge reads its token from the
        // environment it inherits from the process Kronn spawned, so no
        // credential is ever serialized into an ACP payload.
        assert!(bridge.allowed_tools.is_empty());
        assert!(
            !bridge
                .args
                .iter()
                .any(|arg| arg.contains("KRONN_AUTH_TOKEN")),
            "no credential, and no placeholder for one, may travel over ACP"
        );

        match previous {
            Some(value) => std::env::set_var("KRONN_DISC_INTROSPECTION_MCP", value),
            None => std::env::remove_var("KRONN_DISC_INTROSPECTION_MCP"),
        }
    }

    /// Drive the production `forward_chat_line` with Ollama's codec, in the
    /// shape the Ollama tests below were written against. Ollama reports token
    /// counts on the very chunk that ends the stream, so a per-call tally is
    /// equivalent to the stream-scoped one the transport threads through.
    async fn forward_ollama_line(
        line: &str,
        tx: &tokio::sync::mpsc::Sender<String>,
        stderr: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        got_done: &mut bool,
        got_error: &mut bool,
        num_ctx: u64,
    ) -> bool {
        forward_chat_line(
            &crate::agents::chat_codec::OllamaCodec,
            "Ollama",
            line,
            tx,
            stderr,
            got_done,
            got_error,
            &mut None,
            num_ctx,
            &mut TokenTally::default(),
            &mut LeadingThinkingFilter::default(),
            &mut crate::agents::tools::ToolCallAccumulator::default(),
            &mut false,
        )
        .await
    }

    #[test]
    fn rust_syntax_repair_is_one_shot_and_frozen_to_the_refused_target() {
        let failed = crate::agents::tools::ToolCall {
            id: "failed".into(),
            name: "edit_lines".into(),
            arguments: serde_json::json!({
                "path": "src/lib.rs",
                "start_line": 40,
                "end_line": 44,
                "new_string": "fn broken(",
                "expected_sha256": "a".repeat(64),
            }),
        };
        let mut body = serde_json::json!({
            "tools": crate::api::agent_workspace_tools::tool_definitions(),
        });
        let catalogue = body["tools"].as_array().unwrap().clone();
        set_worker_tools_from_catalogue(&mut body, &catalogue, &["edit_lines"]);
        constrain_worker_repair_tool(&mut body, &failed);

        assert_eq!(
            body.pointer("/tools/0/function/name"),
            Some(&serde_json::json!("edit_lines"))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/path/enum/0"),
            Some(&serde_json::json!("src/lib.rs"))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/start_line/enum/0"),
            Some(&serde_json::json!(40))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/end_line/enum/0"),
            Some(&serde_json::json!(44))
        );

        let correction = crate::agents::tools::ToolCall {
            id: "repair".into(),
            name: "edit_lines".into(),
            arguments: serde_json::json!({
                "path": "src/lib.rs",
                "start_line": 40,
                "end_line": 44,
                "new_string": "fn repaired() {}",
                "expected_sha256": "b".repeat(64),
            }),
        };
        assert!(worker_repair_call_matches_target(&failed, &correction));
        let escaped = crate::agents::tools::ToolCall {
            arguments: serde_json::json!({
                "path": "src/other.rs",
                "start_line": 40,
                "end_line": 44,
                "new_string": "fn repaired() {}",
                "expected_sha256": "b".repeat(64),
            }),
            ..correction
        };
        assert!(!worker_repair_call_matches_target(&failed, &escaped));
        assert_eq!(
            worker_repair_iteration_limit(WorkerRepairStage::Edit, true, false),
            1
        );
        assert_eq!(
            worker_repair_iteration_limit(WorkerRepairStage::Edit, false, false),
            WORKER_REPAIR_EDIT_ITERATIONS
        );

        let refusal = crate::agents::tools::ToolOutcome {
            call: failed,
            content: serde_json::json!({
                "error": format!(
                    "{} `src/lib.rs` at line 44, column 1: expected `}}`",
                    crate::api::agent_workspace_tools::RUST_SYNTAX_REFUSAL_PREFIX
                )
            }),
            ok: false,
        };
        assert!(rust_syntax_refusal(&refusal));
    }

    #[test]
    fn prelocalized_worker_has_one_frozen_read_then_one_frozen_cas_edit() {
        let scope = crate::models::TaskWorkerScope::PrelocalizedEdit {
            path: "src/lib.rs".into(),
            start_line: 40,
            end_line: 44,
        };
        let catalogue = crate::api::agent_workspace_tools::tool_definitions();
        let mut body = serde_json::json!({"tools": catalogue.clone()});

        constrain_prelocalized_read_tool(&mut body, &catalogue, &scope);
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            body.pointer("/tools/0/function/name"),
            Some(&serde_json::json!("read_file"))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/path/enum/0"),
            Some(&serde_json::json!("src/lib.rs"))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/offset/enum/0"),
            Some(&serde_json::json!(28))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/limit/enum/0"),
            Some(&serde_json::json!(29))
        );
        let read = crate::agents::tools::ToolCall {
            id: "read".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({
                "path": "src/lib.rs",
                "offset": 28,
                "limit": 29,
            }),
        };
        assert!(prelocalized_call_matches_scope(
            &read,
            WorkerRepairStage::Read,
            &scope,
            None
        ));
        let escaped_read = crate::agents::tools::ToolCall {
            arguments: serde_json::json!({
                "path": "src/lib.rs",
                "offset": 1,
                "limit": 200,
            }),
            ..read
        };
        assert!(!prelocalized_call_matches_scope(
            &escaped_read,
            WorkerRepairStage::Read,
            &scope,
            None
        ));

        let receipt = "a".repeat(64);
        constrain_prelocalized_edit_tool(&mut body, &catalogue, &scope, &receipt);
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            body.pointer("/tools/0/function/name"),
            Some(&serde_json::json!("edit_lines"))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/expected_sha256/enum/0"),
            Some(&serde_json::json!(receipt))
        );
        let edit = crate::agents::tools::ToolCall {
            id: "edit".into(),
            name: "edit_lines".into(),
            arguments: serde_json::json!({
                "path": "src/lib.rs",
                "start_line": 40,
                "end_line": 44,
                "new_string": "replacement",
                "expected_sha256": receipt,
            }),
        };
        assert!(prelocalized_call_matches_scope(
            &edit,
            WorkerRepairStage::Edit,
            &scope,
            edit.arguments["expected_sha256"].as_str(),
        ));
        let broadened = crate::agents::tools::ToolCall {
            arguments: serde_json::json!({
                "path": "src/lib.rs",
                "start_line": 39,
                "end_line": 44,
                "new_string": "replacement",
                "expected_sha256": "a".repeat(64),
            }),
            ..edit
        };
        assert!(!prelocalized_call_matches_scope(
            &broadened,
            WorkerRepairStage::Edit,
            &scope,
            Some(&"a".repeat(64)),
        ));
        assert_eq!(
            worker_repair_iteration_limit(WorkerRepairStage::Read, false, true),
            2
        );
        assert_eq!(
            worker_repair_iteration_limit(WorkerRepairStage::Edit, false, true),
            2
        );
        assert_eq!(
            worker_repair_iteration_limit(WorkerRepairStage::Commit, false, true),
            2
        );
    }

    #[test]
    fn prelocalized_insert_after_exposes_only_text_and_preserves_a_frozen_anchor() {
        let scope = crate::models::TaskWorkerScope::PrelocalizedInsertAfter {
            path: "docs/guide.md".into(),
            anchor_line: 58,
        };
        let catalogue = crate::api::agent_workspace_tools::tool_definitions();
        let mut body = serde_json::json!({"tools": catalogue.clone()});

        constrain_prelocalized_read_tool(&mut body, &catalogue, &scope);
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            body.pointer("/tools/0/function/name"),
            Some(&serde_json::json!("read_file"))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/offset/enum/0"),
            Some(&serde_json::json!(46))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/limit/enum/0"),
            Some(&serde_json::json!(25))
        );

        let receipt = "b".repeat(64);
        constrain_prelocalized_edit_tool(&mut body, &catalogue, &scope, &receipt);
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            body.pointer("/tools/0/function/name"),
            Some(&serde_json::json!("insert_after_line"))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/path/enum/0"),
            Some(&serde_json::json!("docs/guide.md"))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/anchor_line/enum/0"),
            Some(&serde_json::json!(58))
        );
        assert_eq!(
            body.pointer("/tools/0/function/parameters/properties/expected_sha256/enum/0"),
            Some(&serde_json::json!(receipt))
        );
        assert!(
            body.pointer("/tools/0/function/parameters/properties/start_line")
                .is_none(),
            "an insertion tool must expose no replacement range"
        );
        assert!(
            body.pointer("/tools/0/function/parameters/properties/end_line")
                .is_none(),
            "an insertion tool must expose no replacement range"
        );

        let insert = crate::agents::tools::ToolCall {
            id: "insert".into(),
            name: "insert_after_line".into(),
            arguments: serde_json::json!({
                "path": "docs/guide.md",
                "anchor_line": 58,
                "new_string": "new paragraph",
                "expected_sha256": receipt,
            }),
        };
        assert!(prelocalized_call_matches_scope(
            &insert,
            WorkerRepairStage::Edit,
            &scope,
            insert.arguments["expected_sha256"].as_str(),
        ));
        let moved_anchor = crate::agents::tools::ToolCall {
            arguments: serde_json::json!({
                "path": "docs/guide.md",
                "anchor_line": 57,
                "new_string": "new paragraph",
                "expected_sha256": "b".repeat(64),
            }),
            ..insert
        };
        assert!(!prelocalized_call_matches_scope(
            &moved_anchor,
            WorkerRepairStage::Edit,
            &scope,
            Some(&"b".repeat(64)),
        ));
    }

    // ─── parse_claude_stream_line ─────────────────────────────────────────────

    #[test]
    fn parse_stream_empty_line() {
        assert!(matches!(
            parse_claude_stream_line(""),
            StreamJsonEvent::Skip
        ));
        assert!(matches!(
            parse_claude_stream_line("  "),
            StreamJsonEvent::Skip
        ));
    }

    #[test]
    fn the_init_line_yields_the_conversation_id() {
        // Shape captured from a real `claude -p --output-format stream-json`
        // run: the id is on the FIRST line, before any work, which is why a
        // turn cut short still leaves something resumable.
        let line = r#"{"type":"system","subtype":"init","cwd":"/tmp/x","session_id":"2c19fd03-fde4-4c0d-a893-adae1d816df2","tools":["Bash"]}"#;
        match parse_claude_stream_line(line) {
            StreamJsonEvent::SessionId(id) => {
                assert_eq!(id, "2c19fd03-fde4-4c0d-a893-adae1d816df2")
            }
            other => panic!("expected SessionId, got {other:?}"),
        }
    }

    #[test]
    fn a_system_line_without_an_id_is_skipped() {
        let line = r#"{"type":"system","subtype":"something_else"}"#;
        assert!(matches!(
            parse_claude_stream_line(line),
            StreamJsonEvent::Skip
        ));
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
    fn parse_failed_result_retains_real_fable_quota_fields() {
        // Shape captured from Claude/Fable on 2026-08-25: the CLI exits 1,
        // stderr is empty, and the only actionable cause lives in stdout's
        // final stream-json result.
        let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"You've hit your org's monthly spend limit · run /usage-credits to manage your plan.","api_error_status":429,"terminal_reason":"api_error","cost_usd":0,"usage":{"input_tokens":0,"output_tokens":0}}"#;
        match parse_claude_stream_line(line) {
            StreamJsonEvent::TerminalError(failure) => {
                assert!(failure.is_error);
                assert_eq!(
                    failure.text,
                    "You've hit your org's monthly spend limit · run /usage-credits to manage your plan."
                );
                assert_eq!(failure.api_error_status, Some(429));
                assert_eq!(failure.terminal_reason.as_deref(), Some("api_error"));
                assert_eq!(failure.input_tokens, 0);
                assert_eq!(failure.output_tokens, 0);
                let rendered = failure.user_message();
                assert!(rendered.contains("monthly spend limit"));
                assert!(rendered.contains("HTTP 429"));
                assert!(rendered.contains("terminal_reason=api_error"));
            }
            other => panic!("Expected TerminalError event, got {other:?}"),
        }
    }

    #[test]
    fn parse_success_result_still_reports_usage() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","cost_usd":0.01,"usage":{"input_tokens":21,"output_tokens":8}}"#;
        assert!(matches!(
            parse_claude_stream_line(line),
            StreamJsonEvent::Usage {
                input_tokens: 21,
                output_tokens: 8,
                cost_usd: Some(cost),
            } if (cost - 0.01).abs() < f64::EPSILON
        ));
    }

    // ─── strip_thinking_leaks ─────────────────────────────────────────────────

    #[test]
    fn strip_thinking_leaks_removes_closing_tag() {
        // Hot-path case: Claude Opus leaks one closing tag into a text delta.
        assert_eq!(strip_thinking_leaks("</thinking>"), "");
        assert_eq!(strip_thinking_leaks("</thinking>\n"), "\n");
    }

    #[test]
    fn strip_thinking_leaks_removes_runaway_repeats() {
        // EW-7189 reproducer: the partial_response had 6349× `</thinking>\n`.
        // After stripping, the delta collapses to just the newlines.
        let input = "</thinking>\n".repeat(200);
        let out = strip_thinking_leaks(&input);
        assert_eq!(out, "\n".repeat(200));
        assert!(!out.contains("thinking"));
    }

    #[test]
    fn strip_thinking_leaks_is_case_insensitive() {
        // Model quirks: `<Thinking>`, `<THINKING>` seen in the wild.
        assert_eq!(strip_thinking_leaks("<Thinking>x</Thinking>"), "x");
        assert_eq!(strip_thinking_leaks("<THINKING>y</THINKING>"), "y");
    }

    #[test]
    fn strip_thinking_leaks_preserves_legitimate_content() {
        // The word "thinking" in plain text — MUST NOT be stripped.
        assert_eq!(
            strip_thinking_leaks("Thinking about it."),
            "Thinking about it."
        );
        // Unrelated HTML-like content — MUST NOT be stripped.
        assert_eq!(
            strip_thinking_leaks("Use <em>this</em> tag."),
            "Use <em>this</em> tag."
        );
        // A genuine code sample referencing the tag as string — rare, but we
        // err on the side of the stream-pollution fix here: if someone really
        // needs to paste `</thinking>` into a message, they can escape it.
        // Documented trade-off in strip_thinking_leaks's doc comment.
    }

    #[test]
    fn strip_thinking_leaks_catches_qwen3_short_think_tag() {
        // Regression: the regex only matched `<thinking>`, never qwen3's
        // shorter `<think>` / `</think>`. Now both are stripped.
        assert_eq!(
            strip_thinking_leaks("<think>reasoning</think>391"),
            "reasoning391"
        );
        assert_eq!(strip_thinking_leaks("</think>"), "");
        assert_eq!(strip_thinking_leaks("<THINK>x</THINK>"), "x");
        // The longer form still works.
        assert_eq!(strip_thinking_leaks("<thinking>y</thinking>"), "y");
        // Still no false positives on the plain word.
        assert_eq!(strip_thinking_leaks("I think so."), "I think so.");
    }

    #[test]
    fn leading_thinking_filter_hides_split_deepseek_reasoning() {
        let mut filter = LeadingThinkingFilter::default();
        let chunks = [
            "  <th",
            "ink>Je dois retrouver le prompt",
            " et analyser les agents.</thi",
            "nk>\n\nVoici la réponse.",
        ];
        let mut visible = chunks
            .into_iter()
            .map(|chunk| filter.push(chunk))
            .collect::<String>();
        visible.push_str(&filter.finish());

        assert_eq!(visible, "\n\nVoici la réponse.");
    }

    #[test]
    fn leading_thinking_filter_preserves_tags_after_answer_starts() {
        let response = "Pour documenter le format, utilisez `<think>exemple</think>`.";
        assert_eq!(strip_leading_thinking_blocks(response), response);
    }

    #[test]
    fn leading_thinking_filter_drops_unclosed_private_reasoning() {
        assert_eq!(
            strip_leading_thinking_blocks("<THINKING>brouillon interne sans fermeture"),
            ""
        );
    }

    // ─── Ollama /api/chat request body (asserts on the REQUEST, never on the
    //     generated text — greedy-stable ≠ bit-exact on Metal, would be flaky) ─
    #[test]
    fn ollama_body_has_deterministic_options() {
        let body = build_ollama_chat_body("qwen3:8b", "sys", "hi", None, 8192, None);
        let opts = &body["options"];
        assert_eq!(opts["temperature"], 0);
        assert_eq!(opts["top_k"], 1);
        assert_eq!(opts["seed"], 42);
        assert!(
            opts["num_ctx"].as_u64().unwrap() <= 8192,
            "num_ctx must be capped at 8192"
        );
        assert!(
            opts["num_ctx"].as_u64().unwrap() >= 2048,
            "num_ctx must respect the floor"
        );
    }

    /// Reserve output and all loadable tool families before the first turn so a
    /// family load cannot trigger a mid-run context resize.
    #[test]
    fn the_window_does_not_move_when_a_run_loads_a_tool_family() {
        let core =
            crate::api::agent_tools::tiered(crate::api::agent_tools::full_discussion_catalogue());
        let mut before =
            build_ollama_chat_body("qwen3.5:4b", "sys", "génère une image", None, 65_536, None);
        before["tools"] = serde_json::Value::Array(core.clone());
        fit_ollama_num_ctx(&mut before, 65_536);

        // The same run, one `tools_load(media)` later.
        let mut after = before.clone();
        for (family, _, _) in crate::api::agent_tools::TOOL_FAMILIES {
            let loaded = crate::api::agent_tools::declarations_for_family(family);
            assert!(!loaded.is_empty(), "{family} must declare tools");
            after["tools"].as_array_mut().unwrap().extend(loaded);
            fit_ollama_num_ctx(&mut after, 65_536);
            assert_eq!(
                reachable_tools_bytes(&before),
                reachable_tools_bytes(&after),
                "loading {family} must preserve the exact reachable JSON size"
            );
            assert_eq!(
                before["options"]["num_ctx"], after["options"]["num_ctx"],
                "loading {family} must not move the window mid-conversation"
            );
        }
        // And the window it starts with really does cover the whole catalogue,
        // so nothing is dropped once the families arrive.
        let full = serde_json::Value::Array(crate::api::agent_tools::full_discussion_catalogue())
            .to_string()
            .len();
        assert!(
            reachable_tools_bytes(&before) >= full,
            "a run that can load every family must be sized for every family"
        );
    }

    /// A worker's catalogue is frozen: it has no `tools_load`, so nothing can
    /// arrive later and the window is sized for exactly what is on the wire.
    #[test]
    fn a_frozen_catalogue_is_sized_for_itself_and_nothing_more() {
        let mut body = build_ollama_chat_body("qwen3.5:4b", "sys", "hi", None, 65_536, None);
        let frozen = crate::api::agent_tools::declarations_for_family("edit");
        body["tools"] = serde_json::Value::Array(frozen.clone());
        assert_eq!(
            reachable_tools_bytes(&body),
            serde_json::Value::Array(frozen).to_string().len(),
            "without tools_load nothing can be added later"
        );
    }

    /// The project doc is pointed at when the agent can open it, copied in when
    /// it cannot, and copied in regardless when an operator says so.
    #[test]
    fn the_project_doc_is_pointed_at_only_when_the_agent_can_open_it() {
        assert!(http_agent_reads_its_own_files(true, true));
        // No file tools: the inline copy is its only grounding.
        assert!(!http_agent_reads_its_own_files(true, false));
        // A CLI agent reads the doc off the disk itself; this path is not its.
        assert!(!http_agent_reads_its_own_files(false, true));
    }

    /// Infer cache cost from observed memory growth; attention shapes alone do not
    /// account for sliding-window caches.
    #[test]
    fn a_token_of_context_is_priced_from_what_ollama_reported() {
        let observed: std::collections::BTreeMap<u64, u64> = [
            (4_096, 9_473_338_899),
            (16_384, 9_628_161_145),
            (65_536, 10_206_010_408),
        ]
        .into_iter()
        .collect();
        let cost = kv_bytes_per_token_from_samples(&observed).expect("three windows seen");
        assert!(
            (11_000..13_000).contains(&cost),
            "{cost} bytes per token is not the ~12 KB measured on gemma4:e4b"
        );
    }

    /// One window tells you where the model sits, not what a token costs. Until
    /// a second one is seen there is no answer, and the caller keeps its band.
    #[test]
    fn one_observation_is_a_position_and_not_a_slope() {
        let single: std::collections::BTreeMap<u64, u64> =
            [(8_192, 9_611_383_929)].into_iter().collect();
        assert_eq!(kv_bytes_per_token_from_samples(&single), None);
        assert_eq!(
            kv_bytes_per_token_from_samples(&std::collections::BTreeMap::new()),
            None
        );
        // A wider window that somehow reported no more memory says nothing
        // either, rather than a cost of zero.
        let flat: std::collections::BTreeMap<u64, u64> =
            [(4_096, 9_000), (8_192, 9_000)].into_iter().collect();
        assert_eq!(kv_bytes_per_token_from_samples(&flat), None);
    }

    /// Use model weights only with a measured per-token cache cost; otherwise retain
    /// the coarse memory-band estimate.
    #[test]
    fn the_machine_ceiling_is_computed_or_left_exactly_as_it_was() {
        const GB: u64 = 1024 * 1024 * 1024;
        let kv = 42 * 2 * (512 + 512) * 2;
        // The model publishes its attention shape: a heavier model holds less.
        let small = ram_ceiling_for_model(Some(32 * GB), Some(3 * GB), Some(kv));
        let big = ram_ceiling_for_model(Some(32 * GB), Some(18 * GB), Some(kv));
        assert!(
            small > big,
            "a 3 GB model and an 18 GB one cannot share a ceiling: {small} vs {big}"
        );
        // It does not publish it: the previous behaviour, to the token.
        for (total, weight) in [
            (Some(32 * GB), Some(18 * GB)),
            (Some(64 * GB), Some(3 * GB)),
            (Some(64 * GB), None),
            (None, None),
        ] {
            assert_eq!(
                ram_ceiling_for_model(total, weight, None),
                ram_derived_ceiling(total),
                "no cache cost means no new opinion about the ceiling"
            );
        }
    }

    /// When the model says what a token costs, the bound is arithmetic rather
    /// than a band, and it can only tighten one.
    #[test]
    fn an_exact_cache_cost_bounds_the_window_to_what_fits() {
        const GB: u64 = 1024 * 1024 * 1024;
        // 16 GB host, a 9.6 GB model, 168 KB of cache per token: what is left
        // of the 70% share is about 1.6 GB, so roughly 9 500 tokens.
        let kv = 42 * 2 * (512 + 512) * 2;
        let fits = window_the_machine_can_hold(Some(16 * GB), Some(9 * GB + 600_000_000), Some(kv))
            .expect("all three known");
        assert!(
            (8_000..12_000).contains(&fits),
            "{fits} tokens is not what 1.6 GB of cache holds at {kv} bytes per token"
        );
        // A model heavier than the share leaves nothing rather than overflowing.
        assert_eq!(
            window_the_machine_can_hold(Some(16 * GB), Some(15 * GB), Some(kv)),
            None
        );
        assert!(ram_ceiling_for_model(Some(16 * GB), Some(15 * GB), Some(kv)) >= 2_048);
    }

    /// The guard refuses a prompt the window cannot hold, so its arithmetic
    /// decides whether real runs start. Counting the catalogue was the fix;
    /// counting it twice, or pricing it as prose, would refuse runs that fit.
    #[test]
    fn the_guard_prices_a_turn_the_way_the_model_does() {
        let catalogue =
            crate::api::agent_tools::tiered(crate::api::agent_tools::full_discussion_catalogue());
        let tools = serde_json::Value::Array(catalogue).to_string().len();

        // Measured: JSON runs 3.7 bytes per token and prose 4.9. The estimate
        // must sit under both, and not by a factor.
        let tools_only = estimated_prompt_tokens(0, tools) - REPLY_HEADROOM_TOKENS;
        let real = tools as u64 * 10 / 37;
        assert!(
            tools_only >= real && tools_only <= real * 12 / 10,
            "{tools_only} estimated against {real} measured: an estimate this far out \
             refuses runs that would have fit"
        );

        // Prose is cheaper per byte than JSON, so the same bytes as messages
        // must price lower than as catalogue.
        assert!(
            estimated_prompt_tokens(tools, 0) < estimated_prompt_tokens(0, tools),
            "prose and tool schemas do not tokenise alike"
        );

        // An empty turn is the reply headroom and nothing else.
        assert_eq!(estimated_prompt_tokens(0, 0), REPLY_HEADROOM_TOKENS);
    }

    /// The window must hold the reply it asks for. `num_predict` is a share of
    /// `num_ctx`, so a run that sizes one without the other would promise the
    /// model more output than the window can take.
    #[test]
    fn every_window_can_hold_the_reply_it_allows() {
        let catalogue =
            crate::api::agent_tools::tiered(crate::api::agent_tools::full_discussion_catalogue());
        for cap in [2_048_u64, 8_192, 16_384, 24_576, 65_536, 262_144] {
            let mut body = build_ollama_chat_body("qwen3.5:4b", "sys", "hi", None, cap, None);
            body["tools"] = serde_json::Value::Array(catalogue.clone());
            fit_ollama_num_ctx(&mut body, cap);
            let num_ctx = body["options"]["num_ctx"].as_u64().unwrap();
            let num_predict = body["options"]["num_predict"].as_i64().unwrap() as u64;
            assert!(
                num_ctx <= cap.max(OLLAMA_NUM_CTX_FLOOR),
                "{num_ctx} over cap {cap}"
            );
            assert!(
                num_predict < num_ctx,
                "a {num_predict}-token reply cannot fit a {num_ctx}-token window"
            );
        }
    }

    /// What each catalogue mode actually costs, side by side and without a
    /// model: the window is sized for what a run can reach, so tiering saves
    /// prompt tokens per turn and nothing at all on the window.
    #[test]
    fn tiering_saves_tokens_per_turn_and_not_window() {
        let full = crate::api::agent_tools::full_discussion_catalogue();
        let tiered = crate::api::agent_tools::tiered(full.clone());

        let sized = |catalogue: Vec<serde_json::Value>| {
            let mut body = build_ollama_chat_body("qwen3.5:4b", "sys", "hi", None, 65_536, None);
            body["tools"] = serde_json::Value::Array(catalogue);
            fit_ollama_num_ctx(&mut body, 65_536);
            (
                body["tools"].to_string().len(),
                reachable_tools_bytes(&body),
                body["options"]["num_ctx"].as_u64().unwrap(),
            )
        };
        let (full_declared, full_reach, full_window) = sized(full);
        let (tiered_declared, tiered_reach, tiered_window) = sized(tiered);

        println!(
            "\nfull   declared {full_declared} B, reachable {full_reach} B, window {full_window}\n\
             tiered declared {tiered_declared} B, reachable {tiered_reach} B, window {tiered_window}"
        );
        assert!(
            tiered_declared < full_declared,
            "tiering sends less per turn, which is the whole of what it buys"
        );
        // And it buys nothing on the window: a tiered run can reach every
        // family, so it is sized for all of them plus the `tools_load`
        // declaration the full catalogue does not carry.
        assert!(
            tiered_window >= full_window,
            "tiering does not shrink the window: {tiered_window} against {full_window}"
        );
        assert!(
            tiered_reach > full_reach,
            "the reachable catalogue is the full one plus tools_load itself"
        );
    }

    /// The duplicate guard keys on name and arguments, so `task_get(id)` looks
    /// identical every time — and after two calls it is refused and the tool is
    /// withdrawn. But its answer changes the moment an item is ticked, which is
    /// precisely when an agent asks. Ticking clears the cached answer.
    #[test]
    fn ticking_a_task_makes_reading_it_a_new_question() {
        assert!(is_progress_observation_tool("task_get"));
        assert!(is_progress_observation_tool("task_list"));
        assert!(is_progress_observation_tool("plan_get"));
        assert!(!is_progress_observation_tool("read_file"));
        assert!(is_progress_mutation_tool("task_update_dod"));
        assert!(is_progress_mutation_tool("task_create"));
        assert!(!is_progress_mutation_tool("task_get"));

        let mut seen: std::collections::HashMap<String, (bool, serde_json::Value)> = [
            (
                "task_get|{\"task_id\":\"KT-1\"}".to_string(),
                (true, serde_json::json!({})),
            ),
            (
                "read_file|{\"path\":\"a.rs\"}".to_string(),
                (true, serde_json::json!({})),
            ),
        ]
        .into_iter()
        .collect();
        let mut repeated: std::collections::HashMap<String, usize> =
            [("task_get|{\"task_id\":\"KT-1\"}".to_string(), 2)]
                .into_iter()
                .collect();
        let mut results: std::collections::HashMap<(String, u64), String> =
            [(("task_get".to_string(), 1_u64), "old".to_string())]
                .into_iter()
                .collect();

        invalidate_progress_observation_cache(&mut seen, &mut repeated, &mut results);

        assert!(
            !seen.keys().any(|k| k.starts_with("task_get|")),
            "a ticked task must be readable again"
        );
        assert!(repeated.is_empty(), "and its refusal count resets");
        assert!(results.is_empty());
        // A file read is not affected: that cache is invalidated by writes, not
        // by task bookkeeping.
        assert!(seen.keys().any(|k| k.starts_with("read_file|")));
    }

    /// Trimming used to cut the biggest tool result and tell the model to ask
    /// for the rest — which returns the same result, cut the same way, a turn
    /// later. Measured on a 20-file job: the run spent itself re-reading what
    /// Kronn kept cutting. A shortened result keeps its facts and says plainly
    /// that asking again buys nothing.
    #[test]
    fn a_result_too_big_for_the_window_is_shortened_not_destroyed() {
        let file = serde_json::json!({
            "path": "src/Controller/HomeController.php",
            "sha256": "b7f3c1",
            "lines": 412,
            "content": "<?php\n".to_string() + &"// a long file\n".repeat(3_000),
        })
        .to_string();
        let mut body = serde_json::json!({
            "messages": [
                {"role": "system", "content": "sys"},
                {"role": "user", "content": "inventory src/"},
                {"role": "tool", "content": file},
            ],
            "options": {"num_ctx": 4_096},
        });
        clamp_ollama_tool_results(&mut body, 4_096);
        let kept = body["messages"][2]["content"].as_str().unwrap();

        assert!(kept.len() < file.len(), "it must actually shorten");
        // The coordinates of what was read survive, so the model knows what it
        // has without going back for it.
        assert!(
            kept.contains("HomeController.php") && kept.contains("b7f3c1"),
            "the facts must survive the shortening: {kept}"
        );
        // And nothing tells it to spend a turn on the identical call.
        assert!(
            !kept.contains("narrower range if you need the rest"),
            "the old note invited the re-read this whole change exists to stop"
        );
        assert!(
            kept.contains("same result"),
            "say that calling again buys nothing: {kept}"
        );
        // Still parseable: a cut in the middle of the JSON leaves the model a
        // result it cannot read at all.
        assert!(
            serde_json::from_str::<serde_json::Value>(kept).is_ok(),
            "a shortened result must stay valid JSON: {kept}"
        );
    }

    /// Non-JSON results have no facts to keep, so they are still cut — but the
    /// note must not ask for the same call back.
    #[test]
    fn a_result_that_cannot_be_shortened_is_cut_without_inviting_a_repeat() {
        let prose = "x".repeat(40_000);
        let mut body = serde_json::json!({
            "messages": [
                {"role": "user", "content": "go"},
                {"role": "tool", "content": prose},
            ],
            "options": {"num_ctx": 4_096},
        });
        clamp_ollama_tool_results(&mut body, 4_096);
        let kept = body["messages"][1]["content"].as_str().unwrap();
        assert!(kept.len() < 40_000);
        assert!(
            !kept.contains("narrower range if you need the rest"),
            "{kept}"
        );
        assert!(kept.contains("returns the same result"), "{kept}");
    }

    /// The fallback window is what a run gets when Ollama will not say what the
    /// model can hold. It has to fit the smallest real turn Kronn sends, or
    /// every such run is refused before it starts. The declared catalogue is
    /// the larger half of that turn, which is why this is measured and not
    /// assumed.
    #[test]
    fn the_fallback_window_holds_the_smallest_turn_kronn_sends() {
        let catalogue =
            crate::api::agent_tools::tiered(crate::api::agent_tools::full_discussion_catalogue());
        let tools_bytes = serde_json::Value::Array(catalogue).to_string().len();
        // A bare discussion turn: the identity and tools notices, no project
        // doc, no skills, no directives, and a one-line question.
        let system_context = format!(
            "{}\n\n{}",
            http_agent_identity_context(&AgentType::Ollama, "qwen3.5:2b"),
            http_agent_tools_notice(true)
        );
        let est = ((system_context.len() + 80 + tools_bytes) as u64 / 3) + 2048;
        let fallback = resolve_ctx_cap_within(None, None, 65_536).value;
        println!(
            "smallest turn: {est} tokens ({} B of catalogue, {} B of notices) | fallback window: {fallback}",
            tools_bytes,
            system_context.len()
        );
        assert!(
            est + 4_096 <= fallback,
            "the smallest turn needs {est} tokens and the fallback window is {fallback}; \
             a window that only just fits the floor leaves nothing for the conversation, and \
             the run is refused outright on a server that does not answer /api/show"
        );
        // A host that cannot allocate the fallback gets its own ceiling, not a
        // promise the machine cannot keep.
        assert_eq!(resolve_ctx_cap_within(None, None, 8_192).value, 8_192);
        assert_eq!(resolve_ctx_cap_within(None, None, 4_096).value, 4_096);
    }

    #[test]
    fn ollama_body_bounds_what_one_turn_may_generate() {
        let body = build_ollama_chat_body("qwen3:8b", "sys", "hi", None, 8192, None);
        let num_ctx = body["options"]["num_ctx"].as_u64().unwrap();
        let num_predict = body["options"]["num_predict"].as_i64().unwrap();
        assert!(
            num_predict > 0 && (num_predict as u64) < num_ctx,
            "an answer must fit the window it is generated in: {num_predict} of {num_ctx}"
        );
    }

    #[test]
    fn the_generation_cap_is_a_quarter_of_the_window_between_its_bounds() {
        // A quarter of the window, floored so a small window still answers.
        assert_eq!(num_predict_for(24_576, None), Some(6_144));
        assert_eq!(num_predict_for(2_048, None), Some(1_024));
        // Ceilinged: past a point a longer answer is a loop, not an answer.
        assert_eq!(num_predict_for(1_048_576, None), Some(8_192));
    }

    #[test]
    fn an_operator_can_move_the_generation_cap_or_remove_it() {
        assert_eq!(num_predict_for(24_576, Some("2048".into())), Some(2_048));
        // Zero or less: no cap, the previous behaviour.
        assert_eq!(num_predict_for(24_576, Some("0".into())), None);
        assert_eq!(num_predict_for(24_576, Some("-1".into())), None);
        // Blank or mistyped falls back to the default rather than lifting the
        // guard: a typo must not cost an hour of a blocked run.
        assert_eq!(num_predict_for(24_576, Some("   ".into())), Some(6_144));
        assert_eq!(
            num_predict_for(24_576, Some("beaucoup".into())),
            Some(6_144)
        );
    }

    /// The control token was for runtimes that had no `think` flag. A server
    /// that honours the flag must not also be paying for the token.
    #[test]
    fn the_no_think_token_is_only_for_servers_without_the_think_flag() {
        assert!(needs_no_think_token(None));
        assert!(needs_no_think_token(Some((0, 18))));
        assert!(!needs_no_think_token(Some((0, 19))));
        assert!(!needs_no_think_token(Some((0, 34))));

        let modern = build_ollama_chat_body("qwen3:8b", "", "hi", None, 8192, Some((0, 34)));
        assert!(
            !modern["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["content"] == "/no_think"),
            "a server that honours think:false does not need the token too"
        );
        // The flag itself stays: it is the one that actually works.
        assert_eq!(modern["think"], serde_json::json!(false));
    }

    #[test]
    fn ollama_body_injects_no_think_for_qwen3_only() {
        // qwen3 → a dedicated `/no_think` system message is prepended.
        let q = build_ollama_chat_body("qwen3:30b-a3b", "", "hi", None, 8192, None);
        let msgs = q["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "/no_think");
        // Non-qwen3 (e.g. llama3.3) → no /no_think message at all.
        let l = build_ollama_chat_body("llama3.3:70b", "", "hi", None, 8192, None);
        let lmsgs = l["messages"].as_array().unwrap();
        assert!(
            !lmsgs.iter().any(|m| m["content"] == "/no_think"),
            "no_think must be qwen3-only"
        );
    }

    #[test]
    fn ollama_body_sends_think_false_for_qwen3_only() {
        // qwen3 → reasoning switched off in the body too, not just via /no_think.
        let q = build_ollama_chat_body("qwen3.8:27b-mlx", "", "hi", None, 8192, None);
        assert_eq!(q["think"], false);
        // Any other model → the field is absent, so its own default stands.
        // Never `think:true`, which would force reasoning ON.
        let l = build_ollama_chat_body("llama3.3:70b", "", "hi", None, 8192, None);
        assert!(
            l.get("think").is_none(),
            "think must be omitted for non-qwen3, got {:?}",
            l.get("think")
        );
    }

    #[test]
    fn ollama_body_format_switches_to_non_stream() {
        // No format → stream text.
        let free = build_ollama_chat_body("qwen3:8b", "", "hi", None, 8192, None);
        assert_eq!(free["stream"], true);
        assert!(free.get("format").is_none());
        // TypedSchema format → non-stream (one validated JSON blob) + schema passed through.
        let schema = serde_json::json!({"type":"object","properties":{"x":{"type":"integer"}}});
        let typed = build_ollama_chat_body("qwen3:8b", "", "hi", Some(&schema), 8192, None);
        assert_eq!(typed["stream"], false);
        assert_eq!(typed["format"], schema);
    }

    #[test]
    fn a_catalogue_is_estimated_at_what_the_tokenizer_actually_charges() {
        // Measured, not assumed: three catalogue sizes across qwen3.8:27b-mlx
        // and gemma4:12b-mlx came back at 3.78-3.96 bytes per token marginal.
        // Charging declarations the prose ratio of 3.0 overstated a full 83 KB
        // catalogue by 31 % and refused a run whose real prompt was 21 962
        // tokens against a 32 768 ceiling.
        let mut body = serde_json::json!({ "messages": [] });
        let empty = estimated_chat_history_tokens(&body);

        let catalogue_bytes = 83_284usize;
        body["tools"] = serde_json::json!([{
            "type": "function",
            "function": {
                "name": "x",
                // Padded to the measured size of a full native catalogue.
                "description": "d".repeat(catalogue_bytes - 120),
                "parameters": {"type": "object", "properties": {}},
            },
        }]);
        let declared = estimated_chat_history_tokens(&body) - empty;

        // The real measurement for this catalogue was 21 962 tokens. The
        // estimate must sit above it — it is a budget, not a prediction — and
        // below the prose ratio that refused the run.
        assert!(
            (21_962..83_284 / 3).contains(&(declared as usize)),
            "declaration estimate {declared} must be conservative without being the old 3.0"
        );

        // And the whole request must fit the ceiling that used to refuse it.
        assert!(
            estimated_chat_history_tokens(&body) + WORKER_FINALIZATION_REPLY_HEADROOM < 32_768,
            "a full catalogue must be admissible at the MLX ceiling"
        );
    }

    #[test]
    fn the_declared_catalogue_is_charged_to_the_window_it_actually_occupies() {
        // Found by an audit of the deferred-tool design, and true before it:
        // the trimmer measured only `messages` while `tools` rides in the same
        // window. A principal in a workspace room carries ~32 KB of dense JSON
        // there — worth a third of a 32K slot — so the budget promised space
        // that was already spent, nothing was trimmed, and Ollama truncated the
        // history from the front with no error anywhere.
        let declarations = serde_json::json!(vec![
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": "heavy",
                    "description": "d".repeat(5_000),
                    "parameters": {"type": "object", "properties": {}},
                },
            });
            1
        ]);
        let result = "r".repeat(20_000);
        // Sized so the declarations are a real share of the window without
        // exhausting it: the point is that the history yields room to them,
        // not that an oversized catalogue is survivable (the next test owns
        // that case).

        let mut without = build_ollama_chat_body("qwen3.8:27b", "sys", "hi", None, 8192, None);
        let mut with = without.clone();
        with["tools"] = declarations.clone();
        for body in [&mut without, &mut with] {
            // A tooled run is pinned at the ceiling from the first request
            // (runner.rs sets num_ctx to ctx_cap whenever tools are declared);
            // the prompt-derived value the builder leaves would make the budget
            // zero and both arms trim to the floor, hiding what is measured.
            body["options"]["num_ctx"] = serde_json::json!(8192);
            body["messages"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "role": "tool", "tool_call_id": "big", "name": "read_file",
                    "content": result.clone(),
                }));
        }

        clamp_ollama_tool_results(&mut without, 8192);
        clamp_ollama_tool_results(&mut with, 8192);

        let budget = (8192usize - 2048) * 2;
        let declared = declarations.to_string().len();
        assert!(
            with["messages"].to_string().len() + declared <= budget,
            "messages plus declarations must fit the window, got {} + {declared}",
            with["messages"].to_string().len()
        );
        assert!(
            with["messages"].to_string().len() < without["messages"].to_string().len(),
            "a declared catalogue must cost the history room, not be invisible to it: with={} without={}",
            with["messages"].to_string().len(),
            without["messages"].to_string().len()
        );
    }

    #[test]
    fn a_catalogue_larger_than_the_window_trims_without_losing_the_user_turn() {
        // Degradation, not a hazard: the loop only ever trims tool results and
        // stops when none is left, so an oversized catalogue costs context and
        // never the question being answered. The up-front gate refuses this run
        // anyway; the trimmer must not make it worse on the way there.
        let mut body =
            build_ollama_chat_body("qwen3.8:27b", "sys", "the question", None, 8192, None);
        body["tools"] = serde_json::json!([{
            "type": "function",
            "function": {
                "name": "enormous",
                "description": "d".repeat(80_000),
                "parameters": {"type": "object", "properties": {}},
            },
        }]);
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "tool", "tool_call_id": "big", "name": "read_file",
                "content": "r".repeat(50_000),
            }));

        clamp_ollama_tool_results(&mut body, 8192);

        let messages = body["messages"].as_array().unwrap();
        assert!(
            messages
                .iter()
                .any(|m| m["role"] == "user" && m["content"] == "the question"),
            "the user turn survives whatever the catalogue costs"
        );
        assert!(
            messages.iter().any(|m| m["role"] == "system"),
            "and so does the system context"
        );
    }

    #[test]
    fn clamp_trims_the_biggest_tool_result_to_fit_the_cap() {
        let mut body = build_ollama_chat_body("qwen3.8:27b", "sys", "hi", None, 8192, None);
        {
            let messages = body["messages"].as_array_mut().unwrap();
            messages.push(serde_json::json!({
                "role": "tool", "tool_call_id": "small", "name": "git_status",
                "content": "clean",
            }));
            messages.push(serde_json::json!({
                "role": "tool", "tool_call_id": "big", "name": "git_diff",
                "content": "d".repeat(300_000),
            }));
        }
        let before = body["messages"].as_array().unwrap().len();
        clamp_ollama_tool_results(&mut body, 8192);

        // Fits the byte budget the cap can actually hold.
        // Mirrors BYTES_PER_TOKEN in the clamp: a tool loop carries dense JSON,
        // which tokenises far heavier than the forward estimate's prose ratio.
        let budget = (8192usize - 2048) * 2;
        assert!(
            body["messages"].to_string().len() <= budget,
            "still over budget: {}",
            body["messages"].to_string().len()
        );

        let messages = body["messages"].as_array().unwrap();
        let big = messages
            .iter()
            .find(|m| m["tool_call_id"] == "big")
            .unwrap();
        let small = messages
            .iter()
            .find(|m| m["tool_call_id"] == "small")
            .unwrap();
        assert!(
            big["content"]
                .as_str()
                .unwrap()
                .contains("truncated by Kronn"),
            "the oversized result must say what was dropped"
        );
        assert_eq!(
            small["content"], "clean",
            "a small result must survive untouched"
        );
        // Every call stays visible: trimming never drops a message.
        assert_eq!(messages.len(), before);
    }

    #[test]
    fn authoring_schemas_survive_clamping_when_tool_history_fills_the_window() {
        let workflow_schema: serde_json::Value =
            serde_json::from_str(include_str!("../api/workflow_step_schema.json")).unwrap();
        for (name, schema) in [
            ("workflow_step_schema", workflow_schema),
            (
                "tool_manual",
                serde_json::json!({"tool":"signals", "catalogue":crate::api::signal_catalog::catalogue()}),
            ),
        ] {
            let cap = 32768;
            let mut body = build_ollama_chat_body(
                "qwen3.8:27b-mlx",
                "sys",
                "Create a workflow",
                None,
                cap,
                None,
            );
            body["tools"] = serde_json::json!(crate::api::agent_tools::full_discussion_catalogue());
            fit_ollama_num_ctx(&mut body, cap);
            // Production raises a tooled run to its effective cap before sending
            // the first request. Reproduce that, then accumulated tool history.
            body["options"]["num_ctx"] = serde_json::json!(cap);
            let content = schema.to_string();
            body["messages"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "role":"tool","tool_call_id":"schema","name":name,"content":content
                }));
            body["messages"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "role":"assistant","content":"prior reasoning ".repeat(400)
                }));
            body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role":"tool","tool_call_id":"history","name":"git_diff","content":"d".repeat(60_000)
            }));
            // Reproduce the production order under actual history pressure.
            clamp_ollama_tool_results(&mut body, cap);
            resize_ollama_num_ctx(&mut body, cap);
            let messages = body["messages"].as_array().unwrap();
            let sent = messages
                .iter()
                .find(|m| m["tool_call_id"] == "schema")
                .unwrap()["content"]
                .as_str()
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(sent).unwrap(),
                schema
            );
            assert!(messages.last().unwrap()["content"]
                .as_str()
                .unwrap()
                .contains("truncated by Kronn"));
            assert!(
                estimated_chat_history_tokens(&body) <= cap,
                "the complete contract must fit the configured cap"
            );
            assert!(body["options"]["num_ctx"].as_u64().unwrap() <= cap);
        }
    }

    #[test]
    fn clamp_never_blindly_cuts_a_checkpoint_receipt_envelope() {
        let mut body = build_ollama_chat_body("qwen3.8:27b", "sys", "hi", None, 4096, None);
        let protected = serde_json::json!({
            "kronn_checkpoint_compacted": true,
            "preserved_scalar_facts": [{
                "field": "content_sha256",
                "value": "a".repeat(64),
            }],
            "large_field_excerpts": [{"field": "text", "excerpt": "x".repeat(2_000)}],
        })
        .to_string();
        {
            let messages = body["messages"].as_array_mut().unwrap();
            messages.push(serde_json::json!({
                "role": "tool", "tool_call_id": "checkpoint", "name": "read_file",
                "content": protected,
            }));
            messages.push(serde_json::json!({
                "role": "tool", "tool_call_id": "fresh", "name": "git_diff",
                "content": "d".repeat(30_000),
            }));
        }

        clamp_ollama_tool_results(&mut body, 4096);

        let messages = body["messages"].as_array().unwrap();
        let checkpoint = messages
            .iter()
            .find(|message| message["tool_call_id"] == "checkpoint")
            .unwrap()["content"]
            .as_str()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(content).ok())
            .unwrap();
        assert_eq!(
            checkpoint["preserved_scalar_facts"][0]["value"],
            "a".repeat(64)
        );
        let fresh = messages
            .iter()
            .find(|message| message["tool_call_id"] == "fresh")
            .unwrap()["content"]
            .as_str()
            .unwrap();
        assert!(fresh.contains("truncated by Kronn"));
    }

    #[test]
    fn clamp_leaves_results_that_already_fit() {
        let mut body = build_ollama_chat_body("qwen3.8:27b", "sys", "hi", None, 32768, None);
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "tool", "tool_call_id": "c1", "name": "read_file", "content": "small file",
            }));
        let before = body["messages"].clone();
        clamp_ollama_tool_results(&mut body, 32768);
        assert_eq!(body["messages"], before, "nothing to trim, nothing touched");
    }

    #[test]
    fn a_rich_collection_keeps_every_item_in_compact_valid_json() {
        // The Fastly case: 43 services, each carrying its full version history,
        // so only the first used to survive the trim. Keeping every identifier
        // makes the inventory useful without pretending the history also fits.
        let big_entry = |i: usize| {
            serde_json::json!({
                "id": format!("svc-{i}"),
                "created_at": "2026-08-18T10:00:00Z",
                "updated_at": "2026-08-20T12:34:56Z",
                "history": "x".repeat(4_000),
            })
        };
        let payload = serde_json::Value::Array((0..43).map(big_entry).collect());
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 8192, None);
        body["options"]["num_ctx"] = serde_json::json!(8192);
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "tool", "tool_call_id": "c1", "name": "api_call",
                "content": payload.to_string(),
            }));

        clamp_ollama_tool_results(&mut body, 8192);

        let trimmed = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        let (json, _) = trimmed
            .split_once("\n\n[compacted by Kronn:")
            .expect("a compact collection must explain the omitted detail");
        let compact: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(compact.as_array().unwrap().len(), 43);
        assert_eq!(compact[42]["id"], "svc-42");
        assert_eq!(compact[42]["created_at"], "2026-08-18T10:00:00Z");
        assert_eq!(compact[42]["updated_at"], "2026-08-20T12:34:56Z");
        assert!(!trimmed.contains("INCOMPLETE"));
        assert!(
            trimmed.contains("all 43 collection items are still present"),
            "the model must know that the compact inventory is complete"
        );
    }

    #[test]
    fn github_collection_keeps_names_languages_and_timestamps_for_every_repo() {
        let payload = serde_json::Value::Array(
            (0..10)
                .map(|i| {
                    serde_json::json!({
                        "id": i,
                        "name": format!("repo-{i}"),
                        "full_name": format!("euronews/repo-{i}"),
                        "language": if i % 2 == 0 { "Rust" } else { "TypeScript" },
                        "created_at": "2026-08-18T10:00:00Z",
                        "updated_at": format!("2026-08-20T12:{i:02}:00Z"),
                        "permissions": "x".repeat(4_000),
                    })
                })
                .collect(),
        );
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 8192, None);
        body["options"]["num_ctx"] = serde_json::json!(8192);
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "tool", "tool_call_id": "c1", "name": "api_call",
                "content": payload.to_string(),
            }));

        clamp_ollama_tool_results(&mut body, 8192);

        let trimmed = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        let (json, _) = trimmed
            .split_once("\n\n[compacted by Kronn:")
            .expect("the GitHub-like collection should be compacted");
        let compact: serde_json::Value = serde_json::from_str(json).unwrap();
        let repos = compact.as_array().unwrap();
        assert_eq!(repos.len(), 10);
        for (index, repo) in repos.iter().enumerate() {
            assert_eq!(repo["name"], format!("repo-{index}"));
            assert_eq!(repo["full_name"], format!("euronews/repo-{index}"));
            assert!(repo["language"].is_string());
            assert!(repo["created_at"].is_string());
            assert!(repo["updated_at"].is_string());
        }
    }

    #[test]
    fn compacting_a_collection_envelope_preserves_its_shape_and_metadata() {
        let payload = serde_json::json!({
            "data": (0..43)
                .map(|i| serde_json::json!({
                    "id": format!("svc-{i}"),
                    "name": format!("service-{i}"),
                    "versions": (0..125).collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
            "meta": { "total": 43 }
        });
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 8192, None);
        body["options"]["num_ctx"] = serde_json::json!(8192);
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "tool", "tool_call_id": "c1", "name": "api_call",
                "content": payload.to_string(),
            }));

        clamp_ollama_tool_results(&mut body, 8192);

        let trimmed = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        let (json, _) = trimmed
            .split_once("\n\n[compacted by Kronn:")
            .expect("the envelope should be compacted without losing its collection");
        let compact: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(compact["data"].as_array().unwrap().len(), 43);
        assert_eq!(compact["data"][42]["id"], "svc-42");
        assert_eq!(compact["meta"]["total"], 43);
    }

    #[test]
    fn collection_budget_counts_json_escaping_before_dropping_items() {
        let quoted = "\\\"quoted\\\"\\\\path".repeat(2_000);
        let payload = serde_json::json!([{
            "id": "svc-1",
            "verbose_field": quoted,
        }]);
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 8192, None);
        body["options"]["num_ctx"] = serde_json::json!(8192);
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "tool", "tool_call_id": "c1", "name": "api_call",
                "content": payload.to_string(),
            }));

        clamp_ollama_tool_results(&mut body, 8192);

        let trimmed = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        let (json, _) = trimmed
            .split_once("\n\n[compacted by Kronn:")
            .expect("the compact identifier fits once encoded bytes are counted exactly");
        let compact: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(compact.as_array().unwrap().len(), 1);
        assert_eq!(compact[0]["id"], "svc-1");
        assert!(
            compact[0]["verbose_field"]
                .as_str()
                .unwrap()
                .chars()
                .count()
                <= 257
        );
    }

    #[test]
    fn a_collection_that_cannot_fit_keeps_a_valid_prefix_and_exact_count() {
        let payload = serde_json::Value::Array(
            (0..43)
                .map(|i| serde_json::Value::String(format!("item-{i}-{}", "x".repeat(4_000))))
                .collect(),
        );
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 4096, None);
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "tool", "tool_call_id": "c1", "name": "api_call",
                "content": payload.to_string(),
            }));

        clamp_ollama_tool_results(&mut body, 4096);

        let trimmed = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        let (json, note) = trimmed
            .split_once("\n\n[truncated by Kronn:")
            .expect("a partial collection must carry an explicit diagnostic");
        let compact: serde_json::Value = serde_json::from_str(json).unwrap();
        let kept = compact.as_array().unwrap().len();
        assert!(kept < 43);
        assert!(note.contains(&format!("{kept} of 43 items kept")));
        assert!(note.contains("INCOMPLETE"));
    }

    #[test]
    fn a_truncated_text_still_reports_bytes() {
        // Not a collection: there is no item count to give, so the byte note
        // stays — it is the honest thing to say about a cut document.
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 8192, None);
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "tool", "tool_call_id": "c1", "name": "read_file",
                "content": "x".repeat(60_000),
            }));

        clamp_ollama_tool_results(&mut body, 8192);

        let trimmed = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(trimmed.contains("bytes dropped"), "expected the byte note");
        assert!(
            !trimmed.contains("INCOMPLETE"),
            "not a list, must not claim item loss"
        );
    }

    #[test]
    fn clamp_trims_against_the_window_actually_granted() {
        // Regression: a one-line question sized the window near the floor, then
        // the first tool result blew past it. Trimming to the CAP left the
        // prompt just as oversized for the slot Ollama had already fixed, so the
        // history was truncated until the user turn itself was gone (HTTP 500,
        // "no user query found in messages").
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 32768, None);
        body["options"]["num_ctx"] = serde_json::json!(4864);
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "tool", "tool_call_id": "c1", "name": "mcp_list",
                "content": "x".repeat(60_000),
            }));

        clamp_ollama_tool_results(&mut body, 32768);

        // The budget that matters is the granted window, not the ceiling.
        let granted = (4864usize - 2048) * 2;
        assert!(
            body["messages"].to_string().len() <= granted,
            "trimmed to the cap instead of the granted window: {} > {granted}",
            body["messages"].to_string().len()
        );
    }

    #[test]
    fn resize_num_ctx_grows_with_the_tool_results_and_never_shrinks() {
        let mut body = build_ollama_chat_body("qwen3.8:27b", "sys", "hi", None, 32768, None);
        let first_turn = body["options"]["num_ctx"].as_u64().unwrap();

        // The tool loop appends a result far bigger than the first-turn estimate.
        let big = "x".repeat(60_000);
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "tool", "tool_call_id": "c1", "name": "read_file", "content": big,
            }));
        resize_ollama_num_ctx(&mut body, 32768);
        let grown = body["options"]["num_ctx"].as_u64().unwrap();
        assert!(
            grown > first_turn,
            "window must grow with the messages: {first_turn} -> {grown}"
        );

        // Clamped to the cap, never above it.
        resize_ollama_num_ctx(&mut body, 4096);
        assert_eq!(
            body["options"]["num_ctx"].as_u64().unwrap(),
            grown,
            "a smaller cap must not shrink a window already sized for these messages"
        );
    }

    #[test]
    fn resize_num_ctx_respects_the_cap() {
        let mut body = build_ollama_chat_body("qwen3.8:27b", "sys", "hi", None, 8192, None);
        let big = "x".repeat(200_000);
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "role": "tool", "tool_call_id": "c1", "name": "git_diff", "content": big,
            }));
        resize_ollama_num_ctx(&mut body, 8192);
        assert_eq!(body["options"]["num_ctx"].as_u64().unwrap(), 8192);
    }

    #[test]
    fn ollama_num_ctx_is_clamped_both_ends() {
        assert_eq!(ollama_num_ctx("", "", 8192), 2048, "tiny prompt → floor");
        let huge = "x".repeat(100_000);
        assert_eq!(
            ollama_num_ctx(&huge, &huge, 8192),
            8192,
            "huge prompt → cap"
        );
    }

    #[tokio::test]
    async fn forward_ollama_line_forwards_content_and_captures_tokens() {
        use std::sync::{Arc, Mutex};
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(8);
        let stderr = Arc::new(Mutex::new(Vec::<String>::new()));
        let (mut done, mut err) = (false, false);
        // A streamed content chunk...
        forward_ollama_line(
            r#"{"message":{"content":"391"},"done":false}"#,
            &tx,
            &stderr,
            &mut done,
            &mut err,
            0,
        )
        .await;
        assert!(!done && !err);
        // ...then the terminal `done` object (identical shape to a non-stream
        // single-object response), carrying the token counts.
        forward_ollama_line(
            r#"{"message":{"content":""},"done":true,"prompt_eval_count":12,"eval_count":3}"#,
            &tx,
            &stderr,
            &mut done,
            &mut err,
            0,
        )
        .await;
        assert!(done && !err, "terminal chunk sets got_done, no error");
        drop(tx);
        let mut got = String::new();
        while let Some(s) = rx.recv().await {
            got.push_str(&s);
        }
        assert_eq!(got, "391");
        assert_eq!(
            stderr.lock().unwrap().as_slice(),
            &["ollama_tokens:12:3".to_string()]
        );
    }

    /// End-to-end against a REAL local model, to catch what mocks cannot:
    /// that a live model actually emits a call our decoder accepts, and that
    /// it uses the injected result instead of re-asking.
    ///
    /// Ignored by default — needs a LiteLLM proxy on :4000 fronting Ollama.
    /// Run with: `cargo test --lib live_tool_loop -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "requires a live LiteLLM proxy on :4000"]
    async fn live_tool_loop_against_a_real_model() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "Which MCP servers are available? Use the tool, then answer with just their names.",
            "You are an agent inside Kronn. Use the provided tools for data you do not have.",
            "local-fast",
            None,
            Some("http://127.0.0.1:4000"),
            None,
            Some(std::sync::Arc::new(FakeTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("proxy must be reachable");

        let mut out = String::new();
        while let Some(chunk) = process.next_line().await {
            out.push_str(&chunk);
        }
        eprintln!(
            "--- model answer ---\n{out}\n--- calls: {:?}",
            seen.lock().unwrap()
        );

        assert!(
            !seen.lock().unwrap().is_empty(),
            "the model never called the tool: {out:?}"
        );
        // The fake returns github + context7; a model that ignored the result
        // would have no way to name them.
        let lower = out.to_lowercase();
        assert!(
            lower.contains("github") || lower.contains("context7"),
            "answer does not use the tool result: {out:?}"
        );
    }

    // ─── The prose that once defeated the whole feature ──────────────────────

    /// Pointed at a path and nothing else, two of six local models never opened
    /// the doc: one searched the repository with `list_files` eight turns long,
    /// the other went to `git_log` and `web_fetch`. The section index is what
    /// tells them the file exists and what is in it.
    #[test]
    fn the_project_doc_pointer_carries_the_doc_s_own_section_index() {
        let doc = "# Title\n\nintro\n\n## Entry procedure\n\ntext\n\n\
                   ### Not a section\n\n## Source of truth\n\nmore\n";
        assert_eq!(
            doc_section_index(doc),
            "- Entry procedure\n- Source of truth",
            "only the top-level sections, in the doc's own order"
        );
        // A doc with no sections costs nothing rather than an empty bullet.
        assert_eq!(doc_section_index("# Title\n\njust prose\n"), "");
    }

    /// An index that grew with the doc would recreate the cost it removes.
    #[test]
    fn the_section_index_stays_a_map_not_a_copy() {
        let doc: String = (0..80)
            .map(|n| format!("## Section {n}\n\nbody\n\n"))
            .collect();
        let index = doc_section_index(&doc);
        assert_eq!(index.lines().count(), 24);
        assert!(index.len() < 1_024, "{} bytes of index", index.len());
    }

    /// The notice is paid on every turn, so it carries rules only: what a tool
    /// does belongs to its declaration. It grew to 2 700 characters by reciting
    /// twenty tools, some of which a tiered run had not even declared yet.
    #[test]
    fn the_tools_notice_states_rules_rather_than_reciting_the_catalogue() {
        let with = http_agent_tools_notice(true);
        assert!(
            with.len() < 1_900,
            "the notice is {} bytes of every turn; describe tools in their declarations",
            with.len()
        );
        for mechanical in ["expected_sha256", "offset", "recursive", "truncated"] {
            assert!(
                !with.contains(mechanical),
                "`{mechanical}` belongs to its tool's own description: {with}"
            );
        }
    }

    /// With tools on the wire, the prompt must not tell the model it has none.
    /// This exact contradiction shipped once: `tools_declared=5` while the
    /// context said "You have NO executable tools", and the model refused to
    /// call anything. Neither the codec tests nor the loop tests could see it.
    #[test]
    fn tools_notice_matches_whether_tools_were_actually_declared() {
        let with = http_agent_tools_notice(true);
        let without = http_agent_tools_notice(false);

        assert!(
            !with.contains("NO executable tools"),
            "declaring tools then denying them makes the model refuse to call: {with}"
        );
        assert!(
            with.contains("CALL the matching tool"),
            "the model must be told to use what it was given: {with}"
        );
        // Observed on a real NVIDIA run (llama-3.3-70b, MSG-bb425b55): the model
        // wrote "Voici le résultat de `git_diff`" followed by a JSON blob whose
        // `diff` field was the literal "...", BEFORE the call had returned. The
        // anti-hallucination preamble covers unverified FACTS and citations; it says
        // nothing about fabricating a tool's OUTPUT, which is a distinct failure mode
        // of the tool loop. The tools notice is where that gap closes.
        assert!(
            with.contains("something you RECEIVE, never something you"),
            "the model must be told a result is received, not composed: {with}"
        );
        assert!(
            with.contains("never present, quote or summarise a result before"),
            "the ban must name the act (presenting a result early), not just the intent: {with}"
        );
        assert!(
            with.contains("NOT MCP servers")
                && with.contains("mcp_list")
                && with.contains("api_endpoints")
                && with.contains("api_call")
                && with.contains("qa_list")
                && with.contains("qa_run"),
            "HTTP discussion agents need the complete native API route instead of searching for a vendor MCP: {with}"
        );
        assert!(
            without.contains("NO executable tools"),
            "without an executor the model must not invent calls: {without}"
        );

        // KT-338 flipped this half deliberately. File tools now exist for an HTTP
        // agent (workspace-scoped, server-executed), so the OLD assertion — "file
        // access must stay denied in both cases" — would now recreate the exact bug
        // this test was written for, in the other direction: declaring read_file
        // while the prose says "NO file access" makes the model refuse to call it.
        //
        // What must NOT be lost is the invariant behind that old wording: the 2026-07-01
        // incident where a model claimed to have read docs/ it never opened. That is
        // now enforced positively — the notice names the tools AND forbids claiming a
        // file was read or written outside them.
        assert!(
            !with.contains("NO file access"),
            "file tools are declared now; denying them makes the model refuse to call: {with}"
        );
        assert!(
            with.contains("read_file")
                && with.contains("write_file")
                && with.contains("list_files")
                && with.contains("web_fetch"),
            "the model must be told which file and web tools it actually has: {with}"
        );
        assert!(
            with.contains("Never claim to have read, edited or written a file you did not obtain"),
            "the anti-hallucination clause is what the old NO-file-access wording protected: {with}"
        );

        // Codex review (2026-08-24): the notice told the model git was
        // "read-only (no commit, no checkout)" while `git_commit` was declared
        // on the same request — the exact KT-338 contradiction, in a corner
        // this test's other assertions did not cover.
        assert!(
            with.contains("git_commit"),
            "a declared git_commit must be named, or the model will not call it: {with}"
        );
        assert!(
            !with.contains("no commit"),
            "declaring git_commit while the prose still says 'no commit' is the exact \
             KT-338 contradiction: {with}"
        );
        assert!(
            with.contains("edit_file") && with.contains("edit_lines"),
            "the anchored-edit tools must be named so a model reaches for them \
             instead of write_file on an existing file: {with}"
        );
        assert!(
            without.contains("NO executable tools and NO file access"),
            "with no executor the denial must stay absolute: {without}"
        );
    }

    #[test]
    fn http_agent_identity_prevents_a_local_model_from_copying_claude() {
        let ollama = http_agent_identity_context(&AgentType::Ollama, "qwen3:32b");
        assert!(ollama.contains("qwen3:32b"));
        assert!(ollama.contains("served by Ollama"));
        assert!(ollama.contains("not Claude"));
        assert!(ollama.contains("@ollama"));
        assert!(ollama.contains("LiteLlm"));
        assert!(ollama.contains("Never copy another participant's self-identification"));

        let lite_llm = http_agent_identity_context(&AgentType::LiteLlm, "claude-sonnet-4-6");
        assert!(lite_llm.contains("claude-sonnet-4-6"));
        assert!(lite_llm.contains("LiteLLM proxy"));
        assert!(lite_llm.contains("@litellm"));
    }

    #[test]
    fn nvidia_identity_names_the_model_so_it_cannot_borrow_a_human_name() {
        // Observed in a room: asked "tu es QUI ?", the NVIDIA agent answered
        // "Je suis Romu" — the human's first name, lifted from the history. The
        // catch-all arm returned an empty identity context, so nothing anchored it.
        let ctx = http_agent_identity_context(
            &AgentType::Nvidia,
            "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning",
        );
        assert!(
            !ctx.is_empty(),
            "an HTTP agent with no identity context invents one"
        );
        assert!(
            ctx.contains("nvidia/nemotron-3-nano-omni-30b-a3b-reasoning"),
            "the model must be named so 'who are you' has an answer: {ctx}"
        );
        assert!(
            ctx.contains("@nvidia"),
            "the alias that addresses it must be stated: {ctx}"
        );
        assert!(
            ctx.contains("a human's name in the history is never yours"),
            "the exact failure observed must be closed explicitly: {ctx}"
        );
    }

    // ─── Tool loop ───────────────────────────────────────────────────────────

    #[test]
    fn convergence_diagnostic_distinguishes_errors_from_refusals() {
        let calls = [("find_files".to_string(), 20)].into_iter().collect();
        let errors = [("find_files".to_string(), 3)].into_iter().collect();
        let refusals = [("find_files".to_string(), 17)].into_iter().collect();

        assert_eq!(
            tool_convergence_diagnostic(&calls, &errors, &refusals),
            "find_files: 20 attempts (3 errors, 17 refused)"
        );
    }

    fn outcome(
        tool: &str,
        args: serde_json::Value,
        content: serde_json::Value,
    ) -> crate::agents::tools::ToolOutcome {
        crate::agents::tools::ToolOutcome {
            call: crate::agents::tools::ToolCall {
                id: "call-1".into(),
                name: tool.into(),
                arguments: args,
            },
            content,
            ok: true,
        }
    }

    /// The loop measured on a real Ollama delegation: twelve `git_log` calls
    /// with a different `limit` each time. Twelve distinct signatures, so the
    /// exact-call guard stayed silent and the model learnt nothing until the
    /// cap refused the thirteenth.
    #[test]
    fn a_reworded_question_that_returns_the_same_answer_says_so() {
        let mut seen = std::collections::HashMap::new();
        let payload = serde_json::json!({ "count": 2, "commits": ["a", "b"] });

        let mut first = outcome(
            "git_log",
            serde_json::json!({ "limit": 30 }),
            payload.clone(),
        );
        annotate_unproductive_repetition(&mut first, "git_log", "limit=30", 1, 12, &mut seen);
        assert!(
            first.content.get("kronn_same_answer_as_before").is_none(),
            "the first call has nothing to repeat: {}",
            first.content
        );

        let mut second = outcome("git_log", serde_json::json!({ "limit": 50 }), payload);
        annotate_unproductive_repetition(&mut second, "git_log", "limit=50", 2, 12, &mut seen);
        let note = second.content["kronn_same_answer_as_before"]
            .as_str()
            .expect("the second call must be named as unproductive");
        assert!(
            note.contains("limit=30"),
            "name the arguments that already answered: {note}"
        );
        assert!(
            note.contains("limit=50"),
            "name the arguments just used: {note}"
        );
        assert!(second.ok, "the payload is annotated, never withheld");
        assert_eq!(second.content["count"], 2, "the result itself must survive");
    }

    /// Distinct answers are honest work, so no tool may be accused of circling
    /// on their account — but a tool called over and over still gets told where
    /// it stands, a third of the way into its budget rather than at the cap.
    #[test]
    fn a_tool_called_repeatedly_learns_where_it_stands() {
        let mut seen = std::collections::HashMap::new();
        let mut early = outcome(
            "git_log",
            serde_json::json!({ "limit": 10 }),
            serde_json::json!({ "count": 10 }),
        );
        annotate_unproductive_repetition(&mut early, "git_log", "limit=10", 3, 12, &mut seen);
        assert!(
            early.content.get("kronn_call_budget").is_none(),
            "three calls is not yet a loop: {}",
            early.content
        );

        let mut late = outcome(
            "git_log",
            serde_json::json!({ "limit": 15 }),
            serde_json::json!({ "count": 15 }),
        );
        annotate_unproductive_repetition(&mut late, "git_log", "limit=15", 4, 12, &mut seen);
        let note = late.content["kronn_call_budget"]
            .as_str()
            .expect("a fourth call warns");
        assert!(note.contains("4"), "state the call reached: {note}");
        assert!(note.contains("12"), "state the ceiling: {note}");
        assert_eq!(late.content["count"], 15, "the result itself must survive");
    }

    /// Reading a dozen different files is legitimate analysis, which is why
    /// `read_file` has a wider budget. The warning must scale with it instead
    /// of firing on the fourth honest read.
    #[test]
    fn a_wider_budget_pushes_the_warning_back() {
        let mut seen = std::collections::HashMap::new();
        let limit = max_calls_for_tool("read_file", crate::agents::tools::ToolRunMode::General);
        for index in 1..limit.div_ceil(3) {
            let mut read = outcome(
                "read_file",
                serde_json::json!({ "path": format!("f{index}") }),
                serde_json::json!({ "content": format!("body {index}") }),
            );
            annotate_unproductive_repetition(
                &mut read,
                "read_file",
                &format!("path=f{index}"),
                index,
                limit,
                &mut seen,
            );
            assert!(
                read.content.get("kronn_call_budget").is_none(),
                "read {index} of {limit} is honest work: {}",
                read.content
            );
        }
    }

    /// The notes are added to the payload the digest is taken from, so a first
    /// annotated result must still match the second one it is compared against.
    #[test]
    fn the_guards_own_notes_never_mask_a_repetition() {
        let mut seen = std::collections::HashMap::new();
        let payload = serde_json::json!({ "count": 0, "commits": [] });

        let mut fourth = outcome(
            "git_log",
            serde_json::json!({ "limit": 11 }),
            payload.clone(),
        );
        annotate_unproductive_repetition(&mut fourth, "git_log", "limit=11", 4, 12, &mut seen);
        assert!(
            fourth.content.get("kronn_call_budget").is_some(),
            "sanity: this one is annotated"
        );

        let mut fifth = outcome("git_log", serde_json::json!({ "limit": 12 }), payload);
        annotate_unproductive_repetition(&mut fifth, "git_log", "limit=12", 5, 12, &mut seen);
        assert!(
            fifth.content.get("kronn_same_answer_as_before").is_some(),
            "an annotated first result must still be recognised: {}",
            fifth.content
        );
    }

    /// A non-object payload cannot carry a note without being wrapped, and
    /// wrapping changes the shape the model was promised.
    #[test]
    fn a_payload_that_cannot_carry_a_note_is_left_alone() {
        let mut seen = std::collections::HashMap::new();
        let mut listed = outcome(
            "git_log",
            serde_json::json!({ "limit": 5 }),
            serde_json::json!(["a", "b"]),
        );
        annotate_unproductive_repetition(&mut listed, "git_log", "limit=5", 9, 12, &mut seen);
        assert_eq!(listed.content, serde_json::json!(["a", "b"]), "untouched");
    }

    #[test]
    fn worker_progress_nudge_is_aggregate_and_non_destructive() {
        let mut before = outcome(
            "read_file",
            serde_json::json!({ "path": "a.rs" }),
            serde_json::json!({ "content": "evidence" }),
        );
        annotate_worker_exploration(&mut before, WORKER_EXPLORATION_NUDGE_AT - 1);
        assert!(before.content.get("kronn_worker_progress").is_none());

        let mut threshold = outcome(
            "search_text",
            serde_json::json!({ "query": "target" }),
            serde_json::json!({ "matches": [{ "line": 42 }] }),
        );
        annotate_worker_exploration(&mut threshold, WORKER_EXPLORATION_NUDGE_AT);
        let note = threshold.content["kronn_worker_progress"]
            .as_str()
            .expect("the aggregate threshold must nudge");
        assert!(note.contains("evidence already acquired"));
        assert!(note.contains("exact evidence still missing"));
        assert_eq!(
            threshold.content["matches"][0]["line"], 42,
            "the observation itself must remain intact"
        );
    }

    #[test]
    fn mutation_invalidates_only_workspace_observation_replays() {
        let mut seen_calls = std::collections::HashMap::from([
            (
                "read_file|path=\"a.rs\"".into(),
                (true, serde_json::json!({ "content_sha256": "old" })),
            ),
            (
                "git_status|".into(),
                (true, serde_json::json!({ "clean": true })),
            ),
            (
                "api_call|path=\"/charge\"".into(),
                (true, serde_json::json!({ "accepted": true })),
            ),
            (
                "edit_lines|path=\"a.rs\"".into(),
                (true, serde_json::json!({ "changed": true })),
            ),
        ]);
        let mut repeated_calls = std::collections::HashMap::from([
            ("read_file|path=\"a.rs\"".into(), 1),
            ("api_call|path=\"/charge\"".into(), 1),
        ]);
        let mut results_seen = std::collections::HashMap::from([
            (("read_file".into(), 1), "old".into()),
            (("api_call".into(), 2), "effect".into()),
        ]);

        invalidate_workspace_observation_cache(
            &mut seen_calls,
            &mut repeated_calls,
            &mut results_seen,
        );

        assert!(!seen_calls.keys().any(|key| key.starts_with("read_file|")));
        assert!(!seen_calls.keys().any(|key| key.starts_with("git_status|")));
        assert!(seen_calls.keys().any(|key| key.starts_with("api_call|")));
        assert!(seen_calls.keys().any(|key| key.starts_with("edit_lines|")));
        assert!(!repeated_calls
            .keys()
            .any(|key| key.starts_with("read_file|")));
        assert!(repeated_calls
            .keys()
            .any(|key| key.starts_with("api_call|")));
        assert!(!results_seen.keys().any(|(name, _)| name == "read_file"));
        assert!(results_seen.keys().any(|(name, _)| name == "api_call"));
    }

    #[test]
    fn worker_finalization_checkpoint_keeps_authority_and_recent_protocol_tail() {
        let mut body = serde_json::json!({
            "messages": [
                {"role": "system", "content": "/no_think"},
                {"role": "system", "content": "worker policy"},
                {"role": "user", "content": "the principal's exact task brief"},
                {"role": "user", "content": "an obsolete assignment"},
                {"role": "user", "content": "the latest reassignment reason"}
            ],
            "tools": [{
                "type": "function",
                "function": {"name": "read_file", "parameters": {"type": "object"}}
            }],
            "options": {"num_ctx": 65536}
        });
        let seed = WorkerCheckpointSeed::from_body(&body);
        let messages = body["messages"].as_array_mut().unwrap();
        for round in 0..5 {
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": format!("call-{round}"),
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": {"path": format!("backend/src/{round}.rs")}
                    }
                }]
            }));
            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": format!("call-{round}"),
                "name": "read_file",
                "content": if round == 4 {
                    serde_json::json!({
                        "path": "backend/src/api/agent_tools.rs",
                        "content_sha256": "a".repeat(64),
                        "next_offset": 121,
                        "text": "x".repeat(60_000),
                    }).to_string()
                } else {
                    format!("result-{round}")
                }
            }));
        }
        messages.push(serde_json::json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "withdrawn-search",
                "type": "function",
                "function": {"name": "search_text", "arguments": {"query": "obsolete"}}
            }]
        }));
        messages.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": "withdrawn-search",
            "name": "search_text",
            "content": "{\"path\":\"obsolete.rs\"}"
        }));
        let mutated_paths =
            std::collections::BTreeSet::from(["backend/src/api/agent_tools.rs".to_string()]);

        let checkpoint = checkpoint_worker_finalization_history(
            &mut body,
            &seed,
            "inspect the durable worktree and finalize",
            65_536,
            true,
            &mutated_paths,
        );

        assert_eq!(checkpoint.before_messages, 17);
        assert_eq!(checkpoint.seed_messages, 4);
        assert_eq!(checkpoint.tail_messages, 6);
        assert_eq!(checkpoint.after_messages, 11);
        assert_eq!(checkpoint.compacted_tool_results, 1);
        assert!(checkpoint.after_tokens < checkpoint.before_tokens);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["content"], "/no_think");
        assert_eq!(messages[1]["content"], "worker policy");
        assert_eq!(messages[2]["content"], "the principal's exact task brief");
        assert_eq!(messages[3]["content"], "the latest reassignment reason");
        assert!(!body["messages"]
            .to_string()
            .contains("an obsolete assignment"));
        assert!(!body["messages"].to_string().contains("call-0"));
        assert!(!body["messages"].to_string().contains("call-1"));
        assert!(body["messages"].to_string().contains("call-2"));
        assert!(body["messages"].to_string().contains("call-4"));
        assert!(!body["messages"].to_string().contains("withdrawn-search"));
        assert!(!body["messages"].to_string().contains("obsolete.rs"));
        assert!(!body["messages"].to_string().contains(&"x".repeat(10_000)));
        let compacted_read_content = messages
            .iter()
            .find(|message| message["tool_call_id"] == "call-4")
            .unwrap()["content"]
            .as_str()
            .unwrap();
        assert!(compacted_read_content.len() <= WORKER_CHECKPOINT_TOOL_RESULT_BYTES);
        let compacted_read =
            serde_json::from_str::<serde_json::Value>(compacted_read_content).unwrap();
        assert_eq!(compacted_read["kronn_checkpoint_compacted"], true);
        assert!(compacted_read["preserved_scalar_facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fact| fact["field"] == "content_sha256" && fact["value"] == "a".repeat(64)));
        let tool_call_ids = messages
            .iter()
            .filter(|message| message["role"] == "assistant")
            .flat_map(|message| {
                message["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|call| call["id"].as_str())
            })
            .collect::<std::collections::HashSet<_>>();
        for message in messages.iter().filter(|message| message["role"] == "tool") {
            assert!(tool_call_ids.contains(message["tool_call_id"].as_str().unwrap()));
        }
        let checkpoint_prompt = messages.last().unwrap()["content"].as_str().unwrap();
        assert!(
            checkpoint_prompt.contains("workspace mutation succeeded in this provider run: true")
        );
        assert!(checkpoint_prompt.contains("backend/src/api/agent_tools.rs"));
        assert!(checkpoint_prompt.contains("tools declared for the next request: read_file"));
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        assert_eq!(body["options"]["num_ctx"], checkpoint.final_num_ctx);
        assert!(checkpoint.final_num_ctx > checkpoint.after_tokens);
        assert!(checkpoint.final_num_ctx <= 65_536);
    }

    #[test]
    fn finalization_catalogue_keeps_cas_and_delivery_tools_only() {
        let tool = |name: &str| {
            serde_json::json!({
                "type": "function",
                "function": { "name": name, "parameters": { "type": "object" } },
            })
        };
        let mut body = serde_json::json!({
            "tools": [
                tool("search_text"),
                tool("read_file"),
                tool("edit_lines"),
                tool("insert_after_line"),
                tool("git_status"),
                tool("git_diff"),
                tool("git_commit"),
                tool("task_exec_deliver"),
                tool("api_call"),
            ]
        });

        retain_worker_finalization_tools(&mut body);

        let names = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item.pointer("/function/name")?.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "read_file",
                "edit_lines",
                "insert_after_line",
                "git_status",
                "git_diff",
                "git_commit",
                "task_exec_deliver",
            ]
        );
        assert!(
            names.contains(&"read_file"),
            "CAS refresh must remain possible"
        );
    }

    #[test]
    fn delivery_catalogue_keeps_only_manifest_tool() {
        let tool = |name: &str| {
            serde_json::json!({
                "type": "function",
                "function": { "name": name, "parameters": { "type": "object" } },
            })
        };
        let mut body = serde_json::json!({
            "tools": [
                tool("read_file"),
                tool("edit_lines"),
                tool("git_status"),
                tool("git_diff"),
                tool("git_commit"),
                tool("task_exec_deliver"),
            ]
        });
        let original = body["tools"].as_array().unwrap().clone();

        set_worker_tools_from_catalogue(&mut body, &original, &["task_exec_deliver"]);

        let names = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item.pointer("/function/name")?.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["task_exec_deliver"]);
    }

    #[test]
    fn mlx_model_tag_detection_is_case_insensitive_and_scoped_to_the_tag() {
        assert!(ollama_model_has_mlx_tag("qwen3.8:27b-mlx"));
        assert!(ollama_model_has_mlx_tag("gemma4:12B-MLX-Q4"));
        assert!(ollama_model_has_mlx_tag("qwen3.8:mlx"));

        assert!(!ollama_model_has_mlx_tag("mlx-community/qwen3.8:27b"));
        assert!(!ollama_model_has_mlx_tag("qwen3.8:27b-xmlx"));
        assert!(!ollama_model_has_mlx_tag("qwen3.8:27b"));
    }

    #[test]
    fn an_ollama_version_is_read_or_left_unanswered() {
        assert_eq!(parse_ollama_version("0.34.2"), Some((0, 34)));
        assert_eq!(parse_ollama_version("v0.32.14"), Some((0, 32)));
        assert_eq!(parse_ollama_version(" 1.0.0 "), Some((1, 0)));
        // No answer rather than a guess: the mitigation stays on when the
        // server does not say what it is.
        assert_eq!(parse_ollama_version("nightly"), None);
        assert_eq!(parse_ollama_version("0"), None);
    }

    /// The shortened exploration exists for one bug, fixed in 0.34. A server
    /// that no longer has it must not keep paying for the workaround, and a
    /// server that will not say its version keeps it.
    #[test]
    fn the_mlx_exploration_mitigation_stops_at_the_version_that_fixed_it() {
        assert!(!mlx_prefix_cache_reused(None));
        assert!(!mlx_prefix_cache_reused(Some((0, 32))));
        assert!(!mlx_prefix_cache_reused(Some((0, 33))));
        assert!(mlx_prefix_cache_reused(Some((0, 34))));
        assert!(mlx_prefix_cache_reused(Some((1, 0))));

        let affected = worker_exploration_policy("alias:latest", Some("safetensors"), false, false);
        let fixed = worker_exploration_policy("alias:latest", Some("safetensors"), false, true);
        // Both are native MLX: the window cap is a memory bound and stays.
        assert!(affected.mlx_mitigation && fixed.mlx_mitigation);
        assert!(
            fixed.max_iterations > affected.max_iterations,
            "a fixed server must explore as long as any other: {} vs {}",
            fixed.max_iterations,
            affected.max_iterations
        );
        assert_eq!(fixed.max_iterations, WORKER_EXPLORATION_ROUNDS);
        assert!(fixed.max_observations_without_mutation.is_none());
        assert!(affected.max_observations_without_mutation.is_some());
    }

    #[test]
    fn worker_exploration_policy_mitigates_only_native_ollama_mlx() {
        assert_eq!(
            worker_exploration_policy("qwen3.8:27b-mlx", None, false, false),
            WorkerExplorationPolicy {
                max_iterations: MLX_WORKER_EXPLORATION_ITERATIONS,
                max_observations_without_mutation: Some(
                    MLX_WORKER_MAX_OBSERVATIONS_WITHOUT_MUTATION,
                ),
                context_pressure_percent: WORKER_CONTEXT_PRESSURE_PERCENT,
                mlx_mitigation: true,
                mlx_detection_source: Some("model_tag_fallback"),
            }
        );
        assert_eq!(
            worker_exploration_policy("friendly-alias:latest", Some("safetensors"), false, false),
            WorkerExplorationPolicy {
                max_iterations: MLX_WORKER_EXPLORATION_ITERATIONS,
                max_observations_without_mutation: Some(
                    MLX_WORKER_MAX_OBSERVATIONS_WITHOUT_MUTATION,
                ),
                context_pressure_percent: WORKER_CONTEXT_PRESSURE_PERCENT,
                mlx_mitigation: true,
                mlx_detection_source: Some("model_format"),
            },
            "the real storage format must catch aliases that hide the mlx tag"
        );
        assert_eq!(
            worker_exploration_policy("qwen3.8:27b-q4_K_M", Some("gguf"), false, false),
            WorkerExplorationPolicy {
                max_iterations: WORKER_EXPLORATION_ROUNDS,
                max_observations_without_mutation: None,
                context_pressure_percent: WORKER_CONTEXT_PRESSURE_PERCENT,
                mlx_mitigation: false,
                mlx_detection_source: None,
            }
        );
        assert_eq!(
            worker_exploration_policy("upstream:27b-mlx", Some("safetensors"), true, false),
            WorkerExplorationPolicy {
                max_iterations: WORKER_EXPLORATION_ROUNDS,
                max_observations_without_mutation: None,
                context_pressure_percent: WORKER_CONTEXT_PRESSURE_PERCENT,
                mlx_mitigation: false,
                mlx_detection_source: None,
            }
        );
    }

    #[test]
    fn mlx_worker_uses_one_bounded_context_cap_for_the_whole_run() {
        let mlx = worker_exploration_policy("alias:latest", Some("safetensors"), false, false);
        let gguf = worker_exploration_policy("alias:latest", Some("gguf"), false, false);

        assert_eq!(
            mlx.context_pressure_percent, 75,
            "the 32K slot must not be halved again by the old 65K-era pressure threshold"
        );
        assert_eq!(
            worker_context_pressure(
                &serde_json::json!({
                    "messages": [{ "role": "user", "content": "x".repeat(44_000) }]
                }),
                MLX_WORKER_EFFECTIVE_CTX_CAP,
                mlx.context_pressure_percent,
            ),
            None,
            "a roughly 16K first-turn request must retain the exploration catalogue"
        );

        assert_eq!(
            worker_effective_ctx_cap(65_536, crate::agents::tools::ToolRunMode::Worker, mlx),
            MLX_WORKER_EFFECTIVE_CTX_CAP
        );
        assert_eq!(
            worker_effective_ctx_cap(8_192, crate::agents::tools::ToolRunMode::Worker, mlx),
            8_192,
            "an operator's smaller cap remains authoritative"
        );
        assert_eq!(
            worker_effective_ctx_cap(65_536, crate::agents::tools::ToolRunMode::Worker, gguf),
            65_536,
            "GGUF keeps the configured context contract"
        );
        assert_eq!(
            worker_effective_ctx_cap(65_536, crate::agents::tools::ToolRunMode::General, mlx),
            65_536,
            "the MLX mitigation is worker-only"
        );
        assert!(
            worker_oversized_prompt_remedy(65_536, mlx).contains("GGUF/non-MLX"),
            "raising an already-clamped MLX ceiling must not be offered as a remedy"
        );
        assert!(
            worker_oversized_prompt_remedy(8_192, mlx).contains("up to 32768"),
            "an operator-selected cap below the MLX ceiling may still be raised honestly"
        );
        assert!(
            worker_oversized_prompt_remedy(65_536, gguf)
                .contains("increase the configured context cap"),
            "non-MLX workers keep the ordinary configurable-cap remedy"
        );
    }

    #[test]
    fn mlx_observation_budget_forces_finalization_without_affecting_other_engines() {
        let mlx = worker_exploration_policy("alias:latest", Some("safetensors"), false, false);
        assert_eq!(
            worker_exploration_boundary(
                mlx,
                1,
                MLX_WORKER_MAX_OBSERVATIONS_WITHOUT_MUTATION - 1,
                None,
            ),
            None
        );
        assert_eq!(
            worker_exploration_boundary(mlx, 1, MLX_WORKER_MAX_OBSERVATIONS_WITHOUT_MUTATION, None,),
            Some(WorkerExplorationBoundary::ObservationLimit)
        );

        let gguf = worker_exploration_policy("alias:latest", Some("gguf"), false, false);
        assert_eq!(
            worker_exploration_boundary(gguf, 1, usize::MAX, None),
            None,
            "a non-MLX worker keeps the existing round/context boundaries"
        );
    }

    #[test]
    fn worker_context_pressure_counts_tool_schema_and_uses_policy_ratio() {
        let messages_only = serde_json::json!({
            "messages": [{ "role": "user", "content": "x".repeat(10_000) }],
        });
        let estimated = estimated_chat_history_tokens(&messages_only);
        assert!((5_000..7_500).contains(&estimated), "estimate={estimated}");
        assert_eq!(
            worker_context_pressure(&messages_only, 10_000, 50),
            Some(estimated)
        );
        assert_eq!(worker_context_pressure(&messages_only, 10_000, 75), None);

        let schema_heavy = serde_json::json!({
            "messages": [{ "role": "user", "content": "small" }],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "large_tool",
                    "description": "y".repeat(12_000),
                    "parameters": { "type": "object" },
                },
            }],
        });
        let schema_estimate = estimated_chat_history_tokens(&schema_heavy);
        assert!(
            schema_estimate
                > estimated_chat_history_tokens(&serde_json::json!({
                    "messages": schema_heavy["messages"].clone(),
                })),
            "tool declarations must count toward prompt pressure"
        );
        assert_eq!(
            worker_context_pressure(&schema_heavy, 10_000, 50),
            Some(schema_estimate)
        );
        assert_eq!(worker_context_pressure(&schema_heavy, 0, 50), None);
        assert_eq!(worker_context_pressure(&schema_heavy, 10_000, 0), None);
    }

    /// Records what it was asked to run and replies with a canned payload, so
    /// the loop can be exercised without an AppState or a real Kronn API.
    struct FakeTools {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::agents::tools::ToolExecutor for FakeTools {
        fn catalogue(&self) -> Vec<serde_json::Value> {
            vec![serde_json::json!({
                "type": "function",
                "function": {
                    "name": "mcp_list",
                    "description": "List MCP servers.",
                    "parameters": { "type": "object", "properties": {}, "required": [] },
                },
            })]
        }

        async fn execute(
            &self,
            call: &crate::agents::tools::ToolCall,
        ) -> crate::agents::tools::ToolOutcome {
            self.seen.lock().unwrap().push(call.name.clone());
            crate::agents::tools::ToolOutcome {
                call: call.clone(),
                content: serde_json::json!({ "servers": ["github", "context7"] }),
                ok: true,
            }
        }
    }

    struct WorkerTools {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::agents::tools::ToolExecutor for WorkerTools {
        fn catalogue(&self) -> Vec<serde_json::Value> {
            [
                "search_text",
                "read_file",
                "edit_lines",
                "git_status",
                "git_diff",
                "git_commit",
                "task_exec_deliver",
                // A deliberately non-finalization tool makes the phase
                // transition observable in the mock request.
                "api_call",
            ]
            .into_iter()
            .map(|name| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": name,
                        "parameters": { "type": "object", "properties": {} },
                    },
                })
            })
            .collect()
        }

        fn run_mode(&self) -> crate::agents::tools::ToolRunMode {
            crate::agents::tools::ToolRunMode::Worker
        }

        async fn execute(
            &self,
            call: &crate::agents::tools::ToolCall,
        ) -> crate::agents::tools::ToolOutcome {
            self.seen.lock().unwrap().push(call.name.clone());
            if matches!(
                call.name.as_str(),
                "write_file" | "edit_file" | "edit_lines" | "insert_after_line"
            ) && call.arguments["force_fail"].as_bool() == Some(true)
            {
                return crate::agents::tools::ToolOutcome {
                    call: call.clone(),
                    content: serde_json::json!({
                        "error": "simulated stale edit receipt; re-read the exact target"
                    }),
                    ok: false,
                };
            }
            crate::agents::tools::ToolOutcome {
                call: call.clone(),
                content: serde_json::json!({
                    "tool": call.name,
                    "content_sha256": format!("sha-{}", self.seen.lock().unwrap().len()),
                    "ok": true,
                }),
                ok: true,
            }
        }
    }

    struct PrelocalizedWorkerTools {
        inner: WorkerTools,
        scope: crate::models::TaskWorkerScope,
    }

    #[async_trait::async_trait]
    impl crate::agents::tools::ToolExecutor for PrelocalizedWorkerTools {
        fn catalogue(&self) -> Vec<serde_json::Value> {
            self.inner.catalogue()
        }

        fn run_mode(&self) -> crate::agents::tools::ToolRunMode {
            crate::agents::tools::ToolRunMode::Worker
        }

        fn worker_scope(&self) -> Option<crate::models::TaskWorkerScope> {
            Some(self.scope.clone())
        }

        async fn execute(
            &self,
            call: &crate::agents::tools::ToolCall,
        ) -> crate::agents::tools::ToolOutcome {
            self.inner.execute(call).await
        }
    }

    #[tokio::test]
    #[serial]
    async fn prelocalized_ollama_worker_cannot_escape_read_then_cas_edit_contract() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"details":{"format":"gguf"},"model_info":{"test.context_length":32768}}"#,
            ))
            .mount(&server)
            .await;

        let requests = std::sync::Arc::new(AtomicUsize::new(0));
        let saw_read_refusal = std::sync::Arc::new(AtomicBool::new(false));
        let saw_edit_refusal = std::sync::Arc::new(AtomicBool::new(false));
        let requests_for_mock = requests.clone();
        let read_refusal_for_mock = saw_read_refusal.clone();
        let edit_refusal_for_mock = saw_edit_refusal.clone();
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let names = body["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| tool.pointer("/function/name")?.as_str())
                    .collect::<Vec<_>>();
                let round = requests_for_mock.fetch_add(1, Ordering::SeqCst);
                let response = match round {
                    0 => {
                        assert_eq!(names, ["read_file"]);
                        assert_eq!(
                            body.pointer("/tools/0/function/parameters/properties/path/enum/0"),
                            Some(&serde_json::json!("src/lib.rs"))
                        );
                        assert_eq!(
                            body.pointer("/tools/0/function/parameters/properties/offset/enum/0"),
                            Some(&serde_json::json!(28))
                        );
                        // Deliberately try to broaden the read. The executor
                        // must not observe this call even though the tool name
                        // itself is declared.
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"read_file","arguments":{"path":"src/lib.rs","offset":1,"limit":200}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    1 => {
                        assert_eq!(names, ["read_file"]);
                        read_refusal_for_mock.store(
                            body.to_string().contains("prelocalized_scope_mismatch"),
                            Ordering::SeqCst,
                        );
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"read_file","arguments":{"path":"src/lib.rs","offset":28,"limit":29}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    2 => {
                        assert_eq!(names, ["edit_lines"]);
                        assert_eq!(
                            body.pointer("/tools/0/function/parameters/properties/path/enum/0"),
                            Some(&serde_json::json!("src/lib.rs"))
                        );
                        assert_eq!(
                            body.pointer("/tools/0/function/parameters/properties/expected_sha256/enum/0"),
                            Some(&serde_json::json!("sha-1"))
                        );
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"edit_lines","arguments":{"path":"src/lib.rs","start_line":39,"end_line":44,"new_string":"wrong","expected_sha256":"sha-1"}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    3 => {
                        assert_eq!(names, ["edit_lines"]);
                        edit_refusal_for_mock.store(
                            body.to_string().contains("prelocalized_scope_mismatch"),
                            Ordering::SeqCst,
                        );
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"edit_lines","arguments":{"path":"src/lib.rs","start_line":40,"end_line":44,"new_string":"correct","expected_sha256":"sha-1"}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    4 => {
                        assert_eq!(names, ["git_commit"]);
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"git_commit","arguments":{}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    5 => {
                        assert_eq!(names, ["task_exec_deliver"]);
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"task_exec_deliver","arguments":{}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    other => panic!("unexpected prelocalized Ollama round {other}"),
                };
                ResponseTemplate::new(200).set_body_string(response)
            })
            .mount(&server)
            .await;

        let base_url = server.uri();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "perform the exact tiny edit",
            "",
            "test-model",
            None,
            Some(&base_url),
            None,
            Some(std::sync::Arc::new(PrelocalizedWorkerTools {
                inner: WorkerTools { seen: seen.clone() },
                scope: crate::models::TaskWorkerScope::PrelocalizedEdit {
                    path: "src/lib.rs".into(),
                    start_line: 40,
                    end_line: 44,
                },
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

        let mut process = started.expect("start scoped Ollama worker");
        while process.next_line().await.is_some() {}
        let status = process.child.wait().await.expect("lifeline");
        assert!(status.success(), "strict worker must commit and deliver");
        assert_eq!(requests.load(Ordering::SeqCst), 6);
        assert!(saw_read_refusal.load(Ordering::SeqCst));
        assert!(saw_edit_refusal.load(Ordering::SeqCst));
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            ["read_file", "edit_lines", "git_commit", "task_exec_deliver"],
            "escaped read/edit calls must be refused before the executor"
        );
        let captured = process.stderr_capture.lock().unwrap().clone();
        let telemetry = parse_http_turn_telemetry(&captured);
        assert_eq!(telemetry.len(), 6);
        assert_eq!(
            telemetry.iter().map(|turn| turn.phase).collect::<Vec<_>>(),
            [
                crate::models::TaskExecutionHttpPhase::Read,
                crate::models::TaskExecutionHttpPhase::Read,
                crate::models::TaskExecutionHttpPhase::Mutation,
                crate::models::TaskExecutionHttpPhase::Mutation,
                crate::models::TaskExecutionHttpPhase::Commit,
                crate::models::TaskExecutionHttpPhase::Delivery,
            ]
        );
        assert_eq!(telemetry[1].executed_tools[0].name, "read_file");
        assert_eq!(telemetry[3].executed_tools[0].name, "edit_lines");
        assert_eq!(telemetry[4].executed_tools[0].name, "git_commit");
        assert_eq!(telemetry[5].executed_tools[0].name, "task_exec_deliver");
    }

    #[tokio::test]
    #[serial]
    async fn prelocalized_ollama_read_loop_fails_after_one_correction_without_mutation() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"details":{"format":"gguf"},"model_info":{"test.context_length":32768}}"#,
            ))
            .mount(&server)
            .await;
        let requests = std::sync::Arc::new(AtomicUsize::new(0));
        let requests_for_mock = requests.clone();
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let names = body["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| tool.pointer("/function/name")?.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(names, ["read_file"]);
                let response = match requests_for_mock.fetch_add(1, Ordering::SeqCst) {
                    0 => {
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"read_file","arguments":{"path":"src/lib.rs","offset":29,"limit":29}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    1 => {
                        assert!(body.to_string().contains("prelocalized_scope_mismatch"));
                        r#"{"message":{"content":"I need to inspect another nearby slice first."},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    other => panic!("unexpected loop round {other}"),
                };
                ResponseTemplate::new(200).set_body_string(response)
            })
            .mount(&server)
            .await;

        let base_url = server.uri();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "perform the exact tiny edit",
            "",
            "test-model",
            None,
            Some(&base_url),
            None,
            Some(std::sync::Arc::new(PrelocalizedWorkerTools {
                inner: WorkerTools { seen: seen.clone() },
                scope: crate::models::TaskWorkerScope::PrelocalizedEdit {
                    path: "src/lib.rs".into(),
                    start_line: 40,
                    end_line: 44,
                },
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

        let mut process = started.expect("start scoped Ollama worker");
        while process.next_line().await.is_some() {}
        let status = process.child.wait().await.expect("lifeline");
        assert!(!status.success(), "the bounded read loop must fail visibly");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert!(
            seen.lock().unwrap().is_empty(),
            "neither the escaped read nor prose may reach a mutating executor"
        );
        let captured = process.stderr_capture.lock().unwrap().join(" ");
        assert!(
            captured.contains("reason_code=prelocalized_read_exhausted"),
            "the terminal reason must be stable and machine-readable: {captured}"
        );
    }

    #[tokio::test]
    async fn worker_never_executes_a_tool_absent_from_the_declared_catalogue() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let requests = std::sync::Arc::new(AtomicUsize::new(0));
        let requests_for_mock = requests.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |_: &wiremock::Request| {
                let response = if requests_for_mock.fetch_add(1, Ordering::SeqCst) == 0 {
                    sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"hallucinated-plan-write","function":{"name":"task_create","arguments":"{}"}}]}}]}"#,
                    ])
                } else {
                    sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"The unavailable planning tool was not executed."}}]}"#,
                    ])
                };
                ResponseTemplate::new(200).set_body_string(response)
            })
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "complete the worker task",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(WorkerTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        let mut out = String::new();
        while let Some(line) = process.next_line().await {
            out.push_str(&line);
        }
        let status = process.child.wait().await.expect("lifeline");

        assert!(
            !status.success(),
            "a worker that neither executes a valid tool nor delivers must fail"
        );
        assert!(out.contains("was not executed"));
        assert!(
            seen.lock().unwrap().is_empty(),
            "an undeclared governance tool must never reach the executor"
        );
        let captured = process.stderr_capture.lock().unwrap().join(" ");
        assert!(
            captured.contains("refused undeclared tool `task_create`"),
            "the fail-closed decision must be observable: {captured}"
        );
    }

    /// KT-403 — the consumer IS the run. Measured before this existed: the
    /// stream timed out, streaming.rs dropped the receiver and moved on, and
    /// the tool loop kept executing tools for another 40+ minutes while the
    /// watchdog requeued a SECOND worker onto the same worktree.
    #[tokio::test]
    async fn an_abandoned_run_stops_calling_tools() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Never converges: a fresh tool call with fresh arguments every round.
        let round = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let round_for_mock = round.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |_: &wiremock::Request| {
                let n = round_for_mock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_string(sse(&[&format!(
                    r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"c{n}","function":{{"name":"mcp_list","arguments":"{{\"probe\":{n}}}"}}}}]}}}}]}}"#
                )]))
            })
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let process = start_ollama_http(
            &AgentType::LiteLlm,
            "loop forever",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(FakeTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        // The consumer walks away without reading a single line — the shape of
        // a timeout teardown.
        drop(process);

        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let calls = seen.lock().unwrap().len();
        assert!(
            calls <= 3,
            "an abandoned loop must stop at the next round boundary, not run to \
             its budget: {calls} tool calls after the drop"
        );
    }

    /// The first HTTP request is awaited before `AgentProcess` exists, so a
    /// pid-only cancellation cannot reach it. The caller token must interrupt
    /// that cold-load/header wait directly; otherwise Stop can hang for the
    /// whole four-hour local budget.
    #[tokio::test]
    async fn cancelling_interrupts_the_initial_request_before_a_process_exists() {
        use tokio::sync::Notify;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let request_seen = std::sync::Arc::new(Notify::new());
        let seen_for_mock = request_seen.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |_: &wiremock::Request| {
                seen_for_mock.notify_one();
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(60))
                    .set_body_string(sse(&[r#"{"choices":[{"delta":{"content":"late"}}]}"#]))
            })
            .mount(&server)
            .await;

        let parent_cancel = tokio_util::sync::CancellationToken::new();
        let base = server.uri();
        let starting = start_ollama_http(
            &AgentType::LiteLlm,
            "wait for headers",
            "",
            "test-model",
            None,
            Some(&base),
            None,
            None,
            None,
            Some(std::time::Duration::from_secs(240 * 60)),
            Some(&parent_cancel),
            None,
            None,
            None,
        );
        tokio::pin!(starting);

        tokio::select! {
            result = &mut starting => panic!("request completed before cancellation: {}", result.is_ok()),
            _ = request_seen.notified() => parent_cancel.cancel(),
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), starting)
            .await
            .expect("cancellation must not wait for the provider delay");
        let error = match result {
            Ok(_) => panic!("cancellation before headers must not return a process"),
            Err(error) => error,
        };
        assert!(error.contains("cancelled before"), "{error}");
    }

    /// Killing the HTTP lifeline used to leave the real Tokio tool loop alive.
    /// This pins the stronger contract without a timing sleep: cancellation
    /// must DROP the in-flight tool future and must not begin the second tool
    /// that the provider already placed in the same response batch.
    #[tokio::test]
    async fn cancelling_drops_the_in_flight_tool_and_skips_the_rest_of_its_batch() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use tokio::sync::Notify;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        struct DropSignal {
            dropped: std::sync::Arc<AtomicBool>,
            notify: std::sync::Arc<Notify>,
        }
        impl Drop for DropSignal {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::SeqCst);
                self.notify.notify_waiters();
            }
        }

        struct BlockingTools {
            first_started: std::sync::Arc<Notify>,
            first_dropped: std::sync::Arc<AtomicBool>,
            drop_notify: std::sync::Arc<Notify>,
            second_started: std::sync::Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl crate::agents::tools::ToolExecutor for BlockingTools {
            fn catalogue(&self) -> Vec<serde_json::Value> {
                ["first", "second"]
                    .into_iter()
                    .map(|name| {
                        serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": name,
                                "description": "Cancellation test tool.",
                                "parameters": { "type": "object", "properties": {} },
                            },
                        })
                    })
                    .collect()
            }

            async fn execute(
                &self,
                call: &crate::agents::tools::ToolCall,
            ) -> crate::agents::tools::ToolOutcome {
                if call.name == "first" {
                    let _drop_signal = DropSignal {
                        dropped: self.first_dropped.clone(),
                        notify: self.drop_notify.clone(),
                    };
                    // `notify_one` retains a permit if the background task wins
                    // the race and starts before the test begins awaiting.
                    self.first_started.notify_one();
                    std::future::pending::<()>().await;
                    unreachable!("the first tool only ends when cancellation drops its future");
                }
                self.second_started.fetch_add(1, Ordering::SeqCst);
                crate::agents::tools::ToolOutcome {
                    call: call.clone(),
                    content: serde_json::json!({"unexpected": true}),
                    ok: true,
                }
            }
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"first-1","function":{"name":"first","arguments":"{}"}},{"index":1,"id":"second-1","function":{"name":"second","arguments":"{}"}}]}}]}"#,
            ])))
            .mount(&server)
            .await;

        let first_started = std::sync::Arc::new(Notify::new());
        let first_dropped = std::sync::Arc::new(AtomicBool::new(false));
        let drop_notify = std::sync::Arc::new(Notify::new());
        let second_started = std::sync::Arc::new(AtomicUsize::new(0));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "run both tools",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(BlockingTools {
                first_started: first_started.clone(),
                first_dropped: first_dropped.clone(),
                drop_notify: drop_notify.clone(),
                second_started: second_started.clone(),
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        tokio::time::timeout(std::time::Duration::from_secs(2), first_started.notified())
            .await
            .expect("the first tool must start");
        AgentIo::kill(&mut process).await;
        if !first_dropped.load(Ordering::SeqCst) {
            tokio::time::timeout(std::time::Duration::from_secs(2), drop_notify.notified())
                .await
                .expect("cancellation must drop the in-flight tool future");
        }
        assert!(first_dropped.load(Ordering::SeqCst));
        assert_eq!(
            second_started.load(Ordering::SeqCst),
            0,
            "no later effect in the provider's already-decoded batch may start"
        );
    }

    /// Two tools: one the model will overspend, one it needs afterwards.
    struct ReadThenWriteTools {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::agents::tools::ToolExecutor for ReadThenWriteTools {
        fn catalogue(&self) -> Vec<serde_json::Value> {
            ["read_file", "write_file"]
                .into_iter()
                .map(|name| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": name,
                            "description": format!("Test tool {name}"),
                            "parameters": {
                                "type": "object",
                                "properties": { "path": { "type": "string" } },
                                "required": [],
                            },
                        },
                    })
                })
                .collect()
        }

        async fn execute(
            &self,
            call: &crate::agents::tools::ToolCall,
        ) -> crate::agents::tools::ToolOutcome {
            self.seen.lock().unwrap().push(call.name.clone());
            crate::agents::tools::ToolOutcome {
                call: call.clone(),
                content: serde_json::json!({ "text": format!("body of {}", call.arguments) }),
                ok: true,
            }
        }
    }

    /// Overspending ONE budget used to disarm the whole toolbox. Measured on a
    /// real delegation: the worker paged a large file until its read budget was
    /// gone, lost `write_file` with it, and could only describe the fix it had
    /// correctly worked out. Withdrawing the offending tool is what stops the
    /// loop; the rest must survive so the task can still be finished.
    #[tokio::test]
    async fn exhausting_one_budget_leaves_the_other_tools_usable() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let round = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let round_for_mock = round.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let n = round_for_mock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // Only ask for a tool Kronn is still offering. A mock that calls
                // whatever it likes would pass this test without the fix, because
                // the withdrawal it is meant to prove lives in the REQUEST.
                let body: serde_json::Value =
                    serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null);
                let declared = |name: &str| {
                    body["tools"]
                        .as_array()
                        .is_some_and(|tools| {
                            tools.iter().any(|tool| tool["function"]["name"] == name)
                        })
                };
                // Page a file past its budget, then do the work — the shape the
                // real worker had, minus the part where it could not.
                let call = if declared("read_file") {
                    Some(("read_file", format!(r#"{{\"path\":\"big.rs\",\"offset\":{n}}}"#)))
                } else if declared("write_file") {
                    Some(("write_file", r#"{\"path\":\"fix.rs\"}"#.to_string()))
                } else {
                    None
                };
                match call {
                    Some((tool, args)) => ResponseTemplate::new(200).set_body_string(sse(&[&format!(
                        r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"c{n}","function":{{"name":"{tool}","arguments":"{args}"}}}}]}}}}]}}"#
                    )])),
                    None => ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"done"}}]}"#,
                    ])),
                }
            })
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "page then write",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(ReadThenWriteTools {
                seen: seen.clone(),
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        while process.next_line().await.is_some() {}
        process.child.wait().await.expect("lifeline");

        let calls = seen.lock().unwrap().clone();
        let reads = calls.iter().filter(|name| *name == "read_file").count();
        assert_eq!(
            reads,
            crate::agents::runner::MAX_READ_FILE_CALLS,
            "the overspent tool is still capped: {calls:?}"
        );
        assert!(
            calls.iter().any(|name| name == "write_file"),
            "the tool the worker still needed must survive the other's ceiling: {calls:?}"
        );
    }

    struct ConvergenceTools {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        failing: std::collections::HashSet<String>,
    }

    #[async_trait::async_trait]
    impl crate::agents::tools::ToolExecutor for ConvergenceTools {
        fn catalogue(&self) -> Vec<serde_json::Value> {
            ["seed_evidence", "find_files", "list_files"]
                .into_iter()
                .map(|name| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": name,
                            "description": format!("Test tool {name}"),
                            "parameters": { "type": "object", "properties": {} },
                        },
                    })
                })
                .collect()
        }

        async fn execute(
            &self,
            call: &crate::agents::tools::ToolCall,
        ) -> crate::agents::tools::ToolOutcome {
            self.seen.lock().unwrap().push(call.name.clone());
            let ok = !self.failing.contains(&call.name);
            crate::agents::tools::ToolOutcome {
                call: call.clone(),
                content: if ok {
                    serde_json::json!({ "evidence": format!("result from {}", call.name) })
                } else {
                    serde_json::json!({
                        "error": format!("{} failed for {}", call.name, call.arguments),
                    })
                },
                ok,
            }
        }
    }

    /// Simulates normal repository discovery: two absent candidate paths, one
    /// useful file, then two more absent candidates. The success must reset the
    /// circuit streak even though the lifetime error count keeps increasing.
    struct IntermittentReadTools {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::agents::tools::ToolExecutor for IntermittentReadTools {
        fn catalogue(&self) -> Vec<serde_json::Value> {
            vec![serde_json::json!({
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read a candidate repository file.",
                    "parameters": { "type": "object", "properties": {} },
                },
            })]
        }

        async fn execute(
            &self,
            call: &crate::agents::tools::ToolCall,
        ) -> crate::agents::tools::ToolOutcome {
            let attempt = {
                let mut seen = self.seen.lock().unwrap();
                seen.push(
                    call.arguments["path"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
                seen.len()
            };
            let ok = attempt == 3;
            crate::agents::tools::ToolOutcome {
                call: call.clone(),
                content: if ok {
                    serde_json::json!({ "content": "useful configuration" })
                } else {
                    serde_json::json!({ "error": "file not found" })
                },
                ok,
            }
        }
    }

    fn sse(frames: &[&str]) -> String {
        frames
            .iter()
            .map(|f| format!("data: {f}\n\n"))
            .collect::<String>()
            + "data: [DONE]\n\n"
    }

    #[tokio::test]
    async fn structured_output_refusal_falls_back_without_losing_tools_on_both_wires() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        for agent in [AgentType::Ollama, AgentType::LiteLlm] {
            let ollama = agent == AgentType::Ollama;
            let endpoint = if ollama {
                "/api/chat"
            } else {
                "/v1/chat/completions"
            };
            let format_key = if ollama { "format" } else { "response_format" };
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/api/show"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path(endpoint))
                .respond_with(move |request: &wiremock::Request| {
                    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                    if body.get(format_key).is_some() {
                        return if ollama {
                            ResponseTemplate::new(501).set_body_json(serde_json::json!({"error":"structured output is unavailable"}))
                        } else {
                            ResponseTemplate::new(400).set_body_json(serde_json::json!({"error":{"message":"This model does not support response_format"}}))
                        };
                    }
                    let after_tool = body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool");
                    let content = if after_tool { r#"{"data":{"ok":true},"status":"OK"}"# } else { "" };
                    let mut message = serde_json::json!({"role":"assistant","content":content});
                    if !after_tool {
                        message["tool_calls"] = serde_json::json!([{
                            "id":"probe-1", "type":"function",
                            "function":{"name":"mcp_list","arguments":if ollama { serde_json::json!({}) } else { serde_json::json!("{}") }}
                        }]);
                    }
                    ResponseTemplate::new(200).set_body_json(if ollama {
                        serde_json::json!({"message":message,"done":true,"prompt_eval_count":5,"eval_count":2})
                    } else {
                        serde_json::json!({"choices":[{"message":message,"finish_reason":if after_tool { "stop" } else { "tool_calls" }}],"usage":{"prompt_tokens":5,"completion_tokens":2}})
                    })
                })
                .expect(3)
                .mount(&server).await;
            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let schema = serde_json::json!({"type":"object","properties":{"data":{"type":"object"},"status":{"type":"string"}}});
            let mut process = start_ollama_http(
                &agent,
                "Call mcp_list, then return data.ok and status as JSON.",
                "",
                "test-model",
                Some(&schema),
                Some(&server.uri()),
                None,
                Some(std::sync::Arc::new(FakeTools { seen: seen.clone() })),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("an explicit unsupported format must recover before any tool runs");
            let mut output = String::new();
            while let Some(chunk) = process.next_line().await {
                output.push_str(&chunk);
            }
            assert!(process.child.wait().await.unwrap().success(), "{output}");
            assert!(
                output.contains("[structured-output fallback:"),
                "operator notice missing: {output}"
            );
            let envelope = crate::workflows::template::extract_step_envelope(&output)
                .expect("notice must not hide JSON");
            crate::workflows::template::validate_envelope_against_schema(
                &envelope.data_json, &serde_json::json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"]}),
            ).expect("local schema validation remains applicable");
            assert_eq!(*seen.lock().unwrap(), ["mcp_list"]);
            let bodies: Vec<serde_json::Value> = server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|r| r.url.path() == endpoint)
                .map(|r| serde_json::from_slice(&r.body).unwrap())
                .collect();
            assert_eq!(bodies.len(), 3);
            let mut expected = bodies[0].clone();
            expected.as_object_mut().unwrap().remove(format_key);
            assert_eq!(
                bodies[1], expected,
                "only the unsupported format may change"
            );
            assert!(
                bodies[2].get(format_key).is_none(),
                "format must stay removed after tool execution"
            );
            assert_eq!(bodies[2]["tools"], bodies[0]["tools"]);
        }
    }

    #[tokio::test]
    async fn structured_output_fallback_is_bounded_and_does_not_mask_other_errors() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        for (status, detail, expected_calls) in [
            (501, "structured output is unavailable", 2),
            (501, "endpoint not implemented", 1),
            (
                400,
                "Invalid schema for response_format: property maxItems is not supported",
                1,
            ),
            (401, "structured output is unavailable: invalid API key", 1),
            (429, "insufficient_quota", 1),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/chat/completions"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .set_body_json(serde_json::json!({"error":{"message":detail}})),
                )
                .expect(expected_calls)
                .mount(&server)
                .await;
            let schema = serde_json::json!({"type":"object"});
            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let error = match start_ollama_http(
                &AgentType::LiteLlm,
                "hello",
                "",
                "test-model",
                Some(&schema),
                Some(&server.uri()),
                None,
                Some(std::sync::Arc::new(FakeTools { seen: seen.clone() })),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            {
                Ok(_) => panic!("a persistent rejection must stay failed"),
                Err(error) => error,
            };
            assert!(
                error.contains(detail),
                "original diagnostic missing: {error}"
            );
            assert!(
                !error.contains("may not support tool calling"),
                "false tool attribution: {error}"
            );
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                expected_calls as usize
            );
            assert!(seen.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn structured_output_fallback_can_be_cancelled_before_headers() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let fallback_seen = std::sync::Arc::new(tokio::sync::Notify::new());
        let mock_seen = fallback_seen.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                if body.get("response_format").is_some() {
                    ResponseTemplate::new(501).set_body_string("structured output is unavailable")
                } else {
                    mock_seen.notify_one();
                    ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(60))
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let schema = serde_json::json!({"type":"object"});
        let base = server.uri();
        let starting = start_ollama_http(
            &AgentType::LiteLlm,
            "hello",
            "",
            "test-model",
            Some(&schema),
            Some(&base),
            None,
            None,
            None,
            None,
            Some(&cancel),
            None,
            None,
            None,
        );
        tokio::pin!(starting);
        tokio::select! {
            result = &mut starting => panic!("fallback finished before cancellation: {}", result.is_ok()),
            _ = fallback_seen.notified() => cancel.cancel(),
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), starting)
            .await
            .expect("fallback cancellation must remain responsive");
        match result {
            Err(error) => assert!(error.contains("cancelled before"), "{error}"),
            Ok(_) => panic!("cancelled fallback must not start an agent"),
        }
    }

    #[test]
    fn provider_retry_classifier_separates_capacity_from_permanent_failures() {
        assert!(!is_transient_provider_failure(
            Some(reqwest::StatusCode::NOT_IMPLEMENTED),
            "structured output is unavailable"
        ));
        assert!(is_transient_provider_failure(
            Some(reqwest::StatusCode::SERVICE_UNAVAILABLE),
            "upstream unavailable"
        ));
        assert!(is_transient_provider_failure(
            Some(reqwest::StatusCode::GATEWAY_TIMEOUT),
            "gateway timeout"
        ));
        assert!(is_transient_provider_failure(
            None,
            "ResourceExhausted: Worker local total request limit reached (20/16)"
        ));
        assert!(!is_transient_provider_failure(
            Some(reqwest::StatusCode::TOO_MANY_REQUESTS),
            "insufficient_quota: quota exhausted"
        ));
        assert!(!is_transient_provider_failure(
            Some(reqwest::StatusCode::UNAUTHORIZED),
            "invalid API key"
        ));
        assert!(!is_transient_provider_failure(
            Some(reqwest::StatusCode::NOT_FOUND),
            "model not found"
        ));
        assert!(!is_transient_provider_failure(
            None,
            "ResourceExhausted: account quota exhausted"
        ));
    }

    #[test]
    fn structured_output_detection_requires_an_explicit_error_message() {
        for (status, detail, expected) in [
            (
                422,
                r#"{"error":{"message":"structured outputs are not supported"}}"#,
                true,
            ),
            (
                400,
                r#"{"error":{"message":"Invalid schema for response_format: additionalProperties must be false"}}"#,
                false,
            ),
            (
                501,
                r#"{"request":{"prompt":"structured output is unavailable"}}"#,
                false,
            ),
            (
                500,
                r#"{"error":"structured output is unavailable"}"#,
                false,
            ),
            (
                400,
                r#"{"error":"Invalid schema","request":{"prompt":"does not support json_schema"}}"#,
                false,
            ),
        ] {
            let failure = HttpProviderFailure {
                status: Some(reqwest::StatusCode::from_u16(status).unwrap()),
                detail: detail.into(),
                attempts: 1,
            };
            assert_eq!(rejects_structured_output(&failure), expected, "{detail}");
        }
    }

    #[tokio::test]
    async fn configured_litellm_endpoint_is_used_by_agent_start_config() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"index":0,"delta":{"content":"bonjour"}}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":2,"completion_tokens":1}}"#,
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let tokens = crate::models::TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: Vec::new(),
            disabled_overrides: Vec::new(),
        };
        let mut tiers = crate::models::ModelTiersConfig::default();
        tiers.lite_llm.default = Some("corp-default".into());
        let endpoints = crate::models::setup::HttpEndpoints {
            lite_llm: Some(server.uri()),
            nvidia: None,
        };
        let mut process = start_agent_with_config(AgentStartConfig {
            tier: crate::models::ModelTier::Default,
            model_tiers: Some(&tiers),
            http_endpoints: Some(&endpoints),
            ..AgentStartConfig::new(&AgentType::LiteLlm, "", "hello", &tokens)
        })
        .await
        .expect("configured corporate proxy should be reachable");

        let mut output = String::new();
        while let Some(chunk) = process.next_line().await {
            output.push_str(&chunk);
        }
        assert_eq!(output, "bonjour");
        assert!(process.child.wait().await.expect("lifeline").success());
        let requests = server.received_requests().await.expect("request capture");
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
        assert_eq!(body["model"], "corp-default");
        assert_eq!(body["stream"], true);
    }

    #[tokio::test]
    async fn a_later_reported_zero_replaces_the_cached_count_and_absence_keeps_it() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[],"usage":{"prompt_tokens":1000,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":800}}}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":1000,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":0}}}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":1000,"completion_tokens":1}}"#,
                r#"{"choices":[{"index":0,"delta":{"content":"ok"}}]}"#,
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "hello",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("mock proxy reachable");
        while process.next_line().await.is_some() {}
        assert!(process.child.wait().await.expect("lifeline").success());
        let captured = process.stderr_capture.lock().unwrap().clone();
        let turns = parse_http_turn_telemetry(&captured);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].cached_prompt_tokens, Some(0));
    }

    #[tokio::test]
    async fn cached_prompt_tokens_reach_the_turn_telemetry() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"index":0,"delta":{"content":"ok"}}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":1000,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":800}}}"#,
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "hello",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("mock proxy reachable");
        while process.next_line().await.is_some() {}
        assert!(process.child.wait().await.expect("lifeline").success());
        let captured = process.stderr_capture.lock().unwrap().clone();
        let turns = parse_http_turn_telemetry(&captured);
        assert_eq!(turns.len(), 1);
        assert_eq!(
            (turns[0].prompt_tokens, turns[0].cached_prompt_tokens),
            (1000, Some(800))
        );
    }

    /// The regression this pins (KT-337): the NVIDIA endpoint slot was declared
    /// on the spawn config and read by the runner, but written by no call site.
    /// A saved endpoint was therefore ignored and every run went to the public
    /// hosted service — which looked healthy, because the default happens to
    /// work. Before the slots were carried as one value, this test could not
    /// pass: the mock server would never have been contacted.
    #[tokio::test]
    async fn configured_nvidia_endpoint_is_used_and_never_the_litellm_proxy() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let nvidia = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"index":0,"delta":{"content":"salut"}}]}"#,
            ])))
            .expect(1)
            .mount(&nvidia)
            .await;

        // A LiteLLM proxy is configured at the same time and must receive
        // nothing: the providers' slots are not interchangeable.
        let proxy = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&proxy)
            .await;

        let tokens = crate::models::TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: Vec::new(),
            disabled_overrides: Vec::new(),
        };
        let mut tiers = crate::models::ModelTiersConfig::default();
        tiers.nvidia.default = Some("meta/llama-3.3-70b-instruct".into());
        tiers.lite_llm.default = Some("corp-default".into());
        let endpoints = crate::models::setup::HttpEndpoints {
            lite_llm: Some(proxy.uri()),
            nvidia: Some(nvidia.uri()),
        };
        let mut process = start_agent_with_config(AgentStartConfig {
            tier: crate::models::ModelTier::Default,
            model_tiers: Some(&tiers),
            http_endpoints: Some(&endpoints),
            ..AgentStartConfig::new(&AgentType::Nvidia, "", "hello", &tokens)
        })
        .await
        .expect("a configured NVIDIA endpoint should be reachable");

        let mut output = String::new();
        while let Some(chunk) = process.next_line().await {
            output.push_str(&chunk);
        }
        assert_eq!(output, "salut");

        let requests = nvidia.received_requests().await.expect("request capture");
        assert_eq!(
            requests.len(),
            1,
            "the run must reach the configured NVIDIA endpoint, not the hosted default"
        );
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
        // The tier resolves from NVIDIA's own block, not LiteLLM's.
        assert_eq!(body["model"], "meta/llama-3.3-70b-instruct");
        assert!(
            proxy
                .received_requests()
                .await
                .expect("request capture")
                .is_empty(),
            "the LiteLLM proxy must never serve an NVIDIA run"
        );
    }

    #[tokio::test]
    async fn named_external_connection_uses_its_endpoint_key_and_model() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer router-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"index":0,"delta":{"content":"OpenRouter OK"}}]}"#,
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let tokens = crate::models::TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: Vec::new(),
            disabled_overrides: Vec::new(),
        };
        let runtime = ExternalHttpRuntime {
            display_name: "OpenRouter".into(),
            mention_alias: "openrouter".into(),
            endpoint: server.uri(),
            api_key: Some("router-secret".into()),
        };
        let mut process = start_agent_with_config(AgentStartConfig {
            external_http: Some(&runtime),
            model_override: Some("z-ai/glm-5.3"),
            ..AgentStartConfig::new(&AgentType::Custom, "", "hello", &tokens)
        })
        .await
        .expect("named OpenAI-compatible connection should be executable");

        let mut output = String::new();
        while let Some(chunk) = process.next_line().await {
            output.push_str(&chunk);
        }
        assert_eq!(output, "OpenRouter OK");
        let requests = server.received_requests().await.expect("request capture");
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
        assert_eq!(body["model"], "z-ai/glm-5.3");
        assert!(body["messages"][0]["content"]
            .as_str()
            .is_some_and(|context| context.contains("@openrouter")));
    }

    /// The whole point of the feature: a model that asks for a tool gets the
    /// result and answers from it, without the caller doing anything.
    #[tokio::test]
    async fn tool_loop_executes_then_feeds_the_result_back_for_a_second_turn() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Turn 1 — the model asks for `mcp_list` and says nothing else.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"mcp_list","arguments":"{}"}}]}}]}"#,
            ])))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Turn 2 — having the result, it answers.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse(&[
                r#"{"choices":[{"index":0,"delta":{"content":"2 servers"}}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":4}}"#,
            ])))
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "which MCP servers are there?",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(FakeTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        let mut out = String::new();
        while let Some(chunk) = process.next_line().await {
            out.push_str(&chunk);
        }

        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &["mcp_list".to_string()],
            "the tool must be executed exactly once"
        );
        assert!(
            out.contains("2 servers"),
            "final answer not streamed: {out:?}"
        );
        assert!(
            !out.contains("call_1"),
            "tool plumbing must not leak into the reply: {out:?}"
        );
    }

    #[tokio::test]
    async fn provider_rejecting_declared_tools_returns_an_actionable_error() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_string_contains("\"tools\""))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_string(r#"{"error":{"message":"tools are not supported"}}"#),
            )
            .expect(1)
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::LiteLlm,
            "which MCP servers are there?",
            "",
            "text-only-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(FakeTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        let error = match started {
            Ok(_) => panic!(
                "a provider that rejects the declared catalogue must fail before the step runs blind"
            ),
            Err(error) => error,
        };

        assert!(
            error.contains("400"),
            "the provider status must survive: {error}"
        );
        assert!(
            error.contains("may not support tool calling"),
            "the diagnostic must explain the capability mismatch: {error}"
        );
        assert!(
            error.contains("tool-capable model") && error.contains("ApiCall step"),
            "the operator needs both recovery paths: {error}"
        );
        assert!(
            error.contains("tools are not supported"),
            "provider detail lost: {error}"
        );
        assert!(
            seen.lock().unwrap().is_empty(),
            "no tool executes after request rejection"
        );
    }

    #[tokio::test]
    async fn transient_in_band_worker_saturation_retries_then_emits_one_answer() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_mock = attempts.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |_: &wiremock::Request| {
                let attempt = attempts_for_mock
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    + 1;
                if attempt == 1 {
                    ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"error":{"message":"ResourceExhausted: Worker local total request limit reached (22/16)"}}"#,
                    ]))
                } else {
                    ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"single final answer"}}]}"#,
                        r#"{"choices":[],"usage":{"prompt_tokens":4,"completion_tokens":3}}"#,
                    ]))
                }
            })
            .mount(&server)
            .await;

        let mut process = start_ollama_http(
            &AgentType::Nvidia,
            "hello",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("transient saturation must be replayed before returning the process");

        let mut output = String::new();
        while let Some(chunk) = process.next_line().await {
            output.push_str(&chunk);
        }
        assert!(process.child.wait().await.expect("lifeline").success());
        assert_eq!(output, "single final answer");
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
        let trace = process.stderr_capture.lock().unwrap().join("\n");
        assert!(
            trace.contains("attempt 1/3 failed (worker saturation)")
                && trace.contains("completed on attempt 2/3"),
            "the automatic attempt must remain visible: {trace}"
        );
    }

    #[tokio::test]
    async fn permanent_quota_exhaustion_is_never_retried() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(429).set_body_string(
                    r#"{"error":{"message":"insufficient_quota: quota exhausted"}}"#,
                ),
            )
            .mount(&server)
            .await;

        let started = start_ollama_http(
            &AgentType::LiteLlm,
            "hello",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        let error = match started {
            Ok(_) => panic!("permanent quota exhaustion must fail immediately"),
            Err(error) => error,
        };
        assert!(error.contains("429") && error.contains("quota exhausted"));
        assert_eq!(
            server.received_requests().await.expect("requests").len(),
            1,
            "a dead quota must not burn more calls"
        );
    }

    #[tokio::test]
    async fn transient_status_stops_at_the_provider_attempt_cap() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(503).set_body_string("temporarily unavailable"))
            .mount(&server)
            .await;

        let started = start_ollama_http(
            &AgentType::LiteLlm,
            "hello",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        let error = match started {
            Ok(_) => panic!("the capped transient failure must stay failed"),
            Err(error) => error,
        };
        assert!(
            error.contains("after 3 attempts"),
            "attempt count lost from terminal error: {error}"
        );
        assert_eq!(
            server.received_requests().await.expect("requests").len(),
            HTTP_PROVIDER_MAX_ATTEMPTS,
            "retry budget must be bounded"
        );
    }

    #[tokio::test]
    async fn provider_failure_after_a_mutating_tool_is_not_retried() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        struct MutatingTool {
            writes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl crate::agents::tools::ToolExecutor for MutatingTool {
            fn catalogue(&self) -> Vec<serde_json::Value> {
                vec![serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": "write_file",
                        "description": "Mutates a file.",
                        "parameters": { "type": "object", "properties": {}, "required": [] },
                    },
                })]
            }

            async fn execute(
                &self,
                call: &crate::agents::tools::ToolCall,
            ) -> crate::agents::tools::ToolOutcome {
                self.writes
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                crate::agents::tools::ToolOutcome {
                    call: call.clone(),
                    content: serde_json::json!({"written": true}),
                    ok: true,
                }
            }
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(|request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let after_tool = body["messages"]
                    .as_array()
                    .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
                if after_tool {
                    ResponseTemplate::new(503).set_body_string("temporarily unavailable")
                } else {
                    ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"write-1","function":{"name":"write_file","arguments":"{}"}}]}}]}"#,
                    ]))
                }
            })
            .mount(&server)
            .await;

        let writes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "write once",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(MutatingTool {
                writes: writes.clone(),
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("initial request is accepted");
        while process.next_line().await.is_some() {}
        assert!(
            !process.child.wait().await.expect("lifeline").success(),
            "the provider failure must still fail the run"
        );
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            server.received_requests().await.expect("requests").len(),
            2,
            "one initial request plus one failed follow-up, never a blind replay"
        );
        assert!(
            !process
                .stderr_capture
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.starts_with("[provider-retry:")),
            "no retry trace should exist after an external effect"
        );
    }

    /// The Ollama wire, which the LiteLLM test above does NOT cover. This is
    /// the exact gap that let a real bug through: Ollama 400s on
    /// JSON-string `arguments` and needs a real object, so the loop executed
    /// the tool and then died feeding the result back.
    #[tokio::test]
    #[serial]
    async fn tool_loop_round_trips_on_the_ollama_wire() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Context probe the Ollama path makes before chatting.
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        // Turn 1 — NDJSON, tool call on the message, counts on the terminal chunk.
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "{\"message\":{\"content\":\"\",\"tool_calls\":[{\"function\":{\"name\":\"mcp_list\",\"arguments\":{}}}]},\"done\":false}\n\
                 {\"done\":true,\"prompt_eval_count\":5,\"eval_count\":2}\n",
            ))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Turn 2 — must arrive with the tool result AND an OBJECT `arguments`.
        // `body_string_contains` is the assertion: a JSON-string encoding
        // would render `"arguments":"{}"` and never match.
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(body_string_contains(r#""arguments":{}"#))
            .and(body_string_contains(r#""role":"tool""#))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "{\"message\":{\"content\":\"2 servers\"},\"done\":false}\n\
                 {\"done\":true,\"prompt_eval_count\":9,\"eval_count\":3}\n",
            ))
            .mount(&server)
            .await;

        let base_url = server.uri();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "which servers?",
            "",
            "test-model",
            None,
            Some(&base_url),
            None,
            Some(std::sync::Arc::new(FakeTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

        let mut process = started.expect("start");
        let mut out = String::new();
        while let Some(chunk) = process.next_line().await {
            out.push_str(&chunk);
        }
        let status = process.child.wait().await.expect("lifeline");

        assert_eq!(seen.lock().unwrap().as_slice(), &["mcp_list".to_string()]);
        assert!(
            out.contains("2 servers"),
            "second turn not streamed: {out:?}"
        );
        assert!(status.success(), "the run should end clean");
        // KT-408 — counts must be the SUM across turns, not just the last
        // exchange: turn 1 costs 5+2, turn 2 costs 9+3, the real total is 19.
        // Asserting only that "9:3" appears somewhere in the capture used to
        // pass even when parse_token_usage silently returned turn one's cost
        // alone — this proves the actual number a caller receives.
        let captured_lines: Vec<String> = process
            .stderr_capture
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect();
        assert!(
            captured_lines.iter().any(|l| l == "ollama_tokens:5:2"),
            "turn one's own marker must survive: {captured_lines:?}"
        );
        assert!(
            captured_lines.iter().any(|l| l == "ollama_tokens:9:3"),
            "turn two's own marker must survive: {captured_lines:?}"
        );
        let (_, total_tokens) = parse_token_usage(&AgentType::Ollama, "unused", &captured_lines);
        assert_eq!(
            total_tokens, 19,
            "real total across both turns (5+2+9+3), not just one turn's: {captured_lines:?}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn worker_prose_intention_gets_one_bounded_tool_retry_then_delivers() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let requests = std::sync::Arc::new(AtomicUsize::new(0));
        let saw_retry_prompt = std::sync::Arc::new(AtomicBool::new(false));
        let requests_for_mock = requests.clone();
        let prompt_for_mock = saw_retry_prompt.clone();
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(move |request: &wiremock::Request| {
                let turn = requests_for_mock.fetch_add(1, Ordering::SeqCst);
                if turn == 0 {
                    return ResponseTemplate::new(200).set_body_string(
                        "{\"message\":{\"content\":\"I'll inspect the diff now.\"},\"done\":false}\n\
                         {\"done\":true,\"prompt_eval_count\":5,\"eval_count\":2}\n",
                    );
                }
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                prompt_for_mock.store(
                    body["messages"].as_array().is_some_and(|messages| {
                        messages.iter().any(|message| {
                            message["content"]
                                .as_str()
                                .is_some_and(|text| text.contains("only describe the next action"))
                        })
                    }) && body["tools"].as_array().is_some_and(|tools| !tools.is_empty()),
                    Ordering::SeqCst,
                );
                ResponseTemplate::new(200).set_body_string(
                    "{\"message\":{\"content\":\"\",\"tool_calls\":[{\"function\":{\"name\":\"task_exec_deliver\",\"arguments\":{}}}]},\"done\":false}\n\
                     {\"done\":true,\"prompt_eval_count\":7,\"eval_count\":3}\n",
                )
            })
            .mount(&server)
            .await;

        let base_url = server.uri();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "complete the worker task",
            "",
            "test-model",
            None,
            Some(&base_url),
            None,
            Some(std::sync::Arc::new(WorkerTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

        let mut process = started.expect("start");
        while process.next_line().await.is_some() {}
        let status = process.child.wait().await.expect("lifeline");

        assert!(
            status.success(),
            "the corrective turn must be able to deliver"
        );
        assert!(saw_retry_prompt.load(Ordering::SeqCst));
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &["task_exec_deliver".to_string()]
        );
    }

    #[tokio::test]
    #[serial]
    async fn repeated_worker_prose_intentions_fail_without_an_unbounded_loop() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let requests = std::sync::Arc::new(AtomicUsize::new(0));
        let requests_for_mock = requests.clone();
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(move |_: &wiremock::Request| {
                requests_for_mock.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_string(
                    "{\"message\":{\"content\":\"Let me inspect that next.\"},\"done\":false}\n\
                     {\"done\":true,\"prompt_eval_count\":5,\"eval_count\":2}\n",
                )
            })
            .mount(&server)
            .await;

        let base_url = server.uri();
        let started = start_ollama_http(
            &AgentType::Ollama,
            "complete the worker task",
            "",
            "test-model",
            None,
            Some(&base_url),
            None,
            Some(std::sync::Arc::new(WorkerTools {
                seen: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

        let mut process = started.expect("start");
        while process.next_line().await.is_some() {}
        let status = process.child.wait().await.expect("lifeline");

        assert!(!status.success(), "prose without delivery must never pass");
        assert_eq!(
            requests.load(Ordering::SeqCst),
            WORKER_PROSE_ONLY_ITERATIONS
        );
        let captured = process.stderr_capture.lock().unwrap().join(" ");
        assert!(
            captured.contains("answered in prose without using an available tool"),
            "bounded failure reason missing: {captured}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn prose_after_workspace_mutation_enters_bounded_finalization() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let requests = std::sync::Arc::new(AtomicUsize::new(0));
        let narrowed = std::sync::Arc::new(AtomicBool::new(false));
        let delivery_only = std::sync::Arc::new(AtomicBool::new(false));
        let checkpointed = std::sync::Arc::new(AtomicBool::new(false));
        let requests_for_mock = requests.clone();
        let narrowed_for_mock = narrowed.clone();
        let delivery_for_mock = delivery_only.clone();
        let checkpointed_for_mock = checkpointed.clone();
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(move |request: &wiremock::Request| {
                let round = requests_for_mock.fetch_add(1, Ordering::SeqCst);
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let names = body["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| tool.pointer("/function/name")?.as_str())
                    .collect::<Vec<_>>();
                let response = match round {
                    0 => {
                        assert!(names.contains(&"search_text"));
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"edit_lines","arguments":{}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    1 => {
                        // The mutation happened on the prior turn. The model's
                        // intention is exactly the real KT-410 failure shape.
                        assert!(names.contains(&"search_text"));
                        r#"{"message":{"content":"Let me read the actual file now."},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    2 => {
                        let messages = body["messages"].as_array().unwrap();
                        let retained_edit = messages.iter().any(|message| {
                            message["role"] == "assistant"
                                && message["tool_calls"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .any(|call| call["function"]["name"] == "edit_lines")
                        });
                        let retained_edit_result = messages.iter().any(|message| {
                            message["role"] == "tool"
                                && message["name"] == "edit_lines"
                        });
                        checkpointed_for_mock.store(
                            messages.len() <= 6
                                && retained_edit
                                && retained_edit_result
                                && messages.iter().any(|message| {
                                    message["content"]
                                        .as_str()
                                        .is_some_and(|content| content.contains("authoritative state"))
                                }),
                            Ordering::SeqCst,
                        );
                        narrowed_for_mock.store(
                            !names.contains(&"search_text")
                                && !names.contains(&"api_call")
                                && names.contains(&"read_file")
                                && names.contains(&"git_commit")
                                && names.contains(&"task_exec_deliver"),
                            Ordering::SeqCst,
                        );
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"git_commit","arguments":{}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    3 => {
                        delivery_for_mock.store(
                            names.as_slice() == ["task_exec_deliver"],
                            Ordering::SeqCst,
                        );
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"task_exec_deliver","arguments":{}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    other => panic!("unexpected HTTP worker round {other}"),
                };
                ResponseTemplate::new(200).set_body_string(response)
            })
            .mount(&server)
            .await;

        let base_url = server.uri();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "complete the worker task",
            "",
            "test-model",
            None,
            Some(&base_url),
            None,
            Some(std::sync::Arc::new(WorkerTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

        let mut process = started.expect("start");
        while process.next_line().await.is_some() {}
        let status = process.child.wait().await.expect("lifeline");

        assert!(status.success());
        assert!(checkpointed.load(Ordering::SeqCst));
        assert!(narrowed.load(Ordering::SeqCst));
        assert!(delivery_only.load(Ordering::SeqCst));
        assert_eq!(requests.load(Ordering::SeqCst), 4);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &["edit_lines", "git_commit", "task_exec_deliver"]
        );
    }

    #[tokio::test]
    #[serial]
    async fn finalization_git_inspections_are_bounded_per_successful_edit() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let requests = std::sync::Arc::new(AtomicUsize::new(0));
        let saw_limit_note = std::sync::Arc::new(AtomicBool::new(false));
        let inspections_withdrawn = std::sync::Arc::new(AtomicBool::new(false));
        let inspections_restored = std::sync::Arc::new(AtomicBool::new(false));
        let requests_for_mock = requests.clone();
        let limit_for_mock = saw_limit_note.clone();
        let withdrawn_for_mock = inspections_withdrawn.clone();
        let restored_for_mock = inspections_restored.clone();
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(move |request: &wiremock::Request| {
                let round = requests_for_mock.fetch_add(1, Ordering::SeqCst);
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let body_text = String::from_utf8_lossy(&request.body);
                let names = body["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| tool.pointer("/function/name")?.as_str())
                    .collect::<Vec<_>>();
                let response = match round {
                    0 => r#"{"message":{"content":"","tool_calls":[{"function":{"name":"edit_lines","arguments":{}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#,
                    1 => r#"{"message":{"content":"I will inspect before committing."},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#,
                    2 => r#"{"message":{"content":"","tool_calls":[{"function":{"name":"git_status","arguments":{}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#,
                    3 => r#"{"message":{"content":"","tool_calls":[{"function":{"name":"git_diff","arguments":{"path":"first.rs"}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#,
                    4 => r#"{"message":{"content":"","tool_calls":[{"function":{"name":"git_diff","arguments":{"path":"second.rs"}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#,
                    5 => {
                        withdrawn_for_mock.store(
                            !names.contains(&"git_status") && !names.contains(&"git_diff"),
                            Ordering::SeqCst,
                        );
                        limit_for_mock.store(
                            body_text.contains("kronn_finalization_git_inspection_limit"),
                            Ordering::SeqCst,
                        );
                        // Reproduce the real failure shape: the model remembers
                        // a withdrawn inspection tool. The transport must refuse
                        // it and must not call the executor.
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"git_status","arguments":{}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    6 => {
                        assert!(!names.contains(&"git_status"));
                        assert!(!names.contains(&"git_diff"));
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"edit_lines","arguments":{"path":"third.rs"}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    7 => {
                        restored_for_mock.store(
                            names.contains(&"git_status") && names.contains(&"git_diff"),
                            Ordering::SeqCst,
                        );
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"git_status","arguments":{}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    8 => r#"{"message":{"content":"","tool_calls":[{"function":{"name":"git_commit","arguments":{}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#,
                    9 => {
                        assert_eq!(names.as_slice(), ["task_exec_deliver"]);
                        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"task_exec_deliver","arguments":{}}}]},"done":false}
{"done":true,"prompt_eval_count":5,"eval_count":2}
"#
                    }
                    other => panic!("unexpected HTTP worker round {other}"),
                };
                ResponseTemplate::new(200).set_body_string(response)
            })
            .mount(&server)
            .await;

        let base_url = server.uri();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "complete the worker task",
            "",
            "test-model",
            None,
            Some(&base_url),
            None,
            Some(std::sync::Arc::new(WorkerTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

        let mut process = started.expect("start");
        while process.next_line().await.is_some() {}
        let status = process.child.wait().await.expect("lifeline");

        assert!(status.success());
        assert_eq!(requests.load(Ordering::SeqCst), 10);
        assert!(saw_limit_note.load(Ordering::SeqCst));
        assert!(inspections_withdrawn.load(Ordering::SeqCst));
        assert!(inspections_restored.load(Ordering::SeqCst));
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[
                "edit_lines",
                "git_status",
                "git_diff",
                "git_diff",
                "edit_lines",
                "git_status",
                "git_commit",
                "task_exec_deliver",
            ],
            "the remembered fourth inspection must be refused before the executor; a successful edit opens one fresh inspection epoch"
        );
    }

    /// KT-407 V15 — an MLX worker can keep requesting distinct, overlapping
    /// repository slices even after it has identified the target. The native
    /// Ollama loop must turn the existing observation counter into a hard
    /// finalization boundary without applying that policy to OpenAI-wire or
    /// GGUF workers.
    #[tokio::test]
    #[serial]
    async fn native_mlx_worker_finalizes_after_its_preanalysis_budget() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"details":{"format":"safetensors"}}"#),
            )
            .mount(&server)
            .await;

        let full_catalogue_requests = std::sync::Arc::new(AtomicUsize::new(0));
        let saw_finalization_catalogue = std::sync::Arc::new(AtomicBool::new(false));
        let full_for_mock = full_catalogue_requests.clone();
        let finalization_for_mock = saw_finalization_catalogue.clone();
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let names = body["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| tool.pointer("/function/name")?.as_str())
                    .collect::<Vec<_>>();
                if names.contains(&"search_text") {
                    let requested = body["options"]["num_ctx"].as_u64().unwrap();
                    assert!(
                        (OLLAMA_NUM_CTX_FLOOR..=MLX_WORKER_EFFECTIVE_CTX_CAP)
                            .contains(&requested),
                        "the model/operator cap may be smaller, but the full MLX worker catalogue must never reload a 65K slot: {requested}"
                    );
                    let round = full_for_mock.fetch_add(1, Ordering::SeqCst);
                    ResponseTemplate::new(200).set_body_string(format!(
                        "{{\"message\":{{\"content\":\"\",\"tool_calls\":[{{\"function\":{{\"name\":\"search_text\",\"arguments\":{{\"query\":\"probe-{round}\"}}}}}}]}},\"done\":false}}\n{{\"done\":true,\"prompt_eval_count\":5,\"eval_count\":2}}\n"
                    ))
                } else {
                    assert!(
                        body["options"]["num_ctx"].as_u64().unwrap()
                            <= MLX_WORKER_EFFECTIVE_CTX_CAP
                    );
                    let remembered_tool_names = body["messages"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|message| {
                            message["tool_calls"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|call| {
                                    call.pointer("/function/name")
                                        .and_then(serde_json::Value::as_str)
                                })
                        })
                        .collect::<Vec<_>>();
                    assert!(
                        !remembered_tool_names.contains(&"search_text"),
                        "the in-flight boundary call must execute, then the checkpoint must not advertise its withdrawn tool: {body}"
                    );
                    finalization_for_mock.store(
                        !names.contains(&"search_text")
                            && names.contains(&"read_file")
                            && names.contains(&"edit_lines")
                            && names.contains(&"git_commit")
                            && names.contains(&"task_exec_deliver"),
                        Ordering::SeqCst,
                    );
                    ResponseTemplate::new(200).set_body_string(
                        "{\"message\":{\"content\":\"The bounded pre-analysis ended without a safe mutation.\"},\"done\":false}\n{\"done\":true,\"prompt_eval_count\":5,\"eval_count\":2}\n",
                    )
                }
            })
            .mount(&server)
            .await;

        let base_url = server.uri();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "complete the scoped worker task",
            "",
            "local-alias:latest",
            None,
            Some(&base_url),
            None,
            Some(std::sync::Arc::new(WorkerTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

        let mut process = started.expect("start");
        while process.next_line().await.is_some() {}
        let status = process.child.wait().await.expect("lifeline");

        assert!(
            !status.success(),
            "a bounded worker that never commits or delivers must fail visibly"
        );
        assert!(saw_finalization_catalogue.load(Ordering::SeqCst));
        assert_eq!(
            full_catalogue_requests.load(Ordering::SeqCst),
            MLX_WORKER_MAX_OBSERVATIONS_WITHOUT_MUTATION + 1,
            "the response already in flight at the boundary remains valid, then the next request is narrowed"
        );
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|name| *name == "search_text")
                .count(),
            MLX_WORKER_MAX_OBSERVATIONS_WITHOUT_MUTATION + 1
        );
        let captured = process.stderr_capture.lock().unwrap().join(" ");
        assert!(
            captured.contains("13 successful repository observations without a workspace mutation"),
            "the exact fail-fast reason must remain auditable: {captured}"
        );
        assert!(
            captured.contains("answered in prose without using a finalization tool"),
            "the missing durable delivery must be explicit: {captured}"
        );
    }

    /// A model stuck in a successful-but-endless tool loop must stop billing
    /// tokens and return a bounded partial result, even if it ignores the
    /// tool-free synthesis request.
    #[tokio::test]
    async fn tool_loop_stops_at_the_per_tool_budget_with_a_bounded_fallback() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Always asks for another tool, with DIFFERENT arguments every round, so it
        // never converges and never repeats itself either. Identical repeats are
        // short-circuited now (see the test below), so the cap has to be proven on
        // the case it actually guards: a model that keeps exploring.
        let round = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let round_for_mock = round.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |_: &wiremock::Request| {
                let n = round_for_mock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_string(sse(&[&format!(
                    r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"c{n}","function":{{"name":"mcp_list","arguments":"{{\"probe\":{n}}}"}}}}]}}}}]}}"#
                )]))
            })
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "loop forever",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(FakeTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        let mut out = String::new();
        while let Some(line) = process.next_line().await {
            out.push_str(&line);
        }
        let status = process.child.wait().await.expect("lifeline");

        // The per-tool budget is what catches this shape: a model varying one
        // argument each round slips past the identical-repeat guard, so it is
        // stopped after MAX_CALLS_PER_TOOL executions of the same tool — long
        // before the round cap, which stays as the outer backstop. Observed in
        // production before this existed: 47 `api_call`s over 47 minutes.
        let runs = seen.lock().unwrap().len();
        assert!(
            runs < crate::agents::tools::MAX_TOOL_ITERATIONS,
            "the per-tool budget must bite before the round cap, ran {runs}"
        );
        assert_eq!(runs, 12, "one tool gets MAX_CALLS_PER_TOOL executions");
        assert!(
            status.success(),
            "useful tool results must become an honest partial answer"
        );
        assert!(
            out.contains("Kronn stopped a non-progressing tool loop"),
            "the model ignored the synthesis request, so Kronn must emit its bounded fallback: {out:?}"
        );
        let captured = process.stderr_capture.lock().unwrap().join(" ");
        assert!(
            captured.contains("forced convergence fallback"),
            "the convergence reason must be observable: {captured}"
        );
        assert!(
            captured.contains("useful_results=12"),
            "the diagnostic must preserve the amount of useful evidence: {captured}"
        );
    }

    /// A bounded worker must not die at the generic 50-round backstop after a
    /// legitimate repository investigation. It gets a separate, narrow phase
    /// in which a fresh CAS read and the delivery lifecycle remain possible.
    #[tokio::test]
    async fn worker_backstop_transitions_to_bounded_finalization() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let exploration_requests = std::sync::Arc::new(AtomicUsize::new(0));
        let finalization_requests = std::sync::Arc::new(AtomicUsize::new(0));
        let saw_progress_nudge = std::sync::Arc::new(AtomicBool::new(false));
        let saw_finalization_prompt = std::sync::Arc::new(AtomicBool::new(false));
        let finalization_kept_read_file = std::sync::Arc::new(AtomicBool::new(false));
        let repair_read_catalogue_only = std::sync::Arc::new(AtomicBool::new(false));
        let repair_edit_catalogue_only = std::sync::Arc::new(AtomicBool::new(false));
        let repair_commit_catalogue_only = std::sync::Arc::new(AtomicBool::new(false));
        let delivery_catalogue_only = std::sync::Arc::new(AtomicBool::new(false));
        let saw_delivery_retry_prompt = std::sync::Arc::new(AtomicBool::new(false));

        let exploration_for_mock = exploration_requests.clone();
        let finalization_for_mock = finalization_requests.clone();
        let nudge_for_mock = saw_progress_nudge.clone();
        let prompt_for_mock = saw_finalization_prompt.clone();
        let read_for_mock = finalization_kept_read_file.clone();
        let repair_read_for_mock = repair_read_catalogue_only.clone();
        let repair_edit_for_mock = repair_edit_catalogue_only.clone();
        let repair_commit_for_mock = repair_commit_catalogue_only.clone();
        let delivery_for_mock = delivery_catalogue_only.clone();
        let delivery_retry_for_mock = saw_delivery_retry_prompt.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let body_text = String::from_utf8_lossy(&request.body);
                if body_text.contains("kronn_worker_progress") {
                    nudge_for_mock.store(true, Ordering::SeqCst);
                }
                let names = body["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| tool.pointer("/function/name")?.as_str())
                    .collect::<Vec<_>>();

                if names.contains(&"api_call") {
                    let round = exploration_for_mock.fetch_add(1, Ordering::SeqCst);
                    // 24 distinct searches + 26 distinct reads fit their
                    // worker budgets. The worker transitions immediately after
                    // the 50th completed exploration round.
                    let tool = if round < 48 {
                        if round.is_multiple_of(2) {
                            "search_text"
                        } else {
                            "read_file"
                        }
                    } else if round < 50 {
                        "read_file"
                    } else {
                        "search_text"
                    };
                    return ResponseTemplate::new(200).set_body_string(sse(&[&format!(
                        r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"e{round}","function":{{"name":"{tool}","arguments":"{{\"probe\":{round}}}"}}}}]}}}}]}}"#
                    )]));
                }

                prompt_for_mock.store(
                    body_text.contains("exploration window is complete"),
                    Ordering::SeqCst,
                );
                if names.contains(&"read_file") {
                    read_for_mock.store(true, Ordering::SeqCst);
                }
                let stage = finalization_for_mock.fetch_add(1, Ordering::SeqCst);
                if stage == 12 {
                    repair_read_for_mock
                        .store(names.as_slice() == ["read_file"], Ordering::SeqCst);
                }
                if (13..=14).contains(&stage) {
                    repair_edit_for_mock
                        .store(names.as_slice() == ["edit_lines"], Ordering::SeqCst);
                }
                if (15..=17).contains(&stage) {
                    repair_commit_for_mock.store(
                        names.as_slice() == ["git_status", "git_diff", "git_commit"],
                        Ordering::SeqCst,
                    );
                }
                if stage >= 18 {
                    delivery_for_mock.store(
                        names.as_slice() == ["task_exec_deliver"],
                        Ordering::SeqCst,
                    );
                }
                if stage == 19
                    && body_text.contains("delivery-only attempt(s) remain")
                {
                    delivery_retry_for_mock.store(true, Ordering::SeqCst);
                }
                let response = match stage {
                    0 => sse(&[
                        // A model can remember an exploration tool even after
                        // Kronn removes it from the current catalogue. The
                        // transport must refuse it rather than trusting that
                        // declaration removal alone constrains execution.
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"late-api","function":{"name":"api_call","arguments":"{}"}}]}}]}"#,
                    ]),
                    1 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"refresh-1","function":{"name":"read_file","arguments":"{\"probe\":\"final-1\"}"}}]}}]}"#,
                    ]),
                    2 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"refresh-2","function":{"name":"read_file","arguments":"{\"probe\":\"final-2\"}"}}]}}]}"#,
                    ]),
                    3 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"refresh-3","function":{"name":"read_file","arguments":"{\"probe\":\"final-3\"}"}}]}}]}"#,
                    ]),
                    4 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"status-before-edit","function":{"name":"git_status","arguments":"{}"}}]}}]}"#,
                    ]),
                    5 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"f0","function":{"name":"edit_lines","arguments":"{}"}}]}}]}"#,
                    ]),
                    6 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"f1","function":{"name":"git_status","arguments":"{}"}}]}}]}"#,
                    ]),
                    7 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"f2","function":{"name":"git_diff","arguments":"{}"}}]}}]}"#,
                    ]),
                    8 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"f3","function":{"name":"edit_lines","arguments":"{}"}}]}}]}"#,
                    ]),
                    9 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"f4","function":{"name":"git_status","arguments":"{}"}}]}}]}"#,
                    ]),
                    10 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"f5","function":{"name":"git_diff","arguments":"{}"}}]}}]}"#,
                    ]),
                    11 => sse(&[
                        // The edit fails exactly on the twelfth and final
                        // finalization response. A one-shot repair must remain
                        // possible without reopening general exploration.
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"f6","function":{"name":"edit_lines","arguments":"{\"force_fail\":true}"}}]}}]}"#,
                    ]),
                    12 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"repair-read","function":{"name":"read_file","arguments":"{\"path\":\"backend/src/api/orchestration.rs\",\"offset\":735,\"limit\":40}"}}]}}]}"#,
                    ]),
                    13 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"repair-edit-1","function":{"name":"edit_lines","arguments":"{\"force_fail\":true}"}}]}}]}"#,
                    ]),
                    14 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"repair-edit-2","function":{"name":"edit_lines","arguments":"{}"}}]}}]}"#,
                    ]),
                    15 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"repair-status","function":{"name":"git_status","arguments":"{}"}}]}}]}"#,
                    ]),
                    16 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"repair-diff","function":{"name":"git_diff","arguments":"{}"}}]}}]}"#,
                    ]),
                    17 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"repair-commit","function":{"name":"git_commit","arguments":"{}"}}]}}]}"#,
                    ]),
                    18 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"The commit is done."}}]}"#,
                    ]),
                    19 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"f7","function":{"name":"task_exec_deliver","arguments":"{}"}}]}}]}"#,
                    ]),
                    _ => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"Unexpected extra request."}}]}"#,
                    ]),
                };
                ResponseTemplate::new(200).set_body_string(response)
            })
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "complete the worker task",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(WorkerTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        let mut out = String::new();
        while let Some(line) = process.next_line().await {
            out.push_str(&line);
        }
        let status = process.child.wait().await.expect("lifeline");

        assert!(status.success(), "the finalization phase must complete");
        assert!(
            out.contains("DeliveryManifest submitted to Kronn."),
            "durable delivery acknowledgement missing: {out}"
        );
        assert!(saw_progress_nudge.load(Ordering::SeqCst));
        assert!(saw_finalization_prompt.load(Ordering::SeqCst));
        assert!(
            finalization_kept_read_file.load(Ordering::SeqCst),
            "the narrow phase must keep read_file for a fresh CAS receipt"
        );
        assert!(
            repair_read_catalogue_only.load(Ordering::SeqCst),
            "a refused edit must reopen only one bounded read_file phase"
        );
        assert!(
            repair_edit_catalogue_only.load(Ordering::SeqCst),
            "a successful repair read must expose edit tools only"
        );
        assert!(
            repair_commit_catalogue_only.load(Ordering::SeqCst),
            "a successful repair edit must expose Git completion tools only"
        );
        assert!(
            delivery_catalogue_only.load(Ordering::SeqCst),
            "the post-commit request must expose only task_exec_deliver"
        );
        assert!(
            saw_delivery_retry_prompt.load(Ordering::SeqCst),
            "a prose-only delivery turn must receive one bounded manifest retry"
        );
        let calls = seen.lock().unwrap();
        assert_eq!(
            calls.iter().filter(|name| *name == "search_text").count(),
            24
        );
        assert_eq!(
            calls.iter().filter(|name| *name == "read_file").count(),
            26 + WORKER_FINALIZATION_READ_FILE_CALLS + 1,
            "only finalization CAS refreshes plus one repair read may execute"
        );
        assert_eq!(
            calls.iter().filter(|name| *name == "api_call").count(),
            0,
            "the remembered api_call after catalogue narrowing must be refused"
        );
        assert_eq!(
            &calls[calls.len() - 7..],
            [
                "read_file",
                "edit_lines",
                "edit_lines",
                "git_status",
                "git_diff",
                "git_commit",
                "task_exec_deliver"
            ]
        );
        assert!(
            finalization_requests.load(Ordering::SeqCst)
                <= WORKER_FINALIZATION_ITERATIONS
                    + WORKER_REPAIR_READ_ITERATIONS
                    + WORKER_REPAIR_EDIT_ITERATIONS
                    + WORKER_REPAIR_COMMIT_ITERATIONS
                    + WORKER_DELIVERY_ITERATIONS,
            "finalization, one-shot repair and delivery must remain bounded"
        );
        let captured = process.stderr_capture.lock().unwrap().join(" ");
        assert!(
            captured.contains("refused undeclared tool `api_call`"),
            "the refusal must be observable to the model and operator: {captured}"
        );
        for transition in [
            "entering one-shot repair read",
            "entering repair edit",
            "entering repair commit",
        ] {
            assert!(
                captured.contains(transition),
                "repair transition must be observable: {transition}; {captured}"
            );
        }
    }

    /// KT-407 — a real Qwen worker consumed all three finalization reads, then
    /// kept calling the remembered `read_file` six times. Generic convergence
    /// removed the edit/Git tools and made delivery impossible. The first such
    /// refusal must enter the existing non-renewable repair sequence whether it
    /// is an undeclared call on the next turn or a fourth call in the same batch.
    #[tokio::test]
    async fn withdrawn_finalization_read_enters_one_shot_repair_before_convergence() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        for same_batch in [false, true] {
            let server = MockServer::start().await;
            let exploration_requests = std::sync::Arc::new(AtomicUsize::new(0));
            let completion_requests = std::sync::Arc::new(AtomicUsize::new(0));
            let repair_read_only = std::sync::Arc::new(AtomicBool::new(false));
            let repair_edit_only = std::sync::Arc::new(AtomicBool::new(false));
            let repair_commit_only = std::sync::Arc::new(AtomicBool::new(false));
            let delivery_only = std::sync::Arc::new(AtomicBool::new(false));
            let saw_read_refusal_prompt = std::sync::Arc::new(AtomicBool::new(false));

            let exploration_for_mock = exploration_requests.clone();
            let completion_for_mock = completion_requests.clone();
            let read_for_mock = repair_read_only.clone();
            let edit_for_mock = repair_edit_only.clone();
            let commit_for_mock = repair_commit_only.clone();
            let delivery_for_mock = delivery_only.clone();
            let prompt_for_mock = saw_read_refusal_prompt.clone();
            let same_batch_for_mock = same_batch;
            Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let body_text = String::from_utf8_lossy(&request.body);
                let names = body["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| tool.pointer("/function/name")?.as_str())
                    .collect::<Vec<_>>();

                if names.contains(&"api_call") {
                    let round = exploration_for_mock.fetch_add(1, Ordering::SeqCst);
                    let tool = if round.is_multiple_of(2) {
                        "search_text"
                    } else {
                        "read_file"
                    };
                    return ResponseTemplate::new(200).set_body_string(sse(&[&format!(
                        r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"e{round}","function":{{"name":"{tool}","arguments":"{{\"probe\":{round}}}"}}}}]}}}}]}}"#
                    )]));
                }

                let stage = completion_for_mock.fetch_add(1, Ordering::SeqCst);
                let repair_stage = if same_batch_for_mock { 3 } else { 4 };
                let response = match stage {
                    0..=1 => {
                        let refresh = stage + 1;
                        sse(&[&format!(
                            r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"refresh-{refresh}","function":{{"name":"read_file","arguments":"{{\"probe\":\"final-{refresh}\"}}"}}}}]}}}}]}}"#
                        )])
                    }
                    2 if same_batch_for_mock => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"refresh-3","function":{"name":"read_file","arguments":"{\"probe\":\"final-3\"}"}},{"index":1,"id":"overflow-read","function":{"name":"read_file","arguments":"{\"probe\":\"same-batch-overflow\"}"}}]}}]}"#,
                    ]),
                    2 => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"refresh-3","function":{"name":"read_file","arguments":"{\"probe\":\"final-3\"}"}}]}}]}"#,
                    ]),
                    3 if !same_batch_for_mock => sse(&[
                        // The declaration is gone after refresh 3. One remembered
                        // call is refused, then must route into repair immediately.
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"remembered-read","function":{"name":"read_file","arguments":"{\"probe\":\"stale\"}"}}]}}]}"#,
                    ]),
                    stage if stage == repair_stage => {
                        read_for_mock.store(names.as_slice() == ["read_file"], Ordering::SeqCst);
                        prompt_for_mock.store(
                            body_text.contains("finalization read was refused")
                                && body_text.contains("non-renewable repair sequence"),
                            Ordering::SeqCst,
                        );
                        // V11 did this: a remembered Git tool must be refused,
                        // but it must not consume the one valid repair read.
                        sse(&[
                            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"remembered-status","function":{"name":"git_status","arguments":"{}"}}]}}]}"#,
                        ])
                    }
                    stage if stage == repair_stage + 1 => {
                        read_for_mock.store(names.as_slice() == ["read_file"], Ordering::SeqCst);
                        sse(&[
                            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"repair-read","function":{"name":"read_file","arguments":"{\"probe\":\"repair\"}"}}]}}]}"#,
                        ])
                    }
                    stage if stage == repair_stage + 2 => {
                        edit_for_mock.store(names.as_slice() == ["edit_lines"], Ordering::SeqCst);
                        sse(&[
                            // V13 repeated the withdrawn read on its first edit
                            // response. It must be refused without reopening
                            // exploration, while leaving a bounded edit path.
                            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"remembered-edit-read","function":{"name":"read_file","arguments":"{\"probe\":\"remembered\"}"}}]}}]}"#,
                        ])
                    }
                    stage if stage == repair_stage + 3 => {
                        edit_for_mock.store(names.as_slice() == ["edit_lines"], Ordering::SeqCst);
                        // V13's next edit omitted one required argument. The
                        // executor refusal must reach the model once so its
                        // final bounded edit response can correct the call.
                        sse(&[
                            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"invalid-repair-edit","function":{"name":"edit_lines","arguments":"{\"force_fail\":true}"}}]}}]}"#,
                        ])
                    }
                    stage if stage == repair_stage + 4 => {
                        edit_for_mock.store(names.as_slice() == ["edit_lines"], Ordering::SeqCst);
                        sse(&[
                            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"corrected-repair-edit","function":{"name":"edit_lines","arguments":"{}"}}]}}]}"#,
                        ])
                    }
                    stage if stage == repair_stage + 5 => {
                        commit_for_mock.store(
                            names.as_slice() == ["git_status", "git_diff", "git_commit"],
                            Ordering::SeqCst,
                        );
                        sse(&[
                            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"repair-commit","function":{"name":"git_commit","arguments":"{}"}}]}}]}"#,
                        ])
                    }
                    stage if stage == repair_stage + 6 => {
                        delivery_for_mock.store(
                            names.as_slice() == ["task_exec_deliver"],
                            Ordering::SeqCst,
                        );
                        sse(&[
                            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"deliver","function":{"name":"task_exec_deliver","arguments":"{}"}}]}}]}"#,
                        ])
                    }
                    _ => sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"Unexpected extra request."}}]}"#,
                    ]),
                };
                ResponseTemplate::new(200).set_body_string(response)
            })
            .mount(&server)
            .await;

            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let mut process = start_ollama_http(
                &AgentType::LiteLlm,
                "complete the worker task",
                "",
                "test-model",
                None,
                Some(&server.uri()),
                None,
                Some(std::sync::Arc::new(WorkerTools { seen: seen.clone() })),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start");

            let mut out = String::new();
            while let Some(line) = process.next_line().await {
                out.push_str(&line);
            }
            let status = process.child.wait().await.expect("lifeline");

            assert!(status.success(), "the repair path must reach delivery");
            assert!(out.contains("DeliveryManifest submitted to Kronn."));
            assert_eq!(exploration_requests.load(Ordering::SeqCst), 50);
            assert!(repair_read_only.load(Ordering::SeqCst));
            assert!(repair_edit_only.load(Ordering::SeqCst));
            assert!(repair_commit_only.load(Ordering::SeqCst));
            assert!(delivery_only.load(Ordering::SeqCst));
            assert!(saw_read_refusal_prompt.load(Ordering::SeqCst));
            let calls = seen.lock().unwrap();
            assert_eq!(
                calls.iter().filter(|name| *name == "read_file").count(),
                25 + WORKER_FINALIZATION_READ_FILE_CALLS + 1,
                "the extra read must be refused, followed by exactly one repair read"
            );
            assert_eq!(calls.last().map(String::as_str), Some("task_exec_deliver"));
            let captured = process.stderr_capture.lock().unwrap().join(" ");
            if same_batch {
                assert!(captured.contains("refused finalization read_file beyond the 3-call"));
            } else {
                assert!(captured.contains("refused undeclared tool `read_file`"));
            }
            assert!(captured
                .contains("worker finalization read refusal — entering one-shot repair read"));
            assert!(captured.contains("refused undeclared tool `git_status`"));
            assert!(captured.contains("refused undeclared tool `read_file`"));
            assert_eq!(
                calls.iter().filter(|name| *name == "edit_lines").count(),
                2,
                "one invalid edit must be followed by exactly one corrected edit"
            );
            assert!(!captured.contains("tool convergence forced"));
        }
    }

    /// MSG-09618d74 — varying arguments and alternating tool names used to
    /// evade exact-call deduplication until the 50-round hard cap. Repeated
    /// error-only rounds must now open the circuits and yield a bounded partial
    /// answer while preserving the useful evidence obtained before the loop.
    #[tokio::test]
    async fn alternating_failed_tools_force_a_partial_answer_before_the_hard_cap() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let model_turns = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_turns_for_mock = model_turns.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let turn = model_turns_for_mock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                if body.get("tools").is_none() {
                    return ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"Partial answer: the seed evidence is confirmed; file discovery remained unavailable."}}]}"#,
                    ]));
                }
                let (name, probe) = if turn == 0 {
                    ("seed_evidence", 0)
                } else if turn % 2 == 1 {
                    ("find_files", turn)
                } else {
                    ("list_files", turn)
                };
                ResponseTemplate::new(200).set_body_string(sse(&[&format!(
                    r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"c{turn}","function":{{"name":"{name}","arguments":"{{\"probe\":{probe}}}"}}}}]}}}}]}}"#
                )]))
            })
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "find the files, but report what you can prove",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(ConvergenceTools {
                seen: seen.clone(),
                failing: ["find_files".to_string(), "list_files".to_string()]
                    .into_iter()
                    .collect(),
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        let mut out = String::new();
        while let Some(line) = process.next_line().await {
            out.push_str(&line);
        }
        let status = process.child.wait().await.expect("lifeline");

        assert!(
            status.success(),
            "forced synthesis is a usable partial answer"
        );
        assert!(
            out.contains("Partial answer: the seed evidence is confirmed"),
            "the useful result must survive the error loop: {out:?}"
        );
        let calls = seen.lock().unwrap().clone();
        assert_eq!(
            calls.iter().filter(|name| *name == "seed_evidence").count(),
            1
        );
        assert_eq!(calls.iter().filter(|name| *name == "find_files").count(), 3);
        assert_eq!(calls.iter().filter(|name| *name == "list_files").count(), 3);
        assert!(
            model_turns.load(std::sync::atomic::Ordering::SeqCst)
                < crate::agents::tools::MAX_TOOL_ITERATIONS,
            "semantic non-progress must converge before the global cap"
        );
        let captured = process.stderr_capture.lock().unwrap().join(" ");
        assert!(captured.contains("forced tool convergence"), "{captured}");
        assert!(
            captured.contains("find_files: 3 attempts (3 errors, 0 refused)"),
            "{captured}"
        );
        assert!(
            captured.contains("list_files: 3 attempts (3 errors, 0 refused)"),
            "{captured}"
        );
        assert!(captured.contains("useful_results=1"), "{captured}");

        let requests = server.received_requests().await.expect("requests");
        let final_body: serde_json::Value =
            serde_json::from_slice(&requests.last().expect("final request").body).unwrap();
        assert!(final_body.get("tools").is_none(), "tools must be withdrawn");
        assert!(
            final_body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|message| message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("Do not call any more tools")))),
            "the synthesis turn must carry an explicit convergence instruction"
        );
    }

    /// Progressive pagination is useful progress even when it invokes the same
    /// tool with changing arguments many times. Successful rounds reset the
    /// semantic error streak and must never trigger the failure circuit.
    #[tokio::test]
    async fn successful_progressive_pagination_keeps_its_tool_budget() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let turn = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let turn_for_mock = turn.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |_: &wiremock::Request| {
                let current = turn_for_mock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if current < 8 {
                    ResponseTemplate::new(200).set_body_string(sse(&[&format!(
                        r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"p{current}","function":{{"name":"list_files","arguments":"{{\"page\":{current}}}"}}}}]}}}}]}}"#
                    )]))
                } else {
                    ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"All eight pages were read."}}]}"#,
                    ]))
                }
            })
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "read every page",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(ConvergenceTools {
                seen: seen.clone(),
                failing: std::collections::HashSet::new(),
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        let mut out = String::new();
        while let Some(line) = process.next_line().await {
            out.push_str(&line);
        }
        let status = process.child.wait().await.expect("lifeline");
        assert!(status.success());
        assert!(out.contains("All eight pages were read."), "{out:?}");
        assert_eq!(seen.lock().unwrap().len(), 8);
        assert!(!process
            .stderr_capture
            .lock()
            .unwrap()
            .join(" ")
            .contains("forced tool convergence"));
    }

    #[tokio::test]
    async fn a_success_resets_the_per_tool_failure_circuit() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let turn = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let turn_for_mock = turn.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |_: &wiremock::Request| {
                let current = turn_for_mock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if current < 5 {
                    ResponseTemplate::new(200).set_body_string(sse(&[&format!(
                        r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"r{current}","function":{{"name":"read_file","arguments":"{{\"path\":\"candidate-{current}.toml\"}}"}}}}]}}}}]}}"#
                    )]))
                } else {
                    ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"The useful configuration was found."}}]}"#,
                    ]))
                }
            })
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "explore likely configuration paths",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(IntermittentReadTools {
                seen: seen.clone(),
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        let mut out = String::new();
        while let Some(line) = process.next_line().await {
            out.push_str(&line);
        }
        let status = process.child.wait().await.expect("lifeline");

        assert!(status.success());
        assert!(
            out.contains("The useful configuration was found."),
            "{out:?}"
        );
        assert_eq!(
            seen.lock().unwrap().len(),
            5,
            "all legitimate probes must execute"
        );
        assert!(!process
            .stderr_capture
            .lock()
            .unwrap()
            .join(" ")
            .contains("forced tool convergence"));
    }

    #[tokio::test]
    async fn an_identical_repeated_tool_call_is_answered_once_and_told_to_move_on() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // The observed failure: a 30B model asked for task_list() with the exact same
        // (empty) arguments seven rounds running, burning 10 185 tokens before the cap
        // fired. Re-running it returns identical bytes and teaches the model nothing.
        let model_turns = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_turns_for_mock = model_turns.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                model_turns_for_mock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                if body.get("tools").is_some() {
                    ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"mcp_list","arguments":"{}"}}]}}]}"#,
                    ]))
                } else {
                    ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"Final answer from the cached result."}}]}"#,
                    ]))
                }
            })
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "repeat forever",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(FakeTools { seen: seen.clone() })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        let mut out = String::new();
        while let Some(line) = process.next_line().await {
            out.push_str(&line);
        }
        let _ = process.child.wait().await;

        // The executor ran ONCE. Every later round got the first result back with a
        // note, so the loop cost one execution instead of eight.
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "an identical call must not be executed again"
        );
        assert_eq!(
            model_turns.load(std::sync::atomic::Ordering::SeqCst),
            4,
            "execute + one replay + one refusal + forced synthesis"
        );
        assert!(
            out.contains("Final answer from the cached result."),
            "the tools-withdrawn turn must let the model synthesize: {out:?}"
        );
    }

    #[tokio::test]
    async fn forward_chat_line_carries_openai_usage_across_to_the_done_sentinel() {
        // OpenAI splits what Ollama puts on one chunk: the text deltas, then a
        // usage-only frame, then `[DONE]`. The stream-scoped tally is what
        // keeps the token counts from being lost between the last two.
        use crate::agents::chat_codec::OpenAiCodec;
        use std::sync::{Arc, Mutex};
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(8);
        let stderr = Arc::new(Mutex::new(Vec::<String>::new()));
        let (mut done, mut err) = (false, false);
        let mut provider_error = None;
        let mut tally = TokenTally::default();
        let mut thinking_filter = LeadingThinkingFilter::default();

        for line in [
            r#"data: {"choices":[{"delta":{"content":"39"}}]}"#,
            r#"data: {"choices":[{"delta":{"content":"1"}}]}"#,
            r#"data: {"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":3}}"#,
            "data: [DONE]",
        ] {
            assert!(
                forward_chat_line(
                    &OpenAiCodec,
                    "Ollama",
                    line,
                    &tx,
                    &stderr,
                    &mut done,
                    &mut err,
                    &mut provider_error,
                    0,
                    &mut tally,
                    &mut thinking_filter,
                    &mut crate::agents::tools::ToolCallAccumulator::default(),
                    &mut false,
                )
                .await
            );
        }

        assert!(done && !err, "[DONE] ends the stream cleanly");
        drop(tx);
        let mut got = String::new();
        while let Some(s) = rx.recv().await {
            got.push_str(&s);
        }
        assert_eq!(got, "391");
        assert_eq!(
            stderr.lock().unwrap().as_slice(),
            &["ollama_tokens:12:3".to_string()],
            "usage from the earlier frame survives to the sentinel"
        );
    }

    #[tokio::test]
    async fn forward_chat_line_never_emits_litellm_reasoning_content() {
        use crate::agents::chat_codec::OpenAiCodec;
        use std::sync::{Arc, Mutex};

        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(8);
        let stderr = Arc::new(Mutex::new(Vec::<String>::new()));
        let (mut done, mut err) = (false, false);
        let mut provider_error = None;
        let mut tally = TokenTally::default();
        let mut thinking_filter = LeadingThinkingFilter::default();
        let mut pending_tools = crate::agents::tools::ToolCallAccumulator::default();

        for content in ["<thi", "nk>private scratchpad", "</think>Final answer"] {
            let line = format!(
                r#"data: {{"choices":[{{"delta":{{"content":{}}}}}]}}"#,
                serde_json::to_string(content).unwrap()
            );
            assert!(
                forward_chat_line(
                    &OpenAiCodec,
                    "Ollama",
                    &line,
                    &tx,
                    &stderr,
                    &mut done,
                    &mut err,
                    &mut provider_error,
                    0,
                    &mut tally,
                    &mut thinking_filter,
                    &mut pending_tools,
                    &mut false,
                )
                .await
            );
        }
        let trailing = thinking_filter.finish();
        if !trailing.is_empty() {
            tx.send(trailing).await.unwrap();
        }
        drop(tx);

        let mut visible = String::new();
        while let Some(chunk) = rx.recv().await {
            visible.push_str(&chunk);
        }
        assert_eq!(visible, "Final answer");
    }

    #[tokio::test]
    async fn forward_ollama_line_ignores_blank_and_malformed() {
        use std::sync::{Arc, Mutex};
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(8);
        let stderr = Arc::new(Mutex::new(Vec::<String>::new()));
        let (mut done, mut err) = (false, false);
        forward_ollama_line("   ", &tx, &stderr, &mut done, &mut err, 0).await; // blank tail buffer
        forward_ollama_line("{not json", &tx, &stderr, &mut done, &mut err, 0).await; // partial/garbage
        drop(tx);
        assert!(rx.recv().await.is_none(), "no content forwarded");
        assert!(stderr.lock().unwrap().is_empty(), "no token line captured");
        assert!(
            !done && !err,
            "garbage neither completes nor fails the stream"
        );
    }

    #[tokio::test]
    async fn forward_ollama_line_flags_prompt_that_filled_the_window() {
        use std::sync::{Arc, Mutex};
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(8);
        let stderr = Arc::new(Mutex::new(Vec::<String>::new()));
        let (mut done, mut err) = (false, false);
        // prompt_eval_count within 64 of num_ctx → Ollama silently dropped the
        // overflow; the terminal chunk must surface it (exact, unlike the
        // pre-flight estimate).
        forward_ollama_line(
            r#"{"message":{"content":""},"done":true,"prompt_eval_count":8180,"eval_count":3}"#,
            &tx,
            &stderr,
            &mut done,
            &mut err,
            8192,
        )
        .await;
        assert!(done && !err, "truncation warns, it does not fail the step");
        let lines = stderr.lock().unwrap().clone();
        assert!(
            lines.iter().any(|l| l.contains("Ollama truncation")),
            "{lines:?}"
        );

        // Comfortable margin → no truncation marker.
        let stderr2 = Arc::new(Mutex::new(Vec::<String>::new()));
        let (mut done2, mut err2) = (false, false);
        forward_ollama_line(
            r#"{"message":{"content":""},"done":true,"prompt_eval_count":4000,"eval_count":3}"#,
            &tx,
            &stderr2,
            &mut done2,
            &mut err2,
            8192,
        )
        .await;
        assert!(!stderr2
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("truncation")));
    }

    #[tokio::test]
    async fn forward_ollama_line_surfaces_in_band_error() {
        use std::sync::{Arc, Mutex};
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(8);
        let stderr = Arc::new(Mutex::new(Vec::<String>::new()));
        let (mut done, mut err) = (false, false);
        // Ollama's mid-stream error shape (HTTP 200, model crashed): used to
        // be silently ignored → step "succeeded" with empty output.
        forward_ollama_line(
            r#"{"error":"model runner has unexpectedly stopped"}"#,
            &tx,
            &stderr,
            &mut done,
            &mut err,
            0,
        )
        .await;
        drop(tx);
        assert!(err, "in-band error object must mark the stream failed");
        assert!(!done);
        assert!(rx.recv().await.is_none());
        let lines = stderr.lock().unwrap();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("model runner has unexpectedly stopped")),
            "error reason must reach the stderr tail shown on step failure: {lines:?}"
        );
    }

    #[test]
    fn parse_stream_text_delta_with_thinking_leak_is_skipped() {
        // End-to-end: a text_delta whose entire content is the leak should
        // NOT reach `full_response` as an empty chunk (which would still
        // count toward the downstream loop detector).
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"</thinking>"}}}"#;
        assert!(matches!(
            parse_claude_stream_line(line),
            StreamJsonEvent::Skip
        ));
    }

    #[test]
    fn parse_stream_text_delta_with_partial_thinking_leak_preserves_rest() {
        // Mixed chunk: legitimate text with a stray tag — keep the text.
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello</thinking> world"}}}"#;
        match parse_claude_stream_line(line) {
            StreamJsonEvent::Text(t) => assert_eq!(t, "Hello world"),
            other => panic!("Expected Text event, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_usage_from_message_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"message_delta","usage":{"input_tokens":100,"output_tokens":50}}}"#;
        match parse_claude_stream_line(line) {
            StreamJsonEvent::Usage {
                input_tokens,
                output_tokens,
                cost_usd,
            } => {
                assert_eq!(input_tokens, 100);
                assert_eq!(output_tokens, 50);
                assert!(cost_usd.is_none());
            }
            _ => panic!("Expected Usage event"),
        }
    }

    #[test]
    fn parse_stream_result_with_usage() {
        let line = r#"{"type":"result","subtype":"success","cost_usd":0.01,"usage":{"input_tokens":200,"output_tokens":100}}"#;
        match parse_claude_stream_line(line) {
            StreamJsonEvent::Usage {
                input_tokens,
                output_tokens,
                cost_usd,
            } => {
                assert_eq!(input_tokens, 200);
                assert_eq!(output_tokens, 100);
                assert!((cost_usd.unwrap() - 0.01).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Usage event from result"),
        }
    }

    #[test]
    fn parse_stream_result_without_usage() {
        let line = r#"{"type":"result","subtype":"success"}"#;
        assert!(matches!(
            parse_claude_stream_line(line),
            StreamJsonEvent::Skip
        ));
    }

    #[test]
    fn parse_stream_assistant_skipped() {
        let line = r#"{"type":"assistant","message":"full text so far"}"#;
        assert!(matches!(
            parse_claude_stream_line(line),
            StreamJsonEvent::Skip
        ));
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
        assert!(matches!(
            parse_claude_stream_line(line),
            StreamJsonEvent::Skip
        ));
    }

    #[test]
    fn parse_stream_event_without_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"message_start"}}"#;
        assert!(matches!(
            parse_claude_stream_line(line),
            StreamJsonEvent::Skip
        ));
    }

    #[test]
    fn parse_stream_zero_usage_skipped() {
        let line = r#"{"type":"stream_event","event":{"type":"message_delta","usage":{"input_tokens":0,"output_tokens":0}}}"#;
        assert!(matches!(
            parse_claude_stream_line(line),
            StreamJsonEvent::Skip
        ));
    }

    // ─── parse_token_usage ────────────────────────────────────────────────────

    #[test]
    fn repository_reads_have_a_larger_but_still_bounded_budget() {
        use crate::agents::tools::ToolRunMode;

        assert_eq!(max_calls_for_tool("read_file", ToolRunMode::General), 48);
        assert_eq!(max_calls_for_tool("mcp_list", ToolRunMode::General), 12);

        // Enumerating is not looping. Analysing one pull request walked eight
        // pages of the open web and was cut off on the ninth; a repository's
        // issues are read a page at a time, and a hundred issues is an
        // ordinary ask. Both stay bounded, and the loop guards are unchanged.
        assert_eq!(max_calls_for_tool("web_fetch", ToolRunMode::General), 120);
        assert_eq!(max_calls_for_tool("api_call", ToolRunMode::General), 60);
        assert_eq!(max_calls_for_tool("qa_run", ToolRunMode::General), 60);
        assert_eq!(max_calls_for_tool("web_fetch", ToolRunMode::Worker), 120);
        assert_eq!(
            max_calls_for_tool("search_text", ToolRunMode::Worker),
            24,
            "a real worker needed thirteen distinct symbol searches"
        );
        assert_eq!(
            max_calls_for_tool("search_text", ToolRunMode::General),
            12,
            "general/API agents keep the strict anti-loop guard"
        );
    }

    #[test]
    fn codex_tokens_from_stderr() {
        let stderr = vec!["some info".into(), "tokens used".into(), "1,234".into()];
        let (response, tokens) = parse_token_usage(&AgentType::Codex, "response text", &stderr);
        assert_eq!(tokens, 1234);
        assert_eq!(response, "response text"); // response not modified
    }

    #[test]
    fn codex_tokens_survive_trailing_stderr_diagnostics() {
        let stderr = vec![
            "startup".into(),
            "tokens used".into(),
            "12,345".into(),
            "warning: telemetry flush delayed".into(),
        ];
        let (response, tokens) = parse_token_usage(&AgentType::Codex, "response text", &stderr);
        assert_eq!(tokens, 12_345);
        assert_eq!(response, "response text");
    }

    #[test]
    fn codex_tokens_from_stdout_fallback() {
        let response = "Hello world\ntokens used\n5,678";
        let (cleaned, tokens) = parse_token_usage(&AgentType::Codex, response, &[]);
        assert_eq!(tokens, 5678);
        assert_eq!(cleaned, "Hello world"); // token lines stripped
    }

    #[test]
    fn codex_stdout_fallback_preserves_lines_after_usage() {
        let response = "Hello world\ntokens used\n5,678\nlate diagnostic";
        let (cleaned, tokens) = parse_token_usage(&AgentType::Codex, response, &[]);
        assert_eq!(tokens, 5678);
        assert_eq!(cleaned, "Hello world\nlate diagnostic");
    }

    // ─── KT-408 — Ollama/LiteLLM/NVIDIA multi-turn token summation ───────────

    /// The bug as measured: `parse_token_usage` returned on the FIRST
    /// `ollama_tokens:` marker, so any run that used tools reported only
    /// turn one's cost. One marker is pushed per turn (see `forward_chat_line`
    /// on `chunk.done`), so the real total is the sum across every turn.
    #[test]
    fn ollama_family_sums_every_turn_marker_not_just_the_first() {
        let stderr = vec![
            "ollama_tokens:5:2".to_string(),
            "some unrelated diagnostic line".to_string(),
            "ollama_tokens:9:3".to_string(),
        ];
        let (_, tokens) = parse_token_usage(&AgentType::Ollama, "response", &stderr);
        assert_eq!(tokens, 19, "5+2+9+3, not just the first marker's 5+2");

        // Same wire format, same bug surface, for the other HTTP backends
        // that share this branch. Custom is the durable AgentType used by
        // OpenRouter and user-defined OpenAI-compatible connections.
        let (_, tokens) = parse_token_usage(&AgentType::LiteLlm, "response", &stderr);
        assert_eq!(tokens, 19);
        let (_, tokens) = parse_token_usage(&AgentType::Nvidia, "response", &stderr);
        assert_eq!(tokens, 19);
        let (_, tokens) = parse_token_usage(&AgentType::Custom, "response", &stderr);
        assert_eq!(tokens, 19);
    }

    /// A malformed marker (truncated by a stream cut, or simply not this
    /// format) must not erase the valid totals already accumulated before or
    /// after it — it is skipped, not fatal.
    #[test]
    fn a_malformed_marker_does_not_erase_the_valid_ones_around_it() {
        let stderr = vec![
            "ollama_tokens:5:2".to_string(),
            "ollama_tokens:not-a-number:3".to_string(),
            "ollama_tokens:".to_string(),
            "ollama_tokens:9".to_string(),
            "ollama_tokens:9:3".to_string(),
        ];
        let (_, tokens) = parse_token_usage(&AgentType::Ollama, "response", &stderr);
        assert_eq!(
            tokens, 19,
            "only the two well-formed markers (5:2 and 9:3) contribute"
        );
    }

    #[test]
    fn a_single_turn_run_still_sums_to_exactly_that_turns_cost() {
        let stderr = vec!["ollama_tokens:12:3".to_string()];
        let (_, tokens) = parse_token_usage(&AgentType::Ollama, "response", &stderr);
        assert_eq!(tokens, 15, "the single-turn case must not regress");
    }

    #[test]
    fn no_marker_at_all_yields_zero_not_an_error() {
        let (_, tokens) = parse_token_usage(&AgentType::Ollama, "response", &[]);
        assert_eq!(tokens, 0);
    }

    #[test]
    fn http_turn_telemetry_is_provider_neutral_and_never_keeps_payloads() {
        let mut stderr = Vec::new();
        for (turn, provider, phase) in [
            (1, "ollama", "read"),
            (2, "litellm", "mutation"),
            (3, "nvidia", "delivery"),
        ] {
            stderr.push(format!(
                "{HTTP_TURN_TRACE_PREFIX}{}",
                serde_json::json!({
                    "version": 1,
                    "turn": turn,
                    "provider": provider,
                    "phase": phase,
                    "prompt_tokens": turn * 100,
                    "eval_tokens": turn * 10,
                    "duration_ms": turn * 1000,
                    "provider_ok": true,
                    "requested_tools": if turn == 1 {
                        vec!["read_file", "secret=must-not-survive"]
                    } else {
                        vec!["task_exec_deliver"]
                    },
                    "arguments": {"api_key": "never-persist-me"},
                    "result": "never-persist-me-either"
                })
            ));
        }
        stderr.push(format!(
            "{HTTP_TOOL_EXEC_TRACE_PREFIX}{}",
            serde_json::json!({
                "version": 1,
                "turn": 1,
                "name": "read_file",
                "ok": true,
                "arguments": {"path": "private.txt"}
            })
        ));

        let turns = parse_http_turn_telemetry(&stderr);
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].provider, "ollama");
        assert_eq!(turns[1].provider, "litellm");
        assert_eq!(turns[2].provider, "nvidia");
        assert_eq!(turns[0].prompt_tokens, 100);
        assert_eq!(turns[0].eval_tokens, 10);
        assert_eq!(turns[0].executed_tools[0].name, "read_file");
        assert_eq!(turns[0].requested_tools[1], "invalid_tool_name");
        let durable = serde_json::to_string(&turns).unwrap();
        assert!(!durable.contains("never-persist-me"));
        assert!(!durable.contains("private.txt"));
        assert!(!durable.contains("api_key"));
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
    fn gemini_strips_mcp_issues_prefix() {
        // Pin user-reported bug 2026-05-10: when one or more MCPs in the
        // project's `.mcp.json` fail handshake (auth gone stale, missing
        // binaries, network blocks), Gemini CLI 0.32 prepends a noisy
        // header to its reply that confuses the user (they assume Gemini
        // failed when it didn't). parse_token_usage must strip it.
        let raw =
            "MCP issues detected. Run /mcp list for status.\nVoici la réponse réelle de Gemini.";
        let (cleaned, tokens) = parse_token_usage(&AgentType::GeminiCli, raw, &[]);
        assert_eq!(tokens, 0);
        assert_eq!(cleaned, "Voici la réponse réelle de Gemini.");
    }

    #[test]
    fn gemini_strips_mcp_issues_when_inline() {
        // The same marker sometimes lands without a leading newline (Gemini
        // streams it as a continuation of the previous line when the discovery
        // error fires mid-output).
        let raw = "MCP issues detected. Run /mcp list for status.Réponse: ok";
        let (cleaned, _) = parse_token_usage(&AgentType::GeminiCli, raw, &[]);
        assert_eq!(cleaned, "Réponse: ok");
    }

    #[test]
    fn gemini_drops_mcp_debug_lines() {
        // Gemini debug output (`Server '…' supports tool updates...` and
        // `[MCP error] …`) leaks into stdout on some MCP server configs.
        // Strip it so the saved transcript only carries the agent's own
        // reply.
        let raw = "\
Server 'GitLab' supports tool updates. Listening for changes...
[MCP error] Error during discovery for MCP server 'context7': MCP error -32000
Réponse de Gemini.
Suite de la réponse.";
        let (cleaned, _) = parse_token_usage(&AgentType::GeminiCli, raw, &[]);
        assert_eq!(cleaned, "Réponse de Gemini.\nSuite de la réponse.");
    }

    #[test]
    fn gemini_keeps_clean_response_unchanged() {
        let raw = "Réponse propre sans préambule MCP.";
        let (cleaned, _) = parse_token_usage(&AgentType::GeminiCli, raw, &[]);
        assert_eq!(cleaned, raw);
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
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::ClaudeCode, "test prompt", true, "", None);
        assert!(
            args.contains(&"--dangerously-skip-permissions".to_string()),
            "Claude Code with full_access should include --dangerously-skip-permissions"
        );
    }

    #[test]
    fn claude_code_no_full_access_omits_skip_permissions() {
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::ClaudeCode, "test prompt", false, "", None);
        assert!(
            !args.contains(&"--dangerously-skip-permissions".to_string()),
            "Claude Code without full_access should NOT include --dangerously-skip-permissions"
        );
    }

    #[test]
    fn claude_task_worker_uses_fail_closed_workspace_sandbox() {
        let worktree = tempfile::tempdir().unwrap();
        let (_, _, args, _, _, _) = super::super::agent_command_with_task_worker_policy(
            &AgentType::ClaudeCode,
            "test prompt",
            true,
            "worker context",
            None,
            None,
            true,
            Some(worktree.path()),
            None,
        );

        assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
        assert_eq!(
            args.get(
                args.iter()
                    .position(|arg| arg == "--setting-sources")
                    .unwrap()
                    + 1
            )
            .map(String::as_str),
            Some("")
        );
        assert_eq!(
            args.get(
                args.iter()
                    .position(|arg| arg == "--permission-mode")
                    .unwrap()
                    + 1
            )
            .map(String::as_str),
            Some("acceptEdits")
        );
        assert!(args.contains(&"mcp__kronn-internal__task_exec_commit".to_string()));
        assert!(args.contains(&"mcp__kronn-internal__task_exec_deliver".to_string()));

        let settings = args
            .get(args.iter().position(|arg| arg == "--settings").unwrap() + 1)
            .unwrap();
        let settings: serde_json::Value = serde_json::from_str(settings).unwrap();
        assert_eq!(settings["sandbox"]["enabled"], true);
        assert_eq!(settings["sandbox"]["failIfUnavailable"], true);
        assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
        assert_eq!(
            settings["sandbox"]["filesystem"]["allowWrite"],
            serde_json::json!([worktree.path().canonicalize().unwrap()])
        );
    }

    #[test]
    fn claude_task_worker_receipt_proves_unrelated_catalogue_is_not_spawned() {
        let worktree = tempfile::tempdir().unwrap();
        let unrelated: Vec<String> = (0..500)
            .map(|index| {
                format!(
                    "/synthetic/unrelated/{}/.kronn/worktrees/task-{index}",
                    "x".repeat(256)
                )
            })
            .collect();
        let oversized_catalogue = serde_json::to_vec(&unrelated).unwrap();
        assert!(
            oversized_catalogue.len() > MAX_SINGLE_ARG_BYTES,
            "the synthetic catalogue must exceed Claude's per-argument guard"
        );

        let (_, _, mut args, _, _, _) = super::super::agent_command_with_task_worker_policy(
            &AgentType::ClaudeCode,
            "test prompt",
            false,
            "worker context",
            None,
            None,
            true,
            Some(worktree.path()),
            None,
        );
        let mcp_config = r#"{"mcpServers":{"kronn-internal":{}}}"#;
        super::super::insert_claude_mcp_config(&mut args, mcp_config.into(), true);
        let stdin_prompt = args.pop().unwrap();
        let arg_refs: Vec<&std::ffi::OsStr> = args
            .iter()
            .map(|arg| std::ffi::OsStr::new(arg.as_str()))
            .collect();
        let environment = [(
            std::ffi::OsStr::new("SYNTHETIC_ENV"),
            std::ffi::OsStr::new("bounded"),
        )];
        let receipt = super::super::invocation_size_receipt(
            std::ffi::OsStr::new("claude"),
            &arg_refs,
            &environment,
            stdin_prompt.len(),
        );
        let settings_index = args.iter().position(|arg| arg == "--settings").unwrap();
        let settings: serde_json::Value = serde_json::from_str(&args[settings_index + 1]).unwrap();

        assert_eq!(
            settings["sandbox"]["filesystem"]["allowWrite"],
            serde_json::json!([worktree.path().canonicalize().unwrap()])
        );
        assert_eq!(receipt.settings_bytes, args[settings_index + 1].len());
        assert_eq!(receipt.mcp_config_bytes, mcp_config.len());
        assert_eq!(receipt.system_prompt_bytes, "worker context".len());
        assert_eq!(receipt.stdin_bytes, "test prompt".len());
        assert!(receipt.settings_bytes < 4096);
        assert!(receipt.max_argument_bytes < MAX_SINGLE_ARG_BYTES);
        assert_eq!(receipt.validate_single_argument_limit(), Ok(()));
        assert!(
            receipt.argv_payload_bytes + receipt.environment_payload_bytes
                < oversized_catalogue.len(),
            "the measured spawn payload must remain independent of the oversized catalogue"
        );
    }

    #[test]
    fn claude_task_worker_refuses_oversized_mcp_config_before_spawn() {
        let secret_marker = "must-not-leak";
        let oversized_mcp_config = format!("{}{}", secret_marker, "x".repeat(MAX_SINGLE_ARG_BYTES));
        let args = [
            std::ffi::OsStr::new("--print"),
            std::ffi::OsStr::new("--mcp-config"),
            std::ffi::OsStr::new(&oversized_mcp_config),
        ];
        let receipt =
            super::super::invocation_size_receipt(std::ffi::OsStr::new("claude"), &args, &[], 0);

        let error = receipt.validate_single_argument_limit().unwrap_err();
        assert!(error.contains("refused before spawn"));
        assert!(error.contains("mcp_config_bytes"));
        assert!(error.contains("max_argument_bytes="));
        assert!(error.contains("task_exec_reassign"));
        assert!(!error.contains(secret_marker));
    }

    #[test]
    fn claude_task_worker_truncates_system_prompt_within_pre_spawn_limit() {
        let worktree = tempfile::tempdir().unwrap();
        let oversized_system_prompt = "é".repeat(MAX_SINGLE_ARG_BYTES);
        let (_, _, mut args, _, _, _) = super::super::agent_command_with_task_worker_policy(
            &AgentType::ClaudeCode,
            "test prompt",
            false,
            &oversized_system_prompt,
            None,
            None,
            true,
            Some(worktree.path()),
            None,
        );
        let stdin_prompt = args.pop().unwrap();

        let (original_bytes, truncated_bytes) =
            super::super::truncate_claude_system_prompt_argument(&mut args).unwrap();
        let system_prompt_index = args
            .iter()
            .position(|argument| argument == "--append-system-prompt")
            .unwrap();
        let system_prompt = &args[system_prompt_index + 1];
        let arg_refs: Vec<&std::ffi::OsStr> = args
            .iter()
            .map(|argument| std::ffi::OsStr::new(argument.as_str()))
            .collect();
        let receipt = super::super::invocation_size_receipt(
            std::ffi::OsStr::new("claude"),
            &arg_refs,
            &[],
            stdin_prompt.len(),
        );

        assert!(original_bytes > MAX_SINGLE_ARG_BYTES);
        assert_eq!(truncated_bytes, system_prompt.len());
        assert!(truncated_bytes <= MAX_SINGLE_ARG_BYTES);
        assert!(system_prompt.ends_with(super::super::CLAUDE_SYSTEM_PROMPT_TRUNCATION_MARKER));
        assert_eq!(receipt.system_prompt_bytes, truncated_bytes);
        assert_eq!(receipt.validate_single_argument_limit(), Ok(()));
    }

    #[test]
    fn claude_task_worker_auth_probe_accepts_logged_in_status() {
        assert_eq!(
            super::super::claude_task_worker_auth_result(br#"{"loggedIn":true}"#, true),
            Ok(())
        );
    }

    fn synthetic_git_worktree_catalogue(extra_worktrees: usize) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        std::fs::create_dir(&main).unwrap();
        let git = |cwd: &std::path::Path, args: &[&str]| {
            let output = crate::core::cmd::sync_cmd("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&main, &["init", "-q"]);
        git(&main, &["config", "user.email", "test@kronn.local"]);
        git(&main, &["config", "user.name", "Kronn Test"]);
        std::fs::write(main.join("seed"), "seed").unwrap();
        git(&main, &["add", "seed"]);
        git(&main, &["commit", "-qm", "seed"]);
        for index in 0..extra_worktrees {
            let path = root.path().join(format!("worktree-{index}"));
            git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "--detach",
                    path.to_str().unwrap(),
                    "HEAD",
                ],
            );
        }
        root
    }

    fn catalogue_root(
        label: &str,
        path: std::path::PathBuf,
    ) -> super::super::ClaudeSandboxCatalogueRoot {
        super::super::ClaudeSandboxCatalogueRoot {
            label: label.into(),
            path,
        }
    }

    fn catalogue_project(
        path: &std::path::Path,
        linked: &[(&str, String)],
    ) -> crate::models::Project {
        let now = chrono::Utc::now();
        crate::models::Project {
            id: "p1".into(),
            name: "proj".into(),
            path: path.to_string_lossy().into_owned(),
            repo_url: None,
            token_override: None,
            ai_config: crate::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: crate::models::AiAuditStatus::NoTemplate,
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            write_access: None,
            mcp_sync_report: None,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: linked
                .iter()
                .enumerate()
                .map(|(index, (name, location))| crate::models::LinkedRepo {
                    id: format!("lr-{index}"),
                    name: (*name).into(),
                    kind: "api".into(),
                    location: location.clone(),
                    description: String::new(),
                })
                .collect(),
            workspace: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn claude_catalogue_skips_a_linked_repository_missing_on_this_host() {
        let repo = synthetic_git_worktree_catalogue(1);
        let missing = repo.path().join("absent-linked-repo");
        let project = catalogue_project(
            &repo.path().join("main"),
            &[
                ("ghost", missing.to_string_lossy().into_owned()),
                ("remote", "https://github.com/org/remote".into()),
                (
                    "sibling",
                    repo.path()
                        .join("worktree-0")
                        .to_string_lossy()
                        .into_owned(),
                ),
            ],
        );
        let roots = super::super::claude_sandbox_catalogue_roots(&project);
        let labels: Vec<&str> = roots.iter().map(|root| root.label.as_str()).collect();
        assert_eq!(
            labels,
            ["project `proj`", "linked repository `sibling`"],
            "a missing path and a URL are not measured"
        );
        let receipt = super::super::claude_sandbox_catalogue_receipt(&roots).unwrap();
        assert_eq!(receipt.common_dir_count, 1);
        assert_eq!(receipt.worktree_count, 2);
        assert_eq!(receipt.validate(), Ok(()));
    }

    #[test]
    fn claude_catalogue_names_an_existing_linked_repository_it_cannot_read() {
        let repo = synthetic_git_worktree_catalogue(0);
        let not_git = tempfile::tempdir().unwrap();
        let secret_path = not_git.path().to_string_lossy().into_owned();
        let project = catalogue_project(
            &repo.path().join("main"),
            &[("plain-folder", secret_path.clone())],
        );
        let roots = super::super::claude_sandbox_catalogue_roots(&project);
        assert_eq!(roots.len(), 2, "an existing path stays fail-closed");
        let error = super::super::claude_sandbox_catalogue_receipt(&roots).unwrap_err();
        assert!(error.contains("reason_code=claude_sandbox_catalogue_unreadable"));
        assert!(
            error.contains("linked repository `plain-folder`"),
            "{error}"
        );
        assert!(!error.contains("task_exec_reassign"), "{error}");
        assert!(!error.contains(&secret_path));
    }

    #[test]
    fn claude_catalogue_names_the_project_root_when_it_is_not_readable() {
        let not_git = tempfile::tempdir().unwrap();
        let project = catalogue_project(not_git.path(), &[]);
        let error = super::super::claude_sandbox_catalogue_receipt(
            &super::super::claude_sandbox_catalogue_roots(&project),
        )
        .unwrap_err();
        assert!(
            error.contains("the Git root of project `proj` is inaccessible"),
            "{error}"
        );
        assert!(!error.contains("task_exec_reassign"), "{error}");
    }

    #[test]
    fn claude_task_worker_accepts_catalogue_below_conservative_bounds() {
        let repo = synthetic_git_worktree_catalogue(3);
        let duplicate_checkout = repo.path().join("worktree-0");
        let receipt = super::super::claude_sandbox_catalogue_receipt(&[
            catalogue_root("project `main`", repo.path().join("main")),
            catalogue_root("linked repository `dup`", duplicate_checkout),
        ])
        .unwrap();
        assert_eq!(receipt.common_dir_count, 1);
        assert_eq!(receipt.worktree_count, 4);
        assert!(receipt.worktree_bytes > 0);
        assert_eq!(receipt.validate(), Ok(()));
    }

    #[test]
    fn claude_task_worker_refuses_large_catalogue_without_leaking_paths() {
        let repo = synthetic_git_worktree_catalogue(65);
        std::fs::write(
            repo.path().join(".claude.json"),
            br#"{"projects":{"/one/project":{}}}"#,
        )
        .unwrap();
        let secret_path = repo.path().to_string_lossy().to_string();
        let receipt = super::super::claude_sandbox_catalogue_receipt(&[catalogue_root(
            "project `main`",
            repo.path().join("main"),
        )])
        .unwrap();
        let error = receipt.validate().unwrap_err();
        assert_eq!(receipt.common_dir_count, 1);
        assert_eq!(receipt.worktree_count, 66);
        assert!(error.contains("reason_code=claude_sandbox_catalogue_unsafe"));
        assert!(error.contains("git_common_dir_count=1"));
        assert!(error.contains("git_worktree_count=66"));
        assert!(error.contains("git_worktree_bytes="));
        assert!(error.contains("task_exec_reassign"));
        assert!(!error.contains(&secret_path));
        assert!(!error.contains("worktree-0"));
    }

    #[test]
    fn claude_current_version_is_not_used_as_sandbox_recovery_evidence() {
        let source = include_str!("../core/versions.rs");
        assert!(source.contains("AgentType::ClaudeCode => Some("));
        assert!(!source.contains("MIN_CLAUDE_TASK_WORKER_VERSION"));
    }

    #[test]
    fn claude_task_worker_auth_probe_reassigns_when_logged_out() {
        let error = super::super::claude_task_worker_auth_result(
            br#"{"loggedIn":false,"account":"must-not-leak"}"#,
            false,
        )
        .unwrap_err();
        assert!(error.contains("loggedIn=false"));
        assert!(error.contains("task_exec_reassign"));
        assert!(!error.contains("must-not-leak"));
    }

    #[test]
    fn claude_task_worker_auth_probe_reassigns_on_malformed_status() {
        let error = super::super::claude_task_worker_auth_result(b"malformed must-not-leak", true)
            .unwrap_err();
        assert!(error.contains("unrecognized response"));
        assert!(error.contains("task_exec_reassign"));
        assert!(!error.contains("must-not-leak"));
    }

    #[test]
    fn copilot_task_worker_preflight_accepts_a_bounded_account_response() {
        assert_eq!(
            super::super::parse_copilot_task_worker_preflight(b"Signed in as octocat", true),
            CopilotTaskWorkerPreflight::Usable
        );
    }

    #[test]
    fn copilot_task_worker_preflight_classifies_invalid_auth_and_malformed_output() {
        assert_eq!(
            super::super::parse_copilot_task_worker_preflight(b"auth error", false),
            CopilotTaskWorkerPreflight::AuthInvalid
        );
        assert_eq!(
            super::super::parse_copilot_task_worker_preflight(b"", true),
            CopilotTaskWorkerPreflight::Malformed
        );
        let error = super::super::copilot_task_worker_preflight_error(
            CopilotTaskWorkerPreflight::Malformed,
        );
        assert!(error.contains("task_exec_reassign"));
        assert!(!error.contains("auth error"));
    }

    #[test]
    fn copilot_task_worker_preflight_spawn_failure_is_actionable_and_secret_free() {
        let error = super::super::copilot_task_worker_preflight_error(
            CopilotTaskWorkerPreflight::SpawnFailed,
        );
        assert!(error.contains("could not be invoked"));
        assert!(error.contains("task_exec_reassign"));
        assert_eq!(
            CopilotTaskWorkerPreflight::SpawnFailed.reason_code(),
            Some("copilot_preflight_spawn_failed")
        );
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn copilot_task_worker_preflight_times_out_and_terminates_the_child() {
        let mut child = crate::core::cmd::async_cmd("sh")
            .args(["-c", "read -r ignored"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().expect("spawn gives us the real child identity") as i32;
        // Hold stdin open: the child cannot complete the read, even if its
        // startup is delayed. No CPU spin or race against an in-child PID file.
        let _stdin = child.stdin.take().unwrap();
        let result = super::super::wait_copilot_preflight_child_with_timeout(
            child,
            std::time::Duration::from_millis(100),
        )
        .await;

        assert_eq!(result, Err(CopilotTaskWorkerPreflight::TimedOut));
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "the timed-out child must already be gone and collected"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert_eq!(
            CopilotTaskWorkerPreflight::TimedOut.reason_code(),
            Some("copilot_preflight_timed_out")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copilot_preflight_capture_keeps_stdout_stderr_and_spawn_failure() {
        let dir = tempfile::tempdir().unwrap();
        let output = super::super::run_copilot_task_worker_preflight_with_timeout(
            (
                "sh".into(),
                vec![
                    "-c".into(),
                    "printf 'fixture account'; printf 'fixture diagnostic' >&2; exit 7".into(),
                ],
                false,
            ),
            dir.path(),
            super::super::COPILOT_TASK_WORKER_PREFLIGHT_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(output.stdout, b"fixture account");
        assert_eq!(output.stderr, b"fixture diagnostic");
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(
            super::super::run_copilot_task_worker_preflight_with_timeout(
                (
                    dir.path().join("absent-preflight").display().to_string(),
                    Vec::new(),
                    false
                ),
                dir.path(),
                super::super::COPILOT_TASK_WORKER_PREFLIGHT_TIMEOUT,
            )
            .await,
            Err(CopilotTaskWorkerPreflight::SpawnFailed)
        );
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn copilot_preflight_full_spawn_path_preserves_its_four_second_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let timeout = super::super::COPILOT_TASK_WORKER_PREFLIGHT_TIMEOUT;
        assert_eq!(timeout, std::time::Duration::from_secs(4));
        let started = tokio::time::Instant::now();
        // No startup handshake is needed: a child that has not yet run is
        // still a live child the deadline must terminate. The paused clock
        // advances the production deadline without burning four wall seconds.
        let result = super::super::run_copilot_task_worker_preflight_with_timeout(
            (
                "sh".into(),
                vec!["-c".into(), "while :; do :; done".into()],
                false,
            ),
            dir.path(),
            timeout,
        )
        .await;
        assert_eq!(result, Err(CopilotTaskWorkerPreflight::TimedOut));
        assert_eq!(started.elapsed(), timeout);
    }

    #[test]
    fn claude_task_worker_command_receipt_contains_sizes_not_values() {
        let secret_marker = "must-not-leak";
        let settings = r#"{"sandbox":{"enabled":true}}"#;
        let mut command = crate::core::cmd::async_cmd("claude");
        command
            .args([
                "--print",
                "--settings",
                settings,
                "--mcp-config",
                secret_marker,
                "--append-system-prompt",
                "bounded context",
            ])
            .env("KRONN_RECEIPT_TEST_SECRET", secret_marker);

        let receipt = super::super::command_invocation_size_receipt(&command, Some("stdin prompt"));
        let rendered = receipt.compact();

        assert_eq!(receipt.settings_bytes, settings.len());
        assert_eq!(receipt.mcp_config_bytes, secret_marker.len());
        assert_eq!(receipt.system_prompt_bytes, "bounded context".len());
        assert_eq!(receipt.stdin_bytes, "stdin prompt".len());
        assert!(receipt.environment_payload_bytes > secret_marker.len());
        assert!(!rendered.contains(secret_marker));
        assert!(!rendered.contains(settings));
    }

    #[test]
    fn a_nul_byte_is_named_by_its_carrier_never_by_its_value() {
        let secret_with_nul = "sk-ant-must-not-leak\0trailing";

        // In an environment variable — the likeliest source: a key decrypted
        // with a stale material, or a config read as UTF-16.
        let mut command = crate::core::cmd::async_cmd("claude");
        command
            .args(["--print", "hello"])
            .env("ANTHROPIC_API_KEY", secret_with_nul);
        let offender =
            super::super::nul_byte_offender(&command).expect("a NUL byte must be detected");
        assert!(
            offender.contains("ANTHROPIC_API_KEY"),
            "the variable must be named, got: {offender}"
        );
        assert!(
            !offender.contains("must-not-leak"),
            "the value must never be reported, got: {offender}"
        );

        // In an argument — identified by the flag it follows, since positions
        // shift between agents.
        let mut command = crate::core::cmd::async_cmd("claude");
        command.args([
            "--print",
            "--append-system-prompt",
            "context with a \0 inside",
            "hello",
        ]);
        let offender =
            super::super::nul_byte_offender(&command).expect("a NUL byte must be detected");
        assert!(
            offender.contains("--append-system-prompt"),
            "the flag must be named, got: {offender}"
        );
        assert!(!offender.contains("context with"));

        // The working directory: Kronn derives it from a project path it did
        // not necessarily create, and it fails the spawn just the same.
        let mut command = crate::core::cmd::async_cmd("claude");
        command.arg("--print").current_dir("/tmp/pro\0ject");
        let offender =
            super::super::nul_byte_offender(&command).expect("a NUL byte must be detected");
        assert!(
            offender.contains("working directory"),
            "the working directory must be named, got: {offender}"
        );

        // The program name.
        let mut command = crate::core::cmd::async_cmd("cla\0ude");
        command.arg("--print");
        let offender =
            super::super::nul_byte_offender(&command).expect("a NUL byte must be detected");
        assert!(offender.contains("program name"), "got: {offender}");

        // A clean command must not be refused.
        let mut command = crate::core::cmd::async_cmd("claude");
        command
            .args(["--print", "--append-system-prompt", "clean", "hello"])
            .env("ANTHROPIC_API_KEY", "sk-ant-clean")
            .current_dir("/tmp");
        assert!(
            super::super::nul_byte_offender(&command).is_none(),
            "a clean invocation must pass"
        );
    }

    /// The argument/program/cwd checks lean on a standard-library placeholder,
    /// so pin the behaviour they depend on: if a future release stops
    /// substituting `<string-with-nul>`, this fails instead of the detection
    /// silently going blind.
    #[test]
    fn the_standard_library_still_masks_nul_bearing_values_it_cannot_encode() {
        let mut command = std::process::Command::new("/bin/echo");
        command.arg("abc\0def").current_dir("/tm\0p");

        let masked = command
            .get_args()
            .next()
            .expect("one argument")
            .to_string_lossy()
            .into_owned();
        assert_eq!(masked, "<string-with-nul>");
        assert_eq!(
            command
                .get_current_dir()
                .expect("a cwd")
                .to_string_lossy()
                .into_owned(),
            "<string-with-nul>"
        );

        // Environment values, by contrast, keep their NUL — which is why the
        // detection needs both a byte scan and the placeholder check.
        let mut command = std::process::Command::new("/bin/echo");
        command.env("KRONN_PROBE", "abc\0def");
        let (_, value) = command.get_envs().next().expect("one variable");
        assert!(
            value.expect("a value").to_string_lossy().contains('\0'),
            "environment values are handed back unmasked"
        );
    }

    #[test]
    fn claude_task_worker_allows_exact_status_commit_and_delivery_tools() {
        let (_, _, args, _, _, _) = super::super::agent_command_with_task_worker_policy(
            &AgentType::ClaudeCode,
            "test prompt",
            true,
            "worker context",
            None,
            None,
            true,
            None,
            None,
        );

        let allowed_tools_index = args.iter().position(|arg| arg == "--allowedTools").unwrap();
        let permission_mode_index = args
            .iter()
            .position(|arg| arg == "--permission-mode")
            .unwrap();

        assert_eq!(
            &args[allowed_tools_index + 1..permission_mode_index],
            [
                "mcp__kronn-internal__task_exec_status",
                "mcp__kronn-internal__task_exec_commit",
                "mcp__kronn-internal__task_exec_deliver",
            ]
        );
    }

    #[test]
    fn codex_task_worker_forces_workspace_write_despite_full_access() {
        let (_, _, args, _, _, _) = super::super::agent_command_with_task_worker_policy(
            &AgentType::Codex,
            "test prompt",
            true,
            "worker context",
            None,
            None,
            true,
            None,
            None,
        );

        assert!(args.contains(&"--sandbox=workspace-write".to_string()));
        assert!(args.contains(&"--ignore-user-config".to_string()));
        assert!(args.contains(&"--ignore-rules".to_string()));
        assert!(!args.contains(&"--sandbox=danger-full-access".to_string()));
        assert!(!args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(
            !args.contains(&"--add-dir".to_string()),
            "shared Git roots must never enter the model sandbox"
        );

        let override_index = args
            .iter()
            .position(|arg| arg == "-c")
            .expect("isolated worker MCP override");
        let override_value = &args[override_index + 1];
        let parsed: toml::Value =
            toml::from_str(override_value).expect("worker override must be valid TOML document");
        let internal = &parsed["mcp_servers"]["kronn-internal"];
        assert_eq!(internal["command"].as_str(), Some("python3"));
        assert!(internal["args"]
            .as_array()
            .is_some_and(|args| args.len() == 1 && args[0].as_str().is_some()));
        assert!(internal["env_vars"].as_array().is_some_and(|vars| vars
            .iter()
            .any(|v| v.as_str() == Some("KRONN_TASK_WORKER_CONTEXT"))));
        assert_eq!(internal["startup_timeout_sec"].as_integer(), Some(30));
        assert_eq!(internal["required"].as_bool(), Some(true));
        assert_eq!(
            internal["enabled_tools"],
            toml::Value::Array(vec![
                toml::Value::String("task_exec_status".into()),
                toml::Value::String("task_exec_commit".into()),
                toml::Value::String("task_exec_deliver".into()),
            ])
        );
        assert_eq!(
            internal["default_tools_approval_mode"].as_str(),
            Some("prompt")
        );
        assert_eq!(
            internal["tools"]["task_exec_status"]["approval_mode"].as_str(),
            Some("approve")
        );
        assert_eq!(
            internal["tools"]["task_exec_commit"]["approval_mode"].as_str(),
            Some("approve")
        );
        assert_eq!(
            internal["tools"]["task_exec_deliver"]["approval_mode"].as_str(),
            Some("approve")
        );
        assert_eq!(
            internal["tools"].as_table().map(toml::Table::len),
            Some(3),
            "no other worker MCP tool may be auto-approved"
        );
        assert_eq!(
            parsed["mcp_servers"].as_table().map(toml::Table::len),
            Some(1),
            "an isolated worker must inherit no user MCP server"
        );
    }

    #[test]
    fn codex_task_worker_mcp_override_fails_closed_without_bridge_script() {
        assert!(super::super::render_codex_task_worker_mcp_override(None).is_none());
        let rendered = super::super::render_codex_task_worker_mcp_override(Some(
            super::super::InternalMcpCommand::script("/tmp/disc-introspection-mcp.py".into()),
        ))
        .expect("a concrete bridge path is renderable");
        assert!(rendered.contains("command=\"python3\""));
        assert!(rendered.contains("/tmp/disc-introspection-mcp.py"));
        assert!(rendered.contains("KRONN_TASK_WORKER_CONTEXT"));
    }

    #[test]
    fn packaged_internal_mcp_runs_without_python_and_preserves_worker_scope() {
        let dir = tempfile::tempdir().expect("temporary install");
        let executable = dir.path().join("Kronn MCP é.exe");
        std::fs::write(&executable, b"bundle fixture").expect("executable fixture");
        let launch = super::super::resolve_internal_mcp_command(
            Some(executable.clone().into_os_string()),
            || panic!("a packaged launch must not search the checkout"),
        )
        .expect("installed bridge");
        assert_eq!(launch.command, executable.to_string_lossy());
        assert!(launch.args.is_empty());
        let rendered = super::super::render_codex_task_worker_mcp_override(Some(launch))
            .expect("worker configuration");
        let parsed: toml::Value = toml::from_str(&rendered).expect("valid TOML");
        let internal = &parsed["mcp_servers"]["kronn-internal"];
        assert_eq!(internal["command"].as_str(), executable.to_str());
        assert_eq!(internal["args"].as_array().map(Vec::len), Some(0));
        assert_eq!(internal["required"].as_bool(), Some(true));
        assert_eq!(internal["enabled_tools"].as_array().map(Vec::len), Some(3));
        assert_eq!(
            parsed["mcp_servers"].as_table().map(toml::Table::len),
            Some(1)
        );
    }

    #[test]
    fn missing_packaged_bridge_never_falls_back_to_the_build_checkout() {
        let dir = tempfile::tempdir().expect("temporary install");
        assert!(super::super::resolve_internal_mcp_command(
            Some(dir.path().join("missing.exe").into_os_string()),
            || panic!("missing package must fail closed"),
        )
        .is_none());
        assert!(super::super::resolve_internal_mcp_command(
            Some(dir.path().as_os_str().to_owned()),
            || panic!("a directory is not a runnable bundle"),
        )
        .is_none());
        let legacy = super::super::resolve_internal_mcp_command(None, || {
            Some("/app/scripts/disc-introspection-mcp.py".into())
        })
        .expect("existing Docker/script launch");
        assert_eq!(legacy.command, "python3");
        assert_eq!(legacy.args, ["/app/scripts/disc-introspection-mcp.py"]);
    }

    #[test]
    fn packaged_project_mcp_config_binds_the_desktop_instance() {
        const PROBE: &str = "KRONN_TEST_PACKAGED_CONFIG";
        if std::env::var_os(PROBE).is_none() {
            let dir = tempfile::tempdir().expect("temporary installation");
            let bridge = dir.path().join("Kronn MCP é.exe");
            std::fs::write(&bridge, b"bundle fixture").expect("bridge file");
            let output = crate::core::cmd::sync_cmd(
                std::env::current_exe().expect("test executable"),
            )
            .args(["--exact",
                "agents::runner::runner_test::tests::packaged_project_mcp_config_binds_the_desktop_instance",
                "--nocapture"])
            .env(PROBE, "1")
            .env("KRONN_INTERNAL_MCP_EXECUTABLE", &bridge)
            .env("KRONN_BACKEND_URL", "http://127.0.0.1:43127")
            .output().expect("isolated environment probe");
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let mut config = crate::core::mcp_scanner::McpJsonFile {
            mcp_servers: Default::default(),
        };
        let injected = crate::core::mcp_scanner::inject_kronn_internal(&mut config);
        if std::path::Path::new("/.dockerenv").exists() {
            // A container cannot publish its private executable to host CLIs.
            if injected {
                assert_ne!(
                    config.mcp_servers["kronn-internal"].command.as_deref(),
                    std::env::var("KRONN_INTERNAL_MCP_EXECUTABLE")
                        .ok()
                        .as_deref()
                );
            }
            return;
        }
        assert!(injected);
        let json = serde_json::to_value(&config).expect("MCP configuration JSON");
        let internal = &json["mcpServers"]["kronn-internal"];
        assert_eq!(
            internal["command"],
            std::env::var("KRONN_INTERNAL_MCP_EXECUTABLE").expect("bundle path")
        );
        assert_eq!(internal["args"], serde_json::json!([]));
        assert_eq!(
            internal["env"],
            serde_json::json!({
                "KRONN_BACKEND_URL": "http://127.0.0.1:43127"
            })
        );
    }

    #[test]
    fn claude_workers_report_unsupported_windows_sandbox_without_relaxing_policy() {
        let error = super::super::claude_task_worker_platform_check(true, false)
            .expect_err("native Windows cannot satisfy the required sandbox");
        assert!(error.contains("native Windows"));
        assert!(error.contains("ordinary Claude discussions remain available"));
        assert!(super::super::claude_task_worker_platform_check(true, true).is_ok());
        assert!(super::super::claude_task_worker_platform_check(false, false).is_ok());
    }

    #[test]
    fn other_cli_task_workers_never_receive_global_bypass_flags() {
        for (agent, forbidden) in [
            (AgentType::GeminiCli, "--yolo"),
            (AgentType::Kiro, "--trust-all-tools"),
            (AgentType::CopilotCli, "--allow-all-tools"),
        ] {
            let (_, _, args, _, _, _) = super::super::agent_command_with_task_worker_policy(
                &agent,
                "test prompt",
                true,
                "worker context",
                None,
                None,
                true,
                None,
                None,
            );
            assert!(
                !args.contains(&forbidden.to_string()),
                "{agent:?} task worker must not receive {forbidden}: {args:?}"
            );
        }
    }

    #[test]
    fn codex_full_access_uses_explicit_sandbox_only() {
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::Codex, "test prompt", true, "", None);
        assert!(
            !args.contains(&"--full-auto".to_string()),
            "Codex should not include --full-auto (it overrides explicit sandbox)"
        );
    }

    #[test]
    fn codex_no_full_access_omits_full_auto() {
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::Codex, "test prompt", false, "", None);
        assert!(
            !args.contains(&"--full-auto".to_string()),
            "Codex without full_access should NOT include --full-auto"
        );
    }

    #[test]
    fn gemini_full_access_adds_yolo() {
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::GeminiCli, "test prompt", true, "", None);
        assert!(
            args.contains(&"--yolo".to_string()),
            "Gemini CLI with full_access should include --yolo"
        );
        // --yolo must come BEFORE -p (Gemini requires -p <prompt> as last args)
        let yolo_idx = args.iter().position(|a| a == "--yolo").unwrap();
        let p_idx = args.iter().position(|a| a == "-p").unwrap();
        assert!(
            yolo_idx < p_idx,
            "--yolo ({}) must come before -p ({}) to avoid arg parsing issues",
            yolo_idx,
            p_idx
        );
    }

    // ─── agent_command: MCP/skills context injection ───────────────────────────

    #[test]
    fn claude_code_injects_context_via_append_system_prompt() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode,
            "prompt",
            false,
            "MCP context here",
            None,
        );
        let idx = args.iter().position(|a| a == "--append-system-prompt");
        assert!(idx.is_some(), "Should have --append-system-prompt flag");
        assert_eq!(args[idx.unwrap() + 1], "MCP context here");
    }

    #[test]
    fn codex_prepends_context_to_prompt() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Codex,
            "user prompt",
            false,
            "MCP context",
            None,
        );
        let last = args.last().unwrap();
        assert!(
            last.starts_with("MCP context"),
            "Context should be prepended to prompt"
        );
        assert!(
            last.contains("user prompt"),
            "Original prompt should be in the combined prompt"
        );
    }

    #[test]
    fn agent_command_no_context_when_empty() {
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::ClaudeCode, "prompt", false, "", None);
        assert!(
            !args.contains(&"--append-system-prompt".to_string()),
            "Should not add --append-system-prompt when context is empty"
        );
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
        assert!(clean_kiro_line(
            "Running tool jira_get_issue with params (from mcp server: atlassian)"
        )
        .is_none());
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
        assert!(clean_kiro_line(
            "I will run the following command: find /some/path -name '*.yaml' (using tool: shell)"
        )
        .is_none());
        assert!(clean_kiro_line(
            "Getting symbols from: /some/file.php [top_level=true] (using tool: code)"
        )
        .is_none());
        // French variant
        assert!(clean_kiro_line(
            "Je vais exécuter la commande suivante: ls -la (using tool: shell)"
        )
        .is_none());
    }

    #[test]
    fn kiro_keeps_real_content() {
        // Actual response text should NOT be filtered
        assert_eq!(
            clean_kiro_line("Voici l'analyse du problème :"),
            Some("Voici l'analyse du problème :".into())
        );
        assert_eq!(
            clean_kiro_line("## Architecture des redirections"),
            Some("## Architecture des redirections".into())
        );
        assert_eq!(
            clean_kiro_line("Layer 1 — YAML"),
            Some("Layer 1 — YAML".into())
        );
        assert_eq!(
            clean_kiro_line("The fix needed: preserve query params"),
            Some("The fix needed: preserve query params".into())
        );
    }

    #[test]
    fn kiro_strips_ansi_and_prefix() {
        // ANSI codes should be stripped
        assert_eq!(
            clean_kiro_line("\x1b[32mSome text\x1b[0m"),
            Some("Some text".into())
        );
        // "> " prefix should be stripped
        assert_eq!(
            clean_kiro_line("> Response text"),
            Some("Response text".into())
        );
    }

    // ─── agent_command: complete args structure per agent ────────────────────────
    //
    // These tests verify the full command structure for each agent type.
    // They catch regressions like missing flags, wrong binary names,
    // wrong env key, or broken npx fallback packages.

    #[test]
    fn claude_code_command_structure() {
        let (binary, npx, args, env_key, _, output_mode) =
            super::super::agent_command(&AgentType::ClaudeCode, "do something", false, "", None);
        assert_eq!(binary, "claude");
        assert_eq!(npx, Some("@anthropic-ai/claude-code"));
        assert_eq!(env_key, "ANTHROPIC_API_KEY");
        assert!(matches!(output_mode, OutputMode::StreamJson));
        assert!(
            args.contains(&"--print".to_string()),
            "Missing --print flag"
        );
        assert!(
            args.contains(&"--output-format".to_string()),
            "Missing --output-format flag"
        );
        assert!(
            args.contains(&"stream-json".to_string()),
            "Missing stream-json value"
        );
        assert!(
            args.contains(&"--verbose".to_string()),
            "Missing --verbose flag"
        );
        assert!(
            args.contains(&"--include-partial-messages".to_string()),
            "Missing --include-partial-messages"
        );
        // Prompt should be last arg
        assert_eq!(args.last().unwrap(), "do something");
    }

    #[test]
    fn codex_command_structure() {
        let (binary, npx, args, env_key, _, output_mode) =
            super::super::agent_command(&AgentType::Codex, "fix the bug", false, "", None);
        assert_eq!(binary, "codex");
        assert_eq!(npx, Some("@openai/codex"));
        assert_eq!(env_key, "OPENAI_API_KEY");
        assert!(matches!(output_mode, OutputMode::Text));
        assert_eq!(args[0], "exec", "First arg must be 'exec' subcommand");
        assert!(
            args.contains(&"--skip-git-repo-check".to_string()),
            "Missing --skip-git-repo-check"
        );
        let override_index = args
            .iter()
            .position(|arg| arg == "-c")
            .expect("Codex must receive the per-run MCP env allowlist");
        let override_value = &args[override_index + 1];
        assert!(override_value.contains("mcp_servers.kronn-internal.env_vars"));
        assert!(override_value.contains("KRONN_TASK_WORKER_CONTEXT"));
        assert!(override_value.contains("KRONN_DISCUSSION_ID"));
        assert_eq!(args.last().unwrap(), "fix the bug");
    }

    #[test]
    fn vibe_command_structure() {
        let (binary, npx, args, env_key, _, output_mode) =
            super::super::agent_command(&AgentType::Vibe, "analyse this", false, "", None);
        assert_eq!(
            binary, "python3",
            "Vibe must use python3 with vibe-runner.py"
        );
        assert_eq!(npx, None, "Vibe has no npx fallback");
        assert_eq!(env_key, "MISTRAL_API_KEY");
        assert!(matches!(output_mode, OutputMode::Text));
        // First arg should be the runner script path
        assert!(
            args[0].ends_with("vibe-runner.py"),
            "First arg must be vibe-runner.py, got: {}",
            args[0]
        );
        // Prompt should be the last arg
        assert_eq!(args.last().unwrap(), "analyse this");
    }

    #[test]
    fn gemini_command_structure() {
        let (binary, npx, args, env_key, _, output_mode) =
            super::super::agent_command(&AgentType::GeminiCli, "explain this", false, "", None);
        assert_eq!(binary, "gemini");
        assert_eq!(npx, Some("@google/gemini-cli"));
        assert_eq!(env_key, "GEMINI_API_KEY");
        assert!(matches!(output_mode, OutputMode::Text));
        // -p must be just before the prompt (last two args), not first
        let p_idx = args
            .iter()
            .position(|a| a == "-p")
            .expect("-p flag must exist");
        assert_eq!(
            p_idx,
            args.len() - 2,
            "-p must be second-to-last arg (before prompt)"
        );
        assert_eq!(args.last().unwrap(), "explain this");
    }

    #[test]
    fn kiro_command_structure() {
        let (binary, npx, args, env_key, _, output_mode) =
            super::super::agent_command(&AgentType::Kiro, "review code", false, "", None);
        assert_eq!(binary, "kiro-cli");
        assert_eq!(npx, None, "Kiro has no npx fallback");
        assert_eq!(env_key, "AWS_BUILDER_ID");
        assert!(matches!(output_mode, OutputMode::Text));
        assert_eq!(args[0], "chat", "First arg must be 'chat' subcommand");
        assert!(
            args.contains(&"--no-interactive".to_string()),
            "Missing --no-interactive (required for headless)"
        );
        assert!(
            args.contains(&"--trust-all-tools".to_string()),
            "Missing --trust-all-tools (required with --no-interactive)"
        );
        assert!(args.contains(&"--wrap".to_string()), "Missing --wrap flag");
        assert!(
            args.contains(&"never".to_string()),
            "Missing 'never' wrap value"
        );
        assert_eq!(args.last().unwrap(), "review code");
    }

    // ─── agent_command: model flag injection ────────────────────────────────────

    #[test]
    fn claude_code_model_flag_injected() {
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::ClaudeCode, "prompt", false, "", Some("haiku"));
        let idx = args
            .iter()
            .position(|a| a == "--model")
            .expect("Missing --model flag");
        assert_eq!(args[idx + 1], "haiku");
    }

    #[test]
    fn codex_model_flag_injected() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Codex,
            "prompt",
            false,
            "",
            Some("gpt-5-codex-mini"),
        );
        let idx = args
            .iter()
            .position(|a| a == "--model")
            .expect("Missing --model flag");
        assert_eq!(args[idx + 1], "gpt-5-codex-mini");
    }

    #[test]
    fn gemini_model_flag_injected() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::GeminiCli,
            "prompt",
            false,
            "",
            Some("gemini-2.5-flash"),
        );
        let idx = args
            .iter()
            .position(|a| a == "--model")
            .expect("Missing --model flag");
        assert_eq!(args[idx + 1], "gemini-2.5-flash");
    }

    #[test]
    fn vibe_model_flag_injected() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Vibe,
            "prompt",
            false,
            "",
            Some("devstral-small-latest"),
        );
        let idx = args
            .iter()
            .position(|a| a == "--model")
            .expect("Vibe should support --model via runner");
        assert_eq!(args[idx + 1], "devstral-small-latest");
    }

    #[test]
    fn kiro_no_model_flag_support() {
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::Kiro, "prompt", false, "", Some("some-model"));
        assert!(
            !args.contains(&"--model".to_string()),
            "Kiro should not have --model flag (not supported)"
        );
    }

    // ─── Vibe runner path resolution ────────────────────────────────────────────

    #[test]
    fn vibe_runner_path_resolves_to_existing_file() {
        let path = super::super::vibe_runner_path();
        assert!(
            path.ends_with("vibe-runner.py"),
            "Path should end with vibe-runner.py, got: {}",
            path
        );
        assert!(
            std::path::Path::new(&path).exists(),
            "vibe-runner.py must exist at: {}",
            path
        );
    }

    // ─── get_api_key: Mistral provider support ──────────────────────────────────

    fn empty_tokens() -> crate::models::TokensConfig {
        crate::models::TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: vec![],
            disabled_overrides: vec![],
        }
    }

    #[test]
    fn get_api_key_all_providers_no_panic() {
        let tokens = empty_tokens();
        // None of these should panic, all should return None with empty config
        for env_key in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "MISTRAL_API_KEY",
            "UNKNOWN_KEY",
        ] {
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
            ("OPENAI_API_KEY", "openai", "sk-openai-test-456"),
            ("GEMINI_API_KEY", "google", "AIza-gemini-test-789"),
            ("MISTRAL_API_KEY", "mistral", "mist-test-abc"),
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
            assert_eq!(
                key,
                Some(value.to_string()),
                "get_api_key({}) should return the active {} key",
                env_key,
                provider
            );
        }
    }

    #[test]
    fn get_api_key_inactive_key_not_returned() {
        use crate::models::ApiKey;
        let mut tokens = empty_tokens();
        tokens.keys.push(ApiKey {
            id: "k1".into(),
            name: "old".into(),
            provider: "anthropic".into(),
            value: "sk-inactive".into(),
            active: false,
        });
        // No active key → should fall back to env var (which is unset in tests)
        let key = super::super::get_api_key("ANTHROPIC_API_KEY", &tokens);
        assert_ne!(
            key,
            Some("sk-inactive".to_string()),
            "Inactive key should NOT be returned"
        );
    }

    #[test]
    fn get_api_key_disabled_override_skips_config() {
        use crate::models::ApiKey;
        let mut tokens = empty_tokens();
        tokens.keys.push(ApiKey {
            id: "k1".into(),
            name: "test".into(),
            provider: "openai".into(),
            value: "sk-from-config".into(),
            active: true,
        });
        tokens.disabled_overrides.push("openai".into());
        // Override disabled → should NOT use config key, falls back to env
        let key = super::super::get_api_key("OPENAI_API_KEY", &tokens);
        assert_ne!(
            key,
            Some("sk-from-config".to_string()),
            "Disabled override should skip config key"
        );
    }

    #[test]
    fn get_api_key_picks_active_among_multiple() {
        use crate::models::ApiKey;
        let mut tokens = empty_tokens();
        tokens.keys.push(ApiKey {
            id: "k1".into(),
            name: "personal".into(),
            provider: "google".into(),
            value: "AIza-personal".into(),
            active: false,
        });
        tokens.keys.push(ApiKey {
            id: "k2".into(),
            name: "work".into(),
            provider: "google".into(),
            value: "AIza-work".into(),
            active: true,
        });
        let key = super::super::get_api_key("GEMINI_API_KEY", &tokens);
        assert_eq!(
            key,
            Some("AIza-work".to_string()),
            "Should pick the active key among multiple for same provider"
        );
    }

    // ─── agent_command: context injection per agent ─────────────────────────────

    #[test]
    fn vibe_prepends_mcp_context_to_prompt() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Vibe,
            "user prompt",
            false,
            "MCP context here",
            None,
        );
        // vibe-runner.py uses the real Vibe SDK which supports tools,
        // so MCP context is prepended to the prompt (same as Codex/Gemini/Kiro)
        let prompt = args.last().unwrap();
        assert!(
            prompt.contains("MCP context here"),
            "MCP context should be prepended"
        );
        assert!(
            prompt.contains("user prompt"),
            "User prompt should be present"
        );
    }

    #[test]
    fn gemini_prepends_context_to_prompt() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::GeminiCli,
            "user prompt",
            false,
            "MCP context",
            None,
        );
        let prompt = args.last().unwrap();
        assert!(
            prompt.starts_with("MCP context"),
            "Context should be prepended"
        );
        assert!(
            prompt.contains("user prompt"),
            "Original prompt should be present"
        );
    }

    #[test]
    fn kiro_prepends_context_to_prompt() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::Kiro,
            "user prompt",
            false,
            "MCP context",
            None,
        );
        let prompt = args.last().unwrap();
        assert!(
            prompt.starts_with("MCP context"),
            "Context should be prepended"
        );
        assert!(
            prompt.contains("user prompt"),
            "Original prompt should be present"
        );
    }

    // ─── resolve_model_flag: tier mapping per agent ─────────────────────────────

    #[test]
    fn ollama_command_builder_is_a_non_inference_http_sentinel() {
        for model in [None, Some("explicit-operator-model")] {
            let (binary, package, args, env_key, _, _) = super::super::agent_command(
                &AgentType::Ollama,
                "private prompt",
                true,
                "private context",
                model,
            );
            assert_eq!(binary, "echo");
            assert_eq!(package, None);
            assert_eq!(env_key, "NONE");
            assert_eq!(args, ["Ollama runs over HTTP, not as a CLI process"]);
        }
    }

    #[test]
    fn resolve_model_flag_without_catalog_assignments_returns_none() {
        use crate::models::ModelTier;
        for agent in [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::GeminiCli,
            AgentType::Ollama,
        ] {
            for tier in [ModelTier::Economy, ModelTier::Default, ModelTier::Reasoning] {
                assert_eq!(resolve_model_flag(&agent, tier, None), None);
            }
        }
    }

    #[test]
    fn resolve_model_flag_user_override_beats_builtin() {
        // Run-9 finding — the [agents.model_tiers] overrides MUST win over
        // the built-in fallbacks (a Reasoning step configured on `fable`
        // was silently running on the built-in `opus`).
        use crate::models::setup::{ModelTierConfig, ModelTiersConfig};
        use crate::models::ModelTier;
        let cfg = ModelTiersConfig {
            claude_code: ModelTierConfig {
                reasoning: Some("fable".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Reasoning, Some(&cfg)),
            Some("fable".into())
        );
        // An unassigned tier has no runtime fallback.
        assert_eq!(
            resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Economy, Some(&cfg)),
            None
        );
    }

    #[test]
    fn resolve_model_flag_codex_unassigned_tiers_are_not_guessed() {
        use crate::models::ModelTier;
        assert_eq!(
            resolve_model_flag(&AgentType::Codex, ModelTier::Economy, None),
            None
        );
        assert_eq!(
            resolve_model_flag(&AgentType::Codex, ModelTier::Default, None),
            None
        );
        assert_eq!(
            resolve_model_flag(&AgentType::Codex, ModelTier::Reasoning, None),
            None
        );
    }

    #[test]
    fn resolve_model_flag_gemini_unassigned_tiers_are_not_guessed() {
        use crate::models::ModelTier;
        assert_eq!(
            resolve_model_flag(&AgentType::GeminiCli, ModelTier::Economy, None),
            None
        );
        assert_eq!(
            resolve_model_flag(&AgentType::GeminiCli, ModelTier::Default, None),
            None
        );
        assert_eq!(
            resolve_model_flag(&AgentType::GeminiCli, ModelTier::Reasoning, None),
            None
        );
    }

    #[test]
    fn resolve_model_flag_kiro_vibe_always_none() {
        use crate::models::ModelTier;
        for tier in [ModelTier::Economy, ModelTier::Default, ModelTier::Reasoning] {
            assert_eq!(
                resolve_model_flag(&AgentType::Kiro, tier, None),
                None,
                "Kiro should return None for all tiers (no --model support)"
            );
            assert_eq!(
                resolve_model_flag(&AgentType::Vibe, tier, None),
                None,
                "Vibe should return None for all tiers (no --model support)"
            );
        }
    }

    #[test]
    fn resolve_model_flag_user_override_takes_precedence() {
        use crate::models::{ModelTier, ModelTierConfig, ModelTiersConfig};
        let overrides = ModelTiersConfig {
            claude_code: ModelTierConfig {
                economy: Some("custom-haiku-3".into()),
                default: None,
                reasoning: None,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Economy, Some(&overrides)),
            Some("custom-haiku-3".into()),
            "User override should take precedence over built-in"
        );
        // Reasoning has no override and no catalog assignment.
        assert_eq!(
            resolve_model_flag(
                &AgentType::ClaudeCode,
                ModelTier::Reasoning,
                Some(&overrides)
            ),
            None,
        );
    }

    #[test]
    fn resolve_model_flag_default_tier_honors_user_override() {
        // New 2026-05-11: the Default tier now reads `agent_cfg.default`
        // before falling through to the built-in match. Primary use case
        // = Ollama user picks `gemma3:27b` from the OllamaCard, that
        // value overrides the built-in qwen3:30b-a3b fallback.
        use crate::models::{ModelTier, ModelTierConfig, ModelTiersConfig};
        let overrides = ModelTiersConfig {
            ollama: ModelTierConfig {
                economy: None,
                default: Some("gemma3:27b".into()),
                reasoning: None,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            resolve_model_flag(&AgentType::Ollama, ModelTier::Default, Some(&overrides)),
            Some("gemma3:27b".into()),
            "Default-tier user override must win over the built-in qwen3 fallback",
        );
        // Without an override or catalog assignment, the launch has no model.
        let no_override = ModelTiersConfig::default();
        assert_eq!(
            resolve_model_flag(&AgentType::Ollama, ModelTier::Default, Some(&no_override)),
            None,
            "No override must not revive a historical migration seed",
        );
    }

    #[test]
    fn resolve_model_flag_ollama_default_covers_all_tiers() {
        // 2026-07-02: Ollama has no built-in tier notion — the user picks ONE
        // model in the OllamaCard (the `default` slot). An empty economy/
        // reasoning slot must fall back to that single configured model, NOT to
        // a portability fallback the user never asked for. Regression guard for
        // the reported bug "I set qwen3:32b as default but reasoning-tier discs
        // silently used qwen3:30b-a3b".
        use crate::models::{ModelTier, ModelTierConfig, ModelTiersConfig};
        let overrides = ModelTiersConfig {
            ollama: ModelTierConfig {
                economy: None,
                default: Some("qwen3:32b".into()),
                reasoning: None,
                ..Default::default()
            },
            ..Default::default()
        };
        for tier in [ModelTier::Economy, ModelTier::Default, ModelTier::Reasoning] {
            assert_eq!(
                resolve_model_flag(&AgentType::Ollama, tier, Some(&overrides)),
                Some("qwen3:32b".into()),
                "Ollama default model must apply to EVERY tier when the tier slot is empty ({tier:?})",
            );
        }
        // An explicit per-tier slot still wins over the default fallback.
        let mixed = ModelTiersConfig {
            ollama: ModelTierConfig {
                economy: Some("qwen3:4b".into()),
                default: Some("qwen3:32b".into()),
                reasoning: None,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            resolve_model_flag(&AgentType::Ollama, ModelTier::Economy, Some(&mixed)),
            Some("qwen3:4b".into()),
            "An explicit economy slot must beat the default fallback",
        );
        assert_eq!(
            resolve_model_flag(&AgentType::Ollama, ModelTier::Reasoning, Some(&mixed)),
            Some("qwen3:32b".into()),
            "Empty reasoning slot falls back to the user's default, not the built-in",
        );
        // The all-empty case has no runtime assignment.
        assert_eq!(
            resolve_model_flag(&AgentType::Ollama, ModelTier::Reasoning, None),
            None,
        );
    }

    // ─── Historical Ollama defaults remain migration input only ───────────────
    #[test]
    fn migrated_ollama_defaults_are_not_runtime_fallbacks() {
        use crate::models::ModelTier;
        // These values may seed a one-time migration, but resolution must use
        // a current catalog assignment or an explicit configuration instead.
        assert_eq!(
            crate::core::model_catalog::migrated_default(&AgentType::Ollama, ModelTier::Economy),
            Some("qwen3:8b".into())
        );
        assert_eq!(
            crate::core::model_catalog::migrated_default(&AgentType::Ollama, ModelTier::Default),
            Some("qwen3:8b".into())
        );
        assert_eq!(
            crate::core::model_catalog::migrated_default(&AgentType::Ollama, ModelTier::Reasoning),
            Some("qwen3:30b-a3b".into())
        );
    }

    #[test]
    fn parse_keep_alive_maps_seconds_and_durations() {
        use crate::agents::runner::parse_keep_alive;
        // Unset / blank → omit (Ollama's own default applies).
        assert_eq!(parse_keep_alive(None), None);
        assert_eq!(parse_keep_alive(Some("  ".into())), None);
        // Bare integer → a JSON number (seconds).
        assert_eq!(
            parse_keep_alive(Some("1800".into())),
            Some(serde_json::json!(1800))
        );
        assert_eq!(
            parse_keep_alive(Some("-1".into())),
            Some(serde_json::json!(-1))
        );
        // Duration string → kept as a string.
        assert_eq!(
            parse_keep_alive(Some("30m".into())),
            Some(serde_json::json!("30m"))
        );
        assert_eq!(
            parse_keep_alive(Some(" 1h ".into())),
            Some(serde_json::json!("1h"))
        );
    }

    #[test]
    fn parse_num_ctx_cap_honors_override_and_guards() {
        // 0.8.11 — parse returns Option: None lets the model-derived auto path decide.
        assert_eq!(parse_num_ctx_cap(None), None);
        assert_eq!(parse_num_ctx_cap(Some("16384".into())), Some(16384));
        assert_eq!(parse_num_ctx_cap(Some(" 32768 ".into())), Some(32768));
        // Below the floor → rejected (never starve ctx) → auto path.
        assert_eq!(parse_num_ctx_cap(Some("512".into())), None);
        assert_eq!(parse_num_ctx_cap(Some("banana".into())), None);
        assert_eq!(parse_num_ctx_cap(Some("".into())), None);
    }

    // ─── KT-382 — a cold-loading Ollama must not cost us the real window ────

    #[tokio::test]
    async fn a_slow_first_show_still_yields_the_real_context_window() {
        // The probe fires at the first Ollama step after boot — exactly when
        // Ollama may be cold-loading a 27b model and blowing past the 5s
        // timeout. Before KT-382 that single miss handed the call the 8192
        // fallback, and the caller then sent an 11k-token prompt into it.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // First attempt: answers past the client's 5s timeout → transport error.
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(7))
                    .set_body_json(serde_json::json!({
                        "model_info": {"qwen3.context_length": 40960}
                    })),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Second attempt: the model is warm now, and answers immediately.
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model_info": {"qwen3.context_length": 40960}
            })))
            .mount(&server)
            .await;

        let limit = ollama_model_ctx_limit(&server.uri(), "qwen3-slow-load").await;

        assert_eq!(
            limit,
            Some(40960),
            "the retry must recover the model's real window instead of falling back"
        );
        // And the recovered window is what the caller will actually use.
        assert_eq!(resolve_ctx_cap_within(None, limit, 32768).value, 32768);
    }

    #[tokio::test]
    async fn a_persistently_unreachable_show_gives_up_after_a_bounded_number_of_tries() {
        // The other half of the contract: retrying must not turn a genuinely
        // offline Ollama into a long stall on every step. The ladder is short
        // and the fallback still applies.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .respond_with(ResponseTemplate::new(500))
            .expect(OLLAMA_SHOW_ATTEMPTS as u64)
            .mount(&server)
            .await;

        let limit = ollama_model_ctx_limit(&server.uri(), "qwen3-always-down").await;

        assert_eq!(limit, None, "a persistent failure still falls back");
        assert_eq!(
            resolve_ctx_cap_within(None, limit, 32768).value,
            OLLAMA_NUM_CTX_CAP,
            "the portable fallback is what an unanswerable Ollama yields"
        );
        // `expect` above asserts the exact attempt count on drop: bounded, and
        // not silently reduced to a single try either.
    }

    // ─── 0.8.11 — zero-config ctx cap: model-derived, env override wins ──────
    #[test]
    fn resolve_ctx_cap_env_wins_then_model_then_default() {
        let at =
            |env: Option<String>, limit: Option<u64>| resolve_ctx_cap_within(env, limit, 32768);
        // Env override wins over everything (even a bigger model limit).
        assert_eq!(at(Some("24576".into()), Some(131072)).value, 24576);
        // No env: the model's trained context, clamped to what the machine holds.
        assert_eq!(
            at(None, Some(40960)).value,
            32768,
            "qwen3 40K → ceiling 32K"
        );
        assert_eq!(
            at(None, Some(131072)).value,
            32768,
            "llama3.3 131K → ceiling"
        );
        assert_eq!(
            at(None, Some(16384)).value,
            16384,
            "model below ceiling → as-is"
        );
        assert_eq!(at(None, Some(1024)).value, 2048, "tiny model limit → floor");
        // Ollama unreachable → the portable fallback, itself bounded by what
        // the machine can hold.
        assert_eq!(at(None, None).value, OLLAMA_NUM_CTX_CAP.min(32768));
        // Bad env falls through to the model-derived path.
        assert_eq!(at(Some("banana".into()), Some(16384)).value, 16384);
    }

    /// KT-401 — a flat 32 768 throttled a 262 144-token model on a 64 GB machine
    /// and said nothing. The ceiling now follows the machine, and a throttled
    /// run has to name itself.
    #[test]
    fn the_ceiling_follows_the_machine_and_a_throttled_model_says_so() {
        const GB: u64 = 1024 * 1024 * 1024;
        assert_eq!(
            ram_derived_ceiling(Some(8 * GB)),
            8_192,
            "a laptop stays small"
        );
        assert_eq!(ram_derived_ceiling(Some(16 * GB)), 16_384);
        assert_eq!(ram_derived_ceiling(Some(32 * GB)), 32_768);
        assert_eq!(
            ram_derived_ceiling(Some(64 * GB)),
            65_536,
            "Romu's Mac doubles"
        );
        assert_eq!(ram_derived_ceiling(Some(192 * GB)), 131_072);
        assert_eq!(
            ram_derived_ceiling(None),
            32_768,
            "an unreadable machine keeps the old, conservative figure"
        );

        // Held below the model → say which number is ours and which is the model's.
        let throttled = resolve_ctx_cap_within(None, Some(262_144), 65_536);
        assert_eq!(throttled.value, 65_536);
        assert_eq!(
            throttled.origin,
            CtxCapOrigin::MachineCeiling {
                model_limit: 262_144
            }
        );
        let notice = throttled
            .throttle_notice("qwen3.8:27b-mlx")
            .expect("a throttled model owes the reader a sentence");
        assert!(
            notice.contains("262144"),
            "state what the model can do: {notice}"
        );
        assert!(
            notice.contains("65536"),
            "state what it is getting: {notice}"
        );
        assert!(
            notice.contains("KRONN_OLLAMA_NUM_CTX_CAP"),
            "and the way out: {notice}"
        );

        // Not held below it → nothing to announce.
        let whole = resolve_ctx_cap_within(None, Some(32_768), 65_536);
        assert_eq!(whole.origin, CtxCapOrigin::ModelWindow);
        assert!(
            whole.throttle_notice("qwen3:8b").is_none(),
            "a model using its whole window is not being throttled"
        );
        // An operator who chose the number is not being throttled by us either.
        assert!(
            resolve_ctx_cap_within(Some("4096".into()), Some(262_144), 65_536)
                .throttle_notice("qwen3.8:27b-mlx")
                .is_none()
        );
    }

    /// KT-405 — a fallback that never announces itself is indistinguishable
    /// from "this model really only has 8192 tokens", which is only ever true
    /// of the fallback. It must say so even when the current prompt fits.
    #[test]
    fn a_silent_fallback_still_announces_itself() {
        let fallback = resolve_ctx_cap_within(None, None, 65_536);
        assert_eq!(fallback.origin, CtxCapOrigin::PortableFallback);
        assert_eq!(fallback.value, OLLAMA_NUM_CTX_CAP);
        let notice = fallback
            .throttle_notice("qwen3:8b")
            .expect("Ollama being unreachable must not read as a fact about the model");
        assert!(
            notice.contains(&OLLAMA_NUM_CTX_CAP.to_string()),
            "state the fallback used: {notice}"
        );
        assert!(
            notice.contains("/api/show"),
            "name why it fell back: {notice}"
        );
    }

    /// KT-405 — the persistent per-model override sits between the env
    /// break-glass and the auto-derived cap: the env still wins when set (an
    /// operator who cannot reach the UI must still be able to force a
    /// number), but a model with its own override skips RAM-derivation
    /// entirely rather than being clamped by it.
    #[test]
    fn a_model_override_wins_over_auto_derivation_but_not_over_the_env() {
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("qwen3.8:27b-mlx".to_string(), 100_000u64);

        let overridden =
            resolve_ctx_cap_for_model(None, "qwen3.8:27b-mlx", &overrides, Some(262_144), 65_536);
        assert_eq!(
            overridden.value, 100_000,
            "the override, not the RAM ceiling"
        );
        assert_eq!(overridden.origin, CtxCapOrigin::ModelOverride);
        assert!(
            overridden.throttle_notice("qwen3.8:27b-mlx").is_none(),
            "an operator who set this on purpose is not being throttled"
        );

        // The env break-glass still wins over a per-model override.
        let env_wins = resolve_ctx_cap_for_model(
            Some("4096".into()),
            "qwen3.8:27b-mlx",
            &overrides,
            Some(262_144),
            65_536,
        );
        assert_eq!(env_wins.value, 4096);
        assert_eq!(env_wins.origin, CtxCapOrigin::OperatorOverride);

        // A model with no override still falls through to auto-derivation.
        let unrelated =
            resolve_ctx_cap_for_model(None, "qwen3:8b", &overrides, Some(32_768), 65_536);
        assert_eq!(unrelated.origin, CtxCapOrigin::ModelWindow);
        assert_eq!(unrelated.value, 32_768);
    }

    /// The whole point of a PER-MODEL override: two models installed on the
    /// same machine, same env, same RAM ceiling — must be able to resolve to
    /// two different windows.
    #[test]
    fn two_models_take_two_different_windows_from_the_same_override_map() {
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("qwen3.8:27b-mlx".to_string(), 100_000u64);
        overrides.insert("gemma4:12b-mlx".to_string(), 40_000u64);

        let big =
            resolve_ctx_cap_for_model(None, "qwen3.8:27b-mlx", &overrides, Some(262_144), 65_536);
        let small =
            resolve_ctx_cap_for_model(None, "gemma4:12b-mlx", &overrides, Some(131_072), 65_536);
        assert_eq!(big.value, 100_000);
        assert_eq!(small.value, 40_000);
        assert_ne!(big.value, small.value);
    }

    #[test]
    fn hand_edited_model_overrides_are_clamped_to_persistent_safety_bounds() {
        let overrides = std::collections::HashMap::from([
            ("too-small".to_string(), 1u64),
            ("too-large".to_string(), u64::MAX),
        ]);
        assert_eq!(
            resolve_ctx_cap_for_model(None, "too-small", &overrides, None, 65_536).value,
            OLLAMA_NUM_CTX_FLOOR
        );
        assert_eq!(
            resolve_ctx_cap_for_model(None, "too-large", &overrides, None, 65_536).value,
            OLLAMA_NUM_CTX_OVERRIDE_MAX
        );
    }

    #[test]
    fn parse_context_length_reads_arch_prefixed_key() {
        let show = serde_json::json!({
            "model_info": { "general.architecture": "qwen3", "qwen3.context_length": 40960 }
        });
        assert_eq!(parse_context_length(&show), Some(40960));
        let llama = serde_json::json!({ "model_info": { "llama.context_length": 131072 } });
        assert_eq!(parse_context_length(&llama), Some(131072));
        // No model_info / no matching key → None.
        assert_eq!(parse_context_length(&serde_json::json!({})), None);
        assert_eq!(
            parse_context_length(&serde_json::json!({"model_info": {"x": 1}})),
            None
        );
    }

    #[test]
    fn parse_ollama_model_profile_keeps_context_and_storage_format() {
        // An MLX build: it says its window, not its attention shape.
        let profile = parse_ollama_model_profile(&serde_json::json!({
            "details": { "format": "safetensors", "quantization_level": "nvfp4" },
            "model_info": { "qwen3_5.context_length": 262144 },
        }));
        assert_eq!(
            profile,
            OllamaModelProfile {
                context_length: Some(262144),
                storage_format: Some("safetensors".into()),
            }
        );

        assert_eq!(
            parse_ollama_model_profile(&serde_json::json!({})),
            OllamaModelProfile {
                context_length: None,
                storage_format: None,
            }
        );
    }

    // ─── effective_model_flag: explicit model override beats tier ─────────────
    #[test]
    fn unassigned_ollama_model_is_not_replaced_by_a_migrated_default() {
        use crate::models::ModelTier;

        assert_eq!(
            effective_model_flag(None, &AgentType::Ollama, ModelTier::Default, None),
            None,
            "a missing runtime catalog assignment must refuse dispatch instead of reviving a seed",
        );
    }

    #[test]
    fn effective_model_flag_override_wins_over_tier() {
        use crate::models::ModelTier;
        // Explicit model beats a missing tier assignment.
        assert_eq!(
            effective_model_flag(
                Some("qwen3:30b-a3b"),
                &AgentType::Ollama,
                ModelTier::Economy,
                None
            ),
            Some("qwen3:30b-a3b".into()),
        );
    }

    #[test]
    fn effective_model_flag_blank_or_none_falls_back_to_tier() {
        use crate::models::ModelTier;
        // Blank override is treated as unset → no model without an assignment.
        assert_eq!(
            effective_model_flag(Some("   "), &AgentType::Ollama, ModelTier::Default, None),
            resolve_model_flag(&AgentType::Ollama, ModelTier::Default, None),
        );
        // None → identical to resolve_model_flag.
        assert_eq!(
            effective_model_flag(None, &AgentType::ClaudeCode, ModelTier::Reasoning, None),
            None,
        );
    }

    // ─── effective_reasoning_effort: KT-646 precedence + agent gating ─────────

    #[test]
    fn only_claude_and_codex_have_a_verified_effort_transport() {
        assert!(agent_supports_reasoning_effort(&AgentType::ClaudeCode));
        assert!(agent_supports_reasoning_effort(&AgentType::Codex));
        assert!(!agent_supports_reasoning_effort(&AgentType::GeminiCli));
    }

    #[test]
    fn reasoning_effort_override_wins_and_is_trimmed() {
        assert_eq!(
            reasoning_effort_candidate(Some(" high "), Some("pinned"), Some("tier"), Some("low")),
            Some("high".into()),
            "an execution-level override must beat the tier preset",
        );
    }

    #[test]
    fn reasoning_effort_blank_override_falls_back_to_same_model_preset() {
        assert_eq!(
            reasoning_effort_candidate(Some("   "), Some("tier"), Some("tier"), Some("low")),
            Some("low".into()),
        );
    }

    #[test]
    fn reasoning_effort_unset_preset_means_cli_default() {
        // No override, no config at all → None (no flag sent, CLI default
        // applies) — this is the pre-existing/untouched-config behaviour.
        assert_eq!(
            reasoning_effort_candidate(None, Some("tier"), Some("tier"), None),
            None,
        );
    }

    #[test]
    fn reasoning_effort_does_not_apply_without_its_own_tier_preset() {
        assert_eq!(
            reasoning_effort_candidate(None, Some("economy"), Some("economy"), None),
            None,
        );
    }

    #[test]
    fn reasoning_effort_does_not_carry_preset_onto_a_different_internal_model_pin() {
        // The Reasoning tier's preset ("high") was calibrated for whatever
        // model that tier resolves to. A step that pins a DIFFERENT explicit
        // model, with no effort override of its own, must not silently
        // inherit it.
        assert_eq!(
            reasoning_effort_candidate(None, Some("internal-pin"), Some("tier-model"), Some("high")),
            None,
            "an explicit model pin without its own effort override must not inherit the tier preset",
        );
    }

    #[test]
    fn reasoning_effort_explicit_override_still_applies_alongside_a_model_pin() {
        // An operator who explicitly wants an effort on a pinned model sets
        // it explicitly; that request still wins.
        assert_eq!(
            reasoning_effort_candidate(Some("low"), Some("pinned"), Some("tier"), Some("high")),
            Some("low".into()),
        );
    }

    #[test]
    fn reasoning_effort_is_rejected_when_the_catalogue_does_not_advertise_it() {
        assert!(!effort_is_advertised(
            "high",
            &["low".into(), "medium".into()]
        ));
        assert!(effort_is_advertised("high", &["low".into(), "high".into()]));
    }

    #[test]
    fn launch_resolution_rejects_an_explicit_effort_without_a_catalogue_projection() {
        let error = resolve_reasoning_effort(
            Some("high"),
            Some("manual-model"),
            &AgentType::ClaudeCode,
            ModelTier::Default,
            None,
        )
        .expect_err("a mode absent from the available catalogue must not be silently omitted");
        assert!(
            error.contains("Cannot apply reasoning effort 'high'"),
            "{error}"
        );
        assert!(error.contains("manual-model"), "{error}");
    }

    // ─── Codex CLI arg construction: reasoning effort transmission ────────────

    #[test]
    fn codex_command_sends_model_reasoning_effort_override() {
        let (_, _, args, _, _, _) = agent_command_with_task_worker_policy(
            &AgentType::Codex,
            "prompt",
            false,
            "",
            None,
            Some("high"),
            false,
            None,
            None,
        );
        let idx = args
            .iter()
            .position(|a| a == "model_reasoning_effort=\"high\"")
            .expect("Codex args must carry the resolved reasoning effort as a -c TOML override");
        assert_eq!(args[idx - 1], "-c");
    }

    #[test]
    fn codex_command_omits_reasoning_effort_flag_when_unresolved() {
        let (_, _, args, _, _, _) = agent_command_with_task_worker_policy(
            &AgentType::Codex,
            "prompt",
            false,
            "",
            None,
            None,
            false,
            None,
            None,
        );
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("model_reasoning_effort=")),
            "no resolved effort must mean no override flag, not a guessed one",
        );
    }

    #[test]
    fn claude_code_command_sends_the_resolved_effort_flag() {
        let (_, _, args, _, _, _) = agent_command_with_task_worker_policy(
            &AgentType::ClaudeCode,
            "prompt",
            false,
            "",
            None,
            Some("high"),
            false,
            None,
            None,
        );
        let index = args
            .iter()
            .position(|arg| arg == "--effort")
            .expect("Claude argv must include --effort");
        assert_eq!(args.get(index + 1), Some(&"high".to_string()));
    }

    // ─── Claude Code: prompt is always last arg (required for --mcp-config injection)

    #[test]
    fn claude_code_prompt_is_last_arg() {
        // start_agent_with_config inserts --mcp-config before the prompt (last arg).
        // This test ensures prompt remains the last arg so that insertion works.
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode,
            "my prompt",
            true,
            "context",
            Some("sonnet"),
        );
        assert_eq!(
            args.last().unwrap(),
            "my prompt",
            "Prompt must be the last arg for --mcp-config injection to work"
        );
    }

    #[test]
    fn claude_code_prompt_is_last_even_with_context() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode,
            "do stuff",
            false,
            "MCP servers info",
            None,
        );
        assert_eq!(args.last().unwrap(), "do stuff");
        // --append-system-prompt should be before the prompt
        let sys_idx = args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .unwrap();
        assert!(
            sys_idx < args.len() - 1,
            "--append-system-prompt must come before prompt"
        );
    }

    // ─── --mcp-config insertion order ─────────────────────────────────────────

    #[test]
    fn a_529_written_by_the_cli_reaches_the_room_instead_of_vanishing() {
        // Shape taken verbatim from a real transcript on this machine
        // (~/.claude/projects/.../*.jsonl): the CLI answers a 529 with an
        // assistant message it wrote itself. No deltas precede it, so the
        // generic "skip assistant snapshots" rule used to drop the only
        // account of the failure — the turn ended in silence and the human
        // waited minutes before retrying by hand.
        let line = r#"{"type":"assistant","isApiErrorMessage":true,"message":{"model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","content":[{"type":"text","text":"API Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a moment."}]}}"#;
        match super::super::parse_claude_stream_line(line) {
            StreamJsonEvent::Text(text) => {
                assert!(text.contains("529 Overloaded"), "got: {text}");
            }
            other => panic!("a CLI-authored failure must reach the room, got {other:?}"),
        }
    }

    #[test]
    fn a_spend_limit_written_by_the_cli_is_kept_too() {
        // Same carrier, different cause — and the one seen most often here
        // (66 occurrences). Recognised through the synthetic model name alone,
        // so a build that stops setting the flag still surfaces it.
        let line = r#"{"type":"assistant","message":{"model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"You've hit your org's monthly spend limit · run /usage-credits to raise it"}]}}"#;
        match super::super::parse_claude_stream_line(line) {
            StreamJsonEvent::Text(text) => assert!(text.contains("spend limit"), "got: {text}"),
            other => panic!("a spend limit must reach the room, got {other:?}"),
        }
    }

    #[test]
    fn an_ordinary_assistant_snapshot_is_still_skipped() {
        // The reason the blanket skip exists: with --include-partial-messages
        // a real assistant message repeats everything already streamed. Keeping
        // it would print the whole reply a second time.
        let line = r#"{"type":"assistant","message":{"model":"claude-opus-4","role":"assistant","content":[{"type":"text","text":"Voici la réponse complète."}]}}"#;
        assert!(
            matches!(
                super::super::parse_claude_stream_line(line),
                StreamJsonEvent::Skip
            ),
            "a real assistant snapshot must stay skipped, or every reply doubles"
        );
    }

    #[test]
    fn a_cli_error_with_no_text_is_not_turned_into_an_empty_reply() {
        let line = r#"{"type":"assistant","isApiErrorMessage":true,"message":{"model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"   "}]}}"#;
        assert!(matches!(
            super::super::parse_claude_stream_line(line),
            StreamJsonEvent::Skip
        ));
    }

    #[test]
    fn the_session_probe_reads_the_layout_this_machine_actually_has() {
        // Pinned against a real store observed on macOS:
        //   ~/.claude/projects/-Users-priol-Repositories-Kronn-perf201/<id>.jsonl
        // Both `/` and `.` flatten to `-` — the second is easy to miss, and a
        // home under `~/.cache/...` is where it shows.
        assert_eq!(
            super::super::claude_project_slug(Path::new("/Users/priol/Repositories/Kronn")),
            "-Users-priol-Repositories-Kronn"
        );
        assert_eq!(
            super::super::claude_project_slug(Path::new("/Users/priol/.cache/kronn-test-tmp")),
            "-Users-priol--cache-kronn-test-tmp"
        );
    }

    #[test]
    fn an_id_that_would_leave_the_session_store_is_never_probed() {
        // The id lands in a file name. A traversal must be refused outright,
        // not merely fail to match.
        let dir = tempfile::tempdir().unwrap();
        for hostile in ["", "../../etc/passwd", "a/b", "..", "x\\y"] {
            assert!(
                !super::super::cli_print_session_is_resumable(dir.path(), hostile),
                "{hostile:?} should never be probed"
            );
        }
    }

    #[test]
    fn a_resumed_turn_carries_resume_and_a_fresh_one_does_not() {
        let (_, _, resumed, _, _, _) = super::super::agent_command_with_task_worker_policy(
            &AgentType::ClaudeCode,
            "only the new message",
            false,
            "",
            None,
            None,
            false,
            None,
            Some("2c19fd03-fde4-4c0d-a893-adae1d816df2"),
        );
        let flag = resumed
            .iter()
            .position(|arg| arg == "--resume")
            .expect("--resume present when a conversation is resumed");
        assert_eq!(resumed[flag + 1], "2c19fd03-fde4-4c0d-a893-adae1d816df2");
        // The prompt stays the last positional: --resume must not displace it,
        // or `--append-system-prompt` would swallow the wrong argument.
        assert_eq!(resumed.last().unwrap(), "only the new message");

        let (_, _, fresh, _, _, _) = super::super::agent_command_with_task_worker_policy(
            &AgentType::ClaudeCode,
            "the whole history",
            false,
            "",
            None,
            None,
            false,
            None,
            None,
        );
        assert!(
            !fresh.contains(&"--resume".to_string()),
            "a first turn must start a new conversation"
        );
    }

    #[test]
    fn mcp_config_inserted_before_append_system_prompt() {
        // Simulates what start_agent_with_config does: insert --mcp-config
        // before --append-system-prompt and its value.
        let (_, _, mut args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode,
            "the prompt",
            false,
            "MCP context",
            None,
        );

        super::super::insert_claude_mcp_config(&mut args, "/path/to/.mcp.json".into(), false);

        // Verify order: --mcp-config must come BEFORE --append-system-prompt
        let mcp_idx = args.iter().position(|a| a == "--mcp-config").unwrap();
        let sys_idx = args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .unwrap();
        assert!(mcp_idx < sys_idx,
            "--mcp-config ({}) must come before --append-system-prompt ({}) to avoid arg parsing issues. Args: {:?}",
            mcp_idx, sys_idx, args);

        // Prompt must still be last
        assert_eq!(args.last().unwrap(), "the prompt");
    }

    #[test]
    fn strict_flag_keeps_the_ordering_a_discussion_now_depends_on() {
        // Since 0.13.0 an ordinary discussion passes strict too, so this is the
        // real argument shape — not just the task-worker one. Both inserted
        // flags must still land before --append-system-prompt, which would
        // otherwise swallow the next positional as its value.
        let (_, _, mut args, _, _, _) = super::super::agent_command(
            &AgentType::ClaudeCode,
            "the prompt",
            false,
            "MCP context",
            None,
        );

        super::super::insert_claude_mcp_config(&mut args, "/path/to/.mcp.json".into(), true);

        let strict_idx = args
            .iter()
            .position(|a| a == "--strict-mcp-config")
            .expect("strict flag present");
        let mcp_idx = args.iter().position(|a| a == "--mcp-config").unwrap();
        let sys_idx = args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .unwrap();

        assert!(strict_idx < sys_idx, "args: {args:?}");
        assert!(mcp_idx < sys_idx, "args: {args:?}");
        // The path must follow its own flag, not the strict one.
        assert_eq!(args[mcp_idx + 1], "/path/to/.mcp.json");
        assert_eq!(args.last().unwrap(), "the prompt");
    }

    #[test]
    fn mcp_config_works_without_append_system_prompt() {
        // When there's no MCP context, --append-system-prompt is absent
        let (_, _, mut args, _, _, _) =
            super::super::agent_command(&AgentType::ClaudeCode, "the prompt", false, "", None);

        super::super::insert_claude_mcp_config(&mut args, "/path/to/.mcp.json".into(), false);

        assert!(args.contains(&"--mcp-config".to_string()));
        assert_eq!(args.last().unwrap(), "the prompt");
        // No --append-system-prompt should be present
        assert!(!args.contains(&"--append-system-prompt".to_string()));
    }

    #[test]
    fn task_worker_mcp_config_does_not_consume_permission_values() {
        let (_, _, mut args, _, _, _) = super::super::agent_command_with_task_worker_policy(
            &AgentType::ClaudeCode,
            "the prompt",
            true,
            "",
            None,
            None,
            true,
            None,
            None,
        );

        super::super::insert_claude_mcp_config(&mut args, "/path/to/.mcp.json".into(), true);

        assert_eq!(args.last().map(String::as_str), Some("the prompt"));
        assert_eq!(
            args.get(
                args.iter()
                    .position(|arg| arg == "--permission-mode")
                    .unwrap()
                    + 1
            )
            .map(String::as_str),
            Some("acceptEdits")
        );
        assert!(args.contains(&"--mcp-config".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()));
    }

    #[test]
    fn task_worker_mcp_config_keeps_only_kronn_internal() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(".mcp.json"),
            serde_json::json!({
                "mcpServers": {
                    "kronn-internal": {
                        "command": "python3",
                        "args": ["/trusted/disc-introspection-mcp.py"],
                        "env": {"UNRELATED_PROJECT_VALUE": "must-not-reach-worker"}
                    },
                    "github": {
                        "command": "npx",
                        "args": ["server-github"]
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let encoded = super::super::claude_task_worker_mcp_config(root.path()).unwrap();
        let filtered: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        let servers = filtered["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        assert!(servers.contains_key("kronn-internal"));
        assert!(!encoded.contains("server-github"));
        let internal = &servers["kronn-internal"];
        assert_eq!(
            internal["env"],
            serde_json::json!({
                "KRONN_TASK_WORKER_CONTEXT": "${KRONN_TASK_WORKER_CONTEXT}",
                "KRONN_DISCUSSION_ID": "${KRONN_DISCUSSION_ID}",
                "KRONN_BACKEND_URL": "${KRONN_BACKEND_URL:-http://127.0.0.1:3140}",
                "KRONN_AUTH_TOKEN": "${KRONN_AUTH_TOKEN:-}",
            })
        );
        assert!(!encoded.contains("UNRELATED_PROJECT_VALUE"));
        assert!(!encoded.contains("execution_id"));
    }

    #[test]
    fn task_worker_mcp_config_fails_closed_without_delivery_bridge() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(".mcp.json"),
            r#"{"mcpServers":{"github":{"command":"npx"}}}"#,
        )
        .unwrap();

        let error = super::super::claude_task_worker_mcp_config(root.path()).unwrap_err();
        assert!(error.contains("does not define the required `kronn-internal`"));
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
    fn codex_docker_always_full_access_sandbox() {
        // 2026-06-13 (run-9 finding) — bwrap cannot create user namespaces
        // inside the container on ANY host OS, so workspace-write is
        // structurally broken in Docker: Codex couldn't read a single file
        // and the plan review emitted a false NEEDS_RETRIAGE. The container
        // + worktree are the isolation boundary; always danger-full-access.
        std::env::set_var("KRONN_HOST_HOME", "/home/testuser");
        std::env::set_var("KRONN_HOST_OS", "Linux");
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::Codex, "prompt", false, "", None);
        assert!(
            args.contains(&"--sandbox=danger-full-access".to_string()),
            "Docker (any OS) must use danger-full-access — bwrap can't init in the container"
        );
        assert!(!args.contains(&"--sandbox=workspace-write".to_string()));
        std::env::remove_var("KRONN_HOST_HOME");
        std::env::remove_var("KRONN_HOST_OS");
    }

    #[test]
    #[serial]
    fn codex_macos_docker_forces_full_access_sandbox() {
        std::env::set_var("KRONN_HOST_HOME", "/Users/testuser");
        std::env::set_var("KRONN_HOST_OS", "macOS");
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::Codex, "prompt", false, "", None);
        assert!(
            args.contains(&"--sandbox=danger-full-access".to_string()),
            "macOS Docker should force danger-full-access sandbox regardless of full_access flag"
        );
        std::env::remove_var("KRONN_HOST_HOME");
        std::env::remove_var("KRONN_HOST_OS");
    }

    #[test]
    #[serial]
    fn codex_native_full_access_uses_danger_sandbox() {
        std::env::remove_var("KRONN_HOST_HOME");
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::Codex, "prompt", true, "", None);
        assert!(
            args.contains(&"--sandbox=danger-full-access".to_string()),
            "native full_access=true should use danger-full-access sandbox"
        );
    }

    #[test]
    #[serial]
    fn codex_native_restricted_access_keeps_default_sandbox() {
        std::env::remove_var("KRONN_HOST_HOME");
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::Codex, "prompt", false, "", None);
        assert!(
            !args.iter().any(|arg| arg.starts_with("--sandbox=")),
            "native full_access=false should keep Codex's restricted default sandbox"
        );
    }

    // ─── StreamJsonEvent: ToolStart / ToolInputDelta / ToolEnd ──────────────

    #[test]
    fn parse_stream_tool_start() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01","name":"Read","input":{}}}}"#;
        match parse_claude_stream_line(line) {
            StreamJsonEvent::ToolStart(name) => assert_eq!(name, "Read"),
            other => panic!("Expected ToolStart, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_tool_input_delta() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"/src"}}}"#;
        match parse_claude_stream_line(line) {
            StreamJsonEvent::ToolInputDelta(partial) => {
                assert!(
                    partial.contains("file_path"),
                    "Should contain partial JSON, got: {}",
                    partial
                );
            }
            other => panic!("Expected ToolInputDelta, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_tool_end() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_stop","index":1}}"#;
        assert!(matches!(
            parse_claude_stream_line(line),
            StreamJsonEvent::ToolEnd
        ));
    }

    // ─── CopilotCli agent_command ─────────────────────────────────────────────

    #[test]
    fn copilot_agent_command_basic() {
        let (bin, npx, args, env_key, _, _) =
            super::super::agent_command(&AgentType::CopilotCli, "hello", false, "", None);
        assert_eq!(bin, "copilot");
        assert_eq!(npx, Some("@github/copilot"));
        assert_eq!(env_key, "GH_TOKEN");
        assert_eq!(args, vec!["-p", "hello"]);
    }

    #[test]
    fn copilot_agent_command_full_access() {
        let (_, _, args, _, _, _) =
            super::super::agent_command(&AgentType::CopilotCli, "hello world", true, "", None);
        assert_eq!(args, vec!["--allow-all-tools", "-p", "hello world"]);
    }

    #[test]
    fn copilot_agent_command_with_model() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::CopilotCli,
            "hello",
            false,
            "",
            Some("gpt-4o-mini"),
        );
        assert_eq!(args, vec!["--model", "gpt-4o-mini", "-p", "hello"]);
    }

    #[test]
    fn copilot_agent_command_keeps_options_before_prompt_pair() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::CopilotCli,
            "prompt with several words",
            true,
            "",
            Some("gpt-4o-mini"),
        );
        assert_eq!(
            args,
            vec![
                "--model",
                "gpt-4o-mini",
                "--allow-all-tools",
                "-p",
                "prompt with several words",
            ]
        );
    }

    #[test]
    fn copilot_agent_command_with_mcp_context() {
        let (_, _, args, _, _, _) = super::super::agent_command(
            &AgentType::CopilotCli,
            "hello",
            false,
            "MCP context here",
            None,
        );
        // MCP context should be prepended to prompt (no --append-system-prompt for Copilot)
        let prompt = args.last().unwrap();
        assert!(
            prompt.contains("MCP context here"),
            "Prompt should contain MCP context"
        );
        assert!(
            prompt.contains("hello"),
            "Prompt should contain user prompt"
        );
    }

    #[test]
    fn plugin_invocation_rule_reaches_every_supported_cli_command() {
        let rule = "Fastly production: API first via `api_call`";
        // HTTP families return before this builder. Their sentinel must not
        // carry the prompt/context; the Ollama sentinel has its own regression.
        let agents = [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::Vibe,
            AgentType::GeminiCli,
            AgentType::Kiro,
            AgentType::CopilotCli,
        ];

        for agent in agents {
            let (_, _, args, _, _, _) =
                super::super::agent_command(&agent, "user prompt", false, rule, None);
            assert!(
                args.join("\n").contains(rule),
                "{agent:?} dropped the shared plugin invocation rule"
            );
        }
    }

    // ─── Cross-platform: model tier resolution ─────────────────────────────

    #[test]
    fn copilot_model_tiers() {
        let economy =
            super::super::resolve_model_flag(&AgentType::CopilotCli, ModelTier::Economy, None);
        assert_eq!(economy, None);
        let default =
            super::super::resolve_model_flag(&AgentType::CopilotCli, ModelTier::Default, None);
        assert_eq!(default, None); // Use Copilot's default
        let reasoning =
            super::super::resolve_model_flag(&AgentType::CopilotCli, ModelTier::Reasoning, None);
        assert_eq!(reasoning, None);
    }

    #[test]
    fn copilot_explicit_tier_override_is_preserved() {
        let mut overrides = crate::models::ModelTiersConfig::default();
        overrides.copilot_cli.reasoning = Some("gpt-5".into());

        let reasoning = super::super::resolve_model_flag(
            &AgentType::CopilotCli,
            ModelTier::Reasoning,
            Some(&overrides),
        );
        assert_eq!(reasoning, Some("gpt-5".into()));
    }

    // ─── Cross-platform: is_wsl detection ───────────────────────────────────

    #[test]
    fn is_wsl_returns_bool() {
        // Just a smoke test — actual result depends on platform
        let _ = super::super::is_wsl();
    }

    // ─── HOME override skip policy (TD-20260507-home-override) ─────────────

    #[test]
    fn skip_home_override_for_known_cli_agent_binaries() {
        // Every Kronn-managed CLI agent has its config at /home/kronn/<agent>
        // (via docker-compose mounts) — overriding HOME to KRONN_HOST_HOME
        // would route them to /home/<host-user>/<agent> which doesn't exist
        // in the container. Each agent must be in the skip list.
        for binary in ["claude", "codex", "vibe", "gemini", "kiro-cli", "copilot"] {
            assert!(
                should_skip_home_override(binary, None),
                "binary {} must skip the HOME override",
                binary
            );
        }
    }

    #[test]
    fn skip_home_override_for_npx_packaged_agents() {
        // When agents are launched via `npx <pkg>`, the binary name is
        // "npx" — we identify them by the npx_package field instead.
        for pkg in [
            "@anthropic-ai/claude-code",
            "@openai/codex",
            "@google/gemini-cli",
            "@github/copilot",
        ] {
            assert!(
                should_skip_home_override("npx", Some(pkg)),
                "npx package {} must skip the HOME override",
                pkg
            );
        }
    }

    #[test]
    fn skip_home_override_keeps_override_for_unknown_binaries() {
        // Arbitrary tools the operator runs through Kronn need a host-rooted
        // HOME (e.g. config files in $HOME the operator expects to be the
        // host's). Don't strip the override for them.
        assert!(!should_skip_home_override("python", None));
        assert!(!should_skip_home_override("rg", None));
        assert!(!should_skip_home_override("custom-tool", None));
        // Unknown npx package: also keep override.
        assert!(!should_skip_home_override("npx", Some("@some/random-pkg")));
    }

    #[test]
    fn skip_home_override_ollama_keeps_override() {
        // Ollama is a local HTTP server, not a CLI agent reading $HOME/.ollama
        // for auth. It's not in the skip list — let the override stand to
        // match the historical behaviour.
        assert!(!should_skip_home_override("ollama", None));
    }

    #[test]
    fn local_capacity_classification_excludes_remote_http_providers() {
        for agent in [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::GeminiCli,
            AgentType::Kiro,
            AgentType::Vibe,
            AgentType::CopilotCli,
            AgentType::Ollama,
        ] {
            assert!(
                is_local_agent(&agent),
                "{agent:?} must consume local capacity"
            );
        }

        for agent in [AgentType::LiteLlm, AgentType::Nvidia] {
            assert!(
                !is_local_agent(&agent),
                "{agent:?} must bypass local capacity"
            );
        }
    }

    #[test]
    fn default_concurrency_policy_serializes_local_families_only() {
        let config = crate::core::config::default_config();
        let limits: serde_json::Value = serde_json::from_str(&agent_concurrency_limits(
            &config.agents,
            config.server.max_concurrent_agents,
        ))
        .expect("concurrency policy must be valid JSON");

        assert_eq!(limits["__local_global"], 5);
        for agent in [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::GeminiCli,
            AgentType::Kiro,
            AgentType::Vibe,
            AgentType::CopilotCli,
            AgentType::Ollama,
        ] {
            assert_eq!(
                limits[format!("{agent:?}")],
                1,
                "unexpected {agent:?} default"
            );
        }
        assert!(limits.get("LiteLlm").is_none());
        assert!(limits.get("Nvidia").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn process_group_probe_never_signals_the_callers_group_or_all_processes() {
        assert!(group_has_processes(0));
        assert!(group_has_processes(1));
    }

    /// Regression test for KT-418: the production process primitive must kill
    /// a CLI agent's whole Unix group without touching an unrelated process.
    #[cfg(unix)]
    #[tokio::test]
    async fn cli_agent_cancellation_kills_process_tree() {
        struct Witness(std::process::Child);

        impl Drop for Witness {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path().to_path_buf();

        let script_path = temp_path.join("agent.sh");
        let child_pid_file = temp_path.join("child.pid");

        // Create a script that:
        // 1. Records its own PID (will be parent of the group)
        // 2. Spawns a long-running child that records its PID
        // 3. Waits forever (will be killed by the test)
        let test_script = format!(
            r#"#!/bin/bash
# Spawn child that ignores SIGTERM and records its PID
(
  trap '' SIGTERM
  sleep 3600 &
  echo $! > {}
  wait
) &

# Parent waits indefinitely
sleep 3600
"#,
            child_pid_file.to_string_lossy()
        );

        std::fs::write(&script_path, test_script).expect("Failed to write test script");

        // Spawn the test agent using the production spawn path
        let mut cmd = crate::core::cmd::async_cmd("sh");
        cmd.arg(script_path.to_string_lossy().as_ref())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        // Create process group on Unix (same as production)
        #[cfg(unix)]
        {
            unsafe {
                cmd.pre_exec(|| {
                    let ret = libc::setpgid(0, 0);
                    if ret == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
            }
        }

        let child = cmd.spawn().expect("Failed to spawn test agent");
        let agent_pid = child.id().expect("Agent must have a PID");

        // Store pgid = pid (we set setpgid(0,0) above, guaranteed to match)
        #[cfg(unix)]
        let pgid = Some(agent_pid as i32);
        #[cfg(not(unix))]
        let pgid: Option<i32> = None;

        // This process inherits the test runner's group, which is distinct from
        // the dedicated agent group. The guard always kills and reaps it.
        let mut witness = Witness(
            crate::core::cmd::sync_cmd("sleep")
                .arg("120")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("Failed to spawn witness"),
        );

        // Give processes time to start and spawn descendants
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        // Read the child PID if it exists
        let child_pid = std::fs::read_to_string(&child_pid_file)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());

        // Create AgentProcess and call kill()
        let (tx, rx) = mpsc::channel(1);
        drop(tx); // Close sender to avoid channel noise
        let mut agent_process = AgentProcess {
            child,
            output_mode: OutputMode::Text,
            work_dir: temp_path,
            agent_type: AgentType::ClaudeCode,
            rx,
            stderr_capture: Arc::new(Mutex::new(Vec::new())),
            usage: Arc::new(Mutex::new(AgentUsage::default())),
            stderr_task: None,
            http_cancel: None,
            pgid,
            token_fragments: false,
        };

        // Call the production kill() method
        agent_process.kill().await;

        // Verify parent is dead
        assert!(
            agent_process.try_wait().is_some(),
            "Agent parent should be terminated"
        );

        // Verify child descendant is dead (if we could capture it)
        if let Some(child_pid) = child_pid {
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            // Use ESRCH-aware check
            #[cfg(unix)]
            {
                unsafe {
                    match libc::kill(child_pid as libc::pid_t, 0) {
                        0 => panic!("Child descendant PID {} should be terminated", child_pid),
                        -1 => {
                            use std::io;
                            match io::Error::last_os_error().raw_os_error() {
                                Some(libc::ESRCH) => {
                                    // Confirmed dead via ESRCH
                                }
                                _ => panic!("Cannot confirm child PID {} is dead", child_pid),
                            }
                        }
                        _ => panic!("Unexpected kill() return value"),
                    }
                }
            }
        }

        // Verify the entire process group is empty (Unix only)
        #[cfg(unix)]
        if let Some(pgid) = pgid {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            assert!(
                !group_has_processes(pgid),
                "Process group {} should be empty",
                pgid
            );
        }

        assert!(
            witness.0.try_wait().unwrap().is_none(),
            "the unrelated witness must remain alive"
        );
    }

    #[tokio::test]
    async fn agent_process_exposes_structured_transport_usage() {
        let child = crate::core::cmd::async_cmd("sh")
            .args(["-c", "exit 0"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let (_tx, rx) = mpsc::channel(1);
        let process = AgentProcess {
            child,
            output_mode: OutputMode::Text,
            work_dir: std::env::temp_dir(),
            agent_type: AgentType::OpenCode,
            rx,
            stderr_capture: Arc::new(Mutex::new(Vec::new())),
            usage: Arc::new(Mutex::new(AgentUsage {
                input_tokens: 3,
                output_tokens: 5,
            })),
            stderr_task: None,
            http_cancel: None,
            pgid: None,
            token_fragments: false,
        };

        assert_eq!(process.reported_token_usage(), Some(8));
    }

    #[test]
    fn agent_concurrency_limits_preserves_litellm_and_nvidia_when_set() {
        // KT-545 DoD #1 — LiteLLM/NVIDIA becoming named-connection presets
        // must not lose their per-agent-type concurrency cap.
        let mut cfg = crate::core::config::default_config().agents;
        cfg.lite_llm.concurrency = Some(3);
        cfg.nvidia.concurrency = Some(7);

        let limits: serde_json::Value =
            serde_json::from_str(&agent_concurrency_limits(&cfg, 2)).unwrap();

        assert_eq!(limits["LiteLlm"], 3);
        assert_eq!(limits["Nvidia"], 7);
        assert_eq!(limits["__local_global"], 2);
    }

    #[test]
    fn agent_concurrency_limits_leaves_remote_agents_unlimited_by_default() {
        // An operator who never set a remote cap must get no entry at all
        // (unlimited), not a silent default — remote endpoints are someone
        // else's capacity to manage, not this machine's.
        let cfg = crate::core::config::default_config().agents;
        let limits: serde_json::Value =
            serde_json::from_str(&agent_concurrency_limits(&cfg, 1)).unwrap();

        assert!(limits.get("LiteLlm").is_none());
        assert!(limits.get("Nvidia").is_none());
        // Local agents still get their always-present default of 1.
        assert_eq!(limits["ClaudeCode"], 1);
        assert_eq!(limits["Ollama"], 1);
    }

    // ── a ceiling reached in a discussion is reported, and a grant moves it ──

    /// One tool, `probe`, under the generic twelve-call ceiling, with whatever
    /// allowance the test grants. Every result differs, so no same-answer
    /// guard can end the run before the ceiling does.
    struct CeilingTools {
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        allowance: crate::agents::tools::CeilingAllowance,
    }

    #[async_trait::async_trait]
    impl crate::agents::tools::ToolExecutor for CeilingTools {
        fn catalogue(&self) -> Vec<serde_json::Value> {
            vec![serde_json::json!({
                "type": "function",
                "function": {
                    "name": "probe",
                    "description": "Test tool probe",
                    "parameters": {
                        "type": "object",
                        "properties": { "page": { "type": "integer" } },
                        "required": [],
                    },
                },
            })]
        }

        async fn ceiling_allowance(&self) -> crate::agents::tools::CeilingAllowance {
            self.allowance.clone()
        }

        async fn execute(
            &self,
            call: &crate::agents::tools::ToolCall,
        ) -> crate::agents::tools::ToolOutcome {
            self.seen.lock().unwrap().push(call.arguments.to_string());
            crate::agents::tools::ToolOutcome {
                call: call.clone(),
                content: serde_json::json!({ "page": call.arguments["page"], "items": [call.arguments.to_string()] }),
                ok: true,
            }
        }
    }

    /// A provider that pages with `probe` for as long as Kronn declares it and
    /// the page limit allows, then answers.
    async fn paging_provider(pages: usize) -> wiremock::MockServer {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let round = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value =
                    serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null);
                let declared = body["tools"]
                    .as_array()
                    .is_some_and(|tools| tools.iter().any(|tool| tool["function"]["name"] == "probe"));
                let n = round.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if declared && n < pages {
                    ResponseTemplate::new(200).set_body_string(sse(&[&format!(
                        r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"c{n}","function":{{"name":"probe","arguments":"{{\"page\":{n}}}"}}}}]}}}}]}}"#
                    )]))
                } else {
                    ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"partial answer"}}]}"#,
                    ]))
                }
            })
            .mount(&server)
            .await;
        server
    }

    async fn run_paging(
        server: &wiremock::MockServer,
        allowance: crate::agents::tools::CeilingAllowance,
    ) -> (Vec<String>, String, Vec<String>, bool) {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "page through everything",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(CeilingTools {
                seen: seen.clone(),
                allowance,
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");
        let mut text = String::new();
        while let Some(line) = process.next_line().await {
            text.push_str(&line);
        }
        let status = process.child.wait().await.expect("lifeline");
        let stderr = process.captured_stderr_flushed().await;
        let calls = seen.lock().unwrap().clone();
        (calls, text, stderr, status.success())
    }

    fn asking() -> crate::agents::tools::CeilingAllowance {
        crate::agents::tools::CeilingAllowance {
            ask_on_ceiling: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_ceiling_reached_in_a_discussion_is_reported_with_what_was_refused() {
        let server = paging_provider(30).await;
        let (calls, _, stderr, _) = run_paging(&server, asking()).await;

        assert_eq!(
            calls.len(),
            12,
            "the ceiling itself does not move: {calls:?}"
        );
        let report = parse_ceiling_report(&stderr).expect("a ceiling report");
        assert_eq!(report.tools.len(), 1);
        let hit = &report.tools[0];
        assert_eq!(
            (hit.tool.as_str(), hit.limit, hit.refused),
            ("probe", 12, 1)
        );
        // What the refused call was after, so the human sees what is missing.
        assert_eq!(hit.refused_calls, vec!["page=12".to_string()]);
        assert_eq!(report.rounds, None);
    }

    #[tokio::test]
    async fn without_anyone_to_ask_a_ceiling_leaves_no_report() {
        let server = paging_provider(30).await;
        let (calls, _, stderr, _) =
            run_paging(&server, crate::agents::tools::CeilingAllowance::default()).await;

        assert_eq!(calls.len(), 12);
        assert!(
            parse_ceiling_report(&stderr).is_none(),
            "a workflow step has no one to ask"
        );
    }

    #[tokio::test]
    async fn a_granted_allowance_lets_the_tool_run_past_its_ceiling() {
        let server = paging_provider(30).await;
        let mut allowance = asking();
        allowance.extra_calls.insert("probe".into(), 5);
        let (calls, _, stderr, _) = run_paging(&server, allowance).await;

        assert_eq!(calls.len(), 17, "twelve plus the five granted");
        let report = parse_ceiling_report(&stderr).expect("the raised ceiling is reached too");
        assert_eq!(report.tools[0].limit, 17);
    }

    #[tokio::test]
    async fn a_lifted_counter_never_refuses_the_tool() {
        let server = paging_provider(20).await;
        let mut allowance = asking();
        allowance.unlimited_tools.insert("probe".into());
        let (calls, text, stderr, ok) = run_paging(&server, allowance).await;

        assert_eq!(calls.len(), 20, "every page the model asked for ran");
        assert!(parse_ceiling_report(&stderr).is_none());
        assert!(text.contains("partial answer") && ok);
    }

    /// The round ceiling in a discussion is not a failure any more: the last
    /// round's calls are refused, the model answers with what it has, and the
    /// report says which ceiling was reached. Outside a discussion it still
    /// fails, with its reason.
    #[tokio::test]
    async fn the_round_ceiling_in_a_discussion_ends_on_an_answer() {
        let server = paging_provider(1_000).await;
        let mut allowance = asking();
        allowance.unlimited_tools.insert("probe".into());
        let (calls, text, stderr, ok) = run_paging(&server, allowance).await;

        let cap = crate::agents::tools::MAX_TOOL_ITERATIONS;
        assert_eq!(calls.len(), cap, "every round up to the ceiling ran");
        assert!(ok, "an answer, not a failure: {stderr:?}");
        assert!(text.contains("partial answer"));
        let report = parse_ceiling_report(&stderr).expect("a ceiling report");
        assert_eq!(report.rounds, Some(cap));
    }

    #[tokio::test]
    async fn the_round_ceiling_outside_a_discussion_still_fails_with_its_reason() {
        let server = paging_provider(1_000).await;
        let mut allowance = crate::agents::tools::CeilingAllowance::default();
        allowance.unlimited_tools.insert("probe".into());
        let (_, _, stderr, ok) = run_paging(&server, allowance).await;

        assert!(!ok);
        assert!(
            stderr
                .iter()
                .any(|line| line.contains("rounds — giving up")),
            "{stderr:?}"
        );
        assert!(parse_ceiling_report(&stderr).is_none());
    }

    #[tokio::test]
    async fn granted_rounds_extend_the_round_ceiling() {
        let server = paging_provider(1_000).await;
        let mut allowance = asking();
        allowance.unlimited_tools.insert("probe".into());
        allowance.extra_rounds = 10;
        let (calls, _, stderr, _) = run_paging(&server, allowance).await;

        let cap = crate::agents::tools::MAX_TOOL_ITERATIONS + 10;
        assert_eq!(calls.len(), cap);
        assert_eq!(
            parse_ceiling_report(&stderr).and_then(|report| report.rounds),
            Some(cap)
        );
    }

    #[test]
    fn the_round_ceiling_follows_the_model_window() {
        assert_eq!(
            round_cap_for_window(None),
            crate::agents::tools::MAX_TOOL_ITERATIONS
        );
        assert_eq!(
            round_cap_for_window(Some(0)),
            crate::agents::tools::MAX_TOOL_ITERATIONS
        );
        assert_eq!(round_cap_for_window(Some(24_576)), 50);
        assert_eq!(round_cap_for_window(Some(65_536)), 80);
        assert_eq!(
            round_cap_for_window(Some(131_072)),
            crate::agents::tools::MAX_TOOL_ITERATIONS
        );
        assert_eq!(round_cap_for_window(Some(200_000)), 250);
    }

    #[test]
    fn a_remote_window_is_the_smallest_its_provider_routes_to() {
        let body = serde_json::json!({ "data": { "endpoints": [
            { "provider_name": "Bedrock", "context_length": 200000 },
            { "provider_name": "Google", "context_length": 1000000 },
            { "provider_name": "Unknown" },
        ]}});
        assert_eq!(smallest_endpoint_window(&body), Some(200_000));
        assert_eq!(
            smallest_endpoint_window(&serde_json::json!({ "data": {} })),
            None
        );
    }

    // ── the tiered catalogue, measured against a real local model ──
    //
    // Not a unit test: it spends a real model's time. Enable with
    //   KRONN_OLLAMA_BENCH=1 cargo test --lib bench_tiered_catalogue -- --ignored --nocapture
    // and optionally KRONN_BENCH_MODEL / KRONN_BENCH_OLLAMA.

    struct BenchTools {
        catalogue: Vec<serde_json::Value>,
        seen: Arc<Mutex<Vec<String>>>,
        /// Workspace `read_file` reads from, when the run points at a doc
        /// instead of carrying it.
        root: Option<std::path::PathBuf>,
    }

    #[async_trait::async_trait]
    impl crate::agents::tools::ToolExecutor for BenchTools {
        fn catalogue(&self) -> Vec<serde_json::Value> {
            self.catalogue.clone()
        }

        async fn execute(
            &self,
            call: &crate::agents::tools::ToolCall,
        ) -> crate::agents::tools::ToolOutcome {
            self.seen
                .lock()
                .unwrap()
                .push(match call.arguments["family"].as_str() {
                    Some(family) => format!("{}({family})", call.name),
                    // `tools_load` with a family that is not a string is a call
                    // that does not match its declaration — record what it
                    // actually sent, because that is the bug. Every other tool
                    // is recorded by name.
                    None if call.name == "tools_load" => {
                        format!("{}[args={}]", call.name, call.arguments)
                    }
                    None => call.name.clone(),
                });
            let content = match call.name.as_str() {
                "tools_load" => {
                    // Mirror the real executor: a call with no family, or one
                    // Kronn does not know, is answered with the families that
                    // exist. Swallowing it here measured a behaviour Kronn does
                    // not have, and cost a model a turn it would not have lost.
                    let family = call.arguments["family"].as_str().unwrap_or_default();
                    let declarations = crate::api::agent_tools::declarations_for_family(family);
                    if declarations.is_empty() {
                        return crate::agents::tools::ToolOutcome {
                            call: call.clone(),
                            content: serde_json::json!({
                                "families": crate::api::agent_tools::TOOL_FAMILIES
                                    .iter()
                                    .map(|(id, what, _)| serde_json::json!({
                                        "family": id, "brings": what
                                    }))
                                    .collect::<Vec<_>>(),
                                "note": "Call `tools_load` again with one of these as `family`.",
                            }),
                            ok: true,
                        };
                    }
                    let names: Vec<&str> = declarations
                        .iter()
                        .filter_map(|tool| tool["function"]["name"].as_str())
                        .collect();
                    serde_json::json!({
                        "family": family,
                        "loaded": names,
                        "note": "These tools are now declared for the rest of this run.",
                        "__kronn_tools_add": declarations,
                    })
                }
                "plan_get" => serde_json::json!({
                    "active": [
                        {"reference": "KT-101", "title": "Relire la page d'accueil", "status": "todo", "actionable": true},
                        {"reference": "KT-102", "title": "Corriger le lien RSS", "status": "in_progress", "actionable": false}
                    ]
                }),
                "media_generate" => serde_json::json!({
                    "job_id": "job-bench-1", "status": "pending", "modality": "image"
                }),
                "agent_list" => serde_json::json!({
                    "workers": [{"agent": "Codex", "connection_id": "conn-1"}]
                }),
                "read_file" => {
                    let path = call.arguments["path"].as_str().unwrap_or_default();
                    match self.root.as_ref().map(|root| root.join(path)) {
                        Some(full) => match std::fs::read_to_string(&full) {
                            Ok(content) => serde_json::json!({"path": path, "content": content}),
                            Err(err) => serde_json::json!({"error": err.to_string()}),
                        },
                        None => serde_json::json!({"error": "no workspace"}),
                    }
                }
                _ => serde_json::json!({"ok": true}),
            };
            crate::agents::tools::ToolOutcome {
                call: call.clone(),
                content,
                ok: true,
            }
        }
    }

    /// A stateful executor over a real repository, for a job that takes more
    /// turns than a model can hold in its head. The point is the state: a DoD
    /// item ticked on turn 3 is still ticked on turn 9, so the model can put
    /// down what it has done and pick it up again instead of carrying it.
    /// Reads are real; nothing is ever written to the repository.
    #[derive(Default)]
    struct WorkLog {
        dod: Vec<(String, String, bool)>,
        task_title: String,
        /// Each read: the normalised path, and the text actually handed back.
        /// Opening a file is not receiving a value — a slice can be empty, or
        /// land past the line that carried it.
        files_read: Vec<(String, String)>,
        wrote: Vec<(String, String)>,
        calls: Vec<String>,
    }

    /// One spelling per file. `./src/X.php`, `src/X.php` and an absolute path
    /// are the same unit; a stem alone is not — `src/Admin/User.php` and
    /// `src/Public/User.php` are two files.
    ///
    /// An absolute path is kept absolute. Stripping its leading slash and
    /// joining it to the root produced `<root>/<root>/src/X.php`, so the same
    /// file had two keys depending on how it was named.
    fn relative_to(root: &std::path::Path, raw: &str) -> String {
        let candidat = std::path::Path::new(raw);
        let joined = if candidat.is_absolute() {
            candidat.to_path_buf()
        } else {
            root.join(candidat)
        };
        let resolved = joined.canonicalize().unwrap_or(joined);
        resolved
            .strip_prefix(root)
            .unwrap_or(&resolved)
            .to_string_lossy()
            .to_string()
    }

    struct WorkTools {
        catalogue: Vec<serde_json::Value>,
        root: std::path::PathBuf,
        log: Arc<Mutex<WorkLog>>,
    }

    #[async_trait::async_trait]
    impl crate::agents::tools::ToolExecutor for WorkTools {
        fn catalogue(&self) -> Vec<serde_json::Value> {
            self.catalogue.clone()
        }

        async fn execute(
            &self,
            call: &crate::agents::tools::ToolCall,
        ) -> crate::agents::tools::ToolOutcome {
            let mut log = self.log.lock().unwrap();
            log.calls.push(call.name.clone());
            let args = &call.arguments;
            let safe = |root: &std::path::Path, raw: &str| -> Option<std::path::PathBuf> {
                let joined = root.join(raw.trim_start_matches('/'));
                joined.canonicalize().ok().filter(|p| p.starts_with(root))
            };
            let content = match call.name.as_str() {
                "tools_load" => {
                    let family = args["family"].as_str().unwrap_or_default();
                    let declarations = crate::api::agent_tools::declarations_for_family(family);
                    if declarations.is_empty() {
                        serde_json::json!({
                            "families": crate::api::agent_tools::TOOL_FAMILIES
                                .iter()
                                .map(|(id, what, _)| serde_json::json!({"family": id, "brings": what}))
                                .collect::<Vec<_>>(),
                        })
                    } else {
                        let names: Vec<&str> = declarations
                            .iter()
                            .filter_map(|t| t["function"]["name"].as_str())
                            .collect();
                        serde_json::json!({
                            "family": family, "loaded": names,
                            "__kronn_tools_add": declarations,
                        })
                    }
                }
                "task_create" => {
                    log.task_title = args["title"].as_str().unwrap_or_default().to_string();
                    log.dod = args["definition_of_done"]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .enumerate()
                                .map(|(n, item)| {
                                    let text =
                                        item.as_str().map(str::to_string).unwrap_or_else(|| {
                                            item["text"].as_str().unwrap_or("?").to_string()
                                        });
                                    (format!("dod-{}", n + 1), text, false)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    serde_json::json!({
                        "task_id": "KT-BENCH",
                        "title": log.task_title,
                        "definition_of_done": log.dod.iter()
                            .map(|(id, text, done)| serde_json::json!({
                                "dod_id": id, "text": text, "completed": done
                            }))
                            .collect::<Vec<_>>(),
                    })
                }
                "task_get" | "task_list" => serde_json::json!({
                    "task_id": "KT-BENCH",
                    "title": log.task_title,
                    "definition_of_done": log.dod.iter()
                        .map(|(id, text, done)| serde_json::json!({
                            "dod_id": id, "text": text, "completed": done
                        }))
                        .collect::<Vec<_>>(),
                    "remaining": log.dod.iter().filter(|(_, _, done)| !done).count(),
                }),
                "task_update_dod" => {
                    let wanted = args["dod_id"].as_str().unwrap_or_default().to_string();
                    let completed = args["completed"].as_bool().unwrap_or(true);
                    let mut found = false;
                    for (id, _, done) in log.dod.iter_mut() {
                        if *id == wanted {
                            *done = completed;
                            found = true;
                        }
                    }
                    if !found {
                        serde_json::json!({
                            "error": format!("no dod item `{wanted}` on this task"),
                            "known": log.dod.iter().map(|(id, _, _)| id.clone()).collect::<Vec<_>>(),
                        })
                    } else {
                        serde_json::json!({
                            "dod_id": wanted,
                            "completed": completed,
                            "remaining": log.dod.iter().filter(|(_, _, d)| !d).count(),
                        })
                    }
                }
                "read_file" => {
                    // The real tool, not an approximation of it. A bench that
                    // silently cut at 8 000 characters and ignored
                    // `offset`/`limit` penalised the models that honour the
                    // contract and read a large file in slices.
                    let raw = args["path"].as_str().unwrap_or_default();
                    let count = |field: &str| -> Option<usize> {
                        args[field].as_u64().map(|n| n as usize).or_else(|| {
                            args[field]
                                .as_str()
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                    };
                    match crate::api::agent_workspace_tools::read_file_payload(
                        &self.root,
                        raw,
                        count("offset"),
                        count("limit"),
                    ) {
                        Ok(payload) => {
                            if payload["found"].as_bool().unwrap_or(false) {
                                log.files_read.push((
                                    relative_to(&self.root, raw),
                                    payload["text"].as_str().unwrap_or_default().to_string(),
                                ));
                            }
                            payload
                        }
                        Err(message) => serde_json::json!({"error": message}),
                    }
                }
                "list_files" => {
                    let raw = args["path"].as_str().unwrap_or(".");
                    match safe(&self.root, raw) {
                        Some(dir) => {
                            let mut names: Vec<String> = std::fs::read_dir(&dir)
                                .map(|entries| {
                                    entries
                                        .filter_map(Result::ok)
                                        .map(|e| e.file_name().to_string_lossy().to_string())
                                        .filter(|n| !n.starts_with('.'))
                                        .collect()
                                })
                                .unwrap_or_default();
                            names.sort();
                            names.truncate(60);
                            serde_json::json!({"path": raw, "entries": names})
                        }
                        None => serde_json::json!({"error": format!("no directory `{raw}`")}),
                    }
                }
                "find_files" | "search_text" => {
                    let needle = args["pattern"]
                        .as_str()
                        .or_else(|| args["query"].as_str())
                        .or_else(|| args["text"].as_str())
                        .unwrap_or_default();
                    let mut hits = Vec::new();
                    for entry in std::fs::read_dir(&self.root)
                        .into_iter()
                        .flatten()
                        .flatten()
                    {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.contains(needle.trim_matches(['*', '.'])) {
                            hits.push(name);
                        }
                    }
                    hits.truncate(30);
                    serde_json::json!({"pattern": needle, "matches": hits})
                }
                "write_file" => {
                    // Recorded, never written: this is somebody's repository.
                    // The path is normalised so that the last write to the
                    // deliverable can be found whatever spelling led to it.
                    let path = args["path"].as_str().unwrap_or_default().to_string();
                    let body = args["content"].as_str().unwrap_or_default().to_string();
                    log.wrote.push((relative_to(&self.root, &path), body));
                    serde_json::json!({"path": path, "written": true})
                }
                _ => serde_json::json!({"ok": true}),
            };
            crate::agents::tools::ToolOutcome {
                call: call.clone(),
                content,
                ok: true,
            }
        }
    }

    struct BenchRun {
        label: &'static str,
        tools_declared: usize,
        catalogue_bytes: usize,
        first_prompt_tokens: u64,
        total_prompt_tokens: u64,
        turns: usize,
        calls: Vec<String>,
        answer: String,
        seconds: f64,
    }

    async fn bench_run(
        label: &'static str,
        prompt: &str,
        catalogue: Vec<serde_json::Value>,
    ) -> BenchRun {
        let base =
            std::env::var("KRONN_BENCH_OLLAMA").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
        let model = std::env::var("KRONN_BENCH_MODEL").unwrap_or_else(|_| "qwen3.8:27b-mlx".into());
        let catalogue_bytes = serde_json::to_string(&catalogue).unwrap().len();
        let tools_declared = catalogue.len();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let started = std::time::Instant::now();
        let mut process = start_ollama_http(
            &AgentType::Ollama,
            prompt,
            "Tu es un agent Kronn dans une discussion. Utilise les outils quand ils servent.",
            &model,
            None,
            Some(&base),
            None,
            Some(std::sync::Arc::new(BenchTools {
                catalogue,
                seen: seen.clone(),
                root: None,
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("ollama start");
        let mut answer = String::new();
        while let Some(line) = process.next_line().await {
            answer.push_str(&line);
        }
        let _ = process.child.wait().await;
        let stderr = process.captured_stderr_flushed().await;
        let turns = parse_http_turn_telemetry(&stderr);
        let calls = seen.lock().unwrap().clone();
        BenchRun {
            label,
            tools_declared,
            catalogue_bytes,
            first_prompt_tokens: turns.first().map(|turn| turn.prompt_tokens).unwrap_or(0),
            total_prompt_tokens: turns.iter().map(|turn| turn.prompt_tokens).sum(),
            turns: turns.len(),
            calls,
            answer: answer.chars().take(160).collect(),
            seconds: started.elapsed().as_secs_f64(),
        }
    }

    /// One run through the REAL prompt builder, project and all, so the doc
    /// injection and the tools notice are part of what is measured. `bench_run`
    /// goes straight to the transport with a hand-written system context and
    /// sees neither.
    async fn bench_doc_run(
        label: &'static str,
        project: &std::path::Path,
        prompt: &str,
        catalogue: Vec<serde_json::Value>,
    ) -> BenchRun {
        // The doc pointer and the tools notice are shared by every HTTP agent,
        // not just Ollama, so the same scenario runs against an OpenAI-wire
        // provider when one is configured for the bench.
        let external = std::env::var("KRONN_BENCH_HTTP_ENDPOINT")
            .ok()
            .map(|endpoint| ExternalHttpRuntime {
                display_name: "Bench".into(),
                mention_alias: "bench".into(),
                endpoint,
                api_key: std::env::var("KRONN_BENCH_HTTP_KEY").ok(),
            });
        let agent = match external {
            Some(_) => AgentType::Custom,
            None => AgentType::Ollama,
        };
        let model = std::env::var("KRONN_BENCH_MODEL").unwrap_or_else(|_| "gemma4:e4b".into());
        let catalogue_bytes = serde_json::to_string(&catalogue).unwrap().len();
        let tools_declared = catalogue.len();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let tokens = TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: Vec::new(),
            disabled_overrides: Vec::new(),
        };
        let started = std::time::Instant::now();
        let mut process = start_agent_with_config(AgentStartConfig {
            model_override: Some(&model),
            external_http: external.as_ref(),
            tools: Some(std::sync::Arc::new(BenchTools {
                catalogue,
                seen: seen.clone(),
                root: Some(project.to_path_buf()),
            })),
            ..AgentStartConfig::new(&agent, project.to_str().unwrap(), prompt, &tokens)
        })
        .await
        .expect("ollama start");
        let mut answer = String::new();
        while let Some(line) = process.next_line().await {
            answer.push_str(&line);
        }
        let _ = process.child.wait().await;
        let stderr = process.captured_stderr_flushed().await;
        let turns = parse_http_turn_telemetry(&stderr);
        let calls = seen.lock().unwrap().clone();
        BenchRun {
            label,
            tools_declared,
            catalogue_bytes,
            first_prompt_tokens: turns.first().map(|turn| turn.prompt_tokens).unwrap_or(0),
            total_prompt_tokens: turns.iter().map(|turn| turn.prompt_tokens).sum(),
            turns: turns.len(),
            calls,
            answer,
            seconds: started.elapsed().as_secs_f64(),
        }
    }

    /// The project doc used to be copied into every request because an HTTP
    /// agent had no filesystem. It reads files now, so the prompt points at the
    /// doc instead. The trade is measured here: the pointer is cheaper by
    /// construction, and what has to hold is that the model still goes and
    /// opens it. A model that answers without the fact is a model that needs
    /// `KRONN_INLINE_PROJECT_DOC=1`.
    ///
    ///   KRONN_OLLAMA_BENCH=1 KRONN_BENCH_MODELS="a,b" cargo test --lib \
    ///     bench_project_doc_inline_against_pointer -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "spends a real local model's time; KRONN_OLLAMA_BENCH=1"]
    async fn bench_project_doc_inline_against_pointer() {
        if std::env::var("KRONN_OLLAMA_BENCH").as_deref() != Ok("1") {
            eprintln!("skipped: set KRONN_OLLAMA_BENCH=1");
            return;
        }
        // The fact lives ONLY in the doc, so an answer carrying it proves the
        // doc was read rather than guessed from the model's own knowledge.
        // Pointed at a real project, the bench reads ITS doc and asks a question
        // only that doc answers — a repository nobody wrote for this test, with
        // its own tree for the model to get lost in.
        //   KRONN_BENCH_PROJECT=/path KRONN_BENCH_MARKER=... KRONN_BENCH_QUESTION=...
        let real_project = std::env::var("KRONN_BENCH_PROJECT")
            .ok()
            .filter(|p| !p.is_empty());
        const MARKER: &str = "kronn-registry-7731";
        let marker = std::env::var("KRONN_BENCH_MARKER").unwrap_or_else(|_| MARKER.to_string());
        let marker = marker.as_str();
        let project = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(project.path().join("docs")).expect("docs dir");
        // Kronn's own doc, so the inline arm pays what it really costs. A
        // fixture of a few hundred bytes would make both arms look alike and
        // measure nothing.
        if real_project.is_none() {
            let real_doc = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/AGENTS.md"),
            )
            .expect("the project's own docs/AGENTS.md");
            std::fs::write(
                project.path().join("docs/AGENTS.md"),
                format!(
                    "{real_doc}\n\n## Registre des versions\n\n\
                     Le registre de versions de ce projet s'appelle `{MARKER}`. Toute release \
                     doit y être déclarée avant d'être taguée.\n"
                ),
            )
            .expect("write doc");
        }
        // Nothing is ever written into a real project: the bench only reads.
        let project_path: std::path::PathBuf = match real_project.as_deref() {
            Some(path) => std::path::PathBuf::from(path),
            None => project.path().to_path_buf(),
        };
        println!(
            "\nproject: {}  doc: {} bytes",
            project_path.display(),
            std::fs::metadata(project_path.join("docs/AGENTS.md"))
                .map(|m| m.len())
                .unwrap_or(0)
        );

        // Two shapes of turn, because they trade in opposite directions: one
        // that needs the doc (the pointer pays a turn AND still carries the doc
        // back as a tool result) and one that does not (inlining pays for a
        // document the turn never uses).
        let default_question = "Comment s'appelle le registre de versions de ce projet ? \
                                 Réponds avec son nom exact, tel qu'il est écrit dans la \
                                 documentation.";
        let question =
            std::env::var("KRONN_BENCH_QUESTION").unwrap_or_else(|_| default_question.to_string());
        let needs_doc = question.as_str();
        let ignores_doc = "Dis bonjour en une phrase, sans utiliser d'outil.";
        let catalogue =
            crate::api::agent_tools::tiered(crate::api::agent_tools::full_discussion_catalogue());

        let models = std::env::var("KRONN_BENCH_MODELS").unwrap_or_else(|_| "gemma4:e4b".into());
        println!(
            "\n{:<18} {:<12} {:>9} {:>6} {:>9} {:>6}  {:<7} {:<7}",
            "model", "turn", "inline tk", "turns", "ptr tk", "turns", "inline", "pointer"
        );
        for model in models.split(',').map(str::trim).filter(|m| !m.is_empty()) {
            std::env::set_var("KRONN_BENCH_MODEL", model);
            for (scenario, prompt, grounded) in [
                ("needs doc", needs_doc, true),
                ("ignores doc", ignores_doc, false),
            ] {
                std::env::set_var("KRONN_INLINE_PROJECT_DOC", "1");
                let inline =
                    bench_doc_run("doc · inline", &project_path, prompt, catalogue.clone()).await;
                std::env::remove_var("KRONN_INLINE_PROJECT_DOC");
                let pointer =
                    bench_doc_run("doc · pointer", &project_path, prompt, catalogue.clone()).await;

                // Only the doc-dependent turn has a fact to carry; the other is
                // measured on cost alone.
                let verdict = |run: &BenchRun| match (grounded, run.answer.contains(marker)) {
                    (false, _) => "n/a",
                    (true, true) => "ok",
                    (true, false) => "MISSED",
                };
                println!(
                    "{:<18} {:<12} {:>9} {:>6} {:>9} {:>6}  {:<7} {:<7}",
                    model,
                    scenario,
                    inline.total_prompt_tokens,
                    inline.turns,
                    pointer.total_prompt_tokens,
                    pointer.turns,
                    verdict(&inline),
                    verdict(&pointer),
                );
                println!("    pointer calls {:?}", pointer.calls);
            }
        }
    }

    /// One family is the easy case. A run that needs two has to ask twice, and
    /// the window it was given at the start has to still hold the catalogue it
    /// ends up with — the reason sizing looks at what a run can reach rather
    /// than what it has.
    ///
    ///   KRONN_OLLAMA_BENCH=1 KRONN_BENCH_MODELS="a,b" cargo test --lib \
    ///     bench_two_families_in_one_run -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "spends a real local model's time; KRONN_OLLAMA_BENCH=1"]
    async fn bench_two_families_in_one_run() {
        if std::env::var("KRONN_OLLAMA_BENCH").as_deref() != Ok("1") {
            eprintln!("skipped: set KRONN_OLLAMA_BENCH=1");
            return;
        }
        let tiered =
            crate::api::agent_tools::tiered(crate::api::agent_tools::full_discussion_catalogue());
        let full = crate::api::agent_tools::full_discussion_catalogue();
        let prompt = "Deux choses, dans cet ordre. D'abord génère une image d'un phare au lever \
                      du jour, format carré. Ensuite écris un fichier `notes.md` à la racine du \
                      workspace qui dit en une phrase quelle image tu as lancée. Utilise les \
                      outils de Kronn pour les deux.";
        let models = std::env::var("KRONN_BENCH_MODELS").unwrap_or_else(|_| "gemma4:e4b".into());
        println!(
            "\n{:<18} {:>9} {:>6} {:<6} {:<6} {:>9} {:>6} {:<6} {:<6}",
            "model", "full tk", "turns", "image", "file", "tiered tk", "turns", "image", "file"
        );
        println!(
            "{:<18} {:>9} {:>6} {:>9} {:>6}",
            "", "full s", "", "tiered s", ""
        );
        for model in models.split(',').map(str::trim).filter(|m| !m.is_empty()) {
            std::env::set_var("KRONN_BENCH_MODEL", model);
            let full_run = bench_run("two · full", prompt, full.clone()).await;
            let tiered_run = bench_run("two · tiered", prompt, tiered.clone()).await;
            // Both modes are scored. Scoring only the tiered one made the
            // table look like the full catalogue always succeeded.
            let did = |run: &BenchRun, tool: &str| {
                if reached(run, tool) {
                    "ok"
                } else {
                    "MISSED"
                }
            };
            println!(
                "{:<18} {:>9} {:>6} {:<6} {:<6} {:>9} {:>6} {:<6} {:<6}",
                model,
                full_run.total_prompt_tokens,
                full_run.turns,
                did(&full_run, "media_generate"),
                did(&full_run, "write_file"),
                tiered_run.total_prompt_tokens,
                tiered_run.turns,
                did(&tiered_run, "media_generate"),
                did(&tiered_run, "write_file"),
            );
            // Time matters as much as tokens here: the declared catalogue sits
            // at the front of the prompt and is prefix-cached, so loading a
            // family mid-run changes that prefix and costs the cache.
            println!(
                "{:<18} {:>9} {:>6} {:>9} {:>6}",
                "",
                format!("{:.1}s", full_run.seconds),
                "",
                format!("{:.1}s", tiered_run.seconds),
                ""
            );
            println!("    full   {:?}", full_run.calls);
            println!("    tiered {:?}", tiered_run.calls);
            // A run that called nothing has an answer worth reading: it says
            // whether the model refused, misunderstood, or simply replied.
            if tiered_run.calls.is_empty() || !reached(&tiered_run, "write_file") {
                println!(
                    "    tiered said: {}",
                    tiered_run.answer.chars().take(400).collect::<String>()
                );
            }
        }
    }
    /// One thing a deliverable is supposed to contain, and the files that
    /// establish it.
    struct Unite {
        /// What the scored text must name.
        etiquette: String,
        /// Paths whose content establishes it.
        preuves: Vec<String>,
        /// When set, a read of a proof file only counts if the text handed
        /// back actually contained this. Opening `composer.json` at line 400
        /// is not receiving the PHP version. `None` for the inventory job,
        /// where opening the file IS the evidence.
        valeur_attendue: Option<String>,
    }

    /// Two numbers that are never merged, from ONE scored text and the reads
    /// that were observed. Pure, so the awkward cases are testable without a
    /// model: a value present only outside the returned slice, a read past the
    /// end of the file, a unit named with no read at all.
    fn compter(unites: &[Unite], texte: &str, lectures: &[(String, String)]) -> (usize, usize) {
        let texte = texte.to_lowercase();
        let nomme = |u: &Unite| texte.contains(&u.etiquette.to_lowercase());
        let preuve_lue = |u: &Unite| {
            lectures.iter().any(|(chemin, rendu)| {
                u.preuves.iter().any(|p| p.eq_ignore_ascii_case(chemin))
                    && match &u.valeur_attendue {
                        Some(valeur) => rendu.to_lowercase().contains(&valeur.to_lowercase()),
                        None => true,
                    }
            })
        };
        let couvert = unites.iter().filter(|u| nomme(u) && preuve_lue(u)).count();
        let sans_lecture = unites.iter().filter(|u| nomme(u) && !preuve_lue(u)).count();
        (couvert, sans_lecture)
    }

    /// Files under `root` whose content carries `valeur`, as normalised relative
    /// paths. Ties a fact to its source without hard-coding a repository's
    /// layout — and without the earlier nonsense of looking for `8.2` inside a
    /// file PATH, which no path contains.
    fn fichiers_portant(root: &std::path::Path, valeur: &str) -> Vec<String> {
        const IGNORES: [&str; 8] = [
            ".git",
            "vendor",
            "node_modules",
            "target",
            "var",
            "build",
            "dist",
            "public",
        ];
        fn descendre(
            dir: &std::path::Path,
            root: &std::path::Path,
            valeur: &str,
            out: &mut Vec<String>,
            reste: usize,
        ) {
            if reste == 0 || out.len() >= 40 {
                return;
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.filter_map(Result::ok) {
                let nom = entry.file_name().to_string_lossy().to_string();
                if nom.starts_with('.') || IGNORES.contains(&nom.as_str()) {
                    continue;
                }
                let path = entry.path();
                if path.is_dir() {
                    descendre(&path, root, valeur, out, reste - 1);
                } else if path.metadata().is_ok_and(|m| m.len() < 2 * 1024 * 1024)
                    && std::fs::read_to_string(&path).is_ok_and(|texte| texte.contains(valeur))
                {
                    out.push(
                        path.strip_prefix(root)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .to_string(),
                    );
                }
            }
        }
        let mut trouves = Vec::new();
        descendre(root, root, valeur, &mut trouves, 5);
        trouves
    }

    /// Three spellings of one file are one unit, and one stem shared by two
    /// files is two units.
    #[test]
    fn one_file_has_one_identity_whatever_spelling_names_it() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src/Admin")).unwrap();
        std::fs::create_dir_all(root.join("src/Public")).unwrap();
        std::fs::write(root.join("src/Admin/User.php"), "<?php").unwrap();
        std::fs::write(root.join("src/Public/User.php"), "<?php").unwrap();

        let absolu = root.join("src/Admin/User.php").display().to_string();
        for graphie in ["src/Admin/User.php", "./src/Admin/User.php", &absolu] {
            assert_eq!(
                relative_to(&root, graphie),
                "src/Admin/User.php",
                "graphie `{graphie}`"
            );
        }
        assert_ne!(
            relative_to(&root, "src/Admin/User.php"),
            relative_to(&root, "src/Public/User.php"),
            "deux fichiers, un seul radical, deux unités"
        );
    }

    /// Opening a file is not receiving a value. A slice that stops before the
    /// line carrying it proves nothing, and the bench used to count it.
    #[test]
    fn a_value_outside_the_returned_slice_is_not_a_proof() {
        let unites = vec![Unite {
            etiquette: "8.2".into(),
            preuves: vec!["composer.json".into()],
            valeur_attendue: Some("8.2".into()),
        }];
        let lectures = vec![(
            "composer.json".to_string(),
            "{\"name\": \"acme/site\"}".to_string(),
        )];
        assert_eq!(
            compter(&unites, "PHP 8.2", &lectures),
            (0, 1),
            "nommé, mais la valeur n'a jamais été rendue"
        );
    }

    /// Same for a read past the end of the file: found, and empty.
    #[test]
    fn a_read_past_the_end_of_a_file_is_not_a_proof() {
        let unites = vec![Unite {
            etiquette: "7.3".into(),
            preuves: vec!["composer.json".into()],
            valeur_attendue: Some("7.3".into()),
        }];
        let lectures = vec![("composer.json".to_string(), String::new())];
        assert_eq!(compter(&unites, "Symfony 7.3", &lectures), (0, 1));
    }

    /// And when the value IS handed back, it counts.
    #[test]
    fn a_value_present_in_what_was_returned_is_a_proof() {
        let unites = vec![Unite {
            etiquette: "8.2".into(),
            preuves: vec!["composer.json".into()],
            valeur_attendue: Some("8.2".into()),
        }];
        let lectures = vec![(
            "composer.json".to_string(),
            "{\"require\": {\"php\": \">=8.2\"}}".to_string(),
        )];
        assert_eq!(compter(&unites, "PHP 8.2", &lectures), (1, 0));
    }

    /// For the inventory the deliverable names a PATH, which the file's own
    /// text never contains — opening it is the evidence.
    #[test]
    fn opening_the_file_is_the_proof_when_the_unit_is_a_path() {
        let unites = vec![Unite {
            etiquette: "src/Admin/User.php".into(),
            preuves: vec!["src/Admin/User.php".into()],
            valeur_attendue: None,
        }];
        let lectures = vec![("src/Admin/User.php".to_string(), "<?php class User".into())];
        assert_eq!(
            compter(&unites, "- src/Admin/User.php : utilisateurs", &lectures),
            (1, 0)
        );
    }

    /// A unit named with no read at all is an assertion without an observed
    /// read — the second number, never merged into the first.
    #[test]
    fn naming_a_unit_with_no_read_at_all_is_an_assertion_without_a_read() {
        let unites = vec![Unite {
            etiquette: "src/Admin/User.php".into(),
            preuves: vec!["src/Admin/User.php".into()],
            valeur_attendue: None,
        }];
        assert_eq!(
            compter(&unites, "- src/Admin/User.php : utilisateurs", &[]),
            (0, 1)
        );
    }

    /// The repository the bench works in, or `None` when the operator did not
    /// name one.
    fn depot_du_banc() -> Option<std::path::PathBuf> {
        if std::env::var("KRONN_OLLAMA_BENCH").as_deref() != Ok("1") {
            eprintln!("skipped: set KRONN_OLLAMA_BENCH=1");
            return None;
        }
        let Ok(project) = std::env::var("KRONN_BENCH_PROJECT") else {
            eprintln!("skipped: set KRONN_BENCH_PROJECT to a repository to work in");
            return None;
        };
        Some(
            std::path::PathBuf::from(&project)
                .canonicalize()
                .expect("the repository must exist"),
        )
    }

    /// Run one job on one repository, over every model in `KRONN_BENCH_MODELS`,
    /// and score **what the run delivered** — the final content of `livrable`,
    /// or, when the model never wrote it, its final answer. One source, never
    /// the union of both: summing the answer and every written body scored text
    /// the reader of the deliverable would never see. The row says which.
    ///
    /// Three numbers, never merged:
    ///
    /// * **couverture avec lecture observée** — the unit is named in the scored
    ///   text AND one of its proof files was read during the run;
    /// * **affirmation sans lecture observée** — named without any proof read.
    ///   Not "fabrication": the evidence may come from the initial context, an
    ///   earlier turn, or a read this bench does not attribute;
    /// * **exactitude** — NOT measured. Nothing here checks what a line says,
    ///   only that it names a unit whose file was opened.
    async fn bench_deliverable_job(
        root: &std::path::Path,
        job: &str,
        livrable: &str,
        unites: &[Unite],
    ) {
        let sans_preuve = unites.iter().filter(|u| u.preuves.is_empty()).count();
        if sans_preuve > 0 {
            println!(
                "WARNING: {sans_preuve}/{} unité(s) n'ont aucun fichier de preuve dans ce dépôt — \
                 elles ne pourront jamais compter comme couvertes.",
                unites.len()
            );
        }
        let catalogue =
            crate::api::agent_tools::tiered(crate::api::agent_tools::full_discussion_catalogue());
        let models = std::env::var("KRONN_BENCH_MODELS").unwrap_or_else(|_| "gemma4:e4b".into());
        println!("\nrepository: {}", root.display());
        println!(
            "{:<18} {:>6} {:>7} {:>7} {:>5} {:>5} {:>6} {:>7} {:>7} {:>7} {:>9}",
            "model",
            "turns",
            "tokens",
            "seconds",
            "task",
            "dod",
            "reread",
            "files",
            "couvert",
            "sans-lu",
            "scoré sur"
        );
        for model in models.split(',').map(str::trim).filter(|m| !m.is_empty()) {
            std::env::set_var("KRONN_BENCH_MODEL", model);
            let external = std::env::var("KRONN_BENCH_HTTP_ENDPOINT")
                .ok()
                .map(|endpoint| ExternalHttpRuntime {
                    display_name: "Bench".into(),
                    mention_alias: "bench".into(),
                    endpoint,
                    api_key: std::env::var("KRONN_BENCH_HTTP_KEY").ok(),
                });
            let agent_type = match external {
                Some(_) => AgentType::Custom,
                None => AgentType::Ollama,
            };
            let log = Arc::new(Mutex::new(WorkLog::default()));
            let tokens = TokensConfig {
                anthropic: None,
                openai: None,
                google: None,
                keys: Vec::new(),
                disabled_overrides: Vec::new(),
            };
            let agent = agent_type;
            let started = std::time::Instant::now();
            let mut process = start_agent_with_config(AgentStartConfig {
                model_override: Some(model),
                external_http: external.as_ref(),
                tools: Some(std::sync::Arc::new(WorkTools {
                    catalogue: catalogue.clone(),
                    root: root.to_path_buf(),
                    log: log.clone(),
                })),
                ..AgentStartConfig::new(&agent, root.to_str().unwrap(), job, &tokens)
            })
            .await
            .expect("ollama start");
            let mut answer = String::new();
            while let Some(line) = process.next_line().await {
                answer.push_str(&line);
            }
            let _ = process.child.wait().await;
            let stderr = process.captured_stderr_flushed().await;
            let turns = parse_http_turn_telemetry(&stderr);
            let seconds = started.elapsed().as_secs_f64();

            let log = log.lock().unwrap();
            // The deliverable as it stands at the end of the run: the LAST
            // write to that path, not every body the run ever produced. When
            // the model never wrote it, the fallback is EVERY line the process
            // emitted — not its final answer, which this stream does not
            // delimit. The column says so rather than pretending otherwise.
            let vise = relative_to(root, livrable);
            let (source, texte) = match log.wrote.iter().rev().find(|(path, _)| *path == vise) {
                Some((_, body)) => (livrable, body.clone()),
                None => ("texte émis", answer.clone()),
            };
            let (couvert, sans_lecture) = compter(unites, &texte, &log.files_read);
            let total = unites.len();
            let distinct: std::collections::BTreeSet<&str> = log
                .files_read
                .iter()
                .map(|(chemin, _)| chemin.as_str())
                .collect();
            println!(
                "{:<18} {:>6} {:>7} {:>7} {:>5} {:>5} {:>6} {:>7} {:>7} {:>7} {:>9}",
                model,
                turns.len(),
                turns.iter().map(|t| t.prompt_tokens).sum::<u64>(),
                format!("{seconds:.0}s"),
                if log.dod.is_empty() { "no" } else { "yes" },
                format!(
                    "{}/{}",
                    log.dod.iter().filter(|(_, _, done)| *done).count(),
                    log.dod.len()
                ),
                log.calls.iter().filter(|c| *c == "task_get").count(),
                format!("{}/{}", distinct.len(), log.files_read.len()),
                format!("{couvert}/{total}"),
                sans_lecture,
                source,
            );
            if sans_lecture > 0 {
                println!(
                    "    {sans_lecture} unité(s) nommée(s) sans qu'une lecture de leur source \
                     ait été observée"
                );
            }
            let en_minuscules = texte.to_lowercase();
            let absents: Vec<&str> = unites
                .iter()
                .filter(|u| !en_minuscules.contains(&u.etiquette.to_lowercase()))
                .map(|u| u.etiquette.as_str())
                .collect();
            if !absents.is_empty() {
                println!(
                    "    absentes du livrable ({}) {:?}",
                    absents.len(),
                    &absents[..absents.len().min(8)]
                );
            }
            println!("    calls {:?}", log.calls);
            // Turns without a tool call are where a long job leaks: the run
            // keeps paying for the whole prompt and produces nothing.
            let notable: Vec<&str> = stderr
                .iter()
                .map(String::as_str)
                .filter(|l| {
                    l.contains("kronn_ceiling:")
                        || l.contains("repeated")
                        || l.contains("refus")
                        || l.contains("ceiling")
                        || l.contains("identical")
                })
                .take(6)
                .collect();
            if !notable.is_empty() {
                println!("    stderr {notable:?}");
            }
        }
    }

    /// The long job: every PHP file under `src/`, one line each, on a repository
    /// nobody wrote for this test. It does not fit in one window, which is the
    /// only condition under which putting progress down can pay for itself.
    ///
    /// The line must start with the file's relative path. A class NAME is not an
    /// identifier — `src/Admin/User.php` and `src/Public/User.php` are two
    /// files with one stem, and scoring on the stem merged them.
    ///
    ///   KRONN_OLLAMA_BENCH=1 KRONN_BENCH_PROJECT=/path/to/repo \
    ///   KRONN_BENCH_MODELS="a,b" cargo test --lib bench_inventory_of_a_repository \
    ///     -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "spends a real local model's time; KRONN_OLLAMA_BENCH=1"]
    async fn bench_inventory_of_a_repository() {
        let Some(root) = depot_du_banc() else {
            return;
        };
        let sources: Vec<String> = {
            fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<String>) {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    return;
                };
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(&path, root, out);
                    } else if path.extension().is_some_and(|e| e == "php") {
                        out.push(
                            path.strip_prefix(root)
                                .unwrap_or(&path)
                                .to_string_lossy()
                                .to_string(),
                        );
                    }
                }
            }
            let mut found = Vec::new();
            walk(&root.join("src"), &root, &mut found);
            found.sort();
            found
        };
        if sources.is_empty() {
            eprintln!("skipped: no PHP file under `src/` in {}", root.display());
            return;
        }
        let job = format!(
            "Tu travailles sur ce dépôt. Sous `src/` il y a {} fichiers PHP. Produis \
             `inventaire.md` avec UNE ligne par fichier. Chaque ligne COMMENCE par le chemin \
             relatif du fichier (par exemple `src/Controller/HomeController.php`), puis le nom \
             de la classe, puis en quelques mots ce qu'elle fait. Lis chaque fichier avant \
             d'écrire sa ligne, n'en invente aucune, et n'en oublie aucun.",
            sources.len()
        );
        let unites: Vec<Unite> = sources
            .iter()
            .map(|chemin| Unite {
                etiquette: chemin.clone(),
                preuves: vec![chemin.clone()],
                // Opening the file IS the evidence here: the deliverable names
                // a path, which the file's own text never contains.
                valeur_attendue: None,
            })
            .collect();
        bench_deliverable_job(&root, &job, "inventaire.md", &unites).await;
    }

    /// The short job: four facts, each written somewhere in the repository, then
    /// a deliverable. It fits in a window, so holding everything in the
    /// conversation works and a task is pure ceremony — which is exactly what
    /// makes it the control for the long one.
    ///
    /// Each fact's proof files are found by SEARCHING THE REPOSITORY for the
    /// value. The values default to this project's, and
    /// `KRONN_BENCH_FACTS="label=value,..."` replaces them for another one.
    ///
    ///   KRONN_OLLAMA_BENCH=1 KRONN_BENCH_PROJECT=/path/to/repo \
    ///   KRONN_BENCH_MODELS="a,b" cargo test --lib bench_four_facts_from_a_repository \
    ///     -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "spends a real local model's time; KRONN_OLLAMA_BENCH=1"]
    async fn bench_four_facts_from_a_repository() {
        let Some(root) = depot_du_banc() else {
            return;
        };
        let declares = std::env::var("KRONN_BENCH_FACTS").unwrap_or_else(|_| {
            "version de PHP=8.2,version de Symfony=7.3,\
             image du serveur applicatif=frankenphp:1.8-php8.4-alpine,outil de test=phpunit"
                .into()
        });
        let faits: Vec<(String, String)> = declares
            .split(',')
            .filter_map(|pair| pair.split_once('='))
            .map(|(label, valeur)| (label.trim().to_string(), valeur.trim().to_string()))
            .collect();
        let job = format!(
            "Tu travailles sur ce dépôt. Produis un fichier `inventaire.md` qui donne, pour \
             chacun des {} points suivants, la valeur EXACTE lue dans le dépôt et le fichier \
             d'où elle vient : {}. Toutes les valeurs sont dans le dépôt, va les chercher avec \
             tes outils avant de répondre.",
            faits.len(),
            faits
                .iter()
                .map(|(label, _)| label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let unites: Vec<Unite> = faits
            .iter()
            .map(|(label, valeur)| {
                let preuves = fichiers_portant(&root, valeur);
                println!(
                    "  {label} = {valeur} — {} fichier(s) le portent",
                    preuves.len()
                );
                Unite {
                    etiquette: valeur.clone(),
                    preuves,
                    // Here the file must have HANDED BACK the value: a slice
                    // that stops before it proves nothing.
                    valeur_attendue: Some(valeur.clone()),
                }
            })
            .collect();
        bench_deliverable_job(&root, &job, "inventaire.md", &unites).await;
    }

    /// what the bench measured, pinned without a model: a family the
    /// agent asks for is declared on the next turn, and asking twice is named
    /// rather than served again as if it were new.
    #[tokio::test]
    async fn a_loaded_family_is_declared_for_the_rest_of_the_run() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let requests = std::sync::Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
        let seen_by_mock = requests.clone();
        let round = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value =
                    serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null);
                let declared: Vec<String> = body["tools"]
                    .as_array()
                    .map(|tools| {
                        tools
                            .iter()
                            .filter_map(|tool| tool["function"]["name"].as_str())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                seen_by_mock.lock().unwrap().push(declared.clone());
                let n = round.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let call = |id: &str, name: &str, args: &str| {
                    ResponseTemplate::new(200).set_body_string(sse(&[&format!(
                        r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"{id}","function":{{"name":"{name}","arguments":"{args}"}}}}]}}}}]}}"#
                    )]))
                };
                match n {
                    // Ask for the family, then ask again — the second time
                    // teaches nothing and must be told so.
                    0 => call("c0", "tools_load", r#"{\"family\":\"media\"}"#),
                    1 => call("c1", "tools_load", r#"{\"family\":\"media\"}"#),
                    2 => call("c2", "media_generate", r#"{\"prompt\":\"a lighthouse\"}"#),
                    _ => ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"image lancée"}}]}"#,
                    ])),
                }
            })
            .mount(&server)
            .await;

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let core =
            crate::api::agent_tools::tiered(crate::api::agent_tools::full_discussion_catalogue());
        assert!(
            !core
                .iter()
                .any(|tool| tool["function"]["name"] == "media_generate"),
            "the core must not declare a family's tools"
        );
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "génère une image",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(BenchTools {
                catalogue: core,
                seen: seen.clone(),
                root: None,
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");
        let mut answer = String::new();
        while let Some(line) = process.next_line().await {
            answer.push_str(&line);
        }
        let _ = process.child.wait().await;

        let calls = seen.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                "tools_load(media)".to_string(),
                "media_generate".to_string()
            ],
            "the family's tool must become callable, and the second identical \
             load must not be executed again"
        );
        let declared = requests.lock().unwrap().clone();
        assert!(
            !declared[0].contains(&"media_generate".to_string()),
            "turn 1"
        );
        assert!(
            declared[1].contains(&"media_generate".to_string()),
            "the family joins the catalogue for the next turn: {:?}",
            declared[1]
        );
        // Loaded once, not once per request.
        assert_eq!(
            declared[2]
                .iter()
                .filter(|name| *name == "media_generate")
                .count(),
            1,
            "a second load must not duplicate the declaration"
        );
        assert!(
            declared[2].contains(&"tools_load".to_string()),
            "the index stays available for another family"
        );
        assert!(answer.contains("image lancée"));
    }

    /// Did the run reach the capability the task needed?
    fn reached(run: &BenchRun, tool: &str) -> bool {
        run.calls.iter().any(|call| call.starts_with(tool))
    }

    #[tokio::test]
    #[ignore = "spends a real local model's time; KRONN_OLLAMA_BENCH=1"]
    async fn bench_tiered_catalogue_across_models() {
        // the whole point: a wording that helps the weakest model
        // must not cost the strongest one. Every change is measured across
        // the range, not on one model.
        //   KRONN_OLLAMA_BENCH=1 KRONN_BENCH_MODELS="a,b,c" cargo test --lib \
        //     bench_tiered_catalogue_across_models -- --ignored --nocapture
        if std::env::var("KRONN_OLLAMA_BENCH").as_deref() != Ok("1") {
            eprintln!("skipped: set KRONN_OLLAMA_BENCH=1");
            return;
        }
        let models = std::env::var("KRONN_BENCH_MODELS").unwrap_or_else(|_| {
            "qwen3.5:2b,gemma4:e2b,qwen3.5:4b,gemma4:e4b,gemma4:12b-mlx,qwen3.8:27b-mlx".into()
        });
        let full = crate::api::agent_tools::full_discussion_catalogue();
        let tiered = crate::api::agent_tools::tiered(full.clone());
        let media_prompt =
            "Génère une image d'un phare au lever du jour, format carré. Utilise les outils de Kronn.";
        // Both paths are printed: the tiered catalogue saves tokens per turn but
        // can cost a turn, and only the two side by side say which one won.
        println!(
            "\n{:<18} {:>9} {:>6} {:>9} {:>6}  {:<7} {:<7}",
            "model", "full tk", "turns", "tiered tk", "turns", "full", "tiered"
        );
        for model in models.split(',').map(str::trim).filter(|m| !m.is_empty()) {
            std::env::set_var("KRONN_BENCH_MODEL", model);
            let full_run = bench_run("media · full", media_prompt, full.clone()).await;
            let tiered_run = bench_run("media · tiered", media_prompt, tiered.clone()).await;
            println!(
                "{:<18} {:>9} {:>6} {:>9} {:>6}  {:<7} {:<7}",
                model,
                full_run.total_prompt_tokens,
                full_run.turns,
                tiered_run.total_prompt_tokens,
                tiered_run.turns,
                if reached(&full_run, "media_generate") {
                    "ok"
                } else {
                    "MISSED"
                },
                if reached(&tiered_run, "media_generate") {
                    "ok"
                } else {
                    "MISSED"
                },
            );
            println!("    full   {:?}", full_run.calls);
            println!("    tiered {:?}", tiered_run.calls);
        }
    }

    #[tokio::test]
    #[ignore = "spends a real local model's time; KRONN_OLLAMA_BENCH=1"]
    async fn bench_tiered_catalogue_against_full() {
        if std::env::var("KRONN_OLLAMA_BENCH").as_deref() != Ok("1") {
            eprintln!("skipped: set KRONN_OLLAMA_BENCH=1");
            return;
        }
        let full = crate::api::agent_tools::full_discussion_catalogue();
        let tiered = crate::api::agent_tools::tiered(full.clone());
        let plan_prompt =
            "Lis le plan de cette discussion et dis-moi en une phrase quelle tâche est prête à être prise.";
        let media_prompt =
            "Génère une image d'un phare au lever du jour, format carré. Utilise les outils de Kronn.";

        let runs = vec![
            bench_run("plan · full", plan_prompt, full.clone()).await,
            bench_run("plan · tiered", plan_prompt, tiered.clone()).await,
            bench_run("media · full", media_prompt, full).await,
            bench_run("media · tiered", media_prompt, tiered).await,
        ];
        println!("\n=== Tiered catalogue measurements ===");
        for run in &runs {
            println!(
                "{:<16} tools={:<3} catalogue={:>6} B  first_prompt={:>6} tk  total_prompt={:>7} tk  turns={}  {:.1}s\n    calls: {:?}\n    answer: {}",
                run.label,
                run.tools_declared,
                run.catalogue_bytes,
                run.first_prompt_tokens,
                run.total_prompt_tokens,
                run.turns,
                run.seconds,
                run.calls,
                run.answer.replace('\n', " ")
            );
        }
    }

    // ─── Ce que Kronn demande au serveur Ollama lui-même ─────────────────────

    /// La version du serveur décide si les mitigations MLX s'appliquent. Une
    /// panne de transport ne doit pas être mise en cache comme une réponse :
    /// un serveur qui revient doit être revu.
    #[tokio::test]
    #[serial]
    async fn the_server_version_is_read_once_and_a_failure_is_not_an_answer() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Wiremock reuses pooled server addresses, while the production
        // version cache lives for the process. Another fixture can leave its
        // 0.18 answer under this address. Give this test its own cache key.
        let prefix = format!("/version-cache-{}", uuid::Uuid::new_v4());
        let endpoint = format!("{}{prefix}", server.uri());
        Mock::given(method("GET"))
            .and(path(format!("{prefix}/api/version")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": "0.34.2"
            })))
            .expect(1) // la seconde lecture vient du cache
            .mount(&server)
            .await;

        assert_eq!(ollama_server_version(&endpoint).await, Some((0, 34)));
        assert_eq!(ollama_server_version(&endpoint).await, Some((0, 34)));
        assert!(mlx_prefix_cache_reused(
            ollama_server_version(&endpoint).await
        ));

        // Un serveur injoignable : pas de réponse, et rien de mémorisé.
        let dead = "http://127.0.0.1:1";
        assert_eq!(ollama_server_version(dead).await, None);
        assert_eq!(ollama_server_version(dead).await, None);
        // Inconnu ⇒ la mitigation reste en place plutôt que d'être levée.
        assert!(!mlx_prefix_cache_reused(None));
    }

    /// Le poids d'un modèle sert à borner la fenêtre que la machine peut tenir.
    /// `/api/show` ne le donne pas, `/api/tags` si.
    #[tokio::test]
    #[serial]
    async fn a_model_weight_comes_from_the_server_that_pulled_it() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "models": [
                    {"name": "gemma4:e4b", "size": 9_611_383_929u64},
                    {"name": "qwen3.5:2b", "size": 2_740_000_000u64},
                ]
            })))
            .mount(&server)
            .await;

        assert_eq!(
            ollama_model_size_bytes(&server.uri(), "gemma4:e4b").await,
            Some(9_611_383_929)
        );
        // Un modèle que le serveur ne liste pas n'a pas de poids inventé.
        assert_eq!(
            ollama_model_size_bytes(&server.uri(), "jamais-tiré:1b").await,
            None
        );
    }

    /// Ce qu'un token de contexte coûte est MESURÉ, pas calculé : la forme de
    /// l'attention ne dit pas ce que le cache pèse, et l'arithmétique évidente
    /// se trompe d'un facteur dix sur une attention à fenêtre glissante. Deux
    /// fenêtres observées donnent la pente.
    #[tokio::test]
    #[serial]
    async fn a_token_of_context_is_priced_from_two_observed_windows() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let round = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        Mock::given(method("GET"))
            .and(path("/api/ps"))
            .respond_with(move |_: &wiremock::Request| {
                // Mesuré sur gemma4:e4b : ~12 Ko par token de contexte.
                let n = round.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let (ctx, size) = if n == 0 {
                    (4_096u64, 9_473_338_899u64)
                } else {
                    (65_536u64, 10_206_010_408u64)
                };
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "models": [{
                        "name": "measured:model",
                        "context_length": ctx,
                        "size": size,
                    }]
                }))
            })
            .mount(&server)
            .await;

        // Une seule fenêtre observée : une position, pas une pente.
        sample_ollama_memory(&server.uri(), "measured:model").await;
        assert_eq!(
            measured_kv_bytes_per_token(&server.uri(), "measured:model"),
            None
        );

        sample_ollama_memory(&server.uri(), "measured:model").await;
        let cost = measured_kv_bytes_per_token(&server.uri(), "measured:model")
            .expect("deux fenêtres observées donnent la pente");
        assert!(
            (11_000..13_000).contains(&cost),
            "{cost} octets par token ne sont pas les ~12 Ko mesurés"
        );
    }

    /// Le pointeur vers la doc est prouvé sur six modèles, mais l'opérateur
    /// peut remettre le doc en entier pour un modèle qui n'irait pas le lire.
    #[test]
    #[serial]
    fn an_operator_can_put_the_whole_project_doc_back() {
        std::env::remove_var("KRONN_INLINE_PROJECT_DOC");
        assert!(!inline_project_doc_forced());
        assert!(http_agent_reads_its_own_files(true, true));

        std::env::set_var("KRONN_INLINE_PROJECT_DOC", "1");
        assert!(inline_project_doc_forced());
        assert!(
            !http_agent_reads_its_own_files(true, true),
            "forcé, l'agent reçoit le doc entier même s'il sait lire"
        );

        // Une valeur qui n'est pas `1` ne force rien.
        std::env::set_var("KRONN_INLINE_PROJECT_DOC", "oui");
        assert!(!inline_project_doc_forced());
        std::env::remove_var("KRONN_INLINE_PROJECT_DOC");
    }

    /// Vider les caches ne suffisait pas : la garde anti-répétition RETIRE un
    /// outil refusé deux fois, donc un agent qui demandait trois fois où il en
    /// était avant de changer quoi que ce soit avait déjà perdu l'outil. Une
    /// mutation le rend — mais elle ne défait que la répétition.
    #[test]
    fn a_task_change_gives_back_a_reader_withdrawn_for_repeating_itself() {
        use crate::agents::tools::ToolRunMode;
        let mode = ToolRunMode::General;
        let withdrawn: std::collections::HashSet<String> = ["task_get", "plan_get", "read_file"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let no_circuit = std::collections::HashSet::new();
        let few_calls = std::collections::HashMap::new();

        let granted = crate::agents::tools::CeilingAllowance::default();
        let mut back =
            progress_readers_to_restore(&withdrawn, &no_circuit, &few_calls, mode, &granted);
        back.sort();
        assert_eq!(
            back,
            vec!["plan_get".to_string(), "task_get".to_string()],
            "les lecteurs de progression reviennent, pas les autres outils"
        );
    }

    /// Un retrait qui vient d'un plafond d'appels ou d'un circuit d'erreur
    /// n'est pas une répétition : changer une tâche ne doit pas le défaire.
    #[test]
    fn a_task_change_does_not_undo_a_ceiling_or_an_open_circuit() {
        use crate::agents::tools::ToolRunMode;
        let mode = ToolRunMode::General;
        let withdrawn: std::collections::HashSet<String> = ["task_get", "plan_get"]
            .into_iter()
            .map(str::to_string)
            .collect();

        // Circuit ouvert sur task_get : il reste dehors.
        let circuit: std::collections::HashSet<String> =
            ["task_get".to_string()].into_iter().collect();
        let granted = crate::agents::tools::CeilingAllowance::default();
        let back = progress_readers_to_restore(
            &withdrawn,
            &circuit,
            &std::collections::HashMap::new(),
            mode,
            &granted,
        );
        assert_eq!(back, vec!["plan_get".to_string()]);

        // Plafond atteint sur plan_get : il reste dehors aussi.
        let spent: std::collections::HashMap<String, usize> =
            [("plan_get".to_string(), max_calls_for_tool("plan_get", mode))]
                .into_iter()
                .collect();
        let back = progress_readers_to_restore(
            &withdrawn,
            &std::collections::HashSet::new(),
            &spent,
            mode,
            &granted,
        );
        assert_eq!(back, vec!["task_get".to_string()]);
    }

    /// Lier une tâche à la discussion change ce que `plan_get` répondrait.
    /// L'oublier laissait rejouer le plan d'avant le lien.
    #[test]
    fn linking_a_task_to_the_room_changes_what_the_plan_says() {
        for mutator in [
            "task_create",
            "task_update",
            "task_update_dod",
            "task_add_blocker",
            "task_remove_blocker",
            "task_link_discussion",
            "task_unlink_discussion",
        ] {
            assert!(
                is_progress_mutation_tool(mutator),
                "`{mutator}` change le plan et doit invalider ses lecteurs"
            );
        }
        // Lire n'est pas muter.
        for reader in ["plan_get", "task_get", "task_list", "read_file"] {
            assert!(!is_progress_mutation_tool(reader), "{reader}");
        }
    }

    /// Un humain qui a accordé des appels supplémentaires ne doit pas voir sa
    /// décision annulée par une comparaison au plafond de base.
    #[test]
    fn a_granted_allowance_counts_when_giving_a_reader_back() {
        use crate::agents::tools::{CeilingAllowance, ToolRunMode};
        let mode = ToolRunMode::General;
        let withdrawn: std::collections::HashSet<String> =
            ["task_get".to_string()].into_iter().collect();
        let spent: std::collections::HashMap<String, usize> =
            [("task_get".to_string(), max_calls_for_tool("task_get", mode))]
                .into_iter()
                .collect();

        // Sans accord, le plafond est atteint : il reste dehors.
        let none = CeilingAllowance::default();
        assert!(progress_readers_to_restore(
            &withdrawn,
            &std::collections::HashSet::new(),
            &spent,
            mode,
            &none
        )
        .is_empty());

        // Avec des appels accordés, il revient.
        let mut granted = CeilingAllowance::default();
        granted.extra_calls.insert("task_get".to_string(), 5);
        assert_eq!(
            progress_readers_to_restore(
                &withdrawn,
                &std::collections::HashSet::new(),
                &spent,
                mode,
                &granted
            ),
            vec!["task_get".to_string()]
        );
    }

    /// Retirer le nom de la liste des outils confisqués ne suffit pas : la
    /// déclaration elle-même a été SUPPRIMÉE de la requête. Sans elle, le
    /// modèle ne voit toujours pas l'outil.
    #[test]
    fn giving_a_reader_back_puts_its_declaration_in_the_request() {
        let catalogue =
            crate::api::agent_tools::tiered(crate::api::agent_tools::full_discussion_catalogue());
        let readers = progress_reader_declarations(&catalogue);
        assert!(
            readers.iter().any(|t| t["function"]["name"] == "task_get"),
            "les lecteurs de progression doivent être capturés au départ"
        );

        let mut body = serde_json::json!({"tools": []});
        restore_tool_declarations(&mut body, &readers);
        let declared = declared_tool_names(&body);
        assert!(declared.contains("task_get"), "{declared:?}");

        // Rappeler la restauration n'élargit pas le catalogue.
        let before = body["tools"].as_array().unwrap().len();
        restore_tool_declarations(&mut body, &readers);
        assert_eq!(body["tools"].as_array().unwrap().len(), before);
    }

    /// Un exécuteur de plan minimal : `task_get` rend un numéro de révision,
    /// `task_update_dod` l'incrémente. Assez pour prouver qu'une lecture
    /// d'après-mutation rapporte bien la nouvelle valeur.
    struct PlanTools {
        seen: Arc<Mutex<Vec<String>>>,
        revision: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl crate::agents::tools::ToolExecutor for PlanTools {
        fn catalogue(&self) -> Vec<serde_json::Value> {
            ["task_get", "task_update_dod"]
                .into_iter()
                .map(|name| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": name,
                            "description": "plan",
                            "parameters": {"type": "object", "properties": {}},
                        },
                    })
                })
                .collect()
        }

        async fn execute(
            &self,
            call: &crate::agents::tools::ToolCall,
        ) -> crate::agents::tools::ToolOutcome {
            self.seen.lock().unwrap().push(call.name.clone());
            if call.name == "task_update_dod" {
                self.revision
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            let revision = self.revision.load(std::sync::atomic::Ordering::SeqCst);
            crate::agents::tools::ToolOutcome {
                call: call.clone(),
                content: serde_json::json!({"revision": revision}),
                ok: true,
            }
        }
    }

    /// La séquence complète, dans la vraie boucle : lire trois fois, changer la
    /// tâche, relire. Le troisième `task_get` identique fait retirer l'outil, et
    /// la DÉCLARATION est supprimée de la requête — pas seulement marquée. Sans
    /// la restauration des déclarations, le modèle ne revoit jamais `task_get`
    /// et la valeur d'après-mutation ne lui parvient pas.
    #[tokio::test]
    #[serial]
    async fn a_reader_withdrawn_for_repeating_is_declared_again_after_a_task_changes() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let round = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let round_for_mock = round.clone();
        // Ce que le fournisseur a REÇU comme résultat de la dernière lecture.
        // Le compteur local dirait seulement que la mutation a incrémenté.
        let livree: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
        let livree_for_mock = livree.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let n = round_for_mock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body: serde_json::Value =
                    serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null);
                if let Some(resultat) = body["messages"].as_array().and_then(|messages| {
                    messages.iter().find(|m| {
                        m["role"] == "tool" && m["tool_call_id"] == "c4"
                    })
                }) {
                    let contenu = resultat["content"].as_str().unwrap_or_default();
                    *livree_for_mock.lock().unwrap() =
                        Some(serde_json::from_str(contenu).unwrap_or(serde_json::Value::Null));
                }
                let declared = |name: &str| {
                    body["tools"].as_array().is_some_and(|tools| {
                        tools.iter().any(|tool| tool["function"]["name"] == name)
                    })
                };
                // Trois lectures identiques, puis la mutation, puis une dernière
                // lecture — mais seulement si Kronn la déclare encore. Demander
                // un outil non déclaré ferait passer le test sans le correctif.
                let call = match n {
                    0..=2 if declared("task_get") => {
                        Some(("task_get", r#"{\"task_id\":\"KT-1\"}"#))
                    }
                    3 if declared("task_update_dod") => Some((
                        "task_update_dod",
                        r#"{\"task_id\":\"KT-1\",\"dod_id\":\"d1\",\"completed\":true}"#,
                    )),
                    4 if declared("task_get") => {
                        Some(("task_get", r#"{\"task_id\":\"KT-1\"}"#))
                    }
                    _ => None,
                };
                match call {
                    Some((tool, args)) => ResponseTemplate::new(200).set_body_string(sse(&[
                        &format!(
                            r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"c{n}","function":{{"name":"{tool}","arguments":"{args}"}}}}]}}}}]}}"#
                        ),
                    ])),
                    None => ResponseTemplate::new(200).set_body_string(sse(&[
                        r#"{"choices":[{"index":0,"delta":{"content":"fini"}}]}"#,
                    ])),
                }
            })
            .mount(&server)
            .await;

        let seen = Arc::new(Mutex::new(Vec::new()));
        let revision = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut process = start_ollama_http(
            &AgentType::LiteLlm,
            "où en suis-je",
            "",
            "test-model",
            None,
            Some(&server.uri()),
            None,
            Some(std::sync::Arc::new(PlanTools {
                seen: seen.clone(),
                revision: revision.clone(),
            })),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("start");

        while process.next_line().await.is_some() {}
        process.child.wait().await.expect("lifeline");

        let calls = seen.lock().unwrap().clone();
        assert!(
            calls.iter().any(|name| name == "task_update_dod"),
            "la mutation doit avoir eu lieu: {calls:?}"
        );
        // Deux lectures RÉELLEMENT exécutées : la première, puis celle d'après
        // la mutation. Les rejeux et refus n'atteignent pas l'exécuteur.
        let reads = calls.iter().filter(|name| *name == "task_get").count();
        assert_eq!(
            reads, 2,
            "une lecture avant, une après la mutation: {calls:?}"
        );
        assert_eq!(
            revision.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "la mutation a bien incrémenté l'état"
        );
        // Et surtout : le résultat de cette dernière lecture est PARVENU au
        // fournisseur, avec la valeur d'après-mutation. Sans requête suivante,
        // `livree` reste None et l'assertion tombe.
        let livree = livree
            .lock()
            .unwrap()
            .clone()
            .expect("une requête suit la dernière lecture, avec son résultat");
        assert_eq!(
            livree["revision"], 1,
            "le message d'outil c4 renvoyé au modèle porte la révision d'après-mutation: {livree}"
        );
    }
}
