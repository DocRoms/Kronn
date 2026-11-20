// ─── #223: text blocks never share a line ──────────────────────────────────
#[cfg(all(test, feature = "full-tests"))]
mod text_block_tests {
    use crate::agents::runner::*;

    /// Accumulates a Claude stream the way the discussion loop does.
    fn joined_stream(lines: &[&str]) -> String {
        let mut joiner = TextBlockJoiner::default();
        let mut out = String::new();
        for line in lines {
            match parse_claude_stream_line(line) {
                StreamJsonEvent::Text(text) => out.push_str(&joiner.join(text)),
                StreamJsonEvent::ToolEnd => joiner.block_ended(),
                _ => {}
            }
        }
        out
    }

    fn text_delta(text: &str) -> String {
        serde_json::json!({
        "type": "stream_event",
        "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}
    })
    .to_string()
    }

    const BLOCK_STOP: &str =
        r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#;

    #[test]
    fn a_question_fence_closed_at_the_end_of_a_block_stays_closed_when_the_next_turn_follows() {
        // Issue #223: the second turn (relaunched by a Stop hook) was glued after
        // the closing fence, which then never closed and the card was refused.
        let first = text_delta("Une question :\n```kronn-question\n{\"version\":1}\n```");
        let second = text_delta("Je ne modifie pas `docs/css-style-guide.md`.");
        let out = joined_stream(&[&first, BLOCK_STOP, &second]);
        assert_eq!(
        out,
        "Une question :\n```kronn-question\n{\"version\":1}\n```\n\nJe ne modifie pas `docs/css-style-guide.md`."
    );
    }

    #[test]
    fn deltas_inside_one_block_are_joined_as_they_come() {
        let out = joined_stream(&[&text_delta("Hel"), &text_delta("lo")]);
        assert_eq!(out, "Hello");
    }

    #[test]
    fn a_new_block_adds_only_the_newlines_that_are_missing() {
        assert_eq!(
            joined_stream(&[&text_delta("a\n"), BLOCK_STOP, &text_delta("b")]),
            "a\n\nb"
        );
        assert_eq!(
            joined_stream(&[&text_delta("a\n\n"), BLOCK_STOP, &text_delta("b")]),
            "a\n\nb"
        );
        assert_eq!(
            joined_stream(&[&text_delta("a"), BLOCK_STOP, &text_delta("\nb")]),
            "a\n\nb"
        );
        assert_eq!(
            joined_stream(&[
                &text_delta("a"),
                &text_delta("\n"),
                BLOCK_STOP,
                &text_delta("b")
            ]),
            "a\n\nb",
            "a newline-only delta still counts as the end of the text"
        );
    }

    #[test]
    fn a_block_that_ends_before_any_text_adds_no_separator() {
        // A tool call first, then the answer: nothing precedes it.
        assert_eq!(
            joined_stream(&[BLOCK_STOP, &text_delta("Réponse")]),
            "Réponse"
        );
    }

    #[test]
    fn declared_step_tools_narrow_the_acp_session_servers() {
        let server = |id: &str| crate::acp::AcpMcpServer {
            id: id.into(),
            command: "python3".into(),
            args: vec!["bridge.py".into()],
            allowed_tools: vec![],
        };
        let servers = vec![server("kronn-internal"), server("project-safe")];
        let undeclared = super::super::declared_mcp_servers(servers.clone(), None);
        assert_eq!(undeclared.len(), 2, "an undeclared step keeps every server");
        assert_eq!(undeclared[0].args, vec!["bridge.py".to_string()]);

        let none = crate::models::StepTools::default();
        assert!(super::super::declared_mcp_servers(servers.clone(), Some(&none)).is_empty());

        let declared = crate::models::StepTools {
            cli: vec![],
            kronn_internal: vec!["task_get".into(), "plan_get".into()],
        };
        let narrowed = super::super::declared_mcp_servers(servers, Some(&declared));
        assert_eq!(narrowed.len(), 1);
        assert_eq!(narrowed[0].id, "kronn-internal");
        assert_eq!(
            narrowed[0].args,
            vec![
                "bridge.py".to_string(),
                "--step-tools=task_get,plan_get".to_string()
            ]
        );
        assert_eq!(narrowed[0].allowed_tools, declared.kronn_internal);
    }

