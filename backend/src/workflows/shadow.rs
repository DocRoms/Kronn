//! Step shadows (KT-1449, KT-1459): a step re-runs beside its main execution,
//! once per shadow, on exactly the input the main step receives (same rendered
//! context, same previous outputs and variables), with what the shadow changes:
//! the agent, model or connection, the prompt, the skills. A shadow that changes
//! nothing measures how deterministic the step is.
//!
//! Only the main output continues the workflow. A shadow never runs a step that
//! acts outside Kronn, and what happens in it (an error, a stall, a budget
//! spent) settles that shadow's row and nothing else: the main step neither
//! waits for it nor fails with it.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::db::step_shadows::{self, StepShadowRow};
use crate::models::{
    RunStatus, ShadowAgent, ShadowPrompt, StepResult, StepShadow, StepType, Workflow, WorkflowRun,
    WorkflowStep,
};
use crate::AppState;

use super::shadow_artifact::ExecutionHtml;
use super::steps::StepOutcome;
use super::template::TemplateContext;

pub const MAX_SHADOWS_PER_STEP: usize = 8;
const MAX_SAMPLE_EVERY: u32 = 1000;
/// What a shadow keeps of an output: a longer one is cut, and says so with
/// [`CUT_MARKER`].
const MAX_STORED_OUTPUT_BYTES: usize = 1_000_000;
/// The start of the line that ends an output a shadow kept cut.
pub const CUT_MARKER: &str = "[output cut at ";

// A run recorded by the workflow-level shadow (KT-1449) carries its variant in
// its trigger context. No new run takes it, but such a run stays a trial: it
// never acts outside Kronn when it is resumed, and it never notifies.
pub const VARIANT_KEY: &str = "__variant__";
pub const SHADOW_OF_KEY: &str = "__shadow_of__";
pub const NORMAL: &str = "normal";

fn recorded_variant(run: &WorkflowRun) -> Option<&str> {
    run.trigger_context
        .as_ref()
        .and_then(|t| t.get(VARIANT_KEY))
        .and_then(|v| v.as_str())
        .filter(|id| *id != NORMAL)
}

/// Whether the run is a run of the retired workflow-level shadow.
pub fn is_shadow(run: &WorkflowRun) -> bool {
    recorded_variant(run).is_some()
}

/// `{{run.variant}}`: `normal` for every run a user launches; the variant a
/// run of the retired workflow-level shadow recorded.
pub fn variant(run: &WorkflowRun) -> String {
    recorded_variant(run).unwrap_or(NORMAL).to_string()
}

/// The variant of run `parent_run_id` when it is a run of the retired
/// workflow-level shadow: its sub-runs are of that variant too, so they skip
/// their external effects as well.
pub async fn parent_variant(state: &AppState, parent_run_id: &str) -> Option<String> {
    let id = parent_run_id.to_string();
    state
        .db
        .with_read_conn(move |conn| crate::db::workflows::get_run(conn, &id))
        .await
        .ok()
        .flatten()
        .and_then(|run| recorded_variant(&run).map(str::to_string))
}

fn step_matches(step: &WorkflowStep, key: &str) -> bool {
    step.id.as_deref() == Some(key) || step.name == key
}

// ─── What an execution loaded ───────────────────────────────────────────────

/// What one execution of a step loaded: what the comparison header sets side
/// by side to say what differs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExecutionIdentity {
    pub agent: String,
    #[serde(default)]
    #[ts(optional)]
    pub model: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub connection_id: Option<String>,
    /// `qp-id@N` for a Quick Prompt version, `inline:<hash>` for a prompt
    /// written in the step or the shadow.
    pub prompt: String,
    /// The skills the execution loads, versions pinned as `custom-x@3`.
    #[serde(default)]
    pub skills: Vec<String>,
}

/// How a stored execution stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ShadowStatus {
    Running,
    Success,
    Failed,
    /// Not run: the step acts outside Kronn.
    Skipped,
    /// It finished having spent more than its budget.
    OverBudget,
    /// Its time budget ended it.
    TimedOut,
    /// Kronn stopped while it ran.
    Interrupted,
}

fn short_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.trim().as_bytes());
    digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

fn agent_name(agent: &crate::models::AgentType) -> String {
    serde_json::to_value(agent)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{agent:?}"))
}

/// The latest saved version of a Quick Prompt, 0 when it has none.
fn current_version(conn: &rusqlite::Connection, qp_id: &str) -> anyhow::Result<u32> {
    let latest: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version_index), 0) FROM quick_prompt_versions WHERE quick_prompt_id = ?1",
        rusqlite::params![qp_id],
        |row| row.get(0),
    )?;
    Ok(latest.max(0) as u32)
}

/// The skills a step loads: its own, else those of its Quick Prompt's version.
async fn effective_skills(state: &AppState, step: &WorkflowStep) -> Vec<String> {
    if !step.skill_ids.is_empty() {
        return step.skill_ids.clone();
    }
    let Some(qp_id) = step.quick_prompt_id.clone() else {
        return Vec::new();
    };
    let pinned = step.quick_prompt_version;
    state
        .db
        .with_read_conn(move |conn| {
            Ok(match pinned {
                Some(index) => {
                    crate::db::quick_prompts::get_quick_prompt_version(conn, &qp_id, index)?
                        .map(|version| version.skill_ids)
                }
                None => {
                    crate::db::quick_prompts::get_quick_prompt(conn, &qp_id)?.map(|qp| qp.skill_ids)
                }
            }
            .unwrap_or_default())
        })
        .await
        .unwrap_or_default()
}

