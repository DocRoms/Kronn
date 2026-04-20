use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::models::*;

// ─── Helper ─────────────────────────────────────────────────────────────────

fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to parse datetime '{}': {}, using now()", s, e);
            Utc::now()
        })
}

fn parse_run_status(s: &str) -> RunStatus {
    match s {
        "Pending" => RunStatus::Pending,
        "Running" => RunStatus::Running,
        "Success" => RunStatus::Success,
        "Failed" => RunStatus::Failed,
        "Cancelled" => RunStatus::Cancelled,
        "WaitingApproval" => RunStatus::WaitingApproval,
        _ => RunStatus::Pending,
    }
}

fn run_status_str(s: &RunStatus) -> &'static str {
    match s {
        RunStatus::Pending => "Pending",
        RunStatus::Running => "Running",
        RunStatus::Success => "Success",
        RunStatus::Failed => "Failed",
        RunStatus::Cancelled => "Cancelled",
        RunStatus::WaitingApproval => "WaitingApproval",
    }
}

// ─── Workflows CRUD ─────────────────────────────────────────────────────────

/// Prefix used for batch-placeholder workflow rows (Phase 1b).
/// Batch runs live in `workflow_runs` which has a NOT NULL FK on
/// `workflows.id`. Rather than relaxing the constraint we insert a minimal
/// placeholder workflow row per Quick Prompt and filter it out of the
/// user-facing workflow list.
pub const BATCH_WORKFLOW_PREFIX: &str = "qp:";

pub fn list_workflows(conn: &Connection) -> Result<Vec<Workflow>> {
    // Filter out batch placeholders (prefix "qp:") — they shouldn't show
    // up in the Workflows page, they're a plumbing detail for the FK.
    let mut stmt = conn.prepare(
        "SELECT id, name, project_id, trigger_json, steps_json, actions_json,
                safety_json, workspace_config_json, concurrency_limit, enabled,
                created_at, updated_at
         FROM workflows WHERE id NOT LIKE 'qp:%' ORDER BY updated_at DESC"
    )?;

    let workflows = stmt.query_map([], |row| {
        Ok(row_to_workflow(row))
    })?.filter_map(|r| r.ok())
    .collect();

    Ok(workflows)
}

pub fn get_workflow(conn: &Connection, id: &str) -> Result<Option<Workflow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, project_id, trigger_json, steps_json, actions_json,
                safety_json, workspace_config_json, concurrency_limit, enabled,
                created_at, updated_at
         FROM workflows WHERE id = ?1"
    )?;

    let wf = stmt.query_row(params![id], |row| {
        Ok(row_to_workflow(row))
    }).ok();

    Ok(wf)
}

/// Ensure a placeholder workflow row exists for a Quick Prompt, used as the
/// FK target for batch workflow runs. Idempotent — returns the placeholder
/// workflow id (always `qp:<qp_id>`) whether it was created or already existed.
///
/// The placeholder is filtered out of `list_workflows` so it doesn't pollute
/// the Workflows page UI.
pub fn ensure_batch_placeholder_workflow(
    conn: &Connection,
    qp_id: &str,
    qp_name: &str,
    project_id: Option<&str>,
) -> Result<String> {
    let placeholder_id = format!("{}{}", BATCH_WORKFLOW_PREFIX, qp_id);
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO workflows (id, name, project_id, trigger_json, steps_json,
         actions_json, safety_json, workspace_config_json, concurrency_limit, enabled,
         created_at, updated_at)
         VALUES (?1, ?2, ?3, '\"Manual\"', '[]', '[]', '{}', NULL, NULL, 0, ?4, ?4)",
        params![
            placeholder_id,
            format!("[batch placeholder] {}", qp_name),
            project_id,
            now,
        ],
    )?;
    Ok(placeholder_id)
}

