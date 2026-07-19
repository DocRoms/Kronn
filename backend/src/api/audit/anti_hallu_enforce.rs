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

    // ─── Multi-attempt gate SEQUENCE (the full.rs loop contract) ────────────
    //
    // The unit tests above pin each helper in ISOLATION. The streaming
    // generator in `super::full` composes them in a specific order across a
    // bounded retry loop (full.rs:623-925): each attempt re-lints the written
    // file → `decide()` → on Retry it re-injects `corrective_feedback` (which
    // must name the still-broken refs) and re-runs → on Pass it `stamp`s the
    // dates → on Fail it ends the step. None of that SEQUENCING is exercised
    // by the per-helper tests, so a regression in the loop wiring (an
    // off-by-one in the attempt budget, feedback that drops the broken refs,
    // a missing stamp on the winning attempt) would slip through.
    //
    // We can't call full.rs's loop directly — it owns a live agent subprocess
    // with no injection seam (the agent CLI is spawned in-place). So this
    // harness reproduces the loop's CONTROL FLOW faithfully and feeds it a
    // scripted sequence of "what the agent wrote this attempt", driving the
    // REAL gate functions + REAL temp-file linting. It pins the contract the
    // generator must honour; if full.rs's branching ever diverges from this
    // shape, that's the signal to update both together.

    /// Outcome of driving the enforce gate over a scripted attempt sequence.
    struct GateRun {
        decision: GateDecision,
        attempts_used: usize,
        /// `Some(stamped)` iff the winning (Pass) attempt produced a stamp.
        stamped: Option<String>,
        /// One corrective-feedback string per Retry, in order.
        feedbacks: Vec<String>,
    }

    /// Faithful re-creation of full.rs's per-step enforce loop. `scripted` is
    /// the content the (fake) agent writes on each attempt — attempt N reads
    /// `scripted[N-1]`. Returns the terminal decision + side-effects.
    fn drive_enforce_gate(
        scripted: &[&str],
        roots: &[&Path],
        today: &str,
        max_attempts: usize,
    ) -> GateRun {
        let mut feedbacks = Vec::new();
        let mut attempt = 0usize;
        loop {
            attempt += 1;
            // The agent (here: the script) writes the file for this attempt.
            // A real run re-injects the prior feedback into the prompt; we
            // model that by the script simply providing a fixed/fixed-up file.
            let written = scripted
                .get(attempt - 1)
                .copied()
                // If the script runs dry, reuse the last content (an agent that
                // can't fix it keeps emitting the same broken file).
                .or_else(|| scripted.last().copied())
                .unwrap_or("");

            let verdict = lint_step_file(written, roots);
            match decide(&verdict, attempt, max_attempts) {
                GateDecision::Pass => {
                    return GateRun {
                        decision: GateDecision::Pass,
                        attempts_used: attempt,
                        stamped: stamp_curated_audit_dates(written, today),
                        feedbacks,
                    };
                }
                GateDecision::Retry => {
                    feedbacks.push(corrective_feedback("docs/AGENTS.md", &verdict));
                    continue;
                }
                GateDecision::Fail => {
                    return GateRun {
                        decision: GateDecision::Fail,
                        attempts_used: attempt,
                        stamped: None,
                        feedbacks,
                    };
                }
            }
        }
    }

    #[test]
    fn gate_passes_on_first_clean_attempt_and_stamps() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("real.rs"), "a\nb\nc\n").unwrap();
        let clean = "<!-- kronn:section name=\"stack\" curated=\"ai\" -->\n\
                     Uses real.rs [src: file: real.rs:2].\n\
                     <!-- kronn:section:end -->\n";

        let run = drive_enforce_gate(&[clean], &[dir.path()], "2026-06-17", MAX_ATTEMPTS);

        assert_eq!(run.decision, GateDecision::Pass);
        assert_eq!(run.attempts_used, 1, "a clean first pass must not retry");
        assert!(run.feedbacks.is_empty(), "no corrective feedback on a clean pass");
        let stamped = run.stamped.expect("a Pass on an ai-curated section must stamp");
        assert!(stamped.contains("audit=\"2026-06-17\""), "stamp applied: {stamped}");
    }

    #[test]
    fn gate_retries_then_passes_when_the_agent_fixes_the_citation() {
        // Attempt 1: cites a ghost file → Retry (feedback names the ghost).
        // Attempt 2: the agent corrects it to a real path → Pass + stamp.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("real.rs"), "x\ny\n").unwrap();
        let dirty = "<!-- kronn:section name=\"s\" curated=\"ai\" -->\n\
                     See [src: file: ghost.rs:1].\n<!-- kronn:section:end -->\n";
        let fixed = "<!-- kronn:section name=\"s\" curated=\"ai\" -->\n\
                     See [src: file: real.rs:1].\n<!-- kronn:section:end -->\n";

        let run = drive_enforce_gate(&[dirty, fixed], &[dir.path()], "2026-06-17", MAX_ATTEMPTS);

        assert_eq!(run.decision, GateDecision::Pass);
        assert_eq!(run.attempts_used, 2, "one retry, then the fix passes");
        assert_eq!(run.feedbacks.len(), 1, "exactly one corrective round");
        assert!(
            run.feedbacks[0].contains("ghost.rs:1"),
            "the retry feedback must name the broken ref so the agent can fix it: {}",
            run.feedbacks[0],
        );
        assert!(run.stamped.is_some(), "the winning attempt stamps the dates");
    }

    #[test]
    fn gate_fails_after_exhausting_attempts_without_stamping() {
        // The agent never fixes the fabricated citation. After MAX_ATTEMPTS the
        // step must Fail (→ audit ends Interrupted) and NEVER stamp the doc as
        // verified — the whole point of enforce is to not commit hallucinations.
        let dir = tempdir().unwrap();
        let forever_broken = "<!-- kronn:section name=\"s\" curated=\"ai\" -->\n\
                              [src: file: nope.rs:42].\n<!-- kronn:section:end -->\n";

        let run = drive_enforce_gate(&[forever_broken], &[dir.path()], "2026-06-17", MAX_ATTEMPTS);

        assert_eq!(run.decision, GateDecision::Fail);
        assert_eq!(run.attempts_used, MAX_ATTEMPTS, "uses the full budget before failing");
        assert_eq!(
            run.feedbacks.len(),
            MAX_ATTEMPTS - 1,
            "a corrective round between each attempt, none after the last",
        );
        assert!(run.stamped.is_none(), "a failed step must NOT stamp the doc as verified");
    }

    #[test]
    fn gate_passes_immediately_when_file_has_no_formal_citations() {
        // Prose with no `[src: …]` markers can't be fabricated — the gate must
        // not block a citation-free step (e.g. a REVIEW summary). Pass on
        // attempt 1, no retries, no feedback.
        let dir = tempdir().unwrap();
        let prose = "<!-- kronn:section name=\"s\" curated=\"ai\" -->\n\
                     Plain prose, no formal provenance.\n<!-- kronn:section:end -->\n";

        let run = drive_enforce_gate(&[prose], &[dir.path()], "2026-06-17", MAX_ATTEMPTS);

        assert_eq!(run.decision, GateDecision::Pass);
        assert_eq!(run.attempts_used, 1);
        assert!(run.feedbacks.is_empty());
    }
}

