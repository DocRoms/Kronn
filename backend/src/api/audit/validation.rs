//! Per-step output validation + auto-repair.
//!
//! 0.8.3 — Root cause of the empty-tech-debt bug on DOCROMS_WEB.
//!
//! The audit pipeline runs a CLI agent (Claude Code / Cursor / …) on
//! each step. The agent is responsible for writing the step's
//! `target_file` (e.g. `docs/inconsistencies-tech-debt.md`). Before
//! this module, the only signal we had at `step_done` was the CLI's
//! exit code: if the binary exited 0 we trusted that the file was
//! correctly filled. In practice the agent can:
//!
//!   - timeout mid-Write and leave a 0-byte file
//!   - hit a parse error in stream-json mode that surfaces as an
//!     empty Write() call
//!   - get sandbox-blocked from writing and never produce the file
//!     (Codex exec-mode) but still exit 0
//!
//! In all three cases the next audit's `copy_dir_nondestructive`
//! sees the 0-byte file, skips it, and Step 8 has nothing to fill —
//! producing 0 TDs even though the step "succeeded".
//!
//! This module checks each step's output AFTER the CLI exits:
//!   - target file exists
//!   - size is plausible vs the template source (≥ 25 %)
//!
//! If the check fails, we emit a `step_warning` SSE event AND
//! auto-repair the file from the template so the user can either
//! re-run the step OR ship the audit knowing it's flagged.

use std::path::{Path, PathBuf};

/// The minimum dest/src size ratio (in %) below which a step's
/// output is treated as corrupt. Mirrors the threshold used by
/// `api::projects::template::copy_dir_nondestructive` so a step that
/// fails here will also be repaired on the next audit if the user
/// didn't restart.
const MIN_DEST_RATIO_PCT: u64 = 25;

/// Minimum template source size (in bytes) below which the
/// heuristic is disabled. Tiny templates produce too many false
/// positives (a short README, a one-line example file).
const MIN_TEMPLATE_SIZE_B: u64 = 200;

/// Outcome of a step-output validation.
pub struct StepValidationWarning {
    pub reason: String,
    pub repaired: bool,
}

