//! KT-1360 — an audit step stays inside its remit.
//!
//! An audit writes documentation under the project's docs root and nothing
//! else. Two things escaped that in practice, and the prompt alone cannot be
//! trusted to stop either:
//!
//! - an agent following the repository's own commit conventions committed its
//!   work: the audit left commits in the user's history (a push would be one
//!   command away). Kronn leaves the changes uncommitted for the user's review;
//! - with the docs root moved (`.ai`), an agent followed the TD sheets of the
//!   old `docs/` folder and edited them, writing nothing under the new root.
//!
//! [`StepRemit::capture`] records, before the agent runs, where HEAD and the
//! branch stand and which tracked files outside the root were already dirty.
//! [`StepRemit::settle`] runs when the attempt is over:
//!
//! - HEAD moved by commits that all sit on top of the recorded HEAD, on the
//!   same branch, authored during the step → `git reset --soft` back to the
//!   recorded HEAD (the changes stay in the index and the worktree, nothing is
//!   lost). Any other move (a user commit, a rebase, a branch switch) is never
//!   touched, only reported;
//! - a tracked file outside the root that the step changed goes back to its
//!   pre-step content. Untracked files are not tracked content and stay.
//!
//! Every action is reported, never silent.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::core::docs_location::DocsRoot;

/// Files and folders outside the root that Kronn (or the agent CLI it
/// launches) owns: root pointers, per-agent instruction and MCP config.
const KRONN_OWNED: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    ".cursorrules",
    ".cursor",
    ".windsurfrules",
    ".clinerules",
    ".github/copilot-instructions.md",
    ".github/instructions",
    ".kiro",
    ".vibe",
    ".claude",
    ".codex",
    ".gemini",
    ".opencode",
    ".mcp.json",
    "opencode.json",
    "opencode.jsonc",
    ".kronn",
];

fn kronn_owned(rel: &str) -> bool {
    KRONN_OWNED
        .iter()
        .any(|owned| rel == *owned || rel.strip_prefix(owned).is_some_and(|r| r.starts_with('/')))
}

/// What [`StepRemit::settle`] did and saw.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RemitReport {
    /// Commits the step made, undone by a soft reset (short sha + subject).
    pub commits_undone: Vec<String>,
    /// Tracked files outside the docs root put back to their pre-step content.
    pub reverted: Vec<String>,
    /// What Kronn saw and left alone.
    pub warnings: Vec<String>,
}

impl RemitReport {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.commits_undone.is_empty() && self.reverted.is_empty() && self.warnings.is_empty()
    }

    /// One line per finding, ready for a step warning.
    pub fn messages(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.commits_undone.is_empty() {
            out.push(format!(
                "the agent committed during the step; Kronn undid {} commit(s) with a soft reset — the changes are kept uncommitted for your review: {}",
                self.commits_undone.len(),
                self.commits_undone.join("; ")
            ));
        }
        if !self.reverted.is_empty() {
            out.push(format!(
                "the agent edited {} tracked file(s) outside the docs root; Kronn restored them: {}",
                self.reverted.len(),
                self.reverted.join(", ")
            ));
        }
        out.extend(self.warnings.iter().cloned());
        out
    }
}

/// The state of the repository when the step began.
pub(crate) struct StepRemit {
    project: PathBuf,
    /// Project folder inside the repository (`sub/` or empty).
    prefix: String,
    docs_root: DocsRoot,
    head: String,
    branch: Option<String>,
    refs: HashSet<String>,
    /// Unix seconds; a commit authored before it is not the step's.
    started: i64,
    /// Tracked files outside the root that were already modified or staged:
    /// their content at that moment (`None` = absent).
    dirty_outside: HashMap<String, Option<Vec<u8>>>,
}

fn git(project: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = crate::core::cmd::git_cmd()
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(project)
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn git_text(project: &Path, args: &[&str]) -> Option<String> {
    git(project, args).map(|o| String::from_utf8_lossy(&o).trim().to_string())
}

/// Tracked paths with a pending change, project-relative: `(path, ...)`
/// from `git status -z`. Untracked and ignored entries are not listed.
fn tracked_changes(project: &Path, prefix: &str) -> Option<Vec<String>> {
    let raw = git(project, &["status", "--porcelain=v1", "-z", "--untracked-files=no"])?;
    let mut parts = raw.split(|b| *b == 0).map(|p| String::from_utf8_lossy(p).into_owned());
    let mut out = Vec::new();
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let (status, path) = entry.split_at(3);
        let mut paths = vec![path.to_string()];
        // A rename or copy carries its origin as one more NUL field.
        if status.contains(['R', 'C']) {
            paths.extend(parts.next());
        }
        for p in paths {
            if let Some(rel) = p.strip_prefix(prefix) {
                out.push(rel.to_string());
            }
        }
    }
    Some(out)
}

