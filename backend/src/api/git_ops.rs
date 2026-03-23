//! Shared git operation helpers used by both project and discussion endpoints.

use std::path::Path;
use crate::models::*;

/// Run `git status` in the given repo directory and return structured status.
pub fn run_git_status(repo_path: &Path) -> Result<GitStatusResponse, String> {
    let run = |args: &[&str]| -> Result<String, String> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git: {}", e))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };

    let run_with_status = |args: &[&str]| -> (String, bool) {
        match std::process::Command::new("git")
            .args(args)
            .current_dir(repo_path)
            .output()
        {
            Ok(o) => (String::from_utf8_lossy(&o.stdout).trim().to_string(), o.status.success()),
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
                let (_, ok_remote_main) = run_with_status(&["rev-parse", "--verify", "origin/main"]);
                if ok_remote_main {
                    "main".to_string()
                } else {
                    let (_, ok_remote_master) = run_with_status(&["rev-parse", "--verify", "origin/master"]);
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
                raw_path.split(" -> ").last().unwrap_or(&raw_path).to_string()
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
            }.to_string();

            let staged = staged_char != ' ' && staged_char != '?';

            GitFileStatus { path, status, staged }
        })
        .collect();

    // Ahead/behind upstream
    let (ahead, behind) = {
        let (ab_output, ab_ok) = run_with_status(&["rev-list", "--count", "--left-right", "@{upstream}...HEAD"]);
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
            let (count_output, count_ok) = run_with_status(&["rev-list", "--count", &format!("{}..HEAD", default_branch)]);
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

    // Check if branch has an upstream
    let has_upstream = {
        let (_, ok) = run_with_status(&["rev-parse", "--abbrev-ref", "@{upstream}"]);
        ok
    };

    // Check if there's an open PR/MR for this branch
    let pr_url = if !branch.is_empty() && !is_default_branch {
        check_pr_url(repo_path, &branch)
    } else {
        None
    };

    let provider = detect_provider(repo_path).to_string();

    Ok(GitStatusResponse {
        branch,
        default_branch,
        is_default_branch,
        files,
        ahead,
        behind,
        has_upstream,
        provider,
        pr_url,
    })
}

/// Run `git diff` for a specific file in the given repo directory.
pub fn run_git_diff(repo_path: &Path, file_path: &str) -> Result<GitDiffResponse, String> {
    let run_diff = |args: &[&str]| -> String {
        std::process::Command::new("git")
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
                    let lines: Vec<String> = content.lines()
                        .map(|l| format!("+{}", l))
                        .collect();
                    if lines.is_empty() {
                        String::new()
                    } else {
                        format!("--- /dev/null\n+++ b/{}\n@@ -0,0 +1,{} @@\n{}",
                            file_path, lines.len(), lines.join("\n"))
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

    Ok(GitDiffResponse { path: file_path.to_string(), diff })
}

/// Stage files and commit in the given repo directory.
pub fn run_git_commit(repo_path: &Path, files: &[String], message: &str, amend: bool, sign: bool) -> Result<GitCommitResponse, String> {
    // git add each file individually, skip missing files gracefully
    let mut added = 0;
    for file in files {
        let clean_file = file.trim_matches('"');
        let file_abs = repo_path.join(clean_file);

        if file_abs.exists() {
            let add_output = std::process::Command::new("git")
                .args(["add", "--", clean_file])
                .current_dir(repo_path)
                .output()
                .map_err(|e| format!("Failed to run git add: {}", e))?;
            if add_output.status.success() {
                added += 1;
            } else {
                tracing::warn!("git add skipped '{}': {}", clean_file,
                    String::from_utf8_lossy(&add_output.stderr).trim());
            }
        } else {
            let rm_output = std::process::Command::new("git")
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
    let has_user = std::process::Command::new("git")
        .args(["config", "user.name"])
        .current_dir(repo_path)
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    if !has_user {
        let _ = std::process::Command::new("git")
            .args(["config", "user.name", "Kronn"])
            .current_dir(repo_path).status();
        let _ = std::process::Command::new("git")
            .args(["config", "user.email", "kronn@localhost"])
            .current_dir(repo_path).status();
    }

    let mut commit_args = vec!["commit"];
    if amend {
        commit_args.push("--amend");
    }
    if sign {
        commit_args.push("-S");
    }
    commit_args.push("-m");
    commit_args.push(message);

    let commit_output = std::process::Command::new("git")
        .args(&commit_args)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to run git commit: {}", e))?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        return Err(format!("git commit failed: {}", stderr.trim()));
    }

    let hash_output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to get commit hash: {}", e))?;

    let hash = String::from_utf8_lossy(&hash_output.stdout).trim().to_string();

    Ok(GitCommitResponse { hash, message: message.to_string() })
}

/// Push the current branch to origin.
pub fn run_git_push(repo_path: &Path) -> Result<GitPushResponse, String> {
    let branch_output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to get branch: {}", e))?;

    let branch = String::from_utf8_lossy(&branch_output.stdout).trim().to_string();
    if branch.is_empty() {
        return Err("Cannot determine current branch (detached HEAD?)".to_string());
    }

    let push_output = std::process::Command::new("git")
        .args(["push", "-u", "origin", &branch])
        .current_dir(repo_path)
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
        "git", "ls", "find", "wc", "head", "tail", "cat",
        "echo", "date", "whoami", "pwd", "env",
        "npm", "node", "cargo", "python3", "pnpm",
        "which", "grep", "rg", "tree", "file", "stat", "du",
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
        const BLOCKED_GIT: &[&str] = &["push", "rm", "mv", "clean", "checkout", "rebase", "merge", "pull", "fetch", "clone", "init", "remote", "config"];
        if BLOCKED_GIT.contains(&subcommand) {
            return Err(DENY_MSG.to_string());
        }
        // Block git reset --hard specifically
        if subcommand == "reset" && parts.contains(&"--hard") {
            return Err(DENY_MSG.to_string());
        }
        // Only allow known safe git subcommands
        const SAFE_GIT: &[&str] = &["status", "diff", "log", "branch", "stash", "show", "blame", "shortlog", "reset"];
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
    let output = std::process::Command::new("sh")
        .args(["-c", cmd])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to execute: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() && (stderr.contains("not found") || stderr.contains("No such file")) {
        stderr.push_str(
            "\n\nCommand not found. The terminal runs inside the Docker container \
            with access to host binaries (/usr/bin). If the tool is installed elsewhere, \
            check your PATH or install it in the container."
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
    let output = std::process::Command::new("git")
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
pub fn run_create_pr(repo_path: &Path, title: &str, body: &str, base: &str) -> Result<String, String> {
    let provider = detect_provider(repo_path);

    let output = match provider {
        "gitlab" => {
            let mut args = vec!["mr", "create", "--title", title, "--target-branch", base, "--no-editor"];
            if !body.is_empty() {
                args.push("--description");
                args.push(body);
            }
            std::process::Command::new("glab")
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
            std::process::Command::new("gh")
                .args(&args)
                .current_dir(repo_path)
                .output()
                .map_err(|e| format!("Failed to run gh: {} (is gh installed?)", e))?
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let cmd = if provider == "gitlab" { "glab mr create" } else { "gh pr create" };
        return Err(format!("{} failed: {}", cmd, stderr.trim()));
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(url)
}

/// Check if an open PR/MR exists for a branch.
pub fn check_pr_url(repo_path: &Path, branch: &str) -> Option<String> {
    let provider = detect_provider(repo_path);
    let output = match provider {
        "gitlab" => {
            std::process::Command::new("glab")
                .args(["mr", "view", branch, "--json", "web_url", "--jq", ".web_url"])
                .current_dir(repo_path)
                .output().ok()?
        }
        _ => {
            std::process::Command::new("gh")
                .args(["pr", "view", branch, "--json", "url", "--jq", ".url"])
                .current_dir(repo_path)
                .output().ok()?
        }
    };
    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if url.is_empty() { None } else { Some(url) }
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
*Created via [Kronn](https://github.com/DocRoms/Kronn)*"
    , branch = branch)
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
}
