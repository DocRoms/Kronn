//! Shared git operation helpers used by both project and discussion endpoints.

use crate::core::cmd::sync_cmd;
use crate::core::commit_policy::CommitPolicy;
use crate::models::*;
use std::path::Path;

const PROJECT_GIT_GRAPH_LIMIT: usize = 80;

/// How long `git commit` may run: its hooks (a test suite, a formatter) are the project's.
const COMMIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// How long `git push` may run: the network, the remote's own hooks, the project's `pre-push`
/// hook and an LFS upload. Also the bound of the forge CLI that opens the pull request.
const PUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Every git of the panel and its actions within a time bound, its process group killed past
/// it (KT-2084): a FIFO in a checkout (a `.gitignore`, a `HEAD`) makes git wait forever, and
/// each such call would hold a thread of the blocking pool the database also uses.
trait BoundedGit {
    /// `output()` within git's judgement bound.
    fn bounded_output(&mut self) -> std::io::Result<std::process::Output>;
    /// `output()` within `timeout`.
    fn bounded_output_within(
        &mut self,
        timeout: std::time::Duration,
    ) -> std::io::Result<std::process::Output>;
}

impl BoundedGit for std::process::Command {
    fn bounded_output(&mut self) -> std::io::Result<std::process::Output> {
        self.bounded_output_within(crate::core::lane_git::snapshot::judge_timeout())
    }

    fn bounded_output_within(
        &mut self,
        timeout: std::time::Duration,
    ) -> std::io::Result<std::process::Output> {
        match crate::core::cmd::run_bounded_output(self, &[], timeout)? {
            crate::core::cmd::BoundedOutput::Exited(output) => Ok(output),
            // Said as Kronn's own bound: neither git nor the remote gave up.
            crate::core::cmd::BoundedOutput::TimedOut => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "it did not finish within Kronn's own {} s timeout, so Kronn stopped it",
                    timeout.as_secs_f32()
                ),
            )),
        }
    }
}
pub const GIT_COMMIT_PAGE_DEFAULT: u32 = 40;
pub const GIT_COMMIT_PAGE_MAX: u32 = 100;

/// Parse `git diff --name-status <base>...HEAD` output into structured file
/// statuses. Each non-rename line is `<code>\t<path>`; rename/copy lines are
/// `R<score>\t<old>\t<new>` (we keep the destination path).
pub fn parse_committed_diff(diff_output: &str) -> Vec<GitFileStatus> {
    diff_output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let code = parts.next()?;
            let status_char = code.chars().next()?;
            let status = match status_char {
                'A' => "added",
                'D' => "deleted",
                'M' => "modified",
                'R' => "renamed",
                'C' => "copied",
                'T' => "modified",
                _ => return None,
            };
            let path = parts.next_back()?.trim_matches('"').to_string();
            if path.is_empty() {
                return None;
            }
            Some(GitFileStatus {
                path,
                status: status.to_string(),
                staged: true,
            })
        })
        .collect()
}

fn parse_commit_summaries(output: &str) -> Vec<GitCommitSummary> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\x1f');
            let sha = fields.next()?.to_string();
            let short_sha = fields.next()?.to_string();
            let author_name = fields.next()?.to_string();
            let author_time = fields.next()?.parse().ok()?;
            let subject = fields.next()?.to_string();
            Some(GitCommitSummary {
                sha,
                short_sha,
                subject,
                author_name,
                author_time,
            })
        })
        .collect()
}

fn normalized_commit_page(offset: u32, limit: u32) -> (u32, u32) {
    (offset, limit.clamp(1, GIT_COMMIT_PAGE_MAX))
}

/// Read one newest-first page without ever materializing the whole history in
/// the backend. The independent count keeps the UI honest about the total and
/// lets it distinguish an empty history from a bounded response.
fn run_git_commit_page(
    repo_path: &Path,
    range: &str,
    offset: u32,
    limit: u32,
) -> Result<(Vec<GitCommitSummary>, u32, bool), String> {
    let (offset, limit) = normalized_commit_page(offset, limit);
    let count = crate::core::lane_git::git_or_refuse(repo_path)
        .args(["rev-list", "--count", "--end-of-options", range, "--"])
        .current_dir(repo_path)
        .bounded_output()
        .map_err(|error| format!("Failed to count git commits: {error}"))?;
    if !count.status.success() {
        return Err(String::from_utf8_lossy(&count.stderr).trim().to_string());
    }
    let total = String::from_utf8_lossy(&count.stdout)
        .trim()
        .parse::<u32>()
        .map_err(|error| format!("Invalid git commit count: {error}"))?;
    if offset >= total {
        return Ok((Vec::new(), total, false));
    }

    let skip = format!("--skip={offset}");
    let max_count = format!("--max-count={limit}");
    let log = crate::core::lane_git::git_or_refuse(repo_path)
        .args([
            "log",
            "--format=%H%x1f%h%x1f%an%x1f%at%x1f%s",
            skip.as_str(),
            max_count.as_str(),
            "--end-of-options",
            range,
            "--",
        ])
        .current_dir(repo_path)
        .bounded_output()
        .map_err(|error| format!("Failed to run git log: {error}"))?;
    if !log.status.success() {
        return Err(String::from_utf8_lossy(&log.stderr).trim().to_string());
    }
    let commits = parse_commit_summaries(&String::from_utf8_lossy(&log.stdout));
    let returned = u32::try_from(commits.len()).unwrap_or(u32::MAX);
    let truncated = offset.saturating_add(returned) < total;
    Ok((commits, total, truncated))
}

/// Read the exact committed evidence between two durable SHAs.
pub fn run_git_range_page(
    repo_path: &Path,
    base_sha: &str,
    head_sha: &str,
    commit_offset: u32,
    commit_limit: u32,
) -> Result<(Vec<GitFileStatus>, Vec<GitCommitSummary>, u32, bool), String> {
    let range = format!("{base_sha}..{head_sha}");
    let diff = crate::core::lane_git::git_or_refuse(repo_path)
        .args(["diff", "--name-status", "--end-of-options", &range, "--"])
        .current_dir(repo_path)
        .bounded_output()
        .map_err(|error| format!("Failed to run git diff: {error}"))?;
    if !diff.status.success() {
        return Err(String::from_utf8_lossy(&diff.stderr).trim().to_string());
    }
    let (commits, total, truncated) =
        run_git_commit_page(repo_path, &range, commit_offset, commit_limit)?;
    Ok((
        parse_committed_diff(&String::from_utf8_lossy(&diff.stdout)),
        commits,
        total,
        truncated,
    ))
}

pub fn run_git_diff_range(
    repo_path: &Path,
    base_sha: &str,
    head_sha: &str,
    file_path: &str,
) -> Result<GitDiffResponse, String> {
    let range = format!("{base_sha}..{head_sha}");
    let output = crate::core::lane_git::git_or_refuse(repo_path)
        .args(["diff", "--end-of-options", &range, "--", file_path])
        .current_dir(repo_path)
        .bounded_output()
        .map_err(|error| format!("Failed to run git diff: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(GitDiffResponse {
        path: file_path.to_string(),
        diff: String::from_utf8_lossy(&output.stdout).to_string(),
    })
}

/// Run `git status` in the given repo directory and return structured status.
/// A PR lookup there uses `gh`'s own login only.
pub fn run_git_status(repo_path: &Path) -> Result<GitStatusResponse, String> {
    run_git_status_page(repo_path, 0, GIT_COMMIT_PAGE_DEFAULT, &[])
}

/// Run `git status` with an explicitly bounded commit-history page. File
/// status remains complete and independent from commit pagination.
/// `github_env` is the project's GitHub variables for the PR lookup.
pub fn run_git_status_page(
    repo_path: &Path,
    commit_offset: u32,
    commit_limit: u32,
    github_env: &[(String, String)],
) -> Result<GitStatusResponse, String> {
    run_git_status_impl(
        repo_path,
        commit_offset,
        commit_limit,
        true,
        Some(github_env),
    )
}

/// Same status without the network PR lookup: a PR link already known from an
/// earlier lookup is still reported, an unknown one stays empty.
pub fn run_git_status_page_without_pr_lookup(
    repo_path: &Path,
    commit_offset: u32,
    commit_limit: u32,
) -> Result<GitStatusResponse, String> {
    run_git_status_impl(repo_path, commit_offset, commit_limit, true, None)
}

/// Selected discussion workspaces replace branch-relative evidence with their
/// durable base..head range. Skip the redundant branch diff/count/log while
/// still collecting the independent working-tree and repository metadata.
pub fn run_git_status_without_commit_evidence(
    repo_path: &Path,
    github_env: &[(String, String)],
) -> Result<GitStatusResponse, String> {
    run_git_status_impl(
        repo_path,
        0,
        GIT_COMMIT_PAGE_DEFAULT,
        false,
        Some(github_env),
    )
}

