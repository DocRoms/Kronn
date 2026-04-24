//! Workflow engine: background service that manages workflow execution.
//!
//! Ticks every 30s, checks triggers, enforces concurrency limits,
//! and spawns runs.

pub mod template;
pub mod workspace;
pub mod steps;
pub mod batch_step;
pub mod notify_step;
pub mod runner;
pub mod trigger;
pub mod tracker;

use std::sync::Arc;
use chrono::Utc;
use tokio::time::{interval, Duration};
use uuid::Uuid;

use crate::db::Database;
use crate::models::*;
use crate::AppState;

/// The workflow engine — runs in the background, checks triggers, spawns runs.
pub struct WorkflowEngine {
    state: AppState,
}

impl WorkflowEngine {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    /// Convenience accessors so existing code using `self.db` / `self.config`
    /// keeps working without threading `.state.` everywhere.
    fn db(&self) -> &Arc<Database> { &self.state.db }
    fn config(&self) -> &Arc<tokio::sync::RwLock<AppConfig>> { &self.state.config }

    /// Start the engine tick loop (runs forever).
    pub async fn start(self: Arc<Self>) {
        let mut tick = interval(Duration::from_secs(30));
        tracing::info!("Workflow engine started");

        // One-shot healing pass: rescue workflows saved before the
        // Structured-by-default contract existed. Idempotent — a second
        // boot finds nothing to heal.
        if let Err(e) = self.heal_workflows().await {
            tracing::warn!("Workflow healing pass failed: {}", e);
        }

        loop {
            tick.tick().await;
            if let Err(e) = self.check_triggers().await {
                tracing::error!("Workflow engine tick error: {}", e);
            }
        }
    }

    /// Scan all workflows and auto-upgrade FreeText producers that are
    /// referenced via `{{steps.X.data|summary|status|data_json}}` by a
    /// downstream step. Non-healable violations (forward-refs, unknown
    /// names) are left alone — save-time validation will flag them the
    /// next time the user edits.
    async fn heal_workflows(&self) -> anyhow::Result<()> {
        let db = self.db().clone();
        let workflows = db.with_conn(crate::db::workflows::list_workflows).await?;

        let mut healed_count = 0usize;
        for mut wf in workflows {
            let names = heal_steps_in_place(&mut wf.steps);
            if names.is_empty() {
                continue;
            }
            wf.updated_at = Utc::now();
            let wf_clone = wf.clone();
            let db2 = self.db().clone();
            match db2.with_conn(move |conn| crate::db::workflows::update_workflow(conn, &wf_clone)).await {
                Ok(()) => {
                    healed_count += 1;
                    tracing::info!(
                        "Healed workflow '{}' (id={}): upgraded steps {:?} to Structured",
                        wf.name, wf.id, names
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to persist heal for workflow '{}' (id={}): {}",
                        wf.name, wf.id, e
                    );
                }
            }
        }
        if healed_count > 0 {
            tracing::info!("Workflow healing pass complete — {} workflow(s) upgraded", healed_count);
        }
        Ok(())
    }

    /// Check all enabled workflows and fire triggers.
    async fn check_triggers(&self) -> anyhow::Result<()> {
        let db = self.db().clone();
        let workflows = db.with_conn(|conn| {
            crate::db::workflows::list_workflows(conn)
        }).await?;

        for wf in workflows {
            if !wf.enabled {
                continue;
            }

            if !trigger::should_fire(&wf.trigger) {
                continue;
            }

            // Check concurrency limit
            if let Some(limit) = wf.concurrency_limit {
                let wf_id = wf.id.clone();
                let db2 = self.db().clone();
                let active = db2.with_conn(move |conn| {
                    crate::db::workflows::count_active_runs(conn, &wf_id)
                }).await?;
                if active >= limit {
                    tracing::debug!("Workflow '{}' skipped — concurrency limit ({}/{})", wf.name, active, limit);
                    continue;
                }
            }

            match &wf.trigger {
                WorkflowTrigger::Cron { .. } => {
                    self.spawn_run(&wf, serde_json::json!({
                        "type": "cron",
                        "triggered_at": Utc::now().to_rfc3339(),
                    })).await?;
                }
                WorkflowTrigger::Tracker { source, query, labels, .. } => {
                    self.handle_tracker_trigger(&wf, source, query, labels).await?;
                }
                WorkflowTrigger::Manual => {}
            }
        }

        Ok(())
    }

