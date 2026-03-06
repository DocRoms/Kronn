//! Workflow engine: background service that manages workflow execution.
//!
//! Ticks every 30s, checks triggers, enforces concurrency limits,
//! and spawns runs.

pub mod template;
pub mod workspace;
pub mod steps;
pub mod runner;
pub mod trigger;
pub mod tracker;

use std::sync::Arc;
use chrono::Utc;
use tokio::time::{interval, Duration};
use uuid::Uuid;

use crate::db::Database;
use crate::models::*;

/// The workflow engine — runs in the background, checks triggers, spawns runs.
pub struct WorkflowEngine {
    db: Arc<Database>,
    config: Arc<tokio::sync::RwLock<AppConfig>>,
}

impl WorkflowEngine {
    pub fn new(db: Arc<Database>, config: Arc<tokio::sync::RwLock<AppConfig>>) -> Self {
        Self { db, config }
    }

    /// Start the engine tick loop (runs forever).
    pub async fn start(self: Arc<Self>) {
        let mut tick = interval(Duration::from_secs(30));
        tracing::info!("Workflow engine started");

        loop {
            tick.tick().await;
            if let Err(e) = self.check_triggers().await {
                tracing::error!("Workflow engine tick error: {}", e);
            }
        }
    }

    /// Check all enabled workflows and fire triggers.
    async fn check_triggers(&self) -> anyhow::Result<()> {
        let db = self.db.clone();
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
                let db2 = self.db.clone();
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
            let db = self.db.clone();
            let already = db.with_conn(move |conn| {
                crate::db::workflows::is_issue_processed(conn, &wf_id, &issue_id)
            }).await?;

            if already {
                continue;
            }

            // Mark as processed
            let wf_id = wf.id.clone();
            let issue_id = issue.id.clone();
            let db2 = self.db.clone();
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
        };

        // Persist the run
        let r = run.clone();
        let db = self.db.clone();
        db.with_conn(move |conn| crate::db::workflows::insert_run(conn, &r)).await?;

        tracing::info!("Spawning workflow run {} for '{}'", run.id, wf.name);

        // Read config for tokens and full_access
        let config = self.config.read().await;
        let tokens = config.tokens.clone();
        let full_access = config.agents.claude_code.full_access; // TODO: per-agent full_access

        let db2 = self.db.clone();
        let workflow = wf.clone();

        // Execute in background
        tokio::spawn(async move {
            if let Err(e) = runner::execute_run(db2, &workflow, &mut run, &tokens, full_access, None).await {
                tracing::error!("Workflow run {} failed: {}", run.id, e);
            }
        });

        Ok(())
    }
}
