//! KT-1544 — a step may merge its main execution with the shadows it selects:
//! the selected shadows are waited for (with a timeout), a condense pass dedups
//! and verifies their points, and the merged result becomes the step's output.
//! Anything that goes wrong leaves the main output as it was, and a point the
//! condense pass could not verify is never published.
//!
//! The main agent (Claude Code) answers by what it is asked: the review as the
//! main execution, a second look as the `other` shadow, the condense pass as
//! the merge. The `local` shadow runs on its own agent (Codex).

use super::*;
use crate::models::ShadowMerge;
use crate::workflows::shadow_merge::{MergeStatus, PointVerdict, PROVENANCE_BEGIN, PROVENANCE_END};

const MAIN_REVIEW: &str = "MAIN: SQL injection in login.rs:12";
const LOCAL_REVIEW: &str =
    "LOCAL: SQL injection in login.rs:12\nINVENTED: the cache layer leaks tenants";
const OTHER_REVIEW: &str = "OTHER: Surrogate-Capability poisoning in edge.rs:7";
const MERGED_REVIEW: &str =
    "## Review\n- SQL injection (login.rs:12)\n- Surrogate-Capability poisoning (edge.rs:7)";
/// What only the condense pass's prompt says.
const CONDENSE_MARK: &str = "CONDENSE pass";

/// What a faithful condense pass answers: the invented point is dropped.
fn faithful_condense() -> String {
    format!(
        "{PROVENANCE_BEGIN}\n{}\n{PROVENANCE_END}\n{MERGED_REVIEW}",
        serde_json::json!({"points": [
            {"id": "p1", "summary": "SQL injection", "text": "SQL injection in login.rs:12",
             "found_by": ["main", "shadow:local"], "verdict": "kept", "verified": true,
             "evidence": "login.rs:12"},
            {"id": "p2", "summary": "cache leaks tenants",
             "text": "the cache layer leaks tenants", "found_by": ["shadow:local"],
             "verdict": "dropped_unverified", "verified": false,
             "reason": "no such cache layer in the tree"},
            {"id": "p3", "summary": "Surrogate-Capability",
             "text": "Surrogate-Capability poisoning in edge.rs:7",
             "found_by": ["shadow:other"], "verdict": "kept", "verified": true,
             "evidence": "edge.rs:7"}
        ]})
    )
}

/// A condense pass that lets the invented point through.
fn leaking_condense() -> String {
    format!(
        "{PROVENANCE_BEGIN}\n{}\n{PROVENANCE_END}\n{MERGED_REVIEW}\n- The cache layer leaks tenants",
        serde_json::json!({"points": [
            {"id": "p1", "summary": "cache leaks tenants",
             "text": "the cache layer leaks tenants", "found_by": ["shadow:local"],
             "verdict": "dropped_unverified", "verified": false, "reason": "no such cache layer"}
        ]})
    )
}

fn merge() -> ShadowMerge {
    ShadowMerge {
        timeout_secs: Some(30),
        ..Default::default()
    }
}

/// The main agent answers the review, the second look and the condense pass
/// each with its own text; the `local` shadow's agent is scripted apart.
async fn merged_fixture(project: &str, condense_answer: String) -> Fixture {
    let mut main = scripted(MAIN_REVIEW);
    main.replies = vec![
        (CONDENSE_MARK.into(), condense_answer),
        ("Second look".into(), OTHER_REVIEW.into()),
    ];
    let mut fx = fixture(project, main).await;
    fx.model_for(AgentType::Codex, scripted(LOCAL_REVIEW));
    cite_files(&fx);
    fx
}

/// The files the condense pass cites: a citation counts only if it opens.
fn cite_files(fx: &Fixture) {
    for (name, lines) in [("login.rs", 30), ("edge.rs", 20)] {
        std::fs::write(fx.work_dir.join(name), "fn line() {}\n".repeat(lines)).unwrap();
    }
}

/// `local`: another agent; `other`: the main agent asked another way.
fn merged_workflow(fx: &Fixture, merge: Option<ShadowMerge>) -> Workflow {
    let mut local = shadow_of("local");
    local.agent = on(AgentType::Codex, "qwen3:30b");
    let mut other = shadow_of("other");
    other.prompt = Some(ShadowPrompt::Inline {
        template: "Second look at PR-42 :: {{steps.who.summary}}".into(),
    });
    let mut wf = fx.workflow(vec![local, other]);
    wf.steps[1].shadow_merge = merge;
    wf
}

