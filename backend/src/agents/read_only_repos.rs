use std::path::{Path, PathBuf};

use crate::models::AgentType;

/// Invocation-local policy shared by the adapters and direct CLI route.
#[derive(Debug)]
pub(crate) struct ReadOnlyRepos {
    roots: Vec<PathBuf>,
}

/// A registered project's Git checkout that passed validation: the canonical
/// root and the identity (device, inode) it had then. The launch opens this
/// root, never the path the project was registered under, and refuses it if
/// the directory behind it is no longer the one that was validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCheckout {
    root: PathBuf,
    device: u64,
    inode: u64,
}

impl VerifiedCheckout {
    /// Validate a registered project path as an openable checkout: a path
    /// that is, or passes through, a symbolic link is refused (it could lead
    /// to another project's checkout), as is anything that is not the root of
    /// a Git checkout or overlaps the working directory.
    pub(crate) fn verify(
        agent: &AgentType,
        work_dir: &Path,
        location: &str,
    ) -> Result<Self, String> {
        check_platform(agent)?;
        let work_dir = canonical_directory(work_dir)?;
        let path = crate::core::scanner::resolve_host_path(location);
        if !path.is_absolute() || location.trim().is_empty() {
            return Err(format!(
                "a project checkout needs an absolute local path: {location:?}"
            ));
        }
        refuse_redirection(&path)?;
        let root = canonical_directory(&path)?;
        refuse_overlap(&root, &work_dir)?;
        let output = crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--path-format=absolute", "--show-toplevel"])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .output()
            .map_err(|error| format!("Cannot inspect the checkout's Git metadata: {error}"))?;
        let toplevel = String::from_utf8(output.stdout)
            .map_err(|_| "a project checkout's Git paths must be UTF-8".to_string())?;
        if !output.status.success()
            || canonical_directory(Path::new(toplevel.trim()))
                .ok()
                .as_ref()
                != Some(&root)
        {
            return Err(format!(
                "a project checkout must be the root of a Git checkout: {}",
                root.display()
            ));
        }
        let (device, inode) = identity(&root)?;
        Ok(Self {
            root,
            device,
            inode,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The directory behind the validated root is still the validated one.
    fn still_holds(&self) -> Result<(), String> {
        refuse_redirection(&self.root)?;
        let again = canonical_directory(&self.root)?;
        if again != self.root || identity(&again)? != (self.device, self.inode) {
            return Err(format!(
                "the project checkout {} changed since it was validated; it is not opened",
                self.root.display()
            ));
        }
        Ok(())
    }
}

fn check_platform(agent: &AgentType) -> Result<(), String> {
    if !matches!(agent, AgentType::ClaudeCode | AgentType::Codex) {
        return Err("read_only_repos requires Claude Code or Codex".into());
    }
    if cfg!(windows) {
        return Err("read_only_repos requires a native macOS or Linux runner".into());
    }
    Ok(())
}

fn refuse_overlap(root: &Path, work_dir: &Path) -> Result<(), String> {
    // Denying writes on an ancestor would also deny the worktree.
    if work_dir.starts_with(root) || root.starts_with(work_dir) {
        return Err(format!(
            "read_only_repos path {} overlaps the writable working directory {}",
            root.display(),
            work_dir.display()
        ));
    }
    Ok(())
}

/// No component of `path`, itself included, may be a symbolic link, nor a
/// `.`/`..` step: resolving one of them is how a registered project path ends
/// up naming another project's checkout.
fn refuse_redirection(path: &Path) -> Result<(), String> {
    let mut walked = PathBuf::new();
    for component in path.components() {
        if matches!(
            component,
            std::path::Component::CurDir | std::path::Component::ParentDir
        ) {
            return Err(format!(
                "a project checkout path cannot contain `.` or `..`: {}",
                path.display()
            ));
        }
        walked.push(component);
        let metadata = std::fs::symlink_metadata(&walked).map_err(|error| {
            format!(
                "Cannot inspect the project checkout path {}: {error}",
                walked.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "the project checkout path {} goes through a symbolic link ({}); it is not opened",
                path.display(),
                walked.display()
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn identity(path: &Path) -> Result<(u64, u64), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("Cannot inspect {}: {error}", path.display()))?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity(_path: &Path) -> Result<(u64, u64), String> {
    Err("read_only_repos requires a native macOS or Linux runner".into())
}

impl ReadOnlyRepos {
    #[cfg(test)]
    pub(crate) fn resolve(
        agent: &AgentType,
        work_dir: &Path,
        declared: &[String],
    ) -> Result<Option<Self>, String> {
        Self::resolve_with_dirs(agent, work_dir, declared, &[])
    }

    /// `declared` Git checkouts plus plain `dirs` (no Git metadata, e.g. the
    /// run's artifacts directory), all read-only under one policy.
    pub(crate) fn resolve_with_dirs(
        agent: &AgentType,
        work_dir: &Path,
        declared: &[String],
        dirs: &[String],
    ) -> Result<Option<Self>, String> {
        if declared.is_empty() && dirs.is_empty() {
            return Ok(None);
        }
        check_platform(agent)?;
        let work_dir = canonical_directory(work_dir)?;
        let mut roots = Vec::new();
        for location in dirs {
            let path = Path::new(location);
            if !path.is_absolute() {
                return Err(format!(
                    "read-only directory must be absolute: {location:?}"
                ));
            }
            let root = canonical_directory(path)?;
            if work_dir.starts_with(&root) || root.starts_with(&work_dir) {
                return Err(format!(
                    "read-only directory {} overlaps the writable working directory {}",
                    root.display(),
                    work_dir.display()
                ));
            }
            push_unique(&mut roots, root);
        }
        for location in declared {
            let path = crate::core::scanner::resolve_host_path(location);
            if !path.is_absolute() || location.trim().is_empty() {
                return Err(format!(
                    "read_only_repos requires an absolute local path: {location:?}"
                ));
            }
            let root = canonical_directory(&path)?;
            // Denying writes on an ancestor would also deny the worktree.
            if work_dir.starts_with(&root) || root.starts_with(&work_dir) {
                return Err(format!(
                    "read_only_repos path {} overlaps the writable working directory {}",
                    root.display(),
                    work_dir.display()
                ));
            }
            push_unique(&mut roots, root.clone());
            // A linked Git worktree stores its history outside its checkout.
            let output = crate::core::cmd::git_cmd()
                .arg("-C")
                .arg(&root)
                .args([
                    "rev-parse",
                    "--path-format=absolute",
                    "--show-toplevel",
                    "--git-dir",
                    "--git-common-dir",
                ])
                .env("GIT_OPTIONAL_LOCKS", "0")
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_COMMON_DIR")
                .output()
                .map_err(|error| format!("Cannot inspect read_only_repos Git metadata: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "read_only_repos path is not a Git repository: {}",
                    root.display()
                ));
            }
            let metadata = String::from_utf8(output.stdout)
                .map_err(|_| "read_only_repos Git paths must be UTF-8".to_string())?;
            let mut paths = metadata.lines();
            let toplevel = paths
                .next()
                .ok_or("read_only_repos Git checkout root is missing")?;
            if canonical_directory(Path::new(toplevel))? != root {
                return Err(format!(
                    "read_only_repos must name a Git checkout root: {}",
                    root.display()
                ));
            }
            for line in paths {
                let path = canonical_directory(Path::new(line))?;
                if work_dir.starts_with(&path) || path.starts_with(&work_dir) {
                    return Err(
                        "read_only_repos Git metadata overlaps the writable working directory"
                            .into(),
                    );
                }
                if !path.starts_with(&root) {
                    push_unique(&mut roots, path);
                }
            }
        }
        Ok(Some(Self { roots }))
    }

    /// KT-1406 — the policy of a discussion's native Claude Code agent: the
    /// granted projects' validated checkouts as read-only roots. Each
    /// checkout is checked again here, at the launch: one whose directory is
    /// no longer the validated one is refused. Codex keeps its default
    /// sandbox (its read isolation is a separate work item), so it never gets
    /// a room policy.
    pub(crate) fn resolve_room(
        agent: &AgentType,
        work_dir: &Path,
        checkouts: &[VerifiedCheckout],
    ) -> Result<Option<Self>, String> {
        if checkouts.is_empty() {
            return Ok(None);
        }
        if !matches!(agent, AgentType::ClaudeCode) {
            return Err("room checkouts are only opened for Claude Code".into());
        }
        check_platform(agent)?;
        let work_dir = canonical_directory(work_dir)?;
        let mut roots = Vec::new();
        for checkout in checkouts {
            checkout.still_holds()?;
            refuse_overlap(&checkout.root, &work_dir)?;
            push_unique(&mut roots, checkout.root.clone());
        }
        Ok(Some(Self { roots }))
    }

    pub(crate) fn apply(&self, agent: &AgentType, work_dir: &Path, args: &mut Vec<String>) {
        let prompt = args.pop().expect("command builder ends with a prompt");
        args.retain(|arg| {
            arg != "--dangerously-skip-permissions" && !arg.starts_with("--sandbox=")
        });
        args.extend(self.args(agent, work_dir));
        args.push(prompt);
    }

    pub(crate) fn args(&self, agent: &AgentType, work_dir: &Path) -> Vec<String> {
        match agent {
            AgentType::ClaudeCode => {
                let deny: Vec<String> = self
                    .roots
                    .iter()
                    .map(|path| format!("Edit(/{}/**)", path.display()))
                    .collect();
                let settings = serde_json::json!({
                    "permissions": {
                        "blockReadsOutsideWorkingDirectories": true,
                        "deny": deny
                    },
                    "sandbox": {
                        "enabled": true,
                        "failIfUnavailable": true,
                        "autoAllowBashIfSandboxed": true,
                        "allowUnsandboxedCommands": false,
                        "excludedCommands": [],
                        "filesystem": {
                            "disabled": false,
                            "allowWrite": [work_dir],
                            "denyWrite": self.roots
                        }
                    }
                });
                let mut args = vec![
                    "--setting-sources".into(),
                    String::new(),
                    "--settings".into(),
                    settings.to_string(),
                    "--permission-mode".into(),
                    "acceptEdits".into(),
                ];
                for root in &self.roots {
                    args.extend(["--add-dir".into(), root.display().to_string()]);
                }
                args
            }
            AgentType::Codex => {
                // --add-dir is a WRITE grant in Codex. Explicit read entries
                // also protect repositories located below /tmp or TMPDIR.
                let mut args = vec![
                    "--ignore-user-config".into(),
                    "--ignore-rules".into(),
                    "--strict-config".into(),
                ];
                for setting in [
                    "approval_policy=\"never\"",
                    "default_permissions=\"kronn_read_only_repos\"",
                ] {
                    args.extend(["-c".into(), setting.into()]);
                }
                let mut filesystem = vec!["\":root\"=\"read\"".to_string()];
                for root in &self.roots {
                    filesystem.push(format!(
                        "{}=\"read\"",
                        serde_json::to_string(root).expect("validated UTF-8 path")
                    ));
                }
                args.extend(["-c".into(), format!(
                    "permissions={{kronn_read_only_repos={{extends=\":workspace\",workspace_roots={{}},filesystem={{{}}}}}}}",
                    filesystem.join(",")
                )]);
                args
            }
            _ => unreachable!("provider checked at resolution"),
        }
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "Cannot resolve read_only_repos path {}: {error}",
            path.display()
        )
    })?;
    if !canonical.is_dir() || canonical.to_str().is_none() {
        return Err(format!(
            "read_only_repos requires a UTF-8 directory: {}",
            path.display()
        ));
    }
    // Claude permission paths are patterns. Refuse ambiguous literal names.
    if canonical
        .to_string_lossy()
        .contains(['*', '?', '[', ']', '(', ')', '\n', '\r', '\\'])
    {
        return Err(format!(
            "read_only_repos path contains permission-pattern characters: {}",
            path.display()
        ));
    }
    Ok(canonical)
}

fn push_unique(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if !roots.contains(&path) {
        roots.push(path);
    }
}

#[cfg(all(test, unix, feature = "full-tests"))]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        let repo = temp.path().join("API équipe");
        std::fs::create_dir(&work).unwrap();
        std::fs::create_dir(&repo).unwrap();
        let output = crate::core::cmd::git_cmd()
            .arg("init")
            .arg(&repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        (
            temp,
            work.canonicalize().unwrap(),
            repo.canonicalize().unwrap(),
        )
    }

    #[test]
    fn read_only_repos_empty_preserves_legacy_providers() {
        assert!(
            ReadOnlyRepos::resolve(&AgentType::Ollama, Path::new("missing"), &[])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn read_only_repos_rejects_missing_relative_non_git_and_overlapping_paths() {
        let (temp, work, repo) = fixture();
        let plain = temp.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        for path in [
            "relative".into(),
            "https://example.com/repo".into(),
            String::new(),
            temp.path().join("missing").display().to_string(),
            plain.display().to_string(),
            work.display().to_string(),
            temp.path().display().to_string(),
        ] {
            assert!(
                ReadOnlyRepos::resolve(&AgentType::ClaudeCode, &work, std::slice::from_ref(&path))
                    .is_err(),
                "{path}"
            );
        }
        assert!(
            ReadOnlyRepos::resolve(&AgentType::Ollama, &work, &[repo.display().to_string()])
                .is_err()
        );
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&work, &alias).unwrap();
        assert!(
            ReadOnlyRepos::resolve(&AgentType::Codex, &work, &[alias.display().to_string()])
                .is_err()
        );
    }

    #[test]
    fn read_only_repos_canonicalizes_aliases_and_deduplicates() {
        let (temp, work, repo) = fixture();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&repo, &alias).unwrap();
        let policy = ReadOnlyRepos::resolve(
            &AgentType::ClaudeCode,
            &work,
            &[repo.display().to_string(), alias.display().to_string()],
        )
        .unwrap()
        .unwrap();
        assert_eq!(policy.roots, vec![repo]);
    }

    #[test]
    fn read_only_repos_claude_blocks_tools_and_subprocess_writes_without_disabling_read_block() {
        let (_temp, work, repo) = fixture();
        let policy =
            ReadOnlyRepos::resolve(&AgentType::ClaudeCode, &work, &[repo.display().to_string()])
                .unwrap()
                .unwrap();
        let mut args = vec!["--dangerously-skip-permissions".into(), "prompt".into()];
        policy.apply(&AgentType::ClaudeCode, &work, &mut args);
        assert!(!args.iter().any(|a| a == "--dangerously-skip-permissions"));
        assert_eq!(args.last().unwrap(), "prompt");
        assert!(args
            .windows(2)
            .any(|p| p[0] == "--add-dir" && p[1] == repo.to_str().unwrap()));
        let settings: serde_json::Value =
            serde_json::from_str(&args[args.iter().position(|a| a == "--settings").unwrap() + 1])
                .unwrap();
        assert_eq!(
            settings["permissions"]["blockReadsOutsideWorkingDirectories"],
            true
        );
        assert_eq!(
            settings["permissions"]["deny"],
            serde_json::json!([format!("Edit(/{}/**)", repo.display())])
        );
        assert_eq!(
            settings["sandbox"]["filesystem"]["denyWrite"],
            serde_json::json!([repo])
        );
        assert_eq!(
            settings["sandbox"]["filesystem"]["allowWrite"],
            serde_json::json!([work])
        );
        assert_eq!(settings["sandbox"]["failIfUnavailable"], true);
        assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
        assert_eq!(
            settings["sandbox"]["excludedCommands"],
            serde_json::json!([])
        );
    }

    #[test]
    fn read_only_repos_codex_uses_read_entries_even_under_writable_temporary_roots() {
        let (_temp, work, repo) = fixture();
        let policy =
            ReadOnlyRepos::resolve(&AgentType::Codex, &work, &[repo.display().to_string()])
                .unwrap()
                .unwrap();
        let mut args = vec![
            "exec".into(),
            "--sandbox=danger-full-access".into(),
            "prompt".into(),
        ];
        policy.apply(&AgentType::Codex, &work, &mut args);
        assert!(!args
            .iter()
            .any(|arg| arg.starts_with("--sandbox") || arg == "--add-dir"));
        assert!(args.contains(&"approval_policy=\"never\"".into()));
        assert!(args.contains(&"--ignore-user-config".into()));
        assert!(args.contains(&"--strict-config".into()));
        let permissions: toml::Value = toml::from_str(
            args.iter()
                .find(|arg| arg.starts_with("permissions="))
                .unwrap(),
        )
        .unwrap();
        let profile = &permissions["permissions"]["kronn_read_only_repos"];
        assert_eq!(profile["extends"].as_str(), Some(":workspace"));
        // KT-1406: the Codex policy is the workflow one, unchanged.
        assert_eq!(profile["filesystem"][":root"].as_str(), Some("read"));
        assert_eq!(profile["filesystem"].as_table().unwrap().len(), 2);
        assert_eq!(
            profile["filesystem"][repo.to_str().unwrap()].as_str(),
            Some("read")
        );
        assert!(profile["workspace_roots"].as_table().unwrap().is_empty());
    }

    #[test]
    fn read_only_repos_includes_external_git_history_for_a_linked_worktree() {
        let (temp, work, repo) = fixture();
        let output = crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(&repo)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "fixture",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let linked = temp.path().join("linked");
        let output = crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "--detach"])
            .arg(&linked)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let policy = ReadOnlyRepos::resolve(
            &AgentType::ClaudeCode,
            &work,
            &[linked.display().to_string()],
        )
        .unwrap()
        .unwrap();
        assert!(policy.roots.contains(&linked.canonicalize().unwrap()));
        assert!(policy
            .roots
            .contains(&repo.join(".git").canonicalize().unwrap()));
    }

    #[test]
    fn a_plain_directory_is_readable_and_never_writable() {
        let (temp, work, _repo) = fixture();
        let artifacts = temp.path().join("run artifacts");
        std::fs::create_dir(&artifacts).unwrap();
        std::fs::write(artifacts.join("shot.png"), [0x89, b'P', b'N', b'G']).unwrap();
        let artifacts = artifacts.canonicalize().unwrap();
        let dirs = [artifacts.display().to_string()];
        let policy = ReadOnlyRepos::resolve_with_dirs(&AgentType::ClaudeCode, &work, &[], &dirs)
            .unwrap()
            .expect("a directory alone needs the policy");
        let args = policy.args(&AgentType::ClaudeCode, &work);
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--add-dir" && pair[1] == artifacts.to_str().unwrap()));
        let settings: serde_json::Value =
            serde_json::from_str(&args[args.iter().position(|a| a == "--settings").unwrap() + 1])
                .unwrap();
        assert_eq!(
            settings["permissions"]["deny"],
            serde_json::json!([format!("Edit(/{}/**)", artifacts.display())])
        );
        assert_eq!(
            settings["sandbox"]["filesystem"]["denyWrite"],
            serde_json::json!([artifacts])
        );
        assert_eq!(
            settings["sandbox"]["filesystem"]["allowWrite"],
            serde_json::json!([work])
        );
        let codex = policy.args(&AgentType::Codex, &work).join(" ");
        assert!(
            codex.contains(&format!(
                "{}=\"read\"",
                serde_json::to_string(&artifacts).unwrap()
            )),
            "{codex}"
        );
        assert!(
            !codex.contains("--add-dir"),
            "--add-dir is a write grant in Codex"
        );

        for bad in [
            "relative/dir".to_string(),
            work.display().to_string(),
            temp.path().display().to_string(),
        ] {
            assert!(
                ReadOnlyRepos::resolve_with_dirs(
                    &AgentType::ClaudeCode,
                    &work,
                    &[],
                    std::slice::from_ref(&bad)
                )
                .is_err(),
                "{bad}"
            );
        }
    }
}