/// The prompt a step loads, as the comparison names it.
async fn prompt_identity(state: &AppState, step: &WorkflowStep) -> String {
    if !step.prompt_template.trim().is_empty() {
        return format!("inline:{}", short_hash(&step.prompt_template));
    }
    let Some(qp_id) = step.quick_prompt_id.clone() else {
        return "inline:".to_string();
    };
    let pinned = step.quick_prompt_version;
    let lookup = qp_id.clone();
    let version = match pinned {
        Some(index) => index,
        None => state
            .db
            .with_read_conn(move |conn| current_version(conn, &lookup))
            .await
            .unwrap_or(0),
    };
    if version == 0 {
        qp_id
    } else {
        format!("{qp_id}@{version}")
    }
}

/// What `step` loads, read before its Quick Prompt is hydrated.
async fn identity_of_step(state: &AppState, step: &WorkflowStep) -> ExecutionIdentity {
    let settings = step.agent_settings.as_ref();
    ExecutionIdentity {
        agent: agent_name(&step.agent),
        model: settings
            .and_then(|s| s.model.clone())
            .or_else(|| settings.and_then(|s| s.tier.as_ref().map(|t| format!("tier:{t:?}")))),
        connection_id: settings.and_then(|s| s.connection_id.clone()),
        prompt: prompt_identity(state, step).await,
        skills: effective_skills(state, step).await,
    }
}

// ─── What a shadow changes ──────────────────────────────────────────────────

/// Runs the step on the shadow's agent: its agent, model, effort and
/// connection change; tier and the other settings stay, except what the new
/// agent cannot apply, which is dropped.
pub(super) fn apply_shadow_agent(step: &mut WorkflowStep, choice: &ShadowAgent) {
    let settings = step
        .agent_settings
        .get_or_insert_with(crate::models::AgentSettings::default);
    settings.model = choice.model.clone().filter(|m| !m.trim().is_empty());
    settings.reasoning_effort = choice
        .reasoning_effort
        .clone()
        .filter(|e| !e.trim().is_empty());
    settings.connection_id = choice
        .connection_id
        .clone()
        .filter(|c| !c.trim().is_empty());
    if crate::agents::generation_settings::validate(
        &choice.agent,
        settings.reasoning_effort.as_deref(),
        settings.max_tokens,
    )
    .is_err()
    {
        settings.max_tokens = None;
        if crate::agents::generation_settings::validate(
            &choice.agent,
            settings.reasoning_effort.as_deref(),
            None,
        )
        .is_err()
        {
            settings.reasoning_effort = None;
        }
    }
    if settings
        .tools
        .as_ref()
        .is_some_and(|tools| tools.validate(&choice.agent).is_err())
    {
        settings.tools = None;
    }
    step.agent = choice.agent.clone();
}

/// The shadow's prompt: another Quick Prompt (a version of it) or an inline
/// text. What the step would inherit from its own Quick Prompt goes away with
/// it, the new prompt's skills loading instead.
fn apply_prompt(step: &mut WorkflowStep, prompt: &ShadowPrompt) {
    match prompt {
        ShadowPrompt::QuickPrompt { id, version } => {
            step.quick_prompt_id = Some(id.clone());
            step.quick_prompt_version = *version;
            step.prompt_template = String::new();
            step.skill_ids = Vec::new();
            step.profile_ids = Vec::new();
            step.directive_ids = Vec::new();
        }
        ShadowPrompt::Inline { template } => {
            step.prompt_template = template.clone();
            step.quick_prompt_id = None;
            step.quick_prompt_version = None;
        }
    }
}

/// The shadow's token budget handed to the agent as its generation cap, when
/// the agent takes one and it lowers the step's own.
fn apply_generation_cap(step: &mut WorkflowStep, shadow: &StepShadow) {
    let Some(cap) = shadow.budget.as_ref().and_then(|b| b.max_tokens) else {
        return;
    };
    let settings = step
        .agent_settings
        .get_or_insert_with(crate::models::AgentSettings::default);
    let lowered = settings.max_tokens.map_or(cap, |own| own.min(cap));
    if crate::agents::generation_settings::validate(
        &step.agent,
        settings.reasoning_effort.as_deref(),
        Some(lowered),
    )
    .is_ok()
    {
        settings.max_tokens = Some(lowered);
    }
}

/// Everything a shadow changes in its copy of the step.
pub async fn apply_shadow(
    state: &AppState,
    step: &mut WorkflowStep,
    shadow: &StepShadow,
    run_id: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(choice) = shadow.agent.as_ref() {
        apply_shadow_agent(step, choice);
    }
    if let Some(prompt) = shadow.prompt.as_ref() {
        apply_prompt(step, prompt);
    }
    apply_versions(state, step, shadow, run_id).await?;
    apply_generation_cap(step, shadow);
    Ok(())
}