/// What the main agent was asked to condense.
fn condense_prompts(fx: &Fixture) -> Vec<String> {
    prompts_of(&fx.main_prompts)
        .into_iter()
        .filter(|prompt| prompt.contains(CONDENSE_MARK))
        .collect()
}

async fn comparison(fx: &Fixture, run_id: &str) -> shadow::StepShadowComparison {
    let id = run_id.to_string();
    fx.state
        .db
        .with_read_conn(move |conn| {
            let run = crate::db::workflows::get_run(conn, &id)?.unwrap();
            shadow::comparisons(conn, &run)
        })
        .await
        .unwrap()
        .remove(0)
}

fn result_of<'a>(run: &'a WorkflowRun, step: &str) -> &'a StepResult {
    run.step_results
        .iter()
        .find(|result| result.step_name == step)
        .unwrap()
}

fn summary_prompt(fx: &Fixture) -> String {
    prompts_of(&fx.main_prompts)
        .into_iter()
        .find(|prompt| prompt.contains("Summary of:"))
        .expect("the summary step asked the agent")
}

#[tokio::test]
async fn a_merge_condenses_the_main_review_and_its_shadows_into_the_step_output() {
    let fx = merged_fixture("proj-merge-ok", faithful_condense()).await;
    let wf = merged_workflow(&fx, Some(merge()));
    let run = fx.run(&wf, "run-merge-ok").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);

    // The merged result is the step's output, and the next step reads it.
    assert_eq!(result_of(&run, "review").output, MERGED_REVIEW);
    let summary = summary_prompt(&fx);
    assert!(summary.contains("Surrogate-Capability"), "{summary}");
    assert!(!summary.contains("tenants"), "{summary}");

    // The condense pass read the main review and both shadows.
    let asked = condense_prompts(&fx);
    assert_eq!(asked.len(), 1, "{asked:?}");
    for text in [MAIN_REVIEW, LOCAL_REVIEW, OTHER_REVIEW] {
        assert!(asked[0].contains(text), "{}", asked[0]);
    }
    assert!(
        asked[0].contains("main, shadow:local, shadow:other"),
        "{}",
        asked[0]
    );
    assert!(asked[0].contains("VERIFY every point"), "{}", asked[0]);

    // The main and shadow outputs stay readable beside the merged result.
    let review = comparison(&fx, "run-merge-ok").await;
    assert_eq!(review.main.output, MAIN_REVIEW);
    assert_eq!(review.shadows[0].output, LOCAL_REVIEW);
    let report = review.merge.expect("the merge is reported");
    assert_eq!(report.status, MergeStatus::Merged, "{report:?}");
    assert_eq!(report.merged_output, MERGED_REVIEW);
    assert_eq!(report.contributors, ["local", "other"]);
    assert!(report.missing.is_empty(), "{:?}", report.missing);

    // Provenance: which executions found each point, and what was dropped.
    assert_eq!(report.points.len(), 3);
    let dropped = report
        .points
        .iter()
        .find(|point| point.verdict == PointVerdict::DroppedUnverified)
        .unwrap();
    assert_eq!(dropped.found_by, ["shadow:local"]);
    assert!(!report.merged_output.contains("tenants"));
    let counts = |execution: &str| {
        report
            .contributions
            .iter()
            .find(|c| c.execution == execution)
            .unwrap()
            .clone()
    };
    assert_eq!((counts("main").kept, counts("main").unique), (1, 0));
    assert_eq!(
        counts("shadow:local").duplicate,
        1,
        "it restated the main point"
    );
    assert_eq!(counts("shadow:local").dropped_unverified, 1);
    assert_eq!(counts("shadow:other").unique, 1);
}

#[tokio::test]
async fn a_point_the_condense_pass_could_not_verify_is_never_published() {
    let fx = merged_fixture("proj-merge-leak", leaking_condense()).await;
    let wf = merged_workflow(&fx, Some(merge()));
    let run = fx.run(&wf, "run-merge-leak").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);

    // The result that carried the invented finding is refused: the main review
    // stands as the step's output and the next step never sees the invention.
    assert_eq!(result_of(&run, "review").output, MAIN_REVIEW);
    let summary = summary_prompt(&fx);
    assert!(!summary.contains("tenants"), "{summary}");
    let report = comparison(&fx, "run-merge-leak").await.merge.unwrap();
    assert_eq!(report.status, MergeStatus::Rejected);
    assert!(
        report
            .note
            .as_deref()
            .unwrap()
            .contains("appears in the result"),
        "{report:?}"
    );
    assert!(report.merged_output.is_empty());
}

