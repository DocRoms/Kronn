//! Repository profile `kronn/project.toml` (ADR-005 slice 7, KT-920).
//!
//! The profile holds what a generic workflow needs to know about one
//! repository: validation targets, forge labels and rules, tracker statuses
//! and transitions, delivery workflows. Workflows read it as
//! `{{project.<path>}}`.
//!
//! It is read from the main checkout's default branch with git plumbing,
//! never from a working tree: a run's worktree, or a pull request, cannot
//! change the rules of the run that executes it. The ref is, in order, the
//! local branch named by `origin/HEAD`, then `main`, then `master`; the
//! remote-tracking branch of the same name stands in when no local branch
//! exists. `HEAD` is never read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::models::{Workflow, WorkflowStep};

/// Where the profile lives in the repository.
pub const PROFILE_PATH: &str = "kronn/project.toml";
/// The template namespace the profile is exposed under.
pub const NAMESPACE: &str = "project";
/// The only schema version this build reads.
pub const SCHEMA_VERSION: u32 = 1;
/// Larger files are refused: a profile is a few dozen lines.
const MAX_PROFILE_BYTES: usize = 64 * 1024;
/// Upper bound on one string value.
const MAX_VALUE_CHARS: usize = 4096;

/// The repository profile, schema version 1. Every table refuses unknown keys.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectProfile {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<Validation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forge: Option<Forge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<Tracker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Validation {
    /// Named commands a worktree runs to validate a change.
    #[serde(default)]
    pub targets: BTreeMap<String, ValidationTarget>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationTarget {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forge {
    /// Branch pull requests target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_method: Option<MergeMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_approvals: Option<u32>,
    /// Repository-relative path of the pull request template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_template: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, Label>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Label {
    /// The label as the forge spells it.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// CI jobs or workflows that adding the label starts.
    #[serde(default)]
    pub triggers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackerKind {
    Jira,
    Github,
    Gitlab,
    Linear,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tracker {
    pub kind: TrackerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_key: Option<String>,
    /// Repository-relative path of the ticket template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_template: Option<String>,
    /// Logical name → status as the tracker spells it.
    #[serde(default)]
    pub statuses: BTreeMap<String, String>,
    #[serde(default)]
    pub transitions: BTreeMap<String, Transition>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivery {
    #[serde(default)]
    pub workflows: BTreeMap<String, DeliveryWorkflow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryWorkflow {
    /// The CI/CD workflow as the forge names it.
    pub workflow: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
}

/// Parses and validates a profile. Every refusal names the problem.
pub fn parse_profile(text: &str) -> Result<ProjectProfile, String> {
    if text.len() > MAX_PROFILE_BYTES {
        return Err(format!(
            "{PROFILE_PATH} is {} bytes, the limit is {MAX_PROFILE_BYTES}",
            text.len()
        ));
    }
    let profile: ProjectProfile =
        toml::from_str(text).map_err(|error| schema_error(text, &error))?;
    if profile.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{PROFILE_PATH} declares schema_version = {}, this Kronn reads version {SCHEMA_VERSION}",
            profile.schema_version
        ));
    }
    let value = serde_json::to_value(&profile).map_err(|error| error.to_string())?;
    check_value(NAMESPACE, &value)?;
    for path in [
        profile
            .forge
            .as_ref()
            .and_then(|f| f.pr_template.as_deref()),
        profile
            .tracker
            .as_ref()
            .and_then(|t| t.ticket_template.as_deref()),
    ]
    .into_iter()
    .flatten()
    {
        check_repository_path(path)?;
    }
    Ok(profile)
}

/// A refusal that names the key and the position, never a source excerpt nor
/// a value: the file may hold a secret in the wrong place.
fn schema_error(text: &str, error: &toml::de::Error) -> String {
    let at = error
        .span()
        .map(|span| {
            let before = &text.as_bytes()[..span.start.min(text.len())];
            let line = before.iter().filter(|b| **b == b'\n').count() + 1;
            let column = before.iter().rev().take_while(|b| **b != b'\n').count() + 1;
            format!(" (line {line}, column {column})")
        })
        .unwrap_or_default();
    let message = error.message();
    let named = |prefix: &str| {
        message
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('`'))
            .and_then(|rest| rest.split_once('`'))
            .map(|(name, _)| name)
            .filter(|name| valid_key(name) && !crate::core::redact::looks_like_secret(name))
    };
    // Only the schema's own expectation, never the value that broke it.
    let expected = message
        .rsplit_once(", expected ")
        .map(|(_, expected)| expected)
        .filter(|expected| {
            expected.len() <= 200
                && expected
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || " `,_-".contains(c))
        })
        .map(|expected| format!(", expected {expected}"))
        .unwrap_or_default();
    let detail = if message.starts_with("unknown field") {
        match named("unknown field ") {
            Some(name) => format!("unknown key `{name}`"),
            None => "unknown key".to_string(),
        }
    } else if message.starts_with("missing field") {
        match named("missing field ") {
            Some(name) => format!("missing required key `{name}`"),
            None => "missing required key".to_string(),
        }
    } else if message.starts_with("unknown variant") {
        format!("value not allowed{expected}")
    } else if message.starts_with("invalid type") || message.starts_with("invalid value") {
        format!("value of the wrong type{expected}")
    } else {
        "not valid TOML".to_string()
    };
    format!("{PROFILE_PATH} is invalid: {detail}{at}")
}

