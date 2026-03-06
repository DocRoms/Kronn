use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use chrono::Utc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};

use crate::models::{ScheduledTask, TaskRun};

/// Manages scheduled task execution
pub struct Scheduler {
    /// Active tasks indexed by task ID
    tasks: Arc<RwLock<HashMap<String, ScheduledTask>>>,
    /// Execution history
    runs: Arc<RwLock<Vec<TaskRun>>>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            runs: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register a task in the scheduler
    pub async fn register(&self, task: ScheduledTask) {
        let mut tasks = self.tasks.write().await;
        tracing::info!("Registered task: {} ({})", task.name, task.cron_expr);
        tasks.insert(task.id.clone(), task);
    }

    /// Remove a task from the scheduler
    pub async fn unregister(&self, task_id: &str) {
        let mut tasks = self.tasks.write().await;
        tasks.remove(task_id);
        tracing::info!("Unregistered task: {}", task_id);
    }

    /// Toggle a task active/inactive
    pub async fn set_active(&self, task_id: &str, active: bool) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(task_id) {
            task.active = active;
            tracing::info!("Task {} set to {}", task_id, if active { "active" } else { "inactive" });
        }
    }

    /// Get execution history for a task
    pub async fn get_runs(&self, task_id: &str) -> Vec<TaskRun> {
        let runs = self.runs.read().await;
        runs.iter()
            .filter(|r| r.task_id == task_id)
            .cloned()
            .collect()
    }

    /// Start the scheduler tick loop
    /// Checks every 30 seconds if any task should run
    pub async fn start(self: Arc<Self>) {
        let mut tick = interval(Duration::from_secs(30));

        tracing::info!("Scheduler started");

        loop {
            tick.tick().await;
            self.check_and_run().await;
        }
    }

    /// Check all active tasks and run any that are due
    async fn check_and_run(&self) {
        let tasks = self.tasks.read().await;
        let now = Utc::now();

        for task in tasks.values() {
            if !task.active {
                continue;
            }

            // Parse cron expression and check if it matches current time
            let schedule: cron::Schedule = match cron::Schedule::from_str(&format!("0 {}", task.cron_expr)) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Invalid cron for task {}: {}", task.id, e);
                    continue;
                }
            };

            // Check if any upcoming time is within our check window (30s)
            if let Some(next) = schedule.upcoming(Utc).next() {
                let diff = (next - now).num_seconds();
                if diff <= 30 && diff >= 0 {
                    tracing::info!("Task due: {} ({})", task.name, task.id);
                    // TODO: spawn task execution in background
                    // This would call the appropriate agent executor
                }
            }
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}
