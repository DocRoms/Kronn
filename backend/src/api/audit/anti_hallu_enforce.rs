//! Enforce-mode anti-hallucination gate for the audit pipeline (0.8.8, PR-A).
//!
//! In `AntiHallucMode::Enforce`, an audit step that writes a `docs/` file is
//! held to the formal `[src: …]` provenance contract: after the agent finishes,
//! the written file is mechanically re-linted ([`crate::core::anti_halluc`]).
//! If any formal citation is **fabricated** (file doesn't exist, line out of
//! bounds, escapes the project root, training-data), the step is re-run with a
//! corrective addendum (bounded by [`MAX_ATTEMPTS`]); if it still can't produce
//! clean citations, the step fails so the audit ends *Interrupted* rather than
//! committing a hallucinated doc.
//!
//! Everything here is **pure** (no FS writes, no agent spawns) so it is unit
//! testable; the streaming generator in [`super::full`] owns the IO and the
//! retry loop, and only calls into these helpers.

use crate::core::anti_halluc::{self, SourceCheck};
use std::path::Path;

/// Total attempts (1 initial + retries) allowed per step in enforce mode.
/// Design caps the corrective loop at 2-3 attempts: the agent already carries
/// the `[src:]` grammar, so a second pass usually fixes a stale `file:line`
/// after a refactor — but we don't burn unbounded tokens chasing it.
pub const MAX_ATTEMPTS: usize = 3;

/// The fabricated formal citations found in a written step file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CitationVerdict {
    /// The `[src: …]` markers whose mechanical status is high-confidence
    /// fabricated (NotFound / OutOfBounds / EmptyRef / OutsideProject /
    /// Rejected). Soft `unverified` inline anchors are intentionally excluded —
    /// the enforce gate only blocks on the high-confidence signal.
    pub fabricated: Vec<SourceCheck>,
}

impl CitationVerdict {
    pub fn count(&self) -> usize {
        self.fabricated.len()
    }
    pub fn is_clean(&self) -> bool {
        self.fabricated.is_empty()
    }
}

/// What the per-step enforce gate decides after re-linting the written file.
/// Pure so the streaming generator's branching is unit-testable without a live
/// agent (the generator owns the IO: re-run, emit SSE, stamp).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// Citations clean — stamp `audit=` dates and finish the step.
    Pass,
    /// Fabricated citations, but attempts remain — re-run with corrective feedback.
    Retry,
    /// Fabricated citations and the attempt budget is spent — fail the step.
    Fail,
}

/// Decide the gate outcome from the lint verdict and where we are in the retry
/// budget. `attempt` is 1-based (the attempt that just produced this verdict).
pub fn decide(verdict: &CitationVerdict, attempt: usize, max_attempts: usize) -> GateDecision {
    if verdict.is_clean() {
        GateDecision::Pass
    } else if attempt < max_attempts {
        GateDecision::Retry
    } else {
        GateDecision::Fail
    }
}

/// Mechanically lint a written step file's content for fabricated formal
/// citations. `roots` are the project roots the `[src: file:line]` markers are
/// resolved against (the audit runs in the main checkout, so a single root).
pub fn lint_step_file(content: &str, roots: &[&Path]) -> CitationVerdict {
    let report = anti_halluc::analyze_roots(content, roots);
    let fabricated = report
        .sources
        .into_iter()
        .filter(|s| s.status.is_fabricated())
        .collect();
    CitationVerdict { fabricated }
}

/// Build the corrective prompt addendum re-injected on a retry. Names each
/// fabricated citation and the verdict so the agent can fix the reference or
/// drop the claim — it must NOT invent a new path to satisfy the linter.
pub fn corrective_feedback(file_label: &str, verdict: &CitationVerdict) -> String {
    let mut out = String::from(
        "## ⛔ Anti-hallucination gate (enforce mode) — fix before this step can pass\n\n",
    );
    out.push_str(&format!(
        "The file you just wrote for **{file_label}** contains {} formal `[src: …]` citation(s) \
that do NOT resolve against the real codebase. A citation that points at a non-existent \
path / out-of-bounds line / outside the project is treated as **fabricated** and blocks the audit.\n\n",
        verdict.count()
    ));
    out.push_str("Fabricated citations:\n");
    for s in &verdict.fabricated {
        out.push_str(&format!("- `[src: {}]` → {}\n", s.raw.trim(), s.detail.trim()));
    }
    out.push_str(
        "\nFor EACH one: either correct it to a real `path:line` you have actually read, OR \
remove the unverifiable claim entirely (do not weaken it into prose — drop it). \
Do NOT invent a path just to pass the check. Re-write the file, then finish.\n",
    );
    out
}