/// The shadow's versions: skills swapped by base id (or dropped), and a Quick
/// Prompt step loading the chosen version's template and bindings.
pub async fn apply_versions(
    state: &AppState,
    step: &mut WorkflowStep,
    shadow: &StepShadow,
    run_id: Option<&str>,
) -> anyhow::Result<()> {
    let swap = |ids: &mut Vec<String>| {
        ids.retain_mut(|id| {
            let base = crate::core::skill_versions::split_pin(id)
                .map(|(base, _)| base.to_string())
                .unwrap_or_else(|_| id.clone());
            match shadow.skills.get(id).or_else(|| shadow.skills.get(&base)) {
                Some(next) if next.trim().is_empty() => false,
                Some(next) => {
                    *id = next.clone();
                    true
                }
                None => true,
            }
        });
    };
    let chosen = step.quick_prompt_id.as_ref().and_then(|id| {
        shadow
            .quick_prompt_versions
            .get(id)
            .map(|n| (id.clone(), *n))
    });
    // A step without skills of its own loads its QP's: they are read here so
    // the swap below reaches them too.
    let inherited = (step.skill_ids.is_empty() && chosen.is_none())
        .then(|| {
            step.quick_prompt_id
                .clone()
                .map(|id| (id, step.quick_prompt_version))
        })
        .flatten();
    let (version, current_skills) = state
        .db
        .with_read_conn(move |conn| {
            let version = match chosen {
                Some((id, n)) => crate::db::quick_prompts::get_quick_prompt_version(conn, &id, n)?,
                None => None,
            };
            let skills = match inherited {
                Some((id, Some(index))) => {
                    crate::db::quick_prompts::get_quick_prompt_version(conn, &id, index)?
                        .map(|version| version.skill_ids)
                }
                Some((id, None)) => {
                    crate::db::quick_prompts::get_quick_prompt(conn, &id)?.map(|qp| qp.skill_ids)
                }
                None => None,
            };
            Ok((version, skills))
        })
        .await?;
    if let Some(version) = version {
        // Step fields win over the QP's at hydration: setting them here makes
        // the chosen version's content the one that runs.
        if step.prompt_template.is_empty() {
            step.prompt_template = version.prompt_template.clone();
        }
        if step.skill_ids.is_empty() {
            step.skill_ids = version.skill_ids.clone();
        }
        if step.profile_ids.is_empty() {
            step.profile_ids = version.profile_ids.clone();
        }
        if step.directive_ids.is_empty() {
            step.directive_ids = version.directive_ids.clone();
        }
        // Hydration then loads this same version for what the step leaves out.
        step.quick_prompt_version = Some(version.version_index);
    }
    let inherits_skills = step.skill_ids.is_empty();
    if inherits_skills {
        if let Some(skills) = current_skills {
            step.skill_ids = skills;
        }
    }
    let before = step.skill_ids.len();
    swap(&mut step.skill_ids);
    // An empty list inherits the Quick Prompt's at hydration, which would hand
    // back what the shadow dropped: the step then takes its Quick Prompt's
    // content now, and keeps no link that could restore it.
    if inherits_skills && before > 0 && step.skill_ids.is_empty() {
        if let Err(error) =
            super::quick_prompt_hydrate::hydrate_step_from_quick_prompt(step, &state.db, run_id)
                .await
        {
            anyhow::bail!("Shadow `{}`: {error}", shadow.id);
        }
        step.skill_ids = Vec::new();
        step.quick_prompt_id = None;
        step.quick_prompt_version = None;
    }
    Ok(())
}

/// The identity a shadow's step takes, with the prompt it names.
async fn identity_of_shadow(
    state: &AppState,
    original: &WorkflowStep,
    varied: &WorkflowStep,
    shadow: &StepShadow,
) -> ExecutionIdentity {
    let mut identity = identity_of_step(state, varied).await;
    // `varied` has its Quick Prompt content copied in: the prompt is named
    // from what the shadow chose, not from the copy.
    identity.prompt = match shadow.prompt.as_ref() {
        Some(ShadowPrompt::Inline { template }) => format!("inline:{}", short_hash(template)),
        Some(ShadowPrompt::QuickPrompt { id, version }) => {
            let mut named = original.clone();
            named.prompt_template = String::new();
            named.quick_prompt_id = Some(id.clone());
            named.quick_prompt_version = *version;
            prompt_identity(state, &named).await
        }
        None => {
            let mut named = original.clone();
            if let Some(n) = original
                .quick_prompt_id
                .as_ref()
                .and_then(|id| shadow.quick_prompt_versions.get(id))
            {
                named.quick_prompt_version = Some(*n);
                named.prompt_template = String::new();
            }
            prompt_identity(state, &named).await
        }
    };
    identity
}

// ─── Which shadows a run starts ─────────────────────────────────────────────

/// Whether `shadow` covers the project of `run`.
fn covers_project(workflow: &Workflow, shadow: &StepShadow, run: &WorkflowRun) -> bool {
    let project = run.project_id.as_ref().or(workflow.project_id.as_ref());
    shadow.project_ids.is_empty()
        || project.is_some_and(|project| shadow.project_ids.contains(project))
}

/// Whether the run, the `ordinal`-th normal run of its workflow, is one the
/// shadow samples: one in `sample_every`, the first included.
fn is_sampled(shadow: &StepShadow, ordinal: u64) -> bool {
    let every = u64::from(shadow.sample_every.unwrap_or(1).max(1));
    ordinal.saturating_sub(1).is_multiple_of(every)
}

/// Whether a step runs shadows at all: one that acts outside Kronn or waits for
/// a person never does.
pub fn can_run_shadows(step: &WorkflowStep) -> bool {
    matches!(step.step_type, StepType::Agent)
        && !step.external_effect
        && !matches!(step.step_type, StepType::Gate)
}

/// What the runner hands over about the main execution of the step: the same
/// input every shadow receives.
#[derive(Clone)]
pub struct ShadowInput {
    pub project_path: String,
    pub project_id: Option<String>,
    pub work_dir: String,
    pub tokens: crate::models::TokensConfig,
    pub agents: crate::models::AgentsConfig,
    pub ollama_context_overrides: HashMap<String, u64>,
    /// The step's template context, as the main step renders its prompt.
    pub ctx: TemplateContext,
    /// The run's state before the main step consumed a Gate's feedback: the
    /// feedback reaches the shadows' prompts too.
    pub run_state: HashMap<String, String>,
    /// The run's own cancellation token: stopping the run stops its shadows.
    pub cancel: tokio_util::sync::CancellationToken,
}

/// The `cancel_registry` key of one shadow. Every shadow of a run starts with
/// [`registry_prefix`], so cancelling a run tree finds them all, including
/// those that outlive the run's runner.
fn registry_key(run_id: &str, index: usize, shadow_id: &str) -> String {
    format!("{}{index}:{shadow_id}", registry_prefix(run_id))
}

/// What the `cancel_registry` keys of every shadow of `run_id` start with.
pub fn registry_prefix(run_id: &str) -> String {
    format!("shadow:{run_id}:")
}

