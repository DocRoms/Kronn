#[cfg(test)]
mod tests {
    use crate::agents::runner::*;
    use crate::models::AgentType;
    use serial_test::serial;

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
        let body = build_ollama_chat_body("qwen3:8b", "sys", "hi", None, 8192);
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

    #[test]
    fn ollama_body_injects_no_think_for_qwen3_only() {
        // qwen3 → a dedicated `/no_think` system message is prepended.
        let q = build_ollama_chat_body("qwen3:30b-a3b", "", "hi", None, 8192);
        let msgs = q["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "/no_think");
        // Non-qwen3 (e.g. llama3.3) → no /no_think message at all.
        let l = build_ollama_chat_body("llama3.3:70b", "", "hi", None, 8192);
        let lmsgs = l["messages"].as_array().unwrap();
        assert!(
            !lmsgs.iter().any(|m| m["content"] == "/no_think"),
            "no_think must be qwen3-only"
        );
    }

    #[test]
    fn ollama_body_sends_think_false_for_qwen3_only() {
        // qwen3 → reasoning switched off in the body too, not just via /no_think.
        let q = build_ollama_chat_body("qwen3.8:27b-mlx", "", "hi", None, 8192);
        assert_eq!(q["think"], false);
        // Any other model → the field is absent, so its own default stands.
        // Never `think:true`, which would force reasoning ON.
        let l = build_ollama_chat_body("llama3.3:70b", "", "hi", None, 8192);
        assert!(
            l.get("think").is_none(),
            "think must be omitted for non-qwen3, got {:?}",
            l.get("think")
        );
    }

    #[test]
    fn ollama_body_format_switches_to_non_stream() {
        // No format → stream text.
        let free = build_ollama_chat_body("qwen3:8b", "", "hi", None, 8192);
        assert_eq!(free["stream"], true);
        assert!(free.get("format").is_none());
        // TypedSchema format → non-stream (one validated JSON blob) + schema passed through.
        let schema = serde_json::json!({"type":"object","properties":{"x":{"type":"integer"}}});
        let typed = build_ollama_chat_body("qwen3:8b", "", "hi", Some(&schema), 8192);
        assert_eq!(typed["stream"], false);
        assert_eq!(typed["format"], schema);
    }

    #[test]
    fn clamp_trims_the_biggest_tool_result_to_fit_the_cap() {
        let mut body = build_ollama_chat_body("qwen3.8:27b", "sys", "hi", None, 8192);
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
    fn clamp_never_blindly_cuts_a_checkpoint_receipt_envelope() {
        let mut body = build_ollama_chat_body("qwen3.8:27b", "sys", "hi", None, 4096);
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
        let mut body = build_ollama_chat_body("qwen3.8:27b", "sys", "hi", None, 32768);
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
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 8192);
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
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 8192);
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
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 8192);
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
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 8192);
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
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 4096);
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
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 8192);
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
        let mut body = build_ollama_chat_body("qwen3.8:27b", "", "hi", None, 32768);
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
        let mut body = build_ollama_chat_body("qwen3.8:27b", "sys", "hi", None, 32768);
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
        let mut body = build_ollama_chat_body("qwen3.8:27b", "sys", "hi", None, 8192);
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
    fn worker_exploration_policy_mitigates_only_native_ollama_mlx() {
        assert_eq!(
            worker_exploration_policy("qwen3.8:27b-mlx", None, false),
            WorkerExplorationPolicy {
                max_iterations: MLX_WORKER_EXPLORATION_ITERATIONS,
                max_observations_without_mutation: Some(
                    MLX_WORKER_MAX_OBSERVATIONS_WITHOUT_MUTATION,
                ),
                context_pressure_percent: MLX_WORKER_CONTEXT_PRESSURE_PERCENT,
                mlx_mitigation: true,
                mlx_detection_source: Some("model_tag_fallback"),
            }
        );
        assert_eq!(
            worker_exploration_policy("friendly-alias:latest", Some("safetensors"), false),
            WorkerExplorationPolicy {
                max_iterations: MLX_WORKER_EXPLORATION_ITERATIONS,
                max_observations_without_mutation: Some(
                    MLX_WORKER_MAX_OBSERVATIONS_WITHOUT_MUTATION,
                ),
                context_pressure_percent: MLX_WORKER_CONTEXT_PRESSURE_PERCENT,
                mlx_mitigation: true,
                mlx_detection_source: Some("model_format"),
            },
            "the real storage format must catch aliases that hide the mlx tag"
        );
        assert_eq!(
            worker_exploration_policy("qwen3.8:27b-q4_K_M", Some("gguf"), false),
            WorkerExplorationPolicy {
                max_iterations: crate::agents::tools::MAX_TOOL_ITERATIONS,
                max_observations_without_mutation: None,
                context_pressure_percent: DEFAULT_WORKER_CONTEXT_PRESSURE_PERCENT,
                mlx_mitigation: false,
                mlx_detection_source: None,
            }
        );
        assert_eq!(
            worker_exploration_policy("upstream:27b-mlx", Some("safetensors"), true),
            WorkerExplorationPolicy {
                max_iterations: crate::agents::tools::MAX_TOOL_ITERATIONS,
                max_observations_without_mutation: None,
                context_pressure_percent: DEFAULT_WORKER_CONTEXT_PRESSURE_PERCENT,
                mlx_mitigation: false,
                mlx_detection_source: None,
            }
        );
    }

    #[test]
    fn mlx_worker_uses_one_bounded_context_cap_for_the_whole_run() {
        let mlx = worker_exploration_policy("alias:latest", Some("safetensors"), false);
        let gguf = worker_exploration_policy("alias:latest", Some("gguf"), false);

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
        let mlx = worker_exploration_policy("alias:latest", Some("safetensors"), false);
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

        let gguf = worker_exploration_policy("alias:latest", Some("gguf"), false);
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

        let previous_host = std::env::var("OLLAMA_HOST").ok();
        std::env::set_var("OLLAMA_HOST", server.uri());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "perform the exact tiny edit",
            "",
            "test-model",
            None,
            None,
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
        )
        .await;
        match previous_host {
            Some(value) => std::env::set_var("OLLAMA_HOST", value),
            None => std::env::remove_var("OLLAMA_HOST"),
        }

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

        let previous_host = std::env::var("OLLAMA_HOST").ok();
        std::env::set_var("OLLAMA_HOST", server.uri());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "perform the exact tiny edit",
            "",
            "test-model",
            None,
            None,
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
        )
        .await;
        match previous_host {
            Some(value) => std::env::set_var("OLLAMA_HOST", value),
            None => std::env::remove_var("OLLAMA_HOST"),
        }

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

    #[test]
    fn provider_retry_classifier_separates_capacity_from_permanent_failures() {
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

        let prev = std::env::var("OLLAMA_HOST").ok();
        std::env::set_var("OLLAMA_HOST", server.uri());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "which servers?",
            "",
            "test-model",
            None,
            None,
            None,
            Some(std::sync::Arc::new(FakeTools { seen: seen.clone() })),
            None,
            None,
            None,
        )
        .await;
        match prev {
            Some(v) => std::env::set_var("OLLAMA_HOST", v),
            None => std::env::remove_var("OLLAMA_HOST"),
        }

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

        let previous_host = std::env::var("OLLAMA_HOST").ok();
        std::env::set_var("OLLAMA_HOST", server.uri());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "complete the worker task",
            "",
            "test-model",
            None,
            None,
            None,
            Some(std::sync::Arc::new(WorkerTools { seen: seen.clone() })),
            None,
            None,
            None,
        )
        .await;
        match previous_host {
            Some(value) => std::env::set_var("OLLAMA_HOST", value),
            None => std::env::remove_var("OLLAMA_HOST"),
        }

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

        let previous_host = std::env::var("OLLAMA_HOST").ok();
        std::env::set_var("OLLAMA_HOST", server.uri());
        let started = start_ollama_http(
            &AgentType::Ollama,
            "complete the worker task",
            "",
            "test-model",
            None,
            None,
            None,
            Some(std::sync::Arc::new(WorkerTools {
                seen: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            })),
            None,
            None,
            None,
        )
        .await;
        match previous_host {
            Some(value) => std::env::set_var("OLLAMA_HOST", value),
            None => std::env::remove_var("OLLAMA_HOST"),
        }

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

        let previous_host = std::env::var("OLLAMA_HOST").ok();
        std::env::set_var("OLLAMA_HOST", server.uri());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "complete the worker task",
            "",
            "test-model",
            None,
            None,
            None,
            Some(std::sync::Arc::new(WorkerTools { seen: seen.clone() })),
            None,
            None,
            None,
        )
        .await;
        match previous_host {
            Some(value) => std::env::set_var("OLLAMA_HOST", value),
            None => std::env::remove_var("OLLAMA_HOST"),
        }

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

        let previous_host = std::env::var("OLLAMA_HOST").ok();
        std::env::set_var("OLLAMA_HOST", server.uri());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "complete the worker task",
            "",
            "test-model",
            None,
            None,
            None,
            Some(std::sync::Arc::new(WorkerTools { seen: seen.clone() })),
            None,
            None,
            None,
        )
        .await;
        match previous_host {
            Some(value) => std::env::set_var("OLLAMA_HOST", value),
            None => std::env::remove_var("OLLAMA_HOST"),
        }

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

        let previous_host = std::env::var("OLLAMA_HOST").ok();
        std::env::set_var("OLLAMA_HOST", server.uri());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = start_ollama_http(
            &AgentType::Ollama,
            "complete the scoped worker task",
            "",
            "local-alias:latest",
            None,
            None,
            None,
            Some(std::sync::Arc::new(WorkerTools { seen: seen.clone() })),
            None,
            None,
            None,
        )
        .await;
        match previous_host {
            Some(value) => std::env::set_var("OLLAMA_HOST", value),
            None => std::env::remove_var("OLLAMA_HOST"),
        }

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
        assert_eq!(max_calls_for_tool("api_call", ToolRunMode::General), 12);
        assert_eq!(max_calls_for_tool("mcp_list", ToolRunMode::General), 12);
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

        // Same wire format, same bug surface, for the other two HTTP backends
        // that share this branch.
        let (_, tokens) = parse_token_usage(&AgentType::LiteLlm, "response", &stderr);
        assert_eq!(tokens, 19);
        let (_, tokens) = parse_token_usage(&AgentType::Nvidia, "response", &stderr);
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
            true,
            Some(worktree.path()),
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
            true,
            Some(worktree.path()),
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
            true,
            Some(worktree.path()),
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
    fn claude_task_worker_allows_exact_commit_then_delivery_tools() {
        let (_, _, args, _, _, _) = super::super::agent_command_with_task_worker_policy(
            &AgentType::ClaudeCode,
            "test prompt",
            true,
            "worker context",
            None,
            true,
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
            true,
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
                toml::Value::String("task_exec_commit".into()),
                toml::Value::String("task_exec_deliver".into()),
            ])
        );
        assert_eq!(
            internal["default_tools_approval_mode"].as_str(),
            Some("prompt")
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
            Some(2),
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
            "/tmp/disc-introspection-mcp.py",
        ))
        .expect("a concrete bridge path is renderable");
        assert!(rendered.contains("command=\"python3\""));
        assert!(rendered.contains("/tmp/disc-introspection-mcp.py"));
        assert!(rendered.contains("KRONN_TASK_WORKER_CONTEXT"));
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
                true,
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
    fn resolve_model_flag_claude_code_tiers() {
        use crate::models::ModelTier;
        assert_eq!(
            resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Economy, None),
            Some("haiku".into())
        );
        assert_eq!(
            resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Default, None),
            Some("sonnet".into())
        );
        assert_eq!(
            resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Reasoning, None),
            Some("opus".into())
        );
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
        // unset tiers still fall through to the built-ins
        assert_eq!(
            resolve_model_flag(&AgentType::ClaudeCode, ModelTier::Economy, Some(&cfg)),
            Some("haiku".into())
        );
    }

    #[test]
    fn resolve_model_flag_codex_tiers() {
        use crate::models::ModelTier;
        assert_eq!(
            resolve_model_flag(&AgentType::Codex, ModelTier::Economy, None),
            Some("gpt-5.6-luna".into())
        );
        assert_eq!(
            resolve_model_flag(&AgentType::Codex, ModelTier::Default, None),
            None
        );
        assert_eq!(
            resolve_model_flag(&AgentType::Codex, ModelTier::Reasoning, None),
            Some("gpt-5.6-sol".into())
        );
    }

    #[test]
    fn resolve_model_flag_gemini_tiers() {
        use crate::models::ModelTier;
        assert_eq!(
            resolve_model_flag(&AgentType::GeminiCli, ModelTier::Economy, None),
            Some("gemini-2.5-flash".into())
        );
        assert_eq!(
            resolve_model_flag(&AgentType::GeminiCli, ModelTier::Default, None),
            None
        );
        assert_eq!(
            resolve_model_flag(&AgentType::GeminiCli, ModelTier::Reasoning, None),
            Some("gemini-3.1-pro-preview".into())
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
            resolve_model_flag(
                &AgentType::ClaudeCode,
                ModelTier::Reasoning,
                Some(&overrides)
            ),
            Some("opus".into()),
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
            },
            ..Default::default()
        };
        assert_eq!(
            resolve_model_flag(&AgentType::Ollama, ModelTier::Default, Some(&overrides)),
            Some("gemma3:27b".into()),
            "Default-tier user override must win over the built-in qwen3 fallback",
        );
        // Without an override, the legacy built-in is still served.
        let no_override = ModelTiersConfig::default();
        assert_eq!(
            resolve_model_flag(&AgentType::Ollama, ModelTier::Default, Some(&no_override)),
            Some("qwen3:8b".into()),
            "No override → portable built-in default (small, fits most machines), never a bare/absent name",
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
        // The all-empty case is unchanged: portable built-ins per tier.
        assert_eq!(
            resolve_model_flag(&AgentType::Ollama, ModelTier::Reasoning, None),
            Some("qwen3:30b-a3b".into()),
        );
    }

    // ─── Ollama tier fallbacks are real, pullable tags (no opaque 404) ────────
    #[test]
    fn resolve_model_flag_ollama_tiers_are_pullable_tags() {
        use crate::models::ModelTier;
        // Regression: the old fallbacks were `llama3.2` (not pulled) and the
        // bare `qwen3` (not a pullable tag) → opaque Ollama 404 at run time.
        // Portability-first: Default is a small, universal model (qwen3:8b);
        // Reasoning is the only heavy opt-in fallback.
        assert_eq!(
            resolve_model_flag(&AgentType::Ollama, ModelTier::Economy, None),
            Some("qwen3:8b".into())
        );
        assert_eq!(
            resolve_model_flag(&AgentType::Ollama, ModelTier::Default, None),
            Some("qwen3:8b".into())
        );
        assert_eq!(
            resolve_model_flag(&AgentType::Ollama, ModelTier::Reasoning, None),
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
            8192,
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
        // Ollama unreachable → legacy portable default.
        assert_eq!(at(None, None).value, 8192);
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
        assert!(notice.contains("8192"), "state the fallback used: {notice}");
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
    fn effective_model_flag_override_wins_over_tier() {
        use crate::models::ModelTier;
        // Explicit model beats the tier fallback — including the Economy tier
        // that would otherwise resolve to the qwen3:8b built-in.
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
        // Blank override is treated as unset → tier resolution.
        assert_eq!(
            effective_model_flag(Some("   "), &AgentType::Ollama, ModelTier::Default, None),
            resolve_model_flag(&AgentType::Ollama, ModelTier::Default, None),
        );
        // None → identical to resolve_model_flag (here: Claude reasoning → opus).
        assert_eq!(
            effective_model_flag(None, &AgentType::ClaudeCode, ModelTier::Reasoning, None),
            Some("opus".into()),
        );
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
            true,
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
    fn plugin_invocation_rule_reaches_every_supported_agent_command() {
        let rule = "Fastly production: API first via `api_call`";
        let agents = [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::Vibe,
            AgentType::GeminiCli,
            AgentType::Kiro,
            AgentType::CopilotCli,
            AgentType::Ollama,
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
            stderr_task: None,
            http_cancel: None,
            pgid,
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
}
