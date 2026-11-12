//! KT-1694 — what a step's shadows and merge leave to the steps after it.

use super::*;
use crate::models::{StepShadow, WorkflowStep};
use crate::workflows::shadow::ExecutionIdentity;
use crate::workflows::shadow_merge::{ExecutionContribution, MissingShadow};

const MAIN_OUTPUT: &str = "SECRET-MAIN-OUTPUT: SQL injection in login.rs:12";
const SHADOW_OUTPUT: &str = "SECRET-SHADOW-OUTPUT: the cache leaks tenants";
const PROMPT: &str = "SECRET-PROMPT: review PR-42";

fn row(shadow_id: &str, status: ShadowStatus, agent: &str, output: &str) -> StepShadowRow {
    StepShadowRow {
        run_id: "run".into(),
        result_index: 1,
        step_name: "advise".into(),
        shadow_id: shadow_id.into(),
        label: String::new(),
        status,
        identity: ExecutionIdentity {
            agent: agent.into(),
            model: Some("configured-model".into()),
            connection_id: Some("connection-secret".into()),
            prompt: format!("inline:{PROMPT}"),
            skills: vec!["custom-x@3".into()],
        },
        output: output.into(),
        tokens_used: Some(120),
        duration_ms: Some(4_000),
        cost_usd: Some(0.5),
        model: Some("resolved-model".into()),
        stopped: None,
        started_at: "2026-11-11T10:00:00Z".into(),
        finished_at: (status != ShadowStatus::Running).then(|| "2026-11-11T10:00:04Z".into()),
    }
}

fn step(shadows: Vec<StepShadow>) -> WorkflowStep {
    WorkflowStep {
        name: "advise".into(),
        shadows,
        ..WorkflowStep::default()
    }
}

fn shadow(id: &str) -> StepShadow {
    StepShadow {
        id: id.into(),
        ..Default::default()
    }
}

fn result(status: RunStatus, output: &str) -> StepResult {
    StepResult {
        step_name: "advise".into(),
        status,
        output: output.into(),
        tokens_used: Some(900),
        duration_ms: 7_000,
        started_at: None,
        condition_result: None,
        envelope_detected: None,
        step_kind: None,
        step_agent: None,
        step_model: Some("main-model".into()),
        step_api_plugin_slug: None,
        step_api_endpoint_path: None,
        is_rollback: false,
        child_run_id: None,
        agent_provenance: None,
        native_tool_calls: Box::default(),
        cached_prompt_tokens: None,
        cache_write_prompt_tokens: None,
        last_activity: None,
        quota_wait: None,
        terminal_stop: None,
        output_bytes: None,
    }
}

fn point(verdict: PointVerdict, found_by: &[&str]) -> MergePoint {
    MergePoint {
        id: "p".into(),
        summary: "SECRET-POINT-SUMMARY".into(),
        text: Some("SECRET-POINT-TEXT".into()),
        found_by: found_by.iter().map(|s| s.to_string()).collect(),
        verdict,
        verified: true,
        evidence: Some("SECRET-EVIDENCE login.rs:12".into()),
        reason: Some("SECRET-REASON".into()),
        duplicate_of: None,
    }
}

fn report() -> ShadowMergeReport {
    ShadowMergeReport {
        status: MergeStatus::Merged,
        selected: vec!["argon".into(), "qwen".into()],
        contributors: vec!["argon".into()],
        missing: vec![MissingShadow {
            shadow_id: "qwen".into(),
            label: "Qwen".into(),
            reason: "still running when the merge stopped waiting (600 s)".into(),
        }],
        points: vec![
            point(PointVerdict::Kept, &["main", "shadow:argon"]),
            point(PointVerdict::Kept, &["shadow:argon"]),
            point(PointVerdict::Duplicate, &["shadow:argon", "main"]),
            point(PointVerdict::DroppedUnverified, &["shadow:argon"]),
        ],
        contributions: vec![
            ExecutionContribution {
                execution: "main".into(),
                kept: 1,
                unique: 0,
                duplicate: 0,
                dropped_unverified: 0,
            },
            ExecutionContribution {
                execution: "shadow:argon".into(),
                kept: 1,
                unique: 1,
                duplicate: 2,
                dropped_unverified: 1,
            },
        ],
        note: Some("SECRET-NOTE".into()),
        merged_output: "SECRET-MERGED-OUTPUT".into(),
        ..Default::default()
    }
}

fn execution<'a>(summary: &'a Value, key: &str) -> &'a Value {
    summary["executions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|execution| execution["key"] == key)
        .unwrap_or_else(|| panic!("no execution `{key}` in {summary}"))
}