/// Keys become template path segments; secrets have no place in the file.
fn check_value(path: &str, value: &serde_json::Value) -> Result<(), String> {
    match value {
        serde_json::Value::Object(map) => {
            for (index, (key, item)) in map.iter().enumerate() {
                // A refused key never appears raw: it may be a misplaced secret.
                let child = format!("{path}.{}", shown_key(key, index));
                if !valid_key(key) || crate::core::redact::looks_like_secret(key) {
                    return Err(format!(
                        "{PROFILE_PATH}: key `{child}` must be 1 to 64 characters among a-z, 0-9, `_` and `-`, starting with a letter or a digit"
                    ));
                }
                if crate::core::export_secrets::secret_name(key) {
                    return Err(format!(
                        "{PROFILE_PATH}: key `{child}` looks like a secret; a repository profile holds no secret (keep it in Kronn and name it in the workflow)"
                    ));
                }
                check_value(&child, item)?;
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                check_value(&format!("{path}.{index}"), item)?;
            }
        }
        serde_json::Value::String(text) => {
            if text.chars().count() > MAX_VALUE_CHARS {
                return Err(format!(
                    "{PROFILE_PATH}: `{path}` is longer than {MAX_VALUE_CHARS} characters"
                ));
            }
            if text.contains("{{") || text.contains("}}") {
                return Err(format!(
                    "{PROFILE_PATH}: `{path}` holds a template placeholder; profile values are literal"
                ));
            }
            if crate::core::redact::looks_like_secret(text) {
                return Err(format!(
                    "{PROFILE_PATH}: `{path}` looks like a secret; a repository profile holds no secret"
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// The key as diagnostics may print it: itself when it is a plain key, else
/// a placeholder naming its position in its table.
fn shown_key(key: &str, index: usize) -> String {
    if valid_key(key) && !crate::core::redact::looks_like_secret(key) {
        key.to_string()
    } else {
        format!("<invalid key #{}>", index + 1)
    }
}

fn valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    key.len() <= 64
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn check_repository_path(path: &str) -> Result<(), String> {
    let candidate = Path::new(path);
    let escapes = path.trim().is_empty()
        || path.contains('\\')
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)));
    if escapes {
        return Err(format!(
            "{PROFILE_PATH}: a template path must be relative to the repository root, without `..`"
        ));
    }
    Ok(())
}

/// Every value as `project.<path>` → text. A table or an array is also
/// readable whole, as JSON; its items keep their own paths (`.0`, `.1`).
pub fn template_values(profile: &ProjectProfile) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Ok(value) = serde_json::to_value(profile) {
        flatten(NAMESPACE, &value, &mut out);
    }
    out.remove(NAMESPACE);
    out
}

fn flatten(path: &str, value: &serde_json::Value, out: &mut BTreeMap<String, String>) {
    match value {
        serde_json::Value::Null => {}
        serde_json::Value::String(text) => {
            out.insert(path.to_string(), text.clone());
        }
        serde_json::Value::Object(map) => {
            out.insert(path.to_string(), value.to_string());
            for (key, item) in map {
                flatten(&format!("{path}.{key}"), item, out);
            }
        }
        serde_json::Value::Array(items) => {
            out.insert(path.to_string(), value.to_string());
            for (index, item) in items.iter().enumerate() {
                flatten(&format!("{path}.{index}"), item, out);
            }
        }
        other => {
            out.insert(path.to_string(), other.to_string());
        }
    }
}

/// The profile of one repository, as read from its default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSource {
    /// No profile; `git_ref` is the ref that was read, if one resolved.
    Absent { git_ref: Option<String> },
    Loaded {
        git_ref: String,
        commit: String,
        profile: Box<ProjectProfile>,
    },
}

fn git_output(repo: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = crate::core::cmd::git_cmd()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn ref_exists(repo: &Path, refname: &str) -> bool {
    git_output(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{refname}^{{commit}}"),
        ],
    )
    .is_some()
}

/// The full ref the profile is read from, or `None` outside a git repository
/// or when no default branch exists.
pub fn default_branch_ref(repo: &Path) -> Option<String> {
    let mut names = Vec::new();
    if let Some(head) = git_output(
        repo,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    ) {
        let head = String::from_utf8_lossy(&head).trim().to_string();
        if let Some(name) = head.strip_prefix("refs/remotes/origin/") {
            names.push(name.to_string());
        }
    }
    for name in ["main", "master"] {
        if !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    }
    names.into_iter().find_map(|name| {
        [
            format!("refs/heads/{name}"),
            format!("refs/remotes/origin/{name}"),
        ]
        .into_iter()
        .find(|refname| ref_exists(repo, refname))
    })
}

/// Reads the profile at the main checkout's default branch. An absent file is
/// not an error; a malformed one, a symlink or an oversized one is.
pub fn load_from_default_branch(repo: &Path) -> Result<ProfileSource, String> {
    let Some(git_ref) = default_branch_ref(repo) else {
        return Ok(ProfileSource::Absent { git_ref: None });
    };
    let commit = git_output(
        repo,
        &["rev-parse", "--verify", &format!("{git_ref}^{{commit}}")],
    )
    .map(|out| String::from_utf8_lossy(&out).trim().to_string())
    .ok_or_else(|| format!("cannot resolve {git_ref} in {}", repo.display()))?;
    let listing = git_output(repo, &["ls-tree", "-z", &commit, "--", PROFILE_PATH])
        .ok_or_else(|| format!("cannot list {PROFILE_PATH} at {git_ref}"))?;
    let listing = String::from_utf8_lossy(&listing);
    let Some(entry) = listing.split('\0').find(|line| !line.is_empty()) else {
        return Ok(ProfileSource::Absent {
            git_ref: Some(git_ref),
        });
    };
    // `<mode> <type> <object>\t<path>`: only a regular file is read.
    let mode = entry.split_whitespace().next().unwrap_or_default();
    if mode != "100644" && mode != "100755" {
        return Err(format!(
            "{PROFILE_PATH} at {git_ref} is not a regular file (mode {mode})"
        ));
    }
    let bytes = git_output(
        repo,
        &["cat-file", "blob", &format!("{commit}:{PROFILE_PATH}")],
    )
    .ok_or_else(|| format!("cannot read {PROFILE_PATH} at {git_ref}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("{PROFILE_PATH} at {git_ref} is not UTF-8"))?;
    let profile = parse_profile(&text).map_err(|error| format!("{error} (read at {git_ref})"))?;
    Ok(ProfileSource::Loaded {
        git_ref,
        commit,
        profile: Box::new(profile),
    })
}

/// One `{{project.…}}` read by a step: its path and whether `??` guards it.
fn profile_placeholders(template: &str, out: &mut BTreeMap<String, bool>) {
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return;
        };
        let inner = &after[..end];
        let (path, guarded) = match inner.split_once("??") {
            Some((path, _)) => (path, true),
            None => (inner, false),
        };
        let path = path.split('|').next().unwrap_or_default().trim();
        if path.starts_with("project.") {
            let entry = out.entry(path.to_string()).or_insert(guarded);
            *entry = *entry && guarded;
        }
        rest = &after[end + 2..];
    }
}

fn collect_placeholders(value: &serde_json::Value, out: &mut BTreeMap<String, bool>) {
    match value {
        serde_json::Value::String(text) if text.contains("{{") => profile_placeholders(text, out),
        serde_json::Value::Array(items) => items.iter().for_each(|i| collect_placeholders(i, out)),
        serde_json::Value::Object(map) => map.values().for_each(|i| collect_placeholders(i, out)),
        _ => {}
    }
}

/// `{{project.…}}` paths the workflow's steps read → whether every read is
/// guarded by `??`.
pub fn workflow_placeholders(workflow: &Workflow) -> BTreeMap<String, bool> {
    steps_placeholders(workflow.steps.iter().chain(workflow.on_failure.iter()))
}

/// [`workflow_placeholders`] for a list of steps.
pub fn steps_placeholders<'a>(
    steps: impl IntoIterator<Item = &'a WorkflowStep>,
) -> BTreeMap<String, bool> {
    let mut out = BTreeMap::new();
    for step in steps {
        if let Ok(value) = serde_json::to_value(step) {
            collect_placeholders(&value, &mut out);
        }
    }
    out
}

