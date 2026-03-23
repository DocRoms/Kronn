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

#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tracker() -> GitHubTracker {
        GitHubTracker::new("my-owner".into(), "my-repo".into(), "fake-token".into())
    }

    // ─── api_url ─────────────────────────────────────────────────────────

    #[test]
    fn api_url_issues() {
        let t = make_tracker();
        assert_eq!(
            t.api_url("/issues"),
            "https://api.github.com/repos/my-owner/my-repo/issues"
        );
    }

    #[test]
    fn api_url_pulls() {
        let t = make_tracker();
        assert_eq!(
            t.api_url("/pulls"),
            "https://api.github.com/repos/my-owner/my-repo/pulls"
        );
    }

    #[test]
    fn api_url_with_query_params() {
        let t = make_tracker();
        assert_eq!(
            t.api_url("/issues?state=open&per_page=30"),
            "https://api.github.com/repos/my-owner/my-repo/issues?state=open&per_page=30"
        );
    }

    #[test]
    fn api_url_issue_by_number() {
        let t = make_tracker();
        let url = t.api_url(&format!("/issues/{}", 42));
        assert_eq!(url, "https://api.github.com/repos/my-owner/my-repo/issues/42");
    }

    #[test]
    fn api_url_issue_comments() {
        let t = make_tracker();
        let url = t.api_url(&format!("/issues/{}/comments", 7));
        assert_eq!(url, "https://api.github.com/repos/my-owner/my-repo/issues/7/comments");
    }

    // ─── GhIssue deserialization ─────────────────────────────────────────

    #[test]
    fn gh_issue_deserialize_basic() {
        let json = serde_json::json!({
            "number": 42,
            "title": "Fix crash",
            "body": "Server crashes on startup",
            "html_url": "https://github.com/owner/repo/issues/42",
            "state": "open",
            "labels": [{"name": "bug"}, {"name": "urgent"}],
            "pull_request": null,
        });
        let issue: GhIssue = serde_json::from_value(json).unwrap();
        assert_eq!(issue.number, 42);
        assert_eq!(issue.title, "Fix crash");
        assert_eq!(issue.body, Some("Server crashes on startup".into()));
        assert_eq!(issue.state, "open");
        assert_eq!(issue.labels.len(), 2);
        assert_eq!(issue.labels[0].name, "bug");
        assert!(issue.pull_request.is_none());
    }

    #[test]
    fn gh_issue_deserialize_null_body() {
        let json = serde_json::json!({
            "number": 1,
            "title": "No body",
            "body": null,
            "html_url": "https://github.com/o/r/issues/1",
            "state": "open",
            "labels": [],
        });
        let issue: GhIssue = serde_json::from_value(json).unwrap();
        assert!(issue.body.is_none());
    }

    #[test]
    fn gh_issue_with_pull_request_field() {
        let json = serde_json::json!({
            "number": 10,
            "title": "PR title",
            "body": null,
            "html_url": "https://github.com/o/r/pull/10",
            "state": "open",
            "labels": [],
            "pull_request": {"url": "https://api.github.com/repos/o/r/pulls/10"},
        });
        let issue: GhIssue = serde_json::from_value(json).unwrap();
        assert!(issue.pull_request.is_some());
    }

    // ─── PR filtering logic ─────────────────────────────────────────────

    #[test]
    fn filter_out_pull_requests() {
        let issues = vec![
            GhIssue {
                number: 1,
                title: "Real issue".into(),
                body: None,
                html_url: "https://gh/1".into(),
                state: "open".into(),
                labels: vec![],
                pull_request: None,
            },
            GhIssue {
                number: 2,
                title: "This is a PR".into(),
                body: None,
                html_url: "https://gh/2".into(),
                state: "open".into(),
                labels: vec![],
                pull_request: Some(serde_json::json!({"url": "..."})),
            },
            GhIssue {
                number: 3,
                title: "Another issue".into(),
                body: Some("With body".into()),
                html_url: "https://gh/3".into(),
                state: "open".into(),
                labels: vec![GhLabel { name: "bug".into() }],
                pull_request: None,
            },
        ];

        let tracked: Vec<TrackedIssue> = issues.into_iter()
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

        assert_eq!(tracked.len(), 2);
        assert_eq!(tracked[0].number, 1);
        assert_eq!(tracked[0].body, ""); // None becomes empty
        assert_eq!(tracked[1].number, 3);
        assert_eq!(tracked[1].body, "With body");
        assert_eq!(tracked[1].labels, vec!["bug"]);
    }

    // ─── TrackedIssue construction from GhIssue ──────────────────────────

    #[test]
    fn tracked_issue_id_is_number_as_string() {
        let gi = GhIssue {
            number: 999,
            title: "Test".into(),
            body: None,
            html_url: "https://gh/999".into(),
            state: "open".into(),
            labels: vec![],
            pull_request: None,
        };
        let ti = TrackedIssue {
            id: gi.number.to_string(),
            number: gi.number,
            title: gi.title,
            body: gi.body.unwrap_or_default(),
            url: gi.html_url,
            labels: gi.labels.into_iter().map(|l| l.name).collect(),
            state: gi.state,
        };
        assert_eq!(ti.id, "999");
        assert_eq!(ti.number, 999);
    }

    // ─── GhLabel deserialization ─────────────────────────────────────────

    #[test]
    fn gh_label_deserialize() {
        let json = serde_json::json!({"name": "enhancement"});
        let label: GhLabel = serde_json::from_value(json).unwrap();
        assert_eq!(label.name, "enhancement");
    }

    // ─── Status mapping ──────────────────────────────────────────────────

    #[test]
    fn status_mapping_closed_variants() {
        for status in &["closed", "done", "resolved"] {
            let state = match *status {
                "closed" | "done" | "resolved" => "closed",
                _ => "open",
            };
            assert_eq!(state, "closed", "Status '{}' should map to 'closed'", status);
        }
    }

    #[test]
    fn status_mapping_open_variants() {
        for status in &["open", "in_progress", "pending", "anything_else"] {
            let state = match *status {
                "closed" | "done" | "resolved" => "closed",
                _ => "open",
            };
            assert_eq!(state, "open", "Status '{}' should map to 'open'", status);
        }
    }
}
