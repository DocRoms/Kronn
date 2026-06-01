//! Reconciliation pass — runs after Step 9 of a Full audit.
//!
//! Goal: close the loop on TDs that existed BEFORE this audit but were
//! NOT re-emitted. Without this pass, a user who wipes their tech-debt
//! folder gets a clean re-audit but a user who runs an incremental
//! re-audit silently loses information about what's now considered
//! fixed vs missed.
//!
//! Strategy (0.8.2 — mechanical, no LLM call):
//!   1. Snapshot the existing TD detail files (path + content hash)
//!      just BEFORE Step 9 runs.
//!   2. After Step 9 finishes, compute the delta:
//!        - `unchanged`        : file present, identical content (agent didn't touch — candidate)
//!        - `updated`          : file present, content differs (agent refreshed — re-emitted ✓)
//!        - `deleted`          : file gone (agent explicitly removed — likely fixed)
//!        - `new`              : file present but wasn't in snapshot (new finding)
//!   3. Classify each `unchanged` and `deleted` candidate:
//!        - **Fixed**           : file deleted by the audit, OR original problem signature no longer found in source.
//!        - **Stale**           : file untouched, last modified > 90 days ago, no signal either way.
//!        - **Missed**          : file untouched, < 90 days old, original signature still present in source — likely a re-discovery oversight.
//!        - **Uncertain**       : default when we can't tell (e.g. no `Where (pointers)` block in the detail file).
//!   4. Write `docs/tech-debt/_reconciliation-<date>.md` summarizing
//!      the classification. The report is intentionally human-friendly
//!      and the file name starts with `_` so the existing
//!      `count_tech_debt` scanner skips it.
//!
//! A follow-up pass in 0.8.3 will add git-log analysis ("was there a
//! commit touching the cited files since the previous audit?") which
//! is heavier and not needed for the first cut.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One entry in the snapshot taken before Step 9.
#[derive(Debug, Clone)]
pub struct TdSnapshot {
    /// Absolute path on disk at snapshot time.
    pub path: PathBuf,
    /// `TD-<date>-<slug>` (file stem, no extension).
    pub id: String,
    /// SHA-256 of the file content at snapshot time. Used to detect
    /// updates without parsing the markdown.
    pub content_hash: String,
    /// File mtime at snapshot time. Doubles as "age" for the Stale
    /// classifier (when reused as "last modified > 90 days").
    pub mtime: DateTime<Utc>,
}

/// Outcome of the post-audit comparison for a single pre-existing TD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaKind {
    /// File present, content identical to snapshot, AND the TD id is no
    /// longer referenced in the freshly-written index
    /// (`inconsistencies-tech-debt.md`). The agent neither refreshed the
    /// detail file nor re-listed the finding — a genuine reconciliation
    /// candidate (Fixed / Stale / Missed / Uncertain).
    Unchanged,
    /// File present, content differs — agent refreshed in place.
    /// Healthy outcome of the priors rule (§ C of Step 9 prompt).
    Updated,
    /// File present, content identical to snapshot, BUT the TD id is
    /// still referenced in the freshly-written index. The agent kept the
    /// prior verbatim and re-listed it — a healthy carried-over
    /// re-emission, NOT a missed finding. (Fix 2026-06-03: previously
    /// these collapsed to `Unchanged` → classified `Missed`, producing
    /// "Missed: N" reports for priors that were actually re-emitted.)
    Carried,
    /// File gone — agent deleted it (or it was moved). Most likely
    /// "fixed since last audit".
    Deleted,
}

/// Classification for a candidate (Unchanged / Deleted) TD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    Fixed,
    Stale,
    Missed,
    Uncertain,
}

impl Classification {
    pub fn label(self) -> &'static str {
        match self {
            Classification::Fixed => "Fixed",
            Classification::Stale => "Stale",
            Classification::Missed => "Missed",
            Classification::Uncertain => "Uncertain",
        }
    }

    pub fn emoji(self) -> &'static str {
        match self {
            Classification::Fixed => "✅",
            Classification::Stale => "🕰️",
            Classification::Missed => "⚠️",
            Classification::Uncertain => "❓",
        }
    }
}

/// One row of the reconciliation report.
#[derive(Debug, Clone)]
pub struct ReconciliationEntry {
    pub id: String,
    pub delta: DeltaKind,
    pub classification: Classification,
    /// Human-readable reasoning shown in the report.
    pub reason: String,
}

/// Build the pre-Step-9 snapshot of the tech-debt directory.
///
/// Reads every `.md` file at the top level of `<docs>/tech-debt/`,
/// skipping scaffolding (`README.md`, `TEMPLATE.md`, `_template.md`,
/// `_reconciliation-*.md`, and any file starting with `_`).
/// Returns an empty Vec on a fresh project (no tech-debt folder yet).
pub fn snapshot_tech_debt_dir(docs_dir: &Path) -> Vec<TdSnapshot> {
    let td_dir = docs_dir.join("tech-debt");
    if !td_dir.is_dir() {
        return Vec::new();
    }
    let entries = match std::fs::read_dir(&td_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.ends_with(".md") || is_scaffolding(name) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        let id = name.trim_end_matches(".md").to_string();
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(Utc::now);
        out.push(TdSnapshot {
            path: path.clone(),
            id,
            content_hash: sha256_hex(&content),
            mtime,
        });
    }
    out
}

/// chantier 4 (2026-06-04) — a human-readable digest of ONE pre-existing
/// TD, used to build the "known debt" dedup-list injected into Step 8 on a
/// re-audit (Option C: full re-scan + dedup). Distinct from [`TdSnapshot`],
/// which is hash-only — here the agent needs `id + severity + title` to
/// decide "is this finding already on the list?", not a content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorDigest {
    pub id: String,
    /// Capitalised severity ("Critical"/"High"/"Medium"/"Low") or "" if
    /// the detail file had no parseable `**Severity**:` line.
    pub severity: String,
    /// First `# ` heading, with any leading `TD-...:` id prefix stripped.
    /// Falls back to the id when the file has no heading.
    pub title: String,
}

