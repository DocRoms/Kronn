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
        out.push_str(&format!(
            "- `[src: {}]` → {}\n",
            s.raw.trim(),
            s.detail.trim()
        ));
    }
    out.push_str(
        "\nFor EACH one: either correct it to a real `path:line` you have actually read, OR \
remove the unverifiable claim entirely (do not weaken it into prose — drop it). \
Do NOT invent a path just to pass the check. Re-write the file, then finish.\n",
    );
    out
}

/// Idempotently stamp `audit="<today>"` on every audit-owned section opener.
/// Template v2 emits `owner="audit"`; legacy `curated="ai"` markers remain
/// accepted. A human-owned marker always wins if both attributes are present.
pub fn stamp_curated_audit_dates(content: &str, today: &str) -> Option<String> {
    let today_attr = format!("audit=\"{today}\"");
    let mut changed = false;
    let mut lines: Vec<String> = content.lines().map(String::from).collect();

    for line in &mut lines {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("<!-- kronn:section")
            || is_human_owned_marker(line)
            || !(line.contains("owner=\"audit\"") || line.contains("curated=\"ai\""))
        {
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

/// One human-owned section whose content an audit step changed (or deleted)
/// between the pre- and post-agent snapshot of the same file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanSectionDiff {
    pub name: String,
    pub pre_block: String,
    /// `None` when the agent deleted the section entirely (restored by
    /// appending it back).
    pub post_block: Option<String>,
}

struct SectionSpan {
    name: String,
    is_human: bool,
    start: usize,
    end: usize,
}

/// Scan `lines` for `<!-- kronn:section name="X" ... -->` … `<!-- kronn:section:end -->`
/// pairs. An opener with no matching closer is skipped — there is nothing
/// well-formed to protect there.
fn parse_named_sections(lines: &[&str]) -> Vec<SectionSpan> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("<!-- kronn:section")
            && !trimmed.starts_with("<!-- kronn:section:end")
        {
            if let Some(name) = extract_attr(lines[i], "name") {
                let is_human = is_human_owned_marker(lines[i]);
                if let Some(end) = ((i + 1)..lines.len())
                    .find(|&j| lines[j].trim_start().starts_with("<!-- kronn:section:end"))
                {
                    spans.push(SectionSpan {
                        name,
                        is_human,
                        start: i,
                        end,
                    });
                    i = end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    spans
}

fn extract_attr(line: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')?;
    Some(line[start..start + end].to_string())
}

fn is_human_owned_marker(line: &str) -> bool {
    line.contains("owner=\"human\"") || line.contains("curated=\"human\"")
}

pub fn contains_human_owned_section(content: &str) -> bool {
    let visible = crate::core::anti_halluc::strip_fenced_code(content);
    let lines: Vec<&str> = visible.split('\n').collect();
    parse_named_sections(&lines)
        .iter()
        .any(|span| span.is_human)
}

/// KT-843 mechanical backstop: restore every human-owned section found in
/// `pre` to its exact original text wherever `post` changed or removed it,
/// and return the audit-proposed diff. Template v2 uses `owner="human"`;
/// legacy `curated="human"` sections receive the same protection.
///
/// Returns `None` when every human-owned section in `pre` survived
/// byte-identical in `post`, including the common case of no human
/// section at all. Runs independently of the anti-hallu citation gate: this
/// is an ownership guarantee, not a provenance one.
pub fn enforce_human_owned_sections(
    pre: &str,
    post: &str,
) -> Option<(String, Vec<HumanSectionDiff>)> {
    let pre_lines: Vec<&str> = pre.split('\n').collect();
    let post_lines: Vec<&str> = post.split('\n').collect();
    let pre_spans = parse_named_sections(&pre_lines);
    let post_spans = parse_named_sections(&post_lines);

    let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut diffs: Vec<HumanSectionDiff> = Vec::new();
    let mut appended: Vec<String> = Vec::new();

    for pre_span in pre_spans.iter().filter(|s| s.is_human) {
        let pre_block_lines: Vec<String> = pre_lines[pre_span.start..=pre_span.end]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let pre_block = pre_block_lines.join("\n");
        match post_spans.iter().find(|s| s.name == pre_span.name) {
            Some(post_span) => {
                let post_block = post_lines[post_span.start..=post_span.end].join("\n");
                if post_block != pre_block {
                    replacements.push((post_span.start, post_span.end, pre_block_lines));
                    diffs.push(HumanSectionDiff {
                        name: pre_span.name.clone(),
                        pre_block,
                        post_block: Some(post_block),
                    });
                }
            }
            None => {
                appended.push(pre_block.clone());
                diffs.push(HumanSectionDiff {
                    name: pre_span.name.clone(),
                    pre_block,
                    post_block: None,
                });
            }
        }
    }

    if diffs.is_empty() {
        return None;
    }

    // Replace from the bottom up so an earlier range's indices stay valid
    // even when a restored block has a different line count than the text
    // it replaces.
    replacements.sort_by_key(|replacement| std::cmp::Reverse(replacement.0));
    let mut out_lines: Vec<String> = post_lines.iter().map(|s| s.to_string()).collect();
    for (start, end, lines) in replacements {
        out_lines.splice(start..=end, lines);
    }
    for block in appended {
        if out_lines.last().is_some_and(|l| !l.is_empty()) {
            out_lines.push(String::new());
        }
        out_lines.push(String::new());
        out_lines.extend(block.split('\n').map(String::from));
    }

    Some((out_lines.join("\n"), diffs))
}

/// Render the audit-proposed changes for a human to review by hand.
pub fn format_human_section_diff_report(
    file_label: &str,
    diffs: &[HumanSectionDiff],
    today: &str,
) -> String {
    let mut out = format!(
        "# Proposed changes to human-owned sections — {today}\n\n\
The audit tried to update the following `owner=\"human\"` section(s) in \
`{file_label}` but left them untouched, as the convention requires. Review \
each diff below and apply it by hand if you agree with it.\n"
    );
    for d in diffs {
        out.push_str(&format!(
            "\n## Section `{}`\n\n### Kept (current)\n\n```\n{}\n```\n\n### Audit-proposed\n\n```\n{}\n```\n",
            d.name,
            d.pre_block,
            d.post_block
                .as_deref()
                .unwrap_or("(the audit removed this section — it was restored)"),
        ));
    }
    out
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
        assert!(
            verdict.is_clean(),
            "verified citation must not be fabricated"
        );
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
        assert_eq!(
            decide(&clean, MAX_ATTEMPTS, MAX_ATTEMPTS),
            GateDecision::Pass
        );
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
        let content = "A [src: file: ghost.rs:1] and B [src: file: phantom.rs:2] are made up.";
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
        let input = "<!-- kronn:section name=\"stack\" curated=\"ai\" owner=\"audit\" -->\nBODY\n<!-- kronn:section:end -->\n";
        let out = stamp_curated_audit_dates(input, "2026-06-14").expect("should change");
        assert!(out.contains("audit=\"2026-06-14\""));
        assert!(out.contains("curated=\"ai\""));
        // closing marker untouched
        assert!(out.contains("<!-- kronn:section:end -->"));
    }

    #[test]
    fn stamp_accepts_owner_audit_without_legacy_curated_attribute() {
        let input = "<!-- kronn:section name=\"stack\" owner=\"audit\" -->\nBODY\n<!-- kronn:section:end -->\n";
        let out = stamp_curated_audit_dates(input, "2026-06-14").expect("should change");
        assert!(out.contains("owner=\"audit\" audit=\"2026-06-14\""));
    }

    #[test]
    fn stamp_refreshes_stale_audit_date() {
        let input =
            "<!-- kronn:section name=\"stack\" curated=\"ai\" audit=\"2026-01-01\" -->\nB\n";
        let out = stamp_curated_audit_dates(input, "2026-06-14").expect("should change");
        assert!(out.contains("audit=\"2026-06-14\""));
        assert!(!out.contains("2026-01-01"), "stale date must be replaced");
    }

    #[test]
    fn stamp_is_noop_when_already_today() {
        let input =
            "<!-- kronn:section name=\"stack\" curated=\"ai\" audit=\"2026-06-14\" -->\nB\n";
        assert_eq!(stamp_curated_audit_dates(input, "2026-06-14"), None);
    }

    #[test]
    fn stamp_ignores_human_sections() {
        let input =
            "<!-- kronn:section name=\"notes\" curated=\"ai\" owner=\"human\" -->\nfree form\n";
        assert_eq!(
            stamp_curated_audit_dates(input, "2026-06-14"),
            None,
            "human-curated sections are never stamped"
        );
    }

    // ─── KT-843: `owner="human"` sections survive full/partial audit ───────

    #[test]
    fn human_section_untouched_is_a_noop() {
        let content = "# Doc\n\n\
            <!-- kronn:section name=\"team\" owner=\"human\" -->\n\
            Free-form human notes.\n\
            <!-- kronn:section:end -->\n\
            \n## Rest\nSame either way.\n";
        assert_eq!(
            enforce_human_owned_sections(content, content),
            None,
            "byte-identical human section must be a no-op"
        );
    }

    #[test]
    fn human_section_edited_by_the_audit_is_restored_and_diffed() {
        let pre = "# Doc\n\
            <!-- kronn:section name=\"team\" curated=\"human\" owner=\"human\" -->\n\
            Original human note.\n\
            <!-- kronn:section:end -->\n\
            <!-- kronn:section name=\"stack\" owner=\"audit\" -->\n\
            Rust.\n\
            <!-- kronn:section:end -->\n";
        // The agent rewrote BOTH the human section (must be restored) and
        // the ai section (must survive untouched — not this function's job).
        let post = "# Doc\n\
            <!-- kronn:section name=\"team\" owner=\"human\" -->\n\
            Agent-rewritten note — NOT what the human wrote.\n\
            <!-- kronn:section:end -->\n\
            <!-- kronn:section name=\"stack\" owner=\"audit\" -->\n\
            Rust, updated.\n\
            <!-- kronn:section:end -->\n";

        let (restored, diffs) = enforce_human_owned_sections(pre, post)
            .expect("an edited human section must be reported");

        assert_eq!(
            diffs.len(),
            1,
            "only the human section is a diff: {diffs:?}"
        );
        assert_eq!(diffs[0].name, "team");
        assert!(diffs[0].pre_block.contains("Original human note."));
        assert!(diffs[0]
            .post_block
            .as_deref()
            .unwrap()
            .contains("Agent-rewritten note"));

        // The human section is back to its original text …
        assert!(restored.contains("Original human note."));
        assert!(!restored.contains("Agent-rewritten note"));
        // … while the ai section's real update survives (never this
        // function's concern).
        assert!(restored.contains("Rust, updated."));
    }

    #[test]
    fn human_section_removed_by_the_audit_is_restored_at_the_end() {
        let pre = "# Doc\n\
            <!-- kronn:section name=\"team\" curated=\"human\" owner=\"human\" -->\n\
            Do not lose this.\n\
            <!-- kronn:section:end -->\n";
        // The agent deleted the section entirely.
        let post = "# Doc\nNothing left.\n";

        let (restored, diffs) = enforce_human_owned_sections(pre, post)
            .expect("a deleted human section must be flagged");

        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].name, "team");
        assert!(diffs[0].post_block.is_none(), "removal is reported as None");
        assert!(restored.contains("Do not lose this."));
        assert!(
            restored.contains("Nothing left."),
            "the rest of post survives"
        );
    }

    #[test]
    fn no_human_sections_at_all_is_a_noop() {
        let pre = "# Doc\n<!-- kronn:section name=\"s\" curated=\"ai\" -->\nA\n<!-- kronn:section:end -->\n";
        let post = "# Doc\n<!-- kronn:section name=\"s\" curated=\"ai\" -->\nB (audit updated)\n<!-- kronn:section:end -->\n";
        assert_eq!(
            enforce_human_owned_sections(pre, post),
            None,
            "no owner=\"human\" section exists — nothing to protect"
        );
    }

    #[test]
    fn human_marker_inside_a_fenced_report_excerpt_is_not_a_live_section() {
        let report = "# Report\n\n```\n<!-- kronn:section name=\"quoted\" owner=\"human\" -->\nQuoted.\n<!-- kronn:section:end -->\n```\n";
        assert!(!contains_human_owned_section(report));
    }

    #[test]
    fn legacy_curated_human_section_remains_protected() {
        let pre = "<!-- kronn:section name=\"legacy\" curated=\"human\" -->\nOriginal.\n<!-- kronn:section:end -->\n";
        let post = "<!-- kronn:section name=\"legacy\" curated=\"human\" -->\nChanged.\n<!-- kronn:section:end -->\n";
        let (restored, diffs) = enforce_human_owned_sections(pre, post)
            .expect("legacy human ownership must remain supported");
        assert_eq!(diffs.len(), 1);
        assert!(restored.contains("Original."));
        assert!(!restored.contains("Changed."));
    }

    #[test]
    fn diff_report_names_the_file_and_every_section() {
        let diffs = vec![
            HumanSectionDiff {
                name: "team".to_string(),
                pre_block: "kept text".to_string(),
                post_block: Some("agent text".to_string()),
            },
            HumanSectionDiff {
                name: "roadmap".to_string(),
                pre_block: "kept roadmap".to_string(),
                post_block: None,
            },
        ];
        let report = format_human_section_diff_report("docs/AGENTS.md", &diffs, "2026-09-27");
        assert!(report.contains("docs/AGENTS.md"));
        assert!(report.contains("2026-09-27"));
        assert!(
            report.contains("team")
                && report.contains("kept text")
                && report.contains("agent text")
        );
        assert!(report.contains("roadmap") && report.contains("kept roadmap"));
        assert!(
            report.contains("restored"),
            "the removed section notes it was restored"
        );
    }

    #[test]
    fn stamp_preserves_trailing_newline() {
        let with_nl = "<!-- kronn:section name=\"s\" curated=\"ai\" -->\nB\n";
        assert!(stamp_curated_audit_dates(with_nl, "2026-06-14")
            .unwrap()
            .ends_with('\n'));
        let no_nl = "<!-- kronn:section name=\"s\" curated=\"ai\" -->";
        assert!(!stamp_curated_audit_dates(no_nl, "2026-06-14")
            .unwrap()
            .ends_with('\n'));
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
        assert!(
            run.feedbacks.is_empty(),
            "no corrective feedback on a clean pass"
        );
        let stamped = run
            .stamped
            .expect("a Pass on an ai-curated section must stamp");
        assert!(
            stamped.contains("audit=\"2026-06-17\""),
            "stamp applied: {stamped}"
        );
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
        assert!(
            run.stamped.is_some(),
            "the winning attempt stamps the dates"
        );
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
        assert_eq!(
            run.attempts_used, MAX_ATTEMPTS,
            "uses the full budget before failing"
        );
        assert_eq!(
            run.feedbacks.len(),
            MAX_ATTEMPTS - 1,
            "a corrective round between each attempt, none after the last",
        );
        assert!(
            run.stamped.is_none(),
            "a failed step must NOT stamp the doc as verified"
        );
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
                verdict.count(),
                max_attempts
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

        let outcome = evaluate_enforce_gate(true, true, "docs/architecture.md", tmp.path(), 0, 3);
        assert!(
            matches!(outcome, EnforceGateOutcome::Pass { .. }),
            "lint-green must Pass"
        );

        let post = target_snapshot(tmp.path(), "docs/architecture.md").unwrap();
        assert_eq!(pre, post, "a passing gate must not fabricate a rewrite");
        let on_disk = std::fs::read_to_string(tmp.path().join("docs/architecture.md")).unwrap();
        assert_eq!(on_disk, stale, "byte-intact: the stale audit date survives");
    }
}