/// Profiles a run tree read, by project id (`""` for a run without project).
pub type ProfileSnapshots = BTreeMap<String, ProfileSnapshot>;

/// The key a project's profile is pinned and hashed under.
pub fn project_key(project_id: Option<&str>) -> String {
    project_id.unwrap_or_default().to_string()
}

/// What a run read from its project's profile, pinned with the run so a
/// resume or a child sees the same values, or the same absence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSnapshot {
    /// The commit of the selected ref, when one resolved.
    pub git_ref: Option<String>,
    pub commit: Option<String>,
    pub state: SnapshotState,
    /// Why the profile is unusable; sanitized, never a source excerpt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotState {
    Loaded,
    Absent,
    Invalid,
}

impl ProfileSnapshot {
    /// What the run executes with: the state and the values, not the commit,
    /// so an unrelated commit keeps an approval valid.
    pub fn identity(&self) -> serde_json::Value {
        serde_json::json!({ "state": self.state, "values": self.values })
    }
}

/// Reads the profile of `repo` (the run project's main checkout) now.
pub fn snapshot(repo: Option<&Path>) -> ProfileSnapshot {
    let absent = |git_ref: Option<String>| ProfileSnapshot {
        git_ref,
        commit: None,
        state: SnapshotState::Absent,
        error: None,
        values: BTreeMap::new(),
    };
    let Some(repo) = repo else {
        return absent(None);
    };
    match load_from_default_branch(repo) {
        Ok(ProfileSource::Absent { git_ref }) => absent(git_ref),
        Ok(ProfileSource::Loaded {
            git_ref,
            commit,
            profile,
        }) => ProfileSnapshot {
            git_ref: Some(git_ref),
            commit: Some(commit),
            state: SnapshotState::Loaded,
            error: None,
            values: template_values(&profile),
        },
        Err(error) => ProfileSnapshot {
            git_ref: default_branch_ref(repo),
            commit: None,
            state: SnapshotState::Invalid,
            error: Some(error),
            values: BTreeMap::new(),
        },
    }
}

