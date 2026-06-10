//! Workflow runner: orchestrates a full workflow run.
//!
//! Creates workspace → runs hooks → executes steps sequentially →
//! executes actions → cleans up workspace.

use anyhow::Result;
use chrono::Utc;

use crate::models::*;
use crate::AppState;

use super::template::TemplateContext;
use super::workspace::Workspace;
use super::steps::{execute_step, StepOutcome};

/// Events emitted during a workflow run for real-time SSE streaming.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event", content = "data")]
pub enum RunEvent {
    /// A step is about to start executing.
    StepStart { step_name: String, step_index: usize, total_steps: usize },
    /// Partial output from the agent (streamed in real-time).
    StepProgress { text: String },
    /// A step has finished executing.
    StepDone { step_result: StepResult },
    /// 0.7.0 — A `WorkflowGuards` limit was hit and the run was halted.
    /// Distinct from `RunDone { Failed }`: the frontend uses this to
    /// render the orange shield "Stoppé par garde-fou" badge instead of
    /// the red "Échec" one. `actual` is the value at trigger time
    /// (e.g. seconds elapsed for Timeout, calls counted for MaxLlmCalls,
    /// revisit count for LoopDetection).
    GuardTriggered { kind: GuardKind, threshold: u64, actual: u64 },
    /// The entire run has finished.
    RunDone { status: RunStatus },
    /// An error occurred.
    #[allow(dead_code)]
    RunError { error: String },
}

/// Optional sender for real-time progress events.
pub type EventSender = tokio::sync::mpsc::Sender<RunEvent>;

/// 2026-06-11 (Phase 1b-ii) — LLM-calls budget shared across a sub-workflow
/// tree. Without it, each child run reset its own `max_llm_calls` counter, so
/// a nested orchestration could spend depth × cap LLM calls — a token bomb.
/// The ROOT run creates one (`root`) from its resolved guards; every
/// descendant `execute_run` is handed a CLONE (same `Arc` counter + same
/// cap), so the WHOLE tree shares one quota governed by the root's limit.
#[derive(Clone)]
pub struct SharedBudget {
    llm_calls: std::sync::Arc<std::sync::atomic::AtomicU32>,
    max_llm_calls: u32,
}

