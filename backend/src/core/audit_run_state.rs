//! The audit run, portable by the repository (KT-1199).
//!
//! `<docs>/.kronn-audit-run.json` records where an audit stands: the run, one
//! line per step with the digest of the file it produced, and the KT-1174
//! baselines. It is committed with the docs, so a colleague who fetches sees
//! the run, resumes it or validates it, and an UI refreshed after a rebase
//! follows the repository.
//!
//! Trust (KT-1199 §6): while a run is live, Kronn's memory and database are
//! authoritative and the file is only a projection rewritten at each step —
//! nothing in a run ever reads it back. Only a COMMITTED file (identical to
//! `HEAD`, read at resume or rescan time) is trusted, as the state a human
//! committed. A step whose produced file no longer matches its digest is
//! never trusted: it is shown « à revérifier » and replayed.
//!
//! The file holds no secret and no cost: baselines go through the audit
//! artefact redaction, tokens and prices are not part of the schema.
//!
//! The file is not under `.kronn/`: that folder is excluded locally from Git
//! (`audit_state`), this one must travel.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

pub const RUN_STATE_FILENAME: &str = ".kronn-audit-run.json";
pub const RUN_STATE_SCHEMA: u32 = 1;

const README: &str =
    "Managed by Kronn (https://github.com/DocRoms/Kronn). The audit run in progress \
or last finished: commit it with the docs so a colleague can resume or validate the audit. \
Kronn rewrites it at every step; one line per step keeps merges readable.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Done,
    Unchanged,
    Accepted,
    Failed,
    Pending,
}