fn run_git_status_impl(
    repo_path: &Path,
    commit_offset: u32,
    commit_limit: u32,
    include_commit_evidence: bool,
    // `None`: no network PR lookup; `Some`: the GitHub variables for it.
    lookup_pr: Option<&[(String, String)]>,
) -> Result<GitStatusResponse, String> {
    let run = |args: &[&str]| -> Result<String, String> {
        let output = crate::core::lane_git::git_or_refuse(repo_path)
            .args(args)
            .current_dir(repo_path)
            .bounded_output()
            .map_err(|e| format!("Failed to run git: {}", e))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };

    let run_with_status = |args: &[&str]| -> (String, bool) {
        match crate::core::lane_git::git_or_refuse(repo_path)
            .args(args)
            .current_dir(repo_path)
            .bounded_output()
        {
            Ok(o) => (
                String::from_utf8_lossy(&o.stdout).trim().to_string(),
                o.status.success(),
            ),
            Err(_) => (String::new(), false),
        }
    };

    // Current branch
    let branch = run(&["branch", "--show-current"])?;

    // Default branch detection: try local refs first, then remote refs
    let default_branch = {
        let (_, ok_main) = run_with_status(&["rev-parse", "--verify", "main"]);
        if ok_main {
            "main".to_string()
        } else {
            let (_, ok_master) = run_with_status(&["rev-parse", "--verify", "master"]);
            if ok_master {
                "master".to_string()
            } else {
                // Fallback: check remote refs (worktrees may not have local main/master)
                let (_, ok_remote_main) =
                    run_with_status(&["rev-parse", "--verify", "origin/main"]);
                if ok_remote_main {
                    "main".to_string()
                } else {
                    let (_, ok_remote_master) =
                        run_with_status(&["rev-parse", "--verify", "origin/master"]);
                    if ok_remote_master {
                        "master".to_string()
                    } else {
                        String::new()
                    }
                }
            }
        }
    };

    let is_default_branch = !default_branch.is_empty() && branch == default_branch;

    // Parse porcelain v1 status
    let status_output = run(&crate::core::lane_git::with_ignore_submodules(
        repo_path,
        &["status", "--porcelain=v1", "-u"],
    ))?;
    let files: Vec<GitFileStatus> = status_output
        .lines()
        .filter(|l| l.len() >= 3)
        .map(|line| {
            let bytes = line.as_bytes();
            let staged_char = bytes[0] as char;
            let unstaged_char = bytes[1] as char;
            // Porcelain v1 format: XY<space>filename (or XY<space>old -> new for renames)
            // Some git versions may use XY<space><space>filename, so skip all leading spaces after XY
            let raw_path = line[2..].trim_start().to_string();
            let path = if raw_path.contains(" -> ") {
                raw_path
                    .split(" -> ")
                    .last()
                    .unwrap_or(&raw_path)
                    .to_string()
            } else {
                raw_path
            };
            let path = path.trim_matches('"').to_string();

            let status = match (staged_char, unstaged_char) {
                ('?', '?') => "untracked",
                ('A', _) => "added",
                ('D', _) | (_, 'D') => "deleted",
                ('R', _) => "renamed",
                ('M', _) | (_, 'M') => "modified",
                ('C', _) => "copied",
                _ => "modified",
            }
            .to_string();

            let staged = staged_char != ' ' && staged_char != '?';

            GitFileStatus {
                path,
                status,
                staged,
            }
        })
        .collect();

    // Committed-on-branch (vs default_branch). Empty on default branch or when
    // we couldn't resolve a default branch. Use `<default>...HEAD` triple-dot
    // to compare against the merge-base, so unrelated commits on default don't
    // appear as "deleted" here.
    let (committed_files, commits, commits_total, commits_truncated) =
        if include_commit_evidence && !is_default_branch && !default_branch.is_empty() {
            let range = format!("{}...HEAD", default_branch);
            let (diff_out, ok) = run_with_status(&["diff", "--name-status", &range]);
            if ok {
                let (commits, total, truncated) =
                    run_git_commit_page(repo_path, &range, commit_offset, commit_limit)?;
                (parse_committed_diff(&diff_out), commits, total, truncated)
            } else {
                (Vec::new(), Vec::new(), 0, false)
            }
        } else {
            (Vec::new(), Vec::new(), 0, false)
        };

    // Ahead/behind upstream
    let (ahead, behind) = {
        let (ab_output, ab_ok) =
            run_with_status(&["rev-list", "--count", "--left-right", "@{upstream}...HEAD"]);
        if ab_ok {
            let parts: Vec<&str> = ab_output.split_whitespace().collect();
            if parts.len() == 2 {
                let b = parts[0].parse::<u32>().unwrap_or(0);
                let a = parts[1].parse::<u32>().unwrap_or(0);
                (a, b)
            } else {
                (0, 0)
            }
        } else if !branch.is_empty() && !default_branch.is_empty() && branch != default_branch {
            // No upstream: count commits ahead of the default branch (for worktree branches)
            let (count_output, count_ok) =
                run_with_status(&["rev-list", "--count", &format!("{}..HEAD", default_branch)]);
            if count_ok {
                let a = count_output.trim().parse::<u32>().unwrap_or(1);
                // Use at least 1 so the Push button appears (branch needs to be pushed)
                (a.max(1), 0)
            } else {
                // Branch exists but can't compare — still show push button
                (1, 0)
            }
        } else {
            (0, 0)
        }
    };

    // Check if branch has an upstream and retain its human-readable name.
    let upstream = {
        let (name, ok) = run_with_status(&[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ]);
        (ok && !name.is_empty()).then_some(name)
    };
    let has_upstream = upstream.is_some();

    // Check if there's an open PR/MR for this branch
    let pr_url = if !branch.is_empty() && !is_default_branch {
        match lookup_pr {
            Some(github_env) => cached_pr_url(repo_path, &branch, github_env),
            None => known_pr_url(repo_path, &branch),
        }
    } else {
        None
    };

    let provider = detect_provider(repo_path).to_string();
    let remote_url = git_remote_web_url(repo_path);
    let pull_requests_url = remote_url.as_ref().and_then(|url| match provider.as_str() {
        "github" => Some(format!("{url}/pulls")),
        "gitlab" => Some(format!("{url}/-/merge_requests")),
        _ => None,
    });
    let (tag, tag_ok) = run_with_status(&[
        "for-each-ref",
        "--sort=-creatordate",
        "--count=1",
        "--format=%(refname:short)",
        "refs/tags",
    ]);
    let last_tag = (tag_ok && !tag.is_empty()).then_some(tag);

    Ok(GitStatusResponse {
        branch,
        default_branch,
        is_default_branch,
        files,
        committed_files,
        commits,
        commits_total,
        commits_offset: commit_offset,
        commits_truncated,
        workspace: None,
        empty_reason: None,
        ahead,
        behind,
        has_upstream,
        upstream,
        provider,
        remote_url,
        pull_requests_url,
        last_tag,
        pr_url,
        languages: Vec::new(),
        languages_checked_at: None,
        languages_cached: false,
    })
}

fn run_git(repo_path: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    crate::core::lane_git::git_or_refuse(repo_path)
        .args(args)
        .current_dir(repo_path)
        .bounded_output()
        .map_err(|error| format!("Failed to run git: {error}"))
}

pub fn git_output(repo_path: &Path, args: &[&str]) -> Result<String, String> {
    let output = run_git(repo_path, args)?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if error.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            error
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn parse_branch_summary(line: &str, current_branch: &str) -> Option<GitBranchSummary> {
    let fields: Vec<&str> = line.split('\u{1f}').collect();
    if fields.len() != 8 {
        return None;
    }
    let ref_name = fields[0].to_string();
    let is_remote = ref_name.starts_with("refs/remotes/");
    let name = ref_name
        .strip_prefix("refs/heads/")
        .or_else(|| ref_name.strip_prefix("refs/remotes/"))?
        .to_string();
    if name.ends_with("/HEAD") {
        return None;
    }
    let upstream = (!fields[6].is_empty()).then(|| fields[6].to_string());
    let (ahead, behind) = fields[7]
        .split_once(' ')
        .and_then(|(ahead, behind)| Some((ahead.parse().ok()?, behind.parse().ok()?)))
        .unwrap_or((0, 0));
    Some(GitBranchSummary {
        is_current: !is_remote && name == current_branch,
        name,
        ref_name,
        commit: fields[1].to_string(),
        subject: fields[2].to_string(),
        author: fields[3].to_string(),
        committed_at: fields[4].parse().unwrap_or_default(),
        is_remote,
        upstream,
        ahead,
        behind,
    })
}

fn parse_graph_commit(line: &str) -> Option<GitGraphCommit> {
    let fields: Vec<&str> = line.split('\u{1f}').collect();
    if fields.len() != 6 {
        return None;
    }
    let hash = fields[0].to_string();
    Some(GitGraphCommit {
        short_hash: hash.chars().take(8).collect(),
        hash,
        parents: fields[1]
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect(),
        refs: fields[2]
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.contains("HEAD ->"))
            .map(ToOwned::to_owned)
            .collect(),
        subject: fields[3].to_string(),
        author: fields[4].to_string(),
        committed_at: fields[5].parse().unwrap_or_default(),
    })
}

/// Return a bounded, structured overview of local/remote branches and recent
/// commits. Every Git value is passed as a distinct argument; no shell is used.
pub fn run_git_branches(repo_path: &Path) -> Result<GitBranchesResponse, String> {
    let current_branch = git_output(repo_path, &["branch", "--show-current"])?;
    let default_branch = resolve_default_branch(repo_path);
    let refs = git_output(
        repo_path,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)%1f%(objectname)%1f%(subject)%1f%(authorname)%1f%(authordate:unix)%1f%(HEAD)%1f%(upstream:short)%1f%(ahead-behind:HEAD)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let branches = refs
        .lines()
        .filter_map(|line| parse_branch_summary(line, &current_branch))
        .collect();

    let max_count = format!("--max-count={}", PROJECT_GIT_GRAPH_LIMIT + 1);
    let log = git_output(
        repo_path,
        &[
            "log",
            "--all",
            "--topo-order",
            "--date-order",
            &max_count,
            "--pretty=format:%H%x1f%P%x1f%D%x1f%s%x1f%an%x1f%at",
        ],
    )?;
    let mut commits: Vec<_> = log.lines().filter_map(parse_graph_commit).collect();
    let truncated = commits.len() > PROJECT_GIT_GRAPH_LIMIT;
    commits.truncate(PROJECT_GIT_GRAPH_LIMIT);

    Ok(GitBranchesResponse {
        current_branch,
        default_branch,
        branches,
        commits,
        truncated,
    })
}

