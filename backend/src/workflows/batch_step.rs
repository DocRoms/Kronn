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

    let items = parse_items(&rendered);
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

    // Render the QP template for each item. The first variable of the QP is
    // the substitution slot — each item fills it. If the QP has no variables,
    // the same static prompt is used for every disc (unusual but valid).
    let first_var_name = qp.variables.first().map(|v| v.name.clone());
    let batch_items: Vec<(String, String)> = items.iter().map(|item| {
        let prompt = render_qp_prompt(&qp.prompt_template, first_var_name.as_deref(), item);
        (item.clone(), prompt) // title = raw item, prompt = rendered template
    }).collect();

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

    // ── Subscribe to ws_broadcast BEFORE firing the agents ──────────────
    // Ordering matters: if we subscribed after spawning, a fast disc could
    // finish before we're listening and we'd miss its BatchRunFinished event.
    // `broadcast::Receiver::subscribe` captures messages from this point on.
    let mut ws_rx = state.ws_broadcast.subscribe();

    // ── Fan out: spawn agent runs on every child discussion ─────────────
    // Each call into `spawn_agent_run_with_chain` runs its own detached
    // `tokio::spawn`, so the work happens in parallel. The agent_semaphore
    // on AppState caps real concurrency.
    //
    // If `batch_chain_prompt_ids` is non-empty, each discussion will
    // sequentially execute the chained QPs after the initial response —
    // all inside the same conversation thread. Each chain QP receives the
    // SAME raw batch item (e.g. "EW-1234") as its first variable, so an
    // `analyse → review → summary` chain all runs on the same ticket.
    // The batch progress counter only bumps when the ENTIRE chain
    // finishes (the last `make_agent_stream` hits the batch_run_id hook
    // in discussions.rs).
    let chain_ids = step.batch_chain_prompt_ids.clone();
    // `outcome.discussion_ids` is ordered identically to `items` (see
    // `create_batch_run` in db/workflows.rs), so zipping by index is safe.
    for (idx, disc_id) in outcome.discussion_ids.iter().enumerate() {
        let batch_item = items.get(idx).cloned();
        crate::api::discussions::spawn_agent_run_with_chain(
            state.clone(),
            disc_id.clone(),
            chain_ids.clone(),
            batch_item,
        ).await;
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
                condition_result: None,
                // build_structured_output always emits a data/status/summary
                // payload that `set_step_output` can extract via Strategy 2
                // — the contract is met even without `---STEP_OUTPUT---`.
                envelope_detected: Some(true),
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
                    "BatchQuickPrompt step '{}' lagged {} WS messages — keep listening",
                    step.name, n
                );
                continue;
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

    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: step_status,
            output,
            tokens_used: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            condition_result: None,
            envelope_detected: Some(true),
        },
        condition_action: None,
    }
}

/// Produce a `StepOutcome` in the failed state with the given error text.
fn fail(step: &WorkflowStep, start: Instant, msg: impl Into<String>) -> StepOutcome {
    StepOutcome {
        result: StepResult {
            step_name: step.name.clone(),
            status: RunStatus::Failed,
            output: msg.into(),
            tokens_used: 0,
            duration_ms: start.elapsed().as_millis() as u64,
            condition_result: None,
            envelope_detected: None,
        },
        condition_action: None,
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

/// Render a Quick Prompt template by filling its first variable with `value`.
/// Uses the same `{{var_name}}` pattern as the frontend `renderTemplate`.
fn render_qp_prompt(template: &str, first_var_name: Option<&str>, value: &str) -> String {
    let mut out = template.to_string();
    if let Some(name) = first_var_name {
        let placeholder = format!("{{{{{}}}}}", name);
        out = out.replace(&placeholder, value);
    }
    out
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

    serde_json::json!({
        "data": data,
        "status": status,
        "summary": summary,
    }).to_string()
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

    #[test]
    fn build_structured_output_ok() {
        let out = build_structured_output("run-1", 3, 3, 0, &["d1".into(), "d2".into(), "d3".into()], true);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "OK");
        assert_eq!(v["data"]["total"], 3);
        assert_eq!(v["data"]["ok"], 3);
        assert_eq!(v["data"]["failed"], 0);
        assert_eq!(v["data"]["batch_run_id"], "run-1");
        assert_eq!(v["data"]["discussion_ids"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn build_structured_output_partial() {
        let out = build_structured_output("run-1", 5, 3, 2, &[], true);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "PARTIAL");
    }

    #[test]
    fn build_structured_output_error() {
        let out = build_structured_output("run-1", 5, 0, 5, &[], true);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "ERROR");
    }

    #[test]
    fn build_structured_output_pending_fire_and_forget() {
        let out = build_structured_output("run-1", 5, 0, 0, &[], false);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "PENDING");
    }
}
