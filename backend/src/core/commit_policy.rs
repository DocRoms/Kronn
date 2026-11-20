//! Who a Kronn commit is attributed to, and whether it carries a sign-off
//! (KT-1750).
//!
//! Both follow the project the repository belongs to, never a blanket default.
//! The owner sets them in the project card (`ProjectCommitSettings`, stored with
//! the project and handed here by [`register_project`]); what is left unset is
//! detected:
//!
//! - **identity**, the first that is known: (1) the card's name + e-mail;
//!   (2) the repository's own — `kronn.commitName` + `kronn.commitEmail` in its
//!   git config (a detection source, not the way to set it), else its local
//!   `user.name` + `user.email`, so a company repository's identity is never
//!   overwritten by a personal global one; (3) the Kronn identity of Settings >
//!   Workers; (4) the machine's global git config, which is used but reported —
//!   a global fallback is never silent; (5) a Kronn placeholder. It is handed to
//!   git as `GIT_AUTHOR_*` / `GIT_COMMITTER_*`, so no config file is ever written.
//! - **sign-off**, the first that decides: (1) the card's Always / Never;
//!   (2) the repository's own — `kronn.signoff` in its git config, the project
//!   briefing's `signoff_required`, a DCO file, a `commit-msg` hook, or most of
//!   the recent history signed (or unsigned); (3) the Always / Never of Settings
//!   > Workers; (4) off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::RwLock;

use crate::models::{BriefingYesNo, ProjectCommitSettings, SignoffSetting};

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
    /// The name and e-mail set in the project card.
    Setting,
    /// `kronn.commitName` / `kronn.commitEmail` of the repository's git config.
    KronnConfig,
    /// `user.name` / `user.email` of the repository's own config.
    Repository,
    /// An identity git gives this directory only: a conditional include of the
    /// global config (`includeIf` on `gitdir:`, `hasconfig:` or `onbranch:`) or
    /// the worktree config. The repository's own in spirit, so it ranks with it.
    Conditional,
    /// The Kronn identity of Settings > Workers.
    KronnGlobal,
    /// Only the machine's global git config knows one: used, but never silently.
    Global,
    /// Git knows none: the Kronn placeholder.
    Fallback,
}

impl IdentitySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Setting => "setting",
            Self::KronnConfig => "kronn_config",
            Self::Repository => "repository",
            Self::Conditional => "conditional",
            Self::KronnGlobal => "kronn_global",
            Self::Global => "global",
            Self::Fallback => "fallback",
        }
    }
}

/// Where the sign-off decision comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignoffSource {
    /// Always / Never, set in the project card.
    Setting,
    /// `kronn.signoff` of the repository's git config.
    KronnConfig,
    /// The project briefing's `signoff_required`.
    Briefing,
    /// A DCO file at the repository root.
    DcoFile,
    /// The repository's `commit-msg` hook checks for a `Signed-off-by`.
    CommitMsgHook,
    /// Most of the recent history is signed (or none of it is).
    History,
    /// The Always / Never of Settings > Workers.
    KronnGlobal,
    /// Nothing says otherwise.
    Default,
}

impl SignoffSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Setting => "setting",
            Self::KronnConfig => "kronn_config",
            Self::Briefing => "briefing",
            Self::DcoFile => "dco_file",
            Self::CommitMsgHook => "commit_msg_hook",
            Self::History => "history",
            Self::KronnGlobal => "kronn_global",
            Self::Default => "default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitPolicy {
    pub identity: CommitIdentity,
    pub identity_source: IdentitySource,
    pub signoff: bool,
    pub signoff_source: SignoffSource,
}

impl CommitPolicy {
    /// The policy of the repository at (or around) `repo_path`, with the project
    /// card's settings applied; a linked worktree answers for the repository it
    /// belongs to.
    pub fn for_repo(repo_path: &Path) -> Self {
        let policy = Self::resolve(repo_path);
        if policy.uses_global_identity() {
            tracing::warn!(
                path = %repo_path.display(),
                "commit identity {} comes from the machine's git config: set the commit name and \
                 e-mail in the project card (or in Settings > Workers) to pick the one it commits as",
                policy.identity.display()
            );
        }
        policy
    }