impl SharedBudget {
    /// Fresh budget for a top-level run, capped at the run's resolved limit.
    pub fn root(max_llm_calls: u32) -> Self {
        Self {
            llm_calls: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            max_llm_calls,
        }
    }
    pub fn llm_calls(&self) -> u32 {
        self.llm_calls.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn add_llm_calls(&self, n: u32) {
        self.llm_calls.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    }
    /// The tree-wide cap (the root run's `max_llm_calls`, inherited by every
    /// descendant — a child's own `max_llm_calls` is ignored when nested).
    pub fn max_llm_calls(&self) -> u32 {
        self.max_llm_calls
    }
}

/// Compute the workflow-step index where execution should pick up.
///
/// Fresh runs (no prior results) → 0. Resuming runs → the index of the step
/// that follows the last recorded result *in the workflow definition*, not
/// in the result vector — Goto loops can produce more results than there
/// are declared steps, so `step_results.len()` is wrong on resume.
///
/// Returns `workflow.steps.len()` when the last result's step name is no
/// longer in the workflow (the workflow was edited mid-run): treat the run
/// as already past the end so the runner gracefully wraps up rather than
/// re-running steps the operator removed.
pub(crate) fn next_step_index_for_resume(
    steps: &[crate::models::WorkflowStep],
    step_results: &[crate::models::StepResult],
) -> usize {
    match step_results.last() {
        None => 0,
        Some(last) => steps
            .iter()
            .position(|s| s.name == last.step_name)
            .map(|i| i + 1)
            .unwrap_or(steps.len()),
    }
}

/// How many `step_results` to KEEP when a Gate RequestChanges sends the
/// run back to a target step. Cut at the FIRST result that IS the target
/// step (by name): everything before the target's first execution is, by
/// construction, the clean linear prefix `steps[0..target]`, so the
/// resume cursor (`next_step_index_for_resume`, keyed on the LAST KEPT
/// row's name) lands exactly on the target.
///
/// 2026-06-13 (run-10 live bug) — this used to cut at the MOST RECENT
/// occurrence (2026-06-10 audit P1). After a Goto debate loop
/// (`triage → … → plan_review → Goto(triage) → …`), that kept the FIRST
/// round's downstream rows (`plan_lint`, `plan_review`) in the prefix;
/// the resume cursor then keyed off `plan_review` and re-entered the run
/// AT THE GATE — the human's "request changes" never reached the triage.
/// Falls back to the bounded positional index when the target never ran.
pub(crate) fn request_changes_cut(
    step_results: &[crate::models::StepResult],
    target_step_name: Option<&str>,
    fallback_idx: usize,
) -> usize {
    target_step_name
        .and_then(|tn| step_results.iter().position(|r| r.step_name == tn))
        .unwrap_or_else(|| fallback_idx.min(step_results.len()))
}

/// Gate feedback, runtime injection (approach B). When a human Gate sends a run
/// back with "request changes", `decide_run` stashes the comment in
/// `state["last_human_feedback"]`. This prepends it to the re-run target step's
/// prompt and CONSUMES it (removes from state) so only that one step gets it —
/// every preset and every hand-built workflow surfaces the feedback without a
/// `{{state.last_human_feedback}}` placeholder. Returns true when injected.
/// Extracted from the run loop so the consume-once contract is unit-testable.
pub(crate) fn inject_and_consume_gate_feedback(
    prompt_template: &mut String,
    state: &mut std::collections::HashMap<String, String>,
) -> bool {
    match state.remove("last_human_feedback") {
        Some(fb) if !fb.trim().is_empty() => {
            *prompt_template = format!(
                "⚠️ L'humain a relu et demandé ces changements — adresse-les EN PRIORITÉ avant le reste :\n{}\n\n---\n\n{}",
                fb.trim(),
                prompt_template
            );
            true
        }
        _ => false,
    }
}

/// Execute a complete workflow run.
#[allow(clippy::too_many_arguments)]
pub async fn execute_run(
    state: AppState,
    workflow: &Workflow,
    run: &mut WorkflowRun,
    tokens_config: &TokensConfig,
    agents_config: &AgentsConfig,
    events_tx: Option<EventSender>,
    // 2026-06-11 Phase 1b-ii — `Some` when this run is a sub-workflow child
    // (inherits the parent tree's shared LLM-calls budget). `None` for a
    // top-level run → a fresh budget is created from its resolved guards.
    shared_budget: Option<SharedBudget>,
    // 2026-06-11 Phase 2 (worktree handoff) — `Some(path)` when this run is a
    // sub-workflow child that must SHARE the parent's git worktree instead of
    // creating its own. The child then commits to the parent's branch, so a
    // later parent step (e.g. `create_pr`) sees the child's implementation.
    // The child ATTACHES (no create), skips the `before_run` hook, and NEVER
    // cleans up / preserves the worktree — the parent owns its lifecycle.
    // `None` for a top-level run or an isolated child.
    inherited_workspace: Option<String>,
) -> Result<()> {
    // Captured once: drives the attach-vs-create and the skip-cleanup paths.
    let is_inherited_workspace = inherited_workspace.is_some();
    // Helper to send events (best-effort, ignore send errors)
    let emit = |evt: RunEvent| {
        let tx = events_tx.clone();
        async move {
            if let Some(tx) = tx {
                let _ = tx.send(evt).await;
            }
        }
    };

    // 0.8.2 — Broadcast the run state to ALL connected WS clients so
    // WorkflowDetail pages opened in another tab pick up step transitions
    // (especially the Running → WaitingApproval flip) without a refresh.
    // The SSE channel (`emit` above) only feeds the tab that triggered
    // the run; cross-tab updates need WS. Best-effort: ignore send errors
    // when no client is connected.
    let workflow_id_for_ws = workflow.id.clone();
    let run_id_for_ws = run.id.clone();
    let total_steps_for_ws = workflow.steps.len() as u32;
    let broadcast_run_state = |status: &crate::models::RunStatus, step_index: i32, current_step: Option<String>| {
        let _ = state.ws_broadcast.send(crate::models::WsMessage::WorkflowRunUpdated {
            run_id: run_id_for_ws.clone(),
            workflow_id: workflow_id_for_ws.clone(),
            status: format!("{:?}", status),
            step_index,
            total_steps: total_steps_for_ws,
            current_step,
        });
    };

    let db = state.db.clone();

    // Register a cancellation token keyed by the run id. The "⏹ Arrêter" UI
    // triggers this token via POST /api/workflows/.../runs/:run_id/cancel.
    // We check it between steps and during any long await — short-circuiting
    // to status = Cancelled. The CancelGuard auto-cleans on scope exit.
    let cancel_guard = crate::CancelGuard::insert(&state.cancel_registry, run.id.clone());
    let cancel_token = cancel_guard.token.clone();

    // Update run status to Running
    run.status = RunStatus::Running;
    let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
    let db2 = db.clone();
    db2.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap)).await?;

    // Resolve project + companion-repo context. Same pattern as the
    // audit pipeline (api/audit/full.rs:58-74): pre-format the
    // linked_repos + Kronn-projects-universe blocks ONCE here so every
    // Agent step in this run pays for the DB hit exactly once instead
    // of N times. The helper returns an empty string when there's
    // nothing to inject (no project bound, no companions registered).
    let project_path = if let Some(ref pid) = workflow.project_id {
        let pid_clone = pid.clone();
        let db3 = db.clone();
        let project_opt = db3
            .with_conn(move |conn| crate::db::projects::get_project(conn, &pid_clone))
            .await?;
        project_opt.map(|p| p.path).unwrap_or_default()
    } else {
        String::new()
    };
    let agent_extra_context = crate::api::projects::compute_companion_context(
        &state,
        workflow.project_id.as_deref(),
    ).await;

    // 0.7.0 Phase 4 — detect resume: a non-empty step_results means
    // this is a continuation from a Gate pause (or a future restart-
    // recovery). On resume we reattach to the existing worktree
    // instead of creating a new one — both because it already holds
    // the artifacts the operator just inspected, and because creating
    // a second worktree on the same branch would fail.
    let is_resume = !run.step_results.is_empty();

    // Create or attach workspace (if we have a project path)
    let workspace = if !project_path.is_empty() {
        let repo_path = crate::core::scanner::resolve_host_path(&project_path);
        if repo_path.exists() {
            let hooks = workflow.workspace_config.as_ref().map(|c| c.hooks.clone());
            if let Some(inh) = inherited_workspace.as_ref().map(std::path::PathBuf::from).filter(|p| p.exists()) {
                // Phase 2 — sub-workflow child shares the parent's worktree.
                // Attach to the parent's branch so commits land there; the
                // child never creates/destroys this tree (parent owns it).
                run.workspace_path = Some(inh.to_string_lossy().to_string());
                Some(Workspace::attach(inh, repo_path, &workflow.name, &run.id, hooks))
            } else if is_resume {
                match run.workspace_path.as_ref().map(std::path::PathBuf::from) {
                    Some(path) if path.exists() => {
                        Some(Workspace::attach(path, repo_path, &workflow.name, &run.id, hooks))
                    }
                    _ => None, // resume without worktree (or worktree gone) — run in main tree
                }
            } else {
                match Workspace::create(&repo_path, &workflow.name, &run.id, hooks).await {
                    Ok(ws) => {
                        run.workspace_path = Some(ws.path.to_string_lossy().to_string());
                        Some(ws)
                    }
                    Err(e) => {
                        // #8 — worktree fallback is DANGEROUS for code-pushing
                        // workflows: silently running agents that `git push` /
                        // mutate files in the developer's MAIN checkout. When
                        // the workflow declares `require_isolation`, abort the
                        // run instead of falling back (mirror the preflight
                        // failure pattern). Read-only workflows keep the legacy
                        // warn-and-continue behaviour.
                        let requires_isolation = workflow.workspace_config
                            .as_ref()
                            .map(|c| c.require_isolation)
                            .unwrap_or(false);
                        if requires_isolation {
                            let msg = format!(
                                "Workflow requires an isolated git worktree but it could not be created: {}. \
                                 Refusing to run in the main checkout — this workflow pushes/mutates code. \
                                 Check the repo (is it a clean git repo? disk space?), or clear `require_isolation` \
                                 to allow main-tree runs.",
                                e
                            );
                            run.status = RunStatus::Failed;
                            run.step_results.push(StepResult {
                                step_name: "__workspace__".to_string(),
                                status: RunStatus::Failed,
                                output: msg.clone(),
                                tokens_used: 0,
                                duration_ms: 0,
                                started_at: None,
                                condition_result: None,
                                envelope_detected: None,
                                step_kind: Some("Preflight".into()),
                                step_api_plugin_slug: None,
                                step_api_endpoint_path: None,
                                is_rollback: false,
                                child_run_id: None,
                                step_agent: None,
                                step_model: None,
                            });
                            let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
                            let db_w = db.clone();
                            db_w.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap)).await?;
                            emit(RunEvent::RunError { error: msg }).await;
                            return Ok(());
                        }
                        tracing::warn!("Failed to create worktree, running in main tree: {}", e);
                        None
                    }
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Determine working directory
    let work_dir = workspace.as_ref()
        .map(|ws| ws.path.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            let resolved = crate::core::scanner::resolve_host_path(&project_path);
            if resolved.exists() {
                resolved.to_string_lossy().to_string()
            } else {
                project_path.clone()
            }
        });

    // Run before_run hook — but not on resume, the hook already fired
    // before the pause and re-firing it would re-run setup actions
    // (npm install, env preparation, etc.) the operator didn't ask for.
    if !is_resume && !is_inherited_workspace {
        if let Some(ref ws) = workspace {
            let _ = ws.before_run().await;
        }
    }

    // 0.7.1 — scan sensitive files once at run start so the docs/ write
    // filter can flag agent-written content that overlaps with .env/.pem/
    // .ssh/credentials. Cheap (only reads small files), happens once per
    // run, results reused across every Agent step audit. Empty when the
    // run has no project (Notify-only / ApiCall-only workflows).
    let sensitive_substrings = std::sync::Arc::new(
        if work_dir.is_empty() {
            crate::core::docs_write_filter::SensitiveSubstrings::new()
        } else {
            crate::core::docs_write_filter::scan_sensitive_files(std::path::Path::new(&work_dir))
        }
    );

    // Build template context from trigger context
    let mut ctx = TemplateContext::new();
    if let Some(ref trigger_ctx) = run.trigger_context {
        inject_trigger_context(&mut ctx, trigger_ctx);
    }
    // 0.7.0 Phase 3 — pre-seed every declared artifact to "" so a step
    // referencing `{{artifacts.review}}` on round 1 (before any step
    // wrote it) renders cleanly rather than leaving the literal
    // `{{artifacts.review}}` placeholder in the prompt. Steps that
    // produce the artifact later overwrite the seed via `set_step_output`.
    for name in workflow.artifacts.keys() {
        ctx.set(format!("artifacts.{}", name), "");
    }
    // 0.7.0 Phase 4 — Gate resume: replay every prior step result into
    // the template context so downstream steps see `{{steps.X.summary}}`
    // and `{{artifacts.Y}}` exactly as if the run had never paused.
    // Fresh runs have no prior step_results, so this is a no-op for them.
    for prior in &run.step_results {
        ctx.set_step_output(&prior.step_name, &prior.output);
    }
    // 0.7.0 Phase 6 — seed durable state from the run row. On a fresh
    // run this is empty (no-op); on resume / restart-recovery it carries
    // counters and verdicts the agent wrote in prior iterations so the
    // first step after the pause sees them through `{{state.<k>}}`.
    ctx.seed_state(&run.state);

    // Pre-flight: validate every Agent step's agent is actually installed
    // (or runtime-available) before we start. Without this the run fails
    // mid-execution at the spawn site with a confusing subprocess error
    // ("vibe: command not found" or worse), wasting work that's been done
    // by earlier steps. Mirrors the same guard added to the multi-agent
    // debate orchestrator. We skip this on resume — the workflow is
    // already partway through and the user may have intentionally
    // uninstalled an agent that's only used in skipped branches.
    if !is_resume {
        let agent_steps: Vec<&str> = workflow.steps.iter()
            .filter(|s| matches!(s.step_type, StepType::Agent))
            .map(|s| s.name.as_str())
            .collect();
        if !agent_steps.is_empty() {
            let detections = crate::agents::detect_all().await;
            let usable: Vec<crate::models::AgentType> = detections.iter()
                .filter(|d| (d.installed || d.runtime_available) && d.enabled)
                .map(|d| d.agent_type.clone())
                .collect();
            let mut missing: Vec<(String, String)> = Vec::new();
            for step in workflow.steps.iter().filter(|s| matches!(s.step_type, StepType::Agent)) {
                let ok = usable.iter().any(|u| std::mem::discriminant(u) == std::mem::discriminant(&step.agent));
                if !ok {
                    missing.push((step.name.clone(), format!("{:?}", step.agent)));
                }
            }
            if !missing.is_empty() {
                let msg = format!(
                    "Workflow refuses to start — agent(s) not installed/enabled: {}. Install or enable them in Config before running this workflow.",
                    missing.iter().map(|(n, a)| format!("'{}' needs {}", n, a)).collect::<Vec<_>>().join(", ")
                );
                run.status = RunStatus::Failed;
                run.step_results.push(StepResult {
                    step_name: "__preflight__".to_string(),
                    status: RunStatus::Failed,
                    output: msg.clone(),
                    tokens_used: 0,
                    duration_ms: 0,
                    started_at: None,
            condition_result: None,
                    envelope_detected: None,
                    step_kind: Some("Preflight".into()),
                    step_api_plugin_slug: None,
                    step_api_endpoint_path: None,
                    is_rollback: false,
                    child_run_id: None,
                    step_agent: None,
                    step_model: None,
                });
                let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
                let db_p = db.clone();
                db_p.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap)).await?;
                emit(RunEvent::RunError { error: msg }).await;
                return Ok(());
            }
        }
    }

    // Execute steps sequentially
    let mut all_success = true;
    let mut cancelled_by_user = false;
    let mut stopped_by_guard = false;
    let mut paused_for_approval = false;
    let mut step_idx = next_step_index_for_resume(&workflow.steps, &run.step_results);
    let total_steps = workflow.steps.len();
    let max_total_iterations = max_iterations_for(total_steps); // safeguard against infinite Goto loops
    let mut iteration_count = 0;

    // 0.7.0 — execution guards. Resolved once at run start so subsequent
    // edits to the workflow (loosen the timeout, raise max calls) don't
    // affect a running instance — the contract is "what was set when you
    // hit Run". Plain backend defaults apply when `workflow.guards` is None.
    let resolved_guards = WorkflowGuards::resolve_optional(workflow.guards.as_ref());
    // Phase 1b-ii — shared LLM-calls budget. A child inherits the parent
    // tree's (same counter + cap); a top-level run gets a fresh one capped at
    // its own resolved limit. The whole tree is then governed by ONE quota.
    let budget = shared_budget
        .unwrap_or_else(|| SharedBudget::root(resolved_guards.max_llm_calls));
    let mut step_revisits: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    // 0.7.0 Phase 6 — per-Goto-edge counter. Keyed by `(source, target)`
    // so two different loops in the same workflow have independent
    // limits. Falls through (continues past the loop) when the cap is
    // reached on a given edge.
    let mut goto_fires: std::collections::HashMap<(String, String), u32> =
        std::collections::HashMap::new();

    while step_idx < workflow.steps.len() {
        // Cancellation check — fires when the user clicked "⏹ Arrêter" and
        // the /cancel endpoint triggered our token. We break BEFORE executing
        // the next step so a runaway linear run doesn't keep burning tokens.
        // Note: a step already in flight won't stop here — agent steps have
        // their own disc-level token checked inside `make_agent_stream`.
        if cancel_token.is_cancelled() {
            tracing::info!("Workflow run {} cancelled by user before step {}", run.id, step_idx);
            cancelled_by_user = true;
            run.step_results.push(StepResult {
                step_name: "__cancelled_by_user__".to_string(),
                status: RunStatus::Cancelled,
                output: "Workflow run cancelled by user".to_string(),
                tokens_used: 0,
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
            });
            all_success = false;
            break;
        }
        iteration_count += 1;
        if iteration_count > max_total_iterations {
            tracing::error!("Workflow run exceeded {} iterations — aborting to prevent infinite loop", max_total_iterations);
            all_success = false;
            run.step_results.push(StepResult {
                step_name: "__safeguard_abort__".to_string(),
                status: RunStatus::Failed,
                output: format!("Workflow aborted: exceeded {} total step iterations (possible infinite Goto loop)", max_total_iterations),
                tokens_used: 0,
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
            });
            break;
        }

        // ── WorkflowGuards check (0.7.0) ────────────────────────────────
        // Walls-clock timeout: elapsed since `run.started_at` (saved when
        // the run was queued). Resume after a daemon restart counts the
        // downtime — the deadline is absolute, not "active time". The
        // alternative ("pause clock during AwaitingApproval/restart") was
        // considered and rejected: it leaks complex state across reboots
        // and surprises users who set "60 minutes" expecting wall-clock.
        let elapsed_secs = (Utc::now() - run.started_at).num_seconds().max(0) as u64;
        if elapsed_secs >= resolved_guards.timeout_seconds {
            tracing::warn!(target: "kronn::workflow_guard",
                run_id = %run.id, kind = "Timeout",
                threshold_secs = resolved_guards.timeout_seconds, actual_secs = elapsed_secs,
                "Workflow run stopped by Timeout guard");
            emit(RunEvent::GuardTriggered {
                kind: GuardKind::Timeout,
                threshold: resolved_guards.timeout_seconds,
                actual: elapsed_secs,
            }).await;
            run.step_results.push(StepResult {
                step_name: "__guard_timeout__".to_string(),
                status: RunStatus::StoppedByGuard,
                output: format!("Stopped by Timeout guard: {}s elapsed (limit {}s)", elapsed_secs, resolved_guards.timeout_seconds),
                tokens_used: 0,
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
            });
            stopped_by_guard = true;
            break;
        }

        // LLM-calls quota: counts Agent steps as 1 each. BatchQuickPrompt
        // counts as N **AFTER** the fan-out resolves (the executor returns
        // the actual child count); this check uses the cumulative count
        // accumulated from previous iterations. ApiCall and Notify count
        // as 0 because they don't spend tokens.
        if budget.llm_calls() >= budget.max_llm_calls() {
            tracing::warn!(target: "kronn::workflow_guard",
                run_id = %run.id, kind = "MaxLlmCalls",
                threshold = budget.max_llm_calls(), actual = budget.llm_calls(),
                "Workflow run stopped by MaxLlmCalls guard (shared budget — counts the whole sub-workflow tree)");
            emit(RunEvent::GuardTriggered {
                kind: GuardKind::MaxLlmCalls,
                threshold: budget.max_llm_calls() as u64,
                actual: budget.llm_calls() as u64,
            }).await;
            run.step_results.push(StepResult {
                step_name: "__guard_max_llm_calls__".to_string(),
                status: RunStatus::StoppedByGuard,
                output: format!("Stopped by MaxLlmCalls guard: {} LLM calls (limit {})", budget.llm_calls(), budget.max_llm_calls()),
                tokens_used: 0,
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
            });
            stopped_by_guard = true;
            break;
        }

        // Loop detection: count visits PER step. A 100-step linear
        // workflow doesn't trigger this (each step visited once); a
        // workflow that Goto-loops on the same step does. Senior Dev
        // explicitly rejected "total iter count" as fragile.
        let visit_count = {
            let name = &workflow.steps[step_idx].name;
            let n = step_revisits.entry(name.clone()).or_insert(0);
            *n += 1;
            *n
        };
        // 0.7.0 Phase 6 — expose `{{iter.<step_name>}}` in templates so
        // a step can react to its own re-execution (e.g. "first pass:
        // generate; subsequent: refine"). Updated EVERY iteration so
        // looped Goto-back patterns see the right counter.
        ctx.set(
            format!("iter.{}", workflow.steps[step_idx].name),
            visit_count.to_string(),
        );
        if visit_count > resolved_guards.loop_detection_max_revisits {
            let step_name = workflow.steps[step_idx].name.clone();
            tracing::warn!(target: "kronn::workflow_guard",
                run_id = %run.id, kind = "LoopDetection", step = %step_name,
                threshold = resolved_guards.loop_detection_max_revisits, actual = visit_count,
                "Workflow run stopped by LoopDetection guard");
            emit(RunEvent::GuardTriggered {
                kind: GuardKind::LoopDetection { step_name: step_name.clone() },
                threshold: resolved_guards.loop_detection_max_revisits as u64,
                actual: visit_count as u64,
            }).await;
            run.step_results.push(StepResult {
                step_name: "__guard_loop_detection__".to_string(),
                status: RunStatus::StoppedByGuard,
                output: format!("Stopped by LoopDetection guard: step '{}' visited {} times (limit {})", step_name, visit_count, resolved_guards.loop_detection_max_revisits),
                tokens_used: 0,
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
            });
            stopped_by_guard = true;
            break;
        }

        let step = &workflow.steps[step_idx];
        tracing::info!("Executing step {}/{}: '{}'", step_idx + 1, total_steps, step.name);

        emit(RunEvent::StepStart {
            step_name: step.name.clone(),
            step_index: step_idx,
            total_steps,
        }).await;
        // 0.8.2 — cross-tab live update
        broadcast_run_state(&run.status, step_idx as i32, Some(step.name.clone()));

        // Build the step's executor as a single future, then race it
        // against `cancel_token.cancelled()`. When the user clicks Stop
        // mid-step, the cancel branch wins, the executor future is
        // dropped, and the kill_on_drop chain takes over:
        //   - Agent  → AgentProcess.child drops → SIGKILL
        //   - Exec   → tokio Command future drops → SIGKILL
        //   - HTTP   → reqwest cancels on drop (ApiCall, BatchApiCall, Notify)
        //   - BatchQuickPrompt → child agents kill_on_drop the same way
        // See runner.rs:201-209 for the *between-step* cancel check;
        // this is the *in-flight* counterpart.
        let step_start = std::time::Instant::now();
        // 2026-06-11 Phase 1b — sub-workflow recursion context, extracted as
        // owned values BEFORE the `async` block so the SubWorkflow arm
        // doesn't borrow `run`/`workflow` across the await. `__subwf_depth__`
        // is 0 for a top-level run, N for a run that is itself a child.
        let sub_parent_run_id = run.id.clone();
        let sub_current_depth = run
            .trigger_context
            .as_ref()
            .and_then(|t| t.get("__subwf_depth__"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        // Phase 1b-ii — clone the shared budget for the SubWorkflow arm so the
        // child inherits the tree-wide quota (same counter + cap).
        let sub_budget = budget.clone();
        // Phase 2 — hand the parent's worktree path to the child so it commits
        // to the parent's branch (a later parent step like `create_pr` then
        // sees the child's work). `None` when the parent runs in the main tree.
        let sub_parent_workspace = run.workspace_path.clone();
        let step_future = async {
            match step.step_type {
                StepType::BatchQuickPrompt => {
                    // Phase 2 batch workflows — fan out a Quick Prompt over items
                    // from a previous step's output, then optionally wait for all
                    // children to finish before moving on.
                    super::batch_step::execute_batch_quick_prompt_step(
                        step,
                        &run.id,
                        state.clone(),
                        &ctx,
                    ).await
                }
                StepType::Notify => {
                    // Direct HTTP webhook — zero agent tokens. Used as a workflow
                    // finalizer or mechanical data step (post to Slack, create
                    // ticket, etc.). Shipped 0.3.5.
                    super::notify_step::execute_notify_step(step, &ctx).await
                }
                StepType::ApiCall => {
                    // Désagentification: direct HTTP call from the engine.
                    // Uses `SecurityPolicy::production()` — localhost URLs fail
                    // here, as intended. Rate-limiting lands in P0.5b. Project
                    // id comes from the parent workflow, not the run row, since
                    // `WorkflowRun` doesn't carry it (only `workflow_id`).
                    //
                    // 0.8.6 (#59) — stamp source=workflow + run_id on the
                    // api_call_logs row so the audit table can filter by
                    // run.
                    super::api_call_executor::execute_api_call_step_with_db_as(
                        step,
                        workflow.project_id.as_deref(),
                        &state,
                        &ctx,
                        super::api_call_executor::SecurityPolicy::production(),
                        super::api_call_executor::ApiCallLogContext::workflow_for_run(run.id.clone()),
                    ).await
                }
                StepType::Agent => {
                    // 0.7+ — hydrate optional QuickPrompt reference. Le helper
                    // est no-op si `step.quick_prompt_id` est None. Sinon il
                    // injecte prompt_template / tier / skill_ids depuis le QP
                    // dans une copie locale du step (per-field override : le
                    // step gagne quand non-vide).
                    let mut hydrated = step.clone();
                    if let Err(e) = super::quick_prompt_hydrate::hydrate_step_from_quick_prompt(
                        &mut hydrated,
                        &state.db,
                    )
                    .await
                    {
                        StepOutcome {
                            result: StepResult {
                                step_name: step.name.clone(),
                                status: RunStatus::Failed,
                                output: e,
                                tokens_used: 0,
                                duration_ms: step_start.elapsed().as_millis() as u64,
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
                            },
                            condition_action: None,
                        }
                    } else {
                    // Gate feedback, runtime injection (global, approach B):
                    // a human Gate "request changes" persisted its comment to
                    // run.state["last_human_feedback"] before the truncate
                    // (see decide_run). Prepend it to THIS step's prompt (the
                    // re-run target) and consume it once — so every preset AND
                    // every hand-built workflow surfaces the feedback with NO
                    // `{{state.last_human_feedback}}` placeholder needed.
                    inject_and_consume_gate_feedback(&mut hydrated.prompt_template, &mut run.state);
                    let step = &hydrated;
                    let full_access = agents_config.full_access_for(&step.agent);
                    // Live-progress wiring — without this the user gets a
                    // "step is running" pulse with no visible content until
                    // the step finishes (typical Agent step = 30-120s of
                    // silence). Spawn a forwarder that pumps each chunk from
                    // the agent's stdout into the SSE channel as a
                    // `StepProgress` event. Bounded buffer (256) keeps a
                    // slow client from back-pressuring the agent's stdout.
                    let (progress_tx, mut progress_rx) =
                        tokio::sync::mpsc::channel::<String>(256);
                    let forwarder_tx = events_tx.clone();
                    let forwarder = tokio::spawn(async move {
                        while let Some(text) = progress_rx.recv().await {
                            if let Some(ref tx) = forwarder_tx {
                                let _ = tx.send(RunEvent::StepProgress { text }).await;
                            }
                        }
                    });
                    // 0.8.2 — Stamp the wall-clock step start so the
                    // frontend's live-elapsed counter reads an authoritative
                    // value instead of estimating via `runStart + sum of
                    // prior durations`. The estimate diverges from reality
                    // after Goto loops (re-runs of the same step), agent
                    // retries, and any scheduling gap between steps —
                    // and the WorkflowDetail live-mini-dashboard then
                    // disagrees with RunDetail's `LiveStepStatus`.
                    let step_started_at = Utc::now();
                    let outcome = execute_step(
                        step,
                        &project_path,
                        &work_dir,
                        tokens_config,
                        full_access,
                        &ctx,
                        &agent_extra_context,
                        Some(progress_tx),
                        Some(&agents_config.model_tiers),
                    ).await;
                    let mut outcome = outcome;
                    if outcome.result.started_at.is_none() {
                        outcome.result.started_at = Some(step_started_at);
                    }
                    // execute_step took ownership of progress_tx and dropped
                    // it on return → the forwarder's recv() now yields None
                    // and the loop exits naturally. AWAIT it (don't abort —
                    // abort would kill the task before it could drain the
                    // tail of the channel buffer, losing the last few chunks
                    // of the step's output to the SSE stream).
                    let _ = forwarder.await;
                    // 0.7.1 — anti-secret audit on docs/ writes the agent
                    // produced during this step. Soft-reject : we revert
                    // via `git checkout` and log; the step itself stays
                    // Success unless the agent's code write itself failed.
                    // Only fires when there's a real worktree (skips
                    // ApiCall-only / Notify-only workflows where work_dir
                    // is empty or unmounted).
                    if !work_dir.is_empty() {
                        let rejections = crate::core::docs_write_filter::audit_docs_writes(
                            std::path::Path::new(&work_dir),
                            sensitive_substrings.as_ref(),
                        ).await;
                        if !rejections.is_empty() {
                            for (path, reason) in &rejections {
                                tracing::warn!(
                                    target: "kronn::docs_write_filter",
                                    "Step '{}': reverted docs write {} — {}",
                                    step.name, path, reason.explain()
                                );
                            }
                            // Surface count in run state for UI visibility.
                            let count_key = format!("docs_write_rejections.{}", step.name);
                            run.state.insert(count_key, rejections.len().to_string());
                        }
                    }
                    outcome
                    }
                }
                StepType::Gate => {
                    // 0.7.0 Phase 4 — human-in-the-loop pause. Zero tokens —
                    // the engine builds a `WaitingApproval` outcome with the
                    // rendered gate message embedded in the StepResult output.
                    // The run loop breaks below when it sees this status, and
                    // the operator's decision (POST /runs/:id/decide) calls
                    // `resume_run` to continue from the next step.
                    //
                    // 0.8.6 (#25) — checkpoint commit. When the step has
                    // `gate_checkpoint_before: Some(true)`, snapshot the
                    // working tree FIRST so a future "Request Changes" Goto
                    // can `git reset --hard` to this SHA before re-running
                    // the target. Skipped silently in Isolated worktree
                    // mode (the worktree manages its own branch lifecycle).
                    if step.gate_checkpoint_before.unwrap_or(false) {
                        // Isolated workspace = the run has its own
                        // worktree path. Skip checkpoint there — the
                        // worktree manages its own branch lifecycle.
                        let is_isolated = run.workspace_path.is_some();
                        if is_isolated {
                            tracing::info!(
                                run_id = %run.id,
                                step = %step.name,
                                "gate_checkpoint_before skipped — workflow uses Isolated worktree mode",
                            );
                        } else if let Some(pid) = workflow.project_id.as_ref() {
                            let project_path_opt = state.db.with_conn({
                                let pid2 = pid.clone();
                                move |conn| crate::db::projects::get_project(conn, &pid2)
                            }).await.ok().flatten();
                            if let Some(proj) = project_path_opt {
                                let ckp = super::gate_checkpoint::commit_checkpoint(
                                    std::path::Path::new(&proj.path),
                                    &step.name,
                                    &run.id,
                                );
                                match ckp {
                                    super::gate_checkpoint::CheckpointOutcome::Committed { sha } => {
                                        let key = format!("{}{}", super::gate_checkpoint::CHECKPOINT_STATE_PREFIX, step.name);
                                        run.state.insert(key, sha.clone());
                                        tracing::info!(
                                            run_id = %run.id,
                                            step = %step.name,
                                            sha = %sha,
                                            "gate_checkpoint_before committed",
                                        );
                                    }
                                    super::gate_checkpoint::CheckpointOutcome::NotAGitRepo => {
                                        tracing::warn!(
                                            run_id = %run.id,
                                            step = %step.name,
                                            project_path = %proj.path,
                                            "gate_checkpoint_before requested but project_path is not a git repo — skipping",
                                        );
                                    }
                                    super::gate_checkpoint::CheckpointOutcome::StagedChangesPresent => {
                                        tracing::warn!(
                                            run_id = %run.id,
                                            step = %step.name,
                                            "gate_checkpoint_before refused — index has staged changes (user WIP)",
                                        );
                                    }
                                    super::gate_checkpoint::CheckpointOutcome::GitCommandFailed { stderr } => {
                                        tracing::warn!(
                                            run_id = %run.id,
                                            step = %step.name,
                                            stderr = %stderr,
                                            "gate_checkpoint_before commit failed — continuing without checkpoint",
                                        );
                                    }
                                }
                            }
                        }
                    }
                    super::gate_step::execute_gate_step(step, &ctx)
                }
                StepType::Exec => {
                    // 0.7.0 Phase 5 — direct shell execution. Zero tokens.
                    // Allowlist-gated, never-shell, args-as-literal-argv.
                    // The run-time guard mirrors the save-time validator
                    // for defence in depth (a workflow loaded from a
                    // hand-edited JSON could carry a stale Exec step).
                    super::exec_step::execute_exec_step(
                        step,
                        &workflow.exec_allowlist,
                        &work_dir,
                        &ctx,
                    ).await
                }
                StepType::BatchApiCall => {
                    // 0.6.0 — mechanical fan-out of an API call over a list of
                    // items. Zero tokens, parallel HTTP, idempotency-by-prompt
                    // moved to idempotency-by-construction. See
                    // batch_apicall_step.rs for the executor.
                    super::batch_apicall_step::execute_batch_apicall_step(
                        step,
                        workflow.project_id.as_deref(),
                        &state,
                        &ctx,
                        super::api_call_executor::ApiCallLogContext::workflow_for_run(run.id.clone()),
                    ).await
                }
                StepType::JsonData => {
                    // 0.7+ — déterministe data source. Émet le payload
                    // littéral du step dans une envelope Structured. Zéro
                    // token, zéro réseau. Cf. json_data_step.rs.
                    super::json_data_step::execute_json_data_step(step).await
                }
                StepType::SubWorkflow => {
                    // Phase 1b-i — re-entrant: runs the target workflow as a
                    // child run (Box::pin recursion inside the executor),
                    // bounded by depth. Cf. sub_workflow_step.rs.
                    super::sub_workflow_step::execute_sub_workflow_step(
                        &state,
                        &sub_parent_run_id,
                        sub_current_depth,
                        step,
                        tokens_config,
                        agents_config,
                        sub_budget.clone(),
                        sub_parent_workspace.clone(),
                    ).await
                }
            }
        };

        let mut outcome: StepOutcome = tokio::select! {
            o = step_future => o,
            _ = cancel_token.cancelled() => {
                tracing::info!(
                    "Workflow run {} cancelled mid-step '{}' — dropping in-flight executor",
                    run.id, step.name
                );
                cancelled_by_user = true;
                StepOutcome {
                    result: StepResult {
                        step_name: step.name.clone(),
                        status: RunStatus::Cancelled,
                        output: format!("Step '{}' cancelled by user mid-flight.", step.name),
                        tokens_used: 0,
                        duration_ms: step_start.elapsed().as_millis() as u64,
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
                    },
                    condition_action: None,
                }
            }
        };

        // Record step output for template chaining (also extracts any
        // `---ARTIFACT:<name>---` blocks into `{{artifacts.<name>}}`).
        ctx.set_step_output(&step.name, &outcome.result.output);

        // 0.7.0 Phase 3 — persist declared artifacts to disk so they
        // survive past the run (committable when in a worktree,
        // inspectable in the run-detail UI, reusable across reboots if
        // the workspace is reused). Undeclared artifacts in the agent's
        // output are kept in `ctx` (template visibility) but NOT
        // persisted — declaring is the contract that says "this matters
        // enough to write a file for".
        let extracted = super::template::extract_artifacts(&outcome.result.output);
        if !extracted.is_empty() {
            persist_declared_artifacts(workflow, &extracted, std::path::Path::new(&work_dir));
        }

        // 0.7.0 Phase 6 — durable state. `set_step_output` already
        // pushed `state.<k>` into the template ctx for the immediate
        // next step; here we mirror those entries onto `run.state`
        // so they're persisted to the DB on the upcoming progress
        // snapshot and survive Gate pauses / daemon restarts.
        for (k, v) in super::template::extract_state(&outcome.result.output) {
            run.state.insert(k, v);
        }

        // Accumulate tokens
        run.tokens_used += outcome.result.tokens_used;

        // 0.7.0 — count this step toward the LLM-calls quota. Only step
        // types that spawn an agent are counted: BatchQuickPrompt counts
        // as the number of children actually spawned (read from the
        // outcome's batch metadata when present, else 1 — conservative).
        // ApiCall and Notify cost zero by design.
        match step.step_type {
            StepType::Agent => {
                budget.add_llm_calls(1);
            }
            StepType::BatchQuickPrompt => {
                // Conservative count: each batch step counts as 1 LLM call
                // toward the quota (rather than N children). Senior Dev's
                // recommendation was N post-fan-out, but `StepResult`
                // doesn't carry the spawned-children count today, and
                // wiring it through would touch every batch executor —
                // out of scope for the Phase-1 guards. The fan-out cap
                // (`batch_max_items`) already limits the per-step blast
                // radius. Tracked separately as future enhancement.
                budget.add_llm_calls(1);
            }
            StepType::ApiCall
            | StepType::Notify
            | StepType::Gate
            | StepType::Exec
            | StepType::BatchApiCall
            | StepType::JsonData => {}
            // SubWorkflow itself spawns no LLM directly; its child run's
            // Agent steps consume LLM calls. Phase 1b aggregates the child's
            // count into the SHARED budget so the parent quota isn't bypassed
            // (spec §4.2). Stub today → zero direct cost.
            StepType::SubWorkflow => {}
        }

        let step_failed = outcome.result.status == RunStatus::Failed;
        // Captured BEFORE `outcome.result` is moved into `step_results`
        // below — used by the Stop arm to give the run an honest verdict
        // when an agent step exits 0 but declared `[SIGNAL: ERROR]` (audit
        // P1). Exact trailing-line match, not substring (a body excerpt
        // quoting the word ERROR must not count).
        let step_declared_error = outcome
            .result
            .output
            .lines()
            .rev()
            .take(5)
            .any(|l| l.trim().ends_with("[SIGNAL: ERROR]"));
        // 0.7.0 Phase 4 — Gate produced WaitingApproval. Break out of
        // the loop AFTER recording the StepResult so the operator sees
        // the rendered message on the run-detail page. The run is
        // resumed via `resume_run` once a decision arrives.
        let paused_here = outcome.result.status == RunStatus::WaitingApproval;

        // Snapshot the step's "what was actually used here" metadata
        // onto the result row. The user can edit the workflow between
        // runs (swap agent, retarget API plugin, change endpoint),
        // and without this snapshot the run history would silently
        // start describing the *current* config instead of what ran
        // in this run. Done here so every executor path benefits, not
        // per-executor.
        apply_step_snapshot(step, &mut outcome.result, Some(&agents_config.model_tiers));

        // Emit step done event
        emit(RunEvent::StepDone { step_result: outcome.result.clone() }).await;
        // 0.8.2 — cross-tab live update. status reflects the new state
        // (WaitingApproval if the step was a Gate, else still Running).
        // The current_step is cleared since this step is now finished.
        let post_step_status = if outcome.result.status == RunStatus::WaitingApproval {
            RunStatus::WaitingApproval
        } else {
            run.status.clone()
        };
        broadcast_run_state(&post_step_status, step_idx as i32, None);

        // 0.8.3 — Feasibility-Gated triage ingest. When the step that
        // just finished is a triage step (description marker OR schema
        // shape match) AND it succeeded with a valid envelope, parse
        // the manifest's decided/mocked/blocked lists and upsert one
        // row per entry into `agent_decisions`. Idempotent on
        // (run_id, decision_id) — a Goto retriage just rewrites the
        // same rows. Failures here log but never abort the run; the
        // manifest itself is the source of truth in StepResult.output.
        if outcome.result.status == RunStatus::Success
            && crate::workflows::triage::is_triage_step(
                step.description.as_deref(),
                &step.output_format,
            )
        {
            if let Some(env) = crate::workflows::template::extract_step_envelope(&outcome.result.output) {
                if let Ok(mut manifest) = serde_json::from_str::<serde_json::Value>(&env.data_json) {
                    // 2026-06-12 — incremental re-triage: a round ≥ 2 may emit
                    // only the items the plan review flagged, plus
                    // `unchanged: ["<id>", …]`. Hydrate those ids from the
                    // previous round's engine-written `.kronn/manifest.json`,
                    // then rewrite the step output so EVERY downstream reader
                    // ({{steps.triage.data}}, the Gate, pr_draft) sees the
                    // FULL manifest. Decisions ingest + machine-file derive
                    // below run on the merged manifest.
                    let has_unchanged = manifest
                        .get("unchanged")
                        .and_then(|u| u.as_array())
                        .map(|a| !a.is_empty())
                        .unwrap_or(false);
                    if has_unchanged && !work_dir.is_empty() {
                        let prev_path = std::path::Path::new(&work_dir).join(".kronn/manifest.json");
                        let prev = std::fs::read_to_string(&prev_path)
                            .ok()
                            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
                        match prev {
                            Some(prev) => {
                                let (merged, n) = crate::workflows::triage::merge_unchanged_items(&manifest, &prev);
                                manifest = merged;
                                outcome.result.output = crate::workflows::triage::rewrite_envelope_data(
                                    &outcome.result.output,
                                    &manifest,
                                );
                                tracing::info!(
                                    "Triage incremental merge: {} unchanged item(s) hydrated from previous manifest",
                                    n
                                );
                            }
                            None => tracing::warn!(
                                "Triage emitted `unchanged` ids but no previous .kronn/manifest.json — manifest stays partial"
                            ),
                        }
                    }
                    let ticket_ref = run
                        .trigger_context
                        .as_ref()
                        .and_then(|tc| {
                            tc.get("issue")
                                .and_then(|i| i.get("key").or_else(|| i.get("number")))
                                .and_then(|v| v.as_str().map(String::from).or_else(|| v.as_i64().map(|n| n.to_string())))
                        });
                    let project_id = workflow.project_id.clone();
                    let decisions = crate::workflows::triage::manifest_to_decisions(
                        &manifest,
                        &run.id,
                        &workflow.id,
                        &step.name,
                        project_id.as_deref(),
                        ticket_ref.as_deref(),
                    );
                    let dec_count = decisions.len();
                    let db_ingest = db.clone();
                    let ingest_result = db_ingest
                        .with_conn(move |conn| {
                            for d in &decisions {
                                crate::db::agent_decisions::upsert(conn, d)?;
                            }
                            Ok::<_, anyhow::Error>(())
                        })
                        .await;
                    match ingest_result {
                        Ok(()) => tracing::info!(
                            "Triage ingest: {} decision row(s) persisted for run {}",
                            dec_count, run.id
                        ),
                        Err(e) => tracing::warn!(
                            "Triage ingest failed for run {}: {} — manifest stays in StepResult.output",
                            run.id, e
                        ),
                    }

                    // 2026-06-12 (critical fix #1) — derive the machine files
                    // (.kronn/tasks.json + decision_ids.txt + files_touched.txt)
                    // from the VALIDATED envelope instead of trusting the agent
                    // to hand-write consistent copies. The human gates the
                    // envelope; the fan-out + deterministic checks execute
                    // EXACTLY that data. Overwrites whatever the agent may
                    // have written (engine wins). Best-effort: failures log,
                    // never abort the run.
                    if !work_dir.is_empty() {
                        for (rel, content) in crate::workflows::triage::derive_machine_files(&manifest) {
                            let path = std::path::Path::new(&work_dir).join(&rel);
                            if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
                            match std::fs::write(&path, &content) {
                                Ok(()) => tracing::info!("Triage machine file derived: {} ({} bytes)", rel, content.len()),
                                Err(e) => tracing::warn!("Failed to write derived triage file {}: {}", rel, e),
                            }
                        }
                    }
                } else {
                    tracing::warn!(
                        "Triage step '{}' produced an envelope but data_json is not JSON; skipping ingest",
                        step.name
                    );
                }
            }
        }

        run.step_results.push(outcome.result);

        // Persist progress
        let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
        let db4 = db.clone();
        db4.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap)).await?;

        // Mid-step cancellation winner — `cancelled_by_user` was flipped
        // by the `tokio::select!` cancel branch above. We persist the
        // `Cancelled` step row first (so the UI sees what was killed),
        // then break out of the run loop here. Skipping the rollback
        // chain is intentional: a user-initiated stop shouldn't trigger
        // rollback semantics meant for failed-then-recover paths.
        if cancelled_by_user {
            all_success = false;
            break;
        }

        if step_failed {
            all_success = false;
            // 0.7.0+ — let `on_result` Goto/Skip override the short-circuit.
            // Without this, a `cargo test` exit≠0 would always tip into the
            // rollback chain even when the user explicitly declared a
            // recovery rule (`contains "ERROR"` → Goto implement). Stop is
            // the safe synonym for current behaviour, no-match keeps the
            // legacy break + rollback path. The condition_action was already
            // computed by the executor (Exec, ApiCall) — runner just honours
            // it. Agent steps don't reach here because they never set Failed
            // status when emitting a SIGNAL: Failed for them = a real crash.
            match outcome.condition_action {
                Some(ConditionAction::Goto { ref step_name, max_iterations }) => {
                    if let Some(target) = workflow.steps.iter().position(|s| s.name == *step_name) {
                        let edge = (step.name.clone(), step_name.clone());
                        let count = goto_fires.entry(edge.clone()).or_insert(0);
                        if let Some(cap) = max_iterations {
                            if *count >= cap {
                                tracing::info!(
                                    "Step '{}' (Failed) Goto '{}' reached max_iterations ({}) — falling through to rollback",
                                    step.name, step_name, cap
                                );
                                break;
                            }
                        }
                        *count += 1;
                        tracing::info!(
                            "Step '{}' (Failed) honoured on_result Goto '{}' (fire #{}) — skipping rollback",
                            step.name, step_name, *count
                        );
                        // Failed status stays in the StepResult (already emitted
                        // above), but we treat it as recoverable: jump to the
                        // target and clear the abort flag so the run keeps going.
                        all_success = true;
                        step_idx = target;
                        continue;
                    } else {
                        tracing::warn!(
                            "Step '{}' (Failed) Goto target '{}' not found — falling through to rollback",
                            step.name, step_name
                        );
                    }
                }
                Some(ConditionAction::Skip) => {
                    tracing::info!(
                        "Step '{}' (Failed) honoured on_result Skip — skipping next step, no rollback",
                        step.name
                    );
                    all_success = true;
                    step_idx += 2;
                    continue;
                }
                Some(ConditionAction::Stop) | None => {
                    // Stop is identical to no-match here: end the linear run.
                    // Whether on_failure rollback fires depends on `all_success`,
                    // which stays false → rollback chain runs. Same as before.
                }
            }
            break;
        }
        if paused_here {
            paused_for_approval = true;
            // 0.7.0 P1-1 — fire optional webhook to ping ops when the
            // run enters WaitingApproval. Best-effort: spawned in the
            // background, errors logged only, never blocks the run.
            // The URL is templated so `{{state.slack_url}}` etc work.
            if let Some(raw_url) = step.gate_notify_url.as_deref() {
                if let Ok(rendered_url) = ctx.render(raw_url) {
                    if !rendered_url.trim().is_empty() {
                        let payload = serde_json::json!({
                            "run_id": run.id,
                            "workflow_id": workflow.id,
                            "workflow_name": workflow.name,
                            "step_name": step.name,
                            "message": run.step_results
                                .last()
                                .map(|sr| sr.output.clone())
                                .unwrap_or_default(),
                        });
                        let url_clone = rendered_url.clone();
                        let run_id_for_log = run.id.clone();
                        tokio::spawn(async move {
                            let client = match reqwest::Client::builder()
                                .timeout(std::time::Duration::from_secs(10))
                                .build() {
                                Ok(c) => c,
                                Err(e) => {
                                    tracing::warn!(
                                        target: "kronn::gate_webhook",
                                        run_id = %run_id_for_log,
                                        "Gate webhook client build failed: {}", e);
                                    return;
                                }
                            };
                            match client.post(&url_clone).json(&payload).send().await {
                                Ok(resp) => tracing::info!(
                                    target: "kronn::gate_webhook",
                                    run_id = %run_id_for_log, status = resp.status().as_u16(),
                                    "Gate webhook fired"
                                ),
                                Err(e) => tracing::warn!(
                                    target: "kronn::gate_webhook",
                                    run_id = %run_id_for_log, url = %url_clone,
                                    "Gate webhook failed: {}", e
                                ),
                            }
                        });
                    }
                }
            }
            // 0.8.6 (#26) — opt-in auto-approve timer. If the gate
            // step has `gate_auto_approve_after_secs: Some(n)`,
            // spawn a background task that POSTs an Approve decision
            // to our own /decide endpoint after `n` seconds. Failures
            // are logged but don't block the run — the user can
            // still manually decide. Race-safe : the /decide
            // handler refuses to decide a run that's no longer in
            // WaitingApproval, so a human Approve before the timer
            // fires simply wins. Cancellation across backend
            // restart is out of scope for the MVP (the timer
            // resets ; user re-decides).
            if let Some(delay_secs) = step.gate_auto_approve_after_secs {
                if delay_secs > 0 {
                    let run_id_for_timer = run.id.clone();
                    let workflow_id_for_timer = workflow.id.clone();
                    let gate_name_for_timer = step.name.clone();
                    let port = std::env::var("KRONN_BACKEND_PORT")
                        .ok()
                        .and_then(|s| s.parse::<u16>().ok())
                        .unwrap_or(3140);
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(delay_secs as u64)).await;
                        let client = match reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(15))
                            .build() {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(
                                    target: "kronn::gate_auto_approve",
                                    run_id = %run_id_for_timer,
                                    "auto-approve client build failed: {}", e,
                                );
                                return;
                            }
                        };
                        // 2026-06-10 audit P0 — this self-POST was DOUBLY dead:
                        // the URL omitted the `{workflow_id}` segment (no such
                        // route → 404) AND the decision was sent as "Approve"
                        // while the handler matches lowercase "approve". Both
                        // failures were logged as "auto-approve POST sent"
                        // (HTTP status recorded but never checked) — every run
                        // with auto-approve stayed WaitingApproval forever.
                        // `gate_step` pins the decision to THIS gate so a timer
                        // armed on gate A can never approve a later gate B.
                        let url = format!(
                            "http://127.0.0.1:{}/api/workflows/{}/runs/{}/decide",
                            port, workflow_id_for_timer, run_id_for_timer,
                        );
                        let body = serde_json::json!({
                            "decision": "approve",
                            "comment": format!("[auto-approved after {delay_secs}s — no human action]"),
                            "gate_step": gate_name_for_timer,
                        });
                        match client.post(&url).json(&body).send().await {
                            Ok(resp) => tracing::info!(
                                target: "kronn::gate_auto_approve",
                                run_id = %run_id_for_timer, status = resp.status().as_u16(),
                                "auto-approve POST sent",
                            ),
                            Err(e) => tracing::warn!(
                                target: "kronn::gate_auto_approve",
                                run_id = %run_id_for_timer,
                                "auto-approve POST failed: {}", e,
                            ),
                        }
                    });
                }
            }
            break;
        }

        // Delay after step (if configured)
        if let Some(delay_secs) = step.delay_after_secs {
            if delay_secs > 0 {
                tracing::info!("Step '{}' — waiting {}s before next step", step.name, delay_secs);
                tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;
            }
        }

        // Handle condition actions
        match outcome.condition_action {
            Some(ConditionAction::Stop) => {
                // 2026-06-10 audit P1 — honest run verdict on agent-declared
                // errors. Agent steps exit 0 → Success even when they emit
                // `[SIGNAL: ERROR]`; pre-fix an `ERROR → Stop` rule broke the
                // loop with `all_success` still true → the FINAL run read
                // **Success** while the agent had declared a failure (the
                // create-pr preset relies on exactly this signal). We check
                // the trailing signal LINES exactly (not substring — body
                // excerpts quoting the word ERROR don't count): Stop + a
                // declared ERROR signal = the run did not succeed. A Stop on
                // a benign marker (e.g. "DONE_ALL") keeps Success semantics.
                if step_declared_error {
                    all_success = false;
                    tracing::info!(
                        "Step '{}' triggered Stop with a declared [SIGNAL: ERROR] — run marked Failed",
                        step.name
                    );
                } else {
                    tracing::info!("Step '{}' triggered Stop condition", step.name);
                }
                break;
            }
            Some(ConditionAction::Skip) => {
                tracing::info!("Step '{}' triggered Skip — skipping next step", step.name);
                step_idx += 2; // skip next step
                continue;
            }
            Some(ConditionAction::Goto { ref step_name, max_iterations }) => {
                if let Some(target) = workflow.steps.iter().position(|s| s.name == *step_name) {
                    // 0.7.0 Phase 6 — per-edge cap. Counter keyed by
                    // (source, target). When `max_iterations = Some(N)`,
                    // the Goto fires at most N times before the runner
                    // falls through. `None` keeps legacy behaviour
                    // (capped only by the workflow-level loop_detection
                    // guard).
                    let edge = (step.name.clone(), step_name.clone());
                    let count = goto_fires.entry(edge.clone()).or_insert(0);
                    if let Some(cap) = max_iterations {
                        if *count >= cap {
                            tracing::info!(
                                "Step '{}' Goto '{}' reached max_iterations ({}) — falling through",
                                step.name, step_name, cap
                            );
                            // No `continue` — fall through to step_idx += 1.
                            step_idx += 1;
                            continue;
                        }
                    }
                    *count += 1;
                    tracing::info!(
                        "Step '{}' triggered Goto '{}' (index {}, fire #{})",
                        step.name, step_name, target, *count
                    );
                    step_idx = target;
                    continue;
                } else {
                    tracing::warn!("Goto target '{}' not found, continuing normally", step_name);
                }
            }
            None => {}
        }

        step_idx += 1;
    }

    // Run after_run hook (skip when paused — the run isn't done yet).
    if !paused_for_approval {
        if let Some(ref ws) = workspace {
            let _ = ws.after_run().await;
        }
    }

    // Post-step actions (CreatePr, CommentIssue, etc.) are handled by MCP tools
    // injected into agent prompts — no separate actions phase needed.

    // Final status priority: user-cancellation > pause > guard-stop > failure > success.
    // Each one is a deliberate stop with its own UI treatment, so we
    // preserve them as distinct terminal states. WaitingApproval is NOT
    // terminal — `resume_run` flips it back to Running once a decision
    // arrives.
    run.status = if cancelled_by_user {
        RunStatus::Cancelled
    } else if paused_for_approval {
        RunStatus::WaitingApproval
    } else if stopped_by_guard {
        RunStatus::StoppedByGuard
    } else if all_success {
        RunStatus::Success
    } else {
        RunStatus::Failed
    };

    // 0.7.0 Phase 7 — rollback / compensation. Fires only on pure
    // `Failed` (not Cancelled, not StoppedByGuard, not WaitingApproval).
    // Compensation steps see the regular template context PLUS
    // `{{failed_step.*}}` so they can react to what specifically broke.
    // If a rollback step itself fails, subsequent rollback steps are
    // skipped — the run stays `Failed` regardless of rollback outcome.
    if run.status == RunStatus::Failed && !workflow.on_failure.is_empty() {
        let failed = run
            .step_results
            .iter()
            .rev()
            .find(|r| r.status == RunStatus::Failed)
            .cloned();
        let (failed_name, failed_output) = match failed {
            Some(r) => (r.step_name, r.output),
            None => ("(unknown)".to_string(), String::new()),
        };
        ctx.set("failed_step.name", failed_name.clone());
        ctx.set("failed_step.output", failed_output);

        tracing::info!(
            target: "kronn::workflow_rollback",
            run_id = %run.id, failed_step = %failed_name, rollback_count = workflow.on_failure.len(),
            "Workflow run failed — running rollback chain"
        );

        for rb_step in &workflow.on_failure {
            emit(RunEvent::StepStart {
                step_name: rb_step.name.clone(),
                step_index: run.step_results.len(),
                total_steps: run.step_results.len() + 1,
            }).await;

            let mut rb_outcome: StepOutcome = match rb_step.step_type {
                StepType::BatchQuickPrompt => {
                    super::batch_step::execute_batch_quick_prompt_step(
                        rb_step, &run.id, state.clone(), &ctx,
                    ).await
                }
                StepType::Notify => {
                    super::notify_step::execute_notify_step(rb_step, &ctx).await
                }
                StepType::ApiCall => {
                    // 0.8.6 (#59) — same audit stamping as the primary
                    // step dispatch above. This is the rollback / branch
                    // step replay path.
                    super::api_call_executor::execute_api_call_step_with_db_as(
                        rb_step,
                        workflow.project_id.as_deref(),
                        &state,
                        &ctx,
                        super::api_call_executor::SecurityPolicy::production(),
                        super::api_call_executor::ApiCallLogContext::workflow_for_run(run.id.clone()),
                    ).await
                }
                StepType::Agent => {
                    let full_access = agents_config.full_access_for(&rb_step.agent);
                    execute_step(
                        rb_step, &project_path, &work_dir, tokens_config,
                        full_access, &ctx, &agent_extra_context, None,
                        Some(&agents_config.model_tiers),
                    ).await
                }
                StepType::Gate => {
                    // Gate in rollback would deadlock the run on a Failed
                    // status that no resume path serves — explicitly
                    // unsupported. The wizard rejects this at save time
                    // (see validate_on_failure_steps).
                    super::gate_step::execute_gate_step(rb_step, &ctx)
                }
                StepType::Exec => {
                    // Exec in rollback is allowed (e.g. `make revert`
                    // as a compensation step). Same allowlist enforced.
                    super::exec_step::execute_exec_step(
                        rb_step,
                        &workflow.exec_allowlist,
                        &work_dir,
                        &ctx,
                    ).await
                }
                StepType::BatchApiCall => {
                    // BatchApiCall in rollback is meaningful for compensation
                    // (e.g. POST /issue/{key}/transitions = "Cancelled" over
                    // every ticket the failed run had created). Same plugin
                    // wiring as the linear path.
                    super::batch_apicall_step::execute_batch_apicall_step(
                        rb_step,
                        workflow.project_id.as_deref(),
                        &state,
                        &ctx,
                        super::api_call_executor::ApiCallLogContext::workflow_for_run(run.id.clone()),
                    ).await
                }
                StepType::JsonData => {
                    // Marginal en rollback (peu d'intérêt à émettre du
                    // JSON littéral comme step de compensation), mais on
                    // l'accepte pour rester cohérent avec le linear path.
                    super::json_data_step::execute_json_data_step(rb_step).await
                }
                StepType::SubWorkflow => {
                    // FORBIDDEN in on_failure (save-validated). Defence-in-depth
                    // for a hand-edited JSON: fail loudly, never recurse from
                    // a rollback step.
                    super::sub_workflow_step::forbidden_in_rollback(rb_step)
                }
            };

            ctx.set_step_output(&rb_step.name, &rb_outcome.result.output);
            for (k, v) in super::template::extract_state(&rb_outcome.result.output) {
                run.state.insert(k, v);
            }
            run.tokens_used += rb_outcome.result.tokens_used;
            apply_step_snapshot(rb_step, &mut rb_outcome.result, Some(&agents_config.model_tiers));
            // 2026-06-10 (audit P1) — mark this result as COMPENSATION so the
            // UI renders it under a dedicated rollback section. Pre-fix a
            // green rollback step right after the failed step read as "the
            // run continued past the failure".
            rb_outcome.result.is_rollback = true;

            let rb_failed = rb_outcome.result.status == RunStatus::Failed;
            emit(RunEvent::StepDone { step_result: rb_outcome.result.clone() }).await;
            run.step_results.push(rb_outcome.result);

            if rb_failed {
                tracing::warn!(
                    target: "kronn::workflow_rollback",
                    run_id = %run.id, step = %rb_step.name,
                    "Rollback step failed — skipping remaining compensation steps"
                );
                break;
            }
        }
    }

    if !paused_for_approval {
        run.finished_at = Some(Utc::now());
    }

    let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
    let db5 = db.clone();
    db5.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap)).await?;

    // Emit run done
    emit(RunEvent::RunDone { status: run.status.clone() }).await;
    // 0.8.2 — cross-tab final state flip so any open WorkflowDetail can
    // clear its live indicator without polling.
    broadcast_run_state(&run.status, run.step_results.len() as i32 - 1, None);

    // Cleanup workspace — but NOT if we just paused: the worktree
    // (with its uncommitted artifacts and hooks) must persist until
    // the operator decides to resume or reject. And NOT when this run
    // INHERITED the parent's worktree (Phase 2 sub-workflow child): the
    // parent owns that tree's lifecycle — cleaning it here would delete
    // the child's implementation before the parent's `create_pr` runs.
    if !paused_for_approval && !is_inherited_workspace {
        if let Some(ws) = workspace {
            match ws.cleanup().await {
                Ok(outcome) => {
                    if let Some(preserved) = outcome.preserved {
                        // Persist the preserved branch on the run row so the
                        // UI can surface it — without this, the operator only
                        // sees a tracing log line they'll never read.
                        run.produced_branches.push(crate::models::ProducedBranch {
                            branch_name: preserved.branch_name,
                            head_sha: preserved.head_sha,
                            ahead: preserved.ahead,
                            pushed_upstream: preserved.pushed_upstream,
                        });
                        let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
                        let db = state.db.clone();
                        let _ = db.with_conn(move |conn| {
                            crate::db::workflows::update_run_progress(conn, snap)
                        }).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("Workspace cleanup failed: {}", e);
                }
            }
        }
    }

    tracing::info!("Workflow run {} finished: {:?}", run.id, run.status);
    Ok(())
}

