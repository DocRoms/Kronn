//! Executor for `StepType::BatchQuickPrompt` (Phase 2 batch workflows).
//!
//! Fan out a Quick Prompt over a list of items rendered from a previous
//! step's output. Each item becomes one child discussion linked to a batch
//! `workflow_runs` row (`parent_run_id = current_linear_run.id`), then agents
//! are spawned on each child in parallel. If `wait_for_completion` is true
//! (default), the step blocks until a `BatchRunFinished` WS event matches the
//! child batch's id — no polling, reusing the broadcast machinery from Phase 1b.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::models::*;
use crate::AppState;

use super::steps::StepOutcome;
use super::template::TemplateContext;

/// Safety timeout for `wait_for_completion`: the linear run will abandon the
/// wait after this many hours even if no WS signal arrives. Sized to cover
/// the worst-case agent latency for ~50 items.
const BATCH_WAIT_TIMEOUT: Duration = Duration::from_secs(2 * 3600);

/// Default cap on number of items when `batch_max_items` is unset. Matches
/// the same 50-item hard cap enforced by `create_batch_run` for manual batches.
const DEFAULT_MAX_ITEMS: u32 = 50;

/// Default per-batch concurrent agent cap when `batch_concurrent_limit` is
/// unset. The global `max_concurrent_agents` semaphore already throttles the
/// system as a whole, but on a single big batch (17+ Jira tickets) the
/// global cap stays saturated for the whole run — that creates auth file
/// contention (multiple `claude` processes writing `~/.claude/state.json`),
/// network pool pressure, and amplifies the silent-CLI-crash failure mode.
/// 5 in flight at once is a sweet spot: the agents finish the batch in
/// ~⌈N/5⌉ waves, errors stay isolated, the global pool has room left for a
/// concurrent linear workflow. Operators can override per-step.
const DEFAULT_BATCH_CONCURRENT_LIMIT: u32 = 5;
/// Hard ceiling regardless of operator override. Matches the `batch_apicall`
/// hard cap and keeps a single batch from owning every agent slot.
const MAX_BATCH_CONCURRENT_LIMIT: u32 = 20;