/// The `project.*` values a run gets from `snapshot`. An absent or invalid
/// profile fails the run only when a step reads a key without `??`.
pub fn values_for(
    snapshot: &ProfileSnapshot,
    placeholders: &BTreeMap<String, bool>,
) -> Result<BTreeMap<String, String>, String> {
    let required: BTreeSet<&str> = placeholders
        .iter()
        .filter(|(_, guarded)| !**guarded)
        .map(|(path, _)| path.as_str())
        .collect();
    let listed = |keys: &BTreeSet<&str>| {
        keys.iter()
            .map(|k| format!("`{{{{{k}}}}}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let read_at = snapshot
        .git_ref
        .as_deref()
        .unwrap_or("any default branch (main, master or origin/HEAD)");
    match snapshot.state {
        SnapshotState::Invalid if required.is_empty() => {
            tracing::warn!(
                "repository profile not loaded: {}",
                snapshot.error.as_deref().unwrap_or_default()
            );
            Ok(BTreeMap::new())
        }
        SnapshotState::Invalid => Err(format!(
            "The workflow reads {} but the repository profile cannot be used: {}",
            listed(&required),
            snapshot.error.as_deref().unwrap_or("unreadable")
        )),
        SnapshotState::Absent if !required.is_empty() => Err(format!(
            "The workflow reads {} but the project has no {PROFILE_PATH} on {read_at}. Add it to the default branch through a pull request.",
            listed(&required),
        )),
        SnapshotState::Absent => Ok(BTreeMap::new()),
        SnapshotState::Loaded => {
            let missing: BTreeSet<&str> = required
                .iter()
                .copied()
                .filter(|key| !snapshot.values.contains_key(*key))
                .collect();
            if !missing.is_empty() {
                return Err(format!(
                    "The workflow reads {} but {PROFILE_PATH} at {read_at} has no such key",
                    listed(&missing)
                ));
            }
            Ok(snapshot.values.clone())
        }
    }
}

/// Snapshots of the profiles of `keys` (project ids, `""` for none): paths
/// read on one connection, git read afterwards, outside any connection.
pub async fn snapshots_for_keys(
    db: &crate::db::Database,
    keys: BTreeSet<String>,
) -> anyhow::Result<ProfileSnapshots> {
    if keys.is_empty() {
        return Ok(ProfileSnapshots::new());
    }
    let repos = db
        .with_read_conn(move |conn| {
            let mut repos = Vec::new();
            for key in keys {
                let path = if key.is_empty() {
                    None
                } else {
                    crate::db::projects::get_project(conn, &key)?
                        .map(|project| project.path)
                        .filter(|path| !path.is_empty())
                };
                repos.push((key, path));
            }
            Ok(repos)
        })
        .await?;
    Ok(tokio::task::spawn_blocking(move || {
        repos
            .into_iter()
            .map(|(key, path)| {
                let repo = path
                    .map(|path| crate::core::scanner::resolve_host_path(&path))
                    .filter(|repo| repo.exists());
                (key, snapshot(repo.as_deref()))
            })
            .collect()
    })
    .await?)
}

/// The profiles every `(workflow, project)` would read, resolved now: what an
/// approval-grade fingerprint and the pin that follows must share.
pub async fn resolve_for(
    db: &crate::db::Database,
    targets: Vec<(Workflow, Option<String>)>,
) -> anyhow::Result<ProfileSnapshots> {
    let keys = db
        .with_read_conn(move |conn| {
            let mut keys = BTreeSet::new();
            for (workflow, project_id) in &targets {
                keys.extend(crate::workflows::run_pins::profile_projects(
                    conn,
                    workflow,
                    project_id.as_deref(),
                )?);
            }
            Ok(keys)
        })
        .await?;
    snapshots_for_keys(db, keys).await
}

/// [`snapshot`] then [`values_for`], for a run that pins nothing.
pub fn run_template_values(
    repo: Option<&Path>,
    placeholders: &BTreeMap<String, bool>,
) -> Result<BTreeMap<String, String>, String> {
    values_for(&snapshot(repo), placeholders)
}

/// A starter profile for a repository whose default branch has none. It is
/// only returned: the audit keeps it with its run, and a human publishes it.
pub fn draft_if_missing(repo: &Path) -> Option<String> {
    // A malformed profile is for a human to fix; without a default branch
    // nothing would ever read the draft.
    if !matches!(
        load_from_default_branch(repo),
        Ok(ProfileSource::Absent { git_ref: Some(_) })
    ) {
        return None;
    }
    let text = proposal_text(repo);
    match parse_profile(&text) {
        Ok(_) => Some(text),
        Err(error) => {
            tracing::warn!("repository profile draft discarded: {error}");
            None
        }
    }
}

/// Keeps the audit's draft with its run, readable from the audit result.
pub async fn record_audit_draft(
    db: &crate::db::Database,
    audit_run_id: &str,
    repo: PathBuf,
) -> anyhow::Result<bool> {
    let Some(draft) = tokio::task::spawn_blocking(move || draft_if_missing(&repo)).await? else {
        return Ok(false);
    };
    let run_id = audit_run_id.to_string();
    db.with_conn(move |conn| {
        crate::db::audit_runs::set_project_profile_draft(conn, &run_id, &draft)
    })
    .await?;
    Ok(true)
}

/// A starter profile: what the repository shows for sure, the rest commented.
fn proposal_text(repo: &Path) -> String {
    let mut text = String::from(
        "# Repository profile read by Kronn workflows as {{project.<path>}}.\n\
         # Drafted by the Kronn audit: review it, then merge it through a pull request.\n\
         # Kronn reads it from the default branch only. No secret belongs here.\n\
         schema_version = 1\n",
    );
    let targets = detected_validation_targets(repo);
    if !targets.is_empty() {
        text.push_str("\n[validation.targets]\n");
        for (name, command) in targets {
            text.push_str(&format!("{name} = {{ command = {command:?} }}\n"));
        }
    }
    let base_branch = default_branch_ref(repo)
        .and_then(|r| {
            r.strip_prefix("refs/heads/")
                .or_else(|| r.strip_prefix("refs/remotes/origin/"))
                .map(str::to_string)
        })
        .filter(|name| !name.is_empty());
    let pr_template = [
        ".github/pull_request_template.md",
        ".github/PULL_REQUEST_TEMPLATE.md",
        "docs/pull_request_template.md",
    ]
    .into_iter()
    .find(|path| repo.join(path).is_file());
    text.push_str("\n[forge]\n");
    if let Some(branch) = base_branch {
        text.push_str(&format!("base_branch = {branch:?}\n"));
    }
    if let Some(path) = pr_template {
        text.push_str(&format!("pr_template = {path:?}\n"));
    }
    text.push_str(
        "# merge_method = \"rebase\"        # merge | squash | rebase\n\
         # required_approvals = 1\n\
         # [forge.labels.ready]\n\
         # name = \"to-test\"\n\
         # triggers = [\"ci-test\"]\n\
         \n\
         # [tracker]\n\
         # kind = \"jira\"                   # jira | github | gitlab | linear\n\
         # project_key = \"ABC\"\n\
         # [tracker.statuses]\n\
         # review = \"To Review\"\n\
         # [tracker.transitions.deploy]\n\
         # from = \"To Deploy\"\n\
         # to = \"Deployed\"\n\
         \n\
         # [delivery.workflows.staging]\n\
         # workflow = \"deploy-staging\"\n\
         # inputs = { environment = \"staging\" }\n",
    );
    text
}

/// Validation commands the repository declares itself, by conventional name.
fn detected_validation_targets(repo: &Path) -> Vec<(String, String)> {
    const NAMES: [&str; 5] = ["lint", "typecheck", "test", "build", "check"];
    let mut out: Vec<(String, String)> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(repo.join("package.json")) {
        let runner = if repo.join("pnpm-lock.yaml").is_file() {
            "pnpm"
        } else if repo.join("yarn.lock").is_file() {
            "yarn"
        } else {
            "npm run"
        };
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) {
                for name in NAMES {
                    if scripts.contains_key(name) {
                        out.push((name.to_string(), format!("{runner} {name}")));
                    }
                }
            }
        }
    }
    if out.is_empty() && repo.join("Cargo.toml").is_file() {
        out.push(("test".into(), "cargo test".into()));
        out.push(("lint".into(), "cargo clippy --all-targets".into()));
    }
    if out.is_empty() {
        if let Ok(text) = std::fs::read_to_string(repo.join("Makefile")) {
            for name in NAMES {
                if text
                    .lines()
                    .any(|line| line.starts_with(&format!("{name}:")))
                {
                    out.push((name.to_string(), format!("make {name}")));
                }
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "project_profile_test.rs"]
mod tests;
