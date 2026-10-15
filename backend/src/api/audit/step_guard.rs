//! KT-1211 — a cancelled, killed or failed audit step never leaves an unmarked
//! half-written doc.
//!
//! The agent writes its target in place. When Kronn itself stops the step
//! (cancel, a crash found at the next launch) the write is untrusted and the
//! target is put back as it was before the step ([`restore_target`]). When the
//! step fails after the agent finished or died by itself, what it wrote stays
//! — a human may accept it as it stands — but it carries a visible partial
//! marker ([`mark_partial`]) until a step succeeds on it or a human accepts.

use std::path::Path;

use super::validation::{target_snapshot, TargetSnapshot};

pub(crate) const PARTIAL_OPEN: &str = "<!-- kronn:partial-step -->";
pub(crate) const PARTIAL_CLOSE: &str = "<!-- kronn:partial-step:end -->";

/// `target_state` of a step whose target was put back.
pub(crate) const STATE_RESTORED: &str = "restored";
/// `target_state` of a step whose target was kept and marked.
pub(crate) const STATE_PARTIAL: &str = "partial";

/// Longest reason kept inside the marker, in characters.
const MAX_MARKER_REASON_CHARS: usize = 240;

/// The marker block a failed step's kept target starts with. One line of
/// reason, never the agent's text.
pub(crate) fn partial_banner(step: u32, reason: &str) -> String {
    let one_line = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    let bounded: String = one_line.chars().take(MAX_MARKER_REASON_CHARS).collect();
    let bounded = bounded.replace("-->", "—>");
    format!(
        "{PARTIAL_OPEN}\n> ⚠️ **Partial document** — audit step {step} did not complete ({bounded}). \
Kronn kept what the agent wrote: retry the step, or accept it as it stands.\n{PARTIAL_CLOSE}\n\n"
    )
}

/// Whether `content` starts with a partial marker.
pub(crate) fn has_partial_marker(content: &str) -> bool {
    content.trim_start().starts_with(PARTIAL_OPEN)
}

/// `content` without its partial marker; `None` when it has none.
pub(crate) fn strip_partial_marker(content: &str) -> Option<String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with(PARTIAL_OPEN) {
        return None;
    }
    let after = trimmed.find(PARTIAL_CLOSE).map(|at| &trimmed[at + PARTIAL_CLOSE.len()..])?;
    Some(after.trim_start_matches(['\n', '\r']).to_string())
}

/// The target as it is now: `None` when the file is absent.
pub(crate) fn read_target(project_path: &Path, target: &str) -> Result<Option<String>, String> {
    match std::fs::read_to_string(project_path.join(target)) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("target `{target}` unreadable: {e}")),
    }
}

fn write_atomically(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("kronn-restore.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Put `target` back as it was before the step: `pre` is its content then,
/// `None` when it was absent. Returns whether anything changed on disk.
pub(crate) fn restore_target(
    project_path: &Path,
    target: &str,
    pre: Option<&str>,
) -> Result<bool, String> {
    let path = project_path.join(target);
    let now = read_target(project_path, target)?;
    if now.as_deref() == pre {
        return Ok(false);
    }
    match pre {
        Some(content) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("could not restore `{target}`: {e}"))?;
            }
            write_atomically(&path, content)
                .map_err(|e| format!("could not restore `{target}`: {e}"))?;
        }
        None => std::fs::remove_file(&path)
            .map_err(|e| format!("could not remove the half-written `{target}`: {e}"))?,
    }
    Ok(true)
}

/// Mark the kept target of a failed step as partial. Returns whether a
/// marker was written (`false`: nothing to mark — the file is absent or empty).
pub(crate) fn mark_partial(
    project_path: &Path,
    target: &str,
    step: u32,
    reason: &str,
) -> Result<bool, String> {
    let Some(content) = read_target(project_path, target)? else {
        return Ok(false);
    };
    let body = strip_partial_marker(&content).unwrap_or(content);
    if body.trim().is_empty() {
        return Ok(false);
    }
    let marked = format!("{}{}", partial_banner(step, reason), body);
    write_atomically(&project_path.join(target), &marked)
        .map_err(|e| format!("could not mark `{target}` as partial: {e}"))?;
    Ok(true)
}

/// Remove the partial marker of `target`, when it has one.
pub(crate) fn clear_partial_marker(project_path: &Path, target: &str) -> Result<bool, String> {
    let Some(content) = read_target(project_path, target)? else {
        return Ok(false);
    };
    let Some(clean) = strip_partial_marker(&content) else {
        return Ok(false);
    };
    write_atomically(&project_path.join(target), &clean)
        .map_err(|e| format!("could not clear the partial marker of `{target}`: {e}"))?;
    Ok(true)
}

