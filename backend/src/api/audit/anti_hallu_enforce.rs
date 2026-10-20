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

use crate::core::anti_halluc::{self, scan_fences, FenceScan, SourceCheck};
use crate::core::version_sources::{verify_dep_marker, DependencySnapshot};
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

/// [`lint_step_file`], with every `deps:` marker checked against the dependency
/// watch the audit injected (KT-1000): a row it does not hold is fabricated.
pub fn lint_step_file_with_deps(
    content: &str,
    roots: &[&Path],
    deps: Option<&DependencySnapshot>,
) -> CitationVerdict {
    let report = anti_halluc::analyze_roots(content, roots);
    let fabricated = report
        .sources
        .into_iter()
        .filter_map(|mut source| {
            if let Some((status, detail)) = verify_dep_marker(&source.raw, deps) {
                source.status = status;
                source.detail = detail;
            }
            source.status.is_fabricated().then_some(source)
        })
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
        "\nFor EACH one: either correct it to a real `path:line` you have actually read (a binary \
file: its bare path; a claim that a file merely exists or is tracked: `path:exists`), OR \
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

/// Fence characters that hide a `kronn:section` marker from the ownership
/// parser. A `~~~` block is code to a renderer exactly like a ``` one.
const OWNERSHIP_FENCE_CHARS: [char; 2] = ['`', '~'];

/// Which lines of `text` sit inside a fenced code block. A marker on such a
/// line is documentation (an example, a quoted report), never a live section
/// boundary: it neither opens nor closes a protected section.
///
/// An unclosed fence is read leniently (its lines stay visible, so a stray
/// fence in a human's own document does not strip protection) — except when
/// `strict_unclosed` is set, where it is CommonMark's block-to-end-of-file.
pub(crate) fn code_line_mask(text: &str, strict_unclosed: bool) -> FenceScan {
    scan_fences(text, &OWNERSHIP_FENCE_CHARS, strict_unclosed)
}

/// Scan `lines` for `<!-- kronn:section name="X" ... -->` … `<!-- kronn:section:end -->`
/// pairs. `in_code` flags the lines inside fenced code (index-aligned with
/// `lines`, shorter is fine): their markers are ignored on both ends. An
/// opener with no matching closer is skipped — there is nothing well-formed to
/// protect there.
fn parse_named_sections(lines: &[&str], in_code: &[bool]) -> Vec<SectionSpan> {
    let is_code = |idx: usize| in_code.get(idx).copied().unwrap_or(false);
    let mut spans = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if !is_code(i)
            && trimmed.starts_with("<!-- kronn:section")
            && !trimmed.starts_with("<!-- kronn:section:end")
        {
            if let Some(name) = extract_attr(lines[i], "name") {
                let is_human = is_human_owned_marker(lines[i]);
                if let Some(end) = ((i + 1)..lines.len()).find(|&j| {
                    !is_code(j) && lines[j].trim_start().starts_with("<!-- kronn:section:end")
                }) {
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

pub(super) fn extract_attr(line: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')?;
    Some(line[start..start + end].to_string())
}

pub(super) fn is_human_owned_marker(line: &str) -> bool {
    line.contains("owner=\"human\"") || line.contains("curated=\"human\"")
}

/// `content` with every human-owned section blanked (line count kept), and
/// each of those sections as `(name, text)`, markers included.
pub fn split_human_owned_sections(content: &str) -> (String, Vec<(String, String)>) {
    let mut lines: Vec<&str> = content.split('\n').collect();
    let scan = code_line_mask(content, false);
    let mut human = Vec::new();
    for span in parse_named_sections(&lines, &scan.in_fence) {
        if span.is_human {
            human.push((span.name, lines[span.start..=span.end].join("\n")));
            for line in &mut lines[span.start..=span.end] {
                *line = "";
            }
        }
    }
    (lines.join("\n"), human)
}

/// One live named section: its text (markers included) and 0-based line span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedSection {
    pub name: String,
    pub block: String,
    pub human: bool,
    pub start: usize,
    pub end: usize,
}

/// Every live named section of `content`; fenced examples are not sections.
pub fn named_sections(content: &str) -> Vec<NamedSection> {
    let lines: Vec<&str> = content.split('\n').collect();
    let scan = code_line_mask(content, false);
    parse_named_sections(&lines, &scan.in_fence)
        .into_iter()
        .map(|span| NamedSection {
            block: lines[span.start..=span.end].join("\n"),
            name: span.name,
            human: span.is_human,
            start: span.start,
            end: span.end,
        })
        .collect()
}

pub fn contains_human_owned_section(content: &str) -> bool {
    let lines: Vec<&str> = content.split('\n').collect();
    let scan = code_line_mask(content, false);
    parse_named_sections(&lines, &scan.in_fence)
        .iter()
        .any(|span| span.is_human)
}

/// Lines of `content` an inserted block must not land on: inside a fenced
/// code block, or inside a named section (markers included). Index-aligned
/// with `content.split('\n')`, so it also covers `content.lines()`.
pub(super) fn section_or_code_lines(content: &str) -> Vec<bool> {
    let lines: Vec<&str> = content.split('\n').collect();
    let mut occupied = code_line_mask(content, false).in_fence;
    occupied.resize(lines.len(), false);
    let spans = parse_named_sections(&lines, &occupied);
    for span in spans {
        occupied[span.start..=span.end].fill(true);
    }
    occupied
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
    let pre_scan = code_line_mask(pre, false);
    let mut post_scan = code_line_mask(post, false);
    // An audit that leaves a fence open where the document had none turns
    // everything after it into one code block: read `post` the way a renderer
    // does, or a human section behind that fence would pass as untouched.
    if post_scan.unclosed && !pre_scan.unclosed {
        post_scan = code_line_mask(post, true);
    }
    let pre_spans = parse_named_sections(&pre_lines, &pre_scan.in_fence);
    let post_spans = parse_named_sections(&post_lines, &post_scan.in_fence);

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

#[cfg(all(test, feature = "full-tests"))]
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

    /// A repo with a synthetic key, a symlink to it and a plain file.
    fn secret_repo() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".ssh")).unwrap();
        fs::write(
            dir.path().join(".ssh/review-key"),
            "-----BEGIN KEY-----\nsynthetic\n-----END KEY-----\n",
        )
        .unwrap();
        fs::write(dir.path().join("plain.txt"), "a\nb\nc\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(".ssh/review-key", dir.path().join("key-view.txt")).unwrap();
        dir
    }

    /// `.env -> settings-data`, `.ssh/settings-view -> ../settings-data` and
    /// `view.txt -> .env`: the written name is secret although the target is not.
    #[cfg(unix)]
    #[test]
    fn a_secret_written_name_remains_secret_after_resolving_to_an_ordinary_name() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("settings-data"), "a=1\nb=2\nc=3\n").unwrap();
        fs::create_dir_all(dir.path().join(".ssh")).unwrap();
        std::os::unix::fs::symlink("settings-data", dir.path().join(".env")).unwrap();
        std::os::unix::fs::symlink("../settings-data", dir.path().join(".ssh/settings-view"))
            .unwrap();
        std::os::unix::fs::symlink(".env", dir.path().join("view.txt")).unwrap();
        for path in [".env", ".ssh/settings-view", "view.txt"] {
            for cite in [
                format!("[src: file: {path}:2]"),
                format!("[src: file: {path}:1-3]"),
                format!("[src: file: {path}:1-3#PRIVATE]"),
                format!("[src: code-comment: {path}:2]"),
            ] {
                let verdict = lint_step_file(&format!("Claim {cite}."), &[dir.path()]);
                assert_eq!(
                    verdict.fabricated.len(),
                    1,
                    "{cite}: {:?}",
                    verdict.fabricated
                );
                let check = anti_halluc::verify_source_marker(
                    cite.trim_start_matches("[src: ").trim_end_matches(']'),
                    Some(dir.path()),
                );
                assert!(check.status.is_fabricated(), "{cite}: {check:?}");
            }
            let verdict =
                lint_step_file(&format!("Claim [src: file: {path}:exists]."), &[dir.path()]);
            assert!(verdict.is_clean(), "{path}: {:?}", verdict.fabricated);
        }
        let verdict = lint_step_file("Claim [src: file: settings-data:2].", &[dir.path()]);
        assert!(verdict.is_clean(), "{:?}", verdict.fabricated);
    }

    /// `.ssh -> vault`, `keys -> .ssh`: the canonical `vault/review-key` has lost the
    /// `.ssh` name the chain passed through.
    #[cfg(unix)]
    #[test]
    fn chain_directory_component_keeps_secret_names() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("vault")).unwrap();
        fs::write(dir.path().join("vault/review-key"), "k1\nk2\nk3\n").unwrap();
        std::os::unix::fs::symlink("vault", dir.path().join(".ssh")).unwrap();
        std::os::unix::fs::symlink(".ssh", dir.path().join("keys")).unwrap();
        for cite in [
            "[src: file: keys/review-key:2]",
            "[src: file: keys/review-key:1-3]",
            "[src: file: keys/review-key:1-3#PRIVATE]",
            "[src: code-comment: keys/review-key:2]",
        ] {
            let verdict = lint_step_file(&format!("Claim {cite}."), &[dir.path()]);
            assert_eq!(
                verdict.fabricated.len(),
                1,
                "{cite}: {:?}",
                verdict.fabricated
            );
            let check = anti_halluc::verify_source_marker(
                cite.trim_start_matches("[src: ").trim_end_matches(']'),
                Some(dir.path()),
            );
            assert!(check.status.is_fabricated(), "{cite}: {check:?}");
        }
        let verdict = lint_step_file("Claim [src: file: keys/review-key:exists].", &[dir.path()]);
        assert!(verdict.is_clean(), "{:?}", verdict.fabricated);
    }

    #[test]
    fn a_line_of_a_secret_file_is_fabricated_whatever_the_form() {
        let dir = secret_repo();
        let mut paths = vec![".ssh/review-key"];
        if cfg!(unix) {
            paths.push("key-view.txt");
        }
        for path in paths {
            for cite in [
                format!("[src: file: {path}:2]"),
                format!("[src: file: {path}:1-3]"),
                format!("[src: file: {path}:1-3#PRIVATE]"),
                format!("[src: code-comment: {path}:2]"),
            ] {
                let verdict = lint_step_file(&format!("Claim {cite}."), &[dir.path()]);
                assert_eq!(
                    verdict.fabricated.len(),
                    1,
                    "{cite}: {:?}",
                    verdict.fabricated
                );
                assert!(
                    verdict.fabricated[0].detail.contains("path:exists"),
                    "{cite}: {}",
                    verdict.fabricated[0].detail
                );
                let check = anti_halluc::verify_source_marker(
                    cite.trim_start_matches("[src: ").trim_end_matches(']'),
                    Some(dir.path()),
                );
                assert!(check.status.is_fabricated(), "{cite}: {check:?}");
            }
            // Presence is the form that stays.
            let verdict =
                lint_step_file(&format!("Claim [src: file: {path}:exists]."), &[dir.path()]);
            assert!(verdict.is_clean(), "{path}: {:?}", verdict.fabricated);
        }
        let verdict = lint_step_file("Claim [src: file: plain.txt:2].", &[dir.path()]);
        assert!(verdict.is_clean(), "{:?}", verdict.fabricated);
    }

    #[test]
    fn a_dash_range_passes_the_enforce_gate_like_a_hyphen_range() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("long.md"), "x\n".repeat(30)).unwrap();
        // 31 is the empty line after the final newline.
        for ok in [
            "17\u{2013}23",
            "17\u{2014}23",
            "17-23",
            "1\u{2013}30",
            "1\u{2013}31",
        ] {
            let verdict =
                lint_step_file(&format!("Claim [src: file: long.md:{ok}]."), &[dir.path()]);
            assert!(verdict.is_clean(), "{ok}: {:?}", verdict.fabricated);
        }
        for bad in ["17\u{2013}32", "32\u{2014}40", "23\u{2013}17"] {
            let verdict =
                lint_step_file(&format!("Claim [src: file: long.md:{bad}]."), &[dir.path()]);
            assert!(!verdict.is_clean(), "{bad} must still be refused");
        }
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
    fn absolute_citation_outside_the_project_is_fabricated() {
        let dir = tempdir().unwrap();
        let verdict = lint_step_file("Secrets in [src: file: /etc/passwd:1].", &[dir.path()]);
        assert_eq!(verdict.count(), 1);
        assert_eq!(
            verdict.fabricated[0].status,
            anti_halluc::SourceStatus::OutsideProject
        );
    }

    #[test]
    fn quoted_grammar_example_does_not_block_the_gate() {
        // The AGENTS.md template quotes the grammar with placeholders.
        let dir = tempdir().unwrap();
        let content = "Cite `[src: file: <path>:<line>]` or `[src: file: ghost.rs:1]`.";
        assert!(lint_step_file(content, &[dir.path()]).is_clean());
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

    // ─── KT-933: a marker inside a code block is documentation ─────────────

    /// A human section that documents the convention: its example shows both
    /// a section opener and the closing marker inside a fence.
    const HUMAN_WITH_EXAMPLE: &str = "<!-- kronn:section name=\"howto\" owner=\"human\" -->\n\
        Wrap a section like this:\n\
        ```\n\
        <!-- kronn:section name=\"demo\" owner=\"human\" -->\n\
        <!-- kronn:section:end -->\n\
        ```\n\
        Rule that follows the example.\n\
        <!-- kronn:section:end -->";

    #[test]
    fn fenced_section_end_does_not_close_a_human_section_early() {
        let pre = format!("# Doc\n{HUMAN_WITH_EXAMPLE}\n## Rest\nfree\n");
        // The agent rewrites the rule AFTER the documented example. With the
        // fenced marker taken as the closer, protection stopped above it.
        let post = pre.replace("Rule that follows the example.", "Agent-made rule.");

        let (restored, diffs) = enforce_human_owned_sections(&pre, &post)
            .expect("a change below a documented section:end must be caught");
        assert_eq!(diffs.len(), 1, "{diffs:?}");
        assert_eq!(diffs[0].name, "howto");
        assert!(
            diffs[0]
                .pre_block
                .contains("Rule that follows the example."),
            "the protected block runs to the REAL closer"
        );
        assert_eq!(restored, pre, "restored byte-for-byte");
    }

    #[test]
    fn fenced_section_end_does_not_close_a_human_section_early_with_tildes() {
        let pre = HUMAN_WITH_EXAMPLE.replace("```", "~~~");
        let post = pre.replace("Rule that follows the example.", "Agent-made rule.");
        let (restored, diffs) = enforce_human_owned_sections(&pre, &post)
            .expect("a ~~~ fence hides markers just like a ``` one");
        assert_eq!(diffs.len(), 1);
        assert_eq!(restored, pre);
    }

    #[test]
    fn fenced_example_opener_does_not_open_a_protected_section() {
        // Only an EXAMPLE of a human section: nothing live to protect.
        let pre = "# Doc\n```\n<!-- kronn:section name=\"demo\" owner=\"human\" -->\nQuoted.\n<!-- kronn:section:end -->\n```\n";
        let post = "# Doc\nThe example is gone.\n";
        assert_eq!(enforce_human_owned_sections(pre, post), None);
        assert!(!contains_human_owned_section(pre));
    }

    #[test]
    fn fenced_example_opener_before_a_real_section_does_not_swallow_it() {
        let pre = "```\n<!-- kronn:section name=\"fake\" owner=\"human\" -->\n```\n\
            <!-- kronn:section name=\"real\" owner=\"human\" -->\nKeep.\n<!-- kronn:section:end -->\n";
        let post = pre.replace("Keep.", "Changed.");
        let (_, diffs) = enforce_human_owned_sections(pre, &post).expect("real is protected");
        assert_eq!(
            diffs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            vec!["real"],
            "the quoted opener must not become a section of its own"
        );
    }

    #[test]
    fn wrapping_a_human_section_in_a_fence_is_caught_and_the_live_copy_restored() {
        let section = "<!-- kronn:section name=\"team\" owner=\"human\" -->\nKeep me live.\n<!-- kronn:section:end -->";
        let pre = format!("# Doc\n{section}\n## Rest\n");
        for fence in ["```", "~~~", "````"] {
            // Same bytes, but the section is now an example to any renderer.
            let post = format!("# Doc\n{fence}\n{section}\n{fence}\n## Rest\n");
            let (restored, diffs) = enforce_human_owned_sections(&pre, &post)
                .unwrap_or_else(|| panic!("fence-wrapped section must not pass ({fence})"));
            assert_eq!(diffs.len(), 1, "{fence}");
            assert!(
                diffs[0].post_block.is_none(),
                "no live copy left in post ({fence})"
            );
            // The live copy is back, and the guard is now satisfied.
            assert!(contains_human_owned_section(&restored), "{fence}");
            assert_eq!(
                enforce_human_owned_sections(&pre, &restored),
                None,
                "a second pass over the restored text is a no-op ({fence})"
            );
        }
    }

    #[test]
    fn a_fenced_decoy_copy_cannot_stand_in_for_a_deleted_human_section() {
        let section = "<!-- kronn:section name=\"team\" owner=\"human\" -->\nKeep me live.\n<!-- kronn:section:end -->";
        let pre = format!("# Doc\n{section}\n");
        // The agent deletes the live section but leaves an identical quoted copy.
        let post = format!("# Doc\nSee the example:\n```\n{section}\n```\n");
        let (restored, diffs) = enforce_human_owned_sections(&pre, &post)
            .expect("the decoy must not satisfy the guard");
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].post_block.is_none());
        assert!(contains_human_owned_section(&restored));
    }

    #[test]
    fn a_fence_left_open_above_a_human_section_is_caught() {
        let section = "<!-- kronn:section name=\"team\" owner=\"human\" -->\nKeep me live.\n<!-- kronn:section:end -->";
        let pre = format!("# Doc\n{section}\n");
        // One stray ``` and no closer: a renderer treats the rest as code.
        let post = format!("# Doc\n```\n{section}\n");
        let (_, diffs) = enforce_human_owned_sections(&pre, &post)
            .expect("an unclosed fence swallowing the section must not pass");
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].post_block.is_none());
    }

    #[test]
    fn a_fence_already_open_in_the_original_does_not_make_every_audit_a_diff() {
        // The human's own doc has a stray unclosed fence above the section.
        // Lenient reading on both sides: protected, and untouched = no-op.
        let pre = "# Doc\n```\n<!-- kronn:section name=\"team\" owner=\"human\" -->\nKeep.\n<!-- kronn:section:end -->\n";
        assert!(contains_human_owned_section(pre));
        assert_eq!(enforce_human_owned_sections(pre, pre), None);
        let post = pre.replace("Keep.", "Changed.");
        assert!(enforce_human_owned_sections(pre, &post).is_some());
    }

    #[test]
    fn tilde_fenced_report_excerpt_is_not_a_live_section() {
        let report = "# Report\n\n~~~\n<!-- kronn:section name=\"quoted\" owner=\"human\" -->\nQuoted.\n<!-- kronn:section:end -->\n~~~\n";
        assert!(!contains_human_owned_section(report));
    }

    #[test]
    fn section_or_code_lines_covers_sections_and_fences_only() {
        let content = "# Doc\n```\nx\n```\n<!-- kronn:section name=\"s\" owner=\"audit\" -->\nA\n<!-- kronn:section:end -->\nplain\n";
        let occupied = section_or_code_lines(content);
        // # Doc | ``` x ``` | open A close | plain | (trailing empty element)
        assert_eq!(
            occupied,
            vec![false, true, true, true, true, true, true, false, false]
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
    fn a_mistyped_kind_over_a_missing_file_is_retried_then_failed() {
        // `fil:` / `path:` / `source:` are not known kinds: their path is judged
        // as a file, so the step is retried and finally failed, not waved through.
        for raw in [
            "fil: src/ghost.rs:1",
            "path: src/ghost.rs",
            "source: ghost.rs",
        ] {
            let dir = tempdir().unwrap();
            fs::write(
                dir.path().join("AGENTS.md"),
                format!("Claim [src: {raw}].\n"),
            )
            .unwrap();
            let gate = |attempt| {
                evaluate_enforce_gate(true, true, "AGENTS.md", dir.path(), attempt, 2, None)
            };
            assert!(
                matches!(gate(1), EnforceGateOutcome::Retry { fabricated: 1, .. }),
                "{raw}"
            );
            assert!(matches!(gate(2), EnforceGateOutcome::Fail { .. }), "{raw}");
        }
        // A listed non-file kind is not a path: the gate passes it.
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("AGENTS.md"),
            "Recent [src: command: git log --oneline].\n",
        )
        .unwrap();
        assert!(matches!(
            evaluate_enforce_gate(true, true, "AGENTS.md", dir.path(), 1, 2, None),
            EnforceGateOutcome::Pass { .. }
        ));
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
    deps: Option<&DependencySnapshot>,
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
    let verdict = lint_step_file_with_deps(&written, &[project_path], deps);
    match decide(&verdict, attempt, max_attempts) {
        GateDecision::Retry => EnforceGateOutcome::Retry {
            feedback: corrective_feedback(target_file, &verdict),
            fabricated: verdict.count(),
        },
        GateDecision::Fail => EnforceGateOutcome::Fail {
            reason: enforce_failure_reason(verdict.count(), max_attempts),
        },
        GateDecision::Pass => EnforceGateOutcome::Pass { written },
    }
}

/// The reason of a step that still holds fabricated citations once its
/// attempts are spent.
pub(crate) fn enforce_failure_reason(count: usize, max_attempts: usize) -> String {
    format!(
        "{} {}(s) still present after {} attempts (enforce mode)",
        count,
        super::step_guard::FABRICATED_FRAGMENT,
        max_attempts
    )
}

#[cfg(all(test, feature = "full-tests"))]
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

        let outcome =
            evaluate_enforce_gate(true, true, "docs/architecture.md", tmp.path(), 0, 3, None);
        assert!(
            matches!(outcome, EnforceGateOutcome::Pass { .. }),
            "lint-green must Pass"
        );

        let post = target_snapshot(tmp.path(), "docs/architecture.md").unwrap();
        assert_eq!(pre, post, "a passing gate must not fabricate a rewrite");
        let on_disk = std::fs::read_to_string(tmp.path().join("docs/architecture.md")).unwrap();
        assert_eq!(on_disk, stale, "byte-intact: the stale audit date survives");
    }

    fn watch() -> DependencySnapshot {
        DependencySnapshot {
            checked_on: "2026-10-04".into(),
            packages: vec![crate::core::version_sources::WatchedPackage {
                manager: "npm".into(),
                manifest: "package.json".into(),
                name: "react".into(),
                current: "18.2.0".into(),
                latest: "19.1.0".into(),
                major: true,
                declared_only: false,
                lockfile: None,
            }],
            unchecked: vec![],
            up_to_date: vec![],
        }
    }

    /// KT-1295 — the bench row: the npm manager is labelled `JS / TS` by the
    /// producer, and the citation it emits (a `/` in the label) passes the gate.
    #[test]
    fn deps_citations_with_the_real_js_ts_label_pass_the_gate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let roots = [tmp.path()];
        let mut snap = watch();
        snap.checked_on = "2026-10-16".into();
        snap.packages = ["argparse", "joi", "@types/node"]
            .iter()
            .map(|name| crate::core::version_sources::WatchedPackage {
                manager: "JS / TS".into(),
                manifest: "package.json".into(),
                name: (*name).into(),
                current: "1.0.0".into(),
                latest: "2.0.0".into(),
                major: true,
                declared_only: false,
                lockfile: None,
            })
            .collect();
        let cited: String = snap
            .packages
            .iter()
            .map(|p| format!("{} lags {}.\n", p.name, snap.citation(p)))
            .collect();
        assert!(cited.contains("[src: deps: JS / TS/argparse@2026-10-16]"));
        assert!(lint_step_file_with_deps(&cited, &roots, Some(&snap)).is_clean());

        let absent = "Vue lags [src: deps: JS / TS/vue@2026-10-16].";
        assert_eq!(
            lint_step_file_with_deps(absent, &roots, Some(&snap)).count(),
            1
        );
    }

    /// KT-1000 — a `deps:` row the injected watch holds passes; one it does
    /// not hold (or with another date, or with no watch) fails the gate.
    #[test]
    fn deps_citations_are_gated_against_the_injected_watch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let roots = [tmp.path()];
        let snap = watch();
        let held = "React lags a major [src: deps: npm/react@2026-10-04].";
        assert!(lint_step_file_with_deps(held, &roots, Some(&snap)).is_clean());

        for bad in [
            "Vue lags [src: deps: npm/vue@2026-10-04].",
            "React lags [src: deps: npm/react@2026-09-01].",
            "React lags [src: deps: npm/react].",
        ] {
            let verdict = lint_step_file_with_deps(bad, &roots, Some(&snap));
            assert_eq!(verdict.count(), 1, "{bad}");
        }
        assert_eq!(lint_step_file_with_deps(held, &roots, None).count(), 1);
        // Outside an audit the marker stays honestly unchecked.
        assert!(lint_step_file(held, &roots).is_clean());

        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(
            tmp.path().join("docs/inconsistencies-tech-debt.md"),
            "Vue [src: deps: npm/vue@2026-10-04].\n",
        )
        .unwrap();
        let outcome = evaluate_enforce_gate(
            true,
            true,
            "docs/inconsistencies-tech-debt.md",
            tmp.path(),
            1,
            3,
            Some(&snap),
        );
        match outcome {
            EnforceGateOutcome::Retry {
                feedback,
                fabricated,
            } => {
                assert_eq!(fabricated, 1);
                assert!(
                    feedback.contains("not in the dependency watch dated 2026-10-04"),
                    "{feedback}"
                );
            }
            _ => panic!("an absent deps row must be retried"),
        }
    }
}
