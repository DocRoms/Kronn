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
//!   same branch, authored during the step, and that change nothing but the
//!   docs root, Kronn-owned pointers and another AI-doc folder → `git reset
//!   --soft` back to the recorded HEAD (the changes stay in the index and the
//!   worktree, nothing is lost). Any other move (a user commit, a commit that
//!   touches source, a rebase, a branch switch) is never touched, only
//!   reported;
//! - a tracked file inside ANOTHER AI-doc folder (see [`ai_doc_folders`]) that
//!   the step changed goes back to its pre-step content, after the current
//!   bytes were kept as a git blob. A tracked change anywhere else outside the
//!   root is only reported: Kronn cannot tell the agent's edit from the user's,
//!   who keeps working in the repository while an audit runs.
//!
//! Every action is reported, never silent, and none destroys content that
//! cannot be read back.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::core::docs_location::DocsRoot;

/// Files and folders outside the root that Kronn (or the agent CLI it
/// launches) owns: root pointers, per-agent instruction and MCP config.
pub(crate) const KRONN_OWNED: &[&str] = &[
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
    /// Files of another AI-doc folder put back to their pre-step content.
    pub reverted: Vec<Reverted>,
    /// What Kronn saw and left alone.
    pub warnings: Vec<String>,
}

/// A file restored by [`StepRemit::settle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reverted {
    pub path: String,
    /// Blob holding the content that was overwritten (`git cat-file -p`);
    /// `None` when the step had deleted the file.
    pub blob: Option<String>,
}

impl std::fmt::Display for Reverted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.blob {
            Some(sha) => write!(
                f,
                "{} (the version before the restore is blob {sha}: `git cat-file -p {sha}`)",
                self.path
            ),
            None => write!(f, "{} (deleted by the step)", self.path),
        }
    }
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
                "the agent edited {} tracked file(s) of another AI-doc folder outside the docs root; Kronn restored them: {}",
                self.reverted.len(),
                self.reverted
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
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
    /// Tracked files outside the root that were already modified and could
    /// not be read safely at that moment (a link): never restored.
    refused_before: HashSet<String>,
    /// Project-relative folders outside the root that hold AI docs.
    ai_folders: Vec<String>,
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
    let raw = git(
        project,
        &["status", "--porcelain=v1", "-z", "--untracked-files=no"],
    )?;
    let mut parts = raw
        .split(|b| *b == 0)
        .map(|p| String::from_utf8_lossy(p).into_owned());
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
    git_text(
        project,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/tags",
        ],
    )
    .map(|t| t.lines().map(str::to_string).collect())
    .unwrap_or_default()
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(9)]
}

/// Folders outside the docs root that hold AI documentation written by Kronn:
/// the folder of a `.kronn.json`, of a Kronn-signed `AGENTS.md` or of an
/// `inconsistencies-*.md`, and the parent of a `tech-debt/TD-*.md`. Read from
/// the tracked files before the step, so the agent cannot define them. The
/// project folder itself and Kronn-owned paths never count.
fn ai_doc_folders(project: &Path, docs_root: &DocsRoot) -> Vec<String> {
    let listed = git(project, &["ls-files", "-z"]).unwrap_or_default();
    let mut folders: Vec<String> = Vec::new();
    for rel in listed.split(|b| *b == 0).map(String::from_utf8_lossy) {
        if docs_root.contains(&rel) || kronn_owned(&rel) {
            continue;
        }
        let Some((dir, name)) = rel.rsplit_once('/') else {
            continue;
        };
        let folder = match name {
            ".kronn.json" => dir,
            "AGENTS.md" if crate::core::legacy_docs::is_kronn_managed_docs(&project.join(dir)) => {
                dir
            }
            n if n.starts_with("inconsistencies-") && n.ends_with(".md") => dir,
            n if n.starts_with("TD-") && n.ends_with(".md") => match dir.rsplit_once('/') {
                Some((parent, "tech-debt")) => parent,
                _ => continue,
            },
            _ => continue,
        };
        if !folder.is_empty() && !folders.iter().any(|f| f == folder) {
            folders.push(folder.to_string());
        }
    }
    folders
}