/// Check that a step's target file is plausibly filled. If it's
/// missing or suspiciously small, auto-repair from the template (if
/// available) and return a warning. The function returns `(success,
/// Option<warning>)` where `success` is the effective step status
/// the caller should report at `step_done`.
///
/// `cli_success` is the raw CLI exit-code success — if it's already
/// `false`, we don't bother repairing (the failure is already loud).
pub fn validate_and_repair_step_output(
    cli_success: bool,
    project_path: &Path,
    target_file: &str,
) -> (bool, Option<StepValidationWarning>) {
    // Synthetic targets (the final "REVIEW" pseudo-step) have no
    // file on disk to check.
    if target_file == "REVIEW" || target_file.is_empty() {
        return (cli_success, None);
    }

    // We only validate paths that live under the project's `docs/`
    // directory — other targets are out of scope.
    if !target_file.starts_with("docs/") {
        return (cli_success, None);
    }

    let dst_path = project_path.join(target_file);
    let template_path = template_source_for(target_file);

    // Get the on-disk size, treating "missing" as 0.
    let dst_size = std::fs::metadata(&dst_path).map(|m| m.len()).unwrap_or(0);

    // Read template size when available; if there's no template
    // shipped with Kronn for this file, we can only flag "missing"
    // — sizing decisions need the template baseline.
    let template_size = template_path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);

    if template_size < MIN_TEMPLATE_SIZE_B {
        // No usable template OR tiny template — only flag a totally
        // missing file in this branch.
        if dst_size == 0 {
            return (
                false,
                Some(StepValidationWarning {
                    reason: format!(
                        "step produced no output: `{}` is missing or empty",
                        target_file
                    ),
                    repaired: false,
                }),
            );
        }
        return (cli_success, None);
    }

    let ratio_pct = dst_size.saturating_mul(100) / template_size;
    if dst_size == 0 || ratio_pct < MIN_DEST_RATIO_PCT {
        // Try to repair from template so the audit can complete on
        // a clean baseline (or the user can re-run the step).
        let repaired = if let Some(src) = &template_path {
            if let Some(parent) = dst_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::copy(src, &dst_path).is_ok()
        } else {
            false
        };
        let reason = if dst_size == 0 {
            format!(
                "step produced no output: `{}` is empty (0 B) after CLI completed",
                target_file
            )
        } else {
            format!(
                "step output looks truncated: `{}` is {} B but template is {} B (< {}% threshold)",
                target_file, dst_size, template_size, MIN_DEST_RATIO_PCT
            )
        };
        return (false, Some(StepValidationWarning { reason, repaired }));
    }

    // 0.8.3 (#310) — Placeholder leakage check.
    //
    // The size heuristic above passes when dest >= 25% of template,
    // which is correct for legitimately-edited content. BUT it ALSO
    // passes when the dest is the EXACT template byte-for-byte —
    // exactly what happens when claude rate-limits / crashes before
    // touching the file: copy_dir_nondestructive seeded the template
    // in Phase 1, then claude was supposed to fill in placeholders,
    // but never ran. Result: file is identical to template, size is
    // 100% of template, validation thinks step succeeded, audit
    // marches on producing nothing useful for 5 more steps.
    //
    // Detection: scan target_file for raw `{{TOKEN}}` placeholders.
    // Real templates ship with these; a successful agent run replaces
    // every one. If ANY survive, the step did not produce its output
    // — treat as failure (no auto-repair since the file IS the
    // template; re-running the step is the only path forward).
    //
    // Note for Step 8 (tech-debt): the index file
    // `docs/inconsistencies-tech-debt.md` may carry leaked placeholders
    // even when the agent already created several `docs/tech-debt/TD-*.md`
    // detail files (partial success). That's fine: the audit resume
    // mechanism (#311) re-runs Step 8; the prompt's anti-repetition
    // rules preserve the detail files and just finalize the index. So
    // we keep the detection binary — partial progress is recovered by
    // the resume layer, not by relaxing the leak check here.
    if let Ok(content) = std::fs::read_to_string(&dst_path) {
        let leaked = count_raw_placeholders(&content);
        if leaked > 0 {
            return (
                false,
                Some(StepValidationWarning {
                    reason: format!(
                        "step did not fill `{}`: {} raw `{{{{...}}}}` placeholders remain (the file is still the template — agent likely crashed / rate-limited before writing)",
                        target_file, leaked
                    ),
                    repaired: false, // file IS the template; nothing to restore
                }),
            );
        }
        // Step 8 — structural gate on the `## Dimension coverage` matrix
        // (Codex re-review). BLOCKING (`success=false`): a missing/short/
        // ill-formed matrix marks Step 8 incomplete so the resume layer
        // re-runs it (anti-repetition preserves the TD detail files). A
        // non-blocking warning would let `full.rs` mark the step `success`
        // + advance `last_completed_step` → the "incomplete audit" promise
        // would be cosmetic. Re-run cost < validating a knowingly-broken run.
        if target_file.ends_with("inconsistencies-tech-debt.md") {
            if let Err(reason) = validate_dimension_coverage(&content) {
                return (
                    false,
                    Some(StepValidationWarning {
                        reason: format!("dimension coverage incomplete in `{}`: {} (Step 8 will be re-run)", target_file, reason),
                        repaired: false,
                    }),
                );
            }
        }
    }

    (cli_success, None)
}

