// Persistent Kronn-side state for a project, stored as `docs/.kronn.json`
// (or `doc/.kronn.json` / `ai/.kronn.json` depending on the project's docs
// convention, resolved via `scanner::detect_docs_dir`).
//
// Why a side-file instead of HTML markers in `docs/AGENTS.md`:
//   1. AGENTS.md is read by every agent prompt — we do not want to pay
//      tokens for audit history that has no semantic value to the agent.
//   2. HTML comments invite humans to "clean up the noise". A named JSON
//      file with an inline `_readme` field makes the ownership explicit.
//   3. Survives `git clone` so teammates running Kronn see the same audit
//      state without a DB sync.
//
// Anti-fragility notes:
//   - All reads are tolerant: missing/malformed file → `None`, never panic.
//   - Writes preserve unknown fields by round-tripping through `Value`
//     when we land in 0.9 features that extend the schema; for now we
//     only round-trip known fields.
//   - The `_readme` line is rewritten on every write so a teammate who
//     manually edits the JSON and drops it still gets the warning back.

use serde::{Deserialize, Serialize};
use std::path::Path;
use ts_rs::TS;

/// Inline marker on every `.kronn.json` so a human opening the file
/// understands its purpose without consulting external docs.
pub const KRONN_STATE_README: &str = "Managed by Kronn (https://github.com/DocRoms/Kronn). \
Tracks audit/validation state across machines. Do not delete or gitignore — required for \
accurate audit status when this repo is cloned to another Kronn instance.";