    /// [`Self::for_repo`] without the warning a global identity logs.
    pub fn resolve(repo_path: &Path) -> Self {
        Self::detect(repo_path).with_settings(&registered_settings(repo_path).unwrap_or_default())
    }

    /// What a project inherits, its card ignored: the repository's own answers,
    /// then Settings > Workers', then the machine's. The default the card shows
    /// and a setting left on Inherit / Auto follows.
    pub fn detect(repo_path: &Path) -> Self {
        Self::detect_with(repo_path, &global_settings())
    }

    /// [`Self::detect`] with the Settings > Workers commit settings given.
    pub fn detect_with(repo_path: &Path, global: &ProjectCommitSettings) -> Self {
        let (identity, identity_source) = resolve_identity(repo_path, global);
        let (signoff, signoff_source) = inherit_signoff(resolve_signoff(repo_path), global);
        Self {
            identity,
            identity_source,
            signoff,
            signoff_source,
        }
    }

    /// This (detected) policy with the owner's settings laid over it.
    pub fn with_settings(mut self, settings: &ProjectCommitSettings) -> Self {
        if let Some((name, email)) = settings.identity() {
            self.identity = CommitIdentity {
                name: name.to_string(),
                email: email.to_string(),
            };
            self.identity_source = IdentitySource::Setting;
        }
        match settings.signoff {
            SignoffSetting::Auto => {}
            SignoffSetting::Always => {
                self.signoff = true;
                self.signoff_source = SignoffSource::Setting;
            }
            SignoffSetting::Never => {
                self.signoff = false;
                self.signoff_source = SignoffSource::Setting;
            }
        }
        self
    }