/// Read every TD detail file in `<docs>/tech-debt/` into a [`PriorDigest`].
/// Mirrors [`snapshot_tech_debt_dir`]'s dir resolution + scaffolding skip,
/// but parses id/severity/title instead of hashing. Empty Vec on a fresh
/// project (no folder yet) — the caller treats empty as "first audit, no
/// dedup list to inject".
pub fn digest_prior_tech_debt(docs_dir: &Path) -> Vec<PriorDigest> {
    let td_dir = docs_dir.join("tech-debt");
    if !td_dir.is_dir() {
        return Vec::new();
    }
    let entries = match std::fs::read_dir(&td_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        // Only `TD-*.md` detail files (Codex review 2026-06-04): a future
        // hand-written note dropped in tech-debt/ shouldn't be injected as
        // "known debt". `is_scaffolding` already covers README/TEMPLATE/_*.
        if !name.starts_with("TD-") || !name.ends_with(".md") || is_scaffolding(name) {
            continue;
        }
        let id = name.trim_end_matches(".md").to_string();
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        out.push(digest_one(&id, &content));
    }
    // Stable order: highest severity first, then id — so the injected list
    // reads Critical→Low and is deterministic across runs (tests + diffs).
    out.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Parse one TD detail file's content into a digest. Separate fn so it's
/// unit-testable without touching the filesystem.
fn digest_one(id: &str, content: &str) -> PriorDigest {
    let severity = content
        .lines()
        .find(|l| {
            let lc = l.to_ascii_lowercase();
            lc.contains("**severity**:") || lc.trim_start().starts_with("severity:")
        })
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().trim_start_matches('*').trim().to_ascii_lowercase())
        .map(|s| capitalize_severity(&s))
        .unwrap_or_default();

    let title = content
        .lines()
        .map(|l| l.trim())
        .find(|l| l.starts_with("# "))
        .map(|l| strip_id_prefix(l.trim_start_matches('#').trim(), id))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| id.to_string());

    PriorDigest { id: id.to_string(), severity, title }
}

/// `"# TD-20260603-foo: Real title"` → `"Real title"`. Leaves a heading
/// with no id prefix untouched.
fn strip_id_prefix(heading: &str, id: &str) -> String {
    let h = heading.trim();
    // Strip a leading "<id>:" or "<id> —" / "<id> -" prefix.
    if let Some(rest) = h.strip_prefix(id) {
        return rest
            .trim_start()
            .trim_start_matches([':', '—', '-'])
            .trim()
            .to_string();
    }
    h.to_string()
}

fn capitalize_severity(s: &str) -> String {
    match s {
        x if x.starts_with("critical") => "Critical".to_string(),
        x if x.starts_with("high") => "High".to_string(),
        x if x.starts_with("medium") => "Medium".to_string(),
        x if x.starts_with("low") => "Low".to_string(),
        _ => String::new(),
    }
}

fn severity_rank(s: &str) -> u8 {
    match s {
        "Critical" => 0,
        "High" => 1,
        "Medium" => 2,
        "Low" => 3,
        _ => 4,
    }
}

/// Render the RE-AUDIT MODE block injected into Step 8 when priors exist
/// (chantier 4 / Option C). Flips the implicit in-place behaviour — where
/// the agent SEES the existing entries in the target file and recopies
/// them (Carried + anti-repetition) — into an explicit "fresh full pass,
/// these are just a dedup list" contract. Returns "" when there are no
/// priors (caller guards on this, but defensive).
pub fn render_known_debt_block(priors: &[PriorDigest]) -> String {
    if priors.is_empty() {
        return String::new();
    }
    let mut block = String::new();
    block.push_str("## RE-AUDIT MODE — fresh full pass, dedup against known debt\n\n");
    block.push_str(&format!(
        "This project was audited before — `docs/tech-debt/` already holds {} entr{} (listed below). \
**This list is NOT a reason to skip scanning.** Re-run the COMPLETE dimensional pass (all dimensions) \
from the SOURCE CODE, exactly as if auditing from scratch.\n\n",
        priors.len(),
        if priors.len() == 1 { "y" } else { "ies" }
    ));
    block.push_str(
        "How to handle the entries below:\n\
- **KEEP listing every still-valid known TD id in `inconsistencies-tech-debt.md` (the index)** so it is carried over. Do NOT silently drop a prior from the index — that would mark a still-real debt as fixed/missed when it isn't.\n\
- Do NOT create a duplicate `TD-*` detail file for something already on the list, and do NOT rewrite a prior's detail body unless its evidence actually changed in the code.\n\
- For genuinely NEW debt that is **NOT** on the list → create a NEW `TD-*` entry.\n\
- If a prior is genuinely resolved/fixed in the current code, you MAY drop it — but only as an explicit, justified reconciliation outcome, never as a side effect of \"not re-emitting\".\n\n\
The whole point of a re-audit is to find what the LAST pass MISSED. \
Adding **zero** new entries is acceptable ONLY if the dimension-coverage matrix documents a fresh full scan of every dimension AND no detector signal is left undisposed. \
Otherwise this step is INCOMPLETE — it means you re-read the existing files instead of re-scanning the code.\n\n",
    );
    block.push_str("### Known debt (id — severity — title) — carry these in the index, don't duplicate\n");
    for p in priors {
        let sev = if p.severity.is_empty() { "?" } else { &p.severity };
        block.push_str(&format!("- `{}` — {} — {}\n", p.id, sev, p.title));
    }
    block
}

/// Extract the set of `TD-<date>-<slug>` ids still referenced in the
/// freshly-written index (`inconsistencies-tech-debt.md`). A finding is
/// "still alive" if its id appears anywhere in the index — the
/// `## Current list` table, the dimension-coverage evidence cells, the
/// baseline checklist — so a prior re-listed in ANY of those is treated
/// as re-emitted, not missed.
///
/// Pattern: `TD-` followed by 8 digits, `-`, then a lowercase-kebab
/// slug. Matches the file-stem convention used everywhere else.
pub fn parse_index_td_ids(index_content: &str) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::new();
    let bytes = index_content.as_bytes();
    let mut i = 0;
    while let Some(rel) = index_content[i..].find("TD-") {
        let start = i + rel;
        // Scan the id token: TD- then [A-Za-z0-9-] run. We validate the
        // `<8 digits>-<slug>` shape afterwards.
        let mut end = start + 3;
        while end < bytes.len() {
            let c = bytes[end];
            if c.is_ascii_alphanumeric() || c == b'-' {
                end += 1;
            } else {
                break;
            }
        }
        let token = &index_content[start..end];
        // Trim a trailing `-` (e.g. matched at a markdown boundary).
        let token = token.trim_end_matches('-');
        if is_valid_td_id(token) {
            ids.insert(token.to_string());
        }
        i = end.max(start + 3);
    }
    ids
}