pub async fn execute_batch_quick_prompt_step(
    step: &WorkflowStep,
    parent_run_id: &str,
    state: AppState,
    ctx: &TemplateContext,
) -> StepOutcome {
    let start = Instant::now();

    // ── Validate required fields ────────────────────────────────────────
    let qp_id = match step.batch_quick_prompt_id.as_ref() {
        Some(id) if !id.is_empty() => id.clone(),
        _ => return fail(step, start, "BatchQuickPrompt step missing `batch_quick_prompt_id`"),
    };
    let items_from = match step.batch_items_from.as_ref() {
        Some(s) if !s.is_empty() => s.clone(),
        _ => return fail(step, start, "BatchQuickPrompt step missing `batch_items_from`"),
    };
    let wait_for_completion = step.batch_wait_for_completion.unwrap_or(true);
    let max_items = step.batch_max_items.unwrap_or(DEFAULT_MAX_ITEMS);
    let workspace_mode = step.batch_workspace_mode
        .clone()
        .unwrap_or_else(|| "Direct".to_string());

    // ── Render the items expression via the template engine ─────────────
    // The expression can produce either a JSON array (preferred — from a
    // `StepOutputFormat::Structured` upstream step) or plain text with one
    // item per line. Both are accepted.
    let rendered = match ctx.render(&items_from) {
        Ok(s) => s,
        Err(e) => return fail(step, start, format!("Template render error on items_from: {}", e)),
    };

    let items = parse_items_rich(&rendered);
    if items.is_empty() {
        return fail(step, start, "BatchQuickPrompt: `batch_items_from` resolved to an empty list");
    }
    if items.len() > max_items as usize {
        return fail(step, start, format!(
            "BatchQuickPrompt: {} items exceeds max {} (use `batch_max_items` to raise the cap)",
            items.len(), max_items
        ));
    }

    // ── Load the Quick Prompt ───────────────────────────────────────────
    let qp_lookup = qp_id.clone();
    let qp = match state.db.with_conn(move |conn| {
        crate::db::quick_prompts::get_quick_prompt(conn, &qp_lookup)
    }).await {
        Ok(Some(q)) => q,
        Ok(None) => return fail(step, start, format!("Quick prompt '{}' not found", qp_id)),
        Err(e) => return fail(step, start, format!("DB error loading QP: {}", e)),
    };

    // ── Safety: Isolated mode requires a project_id (git repo to worktree) ──
    // The check is done AFTER loading the QP so we can fall back to the QP's
    // own project_id if the workflow step doesn't have one. If neither has it,
    // fail early with a clear message — better than letting each child disc
    // crash at run time when make_agent_stream tries to locate the repo.
    if workspace_mode == "Isolated" && qp.project_id.is_none() {
        return fail(step, start,
            "Isolated workspace mode requires a project-linked Quick Prompt — \
             no git repo available to create per-discussion worktrees. \
             Either attach the QP to a project, or switch back to Direct mode."
        );
    }

    // Render the QP template for each item. Two item shapes are supported:
    //   • JSON object — its fields map onto the QP's `{{var}}` placeholders by
    //     NAME (multi-variable, mirroring the MCP `qp_batch_run` path), and the
    //     disc title comes from a dedicated field (`_title` / `id` / `key` /
    //     `number`) so the sidebar stays readable instead of showing the whole
    //     payload.
    //   • Scalar string — fills the QP's FIRST variable and doubles as the disc
    //     title (back-compat with `["EW-1","EW-2"]`-style item lists).
    // `item_titles` mirrors `batch_items` by index: it carries the clean
    // identifier (the disc title — `id` for objects, the string itself for
    // scalars) that is later handed to each child as its chain-QP first
    // variable (`batch_item`), so a chained apply-framing QP receives the
    // ticket id rather than the whole payload.
    let first_var_name = qp.variables.first().map(|v| v.name.clone());
    let mut batch_items: Vec<crate::db::workflows::BatchItemInput> = Vec::with_capacity(items.len());
    let mut item_titles: Vec<String> = Vec::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        let (vars, title) = item_to_vars_and_title(item, first_var_name.as_deref(), &qp.name, idx);
        item_titles.push(title.clone());
        batch_items.push(crate::db::workflows::BatchItemInput {
            title,
            prompt: render_qp_template_vars(&qp.prompt_template, &vars),
            agent_override: None, // workflow-step batch keeps the QP's default agent
        });
    }

    // ── Pseudo/avatar for message attribution ───────────────────────────
    let (author_pseudo, author_avatar_email) = {
        let cfg = state.config.read().await;
        (cfg.server.pseudo.clone(), cfg.server.avatar_email.clone())
    };

    // ── Resolve parent workflow name + run sequence for a richer batch_name ─
    // Goal: when the sidebar shows multiple batches of the same QP triggered by
    // the same workflow (e.g. an hourly cron), the user can tell them apart at
    // a glance. Format: "{workflow} · run #{N} · {qp} · DD MMM HH:MM:SS".
    //
    // All three lookups (parent run → workflow → runs count) run inside the
    // same `with_conn` closure to avoid acquiring the DB lock 3 times.
    let parent_id = parent_run_id.to_string();
    let parent_meta = {
        let parent_id_for_lookup = parent_id.clone();
        state.db.with_conn(move |conn| {
            let parent_run = crate::db::workflows::get_run(conn, &parent_id_for_lookup)?;
            let Some(parent_run) = parent_run else {
                return Ok::<_, anyhow::Error>(None);
            };
            let workflow = crate::db::workflows::get_workflow(conn, &parent_run.workflow_id)?;
            let run_count = crate::db::workflows::count_runs(conn, &parent_run.workflow_id)?;
            // `count_runs` includes the current linear run itself, so its index
            // is the count (1-based). This is stable even if the user deletes
            // older runs later — the number printed at batch-creation time
            // reflects reality at that moment.
            Ok(Some((workflow.map(|w| w.name), run_count)))
        }).await.ok().flatten()
    };

    let now_stamp = chrono::Utc::now().format("%d %b %H:%M:%S");
    let batch_name = Some(match parent_meta.as_ref() {
        Some((Some(wf_name), run_seq)) => format!(
            "{} · run #{} · {} · {}", wf_name, run_seq, qp.name, now_stamp
        ),
        // Parent run exists but workflow was deleted or placeholder → fall back
        // to run sequence only. Still useful to disambiguate from sibling batches.
        Some((None, run_seq)) => format!(
            "run #{} · {} · {}", run_seq, qp.name, now_stamp
        ),
        // Parent run not found (shouldn't happen in practice since we just
        // created it upstream). Keep the old format so we never crash.
        None => format!("{} · {}", qp.name, now_stamp),
    });
    let qp_for_tx = qp.clone();
    let workspace_mode_for_tx = workspace_mode.clone();
    let outcome = match state.db.with_conn(move |conn| {
        crate::db::workflows::create_batch_run(conn, crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp_for_tx,
            items: batch_items,
            batch_name,
            project_id: qp_for_tx.project_id.clone(),
            parent_run_id: Some(parent_id),
            author_pseudo,
            author_avatar_email,
            language: "fr".into(),
            workspace_mode: workspace_mode_for_tx,
        })
    }).await {
        Ok(o) => o,
        Err(e) => return fail(step, start, format!("Failed to create batch run: {}", e)),
    };

    tracing::info!(
        "BatchQuickPrompt step '{}' created child batch run {} with {} discussions (parent: {})",
        step.name, outcome.run_id, outcome.batch_total, parent_run_id
    );

    // ── Queued state for EVERY child, up front ─────────────────────
    // The semaphore below throttles agents, so most children sit QUEUED
    // (created but not yet streaming) for a while. Broadcast a QUEUED signal
    // for all children NOW so none looks crashed — but a distinct one from
    // "started", so the sidebar can show "en file (n/N)" vs "en cours"
    // instead of N identical spinners. Each child's real
    // `BatchRunChildStarted` (streaming.rs, when its agent actually begins)
    // flips it to running; `BatchRunProgress`/`Finished` clear it.
    for disc_id in &outcome.discussion_ids {
        let _ = state.ws_broadcast.send(WsMessage::BatchRunChildQueued {
            run_id: outcome.run_id.clone(),
            discussion_id: disc_id.clone(),
        });
    }

    // ── Subscribe to ws_broadcast BEFORE firing the agents ──────────────
    // Ordering matters: if we subscribed after spawning, a fast disc could
    // finish before we're listening and we'd miss its BatchRunFinished event.
    // `broadcast::Receiver::subscribe` captures messages from this point on.
    let mut ws_rx = state.ws_broadcast.subscribe();

    // ── Fan out: spawn agent runs on every child discussion ─────────────
    // Throttled by a per-batch semaphore so a single big batch (17+ items)
    // doesn't saturate the global agent pool. The global
    // `max_concurrent_agents` cap still applies on top — actual concurrency
    // is `min(global, per_batch)`. Without this throttle, the spawn loop
    // hands every child to the runtime instantly; the global semaphore's
    // queue grows long, all children share a single auth-file mutex
    // (`~/.claude/state.json`), and the network pool reaches its ceiling —
    // amplifying the Claude Code silent-exit failure mode.
    //
    // If `batch_chain_prompt_ids` is non-empty, each discussion will
    // sequentially execute the chained QPs after the initial response —
    // all inside the same conversation thread. Each chain QP receives the
    // SAME raw batch item (e.g. "EW-1234") as its first variable, so an
    // `analyse → review → summary` chain all runs on the same ticket.
    // The batch progress counter only bumps when the ENTIRE chain
    // finishes (the last `make_agent_stream` hits the batch_run_id hook
    // in discussions.rs).
    let concurrent_limit = step
        .batch_concurrent_limit
        .unwrap_or(DEFAULT_BATCH_CONCURRENT_LIMIT)
        .clamp(1, MAX_BATCH_CONCURRENT_LIMIT);
    let batch_semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrent_limit as usize));
    let chain_ids = step.batch_chain_prompt_ids.clone();
    // `outcome.discussion_ids` is ordered identically to `items` (see
    // `create_batch_run` in db/workflows.rs), so zipping by index is safe.
    for (idx, disc_id) in outcome.discussion_ids.iter().enumerate() {
        let batch_item = item_titles.get(idx).cloned();
        let permit_holder = batch_semaphore.clone();
        let state_for_spawn = state.clone();
        let chain_for_spawn = chain_ids.clone();
        let disc_for_spawn = disc_id.clone();
        // Acquire the batch slot synchronously here so we block the spawn
        // loop until there's room — without this, all N tokio tasks fire
        // immediately and queue inside the global semaphore.
        let permit = match permit_holder.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => break, // semaphore closed (shouldn't happen)
        };
        tokio::spawn(async move {
            // Permit is dropped when this task ends, freeing the next
            // discussion's slot. Hold for the full duration of the agent
            // run so the global pool isn't oversubscribed.
            let _permit = permit;
            crate::api::discussions::spawn_agent_run_with_chain(
                state_for_spawn,
                disc_for_spawn,
                chain_for_spawn,
                batch_item,
            ).await;
        });
        let _ = idx; // index already consumed via zip
    }

    // ── Wait for completion (optional) ──────────────────────────────────
    if !wait_for_completion {
        // Fire-and-forget mode: emit a best-effort structured envelope now
        // and return. Downstream steps won't know the actual counters.
        let output = build_structured_output(
            &outcome.run_id,
            outcome.batch_total,
            0, 0, // not yet known
            &outcome.discussion_ids,
            false, // incomplete
        );
        return StepOutcome {
            result: StepResult {
                step_name: step.name.clone(),
                status: RunStatus::Success,
                output,
                tokens_used: 0,
                duration_ms: start.elapsed().as_millis() as u64,
                started_at: None,
            condition_result: None,
                // build_structured_output always emits a data/status/summary
                // payload that `set_step_output` can extract via Strategy 2
                // — the contract is met even without `---STEP_OUTPUT---`.
                envelope_detected: Some(true),
                step_kind: None,
                step_agent: None,
                step_model: None,
                step_api_plugin_slug: None,
                step_api_endpoint_path: None,
                is_rollback: false,
                child_run_id: None,
            },
            condition_action: None,
        };
    }

    let child_run_id = outcome.run_id.clone();
    let wait_deadline = tokio::time::Instant::now() + BATCH_WAIT_TIMEOUT;
    let final_total: u32;
    let final_ok: u32;
    let final_failed: u32;

    tracing::info!(
        "BatchQuickPrompt step '{}' waiting on BatchRunFinished for child batch {}",
        step.name, child_run_id
    );

    loop {
        let recv = tokio::time::timeout_at(wait_deadline, ws_rx.recv()).await;
        match recv {
            Ok(Ok(WsMessage::BatchRunFinished { run_id, batch_total, batch_completed, batch_failed, .. }))
                if run_id == child_run_id =>
            {
                final_total = batch_total;
                final_ok = batch_completed;
                final_failed = batch_failed;
                tracing::info!(
                    "BatchQuickPrompt step '{}' completed: {}/{} ok, {} failed",
                    step.name, final_ok, final_total, final_failed
                );
                break;
            }
            Ok(Ok(_other_msg)) => {
                // Unrelated broadcast — keep listening.
                continue;
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                tracing::warn!(
                    "BatchQuickPrompt step '{}' lagged {} WS messages — re-checking child run in DB",
                    step.name, n
                );
                // The dropped window may have held the SINGLE terminal
                // BatchRunFinished (emitted exactly once) — without this
                // re-read the step would wait the full 2h timeout and be
                // marked Failed while the child batch is actually Success.
                let cid = child_run_id.clone();
                match state.db.with_conn(move |conn| crate::db::workflows::get_run(conn, &cid)).await {
                    Ok(Some(child)) if matches!(
                        child.status,
                        crate::models::RunStatus::Success
                            | crate::models::RunStatus::Failed
                            | crate::models::RunStatus::Cancelled
                            | crate::models::RunStatus::StoppedByGuard
                            | crate::models::RunStatus::Interrupted
                    ) => {
                        final_total = child.batch_total;
                        final_ok = child.batch_completed;
                        final_failed = child.batch_failed;
                        tracing::info!(
                            "BatchQuickPrompt step '{}' recovered terminal state from DB after lag: {}/{} ok, {} failed",
                            step.name, final_ok, final_total, final_failed
                        );
                        break;
                    }
                    _ => continue, // still running (or transient DB miss) — keep listening
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                return fail(step, start, "WS broadcast channel closed while waiting for batch completion");
            }
            Err(_elapsed) => {
                return fail(step, start, format!(
                    "Timed out after {:?} waiting for child batch {} to finish",
                    BATCH_WAIT_TIMEOUT, child_run_id
                ));
            }
        }
    }

    // ── Build the structured output envelope ────────────────────────────
    let output = build_structured_output(
        &outcome.run_id,
        final_total,
        final_ok,
        final_failed,
        &outcome.discussion_ids,
        true,
    );

    // The step succeeds if AT LEAST one child succeeded — matches the
    // semantics used by `increment_batch_progress` for the child batch run
    // itself. A 0/N batch propagates failure to the linear run.
    let step_status = if final_ok > 0 { RunStatus::Success } else { RunStatus::Failed };

    // 2026-06-10 audit P1 — honour declared `on_result` rules (the runner
    // only acts on `outcome.condition_action`; None = rules silently dead).
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(condition_label);
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: step_status,
            output,
            tokens_used: 0,
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
            child_run_id: None,
        },
        condition_action,
    }
}

