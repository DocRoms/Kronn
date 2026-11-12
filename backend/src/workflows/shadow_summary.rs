//! What a step's shadows and merge leave to the steps after it (KT-1694):
//! `{{steps.<name>.shadows}}`, a read-only JSON summary a PublishPageData step
//! can write into a Page dataset so a Live Page shows how the step was run.
//!
//! The summary says who ran, with what, how it ended and what the merge kept.
//! It carries no output, no prompt and no setting that could hold a secret: a
//! shadow's output never feeds a later step (KT-1459), only the main or merged
//! output continues. A step without shadows has the same summary with no
//! execution, so a template never needs to know whether the step carried some.

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use crate::db::step_shadows::{self, StepShadowRow};
use crate::models::{AgentType, RunStatus, StepResult, Workflow, WorkflowRun, WorkflowStep};
use crate::AppState;

use super::shadow::ShadowStatus;
use super::shadow_merge::{MergePoint, MergeStatus, PointVerdict, ShadowMergeReport, MAIN_TOKEN};
use super::template::TemplateContext;

/// How much of a reason the summary keeps: a reason is engine-written text.
const MAX_REASON_CHARS: usize = 300;

/// The summary of a step that ran no shadow.
pub fn none(step_name: &str) -> Value {
    json!({ "step": step_name, "executions": [], "merge": null, "determinism": null })
}

fn status_name(status: ShadowStatus) -> &'static str {
    match status {
        ShadowStatus::Running => "running",
        ShadowStatus::Success => "success",
        ShadowStatus::Failed => "failed",
        ShadowStatus::Skipped => "skipped",
        ShadowStatus::OverBudget => "over_budget",
        ShadowStatus::TimedOut => "timed_out",
        ShadowStatus::Interrupted => "interrupted",
    }
}

/// How the agent is reached: a CLI process, a local model, or a remote endpoint.
fn transport(agent: &str) -> &'static str {
    match serde_json::from_value::<AgentType>(Value::String(agent.to_string())) {
        Ok(AgentType::Ollama) => "local",
        Ok(agent) if crate::agents::runner::is_http_chat_agent(&agent) => "http",
        _ => "cli",
    }
}

/// One execution of the step, as the summary shows it.
struct Execution {
    key: String,
    label: String,
    identical: bool,
    row_status: ShadowStatus,
    agent: String,
    model: Option<String>,
    tokens: Option<u64>,
    cost_usd: Option<f64>,
    duration_ms: Option<u64>,
    output: String,
}

impl Execution {
    fn json(&self) -> Value {
        json!({
            "key": self.key,
            "label": self.label,
            "identical": self.identical,
            "agent": self.agent,
            "model": self.model,
            "transport": transport(&self.agent),
            "status": status_name(self.row_status),
            "tokens": self.tokens,
            "cost_usd": self.cost_usd,
            "duration_ms": self.duration_ms,
        })
    }

    /// Whether the execution produced an output worth comparing.
    fn has_output(&self) -> bool {
        matches!(
            self.row_status,
            ShadowStatus::Success | ShadowStatus::OverBudget
        ) && !self.output.trim().is_empty()
    }
}

/// The main execution. Once a merge has run, its row keeps the execution's own
/// output and metrics (the step's result then holds the merged output);
/// otherwise they are the step result's.
fn main_execution(row: &StepShadowRow, result: &StepResult) -> Execution {
    let identity_model = row.identity.model.clone();
    if row.finished_at.is_some() {
        return Execution {
            key: MAIN_TOKEN.to_string(),
            label: MAIN_TOKEN.to_string(),
            identical: false,
            row_status: row.status,
            agent: row.identity.agent.clone(),
            model: row.model.clone().or(identity_model),
            tokens: row.tokens_used,
            cost_usd: row.cost_usd,
            duration_ms: row.duration_ms,
            output: row.output.clone(),
        };
    }
    let (tokens, cost, model) = super::shadow::spent(result);
    Execution {
        key: MAIN_TOKEN.to_string(),
        label: MAIN_TOKEN.to_string(),
        identical: false,
        row_status: match result.status {
            RunStatus::Success | RunStatus::Partial => ShadowStatus::Success,
            RunStatus::Running | RunStatus::Pending => ShadowStatus::Running,
            _ => ShadowStatus::Failed,
        },
        agent: row.identity.agent.clone(),
        model: model.or(identity_model),
        tokens: Some(tokens),
        cost_usd: cost,
        duration_ms: Some(result.duration_ms),
        output: result.output.clone(),
    }
}

