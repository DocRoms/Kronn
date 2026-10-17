//! Bounded correction and recovery for the final documentary gate (KT-952).
//! Only unambiguous comma-bundled references are normalized automatically.
//! Real missing paths/invalid lines require an agent retry and remain blocking.

use crate::core::document_optimization::{self, Diagnostic};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(super) type Snapshot = BTreeMap<String, String>;

pub(super) fn snapshot(project: &Path) -> Result<Snapshot, String> {
    document_optimization::document_paths(project)?
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(project)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            Ok((relative, content))
        })
        .collect()
}

/// Which step introduced each blocking diagnostic (KT-1306). A diagnostic is
/// charged to the step whose run made it appear, so a document no step targets
/// (a tech-debt detail sheet, a workflow page) is blamed on the step that wrote
/// it, never on the founding step by default. Persisted with the other audit
/// state so a resume plans from the same attribution.
#[derive(Debug, Default, Clone)]
pub(super) struct Blame(Vec<(Diagnostic, u32)>);

#[derive(serde::Serialize, serde::Deserialize)]
struct BlameEntry {
    code: String,
    path: String,
    message: String,
    step: u32,
}

const BLAME_FILE: &str = "document-blame.json";

impl Blame {
    fn file(project: &Path) -> std::path::PathBuf {
        project
            .join(crate::core::audit_state::STATE_DIR)
            .join(BLAME_FILE)
    }

    /// The attribution an earlier launch of the run left; none when absent or
    /// unreadable (recovery then falls back to the target-file mapping).
    pub(super) fn load(project: &Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(Self::file(project)) else {
            return Self::default();
        };
        let entries: Vec<BlameEntry> = serde_json::from_str(&raw).unwrap_or_default();
        Self(
            entries
                .into_iter()
                .map(|e| {
                    (
                        Diagnostic {
                            code: e.code,
                            path: e.path,
                            message: e.message,
                            blocking: true,
                        },
                        e.step,
                    )
                })
                .collect(),
        )
    }

    /// A fresh run starts without the attribution of an earlier one.
    pub(super) fn reset(project: &Path) -> Self {
        let blame = Self::default();
        if Self::file(project).exists() {
            blame.save(project);
        }
        blame
    }

    /// `before`/`after` are the blocking diagnostics around `step`. One present
    /// after but not before is the step's; one already there keeps its earlier
    /// attribution (or none); one that vanished is forgotten, so it is charged
    /// again to whoever brings it back.
    pub(super) fn record_step(
        &mut self,
        project: &Path,
        step: u32,
        before: &[Diagnostic],
        after: &[Diagnostic],
    ) {
        let prior = std::mem::take(&mut self.0);
        self.0 = after
            .iter()
            .filter(|d| d.blocking)
            .filter_map(|d| {
                if before.contains(d) {
                    prior.iter().find(|(p, _)| p == d).cloned()
                } else {
                    Some((d.clone(), step))
                }
            })
            .collect();
        self.save(project);
    }

    pub(super) fn charge(&self, diagnostic: &Diagnostic) -> Option<u32> {
        self.0
            .iter()
            .find(|(d, _)| d == diagnostic)
            .map(|(_, step)| *step)
    }

    fn save(&self, project: &Path) {
        let entries: Vec<_> = self
            .0
            .iter()
            .map(|(d, step)| BlameEntry {
                code: d.code.clone(),
                path: d.path.clone(),
                message: d.message.clone(),
                step: *step,
            })
            .collect();
        let written = crate::core::audit_state::state_dir(project).and_then(|dir| {
            let text = serde_json::to_string(&entries).map_err(|e| e.to_string())?;
            crate::core::mcp_scanner::atomic_write(&dir.join(BLAME_FILE), &text)
                .map_err(|e| e.to_string())
        });
        if let Err(e) = written {
            tracing::warn!("Documentary blame of the run not recorded: {e}");
        }
    }
}