/// Build the maximum iteration safeguard for a given step count.
/// Formula: total_steps * 10 + 50.
pub(crate) fn max_iterations_for(total_steps: usize) -> usize {
    total_steps * 10 + 50
}

/// 0.7.0 Phase 4 — operator's decision on a paused (Gate) run.
///
/// Three outcomes:
///   - `Approve`: continue from the next step. The Gate's StepResult
///     status flips from `WaitingApproval` to `Success`; the optional
///     comment is appended to its output as a `> Décision:` footer.
///   - `RequestChanges`: jump back to a target step and re-run from
///     there. Default target: the step preceding the gate
///     (Auto-Dev `pause_pre_merge → goto: implement` pattern).
///     StepResults from the target onward are discarded so the run
///     replays cleanly.
///   - `Reject`: terminate the run with status `Failed`. No further
///     steps execute.
///
/// The `comment` field is operator-supplied free text — required by
/// the UX for `RequestChanges` (the agent needs to know what to fix)
/// but optional for the others.
#[derive(Debug, Clone)]
pub enum GateDecision {
    Approve { comment: Option<String> },
    RequestChanges { comment: String },
    Reject { comment: Option<String> },
}

/// Resume a paused workflow run after the operator has decided.
///
/// Mutates `run` in place — applies the decision to the trailing
/// `WaitingApproval` step result, sets up the runner state for
/// continuation (or terminal failure), and dispatches accordingly:
///   - Approve / RequestChanges → re-enters [`execute_run`] which now
///     starts from `step_results.len()` and skips workspace creation
///     thanks to the `is_resume` detection.
///   - Reject → marks the run as `Failed` and persists, no re-entry.
///
/// Caller (the API endpoint) is responsible for:
///   - loading the run + workflow from DB before calling
///   - persisting the final state via the runner's own progress writes
///   - holding the cancel-registry / events_tx if streaming is desired
///     (currently None — Phase 4 keeps the resume non-streamed; the
///     Phase 4b polish pass will reuse the SSE channel)
pub async fn resume_run(
    state: AppState,
    workflow: &Workflow,
    run: &mut WorkflowRun,
    decision: GateDecision,
    tokens_config: &TokensConfig,
    agents_config: &AgentsConfig,
    events_tx: Option<EventSender>,
) -> Result<()> {
    use anyhow::anyhow;

    let gate_step_idx = run
        .step_results
        .len()
        .checked_sub(1)
        .ok_or_else(|| anyhow!("Cannot resume run {}: no step results", run.id))?;

    {
        let last = &mut run.step_results[gate_step_idx];
        if last.status != RunStatus::WaitingApproval {
            return Err(anyhow!(
                "Cannot resume run {}: trailing step status is {:?}, expected WaitingApproval",
                run.id, last.status
            ));
        }
        // 0.8.2 — Rewrite the gate's `duration_ms` to reflect the actual
        // pause time (now - started_at) instead of the ~0ms executor
        // render time it held until approval. Without this, the front-end
        // live-elapsed counter on the next step inflated to include the
        // whole pause (`runStart + sum of completed durations` estimate
        // ignored the invisible gate pause). Falls back to the original
        // value when `started_at` is missing — old gate rows pre-dating
        // this field stay readable.
        if let Some(started_at) = last.started_at {
            let elapsed_ms = (Utc::now() - started_at).num_milliseconds().max(0) as u64;
            last.duration_ms = elapsed_ms;
        }
        match &decision {
            GateDecision::Approve { comment } => {
                last.status = RunStatus::Success;
                append_decision_footer(last, "Approuvé", comment.as_deref());
            }
            GateDecision::RequestChanges { comment } => {
                last.status = RunStatus::Success;
                append_decision_footer(last, "Changements demandés", Some(comment));
            }
            GateDecision::Reject { comment } => {
                last.status = RunStatus::Failed;
                append_decision_footer(last, "Rejeté", comment.as_deref());
            }
        }
    }

    // Persist the human's gate comment into `run.state` BEFORE the (later)
    // `truncate` drops the gate step result. The comment was only ever
    // appended to the gate result's footer via `append_decision_footer`; on a
    // RequestChanges the run truncates step_results down to the target step
    // (e.g. `analyze`/`implement`), which removes the gate result entirely —
    // so the re-run never saw the feedback. `run.state` survives truncate and
    // is seeded into the template ctx, so the target step can read
    // `{{state.last_human_feedback}}`. (Same mechanism as `state.last_review`
    // from the review→implement loop, but for the HUMAN gate path.)
    match &decision {
        GateDecision::RequestChanges { comment } if !comment.trim().is_empty() => {
            run.state.insert("last_human_feedback".into(), comment.clone());
        }
        GateDecision::Approve { comment: Some(c) } if !c.trim().is_empty() => {
            run.state.insert("last_human_feedback".into(), c.clone());
        }
        _ => {}
    }

    let gate_step_name = run.step_results[gate_step_idx].step_name.clone();

    // Reject is terminal — no need to re-spawn execute_run.
    if matches!(decision, GateDecision::Reject { .. }) {
        run.status = RunStatus::Failed;
        run.finished_at = Some(Utc::now());
        let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
        let db = state.db.clone();
        db.with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
            .await?;
        // 0.8.2 — cross-tab WS broadcast: a Reject is terminal, so
        // execute_run won't re-emit. Without this, other tabs viewing
        // the same run stay stuck on "WaitingApproval" until refresh.
        let _ = state.ws_broadcast.send(crate::models::WsMessage::WorkflowRunUpdated {
            run_id: run.id.clone(),
            workflow_id: workflow.id.clone(),
            status: format!("{:?}", run.status),
            step_index: gate_step_idx as i32,
            total_steps: workflow.steps.len() as u32,
            current_step: None,
        });
        // Cleanup workspace if it exists.
        if let Some(ws_path) = run.workspace_path.as_ref().map(std::path::PathBuf::from) {
            if ws_path.exists() {
                let project_path = if let Some(ref pid) = workflow.project_id {
                    let pid = pid.clone();
                    let db2 = state.db.clone();
                    let project = db2
                        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
                        .await?;
                    project.map(|p| p.path).unwrap_or_default()
                } else {
                    String::new()
                };
                if !project_path.is_empty() {
                    let repo_path = crate::core::scanner::resolve_host_path(&project_path);
                    let ws = Workspace::attach(
                        ws_path,
                        repo_path,
                        &workflow.name,
                        &run.id,
                        workflow.workspace_config.as_ref().map(|c| c.hooks.clone()),
                    );
                    if let Ok(outcome) = ws.cleanup().await {
                        if let Some(preserved) = outcome.preserved {
                            // Reject still preserves anything the agent committed —
                            // the operator's "no" is on the gate, not on the work
                            // already on disk. They may want to recover it.
                            run.produced_branches.push(crate::models::ProducedBranch {
                                branch_name: preserved.branch_name,
                                head_sha: preserved.head_sha,
                                ahead: preserved.ahead,
                                pushed_upstream: preserved.pushed_upstream,
                            });
                            let snap = crate::db::workflows::RunProgressSnapshot::from_run(run);
                            let db = state.db.clone();
                            let _ = db.with_conn(move |conn| {
                                crate::db::workflows::update_run_progress(conn, snap)
                            }).await;
                        }
                    }
                }
            }
        }
        return Ok(());
    }

    // RequestChanges → jump back to the target step. Truncate
    // step_results so the engine replays from the target onward.
    if let GateDecision::RequestChanges { .. } = &decision {
        // 2026-06-10 audit P1 — both lookups below were keyed off
        // `gate_step_idx`, which is a `step_results` index. After a Goto
        // loop, `step_results.len()` > `workflow.steps.len()` and the two
        // diverge: `workflow.steps.get(gate_step_idx)` read the WRONG step
        // (or None → silently lost the configured target), and the
        // positional `truncate(target_idx)` cut step_results at the wrong
        // place. Resolve the gate step in `workflow.steps` BY NAME instead
        // (the same model `next_step_index_for_resume` uses on resume).
        let gate_step_def = workflow.steps.iter().find(|s| s.name == gate_step_name);
        let target_name = gate_step_def
            .and_then(|s| s.gate_request_changes_target.clone());

        // Workflow-steps index of the target (where to replay FROM).
        // Default: the step linearly before the gate. Named-but-missing
        // target → warn + same fallback.
        let gate_pos = workflow.steps.iter().position(|s| s.name == gate_step_name);
        let target_idx = if let Some(name) = target_name {
            workflow
                .steps
                .iter()
                .position(|s| s.name == name)
                .unwrap_or_else(|| {
                    tracing::warn!(
                        "Gate '{}' request_changes_target '{}' not found, falling back to previous step",
                        gate_step_name, name
                    );
                    gate_pos.map(|p| p.saturating_sub(1)).unwrap_or(0)
                })
        } else {
            gate_pos.map(|p| p.saturating_sub(1)).unwrap_or(0)
        };

        // Truncate step_results so the run REPLAYS from the target step.
        // Cut at the FIRST step_result that is the target (by name): the
        // rows before it are the clean linear prefix, so the resume cursor
        // lands ON the target (see request_changes_cut for the run-10
        // post-Goto bug the most-recent variant caused). If the target
        // never ran, fall back to the bounded positional index.
        let target_step_name = workflow.steps.get(target_idx).map(|s| s.name.clone());
        let cut = request_changes_cut(
            &run.step_results,
            target_step_name.as_deref(),
            target_idx,
        );
        run.step_results.truncate(cut);

        // 0.8.6 (#25) — checkpoint reset. If the gate captured a
        // checkpoint SHA on its way in, `git reset --hard` to it
        // BEFORE re-running the target step. Makes Goto loops
        // idempotent : the agent re-implements on the same tree
        // state the previous iteration started on, not on top of
        // its own previous output.
        let checkpoint_key = format!(
            "{}{}",
            super::gate_checkpoint::CHECKPOINT_STATE_PREFIX,
            gate_step_name,
        );
        if let Some(sha) = run.state.get(&checkpoint_key).cloned() {
            if let Some(pid) = workflow.project_id.as_ref() {
                let pid2 = pid.clone();
                let project = state.db.with_conn(move |conn| {
                    crate::db::projects::get_project(conn, &pid2)
                }).await.ok().flatten();
                if let Some(proj) = project {
                    match super::gate_checkpoint::reset_to_checkpoint(
                        std::path::Path::new(&proj.path),
                        &sha,
                    ) {
                        Ok(()) => {
                            tracing::info!(
                                run_id = %run.id,
                                gate = %gate_step_name,
                                sha = %sha,
                                "checkpoint reset applied before Goto",
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                run_id = %run.id,
                                gate = %gate_step_name,
                                sha = %sha,
                                error = %e,
                                "checkpoint reset failed — continuing Goto without reset",
                            );
                        }
                    }
                }
            }
        }
    }

    // Approve and RequestChanges both flow into execute_run.
    run.status = RunStatus::Running;
    execute_run(state, workflow, run, tokens_config, agents_config, events_tx, None, None).await
}