/// Safely switch to an existing local branch or an unambiguous `origin/*`
/// remote branch. A dirty worktree is never stashed or reset implicitly.
pub fn run_git_switch_branch(
    repo_path: &Path,
    requested_branch: &str,
) -> Result<GitBranchResponse, String> {
    let branch = requested_branch.trim();
    if branch.is_empty() || branch.len() > 255 {
        return Err("Nom de branche invalide.".to_string());
    }
    let valid = run_git(repo_path, &["check-ref-format", "--branch", branch])?;
    if !valid.status.success() {
        return Err("Nom de branche invalide.".to_string());
    }

    let overview = run_git_branches(repo_path)?;
    if branch == overview.current_branch {
        return Ok(GitBranchResponse {
            branch: branch.to_string(),
        });
    }

    let selected = overview
        .branches
        .iter()
        .find(|candidate| candidate.name == branch)
        .ok_or_else(|| "Branche introuvable. Actualisez la liste puis réessayez.".to_string())?;

    let dirty = git_output(
        repo_path,
        &crate::core::lane_git::with_ignore_submodules(
            repo_path,
            &["status", "--porcelain=v1", "-uall"],
        ),
    )?;
    if !dirty.is_empty() {
        return Err(
            "Le changement de branche est bloqué : le projet contient des modifications locales. Committez ou mettez-les de côté explicitement, puis réessayez."
                .to_string(),
        );
    }

    let output = if selected.is_remote {
        let local_name = branch.strip_prefix("origin/").ok_or_else(|| {
            "Seules les branches distantes origin/* peuvent être suivies automatiquement."
                .to_string()
        })?;
        if overview
            .branches
            .iter()
            .any(|candidate| !candidate.is_remote && candidate.name == local_name)
        {
            return Err(format!(
                "La branche locale {local_name} existe déjà. Sélectionnez-la directement."
            ));
        }
        run_git(repo_path, &["switch", "--track", "-c", local_name, branch])?
    } else {
        run_git(repo_path, &["switch", "--", branch])?
    };

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if error.is_empty() {
            "Git n’a pas pu changer de branche.".to_string()
        } else {
            format!("Git n’a pas pu changer de branche : {error}")
        });
    }
    let switched = git_output(repo_path, &["branch", "--show-current"])?;
    Ok(GitBranchResponse { branch: switched })
}

/// Convert an origin remote into a browser-safe repository URL.
///
/// Supports HTTPS, SSH URLs and SCP-like Git remotes. Local filesystem
/// remotes intentionally return `None`. Any credentials embedded in an HTTP
/// remote are removed before the URL reaches the frontend.
pub fn normalize_git_remote_web_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let web = if let Some(rest) = raw
        .strip_prefix("git@")
        .or_else(|| raw.strip_prefix("ssh://git@"))
    {
        let (host, path) = if let Some((host, path)) = rest.split_once(':') {
            (host, path)
        } else {
            rest.split_once('/')?
        };
        format!("https://{host}/{}", path.trim_start_matches('/'))
    } else if let Some(rest) = raw.strip_prefix("ssh://") {
        let rest = rest
            .rsplit_once('@')
            .map(|(_, value)| value)
            .unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        format!("https://{host}/{}", path.trim_start_matches('/'))
    } else if let Some(rest) = raw.strip_prefix("git://") {
        format!("https://{rest}")
    } else if raw.starts_with("https://") || raw.starts_with("http://") {
        let (scheme, rest) = raw.split_once("://")?;
        let sanitized = rest
            .rsplit_once('@')
            .map(|(_, value)| value)
            .unwrap_or(rest);
        format!("{scheme}://{sanitized}")
    } else {
        return None;
    };

    let web = web
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string();
    (!web.is_empty()).then_some(web)
}

fn git_remote_web_url(repo_path: &Path) -> Option<String> {
    let output = crate::core::lane_git::git_or_refuse(repo_path)
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_path)
        .bounded_output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    normalize_git_remote_web_url(&String::from_utf8_lossy(&output.stdout))
}

/// Run `git diff` for a specific file in the given repo directory.
/// Resolve the repo's default branch (main/master, local then remote refs).
/// Worktrees often lack a local `main`, so we fall back to `origin/*`.
/// Returns an empty string when none resolves (detached / fresh repo).
pub fn resolve_default_branch(repo_path: &Path) -> String {
    let ok = |args: &[&str]| -> bool {
        crate::core::lane_git::git_or_refuse(repo_path)
            .args(args)
            .current_dir(repo_path)
            .bounded_output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    for (refname, branch) in [
        ("main", "main"),
        ("master", "master"),
        ("origin/main", "main"),
        ("origin/master", "master"),
    ] {
        if ok(&["rev-parse", "--verify", refname]) {
            return branch.to_string();
        }
    }
    String::new()
}

/// Committed diff for a single path: `git diff <default>...HEAD -- <path>`
/// (triple-dot = vs the merge-base, so unrelated default-branch commits don't
/// leak in). Falls back to the last commit's change when no default branch
/// resolves. Used by the GitPanel "committed on branch" section.
pub fn run_git_diff_committed(
    repo_path: &Path,
    file_path: &str,
) -> Result<GitDiffResponse, String> {
    let git_stdout = |args: &[&str]| -> String {
        crate::core::lane_git::git_or_refuse(repo_path)
            .args(args)
            .current_dir(repo_path)
            .bounded_output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    };
    let default_branch = resolve_default_branch(repo_path);
    let diff = if !default_branch.is_empty() {
        git_stdout(&[
            "diff",
            &format!("{}...HEAD", default_branch),
            "--",
            file_path,
        ])
    } else {
        // No default branch (detached / fresh): show the file's last-commit change.
        git_stdout(&["diff", "HEAD~1", "HEAD", "--", file_path])
    };
    Ok(GitDiffResponse {
        path: file_path.to_string(),
        diff,
    })
}

pub fn run_git_diff(repo_path: &Path, file_path: &str) -> Result<GitDiffResponse, String> {
    let run_diff = |args: &[&str]| -> String {
        crate::core::lane_git::git_or_refuse(repo_path)
            .args(args)
            .current_dir(repo_path)
            .bounded_output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    };

    // Unstaged diff
    let unstaged = run_diff(&crate::core::lane_git::with_ignore_submodules(
        repo_path,
        &["diff", "--", file_path],
    ));
    // Staged diff
    let staged = run_diff(&crate::core::lane_git::with_ignore_submodules(
        repo_path,
        &["diff", "--cached", "--", file_path],
    ));

    // For untracked or newly added files, git diff returns nothing.
    let untracked_diff = if unstaged.is_empty() && staged.is_empty() {
        let full_path = repo_path.join(file_path);
        if full_path.exists() {
            match std::fs::read_to_string(&full_path) {
                Ok(content) => {
                    let lines: Vec<String> = content.lines().map(|l| format!("+{}", l)).collect();
                    if lines.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "--- /dev/null\n+++ b/{}\n@@ -0,0 +1,{} @@\n{}",
                            file_path,
                            lines.len(),
                            lines.join("\n")
                        )
                    }
                }
                Err(_) => String::new(),
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Combine all diffs
    let diff = if !staged.is_empty() && !unstaged.is_empty() {
        format!("--- Staged ---\n{}\n--- Unstaged ---\n{}", staged, unstaged)
    } else if !staged.is_empty() {
        staged
    } else if !unstaged.is_empty() {
        unstaged
    } else {
        untracked_diff
    };

    Ok(GitDiffResponse {
        path: file_path.to_string(),
        diff,
    })
}

pub fn parse_git_blame_porcelain(output: &str) -> Vec<GitBlameLine> {
    let mut lines = Vec::new();
    let mut commit = String::new();
    let mut line_number = 0u32;
    let mut author = String::new();
    let mut author_time = 0i64;

    for raw_line in output.lines() {
        if raw_line.starts_with('\t') {
            if line_number > 0 {
                lines.push(GitBlameLine {
                    line_number,
                    commit: commit.clone(),
                    author: if author.is_empty() {
                        "Unknown".to_string()
                    } else {
                        author.clone()
                    },
                    author_time,
                });
            }
            continue;
        }

        let mut header = raw_line.split_ascii_whitespace();
        let maybe_commit = header.next().unwrap_or_default();
        let original = header.next().and_then(|value| value.parse::<u32>().ok());
        let final_line = header.next().and_then(|value| value.parse::<u32>().ok());
        if maybe_commit.len() >= 7
            && maybe_commit
                .chars()
                .all(|character| character.is_ascii_hexdigit())
            && original.is_some()
            && final_line.is_some()
        {
            commit.clear();
            commit.push_str(maybe_commit);
            line_number = final_line.unwrap_or_default();
            author.clear();
            author_time = 0;
        } else if let Some(value) = raw_line.strip_prefix("author ") {
            author.clear();
            author.push_str(value);
        } else if let Some(value) = raw_line.strip_prefix("author-time ") {
            author_time = value.parse().unwrap_or_default();
        }
    }
    lines
}

/// Return one Git author/date annotation per current working-tree line.
pub fn run_git_blame(repo_path: &Path, file_path: &str) -> Result<GitBlameResponse, String> {
    let output = crate::core::lane_git::git_or_refuse(repo_path)
        .args(["blame", "--line-porcelain", "--", file_path])
        .current_dir(repo_path)
        .bounded_output()
        .map_err(|error| format!("Failed to run git blame: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git blame failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("git blame returned invalid UTF-8: {error}"))?;
    Ok(GitBlameResponse {
        path: file_path.to_string(),
        lines: parse_git_blame_porcelain(&stdout),
    })
}

/// How many containing branches we report before truncating.
pub const COMMIT_BRANCHES_CAP: usize = 12;

/// A commit-ish is only ever accepted as a hex hash here. It is interpolated
/// into a git invocation, and blame only ever hands us hashes — so anything
/// else is either a bug or an attempt, and both deserve a refusal rather than
/// a best effort.
pub fn valid_commit_ish(sha: &str) -> bool {
    let len = sha.len();
    (7..=40).contains(&len) && sha.chars().all(|c| c.is_ascii_hexdigit())
}

/// KT-67 — the commit behind an annotated line.
///
/// Deliberately NOT the diff: this feeds a detail popover opened from a blame
/// gutter, and a merge commit's patch would be megabytes. Metadata, message,
/// a bounded list of containing branches, and the number of files touched.
pub fn run_git_commit_detail(
    repo_path: &Path,
    sha: &str,
) -> Result<crate::models::git::GitCommitDetail, String> {
    if !valid_commit_ish(sha) {
        return Err("invalid commit hash".to_string());
    }

    // Unit separator between fields, so a subject containing tabs or pipes
    // can't shift the parse.
    let format = "%H%x1f%h%x1f%an%x1f%ae%x1f%at%x1f%cn%x1f%ct%x1f%s%x1f%b";
    let output = crate::core::lane_git::git_or_refuse(repo_path)
        .args(["show", "--no-patch", &format!("--format={format}"), sha])
        .current_dir(repo_path)
        .bounded_output()
        .map_err(|error| format!("Failed to run git show: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git show failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let fields: Vec<&str> = stdout.trim_end().split('\u{1f}').collect();
    if fields.len() < 9 {
        return Err("git show returned an unexpected format".to_string());
    }

    // `diff-tree` rather than `git show --name-only`: `--no-patch` would have
    // suppressed the file list too, and a merge commit prints nothing without
    // `-m` — so an empty result here means "nothing attributable", not an error.
    let files = crate::core::lane_git::git_or_refuse(repo_path)
        .args([
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "--root",
            sha,
        ])
        .current_dir(repo_path)
        .bounded_output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count() as u32
        })
        .unwrap_or(0);

    let mut branches: Vec<String> = crate::core::lane_git::git_or_refuse(repo_path)
        .args([
            "branch",
            "-a",
            "--contains",
            sha,
            "--format=%(refname:short)",
        ])
        .current_dir(repo_path)
        .bounded_output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let branches_truncated = branches.len() > COMMIT_BRANCHES_CAP;
    branches.truncate(COMMIT_BRANCHES_CAP);

    Ok(crate::models::git::GitCommitDetail {
        sha: fields[0].to_string(),
        short_sha: fields[1].to_string(),
        author_name: fields[2].to_string(),
        author_email: fields[3].to_string(),
        author_time: fields[4].parse().unwrap_or(0),
        committer_name: fields[5].to_string(),
        commit_time: fields[6].parse().unwrap_or(0),
        subject: fields[7].to_string(),
        body: fields[8].trim().to_string(),
        branches,
        branches_truncated,
        files_changed: files,
    })
}

/// Bytes of patch we are willing to ship for one commit. Past this the reader
/// is not reading anymore, and the JSON response starts costing real memory.
pub const COMMIT_PATCH_MAX_BYTES: usize = 400 * 1024;

/// KT-75 — the historical patch of one commit: `parent → commit`, every file,
/// every hunk. `--root` is what makes the first commit of a repository work at
/// all; without it `git show` prints its message and no diff.
pub fn run_git_commit_patch(
    repo_path: &Path,
    sha: &str,
) -> Result<crate::models::git::GitCommitPatch, String> {
    if !valid_commit_ish(sha) {
        return Err("invalid commit hash".to_string());
    }

    let header = crate::core::lane_git::git_or_refuse(repo_path)
        .args(["show", "--no-patch", "--format=%H%x1f%h%x1f%s", sha])
        .current_dir(repo_path)
        .bounded_output()
        .map_err(|error| format!("Failed to run git show: {error}"))?;
    if !header.status.success() {
        return Err(format!(
            "git show failed: {}",
            String::from_utf8_lossy(&header.stderr).trim()
        ));
    }
    let header_out = String::from_utf8_lossy(&header.stdout);
    let fields: Vec<&str> = header_out.trim_end().split('\u{1f}').collect();
    if fields.len() < 3 {
        return Err("git show returned an unexpected format".to_string());
    }

    // No parent listed → root commit. Asked separately because `--root` changes
    // what the patch means, and the UI says so.
    let is_root = crate::core::lane_git::git_or_refuse(repo_path)
        .args(["rev-list", "--parents", "-n", "1", sha])
        .current_dir(repo_path)
        .bounded_output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .count()
                <= 1
        })
        .unwrap_or(false);

    // `-m` splits a merge into one patch per parent instead of printing nothing.
    let patch_out = crate::core::lane_git::git_or_refuse(repo_path)
        .args([
            "show",
            "--format=",
            "--patch",
            "--root",
            "-m",
            "--no-color",
            sha,
        ])
        .current_dir(repo_path)
        .bounded_output()
        .map_err(|error| format!("Failed to run git show: {error}"))?;
    if !patch_out.status.success() {
        return Err(format!(
            "git show failed: {}",
            String::from_utf8_lossy(&patch_out.stderr).trim()
        ));
    }

    let full = String::from_utf8_lossy(&patch_out.stdout);
    let truncated = full.len() > COMMIT_PATCH_MAX_BYTES;
    let patch = if truncated {
        // Cut on a char boundary, then back off to the last complete line so the
        // viewer never renders half a hunk header.
        let mut cut = COMMIT_PATCH_MAX_BYTES;
        while cut > 0 && !full.is_char_boundary(cut) {
            cut -= 1;
        }
        let slice = &full[..cut];
        match slice.rfind('\n') {
            Some(end) => slice[..=end].to_string(),
            None => slice.to_string(),
        }
    } else {
        full.to_string()
    };

    let files_changed = crate::core::lane_git::git_or_refuse(repo_path)
        .args([
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "--root",
            sha,
        ])
        .current_dir(repo_path)
        .bounded_output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count() as u32
        })
        .unwrap_or(0);

    Ok(crate::models::git::GitCommitPatch {
        sha: fields[0].to_string(),
        short_sha: fields[1].to_string(),
        subject: fields[2].to_string(),
        patch,
        truncated,
        files_changed,
        is_root,
    })
}

