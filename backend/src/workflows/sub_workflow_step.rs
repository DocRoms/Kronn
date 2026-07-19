//! Executor for `StepType::SubWorkflow` — runs another workflow as a nested
//! child run (recursive sub-workflows, spec `docs/design/recursive-subworkflows.md`).
//!
//! **Phase 1b-i (this file): the re-entrant executor.** Loads the target
//! workflow, creates a child `WorkflowRun` (`parent_run_id` set, `run_type =
//! "subworkflow"`), then re-enters `runner::execute_run` on it via `Box::pin`
//! (async self-recursion needs boxing). The child runs with its OWN steps,
//! Goto/loops and gates-conditions — so "tests fail → back to implement, N×
//! max" lives entirely inside the child. The child's terminal status maps to
//! this step (Success → `OK`, else `SUBWF_FAILED`, branchable via the
//! parent's `on_result`); its tokens aggregate onto the step; its run id is
//! recorded on `StepResult.child_run_id` for drill-down.
//!
//! Recursion is bounded by `MAX_SUBWORKFLOW_DEPTH` (runtime guard here +
//! static guard at save). Cycles are blocked at save (the graph validator);
//! the depth cap is the runtime backstop against runaway nesting.
//!
//! **Known Phase-1b-i limitations** (hardened later, documented honestly):
//! - **Budget is per-child, not shared** — each level has its own
//!   `max_llm_calls`/timeout; total is bounded by depth × per-workflow cap.
//!   The `SharedBudget` refactor (spec §4.2) is Phase 1b-ii.
//! - **Cancel does not cascade** — cancelling the parent doesn't auto-cancel
//!   an in-flight child; the child has its own `/cancel`. Cascade is a follow-up.
//! - **Worktree is per-child** (named by run_id → no collision); sharing/merge
//!   is Phase 2.

use std::time::Instant;

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::models::setup::{AgentsConfig, TokensConfig};
use crate::models::{RunStatus, StepResult, WorkflowRun, WorkflowStep};

use super::steps::StepOutcome;

/// Max nesting depth — mirrors `api::workflows::MAX_SUBWORKFLOW_DEPTH` (the
/// save-time static cap). Kept as a local const to avoid an api→workflows dep.
const MAX_SUBWORKFLOW_DEPTH: usize = 5;