/// Append a `> Décision: <verdict>` (and optional comment) footer to a
/// gate StepResult's output. Keeps the rendered gate message intact so
/// the operator's decision is visible alongside the original prompt
/// when reviewing the run history.
fn append_decision_footer(result: &mut StepResult, verdict: &str, comment: Option<&str>) {
    let separator = if result.output.is_empty() { "" } else { "\n\n" };
    match comment.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => {
            result.output.push_str(separator);
            result.output.push_str("> **Décision : ");
            result.output.push_str(verdict);
            result.output.push_str("**\n> ");
            result.output.push_str(&c.replace('\n', "\n> "));
        }
        None => {
            result.output.push_str(separator);
            result.output.push_str("> **Décision : ");
            result.output.push_str(verdict);
            result.output.push_str("**");
        }
    }
}

/// 0.7.0 Phase 3 — write extracted artifacts to disk for the artifacts
/// declared in `workflow.artifacts`. Undeclared artifacts are silently
/// skipped: `extract_artifacts` always populates the template context,
/// but persistence is opt-in via the workflow's artifact map.
///
/// Path resolution: `spec.path` is interpreted relative to `work_dir`
/// (the run's workspace root). Parent directories are created on demand.
/// Failures are logged but never propagate — a workflow run shouldn't
/// fail because the disk is full or the path is unwritable; the agent's
/// output is still in `StepResult.output` and the template context.
pub(crate) fn persist_declared_artifacts(
    workflow: &Workflow,
    extracted: &::std::collections::HashMap<String, String>,
    work_dir: &std::path::Path,
) {
    for (name, content) in extracted {
        let spec = match workflow.artifacts.get(name) {
            Some(s) => s,
            None => {
                tracing::debug!(
                    target: "kronn::workflow_artifact",
                    artifact = %name,
                    "agent emitted undeclared artifact — keeping in template context but skipping disk write"
                );
                continue;
            }
        };
        let target = work_dir.join(&spec.path);
        if let Some(parent) = target.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    target: "kronn::workflow_artifact",
                    artifact = %name, path = %target.display(),
                    "failed to create artifact parent dir: {}", e
                );
                continue;
            }
        }
        match std::fs::write(&target, content) {
            Ok(()) => tracing::info!(
                target: "kronn::workflow_artifact",
                artifact = %name, path = %target.display(), bytes = content.len(),
                "persisted artifact"
            ),
            Err(e) => tracing::warn!(
                target: "kronn::workflow_artifact",
                artifact = %name, path = %target.display(),
                "failed to write artifact: {}", e
            ),
        }
    }
}