/// Starts the shadows of `step`, the main execution of which sits at
/// `result_index` of the run's results. Which shadows run, and their rows, are
/// settled before it returns, so a merge (KT-1544) knows what to wait for; the
/// shadows themselves run in the background and none is awaited. `step` is the
/// step as the workflow defines it, before its Quick Prompt is hydrated. A
/// shadow that cannot start is only logged.
pub async fn start_for_step(
    state: &AppState,
    workflow: &Workflow,
    run: &WorkflowRun,
    step: &WorkflowStep,
    result_index: usize,
    input: &ShadowInput,
) {
    if step.shadows.iter().all(|shadow| !shadow.enabled) {
        return;
    }
    if let Err(error) =
        run_step_shadows(state, workflow, run, step, result_index, input.clone()).await
    {
        tracing::warn!(
            run_id = %run.id, step = %step.name,
            "step shadows not started: {error}"
        );
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn blank_row(
    run: &WorkflowRun,
    step: &WorkflowStep,
    index: usize,
    shadow_id: &str,
) -> StepShadowRow {
    StepShadowRow {
        run_id: run.id.clone(),
        result_index: index as u32,
        step_name: step.name.clone(),
        shadow_id: shadow_id.to_string(),
        label: String::new(),
        status: ShadowStatus::Running,
        identity: ExecutionIdentity::default(),
        output: String::new(),
        tokens_used: None,
        duration_ms: None,
        cost_usd: None,
        model: None,
        stopped: None,
        started_at: now(),
        finished_at: None,
    }
}

async fn run_step_shadows(
    state: &AppState,
    workflow: &Workflow,
    run: &WorkflowRun,
    step: &WorkflowStep,
    index: usize,
    input: ShadowInput,
) -> anyhow::Result<()> {
    // Sampling counts the normal runs of the workflow, so it is read once.
    let ordinal = if step
        .shadows
        .iter()
        .any(|shadow| shadow.sample_every.is_some_and(|every| every > 1))
    {
        let counted = run.clone();
        Some(
            state
                .db
                .with_read_conn(move |conn| {
                    crate::db::workflows::normal_run_ordinal(conn, &counted)
                })
                .await?,
        )
    } else {
        None
    };
    let wanted: Vec<StepShadow> = step
        .shadows
        .iter()
        .filter(|shadow| shadow.enabled && covers_project(workflow, shadow, run))
        .filter(|shadow| {
            shadow.sample_every.is_none_or(|every| every <= 1)
                || ordinal.is_some_and(|ordinal| is_sampled(shadow, ordinal))
        })
        .cloned()
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }

    // The main execution: what it loads, set beside what each shadow loads.
    let mut main = blank_row(run, step, index, step_shadows::MAIN);
    main.identity = identity_of_step(state, step).await;
    let stored = main.clone();
    state
        .db
        .with_conn(move |conn| step_shadows::insert(conn, &stored))
        .await?;

    // Each shadow has its row before any runs: a merge waiting on it finds it
    // running, never absent.
    for shadow in &wanted {
        let mut row = blank_row(run, step, index, &shadow.id);
        row.label = shadow.display_label().to_string();
        let _ = state
            .db
            .with_conn(move |conn| step_shadows::insert(conn, &row))
            .await;
    }

    let input = std::sync::Arc::new(input);
    for shadow in wanted {
        let (state, workflow, run, step, input) = (
            state.clone(),
            workflow.clone(),
            run.clone(),
            step.clone(),
            input.clone(),
        );
        // Registered before the task starts, so a cancel that lands right after
        // this returns still finds the shadow. The guard leaves the registry
        // when the shadow ends.
        let stop = crate::CancelGuard::insert(
            &state.cancel_registry,
            registry_key(&run.id, index, &shadow.id),
        );
        tokio::spawn(async move {
            // A panic in the shadow settles its row; it never reaches the step.
            let (failed_state, failed_run, failed_step, failed_shadow) =
                (state.clone(), run.clone(), step.clone(), shadow.clone());
            let started = Instant::now();
            let run_cancel = input.cancel.clone();
            let mut attempt = tokio::spawn(async move {
                run_one(&state, &workflow, &run, &step, index, &shadow, &input).await
            });
            // Stopping the run, or cancelling its tree, stops the shadow: the
            // task is dropped where it stands, so nothing it was doing lands.
            let joined = tokio::select! {
                biased;
                _ = stop.token.cancelled() => None,
                _ = run_cancel.cancelled() => None,
                joined = &mut attempt => Some(joined),
            };
            match joined {
                None => {
                    attempt.abort();
                    let _ = (&mut attempt).await;
                    settle_stopped(
                        &failed_state,
                        &failed_run,
                        &failed_step,
                        index,
                        &failed_shadow,
                        started,
                    )
                    .await;
                }
                Some(Err(join)) => {
                    let mut row = blank_row(&failed_run, &failed_step, index, &failed_shadow.id);
                    row.label = failed_shadow.display_label().to_string();
                    row.status = ShadowStatus::Failed;
                    row.stopped = Some(format!("The shadow crashed: {join}"));
                    row.finished_at = Some(now());
                    let _ = failed_state
                        .db
                        .with_conn(move |conn| step_shadows::finish(conn, &row))
                        .await;
                }
                Some(Ok(())) => {}
            }
        });
    }
    Ok(())
}

/// Settles the row of a shadow stopped with its run. It is the last write the
/// shadow makes: what it had produced is dropped, only the identity it loaded
/// is kept for the comparison.
async fn settle_stopped(
    state: &AppState,
    run: &WorkflowRun,
    step: &WorkflowStep,
    index: usize,
    shadow: &StepShadow,
    started: Instant,
) {
    let mut row = blank_row(run, step, index, &shadow.id);
    row.label = shadow.display_label().to_string();
    row.status = ShadowStatus::Interrupted;
    row.stopped = Some("Stopped with its run: the run was cancelled".into());
    row.duration_ms = Some(started.elapsed().as_millis() as u64);
    row.finished_at = Some(now());
    let _ = state
        .db
        .with_conn(move |conn| {
            let known = step_shadows::list_for_run(conn, &row.run_id)?
                .into_iter()
                .find(|known| {
                    known.result_index == row.result_index && known.shadow_id == row.shadow_id
                });
            if let Some(known) = known {
                row.identity = known.identity;
                row.started_at = known.started_at;
            }
            step_shadows::finish(conn, &row)
        })
        .await;
}

/// What a shadow spent: tokens, and the cost of the attempts that report one.
pub(super) fn spent(result: &StepResult) -> (u64, Option<f64>, Option<String>) {
    let mut cost: Option<f64> = None;
    let mut model: Option<String> = None;
    if let Some(provenance) = result.agent_provenance.as_ref() {
        for attempt in &provenance.attempts {
            if let Some(attempt_cost) = attempt.cost_usd {
                cost = Some(cost.unwrap_or(0.0) + attempt_cost);
            }
            if attempt.resolved_model.is_some() {
                model = attempt.resolved_model.clone();
            }
        }
    }
    (
        result.tokens_used.unwrap_or(0),
        cost,
        model.or_else(|| result.step_model.clone()),
    )
}

fn cut(output: String) -> String {
    if output.len() <= MAX_STORED_OUTPUT_BYTES {
        return output;
    }
    let mut end = MAX_STORED_OUTPUT_BYTES;
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n{CUT_MARKER}{} bytes]", &output[..end], end)
}