/// Human label for a fired condition (same convention as ApiCall/Notify).
fn condition_label(a: &crate::models::ConditionAction) -> String {
    use crate::models::ConditionAction;
    match a {
        ConditionAction::Stop => "Stop".to_string(),
        ConditionAction::Skip => "Skip".to_string(),
        ConditionAction::Goto { step_name, .. } => format!("Goto:{step_name}"),
    }
}

/// Produce a `StepOutcome` in the failed state with the given error text.
fn fail(step: &WorkflowStep, start: Instant, msg: impl Into<String>) -> StepOutcome {
    let output: String = msg.into();
    // Failures honour `on_result` recovery rules too (audit P1).
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(condition_label);
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output,
            tokens_used: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            started_at: None,
            condition_result,
            envelope_detected: None,
            step_kind: None,
            step_agent: None,
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: None,
        },
        condition_action,
    }
}

/// Public re-export for the dry-run preview endpoint
/// (`POST /api/workflows/test-batch-step`). Same logic as the runtime path
/// but exposed so we can preview without touching the DB.
pub fn parse_items_for_test(rendered: &str) -> Vec<String> { parse_items(rendered) }

/// Public re-export for the dry-run preview endpoint — see `parse_items_for_test`.
pub fn render_qp_prompt_for_test(template: &str, first_var_name: Option<&str>, value: &str) -> String {
    render_qp_prompt(template, first_var_name, value)
}