/// chantier 1b (2026-06-03) — detector disposition gate.
///
/// After Step 8, every deterministic detector signal injected into the
/// prompt MUST show up addressed somewhere in the Step-8 output surface
/// (the tech-debt index + the `TD-*.md` detail files): emitted as a TD,
/// folded into a baseline note, or cited in the coverage matrix. A signal
/// whose anchor (flagged file / keyword) appears NOWHERE = the agent
/// silently ignored a ground-truth signal → Step 8 is incomplete.
///
/// Returns a blocking `StepValidationWarning` (so the resume layer re-runs
/// Step 8) when ≥1 signal is undisposed; `None` when all are addressed or
/// no signals fired. Lenient by design (see `undisposed_signals`): catches
/// omissions, not mischaracterizations — and never auto-repairs (re-running
/// is the only path; the file content is the agent's, not the template's).
pub fn check_detector_disposition(
    project_path: &Path,
    signals: &[crate::core::audit_detectors::DetectedSignal],
) -> Option<StepValidationWarning> {
    if signals.is_empty() {
        return None;
    }
    // Combined Step-8 output surface = the current index + ONLY the TD
    // detail files still listed in it. On an in-place re-audit, an orphan
    // historical `TD-*.md` (no longer referenced in the freshly-written
    // index) must NOT count as "disposing" a signal — that's exactly the
    // masking-by-history we're fighting (Codex 1b review 2026-06-04). We
    // gate the detail files through `parse_index_td_ids` so only the
    // current run's live findings contribute to disposition.
    let index_content = std::fs::read_to_string(
        project_path.join("docs/inconsistencies-tech-debt.md"),
    )
    .unwrap_or_default();
    let listed_ids = super::reconciliation::parse_index_td_ids(&index_content);
    let mut combined = index_content;
    let td_dir = project_path.join("docs/tech-debt");
    if let Ok(entries) = std::fs::read_dir(&td_dir) {
        for e in entries.flatten() {
            let p = e.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else { continue };
            if !name.starts_with("TD-") || !name.ends_with(".md") {
                continue;
            }
            let id = name.trim_end_matches(".md");
            if !listed_ids.contains(id) {
                continue; // orphan history — not part of this run's output
            }
            if let Ok(body) = std::fs::read_to_string(&p) {
                combined.push('\n');
                combined.push_str(&body);
            }
        }
    }

    let undisposed = crate::core::audit_detectors::undisposed_signals(signals, &combined);
    if undisposed.is_empty() {
        return None;
    }
    let list = undisposed
        .iter()
        .map(|s| format!("{} (`{}`)", s.title, s.evidence))
        .collect::<Vec<_>>()
        .join("; ");
    Some(StepValidationWarning {
        reason: format!(
            "{} detector signal(s) injected but never addressed in the tech-debt output: {} — emit a TD, a baseline note, or a matrix citation for each (Step 8 will be re-run)",
            undisposed.len(),
            list
        ),
        repaired: false,
    })
}

/// Count `{{IDENT}}` shaped placeholders in a markdown body. Conservative:
/// only counts uppercase-snake tokens to avoid matching legitimate Twig /
/// Liquid / Mermaid syntax that uses `{{` for its own purposes (e.g.
/// `{{ asset('foo') }}` in a Twig snippet inside coding-rules.md, or
/// `{{ DECISION_1 }}` is matched, but `{{ asset(...) }}` is not).
fn count_raw_placeholders(content: &str) -> usize {
    // Match `{{IDENT}}` and `{{ IDENT }}` where IDENT is
    // UPPERCASE_SNAKE (with optional digits + _). The trailing
    // boundary is a literal `}}`, not just `}`, to avoid hits on
    // Twig double-brace blocks that contain spaces / parens.
    let mut count = 0usize;
    let mut rest = content;
    while let Some(start) = rest.find("{{") {
        let after_open = &rest[start + 2..];
        // Find closing `}}`
        let Some(end) = after_open.find("}}") else { break };
        let inside = after_open[..end].trim();
        // Must be a single UPPERCASE_SNAKE token (letters / digits / _).
        // No spaces, no parens, no quotes. This excludes Twig.
        let is_placeholder = !inside.is_empty()
            && inside.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && inside.chars().any(|c| c.is_ascii_uppercase());
        if is_placeholder {
            count += 1;
        }
        rest = &after_open[end + 2..];
    }
    count
}

/// The 10 dimensions Step 8 § B must account for in the coverage matrix.
const COVERAGE_DIMENSIONS: &[&str] = &[
    "Dependencies", "Security", "Code quality", "Scalability", "Maintainability",
    "Accessibility", "Observability", "Compliance", "Performance", "Documentation drift",
];