/// One shadow: prepares its copy of the step, runs it under its time budget and
/// records how it ended.
async fn run_one(
    state: &AppState,
    workflow: &Workflow,
    run: &WorkflowRun,
    original: &WorkflowStep,
    index: usize,
    shadow: &StepShadow,
    input: &ShadowInput,
) {
    let mut row = blank_row(run, original, index, &shadow.id);
    row.label = shadow.display_label().to_string();
    let started = Instant::now();

    // Never a step that acts outside Kronn: recorded as skipped, not run.
    if !can_run_shadows(original) {
        row.status = ShadowStatus::Skipped;
        row.stopped =
            Some("Not run in a shadow: this step acts outside Kronn or waits for a person".into());
        row.identity = identity_of_step(state, original).await;
        row.finished_at = Some(now());
        let _ = state
            .db
            .with_conn(move |conn| step_shadows::insert(conn, &row))
            .await;
        return;
    }

    let mut varied = original.clone();
    varied.shadows = Vec::new();
    let prepared = apply_shadow(state, &mut varied, shadow, Some(&run.id)).await;
    row.identity = identity_of_shadow(state, original, &varied, shadow).await;
    let insert = row.clone();
    if state
        .db
        .with_conn(move |conn| step_shadows::insert(conn, &insert))
        .await
        .is_err()
    {
        return;
    }
    let hydrated = match prepared {
        Ok(()) => super::quick_prompt_hydrate::hydrate_step_from_quick_prompt(
            &mut varied,
            &state.db,
            Some(&run.id),
        )
        .await
        .map_err(|reason| anyhow::anyhow!(reason)),
        Err(error) => Err(error),
    };
    if let Err(error) = hydrated {
        row.status = ShadowStatus::Failed;
        row.output = error.to_string();
        row.duration_ms = Some(started.elapsed().as_millis() as u64);
        row.finished_at = Some(now());
        let _ = state
            .db
            .with_conn(move |conn| step_shadows::finish(conn, &row))
            .await;
        return;
    }
    // The Gate feedback the main step prepends reaches the shadow's prompt too.
    let mut feedback_state = input.run_state.clone();
    super::runner::inject_and_consume_gate_feedback(
        &mut varied.prompt_template,
        &mut feedback_state,
    );

    // Its time budget replaces the workflow's timeout, so a slow local model
    // may outlast the main step, which never waits for it.
    let limit = shadow
        .budget
        .as_ref()
        .and_then(|budget| budget.max_duration_secs)
        .or_else(|| {
            workflow
                .guards
                .as_ref()
                .and_then(|guards| guards.timeout_seconds)
        })
        .unwrap_or(crate::models::DEFAULT_GUARD_TIMEOUT_SECS);
    let full_access = input.agents.full_access_for(&varied.agent);
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(limit),
        super::steps::execute_step(
            &varied,
            &input.project_path,
            input.project_id.as_deref(),
            &input.work_dir,
            &input.tokens,
            full_access,
            &input.ctx,
            None,
            None,
            Some(&input.agents.model_tiers),
            Some(&crate::models::setup::HttpEndpoints::from_agents(
                &input.agents,
            )),
            Some(&input.ollama_context_overrides),
            // No room and no Kronn tools: a shadow is a trial of the answer.
            None,
            Some(&state.db),
            None,
            Some(&run.id),
        ),
    )
    .await;
    row.duration_ms = Some(started.elapsed().as_millis() as u64);
    row.finished_at = Some(now());
    match outcome {
        Err(_) => {
            row.status = ShadowStatus::TimedOut;
            row.stopped = Some(format!(
                "Stopped by the shadow's time budget: {limit} s elapsed"
            ));
        }
        Ok(outcome) => {
            let (tokens, cost, model) = spent(&outcome.result);
            row.tokens_used = Some(tokens);
            row.cost_usd = cost;
            row.model = model;
            row.output = cut(outcome.result.output.clone());
            row.status = if matches!(outcome.result.status, RunStatus::Success) {
                ShadowStatus::Success
            } else {
                ShadowStatus::Failed
            };
            if let Some(reason) = budget_exceeded(shadow, tokens, cost) {
                row.status = ShadowStatus::OverBudget;
                row.stopped = Some(reason);
            }
        }
    }
    let _ = state
        .db
        .with_conn(move |conn| step_shadows::finish(conn, &row))
        .await;
}