/// The step that introduced the blocker; without a record (an old run, a
/// blocker already there before the audit) the exact target, then the entry
/// point of whatever root the chain was relocated to. The repair prompt
/// explicitly lists auxiliary documents instead of silently skipping them
/// because their original agent step had returned success.
pub(super) fn recovery_plan(
    diagnostics: &[Diagnostic],
    steps: &[super::AnalysisStep],
    blame: &Blame,
) -> BTreeMap<u32, Vec<Diagnostic>> {
    let mut plan = BTreeMap::<u32, Vec<Diagnostic>>::new();
    // Only blocking diagnostics, and `analyze` never marks a human-owned
    // document blocking: no step is asked to rewrite one.
    for diagnostic in diagnostics.iter().filter(|d| d.blocking) {
        let owner = blame
            .charge(diagnostic)
            .filter(|step| (1..=steps.len() as u32).contains(step))
            .unwrap_or_else(|| {
                steps
                    .iter()
                    .position(|s| s.target_file == diagnostic.path)
                    .or_else(|| {
                        steps
                            .iter()
                            .position(|s| s.target_file.rsplit('/').next() == Some("AGENTS.md"))
                    })
                    .unwrap_or(0) as u32
                    + 1
            });
        plan.entry(owner).or_default().push(diagnostic.clone());
    }
    plan
}

/// Adds the target's blocking diagnostics found before the agent runs, so a
/// re-verified document whose citations no longer resolve cannot end
/// `unchanged`: the step is told what is stale and stays blocked until fixed.
pub(super) fn with_stale_target(
    project: &Path,
    target: &str,
    mut recovery: Vec<Diagnostic>,
) -> Result<Vec<Diagnostic>, String> {
    let report = document_optimization::analyze(project)?;
    for diagnostic in report.blocking_diagnostics().filter(|d| d.path == target) {
        if !recovery.contains(diagnostic) {
            recovery.push(diagnostic.clone());
        }
    }
    Ok(recovery)
}

pub(super) fn feedback(diagnostics: &[Diagnostic]) -> String {
    let mut text = String::from("## Documentary gate — targeted correction required\n\n");
    for d in diagnostics {
        text.push_str(&format!("- `{}`: {}\n", d.path, d.message));
    }
    text.push_str("\nRead and correct EACH named document, including auxiliary findings. Use ONE `[src: file: path:line]` or `[src: file: path:start-end]` marker per reference; never bundle paths with commas. A binary file (.docx, .pdf, image) has no lines: cite it by bare path, `[src: file: path]`. Read the actual source before correcting a missing path or invalid line. If the evidence cannot be established, remove the unsupported claim entirely; do not invent a path or merely hide the citation. In a human-owned section, correct only what is wrong; Kronn asks its owner to reconfirm. A resolvable citation proves existence, not the truth of its claim.\n");
    text
}

pub(super) fn recovery_changed(project: &Path, before: &Snapshot, recovery: &[Diagnostic]) -> bool {
    !changed_recovery_paths(project, before, recovery).is_empty()
}

/// The recovery documents the attempt actually rewrote.
pub(super) fn changed_recovery_paths(
    project: &Path,
    before: &Snapshot,
    recovery: &[Diagnostic],
) -> BTreeSet<String> {
    recovery
        .iter()
        .filter(|d| {
            std::fs::read_to_string(project.join(&d.path))
                .is_ok_and(|after| before.get(&d.path).is_some_and(|prior| prior != &after))
        })
        .map(|d| d.path.clone())
        .collect()
}

/// Save the sanitized original before either mechanical or model correction.
/// Content-addressed, create-new files preserve every distinct failed attempt.
fn preserve_original(project: &Path, content: &str) -> Result<(), String> {
    crate::core::audit_state::preserve_citation_original(project, content)
}

pub(super) fn snapshot_for_recovery(
    project: &Path,
    recovery: &[Diagnostic],
) -> Result<Snapshot, String> {
    let before = snapshot(project)?;
    for diagnostic in recovery {
        if let Some(content) = before.get(&diagnostic.path) {
            preserve_original(project, content)?;
        }
    }
    Ok(before)
}

