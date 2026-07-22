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
//! - If the file already contains `<!-- kronn:section name="anti-hallu"`,
//!   only the `audit="<date>"` attribute on the opening marker is
//!   refreshed. The body inside the markers stays untouched.
//! - If the file does NOT contain the marker, the canonical block is
//!   inserted immediately after the first H1 line (`# AI agent context …`)
//!   and before the next `---` separator. Everything else is preserved.
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

/// The opening marker fragment we search for. We match on the prefix so any
/// `curated=` / `audit=` attributes after the name don't break detection.
const OPEN_MARKER_PREFIX: &str = "<!-- kronn:section name=\"anti-hallu\"";

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
     curated=\"ai\" carry [src: …] provenance per assertion. Any agent — with\n\
     or without Kronn — can read the spec at the URL above to understand the\n\
     markers and the [src:] citation grammar. -->";

/// Canonical opening marker — written with the date filled in.
fn opening_marker_for_today() -> String {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    format!("<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" audit=\"{today}\" -->")
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

/// Find the line index of the opening `kronn:section name="anti-hallu"` marker.
fn find_marker_line(content: &str) -> Option<usize> {
    content
        .lines()
        .position(|line| line.trim_start().starts_with(OPEN_MARKER_PREFIX))
}

/// The marker exists — refresh ONLY the `audit="…"` attribute.
fn refresh_existing(content: &str, open_line_idx: usize) -> (String, AntiHalluApplyResult) {
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
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
/// the next `---` separator. Falls back to prepending if no H1.
fn insert_new(content: &str) -> (String, AntiHalluApplyResult) {
    let block = canonical_block_for_today();
    let lines: Vec<&str> = content.lines().collect();

    // Find the first H1 (`# ...`).
    let h1_idx = lines.iter().position(|line| line.starts_with("# "));

    let (head, tail) = match h1_idx {
        Some(idx) => {
            // Find the first `---` after the H1 to insert just BEFORE it.
            // Otherwise insert immediately after the H1 + a blank line.
            let after_h1 = idx + 1;
            let hr_idx = lines[after_h1..]
                .iter()
                .position(|line| line.trim() == "---")
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
    /// the `<!-- kronn:section:end -->` placement.
    #[test]
    fn canonical_body_shape() {
        assert!(ANTI_HALLU_SECTION_BODY
            .trim_start()
            .starts_with("## 0. Anti-Hallucination Protocol"));
        assert!(ANTI_HALLU_SECTION_BODY.contains("**READ THE CODE**"));
        assert!(ANTI_HALLU_SECTION_BODY.contains("[src: file:"));
        assert!(ANTI_HALLU_SECTION_BODY.contains("[src: url:"));
        assert!(ANTI_HALLU_SECTION_BODY.contains("rejected as fabricated"));
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
            "{SPEC_HEADER}\n\n# Header\n\n<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" audit=\"{today}\" -->\nBODY\n<!-- kronn:section:end -->\n"
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