fn local_refs(project: &Path) -> HashSet<String> {
    git_text(project, &["for-each-ref", "--format=%(refname)", "refs/heads", "refs/tags"])
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(9)]
}

impl StepRemit {
    /// Record the state before the agent runs. `None` outside a git repository
    /// or before its first commit: there is no history to protect.
    pub fn capture_blocking(project: &Path, docs_root: &DocsRoot) -> Option<Self> {
        let head = git_text(project, &["rev-parse", "--verify", "-q", "HEAD"])?;
        let prefix = git_text(project, &["rev-parse", "--show-prefix"]).unwrap_or_default();
        let branch = git_text(project, &["symbolic-ref", "-q", "HEAD"]);
        let mut dirty_outside = HashMap::new();
        for rel in tracked_changes(project, &prefix)? {
            if docs_root.contains(&rel) || kronn_owned(&rel) {
                continue;
            }
            let content = std::fs::read(project.join(&rel)).ok();
            dirty_outside.insert(rel, content);
        }
        Some(Self {
            project: project.to_path_buf(),
            prefix,
            docs_root: docs_root.clone(),
            head,
            branch,
            refs: local_refs(project),
            started: chrono::Utc::now().timestamp(),
            dirty_outside,
        })
    }

    pub async fn capture(project: &Path, docs_root: &DocsRoot) -> Option<Arc<Self>> {
        let (project, docs_root) = (project.to_path_buf(), docs_root.clone());
        tokio::task::spawn_blocking(move || Self::capture_blocking(&project, &docs_root))
            .await
            .ok()
            .flatten()
            .map(Arc::new)
    }

    pub async fn settle(self: &Arc<Self>) -> RemitReport {
        let this = Arc::clone(self);
        tokio::task::spawn_blocking(move || this.settle_blocking())
            .await
            .unwrap_or_default()
    }

    /// Put the repository back inside the step's remit. Idempotent: run after
    /// every attempt, a second call finds nothing left to do.
    pub fn settle_blocking(&self) -> RemitReport {
        let mut report = RemitReport::default();
        let head_settled = self.settle_head(&mut report);
        self.report_new_refs(&mut report);
        if head_settled {
            self.revert_outside_root(&mut report);
        } else {
            report.warnings.push(
                "HEAD moved during the step and was left as it is: the tracked files outside the docs root were not checked"
                    .to_string(),
            );
        }
        report
    }

    /// Whether HEAD is back at (or never left) the recorded commit.
    fn settle_head(&self, report: &mut RemitReport) -> bool {
        let now = match git_text(&self.project, &["rev-parse", "--verify", "-q", "HEAD"]) {
            Some(now) => now,
            None => {
                report.warnings.push("HEAD could not be read after the step".to_string());
                return false;
            }
        };
        if now == self.head {
            return true;
        }
        let branch = git_text(&self.project, &["symbolic-ref", "-q", "HEAD"]);
        if branch != self.branch {
            report.warnings.push(format!(
                "the checked-out branch changed during the step ({} → {}); Kronn left it as it is",
                self.branch.as_deref().unwrap_or("detached"),
                branch.as_deref().unwrap_or("detached"),
            ));
            return false;
        }
        let range = format!("{}..{now}", self.head);
        let descends = git(&self.project, &["merge-base", "--is-ancestor", &self.head, &now]).is_some();
        // `%H %at %P`: only plain commits on a straight line are the step's.
        let log = git_text(&self.project, &["log", "--format=%H%x09%at%x09%P%x09%s", &range]);
        let commits: Vec<Vec<String>> = log
            .as_deref()
            .unwrap_or_default()
            .lines()
            .map(|l| l.splitn(4, '\t').map(str::to_string).collect())
            .collect();
        let all_the_step_s = descends
            && !commits.is_empty()
            && commits.iter().all(|c| {
                c.len() == 4
                    && c[1].parse::<i64>().is_ok_and(|at| at >= self.started)
                    && c[2].split_whitespace().count() == 1
            });
        if !all_the_step_s {
            report.warnings.push(format!(
                "HEAD moved during the step ({} → {}) by commits that are not all the step's own; Kronn left them as they are",
                short(&self.head),
                short(&now),
            ));
            return false;
        }
        if git(&self.project, &["reset", "--soft", &self.head]).is_none() {
            report.warnings.push(format!(
                "the agent committed during the step and Kronn could not undo it (git reset --soft {} failed)",
                short(&self.head)
            ));
            return false;
        }
        report.commits_undone.extend(
            commits
                .iter()
                .rev()
                .map(|c| format!("{} {}", short(&c[0]), c[3])),
        );
        true
    }

