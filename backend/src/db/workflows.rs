use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
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
        "StoppedByGuard" => RunStatus::StoppedByGuard,
        "Interrupted" => RunStatus::Interrupted,
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
        RunStatus::StoppedByGuard => "StoppedByGuard",
        RunStatus::Interrupted => "Interrupted",
    }
}

/// 0.8.11 (B5) — reconcile workflow runs left `Running`/`Pending` when the
/// backend process died mid-run (crash, container restart, `kill -9`,
/// cargo-watch reload). Without this they stay "Running" forever, poison the
/// active-runs badge, and make a cron's "did the last run succeed?" check read a
/// zombie as in-progress. Flips rows older than `stale_after_secs` to the
/// terminal `Interrupted` status (distinct from `Failed`). Mirrors
/// `audit_runs::reconcile_stale_runs`. Returns the number reconciled.
/// A run flipped to `Interrupted` by the boot reconcile — enough info to fire
/// the failure webhook for it. The process that would normally have notified
/// (the engine spawn's tail) died with the run, so the boot path is the ONLY
/// place an Interrupted notification can originate.
#[derive(Debug, Clone)]
pub struct ReconciledRun {
    pub run_id: String,
    pub workflow_name: String,
    /// RFC3339, as stored.
    pub started_at: String,
}