/// Parse the rendered `items_from` expression into a list of item strings.
///
/// Accepts five shapes (in order of precedence):
/// 1. JSON array of strings: `["EW-1", "EW-2", "EW-3"]`
/// 2. JSON array of objects with an `id`/`key`/`number` field (tracker shape)
/// 3. JSON object containing a single array field — `{"tickets":[...]}` —
///    we unwrap it and parse the inner array. Common when the upstream step
///    has `output_format: Structured` and renders to `{{steps.X.data}}`
///    which evaluates to the data envelope object, not the array directly.
/// 4. JSON object that IS itself the data envelope `{data, status, summary}` —
///    we look inside `data` for an array (handles `{{steps.X}}` style render
///    that captures the full envelope).
/// 5. Plain text, one item per line OR comma/semicolon-separated.
///
/// Empty lines and whitespace are trimmed. Duplicates are deduped in order.
fn parse_items(rendered: &str) -> Vec<String> {
    let trimmed = rendered.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    // Helper: extract items from a JSON array (shape 1 or 2)
    fn from_array(arr: &[serde_json::Value]) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for v in arr {
            let item = match v {
                serde_json::Value::String(s) => s.trim().to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Object(m) => {
                    m.get("id").or_else(|| m.get("key")).or_else(|| m.get("number"))
                        .and_then(|v| match v {
                            serde_json::Value::String(s) => Some(s.trim().to_string()),
                            serde_json::Value::Number(n) => Some(n.to_string()),
                            _ => None,
                        })
                        .unwrap_or_default()
                }
                _ => String::new(),
            };
            if !item.is_empty() && seen.insert(item.clone()) {
                out.push(item);
            }
        }
        out
    }

    // Helper: scan a JSON object's values for the first array field — used
    // for shapes 3 and 4. Returns None if no array field exists.
    fn first_array_in_object(m: &serde_json::Map<String, serde_json::Value>) -> Option<&Vec<serde_json::Value>> {
        m.values().find_map(|v| v.as_array())
    }

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
        match &json {
            // Shape 1 + 2: JSON array directly
            serde_json::Value::Array(arr) => return from_array(arr),
            // Shape 3 + 4: JSON object — look for inner array
            serde_json::Value::Object(m) => {
                // Shape 4: full envelope `{data, status, summary}` with array inside data
                if let Some(serde_json::Value::Object(data_obj)) = m.get("data") {
                    if let Some(arr) = first_array_in_object(data_obj) {
                        return from_array(arr);
                    }
                }
                if let Some(serde_json::Value::Array(arr)) = m.get("data") {
                    return from_array(arr);
                }
                // Shape 3: bare object with one array field, e.g. `{"tickets":[...]}`
                if let Some(arr) = first_array_in_object(m) {
                    return from_array(arr);
                }
            }
            _ => {}
        }
    }

    // Shape 5: text split on newline / comma / semicolon — same parser as
    // the manual batch input box in WorkflowsPage.tsx (Phase 1b).
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for chunk in trimmed.split(['\n', ',', ';']) {
        let item = chunk.trim().to_string();
        if !item.is_empty() && seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}

/// Like [`parse_items`] but PRESERVES each item's structure: a JSON array of
/// objects stays a list of objects (so the batch step can map their fields onto
/// the QP's variables and pick a clean title), a JSON array of strings stays
/// strings, and plain text degrades to one string per line/comma/semicolon.
/// Envelope/object-wrapping shapes (`{data:[...]}`, `{tickets:[...]}`) are
/// unwrapped exactly like [`parse_items`]. Duplicates are removed in order
/// (objects keyed by `_title`/`id`/`key`/`number`, scalars by their string).
fn parse_items_rich(rendered: &str) -> Vec<serde_json::Value> {
    use serde_json::Value;
    let trimmed = rendered.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    fn dedup_key(v: &Value) -> String {
        match v {
            Value::Object(m) => ["_title", "id", "key", "number"]
                .iter()
                .find_map(|k| m.get(*k).map(json_scalar_to_string))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| serde_json::to_string(v).unwrap_or_default()),
            Value::String(s) => s.trim().to_string(),
            other => other.to_string(),
        }
    }

    fn collect(arr: &[Value]) -> Vec<Value> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for v in arr {
            let empty = matches!(v, Value::String(s) if s.trim().is_empty())
                || matches!(v, Value::Null);
            if empty {
                continue;
            }
            if seen.insert(dedup_key(v)) {
                match v {
                    Value::String(s) => out.push(Value::String(s.trim().to_string())),
                    _ => out.push(v.clone()),
                }
            }
        }
        out
    }

    fn first_array_in_object(m: &serde_json::Map<String, Value>) -> Option<&Vec<Value>> {
        m.values().find_map(|v| v.as_array())
    }

    if let Ok(json) = serde_json::from_str::<Value>(trimmed) {
        match &json {
            Value::Array(arr) => return collect(arr),
            Value::Object(m) => {
                // Shape 4: full envelope `{data:{...array...}}`.
                if let Some(Value::Object(data_obj)) = m.get("data") {
                    if let Some(arr) = first_array_in_object(data_obj) {
                        return collect(arr);
                    }
                }
                // `{data:[...]}`.
                if let Some(Value::Array(arr)) = m.get("data") {
                    return collect(arr);
                }
                // Shape 3: bare object with one array field, e.g. `{"tickets":[...]}`.
                if let Some(arr) = first_array_in_object(m) {
                    return collect(arr);
                }
            }
            _ => {}
        }
    }

    // Plain-text fallback — one scalar string per line / comma / semicolon.
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for chunk in trimmed.split(['\n', ',', ';']) {
        let item = chunk.trim();
        if !item.is_empty() && seen.insert(item.to_string()) {
            out.push(Value::String(item.to_string()));
        }
    }
    out
}

/// Above this many chars, a substituted value is treated as **injected
/// context** (a ticket payload, a file dump, a big JSON) and wrapped in a
/// `kronn:context` marker so the UI renders it as a collapsible card, visually
/// distinct from the QP's static instructions. Below → inline as before (a
/// date / key / id doesn't deserve a card). 2026-06-24.
const CONTEXT_MARKER_MIN_CHARS: usize = 400;