#[tokio::test]
async fn a_shadow_only_point_kept_without_verification_is_refused() {
    let unverified = format!(
        "{PROVENANCE_BEGIN}\n{}\n{PROVENANCE_END}\n{MERGED_REVIEW}",
        serde_json::json!({"points": [
            {"id": "p1", "summary": "x", "found_by": ["shadow:local"], "verdict": "kept"}
        ]})
    );
    let fx = merged_fixture("proj-merge-unverified", unverified).await;
    let wf = merged_workflow(&fx, Some(merge()));
    let run = fx.run(&wf, "run-merge-unverified").await;
    assert_eq!(result_of(&run, "review").output, MAIN_REVIEW);
    let report = comparison(&fx, "run-merge-unverified").await.merge.unwrap();
    assert_eq!(report.status, MergeStatus::Rejected, "{report:?}");
}

/// A condense pass whose provenance is `provenance` (raw JSON) over the merged review.
fn condense_with(provenance: &str) -> String {
    format!("{PROVENANCE_BEGIN}\n{provenance}\n{PROVENANCE_END}\n{MERGED_REVIEW}")
}

#[tokio::test]
async fn a_fabricated_verdict_is_refused_and_the_principals_output_stands() {
    // KT-1731: no point, an empty claim, an unsupported point, or a verified one
    // on a citation that does not exist each leave the main review as the step's
    // output and say why.
    let cases = [
        ("empty-list", r#"{"points":[]}"#, "listed no point"),
        ("empty-object", "{}", "listed no point"),
        (
            "fabricated",
            r#"{"points":[{"id":"p1","summary":"x","found_by":["main","shadow:local"],"verdict":"kept","verified":true,"evidence":"nonexistent.rs:999999"}]}"#,
            "does not resolve",
        ),
        (
            "unsupported",
            r#"{"points":[{"id":"p1","summary":"x","found_by":["main"],"verdict":"kept"}]}"#,
            "gives no evidence",
        ),
    ];
    for (name, provenance, why) in cases {
        let project = format!("proj-merge-fabricated-{name}");
        let run_id = format!("run-merge-fabricated-{name}");
        let fx = merged_fixture(&project, condense_with(provenance)).await;
        let wf = merged_workflow(&fx, Some(merge()));
        let run = fx.run(&wf, &run_id).await;
        assert_eq!(
            run.status,
            RunStatus::Success,
            "{name}: {:?}",
            run.step_results
        );

        // The principal's output is the step's output; the next step never sees the merge.
        assert_eq!(result_of(&run, "review").output, MAIN_REVIEW, "{name}");
        assert!(
            !summary_prompt(&fx).contains("Surrogate-Capability"),
            "{name}: the refused result reached the next step"
        );

        // The step says why, and the refused result is not kept as a merged one.
        let report = comparison(&fx, &run_id).await.merge.unwrap();
        assert_eq!(report.status, MergeStatus::Rejected, "{name}: {report:?}");
        let note = report.note.as_deref().unwrap();
        assert!(note.contains(why), "{name}: {note}");
        assert!(
            note.contains("main execution's output stands"),
            "{name}: {note}"
        );
        assert!(report.merged_output.is_empty(), "{name}");
    }
}

#[tokio::test]
async fn an_agreement_with_no_point_is_a_valid_merge_when_stated() {
    let provenance = r#"{"points":[],"agreement":"every execution reports nothing to fix"}"#;
    let fx = merged_fixture("proj-merge-agreed", condense_with(provenance)).await;
    let wf = merged_workflow(&fx, Some(merge()));
    let run = fx.run(&wf, "run-merge-agreed").await;
    assert_eq!(result_of(&run, "review").output, MERGED_REVIEW);
    let report = comparison(&fx, "run-merge-agreed").await.merge.unwrap();
    assert_eq!(report.status, MergeStatus::Merged, "{report:?}");
    assert!(
        report
            .note
            .as_deref()
            .unwrap()
            .contains("every execution reports nothing to fix"),
        "{report:?}"
    );
}

#[tokio::test]
async fn a_dead_shadow_never_hangs_the_merge_or_the_run() {
    let mut fx = merged_fixture("proj-merge-dead", faithful_condense()).await;
    // `local` never answers (replaces the route the fixture gave it).
    let mut dead = scripted("never answers");
    dead.delay = Some(std::time::Duration::from_secs(300));
    fx.model_for(AgentType::Codex, dead);
    let mut short = merge();
    short.timeout_secs = Some(1);
    let wf = merged_workflow(&fx, Some(short));

    let started = std::time::Instant::now();
    let run = fx.run(&wf, "run-merge-dead").await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "the run does not wait for the dead shadow"
    );
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);

    // It goes on with the shadow that finished and says which one is missing.
    let report = comparison(&fx, "run-merge-dead").await.merge.unwrap();
    assert_eq!(report.status, MergeStatus::Merged, "{report:?}");
    assert_eq!(report.contributors, ["other"]);
    assert_eq!(report.missing.len(), 1);
    assert_eq!(report.missing[0].shadow_id, "local");
    assert!(
        report.missing[0].reason.contains("still running"),
        "{:?}",
        report.missing
    );
    let asked = condense_prompts(&fx);
    assert!(asked[0].contains(OTHER_REVIEW) && !asked[0].contains("never answers"));
}