    fn report_new_refs(&self, report: &mut RemitReport) {
        let mut created: Vec<String> = local_refs(&self.project)
            .difference(&self.refs)
            .cloned()
            .collect();
        created.sort();
        if !created.is_empty() {
            report.warnings.push(format!(
                "the agent created {} during the step; an audit never creates branches or tags — Kronn left them for you to delete",
                created.join(", ")
            ));
        }
    }

    fn revert_outside_root(&self, report: &mut RemitReport) {
        let Some(changed) = tracked_changes(&self.project, &self.prefix) else {
            return;
        };
        let mut seen = HashSet::new();
        for rel in changed {
            if self.docs_root.contains(&rel) || kronn_owned(&rel) || !seen.insert(rel.clone()) {
                continue;
            }
            let restored = match self.dirty_outside.get(&rel) {
                // A user change that was already there: back to that content.
                Some(before) => self.restore_content(&rel, before.as_deref()),
                // Clean before the step: back to the recorded commit.
                None => self.restore_from_head(&rel),
            };
            match restored {
                Ok(true) => report.reverted.push(rel),
                Ok(false) => {}
                Err(e) => report.warnings.push(format!("`{rel}` was changed outside the docs root and could not be restored: {e}")),
            }
        }
    }

    fn restore_from_head(&self, rel: &str) -> Result<bool, String> {
        // `./`: relative to the project folder, which may sit below the top.
        let spec = format!("{}:./{rel}", self.head);
        if git(&self.project, &["cat-file", "-e", &spec]).is_none() {
            // Added by the step itself: not content the user had.
            return Ok(false);
        }
        git(&self.project, &["checkout", &self.head, "--", rel])
            .map(|_| true)
            .ok_or_else(|| "git checkout failed".to_string())
    }

    fn restore_content(&self, rel: &str, before: Option<&[u8]>) -> Result<bool, String> {
        let path = self.project.join(rel);
        let now = std::fs::read(&path).ok();
        if now.as_deref() == before {
            return Ok(false);
        }
        match before {
            Some(bytes) => std::fs::write(&path, bytes).map_err(|e| e.to_string())?,
            None => std::fs::remove_file(&path).map_err(|e| e.to_string())?,
        }
        Ok(true)
    }
}

/// One `step_warning` event per finding, for a pipeline to relay.
pub(crate) fn warning_events(
    step: impl serde::Serialize + Copy,
    file: &str,
    report: &RemitReport,
) -> Vec<axum::response::sse::Event> {
    report
        .messages()
        .into_iter()
        .map(|reason| {
            tracing::warn!("Audit step {}: {reason}", file);
            axum::response::sse::Event::default().event("step_warning").data(
                serde_json::json!({
                    "step": step, "file": file, "reason": reason,
                    "repaired_from_template": false,
                })
                .to_string(),
            )
        })
        .collect()
}

/// The remit told to the agent (KT-1360). Appended to every step prompt.
pub(crate) fn remit_prompt_block(docs_root: &DocsRoot) -> String {
    let root = docs_root.rel();
    format!(
        "## Remit of this audit (strict)\n\
- Write documentation ONLY under `{root}/`. Do not edit source code, configuration or any other tracked file.\n\
- Another AI-doc folder outside `{root}/` (a leftover docs/, ai/, .ai/ or doc/ folder that is not the root) is read-only history: read it as a source, never edit, append to or reuse its files. Existing TD sheets there are not to be updated — create or update the ones under `{root}/tech-debt/` instead. Kronn reverts and reports any tracked file you change outside `{root}/`.\n\
- NEVER run `git commit`, `git push`, `git tag`, `git branch`, `git checkout`/`git switch`, `git stash`, `git reset` or `git rebase`. Leave every change uncommitted: Kronn leaves them for the user's review, and the repository's own commit conventions (message format, ticket prefix, sign-off) do NOT apply to this audit. Kronn undoes commits made during a step.\n"
    )
}

#[cfg(test)]
#[path = "step_remit_tests.rs"]
mod tests;