/// Wrap a large substituted value in the `kronn:context` marker that
/// `MessageBubble` renders as a collapsible "injected context" card. The
/// marker is an HTML-comment pair — inert markdown, invisible to the agent —
/// and the inner content stays raw so the front renders it as markdown inside
/// the card. Small values pass through unchanged. `title` labels the card
/// (the variable name). Idempotent-ish: a value already wrapped isn't
/// double-wrapped.
fn wrap_injected_context(value: &str, title: &str) -> String {
    if value.len() < CONTEXT_MARKER_MIN_CHARS || value.contains("<!-- kronn:context") {
        return value.to_string();
    }
    let safe_title = title.replace('"', "'").replace(['<', '>'], "");
    format!("<!-- kronn:context title=\"{safe_title}\" -->\n{value}\n<!-- /kronn:context -->")
}

/// Render a Quick Prompt template by filling its first variable with `value`.
/// Uses the same `{{var_name}}` pattern as the frontend `renderTemplate`.
fn render_qp_prompt(template: &str, first_var_name: Option<&str>, value: &str) -> String {
    let mut out = template.to_string();
    if let Some(name) = first_var_name {
        let placeholder = format!("{{{{{}}}}}", name);
        out = out.replace(&placeholder, &wrap_injected_context(value, name));
    }
    out
}

/// Fill EVERY `{{var}}` placeholder in `template` from the `vars` map. Mirrors
/// the MCP `qp_batch_run` renderer (`render_qp_template`) so the workflow batch
/// step and the MCP batch path produce identical prompts from the same vars.
fn render_qp_template_vars(template: &str, vars: &HashMap<String, String>) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{}}}}}", k), &wrap_injected_context(v, k));
    }
    out
}

/// Stringify a JSON value for use as a template substitution / title: strings
/// verbatim, numbers/bools via their literal form, null → empty, nested
/// arrays/objects compact-JSON.
fn json_scalar_to_string(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(_) | Value::Number(_) => v.to_string(),
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// Turn one parsed batch item into `(vars, disc_title)`.
///
/// * Object → every field becomes a `{{field}}` substitution (the reserved
///   `_title` key excepted); the title is the first present & non-empty of
///   `_title` / `id` / `key` / `number`, else `"<qp> #<n>"`.
/// * String / scalar → fills the QP's first variable and is also the title
///   (legacy `["EW-1", ...]` behaviour).
fn item_to_vars_and_title(
    item: &serde_json::Value,
    first_var_name: Option<&str>,
    qp_name: &str,
    idx: usize,
) -> (HashMap<String, String>, String) {
    use serde_json::Value;
    let mut vars = HashMap::new();
    match item {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "_title" { continue; }
                vars.insert(k.clone(), json_scalar_to_string(v));
            }
            let title = ["_title", "id", "key", "number"]
                .iter()
                .find_map(|k| map.get(*k).map(json_scalar_to_string))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{} #{}", qp_name, idx + 1));
            (vars, title)
        }
        other => {
            let s = json_scalar_to_string(other);
            if let Some(name) = first_var_name {
                vars.insert(name.to_string(), s.clone());
            }
            (vars, s)
        }
    }
}