impl StepStatus {
    /// The step counts as finished in the run (written, verified or accepted).
    pub fn is_done(self) -> bool {
        matches!(self, Self::Done | Self::Unchanged | Self::Accepted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepState {
    pub status: StepStatus,
    /// The file the step writes, relative to the project root; `Final review`
    /// for the step that writes none.
    pub target: String,
    /// SHA-256 of the produced file when the step finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Who accepted a failed step as it stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

/// `decisions.md` and the `Decision record` lines as the run found them
/// (KT-1174), one element per line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baselines {
    pub decisions: Vec<String>,
    pub records: Vec<String>,
}

impl Baselines {
    pub fn from_text(decisions: &str, records: &str) -> Self {
        let lines = |text: &str| {
            let (clean, _) = crate::core::redact::redact_for_audit_artifact(text);
            if clean.is_empty() {
                Vec::new()
            } else {
                clean.split('\n').map(str::to_string).collect()
            }
        };
        Self {
            decisions: lines(decisions),
            records: lines(records),
        }
    }

    pub fn decisions_text(&self) -> String {
        self.decisions.join("\n")
    }

    pub fn records_text(&self) -> String {
        self.records.join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunState {
    pub schema: u32,
    pub run_id: String,
    /// `Full`, `Partial` or a sub-audit's kind.
    pub kind: String,
    /// `Running`, `Interrupted`, `Completed`, `Failed` or `Cancelled`.
    pub status: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    pub updated_at: String,
    /// Pseudo of the operator whose Kronn started the run.
    pub started_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_version: Option<String>,
    /// SHA-256 of `<docs>/briefing.md` when the run started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing_digest: Option<String>,
    #[serde(default)]
    pub total_steps: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baselines: Option<Baselines>,
    /// Keyed by the zero-padded 1-based step index.
    #[serde(default)]
    pub steps: BTreeMap<String, StepState>,
}

pub fn step_key(index: u32) -> String {
    format!("{index:03}")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The digest of a produced file: `None` for a target that is no regular
/// file (the final review, a missing file), a symbolic link, or a path that
/// leaves the project.
pub fn file_digest(project: &Path, target: &str) -> Option<String> {
    let relative = Path::new(target);
    if target.is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    let path = project.join(relative);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    // A symbolic link in a parent directory passes the check above and would
    // read outside the project: the resolved file must stay inside it.
    let resolved = std::fs::canonicalize(&path).ok()?;
    let root = std::fs::canonicalize(project).ok()?;
    if !resolved.starts_with(&root) {
        return None;
    }
    std::fs::read(&resolved)
        .ok()
        .map(|bytes| sha256_hex(&bytes))
}

impl RunState {
    pub fn step(&self, index: u32) -> Option<&StepState> {
        self.steps.get(&step_key(index))
    }

    /// `(index, step)` in step order.
    pub fn indexed_steps(&self) -> impl Iterator<Item = (u32, &StepState)> {
        self.steps
            .iter()
            .filter_map(|(key, step)| key.parse::<u32>().ok().map(|index| (index, step)))
    }

    /// The deterministic text of the file: top-level keys sorted, one line
    /// per step, one line per baseline line. Same input, same bytes.
    pub fn render(&self) -> Result<String, String> {
        let value = serde_json::to_value(self).map_err(|e| e.to_string())?;
        let serde_json::Value::Object(map) = value else {
            return Err("run state is not an object".into());
        };
        let mut sorted: BTreeMap<String, serde_json::Value> = map.into_iter().collect();
        sorted.insert("_readme".into(), serde_json::Value::String(README.into()));
        let compact = |v: &serde_json::Value| serde_json::to_string(v).map_err(|e| e.to_string());
        let mut lines: Vec<String> = Vec::new();
        for (key, value) in &sorted {
            let name = serde_json::to_string(key).map_err(|e| e.to_string())?;
            let rendered = match (key.as_str(), value) {
                ("steps", serde_json::Value::Object(steps)) if !steps.is_empty() => {
                    let steps: BTreeMap<_, _> = steps.iter().collect();
                    let mut out = String::from("{\n");
                    let last = steps.len() - 1;
                    for (i, (index, step)) in steps.iter().enumerate() {
                        out.push_str(&format!(
                            "    {}: {}{}\n",
                            serde_json::to_string(index).map_err(|e| e.to_string())?,
                            compact(step)?,
                            if i == last { "" } else { "," }
                        ));
                    }
                    out.push_str("  }");
                    out
                }
                ("baselines", value @ serde_json::Value::Object(_)) => {
                    let pretty = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
                    pretty.replace('\n', "\n  ")
                }
                _ => compact(value)?,
            };
            lines.push(format!("  {name}: {rendered}"));
        }
        Ok(format!("{{\n{}\n}}\n", lines.join(",\n")))
    }

    pub fn parse(text: &str) -> Option<Self> {
        let state: Self = serde_json::from_str(text).ok()?;
        (state.schema <= RUN_STATE_SCHEMA && !state.run_id.is_empty()).then_some(state)
    }
}

/// `<docs>/.kronn-audit-run.json` of the project.
pub fn state_path(project: &Path) -> std::path::PathBuf {
    crate::core::docs_location::DocsRoot::of(project).file(project, RUN_STATE_FILENAME)
}

/// Write the state atomically. A configured docs folder failing containment
/// refuses the write (KT-263).
pub fn write(project: &Path, state: &RunState) -> Result<(), String> {
    let dir = crate::core::docs_location::guard_write(project)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join(RUN_STATE_FILENAME);
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{} is a symbolic link", path.display()));
    }
    crate::core::mcp_scanner::atomic_write(&path, &state.render()?)
}

/// What the repository says about the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoProbe {
    /// No state file in the working tree.
    Absent,
    /// A file that is not a readable state (a conflict left in it, a newer
    /// schema): never trusted, never used.
    Unreadable,
    Found {
        state: Box<RunState>,
        /// The file is exactly the one `HEAD` holds: a human committed it.
        committed: bool,
    },
}

/// The file at `HEAD`, when the project is in a Git repository that has it.
fn committed_text(project: &Path) -> Option<String> {
    let rel = crate::core::docs_location::DocsRoot::of(project).path(RUN_STATE_FILENAME);
    let output = crate::core::cmd::git_cmd()
        .args(["show", &format!("HEAD:./{rel}")])
        .current_dir(project)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

/// The state `HEAD` holds, parsed: the copy a human committed, whatever the
/// working tree says since.
pub fn committed_state(project: &Path) -> Option<RunState> {
    RunState::parse(&committed_text(project)?)
}

/// Read the working-tree file and say whether it is the committed one.
pub fn probe(project: &Path) -> RepoProbe {
    let path = state_path(project);
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return RepoProbe::Unreadable;
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return RepoProbe::Absent,
        Err(_) => return RepoProbe::Unreadable,
    };
    let Some(state) = RunState::parse(&text) else {
        return RepoProbe::Unreadable;
    };
    let committed = committed_text(project).is_some_and(|head| head == text);
    RepoProbe::Found {
        state: Box::new(state),
        committed,
    }
}

/// What a step of a repository run is worth on this checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepVerdict {
    /// Done, and the file it produced is still the one it produced.
    Trusted,
    /// Done once, but its file changed or vanished: shown « à revérifier ».
    Recheck,
    Failed,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepAssessment {
    pub index: u32,
    pub target: String,
    pub status: StepStatus,
    pub verdict: StepVerdict,
}

/// Whether a step's target names a file (else it is a label such as
/// `Final review`).
pub fn is_file_target(target: &str) -> bool {
    target.contains('/') || target.ends_with(".md")
}

/// Every step of `state` against the files of `project`.
pub fn assess(project: &Path, state: &RunState) -> Vec<StepAssessment> {
    state
        .indexed_steps()
        .map(|(index, step)| {
            let verdict = match step.status {
                StepStatus::Failed => StepVerdict::Failed,
                StepStatus::Pending => StepVerdict::Pending,
                StepStatus::Done | StepStatus::Unchanged | StepStatus::Accepted => {
                    match step.digest.as_deref() {
                        Some(recorded)
                            if file_digest(project, &step.target).as_deref() == Some(recorded) =>
                        {
                            StepVerdict::Trusted
                        }
                        Some(_) => StepVerdict::Recheck,
                        // Recorded with no digest: the step writes no file
                        // (the final review), or its file was missing.
                        None if is_file_target(&step.target) => StepVerdict::Recheck,
                        None => StepVerdict::Trusted,
                    }
                }
            };
            StepAssessment {
                index,
                target: step.target.clone(),
                status: step.status,
                verdict,
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "audit_run_state_test.rs"]
mod tests;