/// File name (always under `docs/` — resolved via `detect_docs_dir`).
pub const KRONN_STATE_FILENAME: &str = ".kronn.json";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AuditProvenance {
    /// Produced by Kronn's audit pipeline. This is the backward-compatible
    /// default for state written before provenance became explicit.
    #[default]
    KronnAudit,
    /// A human explicitly attested that the existing documentation is usable.
    HumanAttestation,
    /// Imported from pre-.kronn.json checksums or HTML markers.
    LegacyEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct AuditEntry {
    /// ISO date `YYYY-MM-DD` — when the audit completed.
    pub date: String,
    /// Kronn version that wrote this entry (`CARGO_PKG_VERSION`).
    pub kronn_version: String,
    /// Free-form discriminator: `"full"`, `"partial"`, `"legacy"`, ...
    #[serde(rename = "type")]
    pub audit_type: String,
    #[serde(default)]
    pub provenance: AuditProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct KronnState {
    /// Inline self-explanation — present on every write, ignored on read
    /// (no semantic meaning). Field name starts with `_` so a human
    /// scanning the JSON spots it first.
    #[serde(rename = "_readme", default, skip_serializing_if = "String::is_empty")]
    pub readme: String,

    #[serde(default)]
    pub audits: Vec<AuditEntry>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrapped_at: Option<String>,
}

impl KronnState {
    /// Refresh the README to the canonical text — called before every write
    /// so teammates editing by hand get the warning re-injected.
    fn touch_readme(&mut self) {
        self.readme = KRONN_STATE_README.to_string();
    }

    pub fn has_any_audit(&self) -> bool {
        !self.audits.is_empty()
    }
}

pub fn state_path(project_path: &Path) -> std::path::PathBuf {
    crate::core::scanner::detect_docs_dir(project_path).join(KRONN_STATE_FILENAME)
}

/// Read `docs/.kronn.json` if present and parseable. Any I/O or JSON error
/// returns `None` — callers fall back to legacy detection paths.
pub fn read(project_path: &Path) -> Option<KronnState> {
    read_for_mutation(project_path).ok().flatten()
}

/// Like `read`, but distinguishes a MISSING file (`Ok(None)` — mutators may
/// start from default) from an unreadable/corrupt one (`Err` — mutators must
/// abort rather than rebuild from default and clobber the existing audit
/// history: `bootstrapped_at`, `validated_at`, audits).
fn read_for_mutation(project_path: &Path) -> Result<Option<KronnState>, String> {
    let path = state_path(project_path);
    match std::fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data)
            .map(Some)
            .map_err(|e| format!("{} exists but is not valid JSON: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

/// Load state for a mutation: the legacy evidence (or default) for a missing
/// file, `Err` (with a warn) for an unreadable/corrupt one so the caller aborts
/// instead of clobbering. Seeding from legacy evidence keeps a marker-only
/// project's Validated/Bootstrapped state when its first state file is written.
fn load_for_mutation(project_path: &Path) -> Result<KronnState, String> {
    read_for_mutation(project_path)
        .map(|state| {
            state
                .or_else(|| legacy_state(project_path))
                .unwrap_or_default()
        })
        .inspect_err(|e| {
            tracing::warn!("Refusing to rewrite Kronn state from default: {e}");
        })
}

/// Atomic write of `docs/.kronn.json` (temp file + rename), so a concurrent
/// reader never sees a torn file. Always rewrites the `_readme` field on the
/// in-memory state before serializing.
pub fn write(project_path: &Path, state: &mut KronnState) -> Result<(), String> {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    std::fs::create_dir_all(&docs_dir)
        .map_err(|e| format!("Failed to create {} dir: {e}", docs_dir.display()))?;

    state.touch_readme();
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("JSON serialize error: {e}"))?;

    let path = docs_dir.join(KRONN_STATE_FILENAME);
    crate::core::mcp_scanner::atomic_write(&path, &json)
}

fn today_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn kronn_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Append an audit entry. Creates the file if missing. Idempotent in the
/// sense that calling twice on the same day adds two entries — the audit
/// engine itself decides whether to call this (e.g. once per successful
/// full/partial run).
pub fn record_audit(project_path: &Path, audit_type: &str) -> Result<(), String> {
    let mut state = load_for_mutation(project_path)?;
    state.audits.push(AuditEntry {
        date: today_iso(),
        kronn_version: kronn_version(),
        audit_type: audit_type.to_string(),
        provenance: AuditProvenance::KronnAudit,
    });
    write(project_path, &mut state)
}

/// Record an explicit human attestation without pretending Kronn ran an audit.
/// Repeated clicks are idempotent for the current day.
pub fn attest_documentation(project_path: &Path) -> Result<(), String> {
    let mut state = load_for_mutation(project_path)?;
    let date = today_iso();
    if !state
        .audits
        .iter()
        .any(|entry| entry.provenance == AuditProvenance::HumanAttestation && entry.date == date)
    {
        state.audits.push(AuditEntry {
            date,
            kronn_version: kronn_version(),
            audit_type: "attested".to_string(),
            provenance: AuditProvenance::HumanAttestation,
        });
    }
    write(project_path, &mut state)
}

/// Set `validated_at`. No-op if already set (preserves the original date).
pub fn mark_validated(project_path: &Path) -> Result<(), String> {
    let mut state = load_for_mutation(project_path)?;
    if state.validated_at.is_none() {
        state.validated_at = Some(today_iso());
    }
    write(project_path, &mut state)
}

/// Clear `validated_at` (Codex A5) — called at the start of EVERY audit
/// mutation (full, specialized, partial): the Validated badge asserts the
/// docs match a validated state, which stops being true the moment a new
/// run mutates them. Contractual for callers: a failure must refuse the
/// run, never warn-and-continue. No-op when nothing was validated (a
/// missing .kronn.json is fine — nothing to revoke).
pub fn revoke_validated(project_path: &Path) -> Result<(), String> {
    // Legacy projects carry `KRONN:VALIDATED` markers without a
    // .kronn.json: backfill FIRST (contractually — an error here must
    // refuse the run) so the revocation below clears real state instead
    // of no-oping on a default while the scanner keeps reading the old
    // marker as Validated.
    backfill_from_legacy_state(project_path)?;
    // `load_for_mutation` distinguishes missing (Ok(default) — nothing to
    // revoke) from corrupt/unreadable (Err — refuse the run).
    let mut state = load_for_mutation(project_path)?;
    if state.validated_at.is_none() {
        return Ok(());
    }
    state.validated_at = None;
    write(project_path, &mut state)
}

/// Set `bootstrapped_at`. No-op if already set.
pub fn mark_bootstrapped(project_path: &Path) -> Result<(), String> {
    let mut state = load_for_mutation(project_path)?;
    if state.bootstrapped_at.is_none() {
        state.bootstrapped_at = Some(today_iso());
    }
    write(project_path, &mut state)
}

/// 0.8.6 (#28) — Backfill `.kronn.json` from legacy state markers.
///
/// **Why:** projects audited in 0.7.x → 0.8.3 don't have `.kronn.json` even
/// when they were validated multiple times. Without backfill they appear
/// as `TemplateInstalled` to the audit-status badge — confusing for users
/// (front_euronews case 2026-05-17 : audited many times yet showed as
/// never-touched). Forcing a full re-audit to "fix" the badge is wasteful
/// (~30k tokens, rewrites `docs/AGENTS.md`). This function does the
/// migration cheaply.
///
/// **What it inspects** (cf. `scanner::analyze_audit_state` legacy
/// fallbacks for the exact same set) :
///   - `docs/checksums.json` present → seed one `AuditEntry` with type
///     `"legacy"` + date `today` (we don't try to recover the original
///     audit date from the file mtime — too fragile across `git clone`).
///   - `KRONN:VALIDATED` HTML marker in `docs/AGENTS.md` → set
///     `validated_at = today` (markers don't carry their own date).
///   - `KRONN:BOOTSTRAPPED` marker → set `bootstrapped_at = today`.
///
/// **No-ops** : if `.kronn.json` already exists, OR no legacy signal
/// present. Returns `Ok(true)` when a backfill happened, `Ok(false)` when
/// skipped. Write errors propagate as `Err(String)` — caller decides
/// whether to log + fall through to legacy detection (read-only FS, etc.).
pub fn backfill_from_legacy_state(project_path: &Path) -> Result<bool, String> {
    // Skip if already present — backfill is one-shot. A corrupt/unreadable
    // file also skips: overwriting it would destroy the real audit history.
    match read_for_mutation(project_path) {
        Ok(Some(_)) => return Ok(false),
        Ok(None) => {}
        Err(e) => {
            tracing::warn!("Skipping legacy backfill: {e}");
            return Ok(false);
        }
    }

    let Some(mut state) = legacy_state(project_path) else {
        return Ok(false);
    };
    write(project_path, &mut state)?;
    tracing::info!(
        project = ?project_path,
        "Kronn state backfilled from legacy markers",
    );
    Ok(true)
}

/// The state the legacy evidence implies, computed in memory: `docs/checksums.json`
/// seeds one `legacy` audit entry, the `KRONN:VALIDATED` / `KRONN:BOOTSTRAPPED`
/// markers set their dates (today: markers carry none). `None` without any signal.
/// Pure: read paths use it so a GET never writes into the checkout.
pub fn legacy_state(project_path: &Path) -> Option<KronnState> {
    let has_checksums = crate::core::checksums::read_checksums_file(project_path).is_some();
    let docs_entry = crate::core::scanner::detect_docs_entry(project_path);
    let agents_content = std::fs::read_to_string(&docs_entry).unwrap_or_default();
    let has_validated = agents_content.contains("KRONN:VALIDATED");
    let has_bootstrapped = agents_content.contains("KRONN:BOOTSTRAPPED");
    if !has_checksums && !has_validated && !has_bootstrapped {
        return None;
    }
    let now = today_iso();
    let mut state = KronnState::default();
    // At least one entry, so the project reads as audited (or better).
    state.audits.push(AuditEntry {
        date: now.clone(),
        kronn_version: "legacy".to_string(),
        audit_type: "legacy".to_string(),
        provenance: AuditProvenance::LegacyEvidence,
    });
    if has_validated {
        state.validated_at = Some(now.clone());
    }
    if has_bootstrapped {
        state.bootstrapped_at = Some(now);
    }
    Some(state)
}

/// A state file absent from the checked-out branch but present in git history,
/// e.g. committed on a feature branch only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct StateInHistory {
    /// Full sha of the newest commit that added or changed the file.
    pub commit: String,
    /// Committer date of that commit, ISO 8601.
    pub committed_at: String,
    /// Up to five branches that contain the commit (empty if none does).
    pub branches: Vec<String>,
    /// Path relative to the project root.
    pub path: String,
}

const MAX_HISTORY_BRANCHES: usize = 5;

fn state_rel_path(project_path: &Path) -> String {
    let path = state_path(project_path);
    path.strip_prefix(project_path)
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn git_stdout(project_path: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = crate::core::cmd::git_cmd()
        .arg("-C")
        .arg(project_path)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Look for the state file in the repository history when the working tree
/// has none. Read-only; `None` outside git, when the file is present, or when
/// no commit ever carried it.
pub fn find_in_git_history(project_path: &Path) -> Option<StateInHistory> {
    if state_path(project_path).exists() {
        return None;
    }
    let rel = state_rel_path(project_path);
    let pathspec = format!("./{rel}");
    let log = git_stdout(
        project_path,
        &[
            "log",
            "--all",
            "-1",
            "--diff-filter=AMR",
            "--format=%H%x1f%cI",
            "--",
            &pathspec,
        ],
    )?;
    let log = String::from_utf8_lossy(&log);
    let (commit, committed_at) = log.trim().split_once('\u{1f}')?;
    if commit.is_empty() {
        return None;
    }
    let branches = git_stdout(
        project_path,
        &[
            "branch",
            "-a",
            "--contains",
            commit,
            "--format=%(refname:short)",
        ],
    )
    .map(|out| {
        String::from_utf8_lossy(&out)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(MAX_HISTORY_BRANCHES)
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default();
    Some(StateInHistory {
        commit: commit.to_string(),
        committed_at: committed_at.to_string(),
        branches,
        path: rel,
    })
}

/// Restore the state file from `commit` into the working tree, byte for byte.
/// Refuses when a state file already exists, when the commit id is not a hex
/// sha, or when the stored content is not a valid state file.
pub fn restore_from_git_history(project_path: &Path, commit: &str) -> Result<(), String> {
    let commit = commit.trim();
    if !(7..=64).contains(&commit.len()) || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("invalid commit id".to_string());
    }
    let target = state_path(project_path);
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    let rel = state_rel_path(project_path);
    let object = format!("{commit}:./{rel}");
    let content = git_stdout(project_path, &["show", &object])
        .ok_or_else(|| format!("{rel} not found in commit {commit}"))?;
    serde_json::from_slice::<KronnState>(&content)
        .map_err(|e| format!("{rel} in commit {commit} is not a valid state file: {e}"))?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    crate::core::mcp_scanner::atomic_write_bytes(&target, &content)
}

#[cfg(test)]
#[path = "kronn_state_test.rs"]
mod tests;