/// Run a sub-workflow as a child run. `current_depth` is the depth of the
/// PARENT run (0 for a top-level run); the child is created at
/// `current_depth + 1`. `parent_run_id` links the child for drill-down.
#[allow(clippy::too_many_arguments)]
pub async fn execute_sub_workflow_step(
    state: &crate::AppState,
    parent_run_id: &str,
    current_depth: usize,
    step: &WorkflowStep,
    tokens_config: &TokensConfig,
    agents_config: &AgentsConfig,
    // Phase 1b-ii — the parent tree's shared LLM-calls budget, handed to the
    // child so the WHOLE nested orchestration shares one quota (no per-child
    // reset that would let a deep tree spend depth × cap).
    budget: super::runner::SharedBudget,
    // Phase 2 (worktree handoff) — the parent run's worktree path, when it has
    // one. The child SHARES it (attaches, commits to the parent's branch) so a
    // later parent step (`create_pr`) sees the child's work. `None` → the child
    // gets its own worktree (legacy Phase 1 behaviour, e.g. parent in main tree).
    parent_workspace: Option<String>,
) -> StepOutcome {
    let start = Instant::now();

    let target = match step.sub_workflow_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => t.to_string(),
        None => return fail(step, start, "SubWorkflow step missing `sub_workflow_id`.".into()),
    };

    // Runtime depth backstop (save-validation caps statically; this guards a
    // hand-edited JSON or a workflow edited after a reference was created).
    let child_depth = current_depth + 1;
    if child_depth_exceeds(current_depth) {
        return fail(step, start, format!(
            "Sub-workflow depth limit ({MAX_SUBWORKFLOW_DEPTH}) exceeded at `{target}` — refusing to recurse further."
        ));
    }

    // 2026-06-12 Phase 3b (MVP) — per-item fan-out. When the step declares
    // `sub_workflow_foreach_file`, the child workflow runs ONCE PER ITEM of
    // that JSON-array file (written by an upstream step like triage), each
    // item exposed to the child via `.kronn/current_task.json` in the SHARED
    // worktree. Sequential by design: no worktree race, no merge machinery —
    // the token gain comes from per-item scoped context, not parallelism.
    if let Some(ff) = step.sub_workflow_foreach_file.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return execute_foreach(
            state, parent_run_id, child_depth, step, tokens_config, agents_config,
            budget, parent_workspace, &target, ff, start,
        ).await;
    }

    // Load the child workflow definition.
    let target_for_db = target.clone();
    let child_wf = match state
        .db
        .with_conn(move |c| crate::db::workflows::get_workflow(c, &target_for_db))
        .await
    {
        Ok(Some(w)) => w,
        Ok(None) => return fail(step, start, format!("Sub-workflow `{target}` not found (deleted? wrong id?).")),
        Err(e) => return fail(step, start, format!("DB error loading sub-workflow `{target}`: {e}")),
    };

    // Build + persist the child run. The depth marker rides in
    // `trigger_context` so the child's OWN SubWorkflow steps read the right
    // depth (the dispatch reads `__subwf_depth__`).
    let now = Utc::now();
    let child_run_id = Uuid::new_v4().to_string();
    let trigger = json!({ "__subwf_depth__": child_depth });
    let mut child_run = WorkflowRun {
        id: child_run_id.clone(),
        workflow_id: child_wf.id.clone(),
        status: RunStatus::Pending,
        trigger_context: Some(trigger),
        step_results: vec![],
        tokens_used: 0,
        // Phase 1b-i: child gets its OWN workspace (execute_run handles it,
        // named by run_id → no collision). Sharing/merge is Phase 2.
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "subworkflow".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: None,
        parent_run_id: Some(parent_run_id.to_string()),
        state: Default::default(),
        produced_branches: vec![],
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
    };
    let to_insert = child_run.clone();
    if let Err(e) = state
        .db
        .with_conn(move |c| crate::db::workflows::insert_run(c, &to_insert))
        .await
    {
        return fail(step, start, format!("Failed to create sub-workflow run row: {e}"));
    }

    tracing::info!(
        target: "kronn::sub_workflow",
        parent_run = %parent_run_id, child_run = %child_run.id,
        sub_workflow = %child_wf.id, depth = child_depth,
        "entering sub-workflow"
    );

    // Re-enter the runner on the child. `Box::pin` breaks the otherwise-
    // infinite async recursion type (execute_run → dispatch → here → execute_run).
    let exec_res = Box::pin(crate::workflows::runner::execute_run(
        state.clone(),
        &child_wf,
        &mut child_run,
        tokens_config,
        agents_config,
        None,             // no live SSE for the child in Phase 1; the tree endpoint reads DB
        Some(budget),     // SHARED budget — child counts against the tree-wide quota
        parent_workspace, // Phase 2 — share the parent's worktree when present
    ))
    .await;
    if let Err(e) = exec_res {
        // execute_run returns Err only on infra failure; the child status may
        // still be set. Log and fall through to the status mapping.
        tracing::warn!(
            target: "kronn::sub_workflow",
            child_run = %child_run.id, "sub-workflow run errored: {}", e
        );
    }

    // Map the child's terminal status onto this step.
    let child_status = child_run.status.clone();
    let success = matches!(child_status, RunStatus::Success);
    let status_str = if success { "OK" } else { "SUBWF_FAILED" };
    let signal = status_str;
    let summary = format!(
        "Sous-workflow « {} » → {:?} ({} étapes, {} tokens)",
        child_wf.name,
        child_status,
        child_run.step_results.len(),
        child_run.tokens_used,
    );
    // 2026-06-11 Phase 2 (§5 envelope enrichment) — expose the child's LAST
    // step output + name so a parent step (`create_pr`, `ready_gate`) can read
    // the child's verdict via `{{steps.<subwf>.data.last_output}}` instead of
    // reaching for child-internal step names (which don't resolve across the
    // parent/child boundary). `last_output` carries the review verdict, test
    // signals, etc. — whatever the child's terminal step printed.
    let last = child_run.step_results.last();
    let data = json!({
        "child_run_id": child_run.id,
        "child_workflow_id": child_wf.id,
        "child_status": format!("{child_status:?}"),
        "child_steps": child_run.step_results.len(),
        "last_step": last.map(|s| s.step_name.clone()),
        "last_output": last.map(|s| s.output.clone()),
    });
    let output = super::step_output_format::format_step_output(
        data, status_str, &summary, None, &[signal],
    );
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(|a| match a {
        crate::models::ConditionAction::Stop => "Stop".to_string(),
        crate::models::ConditionAction::Skip => "Skip".to_string(),
        crate::models::ConditionAction::Goto { step_name, .. } => format!("Goto:{step_name}"),
    });

    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            // child Failed/StoppedByGuard/Cancelled → step Failed (parent's
            // on_result can branch on SUBWF_FAILED to retry/escalate).
            status: if success { RunStatus::Success } else { RunStatus::Failed },
            output,
            tokens_used: child_run.tokens_used, // aggregate child cost onto the step
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: None,
            condition_result,
            envelope_detected: Some(true),
            step_kind: None,
            step_agent: None,
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: Some(child_run.id.clone()),
        },
        condition_action,
    }
}

/// Phase 3b — cap on per-item fan-out (runaway-manifest backstop).
const MAX_FOREACH_ITEMS: usize = 30;

/// Phase 3b — parse the foreach file content into items. Pure for tests.
/// Must be a JSON array; empty or oversized arrays are explicit errors (an
/// empty work-list almost always means the upstream step misfired).
pub(crate) fn parse_foreach_items(content: &str) -> Result<Vec<serde_json::Value>, String> {
    let v: serde_json::Value = serde_json::from_str(content)
        .map_err(|e| format!("foreach file is not valid JSON: {e}"))?;
    let arr = v.as_array().ok_or("foreach file must contain a JSON ARRAY of task items")?;
    if arr.is_empty() {
        return Err("foreach file contains an empty array — nothing to implement (upstream step misfired?)".into());
    }
    if arr.len() > MAX_FOREACH_ITEMS {
        return Err(format!("foreach file has {} items — exceeds the cap of {MAX_FOREACH_ITEMS}", arr.len()));
    }
    Ok(arr.clone())
}

/// Flip économique (2026-06-12) — map an item's `complexity` onto a model
/// tier for the child's Agent steps: low → Economy (cheap model on boilerplate),
/// high → Reasoning, med/absent → None (agent default). Pure for tests.
pub(crate) fn tier_for_complexity(complexity: Option<&str>) -> Option<crate::models::ModelTier> {
    match complexity {
        Some("low") => Some(crate::models::ModelTier::Economy),
        Some("high") => Some(crate::models::ModelTier::Reasoning),
        _ => None,
    }
}

