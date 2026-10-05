//! Shared git operation helpers used by both project and discussion endpoints.

use crate::core::cmd::sync_cmd;
use crate::models::*;
use std::path::Path;

const PROJECT_GIT_GRAPH_LIMIT: usize = 80;
pub(crate) const GIT_COMMIT_PAGE_DEFAULT: u32 = 40;
pub(crate) const GIT_COMMIT_PAGE_MAX: u32 = 100;

/// Parse `git diff --name-status <base>...HEAD` output into structured file
/// statuses. Each non-rename line is `<code>\t<path>`; rename/copy lines are
/// `R<score>\t<old>\t<new>` (we keep the destination path).
pub(crate) fn parse_committed_diff(diff_output: &str) -> Vec<GitFileStatus> {
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
    let count = crate::core::cmd::git_cmd()
        .args(["rev-list", "--count", range, "--"])
        .current_dir(repo_path)
        .output()
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
    let log = crate::core::cmd::git_cmd()
        .args([
            "log",
            "--format=%H%x1f%h%x1f%an%x1f%at%x1f%s",
            skip.as_str(),
            max_count.as_str(),
            range,
            "--",
        ])
        .current_dir(repo_path)
        .output()
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
    let diff = crate::core::cmd::git_cmd()
        .args(["diff", "--name-status", &range, "--"])
        .current_dir(repo_path)
        .output()
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
    let output = crate::core::cmd::git_cmd()
        .args(["diff", &range, "--", file_path])
        .current_dir(repo_path)
        .output()
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
pub(crate) fn run_git_status_without_commit_evidence(
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
        let output = crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git: {}", e))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };

    let run_with_status = |args: &[&str]| -> (String, bool) {
        match crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(repo_path)
            .output()
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
    let status_output = run(&["status", "--porcelain=v1", "-u"])?;
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
    crate::core::cmd::git_cmd()
        .args(args)
        .current_dir(repo_path)
        .output()
        .map_err(|error| format!("Failed to run git: {error}"))
}

fn git_output(repo_path: &Path, args: &[&str]) -> Result<String, String> {
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

    let dirty = git_output(repo_path, &["status", "--porcelain=v1", "-uall"])?;
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
pub(crate) fn normalize_git_remote_web_url(raw: &str) -> Option<String> {
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
    let output = crate::core::cmd::git_cmd()
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_path)
        .output()
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
        crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(repo_path)
            .output()
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
        crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(repo_path)
            .output()
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
        crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(repo_path)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    };

    // Unstaged diff
    let unstaged = run_diff(&["diff", "--", file_path]);
    // Staged diff
    let staged = run_diff(&["diff", "--cached", "--", file_path]);

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

fn parse_git_blame_porcelain(output: &str) -> Vec<GitBlameLine> {
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
    let output = crate::core::cmd::git_cmd()
        .args(["blame", "--line-porcelain", "--", file_path])
        .current_dir(repo_path)
        .output()
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
const COMMIT_BRANCHES_CAP: usize = 12;

/// A commit-ish is only ever accepted as a hex hash here. It is interpolated
/// into a git invocation, and blame only ever hands us hashes — so anything
/// else is either a bug or an attempt, and both deserve a refusal rather than
/// a best effort.
fn valid_commit_ish(sha: &str) -> bool {
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
    let output = crate::core::cmd::git_cmd()
        .args(["show", "--no-patch", &format!("--format={format}"), sha])
        .current_dir(repo_path)
        .output()
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
    let files = crate::core::cmd::git_cmd()
        .args([
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "--root",
            sha,
        ])
        .current_dir(repo_path)
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count() as u32
        })
        .unwrap_or(0);

    let mut branches: Vec<String> = crate::core::cmd::git_cmd()
        .args([
            "branch",
            "-a",
            "--contains",
            sha,
            "--format=%(refname:short)",
        ])
        .current_dir(repo_path)
        .output()
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
const COMMIT_PATCH_MAX_BYTES: usize = 400 * 1024;

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

    let header = crate::core::cmd::git_cmd()
        .args(["show", "--no-patch", "--format=%H%x1f%h%x1f%s", sha])
        .current_dir(repo_path)
        .output()
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
    let is_root = crate::core::cmd::git_cmd()
        .args(["rev-list", "--parents", "-n", "1", sha])
        .current_dir(repo_path)
        .output()
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .count()
                <= 1
        })
        .unwrap_or(false);

    // `-m` splits a merge into one patch per parent instead of printing nothing.
    let patch_out = crate::core::cmd::git_cmd()
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
        .output()
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

    let files_changed = crate::core::cmd::git_cmd()
        .args([
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "--root",
            sha,
        ])
        .current_dir(repo_path)
        .output()
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
    run_git_commit_with_child_lock(repo_path, files, message, amend, sign, None)
}

/// Stage each explicit path (`git add`, or `git rm --cached` for a path that no
/// longer exists) and return how many were staged. A path git refuses is skipped.
fn stage_explicit_paths(repo_path: &Path, clean_files: &[&str]) -> Result<usize, String> {
    let mut added = 0;
    for &clean_file in clean_files {
        let file_abs = repo_path.join(clean_file);

        if file_abs.exists() {
            let add_output = crate::core::cmd::git_cmd()
                .args(["add", "--", clean_file])
                .current_dir(repo_path)
                .output()
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
            let rm_output = crate::core::cmd::git_cmd()
                .args(["rm", "--cached", "--ignore-unmatch", "--", clean_file])
                .current_dir(repo_path)
                .output();
            if rm_output.map(|o| o.status.success()).unwrap_or(false) {
                added += 1;
            }
        }
    }
    Ok(added)
}

/// Give the repository a fallback git identity when none is configured.
fn ensure_git_identity(repo_path: &Path) {
    let has_user = crate::core::cmd::git_cmd()
        .args(["config", "user.name"])
        .current_dir(repo_path)
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    if !has_user {
        let _ = crate::core::cmd::git_cmd()
            .args(["config", "user.name", "Kronn"])
            .current_dir(repo_path)
            .status();
        let _ = crate::core::cmd::git_cmd()
            .args(["config", "user.email", "kronn@localhost"])
            .current_dir(repo_path)
            .status();
    }
}