/// `TD-YYYYMMDD-slug` shape check: `TD-`, exactly 8 digits, `-`, then a
/// non-empty slug. Keeps `parse_index_td_ids` from grabbing prose like
/// `TD-list` or `TD-` headers.
fn is_valid_td_id(token: &str) -> bool {
    let Some(rest) = token.strip_prefix("TD-") else { return false };
    let mut parts = rest.splitn(2, '-');
    let date = parts.next().unwrap_or("");
    let slug = parts.next().unwrap_or("");
    date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) && !slug.is_empty()
}

/// Compute the delta between the pre-audit snapshot and the post-audit
/// state. Returns one entry per snapshot TD (so we know what to report
/// on); brand-new TDs created by this audit are NOT included — they're
/// the audit's normal output, not reconciliation candidates.
///
/// Back-compat shim (empty index set): an unchanged file with no index
/// awareness is reported as `Unchanged`. Prefer
/// [`compute_delta_with_index`] in the audit pipeline so a re-listed
/// prior is correctly `Carried`, not a `Missed` candidate.
pub fn compute_delta(snapshot: &[TdSnapshot]) -> Vec<(TdSnapshot, DeltaKind)> {
    compute_delta_with_index(snapshot, &std::collections::HashSet::new())
}

/// Index-aware delta. `still_listed` is the set of TD ids still
/// referenced in the freshly-written index (see [`parse_index_td_ids`]).
/// A byte-identical detail file whose id is still listed is `Carried`
/// (healthy re-emission); only a byte-identical file whose id has
/// DISAPPEARED from the index is an `Unchanged` reconciliation candidate.
pub fn compute_delta_with_index(
    snapshot: &[TdSnapshot],
    still_listed: &std::collections::HashSet<String>,
) -> Vec<(TdSnapshot, DeltaKind)> {
    snapshot
        .iter()
        .map(|snap| {
            let delta = match std::fs::read_to_string(&snap.path) {
                Err(_) => DeltaKind::Deleted,
                Ok(content) => {
                    if sha256_hex(&content) == snap.content_hash {
                        if still_listed.contains(&snap.id) {
                            DeltaKind::Carried
                        } else {
                            DeltaKind::Unchanged
                        }
                    } else {
                        DeltaKind::Updated
                    }
                }
            };
            (snap.clone(), delta)
        })
        .collect()
}

/// Classify candidates (`Unchanged` or `Deleted`) using the heuristics
/// documented at the top of this module. `Updated` entries are not
/// classified — they're the healthy "priors reused" outcome.
///
/// `source_check` is a closure that, given a TD content, attempts to
/// confirm whether the original problem signature is still present in
/// the project source. Passed in so the caller can plug a real
/// implementation (greps the `Where (pointers)` lines) and tests can
/// stub it. Returning `None` = inconclusive; `Some(true)` = signature
/// still found; `Some(false)` = signature gone.
pub fn classify<F>(
    deltas: &[(TdSnapshot, DeltaKind)],
    now: DateTime<Utc>,
    stale_after_days: i64,
    mut source_check: F,
) -> Vec<ReconciliationEntry>
where
    F: FnMut(&TdSnapshot) -> Option<bool>,
{
    let mut out = Vec::with_capacity(deltas.len());
    for (snap, delta) in deltas {
        match delta {
            DeltaKind::Updated => {
                // Healthy outcome — agent re-emitted this TD with refreshed content.
                // We still emit a row in the report so the user sees the explicit
                // "priors reused" signal, but classification is "Updated" (separate
                // from the 4 candidate classifications).
                out.push(ReconciliationEntry {
                    id: snap.id.clone(),
                    delta: delta.clone(),
                    classification: Classification::Uncertain, // unused for Updated
                    reason: "Refreshed by this audit (priors rule).".into(),
                });
            }
            DeltaKind::Carried => {
                // Healthy outcome — detail file kept verbatim but the id is
                // still listed in the new index, so the finding was re-emitted
                // (carried over), not missed. No source_check: presence in the
                // index is the re-emission signal.
                out.push(ReconciliationEntry {
                    id: snap.id.clone(),
                    delta: delta.clone(),
                    classification: Classification::Uncertain, // unused for Carried
                    reason: "Carried over — still listed in the index, detail unchanged.".into(),
                });
            }
            DeltaKind::Deleted => {
                out.push(ReconciliationEntry {
                    id: snap.id.clone(),
                    delta: delta.clone(),
                    classification: Classification::Fixed,
                    reason: "Detail file removed by the audit — most likely resolved.".into(),
                });
            }
            DeltaKind::Unchanged => {
                let age_days = (now - snap.mtime).num_days();
                let signal = source_check(snap);
                let (class, reason) = match (signal, age_days >= stale_after_days) {
                    (Some(true), _) => (
                        Classification::Missed,
                        format!(
                            "Original problem signature still found in source — likely re-discovery oversight (age {} days).",
                            age_days
                        ),
                    ),
                    (Some(false), _) => (
                        Classification::Fixed,
                        format!(
                            "Original problem signature no longer found in source — likely resolved (age {} days).",
                            age_days
                        ),
                    ),
                    (None, true) => (
                        Classification::Stale,
                        format!(
                            "No signal, last touched {} days ago — older than the staleness threshold ({} d).",
                            age_days, stale_after_days
                        ),
                    ),
                    (None, false) => (
                        Classification::Uncertain,
                        format!(
                            "No source signature available to verify (age {} days). Confirm manually.",
                            age_days
                        ),
                    ),
                };
                out.push(ReconciliationEntry {
                    id: snap.id.clone(),
                    delta: delta.clone(),
                    classification: class,
                    reason,
                });
            }
        }
    }
    out
}