pub fn reconcile_stale_runs(conn: &Connection, stale_after_secs: i64) -> Result<Vec<ReconciledRun>> {
    let cutoff = (Utc::now() - chrono::Duration::seconds(stale_after_secs)).to_rfc3339();
    let now_rfc = Utc::now().to_rfc3339();
    // Select-then-update is race-free here: the caller holds the single
    // shared connection for both statements.
    let mut stmt = conn.prepare(
        "SELECT r.id, COALESCE(w.name, r.workflow_id), r.started_at
         FROM workflow_runs r LEFT JOIN workflows w ON w.id = r.workflow_id
         WHERE r.status IN ('Running', 'Pending') AND r.started_at < ?1",
    )?;
    let flipped: Vec<ReconciledRun> = stmt
        .query_map(params![cutoff], |row| {
            Ok(ReconciledRun {
                run_id: row.get(0)?,
                workflow_name: row.get(1)?,
                started_at: row.get(2)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    if !flipped.is_empty() {
        conn.execute(
            "UPDATE workflow_runs SET
                status = 'Interrupted',
                finished_at = COALESCE(finished_at, ?2)
             WHERE status IN ('Running', 'Pending') AND started_at < ?1",
            params![cutoff, now_rfc],
        )?;
    }
    Ok(flipped)
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
                created_at, updated_at, guards, artifacts, on_failure, exec_allowlist, variables
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
                created_at, updated_at, guards, artifacts, on_failure, exec_allowlist, variables
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

/// One row of the batch input — title, fully-rendered prompt, and an
/// optional per-item agent override. The override lets the caller
/// fan out the SAME prompt across MULTIPLE agents (Compare-agents
/// mode) — when `None`, the QP's default `agent` is used (the
/// classic "vary input" mode).
pub struct BatchItemInput {
    pub title: String,
    pub prompt: String,
    pub agent_override: Option<crate::models::AgentType>,
}

/// Input for [`create_batch_run`]. Everything needed to build a batch
/// workflow run plus its N child discussions in a single transaction.
pub struct CreateBatchRunInput<'a> {
    pub quick_prompt: &'a QuickPrompt,
    /// One entry per child discussion. Title + rendered prompt +
    /// optional per-item agent override (Compare-agents mode). The
    /// caller is responsible for template substitution on the prompt.
    pub items: Vec<BatchItemInput>,
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
        state: ::std::collections::HashMap::new(),
        produced_branches: vec![],
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
    };

    let lang = if input.language.is_empty() { "fr".to_string() } else { input.language };
    // Request's project_id overrides QP's default when set.
    let effective_project_id = input.project_id.clone().or_else(|| qp.project_id.clone());
    let workspace_mode = if input.workspace_mode.is_empty() {
        "Direct".to_string()
    } else {
        input.workspace_mode
    };

    let discussions: Vec<(Discussion, DiscussionMessage)> = input.items.iter().map(|item| {
        let disc_id = Uuid::new_v4().to_string();
        let initial_message = DiscussionMessage {
            model: None,
            lint_report: None,
            id: Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content: item.prompt.clone(),
            agent_type: None,
            timestamp: now,
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: input.author_pseudo.clone(),
            author_avatar_email: input.author_avatar_email.clone(),
            source_msg_id: None, duration_ms: None,
        };
        // Per-item agent override (Compare-agents mode) falls back
        // to the QP's default agent when None.
        let effective_agent = item.agent_override.clone().unwrap_or_else(|| qp.agent.clone());
        let discussion = Discussion {
            id: disc_id,
            project_id: effective_project_id.clone(),
            title: item.title.clone(),
            agent: effective_agent.clone(),
            language: lang.clone(),
            participants: vec![effective_agent],
            messages: vec![initial_message.clone()],
            message_count: 1, non_system_message_count: 1,
            skill_ids: qp.skill_ids.clone(),
            // 0.8.5 — QP bindings now flow into the child discussion so
            // a "compare agents", "batch run", or QP-chain spawn inherits
            // the persona + directives picked at the QP level. Pre-0.8.5
            // these were always empty.
            profile_ids: qp.profile_ids.clone(),
            directive_ids: qp.directive_ids.clone(),
            archived: false,
            pinned: false,
            workspace_mode: workspace_mode.clone(),
            workspace_path: None,
            worktree_branch: None,
            tier: qp.tier,
            // 0.8.10 — a QP-launched batch discussion inherits the QP's explicit
            // model (consumed via disc.model → model_override once the batch
            // agent-run path reads it in 2b-2).
            model: qp.agent_settings.as_ref().and_then(|s| s.model.clone()),
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::Auto,
            introspection_call_count: 0,
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

    // 0.8.5 — resolve the CURRENT QP version once outside the loop. The
    // version_index is stamped on every spawned discussion so the metrics
    // aggregator can group "what was the QP body that produced this run".
    // `None` means the QP has no snapshot (legacy QP pre-0.8.5 without a
    // backfill); the discussion-level columns stay NULL in that case.
    let originating_version = crate::db::quick_prompts::current_version_index(conn, &qp.id)
        .ok()
        .flatten();

    // Single transaction: placeholder workflow + run + all discussions/messages.
    conn.execute_batch("BEGIN")?;
    let tx_result: Result<()> = (|| {
        ensure_batch_placeholder_workflow(conn, &qp.id, &qp.name, qp.project_id.as_deref())?;
        insert_run(conn, &run)?;
        for (disc, msg) in &discussions {
            crate::db::discussions::insert_discussion(conn, disc)?;
            crate::db::discussions::insert_message(conn, &disc.id, msg)?;
            // Stamp the QP lineage. Skipped when the QP has no version
            // snapshot — the column simply stays NULL and the metrics
            // aggregator excludes the row.
            if let Some(v) = originating_version {
                crate::db::discussions::set_originating_qp(conn, &disc.id, &qp.id, v)?;
            }
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
         safety_json, workspace_config_json, concurrency_limit, enabled, created_at, updated_at, guards, artifacts, on_failure, exec_allowlist, variables)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
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
            wf.guards.as_ref().map(serde_json::to_string).transpose()?,
            if wf.artifacts.is_empty() { None } else { Some(serde_json::to_string(&wf.artifacts)?) },
            if wf.on_failure.is_empty() { None } else { Some(serde_json::to_string(&wf.on_failure)?) },
            if wf.exec_allowlist.is_empty() { None } else { Some(serde_json::to_string(&wf.exec_allowlist)?) },
            // 0.6.0 UX pass — same NULL-vs-empty discipline. Empty
            // variables = no manual launch form. NULL on disk so a
            // `WHERE variables IS NOT NULL` lists only workflows that
            // actually need a launch dialog.
            if wf.variables.is_empty() { None } else { Some(serde_json::to_string(&wf.variables)?) },
        ],
    )?;
    Ok(())
}

/// Returns `false` when the row no longer exists (deleted concurrently) —
/// a TYPED signal, so callers never string-match the error message.
/// Reporting success on 0 rows would silently drop the caller's edit.
pub fn update_workflow(conn: &Connection, wf: &Workflow) -> Result<bool> {
    let n = conn.execute(
        "UPDATE workflows SET name = ?2, project_id = ?3, trigger_json = ?4, steps_json = ?5,
         actions_json = ?6, safety_json = ?7, workspace_config_json = ?8,
         concurrency_limit = ?9, enabled = ?10, updated_at = ?11, guards = ?12, artifacts = ?13,
         on_failure = ?14, exec_allowlist = ?15, variables = ?16
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
            wf.guards.as_ref().map(serde_json::to_string).transpose()?,
            if wf.artifacts.is_empty() { None } else { Some(serde_json::to_string(&wf.artifacts)?) },
            if wf.on_failure.is_empty() { None } else { Some(serde_json::to_string(&wf.on_failure)?) },
            if wf.exec_allowlist.is_empty() { None } else { Some(serde_json::to_string(&wf.exec_allowlist)?) },
            if wf.variables.is_empty() { None } else { Some(serde_json::to_string(&wf.variables)?) },
        ],
    )?;
    Ok(n > 0)
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

/// Safety cap for the unpaginated `list_runs` — a workflow with thousands of
/// runs (a fast cron) would otherwise load every row WITH its full
/// `step_results_json` into memory on each page open. The UI folds at 10 and
/// paginates; 500 recent runs is far more than any view needs. Callers that
/// truly need everything use `list_runs_paginated` explicitly. (B7, 0.8.11)
pub const MAX_RUNS_UNPAGINATED: u32 = 500;

pub fn list_runs(conn: &Connection, workflow_id: &str) -> Result<Vec<WorkflowRun>> {
    list_runs_paginated(conn, workflow_id, Some(MAX_RUNS_UNPAGINATED), None)
}

/// 0.8.11 (B7) — auto-purge terminal workflow runs older than `days`. Preserves
/// any run still referenced as a parent by a retained child (so provenance
/// chains stay intact) and never touches non-terminal runs. Opt-in: the caller
/// only invokes this when `run_retention_days > 0`. Returns rows deleted.
pub fn purge_runs_older_than(conn: &Connection, days: u32) -> Result<usize> {
    let n = conn.execute(
        "DELETE FROM workflow_runs
          WHERE status IN ('Success','Failed','Cancelled','StoppedByGuard','Interrupted')
            AND finished_at IS NOT NULL
            AND finished_at < datetime('now', ?1)
            AND id NOT IN (
                SELECT parent_run_id FROM workflow_runs WHERE parent_run_id IS NOT NULL
            )",
        params![format!("-{} days", days)],
    )?;
    Ok(n)
}

/// True when at least one workflow run is currently `Running` or `Pending`.
///
/// Used by `mcp_scanner::sync_affected_projects` /
/// `sync_global_configs` to back off host-config writes during a live
/// workflow — see TD-20260427-host-sync-workflow-race. The point is to
/// avoid clobbering `~/.claude.json` etc. while an agent we just spawned
/// is reading those files at startup; the next save (or next sync trigger)
/// will catch up once the run finishes.
pub fn has_running_run(conn: &Connection) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM workflow_runs WHERE status IN ('Running', 'Pending')",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
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

    let mut runs: Vec<WorkflowRun> = stmt.query_map(params![workflow_id], |row| {
        Ok(row_to_run(row))
    })?.filter_map(|r| r.ok())
    .collect();

    enrich_parent_provenance(conn, &mut runs)?;
    Ok(runs)
}

/// Fill the DERIVED `parent_workflow_id/name` + `parent_run_started_at` fields
/// on any run that has a `parent_run_id`, via a SINGLE batch query (no N+1).
/// Resolves each distinct parent run id → its workflow id/name + start time.
/// A dangling parent (deleted run) simply leaves the fields `None`.
pub(crate) fn enrich_parent_provenance(conn: &Connection, runs: &mut [WorkflowRun]) -> Result<()> {
    use std::collections::HashMap;
    // Distinct, non-empty parent ids present in this batch.
    let mut ids: Vec<String> = runs.iter()
        .filter_map(|r| r.parent_run_id.clone())
        .filter(|s| !s.is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Ok(());
    }

    // One query: parent run id → (parent workflow id, name, parent run start).
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT pr.id, w.id, w.name, pr.started_at \
         FROM workflow_runs pr JOIN workflows w ON w.id = pr.workflow_id \
         WHERE pr.id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let mut map: HashMap<String, (String, String, DateTime<Utc>)> = HashMap::new();
    let rows = stmt.query_map(params_ref.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            parse_dt(row.get::<_, String>(3)?),
        ))
    })?;
    for r in rows.filter_map(|r| r.ok()) {
        map.insert(r.0, (r.1, r.2, r.3));
    }

    for run in runs.iter_mut() {
        if let Some(pid) = run.parent_run_id.as_deref() {
            if let Some((wid, wname, started)) = map.get(pid) {
                run.parent_workflow_id = Some(wid.clone());
                run.parent_workflow_name = Some(wname.clone());
                run.parent_run_started_at = Some(*started);
            }
        }
    }
    Ok(())
}

pub fn get_run(conn: &Connection, run_id: &str) -> Result<Option<WorkflowRun>> {
    let sql = format!("SELECT {} FROM workflow_runs WHERE id = ?1", WORKFLOW_RUN_COLS);
    let mut stmt = conn.prepare(&sql)?;

    let run = stmt.query_row(params![run_id], |row| {
        Ok(row_to_run(row))
    }).ok();

    // Enrich provenance so a single run detail also shows "↳ depuis <parent>".
    let mut run = run;
    if let Some(r) = run.as_mut() {
        enrich_parent_provenance(conn, std::slice::from_mut(r))?;
    }
    Ok(run)
}

pub fn insert_run(conn: &Connection, run: &WorkflowRun) -> Result<()> {
    conn.execute(
        "INSERT INTO workflow_runs (id, workflow_id, status, trigger_context,
         step_results_json, tokens_used, workspace_path, started_at, finished_at,
         run_type, batch_total, batch_completed, batch_failed, batch_name, parent_run_id, state,
         produced_branches)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
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
            // Empty map → NULL (lets a `WHERE state IS NOT NULL` query
            // surface only runs that actually carried state).
            if run.state.is_empty() { None } else { Some(serde_json::to_string(&run.state)?) },
            if run.produced_branches.is_empty() { None } else { Some(serde_json::to_string(&run.produced_branches)?) },
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
    // Counters are part of the terminal state (waiters/UI read them): a late
    // child must not advance a batch already Cancelled/Interrupted.
    let bumped = conn.execute(
        &format!(
            "UPDATE workflow_runs SET {0} = {0} + 1              WHERE id = ?1 AND run_type = 'batch' AND status = 'Running'",
            column
        ),
        params![run_id],
    )?;
    if bumped == 0 {
        // A′ observability — a frozen counter means a child completed after
        // the batch left Running (late finisher post-cancel/terminal).
        tracing::warn!(
            target: "kronn::invariant",
            run_id = %run_id, counter = %column,
            "batch progress bump blocked — batch no longer Running"
        );
        return Ok(None);
    }

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
            "UPDATE workflow_runs SET status = ?2, finished_at = ?3              WHERE id = ?1 AND status = 'Running'",
            params![run_id, run_status_str(&final_status), finished.to_rfc3339()],
        )?;
        run.status = final_status;
        run.finished_at = Some(finished);
    }

    Ok(Some(run))
}

