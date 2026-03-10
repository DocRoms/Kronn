//! Tracker adapters for polling issue trackers.
//!
//! TrackerSource trait defines the interface. GitHub is the first implementation.

pub mod github;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// A tracked issue from an external tracker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedIssue {
    pub id: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    pub labels: Vec<String>,
    pub state: String,
}

/// Trait for issue tracker integrations.
#[allow(dead_code)]
#[async_trait::async_trait]
pub trait TrackerSource: Send + Sync {
    /// Poll for new issues matching the query/labels.
    async fn poll_new_items(&self, query: &str, labels: &[String]) -> Result<Vec<TrackedIssue>>;

    /// Update the status of an issue (e.g., close it).
    async fn update_status(&self, issue_id: &str, status: &str) -> Result<()>;

    /// Add a comment to an issue.
    async fn comment(&self, issue_id: &str, body: &str) -> Result<()>;

    /// Create a pull request.
    async fn create_pr(&self, title: &str, body: &str, head: &str, base: &str) -> Result<String>;
}