/// Stamp the step's "what was actually used here" metadata onto a
/// freshly-produced [`StepResult`] so editing the workflow afterwards
/// (swapping the agent, retargeting the plugin, changing the endpoint)
/// can't corrupt the run's history.
///
/// The frontend reads these fields back to render per-step badges in
/// [`RunDetail`] — `step_kind` drives the badge type, `step_agent` /
/// `step_api_plugin_slug` / `step_api_endpoint_path` populate the
/// subtitle.
///
/// Pulled out of `execute_run`'s loop so the snapshot logic is testable
/// in isolation (the loop itself needs a full workspace + agents to
/// drive end-to-end).
pub(crate) fn apply_step_snapshot(
    step: &WorkflowStep,
    result: &mut StepResult,
    model_tiers: Option<&crate::models::setup::ModelTiersConfig>,
) {
    let kind: &'static str = match step.step_type {
        StepType::ApiCall => "ApiCall",
        StepType::Notify => "Notify",
        StepType::BatchQuickPrompt => "BatchQuickPrompt",
        StepType::Agent => "Agent",
        StepType::Gate => "Gate",
        StepType::Exec => "Exec",
        StepType::BatchApiCall => "BatchApiCall",
        StepType::JsonData => "JsonData",
        StepType::SubWorkflow => "SubWorkflow",
    };
    result.step_kind = Some(kind.into());
    result.step_agent = matches!(step.step_type, StepType::Agent).then(|| step.agent.clone());
    // 2026-06-13 — stamp the model/tier actually resolved for this Agent step
    // so the UI shows the real model on EVERY agent step (incl. per-item
    // fan-out routing), not only steps with an explicit reasoning tier.
    if matches!(step.step_type, StepType::Agent) {
        let settings = step.agent_settings.as_ref();
        let tier = settings.and_then(|s| s.tier).unwrap_or_default();
        // explicit model override wins; else resolve the tier → concrete model
        let model = settings
            .and_then(|s| s.model.clone())
            .or_else(|| crate::agents::runner::resolve_model_flag(&step.agent, tier, model_tiers));
        // label e.g. "opus", "sonnet · reasoning", "haiku · economy"
        result.step_model = match (model, tier) {
            (Some(m), crate::models::ModelTier::Default) => Some(m),
            (Some(m), t) => Some(format!("{m} · {}", format!("{t:?}").to_lowercase())),
            (None, crate::models::ModelTier::Default) => None,
            (None, t) => Some(format!("{t:?}").to_lowercase()),
        };
    }
    if matches!(step.step_type, StepType::ApiCall) {
        result.step_api_plugin_slug = step.api_plugin_slug.clone();
        result.step_api_endpoint_path = step.api_endpoint_path.clone();
    }
}