/// Never touches quoted examples. A human section is normalized like the rest:
/// the caller marks a human-validated doc to reconfirm (KT-1079).
fn split_verified_citations(project: &Path, content: &str) -> Option<String> {
    let mask = super::anti_hallu_enforce::code_line_mask(content, true);
    let mut changed = false;
    let mut output = String::new();
    for (index, line) in content.split_inclusive('\n').enumerate() {
        if mask.in_fence.get(index).copied().unwrap_or(false) {
            output.push_str(line);
            continue;
        }
        let mut rest = line;
        while let Some(start) = rest.find("[src: file:") {
            output.push_str(&rest[..start]);
            rest = &rest[start..];
            let Some(end) = rest.find(']') else {
                break;
            };
            let marker = &rest[..=end];
            let raw = &rest[11..end];
            let parts: Vec<_> = raw
                .split(',')
                .enumerate()
                .map(|(index, part)| {
                    let part = part.trim();
                    if index > 0 {
                        part.strip_prefix("file:").map(str::trim).unwrap_or(part)
                    } else {
                        part
                    }
                })
                .collect();
            let replacement = parts
                .iter()
                .map(|p| format!("[src: file: {p}]"))
                .collect::<Vec<_>>()
                .join(" ");
            let strict = parts.iter().all(|part| {
                let Some((path, range)) = part.rsplit_once(':') else {
                    return false;
                };
                let Ok(root) = project.canonicalize() else {
                    return false;
                };
                let Ok(resolved) = project.join(path.trim()).canonicalize() else {
                    return false;
                };
                if !resolved.starts_with(root) {
                    return false;
                }
                let Ok(source) = std::fs::read_to_string(resolved) else {
                    return false;
                };
                crate::core::anti_halluc::parse_line_bounds(range).is_some_and(|(a, b)| {
                    crate::core::anti_halluc::range_is_citable(
                        source.lines().count(),
                        crate::core::anti_halluc::last_citable_line(&source),
                        a,
                        b,
                    )
                })
            });
            if parts.len() > 1
                && strict
                && super::anti_hallu_enforce::lint_step_file(&replacement, &[project]).is_clean()
            {
                output.push_str(&replacement);
                changed = true;
            } else {
                output.push_str(marker);
            }
            rest = &rest[end + 1..];
        }
        output.push_str(rest);
    }
    changed.then_some(output)
}

/// What one attempt's documentary check found (KT-1306).
pub(super) struct AttemptCheck {
    /// Introduced by the step, or explicitly handed to it: blocks it.
    pub blocking: Vec<Diagnostic>,
    /// Already there before the step ran: reported, never blocking it.
    pub preexisting: Vec<Diagnostic>,
}