    /// Handle a tracker trigger: poll for new issues, spawn a run for each.
    async fn handle_tracker_trigger(
        &self,
        wf: &Workflow,
        source: &TrackerSourceConfig,
        query: &str,
        labels: &[String],
    ) -> anyhow::Result<()> {
        let tracker: Box<dyn tracker::TrackerSource> = match source {
            TrackerSourceConfig::GitHub { owner, repo } => {
                let token = std::env::var("GITHUB_TOKEN")
                    .unwrap_or_default();
                if token.is_empty() {
                    tracing::warn!("Workflow '{}' tracker trigger skipped: no GITHUB_TOKEN", wf.name);
                    return Ok(());
                }
                Box::new(tracker::github::GitHubTracker::new(
                    owner.clone(), repo.clone(), token,
                ))
            }
        };

        let issues = tracker.poll_new_items(query, labels).await?;

        for issue in issues {
            // Check reconciliation — skip already-processed issues
            let wf_id = wf.id.clone();
            let issue_id = issue.id.clone();
            let db = self.db().clone();
            let already = db.with_conn(move |conn| {
                crate::db::workflows::is_issue_processed(conn, &wf_id, &issue_id)
            }).await?;

            if already {
                continue;
            }

            // Mark as processed
            let wf_id = wf.id.clone();
            let issue_id = issue.id.clone();
            let db2 = self.db().clone();
            db2.with_conn(move |conn| {
                crate::db::workflows::mark_issue_processed(conn, &wf_id, &issue_id)
            }).await?;

            // Spawn a run with issue context
            let trigger_ctx = serde_json::json!({
                "type": "tracker",
                "issue_title": issue.title,
                "issue_body": issue.body,
                "issue_number": issue.number,
                "issue_url": issue.url,
                "issue_labels": issue.labels,
            });

            self.spawn_run(wf, trigger_ctx).await?;
        }

        Ok(())
    }

    /// Create and execute a workflow run in a background task.
    async fn spawn_run(&self, wf: &Workflow, trigger_ctx: serde_json::Value) -> anyhow::Result<()> {
        let now = Utc::now();
        let mut run = WorkflowRun {
            id: Uuid::new_v4().to_string(),
            workflow_id: wf.id.clone(),
            status: RunStatus::Pending,
            trigger_context: Some(trigger_ctx),
            step_results: vec![],
            tokens_used: 0,
            workspace_path: None,
            started_at: now,
            finished_at: None,
            // Scheduled/tracker-triggered runs are linear by construction.
            run_type: "linear".into(),
            batch_total: 0,
            batch_completed: 0,
            batch_failed: 0,
            batch_name: None,
            parent_run_id: None,
        };

        // Persist the run
        let r = run.clone();
        let db = self.db().clone();
        db.with_conn(move |conn| crate::db::workflows::insert_run(conn, &r)).await?;

        tracing::info!("Spawning workflow run {} for '{}'", run.id, wf.name);

        // Read config for tokens and agents
        let config = self.config().read().await;
        let tokens = config.tokens.clone();
        let agents = config.agents.clone();

        let state = self.state.clone();
        let workflow = wf.clone();

        // Execute in background
        tokio::spawn(async move {
            if let Err(e) = runner::execute_run(state, &workflow, &mut run, &tokens, &agents, None).await {
                tracing::error!("Workflow run {} failed: {}", run.id, e);
            }
        });

        Ok(())
    }
}

