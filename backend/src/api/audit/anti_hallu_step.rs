//! 0.8.7 — Audit STEP 0 (anti-hallu section maintenance).
//!
//! Deterministic Rust function that ensures `docs/AGENTS.md` carries the
//! canonical `<!-- kronn:section name="anti-hallu" curated="ai" -->` block.
//! Called once at the start of every audit run, BEFORE the 10 numbered
//! `ANALYSIS_STEPS`, so the doctrine is in place for every subsequent
//! step + every agent reading the file later.
//!
//! ### Why not an LLM step ?
//!
//! The task is purely mechanical : insert or refresh a fixed text block
//! at a deterministic position. Asking an LLM to do this would burn
//! tokens, risk hallucination (the LLM might rewrite the canonical text
//! to "improve" it), and be slower. A Rust function gives us
//! byte-for-byte determinism + zero cost + no failure mode.
//!
//! ### Idempotence
//!
//! - If the file already contains a live `<!-- kronn:section name="anti-hallu"`
//!   opener (not one quoted inside a fenced code block), only the
//!   `audit="<date>"` attribute on the opening marker is refreshed. The body
//!   inside the markers stays untouched.
//! - If that opener carries `owner="human"` / `curated="human"`, the section
//!   belongs to the human (KT-843) and is never touched at all: this step runs
//!   BEFORE the human-owned snapshot, so there would be nothing left to
//!   restore it from.
//! - If the file does NOT contain the marker, the canonical block is
//!   inserted immediately after the first H1 line (`# AI agent context …`)
//!   and before the next `---` separator — never inside a fenced block or
//!   another section. Everything else is preserved.
//! - If the file has neither H1 nor `---`, the block is prepended.
//!
//! ### Why is this NOT in `core::anti_halluc` ?
//!
//! The doctrine *text* lives in `audit::mod::ANTI_HALLU_SECTION_BODY`
//! because the audit is the only writer ; the runtime `PREAMBLE` of
//! `core::anti_halluc` is now a short pointer toward this section, not a
//! duplicate of its content (see project memory
//! `project_anti_hallucination_program.md` § REDESIGN CANONIQUE).

use chrono::Utc;
use std::path::Path;

use super::anti_hallu_enforce::{
    code_line_mask, extract_attr, is_human_owned_marker, section_or_code_lines,
};
use super::ANTI_HALLU_SECTION_BODY;

/// Result of applying the anti-hallu section maintenance to a single file.
#[derive(Debug, PartialEq, Eq)]
pub enum AntiHalluApplyResult {
    /// Section was missing — inserted at the top of the file.
    Inserted,
    /// Section was already present — only `audit="<date>"` was refreshed.
    Refreshed,
    /// Section already present + `audit=` already today's date — no change.
    NoOp,
    /// File does not exist (project not bootstrapped yet) — caller decides
    /// whether this is fatal.
    FileMissing,
}

/// Name of the section this step maintains.
const SECTION_NAME: &str = "anti-hallu";

/// Prefix of the spec-pointer marker line. Detected to avoid re-inserting it.
const SPEC_MARKER_PREFIX: &str = "<!-- kronn:spec=";

/// The self-describing header block prepended to the top of `docs/AGENTS.md`
/// so ANY agent — Kronn or not — can find the convention spec from the file
/// itself. Points at both the canonical GitHub URL (works offline-of-Kronn)
/// and the local copy (bootstrap drops it in `docs/conventions/`).
const SPEC_HEADER: &str = "\
<!-- kronn:doc-version=\"1.0\" -->\n\
<!-- kronn:spec=\"https://github.com/DocRoms/Kronn/blob/main/docs/conventions/agents-md-format-v1.md\" local=\"docs/conventions/agents-md-format-v1.md\" -->\n\
<!-- This file follows the Kronn AGENTS.md convention v1. Sections marked\n\
     curated=\"ai\" carry [src: …] provenance per assertion. Template v2 adds\n\
     owner=\"audit\" / owner=\"human\"; human-owned sections are never rewritten\n\
     by an audit. Legacy curated=\"human\" sections receive the same protection. -->";

