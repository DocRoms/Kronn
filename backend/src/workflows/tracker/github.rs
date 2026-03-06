//! GitHub tracker adapter — uses GitHub API v3 (REST).
//!
//! Requires a GitHub token (from GITHUB_TOKEN env var or Kronn config).

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{TrackedIssue, TrackerSource};

pub struct GitHubTracker {
    owner: String,
    repo: String,
    token: String,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct GhIssue {
    number: u64,
    title: String,
    body: Option<String>,
    html_url: String,
    state: String,
    labels: Vec<GhLabel>,
    pull_request: Option<serde_json::Value>, // present if it's a PR
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhPr {
    html_url: String,
}

impl GitHubTracker {
    pub fn new(owner: String, repo: String, token: String) -> Self {
        Self {
            owner,
            repo,
            token,
            client: reqwest::Client::new(),
        }
    }

    fn api_url(&self, path: &str) -> String {
        format!("https://api.github.com/repos/{}/{}{}", self.owner, self.repo, path)
    }
}

#[async_trait::async_trait]
impl TrackerSource for GitHubTracker {
    async fn poll_new_items(&self, _query: &str, labels: &[String]) -> Result<Vec<TrackedIssue>> {
        let mut url = self.api_url("/issues?state=open&sort=created&direction=desc&per_page=30");

        if !labels.is_empty() {
            url.push_str(&format!("&labels={}", labels.join(",")));
        }

        let response = self.client.get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "Kronn/0.1")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .context("GitHub API request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error {}: {}", status, body);
        }

        let issues: Vec<GhIssue> = response.json().await
            .context("Failed to parse GitHub issues")?;

        let tracked: Vec<TrackedIssue> = issues.into_iter()
            // Filter out pull requests (GitHub API returns PRs in /issues)
            .filter(|i| i.pull_request.is_none())
            .map(|i| TrackedIssue {
                id: i.number.to_string(),
                number: i.number,
                title: i.title,
                body: i.body.unwrap_or_default(),
                url: i.html_url,
                labels: i.labels.into_iter().map(|l| l.name).collect(),
                state: i.state,
            })
            .collect();

        Ok(tracked)
    }

    async fn update_status(&self, issue_id: &str, status: &str) -> Result<()> {
        let url = self.api_url(&format!("/issues/{}", issue_id));
        let state = match status {
            "closed" | "done" | "resolved" => "closed",
            _ => "open",
        };

        let response = self.client.patch(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "Kronn/0.1")
            .header("Accept", "application/vnd.github+json")
            .json(&serde_json::json!({ "state": state }))
            .send()
            .await
            .context("GitHub API patch failed")?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub update status failed: {}", body);
        }

        Ok(())
    }

    async fn comment(&self, issue_id: &str, body: &str) -> Result<()> {
        let url = self.api_url(&format!("/issues/{}/comments", issue_id));

        let response = self.client.post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "Kronn/0.1")
            .header("Accept", "application/vnd.github+json")
            .json(&serde_json::json!({ "body": body }))
            .send()
            .await
            .context("GitHub API comment failed")?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub comment failed: {}", body);
        }

        Ok(())
    }

    async fn create_pr(&self, title: &str, body: &str, head: &str, base: &str) -> Result<String> {
        let url = self.api_url("/pulls");

        let response = self.client.post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "Kronn/0.1")
            .header("Accept", "application/vnd.github+json")
            .json(&serde_json::json!({
                "title": title,
                "body": body,
                "head": head,
                "base": base,
            }))
            .send()
            .await
            .context("GitHub API create PR failed")?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub create PR failed: {}", body);
        }

        let pr: GhPr = response.json().await?;
        Ok(pr.html_url)
    }
}