/// Stage files and commit in the given repo directory.
pub fn run_git_commit(
    repo_path: &Path,
    files: &[String],
    message: &str,
    amend: bool,
    sign: bool,
) -> Result<GitCommitResponse, String> {
    commit_explicit_paths(repo_path, files, message, amend, sign, None, None)
}

/// Stage each explicit path (`git add`, or `git rm --cached` for a path that no
/// longer exists) and return how many were staged. A path git refuses is skipped.
fn stage_explicit_paths(
    repo_path: &Path,
    clean_files: &[&str],
    worker: bool,
) -> Result<usize, String> {
    let mut added = 0;
    for &clean_file in clean_files {
        let file_abs = repo_path.join(clean_file);

        if file_abs.exists() {
            if worker {
                refuse_new_embedded_repository(repo_path, clean_file, &file_abs)?;
                // A submodule is staged by its sha: `git add` would compare its
                // working tree, which starts a `git status` inside it.
                if stage_gitlink_by_sha(repo_path, clean_file, &file_abs)? {
                    added += 1;
                    continue;
                }
            }
            let add_output = git_in(repo_path, worker)?
                .args(["add", "--", clean_file])
                .bounded_output()
                .map_err(|e| format!("Failed to run git add: {}", e))?;
            if add_output.status.success() {
                added += 1;
            } else {
                tracing::warn!(
                    "git add skipped '{}': {}",
                    clean_file,
                    String::from_utf8_lossy(&add_output.stderr).trim()
                );
            }
        } else {
            let rm_output = git_in(repo_path, worker)?
                .args(["rm", "--cached", "--ignore-unmatch", "--", clean_file])
                .bounded_output();
            if rm_output.map(|o| o.status.success()).unwrap_or(false) {
                added += 1;
            }
        }
    }
    Ok(added)
}

/// When `clean_file` is itself a gitlink of the worktree's index, stage the
/// submodule's current `HEAD` with `update-index --cacheinfo` and return `true`
/// (`git add` would run `git status` inside the submodule, where a program its
/// configuration names would run). A gitlink whose submodule is not materialised has
/// nothing to stage: `true` too. Any other path returns `false`.
fn stage_gitlink_by_sha(
    repo_path: &Path,
    clean_file: &str,
    file_abs: &Path,
) -> Result<bool, String> {
    let wanted = clean_file.trim_end_matches('/');
    let listed = git_in(repo_path, true)?
        .args(["ls-files", "--stage", "-z", "--", wanted])
        .bounded_output()
        .map_err(|e| format!("Failed to run git ls-files: {e}"))?;
    let is_gitlink = listed.stdout.split(|byte| *byte == 0).any(|entry| {
        let entry = String::from_utf8_lossy(entry);
        entry
            .split_once('\t')
            .is_some_and(|(meta, path)| meta.starts_with("160000 ") && path == wanted)
    });
    if !is_gitlink {
        return Ok(false);
    }
    if std::fs::symlink_metadata(file_abs.join(".git")).is_err() {
        return Ok(true);
    }
    // The submodule's own lane: vetted, pinned, and refused when it is not one.
    let head = crate::core::lane_git::git(file_abs)?
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .bounded_output()
        .map_err(|e| format!("Failed to read the submodule HEAD: {e}"))?;
    let sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
    if !head.status.success() || sha.len() < 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "cannot read the HEAD of the submodule `{wanted}`: {}",
            String::from_utf8_lossy(&head.stderr).trim()
        ));
    }
    let update = git_in(repo_path, true)?
        .args([
            "update-index",
            "--cacheinfo",
            &format!("160000,{sha},{wanted}"),
        ])
        .bounded_output()
        .map_err(|e| format!("Failed to run git update-index: {e}"))?;
    if update.status.success() {
        Ok(true)
    } else {
        Err(format!(
            "git update-index failed for `{wanted}`: {}",
            String::from_utf8_lossy(&update.stderr).trim()
        ))
    }
}

