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

/// What became of the target of a step Kronn stopped (cancel, or an agent cut
/// off with its backend). The four cases are told apart on purpose: "the step
/// wrote nothing" must never absorb "the target could not be settled".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KilledOutcome {
    /// The half-written target was put back as it was before the step.
    Restored,
    /// The step had written nothing: the target is whole as it stood.
    Untouched,
    /// It could not be put back; what the agent wrote is kept under a marker.
    Partial,
    /// It could neither be put back nor marked: the doc is in an unknown state.
    Unsettled,
}

impl KilledOutcome {
    /// The `target_state` to record. `None` for [`Self::Unsettled`]: the step
    /// is then neither restored nor visibly partial, so acceptance asks for
    /// an override.
    pub(crate) fn target_state(self) -> Option<&'static str> {
        match self {
            Self::Restored | Self::Untouched => Some(STATE_RESTORED),
            Self::Partial => Some(STATE_PARTIAL),
            Self::Unsettled => None,
        }
    }

    /// The target is whole: nothing a human has to look at.
    pub(crate) fn is_whole(self) -> bool {
        matches!(self, Self::Restored | Self::Untouched)
    }

    /// A target left unsound keeps its snapshot for the acceptance diff.
    pub(crate) fn keeps_snapshot(self) -> bool {
        !self.is_whole()
    }
}

/// Put the target of a stopped step back, or mark it, or say it could do
/// neither. Never silent: an error is [`KilledOutcome::Unsettled`], not
/// "wrote nothing".
pub(crate) fn settle_killed(
    project_path: &Path,
    target: &str,
    pre_content: Option<&str>,
    step: u32,
) -> KilledOutcome {
    match settle_killed_target(project_path, target, pre_content) {
        Ok(Some(_)) => KilledOutcome::Restored,
        Ok(None) => KilledOutcome::Untouched,
        Err(e) => {
            tracing::error!("Audit step {step}: `{target}` could not be put back: {e}");
            let reason = killed_reason(step, KilledOutcome::Partial);
            match mark_partial(project_path, target, step, &reason) {
                Ok(true) => KilledOutcome::Partial,
                Ok(false) => KilledOutcome::Unsettled,
                Err(e) => {
                    tracing::error!("Audit step {step}: `{target}` could not be marked partial: {e}");
                    KilledOutcome::Unsettled
                }
            }
        }
    }
}

/// A step that never ended and has its pre-step snapshot, settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettledStep {
    pub step_index: u32,
    pub target_file: String,
    pub outcome: KilledOutcome,
}

/// Settle the steps of `run_id` that never ended: their agent died with the
/// backend, so the target is untrusted. Each is put back from its snapshot (or
/// marked) and recorded failed with what became of it.
pub(crate) fn settle_unfinished_steps(
    conn: &rusqlite::Connection,
    run_id: &str,
    project_root: &Path,
) -> anyhow::Result<Vec<SettledStep>> {
    let mut settled = Vec::new();
    for snapshot in crate::db::audit_runs::unfinished_step_snapshots(conn, run_id)? {
        let outcome = settle_killed(
            project_root,
            &snapshot.target_file,
            snapshot.content.as_deref(),
            snapshot.step_index,
        );
        crate::db::audit_runs::settle_killed_step(
            conn,
            run_id,
            snapshot.step_index,
            &interrupted_reason(snapshot.step_index, outcome),
            outcome.target_state(),
            outcome.keeps_snapshot(),
            chrono::Utc::now(),
        )?;
        settled.push(SettledStep {
            step_index: snapshot.step_index,
            target_file: snapshot.target_file,
            outcome,
        });
    }
    Ok(settled)
}

/// Boot, after the recorded agents were killed: no step of a previous process
/// is still running, so every step that never ended is settled now — not at
/// the next launch, which may be days away with the doc half-written and
/// nothing marking it. Returns how many steps were settled.
pub(crate) fn settle_unfinished_at_boot(conn: &rusqlite::Connection) -> anyhow::Result<usize> {
    let mut count = 0;
    for (run_id, project_id) in crate::db::audit_runs::runs_with_unfinished_steps(conn)? {
        let Some(project) = crate::db::projects::get_project(conn, &project_id)? else {
            continue;
        };
        let root = crate::core::scanner::resolve_host_path(&project.path);
        match settle_unfinished_steps(conn, &run_id, &root) {
            Ok(settled) => {
                for step in &settled {
                    tracing::warn!(
                        "Boot: step {} of audit run {run_id} was cut off with the backend — `{}`: {:?}",
                        step.step_index, step.target_file, step.outcome
                    );
                }
                count += settled.len();
            }
            Err(e) => tracing::warn!("Boot: could not settle the unfinished steps of run {run_id}: {e}"),
        }
    }
    Ok(count)
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
    /// The step never ended (no verdict was ever recorded on it): whatever its
    /// agent left on disk is unvetted, and no failure reason says why.
    NeverEnded,
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
            Self::NeverEnded => "the step never ended, so nothing vouches for what it wrote",
        }
    }
}