impl StepRemit {
    /// Record the state before the agent runs. `None` outside a git repository
    /// or before its first commit: there is no history to protect.
    pub fn capture_blocking(project: &Path, docs_root: &DocsRoot) -> Option<Self> {
        let head = git_text(project, &["rev-parse", "--verify", "-q", "HEAD"])?;
        let prefix = git_text(project, &["rev-parse", "--show-prefix"]).unwrap_or_default();
        let branch = git_text(project, &["symbolic-ref", "-q", "HEAD"]);
        let mut dirty_outside = HashMap::new();
        let mut refused_before = HashSet::new();
        for rel in tracked_changes(project, &prefix)? {
            if docs_root.contains(&rel) || kronn_owned(&rel) {
                continue;
            }
            // KT-1595: read through the rooted primitive. A link (or anything
            // that is not a readable regular file) is recorded as refused, not
            // as an absent file: settle never rewrites or deletes through it.
            match crate::core::fs_guard::guarded_read(project, &project.join(&rel)) {
                Ok(content) => {
                    dirty_outside.insert(rel, content);
                }
                Err(_) => {
                    refused_before.insert(rel);
                }
            }
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
            refused_before,
            ai_folders: ai_doc_folders(project, docs_root),
        })
    }

    fn in_ai_folder(&self, rel: &str) -> bool {
        self.ai_folders.iter().any(|f| {
            rel.strip_prefix(f.as_str())
                .is_some_and(|r| r.starts_with('/'))
        })
    }

    /// A path a step may legitimately touch, or whose restoration is Kronn's.
    fn within_remit(&self, rel: &str) -> bool {
        self.docs_root.contains(rel) || kronn_owned(rel) || self.in_ai_folder(rel)
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
                report
                    .warnings
                    .push("HEAD could not be read after the step".to_string());
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
        let descends = git(
            &self.project,
            &["merge-base", "--is-ancestor", &self.head, &now],
        )
        .is_some();
        // `%H %at %P`: only plain commits on a straight line are the step's.
        let log = git_text(
            &self.project,
            &["log", "--format=%H%x09%at%x09%P%x09%s", &range],
        );
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
        let foreign = if all_the_step_s {
            self.foreign_paths(&commits)
        } else {
            Vec::new()
        };
        if !foreign.is_empty() {
            report.warnings.push(format!(
                "HEAD moved during the step ({} → {}) by commits that touch files beyond the docs root and the AI-doc folders ({}); they may be yours, so Kronn left them as they are",
                short(&self.head),
                short(&now),
                foreign.join(", "),
            ));
            return false;
        }
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

    /// Paths, in the commits of the range, that the audit has no business
    /// changing (sorted, a handful at most).
    fn foreign_paths(&self, commits: &[Vec<String>]) -> Vec<String> {
        let mut foreign = std::collections::BTreeSet::new();
        for c in commits {
            let Some(raw) = git(
                &self.project,
                &[
                    "diff-tree",
                    "--no-commit-id",
                    "--name-only",
                    "-r",
                    "-z",
                    &c[0],
                ],
            ) else {
                foreign.insert(format!("(commit {} unreadable)", short(&c[0])));
                continue;
            };
            for path in raw.split(|b| *b == 0).filter(|p| !p.is_empty()) {
                let path = String::from_utf8_lossy(path);
                match path.strip_prefix(self.prefix.as_str()) {
                    Some(rel) if self.within_remit(rel) => {}
                    _ => {
                        foreign.insert(path.into_owned());
                    }
                }
            }
        }
        foreign.into_iter().take(8).collect()
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
        let mut left_alone = Vec::new();
        for rel in changed {
            if self.docs_root.contains(&rel) || kronn_owned(&rel) || !seen.insert(rel.clone()) {
                continue;
            }
            let before = self.dirty_outside.get(&rel);
            if !self.in_ai_folder(&rel) {
                // Source, config, anything else: the user works here too.
                // KT-1595: read through the rooted primitive; a refused read
                // (link) counts as changed, never as the content it points at.
                let unchanged = before.is_some_and(|b| {
                    crate::core::fs_guard::guarded_read(&self.project, &self.project.join(&rel))
                        .is_ok_and(|now| *b == now)
                });
                if !unchanged {
                    left_alone.push(rel);
                }
                continue;
            }
            if self.refused_before.contains(&rel) {
                report.warnings.push(format!(
                    "`{rel}` could not be read safely before the step (a link?) and was left as it is"
                ));
                continue;
            }
            let restored = match before {
                // A user change that was already there: back to that content.
                Some(before) => self.restore_content(&rel, before.as_deref()),
                // Clean before the step: back to the recorded commit.
                None => self.restore_from_head(&rel),
            };
            match restored {
                Ok(Some(blob)) => report.reverted.push(Reverted { path: rel, blob }),
                Ok(None) => {}
                Err(e) => report.warnings.push(format!(
                    "`{rel}` was changed outside the docs root and could not be restored: {e}"
                )),
            }
        }
        if !left_alone.is_empty() {
            report.warnings.push(format!(
                "{} tracked file(s) outside the docs root changed during the step and were left untouched (they may be your own edits; only another AI-doc folder is restored): {}",
                left_alone.len(),
                left_alone.join(", ")
            ));
        }
    }

    /// Keep the bytes about to be overwritten as a git blob. `None` when the
    /// file is absent (nothing to keep); an error means do NOT overwrite.
    ///
    /// KT-1595: the bytes are read through the rooted primitive (a link at the
    /// name or in a parent is refused, never followed) and handed to git on
    /// stdin, so git never reads the path itself.
    fn keep_current(&self, rel: &str) -> Result<Option<String>, String> {
        let path = self.project.join(rel);
        let bytes = match crate::core::fs_guard::guarded_read(&self.project, &path) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(None),
            // A folder or another non-regular entry: not content to keep.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => return Ok(None),
            Err(e) => {
                return Err(format!(
                    "the current content could not be read safely ({e}), so it was not overwritten"
                ))
            }
        };
        self.hash_object(&bytes).map(Some).ok_or_else(|| {
            "the current content could not be kept as a git blob, so it was not overwritten"
                .to_string()
        })
    }

    /// `git hash-object -w --stdin`: the blob id of `bytes`, written.
    fn hash_object(&self, bytes: &[u8]) -> Option<String> {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = crate::core::cmd::git_cmd()
            .arg("--literal-pathspecs")
            .arg("-C")
            .arg(&self.project)
            .args(["hash-object", "-w", "--no-filters", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdin = child.stdin.take()?;
        let payload = bytes.to_vec();
        // A writer thread: a large file must not deadlock against git's pipe.
        let writer = std::thread::spawn(move || stdin.write_all(&payload));
        let output = child.wait_with_output().ok()?;
        writer.join().ok()?.ok()?;
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (output.status.success() && !sha.is_empty()).then_some(sha)
    }

    /// `Some(blob)` when the file was restored, the blob being what it held.
    fn restore_from_head(&self, rel: &str) -> Result<Option<Option<String>>, String> {
        // `./`: relative to the project folder, which may sit below the top.
        let spec = format!("{}:./{rel}", self.head);
        if git(&self.project, &["cat-file", "-e", &spec]).is_none() {
            // Added by the step itself: not content the user had.
            return Ok(None);
        }
        // KT-1595: git never writes the path. The blob's bytes are read with
        // `cat-file` and written through the rooted primitive (a link at the
        // name or in a parent is replaced or refused, never written through),
        // as the pre-step content is in `restore_content`. A link or a
        // submodule at HEAD is not bytes to write: refused, left as it is.
        let (mode, index_mode, blob_id) = self.head_file_entry(rel)?;
        let bytes = git(&self.project, &["cat-file", "blob", &spec]).ok_or_else(|| {
            format!("the content of `{rel}` at the recorded commit is unreadable")
        })?;
        let blob = self.keep_current(rel)?;
        let path = self.project.join(rel);
        crate::core::fs_guard::guarded_replace(&self.project, &path, &bytes)?;
        #[cfg(unix)]
        crate::core::fs_guard::guarded_set_mode(&self.project, &path, mode)?;
        #[cfg(not(unix))]
        let _ = mode;
        // The working tree is back; so must the index be. After the
        // `reset --soft` of a committed step (and after a `git add` by the
        // agent) the index still holds the agent's version, and the user's
        // next commit would re-commit the reverted edit. `update-index
        // --cacheinfo` sets the entry from the recorded blob WITHOUT touching
        // the path (it takes a top-relative path, hence the prefix).
        let entry = format!("{index_mode},{blob_id},{}{rel}", self.prefix);
        if git(
            &self.project,
            &["update-index", "--add", "--cacheinfo", &entry],
        )
        .is_none()
        {
            return Err(format!(
                "the file was restored but its index entry could not be put back to the recorded commit (git update-index failed); run `git restore --staged -- {rel}`"
            ));
        }
        Ok(Some(blob))
    }

    /// What git records for `rel` at the recorded commit: the permission bits
    /// (`0o755` or `0o644`), the index mode string and the blob id; an error
    /// for a link, a submodule or anything else that is not a plain file.
    fn head_file_entry(&self, rel: &str) -> Result<(u32, &'static str, String), String> {
        let listing = git(&self.project, &["ls-tree", "-z", &self.head, "--", rel])
            .ok_or_else(|| format!("`{rel}` could not be looked up at the recorded commit"))?;
        let listing = String::from_utf8_lossy(&listing);
        // `<mode> SP <type> SP <blob> TAB <path>`
        let mut fields = listing.split(['\t', ' ']);
        let (mode, kind, id) = (fields.next(), fields.next(), fields.next());
        let valid_id = |id: &str| id.len() >= 40 && id.bytes().all(|b| b.is_ascii_hexdigit());
        match (mode, kind, id) {
            (Some("100644"), Some("blob"), Some(id)) if valid_id(id) => {
                Ok((0o644, "100644", id.to_string()))
            }
            (Some("100755"), Some("blob"), Some(id)) if valid_id(id) => {
                Ok((0o755, "100755", id.to_string()))
            }
            _ => Err(format!(
                "`{rel}` is not a plain file at the recorded commit (mode {}); left as it is",
                mode.unwrap_or("unknown")
            )),
        }
    }

    fn restore_content(
        &self,
        rel: &str,
        before: Option<&[u8]>,
    ) -> Result<Option<Option<String>>, String> {
        // KT-1595: every step goes through the rooted primitive under the
        // project. A link at the name or in a parent folder (the agent may
        // have swapped an AI-doc folder for a link) is refused: nothing is
        // read, written nor deleted through it.
        use crate::core::fs_guard::{
            guarded_read, guarded_remove_file, guarded_replace, guarded_write_new,
        };
        let path = self.project.join(rel);
        let now = guarded_read(&self.project, &path)
            .map_err(|e| format!("refused, left as it is ({e})"))?;
        if now.as_deref() == before {
            return Ok(None);
        }
        let blob = self.keep_current(rel)?;
        match (before, now.is_some()) {
            (Some(bytes), true) => guarded_replace(&self.project, &path, bytes)?,
            (Some(bytes), false) => {
                if !guarded_write_new(&self.project, &path, bytes)? {
                    return Err("an entry appeared at the name meanwhile; left as it is".into());
                }
            }
            (None, _) => match guarded_remove_file(&self.project, &path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            },
        }
        Ok(Some(blob))
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
            axum::response::sse::Event::default()
                .event("step_warning")
                .data(
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
- Another AI-doc folder outside `{root}/` (a leftover docs/, ai/, .ai/ or doc/ folder that is not the root) is read-only history: read it as a source, never edit, append to or reuse its files. Existing TD sheets there are not to be updated — create or update the ones under `{root}/tech-debt/` instead. Kronn reports any tracked file you change outside `{root}/` and restores the ones of that other folder.\n\
- NEVER run `git commit`, `git push`, `git tag`, `git branch`, `git checkout`/`git switch`, `git stash`, `git reset` or `git rebase`. Leave every change uncommitted: Kronn leaves them for the user's review, and the repository's own commit conventions (message format, ticket prefix, sign-off) do NOT apply to this audit. Kronn undoes commits made during a step.\n"
    )
}

#[cfg(test)]
#[path = "step_remit_tests.rs"]
mod tests;
