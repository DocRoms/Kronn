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

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::db::step_shadow_merges;
use crate::db::step_shadows::{self, StepShadowRow};
use crate::models::{RunStatus, ShadowMerge, StepShadow, Workflow, WorkflowRun, WorkflowStep};
use crate::AppState;

use super::shadow::{ShadowInput, ShadowStatus};
use super::steps::StepOutcome;
use super::template::TemplateContext;

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

fn poll_interval() -> Duration {
    if crate::core::test_mode::on() {
        Duration::from_millis(20)
    } else {
        Duration::from_millis(500)
    }
}

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
        tokio::time::sleep(poll_interval()).await;
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
{"points": [{"id": "p1", "summary": "one line", "text": "the point in the words of the execution that raised it", "found_by": ["main", "shadow:<id>"], "verdict": "kept", "verified": true, "evidence": "path:line", "reason": ""}], "agreement": ""}
KRONN_MERGE_PROVENANCE>>>

- `verdict` is `kept`, `duplicate` (with `duplicate_of`, the id of the kept point) or `dropped_unverified` (with `reason`).
- `found_by` uses only the execution names given above.
- Every `kept` point needs an `evidence` that can be resolved: a `path:line` that exists in your working directory (relative to it, the line inside the file), or a step-output reference (`steps.<step>.output`). A point of the `main` execution may also cite an execution name (`main`). An `evidence` that points to nothing, or prose alone, gets the merge refused.
- A `kept` point that no `main` execution raised needs `"verified": true` and an `evidence` of that kind. Never write `"verified": true` for an evidence you did not open.
- The list of points is never empty by omission. When no point is kept (the executions agree there is nothing to report, or every point was dropped), say so in `agreement`, in one sentence; otherwise leave it empty.
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
    /// Stated when no point is kept: the executions agree there is nothing to
    /// report, or every point was dropped. Without it, a list with no kept
    /// point is not a verdict.
    #[serde(default)]
    agreement: Option<String>,
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

/// Where a citation is looked up: the working tree the condense pass read and
/// the worktrees this run's own steps made, the step outputs of the run, and
/// the executions it was given.
struct EvidenceScope<'a> {
    /// The working directory first, then the run's own worktrees.
    roots: &'a [PathBuf],
    ctx: &'a TemplateContext,
    tokens: &'a [String],
}

/// What the resolvable citations of one evidence are.
#[derive(Default)]
struct Citations {
    /// A `path:line` that exists, or a step output that exists: something a
    /// reader can open.
    grounded: usize,
    /// An execution of the merge named as the source (`main`).
    executions: usize,
}

fn citation_words(evidence: &str) -> impl Iterator<Item = &str> {
    evidence
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';'))
        .map(|word| {
            word.trim_start_matches(['`', '"', '\'', '(', '[', '<', '{'])
                .trim_end_matches(['`', '"', '\'', ')', ']', '>', '}', '.', ':'])
        })
        .filter(|word| !word.is_empty())
}

/// `path:line`, `path:line-line` or `path:line:column` (and a `#symbol` tail).
fn file_citation(word: &str) -> Option<(&str, usize, Option<usize>)> {
    let word = word.split('#').next().unwrap_or(word);
    for (colon, _) in word.match_indices(':') {
        let (path, rest) = (&word[..colon], &word[colon + 1..]);
        if path.is_empty() {
            continue;
        }
        let (first, tail) = match rest.find(['-', ':']) {
            Some(at) => (&rest[..at], Some((rest.as_bytes()[at], &rest[at + 1..]))),
            None => (rest, None),
        };
        let Ok(line) = first.parse::<usize>() else {
            continue;
        };
        match tail {
            None => return Some((path, line, None)),
            Some((b'-', end)) => {
                if let Ok(end) = end.parse::<usize>() {
                    return Some((path, line, Some(end)));
                }
            }
            Some((_, column)) => {
                if column.parse::<usize>().is_ok() {
                    return Some((path, line, None));
                }
            }
        }
    }
    None
}

/// The biggest file a cited line is counted in.
const EVIDENCE_FILE_CAP_BYTES: u64 = crate::core::anti_halluc::LINE_COUNT_SIZE_CAP_BYTES;