/// What the shadow spent beyond its token or cost budget, when it did.
pub fn budget_exceeded(shadow: &StepShadow, tokens: u64, cost: Option<f64>) -> Option<String> {
    let budget = shadow.budget.as_ref()?;
    if let Some(max) = budget.max_tokens.filter(|max| tokens > *max) {
        return Some(format!(
            "Over the shadow's token budget: {tokens} tokens spent (limit {max})"
        ));
    }
    if let (Some(max), Some(cost)) = (budget.max_cost_usd, cost) {
        if cost > max {
            return Some(format!(
                "Over the shadow's cost budget: ${cost:.4} spent (limit ${max:.4})"
            ));
        }
    }
    None
}

/// What a skipped `external_effect` step records in a run of the retired
/// workflow-level shadow. The run goes on with the next step: nothing was
/// produced to branch on.
pub fn skipped(step: &WorkflowStep) -> StepOutcome {
    let start = Instant::now();
    let mut result = blank_result(
        &step.name,
        RunStatus::Success,
        "Skipped in this shadow run: this step acts outside Kronn.\n[SIGNAL: SKIPPED_SHADOW]"
            .into(),
    );
    result.duration_ms = start.elapsed().as_millis() as u64;
    StepOutcome {
        result,
        condition_action: None,
    }
}

fn blank_result(name: &str, status: RunStatus, output: String) -> StepResult {
    StepResult {
        step_name: name.to_string(),
        status,
        output,
        tokens_used: Some(0),
        duration_ms: 0,
        started_at: None,
        condition_result: None,
        envelope_detected: None,
        step_kind: None,
        step_agent: None,
        step_model: None,
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

// ─── Saving ─────────────────────────────────────────────────────────────────

/// Checks the shadows a person saves on their steps: ids, scope, sampling,
/// overrides, and that the step may run a shadow at all.
pub fn validate(steps: &[WorkflowStep]) -> Result<(), String> {
    for step in steps
        .iter()
        .filter(|step| !step.shadows.is_empty() || step.shadow_merge.is_some())
    {
        validate_step(step)?;
    }
    Ok(())
}

fn validate_step(step: &WorkflowStep) -> Result<(), String> {
    let name = &step.name;
    if !matches!(step.step_type, StepType::Agent) {
        return Err(format!(
            "Step `{name}`: only an Agent step can carry shadows"
        ));
    }
    if step.external_effect {
        return Err(format!(
            "Step `{name}` acts outside Kronn (external_effect): a shadow never runs it, so it cannot carry shadows"
        ));
    }
    if step.shadows.len() > MAX_SHADOWS_PER_STEP {
        return Err(format!(
            "Step `{name}`: at most {MAX_SHADOWS_PER_STEP} shadows ({} given)",
            step.shadows.len()
        ));
    }
    if let Some(merge) = &step.shadow_merge {
        validate_merge(step, merge)?;
    }
    let mut seen = std::collections::HashSet::new();
    for shadow in &step.shadows {
        let id = &shadow.id;
        let valid_id = !id.is_empty()
            && id.len() <= 40
            && id.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            })
            && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit());
        if !valid_id {
            return Err(format!(
                "Step `{name}`: shadow id `{id}` must be 1 to 40 characters of a-z, 0-9, `.`, `_` or `-`, starting with a letter or digit"
            ));
        }
        if !seen.insert(id.clone()) {
            return Err(format!("Step `{name}`: shadow id `{id}` is used twice"));
        }
        if shadow.label.chars().count() > 80 || shadow.label.chars().any(char::is_control) {
            return Err(format!(
                "Step `{name}`: the label of shadow `{id}` must be one line of at most 80 characters"
            ));
        }
        if shadow
            .sample_every
            .is_some_and(|n| n == 0 || n > MAX_SAMPLE_EVERY)
        {
            return Err(format!(
                "Step `{name}`: shadow `{id}` samples one run in 1 to {MAX_SAMPLE_EVERY}"
            ));
        }
        let replacements: Vec<String> = shadow
            .skills
            .values()
            .filter(|skill| !skill.trim().is_empty())
            .cloned()
            .collect();
        crate::core::skill_versions::validate_pins(&replacements)
            .map_err(|e| format!("Step `{name}`, shadow `{id}`: {e}"))?;
        if let Some((qp, _)) = shadow.quick_prompt_versions.iter().find(|(_, n)| **n == 0) {
            return Err(format!(
                "Step `{name}`, shadow `{id}`: Quick Prompt {qp} has no version 0, versions start at 1"
            ));
        }
        match &shadow.prompt {
            Some(ShadowPrompt::QuickPrompt {
                version: Some(0), ..
            }) => {
                return Err(format!(
                    "Step `{name}`, shadow `{id}`: a Quick Prompt has no version 0, versions start at 1"
                ))
            }
            Some(ShadowPrompt::Inline { template }) if template.trim().is_empty() => {
                return Err(format!(
                    "Step `{name}`, shadow `{id}`: the inline prompt is empty"
                ))
            }
            _ => {}
        }
        if let Some(choice) = &shadow.agent {
            if choice.agent == crate::models::AgentType::Custom && choice.connection_id.is_none() {
                return Err(format!(
                    "Step `{name}`, shadow `{id}`: a custom agent needs the connection it talks to"
                ));
            }
        }
        if let Some(budget) = &shadow.budget {
            if budget.max_tokens == Some(0)
                || budget.max_duration_secs == Some(0)
                || budget
                    .max_cost_usd
                    .is_some_and(|cost| !cost.is_finite() || cost <= 0.0)
            {
                return Err(format!(
                    "Step `{name}`, shadow `{id}`: a budget limit must be above zero"
                ));
            }
        }
    }
    Ok(())
}