#[tokio::test]
async fn with_no_shadow_finished_the_main_output_stands() {
    let mut fx = merged_fixture("proj-merge-none", faithful_condense()).await;
    let mut failing = scripted("");
    failing.fails = true;
    fx.model_for(AgentType::Codex, failing);
    // Only the failing shadow is selected.
    let mut only_local = merge();
    only_local.shadows = vec!["local".into()];
    let wf = merged_workflow(&fx, Some(only_local));
    let run = fx.run(&wf, "run-merge-none").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert_eq!(result_of(&run, "review").output, MAIN_REVIEW);
    let report = comparison(&fx, "run-merge-none").await.merge.unwrap();
    assert_eq!(report.status, MergeStatus::Skipped, "{report:?}");
    assert_eq!(report.missing.len(), 1);
    assert_eq!(report.missing[0].shadow_id, "local");
    assert!(
        condense_prompts(&fx).is_empty(),
        "no condense pass is spent when there is nothing to merge"
    );
}

#[tokio::test]
async fn a_condense_pass_that_fails_leaves_the_main_output() {
    let mut fx = merged_fixture("proj-merge-failed", faithful_condense()).await;
    let mut broken = scripted("");
    broken.fails = true;
    fx.model_for(AgentType::Codex, broken);
    let mut on_codex = merge();
    on_codex.agent = on(AgentType::Codex, "strongest");
    let wf = merged_workflow(&fx, Some(on_codex));
    let run = fx.run(&wf, "run-merge-failed").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert_eq!(result_of(&run, "review").output, MAIN_REVIEW);
    let report = comparison(&fx, "run-merge-failed").await.merge.unwrap();
    assert_eq!(report.status, MergeStatus::Failed, "{report:?}");
}

#[tokio::test]
async fn the_condense_pass_runs_on_the_agent_the_merge_names() {
    let mut fx = merged_fixture("proj-merge-agent", String::new()).await;
    let mut strongest = scripted(LOCAL_REVIEW);
    strongest.replies = vec![(CONDENSE_MARK.into(), faithful_condense())];
    let on_codex = fx.model_for(AgentType::Codex, strongest);
    let mut named = merge();
    named.agent = on(AgentType::Codex, "strongest");
    let wf = merged_workflow(&fx, Some(named));
    let run = fx.run(&wf, "run-merge-agent").await;
    assert_eq!(result_of(&run, "review").output, MERGED_REVIEW);
    assert!(
        condense_prompts(&fx).is_empty(),
        "the main agent was not asked to condense"
    );
    assert_eq!(
        prompts_of(&on_codex)
            .iter()
            .filter(|prompt| prompt.contains(CONDENSE_MARK))
            .count(),
        1
    );
}

#[tokio::test]
async fn the_merge_selects_which_shadows_merge_with_the_main_execution() {
    let fx = merged_fixture("proj-merge-pick", faithful_condense()).await;
    let mut only_other = merge();
    only_other.shadows = vec!["other".into()];
    let wf = merged_workflow(&fx, Some(only_other));
    let run = fx.run(&wf, "run-merge-pick").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    // Both shadows still run and are compared; only `other` is condensed.
    fx.settled("run-merge-pick", 2).await;
    let asked = condense_prompts(&fx);
    assert!(asked[0].contains(OTHER_REVIEW), "{}", asked[0]);
    assert!(!asked[0].contains("LOCAL:"), "{}", asked[0]);
    let review = comparison(&fx, "run-merge-pick").await;
    assert_eq!(review.shadows.len(), 2);
    let report = review.merge.unwrap();
    assert_eq!(report.selected, ["other"]);
    assert_eq!(report.contributors, ["other"]);
}