/// Build the structured JSON envelope used as the step's `output` field so
/// downstream steps can chain on `{{steps.<name>.data.ok}}` etc. Matches the
/// schema enforced by `workflows::template::extract_step_envelope`.
fn build_structured_output(
    run_id: &str,
    total: u32,
    ok: u32,
    failed: u32,
    discussion_ids: &[String],
    completed: bool,
) -> String {
    let status = if completed {
        if ok == total { "OK" }
        else if ok > 0 { "PARTIAL" }
        else { "ERROR" }
    } else {
        "PENDING"
    };

    let summary = if completed {
        format!("{}/{} réussies ({} échecs)", ok, total, failed)
    } else {
        format!("{} discussions lancées (fire-and-forget)", total)
    };

    // Use an indexmap-like construction via HashMap since serde_json::Map
    // preserves insertion order as a feature flag we don't rely on.
    let mut data = HashMap::new();
    data.insert("batch_run_id", serde_json::Value::String(run_id.to_string()));
    data.insert("total", serde_json::Value::Number(total.into()));
    data.insert("ok", serde_json::Value::Number(ok.into()));
    data.insert("failed", serde_json::Value::Number(failed.into()));
    data.insert(
        "discussion_ids",
        serde_json::Value::Array(
            discussion_ids.iter().map(|s| serde_json::Value::String(s.clone())).collect()
        ),
    );

    // 0.8.5 — canonical Kronn envelope (markers + signal) via the
    // shared formatter. The signal matches the status so consumers can
    // branch via `on_result: [{ contains: "PARTIAL", action: Skip }]`
    // or similar without re-parsing the JSON. Cf.
    // [[project_step_output_homogenisation_0_9_0]].
    super::step_output_format::format_step_output(
        serde_json::to_value(&data).unwrap_or(serde_json::Value::Null),
        status,
        &summary,
        None,
        &[status],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_items_json_string_array() {
        let out = parse_items(r#"["EW-1", "EW-2", "EW-3"]"#);
        assert_eq!(out, vec!["EW-1", "EW-2", "EW-3"]);
    }

    #[test]
    fn parse_items_json_object_array_with_id() {
        let out = parse_items(r#"[{"id":"EW-1","title":"foo"},{"id":"EW-2"}]"#);
        assert_eq!(out, vec!["EW-1", "EW-2"]);
    }

    #[test]
    fn parse_items_json_object_array_with_key() {
        let out = parse_items(r#"[{"key":"EW-1"},{"key":"EW-2"}]"#);
        assert_eq!(out, vec!["EW-1", "EW-2"]);
    }

    #[test]
    fn parse_items_plain_newline() {
        let out = parse_items("EW-1\nEW-2\nEW-3");
        assert_eq!(out, vec!["EW-1", "EW-2", "EW-3"]);
    }

    #[test]
    fn parse_items_plain_comma() {
        let out = parse_items("EW-1, EW-2, EW-3");
        assert_eq!(out, vec!["EW-1", "EW-2", "EW-3"]);
    }

    #[test]
    fn parse_items_object_with_inner_array_field() {
        // Shape 3: `{"tickets":[...]}` — the common output of an Agent step
        // with Structured output_format that puts the list inside `data`.
        let out = parse_items(r#"{"tickets":["EW-1","EW-2","EW-3"]}"#);
        assert_eq!(out, vec!["EW-1", "EW-2", "EW-3"]);
    }

    #[test]
    fn parse_items_full_envelope_with_array_in_data() {
        // Shape 4: full `{data:{tickets:[...]},status,summary}` envelope.
        // Happens when items_from is `{{steps.X}}` rather than `{{steps.X.data}}`.
        let out = parse_items(
            r#"{"data":{"tickets":["EW-1","EW-2"]},"status":"OK","summary":"2"}"#
        );
        assert_eq!(out, vec!["EW-1", "EW-2"]);
    }

    #[test]
    fn parse_items_envelope_with_data_as_direct_array() {
        // Shape 4 variant: data IS the array directly.
        let out = parse_items(
            r#"{"data":["EW-1","EW-2","EW-3"],"status":"OK","summary":"3"}"#
        );
        assert_eq!(out, vec!["EW-1", "EW-2", "EW-3"]);
    }

    #[test]
    fn parse_items_dedupe_preserves_order() {
        let out = parse_items("EW-1\nEW-2\nEW-1\nEW-3");
        assert_eq!(out, vec!["EW-1", "EW-2", "EW-3"]);
    }

    #[test]
    fn parse_items_empty() {
        assert!(parse_items("").is_empty());
        assert!(parse_items("   \n\n  ").is_empty());
        assert!(parse_items("[]").is_empty());
    }

    #[test]
    fn render_qp_prompt_substitutes_first_var() {
        let out = render_qp_prompt("Analyse {{ticket}} en profondeur", Some("ticket"), "EW-1234");
        assert_eq!(out, "Analyse EW-1234 en profondeur");
    }

    #[test]
    fn render_qp_prompt_no_var_returns_template_as_is() {
        let out = render_qp_prompt("Static template", None, "EW-1234");
        assert_eq!(out, "Static template");
    }

    // ── 2026-06-24 — injected-context marker (UI collapsible card) ──

    #[test]
    fn small_value_is_not_wrapped() {
        // A short id/key stays inline — no card for trivia.
        assert_eq!(wrap_injected_context("EW-1234", "ticket"), "EW-1234");
        let out = render_qp_prompt("Analyse {{ticket}}", Some("ticket"), "EW-1234");
        assert!(!out.contains("kronn:context"), "small value must not be wrapped: {out}");
    }

    #[test]
    fn large_value_is_wrapped_with_title() {
        let big = "Description du ticket : ".to_string() + &"bla ".repeat(200); // >400 chars
        let wrapped = wrap_injected_context(&big, "ticket");
        assert!(wrapped.starts_with("<!-- kronn:context title=\"ticket\" -->"), "{}", &wrapped[..80]);
        assert!(wrapped.ends_with("<!-- /kronn:context -->"));
        assert!(wrapped.contains(&big), "inner content preserved verbatim");
    }

    #[test]
    fn large_value_wrapped_through_render() {
        let big = "x".repeat(500);
        let out = render_qp_prompt("## Le ticket\n{{ticket}}\n## Méthode", Some("ticket"), &big);
        assert!(out.contains("<!-- kronn:context title=\"ticket\" -->"), "render must wrap the big injected value");
        // surrounding instructions stay outside the marker
        assert!(out.starts_with("## Le ticket"));
        assert!(out.trim_end().ends_with("## Méthode"));
    }

    #[test]
    fn wrap_is_not_doubled_and_title_sanitized() {
        let big = "y".repeat(500);
        let once = wrap_injected_context(&big, "a\"b<c>");
        // title quotes/angle-brackets neutralised
        assert!(once.contains("title=\"a'bc\""), "{}", &once[..60]);
        // already-wrapped → not wrapped again
        assert_eq!(wrap_injected_context(&once, "ticket"), once);
    }

    #[test]
    fn render_template_vars_wraps_only_large_values() {
        let mut vars = HashMap::new();
        vars.insert("key".to_string(), "EW-9".to_string());          // small → inline
        vars.insert("body".to_string(), "z".repeat(500));            // large → carded
        let out = render_qp_template_vars("{{key}} :: {{body}}", &vars);
        assert!(out.contains("EW-9"), "small value inline");
        assert!(!out.contains("title=\"key\""), "small value not carded");
        assert!(out.contains("title=\"body\""), "large value carded");
    }

    // ── Multi-variable batch items (object shape) ───────────────────────

    #[test]
    fn parse_items_rich_preserves_objects() {
        let items = parse_items_rich(r#"[{"id":"EW-1","summary":"S1"},{"id":"EW-2","summary":"S2"}]"#);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], "EW-1");
        assert_eq!(items[0]["summary"], "S1");
        assert_eq!(items[1]["id"], "EW-2");
    }

    #[test]
    fn parse_items_rich_unwraps_envelope_of_objects() {
        let items = parse_items_rich(r#"{"data":{"items":[{"id":"EW-9","x":1}]},"status":"OK"}"#);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "EW-9");
    }

    #[test]
    fn parse_items_rich_string_array_back_compat() {
        let items = parse_items_rich(r#"["EW-1","EW-2"]"#);
        assert_eq!(items, vec![serde_json::json!("EW-1"), serde_json::json!("EW-2")]);
    }

    #[test]
    fn item_object_maps_fields_to_vars_and_picks_id_title() {
        let item = serde_json::json!({"id":"EW-1234","descriptionWiki":"h2. Hi","summary":"Sum"});
        let (vars, title) = item_to_vars_and_title(&item, Some("ticketId"), "Triage", 0);
        assert_eq!(title, "EW-1234"); // clean title from `id`, not the whole payload
        assert_eq!(vars.get("descriptionWiki").unwrap(), "h2. Hi");
        assert_eq!(vars.get("summary").unwrap(), "Sum");
        let prompt = render_qp_template_vars("## {{id}}\n{{descriptionWiki}} — {{summary}}", &vars);
        assert_eq!(prompt, "## EW-1234\nh2. Hi — Sum");
    }

    #[test]
    fn item_object_explicit_title_overrides_id() {
        let item = serde_json::json!({"_title":"Joli titre","id":"EW-1","v":"x"});
        let (vars, title) = item_to_vars_and_title(&item, Some("first"), "Triage", 2);
        assert_eq!(title, "Joli titre");
        assert!(!vars.contains_key("_title")); // reserved, not injected as a var
        assert_eq!(vars.get("id").unwrap(), "EW-1");
    }

    #[test]
    fn item_object_without_id_falls_back_to_indexed_title() {
        let item = serde_json::json!({"foo":"bar"});
        let (_vars, title) = item_to_vars_and_title(&item, None, "Triage", 4);
        assert_eq!(title, "Triage #5");
    }

    #[test]
    fn item_string_fills_first_var_and_is_title() {
        let item = serde_json::json!("EW-777");
        let (vars, title) = item_to_vars_and_title(&item, Some("ticketId"), "Triage", 0);
        assert_eq!(title, "EW-777");
        assert_eq!(vars.get("ticketId").unwrap(), "EW-777");
    }

    #[test]
    fn json_scalar_to_string_handles_kinds() {
        assert_eq!(json_scalar_to_string(&serde_json::json!("s")), "s");
        assert_eq!(json_scalar_to_string(&serde_json::json!(42)), "42");
        assert_eq!(json_scalar_to_string(&serde_json::json!(true)), "true");
        assert_eq!(json_scalar_to_string(&serde_json::Value::Null), "");
        assert_eq!(json_scalar_to_string(&serde_json::json!(["a","b"])), r#"["a","b"]"#);
    }

    // 0.8.5 — outputs go through the canonical Kronn envelope
    // (markers + signal). These tests parse via `parse_envelope_for_test`
    // so a future format tweak only changes the helper, not 20 tests.
    use super::super::step_output_format::parse_envelope_for_test;

    #[test]
    fn build_structured_output_ok() {
        let out = build_structured_output("run-1", 3, 3, 0, &["d1".into(), "d2".into(), "d3".into()], true);
        let v = parse_envelope_for_test(&out);
        assert_eq!(v["status"], "OK");
        assert_eq!(v["data"]["total"], 3);
        assert_eq!(v["data"]["ok"], 3);
        assert_eq!(v["data"]["failed"], 0);
        assert_eq!(v["data"]["batch_run_id"], "run-1");
        assert_eq!(v["data"]["discussion_ids"].as_array().unwrap().len(), 3);
        // Canonical envelope must also carry the matching SIGNAL line so
        // `on_result.contains` rules can branch on status.
        assert!(out.contains("[SIGNAL: OK]"));
    }

    #[test]
    fn build_structured_output_partial() {
        let out = build_structured_output("run-1", 5, 3, 2, &[], true);
        let v = parse_envelope_for_test(&out);
        assert_eq!(v["status"], "PARTIAL");
        assert!(out.contains("[SIGNAL: PARTIAL]"));
    }

    #[test]
    fn build_structured_output_error() {
        let out = build_structured_output("run-1", 5, 0, 5, &[], true);
        let v = parse_envelope_for_test(&out);
        assert_eq!(v["status"], "ERROR");
        assert!(out.contains("[SIGNAL: ERROR]"));
    }

    #[test]
    fn build_structured_output_pending_fire_and_forget() {
        let out = build_structured_output("run-1", 5, 0, 0, &[], false);
        let v = parse_envelope_for_test(&out);
        assert_eq!(v["status"], "PENDING");
        assert!(out.contains("[SIGNAL: PENDING]"));
    }

    // ─── E2E tests for `execute_batch_quick_prompt_step` ─────────────────────
    //
    // These exercise the full pipeline (template render → QP load →
    // `create_batch_run` → fan-out tokio::spawn → optional WS wait → structured
    // output) against an in-memory DB. The fan-out tasks try to spawn a real
    // agent CLI via `spawn_agent_run_with_chain` — in the test env there's no
    // agent binary on PATH so each detached task fails fast inside its own
    // tokio task. We don't observe those failures from the test thread; the
    // BatchRunFinished WS event is the wait-loop's only completion signal, and
    // for the wait-for-completion test we synthesize one via a watchdog task.
    //
    // The original TD-20260510 (memory `project_batch_workflows`) called for
    // "an E2E test on `execute_batch_quick_prompt_step` full flow with mocked
    // WS" — this is that test, paired with the fire-and-forget variant that
    // exercises everything except the WS wait.

    use crate::{AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
    use crate::core::config::default_config;
    use crate::db::Database;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn e2e_state() -> AppState {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let config_arc = Arc::new(RwLock::new(default_config()));
        AppState::new_defaults(config_arc, db, DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    /// Insert a minimal QP with a single variable named `ticket`. The batch
    /// items get substituted into `{{ticket}}` at render time.
    async fn seed_qp(state: &AppState) -> QuickPrompt {
        use chrono::Utc;
        let qp = QuickPrompt {
            id: "qp-e2e".into(),
            name: "Analyse ticket".into(),
            icon: "🎯".into(),
            prompt_template: "Analyse le ticket {{ticket}} en profondeur.".into(),
            variables: vec![PromptVariable {
                name: "ticket".into(),
                label: "Ticket".into(),
                placeholder: "EW-1234".into(),
                description: Some("Ticket ID".into()),
                required: true,
                pattern: None,
            }],
            agent: AgentType::ClaudeCode,
            project_id: None,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            tier: ModelTier::Default,
            agent_settings: None,
            description: "E2E test QP".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let qp_for_insert = qp.clone();
        state.db.with_conn(move |conn| {
            crate::db::quick_prompts::insert_quick_prompt(conn, &qp_for_insert)
        }).await.expect("insert QP");
        qp
    }

    /// Insert a placeholder Workflow + a parent linear WorkflowRun so the
    /// step has something to attach its child batch run to. Returns the run id.
    async fn seed_parent_run(state: &AppState) -> String {
        use chrono::Utc;
        let wf_id = "wf-e2e".to_string();
        let workflow = Workflow {
            id: wf_id.clone(),
            name: "E2E parent workflow".into(),
            project_id: None,
            trigger: WorkflowTrigger::Manual,
            steps: vec![],
            actions: vec![],
            safety: WorkflowSafety {
                sandbox: false,
                max_files: None,
                max_lines: None,
                require_approval: false,
            },
            workspace_config: None,
            concurrency_limit: None,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            guards: None,
            artifacts: std::collections::HashMap::new(),
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![],
        };
        let run_id = "run-parent-e2e".to_string();
        let parent_run = WorkflowRun {
            id: run_id.clone(),
            workflow_id: wf_id.clone(),
            status: RunStatus::Running,
            trigger_context: None,
            step_results: vec![],
            tokens_used: 0,
            workspace_path: None,
            started_at: Utc::now(),
            finished_at: None,
            run_type: "linear".into(),
            batch_total: 0,
            batch_completed: 0,
            batch_failed: 0,
            batch_name: None,
            parent_run_id: None,
            state: std::collections::HashMap::new(),
            produced_branches: vec![],
            parent_workflow_id: None,
            parent_workflow_name: None,
            parent_run_started_at: None,
        };
        state.db.with_conn(move |conn| -> anyhow::Result<()> {
            crate::db::workflows::insert_workflow(conn, &workflow)?;
            crate::db::workflows::insert_run(conn, &parent_run)?;
            Ok(())
        }).await.expect("insert workflow + parent run");
        run_id
    }

    /// Build a minimal `BatchQuickPrompt` step pointing at the seeded QP.
    fn batch_step(qp_id: &str, wait_for_completion: bool) -> WorkflowStep {
        WorkflowStep {
            step_type: StepType::BatchQuickPrompt,
            output_format: StepOutputFormat::default(),
            description: None,
            name: "batch_e2e".into(),
            agent: AgentType::ClaudeCode,
            prompt_template: String::new(),
            mode: StepMode::Normal,
            mcp_config_ids: vec![],
            agent_settings: None,
            on_result: vec![],
            on_timeout: None,
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            batch_quick_prompt_id: Some(qp_id.into()),
            batch_items_from: Some(r#"["EW-1","EW-2","EW-3"]"#.into()),
            batch_wait_for_completion: Some(wait_for_completion),
            batch_max_items: Some(10),
            batch_workspace_mode: Some("Direct".into()),
            batch_chain_prompt_ids: vec![],
            batch_concurrent_limit: Some(2),
            quick_api_id: None,
            notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
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
            exec_stdin: None,
            quick_prompt_id: None,
            json_data_payload: None,
            sub_workflow_id: None,
            sub_workflow_foreach_file: None,
            multi_agent_review: None,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn execute_batch_quick_prompt_step_fire_and_forget_full_pipeline() {
        // Tests the full happy path EXCEPT the WS wait loop:
        //   template render → QP load → create_batch_run → fan-out spawn →
        //   structured output with status=PENDING.
        let state = e2e_state();
        let qp = seed_qp(&state).await;
        let parent_run_id = seed_parent_run(&state).await;
        let step = batch_step(&qp.id, /* wait */ false);
        let ctx = TemplateContext::new();

        let outcome = execute_batch_quick_prompt_step(&step, &parent_run_id, state.clone(), &ctx).await;

        // ─ Step-level assertions
        assert_eq!(outcome.result.status, RunStatus::Success,
            "fire-and-forget should always return Success once the spawn loop fires");
        let envelope = parse_envelope_for_test(&outcome.result.output);
        assert_eq!(envelope["status"], "PENDING",
            "wait_for_completion=false must produce PENDING (caller knows it's racing)");
        let data = &envelope["data"];
        assert_eq!(data["total"], 3, "3 items in batch_items_from → 3 children");
        assert_eq!(data["ok"], 0, "no completion info yet in fire-and-forget");
        assert_eq!(data["failed"], 0);
        assert!(data["batch_run_id"].is_string());
        assert_eq!(data["discussion_ids"].as_array().unwrap().len(), 3);

        // ─ DB-level assertions: the child batch run + 3 child discussions exist
        let parent_id_for_query = parent_run_id.clone();
        let (child_run, disc_count) = state.db.with_conn(move |conn| -> anyhow::Result<_> {
            let children = conn
                .prepare("SELECT id, batch_total FROM workflow_runs WHERE parent_run_id = ?1")?
                .query_map(rusqlite::params![parent_id_for_query], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(children.len(), 1, "exactly one child batch run");
            let (child_id, batch_total) = children.into_iter().next().unwrap();
            let disc_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM discussions WHERE workflow_run_id = ?1",
                rusqlite::params![child_id],
                |row| row.get(0),
            )?;
            Ok(((child_id, batch_total), disc_count))
        }).await.expect("DB readback");
        assert_eq!(child_run.1, 3, "child batch_total = number of items");
        assert_eq!(disc_count, 3, "one discussion row per batch item");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn execute_batch_quick_prompt_step_wait_for_completion_with_mocked_ws() {
        // Tests the WAIT path: a watchdog task synthesizes the
        // `BatchRunFinished` WS event (which would normally come from the last
        // agent's `make_agent_stream` completion in production) so the step
        // can return without waiting on real agents.
        let state = e2e_state();
        let qp = seed_qp(&state).await;
        let parent_run_id = seed_parent_run(&state).await;
        let step = batch_step(&qp.id, /* wait */ true);
        let ctx = TemplateContext::new();

        // Watchdog: polls the DB for the child batch run id, then emits a
        // BatchRunFinished with that id. This stands in for the real
        // `increment_batch_progress` → WS broadcast path.
        let watch_state = state.clone();
        let watch_parent_id = parent_run_id.clone();
        let watchdog = tokio::spawn(async move {
            // Up to ~2s to find the child — `create_batch_run` runs inside a
            // single `with_conn` transaction so it lands very fast.
            for _ in 0..40 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let parent = watch_parent_id.clone();
                let child = watch_state.db.with_conn(move |conn| -> anyhow::Result<Option<String>> {
                    let id: Option<String> = conn.query_row(
                        "SELECT id FROM workflow_runs WHERE parent_run_id = ?1 LIMIT 1",
                        rusqlite::params![parent],
                        |row| row.get(0),
                    ).ok();
                    Ok(id)
                }).await.ok().flatten();
                if let Some(child_id) = child {
                    // Tiny extra delay so the step's ws_broadcast.subscribe()
                    // is definitely listening — without this the message can
                    // land *before* the subscribe() call returns.
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    let _ = watch_state.ws_broadcast.send(WsMessage::BatchRunFinished {
                        run_id: child_id,
                        discussion_id: "any".into(),
                        batch_name: Some("e2e".into()),
                        batch_total: 3,
                        batch_completed: 3,
                        batch_failed: 0,
                    });
                    return;
                }
            }
            panic!("Watchdog never saw a child batch run materialise within 2s");
        });

        let outcome = execute_batch_quick_prompt_step(&step, &parent_run_id, state.clone(), &ctx).await;
        watchdog.await.expect("watchdog finished cleanly");

        // ─ The wait completed and propagated the counters from our synthesized event.
        assert_eq!(outcome.result.status, RunStatus::Success,
            "all 3 children OK → step Success (success = at-least-one OK)");
        let envelope = parse_envelope_for_test(&outcome.result.output);
        assert_eq!(envelope["status"], "OK", "3 ok / 0 failed → OK envelope");
        assert_eq!(envelope["data"]["total"], 3);
        assert_eq!(envelope["data"]["ok"], 3);
        assert_eq!(envelope["data"]["failed"], 0);
    }
}