/// The merge a person sets on a step (KT-1544): it needs shadows to merge, names
/// only shadows the step carries, and bounds its wait.
fn validate_merge(step: &WorkflowStep, merge: &crate::models::ShadowMerge) -> Result<(), String> {
    let name = &step.name;
    if merge.enabled && step.shadows.is_empty() {
        return Err(format!(
            "Step `{name}`: a merge needs at least one shadow to merge with the main execution"
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for id in &merge.shadows {
        if !step.shadows.iter().any(|shadow| &shadow.id == id) {
            return Err(format!(
                "Step `{name}`: the merge selects shadow `{id}`, which the step does not carry"
            ));
        }
        if !seen.insert(id) {
            return Err(format!(
                "Step `{name}`: the merge selects shadow `{id}` twice"
            ));
        }
    }
    if merge
        .timeout_secs
        .is_some_and(|secs| secs == 0 || secs > crate::models::MAX_SHADOW_MERGE_TIMEOUT_SECS)
    {
        return Err(format!(
            "Step `{name}`: the merge waits 1 to {} seconds for its shadows",
            crate::models::MAX_SHADOW_MERGE_TIMEOUT_SECS
        ));
    }
    if let Some(choice) = &merge.agent {
        if choice.agent == crate::models::AgentType::Custom && choice.connection_id.is_none() {
            return Err(format!(
                "Step `{name}`: the merge's custom agent needs the connection it talks to"
            ));
        }
    }
    Ok(())
}

/// Gives each step of `to` the shadows of the step of `from` it matches (by id,
/// else by name): what an agent's write keeps, since shadows are human-only.
pub fn keep_stored_shadows(to: &mut [WorkflowStep], from: &[WorkflowStep]) {
    for step in to {
        let known = from
            .iter()
            .find(|known| known.id.is_some() && known.id == step.id)
            .or_else(|| from.iter().find(|known| known.name == step.name));
        step.shadows = known.map(|known| known.shadows.clone()).unwrap_or_default();
        step.shadow_merge = known.and_then(|known| known.shadow_merge.clone());
    }
}

/// After an import, the shadows name the imported skills; a Quick Prompt comes
/// over without its history, so a chosen version of it is dropped.
pub fn remap_imported(
    workflow: &mut Workflow,
    skills: &HashMap<String, String>,
    quick_prompts: &HashMap<String, String>,
) {
    let swap = |id: &String| skills.get(id).cloned().unwrap_or_else(|| id.clone());
    for step in workflow
        .steps
        .iter_mut()
        .chain(workflow.on_failure.iter_mut())
    {
        for shadow in &mut step.shadows {
            shadow.skills = shadow
                .skills
                .iter()
                .map(|(k, v)| (swap(k), if v.is_empty() { v.clone() } else { swap(v) }))
                .collect();
            shadow
                .quick_prompt_versions
                .retain(|id, _| !quick_prompts.contains_key(id));
            if let Some(ShadowPrompt::QuickPrompt { id, version }) = shadow.prompt.as_mut() {
                if let Some(imported) = quick_prompts.get(id.as_str()) {
                    *id = imported.clone();
                    *version = None;
                }
            }
        }
    }
}

/// What the normal execution takes over from a shadow (KT-1450): the tested
/// version of a skill in the step that carries the shadow, or the tested
/// version of a Quick Prompt pinned there. The shadow's override goes away.
#[derive(Debug, Clone, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PromoteShadowRequest {
    /// `skill` or `quick_prompt`.
    pub kind: String,
    pub id: String,
    /// The step carrying the shadow (id or name).
    pub step: String,
    /// The shadow that tested it; optional while the step has one shadow.
    #[serde(default)]
    #[ts(optional)]
    pub shadow: Option<String>,
}

pub fn promote(
    conn: &rusqlite::Connection,
    workflow: &mut Workflow,
    request: &PromoteShadowRequest,
) -> anyhow::Result<()> {
    let step = workflow
        .steps
        .iter_mut()
        .chain(workflow.on_failure.iter_mut())
        .find(|step| step_matches(step, &request.step))
        .ok_or_else(|| anyhow::anyhow!("This workflow has no step `{}`", request.step))?;
    let position = match request.shadow.as_deref() {
        Some(wanted) => step
            .shadows
            .iter()
            .position(|shadow| shadow.id == wanted)
            .ok_or_else(|| anyhow::anyhow!("Step `{}` has no shadow `{wanted}`", step.name))?,
        None if step.shadows.len() == 1 => 0,
        None => anyhow::bail!("Name the shadow that tested it: this step has several"),
    };
    match request.kind.as_str() {
        "skill" => {
            let tested = step.shadows[position]
                .skills
                .remove(&request.id)
                .ok_or_else(|| {
                    anyhow::anyhow!("The shadow does not replace skill {}", request.id)
                })?;
            for id in std::mem::take(&mut step.skill_ids) {
                let base = crate::core::skill_versions::split_pin(&id)
                    .map(|(base, _)| base.to_string())
                    .unwrap_or_else(|_| id.clone());
                if id == request.id || base == request.id {
                    // A dropped skill is promoted as dropped.
                    if !tested.trim().is_empty() {
                        step.skill_ids.push(tested.clone());
                    }
                } else {
                    step.skill_ids.push(id);
                }
            }
        }
        "quick_prompt" => {
            let version = step.shadows[position]
                .quick_prompt_versions
                .remove(&request.id)
                .ok_or_else(|| {
                    anyhow::anyhow!("The shadow does not choose a version of {}", request.id)
                })?;
            crate::db::quick_prompts::get_quick_prompt_version(conn, &request.id, version)?
                .ok_or_else(|| {
                    anyhow::anyhow!("Quick Prompt {} has no version {version}", request.id)
                })?;
            // The step pins the tested version: the Quick Prompt, which other
            // workflows may load, is left as it is.
            if step.quick_prompt_id.as_deref() == Some(request.id.as_str()) {
                step.quick_prompt_version = Some(version);
            }
        }
        other => anyhow::bail!("Unknown resource kind `{other}`"),
    }
    workflow.updated_at = chrono::Utc::now();
    Ok(())
}

// ─── Reading ────────────────────────────────────────────────────────────────

/// One execution of a step, as the comparison shows it.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ShadowExecution {
    /// `None` for the main execution.
    #[ts(optional)]
    pub shadow_id: Option<String>,
    pub label: String,
    pub status: ShadowStatus,
    pub identity: ExecutionIdentity,
    pub output: String,
    #[ts(optional)]
    pub tokens_used: Option<u64>,
    #[ts(optional)]
    pub duration_ms: Option<u64>,
    #[ts(optional)]
    pub cost_usd: Option<f64>,
    /// The model the execution resolved, when it ran.
    #[ts(optional)]
    pub model: Option<String>,
    /// Why a budget, the time limit or a skip ended the shadow, when one did.
    #[ts(optional)]
    pub stopped: Option<String>,
    /// Whether the output holds an HTML document an Artifact can be made from
    /// (KT-1990).
    pub html: ExecutionHtml,
    /// The Artifact a person made from this execution, when there is one.
    #[ts(optional)]
    pub artifact_id: Option<String>,
}

/// The main execution of a step and its shadows.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct StepShadowComparison {
    pub step_name: String,
    pub result_index: u32,
    pub main: ShadowExecution,
    pub shadows: Vec<ShadowExecution>,
    /// The merge of the step's executions, when the step carries one (KT-1544).
    #[ts(optional)]
    pub merge: Option<super::shadow_merge::ShadowMergeReport>,
}