#[tokio::test]
async fn a_step_with_shadows_only_compares_them_unless_it_merges() {
    let fx = merged_fixture("proj-merge-off", faithful_condense()).await;
    let mut off = merge();
    off.enabled = false;
    for (id, setting) in [("run-merge-unset", None), ("run-merge-off", Some(off))] {
        let wf = merged_workflow(&fx, setting);
        let run = fx.run(&wf, id).await;
        assert_eq!(result_of(&run, "review").output, MAIN_REVIEW, "{id}");
        fx.settled(id, 2).await;
        assert!(comparison(&fx, id).await.merge.is_none(), "{id}");
    }
    assert!(condense_prompts(&fx).is_empty());
}

#[tokio::test]
async fn the_condense_passes_tokens_are_reported_and_counted_in_the_step() {
    let mut main = scripted(MAIN_REVIEW);
    main.replies = vec![
        (CONDENSE_MARK.into(), faithful_condense()),
        ("Second look".into(), OTHER_REVIEW.into()),
    ];
    main.usage = Some((700, 300));
    let mut fx = fixture("proj-merge-tokens", main).await;
    fx.model_for(AgentType::Codex, scripted(LOCAL_REVIEW));
    cite_files(&fx);
    let wf = merged_workflow(&fx, Some(merge()));
    let run = fx.run(&wf, "run-merge-tokens").await;
    let review = comparison(&fx, "run-merge-tokens").await;
    let report = review.merge.unwrap();
    assert_eq!(report.status, MergeStatus::Merged, "{report:?}");
    let condense = report.tokens_used.unwrap_or_default();
    assert!(condense > 0, "the merge reports what it spent");
    assert_eq!(
        result_of(&run, "review").tokens_used.unwrap_or_default(),
        review.main.tokens_used.unwrap_or_default() + condense,
        "the step's tokens include the merge; the main row keeps its own"
    );
}

#[test]
fn a_merge_is_checked_when_a_person_saves_it() {
    let with = |merge: ShadowMerge, shadows: Vec<StepShadow>| {
        let mut step = review_step();
        step.shadows = shadows;
        step.shadow_merge = Some(merge);
        shadow::validate(&[step])
    };
    let two = || vec![shadow_of("a"), shadow_of("b")];
    let picks = |ids: &[&str]| ShadowMerge {
        shadows: ids.iter().map(|id| id.to_string()).collect(),
        ..Default::default()
    };
    assert!(with(ShadowMerge::default(), two()).is_ok(), "all shadows");
    assert!(with(picks(&["b"]), two()).is_ok(), "one shadow");
    assert!(with(picks(&["a", "b"]), two()).is_ok(), "several shadows");
    for (setting, shadows, reason) in [
        (ShadowMerge::default(), vec![], "at least one shadow"),
        (picks(&["ghost"]), two(), "does not carry"),
        (picks(&["a", "a"]), two(), "twice"),
        (
            ShadowMerge {
                timeout_secs: Some(0),
                ..Default::default()
            },
            two(),
            "seconds",
        ),
        (
            ShadowMerge {
                timeout_secs: Some(99_999),
                ..Default::default()
            },
            two(),
            "seconds",
        ),
        (
            ShadowMerge {
                agent: on(AgentType::Custom, "m"),
                ..Default::default()
            },
            two(),
            "connection",
        ),
    ] {
        let error = with(setting, shadows).expect_err(reason);
        assert!(error.contains(reason), "{error}");
    }
    // A merge parked with `enabled: false` needs no shadow.
    let mut parked = review_step();
    parked.shadow_merge = Some(ShadowMerge {
        enabled: false,
        ..Default::default()
    });
    assert!(shadow::validate(&[parked]).is_ok());
    // It rides on an Agent step that stays inside Kronn, like its shadows.
    let mut posts = review_step();
    posts.shadows = two();
    posts.shadow_merge = Some(ShadowMerge::default());
    posts.external_effect = true;
    assert!(shadow::validate(&[posts]).is_err());
}

