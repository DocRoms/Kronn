//! A workflow's Security settings (`WorkflowSafety`), enforced by the runner so
//! the assistant's panel promises only what a run actually does.

use std::path::Path;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::{RunStatus, StepResult, WorkflowRun, WorkflowSafety};

/// Name of the result row that holds a run's pre-start approval.
pub const APPROVAL_STEP: &str = "__safety_approval__";
/// Name of the result row recording a refusal of the Security settings.
pub const REFUSAL_STEP: &str = "__safety__";
/// `run.state` key holding the change baseline, so a resumed run keeps it.
pub const BASELINE_STATE_KEY: &str = "__kronn.safety_baseline";

pub fn limits_set(safety: &WorkflowSafety) -> bool {
    safety.max_files.is_some() || safety.max_lines.is_some()
}

pub fn in_container() -> bool {
    #[cfg(test)]
    if let Some(forced) = TEST_IN_CONTAINER.with(std::cell::Cell::get) {
        return forced;
    }
    crate::core::env::is_docker()
}

#[cfg(test)]
thread_local! {
    /// Lets a current-thread test pick the host the sandbox check sees.
    pub static TEST_IN_CONTAINER: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub fn sandbox_refusal(safety: &WorkflowSafety, in_container: bool) -> Option<String> {
    (safety.sandbox && !in_container).then(|| {
        "This workflow requires the Docker sandbox (Security settings), but Kronn is not \
         running in a container. Run it from Kronn's Docker image, or untick \"Sandbox\"."
            .to_string()
    })
}

/// A sub-workflow child cannot pause: its parent waits on it in-process.
pub fn child_approval_refusal(safety: &WorkflowSafety, run: &WorkflowRun) -> Option<String> {
    (safety.require_approval && run.parent_run_id.is_some()).then(|| {
        "This workflow requires a human approval (Security settings), which a sub-workflow \
         cannot wait for. Untick \"Approval required\" or launch it on its own."
            .to_string()
    })
}

/// Only a run that has not started: a resumed one is never sent back to step 0.
pub fn approval_pending(safety: &WorkflowSafety, run: &WorkflowRun) -> bool {
    safety.require_approval && run.step_results.is_empty() && run.workspace_path.is_none()
}

pub fn approval_result() -> StepResult {
    let mut result = refusal_result(
        "This workflow requires a human approval before it runs (Security settings).",
    );
    result.step_name = APPROVAL_STEP.to_string();
    result.status = RunStatus::WaitingApproval;
    result.step_kind = Some("Gate".into());
    result.started_at = Some(chrono::Utc::now());
    result
}

pub fn refusal_result(message: &str) -> StepResult {
    StepResult {
        step_name: REFUSAL_STEP.to_string(),
        status: RunStatus::Failed,
        output: message.to_string(),
        tokens_used: Some(0),
        duration_ms: 0,
        started_at: None,
        condition_result: None,
        envelope_detected: None,
        step_kind: Some("Preflight".into()),
        step_api_plugin_slug: None,
        step_api_endpoint_path: None,
        is_rollback: false,
        child_run_id: None,
        agent_provenance: None,
        native_tool_calls: Box::default(),
        step_agent: None,
        step_model: None,
        cached_prompt_tokens: None,
        cache_write_prompt_tokens: None,
        last_activity: None,
        quota_wait: None,
        terminal_stop: None,
    }
}

/// The reason a run (or a child of it) ended for good; it ends the whole tree.
pub fn run_terminal_stop(run: &WorkflowRun) -> Option<String> {
    run.step_results
        .iter()
        .find_map(|result| result.terminal_stop.clone())
}

/// A stored setting this host would refuse at run time, shown before any run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SafetyWarning {
    SandboxOutsideContainer,
    LimitsWithoutDirectory,
    LimitsWithoutGit,
    ApprovalOnSubWorkflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryState {
    /// The run's project is chosen at launch.
    PerRun,
    Missing,
    NotGit,
    Git,
}

pub fn warnings(
    safety: &WorkflowSafety,
    in_container: bool,
    directory: DirectoryState,
    used_as_sub_workflow: bool,
) -> Vec<SafetyWarning> {
    let mut found = Vec::new();
    if sandbox_refusal(safety, in_container).is_some() {
        found.push(SafetyWarning::SandboxOutsideContainer);
    }
    if limits_set(safety) {
        match directory {
            DirectoryState::Missing => found.push(SafetyWarning::LimitsWithoutDirectory),
            DirectoryState::NotGit => found.push(SafetyWarning::LimitsWithoutGit),
            DirectoryState::PerRun | DirectoryState::Git => {}
        }
    }
    if safety.require_approval && used_as_sub_workflow {
        found.push(SafetyWarning::ApprovalOnSubWorkflow);
    }
    found
}

pub fn directory_state(path: Option<&Path>) -> DirectoryState {
    match path {
        Some(path) if path.is_dir() => match git(path, &["rev-parse", "--is-inside-work-tree"]) {
            Ok(inside) if inside.trim() == "true" => DirectoryState::Git,
            _ => DirectoryState::NotGit,
        },
        _ => DirectoryState::Missing,
    }
}

/// The working tree when the run started, kept as a git tree object so later
/// states are compared by content, never by line counts against HEAD.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeBaseline {
    pub tree: String,
    /// Flagged entries already absent from disk at the start (sparse paths):
    /// only these stay as they are; any other flagged file gone since is a deletion.
    #[serde(default)]
    pub absent_hidden: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeStats {
    pub files: u64,
    pub lines: u64,
}

pub fn capture_baseline(work_dir: &Path) -> Result<ChangeBaseline> {
    let inside = git(work_dir, &["rev-parse", "--is-inside-work-tree"])?;
    if inside.trim() != "true" {
        return Err(anyhow!("{} is not a git working tree", work_dir.display()));
    }
    let (tree, absent_hidden) = snapshot_tree(work_dir, None)?;
    Ok(ChangeBaseline {
        tree,
        absent_hidden,
    })
}

/// The run's changes: every path whose content differs from the baseline.
pub fn measure(work_dir: &Path, baseline: &ChangeBaseline) -> Result<ChangeStats> {
    let (now, _) = snapshot_tree(work_dir, Some(&baseline.absent_hidden))?;
    let numstat = git(
        work_dir,
        &[
            "diff-tree",
            "-r",
            "--numstat",
            "-z",
            "--no-renames",
            &baseline.tree,
            &now,
        ],
    )?;
    let mut stats = ChangeStats { files: 0, lines: 0 };
    for record in numstat.split('\0').filter(|record| !record.is_empty()) {
        let mut fields = record.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        // Kronn's own files are not the run's.
        if path.starts_with(".kronn/") {
            continue;
        }
        stats.files += 1;
        // Binary files report "-": they count as a file, not as lines.
        stats.lines += added.parse::<u64>().unwrap_or(0) + deleted.parse::<u64>().unwrap_or(0);
    }
    Ok(stats)
}

pub fn limit_breach(safety: &WorkflowSafety, stats: ChangeStats) -> Option<String> {
    let mut over = Vec::new();
    if let Some(max) = safety.max_files.filter(|max| stats.files > u64::from(*max)) {
        over.push(format!("{} files changed (max {max})", stats.files));
    }
    if let Some(max) = safety.max_lines.filter(|max| stats.lines > u64::from(*max)) {
        over.push(format!("{} lines changed (max {max})", stats.lines));
    }
    (!over.is_empty()).then(|| {
        format!(
            "Security limit exceeded: {}. The run stops here; the changes are kept for review.",
            over.join(", ")
        )
    })
}

/// Writes the working tree (tracked and untracked, ignored files excluded) as
/// a tree object through a private index: the user's index and history stay untouched.
/// `sparse`: the flagged paths absent at the baseline; `None` while capturing it.
/// Returns the tree and the flagged paths absent from disk that it kept.
fn snapshot_tree(work_dir: &Path, sparse: Option<&[String]>) -> Result<(String, Vec<String>)> {
    let real_index = git(work_dir, &["rev-parse", "--git-path", "index"])?;
    let real_index = work_dir.join(real_index.trim());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let private =
        real_index.with_file_name(format!("kronn-safety-index-{}-{nanos}", std::process::id()));
    // A copy only reuses the file stat cache; `add -A` rewrites every entry.
    if real_index.exists() {
        std::fs::copy(&real_index, &private)?;
    }
    let written = clear_hidden_entries(work_dir, &private, sparse).and_then(|absent| {
        git_with_index(work_dir, &private, &["add", "-A"])?;
        let tree = git_with_index(work_dir, &private, &["write-tree"])?;
        Ok((tree.trim().to_string(), absent))
    });
    let _ = std::fs::remove_file(&private);
    written
}

/// Copied entries flagged assume-unchanged or skip-worktree are never re-read
/// by `add -A`; on the private index only, files present on disk lose the flag.
fn clear_hidden_entries(
    work_dir: &Path,
    index: &Path,
    sparse: Option<&[String]>,
) -> Result<Vec<String>> {
    let (present, absent) = hidden_entries(work_dir, index)?;
    if !present.is_empty() {
        let paths = present.join("\0") + "\0";
        // One flag per pass: with `--stdin`, git applies only one of the two.
        for flag in ["--no-assume-unchanged", "--no-skip-worktree"] {
            let mut cmd = private_index_git(work_dir, index);
            cmd.args(["update-index", flag, "-z", "--stdin"]);
            run_git_with_input(cmd, paths.as_bytes())?;
        }
    }
    // A flagged file there at the baseline and gone now was deleted: drop it so
    // the tree shows the deletion (`add -A` keeps flagged entries).
    let deleted: Vec<&String> = match sparse {
        Some(sparse) => absent
            .iter()
            .filter(|path| !sparse.contains(path))
            .collect(),
        None => Vec::new(),
    };
    if !deleted.is_empty() {
        let paths = deleted
            .iter()
            .map(|path| path.as_str())
            .collect::<Vec<_>>()
            .join("\0")
            + "\0";
        let mut cmd = private_index_git(work_dir, index);
        cmd.args(["update-index", "--force-remove", "-z", "--stdin"]);
        run_git_with_input(cmd, paths.as_bytes())?;
    }
    let (left, kept) = hidden_entries(work_dir, index)?;
    let unexpected = kept
        .iter()
        .filter(|path| sparse.is_some_and(|sparse| !sparse.contains(path)))
        .count();
    if !left.is_empty() || unexpected > 0 {
        return Err(anyhow!(
            "{} file(s) are hidden from git (assume-unchanged or skip-worktree) and cannot be measured",
            left.len() + unexpected
        ));
    }
    Ok(kept)
}

/// Index entries git would not re-read, split into (present on disk, absent).
fn hidden_entries(work_dir: &Path, index: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let listed = git_with_index(work_dir, index, &["ls-files", "-v", "-z"])?;
    Ok(listed
        .split('\0')
        .filter_map(|record| record.split_once(' '))
        .filter(|(tag, _)| {
            tag.chars()
                .next()
                .is_some_and(|tag| tag.is_ascii_lowercase() || tag == 'S')
        })
        .map(|(_, path)| path.to_string())
        .partition(|path| std::fs::symlink_metadata(work_dir.join(path)).is_ok()))
}

/// Settings that let git trust stale stat data are off for a measurement.
fn private_index_git(work_dir: &Path, index: &Path) -> std::process::Command {
    let mut cmd = super::gate_checkpoint::git_cmd(work_dir);
    cmd.env("GIT_INDEX_FILE", index).args([
        "-c",
        "core.ignoreStat=false",
        "-c",
        "core.fsmonitor=false",
    ]);
    cmd
}

fn git_with_index(work_dir: &Path, index: &Path, args: &[&str]) -> Result<String> {
    run_git(private_index_git(work_dir, index), args)
}

fn run_git_with_input(mut cmd: std::process::Command, input: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| anyhow!("git update-index: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input)?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git update-index failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn git(work_dir: &Path, args: &[&str]) -> Result<String> {
    run_git(super::gate_checkpoint::git_cmd(work_dir), args)
}

fn run_git(mut cmd: std::process::Command, args: &[&str]) -> Result<String> {
    let output = cmd
        .args(args)
        .output()
        .map_err(|error| anyhow!("git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safety(max_files: Option<u32>, max_lines: Option<u32>) -> WorkflowSafety {
        WorkflowSafety {
            sandbox: false,
            max_files,
            max_lines,
            require_approval: false,
        }
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let status = super::super::gate_checkpoint::git_cmd(dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        git_in(dir.path(), &["init", "-q", "-b", "main"]);
        git_in(dir.path(), &["config", "user.email", "test@kronn.local"]);
        git_in(dir.path(), &["config", "user.name", "test"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        git_in(dir.path(), &["add", "."]);
        git_in(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn the_sandbox_refuses_only_outside_a_container() {
        let mut on = safety(None, None);
        on.sandbox = true;
        assert!(sandbox_refusal(&on, false).is_some());
        assert!(sandbox_refusal(&on, true).is_none());
        assert!(sandbox_refusal(&safety(None, None), false).is_none());
    }

    #[test]
    fn the_run_counts_its_own_changes_not_those_already_in_the_tree() {
        let dir = repo();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nmine\n").unwrap();
        let baseline = capture_baseline(dir.path()).unwrap();
        assert_eq!(
            measure(dir.path(), &baseline).unwrap(),
            ChangeStats { files: 0, lines: 0 }
        );

        std::fs::write(dir.path().join("b.txt"), "x\ny\nz").unwrap();
        std::fs::create_dir_all(dir.path().join(".kronn")).unwrap();
        std::fs::write(dir.path().join(".kronn/manifest.json"), "{}\n").unwrap();
        assert_eq!(
            measure(dir.path(), &baseline).unwrap(),
            ChangeStats { files: 1, lines: 3 }
        );

        git_in(dir.path(), &["add", "b.txt"]);
        git_in(dir.path(), &["commit", "-q", "-m", "agent"]);
        assert_eq!(
            measure(dir.path(), &baseline).unwrap(),
            ChangeStats { files: 1, lines: 3 },
            "a commit does not hide the run's changes"
        );
    }

    #[test]
    fn replacing_the_lines_of_an_already_dirty_file_counts_every_changed_line() {
        let dir = repo();
        std::fs::write(dir.path().join("tracked.txt"), "base\n").unwrap();
        git_in(dir.path(), &["add", "tracked.txt"]);
        git_in(dir.path(), &["commit", "-q", "-m", "tracked"]);
        let lines = |tag: &str| (0..20).map(|n| format!("{tag}{n}\n")).collect::<String>();
        // 20 added / 1 removed against HEAD before the run, and again after it.
        std::fs::write(dir.path().join("tracked.txt"), lines("before")).unwrap();
        let index = std::fs::read(dir.path().join(".git/index")).unwrap();
        let baseline = capture_baseline(dir.path()).unwrap();
        std::fs::write(dir.path().join("tracked.txt"), lines("after")).unwrap();
        assert_eq!(
            measure(dir.path(), &baseline).unwrap(),
            ChangeStats {
                files: 1,
                lines: 40
            }
        );
        assert!(limit_breach(
            &safety(Some(0), None),
            measure(dir.path(), &baseline).unwrap()
        )
        .is_some());
        assert_eq!(
            std::fs::read(dir.path().join(".git/index")).unwrap(),
            index,
            "the user's index is never written"
        );
    }

    #[test]
    fn a_hidden_file_deleted_after_the_baseline_is_a_deletion_and_a_sparse_one_is_not() {
        for flag in ["--assume-unchanged", "--skip-worktree"] {
            let dir = repo();
            std::fs::write(dir.path().join("sparse.txt"), "s\n").unwrap();
            git_in(dir.path(), &["add", "sparse.txt"]);
            git_in(dir.path(), &["commit", "-q", "-m", "sparse"]);
            git_in(dir.path(), &["update-index", flag, "a.txt", "sparse.txt"]);
            // Absent before the run starts, like a path outside a sparse checkout.
            std::fs::remove_file(dir.path().join("sparse.txt")).unwrap();
            let index = std::fs::read(dir.path().join(".git/index")).unwrap();
            let baseline = capture_baseline(dir.path()).unwrap();
            assert_eq!(
                baseline.absent_hidden,
                vec!["sparse.txt".to_string()],
                "{flag}"
            );
            assert_eq!(
                measure(dir.path(), &baseline).unwrap(),
                ChangeStats { files: 0, lines: 0 },
                "{flag}: an unchanged sparse entry is no change"
            );
            std::fs::remove_file(dir.path().join("a.txt")).unwrap();
            let stats = measure(dir.path(), &baseline).unwrap();
            assert_eq!(stats, ChangeStats { files: 1, lines: 2 }, "{flag}");
            assert!(
                limit_breach(&safety(Some(0), None), stats).is_some(),
                "{flag}"
            );
            assert_eq!(
                std::fs::read(dir.path().join(".git/index")).unwrap(),
                index,
                "{flag}: the user's index is never written"
            );
        }
    }

    #[test]
    fn a_file_hidden_by_assume_unchanged_or_skip_worktree_is_still_measured() {
        for flag in ["--assume-unchanged", "--skip-worktree"] {
            let dir = repo();
            git_in(dir.path(), &["update-index", flag, "a.txt"]);
            let index = std::fs::read(dir.path().join(".git/index")).unwrap();
            let baseline = capture_baseline(dir.path()).unwrap();
            std::fs::write(dir.path().join("a.txt"), "uno\ndos\n").unwrap();
            let stats = measure(dir.path(), &baseline).unwrap();
            assert_eq!(stats, ChangeStats { files: 1, lines: 4 }, "{flag}");
            assert!(
                limit_breach(&safety(Some(0), None), stats).is_some(),
                "{flag}"
            );
            assert_eq!(
                std::fs::read(dir.path().join(".git/index")).unwrap(),
                index,
                "{flag}: the user's index is never written"
            );
        }
    }

    #[test]
    fn a_dirty_binary_file_or_a_same_size_rewrite_is_a_change() {
        let dir = repo();
        std::fs::write(dir.path().join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(dir.path().join("a.txt"), "uno\ndos\n").unwrap();
        let baseline = capture_baseline(dir.path()).unwrap();
        assert_eq!(
            measure(dir.path(), &baseline).unwrap(),
            ChangeStats { files: 0, lines: 0 }
        );
        std::fs::write(dir.path().join("blob.bin"), [0u8, 9, 9, 9]).unwrap();
        assert_eq!(
            measure(dir.path(), &baseline).unwrap(),
            ChangeStats { files: 1, lines: 0 }
        );
        std::fs::write(dir.path().join("a.txt"), "tre\nfour\n").unwrap();
        assert_eq!(
            measure(dir.path(), &baseline).unwrap(),
            ChangeStats { files: 2, lines: 4 }
        );
    }

    #[test]
    fn each_setting_this_host_would_refuse_is_reported() {
        let mut all = safety(Some(1), None);
        all.sandbox = true;
        all.require_approval = true;
        assert_eq!(
            warnings(&all, false, DirectoryState::Missing, true),
            vec![
                SafetyWarning::SandboxOutsideContainer,
                SafetyWarning::LimitsWithoutDirectory,
                SafetyWarning::ApprovalOnSubWorkflow
            ]
        );
        assert_eq!(
            warnings(&all, true, DirectoryState::NotGit, false),
            vec![SafetyWarning::LimitsWithoutGit]
        );
        assert!(warnings(&all, true, DirectoryState::PerRun, false).is_empty());
        let plain = tempfile::TempDir::new().unwrap();
        let nested = plain.path().join("plain");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(directory_state(Some(&nested)), DirectoryState::NotGit);
        let git = repo();
        assert_eq!(directory_state(Some(git.path())), DirectoryState::Git);
        assert_eq!(directory_state(None), DirectoryState::Missing);
    }

    #[test]
    fn a_limit_is_breached_only_above_its_value() {
        let stats = ChangeStats {
            files: 2,
            lines: 10,
        };
        assert!(limit_breach(&safety(Some(2), Some(10)), stats).is_none());
        assert!(limit_breach(&safety(Some(1), None), stats)
            .unwrap()
            .contains("2 files changed (max 1)"));
        assert!(limit_breach(&safety(None, Some(9)), stats)
            .unwrap()
            .contains("10 lines changed (max 9)"));
        assert!(limit_breach(&safety(None, None), stats).is_none());
    }

    #[test]
    fn a_folder_outside_git_has_no_baseline() {
        let dir = tempfile::TempDir::new().unwrap();
        let nested = dir.path().join("plain");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(capture_baseline(&nested).is_err());
    }
}