/// The FULL enforce-gate decision, shared verbatim by the Full and partial
/// pipelines (Codex lot-3 #8): read the written target, lint its `[src:]`
/// citations against the real tree, and decide. The caller only maps each
/// outcome to its own SSE events — the DECISION can no longer drift between
/// the two loops.
pub enum EnforceGateOutcome {
    /// Gate not applicable (not enforce mode / step already failing /
    /// synthetic REVIEW target).
    NotApplicable,
    /// Target unreadable — a FAIL, never a bypass (Codex lot-3 #3).
    Unreadable(String),
    /// Fabricated citations, budget left: re-run with this feedback.
    Retry { feedback: String, fabricated: usize },
    /// Fabricated citations, retries exhausted: fail the step.
    Fail { reason: String },
    /// Clean citations — the written content, for any post-proof stamping.
    Pass { written: String },
}

pub fn evaluate_enforce_gate(
    enforce_mode: bool,
    step_success_so_far: bool,
    target_file: &str,
    project_path: &Path,
    attempt: usize,
    max_attempts: usize,
) -> EnforceGateOutcome {
    if !enforce_mode || !step_success_so_far || target_file == "REVIEW" {
        return EnforceGateOutcome::NotApplicable;
    }
    let target_path = project_path.join(target_file);
    let written = match std::fs::read_to_string(&target_path) {
        Ok(w) => w,
        Err(e) => {
            return EnforceGateOutcome::Unreadable(format!(
                "enforce mode: target unreadable for citation lint: {e}"
            ));
        }
    };
    let verdict = lint_step_file(&written, &[project_path]);
    match decide(&verdict, attempt, max_attempts) {
        GateDecision::Retry => EnforceGateOutcome::Retry {
            feedback: corrective_feedback(target_file, &verdict),
            fabricated: verdict.count(),
        },
        GateDecision::Fail => EnforceGateOutcome::Fail {
            reason: format!(
                "{} fabricated `[src:]` citation(s) still present after {} attempts (enforce mode)",
                verdict.count(), max_attempts
            ),
        },
        GateDecision::Pass => EnforceGateOutcome::Pass { written },
    }
}

#[cfg(test)]
mod enforce_gate_tests {
    use super::*;

    /// Codex lot-3 #1 — the no-op forge seam. An old curated file the agent
    /// did NOT touch passes the lint (Pass), and the gate must not mutate it:
    /// the old stamp path rewrote `audit="<date>"` BEFORE the rewrite proof,
    /// so the pipeline's own write made a no-op step read as `succeeded`.
    #[test]
    fn passing_gate_leaves_a_stale_curated_target_byte_intact() {
        use crate::api::audit::validation::{target_snapshot, TargetSnapshot};

        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let stale = "<!-- kronn:section id=\"arch\" curated=\"ai\" audit=\"2020-01-01\" -->\n\
                     Old but honest content, no citations.\n";
        std::fs::write(tmp.path().join("docs/architecture.md"), stale).unwrap();

        // The fixture IS stampable — the seam is real, not vacuous.
        assert!(stamp_curated_audit_dates(stale, "2026-07-20").is_some());

        let pre = target_snapshot(tmp.path(), "docs/architecture.md").unwrap();
        assert!(matches!(pre, TargetSnapshot::Present(_)));

        let outcome = evaluate_enforce_gate(
            true, true, "docs/architecture.md", tmp.path(), 0, 3,
        );
        assert!(matches!(outcome, EnforceGateOutcome::Pass { .. }), "lint-green must Pass");

        let post = target_snapshot(tmp.path(), "docs/architecture.md").unwrap();
        assert_eq!(pre, post, "a passing gate must not fabricate a rewrite");
        let on_disk = std::fs::read_to_string(tmp.path().join("docs/architecture.md")).unwrap();
        assert_eq!(on_disk, stale, "byte-intact: the stale audit date survives");
    }
}