/// Best-effort source-signature check: read the TD detail file, find
/// every `path/to/file:line` pattern under a `## Where (pointers)`
/// section, and grep the source for the surrounding line context.
///
/// Returns `Some(true)` if at least one pointer's source file still
/// contains a matching line; `Some(false)` if all pointers are gone;
/// `None` if the TD has no parsable pointers (we can't conclude).
///
/// Conservative on purpose — false positives ("signature still present")
/// are safer than false negatives because they make the agent re-audit
/// next time instead of silently dropping a real issue.
pub fn check_signature_in_source(snap: &TdSnapshot, project_path: &Path) -> Option<bool> {
    let content = std::fs::read_to_string(&snap.path).ok()?;
    let pointers = parse_pointers(&content);
    if pointers.is_empty() {
        return None;
    }
    let mut any_found = false;
    let mut any_missing = false;
    for ptr in &pointers {
        let file_path = project_path.join(&ptr.file);
        if !file_path.exists() {
            any_missing = true;
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&file_path) else {
            any_missing = true;
            continue;
        };
        // Loose match: if the original line snippet is present anywhere
        // in the file (line numbers shift across commits, snippet text
        // usually doesn't).
        if let Some(snippet) = &ptr.snippet {
            if source.contains(snippet.as_str()) {
                any_found = true;
                continue;
            }
        }
        // Fallback to "line still exists at the cited number" if no snippet.
        if let Some(line_no) = ptr.line {
            let lines: Vec<&str> = source.lines().collect();
            if line_no > 0 && line_no <= lines.len() {
                any_found = true; // line exists; we can't tell if it's the same content
                continue;
            }
        }
        any_missing = true;
    }
    match (any_found, any_missing) {
        (true, _) => Some(true),
        (false, true) => Some(false),
        (false, false) => None,
    }
}

/// Render the reconciliation report markdown. Caller writes it to
/// `docs/tech-debt/_reconciliation-<date>.md` (filename leading
/// underscore keeps it out of `count_tech_debt`'s set).
pub fn render_report(
    entries: &[ReconciliationEntry],
    audit_date: &str,
    audit_kind: &str,
) -> String {
    let mut buckets: HashMap<&'static str, Vec<&ReconciliationEntry>> = HashMap::new();
    let mut reused = Vec::new();
    for e in entries {
        // `Updated` (refreshed) and `Carried` (re-listed verbatim) are both
        // healthy re-emissions — group them as "priors reused", never as
        // reconciliation candidates.
        if e.delta == DeltaKind::Updated || e.delta == DeltaKind::Carried {
            reused.push(e);
        } else {
            buckets.entry(e.classification.label()).or_default().push(e);
        }
    }

    let mut out = String::new();
    out.push_str(&format!("# Reconciliation report — {audit_date} {audit_kind}\n\n"));
    out.push_str(
        "Generated by Kronn's reconciliation pass. For each TD that existed BEFORE this \
audit but was NOT re-emitted by Step 9, this report classifies its likely state \
(Fixed / Stale / Missed / Uncertain) so nothing slips silently between audits.\n\n",
    );

    // Summary table at the top so the user sees the headlines first.
    let total_candidates: usize = buckets.values().map(|v| v.len()).sum();
    out.push_str("## Summary\n\n");
    out.push_str(&format!("- **{}** re-emitted (priors reused — healthy: refreshed or carried over)\n", reused.len()));
    out.push_str(&format!("- **{total_candidates}** reconciliation candidates :\n"));
    for label in &["Fixed", "Stale", "Missed", "Uncertain"] {
        let count = buckets.get(*label).map(|v| v.len()).unwrap_or(0);
        if count > 0 {
            out.push_str(&format!("  - {label}: {count}\n"));
        }
    }
    out.push('\n');

    if !reused.is_empty() {
        out.push_str("## ✓ Re-emitted (priors reused)\n\n");
        for e in &reused {
            out.push_str(&format!("- `{}` — {}\n", e.id, e.reason));
        }
        out.push('\n');
    }

    for label in &["Fixed", "Stale", "Missed", "Uncertain"] {
        let Some(list) = buckets.get(*label) else { continue };
        if list.is_empty() { continue }
        let emoji = list[0].classification.emoji();
        out.push_str(&format!("## {emoji} {label}\n\n"));
        for e in list {
            let delta_tag = match e.delta {
                DeltaKind::Deleted => " (file removed)",
                DeltaKind::Unchanged => " (file untouched)",
                _ => "",
            };
            out.push_str(&format!("- `{}`{} — {}\n", e.id, delta_tag, e.reason));
        }
        out.push('\n');
    }

    out.push_str(
        "---\n\n\
*Heuristics — Fixed = signature gone OR detail file removed by audit; \
Stale = no signal AND last touched > 90 d ago; \
Missed = original signature still present in source (likely re-discovery oversight); \
Uncertain = no parsable signature, manual review recommended.*\n",
    );
    out
}

// ─── Internals ───────────────────────────────────────────────────────────

fn is_scaffolding(name: &str) -> bool {
    name == "README.md"
        || name == "TEMPLATE.md"
        || name == "_template.md"
        || name.starts_with('_')
}

fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let result = h.finalize();
    let mut hex = String::with_capacity(64);
    for b in result.iter() {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Cheap markdown pointer parser. We don't need a full markdown AST —
/// we just need to find lines of the shape
///     - `path/to/file.rs:42` — some description
///     - `path/to/file.rs:7-15` ...
///     - `path/to/file.rs` (when no line specified)
/// under a heading starting with `## Where`.
#[derive(Debug, Clone)]
pub struct Pointer {
    pub file: String,
    pub line: Option<usize>,
    /// If a hint of the offending snippet is captured in the bullet,
    /// store it so we can fuzzy-match against the file content even
    /// when line numbers shift. Otherwise None.
    pub snippet: Option<String>,
}

pub fn parse_pointers(content: &str) -> Vec<Pointer> {
    let mut out = Vec::new();
    let mut in_where = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("## ") {
            in_where = trimmed.to_lowercase().contains("where");
            continue;
        }
        if !in_where {
            continue;
        }
        // Line shape: "- `path/to/file:42` — context" or variants.
        let Some(rest) = trimmed.strip_prefix("- ") else { continue };
        let Some(backtick_start) = rest.find('`') else { continue };
        let Some(backtick_end) = rest[backtick_start + 1..].find('`') else { continue };
        let pointer_text = &rest[backtick_start + 1..backtick_start + 1 + backtick_end];
        let (file, line_no) = split_file_line(pointer_text);
        if file.is_empty() {
            continue;
        }
        // Grab whatever follows the closing backtick as a possible snippet.
        let after = &rest[backtick_start + 1 + backtick_end + 1..];
        let snippet = after
            .split_once('`')
            .and_then(|(_, inside)| inside.split_once('`').map(|(s, _)| s.trim().to_string()))
            .filter(|s| !s.is_empty() && s.len() >= 8 && s.len() <= 200);
        out.push(Pointer { file, line: line_no, snippet });
    }
    out
}

