//! KT-911 — each Agent attempt names its CLI session and what it cost.

use super::*;

/// Run one Claude Code Agent step against a scripted `claude` on the production
/// adapter route, so the attempt is recorded exactly as a real launch records it.
async fn run_claude_fixture(script: &str) -> StepResult {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_string_lossy().into_owned();
    let fixture = crate::acp::test_support::write_fixture_script(dir.path(), script);
    let work_dir = crate::agents::runner::resolve_agent_work_dir(Some(&project), &project).unwrap();
    let _route = crate::agents::runner::test_acp_routes::route(
        &work_dir,
        std::sync::Arc::new(crate::acp::ClaudeAcpAdapter::new_with_program(
            fixture.to_string_lossy(),
            None,
            false,
        )),
    );
    let step = WorkflowStep {
        name: "orchestrateur".into(),
        step_type: StepType::Agent,
        agent: AgentType::ClaudeCode,
        prompt_template: "Orchestrate".into(),
        ..WorkflowStep::default()
    };
    let tokens = TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys: vec![],
        disabled_overrides: vec![],
    };
    execute_step(
        &step,
        &project,
        &project,
        &tokens,
        false,
        &TemplateContext::new(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("test-run"),
    )
    .await
    .result
}

/// The attempt exposes the session the CLI reported on its `init` line, which is
/// the name of the transcript that CLI wrote. The fixture does what the real CLI
/// does: it takes its id from `--session-id` and names its transcript after it.
#[tokio::test]
async fn a_claude_code_attempt_exposes_the_session_named_by_its_transcript() {
    let transcripts = tempfile::tempdir().unwrap();
    let script = format!(
        r#"
cat >/dev/null
session=unset
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then session="$2"; fi
  shift
done
: > "{dir}/$session.jsonl"
printf '{{"type":"system","subtype":"init","session_id":"%s","model":"claude-opus-5-5"}}\n' "$session"
printf '%s\n' '{{"type":"assistant","message":{{"model":"claude-opus-5-5-20260915","content":[]}}}}'
printf '%s\n' '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"ok"}}}}}}'
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"usage":{{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":5}}}}'
"#,
        dir = transcripts.path().display()
    );
    let result = run_claude_fixture(&script).await;

    assert_eq!(result.status, RunStatus::Success, "{}", result.output);
    let provenance = result.agent_provenance.as_ref().unwrap();
    let session_id = provenance.attempts[0]
        .session_id
        .as_deref()
        .expect("the attempt names its CLI session");
    assert_ne!(session_id, "unset", "the CLI was given a session id");
    assert!(
        transcripts
            .path()
            .join(format!("{session_id}.jsonl"))
            .exists(),
        "session_id is the transcript's name: {session_id}"
    );
    let persisted: serde_json::Value = serde_json::to_value(&result).unwrap();
    assert_eq!(
        persisted["agent_provenance"]["attempts"][0]["session_id"],
        session_id
    );
}

/// The cost of an attempt is KT-894's: the detailed counters at the rates of the
/// model the provider reported, cache reads and writes apart.
#[tokio::test]
async fn a_claude_code_attempt_is_priced_from_its_detailed_counters() {
    let result = run_claude_fixture(crate::acp::test_support::CLAUDE_TURN_WITH_CACHE).await;

    let attempt = &result.agent_provenance.as_ref().unwrap().attempts[0];
    assert_eq!(attempt.session_id.as_deref(), Some("fixture-session"));
    // claude-opus-5-5: 4.00 in, 0.20 cache read, 5.00 cache write, 20.00 out per M.
    let expected = (48.0 * 4.0 + 1_554_330.0 * 0.2 + 80_271.0 * 5.0 + 21_545.0 * 20.0) / 1e6;
    let cost = attempt.cost_usd.expect("counters and a rate price it");
    assert!((cost - expected).abs() < 1e-9, "{cost} vs {expected}");
    assert_eq!(attempt.cost_unknown_reason, None);
    // Billing the 1.6M-token total as fresh input would have cost several times more.
    assert!(cost < 2.0, "{cost}");

    let persisted: serde_json::Value = serde_json::to_value(&result).unwrap();
    let attempt = &persisted["agent_provenance"]["attempts"][0];
    assert!(attempt["cost_usd"].is_f64());
    assert!(attempt.get("cost_unknown_reason").is_none());
}

/// An attempt that cannot be priced says so: `null` and the reason, never a zero
/// and never a guess from another model's rate.
#[tokio::test]
async fn a_claude_code_attempt_that_cannot_be_priced_says_why() {
    let turn = |model: &str, usage: &str| {
        format!(
            r#"
cat >/dev/null
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"s-unpriced"}}'
printf '%s\n' '{{"type":"assistant","message":{{"model":"{model}","content":[]}}}}'
printf '%s\n' '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"ok"}}}}}}'
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"usage":{usage}}}'
"#
        )
    };
    let full = r#"{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":5}"#;
    let no_cache = r#"{"input_tokens":10,"output_tokens":5}"#;

    for (script, reason) in [
        (turn("model-without-a-rate", full), "no confirmed rate"),
        (
            turn("claude-opus-5-5", no_cache),
            "cache reads were not reported",
        ),
    ] {
        let result = run_claude_fixture(&script).await;
        assert_eq!(result.status, RunStatus::Success, "{}", result.output);
        let attempt = &result.agent_provenance.as_ref().unwrap().attempts[0];
        assert_eq!(attempt.session_id.as_deref(), Some("s-unpriced"));
        assert_eq!(attempt.cost_usd, None, "{reason}");
        assert!(
            attempt
                .cost_unknown_reason
                .as_deref()
                .is_some_and(|text| text.contains(reason)),
            "{reason}: {:?}",
            attempt.cost_unknown_reason
        );
        let persisted: serde_json::Value = serde_json::to_value(&result).unwrap();
        assert!(persisted["agent_provenance"]["attempts"][0]["cost_usd"].is_null());
    }
}