/// Flip économique — validate a `mechanical:true` item's `files[]` payload
/// (engine-appliable content planned by triage, approved at the gate).
/// Returns `(path, content)` pairs, or an error string. Guards:
/// - paths must be relative, no `..`, no leading `/`, not under `.git/`
/// - when the item is decided/mocked (has `chosen` or `placeholder`), at
///   least one file's content must carry its `KRONN-…(<id>)` marker (the
///   completeness contract still holds without an agent).
///
/// Pure for tests.
pub(crate) fn validate_mechanical_files(item: &serde_json::Value) -> Result<Vec<(String, String)>, String> {
    let files = item.get("files").and_then(|f| f.as_array())
        .ok_or("mechanical item has no `files[]` payload")?;
    if files.is_empty() { return Err("mechanical item has an empty `files[]`".into()); }
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let path = f.get("path").and_then(|p| p.as_str()).unwrap_or("").trim().to_string();
        let content = f.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
        if path.is_empty() || content.is_empty() {
            return Err("mechanical file entry missing `path` or `content`".into());
        }
        if path.starts_with('/') || path.split('/').any(|seg| seg == "..") || path.starts_with(".git/") {
            return Err(format!("mechanical file path rejected (absolute/traversal/.git): `{path}`"));
        }
        out.push((path, content));
    }
    let needs_marker = item.get("chosen").is_some() || item.get("placeholder").is_some();
    if needs_marker {
        let id = item.get("id").and_then(|i| i.as_str()).unwrap_or("");
        let marker = format!("({id})");
        if id.is_empty() || !out.iter().any(|(_, c)| c.contains(&marker)) {
            return Err(format!(
                "mechanical decided/mocked item `{id}` must embed its KRONN-…({id}) marker in one of its files"
            ));
        }
    }
    Ok(out)
}

/// Phase 3b — aggregate per-item outcomes into (status, signal). Pure for tests.
pub(crate) fn aggregate_foreach(succeeded: usize, failed: usize) -> (&'static str, &'static str) {
    if failed == 0 { ("OK", "OK") }
    else if succeeded == 0 { ("SUBWF_FAILED", "SUBWF_FAILED") }
    else { ("PARTIAL", "PARTIAL") }
}

/// 2026-06-24 — expose the current foreach item to the CHILD's template engine
/// as `{{current_task.<field>}}`, mirroring the `.kronn/current_task.json` file
/// name (so an ApiCall path can be `/pulls/{{current_task.number}}/reviews`
/// without an extra read step). Top-level scalars stringify directly
/// (`number` → "1234", bool → "true"); nested arrays/objects render as compact
/// JSON; the whole item is also available as `{{current_task}}`. These land as
/// string-valued top-level keys in the child's `trigger_context`, which the
/// runner's generic injector ([`inject_trigger_context`]) turns into template
/// vars. Pure for tests.
pub(crate) fn current_task_template_vars(item: &serde_json::Value) -> Vec<(String, String)> {
    let mut out = vec![("current_task".to_string(), item.to_string())];
    if let Some(obj) = item.as_object() {
        for (k, v) in obj {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            out.push((format!("current_task.{k}"), s));
        }
    }
    out
}