fn split_file_line(s: &str) -> (String, Option<usize>) {
    // Strip surrounding whitespace.
    let s = s.trim();
    // Path can contain colons on Windows (`C:\…`); ignore that case for now
    // — backend project paths are POSIX in our flows.
    let Some((file, line_part)) = s.rsplit_once(':') else {
        return (s.to_string(), None);
    };
    // Handle ranges like `7-15` — take the start.
    let line_no = line_part
        .split('-')
        .next()
        .and_then(|n| n.trim().parse::<usize>().ok());
    if line_no.is_some() {
        (file.to_string(), line_no)
    } else {
        // The `:` was part of the path, not a line marker.
        (s.to_string(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

    fn mk_snap(id: &str, content_hash: &str, age_days: i64) -> TdSnapshot {
        TdSnapshot {
            path: PathBuf::from(format!("/tmp/{id}.md")),
            id: id.to_string(),
            content_hash: content_hash.to_string(),
            mtime: Utc::now() - ChronoDuration::days(age_days),
        }
    }

    #[test]
    fn classify_deleted_file_is_fixed() {
        let snap = mk_snap("TD-A", "abc", 30);
        let deltas = vec![(snap, DeltaKind::Deleted)];
        let entries = classify(&deltas, Utc::now(), 90, |_| None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].classification, Classification::Fixed);
        assert_eq!(entries[0].delta, DeltaKind::Deleted);
    }

    #[test]
    fn classify_unchanged_with_signature_still_present_is_missed() {
        let snap = mk_snap("TD-B", "abc", 10);
        let deltas = vec![(snap, DeltaKind::Unchanged)];
        let entries = classify(&deltas, Utc::now(), 90, |_| Some(true));
        assert_eq!(entries[0].classification, Classification::Missed);
    }

    #[test]
    fn classify_unchanged_with_signature_gone_is_fixed() {
        let snap = mk_snap("TD-C", "abc", 10);
        let deltas = vec![(snap, DeltaKind::Unchanged)];
        let entries = classify(&deltas, Utc::now(), 90, |_| Some(false));
        assert_eq!(entries[0].classification, Classification::Fixed);
    }

    #[test]
    fn classify_unchanged_old_no_signal_is_stale() {
        let snap = mk_snap("TD-D", "abc", 100); // > 90 days
        let deltas = vec![(snap, DeltaKind::Unchanged)];
        let entries = classify(&deltas, Utc::now(), 90, |_| None);
        assert_eq!(entries[0].classification, Classification::Stale);
    }

    #[test]
    fn classify_unchanged_recent_no_signal_is_uncertain() {
        let snap = mk_snap("TD-E", "abc", 30);
        let deltas = vec![(snap, DeltaKind::Unchanged)];
        let entries = classify(&deltas, Utc::now(), 90, |_| None);
        assert_eq!(entries[0].classification, Classification::Uncertain);
    }

    #[test]
    fn classify_updated_is_healthy_not_a_candidate() {
        let snap = mk_snap("TD-F", "abc", 10);
        let deltas = vec![(snap, DeltaKind::Updated)];
        let entries = classify(&deltas, Utc::now(), 90, |_| {
            panic!("source_check must not be called on Updated entries")
        });
        assert_eq!(entries[0].delta, DeltaKind::Updated);
        assert!(entries[0].reason.contains("priors"));
    }

    #[test]
    fn snapshot_skips_scaffolding_files() {
        let tmp = std::env::temp_dir().join("kronn-test-reconcile-scaffolding");
        let _ = std::fs::remove_dir_all(&tmp);
        let docs = tmp.join("docs");
        let td = docs.join("tech-debt");
        std::fs::create_dir_all(&td).unwrap();
        std::fs::write(td.join("TD-20260101-real.md"), "# real\n").unwrap();
        std::fs::write(td.join("README.md"), "# readme\n").unwrap();
        std::fs::write(td.join("TEMPLATE.md"), "# template\n").unwrap();
        std::fs::write(td.join("_reconciliation-2026-01-01.md"), "# report\n").unwrap();
        std::fs::write(td.join("_template.md"), "# t\n").unwrap();
        let snap = snapshot_tech_debt_dir(&docs);
        assert_eq!(snap.len(), 1, "scaffolding files should be skipped");
        assert_eq!(snap[0].id, "TD-20260101-real");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn compute_delta_detects_unchanged_updated_deleted() {
        let tmp = std::env::temp_dir().join("kronn-test-reconcile-delta");
        let _ = std::fs::remove_dir_all(&tmp);
        let td = tmp.join("docs").join("tech-debt");
        std::fs::create_dir_all(&td).unwrap();
        std::fs::write(td.join("TD-stable.md"), "stable content").unwrap();
        std::fs::write(td.join("TD-changed.md"), "before").unwrap();
        std::fs::write(td.join("TD-gone.md"), "doomed").unwrap();
        let snap = snapshot_tech_debt_dir(&tmp.join("docs"));
        assert_eq!(snap.len(), 3);

        // Mutate the disk to simulate post-audit state.
        std::fs::write(td.join("TD-changed.md"), "after").unwrap();
        std::fs::remove_file(td.join("TD-gone.md")).unwrap();

        let deltas = compute_delta(&snap);
        let by_id: HashMap<_, _> = deltas.iter().map(|(s, d)| (s.id.clone(), d.clone())).collect();
        assert_eq!(by_id.get("TD-stable"), Some(&DeltaKind::Unchanged));
        assert_eq!(by_id.get("TD-changed"), Some(&DeltaKind::Updated));
        assert_eq!(by_id.get("TD-gone"), Some(&DeltaKind::Deleted));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn parse_pointers_picks_file_and_line_from_where_section() {
        let md = "\
# TD-test

## Problem (fact)
not interesting

## Where (pointers)
- `src/foo.rs:42` — main offender
- `src/bar.rs:100-120` — block context
- `config/app.yaml` — no line number

## Suggested direction
also not interesting (this `path/that:1` should be ignored)
";
        let ptrs = parse_pointers(md);
        assert_eq!(ptrs.len(), 3);
        assert_eq!(ptrs[0].file, "src/foo.rs");
        assert_eq!(ptrs[0].line, Some(42));
        assert_eq!(ptrs[1].file, "src/bar.rs");
        assert_eq!(ptrs[1].line, Some(100));
        assert_eq!(ptrs[2].file, "config/app.yaml");
        assert!(ptrs[2].line.is_none());
    }

    #[test]
    fn check_signature_some_true_when_pointer_line_exists() {
        let tmp = std::env::temp_dir().join("kronn-test-reconcile-sig");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("src/foo.rs"), "line1\nline2\nline3\n").unwrap();

        let docs = tmp.join("docs/tech-debt");
        std::fs::create_dir_all(&docs).unwrap();
        let td_path = docs.join("TD-sample.md");
        std::fs::write(
            &td_path,
            "## Where (pointers)\n- `src/foo.rs:2` — line2\n",
        )
        .unwrap();

        let snap = TdSnapshot {
            path: td_path,
            id: "TD-sample".into(),
            content_hash: "n/a".into(),
            mtime: Utc::now(),
        };
        // line 2 exists → signature present.
        assert_eq!(check_signature_in_source(&snap, &tmp), Some(true));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn check_signature_none_when_no_parsable_pointer() {
        let tmp = std::env::temp_dir().join("kronn-test-reconcile-nptr");
        let _ = std::fs::remove_dir_all(&tmp);
        let docs = tmp.join("docs/tech-debt");
        std::fs::create_dir_all(&docs).unwrap();
        let td_path = docs.join("TD-x.md");
        std::fs::write(&td_path, "## Problem\nno where section at all\n").unwrap();
        let snap = TdSnapshot {
            path: td_path,
            id: "TD-x".into(),
            content_hash: "h".into(),
            mtime: Utc::now(),
        };
        assert_eq!(check_signature_in_source(&snap, &tmp), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn render_report_groups_by_classification_and_lists_updates() {
        let entries = vec![
            ReconciliationEntry {
                id: "TD-fix".into(),
                delta: DeltaKind::Deleted,
                classification: Classification::Fixed,
                reason: "removed".into(),
            },
            ReconciliationEntry {
                id: "TD-mis".into(),
                delta: DeltaKind::Unchanged,
                classification: Classification::Missed,
                reason: "still found".into(),
            },
            ReconciliationEntry {
                id: "TD-upd".into(),
                delta: DeltaKind::Updated,
                classification: Classification::Uncertain,
                reason: "refreshed".into(),
            },
        ];
        let report = render_report(&entries, "2026-05-13", "Full");
        assert!(report.contains("Reconciliation report — 2026-05-13 Full"));
        assert!(report.contains("**1** re-emitted"));
        assert!(report.contains("**2** reconciliation candidates"));
        assert!(report.contains("## ✅ Fixed"));
        assert!(report.contains("## ⚠️ Missed"));
        assert!(report.contains("TD-fix"));
        assert!(report.contains("TD-mis"));
        assert!(report.contains("TD-upd"));
    }

    // ── parse_pointers edge cases ───────────────────────────────────────

    #[test]
    fn parse_pointers_extracts_multiple_pointers_in_where_section() {
        // Real-world TD: 2-3 pointers per file is the norm. The parser must
        // emit one Pointer per `- ` line, not stop at the first.
        let content = r#"# TD title

## Where

- `src/foo.rs:42` — first hit
- `src/bar.rs:99` — second hit
- `tests/baz.rs:7` — third hit

## Other
- `not/a/pointer.rs:1` — should be ignored
"#;
        let ptrs = parse_pointers(content);
        assert_eq!(ptrs.len(), 3, "should pick exactly the 3 pointers under Where");
        assert_eq!(ptrs[0].file, "src/foo.rs");
        assert_eq!(ptrs[0].line, Some(42));
        assert_eq!(ptrs[1].file, "src/bar.rs");
        assert_eq!(ptrs[1].line, Some(99));
        assert_eq!(ptrs[2].file, "tests/baz.rs");
        assert_eq!(ptrs[2].line, Some(7));
    }

    #[test]
    fn parse_pointers_handles_pointer_without_line_number() {
        let content = "## Where\n- `src/foo.rs` — file-level finding\n";
        let ptrs = parse_pointers(content);
        assert_eq!(ptrs.len(), 1);
        assert_eq!(ptrs[0].file, "src/foo.rs");
        assert_eq!(ptrs[0].line, None);
    }

    #[test]
    fn parse_pointers_handles_line_range_takes_start() {
        // Audit emits `7-15` for multi-line spans. Reconciliation only
        // needs the START line to grep around — verify we don't drop the
        // pointer or take the END.
        let content = "## Where\n- `src/big.rs:7-15` — broad span\n";
        let ptrs = parse_pointers(content);
        assert_eq!(ptrs.len(), 1);
        assert_eq!(ptrs[0].file, "src/big.rs");
        assert_eq!(ptrs[0].line, Some(7), "ranges should yield the start line, not the end");
    }

    #[test]
    fn parse_pointers_extracts_snippet_when_present() {
        let content = "## Where\n- `src/foo.rs:10` — `fn doomed() { panic!() }` — danger\n";
        let ptrs = parse_pointers(content);
        assert_eq!(ptrs.len(), 1);
        assert_eq!(ptrs[0].file, "src/foo.rs");
        assert_eq!(ptrs[0].line, Some(10));
        assert!(ptrs[0].snippet.as_ref().is_some_and(|s| s.contains("doomed")));
    }

    #[test]
    fn parse_pointers_drops_snippet_when_too_short_or_too_long() {
        // 7-char snippet: rejected (< 8 char floor).
        let short = "## Where\n- `src/foo.rs:10` — `tiny` — too short\n";
        let pshort = parse_pointers(short);
        assert!(pshort[0].snippet.is_none(), "<8-char snippet must be discarded");

        // > 200-char snippet : rejected.
        let long_snip = "x".repeat(250);
        let long = format!("## Where\n- `src/foo.rs:10` — `{}` — too long\n", long_snip);
        let plong = parse_pointers(&long);
        assert!(plong[0].snippet.is_none(), ">200-char snippet must be discarded");
    }

    #[test]
    fn parse_pointers_ignores_lines_outside_where_section() {
        let content = r#"## Description
- `src/wrong.rs:1` — not under Where

## Repro
- `src/wrong2.rs:5` — also not Where

## Where
- `src/right.rs:42` — only this counts
"#;
        let ptrs = parse_pointers(content);
        assert_eq!(ptrs.len(), 1);
        assert_eq!(ptrs[0].file, "src/right.rs");
    }

    #[test]
    fn parse_pointers_picks_where_section_case_insensitive() {
        // Audit emits various headings depending on locale / formatter.
        for header in ["## Where", "## WHERE", "## Where (pointers)", "## Where to look"] {
            let content = format!("{}\n- `src/foo.rs:1` — match\n", header);
            let ptrs = parse_pointers(&content);
            assert_eq!(ptrs.len(), 1, "header {header:?} should be Where");
        }
    }

    #[test]
    fn parse_pointers_empty_content_returns_empty() {
        assert!(parse_pointers("").is_empty());
        assert!(parse_pointers("just a body\nno headers no pointers").is_empty());
    }

    #[test]
    fn parse_pointers_skips_non_bullet_lines_under_where() {
        // Paragraph text under Where (no `- `) must not be parsed as a pointer.
        let content = "## Where\nSome prose with `path/foo.rs:1` mention\n- `real/pointer.rs:5`\n";
        let ptrs = parse_pointers(content);
        assert_eq!(ptrs.len(), 1);
        assert_eq!(ptrs[0].file, "real/pointer.rs");
    }

    #[test]
    fn split_file_line_handles_no_colon() {
        let (file, line) = split_file_line("src/foo.rs");
        assert_eq!(file, "src/foo.rs");
        assert_eq!(line, None);
    }

    #[test]
    fn split_file_line_handles_trailing_colon_as_non_line() {
        // `foo:` has empty line part → not a line number.
        let (file, line) = split_file_line("src/foo.rs:");
        assert_eq!(file, "src/foo.rs:", "trailing colon without digits keeps colon in path");
        assert_eq!(line, None);
    }

    #[test]
    fn split_file_line_extracts_simple_line_number() {
        let (file, line) = split_file_line("src/foo.rs:42");
        assert_eq!(file, "src/foo.rs");
        assert_eq!(line, Some(42));
    }

    #[test]
    fn split_file_line_handles_range_takes_start() {
        let (file, line) = split_file_line("src/foo.rs:7-15");
        assert_eq!(file, "src/foo.rs");
        assert_eq!(line, Some(7));
    }

    #[test]
    fn split_file_line_non_numeric_part_keeps_colon_in_path() {
        // Edge case : path with a colon followed by non-numeric (unlikely on
        // Unix but defensive). Should fall back to "no line number" + keep
        // the colon as part of the path.
        let (file, line) = split_file_line("src/foo.rs:abc");
        assert_eq!(file, "src/foo.rs:abc");
        assert_eq!(line, None);
    }

    #[test]
    fn snapshot_returns_empty_for_missing_dir() {
        // Reconciliation must never panic on a fresh project without
        // tech-debt/ — that's the normal greenfield case.
        let tmp = tempfile::tempdir().unwrap();
        let snaps = snapshot_tech_debt_dir(tmp.path());
        assert!(snaps.is_empty());
    }

    #[test]
    fn snapshot_returns_empty_for_dir_with_only_scaffolding() {
        // README.md / TEMPLATE.md / _reconciliation-* are ignored.
        let tmp = tempfile::tempdir().unwrap();
        let td = tmp.path().join("tech-debt");
        std::fs::create_dir_all(&td).unwrap();
        std::fs::write(td.join("README.md"), "x").unwrap();
        std::fs::write(td.join("TEMPLATE.md"), "x").unwrap();
        std::fs::write(td.join("_reconciliation-20260528.md"), "x").unwrap();
        let snaps = snapshot_tech_debt_dir(tmp.path());
        assert!(snaps.is_empty(), "scaffolding files must not be in the snapshot");
    }

    #[test]
    fn snapshot_skips_non_md_files() {
        let tmp = tempfile::tempdir().unwrap();
        let td = tmp.path().join("tech-debt");
        std::fs::create_dir_all(&td).unwrap();
        std::fs::write(td.join("TD-real.md"), "body").unwrap();
        std::fs::write(td.join("notes.txt"), "ignored").unwrap();
        std::fs::write(td.join("data.json"), "{}").unwrap();
        let snaps = snapshot_tech_debt_dir(tmp.path());
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].id, "TD-real");
    }

    // ── Index-aware reconciliation (chantier 2, 2026-06-03) ─────────────
    // Regression: a prior re-listed in the new index but kept byte-for-byte
    // (Step 8 anti-repetition preserves correct detail files) used to be
    // classified `Missed`. It must now be `Carried` (healthy re-emission).

    #[test]
    fn parse_index_td_ids_extracts_ids_from_current_list_and_evidence() {
        let index = "\
## Current list

| ID | Problem | Area | Severity |
|----|---------|------|----------|
| TD-20260603-here-maps-apikey-committed | secret | Security | High |
| TD-20260603-deploy-no-quality-gate | gate | CI | Medium |

## Dimension coverage

| Security | findings | committed key → TD-20260603-here-maps-apikey-committed |
";
        let ids = parse_index_td_ids(index);
        assert_eq!(ids.len(), 2, "two distinct ids despite one being cited twice");
        assert!(ids.contains("TD-20260603-here-maps-apikey-committed"));
        assert!(ids.contains("TD-20260603-deploy-no-quality-gate"));
    }

    #[test]
    fn parse_index_td_ids_rejects_malformed_tokens() {
        // `TD-list`, `TD-` header, and an 8-digit-but-no-slug token must be ignored.
        let index = "Some prose about TD-list and a bare TD- and TD-20260603- with no slug.";
        let ids = parse_index_td_ids(index);
        assert!(ids.is_empty(), "got {ids:?}");
    }

    #[test]
    fn compute_delta_with_index_marks_relisted_prior_as_carried() {
        let tmp = tempfile::tempdir().unwrap();
        let td = tmp.path().join("tech-debt");
        std::fs::create_dir_all(&td).unwrap();
        let id = "TD-20260603-still-here";
        std::fs::write(td.join(format!("{id}.md")), "verbatim body").unwrap();
        let snap = snapshot_tech_debt_dir(tmp.path());
        assert_eq!(snap.len(), 1);

        // File untouched, but the id is still listed in the index.
        let mut listed = std::collections::HashSet::new();
        listed.insert(id.to_string());
        let deltas = compute_delta_with_index(&snap, &listed);
        assert_eq!(deltas[0].1, DeltaKind::Carried, "re-listed verbatim prior must be Carried, not Unchanged");

        // Same file, but the id has dropped from the index → genuine candidate.
        let empty = std::collections::HashSet::new();
        let deltas = compute_delta_with_index(&snap, &empty);
        assert_eq!(deltas[0].1, DeltaKind::Unchanged, "id absent from index → Unchanged candidate");
    }

    #[test]
    fn classify_carried_is_healthy_not_a_candidate() {
        let snap = mk_snap("TD-G", "abc", 5);
        let deltas = vec![(snap, DeltaKind::Carried)];
        let entries = classify(&deltas, Utc::now(), 90, |_| {
            panic!("source_check must not run on Carried entries")
        });
        assert_eq!(entries[0].delta, DeltaKind::Carried);
        assert!(entries[0].reason.to_lowercase().contains("carried"));
    }

    #[test]
    fn docroms_scenario_relisted_priors_are_reused_not_missed() {
        // End-to-end of the 2026-06-03 bug: 3 priors, all kept verbatim by
        // Step 8 AND all re-listed in the index. Old behavior = "Missed: 3";
        // fixed behavior = "3 re-emitted, 0 candidates".
        let tmp = tempfile::tempdir().unwrap();
        let td = tmp.path().join("tech-debt");
        std::fs::create_dir_all(&td).unwrap();
        let ids = ["TD-20260603-a", "TD-20260603-b", "TD-20260603-c"];
        for id in ids {
            std::fs::write(td.join(format!("{id}.md")), format!("body of {id}")).unwrap();
        }
        let snap = snapshot_tech_debt_dir(tmp.path());

        let index = format!(
            "## Current list\n\n| {} | x | y | Low |\n| {} | x | y | Low |\n| {} | x | y | Low |\n",
            ids[0], ids[1], ids[2]
        );
        let listed = parse_index_td_ids(&index);
        let deltas = compute_delta_with_index(&snap, &listed);
        let entries = classify(&deltas, Utc::now(), 90, |_| Some(true)); // signature present
        let report = render_report(&entries, "2026-06-03", "Full");

        assert!(report.contains("**3** re-emitted"), "report: {report}");
        // The word "Missed" appears in the heuristics footer; assert no
        // Missed *section* (no finding bucketed as Missed).
        assert!(!report.contains("## ⚠️ Missed"), "no finding should be Missed:\n{report}");
        assert!(report.contains("**0** reconciliation candidates"), "report: {report}");
    }

    #[test]
    fn snapshot_hashes_content_so_identical_files_share_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let td = tmp.path().join("tech-debt");
        std::fs::create_dir_all(&td).unwrap();
        std::fs::write(td.join("TD-1.md"), "identical body").unwrap();
        std::fs::write(td.join("TD-2.md"), "identical body").unwrap();
        let snaps = snapshot_tech_debt_dir(tmp.path());
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].content_hash, snaps[1].content_hash);
    }

    // ─── chantier 4 (2026-06-04) — re-audit dedup list ──────────────────

    #[test]
    fn digest_one_parses_severity_and_strips_id_prefix() {
        let body = "# TD-20260603-zero-tests: No automated tests\n\n**Severity**: High\n\nbody";
        let d = digest_one("TD-20260603-zero-tests", body);
        assert_eq!(d.id, "TD-20260603-zero-tests");
        assert_eq!(d.severity, "High");
        assert_eq!(d.title, "No automated tests");
    }

    #[test]
    fn digest_one_falls_back_to_id_when_no_heading_and_blank_severity_when_absent() {
        let d = digest_one("TD-20260603-foo", "no heading, no severity line here");
        assert_eq!(d.title, "TD-20260603-foo");
        assert_eq!(d.severity, "");
    }

    #[test]
    fn digest_one_handles_dash_id_separator_and_lowercase_severity() {
        let body = "# TD-20260603-bar — CSP relaxed\nseverity: critical\n";
        let d = digest_one("TD-20260603-bar", body);
        assert_eq!(d.title, "CSP relaxed");
        assert_eq!(d.severity, "Critical");
    }

    #[test]
    fn digest_prior_tech_debt_skips_scaffolding_and_sorts_by_severity() {
        let tmp = tempfile::tempdir().unwrap();
        let td = tmp.path().join("docs").join("tech-debt");
        std::fs::create_dir_all(&td).unwrap();
        std::fs::write(td.join("TD-20260101-low.md"), "# Low thing\n**Severity**: Low\n").unwrap();
        std::fs::write(td.join("TD-20260101-crit.md"), "# Crit thing\n**Severity**: Critical\n").unwrap();
        // scaffolding must be skipped
        std::fs::write(td.join("README.md"), "# readme").unwrap();
        std::fs::write(td.join("_reconciliation-20260101.md"), "x").unwrap();
        std::fs::write(td.join("TEMPLATE.md"), "# template").unwrap();

        let digests = digest_prior_tech_debt(&tmp.path().join("docs"));
        assert_eq!(digests.len(), 2, "scaffolding skipped");
        // Critical sorts before Low.
        assert_eq!(digests[0].severity, "Critical");
        assert_eq!(digests[1].severity, "Low");
    }

    #[test]
    fn digest_prior_tech_debt_empty_for_fresh_project() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(digest_prior_tech_debt(&tmp.path().join("docs")).is_empty());
    }

    #[test]
    fn render_known_debt_block_lists_priors_and_states_zero_new_is_failure() {
        let priors = vec![
            PriorDigest { id: "TD-20260603-csp".into(), severity: "Critical".into(), title: "CSP relaxed".into() },
            PriorDigest { id: "TD-20260603-blank".into(), severity: "Medium".into(), title: "target=_blank".into() },
        ];
        let block = render_known_debt_block(&priors);
        assert!(block.contains("RE-AUDIT MODE"));
        // Soft "zero new" pressure: incomplete UNLESS matrix + detectors
        // justify it (Codex finding 2 — no hallucination incentive).
        assert!(block.contains("zero"));
        assert!(block.contains("INCOMPLETE"));
        // Carried-safe wording (Codex finding 1): priors MUST stay in the
        // index, only duplication/rewrite is forbidden.
        assert!(block.contains("KEEP listing"), "priors must stay in the index = carried");
        assert!(block.contains("Do NOT silently drop"));
        assert!(block.contains("duplicate"));
        assert!(!block.contains("do not re-emit these"), "old carried-hostile wording must be gone");
        assert!(block.contains("TD-20260603-csp"));
        assert!(block.contains("CSP relaxed"));
        assert!(block.contains("TD-20260603-blank"));
    }

    #[test]
    fn render_known_debt_block_empty_priors_is_empty_string() {
        assert_eq!(render_known_debt_block(&[]), "");
    }
}
