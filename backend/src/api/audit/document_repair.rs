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

/// Exact target first; auxiliary findings belong to the founding/review step
/// on recovery when no durable writer attribution exists (including old runs).
/// The repair prompt explicitly lists those documents instead of silently
/// skipping them because their original agent step had returned success.
pub(super) fn recovery_plan(
    diagnostics: &[Diagnostic],
    steps: &[super::AnalysisStep],
) -> BTreeMap<u32, Vec<Diagnostic>> {
    let mut plan = BTreeMap::<u32, Vec<Diagnostic>>::new();
    // Only blocking diagnostics, and `analyze` never marks a human-owned
    // document blocking: no step is asked to rewrite one.
    for diagnostic in diagnostics.iter().filter(|d| d.blocking) {
        let owner = steps
            .iter()
            .position(|s| s.target_file == diagnostic.path)
            .or_else(|| steps.iter().position(|s| s.target_file == "docs/AGENTS.md"))
            .unwrap_or(0) as u32
            + 1;
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
                let bounds: Vec<_> = range.split('-').map(str::parse::<usize>).collect();
                match bounds.as_slice() {
                    [Ok(n)] => *n > 0 && *n <= source.lines().count(),
                    [Ok(a), Ok(b)] => *a > 0 && a <= b && *b <= source.lines().count(),
                    _ => false,
                }
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

/// Check changed documents plus explicit recovery targets. The original step
/// snapshot stays fixed across retries so an unchanged bad file cannot vanish
/// from the gate simply because the second attempt ignored it.
pub(super) fn check_attempt(
    project: &Path,
    before: &Snapshot,
    recovery: &[Diagnostic],
) -> Result<Vec<Diagnostic>, String> {
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
    let blocking: Vec<_> = report
        .blocking_diagnostics()
        .filter(|d| targets.contains(&d.path))
        .cloned()
        .collect();
    for diagnostic in &blocking {
        if let Some(content) = after.get(&diagnostic.path) {
            preserve_original(project, content)?;
        }
    }
    Ok(blocking)
}

#[cfg(test)]
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
        let blocking = check_attempt(dir.path(), &before, &[]).unwrap();
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
        assert_eq!(check_attempt(dir.path(), &before, &[]).unwrap().len(), 2);
        std::fs::write(
            dir.path().join("docs/a.md"),
            "# Facts\nVerified [src: file: code.rs:1-2]\n",
        )
        .unwrap();
        assert!(check_attempt(dir.path(), &before, &blocking)
            .unwrap()
            .is_empty());
        assert!(!recovery_changed(
            dir.path(),
            &snapshot(dir.path()).unwrap(),
            &blocking
        ));
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
            "code.rs:3",
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

        check_attempt(root, &before, &[]).unwrap();

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
        assert!(recovery_plan(&report.diagnostics, &steps).is_empty());
        let blocking = check_attempt(root, &before, &report.diagnostics).unwrap();
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
        let plan = recovery_plan(&diagnostics, &steps);
        assert!(plan.len() >= 2);
        assert_eq!(plan.values().map(Vec::len).sum::<usize>(), 3);
        assert!(feedback(&diagnostics).contains("docs/tech-debt/TD-1.md"));
    }
}