#[tokio::test]
async fn the_merge_is_stored_with_the_shadows_and_kept_through_an_agents_write() {
    let (state, _, _) = test_state_and_configs();
    let mut wf = make_workflow_with_artifacts(Default::default());
    wf.id = "wf-merge-stored".into();
    let mut review = fake_step("review");
    review.shadows = vec![shadow_of("a")];
    review.shadow_merge = Some(ShadowMerge {
        shadows: vec!["a".into()],
        timeout_secs: Some(90),
        ..Default::default()
    });
    wf.steps = vec![review];
    let stored = wf.clone();
    state
        .db
        .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &stored))
        .await
        .unwrap();
    let read = state
        .db
        .with_read_conn(|conn| crate::db::workflows::get_workflow(conn, "wf-merge-stored"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        read.steps[0].shadow_merge.as_ref().map(|m| m.timeout_secs),
        Some(Some(90))
    );
    // Outside `steps_json`, which the trust checks compare.
    let steps_json: String = state
        .db
        .with_read_conn(|conn| {
            Ok(conn.query_row(
                "SELECT steps_json FROM workflows WHERE id = 'wf-merge-stored'",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(!steps_json.contains("shadow_merge"), "{steps_json}");

    // An agent that sends the steps back without it keeps what a person set.
    let mut sent = read.steps.clone();
    sent[0].shadows.clear();
    sent[0].shadow_merge = None;
    shadow::keep_stored_shadows(&mut sent, &read.steps);
    assert_eq!(sent[0].shadow_merge, read.steps[0].shadow_merge);
    let mut edited = read.clone();
    edited.steps[0].shadows.clear();
    edited.steps[0].shadow_merge = None;
    state
        .db
        .with_conn(move |conn| crate::db::workflows::update_workflow_as_agent(conn, &edited))
        .await
        .unwrap();
    let after = state
        .db
        .with_read_conn(|conn| crate::db::workflows::get_workflow(conn, "wf-merge-stored"))
        .await
        .unwrap()
        .unwrap();
    assert!(after.steps[0].shadow_merge.is_some(), "human-only");
}

// ─── KT-1694 — what the shadows and the merge leave to a later step ─────────

const SHADOWS_MARK: &str = "---SHADOWS---";

/// The step after the review reads the summary beside the review itself.
fn publishing(wf: &mut Workflow, reader: &str) {
    wf.steps[2].prompt_template =
        format!("Summary of: {{{{steps.review.output}}}}\n{SHADOWS_MARK}\n{{{{{reader}}}}}");
}

/// The JSON a later step received after the marker.
fn rendered_summary(fx: &Fixture) -> serde_json::Value {
    let prompt = summary_prompt(fx);
    let json = prompt
        .split_once(SHADOWS_MARK)
        .unwrap_or_else(|| panic!("the step did not receive the summary: {prompt}"))
        .1
        .trim_start();
    serde_json::Deserializer::from_str(json)
        .into_iter::<serde_json::Value>()
        .next()
        .unwrap_or_else(|| panic!("no JSON after the marker: {json}"))
        .unwrap_or_else(|error| panic!("not JSON ({error}): {json}"))
}

#[tokio::test]
async fn a_later_step_reads_the_executions_and_the_merge_of_a_merged_step() {
    let fx = merged_fixture("proj-shadows-summary", faithful_condense()).await;
    let mut wf = merged_workflow(&fx, Some(merge()));
    // An identical shadow beside the merged ones: it measures determinism, and
    // the merge (no shadow named) waits for it too.
    wf.steps[1].shadows.push(shadow_of("twin"));
    publishing(&mut wf, "steps.review.shadows");
    let run = fx.run(&wf, "run-shadows-summary").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);

    let summary = rendered_summary(&fx);
    assert_eq!(summary["step"], "review");
    let executions = summary["executions"].as_array().unwrap();
    let keys: Vec<&str> = executions
        .iter()
        .map(|execution| execution["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["main", "local", "other", "twin"], "{summary}");
    let by = |key: &str| executions.iter().find(|e| e["key"] == key).unwrap();
    assert_eq!(by("main")["agent"], "ClaudeCode");
    assert_eq!(by("main")["transport"], "cli");
    assert_eq!(by("main")["status"], "success");
    assert_eq!(by("main")["identical"], false);
    assert_eq!(by("local")["agent"], "Codex");
    assert_eq!(by("local")["identical"], false);
    assert_eq!(by("twin")["identical"], true);
    for execution in executions {
        assert_eq!(execution["status"], "success", "{execution}");
    }

    // The merge ran inside the step: its result is final when the next step renders.
    let merge = &summary["merge"];
    assert_eq!(merge["status"], "merged", "{summary}");
    assert_eq!(
        merge["included"],
        serde_json::json!(["local", "other", "twin"])
    );
    assert_eq!(merge["missing"], serde_json::json!([]));
    assert_eq!(
        merge["points"],
        serde_json::json!({"kept": 2, "unique": 1, "duplicate": 0, "dropped": 1})
    );
    assert_eq!(
        merge["per_execution"]["local"],
        serde_json::json!({"kept": 0, "duplicate": 1, "dropped": 1})
    );
    assert_eq!(
        merge["per_execution"]["other"],
        serde_json::json!({"kept": 1, "duplicate": 0, "dropped": 0})
    );

    // The twin answered the main review as the main execution did.
    assert_eq!(
        summary["determinism"],
        serde_json::json!({"identical_runs": 2, "distinct_outputs": 1})
    );

    // Outputs, prompts and the evidence of a point stay out of it.
    let text = summary.to_string();
    for secret in [
        MAIN_REVIEW,
        LOCAL_REVIEW,
        OTHER_REVIEW,
        MERGED_REVIEW,
        "login.rs",
        "tenants",
        "Review PR-42",
        "Second look",
    ] {
        assert!(!text.contains(secret), "`{secret}` leaked into {text}");
    }
}

#[tokio::test]
async fn a_rejected_merge_is_reported_as_such_to_a_later_step() {
    let fx = merged_fixture("proj-shadows-rejected", leaking_condense()).await;
    let mut wf = merged_workflow(&fx, Some(merge()));
    publishing(&mut wf, "steps.review.shadows.merge.status");
    let run = fx.run(&wf, "run-shadows-rejected").await;
    assert_eq!(result_of(&run, "review").output, MAIN_REVIEW);
    let prompt = summary_prompt(&fx);
    assert!(
        prompt
            .trim_end()
            .ends_with(&format!("{SHADOWS_MARK}\nrejected")),
        "{prompt}"
    );
}

#[tokio::test]
async fn a_step_without_a_merge_has_no_merge_block() {
    let fx = merged_fixture("proj-shadows-nomerge", faithful_condense()).await;
    let mut wf = merged_workflow(&fx, None);
    publishing(&mut wf, "steps.review.shadows");
    fx.run(&wf, "run-shadows-nomerge").await;
    let summary = rendered_summary(&fx);
    assert!(summary["merge"].is_null(), "{summary}");
    assert_eq!(summary["executions"][0]["key"], "main");
}

#[tokio::test]
async fn a_step_without_shadows_renders_an_empty_summary() {
    let fx = fixture("proj-shadows-none", scripted(MAIN_REVIEW)).await;
    let mut wf = fx.workflow(vec![]);
    publishing(&mut wf, "steps.review.shadows");
    let run = fx.run(&wf, "run-shadows-none").await;
    assert_eq!(run.status, RunStatus::Success, "{:?}", run.step_results);
    assert_eq!(
        rendered_summary(&fx),
        serde_json::json!({"step": "review", "executions": [], "merge": null, "determinism": null})
    );
}

#[tokio::test]
async fn a_resumed_run_gives_a_later_step_the_same_summary() {
    let fx = merged_fixture("proj-shadows-resume", faithful_condense()).await;
    let mut wf = merged_workflow(&fx, Some(merge()));
    publishing(&mut wf, "steps.review.shadows");
    let run = fx.run(&wf, "run-shadows-resume").await;
    let seen = rendered_summary(&fx);

    // What a Gate resume replays: the stored results, then the stored shadows.
    let mut ctx = TemplateContext::new();
    replay_prior_results(&mut ctx, &run.step_results);
    assert_eq!(
        ctx.resolve_value("steps.review.shadows.executions"),
        Some(serde_json::json!([])),
        "a replay alone knows no shadow"
    );
    crate::workflows::shadow_summary::restore(&fx.state, &mut ctx, &wf, &run).await;
    assert_eq!(ctx.resolve_value("steps.review.shadows").unwrap(), seen);
    assert_eq!(
        ctx.resolve_value("steps.who.shadows.executions").unwrap(),
        serde_json::json!([])
    );
}
