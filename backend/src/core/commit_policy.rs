//! Who a Kronn commit is attributed to, and whether it carries a sign-off
//! (KT-1750).
//!
//! Both follow the project the repository belongs to, never a blanket default:
//!
//! - **identity** — `kronn.commitName` + `kronn.commitEmail` in the repository's
//!   own git config when both are set (the per-project override); else the
//!   repository's own `user.name` + `user.email`; else the effective git identity
//!   (the global config), which is used but reported — a global fallback is
//!   never silent; else a Kronn placeholder. It is handed to git as
//!   `GIT_AUTHOR_*` / `GIT_COMMITTER_*`, so no config file is ever written.
//! - **sign-off** — `kronn.signoff` in the repository's git config; else the
//!   project briefing's `signoff_required`; else detected (a DCO file, or most
//!   of the recent history signed); else off.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::cmd::git_cmd;
use crate::models::BriefingYesNo;

/// Identity used when git knows none at all.
const FALLBACK_NAME: &str = "Kronn";
const FALLBACK_EMAIL: &str = "kronn@localhost";

/// Recent non-merge commits read to detect a sign-off habit, and the fewest that
/// make a habit: two signed commits on a fresh repository decide nothing.
const HISTORY_WINDOW: usize = 30;
const HISTORY_MINIMUM: usize = 3;

/// Files that say the repository enforces the Developer Certificate of Origin.
const DCO_MARKERS: [&str; 4] = ["DCO", "DCO.md", ".github/dco.yml", ".github/dco.yaml"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitIdentity {
    pub name: String,
    pub email: String,
}

impl CommitIdentity {
    /// `Name <email>`, the form of a `Signed-off-by` trailer.
    pub fn display(&self) -> String {
        format!("{} <{}>", self.name, self.email)
    }
}

/// Where the identity of a commit comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource {
    /// `kronn.commitName` / `kronn.commitEmail` of the repository.
    Project,
    /// `user.name` / `user.email` of the repository's own config.
    Repository,
    /// Only the global git config knows one: used, but never silently.
    Global,
    /// Git knows none: the Kronn placeholder.
    Fallback,
}

impl IdentitySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Repository => "repository",
            Self::Global => "global",
            Self::Fallback => "fallback",
        }
    }
}

/// Where the sign-off decision comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignoffSource {
    /// `kronn.signoff` of the repository.
    Setting,
    /// The project briefing's `signoff_required`.
    Briefing,
    /// A DCO file at the repository root.
    DcoFile,
    /// The repository's `commit-msg` hook checks for a `Signed-off-by`.
    CommitMsgHook,
    /// Most of the recent history is signed (or none of it is).
    History,
    /// Nothing says otherwise.
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitPolicy {
    pub identity: CommitIdentity,
    pub identity_source: IdentitySource,
    pub signoff: bool,
    pub signoff_source: SignoffSource,
}

impl CommitPolicy {
    /// The policy of the repository at (or around) `repo_path`; a linked
    /// worktree answers for the repository it belongs to.
    pub fn for_repo(repo_path: &Path) -> Self {
        let (identity, identity_source) = resolve_identity(repo_path);
        if identity_source == IdentitySource::Global {
            tracing::warn!(
                path = %repo_path.display(),
                "commit identity {} comes from the global git config: set user.name and user.email in \
                 this repository (or kronn.commitName / kronn.commitEmail) to pick the one it commits as",
                identity.display()
            );
        }
        let (signoff, signoff_source) = resolve_signoff(repo_path);
        Self {
            identity,
            identity_source,
            signoff,
            signoff_source,
        }
    }

    /// Hand the identity to a `git` command. An amend keeps its author: only
    /// the committer is replaced.
    pub fn apply_identity(&self, command: &mut Command, amend: bool) {
        if !amend {
            command.env("GIT_AUTHOR_NAME", &self.identity.name);
            command.env("GIT_AUTHOR_EMAIL", &self.identity.email);
        }
        command.env("GIT_COMMITTER_NAME", &self.identity.name);
        command.env("GIT_COMMITTER_EMAIL", &self.identity.email);
    }
}