fn reported(
    tokens: Option<u64>,
    usage: Option<runner::ReportedUsage>,
    cost: Option<f64>,
) -> Result<AgentOutput> {
    Ok(AgentOutput {
        attempt_id: 0,
        text: String::new(),
        tokens_used: tokens,
        prompt_cache: runner::PromptCacheUsage::default(),
        reported_usage: usage,
        reported_cost_usd: cost,
        native_tool_calls: vec![],
        runtime_notices: vec![],
    })
}

fn one_million_each(cache_read: Option<u64>, cache_write: Option<u64>) -> runner::ReportedUsage {
    runner::ReportedUsage {
        input_tokens: 1_000_000,
        output_tokens: 1_000_000,
        prompt_cache: runner::PromptCacheUsage {
            cached_prompt_tokens: cache_read,
            cache_write_prompt_tokens: cache_write,
        },
    }
}

#[test]
fn attempt_cost_is_computed_from_counters_and_the_agents_own_figure_wins() {
    let claude = AgentType::ClaudeCode;
    let sonnet = ["claude-sonnet-5-5".to_string()];
    let usage = one_million_each(Some(0), Some(0));

    // 1M in at $2 + 1M out at $10, no cache.
    let (cost, reason) = attempt_cost(
        &claude,
        &sonnet,
        None,
        &reported(Some(2_000_000), Some(usage), None),
    );
    assert!((cost.unwrap() - 12.0).abs() < 1e-9);
    assert_eq!(reason, None);

    let (cost, reason) = attempt_cost(
        &claude,
        &sonnet,
        None,
        &reported(Some(2_000_000), Some(usage), Some(0.5)),
    );
    assert_eq!((cost, reason), (Some(0.5), None));

    // With no model reported, the resolved or requested one stands in.
    let (cost, _) = attempt_cost(
        &claude,
        &[],
        Some("claude-sonnet-5-5"),
        &reported(Some(2_000_000), Some(usage), None),
    );
    assert!((cost.unwrap() - 12.0).abs() < 1e-9);
}

#[test]
fn attempt_cost_is_unknown_with_a_reason_when_the_runtime_did_not_report_enough() {
    let claude = AgentType::ClaudeCode;
    let sonnet = ["claude-sonnet-5-5".to_string()];
    let usage = one_million_each(Some(0), Some(0));

    for (case, result, observed, expected) in [
        (
            "a bare total",
            reported(Some(2_000_000), None, None),
            &sonnet[..],
            "only a token total",
        ),
        (
            "no model at all",
            reported(Some(2_000_000), Some(usage), None),
            &[][..],
            "model was not recorded",
        ),
        (
            "nothing reported",
            reported(None, None, None),
            &sonnet[..],
            "reported no usage",
        ),
        (
            "a failed attempt",
            Err(anyhow::anyhow!("stalled")),
            &sonnet[..],
            "failed before",
        ),
    ] {
        let (cost, reason) = attempt_cost(&claude, observed, None, &result);
        assert_eq!(cost, None, "{case}");
        assert!(
            reason
                .as_deref()
                .is_some_and(|text| text.contains(expected)),
            "{case}: {reason:?}"
        );
    }

    // Two served models share one aggregate usage that no single rate prices.
    let two = [
        "claude-sonnet-5-5".to_string(),
        "claude-haiku-4-5".to_string(),
    ];
    let (cost, reason) = attempt_cost(
        &claude,
        &two,
        None,
        &reported(Some(2_000_000), Some(usage), None),
    );
    assert_eq!(cost, None);
    assert!(reason.unwrap().contains("several models"));
}