/// A step ended without success and Kronn did not kill it: when its target
/// differs from what it was before the step, keep it and mark it partial.
/// `pre` is the snapshot taken before the agent ran. Returns the
/// `target_state` to record, `None` when the step left its target alone.
pub(crate) fn settle_failed_target(
    project_path: &Path,
    target: &str,
    pre: &TargetSnapshot,
    step: u32,
    reason: &str,
) -> Result<Option<&'static str>, String> {
    if target == "REVIEW" || target.is_empty() {
        return Ok(None);
    }
    match target_snapshot(project_path, target)? {
        TargetSnapshot::Missing => Ok(None),
        now if &now == pre => Ok(None),
        _ => Ok(mark_partial(project_path, target, step, reason)?.then_some(STATE_PARTIAL)),
    }
}

/// The step was stopped by Kronn (cancel): the target goes back as it was.
/// `Ok(None)` when the step had written nothing.
pub(crate) fn settle_killed_target(
    project_path: &Path,
    target: &str,
    pre_content: Option<&str>,
) -> Result<Option<&'static str>, String> {
    if target == "REVIEW" || target.is_empty() {
        return Ok(None);
    }
    Ok(restore_target(project_path, target, pre_content)?.then_some(STATE_RESTORED))
}

/// Why accepting a failed step as it stands is refused unless a human
/// overrides it: the step's document is known to be unsound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AcceptBlocker {
    /// The agent's output is cut short.
    Truncated,
    /// Citations that do not resolve against the tree (or the documentary
    /// gate's blockers).
    FabricatedCitations,
    /// A deterministic detector signal nothing in the output disposes of.
    UndisposedSignal,
    /// The dimension coverage table is incomplete.
    IncompleteCoverage,
    /// The step was stopped halfway: what is on disk is not its output.
    Killed,
}

/// Fragments of the reasons the audit gates persist on a failed step. Each is
/// produced by exactly one gate; `blocker_of_reason_matches_the_real_gates`
/// builds the real reasons and checks they are all recognised.
pub(crate) const TRUNCATED_FRAGMENT: &str = "looks truncated";
pub(crate) const FABRICATED_FRAGMENT: &str = "fabricated `[src:]` citation";
pub(crate) const DOCUMENTARY_FRAGMENT: &str = "Documentary gate";
pub(crate) const DETECTOR_FRAGMENT: &str = "detector signal(s) injected but never addressed";
pub(crate) const COVERAGE_FRAGMENT: &str = "dimension coverage incomplete";
pub(crate) const KILLED_FRAGMENT: &str = "stopped during step";

impl AcceptBlocker {
    pub(crate) fn of_reason(reason: &str) -> Option<Self> {
        if reason.contains(KILLED_FRAGMENT) {
            Some(Self::Killed)
        } else if reason.contains(TRUNCATED_FRAGMENT) {
            Some(Self::Truncated)
        } else if reason.contains(FABRICATED_FRAGMENT) || reason.contains(DOCUMENTARY_FRAGMENT) {
            Some(Self::FabricatedCitations)
        } else if reason.contains(DETECTOR_FRAGMENT) {
            Some(Self::UndisposedSignal)
        } else if reason.contains(COVERAGE_FRAGMENT) {
            Some(Self::IncompleteCoverage)
        } else {
            None
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Truncated => "its output is truncated",
            Self::FabricatedCitations => "it holds citations that do not resolve",
            Self::UndisposedSignal => "a detector signal is not disposed of",
            Self::IncompleteCoverage => "its dimension coverage is incomplete",
            Self::Killed => "the step was stopped before it finished",
        }
    }
}

/// The text recorded on a step the user cancelled.
pub(crate) fn killed_reason(step: u32, restored: bool) -> String {
    let outcome = if restored {
        "its document was put back as it was before the step"
    } else {
        "it had written nothing"
    };
    format!("Audit {KILLED_FRAGMENT} {step} by the user: {outcome}.")
}

