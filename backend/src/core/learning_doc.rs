//! 0.9.0 — Continual Learning doc-wiring (PR4c, spec §0).
//!
//! Maintains a `<!-- kronn:section name="learnings" curated="ai" -->` block in
//! `docs/AGENTS.md` that POINTS at `docs/learnings.md` — so agents reading the
//! tiered loader actually discover the accumulated project learnings (verified:
//! Kronn doesn't auto-inject the `docs/` tree; only `~/.kronn/user-context/*.md`
//! is auto-loaded, which is why USER-scope already works without this).
//!
//! Toggle-driven (mirrors `anti_hallu_step.rs` machinery + adds a REMOVE path):
//! - feature ON  → insert the pointer section (if missing) + seed `docs/learnings.md`.
//! - feature OFF → remove the section (if present). `docs/learnings.md` is left
//!   untouched — we never destroy already-validated learnings, just stop pointing.
//!
//! The block is a POINTER only (the learnings live in their dedicated file), so
//! there's zero mutation of audited content → no drift-checksum churn.

use std::path::Path;

const OPEN_PREFIX: &str = "<!-- kronn:section name=\"learnings\"";
const CLOSE_MARKER: &str = "<!-- kronn:section:end -->";

/// The dedicated learnings file, seeded empty (just the render markers) so the
/// AGENTS.md pointer is always valid.
const LEARNINGS_SEED: &str = "\
# Learned conventions

Validated learnings captured by Kronn's continual-learning feature. Each entry
carries a `(lc_id:N)` id and is rendered between the markers below.

<!-- kronn-learning-block:start -->
<!-- kronn-learning-block:end -->
";

#[derive(Debug, PartialEq, Eq)]
pub enum LearningDocOutcome {
    /// Feature ON, section was missing → inserted.
    Inserted,
    /// Feature OFF, section was present → removed.
    Removed,
    /// Nothing to do (already in the desired state).
    NoOp,
    /// `docs/AGENTS.md` doesn't exist (project not bootstrapped).
    FileMissing,
}

fn pointer_block() -> String {
    format!(
        "<!-- kronn:section name=\"learnings\" curated=\"ai\" -->\n\
## Learned conventions\n\
\n\
Validated learnings accumulate in [`docs/learnings.md`](learnings.md). Load it \
when a task touches project conventions, preferences, or known pitfalls.\n\
{CLOSE_MARKER}"
    )
}

/// Sync the learnings doc-wiring for one project against the feature toggle.
/// `project_path` is the project root (NOT `docs/`).
pub fn sync(project_path: &Path, enabled: bool) -> std::io::Result<LearningDocOutcome> {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    let agents_md = docs_dir.join("AGENTS.md");
    if !agents_md.is_file() {
        return Ok(LearningDocOutcome::FileMissing);
    }
    let original = std::fs::read_to_string(&agents_md)?;
    let (new_content, outcome) = transform(&original, enabled);
    if !matches!(outcome, LearningDocOutcome::NoOp) {
        std::fs::write(&agents_md, new_content)?;
    }
    // Seed the dedicated file when enabling (idempotent — only if absent).
    if enabled {
        let learnings_md = docs_dir.join("learnings.md");
        if !learnings_md.exists() {
            std::fs::write(&learnings_md, LEARNINGS_SEED)?;
        }
    }
    Ok(outcome)
}

/// Pure transform — testable without the filesystem.
pub(crate) fn transform(content: &str, enabled: bool) -> (String, LearningDocOutcome) {
    let present = find_open_line(content).is_some();
    match (enabled, present) {
        (true, true) => (content.to_string(), LearningDocOutcome::NoOp),
        (true, false) => insert_pointer(content),
        (false, true) => remove_section(content),
        (false, false) => (content.to_string(), LearningDocOutcome::NoOp),
    }
}

fn find_open_line(content: &str) -> Option<usize> {
    content
        .lines()
        .position(|line| line.trim_start().starts_with(OPEN_PREFIX))
}