#[test]
fn a_step_without_shadows_has_an_empty_summary() {
    let summary = summarize(&step(vec![]), &[], None, &result(RunStatus::Success, "x"));
    assert_eq!(summary, none("advise"));
    assert_eq!(
        summary,
        json!({"step": "advise", "executions": [], "merge": null, "determinism": null})
    );
}

#[test]
fn the_summary_names_every_execution_with_the_documented_fields() {
    let rows = vec![
        row("", ShadowStatus::Running, "ClaudeCode", ""),
        row("argon", ShadowStatus::Success, "LiteLlm", SHADOW_OUTPUT),
        row("qwen", ShadowStatus::Running, "Ollama", ""),
    ];
    let mut qwen = shadow("qwen");
    qwen.label = "Qwen".into();
    let summary = summarize(
        &step(vec![shadow("argon"), qwen]),
        &rows,
        None,
        &result(RunStatus::Success, MAIN_OUTPUT),
    );
    assert_eq!(summary["step"], "advise");
    let keys: Vec<&str> = summary["executions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["main", "argon", "qwen"]);

    // The main execution is read from the step's own result while it is open.
    let main = execution(&summary, "main");
    assert_eq!(main["status"], "success");
    assert_eq!(main["agent"], "ClaudeCode");
    assert_eq!(main["transport"], "cli");
    assert_eq!(main["model"], "main-model");
    assert_eq!(main["tokens"], 900);
    assert_eq!(main["duration_ms"], 7_000);
    assert_eq!(main["identical"], false);

    let argon = execution(&summary, "argon");
    assert_eq!(
        argon["label"], "argon",
        "the id stands in for an empty label"
    );
    assert_eq!(argon["transport"], "http");
    assert_eq!(argon["model"], "resolved-model");
    assert_eq!(argon["status"], "success");
    assert_eq!(argon["tokens"], 120);
    assert_eq!(argon["cost_usd"], 0.5);
    assert_eq!(argon["duration_ms"], 4_000);

    // A shadow still going is reported as such, and is local on Ollama.
    let qwen = execution(&summary, "qwen");
    assert_eq!(qwen["status"], "running");
    assert_eq!(qwen["transport"], "local");
    assert_eq!(qwen["identical"], true);
    assert!(summary["merge"].is_null());
}

#[test]
fn every_stored_status_has_a_name() {
    for (status, name) in [
        (ShadowStatus::Success, "success"),
        (ShadowStatus::Failed, "failed"),
        (ShadowStatus::Skipped, "skipped"),
        (ShadowStatus::OverBudget, "over_budget"),
        (ShadowStatus::TimedOut, "timed_out"),
        (ShadowStatus::Running, "running"),
        (ShadowStatus::Interrupted, "interrupted"),
    ] {
        let rows = vec![
            row("", ShadowStatus::Success, "ClaudeCode", MAIN_OUTPUT),
            row("argon", status, "LiteLlm", ""),
        ];
        let summary = summarize(
            &step(vec![shadow("argon")]),
            &rows,
            None,
            &result(RunStatus::Success, MAIN_OUTPUT),
        );
        assert_eq!(execution(&summary, "argon")["status"], name);
    }
}

#[test]
fn the_merge_reports_its_status_inclusions_gaps_and_counts() {
    let rows = vec![
        row("", ShadowStatus::Success, "ClaudeCode", MAIN_OUTPUT),
        row("argon", ShadowStatus::Success, "LiteLlm", SHADOW_OUTPUT),
    ];
    let summary = summarize(
        &step(vec![shadow("argon"), shadow("qwen")]),
        &rows,
        Some(&report()),
        &result(RunStatus::Success, "SECRET-MERGED-OUTPUT"),
    );
    let merge = &summary["merge"];
    assert_eq!(merge["status"], "merged");
    assert_eq!(merge["included"], json!(["argon"]));
    assert_eq!(
        merge["missing"],
        json!([{"key": "qwen", "reason": "still running when the merge stopped waiting (600 s)"}])
    );
    assert_eq!(
        merge["points"],
        json!({"kept": 2, "unique": 1, "duplicate": 1, "dropped": 1})
    );
    assert_eq!(
        merge["per_execution"],
        json!({
            "main": {"kept": 1, "duplicate": 0, "dropped": 0},
            "argon": {"kept": 1, "duplicate": 2, "dropped": 1},
        })
    );
}

#[test]
fn a_merged_step_keeps_the_main_executions_own_metrics() {
    // The merge stored the main execution's output and metrics in its row; the
    // step's result then holds the merged output and the condense pass's tokens.
    let rows = vec![
        row("", ShadowStatus::Success, "ClaudeCode", MAIN_OUTPUT),
        row("argon", ShadowStatus::Success, "LiteLlm", SHADOW_OUTPUT),
    ];
    let summary = summarize(
        &step(vec![shadow("argon")]),
        &rows,
        Some(&report()),
        &result(RunStatus::Success, "SECRET-MERGED-OUTPUT"),
    );
    let main = execution(&summary, "main");
    assert_eq!(main["tokens"], 120);
    assert_eq!(main["duration_ms"], 4_000);
}

#[test]
fn no_output_prompt_or_secret_reaches_the_summary() {
    let mut rows = vec![
        row("", ShadowStatus::Success, "ClaudeCode", MAIN_OUTPUT),
        row("argon", ShadowStatus::Success, "LiteLlm", SHADOW_OUTPUT),
        row(
            "twin",
            ShadowStatus::Failed,
            "ClaudeCode",
            "SECRET-FAILURE-OUTPUT",
        ),
    ];
    rows[2].stopped = Some("engine-written reason".into());
    let mut inline = shadow("argon");
    inline.prompt = Some(crate::models::ShadowPrompt::Inline {
        template: PROMPT.into(),
    });
    let summary = summarize(
        &step(vec![inline, shadow("twin")]),
        &rows,
        Some(&report()),
        &result(RunStatus::Success, "SECRET-MERGED-OUTPUT"),
    )
    .to_string();
    assert!(
        !summary.contains("SECRET"),
        "the summary carries something it must not: {summary}"
    );
    assert!(!summary.contains("connection-secret"), "{summary}");
    assert!(!summary.contains("custom-x"), "{summary}");
}

#[test]
fn identical_shadows_measure_the_determinism_of_the_step() {
    let same = "{\"data\": {\"cause\": \"origin 5xx\"}, \"duration_ms\": 10}";
    let again = "{\"data\": {\"cause\": \"origin 5xx\"}, \"duration_ms\": 99}";
    let other = "{\"data\": {\"cause\": \"cache poisoning\"}}";
    let rows = vec![
        row("", ShadowStatus::Success, "ClaudeCode", same),
        row("a", ShadowStatus::Success, "ClaudeCode", again),
        row("b", ShadowStatus::Success, "ClaudeCode", other),
        // Not identical: never counted.
        row("c", ShadowStatus::Success, "LiteLlm", "different agent"),
        // Identical but produced nothing.
        row("d", ShadowStatus::Failed, "ClaudeCode", ""),
    ];
    let mut changed = shadow("c");
    changed.agent = Some(crate::models::ShadowAgent {
        agent: AgentType::LiteLlm,
        model: None,
        connection_id: None,
        reasoning_effort: None,
    });
    let summary = summarize(
        &step(vec![shadow("a"), shadow("b"), changed, shadow("d")]),
        &rows,
        None,
        &result(RunStatus::Success, same),
    );
    assert_eq!(
        summary["determinism"],
        json!({"identical_runs": 3, "distinct_outputs": 2})
    );
}

#[test]
fn without_an_identical_shadow_there_is_no_determinism() {
    let mut changed = shadow("c");
    changed.prompt = Some(crate::models::ShadowPrompt::Inline {
        template: "other".into(),
    });
    let rows = vec![
        row("", ShadowStatus::Success, "ClaudeCode", MAIN_OUTPUT),
        row("c", ShadowStatus::Success, "ClaudeCode", SHADOW_OUTPUT),
    ];
    let summary = summarize(
        &step(vec![changed]),
        &rows,
        None,
        &result(RunStatus::Success, MAIN_OUTPUT),
    );
    assert!(summary["determinism"].is_null());
}

#[test]
fn a_step_whose_shadows_never_started_has_no_execution() {
    let summary = summarize(
        &step(vec![shadow("a")]),
        &[],
        None,
        &result(RunStatus::Success, "x"),
    );
    assert_eq!(summary["executions"], json!([]));
}

#[test]
fn outputs_are_compared_without_their_measurements() {
    assert_eq!(
        comparable_output("ok in 12 ms\n[SIGNAL: DONE]"),
        comparable_output("ok in 340 ms")
    );
    assert_eq!(
        comparable_output(
            "---STEP_OUTPUT---\n{\"data\": 1, \"elapsed_ms\": 5}\n---END_STEP_OUTPUT---"
        ),
        comparable_output("{\"data\": 1, \"elapsed_ms\": 9}")
    );
    assert_ne!(comparable_output("one"), comparable_output("two"));
}

#[test]
fn a_reason_is_cut() {
    let mut long = report();
    long.missing[0].reason = "x".repeat(2_000);
    let merge = merge_summary(&long);
    assert_eq!(
        merge["missing"][0]["reason"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        MAX_REASON_CHARS
    );
}