/// Run `git commit <commit_args>` and return the new short HEAD, optionally
/// retaining the backend's data-directory lock in Git and any hook descendants
/// across backend death.
fn run_commit_command(
    repo_path: &Path,
    commit_args: &[&str],
    data_dir_lock: Option<&std::fs::File>,
) -> Result<String, String> {
    // Only `git commit` executes hooks. Keep the inherited descriptor alive
    // through this spawn; descendants retain it until the whole hook tree exits.
    let child_lock = data_dir_lock
        .map(crate::core::config::inherit_data_dir_lock_for_child)
        .transpose()
        .map_err(|error| {
            format!("Failed to inherit data-directory lock for git commit: {error}")
        })?;
    let mut commit_command = crate::core::cmd::git_cmd();
    commit_command.args(commit_args).current_dir(repo_path);
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
        .output()
        .map_err(|e| format!("Failed to run git commit: {}", e))?;
    drop(child_lock);

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        return Err(format!("git commit failed: {}", stderr.trim()));
    }

    let hash_output = crate::core::cmd::git_cmd()
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to get commit hash: {}", e))?;

    Ok(String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string())
}

/// Stage and commit explicit paths, optionally retaining the backend's
/// data-directory lock in Git and any hook descendants across backend death.
pub fn run_git_commit_with_child_lock(
    repo_path: &Path,
    files: &[String],
    message: &str,
    amend: bool,
    sign: bool,
    data_dir_lock: Option<&std::fs::File>,
) -> Result<GitCommitResponse, String> {
    // git add each file individually, skip missing files gracefully
    let clean_files = files
        .iter()
        .map(|file| file.trim_matches('"'))
        .collect::<Vec<_>>();
    if stage_explicit_paths(repo_path, &clean_files)? == 0 {
        return Err("No files could be staged".to_string());
    }

    ensure_git_identity(repo_path);

    let mut commit_args = vec!["commit"];
    if amend {
        commit_args.push("--amend");
    }
    commit_args.push("-s"); // signoff by default
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

    let hash = run_commit_command(repo_path, &commit_args, data_dir_lock)?;

    Ok(GitCommitResponse {
        hash,
        message: message.to_string(),
    })
}