/// Check changed documents plus explicit recovery targets. The original step
/// snapshot stays fixed across retries so an unchanged bad file cannot vanish
/// from the gate simply because the second attempt ignored it. `baseline` is
/// the blocking set before the step ran: a diagnostic in it blocks only when
/// the step was explicitly asked to fix it (`recovery`).
pub(super) fn check_attempt(
    project: &Path,
    before: &Snapshot,
    recovery: &[Diagnostic],
    baseline: &[Diagnostic],
) -> Result<AttemptCheck, String> {
    let after = snapshot(project)?;
    let root = project.canonicalize().map_err(|e| e.to_string())?;
    let mut targets: BTreeSet<_> = after
        .iter()
        .filter(|(path, text)| before.get(*path) != Some(*text))
        .map(|(path, _)| path.clone())
        .collect();
    targets.extend(recovery.iter().map(|d| d.path.clone()));
    // Legacy docs and root files with their author's own content are not
    // Kronn's docs: reported, never rewritten.
    targets.retain(|path| document_optimization::kronn_owns(project, path));
    for path in &targets {
        let Some(content) = after.get(path) else {
            continue;
        };
        if let Some(corrected) = split_verified_citations(project, content) {
            if !project
                .join(path)
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(&root)
            {
                return Err(format!("repair target escapes project: {path}"));
            }
            preserve_original(project, content)?;
            crate::core::mcp_scanner::atomic_write(&project.join(path), &corrected)
                .map_err(|e| e.to_string())?;
            if let Err(e) = crate::core::doc_provenance::note_write(
                project,
                path,
                content,
                &corrected,
                crate::core::doc_provenance::DocWriter::DocumentRepair,
            ) {
                tracing::warn!("Human-validation provenance of {path} not recorded: {e}");
            }
        }
    }
    let report = document_optimization::analyze(project)?;
    let (preexisting, blocking): (Vec<_>, Vec<_>) = report
        .blocking_diagnostics()
        .filter(|d| targets.contains(&d.path))
        .cloned()
        .partition(|d| baseline.contains(d) && !recovery.contains(d));
    for diagnostic in &blocking {
        if let Some(content) = after.get(&diagnostic.path) {
            preserve_original(project, content)?;
        }
    }
    Ok(AttemptCheck {
        blocking,
        preexisting,
    })
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn repair_splits_verified_bundles_but_keeps_inventions_and_bad_ranges_blocking() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("code.rs"), "first\nsecond\n").unwrap();
        let before = snapshot(dir.path()).unwrap();
        let original = "# Facts\nÉvidence [src: file: code.rs:1, code.rs:2]\nMissing [src: file: invented.rs:1]\nInvalid [src: file: code.rs:9]\n";
        std::fs::write(dir.path().join("docs/a.md"), original).unwrap();
        let blocking = check_attempt(dir.path(), &before, &[], &[])
            .unwrap()
            .blocking;
        let repaired = std::fs::read_to_string(dir.path().join("docs/a.md")).unwrap();
        assert!(repaired.contains("[src: file: code.rs:1] [src: file: code.rs:2]"));
        assert_eq!(blocking.len(), 2);
        assert!(blocking
            .iter()
            .any(|d| d.message.contains("path does not exist")));
        assert!(blocking.iter().any(|d| d.message.contains("outside")));
        let backup = std::fs::read_dir(
            dir.path()
                .join(crate::core::audit_state::CITATION_ORIGINALS_DIR),
        )
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
        assert_eq!(std::fs::read_to_string(backup).unwrap(), original);
        // An unchanged bad document stays a retry target.
        assert_eq!(
            check_attempt(dir.path(), &before, &[], &[])
                .unwrap()
                .blocking
                .len(),
            2
        );
        std::fs::write(
            dir.path().join("docs/a.md"),
            "# Facts\nVerified [src: file: code.rs:1-2]\n",
        )
        .unwrap();
        assert!(check_attempt(dir.path(), &before, &blocking, &[])
            .unwrap()
            .blocking
            .is_empty());
        assert!(!recovery_changed(
            dir.path(),
            &snapshot(dir.path()).unwrap(),
            &blocking
        ));
    }

    #[test]
    fn a_scripted_step_follows_the_gate_feedback_to_a_clean_document() {
        // KT-1322, through the step gate: attempt 1 cites the Read tool's last
        // (empty) line, writes a parenthesised citation, a comment inside a
        // marker and a dead relative link; attempt 2 does what the feedback says.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs/tech-debt")).unwrap();
        std::fs::write(root.join("config.yml"), "a: 1\nb: 2\n").unwrap();
        std::fs::write(root.join("empty.txt"), "").unwrap();
        std::fs::write(root.join("docs/AGENTS.md"), "# P\n").unwrap();
        std::fs::write(root.join("docs/tech-debt/TD-1.md"), "# T\n").unwrap();
        let before = snapshot(root).unwrap();

        // `1-3` is what Read shows for the 2-line file: it must hold.
        let first = "# P\nRead [src: file: config.yml:1-3]\nParen (src: file: config.yml:2)\n\
                     Comment [src: file: config.yml:Focused on x]\nEmpty [src: file: empty.txt]\n\
                     [td](../tech-debt/TD-1.md)\n";
        std::fs::write(root.join("docs/AGENTS.md"), first).unwrap();
        let blocking = check_attempt(root, &before, &[]).unwrap();
        let messages: Vec<_> = blocking.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(messages.len(), 5, "{messages:#?}");
        assert!(!messages.iter().any(|m| m.contains("config.yml:1-3")));
        assert!(messages
            .iter()
            .any(|m| m.contains("write [src: file: config.yml:2]")));
        assert!(messages.iter().any(|m| m.contains("is not a line number")));
        assert!(messages.iter().any(|m| m.contains("the file is empty")));
        assert!(messages
            .iter()
            .any(|m| m.ends_with("did you mean `tech-debt/TD-1.md`?")));

        let second = "# P\nRead [src: file: config.yml:1-3]\nParen [src: file: config.yml:2]\n\
                      Comment [src: file: config.yml:1]\nThe `empty.txt` file is empty.\n\
                      [td](tech-debt/TD-1.md)\n";
        std::fs::write(root.join("docs/AGENTS.md"), second).unwrap();
        let blocking = check_attempt(root, &before, &blocking).unwrap();
        assert!(blocking.is_empty(), "{blocking:#?}");
    }

    #[test]
    fn normalization_handles_repeated_file_prefixes_without_accepting_unverified_references() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("code.rs"), "first\nsecond\n").unwrap();
        let original = "Évidence [src: file: code.rs:1, file: code.rs:2]\n";
        assert_eq!(
            split_verified_citations(dir.path(), original).as_deref(),
            Some("Évidence [src: file: code.rs:1] [src: file: code.rs:2]\n")
        );
        for reference in [
            "invented.rs:1",
            "code.rs:4",
            "code.rs:2-1",
            "../outside.rs:1",
        ] {
            let text = format!("[src: file: code.rs:1, file: {reference}]\n");
            assert!(split_verified_citations(dir.path(), &text).is_none());
        }
    }

    #[test]
    fn normalization_preserves_examples_and_unresolved_bundles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("code.rs"), "line\n").unwrap();
        for text in [
            "```md\n[src: file: code.rs:1, code.rs:1]\n```\n",
            "[src: file: code.rs:1, invented.rs:1]\n",
            "[src: file: code.rs:1, code.rs:2-1]\n",
        ] {
            assert!(
                split_verified_citations(dir.path(), text).is_none(),
                "{text}"
            );
        }
    }

    #[test]
    fn a_human_section_is_repaired_never_skipped_and_marked_to_reconfirm() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("code.rs"), "first\nsecond\n").unwrap();
        std::fs::write(root.join("docs/AGENTS.md"), "# P\n").unwrap();
        let before = snapshot(root).unwrap();
        let human = "# P\n<!-- kronn:section name=\"team\" owner=\"human\" -->\nRule [src: file: code.rs:1, code.rs:2]\n<!-- kronn:section:end -->\n";
        std::fs::write(root.join("docs/AGENTS.md"), human).unwrap();

        check_attempt(root, &before, &[], &[]).unwrap();

        let repaired = std::fs::read_to_string(root.join("docs/AGENTS.md")).unwrap();
        assert!(
            repaired.contains("Rule [src: file: code.rs:1] [src: file: code.rs:2]"),
            "{repaired}"
        );
        let provenance = crate::core::doc_provenance::report(root, true).unwrap();
        assert_eq!(provenance.to_reconfirm, 1);
        assert_eq!(
            provenance.docs[0].kronn_writes[0].writer,
            crate::core::doc_provenance::DocWriter::DocumentRepair
        );
    }

    #[test]
    fn repair_never_targets_a_document_kronn_does_not_own() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs/legacy")).unwrap();
        std::fs::write(root.join("code.rs"), "first\nsecond\n").unwrap();
        std::fs::write(root.join("docs/AGENTS.md"), "# P\n[g](legacy/guide.md)\n").unwrap();
        let before = snapshot(root).unwrap();
        // The human guide changed (an agent touched it) and carries both a
        // bundle the mechanical repair would split and an invented path.
        let human = "# Guide\n[src: file: code.rs:1, code.rs:2]\n[src: file: invented.rs:1]\n[x](../nope.md)\n";
        std::fs::write(root.join("docs/legacy/guide.md"), human).unwrap();

        let report = document_optimization::analyze(root).unwrap();
        let steps = super::super::assemble_chained_steps(crate::models::AuditKind::Full);
        assert!(recovery_plan(&report.diagnostics, &steps, &Blame::default()).is_empty());
        let blocking = check_attempt(root, &before, &report.diagnostics, &[])
            .unwrap()
            .blocking;
        assert!(blocking.is_empty());
        assert_eq!(
            std::fs::read_to_string(root.join("docs/legacy/guide.md")).unwrap(),
            human,
            "the human document is left as written"
        );
        assert!(!root.join(crate::core::audit_state::STATE_DIR).exists());
    }

    #[test]
    fn recovery_targets_previously_successful_documents_and_auxiliary_findings() {
        let steps = super::super::assemble_chained_steps(crate::models::AuditKind::Full);
        let diagnostics: Vec<_> = [
            "docs/AGENTS.md",
            "docs/repo-map.md",
            "docs/tech-debt/TD-1.md",
        ]
        .into_iter()
        .map(|path| Diagnostic {
            code: "broken_citation".into(),
            path: path.into(),
            message: "missing path".into(),
            blocking: true,
        })
        .collect();
        let plan = recovery_plan(&diagnostics, &steps, &Blame::default());
        assert!(plan.len() >= 2);
        assert_eq!(plan.values().map(Vec::len).sum::<usize>(), 3);
        assert!(feedback(&diagnostics).contains("docs/tech-debt/TD-1.md"));
    }

    fn blocker(path: &str, message: &str) -> Diagnostic {
        Diagnostic {
            code: "broken_citation".into(),
            path: path.into(),
            message: message.into(),
            blocking: true,
        }
    }

    #[test]
    fn a_blocker_is_charged_to_the_step_that_introduced_it_not_the_founding_step() {
        let dir = tempfile::tempdir().unwrap();
        let steps = super::super::assemble_chained_steps(crate::models::AuditKind::Full);
        let founding = steps
            .iter()
            .position(|s| s.target_file.ends_with("AGENTS.md"))
            .unwrap() as u32
            + 1;
        let (detail, workflow, carried) = (
            blocker("docs/tech-debt/TD-9.md", "path does not exist `a.rs`"),
            blocker("docs/workflow/ci-cd.md", "path does not exist `b.rs`"),
            blocker("docs/AGENTS.md", "citation line `9` is outside `c.rs`"),
        );
        let mut blame = Blame::reset(dir.path());
        // Already there when the audit began: nobody's.
        blame.record_step(dir.path(), 1, &[], &[]);
        blame.record_step(dir.path(), 5, &[], std::slice::from_ref(&carried));
        blame.record_step(
            dir.path(),
            9,
            std::slice::from_ref(&carried),
            &[carried.clone(), workflow.clone()],
        );
        blame.record_step(
            dir.path(),
            10,
            &[carried.clone(), workflow.clone()],
            &[carried.clone(), workflow.clone(), detail.clone()],
        );
        let all = [detail.clone(), workflow.clone(), carried.clone()];
        let plan = recovery_plan(&all, &steps, &blame);
        assert_eq!(plan.keys().copied().collect::<Vec<_>>(), vec![5, 9, 10]);
        assert_eq!(plan[&10], vec![detail.clone()]);
        assert_eq!(plan[&9], vec![workflow.clone()]);
        assert!(!plan.contains_key(&founding) || founding == 5);
        // A resume reads the same attribution back from disk.
        let reloaded = Blame::load(dir.path());
        assert_eq!(recovery_plan(&all, &steps, &reloaded), plan);
        // A blocker nobody introduced keeps the old mapping.
        let orphan = blocker("docs/other.md", "missing");
        let fallback = recovery_plan(&[orphan], &steps, &blame);
        assert_eq!(fallback.keys().copied().collect::<Vec<_>>(), vec![founding]);
        // Fixed, then brought back by a later step: charged to that one.
        blame.record_step(dir.path(), 11, std::slice::from_ref(&detail), &[]);
        blame.record_step(dir.path(), 12, &[], std::slice::from_ref(&detail));
        assert_eq!(blame.charge(&detail), Some(12));
        // A new run forgets it.
        assert_eq!(Blame::reset(dir.path()).charge(&detail), None);
        assert_eq!(Blame::load(dir.path()).charge(&detail), None);
    }

    #[test]
    fn a_step_is_blocked_only_by_diagnostics_it_introduced() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("code.rs"), "first\nsecond\n").unwrap();
        // A broken citation already sits in a document, before the step runs.
        std::fs::write(
            root.join("docs/old.md"),
            "# Old\nStale [src: file: invented.rs:1]\n",
        )
        .unwrap();
        let baseline = document_optimization::analyze(root)
            .unwrap()
            .blocking_diagnostics()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(baseline.len(), 1, "{baseline:?}");
        let before = snapshot(root).unwrap();
        // The step touches that document (appends a true claim) and writes a
        // new one with its own broken citation.
        std::fs::write(
            root.join("docs/old.md"),
            "# Old\nStale [src: file: invented.rs:1]\nTrue [src: file: code.rs:1]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("docs/new.md"),
            "# New\nBad [src: file: missing.rs:1]\n",
        )
        .unwrap();

        let check = check_attempt(root, &before, &[], &baseline).unwrap();
        assert_eq!(check.blocking.len(), 1, "{:?}", check.blocking);
        assert_eq!(check.blocking[0].path, "docs/new.md");
        assert_eq!(check.preexisting, baseline);

        // Handed to the step explicitly, the pre-existing blocker blocks it.
        let handed = check_attempt(root, &before, &baseline, &baseline).unwrap();
        assert_eq!(handed.blocking.len(), 2);
        assert!(handed.preexisting.is_empty());
    }
}
