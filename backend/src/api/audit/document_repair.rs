//! Bounded correction and recovery for the final documentary gate (KT-952).
//! Only unambiguous comma-bundled references are normalized automatically.
//! Real missing paths/invalid lines require an agent retry and remain blocking.

use crate::core::document_optimization::{self, Diagnostic, UNBACKED_PROVENANCE};
use crate::core::run_provenance::RunProvenance;
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

/// What identifies a blocker from one step to the next: the code, the document
/// and the thing the message names, not the text after it. A message restates
/// the target's line count or a suggested replacement, which change whenever a
/// later step rewrites the cited file; compared whole, the same bare citation
/// would be "new" and charged to that later step (KT-1353).
fn stable_message(message: &str) -> &str {
    let Some(open) = message.find('`') else {
        return message;
    };
    let Some(close) = message[open + 1..].find('`').map(|i| open + 1 + i) else {
        return message;
    };
    let mut end = close + 1;
    const OUTSIDE: &str = " is outside `";
    if let Some(rest) = message[end..].strip_prefix(OUTSIDE) {
        if let Some(i) = rest.find('`') {
            end += OUTSIDE.len() + i + 1;
        }
    }
    &message[..end]
}

pub(super) fn same_blocker(a: &Diagnostic, b: &Diagnostic) -> bool {
    a.code == b.code && a.path == b.path && stable_message(&a.message) == stable_message(&b.message)
}

/// The "step" under which a blocker is recorded when the document held it
/// before the run began (KT-1353): no step introduced it, so none is charged.
const PREEXISTING: u32 = 0;

/// Which step introduced each blocking diagnostic (KT-1306). A diagnostic is
/// charged to the step whose run made it appear, so a document no step targets
/// (a tech-debt detail sheet, a workflow page) is blamed on the step that wrote
/// it, never on the founding step by default. Persisted with the other audit
/// state so a resume plans from the same attribution.
#[derive(Debug, Default, Clone)]
pub(super) struct Blame(
    Vec<(Diagnostic, u32)>,
    /// Pre-existing blockers' documents -> their content hash as the run last
    /// left them: the exemption holds only while a document stays that way.
    BTreeMap<String, String>,
);

#[derive(serde::Serialize, serde::Deserialize)]
struct BlameEntry {
    code: String,
    path: String,
    message: String,
    step: u32,
    /// KT-1353 — only on a pre-existing entry; absent in an older ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    doc_hash: Option<String>,
}