/// Append the pointer block at the end of the file (least-disruptive: the
/// pointer doesn't need a precise position, and the agent reads the whole
/// entry-point file). Ensures one blank line of separation.
fn insert_pointer(content: &str) -> (String, LearningDocOutcome) {
    let mut out = String::with_capacity(content.len() + 256);
    out.push_str(content);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(&pointer_block());
    out.push('\n');
    (out, LearningDocOutcome::Inserted)
}

/// Remove the whole `name="learnings"` block (open marker line .. close marker
/// line inclusive) + collapse the surrounding blank lines it leaves behind.
fn remove_section(content: &str) -> (String, LearningDocOutcome) {
    let lines: Vec<&str> = content.lines().collect();
    let Some(open) = lines
        .iter()
        .position(|l| l.trim_start().starts_with(OPEN_PREFIX))
    else {
        return (content.to_string(), LearningDocOutcome::NoOp);
    };
    // Closing marker on or after the open line.
    let close = lines[open..]
        .iter()
        .position(|l| l.trim() == CLOSE_MARKER)
        .map(|rel| open + rel)
        .unwrap_or(open); // malformed (no close) → drop just the open line

    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    kept.extend_from_slice(&lines[..open]);
    kept.extend_from_slice(&lines[(close + 1)..]);

    // Collapse a blank line left dangling at the seam (avoid 3+ newlines).
    while kept.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        kept.pop();
    }
    let mut new_content = kept.join("\n");
    if content.ends_with('\n') {
        new_content.push('\n');
    }
    (new_content, LearningDocOutcome::Removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "# AI agent context\n\n## 1. Stack\nRust.\n";

    #[test]
    fn enable_inserts_pointer_block() {
        let (out, outcome) = transform(BASE, true);
        assert_eq!(outcome, LearningDocOutcome::Inserted);
        assert!(out.contains("kronn:section name=\"learnings\""));
        assert!(out.contains("docs/learnings.md"));
        assert!(out.contains(CLOSE_MARKER));
        // original content preserved
        assert!(out.contains("## 1. Stack"));
    }

    #[test]
    fn enable_is_noop_when_already_present() {
        let (once, _) = transform(BASE, true);
        let (twice, outcome) = transform(&once, true);
        assert_eq!(outcome, LearningDocOutcome::NoOp);
        assert_eq!(once, twice, "idempotent when enabled");
        assert_eq!(once.matches("name=\"learnings\"").count(), 1);
    }

    #[test]
    fn disable_removes_the_block_only() {
        let (with, _) = transform(BASE, true);
        let (without, outcome) = transform(&with, false);
        assert_eq!(outcome, LearningDocOutcome::Removed);
        assert!(!without.contains("kronn:section name=\"learnings\""));
        assert!(!without.contains("docs/learnings.md"));
        // surrounding doc intact
        assert!(without.contains("# AI agent context"));
        assert!(without.contains("## 1. Stack"));
    }

    #[test]
    fn enable_then_disable_round_trips_to_original_shape() {
        let (with, _) = transform(BASE, true);
        let (back, _) = transform(&with, false);
        assert!(back.contains("# AI agent context") && back.contains("## 1. Stack"));
        assert!(!back.contains("learnings"));
    }

    #[test]
    fn disable_is_noop_when_absent() {
        let (out, outcome) = transform(BASE, false);
        assert_eq!(outcome, LearningDocOutcome::NoOp);
        assert_eq!(out, BASE);
    }

    #[test]
    fn does_not_disturb_an_anti_hallu_section() {
        let input = "# Header\n\n<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" audit=\"2026-01-01\" -->\nDOCTRINE\n<!-- kronn:section:end -->\n\n## 1. Stack\n";
        let (with, _) = transform(input, true);
        // anti-hallu block survives, learnings added
        assert!(with.contains("name=\"anti-hallu\"") && with.contains("DOCTRINE"));
        assert!(with.contains("name=\"learnings\""));
        // removing learnings leaves anti-hallu intact
        let (without, _) = transform(&with, false);
        assert!(without.contains("name=\"anti-hallu\"") && without.contains("DOCTRINE"));
        assert!(!without.contains("name=\"learnings\""));
    }
}
