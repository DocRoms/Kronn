//! Shadow runs (KT-1449): alongside a normal run, the same workflow runs on the
//! same inputs with other skill/QP versions, its `external_effect` steps
//! skipped, so the two can be compared without acting twice. The variant lives
//! in the run's trigger context, like the agents chosen at launch.

use std::time::Instant;

use crate::models::{RunStatus, StepResult, Workflow, WorkflowRun, WorkflowStep};
use crate::AppState;

use super::steps::StepOutcome;

pub const VARIANT_KEY: &str = "__variant__";
pub const SHADOW_OF_KEY: &str = "__shadow_of__";
pub const SHADOW: &str = "shadow";
pub const NORMAL: &str = "normal";

pub fn is_shadow(run: &WorkflowRun) -> bool {
    run.trigger_context
        .as_ref()
        .and_then(|t| t.get(VARIANT_KEY))
        .and_then(|v| v.as_str())
        == Some(SHADOW)
}

pub fn variant(run: &WorkflowRun) -> &'static str {
    if is_shadow(run) {
        SHADOW
    } else {
        NORMAL
    }
}

/// Whether run `parent_run_id` is a shadow run: its sub-runs are too, so
/// they skip their external effects as well.
pub async fn parent_is_shadow(state: &AppState, parent_run_id: &str) -> bool {
    let id = parent_run_id.to_string();
    state
        .db
        .with_read_conn(move |conn| crate::db::workflows::get_run(conn, &id))
        .await
        .ok()
        .flatten()
        .is_some_and(|run| is_shadow(&run))
}

/// A normal run on its first execution, of a project the shadow covers.
fn wants_shadow(workflow: &Workflow, run: &WorkflowRun) -> bool {
    let Some(shadow) = workflow.shadow.as_ref().filter(|s| s.enabled) else {
        return false;
    };
    if is_shadow(run) || !run.step_results.is_empty() {
        return false;
    }
    let project = run.project_id.as_ref().or(workflow.project_id.as_ref());
    shadow.project_ids.is_empty()
        || project.is_some_and(|project| shadow.project_ids.contains(project))
}

/// The shadow copy of `run`: same inputs, its own id, linked to `run`.
pub fn shadow_of(run: &WorkflowRun) -> WorkflowRun {
    let mut context = match run.trigger_context.clone() {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    context.insert(VARIANT_KEY.into(), SHADOW.into());
    context.insert(SHADOW_OF_KEY.into(), run.id.clone().into());
    WorkflowRun {
        id: uuid::Uuid::new_v4().to_string(),
        status: RunStatus::Pending,
        trigger_context: Some(serde_json::Value::Object(context)),
        step_results: Vec::new(),
        tokens_used: 0,
        workspace_path: None,
        started_at: chrono::Utc::now(),
        finished_at: None,
        // Its own lifecycle: the parent of `run` never waits for it.
        parent_run_id: None,
        state: Default::default(),
        produced_branches: Vec::new(),
        concurrency_key: None,
        triggered_by_run_id: None,
        outcome: None,
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
        ..run.clone()
    }
}

/// Starts the shadow of `run` once, in the background. It never blocks or
/// fails the normal run: a shadow that cannot start is only logged.
pub async fn spawn_if_wanted(state: &AppState, workflow: &Workflow, run: &WorkflowRun) {
    if !wants_shadow(workflow, run) {
        return;
    }
    let shadow = shadow_of(run);
    let (normal_id, inserted) = (run.id.clone(), shadow.clone());
    let started = state
        .db
        .with_conn(move |conn| {
            if crate::db::workflows::shadow_run_of(conn, &normal_id)?.is_some() {
                return Ok(false);
            }
            crate::db::workflows::insert_run(conn, &inserted)?;
            Ok(true)
        })
        .await;
    match started {
        Ok(true) => {
            crate::api::workflows::spawn_manual_run(state, workflow.clone(), shadow, None, false)
        }
        Ok(false) => {}
        Err(error) => tracing::warn!(run_id = %run.id, "shadow run not started: {error}"),
    }
}

/// A shadow runs alongside its normal run, which may hold the project's
/// checkout: unless the workflow never writes it, the shadow gets its own
/// worktree instead of waiting on the checkout and being refused.
pub fn isolate(workflow: &mut Workflow) {
    match workflow.workspace_config.as_mut() {
        Some(config) if config.main_tree_read_only => {}
        Some(config) => config.require_isolation = true,
        None => {
            workflow.workspace_config = Some(crate::models::WorkspaceConfig {
                hooks: Default::default(),
                require_isolation: true,
                main_tree_read_only: false,
                base_ref: None,
            })
        }
    }
}

/// The shadow's versions: skills swapped by base id, and each Quick Prompt
/// step loading the chosen version's template and bindings.
pub async fn apply_versions(state: &AppState, workflow: &mut Workflow) -> anyhow::Result<()> {
    let Some(shadow) = workflow.shadow.clone() else {
        return Ok(());
    };
    let swap = |ids: &mut Vec<String>| {
        for id in ids.iter_mut() {
            let base = crate::core::skill_versions::split_pin(id)
                .map(|(base, _)| base.to_string())
                .unwrap_or_else(|_| id.clone());
            if let Some(next) = shadow.skills.get(id).or_else(|| shadow.skills.get(&base)) {
                *id = next.clone();
            }
        }
    };
    let wanted: Vec<(String, u32)> = shadow
        .quick_prompt_versions
        .iter()
        .map(|(id, n)| (id.clone(), *n))
        .collect();
    // A step without skills of its own loads its QP's: they are read here so
    // the swap below reaches them too.
    let inherited: Vec<String> = workflow
        .steps
        .iter()
        .chain(workflow.on_failure.iter())
        .filter(|step| step.skill_ids.is_empty())
        .filter_map(|step| step.quick_prompt_id.clone())
        .filter(|id| !shadow.quick_prompt_versions.contains_key(id))
        .collect();
    let (versions, current_skills) = state
        .db
        .with_read_conn(move |conn| {
            let mut found = std::collections::HashMap::new();
            for (id, n) in wanted {
                if let Some(version) =
                    crate::db::quick_prompts::get_quick_prompt_version(conn, &id, n)?
                {
                    found.insert(id, version);
                }
            }
            let mut skills = std::collections::HashMap::new();
            for id in inherited {
                if let Some(qp) = crate::db::quick_prompts::get_quick_prompt(conn, &id)? {
                    skills.insert(id, qp.skill_ids);
                }
            }
            Ok((found, skills))
        })
        .await?;
    for step in workflow
        .steps
        .iter_mut()
        .chain(workflow.on_failure.iter_mut())
    {
        if let Some(version) = step
            .quick_prompt_id
            .as_ref()
            .and_then(|id| versions.get(id))
        {
            // Step fields win over the QP's at hydration: setting them here
            // makes the chosen version's content the one that runs.
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
        }
        if step.skill_ids.is_empty() {
            if let Some(skills) = step
                .quick_prompt_id
                .as_ref()
                .and_then(|id| current_skills.get(id))
            {
                step.skill_ids = skills.clone();
            }
        }
        swap(&mut step.skill_ids);
    }
    Ok(())
}

/// After an import, the shadow names the imported skills; a Quick Prompt
/// comes over without its history, so a chosen version of it is dropped.
pub fn remap_imported(
    workflow: &mut Workflow,
    skills: &std::collections::HashMap<String, String>,
    quick_prompts: &std::collections::HashMap<String, String>,
) {
    let Some(shadow) = workflow.shadow.as_mut() else {
        return;
    };
    let swap = |id: &String| skills.get(id).cloned().unwrap_or_else(|| id.clone());
    shadow.skills = shadow
        .skills
        .iter()
        .map(|(k, v)| (swap(k), swap(v)))
        .collect();
    shadow
        .quick_prompt_versions
        .retain(|id, _| !quick_prompts.contains_key(id));
}

/// What a skipped `external_effect` step records in a shadow run. The run
/// goes on with the next step: nothing was produced to branch on.
pub fn skipped(step: &WorkflowStep) -> StepOutcome {
    let start = Instant::now();
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Success,
            output: "Skipped in this shadow run: this step acts outside Kronn.\n[SIGNAL: SKIPPED_SHADOW]"
                .into(),
            tokens_used: Some(0),
            duration_ms: start.elapsed().as_millis() as u64,
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
        },
        condition_action: None,
    }
}