/// List all batch runs along with their parent workflow name + run sequence.
///
/// The sidebar consumes this to render a clickable pastille on each batch group
/// that takes the user back to the workflow run that spawned it. Kept as a
/// single DB query + per-batch resolution so the cost is ~O(N batches) rather
/// than ~O(N discussions): N_batches is typically small (<100) even with heavy
/// cron usage.
///
/// Manual batches (no `parent_run_id`) return `None` for the parent fields.
pub fn list_batch_run_summaries(conn: &Connection) -> Result<Vec<BatchRunSummary>> {
    let sql = format!(
        "SELECT {} FROM workflow_runs WHERE run_type = 'batch' ORDER BY started_at DESC",
        WORKFLOW_RUN_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let batch_runs: Vec<WorkflowRun> = stmt
        .query_map([], |row| Ok(row_to_run(row)))?
        .filter_map(|r| r.ok())
        .collect();

    // Cache workflow-id → (name, ordered_run_ids) so N batches with the same
    // parent workflow only hit the DB once.
    let mut workflow_cache: std::collections::HashMap<String, (Option<String>, Vec<String>)> =
        std::collections::HashMap::new();
    // Cache qp-id → (name, icon) for the same reason (batches of the same QP
    // run thousands of times on a cron; skip redundant lookups).
    let mut qp_cache: std::collections::HashMap<String, (Option<String>, Option<String>)> =
        std::collections::HashMap::new();

    let mut out = Vec::with_capacity(batch_runs.len());
    for br in batch_runs {
        // Resolve the Quick Prompt that this batch fans out. Batch runs have
        // a virtual workflow id of the form "qp:<uuid>" (see BATCH_WORKFLOW_PREFIX)
        // — strip the prefix and look up the QP by its real id. None if the
        // QP was deleted after the batch ran.
        let (quick_prompt_id, quick_prompt_name, quick_prompt_icon) = {
            if let Some(qp_id) = br.workflow_id.strip_prefix(BATCH_WORKFLOW_PREFIX) {
                let qp_id = qp_id.to_string();
                let (name, icon) = if let Some(cached) = qp_cache.get(&qp_id) {
                    cached.clone()
                } else {
                    // Fallback to None on DB errors so the whole summary list
                    // doesn't fail because of one corrupt row, but log so the
                    // degradation is visible in prod logs instead of silent.
                    let fetched = match crate::db::quick_prompts::get_quick_prompt(conn, &qp_id) {
                        Ok(qp) => qp,
                        Err(e) => {
                            tracing::warn!(
                                "list_batch_run_summaries: failed to load QP {}: {}",
                                qp_id, e
                            );
                            None
                        }
                    };
                    let pair = (
                        fetched.as_ref().map(|qp| qp.name.clone()),
                        fetched.as_ref().map(|qp| qp.icon.clone()),
                    );
                    qp_cache.insert(qp_id.clone(), pair.clone());
                    pair
                };
                (Some(qp_id), name, icon)
            } else {
                (None, None, None)
            }
        };

        let (parent_workflow_id, parent_workflow_name, parent_run_sequence) =
            if let Some(ref parent_id) = br.parent_run_id {
                match get_run(conn, parent_id)? {
                    Some(parent_run) => {
                        let wf_id = parent_run.workflow_id.clone();
                        let entry = if let Some(cached) = workflow_cache.get(&wf_id) {
                            cached.clone()
                        } else {
                            let wf = get_workflow(conn, &wf_id)?;
                            // Run ordering for sequence numbers: ascending by started_at
                            // so run #1 is the oldest. That matches how users think
                            // about "the 3rd run of my cron today".
                            let mut stmt2 = conn.prepare(
                                "SELECT id FROM workflow_runs WHERE workflow_id = ?1 ORDER BY started_at ASC"
                            )?;
                            let rows = stmt2.query_map(params![wf_id], |row| row.get::<_, String>(0))?;
                            let run_ids: Vec<String> = rows.filter_map(|r| r.ok()).collect();
                            drop(stmt2); // release borrow on `conn` before further queries
                            let cached = (wf.map(|w| w.name), run_ids);
                            workflow_cache.insert(wf_id.clone(), cached.clone());
                            cached
                        };
                        let (name, run_ids) = entry;
                        let seq = run_ids.iter().position(|id| id == parent_id)
                            .map(|i| (i + 1) as u32);
                        (Some(wf_id), name, seq)
                    }
                    None => (None, None, None),
                }
            } else {
                (None, None, None)
            };

        out.push(BatchRunSummary {
            run_id: br.id,
            batch_name: br.batch_name,
            batch_total: br.batch_total,
            status: br.status,
            quick_prompt_id,
            quick_prompt_name,
            quick_prompt_icon,
            parent_run_id: br.parent_run_id,
            parent_workflow_id,
            parent_workflow_name,
            parent_run_sequence,
        });
    }

    Ok(out)
}

/// Result of [`delete_batch_run_with_discussions`].
#[derive(Debug)]
pub struct DeletedBatchSummary {
    pub run_id: String,
    pub discussions_deleted: usize,
}

/// Delete a batch workflow run AND every child discussion linked to it.
///
/// We can't rely on the FK cascade alone: `discussions.workflow_run_id` is
/// `ON DELETE SET NULL`, which would just unlink the discs (orphan them with
/// their messages) — not what the user wants when they click "delete batch".
///
/// Behaviour:
/// - Verifies the run exists AND is `run_type = 'batch'` (refuses to delete
///   linear runs by accident).
/// - Deletes child discussions first (which cascades to discussion_messages
///   via the existing FK), then the batch run row.
/// - Wrapped in a transaction so partial state is impossible on failure.
///
/// Returns the count of discussions actually deleted (handy for the frontend
/// confirmation toast: "Batch + 12 discussions supprimés").
pub fn delete_batch_run_with_discussions(
    conn: &Connection,
    run_id: &str,
) -> Result<DeletedBatchSummary> {
    // Validate that the target is a batch run, not a linear one.
    let run = get_run(conn, run_id)?
        .ok_or_else(|| anyhow::anyhow!("Batch run not found: {}", run_id))?;
    if run.run_type != "batch" {
        anyhow::bail!(
            "Refusing to delete: run {} is type '{}', not 'batch'. \
             Use the workflow run delete endpoint for linear runs.",
            run_id, run.run_type
        );
    }

    conn.execute_batch("BEGIN")?;
    let tx_result: Result<usize> = (|| {
        let n_discs: usize = conn.execute(
            "DELETE FROM discussions WHERE workflow_run_id = ?1",
            params![run_id],
        )?;
        conn.execute(
            "DELETE FROM workflow_runs WHERE id = ?1",
            params![run_id],
        )?;
        Ok(n_discs)
    })();

    match tx_result {
        Ok(n) => {
            conn.execute_batch("COMMIT")?;
            Ok(DeletedBatchSummary {
                run_id: run_id.to_string(),
                discussions_deleted: n,
            })
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

// ─── Batch run creation (pure, reusable from HTTP + workflow runner) ───────

/// Input for [`create_batch_run`]. Everything needed to build a batch
/// workflow run plus its N child discussions in a single transaction.
pub struct CreateBatchRunInput<'a> {
    pub quick_prompt: &'a QuickPrompt,
    /// `(title, fully_rendered_prompt)` pairs — one per child discussion.
    /// The caller is responsible for template substitution on the prompt.
    pub items: Vec<(String, String)>,
    /// Display name of the batch group, e.g. "Cadrage to-Frame — 10 avr 14:00".
    pub batch_name: Option<String>,
    /// Optional project to attach all child discussions to. Overrides the QP's
    /// default project when set; otherwise falls back to `quick_prompt.project_id`.
    pub project_id: Option<String>,
    /// Parent linear workflow run id when this batch is spawned from a
    /// `BatchQuickPrompt` step. `None` for top-level batches triggered from the UI.
    pub parent_run_id: Option<String>,
    /// Pseudo + avatar for message attribution on the initial user message.
    pub author_pseudo: Option<String>,
    pub author_avatar_email: Option<String>,
    /// Discussion language, e.g. "fr". Falls back to "fr" if empty.
    pub language: String,
    /// Workspace mode for each child discussion. `"Direct"` (default) runs
    /// all children in the project's main working tree — fine for read-only
    /// analysis batches. `"Isolated"` triggers a per-disc git worktree on the
    /// first agent run, branch named after the discussion title. Essential
    /// when the agents write code in parallel, otherwise they clobber each
    /// other in the main tree.
    pub workspace_mode: String,
}

/// Result of [`create_batch_run`] — the ids the caller needs to fan out or
/// broadcast.
pub struct CreateBatchRunOutput {
    pub run_id: String,
    pub discussion_ids: Vec<String>,
    pub batch_total: u32,
}

/// Create a batch workflow run + N child discussions atomically.
///
/// Callable from both:
/// - the HTTP handler `POST /api/quick-prompts/:id/batch` (top-level batch,
///   `parent_run_id = None`)
/// - the workflow step executor for `StepType::BatchQuickPrompt` (chained
///   batch, where `parent_run_id = Some(linear_run.id)` links back to the
///   linear parent run)
///
/// The whole creation runs in a single transaction: placeholder workflow row,
/// the batch run, and all child discussions + their initial messages. On any
/// error the transaction is rolled back and nothing is persisted.
pub fn create_batch_run(
    conn: &Connection,
    input: CreateBatchRunInput,
) -> Result<CreateBatchRunOutput> {
    let batch_total = input.items.len() as u32;
    let run_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let qp = input.quick_prompt;

    let run = WorkflowRun {
        id: run_id.clone(),
        // Batch runs are not tied to a saved Workflow — reuse the QP id as the
        // virtual workflow id so the existing list_runs(workflow_id) query still
        // works and users can view all runs for a QP in one place.
        workflow_id: format!("{}{}", BATCH_WORKFLOW_PREFIX, qp.id),
        status: RunStatus::Running,
        trigger_context: Some(serde_json::json!({
            "type": "batch",
            "quick_prompt_id": qp.id,
            "quick_prompt_name": qp.name,
            "batch_size": batch_total,
            "parent_run_id": input.parent_run_id,
        })),
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "batch".into(),
        batch_total,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: input.batch_name.clone(),
        parent_run_id: input.parent_run_id.clone(),
    };

    let lang = if input.language.is_empty() { "fr".to_string() } else { input.language };
    // Request's project_id overrides QP's default when set.
    let effective_project_id = input.project_id.clone().or_else(|| qp.project_id.clone());
    let workspace_mode = if input.workspace_mode.is_empty() {
        "Direct".to_string()
    } else {
        input.workspace_mode
    };

    let discussions: Vec<(Discussion, DiscussionMessage)> = input.items.iter().map(|(title, prompt)| {
        let disc_id = Uuid::new_v4().to_string();
        let initial_message = DiscussionMessage {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content: prompt.clone(),
            agent_type: None,
            timestamp: now,
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: input.author_pseudo.clone(),
            author_avatar_email: input.author_avatar_email.clone(),
        };
        let discussion = Discussion {
            id: disc_id,
            project_id: effective_project_id.clone(),
            title: title.clone(),
            agent: qp.agent.clone(),
            language: lang.clone(),
            participants: vec![qp.agent.clone()],
            messages: vec![initial_message.clone()],
            message_count: 1,
            skill_ids: qp.skill_ids.clone(),
            profile_ids: vec![],
            directive_ids: vec![],
            archived: false,
            pinned: false,
            workspace_mode: workspace_mode.clone(),
            workspace_path: None,
            worktree_branch: None,
            tier: qp.tier,
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            shared_id: None,
            shared_with: vec![],
            workflow_run_id: Some(run_id.clone()),
            test_mode_restore_branch: None,
            test_mode_stash_ref: None,
            created_at: now,
            updated_at: now,
        };
        (discussion, initial_message)
    }).collect();

    let discussion_ids: Vec<String> = discussions.iter().map(|(d, _)| d.id.clone()).collect();

    // Single transaction: placeholder workflow + run + all discussions/messages.
    conn.execute_batch("BEGIN")?;
    let tx_result: Result<()> = (|| {
        ensure_batch_placeholder_workflow(conn, &qp.id, &qp.name, qp.project_id.as_deref())?;
        insert_run(conn, &run)?;
        for (disc, msg) in &discussions {
            crate::db::discussions::insert_discussion(conn, disc)?;
            crate::db::discussions::insert_message(conn, &disc.id, msg)?;
        }
        Ok(())
    })();
    if let Err(e) = tx_result {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e);
    }
    conn.execute_batch("COMMIT")?;

    Ok(CreateBatchRunOutput {
        run_id,
        discussion_ids,
        batch_total,
    })
}

pub fn insert_workflow(conn: &Connection, wf: &Workflow) -> Result<()> {
    conn.execute(
        "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, actions_json,
         safety_json, workspace_config_json, concurrency_limit, enabled, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            wf.id,
            wf.name,
            wf.project_id,
            serde_json::to_string(&wf.trigger)?,
            serde_json::to_string(&wf.steps)?,
            serde_json::to_string(&wf.actions)?,
            serde_json::to_string(&wf.safety)?,
            wf.workspace_config.as_ref().map(serde_json::to_string).transpose()?,
            wf.concurrency_limit,
            wf.enabled as i32,
            wf.created_at.to_rfc3339(),
            wf.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn update_workflow(conn: &Connection, wf: &Workflow) -> Result<()> {
    conn.execute(
        "UPDATE workflows SET name = ?2, project_id = ?3, trigger_json = ?4, steps_json = ?5,
         actions_json = ?6, safety_json = ?7, workspace_config_json = ?8,
         concurrency_limit = ?9, enabled = ?10, updated_at = ?11
         WHERE id = ?1",
        params![
            wf.id,
            wf.name,
            wf.project_id,
            serde_json::to_string(&wf.trigger)?,
            serde_json::to_string(&wf.steps)?,
            serde_json::to_string(&wf.actions)?,
            serde_json::to_string(&wf.safety)?,
            wf.workspace_config.as_ref().map(serde_json::to_string).transpose()?,
            wf.concurrency_limit,
            wf.enabled as i32,
            wf.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn delete_workflow(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM workflows WHERE id = ?1", params![id])?;
    Ok(())
}

// ─── Workflow Runs CRUD ─────────────────────────────────────────────────────

pub fn count_runs(conn: &Connection, workflow_id: &str) -> Result<u32> {
    let count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM workflow_runs WHERE workflow_id = ?1",
        params![workflow_id], |row| row.get(0),
    )?;
    Ok(count)
}

pub fn list_runs(conn: &Connection, workflow_id: &str) -> Result<Vec<WorkflowRun>> {
    list_runs_paginated(conn, workflow_id, None, None)
}

pub fn list_runs_paginated(conn: &Connection, workflow_id: &str, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<WorkflowRun>> {
    let sql = format!(
        "SELECT {} FROM workflow_runs WHERE workflow_id = ?1
         ORDER BY started_at DESC{}",
        WORKFLOW_RUN_COLS,
        match (limit, offset) {
            (Some(l), Some(o)) => format!(" LIMIT {} OFFSET {}", l, o),
            (Some(l), None) => format!(" LIMIT {}", l),
            _ => String::new(),
        }
    );
    let mut stmt = conn.prepare(&sql)?;

    let runs = stmt.query_map(params![workflow_id], |row| {
        Ok(row_to_run(row))
    })?.filter_map(|r| r.ok())
    .collect();

    Ok(runs)
}

pub fn get_run(conn: &Connection, run_id: &str) -> Result<Option<WorkflowRun>> {
    let sql = format!("SELECT {} FROM workflow_runs WHERE id = ?1", WORKFLOW_RUN_COLS);
    let mut stmt = conn.prepare(&sql)?;

    let run = stmt.query_row(params![run_id], |row| {
        Ok(row_to_run(row))
    }).ok();

    Ok(run)
}

pub fn insert_run(conn: &Connection, run: &WorkflowRun) -> Result<()> {
    conn.execute(
        "INSERT INTO workflow_runs (id, workflow_id, status, trigger_context,
         step_results_json, tokens_used, workspace_path, started_at, finished_at,
         run_type, batch_total, batch_completed, batch_failed, batch_name, parent_run_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            run.id,
            run.workflow_id,
            run_status_str(&run.status),
            run.trigger_context.as_ref().map(serde_json::to_string).transpose()?,
            serde_json::to_string(&run.step_results)?,
            run.tokens_used as i64,
            run.workspace_path,
            run.started_at.to_rfc3339(),
            run.finished_at.map(|d| d.to_rfc3339()),
            run.run_type,
            run.batch_total as i64,
            run.batch_completed as i64,
            run.batch_failed as i64,
            run.batch_name,
            run.parent_run_id,
        ],
    )?;
    Ok(())
}

/// Atomically increment batch_completed or batch_failed on a batch run, and
/// mark the run as Success/Failed when all children are accounted for.
/// Returns the updated run if the transition happened (either progress tick
/// or final completion), so the caller can broadcast the right WS event.
pub fn increment_batch_progress(
    conn: &Connection,
    run_id: &str,
    child_succeeded: bool,
) -> Result<Option<WorkflowRun>> {
    let column = if child_succeeded { "batch_completed" } else { "batch_failed" };
    conn.execute(
        &format!("UPDATE workflow_runs SET {0} = {0} + 1 WHERE id = ?1 AND run_type = 'batch'", column),
        params![run_id],
    )?;

    // Re-read the run to check if we've reached batch_total.
    let Some(mut run) = get_run(conn, run_id)? else { return Ok(None); };
    if run.run_type != "batch" { return Ok(None); }

    let done = run.batch_completed + run.batch_failed;
    if done >= run.batch_total && run.status == RunStatus::Running {
        // All children done — mark the run final. Success if at least one
        // succeeded, Failed if ALL failed. This matches user intuition:
        // "the batch did something useful" vs "the batch accomplished nothing".
        let final_status = if run.batch_completed > 0 { RunStatus::Success } else { RunStatus::Failed };
        let finished = chrono::Utc::now();
        conn.execute(
            "UPDATE workflow_runs SET status = ?2, finished_at = ?3 WHERE id = ?1",
            params![run_id, run_status_str(&final_status), finished.to_rfc3339()],
        )?;
        run.status = final_status;
        run.finished_at = Some(finished);
    }

    Ok(Some(run))
}

pub fn update_run(conn: &Connection, run: &WorkflowRun) -> Result<()> {
    update_run_progress(conn, RunProgressSnapshot::from_run(run))
}

/// Lightweight snapshot of a WorkflowRun for progress updates.
/// Avoids cloning the entire WorkflowRun (trigger_context, etc.) on every step.
pub struct RunProgressSnapshot {
    pub id: String,
    pub status: RunStatus,
    pub step_results: Vec<StepResult>,
    pub tokens_used: u64,
    pub workspace_path: Option<String>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl RunProgressSnapshot {
    pub fn from_run(run: &WorkflowRun) -> Self {
        Self {
            id: run.id.clone(),
            status: run.status.clone(),
            step_results: run.step_results.clone(),
            tokens_used: run.tokens_used,
            workspace_path: run.workspace_path.clone(),
            finished_at: run.finished_at,
        }
    }
}

pub fn update_run_progress(conn: &Connection, snap: RunProgressSnapshot) -> Result<()> {
    conn.execute(
        "UPDATE workflow_runs SET status = ?2, step_results_json = ?3,
         tokens_used = ?4, workspace_path = ?5, finished_at = ?6
         WHERE id = ?1",
        params![
            snap.id,
            run_status_str(&snap.status),
            serde_json::to_string(&snap.step_results)?,
            snap.tokens_used as i64,
            snap.workspace_path,
            snap.finished_at.map(|d| d.to_rfc3339()),
        ],
    )?;
    Ok(())
}

/// Delete a single run.
pub fn delete_run(conn: &Connection, run_id: &str) -> Result<()> {
    conn.execute("DELETE FROM workflow_runs WHERE id = ?1", params![run_id])?;
    Ok(())
}

/// Delete all runs for a workflow.
pub fn delete_all_runs(conn: &Connection, workflow_id: &str) -> Result<()> {
    conn.execute("DELETE FROM workflow_runs WHERE workflow_id = ?1", params![workflow_id])?;
    Ok(())
}

/// Get the last run for a workflow (for summaries).
/// Batch-load the last run for every workflow in one query (avoids N+1).
pub fn get_last_runs_all(conn: &Connection) -> Result<std::collections::HashMap<String, WorkflowRun>> {
    // Must alias columns with wr. prefix since we join to `latest` — can't
    // reuse the WORKFLOW_RUN_COLS constant directly. Keep the list in sync.
    let mut stmt = conn.prepare(
        "SELECT wr.id, wr.workflow_id, wr.status, wr.trigger_context, wr.step_results_json,
                wr.tokens_used, wr.workspace_path, wr.started_at, wr.finished_at,
                wr.run_type, wr.batch_total, wr.batch_completed, wr.batch_failed, wr.batch_name
         FROM workflow_runs wr
         INNER JOIN (
             SELECT workflow_id, MAX(started_at) AS max_started
             FROM workflow_runs GROUP BY workflow_id
         ) latest ON wr.workflow_id = latest.workflow_id AND wr.started_at = latest.max_started"
    )?;

    let mut map = std::collections::HashMap::new();
    let rows = stmt.query_map([], |row| {
        Ok(row_to_run(row))
    })?;
    for row in rows.filter_map(|r| r.ok()) {
        map.insert(row.workflow_id.clone(), row);
    }
    Ok(map)
}

pub fn get_last_run(conn: &Connection, workflow_id: &str) -> Result<Option<WorkflowRun>> {
    let sql = format!(
        "SELECT {} FROM workflow_runs WHERE workflow_id = ?1 ORDER BY started_at DESC LIMIT 1",
        WORKFLOW_RUN_COLS
    );
    let mut stmt = conn.prepare(&sql)?;

    let run = stmt.query_row(params![workflow_id], |row| {
        Ok(row_to_run(row))
    }).ok();

    Ok(run)
}

/// Count active runs for a workflow (for concurrency limiting).
pub fn count_active_runs(conn: &Connection, workflow_id: &str) -> Result<u32> {
    let count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM workflow_runs WHERE workflow_id = ?1 AND status IN ('Pending', 'Running')",
        params![workflow_id],
        |row| row.get(0),
    )?;
    Ok(count)
}

// ─── Tracker reconciliation ─────────────────────────────────────────────────

pub fn is_issue_processed(conn: &Connection, workflow_id: &str, issue_id: &str) -> Result<bool> {
    let count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM workflow_tracker_processed WHERE workflow_id = ?1 AND issue_id = ?2",
        params![workflow_id, issue_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub fn mark_issue_processed(conn: &Connection, workflow_id: &str, issue_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO workflow_tracker_processed (workflow_id, issue_id, processed_at)
         VALUES (?1, ?2, ?3)",
        params![workflow_id, issue_id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

// ─── Row mappers ────────────────────────────────────────────────────────────

fn row_to_workflow(row: &rusqlite::Row) -> Workflow {
    let trigger_str: String = row.get(3).unwrap_or_default();
    let steps_str: String = row.get(4).unwrap_or_default();
    let actions_str: String = row.get(5).unwrap_or_default();
    let safety_str: String = row.get(6).unwrap_or_default();
    let ws_config_str: Option<String> = row.get(7).unwrap_or(None);
    let concurrency: Option<u32> = row.get(8).unwrap_or(None);

    Workflow {
        id: row.get(0).unwrap_or_default(),
        name: row.get(1).unwrap_or_default(),
        project_id: row.get(2).unwrap_or(None),
        trigger: serde_json::from_str(&trigger_str).unwrap_or(WorkflowTrigger::Manual),
        steps: serde_json::from_str(&steps_str).unwrap_or_default(),
        actions: serde_json::from_str(&actions_str).unwrap_or_default(),
        safety: serde_json::from_str(&safety_str).unwrap_or(WorkflowSafety {
            sandbox: false, max_files: None, max_lines: None, require_approval: false,
        }),
        workspace_config: ws_config_str.and_then(|s| serde_json::from_str(&s).ok()),
        concurrency_limit: concurrency,
        enabled: row.get::<_, i32>(9).unwrap_or(1) != 0,
        created_at: parse_dt(row.get::<_, String>(10).unwrap_or_default()),
        updated_at: parse_dt(row.get::<_, String>(11).unwrap_or_default()),
    }
}

fn row_to_run(row: &rusqlite::Row) -> WorkflowRun {
    let status_str: String = row.get(2).unwrap_or_default();
    let ctx_str: Option<String> = row.get(3).unwrap_or(None);
    let results_str: String = row.get(4).unwrap_or_default();
    // Batch columns 9..13 were added in migration 029, parent_run_id in 030.
    // If the query only selected the legacy columns we fall back to sane
    // defaults via unwrap_or (row.get returns Err on missing index).
    let run_type: String = row.get(9).unwrap_or_else(|_| "linear".to_string());
    let batch_total: i64 = row.get(10).unwrap_or(0);
    let batch_completed: i64 = row.get(11).unwrap_or(0);
    let batch_failed: i64 = row.get(12).unwrap_or(0);
    let batch_name: Option<String> = row.get(13).unwrap_or(None);
    let parent_run_id: Option<String> = row.get(14).unwrap_or(None);

    WorkflowRun {
        id: row.get(0).unwrap_or_default(),
        workflow_id: row.get(1).unwrap_or_default(),
        status: parse_run_status(&status_str),
        trigger_context: ctx_str.and_then(|s| serde_json::from_str(&s).ok()),
        step_results: serde_json::from_str(&results_str).unwrap_or_default(),
        tokens_used: row.get::<_, i64>(5).unwrap_or(0) as u64,
        workspace_path: row.get(6).unwrap_or(None),
        started_at: parse_dt(row.get::<_, String>(7).unwrap_or_default()),
        finished_at: row.get::<_, Option<String>>(8).unwrap_or(None).map(parse_dt),
        run_type,
        batch_total: batch_total as u32,
        batch_completed: batch_completed as u32,
        batch_failed: batch_failed as u32,
        batch_name,
        parent_run_id,
    }
}

/// The column list used in every SELECT that wants the full WorkflowRun.
/// Centralized so adding/removing columns doesn't drift between queries.
const WORKFLOW_RUN_COLS: &str = "id, workflow_id, status, trigger_context, step_results_json, \
    tokens_used, workspace_path, started_at, finished_at, \
    run_type, batch_total, batch_completed, batch_failed, batch_name, parent_run_id";