/// What a step result cost, from the attempts that report a cost.
pub fn result_cost_usd(result: &StepResult) -> Option<f64> {
    spent(result).1
}

fn main_execution(row: &StepShadowRow, result: Option<&StepResult>) -> ShadowExecution {
    // A merge stores the main execution's own output and metrics in its row:
    // the step's result then holds the merged output.
    if row.finished_at.is_some() {
        return ShadowExecution {
            shadow_id: None,
            label: String::new(),
            status: row.status,
            identity: row.identity.clone(),
            output: row.output.clone(),
            tokens_used: row.tokens_used,
            duration_ms: row.duration_ms,
            cost_usd: row.cost_usd,
            model: row.model.clone().or_else(|| row.identity.model.clone()),
            stopped: None,
            html: ExecutionHtml::of(&row.output),
            artifact_id: None,
        };
    }
    let (tokens, cost, model) = result
        .map(spent)
        .map_or((None, None, None), |(t, c, m)| (Some(t), c, m));
    let status = match result.map(|r| &r.status) {
        Some(RunStatus::Success) | Some(RunStatus::Partial) => ShadowStatus::Success,
        Some(RunStatus::Running) | Some(RunStatus::Pending) | None => ShadowStatus::Running,
        Some(_) => ShadowStatus::Failed,
    };
    let output = result.map(|r| r.output.clone()).unwrap_or_default();
    ShadowExecution {
        shadow_id: None,
        label: String::new(),
        status,
        identity: row.identity.clone(),
        html: ExecutionHtml::of(&output),
        output,
        tokens_used: tokens,
        duration_ms: result.map(|r| r.duration_ms),
        cost_usd: cost,
        model: model.or_else(|| row.identity.model.clone()),
        stopped: None,
        artifact_id: None,
    }
}

fn shadow_execution(row: &StepShadowRow) -> ShadowExecution {
    ShadowExecution {
        shadow_id: Some(row.shadow_id.clone()),
        label: row.label.clone(),
        status: row.status,
        identity: row.identity.clone(),
        output: row.output.clone(),
        tokens_used: row.tokens_used,
        duration_ms: row.duration_ms,
        cost_usd: row.cost_usd,
        model: row.model.clone().or_else(|| row.identity.model.clone()),
        stopped: row.stopped.clone(),
        html: ExecutionHtml::of(&row.output),
        artifact_id: None,
    }
}

/// The steps of run `run_id` that ran shadows: per step, the main execution
/// beside what each shadow was and produced.
pub fn comparisons(
    conn: &rusqlite::Connection,
    run: &WorkflowRun,
) -> anyhow::Result<Vec<StepShadowComparison>> {
    let rows = step_shadows::list_for_run(conn, &run.id)?;
    let mut steps: Vec<StepShadowComparison> = Vec::new();
    for row in rows {
        if row.shadow_id == step_shadows::MAIN {
            let result = run
                .step_results
                .get(row.result_index as usize)
                .filter(|result| result.step_name == row.step_name);
            steps.push(StepShadowComparison {
                step_name: row.step_name.clone(),
                result_index: row.result_index,
                main: main_execution(&row, result),
                shadows: Vec::new(),
                merge: None,
            });
        } else if let Some(step) = steps
            .iter_mut()
            .find(|step| step.result_index == row.result_index)
        {
            step.shadows.push(shadow_execution(&row));
        }
    }
    for merged in crate::db::step_shadow_merges::list_for_run(conn, &run.id)? {
        if let Some(step) = steps
            .iter_mut()
            .find(|step| step.result_index == merged.result_index)
        {
            step.merge = Some(merged.report);
        }
    }
    let artifacts = crate::db::step_artifacts::for_run(conn, &run.id)?;
    for step in &mut steps {
        let index = step.result_index;
        for execution in std::iter::once(&mut step.main).chain(step.shadows.iter_mut()) {
            let key = (index, execution.shadow_id.clone().unwrap_or_default());
            execution.artifact_id = artifacts.get(&key).cloned();
        }
    }
    Ok(steps)
}

pub use crate::db::legacy_shadow::{legacy_shadow_to_steps, migrate_workflow_shadows};
