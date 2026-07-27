//! Shared git operation helpers used by both project and discussion endpoints.

use crate::core::cmd::sync_cmd;
use crate::models::*;
use rusqlite::Connection;
use std::path::Path;

const PROJECT_GIT_GRAPH_LIMIT: usize = 80;

/// Resolve a GitHub token from MCP configs in the database.
/// Looks for configs with server_id "mcp-github" and extracts GITHUB_PERSONAL_ACCESS_TOKEN.
pub fn resolve_github_token(conn: &Connection, secret: &str) -> Option<String> {
    let configs = crate::db::mcps::list_configs(conn).ok()?;
    for config in &configs {
        if config.server_id == "mcp-github" {
            if let Ok(env) = crate::db::mcps::decrypt_env(&config.env_encrypted, secret) {
                if let Some(token) = env.get("GITHUB_PERSONAL_ACCESS_TOKEN") {
                    if !token.is_empty() {
                        return Some(token.clone());
                    }
                }
            }
        }
    }
    None
}

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

/// Run `git status` in the given repo directory and return structured status.
pub fn run_git_status(repo_path: &Path) -> Result<GitStatusResponse, String> {
    let run = |args: &[&str]| -> Result<String, String> {
        let output = sync_cmd("git")
            .args(args)
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git: {}", e))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };

    let run_with_status = |args: &[&str]| -> (String, bool) {
        match sync_cmd("git").args(args).current_dir(repo_path).output() {
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
    let committed_files = if !is_default_branch && !default_branch.is_empty() {
        let range = format!("{}...HEAD", default_branch);
        let (diff_out, ok) = run_with_status(&["diff", "--name-status", &range]);
        if ok {
            parse_committed_diff(&diff_out)
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
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
        check_pr_url(repo_path, &branch)
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
    sync_cmd("git")
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
    let output = sync_cmd("git")
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
        sync_cmd("git")
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
        sync_cmd("git")
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
        sync_cmd("git")
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
    let output = sync_cmd("git")
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
    let output = sync_cmd("git")
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
    let files = sync_cmd("git")
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

    let mut branches: Vec<String> = sync_cmd("git")
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

    let header = sync_cmd("git")
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
    let is_root = sync_cmd("git")
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
    let patch_out = sync_cmd("git")
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

    let files_changed = sync_cmd("git")
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
    // git add each file individually, skip missing files gracefully
    let mut added = 0;
    for file in files {
        let clean_file = file.trim_matches('"');
        let file_abs = repo_path.join(clean_file);

        if file_abs.exists() {
            let add_output = sync_cmd("git")
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
            let rm_output = sync_cmd("git")
                .args(["rm", "--cached", "--ignore-unmatch", "--", clean_file])
                .current_dir(repo_path)
                .output();
            if rm_output.map(|o| o.status.success()).unwrap_or(false) {
                added += 1;
            }
        }
    }
    if added == 0 {
        return Err("No files could be staged".to_string());
    }

    // Ensure git identity is set
    let has_user = sync_cmd("git")
        .args(["config", "user.name"])
        .current_dir(repo_path)
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    if !has_user {
        let _ = sync_cmd("git")
            .args(["config", "user.name", "Kronn"])
            .current_dir(repo_path)
            .status();
        let _ = sync_cmd("git")
            .args(["config", "user.email", "kronn@localhost"])
            .current_dir(repo_path)
            .status();
    }

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

    let commit_output = sync_cmd("git")
        .args(&commit_args)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to run git commit: {}", e))?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        return Err(format!("git commit failed: {}", stderr.trim()));
    }

    let hash_output = sync_cmd("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to get commit hash: {}", e))?;

    let hash = String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string();

    Ok(GitCommitResponse {
        hash,
        message: message.to_string(),
    })
}

/// Convert a git SSH URL to HTTPS with embedded token for push.
/// `git@github.com:org/repo.git` → `https://x-access-token:TOKEN@github.com/org/repo.git`
fn ssh_to_https_with_token(remote_url: &str, token: &str) -> Option<String> {
    remote_url
        .strip_prefix("git@github.com:")
        .map(|rest| format!("https://x-access-token:{}@github.com/{}", token, rest))
        .or_else(|| {
            remote_url
                .strip_prefix("git@gitlab.com:")
                .map(|rest| format!("https://oauth2:{}@gitlab.com/{}", token, rest))
        })
}

/// Push the current branch to origin.
pub fn run_git_push(
    repo_path: &Path,
    github_token: Option<&str>,
) -> Result<GitPushResponse, String> {
    let branch_output = sync_cmd("git")
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

    // Determine push target: if we have a token and the remote is SSH, use HTTPS with embedded token
    let push_target = if let Some(token) = github_token {
        let remote_url = sync_cmd("git")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["remote", "get-url", "origin"])
            .current_dir(repo_path)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        ssh_to_https_with_token(&remote_url, token)
    } else {
        None
    };

    let mut cmd = sync_cmd("git");
    if let Some(ref https_url) = push_target {
        // Push to HTTPS URL with embedded token (avoids SSH auth issues)
        cmd.args(["push", "-u", https_url, &branch]);
    } else {
        // Fallback: push to origin via SSH
        cmd.args(["push", "-u", "origin", &branch]);
    }
    cmd.current_dir(repo_path);
    // Never let git block the thread on an interactive prompt (SSH passphrase,
    // username/password): this runs on a blocking thread with no timeout, so a
    // prompt would hang it forever. Fail fast instead. The low-speed envs abort
    // an HTTPS push stalled under 1 KB/s for 60s (dead network / dead proxy).
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1024")
        .env("GIT_HTTP_LOW_SPEED_TIME", "60");
    if let Some(token) = github_token {
        cmd.env("GH_TOKEN", token);
    }
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
    const ALLOWED_CMDS: &[&str] = &[
        "git", "ls", "find", "wc", "head", "tail", "cat", "echo", "date", "whoami", "pwd", "env",
        "npm", "node", "cargo", "python3", "pnpm", "which", "grep", "rg", "tree", "file", "stat",
        "du",
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

/// Execute a shell command in the given directory.
/// The caller MUST call `validate_exec_command` before this function.
pub fn run_exec(repo_path: &Path, cmd: &str) -> Result<ExecResponse, String> {
    let output = sync_cmd("sh")
        .args(["-c", cmd])
        .current_dir(repo_path)
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
    let output = sync_cmd("git")
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

/// Create a pull/merge request via gh (GitHub) or glab (GitLab) CLI.
/// Automatically pushes the current branch first if it has no upstream.
pub fn run_create_pr(
    repo_path: &Path,
    title: &str,
    body: &str,
    base: &str,
    github_token: Option<&str>,
) -> Result<String, String> {
    // Ensure the branch is pushed before creating the PR
    let has_upstream = sync_cmd("git")
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .current_dir(repo_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !has_upstream {
        let push_result = run_git_push(repo_path, github_token)?;
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
            sync_cmd("glab")
                .args(&args)
                .current_dir(repo_path)
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
            let mut cmd = sync_cmd("gh");
            cmd.args(&args).current_dir(repo_path);
            if let Some(token) = github_token {
                cmd.env("GH_TOKEN", token);
            }
            cmd.output()
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
    Ok(url)
}

/// Check if an open PR/MR exists for a branch.
pub fn check_pr_url(repo_path: &Path, branch: &str) -> Option<String> {
    let provider = detect_provider(repo_path);
    let output = match provider {
        "gitlab" => sync_cmd("glab")
            .args([
                "mr", "view", branch, "--json", "web_url", "--jq", ".web_url",
            ])
            .current_dir(repo_path)
            .output()
            .ok()?,
        _ => sync_cmd("gh")
            .args(["pr", "view", branch, "--json", "url", "--jq", ".url"])
            .current_dir(repo_path)
            .output()
            .ok()?,
    };
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
    use super::*;

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