/// 2026-06-10 (audit P1, TOCTOU) — atomic claim of a gate decision. Flips a
/// run from `WaitingApproval` to `new_status` ONLY if it is still waiting,
/// and reports whether THIS caller won. Two concurrent `decide_run`s (a
/// double-click, or a human racing the auto-approve timer) used to both
/// pass the read-then-check and spawn two concurrent `resume_run`s on the
/// same run; the conditional UPDATE makes exactly one of them win.
/// Atomic status claim: flips `from_status` → `new_status` iff the row still
/// holds `from_status`. Exactly ONE concurrent caller wins (TOCTOU-free).
/// The only sanctioned way OUT of `WaitingApproval` (gate decide) and
/// `Interrupted` (manual resume) — ordinary snapshots can't touch those.
/// Atomically merge ONE key into a run's durable `state` map, guarded by
/// status (A2). Unlike a full `update_run_progress` snapshot, this can't
/// clobber concurrent fields and can't move the run's status — it only lands
/// while the row still holds one of `allowed` (read+merge+write under the
/// single shared connection; the UPDATE re-checks the observed status).
/// Returns `false` when the run is gone or its status changed hands.
pub fn set_run_state_key(
    conn: &Connection,
    run_id: &str,
    key: &str,
    value: &str,
    allowed: &[RunStatus],
) -> Result<bool> {
    let row: Option<(Option<String>, String)> = conn
        .query_row(
            "SELECT state, status FROM workflow_runs WHERE id = ?1",
            params![run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((state_json, status_s)) = row else { return Ok(false) };
    let status = parse_run_status(&status_s);
    if !allowed.contains(&status) {
        return Ok(false);
    }
    let mut map: std::collections::HashMap<String, String> = state_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    map.insert(key.to_string(), value.to_string());
    let n = conn.execute(
        "UPDATE workflow_runs SET state = ?2 WHERE id = ?1 AND status = ?3",
        params![run_id, serde_json::to_string(&map)?, status_s],
    )?;
    Ok(n == 1)
}

/// A2 — append one entry to the versioned foreach done-set
/// (`__kronn.foreach_done.<step>` → `{"v":1,"items":[…]}`), only while the
/// parent is still `Running`. One connection borrow = atomic read+append+write.
pub fn append_foreach_done(
    conn: &Connection,
    run_id: &str,
    step_name: &str,
    entry: serde_json::Value,
) -> Result<bool> {
    let key = format!("__kronn.foreach_done.{step_name}");
    let cur: Option<Option<String>> = conn
        .query_row(
            "SELECT state FROM workflow_runs WHERE id = ?1",
            params![run_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(state_json) = cur else { return Ok(false) };
    let map: std::collections::HashMap<String, String> = state_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let mut doc: serde_json::Value = map
        .get(&key)
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_else(|| serde_json::json!({"v": 1, "items": []}));
    if let Some(items) = doc.get_mut("items").and_then(|i| i.as_array_mut()) {
        items.push(entry);
    }
    set_run_state_key(conn, run_id, &key, &doc.to_string(), &[RunStatus::Running])
}

/// A2 — item ids of this parent's `Success` sub-workflow children (read off
/// `trigger_context.__subwf_item_id__`). Resume reconciliation source #2:
/// a child that finished right before the crash counts as done even when the
/// parent died before writing its done-set entry.
pub fn successful_child_item_ids(
    conn: &Connection,
    parent_run_id: &str,
) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT trigger_context FROM workflow_runs
         WHERE parent_run_id = ?1 AND run_type = 'subworkflow' AND status = 'Success'",
    )?;
    let rows = stmt.query_map(params![parent_run_id], |r| r.get::<_, Option<String>>(0))?;
    let mut ids = std::collections::HashSet::new();
    for row in rows {
        if let Some(id) = row?
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v.get("__subwf_item_id__").and_then(|i| i.as_str()).map(String::from))
            .filter(|i| !i.is_empty())
        {
            ids.insert(id);
        }
    }
    Ok(ids)
}

pub fn claim_run_status(
    conn: &Connection,
    run_id: &str,
    from_status: &RunStatus,
    new_status: &RunStatus,
) -> Result<bool> {
    let n = conn.execute(
        "UPDATE workflow_runs SET status = ?3 WHERE id = ?1 AND status = ?2",
        params![run_id, run_status_str(from_status), run_status_str(new_status)],
    )?;
    Ok(n == 1)
}

pub fn claim_waiting_run(conn: &Connection, run_id: &str, new_status: &RunStatus) -> Result<bool> {
    claim_run_status(conn, run_id, &RunStatus::WaitingApproval, new_status)
}

pub fn update_run(conn: &Connection, run: &WorkflowRun) -> Result<()> {
    // Same Cancelled-stickiness semantics; callers of this convenience
    // wrapper don't act on the raced-cancel signal.
    update_run_progress(conn, RunProgressSnapshot::from_run(run)).map(|_| ())
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
    /// 0.7.0 Phase 6 — durable state map. Persisted on every progress
    /// write so a daemon crash mid-run doesn't lose accumulated counters.
    pub state: ::std::collections::HashMap<String, String>,
    /// 0.7.0 — branches preserved during worktree cleanup. Cleared
    /// every snapshot — the runner re-sets it (typically once at the
    /// terminal write) so we don't accumulate duplicates across writes.
    pub produced_branches: Vec<crate::models::ProducedBranch>,
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
            state: run.state.clone(),
            produced_branches: run.produced_branches.clone(),
        }
    }
}