fn shadow_execution(step: &WorkflowStep, row: &StepShadowRow) -> Execution {
    let label = if row.label.trim().is_empty() {
        row.shadow_id.clone()
    } else {
        row.label.clone()
    };
    Execution {
        key: row.shadow_id.clone(),
        label,
        identical: step
            .shadows
            .iter()
            .find(|shadow| shadow.id == row.shadow_id)
            .is_some_and(|shadow| shadow.is_identical()),
        row_status: row.status,
        agent: row.identity.agent.clone(),
        model: row.model.clone().or_else(|| row.identity.model.clone()),
        tokens: row.tokens_used,
        cost_usd: row.cost_usd,
        duration_ms: row.duration_ms,
        output: row.output.clone(),
    }
}

// ─── Determinism ────────────────────────────────────────────────────────────

/// Measurements that differ on every run: an output is not « different » for them.
const VOLATILE_KEYS: [&str; 7] = [
    "duration_ms",
    "elapsed_ms",
    "started_at",
    "finished_at",
    "published_at",
    "observed_at",
    "data_revision",
];

fn timings() -> &'static regex_lite::Regex {
    static RE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r"\b\d+(?:\.\d+)?\s?(?:ms|s)\b").expect("static regex")
    });
    &RE
}

fn without_measurements(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(without_measurements).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(key, _)| !VOLATILE_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key, without_measurements(value)))
                .collect(),
        ),
        Value::String(text) => Value::String(timings().replace_all(&text, "…").into_owned()),
        other => other,
    }
}

/// The output as two runs are compared: the signal and the envelope dropped,
/// timings and timestamps left out. Mirrors the run view's own comparison.
fn comparable_output(output: &str) -> String {
    let body = match output.trim_end().rfind("[SIGNAL: ") {
        Some(at)
            if output[at..]
                .trim_end()
                .trim_end_matches(']')
                .chars()
                .skip("[SIGNAL: ".len())
                .all(|c| c.is_ascii_alphanumeric() || c == '_') =>
        {
            output[..at].trim()
        }
        _ => output.trim(),
    };
    let candidate = match (
        body.find("---STEP_OUTPUT---"),
        body.find("---END_STEP_OUTPUT---"),
    ) {
        (Some(open), Some(close)) if close > open => {
            body[open + "---STEP_OUTPUT---".len()..close].trim()
        }
        _ => body,
    };
    match serde_json::from_str::<Value>(candidate) {
        Ok(parsed) => without_measurements(parsed).to_string(),
        Err(_) => timings().replace_all(body, "…").into_owned(),
    }
}

/// For the shadows identical to the step: how many executions produced an
/// output and how many distinct outputs they gave. `None` without such a shadow.
fn determinism(main: &Execution, shadows: &[Execution]) -> Option<Value> {
    let twins: Vec<&Execution> = shadows.iter().filter(|shadow| shadow.identical).collect();
    if twins.is_empty() {
        return None;
    }
    let compared: Vec<&Execution> = std::iter::once(main)
        .chain(twins)
        .filter(|execution| execution.has_output())
        .collect();
    let distinct: HashSet<String> = compared
        .iter()
        .map(|execution| comparable_output(&execution.output))
        .collect();
    Some(json!({ "identical_runs": compared.len(), "distinct_outputs": distinct.len() }))
}

// ─── The merge ──────────────────────────────────────────────────────────────

fn merge_status(status: MergeStatus) -> &'static str {
    match status {
        MergeStatus::Waiting => "waiting",
        MergeStatus::Condensing => "condensing",
        MergeStatus::Merged => "merged",
        MergeStatus::Skipped => "skipped",
        MergeStatus::Rejected => "rejected",
        MergeStatus::Failed => "failed",
    }
}

/// The key of an execution as the summary names it: `main` or the shadow id.
fn key_of(token: &str) -> &str {
    token.strip_prefix("shadow:").unwrap_or(token)
}