/// Inject trigger context JSON into template variables.
fn inject_trigger_context(ctx: &mut TemplateContext, trigger_json: &serde_json::Value) {
    if let Some(obj) = trigger_json.as_object() {
        // Issue fields from tracker trigger
        if let Some(title) = obj.get("issue_title").and_then(|v| v.as_str()) {
            ctx.set("issue.title", title);
        }
        if let Some(body) = obj.get("issue_body").and_then(|v| v.as_str()) {
            ctx.set("issue.body", body);
        }
        if let Some(number) = obj.get("issue_number").and_then(|v| v.as_str()) {
            ctx.set("issue.number", number);
        } else if let Some(number) = obj.get("issue_number").and_then(|v| v.as_u64()) {
            ctx.set("issue.number", number.to_string());
        }
        if let Some(url) = obj.get("issue_url").and_then(|v| v.as_str()) {
            ctx.set("issue.url", url);
        }
        if let Some(labels) = obj.get("issue_labels").and_then(|v| v.as_array()) {
            let label_strs: Vec<String> = labels.iter()
                .filter_map(|l| l.as_str().map(String::from))
                .collect();
            ctx.set("issue.labels", label_strs.join(", "));
        }

        // Generic fields — expose any string value from trigger context
        for (key, value) in obj {
            if let Some(s) = value.as_str() {
                ctx.set(key.clone(), s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── inject_and_consume_gate_feedback — gate feedback, approach B ───
    #[test]
    fn gate_feedback_is_injected_and_consumed_once() {
        let mut state = std::collections::HashMap::new();
        state.insert("last_human_feedback".to_string(), "Use the existing CircuitBreaker, don't add a new one".to_string());
        let mut prompt = "Implémente le plan validé.".to_string();

        // First step after the gate: injected + feedback present in prompt.
        let injected = inject_and_consume_gate_feedback(&mut prompt, &mut state);
        assert!(injected);
        assert!(prompt.contains("Use the existing CircuitBreaker"), "feedback must be prepended");
        assert!(prompt.contains("Implémente le plan validé."), "original prompt preserved");
        // Consumed — no longer in state.
        assert!(!state.contains_key("last_human_feedback"));

        // Subsequent step in the same run: nothing left → no-op, prompt untouched.
        let mut prompt2 = "Run the tests.".to_string();
        let injected2 = inject_and_consume_gate_feedback(&mut prompt2, &mut state);
        assert!(!injected2);
        assert_eq!(prompt2, "Run the tests.");
    }

    #[test]
    fn gate_feedback_noop_when_absent_or_blank() {
        let mut state = std::collections::HashMap::new();
        let mut prompt = "do work".to_string();
        assert!(!inject_and_consume_gate_feedback(&mut prompt, &mut state));
        assert_eq!(prompt, "do work");

        // Blank comment is treated as no feedback (still consumed, not injected).
        state.insert("last_human_feedback".to_string(), "   ".to_string());
        assert!(!inject_and_consume_gate_feedback(&mut prompt, &mut state));
        assert_eq!(prompt, "do work");
    }

    // ─── SharedBudget (Phase 1b-ii) ─────────────────────────────────────

    #[test]
    fn shared_budget_clone_shares_one_counter_and_cap() {
        // A child's budget is a CLONE of the parent's — adding on either
        // side moves the SAME tree-wide counter, and the cap is shared.
        let root = super::SharedBudget::root(3);
        let child = root.clone();
        assert_eq!(root.llm_calls(), 0);
        root.add_llm_calls(1);          // parent spends 1
        child.add_llm_calls(1);         // child spends 1 — same counter
        assert_eq!(root.llm_calls(), 2, "both views observe the shared count");
        assert_eq!(child.llm_calls(), 2);
        assert_eq!(child.max_llm_calls(), 3, "child inherits the root cap");
        // The whole tree trips the cap together.
        child.add_llm_calls(1);
        assert!(root.llm_calls() >= root.max_llm_calls(), "tree-wide quota reached");
    }

    // ─── next_step_index_for_resume — Goto-loop bug fix (0.7.0) ─────────

    fn fake_step(name: &str) -> crate::models::WorkflowStep {
        crate::models::WorkflowStep {
            name: name.into(),
            step_type: crate::models::StepType::Agent,
            description: None,
            agent: crate::models::AgentType::ClaudeCode,
            prompt_template: String::new(),
            mode: crate::models::StepMode::Normal,
            output_format: crate::models::StepOutputFormat::FreeText,
            on_result: vec![],
            agent_settings: None,
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
            mcp_config_ids: vec![],
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            batch_quick_prompt_id: None,
            batch_items_from: None,
            batch_wait_for_completion: None,
            batch_max_items: None,
            batch_workspace_mode: None,
            batch_chain_prompt_ids: vec![],
            batch_concurrent_limit: None,
            quick_api_id: None,
            notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
            api_method: None,
            api_query: None,
            api_path_params: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            gate_checkpoint_before: None,
            gate_auto_approve_after_secs: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            exec_setup_command: None,
            exec_setup_args: vec![],
            quick_prompt_id: None,
            json_data_payload: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
        }
    }
    fn fake_result(name: &str) -> crate::models::StepResult {
        crate::models::StepResult {
            step_name: name.into(),
            status: crate::models::RunStatus::Success,
            output: String::new(),
            tokens_used: 0,
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
        }
    }

    #[test]
    fn fresh_run_starts_at_zero() {
        let steps = vec![fake_step("a"), fake_step("b")];
        let results: Vec<crate::models::StepResult> = vec![];
        assert_eq!(next_step_index_for_resume(&steps, &results), 0);
    }

    #[test]
    fn linear_resume_continues_after_last_result() {
        let steps = vec![fake_step("a"), fake_step("b"), fake_step("c")];
        let results = vec![fake_result("a"), fake_result("b")];
        assert_eq!(next_step_index_for_resume(&steps, &results), 2);
    }

    #[test]
    fn resume_after_goto_loop_uses_step_name_not_results_len() {
        // Ticket Autopilot regression: review→implement Goto fired twice,
        // so step_results has duplicate entries. The next index after the
        // gate (last result) must come from the workflow definition, NOT
        // from `step_results.len()` which would overshoot the workflow.
        let steps = vec![
            fake_step("fetch"),       // 0
            fake_step("analyze"),     // 1
            fake_step("plan_gate"),   // 2
            fake_step("implement"),   // 3
            fake_step("run_tests"),   // 4
            fake_step("review"),      // 5
            fake_step("create_pr"),   // 6
            fake_step("ready_gate"),  // 7
            fake_step("notify_done"), // 8
        ];
        // Two extra results from the Goto loop firing twice.
        let results = vec![
            fake_result("fetch"), fake_result("analyze"), fake_result("plan_gate"),
            fake_result("implement"), fake_result("run_tests"), fake_result("review"),
            // Goto loop iteration:
            fake_result("implement"), fake_result("run_tests"), fake_result("review"),
            fake_result("create_pr"), fake_result("ready_gate"),
        ];
        assert_eq!(results.len(), 11, "sanity: 11 results, 9 steps");
        // Buggy old logic would return 11 → loop never runs → notify_done skipped.
        // Correct: ready_gate is at index 7, next is 8 (notify_done).
        assert_eq!(next_step_index_for_resume(&steps, &results), 8);
    }

    #[test]
    fn last_result_name_not_in_workflow_returns_steps_len() {
        // Workflow was edited mid-run (a step renamed/removed). Don't
        // re-run earlier steps blindly — wrap up gracefully.
        let steps = vec![fake_step("a"), fake_step("b")];
        let results = vec![fake_result("ghost_step")];
        assert_eq!(next_step_index_for_resume(&steps, &results), 2);
    }

    #[test]
    fn request_changes_cut_common_case_equals_positional_index() {
        // No Goto: results align with steps. RequestChanges → re-run from
        // `implement` (idx 3). Cut keeps [fetch, analyze, plan_gate] = 3,
        // exactly the old positional truncate — common case unchanged.
        let results = vec![
            fake_result("fetch"), fake_result("analyze"),
            fake_result("plan_gate"), fake_result("implement"),
            fake_result("review_gate"),
        ];
        assert_eq!(request_changes_cut(&results, Some("implement"), 3), 3);
    }

    #[test]
    fn request_changes_cut_after_goto_replays_from_first_occurrence() {
        // Post-Goto: `implement` ran twice. The cut MUST be the FIRST
        // occurrence (idx 3): keeping rows 0..3 leaves `plan_gate` as the
        // last row, so the resume cursor lands on `implement`. The old
        // most-recent cut (idx 5) kept the first round's `review` in the
        // prefix and the run resumed at the GATE instead of the target
        // (run-10 live bug, 2026-06-13).
        let results = vec![
            fake_result("fetch"), fake_result("analyze"),       // 0,1
            fake_result("plan_gate"), fake_result("implement"), // 2,3
            fake_result("review"),                              // 4
            fake_result("implement"),                           // 5  ← Goto re-run
            fake_result("review"), fake_result("ready_gate"),   // 6,7
        ];
        assert_eq!(request_changes_cut(&results, Some("implement"), 3), 3);
        // resume-cursor contract: last kept row (`plan_gate`) + 1 = target
        let steps = vec![
            fake_step("fetch"), fake_step("analyze"), fake_step("plan_gate"),
            fake_step("implement"), fake_step("review"), fake_step("ready_gate"),
        ];
        let kept = &results[..3];
        assert_eq!(next_step_index_for_resume(&steps, kept), 3, "cursor lands ON implement");
    }

    #[test]
    fn request_changes_cut_target_never_ran_falls_back_bounded() {
        // Target step exists in the workflow but never executed (e.g. a
        // Skip jumped over it). Fall back to the bounded positional index.
        let results = vec![fake_result("fetch"), fake_result("gate")];
        assert_eq!(request_changes_cut(&results, Some("never_ran"), 1), 1);
        // Fallback index past the end is clamped.
        assert_eq!(request_changes_cut(&results, Some("never_ran"), 99), 2);
    }

    // ─── inject_trigger_context ──────────────────────────────────────────

    #[test]
    fn inject_trigger_context_issue_fields() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_title": "Fix the bug",
            "issue_body": "It crashes on startup",
            "issue_number": "42",
            "issue_url": "https://github.com/owner/repo/issues/42",
            "issue_labels": ["bug", "priority-high"],
        });
        inject_trigger_context(&mut ctx, &json);

        assert_eq!(ctx.render("{{issue.title}}").unwrap(), "Fix the bug");
        assert_eq!(ctx.render("{{issue.body}}").unwrap(), "It crashes on startup");
        assert_eq!(ctx.render("{{issue.number}}").unwrap(), "42");
        assert_eq!(ctx.render("{{issue.url}}").unwrap(), "https://github.com/owner/repo/issues/42");
        assert_eq!(ctx.render("{{issue.labels}}").unwrap(), "bug, priority-high");
    }

    #[test]
    fn inject_trigger_context_issue_number_as_u64() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_number": 99,
        });
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{issue.number}}").unwrap(), "99");
    }

    #[test]
    fn inject_trigger_context_generic_string_fields() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "type": "tracker",
            "custom_field": "hello",
        });
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{type}}").unwrap(), "tracker");
        assert_eq!(ctx.render("{{custom_field}}").unwrap(), "hello");
    }

    #[test]
    fn inject_trigger_context_non_string_values_ignored() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "count": 42,
            "nested": {"key": "val"},
        });
        inject_trigger_context(&mut ctx, &json);
        // Non-string generic fields are NOT injected
        assert_eq!(ctx.render("{{count}}").unwrap(), "{{count}}");
        assert_eq!(ctx.render("{{nested}}").unwrap(), "{{nested}}");
    }

    #[test]
    fn inject_trigger_context_empty_object() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({});
        inject_trigger_context(&mut ctx, &json);
        // No variables set — template placeholders remain
        assert_eq!(ctx.render("{{issue.title}}").unwrap(), "{{issue.title}}");
    }

    #[test]
    fn inject_trigger_context_not_an_object() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!("just a string");
        inject_trigger_context(&mut ctx, &json);
        // Should silently do nothing
        assert_eq!(ctx.render("{{anything}}").unwrap(), "{{anything}}");
    }

    #[test]
    fn inject_trigger_context_null_value() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::Value::Null;
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{anything}}").unwrap(), "{{anything}}");
    }

    #[test]
    fn inject_trigger_context_empty_labels() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_labels": [],
        });
        inject_trigger_context(&mut ctx, &json);
        assert_eq!(ctx.render("{{issue.labels}}").unwrap(), "");
    }

    #[test]
    fn inject_trigger_context_labels_with_non_string_items() {
        let mut ctx = TemplateContext::new();
        let json = serde_json::json!({
            "issue_labels": ["bug", 42, "feature"],
        });
        inject_trigger_context(&mut ctx, &json);
        // Non-string items in labels array are filtered out
        assert_eq!(ctx.render("{{issue.labels}}").unwrap(), "bug, feature");
    }

    // ─── apply_step_snapshot ────────────────────────────────────────────
    //
    // The whole point of these snapshots is run-history honesty: editing
    // the workflow definition after a run completes must NOT corrupt the
    // run record. These tests lock the per-step-kind shape so a future
    // refactor can't silently start setting step_agent on an ApiCall row
    // (or skip step_api_endpoint_path on the ApiCall path).

    fn mk_step_for_snapshot(kind: StepType) -> WorkflowStep {
        WorkflowStep {
            name: "s".into(),
            step_type: kind,
            description: None,
            agent: AgentType::Codex,
            prompt_template: String::new(),
            mode: StepMode::Normal,
            output_format: StepOutputFormat::FreeText,
            mcp_config_ids: vec![],
            agent_settings: None,
            on_result: vec![],
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            batch_quick_prompt_id: None,
            batch_items_from: None,
            batch_wait_for_completion: None,
            batch_max_items: None,
            batch_workspace_mode: None,
            batch_chain_prompt_ids: vec![],
            batch_concurrent_limit: None,
            quick_api_id: None,
            notify_config: None,
            api_plugin_slug: Some("mcp-github".into()),
            api_config_id: Some("cfg-1".into()),
            api_endpoint_path: Some("/repos/anthropics/cookbook/issues".into()),
            api_method: None,
            api_path_params: None,
            api_query: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            gate_checkpoint_before: None,
            gate_auto_approve_after_secs: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            exec_setup_command: None,
            exec_setup_args: vec![],
            quick_prompt_id: None,
            json_data_payload: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
        }
    }

    fn empty_result() -> StepResult {
        StepResult {
            step_name: "s".into(),
            status: RunStatus::Success,
            output: String::new(),
            tokens_used: 0,
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
        }
    }

    #[test]
    fn snapshot_agent_step_records_agent_only() {
        let step = mk_step_for_snapshot(StepType::Agent);
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        assert_eq!(r.step_kind.as_deref(), Some("Agent"));
        assert_eq!(r.step_agent, Some(AgentType::Codex));
        // ApiCall fields stay None for Agent steps even when the step
        // happens to carry stale api_* values (legacy edits).
        assert!(r.step_api_plugin_slug.is_none());
        assert!(r.step_api_endpoint_path.is_none());
    }

    #[test]
    fn snapshot_stamps_resolved_model_on_agent_step() {
        // 2026-06-13 — step_model must be stamped so the UI shows the real
        // model on EVERY agent step. With no explicit tier (Default), Codex
        // resolves to None (no --model) → step_model None; with a reasoning
        // tier it resolves to the concrete model + tier label.
        use crate::models::{AgentSettings, ModelTier};
        let mut step = mk_step_for_snapshot(StepType::Agent);
        step.agent = AgentType::ClaudeCode;
        step.agent_settings = Some(AgentSettings {
            model: None, tier: Some(ModelTier::Reasoning), reasoning_effort: None, max_tokens: None,
        });
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        // built-in ClaudeCode reasoning → opus, labelled with the tier
        assert_eq!(r.step_model.as_deref(), Some("opus · reasoning"));
        // explicit model override wins, default tier → bare model
        step.agent_settings = Some(AgentSettings {
            model: Some("o3".into()), tier: None, reasoning_effort: None, max_tokens: None,
        });
        let mut r2 = empty_result();
        apply_step_snapshot(&step, &mut r2, None);
        assert_eq!(r2.step_model.as_deref(), Some("o3"));
    }

    #[test]
    fn snapshot_apicall_step_records_plugin_and_endpoint_no_agent() {
        let step = mk_step_for_snapshot(StepType::ApiCall);
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        assert_eq!(r.step_kind.as_deref(), Some("ApiCall"));
        assert!(r.step_agent.is_none(),
            "ApiCall has no agent — leaving step_agent null is what powers the per-step badge");
        assert_eq!(r.step_api_plugin_slug.as_deref(), Some("mcp-github"));
        assert_eq!(r.step_api_endpoint_path.as_deref(), Some("/repos/anthropics/cookbook/issues"));
    }

    #[test]
    fn snapshot_notify_step_records_notify_kind_no_agent_no_plugin() {
        let step = mk_step_for_snapshot(StepType::Notify);
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        assert_eq!(r.step_kind.as_deref(), Some("Notify"));
        assert!(r.step_agent.is_none());
        assert!(r.step_api_plugin_slug.is_none());
    }

    #[test]
    fn snapshot_batch_step_records_batch_kind_no_agent_no_plugin() {
        let step = mk_step_for_snapshot(StepType::BatchQuickPrompt);
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        assert_eq!(r.step_kind.as_deref(), Some("BatchQuickPrompt"));
        assert!(r.step_agent.is_none());
        assert!(r.step_api_plugin_slug.is_none());
    }

    #[test]
    fn snapshot_gate_step_records_gate_kind_no_agent_no_plugin() {
        let step = mk_step_for_snapshot(StepType::Gate);
        let mut r = empty_result();
        apply_step_snapshot(&step, &mut r, None);
        assert_eq!(r.step_kind.as_deref(), Some("Gate"));
        assert!(r.step_agent.is_none(),
            "Gate has no agent — the badge should render as a 'pause' chip with no agent name");
        assert!(r.step_api_plugin_slug.is_none());
    }

    // ─── append_decision_footer (Phase 4 — gate decision) ───────────────

    fn gate_result_with(output: &str) -> StepResult {
        StepResult {
            step_name: "gate".into(),
            status: RunStatus::WaitingApproval,
            output: output.into(),
            tokens_used: 0,
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
        }
    }

    #[test]
    fn footer_appends_with_separator_and_verdict_no_comment() {
        let mut r = gate_result_with("Validate the PR?");
        append_decision_footer(&mut r, "Approuvé", None);
        assert!(r.output.starts_with("Validate the PR?"));
        assert!(r.output.contains("> **Décision : Approuvé**"));
        assert!(r.output.contains("\n\n"), "should keep gate body and footer separated by blank line");
    }

    #[test]
    fn footer_appends_with_comment() {
        let mut r = gate_result_with("Validate?");
        append_decision_footer(&mut r, "Changements demandés", Some("Add unit tests"));
        assert!(r.output.contains("> **Décision : Changements demandés**"));
        assert!(r.output.contains("> Add unit tests"));
    }

    #[test]
    fn footer_handles_multiline_comment_with_blockquote_continuation() {
        // Multi-line comments are blockquoted line-by-line so the
        // run-detail markdown renders the comment as a single
        // visually-grouped quote rather than a quote followed by raw
        // text. UX preserves operator's line-breaks.
        let mut r = gate_result_with("Gate body");
        append_decision_footer(&mut r, "Rejeté", Some("Line1\nLine2"));
        assert!(r.output.contains("> Line1\n> Line2"));
    }

    #[test]
    fn footer_skips_separator_when_body_is_empty() {
        // If the gate had an empty rendered message, don't prepend a
        // pair of blank lines (would render as a leading whitespace
        // block in the UI).
        let mut r = gate_result_with("");
        append_decision_footer(&mut r, "Approuvé", None);
        assert!(!r.output.starts_with("\n"), "got: {:?}", r.output);
        assert!(r.output.contains("Approuvé"));
    }

    #[test]
    fn footer_treats_blank_comment_as_no_comment() {
        // Whitespace-only comment from the UI should fall back to
        // verdict-only — preventing "> " stray-prefix lines.
        let mut r = gate_result_with("Body");
        append_decision_footer(&mut r, "Approuvé", Some("   \n\t  "));
        assert!(r.output.contains("> **Décision : Approuvé**"));
        // No bare "> " line beneath the verdict.
        let lines: Vec<&str> = r.output.lines().collect();
        let last = lines.last().copied().unwrap_or("");
        assert!(last.contains("Approuvé**"), "last line should be the verdict line, got: {:?}", last);
    }

    // ─── max_iterations_for ──────────────────────────────────────────────

    #[test]
    fn max_iterations_zero_steps() {
        assert_eq!(max_iterations_for(0), 50);
    }

    #[test]
    fn max_iterations_typical_workflow() {
        assert_eq!(max_iterations_for(3), 80);  // 3*10 + 50
        assert_eq!(max_iterations_for(5), 100); // 5*10 + 50
    }

    #[test]
    fn max_iterations_single_step() {
        assert_eq!(max_iterations_for(1), 60);  // 1*10 + 50
    }

    // ─── RunEvent serialization ──────────────────────────────────────────

    #[test]
    fn run_event_step_start_serializes() {
        let evt = RunEvent::StepStart {
            step_name: "analyze".into(),
            step_index: 0,
            total_steps: 3,
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["event"], "StepStart");
        assert_eq!(json["data"]["step_name"], "analyze");
        assert_eq!(json["data"]["step_index"], 0);
        assert_eq!(json["data"]["total_steps"], 3);
    }

    #[test]
    fn run_event_run_done_serializes() {
        let evt = RunEvent::RunDone { status: RunStatus::Success };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["event"], "RunDone");
    }

    #[test]
    fn run_event_run_error_serializes() {
        let evt = RunEvent::RunError { error: "timeout".into() };
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["event"], "RunError");
        assert_eq!(json["data"]["error"], "timeout");
    }

    // ─── WorkflowGuards (Phase 1 — 0.7.0) ────────────────────────────────

    #[test]
    fn workflow_guards_default_resolves_to_backend_constants() {
        let g = WorkflowGuards::default().resolved();
        assert_eq!(g.timeout_seconds, DEFAULT_GUARD_TIMEOUT_SECS);
        assert_eq!(g.max_llm_calls, DEFAULT_GUARD_MAX_LLM_CALLS);
        assert_eq!(g.loop_detection_max_revisits, DEFAULT_GUARD_LOOP_MAX_REVISITS);
    }

    #[test]
    fn workflow_guards_partial_override_uses_defaults_for_unset() {
        // User sets only timeout; the other two should fall back to defaults.
        let g = WorkflowGuards {
            timeout_seconds: Some(60),
            max_llm_calls: None,
            loop_detection_max_revisits: None,
        };
        let r = g.resolved();
        assert_eq!(r.timeout_seconds, 60);
        assert_eq!(r.max_llm_calls, DEFAULT_GUARD_MAX_LLM_CALLS);
        assert_eq!(r.loop_detection_max_revisits, DEFAULT_GUARD_LOOP_MAX_REVISITS);
    }

    #[test]
    fn workflow_guards_resolve_optional_none_yields_defaults() {
        let r = WorkflowGuards::resolve_optional(None);
        assert_eq!(r.timeout_seconds, DEFAULT_GUARD_TIMEOUT_SECS);
        assert_eq!(r.max_llm_calls, DEFAULT_GUARD_MAX_LLM_CALLS);
        assert_eq!(r.loop_detection_max_revisits, DEFAULT_GUARD_LOOP_MAX_REVISITS);
    }

    #[test]
    fn workflow_guards_full_override() {
        let g = WorkflowGuards {
            timeout_seconds: Some(120),
            max_llm_calls: Some(5),
            loop_detection_max_revisits: Some(3),
        };
        let r = g.resolved();
        assert_eq!(r.timeout_seconds, 120);
        assert_eq!(r.max_llm_calls, 5);
        assert_eq!(r.loop_detection_max_revisits, 3);
    }

    #[test]
    fn run_status_stopped_by_guard_round_trips_through_db() {
        // Ensure parse + serialize for the new variant doesn't drop data.
        // Mirror test for the matching `parse_run_status` / `run_status_str`
        // pair in `db/workflows.rs`.
        let status = RunStatus::StoppedByGuard;
        let json = serde_json::to_string(&status).unwrap();
        let back: RunStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, RunStatus::StoppedByGuard);
    }

    #[test]
    fn guard_kind_serializes_with_step_name_for_loop_detection() {
        // The `LoopDetection` variant carries which step looped — the
        // frontend uses this to render "step 'self_review' visited 11x"
        // instead of a generic "loop detected".
        let k = GuardKind::LoopDetection { step_name: "self_review".into() };
        let json = serde_json::to_value(&k).unwrap();
        assert_eq!(json["type"], "LoopDetection");
        assert_eq!(json["step_name"], "self_review");
    }

    // ─── Artifacts persistence (0.7.0 Phase 3) ────────────────────────────────

    fn make_workflow_with_artifacts(artifacts: ::std::collections::HashMap<String, ArtifactSpec>) -> Workflow {
        Workflow {
            id: "test".into(),
            name: "test".into(),
            project_id: None,
            trigger: WorkflowTrigger::Manual,
            steps: vec![],
            actions: vec![],
            safety: WorkflowSafety { sandbox: false, max_files: None, max_lines: None, require_approval: false },
            workspace_config: None,
            concurrency_limit: None,
            guards: None,
            artifacts,
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![],
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn persist_writes_declared_artifacts_to_workspace() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut artifacts = ::std::collections::HashMap::new();
        artifacts.insert("plan".to_string(), ArtifactSpec {
            path: ".kronn/plan.md".into(),
            format: Some("markdown".into()),
        });
        let wf = make_workflow_with_artifacts(artifacts);

        let mut extracted = ::std::collections::HashMap::new();
        extracted.insert("plan".to_string(), "# Plan body".to_string());

        persist_declared_artifacts(&wf, &extracted, dir.path());

        let written = dir.path().join(".kronn/plan.md");
        assert!(written.exists(), "artifact must be written");
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "# Plan body");
    }

    #[test]
    fn persist_skips_undeclared_artifacts() {
        let dir = tempfile::TempDir::new().unwrap();
        let wf = make_workflow_with_artifacts(::std::collections::HashMap::new());

        let mut extracted = ::std::collections::HashMap::new();
        extracted.insert("rogue".to_string(), "should not land".to_string());

        persist_declared_artifacts(&wf, &extracted, dir.path());

        let walked: Vec<_> = walkdir::WalkDir::new(dir.path())
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .collect();
        assert!(walked.is_empty(),
            "no file should be written for undeclared artifacts (got {} files)",
            walked.len());
    }

    #[test]
    fn persist_creates_parent_directories_on_demand() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut artifacts = ::std::collections::HashMap::new();
        artifacts.insert("trace".to_string(), ArtifactSpec {
            path: ".kronn/deep/nested/trace.yaml".into(),
            format: None,
        });
        let wf = make_workflow_with_artifacts(artifacts);

        let mut extracted = ::std::collections::HashMap::new();
        extracted.insert("trace".to_string(), "step: ok".to_string());

        persist_declared_artifacts(&wf, &extracted, dir.path());

        let written = dir.path().join(".kronn/deep/nested/trace.yaml");
        assert!(written.exists(), "deep parent dirs must be created");
    }

    #[test]
    fn persist_overwrites_existing_artifact_on_re_emit() {
        // Auto-Dev's review→implement→review loop emits the same
        // `review` artifact on every iteration. The latest write wins.
        let dir = tempfile::TempDir::new().unwrap();
        let mut artifacts = ::std::collections::HashMap::new();
        artifacts.insert("review".to_string(), ArtifactSpec {
            path: ".kronn/review.yaml".into(),
            format: None,
        });
        let wf = make_workflow_with_artifacts(artifacts);

        let mut e1 = ::std::collections::HashMap::new();
        e1.insert("review".to_string(), "v1".to_string());
        persist_declared_artifacts(&wf, &e1, dir.path());

        let mut e2 = ::std::collections::HashMap::new();
        e2.insert("review".to_string(), "v2 with more".to_string());
        persist_declared_artifacts(&wf, &e2, dir.path());

        let written = dir.path().join(".kronn/review.yaml");
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "v2 with more");
    }
}