/// Canonical opening marker — written with the date filled in.
fn opening_marker_for_today() -> String {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    format!(
        "<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" owner=\"audit\" audit=\"{today}\" -->"
    )
}

/// The full canonical block (open marker + body + close marker), with today's
/// date filled into the `audit=` attribute.
fn canonical_block_for_today() -> String {
    format!(
        "{opener}\n{body}\n<!-- kronn:section:end -->",
        opener = opening_marker_for_today(),
        body = ANTI_HALLU_SECTION_BODY.trim_end(),
    )
}

/// Apply the anti-hallu STEP 0 to a project's `docs/AGENTS.md`.
///
/// `project_path` is the host-resolved project root (NOT the `docs/`
/// subdirectory). The function looks for `<project_path>/docs/AGENTS.md`
/// (and falls back to legacy `<project_path>/ai/AGENTS.md` if `docs/` is
/// missing but `ai/` is present — same `detect_docs_dir` semantics as the
/// rest of the audit).
pub fn apply(project_path: &Path) -> std::io::Result<AntiHalluApplyResult> {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    let agents_md = docs_dir.join("AGENTS.md");
    if !agents_md.is_file() {
        return Ok(AntiHalluApplyResult::FileMissing);
    }

    let original = std::fs::read_to_string(&agents_md)?;
    let (new_content, result) = transform(&original);
    if !matches!(result, AntiHalluApplyResult::NoOp) {
        std::fs::write(&agents_md, new_content)?;
    }
    Ok(result)
}

/// Pure transformation — extracted so we can unit-test without touching the
/// filesystem. Takes the current file content, returns the new content +
/// what changed.
pub(crate) fn transform(content: &str) -> (String, AntiHalluApplyResult) {
    // First, ensure the self-describing spec-pointer header is at the top.
    // `spec_added` tracks whether we changed anything so the headline
    // result reflects a write even when the anti-hallu section is already
    // current.
    let (content, spec_added) = ensure_spec_header(content);

    let (out, section_result) = match find_marker_line(&content) {
        Some(open_line_idx) => refresh_existing(&content, open_line_idx),
        None => insert_new(&content),
    };

    // If the section was a no-op but we added the spec header, the file
    // still changed → report Refreshed so the caller writes it.
    let result = match (section_result, spec_added) {
        (AntiHalluApplyResult::NoOp, true) => AntiHalluApplyResult::Refreshed,
        (other, _) => other,
    };
    (out, result)
}

/// Ensure the `<!-- kronn:spec=… -->` header is present near the top. If a
/// `kronn:spec` marker already exists, leave it untouched (the URL may have
/// been customised). Otherwise prepend the full SPEC_HEADER block. Returns
/// the (possibly modified) content + whether a change was made.
fn ensure_spec_header(content: &str) -> (String, bool) {
    let has_spec = content
        .lines()
        .any(|line| line.trim_start().starts_with(SPEC_MARKER_PREFIX));
    if has_spec {
        return (content.to_string(), false);
    }
    // Prepend the header + a blank line, then the original content.
    let mut out = String::with_capacity(content.len() + SPEC_HEADER.len() + 2);
    out.push_str(SPEC_HEADER);
    out.push('\n');
    if !content.starts_with('\n') {
        out.push('\n');
    }
    out.push_str(content);
    (out, true)
}

/// Find the line index of the live opening `kronn:section name="anti-hallu"`
/// marker. Ownership / provenance attributes may come in any order around
/// the name, and a marker inside a fenced code block is an example, not the
/// section.
fn find_marker_line(content: &str) -> Option<usize> {
    let in_fence = code_line_mask(content, false).in_fence;
    content.lines().enumerate().find_map(|(idx, line)| {
        let trimmed = line.trim_start();
        let is_opener = trimmed.starts_with("<!-- kronn:section")
            && !trimmed.starts_with("<!-- kronn:section:end")
            && extract_attr(line, "name").as_deref() == Some(SECTION_NAME);
        (is_opener && !in_fence.get(idx).copied().unwrap_or(false)).then_some(idx)
    })
}

