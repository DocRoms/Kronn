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

/// Inline marker on every `.kronn.json` so a human opening the file
/// understands its purpose without consulting external docs.
pub const KRONN_STATE_README: &str = "Managed by Kronn (https://github.com/DocRoms/Kronn). \
Tracks audit/validation state across machines. Do not delete or gitignore — required for \
accurate audit status when this repo is cloned to another Kronn instance.";

/// File name (always under `docs/` — resolved via `detect_docs_dir`).
pub const KRONN_STATE_FILENAME: &str = ".kronn.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEntry {
    /// ISO date `YYYY-MM-DD` — when the audit completed.
    pub date: String,
    /// Kronn version that wrote this entry (`CARGO_PKG_VERSION`).
    pub kronn_version: String,
    /// Free-form discriminator: `"full"`, `"partial"`, `"legacy"`, ...
    #[serde(rename = "type")]
    pub audit_type: String,
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

fn state_path(project_path: &Path) -> std::path::PathBuf {
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

/// Load state for a mutation: default for a missing file, `Err` (with a warn)
/// for an unreadable/corrupt one so the caller aborts instead of clobbering.
fn load_for_mutation(project_path: &Path) -> Result<KronnState, String> {
    read_for_mutation(project_path)
        .map(Option::unwrap_or_default)
        .inspect_err(|e| {
            tracing::warn!("Refusing to rewrite Kronn state from default: {e}");
        })
}

/// Atomic-ish write of `docs/.kronn.json`. Always rewrites the `_readme`
/// field on the in-memory state before serializing.
pub fn write(project_path: &Path, state: &mut KronnState) -> Result<(), String> {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    std::fs::create_dir_all(&docs_dir)
        .map_err(|e| format!("Failed to create {} dir: {e}", docs_dir.display()))?;

    state.touch_readme();
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| format!("JSON serialize error: {e}"))?;

    let path = docs_dir.join(KRONN_STATE_FILENAME);
    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    Ok(())
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
    });
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

    let has_checksums =
        crate::core::checksums::read_checksums_file(project_path).is_some();

    // Read AGENTS.md (or whatever the project's docs entry is) once to
    // probe for the two legacy HTML markers. Tolerant : missing file
    // → no markers detected.
    let docs_entry = crate::core::scanner::detect_docs_entry(project_path);
    let agents_content = std::fs::read_to_string(&docs_entry).unwrap_or_default();
    let has_validated = agents_content.contains("KRONN:VALIDATED");
    let has_bootstrapped = agents_content.contains("KRONN:BOOTSTRAPPED");

    // No legacy signal at all → nothing to backfill from. Return false so
    // the caller can fall through to default state.
    if !has_checksums && !has_validated && !has_bootstrapped {
        return Ok(false);
    }

    let now = today_iso();
    let mut state = KronnState::default();

    // Always seed at least one audit entry so `has_any_audit()` is true
    // and the project surfaces as `Audited` (or `Validated` /
    // `Bootstrapped`) rather than `TemplateInstalled` on next scan.
    state.audits.push(AuditEntry {
        date: now.clone(),
        kronn_version: "legacy".to_string(),
        audit_type: "legacy".to_string(),
    });

    if has_validated {
        state.validated_at = Some(now.clone());
    }
    if has_bootstrapped {
        state.bootstrapped_at = Some(now);
    }

    write(project_path, &mut state)?;
    tracing::info!(
        project = ?project_path,
        checksums = has_checksums,
        validated_marker = has_validated,
        bootstrapped_marker = has_bootstrapped,
        "Kronn state backfilled from legacy markers",
    );
    Ok(true)
}

#[cfg(test)]
#[path = "kronn_state_test.rs"]
mod tests;