/// Idempotently stamp `audit="<today>"` on every `curated="ai"` section opener
/// in `content`. Returns `Some(new_content)` when something changed, `None`
/// when every `curated="ai"` marker already carries today's date (no write
/// needed). The audit just (re)generated this file, so today's date honestly
/// reflects "verified conformant today".
pub fn stamp_curated_audit_dates(content: &str, today: &str) -> Option<String> {
    let today_attr = format!("audit=\"{today}\"");
    let mut changed = false;
    let mut lines: Vec<String> = content.lines().map(String::from).collect();

    for line in &mut lines {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("<!-- kronn:section") || !line.contains("curated=\"ai\"") {
            continue;
        }
        if line.contains(&today_attr) {
            continue; // already stamped today
        }
        if let Some(start) = line.find("audit=\"") {
            // Replace the existing (stale) date in place.
            let date_start = start + "audit=\"".len();
            if let Some(rel_end) = line[date_start..].find('"') {
                let date_end = date_start + rel_end;
                line.replace_range(date_start..date_end, today);
                changed = true;
            }
        } else if let Some(close) = line.rfind(" -->") {
            // No audit attr yet — insert one just before the closing marker.
            line.insert_str(close, &format!(" {today_attr}"));
            changed = true;
        }
    }

    if !changed {
        return None;
    }
    let mut out = lines.join("\n");
    if content.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn clean_file_yields_no_fabricated() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("real.rs"), "line1\nline2\nline3\n").unwrap();
        let content = "Stack uses real.rs [src: file: real.rs:2].";
        let verdict = lint_step_file(content, &[dir.path()]);
        assert!(verdict.is_clean(), "verified citation must not be fabricated");
    }

    #[test]
    fn nonexistent_path_is_fabricated() {
        let dir = tempdir().unwrap();
        let content = "It lives in [src: file: does/not/exist.rs:10].";
        let verdict = lint_step_file(content, &[dir.path()]);
        assert_eq!(verdict.count(), 1, "missing file → one fabricated citation");
        assert!(!verdict.is_clean());
    }

    #[test]
    fn out_of_bounds_line_is_fabricated() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("short.rs"), "only one line\n").unwrap();
        let content = "See [src: file: short.rs:999].";
        let verdict = lint_step_file(content, &[dir.path()]);
        assert_eq!(verdict.count(), 1, "out-of-bounds line → fabricated");
    }

    #[test]
    fn decide_passes_on_clean_verdict() {
        let clean = CitationVerdict::default();
        assert_eq!(decide(&clean, 1, MAX_ATTEMPTS), GateDecision::Pass);
        // Clean always passes, even on the last attempt.
        assert_eq!(decide(&clean, MAX_ATTEMPTS, MAX_ATTEMPTS), GateDecision::Pass);
    }

    #[test]
    fn decide_retries_while_budget_remains() {
        let dir = tempdir().unwrap();
        let dirty = lint_step_file("X [src: file: nope.rs:1].", &[dir.path()]);
        assert!(!dirty.is_clean());
        assert_eq!(decide(&dirty, 1, 3), GateDecision::Retry);
        assert_eq!(decide(&dirty, 2, 3), GateDecision::Retry);
    }

    #[test]
    fn decide_fails_when_budget_exhausted() {
        let dir = tempdir().unwrap();
        let dirty = lint_step_file("X [src: file: nope.rs:1].", &[dir.path()]);
        assert_eq!(decide(&dirty, 3, 3), GateDecision::Fail);
        // A single-attempt budget (warn/off would never call this) fails immediately.
        assert_eq!(decide(&dirty, 1, 1), GateDecision::Fail);
    }

    #[test]
    fn corrective_feedback_names_each_broken_citation() {
        let dir = tempdir().unwrap();
        let content =
            "A [src: file: ghost.rs:1] and B [src: file: phantom.rs:2] are made up.";
        let verdict = lint_step_file(content, &[dir.path()]);
        let fb = corrective_feedback("docs/AGENTS.md", &verdict);
        assert!(fb.contains("docs/AGENTS.md"));
        assert!(fb.contains("ghost.rs:1"));
        assert!(fb.contains("phantom.rs:2"));
        assert!(fb.contains("enforce mode"));
        // Must steer the agent away from inventing a path.
        assert!(fb.to_lowercase().contains("do not invent"));
    }

    #[test]
    fn stamp_inserts_missing_audit_attr() {
        let input = "<!-- kronn:section name=\"stack\" curated=\"ai\" -->\nBODY\n<!-- kronn:section:end -->\n";
        let out = stamp_curated_audit_dates(input, "2026-06-14").expect("should change");
        assert!(out.contains("audit=\"2026-06-14\""));
        assert!(out.contains("curated=\"ai\""));
        // closing marker untouched
        assert!(out.contains("<!-- kronn:section:end -->"));
    }

    #[test]
    fn stamp_refreshes_stale_audit_date() {
        let input = "<!-- kronn:section name=\"stack\" curated=\"ai\" audit=\"2026-01-01\" -->\nB\n";
        let out = stamp_curated_audit_dates(input, "2026-06-14").expect("should change");
        assert!(out.contains("audit=\"2026-06-14\""));
        assert!(!out.contains("2026-01-01"), "stale date must be replaced");
    }

    #[test]
    fn stamp_is_noop_when_already_today() {
        let input = "<!-- kronn:section name=\"stack\" curated=\"ai\" audit=\"2026-06-14\" -->\nB\n";
        assert_eq!(stamp_curated_audit_dates(input, "2026-06-14"), None);
    }

    #[test]
    fn stamp_ignores_human_sections() {
        let input = "<!-- kronn:section name=\"notes\" curated=\"human\" -->\nfree form\n";
        assert_eq!(
            stamp_curated_audit_dates(input, "2026-06-14"),
            None,
            "human-curated sections are never stamped"
        );
    }

    #[test]
    fn stamp_preserves_trailing_newline() {
        let with_nl = "<!-- kronn:section name=\"s\" curated=\"ai\" -->\nB\n";
        assert!(stamp_curated_audit_dates(with_nl, "2026-06-14").unwrap().ends_with('\n'));
        let no_nl = "<!-- kronn:section name=\"s\" curated=\"ai\" -->";
        assert!(!stamp_curated_audit_dates(no_nl, "2026-06-14").unwrap().ends_with('\n'));
    }
}