fn outcome_text(outcome: KilledOutcome) -> &'static str {
    match outcome {
        KilledOutcome::Restored => "its document was put back as it was before the step",
        KilledOutcome::Untouched => "it had written nothing",
        KilledOutcome::Partial => {
            "its document could not be put back: what the agent wrote is kept under a partial marker"
        }
        KilledOutcome::Unsettled => {
            "its document could neither be put back nor marked: check it by hand"
        }
    }
}

/// The text recorded on a step the user cancelled.
pub(crate) fn killed_reason(step: u32, outcome: KilledOutcome) -> String {
    format!("Audit {KILLED_FRAGMENT} {step} by the user: {}.", outcome_text(outcome))
}

/// The text recorded on a step whose agent died with the backend.
pub(crate) fn interrupted_reason(step: u32, outcome: KilledOutcome) -> String {
    format!(
        "Audit {KILLED_FRAGMENT} {step} (the backend stopped): {}.",
        outcome_text(outcome)
    )
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

    // One test per gate, each on the reason the gate's own code builds, so a
    // gate that stops being recognised fails exactly its own test.
    fn recognised(reason: String, expected: Option<AcceptBlocker>) {
        assert_eq!(AcceptBlocker::of_reason(&reason), expected, "{reason}");
    }

    #[test]
    fn the_size_floor_reason_blocks_acceptance() {
        recognised(
            super::super::validation::truncated_reason("docs/a.md", 40, 4000),
            Some(AcceptBlocker::Truncated),
        );
    }

    #[test]
    fn the_enforce_gate_reason_blocks_acceptance() {
        recognised(
            super::super::anti_hallu_enforce::enforce_failure_reason(3, 3),
            Some(AcceptBlocker::FabricatedCitations),
        );
    }

    #[test]
    fn the_documentary_gate_reason_blocks_acceptance() {
        let diagnostic = crate::core::document_optimization::Diagnostic {
            code: "stale_citation".into(),
            path: "docs/a.md".into(),
            message: "x".into(),
            blocking: true,
        };
        recognised(
            super::super::document_repair::feedback(&[diagnostic]),
            Some(AcceptBlocker::FabricatedCitations),
        );
    }

    #[test]
    fn the_detector_disposition_reason_blocks_acceptance() {
        recognised(
            super::super::validation::undisposed_reason(2, "a (`x`)"),
            Some(AcceptBlocker::UndisposedSignal),
        );
    }

    #[test]
    fn the_coverage_reason_blocks_acceptance() {
        recognised(
            super::super::validation::coverage_reason("docs/x.md", "3 missing"),
            Some(AcceptBlocker::IncompleteCoverage),
        );
    }

    #[test]
    fn the_stopped_step_reasons_block_acceptance() {
        recognised(killed_reason(5, KilledOutcome::Restored), Some(AcceptBlocker::Killed));
        recognised(killed_reason(5, KilledOutcome::Unsettled), Some(AcceptBlocker::Killed));
        recognised(interrupted_reason(5, KilledOutcome::Partial), Some(AcceptBlocker::Killed));
    }

    #[test]
    fn an_ordinary_failure_is_not_a_blocker() {
        // Not an unsound document: an agent that exited, an empty output.
        recognised("agent exited with code 1".to_string(), None);
        recognised(
            "step produced no output: `docs/a.md` is empty (0 B) after CLI completed".to_string(),
            None,
        );
    }

    #[test]
    fn a_target_that_cannot_be_settled_is_never_reported_as_wrote_nothing() {
        let root = project();
        // Nothing written: whole.
        std::fs::write(root.path().join("docs/a.md"), "whole").unwrap();
        assert_eq!(
            settle_killed(root.path(), "docs/a.md", Some("whole"), 2),
            KilledOutcome::Untouched
        );
        // Written: put back.
        std::fs::write(root.path().join("docs/a.md"), "half").unwrap();
        assert_eq!(
            settle_killed(root.path(), "docs/a.md", Some("whole"), 2),
            KilledOutcome::Restored
        );
        // The target is a directory: it can be neither read, restored nor marked.
        std::fs::create_dir_all(root.path().join("docs/dir.md")).unwrap();
        let stuck = settle_killed(root.path(), "docs/dir.md", Some("whole"), 2);
        assert_eq!(stuck, KilledOutcome::Unsettled);
        assert_eq!(stuck.target_state(), None, "never recorded as restored");
        assert!(stuck.keeps_snapshot());
        assert!(KilledOutcome::Untouched.target_state() == Some(STATE_RESTORED));
    }

    fn boot_conn(root: &Path) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at) VALUES ('p1', 'P', ?1, ?2, ?2)",
            rusqlite::params![root.to_str().unwrap(), now],
        )
        .unwrap();
        crate::db::audit_runs::insert_running(&conn, "r1", "p1", "Full", "ClaudeCode", chrono::Utc::now())
            .unwrap();
        conn
    }

    #[test]
    fn boot_puts_back_the_doc_of_a_step_that_never_ended() {
        let root = project();
        let file = root.path().join("docs/stack.md");
        let conn = boot_conn(root.path());
        // Step 1 done; step 2 was rewriting stack.md when the backend died.
        crate::db::audit_runs::insert_audit_step_start(&conn, "r1", 2, "docs/stack.md", chrono::Utc::now())
            .unwrap();
        crate::db::audit_runs::save_step_snapshot(&conn, "r1", 2, "docs/stack.md", Some("# Stack\nwhole\n"))
            .unwrap();
        std::fs::write(&file, "# Stack\nhalf-writ").unwrap();

        assert_eq!(settle_unfinished_at_boot(&conn).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Stack\nwhole\n");
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        let step = steps.iter().find(|s| s.step_index == 2).unwrap();
        assert!(step.ended_at.is_some(), "the step now has a verdict");
        assert!(!step.cli_success);
        assert_eq!(step.target_state.as_deref(), Some(STATE_RESTORED));
        assert_eq!(
            AcceptBlocker::of_reason(step.step_warning.as_deref().unwrap()),
            Some(AcceptBlocker::Killed)
        );
        assert!(crate::db::audit_runs::step_snapshot(&conn, "r1", 2).unwrap().is_none());
        // Idempotent: nothing left to settle.
        assert_eq!(settle_unfinished_at_boot(&conn).unwrap(), 0);
    }

    #[test]
    fn boot_removes_a_doc_the_step_created_and_keeps_the_snapshot_of_an_unsettled_one() {
        let root = project();
        let conn = boot_conn(root.path());
        // Created by the step (absent before): removed.
        crate::db::audit_runs::insert_audit_step_start(&conn, "r1", 1, "docs/new.md", chrono::Utc::now())
            .unwrap();
        crate::db::audit_runs::save_step_snapshot(&conn, "r1", 1, "docs/new.md", None).unwrap();
        std::fs::write(root.path().join("docs/new.md"), "half").unwrap();
        // Unsettleable (a directory): marked neither restored nor partial.
        crate::db::audit_runs::insert_audit_step_start(&conn, "r1", 2, "docs/dir.md", chrono::Utc::now())
            .unwrap();
        crate::db::audit_runs::save_step_snapshot(&conn, "r1", 2, "docs/dir.md", Some("whole")).unwrap();
        std::fs::create_dir_all(root.path().join("docs/dir.md")).unwrap();

        assert_eq!(settle_unfinished_at_boot(&conn).unwrap(), 2);
        assert!(!root.path().join("docs/new.md").exists());
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        let stuck = steps.iter().find(|s| s.step_index == 2).unwrap();
        assert_eq!(stuck.target_state, None);
        assert!(stuck.ended_at.is_some());
        assert!(
            crate::db::audit_runs::step_snapshot(&conn, "r1", 2).unwrap().is_some(),
            "kept for the acceptance diff"
        );
        // Accepting it needs an override: it is not a restored step.
        assert_eq!(
            super::super::accept::accept_blocker_of(stuck),
            Some(AcceptBlocker::Killed)
        );
    }

    #[test]
    fn accept_asks_for_an_override_on_a_step_that_never_ended() {
        let conn = boot_conn(Path::new("/tmp"));
        crate::db::audit_runs::insert_audit_step_start(&conn, "r1", 3, "docs/x.md", chrono::Utc::now())
            .unwrap();
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        let open = &steps[0];
        assert!(open.ended_at.is_none() && open.step_warning.is_none());
        assert_eq!(
            super::super::accept::accept_blocker_of(open),
            Some(AcceptBlocker::NeverEnded)
        );
        // A step that ended failed for an ordinary reason is not blocked.
        crate::db::audit_runs::finalize_audit_step(
            &conn, "r1", 3, chrono::Utc::now(), 5, &crate::db::audit_runs::StepTokens::default(),
            None, false, Some("agent exited with code 1"), false,
        )
        .unwrap();
        let steps = crate::db::audit_runs::list_audit_steps(&conn, "r1").unwrap();
        assert_eq!(super::super::accept::accept_blocker_of(&steps[0]), None);
    }
}
