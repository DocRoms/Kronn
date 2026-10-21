//! Which docs a human validated, and which Kronn changed since (KT-1079).
//! A validation records the digest of what the person approved; a later write
//! is applied as usual and leaves the doc "to reconfirm" until confirmed.
//!
//! Stored as `docs/provenance.json`, next to `citations.json`, so a teammate
//! sees the same state after `git clone`. Keyed by doc path and section name:
//! it survives any rewrite and adds no marker to the prose. The validated text
//! itself (the "before" of the diff) stays local under `.kronn/audit/`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use ts_rs::TS;

use crate::api::audit::anti_hallu_enforce::{named_sections, NamedSection};

pub const PROVENANCE_FILENAME: &str = "provenance.json";

const README: &str = "Managed by Kronn. Which docs a human validated, as digests of the \
validated text. A doc whose text no longer matches is to reconfirm. Commit it with the docs.";

/// Per side of a diff sent to the UI.
const DIFF_CAP_CHARS: usize = 40_000;
const MAX_WRITES_KEPT: usize = 20;

/// Serializes the read-modify-write of the record across writers.
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ValidationReason {
    /// `validate-audit`: the whole doc set.
    AuditValidation,
    /// A TD decision taken on an audit validation card.
    ValidationCard,
    /// « Vérifié, inchangé ».
    Acknowledged,
    /// A revalidation (KT-1168) that left the docs as validated.
    Revalidation,
    /// « Confirmer » after a Kronn write.
    Confirmed,
    /// An `owner="human"` section never validated in Kronn: the text it had
    /// before the first Kronn write is the person's.
    HumanOwned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DocWriter {
    FullAudit,
    PartialAudit,
    Revalidation,
    TemplateReinstall,
    DocumentRepair,
    TdIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceScope {
    Doc,
    Sections,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KronnWrite {
    pub writer: DocWriter,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocRecord {
    pub validated_at: String,
    pub reason: ValidationReason,
    pub scope: ProvenanceScope,
    /// Digest of the whole doc when validated: names its kept original.
    pub doc_sha256: String,
    /// `Sections` scope: section name → digest of its validated block.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sections: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kronn_writes: Vec<KronnWrite>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProvenanceFile {
    #[serde(rename = "_readme", default)]
    pub readme: String,
    #[serde(default)]
    pub docs: BTreeMap<String, DocRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceState {
    HumanValidated,
    ToReconfirm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct ProvenanceChange {
    /// `None`: the whole doc.
    pub section: Option<String>,
    /// `None`: the validated text is not kept on this machine.
    pub before: Option<String>,
    /// `None`: the doc or section was removed.
    pub after: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct DocProvenanceStatus {
    pub doc: String,
    pub state: ProvenanceState,
    pub reason: ValidationReason,
    pub scope: ProvenanceScope,
    pub validated_at: String,
    /// The sections in scope (`sections` scope only).
    pub sections: Vec<String>,
    /// Kronn writes since the validation; empty when edited outside Kronn.
    pub kronn_writes: Vec<KronnWrite>,
    /// Filled on request, for docs to reconfirm only.
    pub changes: Vec<ProvenanceChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS, Default)]
#[ts(export)]
pub struct DocProvenanceReport {
    pub docs: Vec<DocProvenanceStatus>,
    #[ts(type = "number")]
    pub to_reconfirm: usize,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ConfirmDocsRequest {
    /// Docs to confirm; empty confirms every doc to reconfirm.
    #[serde(default)]
    pub docs: Vec<String>,
}

/// How a learning source counts (KT-1079 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SourceProvenance {
    HumanValidated,
    AiGenerated,
}

// ─── Store ────────────────────────────────────────────────────────────────

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn provenance_path(project: &Path) -> PathBuf {
    crate::core::scanner::detect_docs_dir(project).join(PROVENANCE_FILENAME)
}

/// Missing reads as empty; malformed is an error, so no writer rebuilds the
/// record from nothing and silently drops the other docs' validations.
pub fn read_strict(project: &Path) -> Result<ProvenanceFile, String> {
    let path = provenance_path(project);
    match std::fs::read_to_string(&path) {
        Ok(data) => {
            serde_json::from_str(&data).map_err(|e| format!("{} is malformed: {e}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ProvenanceFile::default()),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

fn write(project: &Path, file: &mut ProvenanceFile) -> Result<(), String> {
    let path = provenance_path(project);
    if file.docs.is_empty() && !path.exists() {
        return Ok(());
    }
    file.readme = README.to_string();
    let json = serde_json::to_string_pretty(file).map_err(|e| format!("JSON error: {e}"))?;
    crate::core::fs_guard::assert_contained_no_symlink(project, &path)?;
    crate::core::mcp_scanner::atomic_write(&path, &format!("{json}\n"))?;
    let keep: BTreeSet<String> = file.docs.values().map(|r| r.doc_sha256.clone()).collect();
    crate::core::audit_state::prune_validated_originals(project, &keep);
    Ok(())
}

/// A plain relative Markdown path inside the docs dir.
fn is_doc(project: &Path, doc: &str) -> bool {
    let rel = Path::new(doc);
    let plain = !doc.is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)));
    let docs_dir = crate::core::scanner::detect_docs_dir(project);
    plain
        && rel.extension().is_some_and(|e| e == "md")
        && project
            .join(rel)
            .strip_prefix(&docs_dir)
            .is_ok_and(|p| !p.as_os_str().is_empty())
}

fn read_doc(project: &Path, doc: &str) -> Option<String> {
    std::fs::read_to_string(project.join(doc)).ok()
}

fn digests(sections: &[NamedSection], human_only: bool) -> BTreeMap<String, String> {
    sections
        .iter()
        .filter(|s| !human_only || s.human)
        .map(|s| (s.name.clone(), sha256_hex(s.block.as_bytes())))
        .collect()
}

fn whole_doc_record(
    project: &Path,
    content: &str,
    reason: ValidationReason,
) -> Result<DocRecord, String> {
    crate::core::audit_state::preserve_validated_original(project, content)?;
    Ok(DocRecord {
        validated_at: now(),
        reason,
        scope: ProvenanceScope::Doc,
        doc_sha256: sha256_hex(content.as_bytes()),
        sections: BTreeMap::new(),
        kronn_writes: Vec::new(),
    })
}

fn sections_record(
    project: &Path,
    content: &str,
    names: &BTreeSet<String>,
    reason: ValidationReason,
) -> Result<Option<DocRecord>, String> {
    let sections: BTreeMap<String, String> = digests(&named_sections(content), false)
        .into_iter()
        .filter(|(name, _)| names.contains(name))
        .collect();
    if sections.is_empty() {
        return Ok(None);
    }
    crate::core::audit_state::preserve_validated_original(project, content)?;
    Ok(Some(DocRecord {
        validated_at: now(),
        reason,
        scope: ProvenanceScope::Sections,
        doc_sha256: sha256_hex(content.as_bytes()),
        sections,
        kronn_writes: Vec::new(),
    }))
}

// ─── Validation ───────────────────────────────────────────────────────────

/// A human validated these docs as they are now, whole.
pub fn record_validation(
    project: &Path,
    docs: &[String],
    reason: ValidationReason,
) -> Result<(), String> {
    if docs.is_empty() {
        return Ok(());
    }
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut file = read_strict(project)?;
    for doc in docs.iter().filter(|d| is_doc(project, d)) {
        if let Some(content) = read_doc(project, doc) {
            file.docs
                .insert(doc.clone(), whole_doc_record(project, &content, reason)?);
        }
    }
    write(project, &mut file)
}

/// A revalidation that found these docs unchanged re-dates their validation.
/// A doc already modified since stays to reconfirm: no edit is laundered.
pub fn refresh_validation(
    project: &Path,
    docs: &[String],
    reason: ValidationReason,
) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut file = read_strict(project)?;
    let mut changed = false;
    for doc in docs {
        let current = read_doc(project, doc);
        if let Some(record) = file.docs.get_mut(doc) {
            if !is_modified(record, current.as_deref()) {
                record.validated_at = now();
                record.reason = reason;
                record.kronn_writes.clear();
                changed = true;
            }
        }
    }
    if changed {
        write(project, &mut file)?;
    }
    Ok(())
}

/// `validate-audit`: every doc of the set is human-validated as it is now.
pub fn record_all(project: &Path, reason: ValidationReason) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut file = ProvenanceFile::default();
    for doc in crate::core::citation_drift::doc_files(project) {
        if let Some(content) = read_doc(project, &doc) {
            let record = whole_doc_record(project, &content, reason)?;
            file.docs.insert(doc, record);
        }
    }
    write(project, &mut file)
}

// ─── Kronn writes ─────────────────────────────────────────────────────────

fn push_write(record: &mut DocRecord, writer: DocWriter) {
    record.kronn_writes.retain(|w| w.writer != writer);
    record.kronn_writes.push(KronnWrite { writer, at: now() });
    if record.kronn_writes.len() > MAX_WRITES_KEPT {
        let excess = record.kronn_writes.len() - MAX_WRITES_KEPT;
        record.kronn_writes.drain(..excess);
    }
}

/// A Kronn writer changed `doc` from `before` to `after` (already on disk).
/// Never refuses: it only marks the doc to reconfirm when the change touches
/// what a human validated, or an `owner="human"` section. Returns whether it
/// marked the doc.
pub fn note_write(
    project: &Path,
    doc: &str,
    before: &str,
    after: &str,
    writer: DocWriter,
) -> Result<bool, String> {
    if before == after || !is_doc(project, doc) {
        return Ok(false);
    }
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut file = read_strict(project)?;
    let after_digests = digests(&named_sections(after), false);
    match file.docs.get_mut(doc) {
        Some(record) => {
            let touched = match record.scope {
                ProvenanceScope::Doc => true,
                ProvenanceScope::Sections => {
                    let before_digests = digests(&named_sections(before), false);
                    record
                        .sections
                        .keys()
                        .any(|name| before_digests.get(name) != after_digests.get(name))
                }
            };
            if !touched {
                return Ok(false);
            }
            push_write(record, writer);
        }
        None => {
            let human = digests(&named_sections(before), true);
            if human
                .iter()
                .all(|(name, digest)| after_digests.get(name) == Some(digest))
            {
                return Ok(false);
            }
            let all_human: BTreeSet<String> = human.into_keys().collect();
            let Some(mut record) =
                sections_record(project, before, &all_human, ValidationReason::HumanOwned)?
            else {
                return Ok(false);
            };
            push_write(&mut record, writer);
            file.docs.insert(doc.to_string(), record);
        }
    }
    write(project, &mut file)?;
    Ok(true)
}

/// The docs a writer may touch whose provenance matters: those with a record
/// or an `owner="human"` section, with their text before the write.
pub fn snapshot_tracked(project: &Path) -> Result<Vec<(String, String)>, String> {
    let recorded = read_strict(project)?;
    let mut out = Vec::new();
    for doc in crate::core::citation_drift::doc_files(project) {
        let Some(content) = read_doc(project, &doc) else {
            continue;
        };
        if recorded.docs.contains_key(&doc)
            || crate::api::audit::anti_hallu_enforce::contains_human_owned_section(&content)
        {
            out.push((doc, content));
        }
    }
    Ok(out)
}

/// After a writer ran: mark every tracked doc it changed. Returns how many.
pub fn note_snapshot_writes(
    project: &Path,
    snapshot: &[(String, String)],
    writer: DocWriter,
) -> Result<usize, String> {
    let mut marked = 0;
    let mut errors = Vec::new();
    for (doc, before) in snapshot {
        let after = read_doc(project, doc).unwrap_or_default();
        match note_write(project, doc, before, &after, writer) {
            Ok(true) => marked += 1,
            Ok(false) => {}
            Err(e) => errors.push(format!("{doc}: {e}")),
        }
    }
    if errors.is_empty() {
        Ok(marked)
    } else {
        Err(errors.join("; "))
    }
}

// ─── Status ───────────────────────────────────────────────────────────────

fn changed_sections(record: &DocRecord, current: &str) -> Vec<String> {
    let now = digests(&named_sections(current), false);
    record
        .sections
        .iter()
        .filter(|(name, digest)| now.get(*name) != Some(*digest))
        .map(|(name, _)| name.clone())
        .collect()
}

fn is_modified(record: &DocRecord, current: Option<&str>) -> bool {
    match (current, record.scope) {
        (None, _) => true,
        (Some(text), ProvenanceScope::Doc) => sha256_hex(text.as_bytes()) != record.doc_sha256,
        (Some(text), ProvenanceScope::Sections) => !changed_sections(record, text).is_empty(),
    }
}

fn capped(text: Option<String>) -> (Option<String>, bool) {
    match text {
        Some(t) if t.chars().count() > DIFF_CAP_CHARS => {
            (Some(t.chars().take(DIFF_CAP_CHARS).collect()), true)
        }
        other => (other, false),
    }
}

fn change(
    section: Option<String>,
    before: Option<String>,
    after: Option<String>,
) -> ProvenanceChange {
    let (before, cut_before) = capped(before);
    let (after, cut_after) = capped(after);
    ProvenanceChange {
        section,
        before,
        after,
        truncated: cut_before || cut_after,
    }
}

fn block_of(content: &str, name: &str) -> Option<String> {
    named_sections(content)
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| s.block)
}

fn evaluate(project: &Path, doc: &str, record: &DocRecord, detail: bool) -> DocProvenanceStatus {
    let current = read_doc(project, doc);
    let modified = is_modified(record, current.as_deref());
    let mut status = DocProvenanceStatus {
        doc: doc.to_string(),
        state: if modified {
            ProvenanceState::ToReconfirm
        } else {
            ProvenanceState::HumanValidated
        },
        reason: record.reason,
        scope: record.scope,
        validated_at: record.validated_at.clone(),
        sections: record.sections.keys().cloned().collect(),
        kronn_writes: if modified {
            record.kronn_writes.clone()
        } else {
            Vec::new()
        },
        changes: Vec::new(),
    };
    if !(detail && modified) {
        return status;
    }
    let original = crate::core::audit_state::read_validated_original(project, &record.doc_sha256);
    status.changes = match record.scope {
        ProvenanceScope::Doc => vec![change(None, original, current)],
        ProvenanceScope::Sections => {
            let names = match &current {
                Some(text) => changed_sections(record, text),
                None => record.sections.keys().cloned().collect(),
            };
            names
                .into_iter()
                .map(|name| {
                    let before = original.as_deref().and_then(|o| block_of(o, &name));
                    let after = current.as_deref().and_then(|c| block_of(c, &name));
                    change(Some(name), before, after)
                })
                .collect()
        }
    };
    status
}

/// Every recorded doc, human-validated or to reconfirm. `detail` adds the
/// before/after of each doc to reconfirm.
pub fn report(project: &Path, detail: bool) -> Result<DocProvenanceReport, String> {
    let file = read_strict(project)?;
    let docs: Vec<DocProvenanceStatus> = file
        .docs
        .iter()
        .map(|(doc, record)| evaluate(project, doc, record, detail))
        .collect();
    Ok(DocProvenanceReport {
        to_reconfirm: docs
            .iter()
            .filter(|d| d.state == ProvenanceState::ToReconfirm)
            .count(),
        docs,
    })
}

// ─── Confirm ──────────────────────────────────────────────────────────────

/// « Confirmer »: the docs as they are now become human-validated again, in
/// the same scope. An empty list confirms every doc to reconfirm. A doc that
/// no longer exists leaves the record.
pub fn confirm(project: &Path, docs: &[String]) -> Result<Vec<String>, String> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut file = read_strict(project)?;
    let targets: Vec<String> = if docs.is_empty() {
        file.docs
            .iter()
            .filter(|(doc, record)| is_modified(record, read_doc(project, doc).as_deref()))
            .map(|(doc, _)| doc.clone())
            .collect()
    } else {
        for doc in docs {
            if !file.docs.contains_key(doc) {
                return Err(format!("`{doc}` has no recorded human validation"));
            }
        }
        docs.to_vec()
    };
    for doc in &targets {
        let Some(record) = file.docs.get(doc) else {
            continue;
        };
        let replaced = match read_doc(project, doc) {
            None => None,
            Some(content) => match record.scope {
                ProvenanceScope::Doc => Some(whole_doc_record(
                    project,
                    &content,
                    ValidationReason::Confirmed,
                )?),
                ProvenanceScope::Sections => {
                    let names: BTreeSet<String> = record.sections.keys().cloned().collect();
                    sections_record(project, &content, &names, ValidationReason::Confirmed)?
                }
            },
        };
        match replaced {
            Some(record) => file.docs.insert(doc.clone(), record),
            None => file.docs.remove(doc),
        };
    }
    write(project, &mut file)?;
    Ok(targets)
}

// ─── Learning ─────────────────────────────────────────────────────────────

/// `docs/x.md:12`, `docs/x.md:3-5`, `docs/x.md` or an absolute path inside
/// the project, as a project-relative path and its first line.
fn parse_reference(project: &Path, reference: &str) -> Option<(String, Option<usize>)> {
    let reference = reference.trim();
    let (path, line) = match reference.rsplit_once(':') {
        Some((path, tail))
            if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit() || c == '-') =>
        {
            let start = tail.split('-').next().and_then(|s| s.parse::<usize>().ok());
            (path, start)
        }
        _ => (reference, None),
    };
    let rel = match Path::new(path).strip_prefix(project) {
        Ok(stripped) => stripped.to_string_lossy().into_owned(),
        Err(_) if Path::new(path).is_absolute() => return None,
        Err(_) => path.to_string(),
    };
    Some((rel.replace('\\', "/"), line))
}

/// How a learning source pointing into the docs counts: human-validated when
/// a person validated that text and nothing changed it since, AI-generated
/// otherwise. `None` for a source outside the docs.
pub fn source_provenance(project: &Path, reference: &str) -> Option<SourceProvenance> {
    let (doc, line) = parse_reference(project, reference)?;
    if !is_doc(project, &doc) {
        return None;
    }
    let content = read_doc(project, &doc)?;
    let sections = named_sections(&content);
    let line_in = |names: &dyn Fn(&NamedSection) -> bool| {
        line.is_some_and(|l| {
            sections
                .iter()
                .any(|s| names(s) && l > s.start && l <= s.end + 1)
        })
    };
    let file = read_strict(project).unwrap_or_default();
    let human = match file.docs.get(&doc) {
        Some(record) if is_modified(record, Some(&content)) => false,
        Some(record) => match record.scope {
            ProvenanceScope::Doc => true,
            ProvenanceScope::Sections => line_in(&|s| record.sections.contains_key(&s.name)),
        },
        None => line_in(&|s| s.human),
    };
    Some(if human {
        SourceProvenance::HumanValidated
    } else {
        SourceProvenance::AiGenerated
    })
}

#[cfg(test)]
#[path = "doc_provenance_test.rs"]
mod tests;