fn content_hash(project: &Path, relative: &str) -> Option<String> {
    let bytes = std::fs::read(project.join(relative)).ok()?;
    Some(crate::core::docs_merge::sha256_hex(&bytes))
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
        let docs = entries
            .iter()
            .filter_map(|e| Some((e.path.clone(), e.doc_hash.clone()?)))
            .collect();
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
            docs,
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
                if before.iter().any(|b| same_blocker(b, d)) {
                    prior
                        .iter()
                        .find(|(p, _)| same_blocker(p, d))
                        .map(|(_, charged)| (d.clone(), *charged))
                } else {
                    Some((d.clone(), step))
                }
            })
            .collect();
        self.seal(project);
    }

    pub(super) fn charge(&self, diagnostic: &Diagnostic) -> Option<u32> {
        self.0
            .iter()
            .find(|(d, _)| same_blocker(d, diagnostic))
            .map(|(_, step)| *step)
    }

    /// KT-1353 — the blockers the documents held before the run began. They are
    /// reported, never charged to a step and never failing the run, for as long
    /// as the blocker itself (code, document, subject) is unchanged: a step that
    /// rewrites the document and leaves the citation as it was (the history entry
    /// of a re-verification) did not bring it. Only a step handed the blocker to
    /// fix answers for it ([`Self::release`]).
    pub(super) fn seed_preexisting(&mut self, project: &Path, blocking: &[Diagnostic]) {
        self.0 = blocking
            .iter()
            .filter(|d| d.blocking)
            .map(|d| (d.clone(), PREEXISTING))
            .collect();
        self.seal(project);
    }

    /// Records each pre-existing blocker's document as it stands now, and forgets
    /// the documents no longer holding one. Called after every change the run
    /// itself makes, so the hash is the run's own last word on the document.
    pub(super) fn seal(&mut self, project: &Path) {
        let paths: BTreeSet<&str> = self
            .0
            .iter()
            .filter(|(_, charged)| *charged == PREEXISTING)
            .map(|(d, _)| d.path.as_str())
            .collect();
        self.1 = paths
            .into_iter()
            .filter_map(|path| Some((path.to_string(), content_hash(project, path)?)))
            .collect();
        self.save(project);
    }

    /// The documents of the pre-existing blockers that are exactly as the run
    /// left them. The ledger outlives the run: an edit made since (a validation
    /// agent's, a human's) hands the document's blockers back to whoever is
    /// asked about them, so the same key coming back is not waved through.
    pub(super) fn unchanged_since_run(&self, project: &Path) -> BTreeSet<String> {
        self.1
            .iter()
            .filter(|(path, hash)| content_hash(project, path).as_ref() == Some(*hash))
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub(super) fn is_preexisting(&self, diagnostic: &Diagnostic) -> bool {
        self.charge(diagnostic) == Some(PREEXISTING)
    }

    /// `(pre-existing, the run's own)` among `diagnostics`.
    pub(super) fn split_preexisting(
        &self,
        diagnostics: &[Diagnostic],
    ) -> (Vec<Diagnostic>, Vec<Diagnostic>) {
        diagnostics
            .iter()
            .cloned()
            .partition(|d| self.is_preexisting(d))
    }

    /// [`Self::split_preexisting`], honouring only the documents in `unchanged`
    /// (see [`Self::unchanged_since_run`]).
    pub(super) fn split_preexisting_in(
        &self,
        diagnostics: &[Diagnostic],
        unchanged: &BTreeSet<String>,
    ) -> (Vec<Diagnostic>, Vec<Diagnostic>) {
        diagnostics
            .iter()
            .cloned()
            .partition(|d| self.is_preexisting(d) && unchanged.contains(&d.path))
    }

    /// A pre-existing blocker handed to `step` to fix is that step's from now on:
    /// if it still holds it afterwards, the step answers for it (retried, then
    /// redone), not the founding step by default.
    pub(super) fn release(&mut self, project: &Path, step: u32, handed: &[Diagnostic]) {
        let mut released = false;
        for (d, charged) in &mut self.0 {
            if *charged == PREEXISTING && handed.iter().any(|h| same_blocker(h, d)) {
                *charged = step;
                released = true;
            }
        }
        if released {
            self.seal(project);
        }
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
                doc_hash: (*step == PREEXISTING)
                    .then(|| self.1.get(&d.path).cloned())
                    .flatten(),
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
    // A blocker the documents held before the run began is nobody's to redo.
    for diagnostic in diagnostics
        .iter()
        .filter(|d| d.blocking && !blame.is_preexisting(d))
    {
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
    if diagnostics.iter().any(|d| d.code == UNBACKED_PROVENANCE) {
        text.push_str("A `[src: deps: …]` row must be one the dependency watch holds, and a `[src: url: …]` page one you fetched in this run. For each one flagged above, fetch the page or keep the watch row; otherwise replace the value AND its citation with `not verified: no source`. Never write a version, date or support end from memory, and never attach a citation to it.\n");
    }
    text
}

/// KT-1353 — what the run's documentary gate let through or repaired by
/// itself, as stored on the run (`audit_runs.documentary_notes`) for the audit
/// timeline. `None` when there is nothing to report. KT-1370: with the markers
/// demoted after the last attempt, each with the step that left it.
pub(super) fn notes_json(
    preexisting: &[Diagnostic],
    autofixes: &[(u32, document_optimization::AppliedFix)],
    unverified: &[(u32, document_optimization::DemotedReference)],
) -> Option<String> {
    if preexisting.is_empty() && autofixes.is_empty() && unverified.is_empty() {
        return None;
    }
    serde_json::to_string(&serde_json::json!({
        "preexisting": preexisting
            .iter()
            .map(|d| serde_json::json!({"path": d.path, "message": d.message}))
            .collect::<Vec<_>>(),
        "autofixes": autofixes
            .iter()
            .map(|(step, f)| serde_json::json!({
                "step": step, "path": f.path, "kind": f.kind, "from": f.from, "to": f.to,
            }))
            .collect::<Vec<_>>(),
        "unverified": unverified
            .iter()
            .map(|(step, r)| serde_json::json!({"step": step, "doc": r.doc, "marker": r.marker}))
            .collect::<Vec<_>>(),
    }))
    .ok()
}

/// KT-1370 — the lines a step records in its `step_warning` for the markers
/// demoted after its last attempt: the count, then each document and the marker
/// as the agent wrote it. Empty when nothing was demoted.
pub(super) fn demotion_notes(demoted: &[document_optimization::DemotedReference]) -> Vec<String> {
    if demoted.is_empty() {
        return Vec::new();
    }
    let mut notes = vec![format!(
        "{} citation marker(s) Kronn could not repair were demoted to backticked paths flagged {}:",
        demoted.len(),
        document_optimization::UNVERIFIED_FLAG
    )];
    notes.extend(
        demoted
            .iter()
            .map(|r| format!("- `{}`: {}", r.doc, r.marker)),
    );
    notes
}

/// KT-1370 — after the LAST attempt: the markers of the gate that neither the
/// agent nor the exact fixes repaired no longer interrupt the run. Applies only
/// when every blocker of the step is a citation-marker defect (`broken_citation`)
/// and demoting the markers clears them all; anything else leaves the documents
/// as they were and returns nothing, so the caller fails the step as before.
/// `holds_after` is the caller's last word on the documents once demoted (the
/// enforce gate's lint of the target); a refusal puts the originals back.
pub(super) fn demote_last_attempt(
    project: &Path,
    blocking: &[Diagnostic],
    run: Option<&RunProvenance>,
    holds_after: &dyn Fn() -> bool,
) -> Result<Vec<document_optimization::DemotedReference>, String> {
    if blocking.is_empty() || blocking.iter().any(|d| d.code != "broken_citation") {
        return Ok(Vec::new());
    }
    let root = project.canonicalize().map_err(|e| e.to_string())?;
    let paths: BTreeSet<&str> = blocking.iter().map(|d| d.path.as_str()).collect();
    let mut writes: Vec<(&str, String, String)> = Vec::new();
    let mut references = Vec::new();
    for path in paths {
        if !document_optimization::kronn_owns(project, path) {
            return Ok(Vec::new());
        }
        let original = std::fs::read_to_string(project.join(path)).map_err(|e| e.to_string())?;
        let Some(demotion) = document_optimization::demote_unverifiable(project, path, &original)
        else {
            return Ok(Vec::new());
        };
        if !project
            .join(path)
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(&root)
        {
            return Err(format!("demotion target escapes project: {path}"));
        }
        references.extend(demotion.references);
        writes.push((path, original, demotion.content));
    }
    let write = |path: &str, from: &str, to: &str| -> Result<(), String> {
        crate::core::mcp_scanner::atomic_write(&project.join(path), to)
            .map_err(|e| e.to_string())?;
        if let Err(e) = crate::core::doc_provenance::note_write(
            project,
            path,
            from,
            to,
            crate::core::doc_provenance::DocWriter::DocumentRepair,
        ) {
            tracing::warn!("Human-validation provenance of {path} not recorded: {e}");
        }
        Ok(())
    };
    let undo = |done: &[(&str, String, String)]| {
        for (path, original, demoted) in done {
            if let Err(e) = write(path, demoted, original) {
                tracing::error!("Could not put {path} back after a refused demotion: {e}");
            }
        }
    };
    for (index, (path, original, demoted)) in writes.iter().enumerate() {
        let attempted =
            preserve_original(project, original).and_then(|()| write(path, original, demoted));
        if let Err(e) = attempted {
            undo(&writes[..index]);
            return Err(e);
        }
    }
    let left = document_optimization::analyze_with(project, run).map(|report| {
        report
            .blocking_diagnostics()
            .any(|d| blocking.iter().any(|b| same_blocker(b, d)))
    });
    if !matches!(left, Ok(false)) || !holds_after() {
        undo(&writes);
        return Ok(Vec::new());
    }
    Ok(references)
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
            // A marker quoted inside an inline code span is an example: kept as is.
            let offset = line.len() - rest.len();
            if line[..offset].matches('`').count() % 2 == 1 {
                output.push_str(marker);
                rest = &rest[end + 1..];
                continue;
            }
            let pieces: Vec<&str> = raw
                .split(',')
                .map(|part| {
                    let part = part.trim();
                    part.strip_prefix("file:").map(str::trim).unwrap_or(part)
                })
                .collect();
            // KT-1357: `deploy.yml:210,214,218` lists lines of the first path.
            // A bare line is only attributed when the list names a single path:
            // with several, nothing says which one the number belongs to.
            let first_path = pieces.first().and_then(|first| {
                let (path, _) = first.rsplit_once(':')?;
                Some(path.trim())
            });
            let single_path = first_path.is_some_and(|only| {
                pieces
                    .iter()
                    .filter_map(|piece| piece.rsplit_once(':'))
                    .all(|(path, _)| path.trim() == only)
            });
            let parts: Vec<String> = raw
                .split(',')
                .enumerate()
                .map(|(index, part)| {
                    let part = part.trim();
                    if index == 0 {
                        return part.to_string();
                    }
                    let part = part.strip_prefix("file:").map(str::trim).unwrap_or(part);
                    match first_path {
                        Some(path)
                            if single_path
                                && !part.contains(':')
                                && crate::core::anti_halluc::parse_line_bounds(part).is_some() =>
                        {
                            format!("{path}:{part}")
                        }
                        _ => part.to_string(),
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
    /// The gate's own exact replacements Kronn applied to the step's documents
    /// before judging them (KT-1353).
    pub repaired: Vec<document_optimization::AppliedFix>,
}

/// Check changed documents plus explicit recovery targets. The original step
/// snapshot stays fixed across retries so an unchanged bad file cannot vanish
/// from the gate simply because the second attempt ignored it. `baseline` is
/// the blocking set before the step ran: a diagnostic in it blocks only when
/// the step was explicitly asked to fix it (`recovery`). `run` is what the
/// audit run read, against which `deps:` / `url:` citations are held (KT-1308).
pub(super) fn check_attempt(
    project: &Path,
    before: &Snapshot,
    recovery: &[Diagnostic],
    baseline: &[Diagnostic],
    run: Option<&RunProvenance>,
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
    let mut repaired = Vec::new();
    for path in &targets {
        let Some(content) = after.get(path) else {
            continue;
        };
        // Bundles first: each reference then stands alone for the exact fixes.
        let split = split_verified_citations(project, content);
        let exact = document_optimization::mechanical_repair(
            project,
            path,
            split.as_deref().unwrap_or(content),
        );
        let corrected = match (split, exact) {
            (None, None) => None,
            (split, None) => split,
            (_, Some(exact)) => {
                for fix in &exact.fixes {
                    tracing::info!(
                        "Documentary gate suggestion applied in {}: {} `{}` -> `{}`",
                        fix.path,
                        fix.kind,
                        fix.from,
                        fix.to
                    );
                }
                repaired.extend(exact.fixes);
                Some(exact.content)
            }
        };
        if let Some(corrected) = corrected {
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
    let report = document_optimization::analyze_with(project, run)?;
    let (preexisting, blocking): (Vec<_>, Vec<_>) = report
        .blocking_diagnostics()
        .filter(|d| targets.contains(&d.path))
        .cloned()
        .partition(|d| {
            baseline.iter().any(|b| same_blocker(b, d))
                && !recovery.iter().any(|r| same_blocker(r, d))
        });
    for diagnostic in &blocking {
        if let Some(content) = after.get(&diagnostic.path) {
            preserve_original(project, content)?;
        }
    }
    Ok(AttemptCheck {
        blocking,
        preexisting,
        repaired,
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
        let blocking = check_attempt(dir.path(), &before, &[], &[], None)
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
            check_attempt(dir.path(), &before, &[], &[], None)
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
        assert!(check_attempt(dir.path(), &before, &blocking, &[], None)
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
        // The citation of an empty file is named in backticks by Kronn (KT-1357).
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
        let blocking = check_attempt(root, &before, &[], &[], None)
            .unwrap()
            .blocking;
        let messages: Vec<_> = blocking.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(messages.len(), 4, "{messages:#?}");
        assert!(!messages.iter().any(|m| m.contains("config.yml:1-3")));
        assert!(messages
            .iter()
            .any(|m| m.contains("write [src: file: config.yml:2]")));
        assert!(messages.iter().any(|m| m.contains("is not a line number")));
        assert!(!messages.iter().any(|m| m.contains("the file is empty")));
        assert!(std::fs::read_to_string(root.join("docs/AGENTS.md"))
            .unwrap()
            .contains("Empty `empty.txt` (empty)\n"));
        assert!(messages
            .iter()
            .any(|m| m.ends_with("did you mean `tech-debt/TD-1.md`?")));

        let second = "# P\nRead [src: file: config.yml:1-3]\nParen [src: file: config.yml:2]\n\
                      Comment [src: file: config.yml:1]\nThe `empty.txt` file is empty.\n\
                      [td](tech-debt/TD-1.md)\n";
        std::fs::write(root.join("docs/AGENTS.md"), second).unwrap();
        let blocking = check_attempt(root, &before, &blocking, &[], None)
            .unwrap()
            .blocking;
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
    fn a_list_of_bare_lines_of_one_file_becomes_one_marker_per_line() {
        // Africanews step 11: `[src: file: .github/workflows/deploy.yml:210,214,218]`.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("deploy.yml"), "a\nb\nc\nd\ne\n").unwrap();
        assert_eq!(
            split_verified_citations(dir.path(), "Deploys [src: file: deploy.yml:1,3, 5]\n")
                .as_deref(),
            Some("Deploys [src: file: deploy.yml:1] [src: file: deploy.yml:3] [src: file: deploy.yml:5]\n")
        );
        assert_eq!(
            split_verified_citations(dir.path(), "[src: file: deploy.yml:1, 2-3]\n").as_deref(),
            Some("[src: file: deploy.yml:1] [src: file: deploy.yml:2-3]\n")
        );
        // One line out of range, or not a line: nothing is split.
        for text in [
            "[src: file: deploy.yml:1,9]\n",
            "[src: file: deploy.yml:1,x]\n",
            "[src: file: deploy.yml,1,2]\n",
        ] {
            assert!(
                split_verified_citations(dir.path(), text).is_none(),
                "{text}"
            );
        }
    }

    #[test]
    fn the_comma_lists_of_the_matrix_split_and_an_ambiguous_mix_is_left_alone() {
        // Bc matrix 682170025: apollo step 5, docroms step 8, africanews step 8.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::create_dir_all(root.join("templates/partials")).unwrap();
        let lines = (1..=220).map(|n| format!("l{n}\n")).collect::<String>();
        for file in ["CI_Quality-test.yaml", "deploy.yml", "ops.yml"] {
            std::fs::write(root.join(".github/workflows").join(file), &lines).unwrap();
        }
        std::fs::write(root.join("templates/partials/footer.html.twig"), &lines).unwrap();
        assert_eq!(
            split_verified_citations(
                root,
                "[src: file: .github/workflows/CI_Quality-test.yaml:19,25]\n"
            )
            .as_deref(),
            Some("[src: file: .github/workflows/CI_Quality-test.yaml:19] [src: file: .github/workflows/CI_Quality-test.yaml:25]\n")
        );
        assert_eq!(
            split_verified_citations(root, "[src: file: .github/workflows/ops.yml:96,156,195]\n")
                .as_deref(),
            Some("[src: file: .github/workflows/ops.yml:96] [src: file: .github/workflows/ops.yml:156] [src: file: .github/workflows/ops.yml:195]\n")
        );
        assert_eq!(
            split_verified_citations(root, "[src: file: templates/partials/footer.html.twig:31,43]\n")
                .as_deref(),
            Some("[src: file: templates/partials/footer.html.twig:31] [src: file: templates/partials/footer.html.twig:43]\n")
        );
        // Two paths that both resolve: each keeps its own line, nothing is guessed.
        assert_eq!(
            split_verified_citations(
                root,
                "[src: file: .github/workflows/deploy.yml:210, .github/workflows/ops.yml:96]\n"
            )
            .as_deref(),
            Some("[src: file: .github/workflows/deploy.yml:210] [src: file: .github/workflows/ops.yml:96]\n")
        );
        // A bare number after several paths belongs to none of them in particular.
        std::fs::write(root.join("a.rs"), "1\n2\n3\n").unwrap();
        std::fs::write(root.join("b.rs"), "1\n2\n3\n").unwrap();
        for text in [
            "[src: file: a.rs:1, b.rs:2, 3]\n",
            "[src: file: a.rs:1, 3, b.rs:2]\n",
            "[src: file: a.rs:1, b.rs:2, file: 3]\n",
        ] {
            assert!(split_verified_citations(root, text).is_none(), "{text}");
        }
        // One path named again keeps the shortcut.
        assert_eq!(
            split_verified_citations(root, "[src: file: a.rs:1, a.rs:2, 3]\n").as_deref(),
            Some("[src: file: a.rs:1] [src: file: a.rs:2] [src: file: a.rs:3]\n")
        );
    }

    #[test]
    fn a_list_quoted_in_an_inline_code_span_is_an_example_and_stays_intact() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "1\n2\n3\n").unwrap();
        let quoted = "The form `[src: file: a.rs:1,2]` is split by Kronn.\n";
        assert!(split_verified_citations(dir.path(), quoted).is_none());
        // The real list on the same line is still split; the quoted one is not.
        assert_eq!(
            split_verified_citations(
                dir.path(),
                "Write `[src: file: a.rs:1,2]` not [src: file: a.rs:1,3]\n"
            )
            .as_deref(),
            Some("Write `[src: file: a.rs:1,2]` not [src: file: a.rs:1] [src: file: a.rs:3]\n")
        );
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

        check_attempt(root, &before, &[], &[], None).unwrap();

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
        let blocking = check_attempt(root, &before, &report.diagnostics, &[], None)
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

        let check = check_attempt(root, &before, &[], &baseline, None).unwrap();
        assert_eq!(check.blocking.len(), 1, "{:?}", check.blocking);
        assert_eq!(check.blocking[0].path, "docs/new.md");
        assert_eq!(check.preexisting, baseline);

        // Handed to the step explicitly, the pre-existing blocker blocks it.
        let handed = check_attempt(root, &before, &baseline, &baseline, None).unwrap();
        assert_eq!(handed.blocking.len(), 2);
        assert!(handed.preexisting.is_empty());
    }

    fn put(root: &std::path::Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn held(root: &std::path::Path) -> Vec<Diagnostic> {
        document_optimization::analyze(root)
            .unwrap()
            .blocking_diagnostics()
            .cloned()
            .collect()
    }

    #[test]
    fn a_sheet_cited_bare_stays_charged_to_the_step_that_wrote_the_citation() {
        // KT-1353, africanews: step 8 wrote the index with bare sheet citations;
        // step 10 then grew a sheet, so the gate's message (it restates the line
        // count) changed and the same citation was charged to step 10.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (index, sheet) = (
            "docs/inconsistencies-tech-debt.md",
            "docs/tech-debt/TD-20261018-email-header-injection.md",
        );
        put(root, "docs/AGENTS.md", "# P\n");
        put(root, sheet, "# TD\n\nMail header injection.\n");
        let mut blame = Blame::reset(root);
        let step8_before = held(root);
        put(
            root,
            index,
            "# Debt\n\nHeader injection [src: file: docs/tech-debt/TD-20261018-email-header-injection.md]\n",
        );
        let after8 = held(root);
        blame.record_step(root, 8, &step8_before, &after8);
        let wrote = after8
            .iter()
            .find(|d| d.path == index)
            .expect("the bare sheet citation blocks")
            .clone();
        assert!(wrote.message.contains(":1-3]"), "{}", wrote.message);

        put(
            root,
            sheet,
            "# TD\n\nMail header injection.\n\n## Fix\nValidate.\n\nMore.\n",
        );
        let after10 = held(root);
        blame.record_step(root, 10, &after8, &after10);
        let grown = after10.iter().find(|d| d.path == index).unwrap();
        assert_ne!(grown.message, wrote.message, "the line count moved");
        assert_eq!(blame.charge(grown), Some(8));
        let steps = super::super::assemble_chained_steps(crate::models::AuditKind::Full);
        let plan = recovery_plan(&after10, &steps, &blame);
        assert!(plan[&8].iter().any(|d| d.path == index), "{plan:#?}");
        assert!(!plan.contains_key(&10), "{plan:#?}");
    }

    #[test]
    fn the_blockers_of_the_committed_docs_never_fail_the_run_even_when_a_step_rewrites_the_document(
    ) {
        // KT-1353, africanews: TD-20260828-* sheets cite a gitignored vendor path
        // and an out-of-range line; the final gate charged them to step 1.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sheet = "docs/tech-debt/TD-20260828-api-json-contract-inconsistent.md";
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        put(root, ".gitignore", "application/vendor/\n");
        put(
            root,
            "application/src/LocalLiveRoutingTest.php",
            &"x\n".repeat(29),
        );
        put(root, "docs/AGENTS.md", "# P\n");
        put(root, "docs/glossary.md", "# G\n");
        put(
            root,
            sheet,
            "# TD\n\nSymfony [src: file: application/vendor/symfony/symfony/src/Response.php:10]\n\
             Test [src: file: application/src/LocalLiveRoutingTest.php:35-40]\n",
        );
        let committed = held(root);
        assert_eq!(committed.len(), 2, "{committed:#?}");
        assert!(committed.iter().any(|d| d.message.contains("git-ignored")));

        let steps = super::super::assemble_chained_steps(crate::models::AuditKind::Full);
        let mut blame = Blame::reset(root);
        blame.seed_preexisting(root, &committed);
        // Step 1 rewrites only AGENTS.md; the sheets are not its doing.
        put(root, "docs/AGENTS.md", "# P\n\nRewritten.\n");
        blame.record_step(root, 1, &committed, &held(root));
        let (preexisting, own) = blame.split_preexisting(&held(root));
        assert_eq!((preexisting.len(), own.len()), (2, 0));
        assert!(recovery_plan(&held(root), &steps, &blame).is_empty());
        // A resume reads the same record back.
        assert!(recovery_plan(&held(root), &steps, &Blame::load(root)).is_empty());

        // Step 2 brings a blocker of its own: that one is charged to it.
        put(
            root,
            "docs/glossary.md",
            "# G\n[src: file: invented.rs:1]\n",
        );
        let after2 = held(root);
        blame.record_step(root, 2, &committed, &after2);
        let plan = recovery_plan(&after2, &steps, &blame);
        assert_eq!(plan.keys().copied().collect::<Vec<_>>(), vec![2]);

        // Step 8 re-verifies the sheet: it appends a history entry and leaves the
        // vendor citation as it was (africanews). The blocker is the same one
        // (code, document, subject), so it is still nobody's: not charged to the
        // founding step, not failing the run.
        let text = std::fs::read_to_string(root.join(sheet)).unwrap();
        put(
            root,
            sheet,
            &format!("{text}\n## audit_history\n- API Design Audit re-verification\n"),
        );
        let after8 = held(root);
        blame.record_step(root, 8, &after2, &after8);
        let plan = recovery_plan(&after8, &steps, &blame);
        assert_eq!(
            plan.keys().copied().collect::<Vec<_>>(),
            vec![2],
            "{plan:#?}"
        );
        let (preexisting, own) =
            blame.split_preexisting_in(&after8, &blame.unchanged_since_run(root));
        assert_eq!(preexisting.len(), 2, "{preexisting:#?}");
        assert_eq!(own.len(), 1, "only step 2's own blocker: {own:#?}");
        assert_eq!(recovery_plan(&after8, &steps, &Blame::load(root)), plan);

        // A step handed the blocker to fix answers for it, not the founding step.
        blame.release(root, 8, &committed);
        let plan = recovery_plan(&after8, &steps, &blame);
        assert_eq!(plan.keys().copied().collect::<Vec<_>>(), vec![2, 8]);
        assert_eq!(plan[&8].len(), 2);
    }

    #[test]
    fn the_exemption_ends_with_the_run_when_the_document_is_edited_afterwards() {
        // KT-1353 review: the ledger outlives the run; an edit between the run
        // and the validation that brings the same key back must not pass.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sheet = "docs/tech-debt/TD-20260828-x.md";
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        put(root, ".gitignore", "application/vendor/\n");
        put(root, "docs/AGENTS.md", "# P\n");
        put(
            root,
            sheet,
            "# TD\n[src: file: application/vendor/symfony/Response.php:10]\n",
        );
        let committed = held(root);
        assert_eq!(committed.len(), 1, "{committed:#?}");
        let mut blame = Blame::reset(root);
        blame.seed_preexisting(root, &committed);

        // Untouched since the run: waved through, from the ledger on disk too.
        let reloaded = Blame::load(root);
        let unchanged = reloaded.unchanged_since_run(root);
        let (waved, own) = reloaded.split_preexisting_in(&held(root), &unchanged);
        assert_eq!((waved.len(), own.len()), (1, 0));

        // The citation is fixed, then reintroduced by a later edit (the key is
        // the same): the document is no longer as the run left it.
        put(root, sheet, "# TD\nFixed.\n");
        put(
            root,
            sheet,
            "# TD\n[src: file: application/vendor/symfony/Response.php:10]\nEdited.\n",
        );
        let reloaded = Blame::load(root);
        let unchanged = reloaded.unchanged_since_run(root);
        assert!(unchanged.is_empty(), "{unchanged:?}");
        let (waved, own) = reloaded.split_preexisting_in(&held(root), &unchanged);
        assert_eq!((waved.len(), own.len()), (0, 1), "{own:#?}");

        // A ledger written before the hash existed honours nothing.
        let old = std::fs::read_to_string(Blame::file(root)).unwrap();
        let entries: Vec<serde_json::Value> = serde_json::from_str(&old).unwrap();
        let without: Vec<_> = entries
            .into_iter()
            .map(|mut e| {
                e.as_object_mut().unwrap().remove("doc_hash");
                e
            })
            .collect();
        std::fs::write(Blame::file(root), serde_json::to_string(&without).unwrap()).unwrap();
        assert!(Blame::load(root).unchanged_since_run(root).is_empty());
    }

    #[test]
    fn the_notes_of_a_run_list_what_was_let_through_and_repaired() {
        let held = vec![blocker(
            "docs/tech-debt/TD-1.md",
            "path does not exist `v.php`",
        )];
        let fixes = vec![(
            10,
            document_optimization::AppliedFix {
                path: "docs/AGENTS.md".into(),
                kind: "link_base",
                from: "docs/operations/".into(),
                to: "operations".into(),
            },
        )];
        assert_eq!(notes_json(&[], &[], &[]), None);
        let json: serde_json::Value =
            serde_json::from_str(&notes_json(&held, &fixes, &[]).unwrap()).unwrap();
        assert_eq!(json["preexisting"][0]["path"], "docs/tech-debt/TD-1.md");
        assert_eq!(json["autofixes"][0]["step"], 10);
        assert_eq!(json["autofixes"][0]["to"], "operations");
        for language in ["fr", "en", "es"] {
            let block =
                super::super::helpers::documentary_notes_block(&held, &fixes, &[], language);
            assert!(
                block.contains("docs/tech-debt/TD-1.md"),
                "{language}: {block}"
            );
            assert!(block.contains("`operations`"), "{language}: {block}");
        }
        assert!(super::super::helpers::documentary_notes_block(&[], &[], &[], "en").is_empty());
    }

    #[test]
    fn a_step_gets_the_exact_replacements_of_the_gate_applied_before_it_fails() {
        // KT-1353, apollo step 10 (link base, secret file) and africanews step 8
        // (sheet cited bare): applied at once, recorded, the original preserved.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(root, "docs/AGENTS.md", "# P\n");
        put(root, "docs/operations/http.md", "# Http\n");
        put(
            root,
            ".ssh/vr-v2njs01-s",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nx\n",
        );
        put(
            root,
            "docs/tech-debt/TD-20261018-ssh-key-tracked.md",
            "# TD\n",
        );
        put(root, "tests/x.js", "a\nb\n");
        let before = snapshot(root).unwrap();

        let security = "# Security\n[ops](docs/operations/)\n";
        let sheet = "# TD\nTracked key [src: file: .ssh/vr-v2njs01-s]\n";
        let index = "# Debt\nSheet [src: file: docs/tech-debt/TD-20261018-ssh-key-tracked.md]\n\
                     Ambiguous [src: file: tests/x.js:1, invented.rs:2]\n";
        put(root, "docs/inconsistencies-security.md", security);
        put(root, "docs/tech-debt/TD-20261018-ssh-key-tracked.md", sheet);
        put(root, "docs/inconsistencies-tech-debt.md", index);

        let check = check_attempt(root, &before, &[], &[], None).unwrap();
        let kinds: Vec<_> = check.repaired.iter().map(|f| f.kind).collect();
        assert_eq!(kinds.len(), 3, "{:#?}", check.repaired);
        for kind in ["link_base", "secret_presence", "td_sheet_presence"] {
            assert!(kinds.contains(&kind), "{kind} in {kinds:?}");
        }
        let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap();
        assert!(read("docs/inconsistencies-security.md").contains("[ops](operations)"));
        assert!(read("docs/tech-debt/TD-20261018-ssh-key-tracked.md")
            .contains("[src: file: .ssh/vr-v2njs01-s:exists]"));
        assert!(read("docs/inconsistencies-tech-debt.md")
            .contains("TD-20261018-ssh-key-tracked.md:exists]"));
        // The comma bundle with an invented path is ambiguous: still blocking.
        assert_eq!(check.blocking.len(), 1, "{:#?}", check.blocking);
        assert!(check.blocking[0]
            .message
            .contains("one [src: file: path:line] marker"));
        assert!(read("docs/inconsistencies-tech-debt.md").contains("tests/x.js:1, invented.rs:2"));
        // Every original is kept, as for the bundle repair.
        let originals =
            std::fs::read_dir(root.join(crate::core::audit_state::CITATION_ORIGINALS_DIR))
                .unwrap()
                .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
                .collect::<Vec<_>>();
        for original in [security, sheet, index] {
            assert!(originals.iter().any(|o| o == original), "{original}");
        }
    }

    #[test]
    fn the_index_kronn_generates_passes_the_gate() {
        // KT-1353: whatever the sheets say, the rows Kronn writes link to them.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        put(root, "docs/AGENTS.md", "# P\n");
        let sheet = |id: &str| {
            format!(
                "---\nname: {}\ndescription: Finding {id}\nmetadata:\n  type: tech-debt\n  audit_history:\n    - date: 2026-10-01\n      status: Open\n      reviewer: Full audit\n---\n\n# {id}\n\n- **Area**: Backend\n- **Severity**: High\n- **Status**: Open\n\n## Problem (fact)\nSomething.\n",
                id.to_lowercase()
            )
        };
        for id in ["TD-20261001-a", "TD-20261001-b"] {
            put(root, &format!("docs/tech-debt/{id}.md"), &sheet(id));
        }
        put(
            root,
            "docs/inconsistencies-tech-debt.md",
            "# Tech debt (index)\n\n## Current list\n\n| ID | Problem | Area | Severity |\n|----|---------|------|----------|\n| TD-20261001-a | Finding A | Backend | High |\n",
        );
        let outcome = crate::core::td_index::resync(
            &root.join("docs"),
            crate::core::adr::Trust::RunBaseline {
                decisions: "",
                records: "",
            },
        )
        .unwrap();
        assert_eq!(outcome.listed, 2);
        let index =
            std::fs::read_to_string(root.join("docs/inconsistencies-tech-debt.md")).unwrap();
        assert!(
            index.contains("[sheet](tech-debt/TD-20261001-b.md)"),
            "{index}"
        );
        assert!(!index.contains("[src: file: docs/tech-debt/"), "{index}");
        let blocking = held(root);
        assert!(blocking.is_empty(), "{blocking:#?}");
    }

    /// KT-1308 x KT-1306 — an unbacked `deps:` citation is a blocker like any
    /// other: when it was already there it is in the baseline and does not
    /// block the step, when the step wrote it, it blocks and is charged to it.
    #[test]
    fn an_unbacked_provenance_follows_the_baseline_and_the_blame() {
        use crate::core::version_sources::{DependencySnapshot, WatchedPackage};
        let run = RunProvenance {
            deps: Some(DependencySnapshot {
                checked_on: "2026-10-16".into(),
                packages: vec![WatchedPackage {
                    manager: "JS / TS".into(),
                    manifest: "package.json".into(),
                    name: "joi".into(),
                    current: "1.0.0".into(),
                    latest: "2.0.0".into(),
                    major: true,
                    declared_only: false,
                    lockfile: None,
                }],
                unchecked: vec![],
                up_to_date: vec![],
            }),
            fetched: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("code.rs"), "first\nsecond\n").unwrap();
        let invented =
            |name: &str| format!("| {name} | 9.9.9 [src: deps: JS / TS/{name}@2026-10-16] |\n");
        std::fs::write(
            root.join("docs/old.md"),
            format!("# Old\n{}", invented("prettier")),
        )
        .unwrap();
        // Outside a run nothing can hold the citation: no baseline.
        assert!(document_optimization::analyze(root)
            .unwrap()
            .blocking_diagnostics()
            .next()
            .is_none());
        let baseline = document_optimization::analyze_with(root, Some(&run))
            .unwrap()
            .blocking_diagnostics()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(baseline.len(), 1, "{baseline:?}");
        assert_eq!(baseline[0].code, UNBACKED_PROVENANCE);
        let before = snapshot(root).unwrap();
        // The step touches the same document with a true claim and writes a
        // new one with its own invented version.
        std::fs::write(
            root.join("docs/old.md"),
            format!(
                "# Old\n{}True [src: file: code.rs:1]\n",
                invented("prettier")
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("docs/new.md"),
            format!("# New\n{}", invented("jest")),
        )
        .unwrap();

        let check = check_attempt(root, &before, &[], &baseline, Some(&run)).unwrap();
        assert_eq!(check.blocking.len(), 1, "{:?}", check.blocking);
        assert_eq!(check.blocking[0].path, "docs/new.md");
        assert_eq!(check.blocking[0].code, UNBACKED_PROVENANCE);
        assert_eq!(check.preexisting, baseline);

        // The ledger charges the new one to the step, keeps the old one's owner.
        let after = document_optimization::analyze_with(root, Some(&run))
            .unwrap()
            .blocking_diagnostics()
            .cloned()
            .collect::<Vec<_>>();
        let mut blame = Blame::reset(root);
        blame.record_step(root, 4, &[], &baseline);
        blame.record_step(root, 7, &baseline, &after);
        assert_eq!(blame.charge(&baseline[0]), Some(4));
        assert_eq!(blame.charge(&check.blocking[0]), Some(7));
    }

    /// docroms step 8, reduced: a text file cited without a line, a secret file.
    fn docroms(doc_text: &str) -> (tempfile::TempDir, Vec<Diagnostic>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join("config/packages/prod")).unwrap();
        std::fs::write(
            root.join("config/packages/prod/monolog.yaml"),
            "monolog:\n    handlers: {}\n",
        )
        .unwrap();
        std::fs::write(root.join(".env"), "TOKEN=1\n").unwrap();
        std::fs::write(root.join("docs/AGENTS.md"), "# P\n").unwrap();
        std::fs::write(root.join("docs/tech-debt.md"), doc_text).unwrap();
        let blocking = document_optimization::analyze(root)
            .unwrap()
            .blocking_diagnostics()
            .cloned()
            .collect();
        (dir, blocking)
    }

    #[test]
    fn the_last_attempt_demotes_the_docroms_marker_to_a_flagged_path() {
        let text = "# T\n\nLogs [src: file: config/packages/prod/monolog.yaml] in prod.\n";
        let (dir, blocking) = docroms(text);
        assert_eq!(blocking.len(), 1, "{blocking:?}");
        let demoted = demote_last_attempt(dir.path(), &blocking, None, &|| true).unwrap();
        assert_eq!(demoted.len(), 1);
        assert_eq!(demoted[0].doc, "docs/tech-debt.md");
        assert_eq!(
            demoted[0].marker,
            "[src: file: config/packages/prod/monolog.yaml]"
        );
        let written = std::fs::read_to_string(dir.path().join("docs/tech-debt.md")).unwrap();
        assert_eq!(
            written,
            "# T\n\nLogs `config/packages/prod/monolog.yaml` (unverified reference) in prod.\n"
        );
        // The original is kept, and the gate no longer sees a citation.
        assert!(std::fs::read_dir(
            dir.path()
                .join(crate::core::audit_state::CITATION_ORIGINALS_DIR)
        )
        .unwrap()
        .next()
        .is_some());
        assert!(document_optimization::analyze(dir.path())
            .unwrap()
            .blocking_diagnostics()
            .next()
            .is_none());
        let notes = demotion_notes(&demoted);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[1].contains("docs/tech-debt.md"));
        assert!(notes[1].contains("[src: file: config/packages/prod/monolog.yaml]"));
    }

    #[test]
    fn a_blocker_that_is_not_a_citation_marker_demotes_nothing() {
        let text =
            "# T\n\nLogs [src: file: config/packages/prod/monolog.yaml] [x](nowhere/gone.md)\n";
        let (dir, blocking) = docroms(text);
        assert!(blocking.len() >= 2, "{blocking:?}");
        assert!(blocking.iter().any(|d| d.code != "broken_citation"));
        let demoted = demote_last_attempt(dir.path(), &blocking, None, &|| true).unwrap();
        assert!(demoted.is_empty());
        let written = std::fs::read_to_string(dir.path().join("docs/tech-debt.md")).unwrap();
        assert_eq!(written, text);
        // Nothing to demote: nothing to do.
        assert!(demote_last_attempt(dir.path(), &[], None, &|| true)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_demotion_the_caller_refuses_puts_the_document_back() {
        let text = "# T\n\nLogs [src: file: config/packages/prod/monolog.yaml] in prod.\n";
        let (dir, blocking) = docroms(text);
        let demoted = demote_last_attempt(dir.path(), &blocking, None, &|| false).unwrap();
        assert!(demoted.is_empty());
        let written = std::fs::read_to_string(dir.path().join("docs/tech-debt.md")).unwrap();
        assert_eq!(written, text);
    }

    #[test]
    fn a_blocker_demotion_cannot_clear_leaves_the_document_whole() {
        // A parenthesised citation is `broken_citation` too, but it is not a
        // marker: the document is left whole and the step fails as before.
        let text =
            "# T\n\nLogs [src: file: config/packages/prod/monolog.yaml] (src: file: src/a.rs:1)\n";
        let (dir, blocking) = docroms(text);
        assert!(blocking.iter().all(|d| d.code == "broken_citation"));
        let demoted = demote_last_attempt(dir.path(), &blocking, None, &|| true).unwrap();
        assert!(demoted.is_empty());
        let written = std::fs::read_to_string(dir.path().join("docs/tech-debt.md")).unwrap();
        assert_eq!(written, text);
    }

    #[test]
    fn a_secret_file_marker_is_removed_with_no_path_and_no_line() {
        let text = "# T\n\nSet the token [src: file: .env:3 the token line] first.\n";
        let (dir, blocking) = docroms(text);
        assert_eq!(blocking.len(), 1, "{blocking:?}");
        let demoted = demote_last_attempt(dir.path(), &blocking, None, &|| true).unwrap();
        assert_eq!(demoted.len(), 1);
        let written = std::fs::read_to_string(dir.path().join("docs/tech-debt.md")).unwrap();
        assert_eq!(written, "# T\n\nSet the token  first.\n");
        let listed = format!("{demoted:?}{:?}", demotion_notes(&demoted));
        assert!(!listed.contains(".env"), "{listed}");
        assert!(!listed.contains("token line"), "{listed}");
    }
}
