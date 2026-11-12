//! The optional merge of a step's main execution with its shadows (KT-1544).
//!
//! A step with shadows only compares them (KT-1459). When it carries a
//! `shadow_merge`, the runner calls [`merge_step`] once the main execution has
//! succeeded: the merge waits for the selected shadows (up to a timeout, and goes
//! on with those that finished), then a condense pass (an Agent, usually the
//! strongest model) dedups their points, VERIFIES each against the code,
//! calibrates severity and records which execution found what. The merged
//! result replaces the step's output for the rest of the workflow; the main and
//! shadow outputs stay stored for comparison.
//!
//! The merge never hurts the step: whatever goes wrong (no shadow finished, the
//! condense pass fails, its provenance breaks the contract) the main execution's
//! own output stays the step's output, and the report says why. A point the
//! condense pass could not verify is never published.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::db::step_shadow_merges;
use crate::db::step_shadows::{self, StepShadowRow};
use crate::models::{RunStatus, ShadowMerge, StepShadow, Workflow, WorkflowRun, WorkflowStep};
use crate::AppState;

use super::shadow::{ShadowInput, ShadowStatus};
use super::steps::StepOutcome;

/// Opens the provenance block the condense pass writes before its result.
pub const PROVENANCE_BEGIN: &str = "<<<KRONN_MERGE_PROVENANCE";
/// Closes it.
pub const PROVENANCE_END: &str = "KRONN_MERGE_PROVENANCE>>>";

/// How an execution is named in the condense pass and its provenance: the main
/// execution is `main`, a shadow `shadow:<id>`.
pub const MAIN_TOKEN: &str = "main";

/// What the condense pass reads of the step's task: the rendered prompt, cut.
const MAX_TASK_BYTES: usize = 16_000;
/// What it reads of one execution's output.
const MAX_EXECUTION_BYTES: usize = 60_000;

#[cfg(not(test))]
const POLL_INTERVAL: Duration = Duration::from_millis(500);
#[cfg(test)]
const POLL_INTERVAL: Duration = Duration::from_millis(20);

pub fn shadow_token(shadow_id: &str) -> String {
    format!("shadow:{shadow_id}")
}

// ─── The report ─────────────────────────────────────────────────────────────

/// How a merge stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MergeStatus {
    /// Waiting for the selected shadows.
    #[default]
    Waiting,
    /// The condense pass is running.
    Condensing,
    /// The merged result is the step's output.
    Merged,
    /// Nothing to merge: no selected shadow finished. The main output stays.
    Skipped,
    /// The condense pass broke its contract; the main output stays.
    Rejected,
    /// The condense pass failed; the main output stays.
    Failed,
}

/// What the condense pass decided about one point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PointVerdict {
    /// Verified (or the main execution's own) and published.
    Kept,
    /// The same point as another, listed apart.
    Duplicate,
    /// Not confirmed against the code: never published.
    DroppedUnverified,
}

/// One point of an execution's output (a finding, a claim) and what became of
/// it: which executions found it, whether it was verified, why it was dropped.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct MergePoint {
    pub id: String,
    pub summary: String,
    /// The point as the execution worded it: a dropped point's text never
    /// appears in the merged result.
    #[serde(default)]
    #[ts(optional)]
    pub text: Option<String>,
    /// `main` and `shadow:<id>`.
    #[serde(default)]
    pub found_by: Vec<String>,
    pub verdict: PointVerdict,
    #[serde(default)]
    pub verified: bool,
    /// What confirmed it: the file and line, the output read.
    #[serde(default)]
    #[ts(optional)]
    pub evidence: Option<String>,
    /// Why it was dropped or calibrated.
    #[serde(default)]
    #[ts(optional)]
    pub reason: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub duplicate_of: Option<String>,
}

/// A selected shadow that did not contribute, and why.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct MissingShadow {
    pub shadow_id: String,
    pub label: String,
    pub reason: String,
}