/// Structural validation of the `## Dimension coverage` matrix in
/// `inconsistencies-tech-debt.md` (Codex review #26, 2026-06-03). Cheap, and
/// does NOT prove the scan actually happened — it just catches a missing /
/// short / ill-formed matrix so the prompt's "incomplete audit" promise is
/// mechanically backed (the future detectors anchor ground truth). Returns
/// `Err(reason)` on the first structural problem found.
fn validate_dimension_coverage(content: &str) -> Result<(), String> {
    let Some(sec_start) = content.find("## Dimension coverage") else {
        return Err("`## Dimension coverage` matrix is missing".to_string());
    };
    // Section body = from the heading until the next `## ` heading or EOF.
    let after = &content[sec_start..];
    let body = after.find("\n## ").map(|n| &after[..n]).unwrap_or(after);

    // N/A reasons we refuse when they stand alone (no verifiable content).
    const TRIVIAL: &[&str] = &["not relevant", "pas pertinent", "n/a", "na", "unknown", "none", "-", ""];

    let mut missing: Vec<&str> = Vec::new();
    for dim in COVERAGE_DIMENSIONS {
        // Match the row by a case-insensitive PREFIX on the first cell, not an
        // exact equality: agents naturally embellish the canonical label
        // (e.g. `Accessibility (a11y)`, `Documentation drift / docs`) and an
        // exact match would mis-count those as a missing row and FAIL Step 8.
        // None of the 10 dimension names is a prefix of another, so a prefix
        // match stays unambiguous. (Fixes the 2026-06-03 self-inflicted
        // Step-8-red on DOCROMS_WEB: the agent wrote `Accessibility (a11y)`.)
        let dim_lc = dim.to_ascii_lowercase();
        let rows: Vec<&str> = body.lines().filter(|l| {
            let l = l.trim();
            l.starts_with('|') && {
                let first = l.trim_start_matches('|').split('|').next().unwrap_or("").trim();
                first.to_ascii_lowercase().starts_with(&dim_lc)
            }
        }).collect();
        let row = match rows.as_slice() {
            [] => { missing.push(dim); continue; }
            [r] => *r,
            _ => return Err(format!("dimension `{}` appears {} times — exactly one row per dimension", dim, rows.len())),
        };
        let cells: Vec<&str> = row.trim().trim_matches('|').split('|').map(|c| c.trim()).collect();
        let outcome = cells.get(1).copied().unwrap_or("");
        let evidence = cells.get(2).copied().unwrap_or("");
        let o = outcome.to_ascii_lowercase();
        // `scanned` must be the full phrase (em-dash OR plain hyphen tolerated) —
        // a bare "scanned later" / "scanned?" must NOT pass (Codex re-review).
        let valid_prefix = o.starts_with("findings")
            || o.starts_with("scanned — nothing substantiable")
            || o.starts_with("scanned - nothing substantiable")
            || o.starts_with("n/a");
        if outcome.is_empty() || !valid_prefix {
            return Err(format!(
                "dimension `{}`: outcome must start with `findings` / `scanned — nothing substantiable` / `N/A:`, got `{}`",
                dim, outcome
            ));
        }
        if evidence.is_empty() {
            return Err(format!("dimension `{}`: evidence/reason cell is empty", dim));
        }
        if o.starts_with("n/a") {
            // Reason after `N/A:` (or, failing that, the evidence cell) must be non-trivial.
            let reason = outcome.split_once(':').map(|(_, r)| r.trim()).filter(|s| !s.is_empty()).unwrap_or(evidence);
            if TRIVIAL.contains(&reason.to_ascii_lowercase().trim()) {
                return Err(format!("dimension `{}`: N/A needs a human-verifiable reason, got `{}`", dim, reason));
            }
        }
    }
    if !missing.is_empty() {
        return Err(format!("coverage matrix missing rows: {}", missing.join(", ")));
    }
    Ok(())
}