    /// KT-1656 round 5 — the direct CLI route builds its command from the shared
    /// reach rule. With read-only grants the launch is restricted: the command
    /// carries `--allowedTools=mcp__kronn-internal` and no global bypass, and
    /// keeps the allow-list once the policy is applied. Fed the raw
    /// `launch_full_access` instead, the bypass is pushed, `policy.apply` strips
    /// it, and the launch is left without its Kronn tools.
    #[cfg(unix)] // ReadOnlyRepos::resolve* refuses Windows in check_platform
    #[test]
    fn a_direct_cli_launch_with_read_only_grants_keeps_its_kronn_tools() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        let dir = temp.path().join("shared");
        std::fs::create_dir(&work).unwrap();
        std::fs::create_dir(&dir).unwrap();
        let work = work.canonicalize().unwrap();
        let dir = dir.canonicalize().unwrap();
        let policy = crate::agents::read_only_repos::ReadOnlyRepos::resolve_with_dirs(
            &AgentType::ClaudeCode,
            &work,
            &[],
            &[dir.display().to_string()],
        )
        .unwrap()
        .expect("a read-only directory needs the policy");
        let launch_full_access = true;
        let build = |full_access: bool| {
            let (_, _, mut args, _, _, _) = super::super::agent_command_with_task_worker_policy(
                &AgentType::ClaudeCode,
                "prompt",
                full_access,
                "",
                None,
                None,
                false,
                None,
                None,
            );
            let before = args.clone();
            policy.apply(&AgentType::ClaudeCode, &work, &mut args);
            (before, args)
        };
        let allow = "--allowedTools=mcp__kronn-internal".to_string();
        let bypass = "--dangerously-skip-permissions".to_string();

        // What the route now passes: the shared rule, read-only grants present.
        let (before, after) = build(reach_is_full_access(launch_full_access, true));
        assert!(!before.contains(&bypass), "{before:?}");
        assert!(before.contains(&allow), "{before:?}");
        assert!(after.contains(&allow), "{after:?}");
        assert!(!after.contains(&bypass), "{after:?}");

        // Without grants the owner's full access is unchanged.
        let (before, _) = build(reach_is_full_access(launch_full_access, false));
        assert!(before.contains(&bypass), "{before:?}");
        assert!(!before.contains(&allow), "{before:?}");

        // The leak this closes: the raw flag loses the allow-list under a policy.
        let (_, raw_after) = build(launch_full_access);
        assert!(!raw_after.contains(&allow), "{raw_after:?}");
    }
}

// ─── the cost a gateway states in a header ─────────────────────────────────

#[test]
fn a_gateway_cost_header_is_read_in_micro_usd() {
    let mut headers = reqwest::header::HeaderMap::new();
    assert_eq!(
        crate::agents::runner::response_cost_header_micros(&headers),
        None
    );
    headers.insert("x-litellm-response-cost", "2.85e-05".parse().unwrap());
    assert_eq!(
        crate::agents::runner::response_cost_header_micros(&headers),
        Some(29)
    );
    headers.insert("x-litellm-response-cost", " 0.0412 ".parse().unwrap());
    assert_eq!(
        crate::agents::runner::response_cost_header_micros(&headers),
        Some(41_200)
    );
}

#[test]
fn a_cost_header_that_is_not_an_amount_is_no_cost() {
    for value in ["None", "", "-1", "NaN", "inf"] {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-litellm-response-cost", value.parse().unwrap());
        assert_eq!(
            crate::agents::runner::response_cost_header_micros(&headers),
            None,
            "{value}"
        );
    }
}