/// The first nested git repository at or under `path` (a `.git` entry), looking a
/// few levels down and skipping build output.
fn embedded_repository(path: &Path, depth: usize) -> Option<std::path::PathBuf> {
    if path.join(".git").exists() {
        return Some(path.to_path_buf());
    }
    if depth == 0 || !path.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(path).ok()?;
    for entry in entries.flatten().take(5_000) {
        let name = entry.file_name();
        if matches!(name.to_str(), Some("node_modules" | "target" | ".git")) {
            continue;
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(found) = embedded_repository(&entry.path(), depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

/// A repository a worker created inside its worktree is never staged by Kronn: it
/// would become a gitlink whose own configuration (the worker wrote it) Kronn's
/// later `git add -A` / `commit -a` would read. A gitlink the index already holds
/// (a real submodule) is fine.
fn refuse_new_embedded_repository(
    repo_path: &Path,
    clean_file: &str,
    file_abs: &Path,
) -> Result<(), String> {
    let Some(nested) = embedded_repository(file_abs, 4) else {
        return Ok(());
    };
    let relative = nested.strip_prefix(repo_path).unwrap_or(&nested);
    let known = git_in(repo_path, true)?
        .args(["ls-files", "--stage", "--"])
        .arg(relative)
        .bounded_output()
        .map(|output| String::from_utf8_lossy(&output.stdout).starts_with("160000 "))
        .unwrap_or(false);
    if known {
        return Ok(());
    }
    Err(format!(
        "EmbeddedRepository: `{clean_file}` contains a git repository the task created at `{}`; \
         Kronn does not stage a new embedded repository (its configuration is worker-written). \
         Remove its `.git` or leave it out of the commit.",
        relative.display()
    ))
}

/// `git` in `repo_path`: for a commit Kronn makes for a worker (`worker`), the
/// lane's pinned builder ([`crate::core::lane_git`]); for a commit the user asks
/// for in their own checkout, plain git with the repository's hooks. A commit the
/// user asks for in a Kronn-managed worktree (a discussion's, an agent's) is the
/// pinned, hook-less command too: that worktree is written by an agent, so its
/// hooks and `.git` pointer are not the user's.
fn git_in(repo_path: &Path, worker: bool) -> Result<std::process::Command, String> {
    if worker {
        return crate::core::lane_git::git(repo_path);
    }
    crate::core::lane_git::git_keeping_owner_hooks(repo_path)
}

/// Run `git commit <commit_args>` as the project's commit identity and return
/// the new short HEAD, optionally retaining the backend's data-directory lock in
/// Git and any hook descendants across backend death.
///
/// `hooks` is `None` for a commit the user asked for in their own checkout (the
/// repository's hooks run, as ever). `Some((hooks_policy, gate))` is a commit
/// Kronn makes for a worker (KT-1745): git runs no hook that lives in the
/// worktree, and the gating hook `gate` runs first as `hooks_policy` allows, its
/// refusal being a `HookRefused`. The identity reaches git as `GIT_AUTHOR_*` /
/// `GIT_COMMITTER_*`; no config is written (KT-1750).
fn run_commit_command(
    repo_path: &Path,
    commit_args: &[&str],
    data_dir_lock: Option<&std::fs::File>,
    hooks: Option<(crate::models::WorkerCommitHooks, &str)>,
    policy: &CommitPolicy,
    amend: bool,
) -> Result<String, String> {
    if let Some((hooks_policy, gate)) = hooks {
        crate::core::commit_hooks::run_gating_hook(repo_path, hooks_policy, gate)
            .map_err(|refusal| refusal.to_string())?;
    }
    // Only `git commit` executes hooks. Keep the inherited descriptor alive
    // through this spawn; descendants retain it until the whole hook tree exits.
    let child_lock = data_dir_lock
        .map(crate::core::config::inherit_data_dir_lock_for_child)
        .transpose()
        .map_err(|error| {
            format!("Failed to inherit data-directory lock for git commit: {error}")
        })?;
    let mut commit_command = git_in(repo_path, hooks.is_some())?;
    commit_command.args(commit_args);
    policy.apply_identity(&mut commit_command, amend);
    #[cfg(unix)]
    if let Some(child_lock) = child_lock.as_ref() {
        crate::core::config::inherit_data_dir_lock_on_command(&mut commit_command, child_lock);
    }
    #[cfg(windows)]
    if let Some(child_lock) = child_lock.as_ref() {
        crate::core::config::inherit_data_dir_lock_on_command(&mut commit_command, child_lock)
            .map_err(|error| {
                format!("Failed to attach data-directory lock to git commit: {error}")
            })?;
    }
    let commit_output = commit_command
        .bounded_output_within(COMMIT_TIMEOUT)
        .map_err(|e| format!("Failed to run git commit: {}", e))?;
    drop(child_lock);

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        return Err(format!("git commit failed: {}", stderr.trim()));
    }

    let hash_output = git_in(repo_path, hooks.is_some())?
        .args(["rev-parse", "--short", "HEAD"])
        .bounded_output()
        .map_err(|e| format!("Failed to get commit hash: {}", e))?;

    Ok(String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string())
}

/// Stage and commit explicit paths, optionally retaining the backend's
/// data-directory lock in Git and any hook descendants across backend death.
///
/// This is a commit Kronn makes for a worker: no hook of the worktree runs
/// (KT-1745), see [`run_git_commit_for_worker`].
pub fn run_git_commit_with_child_lock(
    repo_path: &Path,
    files: &[String],
    message: &str,
    amend: bool,
    sign: bool,
    data_dir_lock: Option<&std::fs::File>,
) -> Result<GitCommitResponse, String> {
    run_git_commit_for_worker(
        repo_path,
        files,
        message,
        amend,
        sign,
        data_dir_lock,
        crate::models::WorkerCommitHooks::default(),
    )
}

/// [`run_git_commit_with_child_lock`] under the project's worker commit hook
/// policy (KT-1745): git never resolves a hook inside the worktree, and the
/// `pre-commit` hook runs only as `policy` allows.
pub fn run_git_commit_for_worker(
    repo_path: &Path,
    files: &[String],
    message: &str,
    amend: bool,
    sign: bool,
    data_dir_lock: Option<&std::fs::File>,
    policy: crate::models::WorkerCommitHooks,
) -> Result<GitCommitResponse, String> {
    commit_explicit_paths(
        repo_path,
        files,
        message,
        amend,
        sign,
        data_dir_lock,
        Some(policy),
    )
}

fn commit_explicit_paths(
    repo_path: &Path,
    files: &[String],
    message: &str,
    amend: bool,
    sign: bool,
    data_dir_lock: Option<&std::fs::File>,
    hooks_policy: Option<crate::models::WorkerCommitHooks>,
) -> Result<GitCommitResponse, String> {
    // git add each file individually, skip missing files gracefully
    let clean_files = files
        .iter()
        .map(|file| file.trim_matches('"'))
        .collect::<Vec<_>>();
    let worker = hooks_policy.is_some();
    if stage_explicit_paths(repo_path, &clean_files, worker)? == 0 {
        return Err("No files could be staged".to_string());
    }

    let policy = CommitPolicy::for_repo(repo_path);

    let mut commit_args = vec!["commit"];
    if amend {
        commit_args.push("--amend");
    }
    if policy.signoff {
        commit_args.push("-s");
    }
    if sign {
        commit_args.push("-S");
    } else {
        commit_args.push("--no-gpg-sign");
    }
    commit_args.push("-m");
    commit_args.push(message);
    // The index may contain unrelated entries staged earlier by a CLI worker.
    // `--only -- <paths>` makes the explicit inventory authoritative: Git
    // commits those working-tree paths and cannot smuggle another staged file
    // into the mediated commit.
    commit_args.push("--only");
    commit_args.push("--");
    commit_args.extend(clean_files);

    let hash = run_commit_command(
        repo_path,
        &commit_args,
        data_dir_lock,
        hooks_policy.map(|hooks_policy| (hooks_policy, "pre-commit")),
        &policy,
        amend,
    )?;

    Ok(GitCommitResponse {
        hash,
        message: message.to_string(),
    })
}

/// Whether `repo_path` is in the middle of a merge: git keeps `MERGE_HEAD` in
/// the worktree's own git dir, so this holds per linked worktree.
pub fn merge_in_progress(repo_path: &Path) -> bool {
    crate::core::lane_git::git(repo_path)
        .and_then(|mut git| {
            git.args(["rev-parse", "-q", "--verify", "MERGE_HEAD"])
                .bounded_output()
                .map_err(|error| error.to_string())
        })
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a `git diff`-style command on a worker's worktree and split its
/// NUL-separated path list.
fn git_path_list(repo_path: &Path, args: &[&str]) -> Result<Vec<String>, String> {
    let mut git = crate::core::lane_git::git(repo_path)?;
    if let Some((subcommand, rest)) = args.split_first() {
        git.arg(subcommand)
            .args(crate::core::lane_git::no_external_diff(subcommand))
            .args(crate::core::lane_git::ignore_submodules(
                repo_path, subcommand,
            ))
            .args(rest);
    }
    let output = git
        .bounded_output()
        .map_err(|e| format!("Failed to run git {}: {}", args.join(" "), e))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

/// Paths the side being merged in (`MERGE_HEAD`) changed relative to each merge
/// base. Several bases (a criss-cross history) widen the set, never narrow it;
/// with none (unrelated histories) every path where the two sides differ counts.
fn paths_changed_by_merged_side(repo_path: &Path) -> Result<Vec<String>, String> {
    let output = crate::core::lane_git::git(repo_path)?
        .args(["merge-base", "--all", "HEAD", "MERGE_HEAD"])
        .bounded_output()
        .map_err(|e| format!("Failed to run git merge-base: {}", e))?;
    let mut bases = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|base| !base.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if bases.is_empty() {
        bases.push("HEAD".to_string());
    }
    let mut changed = Vec::new();
    for base in &bases {
        changed.extend(git_path_list(
            repo_path,
            &[
                "diff",
                "--name-only",
                "-z",
                "--no-renames",
                base.as_str(),
                "MERGE_HEAD",
            ],
        )?);
    }
    Ok(changed)
}

/// Finish the merge in progress with its own parents.
///
/// `git commit --only -- <paths>` is refused during a merge, and the way out a
/// worker finds on its own — erasing `MERGE_HEAD` — yields a single-parent commit
/// whose tree is right but whose history makes the target's files look added on
/// both sides at integration. So the merge is committed here, whole, with
/// `MERGE_HEAD` as its second parent.
///
/// A merge commit cannot be limited to paths, so the explicit inventory guards
/// it differently: every conflict must be resolved (or named, and thereby
/// staged by Kronn), and no staged path may fall outside what the merge brings
/// in plus the named files. Refusals leave the merge state untouched.
pub fn run_git_merge_commit_with_child_lock(
    repo_path: &Path,
    files: &[String],
    message: &str,
    sign: bool,
    data_dir_lock: Option<&std::fs::File>,
) -> Result<GitCommitResponse, String> {
    run_git_merge_commit_for_worker(
        repo_path,
        files,
        message,
        sign,
        data_dir_lock,
        crate::models::WorkerCommitHooks::default(),
    )
}

/// [`run_git_merge_commit_with_child_lock`] under the project's worker commit
/// hook policy (KT-1745).
pub fn run_git_merge_commit_for_worker(
    repo_path: &Path,
    files: &[String],
    message: &str,
    sign: bool,
    data_dir_lock: Option<&std::fs::File>,
    hooks_policy: crate::models::WorkerCommitHooks,
) -> Result<GitCommitResponse, String> {
    let clean_files = files
        .iter()
        .map(|file| file.trim_matches('"'))
        .collect::<Vec<_>>();
    if !merge_in_progress(repo_path) {
        return Err("no merge in progress in this worktree".to_string());
    }

    // A doc conflict on nothing but citation line anchors is not the worker's to
    // resolve by hand (KT-1321): the target's text is taken, then re-anchored.
    let resolved_docs =
        crate::core::docs_citations::merge::resolve_anchor_only_conflicts(repo_path)
            .unwrap_or_else(|error| {
                tracing::warn!("anchor-only doc conflicts left to the worker: {error}");
                Vec::new()
            });

    let unresolved = git_path_list(repo_path, &["diff", "--name-only", "-z", "--diff-filter=U"])?
        .into_iter()
        .filter(|path| !clean_files.contains(&path.as_str()))
        .collect::<Vec<_>>();
    if !unresolved.is_empty() {
        return Err(format!(
            "refused: the merge in progress still has unresolved conflicts in: {}. Resolve them in the \
             files, then name each resolved file in `files` so Kronn stages it and finishes the merge \
             with both parents. Do not delete MERGE_HEAD or commit by hand: a single-parent commit \
             makes the target's files look added on both sides at integration.",
            unresolved.join(", ")
        ));
    }

    if stage_explicit_paths(repo_path, &clean_files, true)? == 0 {
        return Err("No files could be staged".to_string());
    }

    // What the merge legitimately changes in the index: the paths the merged-in
    // side changed since the histories diverged (a three-way merge leaves a path
    // at our version whenever the other side did not touch it). Anything else
    // staged was put there by hand and is not part of what the worker named. This
    // runs before the docs are settled, so nothing the settling stages can hide
    // behind it.
    let brought_by_merge = paths_changed_by_merged_side(repo_path)?;
    let resolved_paths: Vec<String> = resolved_docs
        .iter()
        .map(|doc| doc.to_string_lossy().into_owned())
        .collect();
    let foreign = git_path_list(
        repo_path,
        &[
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--no-renames",
            "HEAD",
        ],
    )?
    .into_iter()
    .filter(|path| {
        !brought_by_merge.contains(path)
            && !clean_files.contains(&path.as_str())
            && !resolved_paths.contains(path)
    })
    .collect::<Vec<_>>();
    if !foreign.is_empty() {
        return Err(format!(
            "refused: these staged paths belong neither to the merge nor to `files`: {}. Unstage them \
             (`git restore --staged -- <path>`) or name them in `files`; the merge state was left as it was.",
            foreign.join(", ")
        ));
    }

    // The citations the merge shifted are re-anchored into the same commit, so
    // a worker needs no `make docs-reanchor` in its sandbox. This reads the merged
    // index only and refuses to rewrite a doc that carries unstaged work.
    use crate::core::docs_citations::merge::SettleError;
    let settled =
        match crate::core::docs_citations::merge::reanchor_and_stage(repo_path, resolved_docs) {
            Ok(docs) => Some(docs),
            Err(SettleError::Failed(error)) => {
                tracing::warn!("doc citations not re-anchored by the merge: {error}");
                None
            }
            Err(SettleError::Refused { message, .. }) => return Err(message),
            Err(error @ SettleError::Unplaced { .. }) => return Err(format!("refused: {error}")),
        };
    let message = match settled.as_ref().and_then(|docs| docs.summary()) {
        Some(note) => format!("{message}\n\n{note}"),
        None => message.to_string(),
    };
    let message = message.as_str();

    let policy = CommitPolicy::for_repo(repo_path);

    let mut commit_args = vec!["commit"];
    if policy.signoff {
        commit_args.push("-s");
    }
    commit_args.push(if sign { "-S" } else { "--no-gpg-sign" });
    commit_args.extend(["-m", message]);
    let hash = run_commit_command(
        repo_path,
        &commit_args,
        data_dir_lock,
        Some((hooks_policy, "pre-merge-commit")),
        &policy,
        false,
    )?;

    let has_second_parent = git_in(repo_path, true)?
        .args(["rev-parse", "-q", "--verify", "HEAD^2"])
        .bounded_output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has_second_parent {
        return Err(format!(
            "the merge was committed as {hash} but without its second parent; do not deliver it"
        ));
    }

    Ok(GitCommitResponse {
        hash,
        message: message.to_string(),
    })
}

/// The HTTPS URL of a GitHub remote (SSH or HTTPS form), without any
/// credential in it.
pub fn github_https_url(remote_url: &str) -> Option<String> {
    let path = remote_url
        .strip_prefix("git@github.com:")
        .or_else(|| remote_url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| {
            let rest = remote_url.strip_prefix("https://")?;
            let host_and_path = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
            host_and_path.strip_prefix("github.com/")
        })?;
    (!path.is_empty()).then(|| format!("https://github.com/{path}"))
}

/// What pushes the local branch `branch` to the branch of the same name, both named in full
/// (KT-2084): a short name is a refspec git resolves itself, and may reach another ref.
fn branch_refspec(branch: &str) -> String {
    format!("refs/heads/{branch}:refs/heads/{branch}")
}

/// The authenticated `git push`: the token never appears in an argument or
/// in a variable a hook reads by name. It travels as an `http.extraHeader`
/// scoped to github.com through `GIT_CONFIG_*`, with hooks off, no
/// credential helper (a repository's could receive it) and TLS verified.
pub fn authenticated_push_command(
    repo_path: &Path,
    https_url: &str,
    branch: &str,
    token: &str,
) -> Result<std::process::Command, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let header = format!(
        "Authorization: Basic {}",
        STANDARD.encode(format!("x-access-token:{token}"))
    );
    // KT-1745 — the repository is the registered one (the pointer of a managed
    // worktree is never followed): a configuration the worker wrote cannot pick
    // the proxy, the CA, a `url.*.insteadOf` or a program this push, which
    // carries the token, goes through.
    let mut cmd = crate::core::lane_git::git(repo_path)?;
    cmd.args([
        "-c",
        "credential.helper=",
        "-c",
        "http.sslVerify=true",
        "push",
        "--no-verify",
        "-u",
        "--end-of-options",
        https_url,
        &branch_refspec(branch),
    ])
    .env("GIT_CONFIG_COUNT", "1")
    .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraHeader")
    .env("GIT_CONFIG_VALUE_0", header);
    Ok(cmd)
}

/// Push the current branch to origin. `github_env` is the project's GitHub
/// variables (`github_connection::env_for_launch`): its token, when the
/// project is connected and the remote is on GitHub, authenticates the push.
pub fn run_git_push(
    repo_path: &Path,
    github_env: &[(String, String)],
) -> Result<GitPushResponse, String> {
    // KT-1745 — a push from a managed worktree (a discussion's) goes through the
    // lane builder too: the repository is pinned, never read from the worktree's
    // `.git` file, and no hook runs.
    let branch_output = crate::core::lane_git::git(repo_path)?
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["branch", "--show-current"])
        .bounded_output()
        .map_err(|e| format!("Failed to get branch: {}", e))?;

    let branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();
    if branch.is_empty() {
        return Err("Cannot determine current branch (detached HEAD?)".to_string());
    }
    // Read from the checkout: a name git would take as an option is never pushed (KT-2084).
    crate::core::lane_git::check_branch_name(&branch)?;

    let token = github_env
        .iter()
        .find(|(name, value)| name == "GH_TOKEN" && !value.is_empty())
        .map(|(_, value)| value.as_str());
    let authenticated = match token {
        Some(token) => {
            let remote_url = crate::core::lane_git::git(repo_path)?
                .env("GIT_TERMINAL_PROMPT", "0")
                .args(["remote", "get-url", "origin"])
                .bounded_output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            github_https_url(&remote_url).map(|url| (url, token))
        }
        None => None,
    };

    let mut cmd = match authenticated {
        Some((url, token)) => authenticated_push_command(repo_path, &url, &branch, token)?,
        None => {
            // The user's own credentials (SSH agent, credential helper).
            let mut cmd = crate::core::lane_git::git_keeping_owner_hooks(repo_path)?;
            cmd.args([
                "push",
                "-u",
                "--end-of-options",
                "origin",
                &branch_refspec(&branch),
            ]);
            cmd
        }
    };
    // Never let git block the thread on an interactive prompt (SSH passphrase,
    // username/password): this runs on a blocking thread, so a prompt would hold
    // it until the push's time bound. Fail fast instead. The low-speed envs abort
    // an HTTPS push stalled under 1 KB/s for 60s (dead network / dead proxy).
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1024")
        .env("GIT_HTTP_LOW_SPEED_TIME", "60");
    let push_output = cmd.bounded_output_within(push_timeout()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::TimedOut {
            format!("git push was stopped: {e}. A push that needs longer runs from a terminal.")
        } else {
            format!("Failed to run git push: {e}")
        }
    })?;

    if push_output.status.success() {
        let stdout = String::from_utf8_lossy(&push_output.stdout);
        let stderr = String::from_utf8_lossy(&push_output.stderr);
        let msg = if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        Ok(GitPushResponse {
            success: true,
            message: msg,
        })
    } else {
        let stderr = String::from_utf8_lossy(&push_output.stderr);
        Ok(GitPushResponse {
            success: false,
            message: stderr.trim().to_string(),
        })
    }
}

/// Validate a command against the allowlist before execution.
/// Returns Ok(()) if allowed, Err(message) if blocked.
pub fn validate_exec_command(cmd: &str) -> Result<(), String> {
    const DENY_MSG: &str = "Command not allowed. Only read-only commands are permitted.";

    // Block shell metacharacters in the full command
    // These enable injection: ; | & $() `` > < \n
    for ch in [';', '|', '&', '>', '<', '`', '\n'] {
        if cmd.contains(ch) {
            return Err(DENY_MSG.to_string());
        }
    }
    if cmd.contains("$(") {
        return Err(DENY_MSG.to_string());
    }

    let first_word = cmd.split_whitespace().next().unwrap_or("");

    // Allowlist of safe commands
    // No `env`: it would print the process environment (KT-1006).
    const ALLOWED_CMDS: &[&str] = &[
        "git", "ls", "find", "wc", "head", "tail", "cat", "echo", "date", "whoami", "pwd", "npm",
        "node", "cargo", "python3", "pnpm", "which", "grep", "rg", "tree", "file", "stat", "du",
    ];

    if !ALLOWED_CMDS.contains(&first_word) {
        return Err(DENY_MSG.to_string());
    }

    let parts: Vec<&str> = cmd.split_whitespace().collect();

    // For version-only commands, require --version as the sole argument
    const VERSION_ONLY: &[&str] = &["npm", "node", "cargo", "python3", "pnpm"];
    if VERSION_ONLY.contains(&first_word) && (parts.len() != 2 || parts[1] != "--version") {
        return Err(DENY_MSG.to_string());
    }

    // Block dangerous git subcommands
    if first_word == "git" && parts.len() >= 2 {
        let subcommand = parts[1];
        const BLOCKED_GIT: &[&str] = &[
            "push", "rm", "mv", "clean", "checkout", "rebase", "merge", "pull", "fetch", "clone",
            "init", "remote", "config",
        ];
        if BLOCKED_GIT.contains(&subcommand) {
            return Err(DENY_MSG.to_string());
        }
        // Block git reset --hard specifically
        if subcommand == "reset" && parts.contains(&"--hard") {
            return Err(DENY_MSG.to_string());
        }
        // `--no-index` diffs any two files on disk; `--output` writes a file.
        if parts
            .iter()
            .any(|part| *part == "--no-index" || part.starts_with("--output"))
        {
            return Err(DENY_MSG.to_string());
        }
        // Only allow known safe git subcommands
        const SAFE_GIT: &[&str] = &[
            "status", "diff", "log", "branch", "stash", "show", "blame", "shortlog", "reset",
        ];
        if !SAFE_GIT.contains(&subcommand) {
            return Err(DENY_MSG.to_string());
        }
    }

    // Block rm and mv even if somehow reached (belt and suspenders)
    if first_word == "rm" || first_word == "mv" {
        return Err(DENY_MSG.to_string());
    }

    Ok(())
}

/// Commands whose arguments name files to read: every path among them must
/// stay inside the project (symlinks followed).
const FILE_READING_CMDS: &[&str] = &[
    "cat", "head", "tail", "find", "stat", "grep", "rg", "wc", "du", "file", "tree", "ls",
];

/// `find` primaries that run a program or write/delete files.
const FIND_ACTIONS: &[&str] = &[
    "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprint0", "-fprintf", "-fls",
];

/// Split a command line the way a POSIX shell quotes words, without any
/// expansion: no variables, no `~`, no globs. Errors on an unclosed quote.
pub fn split_exec_words(cmd: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(inner) => word.push(inner),
                        None => return Err("Unclosed single quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(escaped @ ('"' | '\\' | '$' | '`')) => word.push(escaped),
                            Some(other) => {
                                word.push('\\');
                                word.push(other);
                            }
                            None => return Err("Unclosed double quote".into()),
                        },
                        Some(inner) => word.push(inner),
                        None => return Err("Unclosed double quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(escaped) = chars.next() {
                    word.push(escaped);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// The argv `run_exec` will start, refused when a file-reading command names
/// a path outside `repo_path` or `find` asks to run or write something.
pub fn exec_argv(repo_path: &Path, cmd: &str) -> Result<Vec<String>, String> {
    let words = split_exec_words(cmd)?;
    let Some(program) = words.first() else {
        return Err("Empty command".into());
    };
    if !FILE_READING_CMDS.contains(&program.as_str()) {
        return Ok(words);
    }
    if program == "find" {
        if let Some(action) = words.iter().find(|w| FIND_ACTIONS.contains(&w.as_str())) {
            return Err(format!("find {action} is not allowed in the terminal"));
        }
    }
    // grep/rg take their pattern as the first operand unless -e/-f gives it.
    let pattern_flag = |w: &String| {
        matches!(w.as_str(), "-e" | "-f" | "--regexp" | "--file")
            || w.starts_with("--regexp=")
            || w.starts_with("--file=")
    };
    let mut skip_pattern =
        matches!(program.as_str(), "grep" | "rg") && !words.iter().any(pattern_flag);
    for word in &words[1..] {
        let candidate = match word.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => value,
            _ if word.starts_with('-') && word.len() > 1 => continue,
            _ => word.as_str(),
        };
        if skip_pattern && !word.starts_with('-') {
            skip_pattern = false;
            continue;
        }
        if candidate.is_empty() {
            continue;
        }
        crate::core::fs_guard::resolve_contained_read(repo_path, Path::new(candidate)).map_err(
            |_| format!("`{candidate}` is outside the project; the terminal only reads inside it"),
        )?;
    }
    Ok(words)
}

/// `git` typed in the panel's terminal for a managed worktree (KT-2084): the pinned, hardened
/// command of every other panel call, so nothing the worktree's `.git` or configuration names
/// is run. `None` for any other program and anywhere else (the owner's checkout, a declared
/// workspace outside `.kronn/worktrees`); `Err` for a managed worktree Kronn refuses.
fn managed_worktree_git(
    repo_path: &Path,
    argv: &[String],
) -> Result<Option<std::process::Command>, String> {
    let Some(("git", args)) = argv
        .split_first()
        .map(|(program, args)| (program.as_str(), args))
    else {
        return Ok(None);
    };
    let lane = crate::core::lane_git::LaneGit::resolve(repo_path)?;
    if !lane.is_pinned() {
        return Ok(None);
    }
    let mut command = lane.command();
    if let Some((subcommand, rest)) = args.split_first() {
        command
            .arg(subcommand)
            .args(crate::core::lane_git::no_external_diff(subcommand))
            .args(crate::core::lane_git::ignore_submodules(
                repo_path, subcommand,
            ))
            .args(rest);
    }
    Ok(Some(command))
}

/// The process `run_exec` starts for `argv` when it is not [`managed_worktree_git`].
fn plain_exec_command(
    repo_path: &Path,
    argv: &[String],
    github_env: &[(String, String)],
) -> std::process::Command {
    let mut command = sync_cmd(&argv[0], crate::core::child_env::ChildRoute::ProjectExec);
    command.args(&argv[1..]).current_dir(repo_path);
    crate::core::child_env::isolate_with_github(
        &mut command,
        crate::core::child_env::ChildRoute::ProjectExec,
        github_env,
    );
    command
}

/// The exact process `run_exec` starts: argv without a shell, a built
/// environment, the project as working directory.
/// `github_env` is the project's GitHub variables when it is connected (D2).
pub fn exec_command(
    repo_path: &Path,
    cmd: &str,
    github_env: &[(String, String)],
) -> Result<std::process::Command, String> {
    let argv = exec_argv(repo_path, cmd)?;
    Ok(match managed_worktree_git(repo_path, &argv)? {
        Some(git) => git,
        None => plain_exec_command(repo_path, &argv, github_env),
    })
}

/// Execute an allow-listed command in the given directory, without a shell
/// and with a built environment.
/// The caller MUST call `validate_exec_command` before this function.
pub fn run_exec(
    repo_path: &Path,
    cmd: &str,
    github_env: &[(String, String)],
) -> Result<ExecResponse, String> {
    let argv = exec_argv(repo_path, cmd)?;
    // Both within git's time bound, the process group killed past it (KT-2084): a FIFO in the
    // checkout makes `cat` or `grep` wait for ever, holding the request and its permit.
    let output = match managed_worktree_git(repo_path, &argv)? {
        Some(mut git) => git.bounded_output(),
        None => plain_exec_command(repo_path, &argv, github_env).bounded_output(),
    }
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::TimedOut {
            format!("The command was stopped: {e}")
        } else {
            format!("Failed to execute: {e}")
        }
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() && (stderr.contains("not found") || stderr.contains("No such file"))
    {
        stderr.push_str(
            "\n\nCommand not found. The terminal runs inside the Docker container \
            with access to host binaries (/usr/bin). If the tool is installed elsewhere, \
            check your PATH or install it in the container.",
        );
    }

    Ok(ExecResponse {
        stdout,
        stderr,
        exit_code: output.status.code().unwrap_or(-1),
    })
}

/// Detect the git hosting provider from the remote origin URL.
/// Returns "github", "gitlab", or "unknown".
pub fn detect_provider(repo_path: &Path) -> &'static str {
    let output = crate::core::lane_git::git_or_refuse(repo_path)
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_path)
        .bounded_output();
    let url = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_lowercase(),
        _ => return "unknown",
    };

    // Detect by domain in the remote URL (handles SSH, HTTPS, and self-hosted)
    // SSH format: git@github.com:user/repo.git
    // HTTPS format: https://github.com/user/repo.git
    if url.contains("github.com") {
        "github"
    } else if url.contains("gitlab") {
        // Matches gitlab.com, gitlab.company.com, self-hosted.com/gitlab/...
        "gitlab"
    } else {
        "unknown"
    }
}

/// `gh` or `glab` in a repository. They start git there themselves, so they
/// get the git-host environment, never the backend's (KT-1006); `gh` also
/// gets the project's GitHub variables (its §4.5 connection), nothing else.
pub fn host_cli_command(
    program: &str,
    repo_path: &Path,
    github_env: &[(String, String)],
) -> std::process::Command {
    use crate::core::child_env::{self, ChildRoute};
    let mut command = sync_cmd(program, ChildRoute::GitHost);
    command.current_dir(repo_path);
    if program == "gh" {
        crate::core::github_connection::apply_launch_env(&mut command, github_env);
    }
    child_env::seal(&mut command, ChildRoute::GitHost, child_env::GITHUB_ENV);
    // They start git themselves (`gh pr create` runs `git status`): in a managed
    // worktree that git is pinned to the registered repository, with no hook and
    // no file-system monitor (KT-1745), never to what the worker's `.git` names.
    crate::core::lane_git::pin_foreign_command(&mut command, repo_path);
    command
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static PUSH_TIMEOUT_FOR_TESTS: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

/// Shortens, on this thread, the bound of a push and of the forge CLI: a test of one that
/// never ends.
#[cfg(any(test, feature = "test-support"))]
pub fn set_push_timeout_for_tests(timeout: Option<std::time::Duration>) {
    PUSH_TIMEOUT_FOR_TESTS.with(|bound| bound.set(timeout));
}

/// How long a push, then `gh pr create` / `glab mr create`, may run (KT-2084): past it the
/// process group is killed, so neither holds a blocking thread for ever.
pub fn push_timeout() -> std::time::Duration {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(bound) = PUSH_TIMEOUT_FOR_TESTS.with(std::cell::Cell::get) {
        return bound;
    }
    PUSH_TIMEOUT
}

/// Why `program` (`gh`, `glab`) gave no result: stopped at Kronn's bound, or never started.
fn forge_error(program: &str, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::TimedOut {
        format!("{program} was stopped: {error}")
    } else {
        format!("Failed to run {program}: {error} (is {program} installed?)")
    }
}

/// Create a pull/merge request via gh (GitHub) or glab (GitLab) CLI.
/// Automatically pushes the current branch first if it has no upstream.
/// `github_env` is the project's GitHub variables (`env_for_launch`).
pub fn run_create_pr(
    repo_path: &Path,
    title: &str,
    body: &str,
    base: &str,
    github_env: &[(String, String)],
) -> Result<String, String> {
    // The base reaches `gh` / `glab` as an option's value: only a name git takes as a branch
    // (KT-2084). The head is the current branch, which they read through the pinned git.
    crate::core::lane_git::check_branch_name(base)?;
    // Ensure the branch is pushed before creating the PR
    let has_upstream = crate::core::lane_git::git_or_refuse(repo_path)
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .current_dir(repo_path)
        .bounded_output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !has_upstream {
        let push_result = run_git_push(repo_path, github_env)?;
        if !push_result.success {
            return Err(format!(
                "Auto-push failed before PR creation: {}",
                push_result.message
            ));
        }
    }

    let provider = detect_provider(repo_path);

    let output = match provider {
        "gitlab" => {
            let mut args = vec![
                "mr",
                "create",
                "--title",
                title,
                "--target-branch",
                base,
                "--no-editor",
            ];
            if !body.is_empty() {
                args.push("--description");
                args.push(body);
            }
            host_cli_command("glab", repo_path, github_env)
                .args(&args)
                .bounded_output_within(push_timeout())
                .map_err(|e| forge_error("glab", &e))?
        }
        _ => {
            // Default to GitHub
            let mut args = vec!["pr", "create", "--title", title, "--base", base];
            if body.is_empty() {
                args.push("--fill");
            } else {
                args.push("--body");
                args.push(body);
            }
            host_cli_command("gh", repo_path, github_env)
                .args(&args)
                .bounded_output_within(push_timeout())
                .map_err(|e| forge_error("gh", &e))?
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let cmd = if provider == "gitlab" {
            "glab mr create"
        } else {
            "gh pr create"
        };
        return Err(format!("{} failed: {}", cmd, stderr.trim()));
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    forget_pr_urls(repo_path);
    Ok(url)
}

/// How long a branch's PR lookup is reused. `gh pr view` is a network call and
/// git-status runs each time a discussion opens (KT-983): uncached, it added
/// ~0.7 s to every open, and far more on a slow network.
const PR_URL_TTL: std::time::Duration = std::time::Duration::from_secs(120);
/// A lookup that takes longer than this gives up: a missing PR link is better
/// than a discussion that does not open.
const PR_URL_LOOKUP_LIMIT: std::time::Duration = std::time::Duration::from_secs(4);

type PrUrlCache =
    std::collections::HashMap<(std::path::PathBuf, String), (Option<String>, std::time::Instant)>;
pub static PR_URLS: std::sync::LazyLock<std::sync::Mutex<PrUrlCache>> =
    std::sync::LazyLock::new(Default::default);

/// [`check_pr_url`], reused for [`PR_URL_TTL`] per repository and branch,
/// "no PR" included.
pub fn cached_pr_url(
    repo_path: &Path,
    branch: &str,
    github_env: &[(String, String)],
) -> Option<String> {
    let key = (repo_path.to_path_buf(), branch.to_string());
    if let Ok(cache) = PR_URLS.lock() {
        if let Some((url, at)) = cache.get(&key) {
            if at.elapsed() < PR_URL_TTL {
                return url.clone();
            }
        }
    }
    let url = check_pr_url(repo_path, branch, github_env);
    if let Ok(mut cache) = PR_URLS.lock() {
        cache.retain(|_, (_, at)| at.elapsed() < PR_URL_TTL);
        cache.insert(key, (url.clone(), std::time::Instant::now()));
    }
    url
}

/// The PR link a previous lookup found, without ever starting a new one.
fn known_pr_url(repo_path: &Path, branch: &str) -> Option<String> {
    let key = (repo_path.to_path_buf(), branch.to_string());
    let cache = PR_URLS.lock().ok()?;
    let (url, at) = cache.get(&key)?;
    (at.elapsed() < PR_URL_TTL).then(|| url.clone()).flatten()
}

/// Drop what is known about this repository's PRs: Kronn just created one.
pub fn forget_pr_urls(repo_path: &Path) {
    if let Ok(mut cache) = PR_URLS.lock() {
        cache.retain(|(repo, _), _| repo != repo_path);
    }
}

/// Run `command`, giving up after `limit`.
pub fn output_within(
    mut command: std::process::Command,
    limit: std::time::Duration,
) -> Option<std::process::Output> {
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if started.elapsed() < limit => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Check if an open PR/MR exists for a branch.
pub fn check_pr_url(
    repo_path: &Path,
    branch: &str,
    github_env: &[(String, String)],
) -> Option<String> {
    // The branch is read from the checkout: a name `gh` or `glab` would take as an option is
    // never looked up (KT-2084).
    crate::core::lane_git::check_branch_name(branch).ok()?;
    let command = pr_lookup_command(repo_path, branch, github_env);
    let output = output_within(command, PR_URL_LOOKUP_LIMIT)?;
    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if url.is_empty() {
            None
        } else {
            Some(url)
        }
    } else {
        None
    }
}

/// The `gh pr view` / `glab mr view` process [`check_pr_url`] starts.
fn pr_lookup_command(
    repo_path: &Path,
    branch: &str,
    github_env: &[(String, String)],
) -> std::process::Command {
    match detect_provider(repo_path) {
        "gitlab" => {
            let mut command = host_cli_command("glab", repo_path, github_env);
            command.args([
                "mr", "view", branch, "--json", "web_url", "--jq", ".web_url",
            ]);
            command
        }
        _ => {
            let mut command = host_cli_command("gh", repo_path, github_env);
            command.args(["pr", "view", branch, "--json", "url", "--jq", ".url"]);
            command
        }
    }
}

/// Most bytes of a PR template Kronn reads: a forge takes far less in a description.
const PR_TEMPLATE_CAP: u64 = 256 * 1024;

/// Read the PR/MR template from the project, if one exists: a regular file inside
/// `repo_path`, under [`PR_TEMPLATE_CAP`], never through a link or from a FIFO (KT-2084).
pub fn read_pr_template(repo_path: &Path) -> Option<String> {
    let candidates = [
        // GitHub
        ".github/pull_request_template.md",
        ".github/PULL_REQUEST_TEMPLATE.md",
        ".github/PULL_REQUEST_TEMPLATE/default.md",
        "docs/pull_request_template.md",
        "PULL_REQUEST_TEMPLATE.md",
        // GitLab
        ".gitlab/merge_request_templates/Default.md",
        ".gitlab/merge_request_templates/default.md",
    ];
    for candidate in &candidates {
        let path = repo_path.join(candidate);
        let read = crate::core::fs_guard::guarded_read_head(repo_path, &path, PR_TEMPLATE_CAP + 1);
        let Ok(Some(bytes)) = read else { continue };
        if bytes.len() as u64 > PR_TEMPLATE_CAP {
            continue;
        }
        if let Ok(content) = String::from_utf8(bytes) {
            if !content.trim().is_empty() {
                return Some(content);
            }
        }
    }
    None
}

/// The branch checked out in `repo_path`, read within git's time bound (KT-2084); empty when
/// detached or when git gave none.
pub fn current_branch_name(repo_path: &Path) -> String {
    crate::core::lane_git::git_or_refuse(repo_path)
        .args(["branch", "--show-current"])
        .current_dir(repo_path)
        .bounded_output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Default Kronn PR template when no project template exists.
pub fn default_pr_template(branch: &str) -> String {
    format!(
        "## Summary

<!-- Describe what this PR does -->

## Changes

<!-- List the main changes -->
-

## Branch: `{branch}`

---
*Created via [Kronn](https://github.com/DocRoms/Kronn)*",
        branch = branch
    )
}