/// What the normal lane takes over from the shadow (KT-1450): the tested
/// version of a skill in every step that loads it, or the tested version of a
/// Quick Prompt made its current one. The shadow's override goes away.
#[derive(Debug, Clone, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PromoteShadowRequest {
    /// `skill` or `quick_prompt`.
    pub kind: String,
    pub id: String,
}

pub fn promote(
    conn: &rusqlite::Connection,
    workflow: &mut Workflow,
    request: &PromoteShadowRequest,
) -> anyhow::Result<()> {
    let Some(shadow) = workflow.shadow.as_mut() else {
        anyhow::bail!("This workflow has no shadow variant");
    };
    match request.kind.as_str() {
        "skill" => {
            let tested = shadow.skills.remove(&request.id).ok_or_else(|| {
                anyhow::anyhow!("The shadow does not replace skill {}", request.id)
            })?;
            for step in workflow
                .steps
                .iter_mut()
                .chain(workflow.on_failure.iter_mut())
            {
                for id in step.skill_ids.iter_mut() {
                    let base = crate::core::skill_versions::split_pin(id)
                        .map(|(base, _)| base.to_string())
                        .unwrap_or_else(|_| id.clone());
                    if *id == request.id || base == request.id {
                        *id = tested.clone();
                    }
                }
            }
        }
        "quick_prompt" => {
            let version = shadow
                .quick_prompt_versions
                .remove(&request.id)
                .ok_or_else(|| {
                    anyhow::anyhow!("The shadow does not choose a version of {}", request.id)
                })?;
            let tested =
                crate::db::quick_prompts::get_quick_prompt_version(conn, &request.id, version)?
                    .ok_or_else(|| {
                        anyhow::anyhow!("Quick Prompt {} has no version {version}", request.id)
                    })?;
            let mut current = crate::db::quick_prompts::get_quick_prompt(conn, &request.id)?
                .ok_or_else(|| anyhow::anyhow!("Quick Prompt {} not found", request.id))?;
            current.prompt_template = tested.prompt_template;
            current.variables = tested.variables;
            current.skill_ids = tested.skill_ids;
            current.profile_ids = tested.profile_ids;
            current.directive_ids = tested.directive_ids;
            current.updated_at = chrono::Utc::now();
            crate::db::quick_prompts::update_quick_prompt(conn, &current)?;
        }
        other => anyhow::bail!("Unknown resource kind `{other}`"),
    }
    workflow.updated_at = chrono::Utc::now();
    Ok(())
}