/// The text recorded on a step whose agent died with the backend.
pub(crate) fn interrupted_reason(step: u32, restored: bool) -> String {
    let outcome = if restored {
        "its document was put back as it was before the step"
    } else {
        "it had written nothing"
    };
    format!("Audit {KILLED_FRAGMENT} {step} (the backend stopped): {outcome}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        root
    }

    #[test]
    fn a_marked_target_is_marked_once_and_clears_back_to_the_agents_text() {
        let root = project();
        let file = root.path().join("docs/architecture.md");
        std::fs::write(&file, "# Architecture\n\nhalf a sentence").unwrap();
        assert!(mark_partial(root.path(), "docs/architecture.md", 3, "output looks truncated").unwrap());
        let marked = std::fs::read_to_string(&file).unwrap();
        assert!(has_partial_marker(&marked));
        assert!(marked.contains("step 3"), "{marked}");
        // Marking again replaces the marker, never stacks one.
        assert!(mark_partial(root.path(), "docs/architecture.md", 3, "again").unwrap());
        let again = std::fs::read_to_string(&file).unwrap();
        assert_eq!(again.matches(PARTIAL_OPEN).count(), 1);
        assert!(clear_partial_marker(root.path(), "docs/architecture.md").unwrap());
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "# Architecture\n\nhalf a sentence"
        );
        assert!(!clear_partial_marker(root.path(), "docs/architecture.md").unwrap());
    }

    #[test]
    fn an_absent_or_empty_target_gets_no_marker() {
        let root = project();
        assert!(!mark_partial(root.path(), "docs/none.md", 1, "x").unwrap());
        std::fs::write(root.path().join("docs/empty.md"), " \n").unwrap();
        assert!(!mark_partial(root.path(), "docs/empty.md", 1, "x").unwrap());
        assert_eq!(std::fs::read_to_string(root.path().join("docs/empty.md")).unwrap(), " \n");
    }

    #[test]
    fn a_reason_cannot_close_the_marker_early() {
        let banner = partial_banner(2, "oops --> <script>");
        assert_eq!(banner.matches("-->").count(), 2, "only the open and close markers: {banner}");
    }

    #[test]
    fn restore_puts_back_the_prior_content_or_removes_a_new_file() {
        let root = project();
        let file = root.path().join("docs/glossary.md");
        std::fs::write(&file, "half-writ").unwrap();
        assert!(restore_target(root.path(), "docs/glossary.md", Some("# Glossary\nwhole\n")).unwrap());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Glossary\nwhole\n");
        assert!(!restore_target(root.path(), "docs/glossary.md", Some("# Glossary\nwhole\n")).unwrap());
        assert!(restore_target(root.path(), "docs/glossary.md", None).unwrap());
        assert!(!file.exists());
        assert!(!restore_target(root.path(), "docs/glossary.md", None).unwrap());
    }

    #[test]
    fn a_failed_step_marks_only_a_target_it_changed() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        std::fs::write(&file, "# Stack\nrust\n").unwrap();
        let pre = target_snapshot(root.path(), "docs/stack.md").unwrap();
        // Untouched: nothing to mark.
        assert_eq!(
            settle_failed_target(root.path(), "docs/stack.md", &pre, 4, "agent exited 1").unwrap(),
            None
        );
        assert!(!has_partial_marker(&std::fs::read_to_string(&file).unwrap()));
        // Changed: kept and marked.
        std::fs::write(&file, "# Stack\nrust, and then the agent died mid-sen").unwrap();
        assert_eq!(
            settle_failed_target(root.path(), "docs/stack.md", &pre, 4, "agent exited 1").unwrap(),
            Some(STATE_PARTIAL)
        );
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(has_partial_marker(&content));
        assert!(content.contains("mid-sen"), "the agent's text is kept: {content}");
    }

    #[test]
    fn a_killed_step_gets_its_prior_document_back() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        std::fs::write(&file, "half").unwrap();
        assert_eq!(
            settle_killed_target(root.path(), "docs/stack.md", Some("# Stack\nwhole\n")).unwrap(),
            Some(STATE_RESTORED)
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Stack\nwhole\n");
        // Nothing written: nothing restored.
        assert_eq!(
            settle_killed_target(root.path(), "docs/stack.md", Some("# Stack\nwhole\n")).unwrap(),
            None
        );
    }

    #[test]
    fn blocker_of_reason_matches_the_real_gates() {
        // The enforce gate's, the detector gate's and the coverage gate's.
        assert_eq!(
            AcceptBlocker::of_reason(
                "3 fabricated `[src:]` citation(s) still present after 3 attempts (enforce mode)"
            ),
            Some(AcceptBlocker::FabricatedCitations)
        );
        assert_eq!(
            AcceptBlocker::of_reason("## Documentary gate — targeted correction required\n\n- `docs/a.md`: x"),
            Some(AcceptBlocker::FabricatedCitations)
        );
        assert_eq!(
            AcceptBlocker::of_reason(
                "2 detector signal(s) injected but never addressed in the tech-debt output: a — emit a TD"
            ),
            Some(AcceptBlocker::UndisposedSignal)
        );
        assert_eq!(
            AcceptBlocker::of_reason("dimension coverage incomplete in `docs/x.md`: 3 missing (Step 8 will be re-run)"),
            Some(AcceptBlocker::IncompleteCoverage)
        );
        assert_eq!(
            AcceptBlocker::of_reason(&killed_reason(5, true)),
            Some(AcceptBlocker::Killed)
        );
        assert_eq!(AcceptBlocker::of_reason("agent exited with code 1"), None);
    }
}