    /// Whether the only identity found is the global git config's: a personal
    /// one, on a repository that does not say who commits for it.
    pub fn uses_global_identity(&self) -> bool {
        self.identity_source == IdentitySource::Global
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

/// The commit settings of Settings > Workers, published at boot and on save like
/// the other worker settings: what a project inherits when its card and its
/// repository say nothing.
static GLOBAL_SETTINGS: RwLock<Option<ProjectCommitSettings>> = RwLock::new(None);

pub fn set_global_settings(settings: &ProjectCommitSettings) {
    *GLOBAL_SETTINGS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(settings.clone());
}

pub fn global_settings() -> ProjectCommitSettings {
    GLOBAL_SETTINGS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .unwrap_or_default()
}

/// The Settings > Workers sign-off decides only what the repository left
/// undecided.
fn inherit_signoff(
    detected: (bool, SignoffSource),
    global: &ProjectCommitSettings,
) -> (bool, SignoffSource) {
    if detected.1 != SignoffSource::Default {
        return detected;
    }
    match global.signoff {
        SignoffSetting::Auto => detected,
        SignoffSetting::Always => (true, SignoffSource::KronnGlobal),
        SignoffSetting::Never => (false, SignoffSource::KronnGlobal),
    }
}

/// The project cards' settings by project directory. The merge and worker-commit
/// code runs without a database, so the settings are published here: at boot
/// ([`register_project`] for each project) and whenever a card saves them.
static PROJECT_SETTINGS: RwLock<Option<HashMap<PathBuf, ProjectCommitSettings>>> =
    RwLock::new(None);

fn settings_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Publish the commit settings of the project whose checkout is `project_path`;
/// default settings remove the entry.
pub fn register_project(project_path: &Path, settings: &ProjectCommitSettings) {
    let mut registry = PROJECT_SETTINGS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = settings_key(project_path);
    if settings.is_default() {
        if let Some(map) = registry.as_mut() {
            map.remove(&key);
        }
    } else {
        registry
            .get_or_insert_with(HashMap::new)
            .insert(key, settings.clone());
    }
}

/// The settings of the project `repo_path` belongs to, when its card set any.
pub fn registered_settings(repo_path: &Path) -> Option<ProjectCommitSettings> {
    let registry = PROJECT_SETTINGS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let map = registry.as_ref()?;
    map.get(&settings_key(&project_root(repo_path)))
        .or_else(|| map.get(&settings_key(repo_path)))
        .cloned()
}

/// Run `git config <args>` in `repo_path`: the trimmed, non-empty output. In a
/// worker's worktree git reads the registered repository's configuration, never
/// one the worker pointed `.git` at (KT-1745).
fn git_config(repo_path: &Path, args: &[&str]) -> Option<String> {
    let output = crate::core::lane_git::git_or_refuse(repo_path)
        .arg("config")
        .args(args)
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

fn resolve_identity(
    repo_path: &Path,
    global: &ProjectCommitSettings,
) -> (CommitIdentity, IdentitySource) {
    let effective = identity_of(repo_path, &[], "user.name", "user.email");
    // What git gives every directory: the plain global and system files, their
    // includes left unfollowed. An effective identity that is neither of them
    // comes from a conditional include or the worktree config: it is this
    // directory's own.
    let plain = |scope: &str| {
        identity_of(
            repo_path,
            &[scope, "--no-includes"],
            "user.name",
            "user.email",
        )
    };
    let (directory, machine) = match effective {
        Some(identity)
            if plain("--global").as_ref() != Some(&identity)
                && plain("--system").as_ref() != Some(&identity) =>
        {
            (Some(identity), None)
        }
        other => (None, other),
    };
    choose_identity(
        identity_of(
            repo_path,
            &["--local"],
            "kronn.commitName",
            "kronn.commitEmail",
        ),
        identity_of(repo_path, &["--local"], "user.name", "user.email"),
        directory,
        global.identity().map(|(name, email)| CommitIdentity {
            name: name.to_string(),
            email: email.to_string(),
        }),
        machine,
    )
}

/// The first identity that is known, in order of authority: the repository's own
/// (`kronn.commit*`, then local `user.*`, then what git gives this directory
/// only), the Kronn global setting, the machine's git config.
fn choose_identity(
    kronn_config: Option<CommitIdentity>,
    repository: Option<CommitIdentity>,
    directory: Option<CommitIdentity>,
    kronn_global: Option<CommitIdentity>,
    machine: Option<CommitIdentity>,
) -> (CommitIdentity, IdentitySource) {
    if let Some(identity) = kronn_config {
        return (identity, IdentitySource::KronnConfig);
    }
    if let Some(identity) = repository {
        return (identity, IdentitySource::Repository);
    }
    if let Some(identity) = directory {
        return (identity, IdentitySource::Conditional);
    }
    if let Some(identity) = kronn_global {
        return (identity, IdentitySource::KronnGlobal);
    }
    if let Some(identity) = machine {
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
        return (value == "true", SignoffSource::KronnConfig);
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
    // Not `rev-parse --git-path`: the lane's builder turns `core.hooksPath` off.
    let Some(hooks) = crate::core::commit_hooks::repository_hooks_dir(repo_path) else {
        return false;
    };
    std::fs::read_to_string(hooks.join("commit-msg"))
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
    let output = crate::core::lane_git::git_or_refuse(repo_path)
        .args(args)
        .output()
        .ok()?;
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
        if let Some(answers) = crate::models::projects::parse_answers(&notes) {
            return answers.signoff_required;
        }
    }
    BriefingYesNo::Unknown
}

/// Whether most of the recent non-merge commits carry a `Signed-off-by`;
/// `None` when there are too few to tell.
fn history_signs_off(repo_path: &Path) -> Option<bool> {
    let output = crate::core::lane_git::git_or_refuse(repo_path)
        .args([
            "log",
            "--no-merges",
            "-n",
            &HISTORY_WINDOW.to_string(),
            "--format=%B%x00",
        ])
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