/// What one execution brought to the merged result.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExecutionContribution {
    /// `main` or `shadow:<id>`.
    pub execution: String,
    /// Points it found that the merged result keeps and that no execution
    /// before it in the list found.
    pub kept: u32,
    /// Of those kept, the points only this execution found.
    pub unique: u32,
    /// Points it found that another execution already brought.
    pub duplicate: u32,
    /// Points it found that could not be verified and were not published.
    pub dropped_unverified: u32,
}

/// The merge of one step, as the run view shows it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ShadowMergeReport {
    pub status: MergeStatus,
    /// The shadows the merge selected, by id.
    #[serde(default)]
    pub selected: Vec<String>,
    /// The shadows whose output went into the condense pass.
    #[serde(default)]
    pub contributors: Vec<String>,
    /// The selected shadows that did not make it, with the reason.
    #[serde(default)]
    pub missing: Vec<MissingShadow>,
    #[serde(default)]
    pub timeout_secs: u64,
    #[serde(default)]
    pub points: Vec<MergePoint>,
    #[serde(default)]
    pub contributions: Vec<ExecutionContribution>,
    /// Why the main output stayed, or anything the reader should know.
    #[serde(default)]
    #[ts(optional)]
    pub note: Option<String>,
    /// The condense pass's own spend: hosted tokens the merge cost.
    #[serde(default)]
    #[ts(optional)]
    pub tokens_used: Option<u64>,
    #[serde(default)]
    #[ts(optional)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    #[ts(optional)]
    pub model: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub duration_ms: Option<u64>,
    /// The merged result: the step's output when `status` is `merged`.
    #[serde(default)]
    pub merged_output: String,
}

// ─── Waiting for the shadows ────────────────────────────────────────────────

/// What the merge found of the selected shadows once it stopped waiting.
struct Gathered {
    contributors: Vec<StepShadowRow>,
    missing: Vec<MissingShadow>,
}

fn selected_shadows<'a>(step: &'a WorkflowStep, merge: &ShadowMerge) -> Vec<&'a StepShadow> {
    step.shadows
        .iter()
        .filter(|shadow| merge.selects(&shadow.id))
        .collect()
}

async fn rows_of(state: &AppState, run_id: &str, index: usize) -> Vec<StepShadowRow> {
    let run_id = run_id.to_string();
    state
        .db
        .with_read_conn(move |conn| step_shadows::list_for_run(conn, &run_id))
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.result_index as usize == index)
        .collect()
}

fn why_missing(row: &StepShadowRow, timeout_secs: u64) -> String {
    let stopped = row.stopped.clone().unwrap_or_default();
    match row.status {
        ShadowStatus::Running => {
            format!("still running when the merge stopped waiting ({timeout_secs} s)")
        }
        ShadowStatus::Failed if stopped.is_empty() => "failed".to_string(),
        ShadowStatus::Failed => format!("failed: {stopped}"),
        ShadowStatus::Skipped => format!("not run: {stopped}"),
        ShadowStatus::TimedOut => format!("stopped by its time budget: {stopped}"),
        ShadowStatus::Interrupted => format!("interrupted: {stopped}"),
        ShadowStatus::Success | ShadowStatus::OverBudget => {
            "finished with an empty output".to_string()
        }
    }
}