/// How many worktrees of a run a citation is looked up in.
const MAX_RUN_WORKTREES: usize = 8;

/// The linked worktrees of the project's repository that an Exec step of this
/// run printed as its output (a `setup` step that checks a pull request out
/// beside the checkout, KT-1960). The repository's own registry decides
/// (`.git/worktrees/<id>/gitdir`), never the candidate's `.git` pointer: a
/// directory that only claims to be a worktree is not one, and a worktree no
/// step of the run named stays out, whatever branch it holds.
fn run_worktrees(
    steps: &[WorkflowStep],
    ctx: &TemplateContext,
    checkouts: &[&Path],
) -> Vec<PathBuf> {
    let registered: Vec<PathBuf> = checkouts
        .iter()
        .flat_map(|checkout| registered_worktrees(checkout))
        .collect();
    let mut found: Vec<PathBuf> = Vec::new();
    for step in steps {
        if !matches!(step.step_type, crate::models::StepType::Exec) {
            continue;
        }
        let Some(value) = ctx.resolve_value(&format!("steps.{}.output", step.name)) else {
            continue;
        };
        let Some(printed) = value.as_str().map(str::trim) else {
            continue;
        };
        if printed.contains('\n') || !Path::new(printed).is_absolute() {
            continue;
        }
        let Ok(real) = Path::new(printed).canonicalize() else {
            continue;
        };
        if registered.contains(&real) && !found.contains(&real) {
            found.push(real);
        }
        if found.len() == MAX_RUN_WORKTREES {
            break;
        }
    }
    found
}

/// The linked worktrees `checkout` registers, by their real path.
fn registered_worktrees(checkout: &Path) -> Vec<PathBuf> {
    let registry = checkout.join(".git").join("worktrees");
    let Ok(entries) = std::fs::read_dir(&registry) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let record = entry.path().join("gitdir");
            let small = std::fs::metadata(&record).is_ok_and(|meta| meta.len() <= 4096);
            let raw = small
                .then(|| std::fs::read_to_string(&record).ok())
                .flatten()?;
            let recorded = PathBuf::from(raw.trim());
            // `worktree.useRelativePaths` records it relative to the entry.
            let recorded = if recorded.is_relative() {
                entry.path().join(recorded)
            } else {
                recorded
            };
            (recorded.file_name()? == ".git").then_some(())?;
            recorded.parent()?.canonicalize().ok()
        })
        .collect()
}