#[cfg(test)]
mod preflight_validation_tests {
    use crate::models::AgentType;

    /// Mirror the discriminant-comparison logic from execute_run's preflight
    /// block — keeps the contract testable without spinning up `detect_all`
    /// (which hits the host filesystem and depends on which agent binaries
    /// are actually on PATH).
    fn missing_step_agents(step_agents: &[(&str, AgentType)], usable: &[AgentType])
        -> Vec<(String, AgentType)>
    {
        step_agents.iter()
            .filter(|(_, a)| !usable.iter().any(|u| std::mem::discriminant(u) == std::mem::discriminant(a)))
            .map(|(n, a)| (n.to_string(), a.clone()))
            .collect()
    }

    #[test]
    fn empty_workflow_yields_no_missing() {
        assert!(missing_step_agents(&[], &[AgentType::ClaudeCode]).is_empty());
    }

    #[test]
    fn all_agents_usable_yields_no_missing() {
        let steps = vec![("plan", AgentType::ClaudeCode), ("test", AgentType::Codex)];
        let usable = vec![AgentType::ClaudeCode, AgentType::Codex];
        assert!(missing_step_agents(&steps, &usable).is_empty());
    }

    #[test]
    fn cross_agent_workflow_with_one_uninstalled_step_is_flagged() {
        // Real-world case: workflow uses ClaudeCode for planning + Vibe for
        // a cheap summarisation step. User uninstalls Vibe between editing
        // the workflow and clicking Run. Pre-fix the run would fail
        // mid-execution at the spawn site with "vibe: command not found".
        let steps = vec![
            ("plan", AgentType::ClaudeCode),
            ("summarise", AgentType::Vibe),
            ("review", AgentType::ClaudeCode),
        ];
        let usable = vec![AgentType::ClaudeCode]; // Vibe uninstalled
        let missing = missing_step_agents(&steps, &usable);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].0, "summarise");
        assert!(matches!(missing[0].1, AgentType::Vibe));
    }

    #[test]
    fn empty_usable_flags_every_step() {
        let steps = vec![
            ("a", AgentType::ClaudeCode),
            ("b", AgentType::Codex),
            ("c", AgentType::Vibe),
        ];
        let missing = missing_step_agents(&steps, &[]);
        assert_eq!(missing.len(), 3);
    }
}