/// Run `git config <args>` in `repo_path`: the trimmed, non-empty output.
fn git_config(repo_path: &Path, args: &[&str]) -> Option<String> {
    let output = git_cmd()
        .arg("config")
        .args(args)
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// A name and an e-mail read with `scope` (`--local`, or nothing for the
/// effective config).
fn identity_of(
    repo_path: &Path,
    scope: &[&str],
    name_key: &str,
    email_key: &str,
) -> Option<CommitIdentity> {
    let read = |key: &str| {
        let mut args = scope.to_vec();
        args.extend(["--get", key]);
        git_config(repo_path, &args)
    };
    Some(CommitIdentity {
        name: read(name_key)?,
        email: read(email_key)?,
    })
}

fn resolve_identity(repo_path: &Path) -> (CommitIdentity, IdentitySource) {
    choose_identity(
        identity_of(
            repo_path,
            &["--local"],
            "kronn.commitName",
            "kronn.commitEmail",
        ),
        identity_of(repo_path, &["--local"], "user.name", "user.email"),
        identity_of(repo_path, &[], "user.name", "user.email"),
    )
}

/// The first identity that is known, in order of authority.
fn choose_identity(
    project: Option<CommitIdentity>,
    repository: Option<CommitIdentity>,
    effective: Option<CommitIdentity>,
) -> (CommitIdentity, IdentitySource) {
    if let Some(identity) = project {
        return (identity, IdentitySource::Project);
    }
    if let Some(identity) = repository {
        return (identity, IdentitySource::Repository);
    }
    if let Some(identity) = effective {
        return (identity, IdentitySource::Global);
    }
    (
        CommitIdentity {
            name: FALLBACK_NAME.into(),
            email: FALLBACK_EMAIL.into(),
        },
        IdentitySource::Fallback,
    )
}

fn resolve_signoff(repo_path: &Path) -> (bool, SignoffSource) {
    if let Some(value) = git_config(
        repo_path,
        &["--local", "--type=bool", "--get", "kronn.signoff"],
    ) {
        return (value == "true", SignoffSource::Setting);
    }
    let root = project_root(repo_path);
    match briefing_signoff(&root, repo_path) {
        BriefingYesNo::Yes => return (true, SignoffSource::Briefing),
        BriefingYesNo::No => return (false, SignoffSource::Briefing),
        BriefingYesNo::Unknown => {}
    }
    if DCO_MARKERS
        .iter()
        .any(|marker| repo_path.join(marker).is_file() || root.join(marker).is_file())
    {
        return (true, SignoffSource::DcoFile);
    }
    if commit_msg_hook_requires_signoff(repo_path) {
        return (true, SignoffSource::CommitMsgHook);
    }
    match history_signs_off(repo_path) {
        Some(signed) => (signed, SignoffSource::History),
        None => (false, SignoffSource::Default),
    }
}

/// Whether the repository's own `commit-msg` hook checks for a `Signed-off-by`
/// (a DCO check run at commit time).
fn commit_msg_hook_requires_signoff(repo_path: &Path) -> bool {
    let Some(hook) = git_output(repo_path, &["rev-parse", "--git-path", "hooks/commit-msg"]) else {
        return false;
    };
    let hook = PathBuf::from(hook);
    let hook = if hook.is_absolute() {
        hook
    } else {
        repo_path.join(hook)
    };
    std::fs::read_to_string(hook)
        .is_ok_and(|script| script.to_ascii_lowercase().contains("signed-off-by"))
}

/// The main checkout of the repository `repo_path` belongs to (the worktree
/// itself when git cannot say).
fn project_root(repo_path: &Path) -> PathBuf {
    let Some(common) = git_output(repo_path, &["rev-parse", "--git-common-dir"]) else {
        return repo_path.to_path_buf();
    };
    let common = PathBuf::from(common);
    let common = if common.is_absolute() {
        common
    } else {
        repo_path.join(common)
    };
    match common.file_name() {
        Some(name) if name == ".git" => common
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| repo_path.to_path_buf()),
        _ => repo_path.to_path_buf(),
    }
}

fn git_output(repo_path: &Path, args: &[&str]) -> Option<String> {
    let output = git_cmd().args(args).current_dir(repo_path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// `signoff_required` of the saved briefing: the main checkout's copy first,
/// then the worktree's own.
fn briefing_signoff(root: &Path, repo_path: &Path) -> BriefingYesNo {
    for base in [root, repo_path] {
        let file = crate::core::scanner::detect_docs_dir(base).join("briefing.md");
        let Ok(notes) = std::fs::read_to_string(file) else {
            continue;
        };
        if let Some(answers) = crate::api::audit::project_doc::parse_answers(&notes) {
            return answers.signoff_required;
        }
    }
    BriefingYesNo::Unknown
}

/// Whether most of the recent non-merge commits carry a `Signed-off-by`;
/// `None` when there are too few to tell.
fn history_signs_off(repo_path: &Path) -> Option<bool> {
    let output = git_cmd()
        .args([
            "log",
            "--no-merges",
            "-n",
            &HISTORY_WINDOW.to_string(),
            "--format=%B%x00",
        ])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let messages: Vec<&str> = text
        .split('\0')
        .filter(|message| !message.trim().is_empty())
        .collect();
    if messages.len() < HISTORY_MINIMUM {
        return None;
    }
    let signed = messages
        .iter()
        .filter(|message| {
            message.lines().any(|line| {
                line.trim_start()
                    .to_ascii_lowercase()
                    .starts_with("signed-off-by:")
            })
        })
        .count();
    Some(signed * 2 > messages.len())
}

#[cfg(test)]
#[path = "commit_policy_tests.rs"]
mod tests;