/// A citation resolves when one of the roots holds the file with that line.
/// What is told about a path is what a reader of the tree already sees: a
/// path written to leave the tree is refused on its spelling alone, and one
/// that leaves it through a link is reported like a missing file.
fn check_file_line(
    roots: &[PathBuf],
    path: &str,
    line: usize,
    end: Option<usize>,
) -> Result<(), String> {
    // Refused before any lookup: whether a file outside exists is not told.
    let escapes = Path::new(path).components().any(|part| {
        !matches!(
            part,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    if escapes {
        return Err(format!("`{path}` is outside the working directory"));
    }
    let mut refusal = None;
    for root in roots {
        match check_file_line_in(root, path, line, end) {
            Ok(()) => return Ok(()),
            // The first tree that holds the file says why its line is refused.
            Err(Some(reason)) => refusal = refusal.or(Some(reason)),
            Err(None) => {}
        }
    }
    Err(refusal.unwrap_or_else(|| format!("`{path}` does not exist")))
}

/// `Err(None)`: this root does not hold the file.
fn check_file_line_in(
    root: &Path,
    path: &str,
    line: usize,
    end: Option<usize>,
) -> Result<(), Option<String>> {
    use crate::core::anti_halluc::{resolve_in_root, secret_line_refusal, Jailed};
    let rel = Path::new(path);
    let Jailed::Inside(file) = resolve_in_root(root, rel) else {
        return Err(None);
    };
    let last = end.unwrap_or(line);
    if secret_line_refusal(root, rel, &file, Some((line, last))).is_some() {
        return Err(Some(format!(
            "`{path}` may hold secrets: no line of it is an evidence"
        )));
    }
    let meta = std::fs::symlink_metadata(&file).map_err(|_| None)?;
    if !meta.is_file() {
        return Err(Some(format!("`{path}` is not a file")));
    }
    if meta.len() > EVIDENCE_FILE_CAP_BYTES {
        return Err(Some(format!("`{path}` is too large to count its lines")));
    }
    let unreadable = || Some(format!("`{path}` cannot be read"));
    let real_root = root.canonicalize().map_err(|_| unreadable())?;
    let inside = file.strip_prefix(&real_root).map_err(|_| unreadable())?;
    let bytes = crate::core::rooted_io::read_capped(&real_root, inside, EVIDENCE_FILE_CAP_BYTES)
        .map_err(|_| unreadable())?;
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count();
    let lines = newlines + usize::from(bytes.last().is_some_and(|byte| *byte != b'\n'));
    if line == 0 || last < line || last > lines {
        return Err(Some(format!(
            "`{path}:{line}` is not inside the file ({lines} lines)"
        )));
    }
    Ok(())
}

/// Resolves what an evidence cites. Every `path:line` and every step-output
/// reference it carries must exist; prose around them is not checked, and prose
/// alone resolves to nothing.
fn resolve_evidence(evidence: &str, scope: &EvidenceScope) -> Result<Citations, String> {
    let mut found = Citations::default();
    for word in citation_words(evidence) {
        if scope.tokens.iter().any(|token| token == word) {
            found.executions += 1;
        } else if word.starts_with("steps.") || word.starts_with("previous_step.") {
            let present = scope.ctx.resolve_value(word).is_some_and(|value| {
                !value.is_null() && value.as_str().is_none_or(|text| !text.trim().is_empty())
            });
            if !present {
                return Err(format!("the step output `{word}` does not exist"));
            }
            found.grounded += 1;
        } else if let Some((path, line, end)) = file_citation(word) {
            check_file_line(scope.roots, path, line, end)?;
            found.grounded += 1;
        }
    }
    if found.grounded + found.executions == 0 {
        return Err(
            "it cites no `path:line` and no step output (prose alone proves nothing)".to_string(),
        );
    }
    Ok(found)
}

/// The checked answer: the points, the result and the agreement stated when no
/// point is kept; or the points read and why the answer is refused.
type Checked = Result<(Vec<MergePoint>, String, Option<String>), (Vec<MergePoint>, String)>;

/// What the condense pass answered, held to its contract: the provenance names
/// only the executions it was given; every kept point cites an evidence that
/// resolves (a `path:line` that exists, a step output that exists); a point
/// only a shadow raised, and any point marked verified, rests on such an
/// evidence found in the tree or the run, never on an execution's own word; a
/// verdict that keeps no point states why (`agreement`); and no dropped point
/// appears in the result. A breach returns the points read (for the report)
/// and the reason. On success, the agreement stated when no point is kept.
/// The main execution's own output and metrics stay readable beside the merge:
/// its comparison row keeps them, the step's result then carries the pass's
/// spend and, when published, the merged output.
async fn keep_main_row(state: &AppState, run_id: &str, index: usize, outcome: &StepOutcome) {
    let (tokens, cost, model) = super::shadow::spent(&outcome.result);
    let main_row = rows_of(state, run_id, index)
        .await
        .into_iter()
        .find(|row| row.shadow_id == step_shadows::MAIN);
    let Some(mut row) = main_row else { return };
    row.status = ShadowStatus::Success;
    row.output = outcome.result.output.clone();
    row.tokens_used = Some(tokens);
    row.cost_usd = cost;
    row.model = model;
    row.duration_ms = Some(outcome.result.duration_ms);
    row.finished_at = Some(now());
    let _ = state
        .db
        .with_conn(move |conn| step_shadows::finish(conn, &row))
        .await;
}

/// Why a point of the main execution lost its verified mark.
const UNPROVEN_MARK: &str = "marked verified on the word of an execution alone: kept, not verified";

fn check_answer(output: &str, tokens: &[String], scope: &EvidenceScope) -> Checked {
    let (provenance, merged) = match split_answer(output) {
        Ok(parts) => parts,
        Err(reason) => return Err((Vec::new(), reason)),
    };
    let agreement = provenance
        .agreement
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
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
    for index in 0..points.len() {
        let point = &points[index];
        // What is published is traced to the executions that raised it.
        if point.verdict == PointVerdict::Kept && point.found_by.is_empty() {
            return Err((
                points.clone(),
                format!("point `{}` names no execution that found it", point.id),
            ));
        }
        let evidence = point.evidence.as_deref().unwrap_or("").trim();
        // Verified is a claim about the evidence: it holds only if it opens.
        let resolved = if evidence.is_empty() {
            Err("it gives no evidence".to_string())
        } else {
            resolve_evidence(evidence, scope)
        };
        let by_main = point.found_by.iter().any(|token| token == MAIN_TOKEN);
        let mut unproven = false;
        if point.verified {
            match &resolved {
                Ok(found) if found.grounded > 0 => {}
                // The main execution raised it: the point stood before the
                // merge and stands after it, without the mark it did not earn.
                Ok(_) if by_main => unproven = true,
                Ok(_) => {
                    return Err((
                        points.clone(),
                        format!(
                            "point `{}` is marked verified but its evidence cites only an execution, not the code or a step output",
                            point.id
                        ),
                    ))
                }
                Err(why) => {
                    return Err((
                        points.clone(),
                        format!(
                            "point `{}` is marked verified but its evidence does not resolve: {why}",
                            point.id
                        ),
                    ))
                }
            }
        }
        match point.verdict {
            PointVerdict::Kept => {
                if !by_main && (!point.verified || evidence.is_empty()) {
                    return Err((
                        points.clone(),
                        format!(
                            "point `{}` comes only from a shadow and is kept without a verification",
                            point.id
                        ),
                    ));
                }
                if let Err(why) = resolved {
                    return Err((
                        points.clone(),
                        format!(
                            "point `{}` is kept on an evidence that does not resolve: {why}",
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
        if unproven {
            let point = &mut points[index];
            point.verified = false;
            if point
                .reason
                .as_deref()
                .is_none_or(|reason| reason.trim().is_empty())
            {
                point.reason = Some(UNPROVEN_MARK.to_string());
            }
        }
    }
    let kept = points
        .iter()
        .any(|point| point.verdict == PointVerdict::Kept);
    if kept {
        return Ok((points, merged, None));
    }
    // Nothing is kept: a result with no point behind it is a claim, unless the
    // condense pass states that the executions agree there is nothing to report.
    match agreement {
        Some(statement) => Ok((points, merged, Some(statement))),
        None if points.is_empty() => Err((
            points,
            "the condense pass listed no point and did not state that the executions agree"
                .to_string(),
        )),
        None => Err((
            points,
            "the condense pass kept no point and did not state that the executions agree"
                .to_string(),
        )),
    }
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
    let (logged_run, logged_step) = (run_id.clone(), step_name.clone());
    let saved = state
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
    // A merge nobody can read back looks like a merge that never ran.
    if let Err(error) = saved {
        tracing::error!(
            run_id = %logged_run,
            step = %logged_step,
            "the merge report could not be saved: {error:#}"
        );
    }
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
        tracing::info!(
            run_id = %run.id,
            step = %step.name,
            status = ?outcome.result.status,
            "no merge: the main execution did not succeed"
        );
        return;
    }
    let started_at = now();
    let started = Instant::now();
    tracing::info!(run_id = %run.id, step = %step.name, "merging the step with its shadows");
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
        keep_main_row(state, &run.id, index, outcome).await;
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

    let mut roots = vec![PathBuf::from(&input.work_dir)];
    roots.extend(run_worktrees(
        &workflow.steps,
        &input.ctx,
        &[Path::new(&input.project_path), Path::new(&input.work_dir)],
    ));
    let scope = EvidenceScope {
        roots: &roots,
        ctx: &input.ctx,
        tokens: &tokens,
    };
    match check_answer(&condensed.result.output, &tokens, &scope) {
        Err((points, reason)) => {
            report.contributions = contributions(&tokens, &points);
            report.points = points;
            report.status = MergeStatus::Rejected;
            report.note = Some(format!(
                "The merged result was not published ({reason}): the main execution's output stands"
            ));
            keep_main_row(state, &run.id, index, outcome).await;
            outcome.result.tokens_used =
                Some(outcome.result.tokens_used.unwrap_or(0) + spent_tokens);
        }
        Ok((points, merged, agreement)) => {
            keep_main_row(state, &run.id, index, outcome).await;
            report.contributions = contributions(&tokens, &points);
            report.points = points;
            report.status = MergeStatus::Merged;
            report.merged_output = merged.clone();
            report.note = agreement.map(|statement| {
                format!("No point is kept; the condense pass states that the executions agree: {statement}")
            });
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

    /// A working tree with `src/a.rs` (20 lines) and a run that has a step `fetch`.
    struct Env {
        dir: tempfile::TempDir,
        ctx: TemplateContext,
        tokens: Vec<String>,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "line\n".repeat(20)).unwrap();
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("fetch", "the fetched rows");
        Env {
            dir,
            ctx,
            tokens: tokens(),
        }
    }

    impl Env {
        fn check(&self, output: &str) -> Checked {
            let roots = [self.dir.path().to_path_buf()];
            let scope = EvidenceScope {
                roots: &roots,
                ctx: &self.ctx,
                tokens: &self.tokens,
            };
            check_answer(output, &self.tokens, &scope)
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let done = std::process::Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(done.status.success(), "git {args:?}: {done:?}");
    }

    fn step(name: &str, kind: &str) -> WorkflowStep {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "step_type": { "type": kind },
            "agent": "ClaudeCode",
            "prompt_template": "",
            "mode": { "type": "Normal" },
        }))
        .unwrap()
    }

    /// A checkout with two linked worktrees beside it, each holding `only.rs`
    /// (5 lines) that the checkout does not have.
    struct Repo {
        _dir: tempfile::TempDir,
        checkout: PathBuf,
        named: PathBuf,
        unrelated: PathBuf,
    }

    fn repo() -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        git(&checkout, &["init", "-q"]);
        git(&checkout, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let (named, unrelated) = (dir.path().join("pr-1"), dir.path().join("lane"));
        for tree in [&named, &unrelated] {
            git(
                &checkout,
                &["worktree", "add", "-q", "--detach", tree.to_str().unwrap()],
            );
            std::fs::write(tree.join("only.rs"), "line\n".repeat(5)).unwrap();
        }
        Repo {
            _dir: dir,
            checkout,
            named,
            unrelated,
        }
    }

    impl Repo {
        fn roots(&self, steps: &[WorkflowStep], ctx: &TemplateContext) -> Vec<PathBuf> {
            let mut roots = vec![self.checkout.clone()];
            roots.extend(run_worktrees(steps, ctx, &[&self.checkout]));
            roots
        }
    }

    #[test]
    fn a_citation_resolves_in_a_worktree_an_exec_step_of_the_run_named() {
        let repo = repo();
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("setup", &format!("{}\n", repo.named.display()));
        let steps = [step("setup", "Exec")];
        let roots = repo.roots(&steps, &ctx);
        assert_eq!(roots.len(), 2, "{roots:?}");
        assert_eq!(check_file_line(&roots, "only.rs", 5, None), Ok(()));
        let past = check_file_line(&roots, "only.rs", 6, None).unwrap_err();
        assert!(past.contains("not inside the file (5 lines)"), "{past}");
    }

    #[test]
    fn a_worktree_no_exec_step_of_the_run_named_holds_no_evidence() {
        let repo = repo();
        let stray = repo.checkout.parent().unwrap().join("stray");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("only.rs"), "line\n".repeat(5)).unwrap();
        // A folder that points at a real worktree's git directory is not one.
        std::fs::copy(repo.named.join(".git"), stray.join(".git")).unwrap();
        let cases: [(&str, &str, String); 4] = [
            ("nothing named", "Exec", "done".into()),
            // The agent's own word is not the run's setup.
            (
                "named by an agent",
                "Agent",
                repo.named.display().to_string(),
            ),
            ("not a worktree", "Exec", stray.display().to_string()),
            (
                "two lines",
                "Exec",
                format!("{}\n{}", repo.named.display(), repo.unrelated.display()),
            ),
        ];
        for (case, kind, output) in cases {
            let mut ctx = TemplateContext::new();
            ctx.set_step_output("setup", &output);
            let roots = repo.roots(&[step("setup", kind)], &ctx);
            assert_eq!(roots, vec![repo.checkout.clone()], "{case}");
            let refused = check_file_line(&roots, "only.rs", 1, None).unwrap_err();
            assert!(refused.contains("does not exist"), "{case}: {refused}");
        }
        // Naming one worktree opens that one, not its neighbour.
        std::fs::write(repo.unrelated.join("lane.rs"), "line\n").unwrap();
        let mut ctx = TemplateContext::new();
        ctx.set_step_output("setup", &repo.named.display().to_string());
        let roots = repo.roots(&[step("setup", "Exec")], &ctx);
        assert!(check_file_line(&roots, "lane.rs", 1, None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_citation_tells_nothing_about_what_lies_outside_the_tree_or_holds_secrets() {
        let env = env();
        let root = env.dir.path();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("present"), "a\nb\n").unwrap();
        for (link, target) in [("to-present", "present"), ("to-absent", "absent")] {
            std::os::unix::fs::symlink(outside.path().join(target), root.join(link)).unwrap();
        }
        std::fs::write(root.join(".env"), "KEY=value\n").unwrap();
        std::os::unix::fs::symlink(root.join(".env"), root.join("settings")).unwrap();
        let big = std::fs::File::create(root.join("big.txt")).unwrap();
        big.set_len(EVIDENCE_FILE_CAP_BYTES + 1).unwrap();
        let roots = [root.to_path_buf()];
        let refused = |path: &str| check_file_line(&roots, path, 1, None).unwrap_err();
        // A link out of the tree answers the same whether its target exists.
        assert_eq!(refused("to-present"), "`to-present` does not exist");
        assert_eq!(refused("to-absent"), "`to-absent` does not exist");
        for secret in [".env", "settings"] {
            let reason = refused(secret);
            assert!(reason.contains("may hold secrets"), "{reason}");
            assert!(!reason.contains("lines"), "{reason}");
        }
        assert!(refused("big.txt").contains("too large"));
    }

    const GOOD_POINT: &str = r#"[{"id":"p1","summary":"s","found_by":["main"],"verdict":"kept","verified":true,"evidence":"src/a.rs:10"}]"#;

    #[test]
    fn the_provenance_block_is_split_from_the_result() {
        let text = answer(GOOD_POINT, "## Review\nOne finding.");
        let (points, merged, agreement) = env().check(&text).expect("a valid answer");
        assert_eq!(points.len(), 1);
        assert_eq!(merged, "## Review\nOne finding.");
        assert!(!merged.contains("KRONN_MERGE"));
        assert!(agreement.is_none());
    }

    #[test]
    fn an_answer_without_provenance_is_refused() {
        let (points, reason) = env().check("## Review only").unwrap_err();
        assert!(points.is_empty());
        assert!(reason.contains("no provenance block"), "{reason}");
    }

    #[test]
    fn a_point_only_a_shadow_raised_needs_its_verification() {
        let unverified = answer(
            r#"[{"id":"p1","summary":"s","found_by":["shadow:qwen"],"verdict":"kept","verified":false}]"#,
            "Invented bug.",
        );
        let (_, reason) = env().check(&unverified).unwrap_err();
        assert!(reason.contains("without a verification"), "{reason}");
        let verified = answer(
            r#"[{"id":"p1","summary":"s","found_by":["shadow:qwen"],"verdict":"kept","verified":true,"evidence":"src/a.rs:10"}]"#,
            "Real bug.",
        );
        assert!(env().check(&verified).is_ok());
    }

    #[test]
    fn a_dropped_point_never_appears_in_the_result() {
        let leaked = answer(
            r#"[{"id":"p1","summary":"s","text":"The cache key ignores the tenant","found_by":["shadow:qwen"],"verdict":"dropped_unverified","reason":"no such code"}]"#,
            "- the cache key   ignores the TENANT\n",
        );
        let (points, reason) = env().check(&leaked).unwrap_err();
        assert_eq!(points.len(), 1);
        assert!(reason.contains("appears in the result"), "{reason}");
    }

    #[test]
    fn unknown_executions_are_ignored_and_a_point_left_with_none_is_refused() {
        let text = answer(
            r#"[{"id":"p1","summary":"s","found_by":["shadow:ghost"],"verdict":"kept","verified":true,"evidence":"src/a.rs:1"}]"#,
            "Result.",
        );
        let (_, reason) = env().check(&text).unwrap_err();
        assert!(reason.contains("names no execution"), "{reason}");
    }

    // ─── KT-1731 — a verdict that proves nothing is not a merge ──────────────

    #[test]
    fn an_empty_list_of_points_is_refused_unless_the_agreement_is_stated() {
        let env = env();
        for text in [
            answer("[]", "Everything is fine."),
            format!("{PROVENANCE_BEGIN}\n{{}}\n{PROVENANCE_END}\nEverything is fine."),
            format!(
                "{PROVENANCE_BEGIN}\n{{\"points\":[],\"agreement\":\"  \"}}\n{PROVENANCE_END}\nEverything is fine."
            ),
        ] {
            let (points, reason) = env.check(&text).unwrap_err();
            assert!(points.is_empty());
            assert!(reason.contains("listed no point"), "{reason}");
        }
        let stated = format!(
            "{PROVENANCE_BEGIN}\n{{\"points\":[],\"agreement\":\"All three executions report nothing to fix\"}}\n{PROVENANCE_END}\nNothing to fix."
        );
        let (points, merged, agreement) = env.check(&stated).expect("a stated agreement");
        assert!(points.is_empty());
        assert_eq!(merged, "Nothing to fix.");
        assert_eq!(
            agreement.as_deref(),
            Some("All three executions report nothing to fix")
        );
    }

    #[test]
    fn a_result_with_no_kept_point_needs_the_agreement_too() {
        let dropped = r#"[{"id":"p1","summary":"s","text":"The cache key ignores the tenant","found_by":["shadow:qwen"],"verdict":"dropped_unverified","reason":"no such code"}]"#;
        let (points, reason) = env()
            .check(&answer(dropped, "Whatever the pass claims."))
            .unwrap_err();
        assert_eq!(points.len(), 1, "the points read stay in the report");
        assert!(reason.contains("kept no point"), "{reason}");
        let stated = format!(
            "{PROVENANCE_BEGIN}\n{{\"points\":{dropped},\"agreement\":\"The only shadow point was invented; main reports nothing\"}}\n{PROVENANCE_END}\nNothing to fix."
        );
        assert!(env().check(&stated).is_ok());
    }

    #[test]
    fn a_kept_point_without_a_resolvable_evidence_is_refused() {
        let env = env();
        for (evidence, why) in [
            (r#","evidence":"""#, "gives no evidence"),
            ("", "gives no evidence"),
            (r#","evidence":"I read the code""#, "prose alone"),
            (r#","evidence":"nonexistent.rs:999999""#, "does not exist"),
            (
                r#","evidence":"src/a.rs:999999""#,
                "not inside the file (20 lines)",
            ),
            (r#","evidence":"src/a.rs:0""#, "not inside the file"),
            (r#","evidence":"src/a.rs:15-30""#, "not inside the file"),
            (r#","evidence":"src/a.rs:12-3""#, "not inside the file"),
            (
                r#","evidence":"../../etc/hosts:1""#,
                "outside the working directory",
            ),
            (
                r#","evidence":"/etc/hosts:1""#,
                "outside the working directory",
            ),
            (r#","evidence":"src:1""#, "not a file"),
            (r#","evidence":"steps.ghost.output""#, "does not exist"),
            (
                r#","evidence":"src/a.rs:10 and nonexistent.rs:1""#,
                "does not exist",
            ),
        ] {
            let points = format!(
                r#"[{{"id":"p1","summary":"s","found_by":["main"],"verdict":"kept"{evidence}}}]"#
            );
            let (read, reason) = env.check(&answer(&points, "Result.")).unwrap_err();
            assert_eq!(read.len(), 1, "{evidence}");
            assert!(reason.contains(why), "{evidence}: {reason}");
        }
    }

    #[test]
    fn an_evidence_that_resolves_is_accepted_in_each_of_its_forms() {
        let env = env();
        for (found_by, evidence) in [
            ("main", "src/a.rs:20"),
            ("main", "src/a.rs:3-9 (the loop)"),
            ("main", "`src/a.rs:4:7`, then src/a.rs:5."),
            ("main", "src/a.rs:4#some_symbol"),
            ("main", "steps.fetch.output"),
            ("shadow:qwen", "steps.fetch.output and src/a.rs:1"),
        ] {
            let points = format!(
                r#"[{{"id":"p1","summary":"s","found_by":["{found_by}"],"verdict":"kept","verified":true,"evidence":"{evidence}"}}]"#
            );
            assert!(
                env.check(&answer(&points, "Result.")).is_ok(),
                "{found_by} / {evidence}"
            );
        }
        // A point of the main execution may cite the execution itself, unverified.
        let own = answer(
            r#"[{"id":"p1","summary":"s","found_by":["main"],"verdict":"kept","verified":false,"evidence":"main"}]"#,
            "Result.",
        );
        assert!(env.check(&own).is_ok());
    }

    #[test]
    fn verified_is_never_true_on_an_unresolved_citation() {
        let env = env();
        // Whatever the verdict, a verified mark holds only if its evidence opens.
        for verdict in ["kept", "duplicate", "dropped_unverified"] {
            let points = format!(
                r#"[{{"id":"p1","summary":"s","found_by":["main"],"verdict":"{verdict}","verified":true,"evidence":"nonexistent.rs:999999"}},{{"id":"p2","summary":"s","found_by":["main"],"verdict":"kept","verified":true,"evidence":"src/a.rs:1"}}]"#
            );
            let (read, reason) = env.check(&answer(&points, "Result.")).unwrap_err();
            assert_eq!(read.len(), 2);
            assert!(
                reason.contains("marked verified") && reason.contains("does not resolve"),
                "{verdict}: {reason}"
            );
        }
        // An execution's own word does not verify anything.
        let circular = answer(
            r#"[{"id":"p1","summary":"s","found_by":["shadow:qwen"],"verdict":"kept","verified":true,"evidence":"shadow:qwen"}]"#,
            "Result.",
        );
        let (_, reason) = env.check(&circular).unwrap_err();
        assert!(reason.contains("cites only an execution"), "{reason}");
    }

    #[test]
    fn a_point_of_the_main_execution_loses_an_unproven_mark_and_stays() {
        let env = env();
        // The main execution raised it and the pass vouched for it on the
        // execution's word: the merge is published, the mark is not.
        for found_by in [r#"["main"]"#, r#"["main","shadow:qwen"]"#] {
            let points = format!(
                r#"[{{"id":"p1","summary":"s","found_by":{found_by},"verdict":"kept","verified":true,"evidence":"main"}},{{"id":"p2","summary":"s","found_by":["main"],"verdict":"kept","verified":true,"evidence":"src/a.rs:1"}}]"#
            );
            let (read, merged, _) = env.check(&answer(&points, "Result.")).unwrap();
            assert_eq!(merged, "Result.");
            assert!(!read[0].verified, "{found_by}");
            assert_eq!(read[0].verdict, PointVerdict::Kept);
            assert_eq!(read[0].reason.as_deref(), Some(UNPROVEN_MARK));
            assert!(read[1].verified, "an opened citation keeps its mark");
            assert_eq!(read[1].reason, None);
        }
        // The reason the pass gave is not overwritten.
        let reasoned = answer(
            r#"[{"id":"p1","summary":"s","found_by":["main"],"verdict":"kept","verified":true,"evidence":"main","reason":"stated by main"}]"#,
            "Result.",
        );
        let (read, _, _) = env.check(&reasoned).unwrap();
        assert!(!read[0].verified);
        assert_eq!(read[0].reason.as_deref(), Some("stated by main"));
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