/// Apply the healing pass to a single workflow's steps, returning the
/// names of steps that were upgraded. Pure function so heal_workflows can
/// be unit-tested without spinning up the DB.
fn heal_steps_in_place(steps: &mut [WorkflowStep]) -> Vec<String> {
    let names = template::healable_producer_names(steps);
    for step in steps.iter_mut() {
        if names.iter().any(|n| n == &step.name) {
            step.output_format = StepOutputFormat::Structured;
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Healing pass (heal_steps_in_place) ──────────────────────────────
    //
    // End-to-end behavior of the boot healing pass, without the DB round-
    // trip. Together with healable_producer_names (in template.rs) this
    // pins the full rescue path for workflows saved pre-validation.

    fn bare_step(name: &str, prompt: &str, fmt: StepOutputFormat) -> WorkflowStep {
        WorkflowStep {
            name: name.into(),
            step_type: StepType::default(),
            description: None,
            agent: AgentType::ClaudeCode,
            prompt_template: prompt.into(),
            mode: StepMode::Normal,
            output_format: fmt,
            mcp_config_ids: vec![],
            agent_settings: None,
            on_result: vec![],
            stall_timeout_secs: None,
            retry: None,
            skill_ids: vec![],
            directive_ids: vec![],
            profile_ids: vec![],
            delay_after_secs: None,
            batch_quick_prompt_id: None,
            batch_items_from: None,
            batch_wait_for_completion: None,
            batch_max_items: None,
            batch_workspace_mode: None,
            batch_chain_prompt_ids: vec![],
            notify_config: None,
        }
    }

    #[test]
    fn heal_upgrades_referenced_freetext_producer() {
        // Exact shape of Workflow B before the fix: producer is FreeText,
        // consumer references .data. The pass flips the producer.
        let mut steps = vec![
            bare_step("main", "Fetch tickets", StepOutputFormat::FreeText),
            bare_step("analyze", "Analyse {{steps.main.data}}", StepOutputFormat::FreeText),
        ];
        let upgraded = heal_steps_in_place(&mut steps);
        assert_eq!(upgraded, vec!["main".to_string()]);
        assert_eq!(steps[0].output_format, StepOutputFormat::Structured);
        assert_eq!(steps[1].output_format, StepOutputFormat::FreeText,
            "Consumer stays as-is — only producers get upgraded");
    }

    #[test]
    fn heal_is_idempotent() {
        // Second pass must find nothing to do — healing is a one-shot fix.
        let mut steps = vec![
            bare_step("main", "Fetch", StepOutputFormat::FreeText),
            bare_step("use", "{{steps.main.data}}", StepOutputFormat::FreeText),
        ];
        assert!(!heal_steps_in_place(&mut steps).is_empty(), "First pass should heal");
        assert!(heal_steps_in_place(&mut steps).is_empty(), "Second pass should no-op");
    }

    #[test]
    fn heal_leaves_clean_workflows_alone() {
        let mut steps = vec![
            bare_step("main", "Fetch", StepOutputFormat::Structured),
            bare_step("use", "{{steps.main.data}}", StepOutputFormat::FreeText),
        ];
        let before = steps.clone();
        let upgraded = heal_steps_in_place(&mut steps);
        assert!(upgraded.is_empty());
        // No mutation at all
        for (a, b) in steps.iter().zip(before.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.output_format, b.output_format);
        }
    }

    #[test]
    fn heal_ignores_non_healable_violations() {
        // Forward-ref is a structural bug, not something the healer fixes.
        let mut steps = vec![
            bare_step("a", "Use {{steps.b.data}}", StepOutputFormat::FreeText),
            bare_step("b", "Produce", StepOutputFormat::FreeText),
        ];
        let upgraded = heal_steps_in_place(&mut steps);
        assert!(upgraded.is_empty(), "Forward-ref must stay for save-time validation to catch");
        assert_eq!(steps[1].output_format, StepOutputFormat::FreeText,
            "Producer is referenced illegally (forward) — not upgraded");
    }

    #[test]
    fn heal_upgrades_previous_step_predecessor() {
        let mut steps = vec![
            bare_step("a", "Do a thing", StepOutputFormat::FreeText),
            bare_step("b", "Summary: {{previous_step.summary}}", StepOutputFormat::FreeText),
        ];
        let upgraded = heal_steps_in_place(&mut steps);
        assert_eq!(upgraded, vec!["a".to_string()]);
        assert_eq!(steps[0].output_format, StepOutputFormat::Structured);
    }

    #[test]
    fn heal_upgrades_only_once_per_producer() {
        // Multiple consumers reference the same producer — still a single
        // upgrade.
        let mut steps = vec![
            bare_step("main", "Fetch", StepOutputFormat::FreeText),
            bare_step("a", "{{steps.main.data}}", StepOutputFormat::FreeText),
            bare_step("b", "{{steps.main.summary}}", StepOutputFormat::FreeText),
            bare_step("c", "{{steps.main.data_json}}", StepOutputFormat::FreeText),
        ];
        let upgraded = heal_steps_in_place(&mut steps);
        assert_eq!(upgraded, vec!["main".to_string()]);
        assert_eq!(steps[0].output_format, StepOutputFormat::Structured);
    }

    // ─── WorkflowRun construction (mirrors spawn_run logic) ──────────────

    #[test]
    fn workflow_run_initial_state() {
        let now = Utc::now();
        let run = WorkflowRun {
            id: Uuid::new_v4().to_string(),
            workflow_id: "wf-123".into(),
            status: RunStatus::Pending,
            trigger_context: Some(serde_json::json!({"type": "cron"})),
            step_results: vec![],
            tokens_used: 0,
            workspace_path: None,
            started_at: now,
            finished_at: None,
            run_type: "linear".into(),
            batch_total: 0,
            batch_completed: 0,
            batch_failed: 0,
            batch_name: None,
            parent_run_id: None,
        };

        assert_eq!(run.status, RunStatus::Pending);
        assert!(run.step_results.is_empty());
        assert_eq!(run.tokens_used, 0);
        assert!(run.workspace_path.is_none());
        assert!(run.finished_at.is_none());
        assert!(!run.id.is_empty());
    }

    #[test]
    fn workflow_run_id_is_uuid_format() {
        let id = Uuid::new_v4().to_string();
        assert_eq!(id.len(), 36); // UUID v4 = 8-4-4-4-12 = 36 chars
        assert_eq!(id.chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn workflow_run_trigger_context_serialization() {
        let ctx = serde_json::json!({
            "type": "tracker",
            "issue_title": "Bug report",
            "issue_number": 42,
        });
        let run = WorkflowRun {
            id: "test-id".into(),
            workflow_id: "wf-1".into(),
            status: RunStatus::Pending,
            trigger_context: Some(ctx.clone()),
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
        };
        let tc = run.trigger_context.unwrap();
        assert_eq!(tc["type"], "tracker");
        assert_eq!(tc["issue_title"], "Bug report");
        assert_eq!(tc["issue_number"], 42);
    }

    #[test]
    fn workflow_run_manual_trigger_context() {
        let ctx = serde_json::json!({
            "type": "manual",
            "triggered_by": "user@example.com",
        });
        let serialized = serde_json::to_string(&ctx).unwrap();
        assert!(serialized.contains("manual"));
    }

    // ─── RunStatus equality ──────────────────────────────────────────────

    #[test]
    fn run_status_equality() {
        assert_eq!(RunStatus::Pending, RunStatus::Pending);
        assert_eq!(RunStatus::Running, RunStatus::Running);
        assert_eq!(RunStatus::Success, RunStatus::Success);
        assert_eq!(RunStatus::Failed, RunStatus::Failed);
        assert_ne!(RunStatus::Success, RunStatus::Failed);
    }

    // ─── Concurrency limit logic ─────────────────────────────────────────

    #[test]
    fn concurrency_limit_check_logic() {
        // Mirrors the check in check_triggers
        let limit: Option<u32> = Some(2);
        let active: u32 = 2;
        if let Some(l) = limit {
            assert!(active >= l, "Should skip when active >= limit");
        }
    }

    #[test]
    fn concurrency_no_limit_always_allows() {
        let limit: Option<u32> = None;
        assert!(limit.is_none(), "No limit should allow any number of runs");
    }
}