fn counted(points: &[MergePoint], verdict: PointVerdict) -> usize {
    points.iter().filter(|p| p.verdict == verdict).count()
}

fn merge_summary(report: &ShadowMergeReport) -> Value {
    let unique = report
        .points
        .iter()
        .filter(|p| p.verdict == PointVerdict::Kept && p.found_by.len() == 1)
        .count();
    let mut per_execution = Map::new();
    for contribution in &report.contributions {
        per_execution.insert(
            key_of(&contribution.execution).to_string(),
            json!({
                "kept": contribution.kept,
                "duplicate": contribution.duplicate,
                "dropped": contribution.dropped_unverified,
            }),
        );
    }
    json!({
        "status": merge_status(report.status),
        "included": report.contributors,
        "missing": report.missing.iter().map(|missing| json!({
            "key": missing.shadow_id,
            "reason": missing.reason.chars().take(MAX_REASON_CHARS).collect::<String>(),
        })).collect::<Vec<_>>(),
        "points": {
            "kept": counted(&report.points, PointVerdict::Kept),
            "unique": unique,
            "duplicate": counted(&report.points, PointVerdict::Duplicate),
            "dropped": counted(&report.points, PointVerdict::DroppedUnverified),
        },
        "per_execution": per_execution,
    })
}

// ─── The summary ────────────────────────────────────────────────────────────

/// The summary of `step` from the rows its run stored for it, its merge report
/// and its result so far. A step without a recorded main execution has none.
pub fn summarize(
    step: &WorkflowStep,
    rows: &[StepShadowRow],
    merge: Option<&ShadowMergeReport>,
    result: &StepResult,
) -> Value {
    let Some(main_row) = rows.iter().find(|row| row.shadow_id == step_shadows::MAIN) else {
        return none(&step.name);
    };
    let main = main_execution(main_row, result);
    let shadows: Vec<Execution> = rows
        .iter()
        .filter(|row| row.shadow_id != step_shadows::MAIN)
        .map(|row| shadow_execution(step, row))
        .collect();
    let executions: Vec<Value> = std::iter::once(&main)
        .chain(shadows.iter())
        .map(Execution::json)
        .collect();
    json!({
        "step": step.name,
        "executions": executions,
        "merge": merge.map(merge_summary),
        "determinism": determinism(&main, &shadows),
    })
}

/// Reads the rows and the merge a run stored for the step at `index`.
async fn stored(
    state: &AppState,
    run_id: &str,
    index: usize,
) -> (Vec<StepShadowRow>, Option<ShadowMergeReport>) {
    let run_id = run_id.to_string();
    state
        .db
        .with_read_conn(move |conn| {
            let rows = step_shadows::list_for_run(conn, &run_id)?
                .into_iter()
                .filter(|row| row.result_index as usize == index)
                .collect();
            let merge = crate::db::step_shadow_merges::list_for_run(conn, &run_id)?
                .into_iter()
                .find(|merged| merged.result_index as usize == index)
                .map(|merged| merged.report);
            Ok((rows, merge))
        })
        .await
        .unwrap_or_default()
}

/// The summary of the step that has just completed at result `index`: after
/// its merge, which runs inside the step's own future, so it is final when the
/// next step renders. A shadow still going is reported `running`.
pub async fn for_completed_step(
    state: &AppState,
    run_id: &str,
    index: usize,
    step: &WorkflowStep,
    result: &StepResult,
) -> Value {
    if step.shadows.is_empty() {
        return none(&step.name);
    }
    let (rows, merge) = stored(state, run_id, index).await;
    summarize(step, &rows, merge.as_ref(), result)
}

/// Gives a resumed run's context the summaries of the steps it already ran.
pub async fn restore(
    state: &AppState,
    ctx: &mut TemplateContext,
    workflow: &Workflow,
    run: &WorkflowRun,
) {
    for (index, prior) in run.step_results.iter().enumerate() {
        let Some(step) = workflow
            .steps
            .iter()
            .find(|step| step.name == prior.step_name && !step.shadows.is_empty())
        else {
            continue;
        };
        let summary = for_completed_step(state, &run.id, index, step, prior).await;
        ctx.set_step_shadows(&prior.step_name, &summary.to_string());
    }
}

#[cfg(test)]
mod tests;