/// The marker exists — refresh ONLY the `audit="…"` attribute.
///
/// A marker carrying `owner="human"` / `curated="human"` is the human's
/// section (KT-843): this step runs before the human-owned snapshot, so
/// rewriting that line would strip the ownership with nothing left to
/// restore it. It is left byte-for-byte alone.
fn refresh_existing(content: &str, open_line_idx: usize) -> (String, AntiHalluApplyResult) {
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    if is_human_owned_marker(&lines[open_line_idx]) {
        return (content.to_string(), AntiHalluApplyResult::NoOp);
    }
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let new_opener = opening_marker_for_today();

    // Check if the marker is ALREADY at today's date — if so, no-op.
    let existing = &lines[open_line_idx];
    let today_attr = format!("audit=\"{today}\"");
    if existing.trim() == new_opener.trim() && existing.contains(&today_attr) {
        return (content.to_string(), AntiHalluApplyResult::NoOp);
    }

    lines[open_line_idx] = new_opener;
    let mut new_content = lines.join("\n");
    // Preserve trailing newline if the original had one.
    if content.ends_with('\n') && !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    (new_content, AntiHalluApplyResult::Refreshed)
}

/// The marker is absent — insert the canonical block at the top.
///
/// Placement rule : immediately after the first H1 line if present, before
/// the next `---` separator. Falls back to prepending if no H1. Only lines
/// outside fenced code and outside every existing section count: an H1 or a
/// `---` found inside a (human-owned) section is its content, and the block
/// must never be spliced into it.
fn insert_new(content: &str) -> (String, AntiHalluApplyResult) {
    let block = canonical_block_for_today();
    let lines: Vec<&str> = content.lines().collect();
    let occupied = section_or_code_lines(content);
    let is_free = |idx: usize| !occupied.get(idx).copied().unwrap_or(false);

    // Find the first H1 (`# ...`).
    let h1_idx = lines
        .iter()
        .enumerate()
        .position(|(idx, line)| line.starts_with("# ") && is_free(idx));

    let (head, tail) = match h1_idx {
        Some(idx) => {
            // Find the first `---` after the H1 to insert just BEFORE it.
            // Otherwise insert immediately after the H1 + a blank line.
            let after_h1 = idx + 1;
            let hr_idx = lines[after_h1..]
                .iter()
                .enumerate()
                .position(|(rel, line)| line.trim() == "---" && is_free(after_h1 + rel))
                .map(|rel| after_h1 + rel);
            match hr_idx {
                Some(hr) => {
                    // Insert before the `---` line.
                    (&lines[..hr], &lines[hr..])
                }
                None => {
                    // No `---` after H1 — insert right after the H1.
                    (&lines[..after_h1], &lines[after_h1..])
                }
            }
        }
        None => {
            // No H1 — prepend.
            (&[][..], &lines[..])
        }
    };

    let mut out = String::new();
    for line in head {
        out.push_str(line);
        out.push('\n');
    }
    // Ensure a blank line before the inserted block if the previous line
    // wasn't already blank.
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(&block);
    out.push_str("\n\n");
    // Add an explicit separator before the next content if the tail
    // doesn't already start with one.
    let needs_sep = !tail.first().map(|l| l.trim() == "---").unwrap_or(false);
    if needs_sep && !tail.is_empty() {
        out.push_str("---\n\n");
    }
    for (i, line) in tail.iter().enumerate() {
        out.push_str(line);
        if i < tail.len() - 1 || content.ends_with('\n') {
            out.push('\n');
        }
    }

    (out, AntiHalluApplyResult::Inserted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical body must always start with the H2 and end with the
    /// last sentence — no stray leading/trailing newlines that would break
    /// the `<!-- kronn:section:end -->` placement. Kept to 2 lines plus a
    /// link to the full spec (KT-840): the doctrine itself lives in
    /// `docs/conventions/agents-md-format-v1.md`, not duplicated here.
    #[test]
    fn canonical_body_shape() {
        assert!(ANTI_HALLU_SECTION_BODY
            .trim_start()
            .starts_with("## 0. Anti-Hallucination Protocol"));
        assert!(ANTI_HALLU_SECTION_BODY.contains("[src: file:"));
        assert!(ANTI_HALLU_SECTION_BODY.contains("[src: url:"));
        assert!(ANTI_HALLU_SECTION_BODY.contains("rejected as fabricated"));
        assert!(ANTI_HALLU_SECTION_BODY.contains("conventions/agents-md-format-v1.md"));
        let word_count = ANTI_HALLU_SECTION_BODY.split_whitespace().count();
        assert!(
            word_count < 90,
            "body ballooned to {word_count} words — it must stay a short pointer to the full spec"
        );
    }

    #[test]
    fn insert_when_marker_missing_and_h1_present() {
        let input = "# AI agent context — Entry point\n\n## 1. Section A\nstuff\n";
        let (out, result) = transform(input);
        assert_eq!(result, AntiHalluApplyResult::Inserted);
        assert!(out.contains("<!-- kronn:section name=\"anti-hallu\""));
        assert!(out.contains("## 0. Anti-Hallucination Protocol"));
        // The H1 must still be there, and our block must come after it.
        let h1_pos = out.find("# AI agent context").unwrap();
        let section_pos = out.find("kronn:section name=\"anti-hallu\"").unwrap();
        let section_a_pos = out.find("## 1. Section A").unwrap();
        assert!(h1_pos < section_pos);
        assert!(section_pos < section_a_pos);
    }

    #[test]
    fn insert_before_first_hr_after_h1() {
        let input = "# Header\n\n---\n\n## 1. Body\n";
        let (out, _) = transform(input);
        assert!(out.contains("kronn:section"));
        // The block must come BEFORE the first `---` separator.
        let section_pos = out.find("kronn:section").unwrap();
        let hr_pos = out.find("\n---\n").unwrap();
        assert!(
            section_pos < hr_pos,
            "block must precede the --- separator\n--- got ---\n{out}"
        );
    }

    #[test]
    fn refresh_keeps_body_untouched() {
        let stale_date = "2020-01-01";
        let input = format!(
            "# Header\n\n<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" audit=\"{stale_date}\" -->\n## 0. Anti-Hallucination Protocol\n\nSOME USER-EDITED BODY THAT SHOULD STAY\n<!-- kronn:section:end -->\n\n## 1. Section A\n"
        );
        let (out, result) = transform(&input);
        assert_eq!(result, AntiHalluApplyResult::Refreshed);
        assert!(
            !out.contains(stale_date),
            "stale audit date must be replaced"
        );
        assert!(
            out.contains("SOME USER-EDITED BODY THAT SHOULD STAY"),
            "body inside markers must be preserved"
        );
        assert!(out.contains("audit=\""));
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert!(out.contains(&today), "new audit date must be today");
    }

    #[test]
    fn no_op_when_already_today_and_spec_present() {
        // For a true no-op the file must have BOTH the spec header AND the
        // anti-hallu section already at today's date. Otherwise the spec
        // header gets prepended → Refreshed (see next test).
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let input = format!(
            "{SPEC_HEADER}\n\n# Header\n\n<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" owner=\"audit\" audit=\"{today}\" -->\nBODY\n<!-- kronn:section:end -->\n"
        );
        let (out, result) = transform(&input);
        assert_eq!(result, AntiHalluApplyResult::NoOp);
        assert_eq!(out, input);
    }

    #[test]
    fn spec_header_prepended_when_absent_even_if_section_current() {
        // Section is current at today's date, but the spec header is
        // missing → transform must prepend it and report Refreshed.
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let input = format!(
            "# Header\n\n<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" audit=\"{today}\" -->\nBODY\n<!-- kronn:section:end -->\n"
        );
        let (out, result) = transform(&input);
        assert_eq!(result, AntiHalluApplyResult::Refreshed);
        assert!(out.contains("kronn:spec="), "spec header must be added");
        assert!(
            out.contains("github.com/DocRoms/Kronn"),
            "canonical URL must be present"
        );
        // Section body must be untouched.
        assert!(out.contains("BODY"));
    }

    #[test]
    fn spec_header_not_duplicated_on_second_pass() {
        let input = "# Header\n\nstuff\n";
        let (out1, _) = transform(input);
        let (out2, _) = transform(&out1);
        let count = out2.matches("kronn:spec=").count();
        assert_eq!(
            count, 1,
            "spec marker must appear exactly once after two passes"
        );
    }

    #[test]
    fn prepend_when_no_h1() {
        let input = "no h1 here\njust prose\n";
        let (out, result) = transform(input);
        assert_eq!(result, AntiHalluApplyResult::Inserted);
        let section_pos = out.find("kronn:section").unwrap();
        let prose_pos = out.find("no h1 here").unwrap();
        assert!(section_pos < prose_pos);
    }

    #[test]
    fn empty_input_inserts_block() {
        let (out, result) = transform("");
        assert_eq!(result, AntiHalluApplyResult::Inserted);
        assert!(out.contains("kronn:section name=\"anti-hallu\""));
    }

    // ─── KT-933: STEP 0 never rewrites a human-owned section ────────────────

    /// A live anti-hallu section the human owns, stale audit date and all.
    fn human_anti_hallu_doc(opener: &str) -> String {
        format!(
            "{SPEC_HEADER}\n\n# Header\n\n{opener}\n## 0. Anti-Hallucination Protocol\n\nOur own house rules, not the canonical text.\n<!-- kronn:section:end -->\n\n## 1. Section A\n"
        )
    }

    #[test]
    fn human_owned_anti_hallu_marker_is_never_rewritten() {
        for opener in [
            "<!-- kronn:section name=\"anti-hallu\" owner=\"human\" audit=\"2020-01-01\" -->",
            "<!-- kronn:section name=\"anti-hallu\" curated=\"human\" -->",
            // A human marker wins even when an ai attribute rides along
            // (same precedence as the stamp / restore code).
            "<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" owner=\"human\" audit=\"2020-01-01\" -->",
            // Attribute order is free: the name need not come first.
            "<!-- kronn:section owner=\"human\" name=\"anti-hallu\" -->",
        ] {
            let input = human_anti_hallu_doc(opener);
            let (out, result) = transform(&input);
            assert_eq!(
                result,
                AntiHalluApplyResult::NoOp,
                "nothing to do for {opener}"
            );
            assert_eq!(out, input, "the file must be byte-identical for {opener}");
        }
    }

    #[test]
    fn human_owned_anti_hallu_marker_survives_the_spec_header_being_added() {
        // The header insertion is the only legitimate change here — it
        // reports Refreshed, but the human section itself stays byte-intact.
        let opener =
            "<!-- kronn:section name=\"anti-hallu\" owner=\"human\" audit=\"2020-01-01\" -->";
        let human_block = format!(
            "{opener}\n## 0. Anti-Hallucination Protocol\n\nOur own house rules.\n<!-- kronn:section:end -->"
        );
        let input = format!("# Header\n\n{human_block}\n\n## 1. Section A\n");
        let (out, result) = transform(&input);
        assert_eq!(result, AntiHalluApplyResult::Refreshed);
        assert!(out.contains("kronn:spec="), "header is still added");
        assert!(out.contains(&human_block), "human block byte-intact");
        assert!(
            !out.contains("name=\"anti-hallu\" curated=\"ai\" owner=\"audit\""),
            "no ownership swap"
        );
        assert_eq!(
            out.matches("name=\"anti-hallu\"").count(),
            1,
            "no second canonical block next to the human one"
        );
    }

    #[test]
    fn ai_owned_anti_hallu_marker_is_still_refreshed() {
        // The guard is about ownership, not a blanket stop: an audit-owned
        // marker keeps getting its date refreshed (and normalised).
        let input = format!(
            "{SPEC_HEADER}\n\n# Header\n\n<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" audit=\"2020-01-01\" -->\nBODY\n<!-- kronn:section:end -->\n"
        );
        let (out, result) = transform(&input);
        assert_eq!(result, AntiHalluApplyResult::Refreshed);
        assert!(out.contains("owner=\"audit\""));
        assert!(!out.contains("2020-01-01"));
    }

    #[test]
    fn fenced_anti_hallu_example_is_not_the_live_section() {
        // A doc that QUOTES the opener in a code block must not have that
        // quoted line "refreshed" — nor shadow the real human section below.
        let quoted =
            "<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" audit=\"2020-01-01\" -->";
        for fence in ["```", "~~~"] {
            let human = "<!-- kronn:section name=\"anti-hallu\" owner=\"human\" -->\nMine.\n<!-- kronn:section:end -->";
            let input = format!(
                "{SPEC_HEADER}\n\n# Header\n\nExample of the marker:\n\n{fence}\n{quoted}\n{fence}\n\n{human}\n"
            );
            let (out, result) = transform(&input);
            assert_eq!(
                result,
                AntiHalluApplyResult::NoOp,
                "the live section is the human one ({fence})"
            );
            assert_eq!(
                out, input,
                "quoted line and human block untouched ({fence})"
            );
        }
    }

    #[test]
    fn fenced_anti_hallu_example_alone_does_not_count_as_present() {
        // Only a quoted example in the file → the real section is missing and
        // gets inserted; the example stays exactly as written.
        let quoted =
            "<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" audit=\"2020-01-01\" -->";
        let input = format!("# Header\n\n```\n{quoted}\n```\n");
        let (out, result) = transform(&input);
        assert_eq!(result, AntiHalluApplyResult::Inserted);
        assert!(
            out.contains(&format!("```\n{quoted}\n```")),
            "example intact"
        );
        assert!(out.contains("owner=\"audit\""), "live section inserted");
    }

    #[test]
    fn insertion_never_lands_inside_a_human_section() {
        // No anti-hallu section yet, and the first `---` after the H1 lives
        // INSIDE a human-owned section: the canonical block must go around
        // that section, not through it.
        let human_block = "<!-- kronn:section name=\"notes\" owner=\"human\" -->\nBefore the rule\n---\nAfter the rule\n<!-- kronn:section:end -->";
        let input = format!("# Header\n{human_block}\n\n## 1. Section A\n");
        let (out, result) = transform(&input);
        assert_eq!(result, AntiHalluApplyResult::Inserted);
        assert!(out.contains(human_block), "human block byte-intact:\n{out}");
        let canonical_pos = out.find("kronn:section name=\"anti-hallu\"").unwrap();
        let human_pos = out.find(human_block).unwrap();
        assert!(canonical_pos < human_pos, "block goes right after the H1");
    }

    #[test]
    fn insertion_skips_a_heading_quoted_in_a_code_block() {
        // No real H1 — the only `# ` line is a shell comment inside a fence.
        // The block is prepended instead of being spliced into the code.
        let input = "```sh\n# build it\nmake\n```\n";
        let (out, result) = transform(input);
        assert_eq!(result, AntiHalluApplyResult::Inserted);
        assert!(
            out.contains("```sh\n# build it\nmake\n```\n"),
            "code intact"
        );
        let canonical_pos = out.find("kronn:section name=\"anti-hallu\"").unwrap();
        assert!(canonical_pos < out.find("```sh").unwrap());
    }

    // ─── KT-933: the chain, end to end (Codex harness MSG-039629c2, case 1) ──

    /// One audit step the way `full.rs` runs it, with the REAL functions in
    /// the REAL order: STEP 0 (`apply`, `full.rs:807`), the human-owned
    /// snapshot (`full.rs:1057`), whatever the step writes, then the guard +
    /// restore (`full.rs:1355`). `step_writes` gets the doc as STEP 0 left it
    /// and returns what the step leaves on disk. Returns what STEP 0 reported,
    /// the doc as STEP 0 left it, the number of sections the guard restored
    /// and the final doc.
    fn replay_step0_then_one_audit_step(
        project: &Path,
        step_writes: impl FnOnce(&str) -> String,
    ) -> (AntiHalluApplyResult, String, usize, String) {
        use crate::api::audit::helpers::{
            capture_human_owned_sections, protect_human_owned_sections,
        };
        let agents_md = project.join("docs/AGENTS.md");

        let step0 = apply(project).unwrap();
        let after_step0 = std::fs::read_to_string(&agents_md).unwrap();

        let snapshot = capture_human_owned_sections(project).unwrap();
        std::fs::write(&agents_md, step_writes(&after_step0)).unwrap();
        let restored = protect_human_owned_sections(project, &snapshot, "2026-10-01").unwrap();

        let final_doc = std::fs::read_to_string(&agents_md).unwrap();
        (step0, after_step0, restored, final_doc)
    }

    fn project_with_agents_md(doc: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("docs/AGENTS.md"), doc).unwrap();
        tmp
    }

    /// The case the ticket falsified. Before the fix STEP 0 swapped
    /// `owner="human"` for `curated="ai" owner="audit"` on the live opener,
    /// ahead of the snapshot: the snapshot then saw no human section, and the
    /// step's later write to the body triggered no restoration at all.
    ///
    /// Against the old `refresh_existing` this fails at the first ownership
    /// assertion (STEP 0 no longer leaves the section human).
    #[test]
    fn human_anti_hallu_survives_step0_then_snapshot_then_a_rewrite_then_the_guard() {
        use crate::api::audit::anti_hallu_enforce::contains_human_owned_section;

        for opener in [
            "<!-- kronn:section name=\"anti-hallu\" owner=\"human\" audit=\"2020-01-01\" -->",
            "<!-- kronn:section name=\"anti-hallu\" curated=\"human\" audit=\"2020-01-01\" -->",
            "<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" owner=\"human\" audit=\"2020-01-01\" -->",
            "<!-- kronn:section owner=\"human\" audit=\"2020-01-01\" name=\"anti-hallu\" -->",
        ] {
            let human_block = format!(
                "{opener}\n## 0. Anti-Hallucination Protocol\n\nOur own house rules.\n<!-- kronn:section:end -->"
            );
            // With and without the spec header: STEP 0 reports NoOp when only
            // the (skipped) refresh was due, Refreshed when it also writes the
            // header. The human block must survive both writes.
            for (header, expected_step0) in [
                (format!("{SPEC_HEADER}\n\n"), AntiHalluApplyResult::NoOp),
                (String::new(), AntiHalluApplyResult::Refreshed),
            ] {
                let doc = format!("{header}# Header\n\n{human_block}\n\n## 1. Section A\n");
                // The doc really goes through `refresh_existing` (the live
                // opener is found), not through a fresh insertion.
                assert!(find_marker_line(&doc).is_some(), "live opener: {opener}");

                // What the step does to the section: `body` edits a line in
                // place; `wholesale` swaps the section for the canonical
                // audit-owned block (the shape the old STEP 0 had already
                // produced by itself).
                for rewrite_name in ["body", "wholesale"] {
                    let ctx = format!("{opener} / header={} / {rewrite_name}", !header.is_empty());
                    let tmp = project_with_agents_md(&doc);
                    let (step0, after_step0, restored, final_doc) =
                        replay_step0_then_one_audit_step(tmp.path(), |d| match rewrite_name {
                            "body" => d.replace("Our own house rules.", "Audit-flavoured rules."),
                            _ => d.replace(human_block.as_str(), &canonical_block_for_today()),
                        });

                    // Link 1: STEP 0 leaves the human section human, byte for byte.
                    assert_eq!(step0, expected_step0, "STEP 0 result: {ctx}");
                    assert!(
                        after_step0.contains(&human_block),
                        "STEP 0 left it alone: {ctx}\n{after_step0}"
                    );
                    // Link 2: so the snapshot that follows still has something to guard.
                    assert!(
                        contains_human_owned_section(&after_step0),
                        "the snapshot must still see a human-owned section: {ctx}"
                    );
                    // Link 3: the step's write is detected and undone by the guard.
                    assert_eq!(restored, 1, "the rewrite is detected and undone: {ctx}");
                    assert!(
                        final_doc.contains(&human_block),
                        "human block restored: {ctx}\n{final_doc}"
                    );
                    assert!(
                        !final_doc.contains("Audit-flavoured rules."),
                        "the step's text is gone: {ctx}"
                    );
                    assert_eq!(
                        final_doc.matches("name=\"anti-hallu\"").count(),
                        1,
                        "exactly one anti-hallu section, the human one: {ctx}"
                    );
                    // The refused proposal is recorded for the human to review.
                    let report = std::fs::read_to_string(
                        tmp.path()
                            .join("docs/reports/2026-10-01-human-section-diff-docs-AGENTS.md"),
                    )
                    .unwrap_or_else(|e| panic!("diff report missing ({e}): {ctx}"));
                    assert!(report.contains("Our own house rules."), "report: {ctx}");
                }
            }
        }
    }

    /// Control from the same harness: a plain edit of a human section is
    /// caught through the very same chain. STEP 0 still does its job on the
    /// audit-owned anti-hallu section next to it, so the protection above is
    /// about ownership, not a blanket stop.
    #[test]
    fn ordinary_edit_of_a_human_section_is_still_caught_after_step0() {
        let notes = "<!-- kronn:section name=\"team-notes\" owner=\"human\" -->\nOriginal human note.\n<!-- kronn:section:end -->";
        let doc = format!(
            "{SPEC_HEADER}\n\n# Header\n\n<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" audit=\"2020-01-01\" -->\nOLD CANON\n<!-- kronn:section:end -->\n\n{notes}\n"
        );
        let tmp = project_with_agents_md(&doc);

        let (step0, after_step0, restored, final_doc) =
            replay_step0_then_one_audit_step(tmp.path(), |d| {
                d.replace("Original human note.", "Audit replacement.")
            });

        assert_eq!(step0, AntiHalluApplyResult::Refreshed);
        assert!(
            after_step0.contains(&opening_marker_for_today()),
            "the audit-owned section is still refreshed:\n{after_step0}"
        );
        assert!(
            after_step0.contains(notes),
            "human section untouched by STEP 0"
        );
        assert_eq!(restored, 1, "the ordinary edit is detected");
        assert!(final_doc.contains(notes));
        assert!(!final_doc.contains("Audit replacement."));
        let report = std::fs::read_to_string(
            tmp.path()
                .join("docs/reports/2026-10-01-human-section-diff-docs-AGENTS.md"),
        )
        .unwrap();
        assert!(report.contains("Original human note.") && report.contains("Audit replacement."));
    }

    #[test]
    fn template_and_const_are_in_sync() {
        // The section body in templates/docs/AGENTS.md must match the const
        // ANTI_HALLU_SECTION_BODY byte-for-byte (modulo the audit date
        // placeholder). A drift here = the section in newly-bootstrapped
        // projects diverges from what subsequent audits would refresh,
        // creating a churning loop on every re-audit.
        let template_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("backend has a parent")
            .join("templates/docs/AGENTS.md");
        let template =
            std::fs::read_to_string(&template_path).expect("templates/docs/AGENTS.md must exist");

        // Extract the section between the markers in the template.
        let open_idx = template
            .find("<!-- kronn:section name=\"anti-hallu\"")
            .expect("template must contain the anti-hallu opening marker");
        let open_line_end = template[open_idx..]
            .find('\n')
            .map(|n| open_idx + n + 1)
            .expect("opening marker line must be followed by a newline");
        let close_idx = template[open_line_end..]
            .find("<!-- kronn:section:end -->")
            .map(|n| open_line_end + n)
            .expect("template must contain the closing marker");

        let template_body = &template[open_line_end..close_idx];
        let template_body_trimmed = template_body.trim_end_matches('\n');
        let const_body = ANTI_HALLU_SECTION_BODY.trim_end_matches('\n');

        assert_eq!(
            template_body_trimmed, const_body,
            "templates/docs/AGENTS.md anti-hallu section body must match ANTI_HALLU_SECTION_BODY const",
        );
    }
}