/// Returns `false` when the row was NOT updated — it no longer exists, or the
/// state-machine guard blocked the write.
///
/// Guard (0.8.11 A1): a progress snapshot may only land when the row is
/// `Pending`/`Running`, or when it rewrites the SAME status (idempotence).
/// Every terminal status is sticky (a zombie runner's late snapshot could
/// resurrect `Cancelled`/`Interrupted`/`Failed`/`Success`), and
/// `WaitingApproval` only leaves via the claim family (`claim_run_status`),
/// never via an ordinary snapshot. Runner sites treat `false` as "the run
/// changed hands beneath us" and stop.
pub fn update_run_progress(conn: &Connection, snap: RunProgressSnapshot) -> Result<bool> {
    let new_status = run_status_str(&snap.status);
    let affected = conn.execute(
        "UPDATE workflow_runs SET status = ?2, step_results_json = ?3,
         tokens_used = ?4, workspace_path = ?5, finished_at = ?6, state = ?7,
         produced_branches = ?8
         WHERE id = ?1 AND (status = ?2 OR status IN ('Pending', 'Running'))",
        params![
            snap.id,
            new_status,
            serde_json::to_string(&snap.step_results)?,
            snap.tokens_used as i64,
            snap.workspace_path,
            snap.finished_at.map(|d| d.to_rfc3339()),
            if snap.state.is_empty() { None } else { Some(serde_json::to_string(&snap.state)?) },
            if snap.produced_branches.is_empty() { None } else { Some(serde_json::to_string(&snap.produced_branches)?) },
        ],
    )?;
    if affected == 0 {
        // A′ observability — a blocked write is either the stickiness guard
        // doing its job (late runner write racing a Cancel) or a genuinely
        // impossible transition; both must be visible, not silent.
        let held: Option<String> = conn
            .query_row(
                "SELECT status FROM workflow_runs WHERE id = ?1",
                params![snap.id],
                |r| r.get(0),
            )
            .optional()?;
        match held {
            Some(held) => tracing::warn!(
                target: "kronn::invariant",
                run_id = %snap.id, held = %held, attempted = %new_status,
                "run progress write blocked by status guard"
            ),
            None => tracing::warn!(
                target: "kronn::invariant",
                run_id = %snap.id, attempted = %new_status,
                "run progress write targets a missing run row"
            ),
        }
    }
    Ok(affected > 0)
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
                wr.run_type, wr.batch_total, wr.batch_completed, wr.batch_failed, wr.batch_name,
                wr.parent_run_id, wr.state
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
    let id: String = row.get(0).unwrap_or_default();
    let trigger_str: String = row.get(3).unwrap_or_default();
    let steps_str: String = row.get(4).unwrap_or_default();
    let actions_str: String = row.get(5).unwrap_or_default();
    let safety_str: String = row.get(6).unwrap_or_default();
    let ws_config_str: Option<String> = row.get(7).unwrap_or(None);
    let concurrency: Option<u32> = row.get(8).unwrap_or(None);
    let guards_str: Option<String> = row.get(12).unwrap_or(None);
    let artifacts_str: Option<String> = row.get(13).unwrap_or(None);
    let on_failure_str: Option<String> = row.get(14).unwrap_or(None);
    let exec_allowlist_str: Option<String> = row.get(15).unwrap_or(None);
    let variables_str: Option<String> = row.get(16).unwrap_or(None);

    // These two fallbacks keep a workflow with corrupt JSON loadable (booting
    // matters), but they MUST be loud: a silently-Manual trigger kills a cron
    // workflow, and silently-empty steps make a corrupt workflow run green as
    // a no-op.
    let trigger = serde_json::from_str(&trigger_str).unwrap_or_else(|e| {
        tracing::error!(
            workflow_id = %id,
            error = %e,
            "corrupt trigger_json — falling back to Manual (scheduled/tracker triggers DISABLED for this workflow)"
        );
        WorkflowTrigger::Manual
    });
    let steps: Vec<WorkflowStep> = serde_json::from_str(&steps_str).unwrap_or_else(|e| {
        tracing::error!(
            workflow_id = %id,
            error = %e,
            "corrupt steps_json — falling back to ZERO steps (this workflow would run as a no-op)"
        );
        Vec::new()
    });

    Workflow {
        id,
        name: row.get(1).unwrap_or_default(),
        project_id: row.get(2).unwrap_or(None),
        trigger,
        steps,
        actions: serde_json::from_str(&actions_str).unwrap_or_default(),
        safety: serde_json::from_str(&safety_str).unwrap_or(WorkflowSafety {
            sandbox: false, max_files: None, max_lines: None, require_approval: false,
        }),
        workspace_config: ws_config_str.and_then(|s| serde_json::from_str(&s).ok()),
        concurrency_limit: concurrency,
        // Defensive: a corrupt JSON blob in `guards` should NOT silently
        // disable the safety net — fall back to the column being absent
        // (= backend defaults applied) so the runner still kills runaway
        // runs. Logging would be nice to add when we wire structured
        // tracing for the workflow engine.
        guards: guards_str.as_deref().and_then(|s| serde_json::from_str::<WorkflowGuards>(s).ok()),
        // Same defensive pattern: corrupt artifacts JSON falls back to
        // empty (workflow runs without artifact persistence) instead of
        // failing the whole load. The user sees missing artifacts in
        // the UI rather than a workflow that won't list at all.
        artifacts: artifacts_str.as_deref()
            .and_then(|s| serde_json::from_str::<::std::collections::HashMap<String, ArtifactSpec>>(s).ok())
            .unwrap_or_default(),
        // Same defensive pattern as artifacts: corrupt JSON or missing
        // column → empty rollback chain. The main pipeline still runs;
        // only the safety net is silently skipped.
        on_failure: on_failure_str.as_deref()
            .and_then(|s| serde_json::from_str::<Vec<WorkflowStep>>(s).ok())
            .unwrap_or_default(),
        // 0.7.0 Phase 5 — defensive parse; corrupt allowlist JSON →
        // empty (Exec disabled). Failing closed is the right default
        // for a security-sensitive feature.
        exec_allowlist: exec_allowlist_str.as_deref()
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .unwrap_or_default(),
        // 0.6.0 UX pass — defensive parse; corrupt JSON / missing
        // column → empty (legacy workflows without manual variables).
        variables: variables_str.as_deref()
            .and_then(|s| serde_json::from_str::<Vec<PromptVariable>>(s).ok())
            .unwrap_or_default(),
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
    // 0.7.0 Phase 6 — state map. Tolerant: missing column on legacy
    // rows or corrupt JSON falls back to empty (the run still loads,
    // just without prior state — fresh runs aren't affected).
    let state_str: Option<String> = row.get(15).unwrap_or(None);
    // 0.7.0 — branches preserved by the runner during worktree cleanup.
    // Same tolerance as `state` for legacy / corrupt rows.
    let produced_branches_str: Option<String> = row.get(16).unwrap_or(None);

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
        state: state_str.as_deref()
            .and_then(|s| serde_json::from_str::<::std::collections::HashMap<String, String>>(s).ok())
            .unwrap_or_default(),
        produced_branches: produced_branches_str.as_deref()
            .and_then(|s| serde_json::from_str::<Vec<crate::models::ProducedBranch>>(s).ok())
            .unwrap_or_default(),
        // Derived, filled by enrich_parent_provenance (never from a column).
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
    }
}

/// The column list used in every SELECT that wants the full WorkflowRun.
/// Centralized so adding/removing columns doesn't drift between queries.
const WORKFLOW_RUN_COLS: &str = "id, workflow_id, status, trigger_context, step_results_json, \
    tokens_used, workspace_path, started_at, finished_at, \
    run_type, batch_total, batch_completed, batch_failed, batch_name, parent_run_id, state, \
    produced_branches";