/// Phase 3b — sequential per-item fan-out of the child workflow over the
/// items of `foreach_file` (workspace-relative JSON array). Each iteration
/// writes the item to `.kronn/current_task.json` (shared worktree) so the
/// child's prompts read ONLY their slice, then re-enters the runner. A failed
/// item doesn't stop the loop (PARTIAL surfaces it); parent cancel is honoured
/// between items.
/// A2 — durable per-item done marker on the PARENT run, written only AFTER a
/// durable effect (commit landed / child Success). Best-effort: child rows and
/// the git ledger remain the reconciliation sources at resume.
async fn record_foreach_done(
    state: &crate::AppState,
    parent_run_id: &str,
    step_name: &str,
    entry: serde_json::Value,
) {
    let (rid, sname) = (parent_run_id.to_string(), step_name.to_string());
    if let Err(e) = state.db.with_conn(move |conn| {
        crate::db::workflows::append_foreach_done(conn, &rid, &sname, entry)
    }).await {
        tracing::warn!(target: "kronn::sub_workflow", "foreach done-set write failed: {e}");
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_foreach(
    state: &crate::AppState,
    parent_run_id: &str,
    child_depth: usize,
    step: &WorkflowStep,
    tokens_config: &TokensConfig,
    agents_config: &AgentsConfig,
    budget: super::runner::SharedBudget,
    parent_workspace: Option<String>,
    target: &str,
    foreach_file: &str,
    start: Instant,
) -> StepOutcome {
    // The fan-out is defined around the SHARED worktree (the items file +
    // current_task.json + the children's commits all live there).
    let ws = match parent_workspace {
        Some(w) if !w.trim().is_empty() => w,
        _ => return fail(step, start, "SubWorkflow foreach requires the parent to run in a git worktree (the items file lives there).".into()),
    };
    let items_path = std::path::Path::new(&ws).join(foreach_file);
    let content = match std::fs::read_to_string(&items_path) {
        Ok(c) => c,
        Err(e) => return fail(step, start, format!("Cannot read foreach file `{foreach_file}` in the worktree: {e}")),
    };
    let items = match parse_foreach_items(&content) {
        Ok(i) => i,
        Err(e) => return fail(step, start, e),
    };

    // Load the child workflow once (same definition for every item).
    let target_for_db = target.to_string();
    let child_wf = match state.db.with_conn(move |c| crate::db::workflows::get_workflow(c, &target_for_db)).await {
        Ok(Some(w)) => w,
        Ok(None) => return fail(step, start, format!("Sub-workflow `{target}` not found (deleted? wrong id?).")),
        Err(e) => return fail(step, start, format!("DB error loading sub-workflow `{target}`: {e}")),
    };

    // A2 resume reconciliation — three sources, trusted in this order:
    //   1. git ledger (`[item_id]` commit subjects, checked per-item below)
    //   2. Success child rows (a child that finished right before a crash,
    //      before its done-set entry landed)
    //   3. the done-set in run.state — NEVER trusted alone: an entry that
    //      neither the ledger nor a child row confirms is stale and the
    //      item re-runs (warn below).
    let child_done: std::collections::HashSet<String> = {
        let pid = parent_run_id.to_string();
        state.db.with_conn(move |c| crate::db::workflows::successful_child_item_ids(c, &pid))
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(target: "kronn::sub_workflow", parent_run=%parent_run_id, error=%e,
                    "cannot load Success child rows — reconciliation falls back to the git ledger only");
                Default::default()
            })
    };
    let state_done: std::collections::HashSet<String> = {
        let pid = parent_run_id.to_string();
        let key = format!("__kronn.foreach_done.{}", step.name);
        state.db.with_conn(move |c| crate::db::workflows::get_run(c, &pid)).await
            .ok().flatten()
            .and_then(|r| r.state.get(&key).cloned())
            .and_then(|doc| serde_json::from_str::<serde_json::Value>(&doc).ok())
            .and_then(|d| d.get("items").and_then(|i| i.as_array()).map(|items| {
                items.iter()
                    .filter_map(|e| e.get("id").and_then(|i| i.as_str()).map(String::from))
                    .filter(|i| !i.is_empty())
                    .collect()
            }))
            .unwrap_or_default()
    };

    let task_file = std::path::Path::new(&ws).join(".kronn/current_task.json");
    let mut results: Vec<serde_json::Value> = Vec::with_capacity(items.len());
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut total_tokens = 0u64;
    let mut last_child_id: Option<String> = None;
    let mut last_output: Option<String> = None;

    for (idx, item) in items.iter().enumerate() {
        // Parent cancel between items — stop fan-out, keep what's done.
        let cancelled = state.cancel_registry.lock()
            .ok().and_then(|m| m.get(parent_run_id).map(|t| t.is_cancelled()))
            .unwrap_or(false);
        if cancelled {
            tracing::info!(target: "kronn::sub_workflow", parent_run=%parent_run_id, "foreach cancelled at item {idx}");
            break;
        }

        let item_id = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();

        // 2026-06-12 (#3, in-run resume) — skip items ALREADY COMMITTED in
        // this worktree (commit subjects carry `[<id>]`): a Goto-retry of the
        // SubWorkflow step (gate request-changes, SUBWF_FAILED loop) then
        // re-runs ONLY the missing/failed items instead of re-implementing
        // everything. Deterministic, 0 token. Cross-RUN resume is out of
        // scope (a new run gets a fresh worktree/branch).
        if !item_id.is_empty() {
            let git_done = crate::core::cmd::async_cmd("git")
                .args(["log", "--oneline", "--fixed-strings", "--grep", &format!("[{item_id}]")])
                .current_dir(&ws)
                .output().await
                .map(|o| o.status.success() && !o.stdout.is_empty())
                .unwrap_or(false);
            if git_done || child_done.contains(&item_id) {
                tracing::info!(target: "kronn::sub_workflow", parent_run=%parent_run_id, item=%idx, item_id=%item_id,
                    source = if git_done { "git-ledger" } else { "child-row" },
                    "foreach: item already done — skipping");
                succeeded += 1;
                results.push(json!({ "item": idx, "id": item_id, "child_run_id": null, "status": "SkippedAlreadyDone" }));
                record_foreach_done(state, parent_run_id, &step.name,
                    json!({"idx": idx, "id": item_id, "status": "SkippedAlreadyDone", "child_run_id": null})).await;
                continue;
            }
            if state_done.contains(&item_id) {
                tracing::warn!(target: "kronn::sub_workflow", parent_run=%parent_run_id, item=%idx, item_id=%item_id,
                    "foreach: done-set entry not confirmed by git ledger or a Success child row — stale marker ignored, re-running the item");
            }
        }

        // Flip économique (2026-06-12) — `mechanical:true` items whose content
        // was fully planned by triage (`files[]`, human-approved at the gate)
        // are applied by the ENGINE: write + commit, ZERO agent run. Any
        // validation problem falls back to the normal agent path (graceful).
        if item.get("mechanical").and_then(|m| m.as_bool()) == Some(true) {
            match validate_mechanical_files(item) {
                Ok(files) => {
                    let mut write_err = None;
                    for (rel, content) in &files {
                        let p = std::path::Path::new(&ws).join(rel);
                        if let Some(parent) = p.parent() { let _ = std::fs::create_dir_all(parent); }
                        if let Err(e) = std::fs::write(&p, content) { write_err = Some(format!("{rel}: {e}")); break; }
                    }
                    if let Some(e) = write_err {
                        tracing::warn!(target: "kronn::sub_workflow", item_id=%item_id, "mechanical write failed ({e}) — falling back to agent");
                    } else {
                        // Journal + commit (deterministic, mirrors the child's commit step).
                        let what = item.get("what").and_then(|w| w.as_str()).unwrap_or("");
                        let _ = std::fs::OpenOptions::new().create(true).append(true)
                            .open(std::path::Path::new(&ws).join(".kronn/decisions.md"))
                            .and_then(|mut f| { use std::io::Write; writeln!(f, "\n[{item_id}] engine-applied (mechanical): {what}") });
                        let mut add = vec!["add".to_string(), ".kronn/decisions.md".to_string()];
                        add.extend(files.iter().map(|(p, _)| p.clone()));
                        let _ = crate::core::cmd::async_cmd("git").args(add.iter().map(|s| s.as_str())).current_dir(&ws).output().await;
                        let subject = format!("Kronn AutoPilot [{item_id}] — mechanical (engine-applied)");
                        let body = files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join("\n");
                        let commit = crate::core::cmd::async_cmd("git")
                            .args(["-c", "user.email=autopilot@kronn.local", "-c", "user.name=Kronn AutoPilot",
                                   "commit", "--no-verify", "-m", &subject, "-m", &body])
                            .current_dir(&ws).output().await;
                        let committed = commit.map(|o| o.status.success()).unwrap_or(false);
                        if committed {
                            tracing::info!(target: "kronn::sub_workflow", item_id=%item_id, files=files.len(), "mechanical item engine-applied (0 tokens)");
                            succeeded += 1;
                            results.push(json!({ "item": idx, "id": item_id, "child_run_id": null, "status": "MechanicalApplied", "files": files.len() }));
                            record_foreach_done(state, parent_run_id, &step.name,
                                json!({"idx": idx, "id": item_id, "status": "MechanicalApplied", "child_run_id": null})).await;
                            continue;
                        }
                        tracing::warn!(target: "kronn::sub_workflow", item_id=%item_id, "mechanical commit failed — falling back to agent");
                    }
                }
                Err(reason) => {
                    tracing::info!(target: "kronn::sub_workflow", item_id=%item_id, "mechanical not engine-appliable ({reason}) — agent path");
                }
            }
        }

        // Expose the item to the child via the shared worktree.
        if let Err(e) = std::fs::write(&task_file, serde_json::to_string_pretty(item).unwrap_or_default()) {
            // A per-item infra hiccup (transient FS error on ONE item) must NOT
            // abort the whole sweep — record it as a failed item and move on so
            // the remaining items still get processed. Only a pre-loop
            // catastrophic error (bad child workflow, unreadable foreach file)
            // aborts; inside the loop the sole early-out is parent cancellation.
            tracing::warn!(target: "kronn::sub_workflow", item=%idx, item_id=%item_id, "foreach: cannot write current_task.json ({e}) — skipping this item");
            failed += 1;
            results.push(json!({ "item": idx, "id": item_id, "child_run_id": null, "status": "SkippedWriteError", "error": e.to_string() }));
            continue;
        }

        // Build the child's trigger_context: the fan-out bookkeeping keys PLUS
        // the item flattened to `current_task.<field>` string vars so the
        // child's step templates can interpolate the item directly
        // (`{{current_task.number}}`) instead of only reading the file.
        let mut tctx = serde_json::Map::new();
        tctx.insert("__subwf_depth__".into(), json!(child_depth));
        tctx.insert("__subwf_item__".into(), json!(idx));
        tctx.insert("__subwf_item_id__".into(), json!(item_id));
        for (k, v) in current_task_template_vars(item) {
            tctx.insert(k, serde_json::Value::String(v));
        }

        let now = Utc::now();
        let mut child_run = WorkflowRun {
            id: Uuid::new_v4().to_string(),
            workflow_id: child_wf.id.clone(),
            status: RunStatus::Pending,
            trigger_context: Some(serde_json::Value::Object(tctx)),
            step_results: vec![],
            tokens_used: 0,
            workspace_path: None,
            started_at: now,
            finished_at: None,
            run_type: "subworkflow".into(),
            batch_total: 0,
            batch_completed: 0,
            batch_failed: 0,
            batch_name: None,
            parent_run_id: Some(parent_run_id.to_string()),
            state: Default::default(),
            produced_branches: vec![],
            parent_workflow_id: None,
            parent_workflow_name: None,
            parent_run_started_at: None,
        };
        let to_insert = child_run.clone();
        if let Err(e) = state.db.with_conn(move |c| crate::db::workflows::insert_run(c, &to_insert)).await {
            // Same rationale as the task-file write above: a per-item DB hiccup
            // skips THIS item, it doesn't kill the sweep.
            tracing::warn!(target: "kronn::sub_workflow", item=%idx, item_id=%item_id, "foreach: cannot create child run row ({e}) — skipping this item");
            failed += 1;
            results.push(json!({ "item": idx, "id": item_id, "child_run_id": null, "status": "SkippedDbError", "error": e.to_string() }));
            continue;
        }
        tracing::info!(target: "kronn::sub_workflow", parent_run=%parent_run_id, child_run=%child_run.id, item=%idx, item_id=%item_id, "foreach: entering child");

        // Flip économique — tier routing: low-complexity items run their Agent
        // steps on the Economy tier (cheap model), high on Reasoning. Only
        // overrides steps WITHOUT an explicit tier (author choice wins).
        let mut wf_for_item = child_wf.clone();
        if let Some(tier) = tier_for_complexity(item.get("complexity").and_then(|c| c.as_str())) {
            for s in wf_for_item.steps.iter_mut().filter(|s| matches!(s.step_type, crate::models::StepType::Agent)) {
                match s.agent_settings.as_mut() {
                    Some(st) if st.tier.is_none() => st.tier = Some(tier),
                    None => s.agent_settings = Some(crate::models::AgentSettings {
                        model: None, tier: Some(tier), reasoning_effort: None, max_tokens: None,
                    }),
                    _ => {}
                }
            }
        }

        let exec_res = Box::pin(crate::workflows::runner::execute_run(
            state.clone(), &wf_for_item, &mut child_run, tokens_config, agents_config,
            None, Some(budget.clone()), Some(ws.clone()),
        )).await;
        if let Err(e) = exec_res {
            tracing::warn!(target: "kronn::sub_workflow", child_run=%child_run.id, "foreach child errored: {e}");
        }

        let ok = matches!(child_run.status, RunStatus::Success);
        if ok { succeeded += 1; } else { failed += 1; }
        total_tokens += child_run.tokens_used;
        last_output = child_run.step_results.last().map(|s| s.output.clone());
        last_child_id = Some(child_run.id.clone());
        results.push(json!({
            "item": idx,
            "id": item_id,
            "child_run_id": child_run.id,
            "status": format!("{:?}", child_run.status),
        }));
        // Done ONLY on child Success — a Failed child must be re-attempted
        // by a resume, never skipped on the strength of a stale marker.
        if ok {
            record_foreach_done(state, parent_run_id, &step.name,
                json!({"idx": idx, "id": item_id, "status": "Success", "child_run_id": child_run.id})).await;
        }
    }
    let _ = std::fs::remove_file(&task_file); // best-effort cleanup

    let (status_str, signal) = aggregate_foreach(succeeded, failed);
    let summary = format!(
        "Sous-workflow « {} » × {} tâche(s) → {} ok / {} échec(s) ({} tokens)",
        child_wf.name, results.len(), succeeded, failed, total_tokens,
    );
    let data = json!({
        "mode": "foreach",
        "child_workflow_id": child_wf.id,
        "total": results.len(),
        "succeeded": succeeded,
        "failed": failed,
        "items": results,
        "child_run_id": last_child_id,
        "last_output": last_output,
    });
    let output = super::step_output_format::format_step_output(data, status_str, &summary, None, &[signal]);
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(|a| match a {
        crate::models::ConditionAction::Stop => "Stop".to_string(),
        crate::models::ConditionAction::Skip => "Skip".to_string(),
        crate::models::ConditionAction::Goto { step_name, .. } => format!("Goto:{step_name}"),
    });

    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            // PARTIAL (some items ok) is SUCCESS at the step level — the run
            // must reach pr_draft so the failure is DOCUMENTED in the PR body
            // (run-4 live finding: 12/13 ok killed the whole run + lost the
            // draft). The PARTIAL signal stays branchable via on_result for
            // workflows that want to retry instead. All-failed stays Failed.
            status: if succeeded > 0 { RunStatus::Success } else { RunStatus::Failed },
            output,
            tokens_used: total_tokens,
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: None,
            condition_result,
            envelope_detected: Some(true),
            step_kind: None,
            step_agent: None,
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: last_child_id,
        },
        condition_action,
    }
}