/// Waits until every selected shadow of the step has settled, or `timeout`
/// passes, and sorts them into those whose output goes on and those missing.
/// A shadow that never started (disabled, outside its projects, not sampled)
/// has no row: it is reported missing at once, never waited for.
async fn gather(
    state: &AppState,
    run_id: &str,
    index: usize,
    step: &WorkflowStep,
    merge: &ShadowMerge,
) -> Gathered {
    let timeout = merge.timeout();
    let selected = selected_shadows(step, merge);
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let rows = loop {
        let rows: Vec<StepShadowRow> = rows_of(state, run_id, index)
            .await
            .into_iter()
            .filter(|row| row.shadow_id != step_shadows::MAIN && merge.selects(&row.shadow_id))
            .collect();
        let settled = rows.iter().all(|row| row.status != ShadowStatus::Running);
        if settled || Instant::now() >= deadline {
            break rows;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    let mut contributors = Vec::new();
    let mut missing = Vec::new();
    for shadow in selected {
        match rows.iter().find(|row| row.shadow_id == shadow.id) {
            None => missing.push(MissingShadow {
                shadow_id: shadow.id.clone(),
                label: shadow.display_label().to_string(),
                reason: if shadow.enabled {
                    "did not run in this run (outside its projects or not sampled)".to_string()
                } else {
                    "disabled".to_string()
                },
            }),
            Some(row)
                if matches!(row.status, ShadowStatus::Success | ShadowStatus::OverBudget)
                    && !row.output.trim().is_empty() =>
            {
                contributors.push(row.clone());
            }
            Some(row) => missing.push(MissingShadow {
                shadow_id: row.shadow_id.clone(),
                label: if row.label.is_empty() {
                    row.shadow_id.clone()
                } else {
                    row.label.clone()
                },
                reason: why_missing(row, timeout),
            }),
        }
    }
    Gathered {
        contributors,
        missing,
    }
}

// ─── The condense pass ──────────────────────────────────────────────────────

const CONDENSE_TEMPLATE: &str = r#"You are the CONDENSE pass of the step "{{shadow_merge.step}}". Several executions answered the same task on the same input. Produce ONE result that replaces all of them.

THE TASK EVERY EXECUTION ANSWERED
{{shadow_merge.task}}

THE EXECUTIONS ({{shadow_merge.ids}})
{{shadow_merge.executions}}

HOW TO CONDENSE
1. Split each execution's output into distinct points (a finding, a claim, a recommendation).
2. Group the points that say the same thing: the group is one point, and `found_by` lists every execution that raised it.
3. VERIFY every point that an execution other than `main` raised against the code or data in your working directory before you keep it. A shadow, a local model especially, may invent a point: one you cannot confirm is DROPPED and never appears in your result. Do not take a point on trust because several executions agree.
4. Calibrate severity yourself, from what you verified, not from what the executions claimed.
5. Write the result in exactly the format the task asked for. It must contain every kept point once and no dropped point.

PROVENANCE
Before the result, write exactly one block, in this form, listing EVERY point of every execution:

<<<KRONN_MERGE_PROVENANCE
{"points": [{"id": "p1", "summary": "one line", "text": "the point in the words of the execution that raised it", "found_by": ["main", "shadow:<id>"], "verdict": "kept", "verified": true, "evidence": "path:line or what you read", "reason": ""}]}
KRONN_MERGE_PROVENANCE>>>

- `verdict` is `kept`, `duplicate` (with `duplicate_of`, the id of the kept point) or `dropped_unverified` (with `reason`).
- `found_by` uses only the execution names given above.
- A `kept` point that no `main` execution raised needs `"verified": true` and a non-empty `evidence`.
- `text` of a dropped point must not appear in your result.
Then write the result, and nothing else after it."#;

fn cut_to(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[cut at {end} bytes]", &text[..end])
}

/// Keeps an execution's output from reading as a workflow placeholder when the
/// condense prompt is rendered or checked for unresolved references.
fn inert(text: &str) -> String {
    text.replace("{{", "{ {")
}

fn executions_block(main_output: &str, contributors: &[StepShadowRow]) -> String {
    let mut block = format!(
        "=== EXECUTION {MAIN_TOKEN} (the main execution) ===\n{}\n=== END {MAIN_TOKEN} ===\n",
        inert(&cut_to(main_output, MAX_EXECUTION_BYTES))
    );
    for row in contributors {
        let token = shadow_token(&row.shadow_id);
        let model = row
            .model
            .clone()
            .or_else(|| row.identity.model.clone())
            .unwrap_or_default();
        block.push_str(&format!(
            "\n=== EXECUTION {token} (shadow \"{}\", agent {} {model}) ===\n{}\n=== END {token} ===\n",
            row.label,
            row.identity.agent,
            inert(&cut_to(&row.output, MAX_EXECUTION_BYTES))
        ));
    }
    block
}

/// The step the condense pass runs: the step's own contract (output format,
/// signals, skills) on its own agent, or the one the merge names, with the
/// condense prompt in place of the step's.
fn condense_step(step: &WorkflowStep, merge: &ShadowMerge) -> WorkflowStep {
    let mut condense = step.clone();
    condense.name = format!("{} (merge)", step.name);
    condense.shadows = Vec::new();
    condense.shadow_merge = None;
    condense.prompt_template = CONDENSE_TEMPLATE.to_string();
    condense.quick_prompt_id = None;
    condense.quick_prompt_version = None;
    condense.multi_agent_review = None;
    if let Some(choice) = merge.agent.as_ref() {
        super::shadow::apply_shadow_agent(&mut condense, choice);
    }
    condense
}

/// What a point must satisfy for the merged result to be published.
#[derive(Debug, Deserialize)]
struct Provenance {
    #[serde(default)]
    points: Vec<MergePoint>,
}

fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Splits the condense pass's answer into its provenance block and its result.
fn split_answer(output: &str) -> Result<(Provenance, String), String> {
    let begin = output
        .find(PROVENANCE_BEGIN)
        .ok_or("the condense pass wrote no provenance block")?;
    let after = begin + PROVENANCE_BEGIN.len();
    let end = output[after..]
        .find(PROVENANCE_END)
        .map(|offset| after + offset)
        .ok_or("the provenance block is not closed")?;
    let raw = output[after..end]
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let provenance: Provenance = serde_json::from_str(raw)
        .map_err(|error| format!("the provenance block is not valid: {error}"))?;
    let merged = format!(
        "{}{}",
        &output[..begin],
        &output[end + PROVENANCE_END.len()..]
    )
    .trim()
    .to_string();
    Ok((provenance, merged))
}

/// The per-execution counts of a verified list of points: the first execution
/// of a kept point's `found_by` brings it, the others only restate it.
fn contributions(tokens: &[String], points: &[MergePoint]) -> Vec<ExecutionContribution> {
    tokens
        .iter()
        .map(|token| {
            let mut counts = ExecutionContribution {
                execution: token.clone(),
                ..Default::default()
            };
            for point in points.iter().filter(|p| p.found_by.contains(token)) {
                match point.verdict {
                    PointVerdict::Kept if point.found_by.first() == Some(token) => {
                        counts.kept += 1;
                        if point.found_by.len() == 1 {
                            counts.unique += 1;
                        }
                    }
                    PointVerdict::Kept | PointVerdict::Duplicate => counts.duplicate += 1,
                    PointVerdict::DroppedUnverified => counts.dropped_unverified += 1,
                }
            }
            counts
        })
        .collect()
}

/// What the condense pass answered, held to its contract: the provenance names
/// only the executions it was given, a point that only a shadow raised is kept
/// only with its verification, and no dropped point appears in the result.
/// A breach returns the points read (for the report) and the reason.
fn check_answer(
    output: &str,
    tokens: &[String],
) -> Result<(Vec<MergePoint>, String), (Vec<MergePoint>, String)> {
    let (provenance, merged) = match split_answer(output) {
        Ok(parts) => parts,
        Err(reason) => return Err((Vec::new(), reason)),
    };
    let mut points = provenance.points;
    for point in &mut points {
        point.found_by.retain(|token| tokens.contains(token));
        point.found_by.dedup();
        // `main` first: it is the execution that already stood.
        point
            .found_by
            .sort_by_key(|token| tokens.iter().position(|known| known == token));
    }
    if merged.is_empty() {
        return Err((points, "the condense pass wrote no result".to_string()));
    }
    let merged_text = normalized(&merged);
    for point in &points {
        match point.verdict {
            PointVerdict::Kept => {
                // What is published is traced to the executions that raised it.
                if point.found_by.is_empty() {
                    return Err((
                        points.clone(),
                        format!("point `{}` names no execution that found it", point.id),
                    ));
                }
                let by_main = point.found_by.iter().any(|token| token == MAIN_TOKEN);
                let evidence = point.evidence.as_deref().unwrap_or("").trim();
                if !by_main && (!point.verified || evidence.is_empty()) {
                    return Err((
                        points.clone(),
                        format!(
                            "point `{}` comes only from a shadow and is kept without a verification",
                            point.id
                        ),
                    ));
                }
            }
            PointVerdict::DroppedUnverified => {
                let text = normalized(point.text.as_deref().unwrap_or(""));
                if text.len() >= 8 && merged_text.contains(&text) {
                    return Err((
                        points.clone(),
                        format!(
                            "point `{}` was dropped as unverified yet appears in the result",
                            point.id
                        ),
                    ));
                }
            }
            PointVerdict::Duplicate => {}
        }
    }
    Ok((points, merged))
}

// ─── Running the merge ──────────────────────────────────────────────────────

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

async fn save(
    state: &AppState,
    run_id: &str,
    index: usize,
    step_name: &str,
    report: &ShadowMergeReport,
    started_at: &str,
    finished_at: Option<String>,
) {
    let (run_id, step_name) = (run_id.to_string(), step_name.to_string());
    let (report, started_at) = (report.clone(), started_at.to_string());
    let _ = state
        .db
        .with_conn(move |conn| {
            step_shadow_merges::upsert(
                conn,
                &run_id,
                index as u32,
                &step_name,
                &report,
                &started_at,
                finished_at.as_deref(),
            )
        })
        .await;
}

/// Merges the step's main execution with the shadows its `shadow_merge`
/// selects, when the main execution succeeded. On a merge the result of the
/// condense pass replaces the step's output (and the decision its signals
/// made); otherwise `outcome` is left exactly as the main execution produced it.
/// `step` is the step as it ran, `input` what the main step received.
pub async fn merge_step(
    state: &AppState,
    workflow: &Workflow,
    run: &WorkflowRun,
    step: &WorkflowStep,
    index: usize,
    input: &ShadowInput,
    outcome: &mut StepOutcome,
) {
    let Some(merge) = step.shadow_merge.as_ref().filter(|merge| merge.enabled) else {
        return;
    };
    if !matches!(outcome.result.status, RunStatus::Success) {
        return;
    }
    let started_at = now();
    let started = Instant::now();
    let mut report = ShadowMergeReport {
        status: MergeStatus::Waiting,
        selected: selected_shadows(step, merge)
            .iter()
            .map(|shadow| shadow.id.clone())
            .collect(),
        timeout_secs: merge.timeout(),
        ..Default::default()
    };
    save(
        state,
        &run.id,
        index,
        &step.name,
        &report,
        &started_at,
        None,
    )
    .await;

    let gathered = gather(state, &run.id, index, step, merge).await;
    report.contributors = gathered
        .contributors
        .iter()
        .map(|row| row.shadow_id.clone())
        .collect();
    report.missing = gathered.missing;
    if gathered.contributors.is_empty() {
        report.status = MergeStatus::Skipped;
        report.note = Some(
            "No selected shadow finished with an output: the main execution's output stands"
                .to_string(),
        );
        report.duration_ms = Some(started.elapsed().as_millis() as u64);
        save(
            state,
            &run.id,
            index,
            &step.name,
            &report,
            &started_at,
            Some(now()),
        )
        .await;
        return;
    }

    report.status = MergeStatus::Condensing;
    save(
        state,
        &run.id,
        index,
        &step.name,
        &report,
        &started_at,
        None,
    )
    .await;

    let main_output = outcome.result.output.clone();
    let mut tokens = vec![MAIN_TOKEN.to_string()];
    tokens.extend(
        gathered
            .contributors
            .iter()
            .map(|row| shadow_token(&row.shadow_id)),
    );

    // The task the executions answered: what the main step was asked, cut.
    let mut ctx = input.ctx.clone();
    let task = ctx
        .render(&step.prompt_template)
        .map(|prompt| cut_to(&prompt, MAX_TASK_BYTES))
        .unwrap_or_else(|_| "(the task prompt could not be rendered)".to_string());
    ctx.set("shadow_merge.step", step.name.clone());
    ctx.set("shadow_merge.task", inert(&task));
    ctx.set("shadow_merge.ids", tokens.join(", "));
    ctx.set(
        "shadow_merge.executions",
        executions_block(&main_output, &gathered.contributors),
    );

    let condense = condense_step(step, merge);
    let limit = workflow
        .guards
        .as_ref()
        .and_then(|guards| guards.timeout_seconds)
        .unwrap_or(crate::models::DEFAULT_GUARD_TIMEOUT_SECS);
    let full_access = input.agents.full_access_for(&condense.agent);
    let answered = tokio::time::timeout(
        Duration::from_secs(limit),
        super::steps::execute_step(
            &condense,
            &input.project_path,
            input.project_id.as_deref(),
            &input.work_dir,
            &input.tokens,
            full_access,
            &ctx,
            None,
            None,
            Some(&input.agents.model_tiers),
            Some(&crate::models::setup::HttpEndpoints::from_agents(
                &input.agents,
            )),
            Some(&input.ollama_context_overrides),
            // Like a shadow: no room and no Kronn tool, it reads the working tree.
            None,
            Some(&state.db),
            None,
            Some(&run.id),
        ),
    )
    .await;
    report.duration_ms = Some(started.elapsed().as_millis() as u64);

    let condensed = match answered {
        Err(_) => {
            report.status = MergeStatus::Failed;
            report.note = Some(format!(
                "The condense pass was stopped after {limit} s: the main execution's output stands"
            ));
            save(
                state,
                &run.id,
                index,
                &step.name,
                &report,
                &started_at,
                Some(now()),
            )
            .await;
            return;
        }
        Ok(condensed) => condensed,
    };
    let (spent_tokens, cost, model) = super::shadow::spent(&condensed.result);
    report.tokens_used = Some(spent_tokens);
    report.cost_usd = cost;
    report.model = model;
    if !matches!(condensed.result.status, RunStatus::Success) {
        report.status = MergeStatus::Failed;
        report.note = Some(format!(
            "The condense pass failed ({}): the main execution's output stands",
            cut_to(condensed.result.output.trim(), 300)
        ));
        outcome.result.tokens_used = Some(outcome.result.tokens_used.unwrap_or(0) + spent_tokens);
        save(
            state,
            &run.id,
            index,
            &step.name,
            &report,
            &started_at,
            Some(now()),
        )
        .await;
        return;
    }

    match check_answer(&condensed.result.output, &tokens) {
        Err((points, reason)) => {
            report.contributions = contributions(&tokens, &points);
            report.points = points;
            report.status = MergeStatus::Rejected;
            report.note = Some(format!(
                "The merged result was not published ({reason}): the main execution's output stands"
            ));
            outcome.result.tokens_used =
                Some(outcome.result.tokens_used.unwrap_or(0) + spent_tokens);
        }
        Ok((points, merged)) => {
            // The main execution's own output and metrics stay readable beside
            // the merged result: its comparison row keeps them.
            let (main_tokens, main_cost, main_model) = super::shadow::spent(&outcome.result);
            let main_duration = outcome.result.duration_ms;
            let kept = outcome.result.output.clone();
            let main_row = rows_of(state, &run.id, index)
                .await
                .into_iter()
                .find(|row| row.shadow_id == step_shadows::MAIN);
            if let Some(mut row) = main_row {
                row.status = ShadowStatus::Success;
                row.output = kept;
                row.tokens_used = Some(main_tokens);
                row.cost_usd = main_cost;
                row.model = main_model;
                row.duration_ms = Some(main_duration);
                row.finished_at = Some(now());
                let _ = state
                    .db
                    .with_conn(move |conn| step_shadows::finish(conn, &row))
                    .await;
            }
            report.contributions = contributions(&tokens, &points);
            report.points = points;
            report.status = MergeStatus::Merged;
            report.merged_output = merged.clone();
            // The merged result is what the rest of the workflow reads, and the
            // signals of the step are read from it.
            outcome.result.output = merged;
            outcome.result.envelope_detected = condensed.result.envelope_detected;
            outcome.condition_action = condensed.condition_action;
            outcome.result.tokens_used =
                Some(outcome.result.tokens_used.unwrap_or(0) + spent_tokens);
        }
    }
    save(
        state,
        &run.id,
        index,
        &step.name,
        &report,
        &started_at,
        Some(now()),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens() -> Vec<String> {
        vec![
            MAIN_TOKEN.to_string(),
            shadow_token("qwen"),
            shadow_token("other"),
        ]
    }

    fn answer(points: &str, result: &str) -> String {
        format!("{PROVENANCE_BEGIN}\n{{\"points\": {points}}}\n{PROVENANCE_END}\n{result}")
    }

    #[test]
    fn the_provenance_block_is_split_from_the_result() {
        let text = answer(
            r#"[{"id":"p1","summary":"s","found_by":["main"],"verdict":"kept"}]"#,
            "## Review\nOne finding.",
        );
        let (points, merged) = check_answer(&text, &tokens()).expect("a valid answer");
        assert_eq!(points.len(), 1);
        assert_eq!(merged, "## Review\nOne finding.");
        assert!(!merged.contains("KRONN_MERGE"));
    }

    #[test]
    fn an_answer_without_provenance_is_refused() {
        let (points, reason) = check_answer("## Review only", &tokens()).unwrap_err();
        assert!(points.is_empty());
        assert!(reason.contains("no provenance block"), "{reason}");
    }

    #[test]
    fn a_point_only_a_shadow_raised_needs_its_verification() {
        let unverified = answer(
            r#"[{"id":"p1","summary":"s","found_by":["shadow:qwen"],"verdict":"kept","verified":false}]"#,
            "Invented bug.",
        );
        let (_, reason) = check_answer(&unverified, &tokens()).unwrap_err();
        assert!(reason.contains("without a verification"), "{reason}");
        let verified = answer(
            r#"[{"id":"p1","summary":"s","found_by":["shadow:qwen"],"verdict":"kept","verified":true,"evidence":"src/a.rs:10"}]"#,
            "Real bug.",
        );
        assert!(check_answer(&verified, &tokens()).is_ok());
    }

    #[test]
    fn a_dropped_point_never_appears_in_the_result() {
        let leaked = answer(
            r#"[{"id":"p1","summary":"s","text":"The cache key ignores the tenant","found_by":["shadow:qwen"],"verdict":"dropped_unverified","reason":"no such code"}]"#,
            "- the cache key   ignores the TENANT\n",
        );
        let (points, reason) = check_answer(&leaked, &tokens()).unwrap_err();
        assert_eq!(points.len(), 1);
        assert!(reason.contains("appears in the result"), "{reason}");
    }

    #[test]
    fn unknown_executions_are_ignored_and_a_point_left_with_none_is_refused() {
        let text = answer(
            r#"[{"id":"p1","summary":"s","found_by":["shadow:ghost"],"verdict":"kept","verified":true,"evidence":"x"}]"#,
            "Result.",
        );
        let (_, reason) = check_answer(&text, &tokens()).unwrap_err();
        assert!(reason.contains("names no execution"), "{reason}");
    }

    #[test]
    fn contributions_credit_the_first_finder_and_count_the_restatements() {
        let points = vec![
            MergePoint {
                id: "p1".into(),
                summary: String::new(),
                text: None,
                found_by: vec![MAIN_TOKEN.into(), shadow_token("qwen")],
                verdict: PointVerdict::Kept,
                verified: true,
                evidence: None,
                reason: None,
                duplicate_of: None,
            },
            MergePoint {
                id: "p2".into(),
                summary: String::new(),
                text: None,
                found_by: vec![shadow_token("qwen")],
                verdict: PointVerdict::Kept,
                verified: true,
                evidence: Some("a:1".into()),
                reason: None,
                duplicate_of: None,
            },
            MergePoint {
                id: "p3".into(),
                summary: String::new(),
                text: None,
                found_by: vec![shadow_token("other")],
                verdict: PointVerdict::DroppedUnverified,
                verified: false,
                evidence: None,
                reason: Some("invented".into()),
                duplicate_of: None,
            },
        ];
        let counts = contributions(&tokens(), &points);
        assert_eq!((counts[0].kept, counts[0].unique), (1, 0));
        assert_eq!(
            (counts[1].kept, counts[1].unique, counts[1].duplicate),
            (1, 1, 1)
        );
        assert_eq!(counts[2].dropped_unverified, 1);
    }

    #[test]
    fn an_execution_cannot_pass_for_a_placeholder() {
        assert_eq!(inert("a {{steps.x.data}} b"), "a { {steps.x.data}} b");
    }
}