/// Whether `repo_path` is in the middle of a merge: git keeps `MERGE_HEAD` in
/// the worktree's own git dir, so this holds per linked worktree.
pub fn merge_in_progress(repo_path: &Path) -> bool {
    crate::core::cmd::git_cmd()
        .args(["rev-parse", "-q", "--verify", "MERGE_HEAD"])
        .current_dir(repo_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a `git diff`-style command and split its NUL-separated path list.
fn git_path_list(repo_path: &Path, args: &[&str]) -> Result<Vec<String>, String> {
    let output = crate::core::cmd::git_cmd()
        .args(args)
        .current_dir(repo_path)
        .output()
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
    let output = crate::core::cmd::git_cmd()
        .args(["merge-base", "--all", "HEAD", "MERGE_HEAD"])
        .current_dir(repo_path)
        .output()
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
    let clean_files = files
        .iter()
        .map(|file| file.trim_matches('"'))
        .collect::<Vec<_>>();
    if !merge_in_progress(repo_path) {
        return Err("no merge in progress in this worktree".to_string());
    }

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

    if stage_explicit_paths(repo_path, &clean_files)? == 0 {
        return Err("No files could be staged".to_string());
    }

    // What the merge legitimately changes in the index: the paths the merged-in
    // side changed since the histories diverged (a three-way merge leaves a path
    // at our version whenever the other side did not touch it). Anything else
    // staged was put there by hand and is not part of what the worker named.
    let brought_by_merge = paths_changed_by_merged_side(repo_path)?;
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
    .filter(|path| !brought_by_merge.contains(path) && !clean_files.contains(&path.as_str()))
    .collect::<Vec<_>>();
    if !foreign.is_empty() {
        return Err(format!(
            "refused: these staged paths belong neither to the merge nor to `files`: {}. Unstage them \
             (`git restore --staged -- <path>`) or name them in `files`; the merge state was left as it was.",
            foreign.join(", ")
        ));
    }

    ensure_git_identity(repo_path);

    let mut commit_args = vec!["commit", "-s"];
    commit_args.push(if sign { "-S" } else { "--no-gpg-sign" });
    commit_args.extend(["-m", message]);
    let hash = run_commit_command(repo_path, &commit_args, data_dir_lock)?;

    let has_second_parent = crate::core::cmd::git_cmd()
        .args(["rev-parse", "-q", "--verify", "HEAD^2"])
        .current_dir(repo_path)
        .output()
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
fn github_https_url(remote_url: &str) -> Option<String> {
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

/// The authenticated `git push`: the token never appears in an argument or
/// in a variable a hook reads by name. It travels as an `http.extraHeader`
/// scoped to github.com through `GIT_CONFIG_*`, with hooks off, no
/// credential helper (a repository's could receive it) and TLS verified.
fn authenticated_push_command(
    repo_path: &Path,
    https_url: &str,
    branch: &str,
    token: &str,
) -> std::process::Command {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let header = format!(
        "Authorization: Basic {}",
        STANDARD.encode(format!("x-access-token:{token}"))
    );
    let mut cmd = crate::core::cmd::git_cmd();
    cmd.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "credential.helper=",
        "-c",
        "http.sslVerify=true",
        "push",
        "--no-verify",
        "-u",
        https_url,
        branch,
    ])
    .current_dir(repo_path)
    .env("GIT_CONFIG_COUNT", "1")
    .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraHeader")
    .env("GIT_CONFIG_VALUE_0", header);
    cmd
}

/// Push the current branch to origin. `github_env` is the project's GitHub
/// variables (`github_connection::env_for_launch`): its token, when the
/// project is connected and the remote is on GitHub, authenticates the push.
pub fn run_git_push(
    repo_path: &Path,
    github_env: &[(String, String)],
) -> Result<GitPushResponse, String> {
    let branch_output = crate::core::cmd::git_cmd()
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["branch", "--show-current"])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to get branch: {}", e))?;

    let branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();
    if branch.is_empty() {
        return Err("Cannot determine current branch (detached HEAD?)".to_string());
    }

    let token = github_env
        .iter()
        .find(|(name, value)| name == "GH_TOKEN" && !value.is_empty())
        .map(|(_, value)| value.as_str());
    let authenticated = token.and_then(|token| {
        let remote_url = crate::core::cmd::git_cmd()
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["remote", "get-url", "origin"])
            .current_dir(repo_path)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        github_https_url(&remote_url).map(|url| (url, token))
    });

    let mut cmd = match authenticated {
        Some((url, token)) => authenticated_push_command(repo_path, &url, &branch, token),
        None => {
            // The user's own credentials (SSH agent, credential helper).
            let mut cmd = crate::core::cmd::git_cmd();
            cmd.args(["push", "-u", "origin", &branch])
                .current_dir(repo_path);
            cmd
        }
    };
    // Never let git block the thread on an interactive prompt (SSH passphrase,
    // username/password): this runs on a blocking thread with no timeout, so a
    // prompt would hang it forever. Fail fast instead. The low-speed envs abort
    // an HTTPS push stalled under 1 KB/s for 60s (dead network / dead proxy).
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1024")
        .env("GIT_HTTP_LOW_SPEED_TIME", "60");
    let push_output = cmd
        .output()
        .map_err(|e| format!("Failed to run git push: {}", e))?;

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

/// The exact process `run_exec` starts: argv without a shell, a built
/// environment, the project as working directory.
/// `github_env` is the project's GitHub variables when it is connected (D2).
pub fn exec_command(
    repo_path: &Path,
    cmd: &str,
    github_env: &[(String, String)],
) -> Result<std::process::Command, String> {
    let argv = exec_argv(repo_path, cmd)?;
    let mut command = sync_cmd(&argv[0], crate::core::child_env::ChildRoute::ProjectExec);
    command.args(&argv[1..]).current_dir(repo_path);
    crate::core::child_env::isolate_with_github(
        &mut command,
        crate::core::child_env::ChildRoute::ProjectExec,
        github_env,
    );
    Ok(command)
}

/// Execute an allow-listed command in the given directory, without a shell
/// and with a built environment.
/// The caller MUST call `validate_exec_command` before this function.
pub fn run_exec(
    repo_path: &Path,
    cmd: &str,
    github_env: &[(String, String)],
) -> Result<ExecResponse, String> {
    let output = exec_command(repo_path, cmd, github_env)?
        .output()
        .map_err(|e| format!("Failed to execute: {}", e))?;

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
    let output = crate::core::cmd::git_cmd()
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_path)
        .output();
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
fn host_cli_command(
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
    command
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
    // Ensure the branch is pushed before creating the PR
    let has_upstream = crate::core::cmd::git_cmd()
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .current_dir(repo_path)
        .output()
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
                .output()
                .map_err(|e| format!("Failed to run glab: {} (is glab installed?)", e))?
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
                .output()
                .map_err(|e| format!("Failed to run gh: {} (is gh installed?)", e))?
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
static PR_URLS: std::sync::LazyLock<std::sync::Mutex<PrUrlCache>> =
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
fn output_within(
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

/// Read the PR/MR template from the project, if one exists.
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
        if let Ok(content) = std::fs::read_to_string(&path) {
            if !content.trim().is_empty() {
                return Some(content);
            }
        }
    }
    None
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

#[cfg(test)]
mod tests {

    /// `gh` and `glab` start git in the repository: they get the git-host
    /// environment and the project's GitHub token, never the backend's.
    #[cfg(unix)]
    #[test]
    fn gh_and_glab_run_without_the_backend_environment() {
        use crate::core::child_env::probe;
        probe::plant_real_sentinel();
        let bin = tempfile::tempdir().unwrap();
        let gh_out = probe::env_dumping_program(bin.path(), "gh");
        let glab_out = probe::env_dumping_program(bin.path(), "glab");
        let path = format!(
            "{}:/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin",
            bin.path().display()
        );
        let repo = |remote: &str| {
            let dir = tempfile::tempdir().unwrap();
            for args in [vec!["init", "-q"], vec!["remote", "add", "origin", remote]] {
                assert!(crate::core::cmd::git_cmd()
                    .args(&args)
                    .current_dir(dir.path())
                    .status()
                    .unwrap()
                    .success());
            }
            dir
        };
        let github = repo("https://github.com/acme/app.git");
        let gitlab = repo("https://gitlab.com/acme/app.git");
        let project_env = vec![("GH_TOKEN".to_string(), "project-token".to_string())];
        let home = bin.path().to_str().unwrap();
        let create = probe::with_secret_parent(&path, home, || {
            check_pr_url(github.path(), "feature", &project_env);
            check_pr_url(gitlab.path(), "feature", &project_env);
            host_cli_command("gh", github.path(), &project_env)
        });

        let gh = probe::read_dump(&gh_out);
        probe::assert_dump_without_secrets(&gh, &["GH_TOKEN"]);
        assert_eq!(
            gh.get("GH_TOKEN").map(String::as_str),
            Some("project-token"),
            "gh gets the connected project's token, not the backend's"
        );
        let glab = probe::read_dump(&glab_out);
        probe::assert_dump_without_secrets(&glab, &[]);

        probe::assert_built_without_secrets(&create, &path, &["GH_TOKEN"]);
        assert_eq!(
            probe::env_of(&create).get("GH_TOKEN").map(String::as_str),
            Some("project-token")
        );
    }

    #[test]
    fn github_remotes_map_to_a_credential_free_https_url() {
        for remote in [
            "git@github.com:acme/app.git",
            "ssh://git@github.com/acme/app.git",
            "https://github.com/acme/app.git",
            "https://x-access-token:old@github.com/acme/app.git",
        ] {
            assert_eq!(
                github_https_url(remote).as_deref(),
                Some("https://github.com/acme/app.git"),
                "{remote}"
            );
        }
        assert_eq!(github_https_url("git@gitlab.com:acme/app.git"), None);
        assert_eq!(github_https_url("/srv/git/app.git"), None);
    }

    /// The push token reaches no repository hook: neither in an argument, nor
    /// in the environment, and the authenticated push runs no hook (B3-10).
    #[cfg(unix)]
    #[test]
    fn a_push_never_shows_its_token_to_a_pre_push_hook() {
        use std::os::unix::fs::PermissionsExt;
        let token = "ghp_sentinel_push_token";
        let base = tempfile::tempdir().unwrap();
        let remote = base.path().join("remote.git");
        let repo = base.path().join("repo");
        let git = |dir: &Path, args: &[&str]| {
            let output = crate::core::cmd::git_cmd()
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "Fixture")
                .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
                .env("GIT_COMMITTER_NAME", "Fixture")
                .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {output:?}");
        };
        git(
            base.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );
        git(base.path(), &["init", "-q", repo.to_str().unwrap()]);
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(repo.join("f.txt"), "x").unwrap();
        git(&repo, &["add", "f.txt"]);
        git(&repo, &["commit", "-q", "-m", "fixture"]);
        let dump = base.path().join("hook.dump");
        let hook = repo.join(".git/hooks/pre-push");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\n{{ echo \"$@\"; /usr/bin/env; git config --list; }} >> '{}'\n",
                dump.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let encoded = {
            use base64::{engine::general_purpose::STANDARD, Engine};
            STANDARD.encode(format!("x-access-token:{token}"))
        };

        // A non-GitHub remote: the user's own push, hook included, no token.
        let env = vec![("GH_TOKEN".to_string(), token.to_string())];
        let pushed = run_git_push(&repo, &env).unwrap();
        assert!(pushed.success, "{}", pushed.message);
        let seen = std::fs::read_to_string(&dump).unwrap();
        assert!(!seen.contains(token) && !seen.contains(&encoded), "{seen}");

        // The authenticated form: token only in a scoped header, hooks off.
        std::fs::remove_file(&dump).unwrap();
        let branch = String::from_utf8(
            crate::core::cmd::git_cmd()
                .args(["branch", "--show-current"])
                .current_dir(&repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let mut command =
            authenticated_push_command(&repo, remote.to_str().unwrap(), branch.trim(), token);
        assert!(command
            .get_args()
            .all(|arg| !arg.to_string_lossy().contains(token)));
        assert!(!crate::core::child_env::probe::env_of(&command)
            .values()
            .any(|value| value.contains(token)));
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(!dump.exists(), "the authenticated push ran a hook");
    }
    #[test]
    fn status_without_pr_lookup_reports_only_a_link_already_known() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = crate::core::cmd::git_cmd()
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap()
                .status;
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
        git(&["checkout", "-q", "-b", "feature"]);

        let unknown = run_git_status_page_without_pr_lookup(dir.path(), 0, 10).unwrap();
        assert_eq!(unknown.branch, "feature");
        assert_eq!(unknown.pr_url, None, "no lookup, nothing known");

        PR_URLS.lock().unwrap().insert(
            (dir.path().to_path_buf(), "feature".to_string()),
            (
                Some("https://example.test/pr/7".into()),
                std::time::Instant::now(),
            ),
        );
        let known = run_git_status_page_without_pr_lookup(dir.path(), 0, 10).unwrap();
        assert_eq!(known.pr_url.as_deref(), Some("https://example.test/pr/7"));
        forget_pr_urls(dir.path());
    }

    #[cfg(unix)]
    #[test]
    fn a_pr_lookup_that_hangs_is_given_up_instead_of_blocking_the_discussion() {
        // KT-983 — `gh pr view` runs on every git-status; a slow network must
        // cost at most the limit, never the whole page.
        let mut command = std::process::Command::new("sleep");
        command.arg("5");
        let started = std::time::Instant::now();
        let output = super::output_within(command, std::time::Duration::from_millis(200));
        assert!(output.is_none());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );

        let mut quick = std::process::Command::new("echo");
        quick.arg("https://example.test/pr/1");
        let output = super::output_within(quick, std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "https://example.test/pr/1"
        );
    }

    use super::*;

    #[test]
    fn exec_refuses_env() {
        assert!(validate_exec_command("env").is_err());
        assert!(validate_exec_command("env FOO=1").is_err());
    }

    #[test]
    fn exec_refuses_a_read_outside_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn x() {}").unwrap();
        for cmd in [
            "cat /etc/passwd",
            "cat ../../x",
            "head -n 3 ../secret",
            "tail src/../../secret",
            "find / -name passwd",
            "stat /etc",
            "grep -r root /etc",
            "rg --file=/etc/passwd src",
            "wc -l /etc/hosts",
            "du -sh ..",
            "file /bin/sh",
            "tree /",
            "ls /",
        ] {
            assert!(
                validate_exec_command(cmd).is_ok(),
                "{cmd} passes the allow-list"
            );
            assert!(exec_argv(&root, cmd).is_err(), "{cmd} must be refused");
        }
        for cmd in [
            "cat src/lib.rs",
            "head -n 1 src/lib.rs",
            "grep -rn \"/api/\" src",
            "find . -name \"*.rs\"",
            "ls -la",
            "wc -l src/lib.rs",
            "git status",
        ] {
            assert!(exec_argv(&root, cmd).is_ok(), "{cmd} must be accepted");
        }
    }

    #[cfg(unix)]
    #[test]
    fn exec_refuses_a_symlink_leaving_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(dir.path().join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret"), root.join("link")).unwrap();
        assert!(exec_argv(&root, "cat link").is_err());
    }

    #[test]
    fn exec_refuses_find_actions_and_git_no_index() {
        let root = tempfile::tempdir().unwrap();
        assert!(exec_argv(root.path(), "find . -exec cat {} +").is_err());
        assert!(exec_argv(root.path(), "find . -delete").is_err());
        assert!(validate_exec_command("git diff --no-index /etc/passwd README.md").is_err());
        assert!(validate_exec_command("git log --output=x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn exec_runs_an_in_project_read_without_a_shell_or_the_backend_env() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("note é.txt"), "bonjour").unwrap();
        let read = run_exec(dir.path(), "cat 'note é.txt'", &[]).unwrap();
        assert_eq!(read.stdout, "bonjour");
        assert_eq!(read.exit_code, 0);
        // No shell: a variable stays literal instead of expanding.
        let echoed = run_exec(dir.path(), "echo $HOME", &[]).unwrap();
        assert_eq!(echoed.stdout.trim(), "$HOME");
        assert!(run_exec(dir.path(), "cat /etc/passwd", &[]).is_err());
    }

    #[test]
    fn the_exec_command_carries_a_built_environment() {
        let dir = tempfile::tempdir().unwrap();
        let command = crate::core::child_env::with_parent_env(
            &[
                ("PATH", "/usr/bin:/bin"),
                ("KRONN_AUTH_TOKEN", "admin"),
                ("KRONN_ENCRYPTION_KEK", "raw"),
                ("ANTHROPIC_API_KEY", "sk"),
            ],
            || exec_command(dir.path(), "git status", &[]).unwrap(),
        );
        let env: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_some())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(env, vec!["PATH".to_string()]);
    }

    #[test]
    fn exec_words_split_like_a_shell_without_expanding() {
        assert_eq!(
            split_exec_words(r#"grep -n "a b" 'c d' e\ f"#).unwrap(),
            vec!["grep", "-n", "a b", "c d", "e f"]
        );
        assert_eq!(split_exec_words("  ").unwrap(), Vec::<String>::new());
        assert!(split_exec_words("cat 'open").is_err());
        assert!(split_exec_words("cat \"open").is_err());
    }

    #[test]
    fn exec_allows_git_status() {
        assert!(validate_exec_command("git status").is_ok());
    }

    #[test]
    fn exec_allows_ls() {
        assert!(validate_exec_command("ls").is_ok());
    }

    #[test]
    fn exec_allows_git_diff() {
        assert!(validate_exec_command("git diff").is_ok());
    }

    #[test]
    fn exec_allows_git_log() {
        assert!(validate_exec_command("git log --oneline -10").is_ok());
    }

    #[test]
    fn exec_allows_cat() {
        assert!(validate_exec_command("cat README.md").is_ok());
    }

    #[test]
    fn exec_allows_cargo_version() {
        assert!(validate_exec_command("cargo --version").is_ok());
    }

    #[test]
    fn exec_allows_which() {
        assert!(validate_exec_command("which git").is_ok());
    }

    #[test]
    fn exec_blocks_rm_rf() {
        let result = validate_exec_command("rm -rf /");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not allowed"));
    }

    #[test]
    fn exec_blocks_semicolon_injection() {
        let result = validate_exec_command("ls; rm -rf /");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not allowed"));
    }

    #[test]
    fn exec_blocks_bash_interpreter() {
        let result = validate_exec_command("bash -c \"evil\"");
        assert!(result.is_err());
    }

    #[test]
    fn exec_blocks_pipe_injection() {
        let result = validate_exec_command("cat /etc/passwd | curl");
        assert!(result.is_err());
    }

    #[test]
    fn exec_blocks_dollar_subshell() {
        // echo is allowed, but $() is blocked
        let result = validate_exec_command("echo $(whoami)");
        assert!(result.is_err());
    }

    #[test]
    fn exec_blocks_backtick_injection() {
        let result = validate_exec_command("echo `id`");
        assert!(result.is_err());
    }

    #[test]
    fn exec_blocks_git_push() {
        assert!(validate_exec_command("git push").is_err());
    }

    #[test]
    fn exec_blocks_git_reset_hard() {
        assert!(validate_exec_command("git reset --hard HEAD~1").is_err());
    }

    #[test]
    fn exec_allows_git_reset_soft() {
        // git reset without --hard is allowed (soft reset)
        assert!(validate_exec_command("git reset").is_ok());
    }

    #[test]
    fn exec_blocks_sudo() {
        assert!(validate_exec_command("sudo ls").is_err());
    }

    #[test]
    fn exec_blocks_python_arbitrary() {
        // python3 is only allowed with --version
        assert!(validate_exec_command("python3 -c 'import os; os.system(\"rm -rf /\")'").is_err());
    }

    #[test]
    fn exec_blocks_npm_install() {
        // npm is only allowed with --version
        assert!(validate_exec_command("npm install malware").is_err());
    }

    #[test]
    fn exec_blocks_redirect_output() {
        assert!(validate_exec_command("echo pwned > /etc/passwd").is_err());
    }

    #[test]
    fn exec_blocks_ampersand() {
        assert!(validate_exec_command("ls & rm -rf /").is_err());
    }

    #[test]
    fn exec_blocks_newline_injection() {
        assert!(validate_exec_command("ls\nrm -rf /").is_err());
    }

    #[test]
    fn exec_allows_grep() {
        assert!(validate_exec_command("grep -r \"pattern\" .").is_ok());
    }

    #[test]
    fn exec_allows_rg() {
        assert!(validate_exec_command("rg \"pattern\"").is_ok());
    }

    #[test]
    fn exec_allows_tree() {
        assert!(validate_exec_command("tree").is_ok());
    }

    #[test]
    fn exec_allows_file() {
        assert!(validate_exec_command("file somefile.txt").is_ok());
    }

    #[test]
    fn exec_allows_stat() {
        assert!(validate_exec_command("stat somefile.txt").is_ok());
    }

    #[test]
    fn exec_allows_du() {
        assert!(validate_exec_command("du -sh .").is_ok());
    }

    #[test]
    fn remote_urls_are_normalized_for_the_browser_without_credentials() {
        assert_eq!(
            normalize_git_remote_web_url("git@github.com:DocRoms/Kronn.git"),
            Some("https://github.com/DocRoms/Kronn".into())
        );
        assert_eq!(
            normalize_git_remote_web_url("ssh://git@gitlab.example.com/team/app.git"),
            Some("https://gitlab.example.com/team/app".into())
        );
        assert_eq!(
            normalize_git_remote_web_url("https://oauth:secret@gitlab.com/team/app.git"),
            Some("https://gitlab.com/team/app".into())
        );
        assert_eq!(normalize_git_remote_web_url("../local-repo"), None);
    }

    // ── Commit args tests ────────────────────────────────────────────────────

    fn make_test_repo(name: &str) -> tempfile::TempDir {
        let dir = tempfile::Builder::new()
            .prefix(&format!("kronn-git-{}", name))
            .tempdir()
            .unwrap();
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::fs::write(dir.path().join("init.txt"), "init").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn git_status_exposes_repository_overview_metadata() {
        let repo = make_test_repo("overview-metadata");
        std::process::Command::new("git")
            .args(["remote", "add", "origin", "git@github.com:team/demo.git"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["tag", "v1.2.3"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        let status = run_git_status(repo.path()).unwrap();

        assert_eq!(
            status.remote_url.as_deref(),
            Some("https://github.com/team/demo")
        );
        assert_eq!(
            status.pull_requests_url.as_deref(),
            Some("https://github.com/team/demo/pulls")
        );
        assert_eq!(status.last_tag.as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn git_branches_returns_bounded_local_branch_graph() {
        let repo = make_test_repo("branch-graph");
        std::process::Command::new("git")
            .args(["switch", "-c", "feature/graph"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        std::fs::write(repo.path().join("feature.txt"), "feature").unwrap();
        std::process::Command::new("git")
            .args(["add", "feature.txt"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "feature commit"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        let graph = run_git_branches(repo.path()).unwrap();

        assert_eq!(graph.current_branch, "feature/graph");
        assert_eq!(graph.default_branch, "main");
        assert!(graph
            .branches
            .iter()
            .any(|branch| branch.name == "feature/graph" && branch.is_current));
        assert!(graph
            .branches
            .iter()
            .any(|branch| branch.name == "main" && !branch.is_remote));
        assert_eq!(graph.commits[0].subject, "feature commit");
        assert!(!graph.truncated);
    }

    #[test]
    fn git_switch_changes_clean_local_branch() {
        let repo = make_test_repo("switch-clean");
        std::process::Command::new("git")
            .args(["branch", "feature/safe"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        let switched = run_git_switch_branch(repo.path(), "feature/safe").unwrap();

        assert_eq!(switched.branch, "feature/safe");
        assert_eq!(
            git_output(repo.path(), &["branch", "--show-current"]).unwrap(),
            "feature/safe"
        );
    }

    #[test]
    fn git_switch_refuses_dirty_worktree_without_changing_branch() {
        let repo = make_test_repo("switch-dirty");
        std::process::Command::new("git")
            .args(["branch", "feature/blocked"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        std::fs::write(repo.path().join("init.txt"), "local edit").unwrap();

        let error = run_git_switch_branch(repo.path(), "feature/blocked").unwrap_err();

        assert!(error.contains("modifications locales"));
        assert_eq!(
            git_output(repo.path(), &["branch", "--show-current"]).unwrap(),
            "main"
        );
        assert_eq!(
            std::fs::read_to_string(repo.path().join("init.txt")).unwrap(),
            "local edit"
        );
    }

    #[test]
    fn git_switch_rejects_unknown_or_malformed_branch() {
        let repo = make_test_repo("switch-invalid");

        assert!(run_git_switch_branch(repo.path(), "--upload-pack=evil").is_err());
        assert!(run_git_switch_branch(repo.path(), "missing").is_err());
        assert_eq!(
            git_output(repo.path(), &["branch", "--show-current"]).unwrap(),
            "main"
        );
    }

    #[test]
    fn commit_adds_signoff_by_default() {
        let repo = make_test_repo("signoff");
        std::fs::write(repo.path().join("file.txt"), "content").unwrap();
        let result = run_git_commit(
            repo.path(),
            &["file.txt".into()],
            "test signoff",
            false,
            false,
        );
        assert!(result.is_ok(), "commit failed: {:?}", result.err());

        // Check that the commit message contains Signed-off-by
        let log = std::process::Command::new("git")
            .args(["log", "-1", "--format=%B"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let msg = String::from_utf8_lossy(&log.stdout);
        assert!(
            msg.contains("Signed-off-by:"),
            "Commit should have Signed-off-by, got: {}",
            msg
        );
    }

    // ── parse_committed_diff tests ───────────────────────────────────────────

    #[test]
    fn parse_committed_diff_handles_modified_added_deleted() {
        let out = "M\tsrc/lib.rs\nA\tdocs/new.md\nD\told.txt";
        let parsed = parse_committed_diff(out);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].path, "src/lib.rs");
        assert_eq!(parsed[0].status, "modified");
        assert!(parsed[0].staged);
        assert_eq!(parsed[1].path, "docs/new.md");
        assert_eq!(parsed[1].status, "added");
        assert_eq!(parsed[2].path, "old.txt");
        assert_eq!(parsed[2].status, "deleted");
    }

    #[test]
    fn parse_committed_diff_renames_use_destination_path() {
        let out = "R100\told/path.rs\tnew/path.rs";
        let parsed = parse_committed_diff(out);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].path, "new/path.rs");
        assert_eq!(parsed[0].status, "renamed");
    }

    #[test]
    fn parse_committed_diff_ignores_empty_and_garbage() {
        let out = "\n\nZ\tweird\nM\tok.rs\n";
        let parsed = parse_committed_diff(out);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].path, "ok.rs");
    }

    #[test]
    fn parse_committed_diff_type_change_treated_as_modified() {
        let out = "T\tsymlink.txt";
        let parsed = parse_committed_diff(out);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].status, "modified");
    }

    #[test]
    fn parse_git_blame_porcelain_returns_author_and_time_per_line() {
        let output = "\
0123456789abcdef0123456789abcdef01234567 1 1 1
author Ada Lovelace
author-mail <ada@example.test>
author-time 1710000000
author-tz +0100
filename src/main.rs
\tfirst
fedcba9876543210fedcba9876543210fedcba98 2 2 1
author Grace Hopper
author-mail <grace@example.test>
author-time 1720000000
author-tz +0200
filename src/main.rs
\tsecond";
        let lines = parse_git_blame_porcelain(output);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line_number, 1);
        assert_eq!(lines[0].author, "Ada Lovelace");
        assert_eq!(lines[0].author_time, 1_710_000_000);
        assert_eq!(lines[1].line_number, 2);
        assert_eq!(lines[1].author, "Grace Hopper");
        assert_eq!(lines[1].commit, "fedcba9876543210fedcba9876543210fedcba98");
    }

    // ── run_git_status committed_files integration tests ─────────────────────

    fn make_branch_repo(name: &str) -> tempfile::TempDir {
        let repo = make_test_repo(name);
        // Create a feature branch with two commits worth of changes.
        std::process::Command::new("git")
            .args(["checkout", "-b", "feature/x"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        std::fs::write(repo.path().join("added.txt"), "added").unwrap();
        std::fs::write(repo.path().join("init.txt"), "modified").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(repo.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "feature changes"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        repo
    }

    #[test]
    fn run_git_status_exposes_committed_files_on_feature_branch() {
        let repo = make_branch_repo("committed-feature");
        let status = run_git_status(repo.path()).unwrap();
        assert_eq!(status.branch, "feature/x");
        assert_eq!(status.default_branch, "main");
        assert!(!status.is_default_branch);
        let paths: Vec<&str> = status
            .committed_files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert!(
            paths.contains(&"added.txt"),
            "expected added.txt in {:?}",
            paths
        );
        assert_eq!(status.commits.len(), 1);
        assert_eq!(status.commits_total, 1);
        assert_eq!(status.commits_offset, 0);
        assert!(!status.commits_truncated);
        assert_eq!(status.commits[0].subject, "feature changes");
        assert!(
            paths.contains(&"init.txt"),
            "expected init.txt in {:?}",
            paths
        );
        for f in &status.committed_files {
            assert!(f.staged, "committed files should be marked staged: {:?}", f);
        }
    }

    #[test]
    fn run_git_status_pages_a_300_plus_commit_branch_without_materializing_it() {
        let repo = make_test_repo("commit-pages");
        std::process::Command::new("git")
            .args(["checkout", "-b", "feature/history"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        for index in 0..305 {
            let output = std::process::Command::new("git")
                // Background auto-gc repacks refs mid-loop and the next commit
                // can fail with "could not parse HEAD" on a loaded machine.
                .args([
                    "-c",
                    "gc.auto=0",
                    "-c",
                    "maintenance.auto=false",
                    "commit",
                    "--allow-empty",
                    "-m",
                    &format!("history {index}"),
                ])
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "empty history commit {index} failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let first = run_git_status_page(repo.path(), 0, 40, &[]).unwrap();
        assert_eq!(first.commits_total, 305);
        assert_eq!(first.commits_offset, 0);
        assert_eq!(first.commits.len(), 40);
        assert!(first.commits_truncated);
        assert_eq!(first.commits[0].subject, "history 304");
        assert_eq!(first.commits[39].subject, "history 265");

        let second = run_git_status_page(repo.path(), 40, 40, &[]).unwrap();
        assert_eq!(second.commits_total, 305);
        assert_eq!(second.commits_offset, 40);
        assert_eq!(second.commits.len(), 40);
        assert!(second.commits_truncated);
        assert_eq!(second.commits[0].subject, "history 264");

        let last = run_git_status_page(repo.path(), 300, 1_000, &[]).unwrap();
        assert_eq!(last.commits_total, 305);
        assert_eq!(last.commits_offset, 300);
        assert_eq!(last.commits.len(), 5);
        assert!(!last.commits_truncated);
        assert_eq!(last.commits[0].subject, "history 4");
        assert_eq!(last.commits[4].subject, "history 0");
    }

    #[test]
    fn resolve_default_branch_resolves_main_on_a_feature_branch() {
        let repo = make_branch_repo("default-branch");
        assert_eq!(resolve_default_branch(repo.path()), "main");
    }

    #[test]
    fn run_git_diff_committed_shows_the_branch_diff_for_a_committed_file() {
        // Regression for the GitPanel "committed on branch" bug: the file is
        // committed (clean working tree), so a plain `git diff` is useless —
        // the committed diff (`main...HEAD`) must surface the change.
        let repo = make_branch_repo("committed-diff");
        let res = run_git_diff_committed(repo.path(), "added.txt").unwrap();
        assert!(
            res.diff.contains("added.txt"),
            "committed diff must reference the file, got: {:?}",
            res.diff
        );
        assert!(
            res.diff.contains("@@"),
            "committed diff must contain a hunk header, got: {:?}",
            res.diff
        );
    }

    #[test]
    fn run_git_status_committed_files_empty_on_default_branch() {
        let repo = make_test_repo("on-main");
        let status = run_git_status(repo.path()).unwrap();
        assert!(status.is_default_branch);
        assert!(
            status.committed_files.is_empty(),
            "expected no committed_files on default branch, got {:?}",
            status.committed_files
        );
    }

    #[test]
    fn run_git_status_committed_and_uncommitted_are_disjoint_sections() {
        let repo = make_branch_repo("disjoint");
        // Add an uncommitted change on top of the committed work.
        std::fs::write(repo.path().join("untracked.txt"), "wip").unwrap();
        let status = run_git_status(repo.path()).unwrap();
        let committed_paths: Vec<&str> = status
            .committed_files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        let uncommitted_paths: Vec<&str> = status.files.iter().map(|f| f.path.as_str()).collect();
        assert!(committed_paths.contains(&"added.txt"));
        assert!(uncommitted_paths.contains(&"untracked.txt"));
        // The committed section must NOT leak the uncommitted file (and vice versa for committed-only paths).
        assert!(!committed_paths.contains(&"untracked.txt"));
        assert!(!uncommitted_paths.contains(&"added.txt"));
    }

    #[test]
    fn commit_without_sign_uses_no_gpg_sign() {
        let repo = make_test_repo("nogpg");
        // Set commit.gpgsign=true to simulate a user config that would fail without --no-gpg-sign
        std::process::Command::new("git")
            .args(["config", "commit.gpgsign", "true"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        // Set a nonexistent signing key to guarantee failure if --no-gpg-sign doesn't work
        std::process::Command::new("git")
            .args(["config", "user.signingkey", "/nonexistent/key"])
            .current_dir(repo.path())
            .output()
            .unwrap();

        std::fs::write(repo.path().join("file.txt"), "content").unwrap();
        let result = run_git_commit(repo.path(), &["file.txt".into()], "no gpg", false, false);
        assert!(
            result.is_ok(),
            "commit should succeed with --no-gpg-sign even when gpgsign=true: {:?}",
            result.err()
        );
    }
    // ─── KT-67 — commit detail behind an annotated line ─────────────────────

    #[test]
    fn commit_detail_refuses_anything_that_is_not_a_hash() {
        // The value is interpolated into a git invocation and blame only ever
        // hands us hashes, so a refusal is the right answer — not a best effort.
        for bad in [
            "HEAD",
            "main",
            "../../etc/passwd",
            "abc123; rm -rf /",
            "abc",                                       // too short
            "0123456789012345678901234567890123456789a", // too long
            "zzzzzzz",                                   // not hex
            "",
        ] {
            assert!(!valid_commit_ish(bad), "{bad:?} must be refused");
        }
        for good in ["abc1234", "0123456789abcdef0123456789abcdef01234567"] {
            assert!(valid_commit_ish(good), "{good:?} must be accepted");
        }

        let repo = make_test_repo("commit-detail-refuse");
        let err = run_git_commit_detail(repo.path(), "HEAD").unwrap_err();
        assert!(err.contains("invalid commit hash"), "{err}");
    }

    #[test]
    fn commit_detail_reports_message_author_and_branches() {
        let repo = make_test_repo("commit-detail-ok");
        std::fs::write(repo.path().join("a.txt"), "a").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(repo.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "commit",
                "-m",
                "sujet du commit",
                "-m",
                "corps sur\nplusieurs lignes",
            ])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let sha = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        let detail = run_git_commit_detail(repo.path(), &sha).unwrap();
        assert_eq!(detail.sha, sha);
        assert_eq!(detail.short_sha, sha[..detail.short_sha.len()]);
        assert_eq!(detail.subject, "sujet du commit");
        assert!(
            detail.body.contains("corps sur"),
            "body was {:?}",
            detail.body
        );
        assert_eq!(detail.author_name, "Test User");
        assert_eq!(detail.author_email, "test@test.com");
        assert!(detail.author_time > 0, "author_time must be a real epoch");
        assert_eq!(detail.files_changed, 1);
        assert!(
            detail.branches.contains(&"main".to_string()),
            "{:?}",
            detail.branches
        );
        assert!(!detail.branches_truncated);
    }

    #[test]
    fn commit_detail_truncates_a_long_branch_list_honestly() {
        let repo = make_test_repo("commit-detail-branches");
        let sha = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        // Every branch here contains the root commit.
        for i in 0..COMMIT_BRANCHES_CAP + 3 {
            std::process::Command::new("git")
                .args(["branch", &format!("topic-{i}")])
                .current_dir(repo.path())
                .output()
                .unwrap();
        }

        let detail = run_git_commit_detail(repo.path(), &sha).unwrap();
        assert_eq!(
            detail.files_changed, 1,
            "the initial commit must count its root-tree file"
        );
        assert_eq!(
            detail.branches.len(),
            COMMIT_BRANCHES_CAP,
            "list must be capped"
        );
        assert!(
            detail.branches_truncated,
            "a capped list must SAY it was capped — otherwise the UI implies it is complete",
        );
    }

    #[test]
    fn commit_detail_on_an_unknown_hash_fails_instead_of_inventing() {
        let repo = make_test_repo("commit-detail-unknown");
        let err = run_git_commit_detail(repo.path(), "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
            .unwrap_err();
        assert!(err.contains("git show failed"), "{err}");
    }

    fn commit_all(repo: &Path, message: &str) -> String {
        std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(repo)
            .output()
            .unwrap();
        String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    }

    /// KT-75 — the patch must be the commit against its PARENT, not against
    /// whatever the file looks like today.
    #[test]
    fn commit_patch_shows_the_change_as_it_was_made() {
        let repo = make_test_repo("commit-patch-parent");
        std::fs::write(repo.path().join("a.txt"), "premiere ligne\n").unwrap();
        commit_all(repo.path(), "initial");
        std::fs::write(
            repo.path().join("a.txt"),
            "premiere ligne\ndeuxieme ligne\n",
        )
        .unwrap();
        let second = commit_all(repo.path(), "ajoute une ligne");
        // The file moves on afterwards: the patch of `second` must not change.
        std::fs::write(repo.path().join("a.txt"), "tout autre chose\n").unwrap();
        commit_all(repo.path(), "reecrit tout");

        let patch = run_git_commit_patch(repo.path(), &second).unwrap();
        assert_eq!(patch.sha, second);
        assert_eq!(patch.subject, "ajoute une ligne");
        assert!(!patch.is_root);
        assert_eq!(patch.files_changed, 1);
        assert!(patch.patch.contains("+deuxieme ligne"), "{}", patch.patch);
        assert!(
            !patch.patch.contains("tout autre chose"),
            "the later rewrite leaked into an older commit's patch: {}",
            patch.patch
        );
        assert!(!patch.truncated);
    }

    /// The first commit has no parent. Without `--root`, `git show` prints the
    /// message and no diff at all — the tab would open empty.
    #[test]
    fn commit_patch_covers_the_root_commit() {
        // `make_test_repo` already lands one commit, so the root is ITS commit —
        // asking git for it beats assuming the one we just made is first.
        let repo = make_test_repo("commit-patch-root");
        std::fs::write(repo.path().join("second.txt"), "suite\n").unwrap();
        let child = commit_all(repo.path(), "deuxieme commit");
        let root = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-list", "--max-parents=0", "HEAD"])
                .current_dir(repo.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        assert_ne!(root, child);

        let patch = run_git_commit_patch(repo.path(), &root).unwrap();
        assert!(patch.is_root, "the first commit must be reported as root");
        assert!(
            patch.patch.contains("+init"),
            "root patch was {:?}",
            patch.patch
        );
        assert_eq!(patch.files_changed, 1);
        assert!(!run_git_commit_patch(repo.path(), &child).unwrap().is_root);
    }

    #[test]
    fn commit_patch_truncates_on_a_line_boundary_and_says_so() {
        let repo = make_test_repo("commit-patch-truncate");
        // Comfortably past the cap, so the branch is actually exercised.
        let big: String = (0..40_000).map(|i| format!("ligne numero {i}\n")).collect();
        std::fs::write(repo.path().join("big.txt"), big).unwrap();
        let sha = commit_all(repo.path(), "gros fichier");

        let patch = run_git_commit_patch(repo.path(), &sha).unwrap();
        assert!(patch.truncated, "a 600 KB patch must be reported as cut");
        assert!(patch.patch.len() <= COMMIT_PATCH_MAX_BYTES);
        assert!(
            patch.patch.ends_with('\n'),
            "the cut must land on a line boundary, not mid-hunk"
        );
    }

    #[test]
    fn commit_patch_refuses_a_non_hash_and_an_unknown_hash() {
        let repo = make_test_repo("commit-patch-guards");
        assert!(run_git_commit_patch(repo.path(), "HEAD")
            .unwrap_err()
            .contains("invalid commit hash"));
        assert!(run_git_commit_patch(repo.path(), "../../etc/passwd")
            .unwrap_err()
            .contains("invalid commit hash"));
        assert!(
            run_git_commit_patch(repo.path(), "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef")
                .unwrap_err()
                .contains("git show failed")
        );
    }
}