/// SubWorkflow is forbidden in an `on_failure` rollback chain (spec arbitrage
/// B + save-validation). This is the defence-in-depth path for a hand-edited
/// JSON that slipped a SubWorkflow into `on_failure`: fail loudly, never
/// recurse from a compensation step.
pub fn forbidden_in_rollback(step: &WorkflowStep) -> StepOutcome {
    fail(
        step,
        Instant::now(),
        "SubWorkflow n'est pas autorisé dans une chaîne on_failure (rollback) — un sous-pipeline en compensation invite aux boucles/deadlocks.".into(),
    )
}

fn fail(step: &WorkflowStep, start: Instant, msg: String) -> StepOutcome {
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output: msg,
            tokens_used: 0,
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
        },
        condition_action: None,
    }
}

/// True when recursing one level below `current_depth` would exceed the cap.
/// Pure (the executor itself needs `AppState`, so this carries the rule for
/// unit testing). Runtime backstop to the save-time static depth check.
fn child_depth_exceeds(current_depth: usize) -> bool {
    current_depth + 1 > MAX_SUBWORKFLOW_DEPTH
}

#[cfg(test)]
mod tests {
    use super::child_depth_exceeds;
    use super::{aggregate_foreach, parse_foreach_items, MAX_SUBWORKFLOW_DEPTH};

