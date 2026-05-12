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
    /// File present, content identical to snapshot — agent didn't
    /// re-emit. Candidate for classification.
    Unchanged,
    /// File present, content differs — agent refreshed in place.
    /// Healthy outcome of the priors rule (§ C of Step 9 prompt).
    Updated,
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

/// Compute the delta between the pre-audit snapshot and the post-audit
/// state. Returns one entry per snapshot TD (so we know what to report
/// on); brand-new TDs created by this audit are NOT included — they're
/// the audit's normal output, not reconciliation candidates.
pub fn compute_delta(snapshot: &[TdSnapshot]) -> Vec<(TdSnapshot, DeltaKind)> {
    snapshot
        .iter()
        .map(|snap| {
            let delta = match std::fs::read_to_string(&snap.path) {
                Err(_) => DeltaKind::Deleted,
                Ok(content) => {
                    if sha256_hex(&content) == snap.content_hash {
                        DeltaKind::Unchanged
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
    let mut updated = Vec::new();
    for e in entries {
        if e.delta == DeltaKind::Updated {
            updated.push(e);
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
    out.push_str(&format!("- **{}** updated in place (priors reused — healthy)\n", updated.len()));
    out.push_str(&format!("- **{total_candidates}** reconciliation candidates :\n"));
    for label in &["Fixed", "Stale", "Missed", "Uncertain"] {
        let count = buckets.get(*label).map(|v| v.len()).unwrap_or(0);
        if count > 0 {
            out.push_str(&format!("  - {label}: {count}\n"));
        }
    }
    out.push('\n');

    if !updated.is_empty() {
        out.push_str("## ✓ Updated in place (priors reused)\n\n");
        for e in &updated {
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
        assert!(report.contains("**1** updated in place"));
        assert!(report.contains("**2** reconciliation candidates"));
        assert!(report.contains("## ✅ Fixed"));
        assert!(report.contains("## ⚠️ Missed"));
        assert!(report.contains("TD-fix"));
        assert!(report.contains("TD-mis"));
        assert!(report.contains("TD-upd"));
    }
}