/// Resolve the source-of-truth template path for a project-relative
/// docs path. Returns `None` if no template ships with that name
/// (e.g. a sub-audit's `inconsistencies-security.md` doesn't have a
/// pre-installed template).
fn template_source_for(target_file: &str) -> Option<PathBuf> {
    // Templates live under `templates/` relative to the repo root.
    // The resolver in `api::projects` handles WSL paths + Docker
    // container layouts; we reuse it so this module works in both
    // local-cargo and docker-prod paths.
    let template_dir = crate::api::projects::resolve_templates_dir();
    if !template_dir.exists() {
        return None;
    }
    let candidate = template_dir.join(target_file);
    if candidate.exists() { Some(candidate) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;

    // All tests that mutate the `KRONN_TEMPLATES_DIR` env var are
    // serialized under the `kronn_templates_env` key so they don't
    // race with each other or with mcp_scanner_test.rs (which touches
    // the same env var). Tests that don't read the env var are left
    // parallel.

    /// Build a fake project root with `target_file` written at the
    /// requested size, paired with a fake `templates/` directory
    /// containing a template of the requested size.
    fn fixture(target_file: &str, dst_bytes: usize, template_bytes: usize) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let templates = tmp.path().join("templates");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&templates).unwrap();

        let dst = project.join(target_file);
        if let Some(p) = dst.parent() { std::fs::create_dir_all(p).unwrap(); }
        let mut f = std::fs::File::create(&dst).unwrap();
        if dst_bytes > 0 {
            f.write_all(&vec![b'x'; dst_bytes]).unwrap();
        }
        let tsrc = templates.join(target_file);
        if let Some(p) = tsrc.parent() { std::fs::create_dir_all(p).unwrap(); }
        let mut t = std::fs::File::create(&tsrc).unwrap();
        if template_bytes > 0 {
            t.write_all(&vec![b'y'; template_bytes]).unwrap();
        }
        // KRONN_TEMPLATES_DIR is consumed by resolve_templates_dir
        // when set; the function honors it as the override so tests
        // can point at our fake templates root.
        std::env::set_var("KRONN_TEMPLATES_DIR", &templates);
        (tmp, project)
    }

    #[test]
    fn review_pseudo_step_is_always_ok() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (success, warn) = validate_and_repair_step_output(true, tmp.path(), "REVIEW");
        assert!(success);
        assert!(warn.is_none());
    }

    #[test]
    fn empty_target_file_path_is_passthrough() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (success, warn) = validate_and_repair_step_output(true, tmp.path(), "");
        assert!(success);
        assert!(warn.is_none());
    }

    #[test]
    fn non_docs_path_is_passthrough() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (success, _) = validate_and_repair_step_output(true, tmp.path(), "src/foo.rs");
        assert!(success);
    }

    #[test]
    fn already_failed_cli_is_preserved() {
        // If the CLI itself exited non-zero, we don't override the
        // success bit — the failure is already loud.
        let tmp = tempfile::TempDir::new().unwrap();
        let (success, _) = validate_and_repair_step_output(false, tmp.path(), "REVIEW");
        assert!(!success);
    }

    #[test]
    #[serial(kronn_templates_env)]
    fn healthy_dest_passes_through() {
        let (_tmp, project) = fixture("docs/foo.md", 1000, 1000);
        let (success, warn) =
            validate_and_repair_step_output(true, &project, "docs/foo.md");
        assert!(success);
        assert!(warn.is_none());
    }

    // ── Dimension coverage matrix (Codex review) ──────────────────────────
    fn valid_coverage_matrix() -> String {
        let mut s = String::from("## Dimension coverage\n\n| Dimension | Outcome | Evidence / reason |\n|---|---|---|\n");
        for (dim, outcome, ev) in [
            ("Dependencies", "findings", "TD-x"),
            ("Security", "findings", "TD-y"),
            ("Code quality", "scanned — nothing substantiable", "read src/, clean"),
            ("Scalability", "N/A: no DB/ORM layer", "grep: no ORM import"),
            ("Maintainability", "scanned — nothing substantiable", "ok"),
            ("Accessibility", "N/A: no web surface", "CLI binary only"),
            ("Observability", "findings", "TD-z"),
            ("Compliance", "scanned — nothing substantiable", "MIT only"),
            ("Performance", "N/A: not perf-sensitive per README", "README says batch tool"),
            ("Documentation drift", "findings", "TD-w"),
        ] {
            s.push_str(&format!("| {} | {} | {} |\n", dim, outcome, ev));
        }
        s
    }

    // ── detector disposition gate (chantier 1b) ──
    use crate::core::audit_detectors::{DetectedSignal, Severity};

    fn mk_signal(id: &'static str, ev: &str, title: &str) -> DetectedSignal {
        DetectedSignal {
            detector_id: id,
            dimension: "Security",
            severity: Severity::Medium,
            title: title.into(),
            evidence: ev.into(),
        }
    }

    #[test]
    fn disposition_gate_empty_signals_is_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(check_detector_disposition(tmp.path(), &[]).is_none());
    }

    #[test]
    fn disposition_gate_fails_on_unaddressed_signal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(docs.join("tech-debt")).unwrap();
        // Index cites the CSP file but never the _blank template.
        std::fs::write(
            docs.join("inconsistencies-tech-debt.md"),
            "## Baseline\nCSP in src/Headers.php reviewed.\n",
        )
        .unwrap();
        let signals = vec![
            mk_signal("csp-unsafe", "src/Headers.php:87", "CSP unsafe-eval"),
            mk_signal("blank-noopener", "templates/pages/projets.html.twig:12", "_blank w/o noopener"),
        ];
        let w = check_detector_disposition(tmp.path(), &signals)
            .expect("an unaddressed signal must produce a blocking warning");
        assert!(!w.repaired);
        assert!(w.reason.contains("projets.html.twig"), "warning names the undisposed file: {}", w.reason);
        assert!(!w.reason.contains("Headers.php"), "the cited file must NOT be flagged: {}", w.reason);
    }

    #[test]
    fn disposition_gate_passes_when_all_addressed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(docs.join("tech-debt")).unwrap();
        // Index LISTS the TD id (a disposed TD is referenced in the index)
        // so it counts toward the combined surface (Codex 1b #1).
        std::fs::write(
            docs.join("inconsistencies-tech-debt.md"),
            "## Current list\n- TD-20260603-blank — see detail\n",
        ).unwrap();
        // The TD detail file addresses the _blank template (combined surface).
        std::fs::write(
            docs.join("tech-debt/TD-20260603-blank.md"),
            "Tabnabbing in templates/pages/projets.html.twig — add rel=noopener.\n",
        )
        .unwrap();
        let signals = vec![mk_signal("blank-noopener", "templates/pages/projets.html.twig:12", "x")];
        assert!(
            check_detector_disposition(tmp.path(), &signals).is_none(),
            "signal addressed in a TD detail file (listed in the index) must pass the gate"
        );
    }

    #[test]
    fn disposition_gate_orphan_td_not_in_index_does_not_dispose() {
        // Codex 1b review (2026-06-04): an old TD file left on disk but NO
        // LONGER referenced in the freshly-written index must NOT satisfy
        // the gate — otherwise history masks an undisposed signal on an
        // in-place re-audit. Same content as the passing test above, but
        // the index does NOT list the TD id → the signal is undisposed.
        let tmp = tempfile::TempDir::new().unwrap();
        let docs = tmp.path().join("docs");
        std::fs::create_dir_all(docs.join("tech-debt")).unwrap();
        std::fs::write(
            docs.join("inconsistencies-tech-debt.md"),
            "## Current list\nsee detail files\n", // no TD id referenced
        ).unwrap();
        std::fs::write(
            docs.join("tech-debt/TD-20260603-blank.md"),
            "Tabnabbing in templates/pages/projets.html.twig — add rel=noopener.\n",
        )
        .unwrap();
        let signals = vec![mk_signal("blank-noopener", "templates/pages/projets.html.twig:12", "x")];
        let w = check_detector_disposition(tmp.path(), &signals)
            .expect("orphan TD not in the index must NOT dispose the signal");
        assert!(w.reason.contains("projets.html.twig"), "got: {}", w.reason);
    }

    #[test]
    fn coverage_matrix_valid_passes() {
        assert!(validate_dimension_coverage(&valid_coverage_matrix()).is_ok());
    }

    #[test]
    fn coverage_matrix_embellished_dimension_label_passes() {
        // Regression (2026-06-03 self-inflicted Step-8-red on DOCROMS_WEB):
        // the agent wrote `Accessibility (a11y)` as the row label. An exact
        // `eq_ignore_ascii_case` match mis-counted it as a missing row and
        // FAILED Step 8 → resume re-ran → run Interrupted. A prefix match
        // accepts the natural embellishment while staying unambiguous.
        let m = valid_coverage_matrix()
            .replace("| Accessibility |", "| Accessibility (a11y) |")
            .replace("| Documentation drift |", "| Documentation drift / docs |");
        assert!(validate_dimension_coverage(&m).is_ok(), "embellished labels must still match");
    }

    #[test]
    fn coverage_matrix_missing_section_fails() {
        let err = validate_dimension_coverage("# tech debt\nno matrix here").unwrap_err();
        assert!(err.contains("missing"), "{err}");
    }

    #[test]
    fn coverage_matrix_missing_a_dimension_fails() {
        // Drop the "Performance" row.
        let m = valid_coverage_matrix().replace("| Performance | N/A: not perf-sensitive per README | README says batch tool |\n", "");
        let err = validate_dimension_coverage(&m).unwrap_err();
        assert!(err.contains("Performance"), "{err}");
    }

    #[test]
    fn coverage_matrix_trivial_na_reason_fails() {
        let m = valid_coverage_matrix().replace("N/A: no web surface", "N/A: pas pertinent").replace("CLI binary only", "");
        let err = validate_dimension_coverage(&m).unwrap_err();
        assert!(err.to_lowercase().contains("verifiable") || err.contains("Accessibility"), "{err}");
    }

    #[test]
    fn coverage_matrix_bad_outcome_prefix_fails() {
        let m = valid_coverage_matrix().replace("| Security | findings | TD-y |", "| Security | maybe later | TD-y |");
        let err = validate_dimension_coverage(&m).unwrap_err();
        assert!(err.contains("Security"), "{err}");
    }

    #[test]
    fn coverage_matrix_duplicate_dimension_fails() {
        // Two Security rows — rejected even though all 10 dimensions are present.
        let m = valid_coverage_matrix() + "| Security | findings | TD-dup |\n";
        let err = validate_dimension_coverage(&m).unwrap_err();
        assert!(err.contains("Security") && err.contains("appears"), "{err}");
    }

    #[test]
    fn coverage_matrix_vague_scanned_fails() {
        // `scanned later` must NOT satisfy the `scanned — nothing substantiable` outcome.
        let m = valid_coverage_matrix().replace(
            "| Code quality | scanned — nothing substantiable | read src/, clean |",
            "| Code quality | scanned later | read src/, clean |",
        );
        let err = validate_dimension_coverage(&m).unwrap_err();
        assert!(err.contains("Code quality"), "{err}");
    }

    #[test]
    #[serial(kronn_templates_env)]
    fn empty_dest_flagged_and_repaired() {
        // 0 B vs 1000 B template — must flag + repair.
        let (_tmp, project) = fixture("docs/inconsistencies-tech-debt.md", 0, 1000);
        let (success, warn) = validate_and_repair_step_output(
            true,
            &project,
            "docs/inconsistencies-tech-debt.md",
        );
        assert!(!success, "empty dest must be reported as failure");
        let w = warn.expect("a warning must be emitted for empty output");
        assert!(w.repaired, "template repair must succeed when template is on disk");
        assert!(w.reason.contains("empty (0 B)"));
        // File now contains the template bytes.
        let after = std::fs::read(project.join("docs/inconsistencies-tech-debt.md")).unwrap();
        assert_eq!(after.len(), 1000);
    }

    #[test]
    #[serial(kronn_templates_env)]
    fn truncated_dest_flagged_and_repaired() {
        // 100 B vs 1000 B = 10 % → below 25 % threshold.
        let (_tmp, project) = fixture("docs/architecture/overview.md", 100, 1000);
        let (success, warn) = validate_and_repair_step_output(
            true,
            &project,
            "docs/architecture/overview.md",
        );
        assert!(!success);
        let w = warn.unwrap();
        assert!(w.repaired);
        assert!(w.reason.contains("truncated"));
    }

    #[test]
    #[serial(kronn_templates_env)]
    fn dest_at_threshold_is_preserved() {
        // 300 B vs 1000 B = 30 % → above 25 % threshold, must NOT
        // flag or touch the user's content.
        let (_tmp, project) = fixture("docs/foo.md", 300, 1000);
        let before = std::fs::read(project.join("docs/foo.md")).unwrap();
        let (success, warn) =
            validate_and_repair_step_output(true, &project, "docs/foo.md");
        assert!(success);
        assert!(warn.is_none());
        let after = std::fs::read(project.join("docs/foo.md")).unwrap();
        assert_eq!(before, after, "user content must be untouched above threshold");
    }

    #[test]
    #[serial(kronn_templates_env)]
    fn placeholder_leakage_is_detected_even_when_size_matches_template() {
        // 0.8.3 (#310) — DOCROMS_WEB user hit this: claude rate-limited
        // BEFORE writing decisions.md, the file stayed at exact template
        // (1.8K, 100% of template size), validate passed → audit
        // continued to Step 9 producing nothing → marked Audited
        // wrongly. The new placeholder check rejects the step even
        // though the size is right.
        let template_body =
            "# Decisions\n\n## Real content the agent should fill\n\n\
             | {{DECISION_1}} | {{REASON}} | {{ANTI_PATTERN}} | {{FILE_OR_USER}} |\n\
             | {{DECISION_2}} | {{REASON}} | {{ANTI_PATTERN}} | {{FILE_OR_USER}} |\n\n\
             Lots of useful prose to push template size above 200 B and \
             enough body to satisfy the 25% size ratio. ".repeat(2);
        let (_tmp, project) = fixture("docs/decisions.md", template_body.len(), template_body.len());
        // Overwrite both with the identical placeholder-laden content so
        // the size ratio is 100% but the placeholders remain.
        std::fs::write(project.join("docs/decisions.md"), &template_body).unwrap();
        // Also overwrite the template (the fixture wrote `y`-bytes).
        let tpl_dir = std::env::var("KRONN_TEMPLATES_DIR").unwrap();
        std::fs::write(std::path::PathBuf::from(tpl_dir).join("docs/decisions.md"), &template_body).unwrap();

        let (success, warn) = validate_and_repair_step_output(true, &project, "docs/decisions.md");
        assert!(!success, "step with leaked placeholders must fail validation");
        let w = warn.expect("a warning must be emitted");
        assert!(w.reason.contains("placeholders remain"),
            "warning must call out the placeholder leakage explicitly (got: {})", w.reason);
        assert!(!w.repaired,
            "no auto-repair when the file IS the template (re-running the step is the only path)");
    }

    #[test]
    fn count_raw_placeholders_recognizes_uppercase_snake_tokens() {
        // Pin the placeholder shape so a refactor doesn't widen / narrow
        // the regex unintentionally. Examples below cover (a) the
        // canonical `{{DECISION_1}}`, (b) spaced variants, (c) Twig
        // syntax that must NOT match.
        assert_eq!(count_raw_placeholders("| {{DECISION_1}} |"), 1);
        assert_eq!(count_raw_placeholders("{{ DECISION_1 }} and {{REASON}}"), 2);
        assert_eq!(count_raw_placeholders("nothing here"), 0);
        // Twig: don't false-positive — the inner expression has lowercase + parens.
        assert_eq!(count_raw_placeholders("{{ asset('app.css') }}"), 0);
        assert_eq!(count_raw_placeholders("{{ cspNonce }}"), 0);
        // Mixed body
        let body = "| {{ID}} | {{ asset('x') }} | {{SEVERITY}} |";
        assert_eq!(count_raw_placeholders(body), 2);
    }

    #[test]
    #[serial(kronn_templates_env)]
    fn missing_template_only_flags_empty_dest() {
        // No template on disk (sub-audit case): we can only flag a
        // total miss; we can't size-check.
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join("docs")).unwrap();
        std::fs::write(project.join("docs/inconsistencies-security.md"), "").unwrap();
        // Force resolver away from real templates dir.
        std::env::set_var("KRONN_TEMPLATES_DIR", tmp.path().join("nope"));
        let (success, warn) = validate_and_repair_step_output(
            true,
            &project,
            "docs/inconsistencies-security.md",
        );
        assert!(!success);
        let w = warn.unwrap();
        assert!(!w.repaired, "no template to repair from");
        assert!(w.reason.contains("missing or empty"));
    }
}