    // ── A2 — foreach resume reconciliation (3 sources) ──────────────────

    fn step_json(v: serde_json::Value) -> crate::models::WorkflowStep {
        serde_json::from_value(v).expect("minimal step JSON")
    }

    /// Parent Running run + trivial JsonData child workflow + a git-init'd
    /// worktree holding `tasks.json` with items T1/T2. Returns everything a
    /// reconciliation test needs to call `execute_sub_workflow_step`.
    async fn foreach_fixture() -> (
        crate::AppState,
        crate::models::TokensConfig,
        crate::models::AgentsConfig,
        tempfile::TempDir,
        crate::models::WorkflowStep,
    ) {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let cfg = crate::core::config::default_config();
        let tokens = cfg.tokens.clone();
        let agents = cfg.agents.clone();
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(cfg));
        let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);

        // Child workflow: one deterministic JsonData step — no LLM, no project.
        let child_wf = crate::models::Workflow {
            pinned: false,
            id: "child-wf".into(),
            name: "child".into(),
            project_id: None,
            trigger: crate::models::WorkflowTrigger::Manual,
            steps: vec![step_json(serde_json::json!({
                "name": "emit", "step_type": {"type": "JsonData"},
                "json_data_payload": { "done": true },
            }))],
            actions: vec![],
            safety: crate::models::WorkflowSafety { sandbox: false, max_files: None, max_lines: None, require_approval: false },
            workspace_config: None,
            concurrency_limit: None,
            guards: None,
            artifacts: Default::default(),
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![],
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let parent_run = crate::models::WorkflowRun {
            id: "parent-run".into(),
            workflow_id: "parent-wf".into(),
            status: crate::models::RunStatus::Running,
            trigger_context: None,
            step_results: vec![],
            tokens_used: 0,
            workspace_path: None,
            started_at: chrono::Utc::now(),
            finished_at: None,
            run_type: "linear".into(),
            batch_total: 0,
            batch_completed: 0,
            batch_failed: 0,
            batch_name: None,
            parent_run_id: None,
            state: Default::default(),
            produced_branches: vec![],
            parent_workflow_id: None,
            parent_workflow_name: None,
            parent_run_started_at: None,
        };
        // FK: the parent run's workflow row must exist too.
        let mut parent_wf = child_wf.clone();
        parent_wf.id = "parent-wf".into();
        state.db.with_conn(move |c| {
            crate::db::workflows::insert_workflow(c, &child_wf)?;
            crate::db::workflows::insert_workflow(c, &parent_wf)?;
            crate::db::workflows::insert_run(c, &parent_run)
        }).await.unwrap();

        // Worktree stand-in: a real git repo with the items file.
        let ws = tempfile::TempDir::new().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args).current_dir(ws.path()).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        std::fs::create_dir_all(ws.path().join(".kronn")).unwrap();
        std::fs::write(
            ws.path().join("tasks.json"),
            r#"[{"id":"T1","what":"a"},{"id":"T2","what":"b"}]"#,
        ).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "init"]);

        let step = step_json(serde_json::json!({
            "name": "fanout", "step_type": {"type": "SubWorkflow"},
            "sub_workflow_id": "child-wf",
            "sub_workflow_foreach_file": "tasks.json",
        }));
        (state, tokens, agents, ws, step)
    }

    async fn run_foreach(
        state: &crate::AppState,
        tokens: &crate::models::TokensConfig,
        agents: &crate::models::AgentsConfig,
        ws: &tempfile::TempDir,
        step: &crate::models::WorkflowStep,
    ) -> serde_json::Value {
        let outcome = super::execute_sub_workflow_step(
            state, "parent-run", 0, step, tokens, agents,
            crate::workflows::runner::SharedBudget::root(50),
            Some(ws.path().to_string_lossy().to_string()),
        ).await;
        assert_eq!(outcome.result.status, crate::models::RunStatus::Success,
            "foreach must succeed: {}", outcome.result.output);
        crate::workflows::step_output_format::parse_envelope_for_test(&outcome.result.output)["data"].clone()
    }

    async fn child_rows_for(state: &crate::AppState, item_id: &str) -> usize {
        let iid = item_id.to_string();
        state.db.with_conn(move |c| {
            let mut stmt = c.prepare(
                "SELECT trigger_context FROM workflow_runs WHERE parent_run_id = 'parent-run'",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, Option<String>>(0))?;
            let mut n = 0;
            for row in rows {
                if row?.as_deref().map(|t| t.contains(&format!("\"{iid}\""))).unwrap_or(false) {
                    n += 1;
                }
            }
            Ok(n)
        }).await.unwrap()
    }

    #[tokio::test]
    async fn foreach_skips_items_already_in_the_git_ledger() {
        let (state, tokens, agents, ws, step) = foreach_fixture().await;
        // The crash happened after T1's commit landed.
        let out = std::process::Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=t",
                   "commit", "-q", "--allow-empty", "-m", "Kronn AutoPilot [T1] done"])
            .current_dir(ws.path()).output().unwrap();
        assert!(out.status.success());

        let data = run_foreach(&state, &tokens, &agents, &ws, &step).await;
        let items = data["items"].as_array().unwrap();
        assert_eq!(items[0]["status"], "SkippedAlreadyDone", "T1 confirmed by the ledger");
        assert_eq!(items[1]["status"], "Success", "T2 ran its child");
        assert_eq!(child_rows_for(&state, "T1").await, 0, "no duplicate child for T1");
        assert_eq!(child_rows_for(&state, "T2").await, 1);
    }

    #[tokio::test]
    async fn foreach_rebuilds_from_a_success_child_row_without_duplicating() {
        let (state, tokens, agents, ws, step) = foreach_fixture().await;
        // Crash window: T1's child finished (row Success) but the parent died
        // BEFORE writing the done-set entry — and its child made no commit.
        state.db.with_conn(|c| {
            c.execute(
                "INSERT INTO workflow_runs (id, workflow_id, status, run_type, parent_run_id, trigger_context, started_at)
                 VALUES ('pre-child', 'child-wf', 'Success', 'subworkflow', 'parent-run',
                         '{\"__subwf_item_id__\":\"T1\"}', datetime('now'))",
                [],
            ).map_err(Into::into)
        }).await.unwrap();

        let data = run_foreach(&state, &tokens, &agents, &ws, &step).await;
        let items = data["items"].as_array().unwrap();
        assert_eq!(items[0]["status"], "SkippedAlreadyDone", "T1 rebuilt from the child row");
        assert_eq!(items[1]["status"], "Success");
        assert_eq!(child_rows_for(&state, "T1").await, 1, "only the pre-crash child — no duplicate");
    }

    #[tokio::test]
    async fn foreach_ignores_a_stale_done_set_entry_and_reruns_the_item() {
        let (state, tokens, agents, ws, step) = foreach_fixture().await;
        // A done-set entry NOBODY confirms (no commit, no child row) — e.g. a
        // hand-edited or corrupted state. Trusting it would silently drop T1.
        state.db.with_conn(|c| {
            crate::db::workflows::set_run_state_key(
                c, "parent-run", "__kronn.foreach_done.fanout",
                r#"{"v":1,"items":[{"idx":0,"id":"T1","status":"Success","child_run_id":"ghost"}]}"#,
                &[crate::models::RunStatus::Running],
            ).map(|_| ())
        }).await.unwrap();

        let data = run_foreach(&state, &tokens, &agents, &ws, &step).await;
        let items = data["items"].as_array().unwrap();
        assert_eq!(items[0]["status"], "Success", "stale marker ignored — T1 re-ran");
        assert_eq!(child_rows_for(&state, "T1").await, 1, "T1 got a real child this time");
    }

    #[test]
    fn depth_guard_allows_up_to_cap_and_refuses_beyond() {
        // From depth 0 we can descend until the child reaches the cap.
        assert!(!child_depth_exceeds(0));
        assert!(!child_depth_exceeds(MAX_SUBWORKFLOW_DEPTH - 1)); // child == cap, allowed
        assert!(child_depth_exceeds(MAX_SUBWORKFLOW_DEPTH));      // child == cap+1, refused
        assert!(child_depth_exceeds(MAX_SUBWORKFLOW_DEPTH + 5));
    }

    // ── Phase 3b — foreach helpers ──────────────────────────────────────

    #[test]
    fn parse_foreach_accepts_array_of_objects() {
        let items = parse_foreach_items(r#"[{"id":"a","what":"x"},{"id":"b"}]"#).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], "a");
    }

    #[test]
    fn parse_foreach_rejects_non_array_empty_and_oversized() {
        assert!(parse_foreach_items(r#"{"id":"a"}"#).is_err(), "object is not a work-list");
        assert!(parse_foreach_items("not json").is_err());
        assert!(parse_foreach_items("[]").unwrap_err().contains("empty"));
        let big = format!("[{}]", vec!["{}"; 31].join(","));
        assert!(parse_foreach_items(&big).unwrap_err().contains("cap"));
    }

    #[test]
    fn aggregate_foreach_maps_outcomes_to_signals() {
        assert_eq!(aggregate_foreach(3, 0), ("OK", "OK"));
        assert_eq!(aggregate_foreach(2, 1), ("PARTIAL", "PARTIAL"));
        assert_eq!(aggregate_foreach(0, 3), ("SUBWF_FAILED", "SUBWF_FAILED"));
    }

    #[test]
    fn current_task_vars_expose_scalars_for_templating() {
        let item = serde_json::json!({
            "id": "pr-42", "number": 42, "draft": false,
            "title": "Fix bug", "labels": ["a", "b"], "meta": null,
        });
        let vars: std::collections::HashMap<_, _> =
            super::current_task_template_vars(&item).into_iter().collect();
        // scalar fields stringify so `{{current_task.number}}` resolves to "42"
        assert_eq!(vars["current_task.number"], "42");
        assert_eq!(vars["current_task.id"], "pr-42");
        assert_eq!(vars["current_task.title"], "Fix bug");
        assert_eq!(vars["current_task.draft"], "false");
        // null → empty string (no literal "null" leaking into a path)
        assert_eq!(vars["current_task.meta"], "");
        // nested arrays/objects render as compact JSON
        assert_eq!(vars["current_task.labels"], "[\"a\",\"b\"]");
        // the whole item is also available
        assert!(vars["current_task"].contains("\"number\":42"));
    }

    #[test]
    fn current_task_vars_tolerate_a_non_object_item() {
        // a bare-scalar work-list item (e.g. `["EW-1","EW-2"]`) still yields
        // the whole-item var without panicking.
        let vars: std::collections::HashMap<_, _> =
            super::current_task_template_vars(&serde_json::json!("EW-1")).into_iter().collect();
        assert_eq!(vars["current_task"], "\"EW-1\"");
        assert!(!vars.keys().any(|k| k.starts_with("current_task.")));
    }

    // ── Flip économique — tier routing + mechanical files ───────────────

    #[test]
    fn tier_for_complexity_maps_low_high_only() {
        use crate::models::ModelTier;
        assert!(matches!(super::tier_for_complexity(Some("low")), Some(ModelTier::Economy)));
        assert!(matches!(super::tier_for_complexity(Some("high")), Some(ModelTier::Reasoning)));
        assert!(super::tier_for_complexity(Some("med")).is_none());
        assert!(super::tier_for_complexity(None).is_none());
    }

    #[test]
    fn mechanical_files_validated_and_guarded() {
        use serde_json::json;
        // happy path: clear mechanical item, no marker required
        let ok = json!({"id":"a","mechanical":true,"files":[{"path":"config/a.yaml","content":"x: 1\n"}]});
        assert_eq!(super::validate_mechanical_files(&ok).unwrap().len(), 1);
        // decided item must embed its marker
        let dec = json!({"id":"a","chosen":"x","files":[{"path":"c.yaml","content":"no marker"}]});
        assert!(super::validate_mechanical_files(&dec).unwrap_err().contains("marker"));
        let dec_ok = json!({"id":"a","chosen":"x","files":[{"path":"c.yaml","content":"# KRONN-ASSUMED(a): x\nv: 1\n"}]});
        assert!(super::validate_mechanical_files(&dec_ok).is_ok());
        // path traversal / absolute / .git rejected
        for bad in ["../evil", "/etc/passwd", ".git/hooks/pre-commit"] {
            let item = json!({"id":"a","files":[{"path": bad, "content":"x"}]});
            assert!(super::validate_mechanical_files(&item).unwrap_err().contains("rejected"), "{bad}");
        }
        // missing files[] → fall back to agent (error string)
        assert!(super::validate_mechanical_files(&json!({"id":"a"})).is_err());
    }
}
