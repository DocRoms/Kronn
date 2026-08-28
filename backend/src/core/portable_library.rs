//! Portable `.agents/` library contract.
//!
//! `SKILL.md` remains an unmodified Agent Skills document. Kronn-specific
//! metadata is stored next to it in `SKILL.kronn.json`; the other library
//! kinds use deterministic `*.kronn.json` documents.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const CONTRACT_VERSION: u32 = 1;

const WORKFLOW_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://kronn.local/schema/workflow.v1.schema.json",
  "title": "Kronn Portable Workflow v1",
  "type": "object",
  "additionalProperties": false,
  "required": ["version", "engine", "requires", "steps"],
  "properties": {
    "version": { "const": "1.0" },
    "engine": { "const": "kronn" },
    "requires": {
      "type": "array",
      "minItems": 1,
      "uniqueItems": true,
      "items": { "type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9._+-]*$" }
    },
    "inputs": {
      "type": "object",
      "propertyNames": { "pattern": "^[A-Za-z_][A-Za-z0-9_.-]*$" },
      "additionalProperties": {
        "type": "object",
        "additionalProperties": false,
        "required": ["type"],
        "properties": {
          "type": { "enum": ["boolean", "number", "string", "list"] },
          "description": { "type": "string" },
          "required": { "type": "boolean", "default": false },
          "default": {}
        }
      }
    },
    "env": {
      "type": "object",
      "propertyNames": { "pattern": "^[A-Z_][A-Z0-9_]*$" },
      "additionalProperties": {
        "type": "string",
        "pattern": "^\\$\\{env:[A-Z_][A-Z0-9_]*\\}$"
      }
    },
    "steps": {
      "type": "array",
      "minItems": 1,
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["name", "command"],
        "properties": {
          "name": { "type": "string", "minLength": 1 },
          "command": { "type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9._+-]*$" },
          "args": {
            "type": "array",
            "maxItems": 128,
            "items": { "type": "string", "not": { "pattern": "^/|^[^=]+=/(?:[^/]|$)" } }
          }
        }
      }
    }
  }
}
"#;

const RENDER_ENV_SCRIPT: &str = r#"#!/bin/sh
set -eu
if [ "$#" -ne 1 ]; then
  echo "Usage: $0 <workflow.kronn.json>" >&2
  exit 1
fi
exec kronn run "$1" --render-env
"#;

const BOOTSTRAP_SCRIPT: &str = r#"#!/bin/sh
set -eu
if [ "$#" -ne 1 ]; then
  echo "Usage: $0 <workflow.kronn.json>" >&2
  exit 1
fi
workflow=$1
workflow_dir=$(CDPATH= cd -- "$(dirname -- "$workflow")" && pwd)
kronn run "$workflow" --check
kronn run "$workflow" --render-env > "$workflow_dir/.env.example"
echo "Generated $workflow_dir/.env.example; create .env yourself after review."
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryKind {
    Skill,
    Directive,
    QuickPrompt,
    Workflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryScope {
    Global,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub scope: LibraryScope,
    /// Portable path relative to the `.agents` root.
    pub source: String,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KronnSidecar {
    pub version: u32,
    pub kind: LibraryKind,
    pub id: String,
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Typed input contract stored in a Quick Prompt's Kronn sidecar. The
/// third-party-facing `SKILL.md` deliberately stays a plain Agent Skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickPromptInputKind {
    Boolean,
    Number,
    String,
    List,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuickPromptInput {
    pub name: String,
    pub kind: QuickPromptInputKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowInputKind {
    Boolean,
    Number,
    String,
    List,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInputDef {
    #[serde(rename = "type")]
    pub kind: WorkflowInputKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStepDef {
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableWorkflow {
    pub version: String,
    pub engine: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, WorkflowInputDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    pub steps: Vec<WorkflowStepDef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedWorkflowStep {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPortableWorkflow {
    pub env: BTreeMap<String, String>,
    pub steps: Vec<RenderedWorkflowStep>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuickPromptSkillData {
    pub schema_version: u32,
    pub quick_prompt: crate::models::QuickPrompt,
    pub inputs: Vec<QuickPromptInput>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibraryItem {
    pub kind: LibraryKind,
    pub id: String,
    pub scope: LibraryScope,
    pub relative_path: PathBuf,
    pub content: Vec<u8>,
    pub sidecar: KronnSidecar,
    /// Skill resources other than `SKILL.md` and `SKILL.kronn.json`.
    pub auxiliary_files: BTreeMap<PathBuf, Vec<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LibraryCatalog {
    items: BTreeMap<(LibraryKind, String), LibraryItem>,
}

impl LibraryCatalog {
    pub fn items(&self) -> impl Iterator<Item = &LibraryItem> {
        self.items.values()
    }

    pub fn get(&self, kind: LibraryKind, id: &str) -> Option<&LibraryItem> {
        self.items.get(&(kind, id.to_string()))
    }

    pub fn search(&self, query: &str) -> Vec<&LibraryItem> {
        let query = query.to_lowercase();
        self.items
            .values()
            .filter(|item| {
                query.is_empty()
                    || item.id.to_lowercase().contains(&query)
                    || String::from_utf8_lossy(&item.content)
                        .to_lowercase()
                        .contains(&query)
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub created: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
    pub unchanged: Vec<PathBuf>,
}

impl SyncReport {
    pub fn changed(&self) -> bool {
        !(self.created.is_empty() && self.modified.is_empty() && self.deleted.is_empty())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedManifest {
    version: u32,
    files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockProvenance {
    Vendored,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockResource {
    pub kind: String,
    pub id: String,
    pub provenance: LockProvenance,
    pub version: String,
    pub source: String,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KronnLock {
    pub version: u32,
    pub resources: Vec<LockResource>,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockApprovals {
    version: u32,
    canonical_project_sha256: String,
    approved_lock_sha256: String,
}

/// Discover global and project libraries. A project item deterministically
/// overrides a global item with the same `(kind, id)`; duplicates inside one
/// scope are rejected instead of depending on filesystem iteration order.
pub fn discover(
    global_root: Option<&Path>,
    project_root: Option<&Path>,
) -> Result<LibraryCatalog, String> {
    let mut catalog = LibraryCatalog::default();
    if let Some(root) = global_root {
        merge_scope(&mut catalog, scan_root(root, LibraryScope::Global)?)?;
    }
    if let Some(root) = project_root {
        merge_scope(&mut catalog, scan_root(root, LibraryScope::Project)?)?;
    }
    Ok(catalog)
}

fn merge_scope(catalog: &mut LibraryCatalog, items: Vec<LibraryItem>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for item in items {
        let key = (item.kind, item.id.clone());
        if !seen.insert(key.clone()) {
            return Err(format!(
                "duplicate {:?} id '{}' in {:?} library",
                item.kind, item.id, item.scope
            ));
        }
        catalog.items.insert(key, item);
    }
    Ok(())
}

fn scan_root(root: &Path, scope: LibraryScope) -> Result<Vec<LibraryItem>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("cannot resolve library root: {e}"))?;
    let mut out = Vec::new();
    scan_skills(&root, scope, &mut out)?;
    scan_json_kind(&root, scope, LibraryKind::Directive, "directives", &mut out)?;
    scan_legacy_quick_prompts(&root, scope, &mut out)?;
    scan_json_kind(&root, scope, LibraryKind::Workflow, "workflows", &mut out)?;
    Ok(out)
}

fn scan_skills(root: &Path, scope: LibraryScope, out: &mut Vec<LibraryItem>) -> Result<(), String> {
    let dir = root.join("skills");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(());
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let folder = entry.path();
        if !folder.is_dir() {
            continue;
        }
        let skill_path = folder.join("SKILL.md");
        if !skill_path.is_file() {
            continue;
        }
        let content = fs::read(&skill_path).map_err(|e| format!("cannot read skill: {e}"))?;
        let relative = skill_path
            .strip_prefix(root)
            .map_err(|_| "skill escaped library root")?
            .to_path_buf();
        let id = entry.file_name().to_string_lossy().to_string();
        validate_id(&id)?;
        validate_skill(&content, &id)?;
        let sidecar_path = folder.join("SKILL.kronn.json");
        let sidecar = read_or_derive_sidecar(&sidecar_path, &id, scope, &relative, &content)?;
        if sidecar.kind == LibraryKind::QuickPrompt {
            let data = quick_prompt_data(&sidecar)?;
            if data.quick_prompt.id != id {
                return Err(format!("Quick Prompt sidecar does not match '{id}'"));
            }
            validate_quick_prompt_inputs(&data.inputs)?;
        }
        if scope == LibraryScope::Project && sidecar.provenance.scope == LibraryScope::Global {
            continue;
        }
        let mut auxiliary_files = BTreeMap::new();
        collect_auxiliary_files(&folder, &folder, &mut auxiliary_files)?;
        auxiliary_files.remove(Path::new("SKILL.md"));
        auxiliary_files.remove(Path::new("SKILL.kronn.json"));
        out.push(LibraryItem {
            kind: sidecar.kind,
            id,
            scope,
            relative_path: relative,
            content,
            sidecar,
            auxiliary_files,
        });
    }
    Ok(())
}

fn scan_json_kind(
    root: &Path,
    scope: LibraryScope,
    kind: LibraryKind,
    folder: &str,
    out: &mut Vec<LibraryItem>,
) -> Result<(), String> {
    let dir = root.join(folder);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(());
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !path.is_file() || !name.ends_with(".kronn.json") {
            continue;
        }
        let content = fs::read(&path).map_err(|e| format!("cannot read {folder} item: {e}"))?;
        reject_secrets(&content)?;
        let mut sidecar: KronnSidecar =
            serde_json::from_slice(&content).map_err(|e| format!("invalid {name}: {e}"))?;
        if sidecar.version != CONTRACT_VERSION || sidecar.kind != kind {
            return Err(format!("invalid kind or version in {name}"));
        }
        validate_id(&sidecar.id)?;
        if scope == LibraryScope::Project && sidecar.provenance.scope == LibraryScope::Global {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "item escaped library root")?
            .to_path_buf();
        validate_portable_source(&sidecar.provenance.source)?;
        // Provenance records origin, not the tree it was found in: keep the
        // recorded scope so a global item synced into a project is not promoted
        // to project on rediscovery. Refresh the portable path and hash the
        // payload (excluding provenance) so resync is a stable no-op.
        sidecar.provenance.source = portable_path(&relative)?;
        sidecar.provenance.content_sha256 = sidecar_payload_sha256(&sidecar)?;
        out.push(LibraryItem {
            kind,
            id: sidecar.id.clone(),
            scope,
            relative_path: relative,
            content,
            sidecar,
            auxiliary_files: BTreeMap::new(),
        });
    }
    Ok(())
}

fn scan_legacy_quick_prompts(
    root: &Path,
    scope: LibraryScope,
    out: &mut Vec<LibraryItem>,
) -> Result<(), String> {
    let dir = root.join("quick-prompts");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(());
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = fs::read(&path).map_err(|e| format!("cannot read legacy QP: {e}"))?;
        reject_secrets(&raw)?;
        let sidecar: KronnSidecar =
            serde_json::from_slice(&raw).map_err(|e| format!("invalid legacy QP: {e}"))?;
        let Some(id) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".kronn.json"))
            .map(str::to_owned)
        else {
            continue;
        };
        if sidecar.version != CONTRACT_VERSION
            || sidecar.kind != LibraryKind::QuickPrompt
            || sidecar.id != id
        {
            return Err(format!("legacy QP does not match '{id}'"));
        }
        validate_portable_source(&sidecar.provenance.source)?;
        if scope == LibraryScope::Project && sidecar.provenance.scope == LibraryScope::Global {
            continue;
        }
        // A materialized skill wins over its legacy source. This makes the
        // migration idempotent after the first sync.
        if root.join("skills").join(&id).join("SKILL.md").is_file() {
            continue;
        }
        let data = sidecar
            .data
            .clone()
            .ok_or_else(|| format!("legacy QP '{id}' has no data"))?;
        let (prompt, inputs) = match serde_json::from_value::<QuickPromptSkillData>(data.clone()) {
            Ok(portable) => (portable.quick_prompt, portable.inputs),
            Err(_) => {
                let prompt: crate::models::QuickPrompt = serde_json::from_value(data)
                    .map_err(|e| format!("invalid legacy QP '{id}': {e}"))?;
                let inputs = legacy_inputs(&prompt);
                (prompt, inputs)
            }
        };
        let mut item =
            quick_prompt_to_skill_with_inputs(&prompt, inputs, sidecar.provenance.scope)?;
        item.scope = scope;
        out.push(item);
    }
    Ok(())
}
fn read_or_derive_sidecar(
    path: &Path,
    id: &str,
    scope: LibraryScope,
    relative: &Path,
    content: &[u8],
) -> Result<KronnSidecar, String> {
    let source = portable_path(relative)?;
    // A skill's content hash is over `SKILL.md`, which never embeds the hash, so
    // it is already stable across resync.
    let content_sha256 = sha256(content);
    if !path.exists() {
        return Ok(KronnSidecar {
            version: CONTRACT_VERSION,
            kind: LibraryKind::Skill,
            id: id.to_string(),
            provenance: Provenance {
                scope,
                source,
                content_sha256,
            },
            data: None,
        });
    }
    let raw = fs::read(path).map_err(|e| format!("cannot read skill sidecar: {e}"))?;
    reject_secrets(&raw)?;
    let mut sidecar: KronnSidecar =
        serde_json::from_slice(&raw).map_err(|e| format!("invalid skill sidecar: {e}"))?;
    if sidecar.version != CONTRACT_VERSION || sidecar.id != id {
        return Err(format!("skill sidecar does not match '{id}'"));
    }
    if sidecar.kind != LibraryKind::Skill && sidecar.kind != LibraryKind::QuickPrompt {
        return Err(format!("skill sidecar has invalid kind {:?}", sidecar.kind));
    }
    validate_portable_source(&sidecar.provenance.source)?;
    // Keep the recorded origin scope (no global->project promotion); refresh the
    // portable path and content hash from the current tree.
    sidecar.provenance.source = source;
    sidecar.provenance.content_sha256 = content_sha256;
    Ok(sidecar)
}
/// Materialize the effective catalog into `target/.agents`. Only files listed
/// in the previous managed manifest can be deleted; unrelated user files and
/// Agent Skills auxiliary resources are preserved.
pub fn sync(catalog: &LibraryCatalog, target: &Path) -> Result<SyncReport, String> {
    let root = target.join(".agents");
    fs::create_dir_all(&root).map_err(|e| format!("cannot create .agents: {e}"))?;
    let manifest_path = root.join(".kronn-sync.json");
    let previous = read_manifest(&manifest_path)?;
    let mut desired = BTreeMap::<String, Vec<u8>>::new();
    for item in catalog.items() {
        let base = export_relative_path(item);
        desired.insert(
            portable_path(&base)?,
            if uses_skill_layout(item) {
                item.content.clone()
            } else {
                canonical_json(&item.sidecar)?
            },
        );
        if uses_skill_layout(item) {
            let sidecar_path = base.parent().unwrap().join("SKILL.kronn.json");
            desired.insert(
                portable_path(&sidecar_path)?,
                canonical_json(&item.sidecar)?,
            );
            for (path, bytes) in &item.auxiliary_files {
                validate_relative(path)?;
                desired.insert(
                    portable_path(&base.parent().unwrap().join(path))?,
                    bytes.clone(),
                );
            }
        }
    }
    insert_workflow_router_assets(catalog, &mut desired)?;
    let lock_bytes = canonical_json(&build_lock(catalog, &desired))?;
    let mut report = SyncReport::default();
    let mut hashes = BTreeMap::new();
    for (relative, bytes) in &desired {
        reject_secrets(bytes)?;
        let path = root.join(relative);
        ensure_inside(&root, &path)?;
        let existed = path.exists();
        let same = existed
            && fs::read(&path).map_err(|e| format!("cannot read sync target: {e}"))? == *bytes;
        if same {
            report.unchanged.push(PathBuf::from(relative));
        } else {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create sync directory: {e}"))?;
            }
            crate::core::mcp_scanner::atomic_write_bytes(&path, bytes)
                .map_err(|e| format!("cannot sync {relative}: {e}"))?;
            if existed {
                report.modified.push(PathBuf::from(relative));
            } else {
                report.created.push(PathBuf::from(relative));
            }
        }
        hashes.insert(relative.clone(), sha256(bytes));
    }
    let lock_path = root.join("kronn.lock");
    let lock_existed = lock_path.exists();
    if fs::read(&lock_path).ok().as_deref() == Some(lock_bytes.as_slice()) {
        report.unchanged.push(PathBuf::from("kronn.lock"));
    } else {
        crate::core::mcp_scanner::atomic_write_bytes(&lock_path, &lock_bytes)
            .map_err(|e| format!("cannot write kronn.lock: {e}"))?;
        if lock_existed {
            report.modified.push(PathBuf::from("kronn.lock"));
        } else {
            report.created.push(PathBuf::from("kronn.lock"));
        }
    }
    for relative in previous
        .files
        .keys()
        .filter(|path| !desired.contains_key(*path))
    {
        validate_portable_source(relative)?;
        let path = root.join(relative);
        ensure_inside(&root, &path)?;
        if path.is_file() {
            fs::remove_file(&path).map_err(|e| format!("cannot delete stale managed file: {e}"))?;
            report.deleted.push(PathBuf::from(relative));
        }
    }
    let manifest = ManagedManifest {
        version: CONTRACT_VERSION,
        files: hashes,
    };
    let manifest_bytes = canonical_json(&manifest)?;
    let current = fs::read(&manifest_path).ok();
    if current.as_deref() != Some(manifest_bytes.as_slice()) {
        crate::core::mcp_scanner::atomic_write_bytes(&manifest_path, &manifest_bytes)
            .map_err(|e| format!("cannot write sync manifest: {e}"))?;
    }
    Ok(report)
}

fn build_lock(catalog: &LibraryCatalog, desired: &BTreeMap<String, Vec<u8>>) -> KronnLock {
    let mut resources: Vec<_> = catalog
        .items()
        .map(|item| LockResource {
            kind: format!("{:?}", item.kind).to_ascii_lowercase(),
            id: item.id.clone(),
            provenance: match item.sidecar.provenance.scope {
                LibraryScope::Global => LockProvenance::Vendored,
                LibraryScope::Project => LockProvenance::Local,
            },
            version: item.sidecar.version.to_string(),
            source: item.sidecar.provenance.source.clone(),
            content_sha256: item.sidecar.provenance.content_sha256.clone(),
        })
        .collect();
    resources.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    KronnLock {
        version: CONTRACT_VERSION,
        resources,
        files: desired
            .iter()
            .map(|(path, bytes)| (path.clone(), sha256(bytes)))
            .collect(),
    }
}

pub fn check_frozen_hash(project_root: &Path) -> Result<KronnLock, String> {
    let root = project_root.join(".agents");
    let lock_path = root.join("kronn.lock");
    let lock: KronnLock = serde_json::from_slice(
        &fs::read(&lock_path).map_err(|e| format!("cannot read '{}': {e}", lock_path.display()))?,
    )
    .map_err(|e| format!("invalid kronn.lock: {e}"))?;
    if lock.version != CONTRACT_VERSION {
        return Err("unsupported kronn.lock version".into());
    }
    let actual = collect_locked_files(&root, &root)?;
    let expected: BTreeSet<_> = lock.files.keys().cloned().collect();
    let present: BTreeSet<_> = actual.keys().cloned().collect();
    let added: Vec<_> = present.difference(&expected).cloned().collect();
    let removed: Vec<_> = expected.difference(&present).cloned().collect();
    let altered: Vec<_> = expected
        .intersection(&present)
        .filter(|path| actual.get(*path) != lock.files.get(*path))
        .cloned()
        .collect();
    if !added.is_empty() || !removed.is_empty() || !altered.is_empty() {
        return Err(format!(
            "frozen hash mismatch (added: {}; removed: {}; altered: {})",
            added.join(", "),
            removed.join(", "),
            altered.join(", ")
        ));
    }
    Ok(lock)
}

fn collect_locked_files(root: &Path, directory: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(directory)
        .map_err(|e| format!("cannot inspect '{}': {e}", directory.display()))?
    {
        let entry = entry.map_err(|e| format!("cannot inspect .agents entry: {e}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("cannot inspect .agents entry: {e}"))?;
        if file_type.is_symlink() {
            return Err(format!("frozen hash refuses symlink '{}'", path.display()));
        }
        if file_type.is_dir() {
            files.extend(collect_locked_files(root, &path)?);
        } else if file_type.is_file() {
            let relative = portable_path(
                path.strip_prefix(root)
                    .map_err(|_| "file escaped .agents")?,
            )?;
            if matches!(relative.as_str(), "kronn.lock" | ".kronn-sync.json") {
                continue;
            }
            files.insert(
                relative,
                sha256(
                    &fs::read(&path)
                        .map_err(|e| format!("cannot hash '{}': {e}", path.display()))?,
                ),
            );
        }
    }
    Ok(files)
}

pub fn approve_lock(project_root: &Path) -> Result<(), String> {
    let trust_dir = crate::core::config::config_dir()
        .map_err(|e| format!("cannot determine Kronn config directory: {e}"))?
        .join("trust/portable-library");
    approve_lock_at(project_root, &trust_dir)
}

fn canonical_project_identity(project_root: &Path) -> Result<String, String> {
    let canonical = project_root.canonicalize().map_err(|e| {
        format!(
            "cannot resolve project root '{}': {e}",
            project_root.display()
        )
    })?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::ffi::OsStrExt;
        sha256(canonical.as_os_str().as_bytes())
    };
    #[cfg(windows)]
    let identity = {
        use std::os::windows::ffi::OsStrExt;
        let bytes: Vec<u8> = canonical
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect();
        sha256(&bytes)
    };
    Ok(identity)
}

fn approval_path(project_root: &Path, trust_dir: &Path) -> Result<(PathBuf, String), String> {
    let identity = canonical_project_identity(project_root)?;
    Ok((trust_dir.join(format!("{identity}.json")), identity))
}

fn approve_lock_at(project_root: &Path, trust_dir: &Path) -> Result<(), String> {
    check_frozen_hash(project_root)?;
    let root = project_root.join(".agents");
    let (approval_path, canonical_project_sha256) = approval_path(project_root, trust_dir)?;
    let approval = LockApprovals {
        version: CONTRACT_VERSION,
        canonical_project_sha256,
        approved_lock_sha256: sha256(
            &fs::read(root.join("kronn.lock"))
                .map_err(|e| format!("cannot read kronn.lock: {e}"))?,
        ),
    };
    fs::create_dir_all(trust_dir)
        .map_err(|e| format!("cannot create TOFU trust directory: {e}"))?;
    crate::core::mcp_scanner::atomic_write_bytes(&approval_path, &canonical_json(&approval)?)
        .map_err(|e| format!("cannot record TOFU approval: {e}"))
}

fn require_lock_approval(project_root: &Path) -> Result<(), String> {
    let trust_dir = crate::core::config::config_dir()
        .map_err(|e| format!("cannot determine Kronn config directory: {e}"))?
        .join("trust/portable-library");
    require_lock_approval_at(project_root, &trust_dir)
}

fn require_lock_approval_at(project_root: &Path, trust_dir: &Path) -> Result<(), String> {
    check_frozen_hash(project_root)?;
    let root = project_root.join(".agents");
    let (approval_path, canonical_project_sha256) = approval_path(project_root, trust_dir)?;
    let approval: LockApprovals =
        serde_json::from_slice(&fs::read(approval_path).map_err(|_| {
            "TOFU approval required; run 'kronn check --approve' first".to_string()
        })?)
        .map_err(|e| format!("invalid TOFU approval: {e}"))?;
    let lock_hash = sha256(
        &fs::read(root.join("kronn.lock")).map_err(|e| format!("cannot read kronn.lock: {e}"))?,
    );
    if approval.version != CONTRACT_VERSION
        || approval.canonical_project_sha256 != canonical_project_sha256
        || approval.approved_lock_sha256 != lock_hash
    {
        return Err(
            "TOFU approval is stale or belongs to a different project; run 'kronn check --approve' again"
                .into(),
        );
    }
    Ok(())
}

pub fn run_cli_check(args: &[String]) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    match args {
        [flag] if flag == "--frozen-hash" => check_frozen_hash(&cwd).map(|_| ()),
        [flag] if flag == "--approve" => approve_lock(&cwd),
        _ => Err("Usage: kronn check --frozen-hash | --approve".into()),
    }
}

/// CLI entry point for `kronn sync`. The global source is
/// `<Kronn config dir>/.agents`; the project source and destination are the
/// current working directory's `.agents` tree.
pub fn run_cli_sync() -> Result<SyncReport, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot read current directory: {e}"))?;
    let global = crate::core::config::config_dir()
        .map_err(|e| format!("cannot determine Kronn config directory: {e}"))?
        .join(".agents");
    let project = cwd.join(".agents");
    let catalog = discover(Some(&global), Some(&project))?;
    sync(&catalog, &cwd)
}

fn export_relative_path(item: &LibraryItem) -> PathBuf {
    match item.kind {
        LibraryKind::Skill => PathBuf::from("skills").join(&item.id).join("SKILL.md"),
        LibraryKind::Directive => {
            PathBuf::from("directives").join(format!("{}.kronn.json", item.id))
        }
        LibraryKind::QuickPrompt => PathBuf::from("skills").join(&item.id).join("SKILL.md"),
        LibraryKind::Workflow => PathBuf::from("workflows").join(format!("{}.kronn.json", item.id)),
    }
}

fn uses_skill_layout(item: &LibraryItem) -> bool {
    matches!(item.kind, LibraryKind::Skill | LibraryKind::QuickPrompt)
}

fn insert_workflow_router_assets(
    catalog: &LibraryCatalog,
    desired: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    if catalog.get(LibraryKind::Skill, "kronn").is_some()
        || catalog.get(LibraryKind::QuickPrompt, "kronn").is_some()
    {
        return Err("'kronn' is reserved for the generated workflow router skill".into());
    }
    let router = render_workflow_router(catalog).into_bytes();
    validate_skill(&router, "kronn")?;
    let router_path = PathBuf::from("skills/kronn/SKILL.md");
    let sidecar = KronnSidecar {
        version: CONTRACT_VERSION,
        kind: LibraryKind::Skill,
        id: "kronn".into(),
        provenance: Provenance {
            scope: LibraryScope::Global,
            source: portable_path(&router_path)?,
            content_sha256: sha256(&router),
        },
        data: None,
    };
    desired.insert(portable_path(&router_path)?, router);
    desired.insert(
        "skills/kronn/SKILL.kronn.json".into(),
        canonical_json(&sidecar)?,
    );
    desired.insert(
        "schema/workflow.v1.schema.json".into(),
        WORKFLOW_SCHEMA.as_bytes().to_vec(),
    );
    desired.insert(
        "scripts/render-env.sh".into(),
        RENDER_ENV_SCRIPT.as_bytes().to_vec(),
    );
    desired.insert(
        "scripts/bootstrap.sh".into(),
        BOOTSTRAP_SCRIPT.as_bytes().to_vec(),
    );
    Ok(())
}

fn render_workflow_router(catalog: &LibraryCatalog) -> String {
    let workflows: Vec<_> = catalog
        .items()
        .filter(|item| item.kind == LibraryKind::Workflow)
        .map(|item| item.id.as_str())
        .collect();
    let inventory = if workflows.is_empty() {
        "- No portable workflow is currently installed.\n".to_string()
    } else {
        workflows
            .iter()
            .map(|id| format!("- `{id}` — `.agents/workflows/{id}.kronn.json`\n"))
            .collect()
    };
    format!(
        "---\nname: kronn\ndescription: Route validated portable Kronn workflows from the project .agents library.\n---\n\n# Kronn workflow router\n\n## Installed workflows\n\n{inventory}\n## Validate, approve, and run\n\nVerify the committed lock without changing files (suitable for a pre-commit hook):\n\n```sh\nkronn check --frozen-hash\n```\n\nTrust the current lock on first use or after a reviewed hash change:\n\n```sh\nkronn check --approve\n```\n\nValidate without executing:\n\n```sh\nkronn run .agents/workflows/<id>.kronn.json --check --var name=value\n```\n\nExecution is always explicit and uses literal argv, never a shell:\n\n```sh\nkronn run .agents/workflows/<id>.kronn.json --var name=value --allow-exec\n```\n\nGenerate the secret-free environment template with `sh .agents/scripts/render-env.sh <workflow>`. Bootstrap validation with `sh .agents/scripts/bootstrap.sh <workflow>`.\n\n## Container fallback\n\n```sh\ndocker run --rm -v \"$PWD:/workspace\" -w /workspace kronn:0.12.0 kronn run .agents/workflows/<id>.kronn.json --check --var name=value\n```\n"
    )
}

/// Convert an existing Quick Prompt to the portable Agent Skill layout. Legacy
/// variables become required string inputs, preserving their previous runtime
/// semantics while allowing new sidecars to opt into richer types/defaults.
pub fn quick_prompt_to_skill(
    prompt: &crate::models::QuickPrompt,
    scope: LibraryScope,
) -> Result<LibraryItem, String> {
    quick_prompt_to_skill_with_inputs(prompt, legacy_inputs(prompt), scope)
}

pub fn quick_prompt_to_skill_with_inputs(
    prompt: &crate::models::QuickPrompt,
    inputs: Vec<QuickPromptInput>,
    scope: LibraryScope,
) -> Result<LibraryItem, String> {
    validate_id(&prompt.id)?;
    validate_quick_prompt_inputs(&inputs)?;
    let declared: BTreeSet<_> = inputs.iter().map(|input| input.name.as_str()).collect();
    for variable in &prompt.variables {
        if !declared.contains(variable.name.as_str()) {
            return Err(format!(
                "Quick Prompt variable '{}' has no portable input declaration",
                variable.name
            ));
        }
    }

    let description = if prompt.description.trim().is_empty() {
        format!("Kronn Quick Prompt: {}", prompt.name.trim())
    } else {
        prompt.description.replace(['\r', '\n'], " ")
    };
    let description: String = description.chars().take(1024).collect();
    let yaml_description = serde_json::to_string(&description)
        .map_err(|e| format!("cannot encode Quick Prompt description: {e}"))?;
    let content = format!(
        "---\nname: {}\ndescription: {}\n---\n{}",
        prompt.id, yaml_description, prompt.prompt_template
    )
    .into_bytes();
    validate_skill(&content, &prompt.id)?;

    let relative_path = PathBuf::from("skills").join(&prompt.id).join("SKILL.md");
    let data = QuickPromptSkillData {
        schema_version: CONTRACT_VERSION,
        quick_prompt: prompt.clone(),
        inputs,
    };
    let mut sidecar = KronnSidecar {
        version: CONTRACT_VERSION,
        kind: LibraryKind::QuickPrompt,
        id: prompt.id.clone(),
        provenance: Provenance {
            scope,
            source: portable_path(&relative_path)?,
            content_sha256: sha256(&content),
        },
        data: Some(
            serde_json::to_value(data)
                .map_err(|e| format!("cannot encode Quick Prompt sidecar: {e}"))?,
        ),
    };
    // Keep the same semantic-payload hash convention as every JSON sidecar.
    // The SKILL.md hash remains available after discovery in `provenance`.
    sidecar.provenance.content_sha256 = sha256(&content);
    Ok(LibraryItem {
        kind: LibraryKind::QuickPrompt,
        id: prompt.id.clone(),
        scope,
        relative_path,
        content,
        sidecar,
        auxiliary_files: BTreeMap::new(),
    })
}

/// Reimport a Quick Prompt skill. The editable `SKILL.md` body is authoritative
/// for the prompt text; all other legacy bindings round-trip through the
/// sidecar's complete `QuickPrompt` snapshot.
pub fn quick_prompt_from_skill(item: &LibraryItem) -> Result<crate::models::QuickPrompt, String> {
    if item.kind != LibraryKind::QuickPrompt {
        return Err("portable item is not a Quick Prompt".into());
    }
    validate_skill(&item.content, &item.id)?;
    let data = quick_prompt_data(&item.sidecar)?;
    validate_quick_prompt_inputs(&data.inputs)?;
    if data.quick_prompt.id != item.id {
        return Err(format!("Quick Prompt sidecar does not match '{}'", item.id));
    }
    let mut prompt = data.quick_prompt;
    prompt.prompt_template = skill_body(&item.content)?.to_string();
    Ok(prompt)
}

/// Render a parameterized Quick Prompt in one non-recursive pass. Literal
/// placeholder-shaped text uses `{{{{name}}}}`; inserted values are never
/// parsed again, so a value containing braces cannot create a second variable.
pub fn render_quick_prompt_skill(
    item: &LibraryItem,
    supplied: &BTreeMap<String, Value>,
) -> Result<String, String> {
    if item.kind != LibraryKind::QuickPrompt {
        return Err("portable item is not a Quick Prompt".into());
    }
    let data = quick_prompt_data(&item.sidecar)?;
    validate_quick_prompt_inputs(&data.inputs)?;
    let inputs: BTreeMap<_, _> = data
        .inputs
        .iter()
        .map(|input| (input.name.as_str(), input))
        .collect();
    for name in supplied.keys() {
        if !inputs.contains_key(name.as_str()) {
            return Err(format!("unknown Quick Prompt variable '{name}'"));
        }
    }

    let mut values = BTreeMap::<&str, String>::new();
    for input in &data.inputs {
        let value = supplied.get(&input.name).or(input.default.as_ref());
        match value {
            Some(value) => {
                validate_input_value(input, value)?;
                if input.required
                    && ((input.kind == QuickPromptInputKind::String
                        && value.as_str().unwrap().trim().is_empty())
                        || (input.kind == QuickPromptInputKind::List
                            && value.as_array().unwrap().is_empty()))
                {
                    return Err(format!(
                        "missing required Quick Prompt variable '{}'",
                        input.name
                    ));
                }
                values.insert(&input.name, render_input_value(input.kind, value)?);
            }
            None if input.required => {
                return Err(format!(
                    "missing required Quick Prompt variable '{}'",
                    input.name
                ));
            }
            None => {
                values.insert(&input.name, String::new());
            }
        }
    }
    for variable in &data.quick_prompt.variables {
        let Some(pattern) = variable
            .pattern
            .as_deref()
            .filter(|pattern| !pattern.trim().is_empty())
        else {
            continue;
        };
        let Some(value) = values.get(variable.name.as_str()) else {
            continue;
        };
        match regex_lite::Regex::new(&format!("^(?:{pattern})$")) {
            Ok(regex) if !value.is_empty() && !regex.is_match(value) => {
                return Err(format!(
                    "Quick Prompt variable '{}' does not match its pattern",
                    variable.name
                ));
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(
                "Quick Prompt variable '{}' has an invalid pattern '{}' ({}); skipping shape check",
                variable.name,
                pattern,
                error
            ),
        }
    }
    render_template(skill_body(&item.content)?, &values)
}

fn legacy_inputs(prompt: &crate::models::QuickPrompt) -> Vec<QuickPromptInput> {
    prompt
        .variables
        .iter()
        .map(|variable| QuickPromptInput {
            name: variable.name.clone(),
            kind: QuickPromptInputKind::String,
            required: variable.required,
            default: None,
        })
        .collect()
}

fn quick_prompt_data(sidecar: &KronnSidecar) -> Result<QuickPromptSkillData, String> {
    if sidecar.kind != LibraryKind::QuickPrompt {
        return Err("sidecar is not a Quick Prompt".into());
    }
    let data = sidecar
        .data
        .clone()
        .ok_or("Quick Prompt sidecar has no data")?;
    let data: QuickPromptSkillData = serde_json::from_value(data)
        .map_err(|e| format!("invalid Quick Prompt sidecar data: {e}"))?;
    if data.schema_version != CONTRACT_VERSION {
        return Err(format!(
            "unsupported Quick Prompt schema version {}",
            data.schema_version
        ));
    }
    Ok(data)
}

fn validate_quick_prompt_inputs(inputs: &[QuickPromptInput]) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for input in inputs {
        if input.name.is_empty()
            || input.name.len() > 128
            || !input.name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
            })
        {
            return Err(format!("invalid Quick Prompt input name '{}'", input.name));
        }
        if !names.insert(&input.name) {
            return Err(format!("duplicate Quick Prompt input '{}'", input.name));
        }
        if let Some(default) = &input.default {
            validate_input_value(input, default).map_err(|error| {
                format!(
                    "invalid default for Quick Prompt input '{}': {error}",
                    input.name
                )
            })?;
        }
    }
    Ok(())
}

fn validate_input_value(input: &QuickPromptInput, value: &Value) -> Result<(), String> {
    let valid = match input.kind {
        QuickPromptInputKind::Boolean => value.is_boolean(),
        QuickPromptInputKind::Number => value.is_number(),
        QuickPromptInputKind::String => value.is_string(),
        QuickPromptInputKind::List => value.is_array(),
    };
    valid.then_some(()).ok_or_else(|| {
        format!(
            "Quick Prompt variable '{}' must be {:?}",
            input.name, input.kind
        )
    })
}

fn render_input_value(kind: QuickPromptInputKind, value: &Value) -> Result<String, String> {
    match kind {
        QuickPromptInputKind::String => Ok(value.as_str().unwrap().to_string()),
        QuickPromptInputKind::Boolean | QuickPromptInputKind::Number => Ok(value.to_string()),
        QuickPromptInputKind::List => serde_json::to_string(value)
            .map_err(|e| format!("cannot render Quick Prompt list: {e}")),
    }
}

fn render_template(template: &str, values: &BTreeMap<&str, String>) -> Result<String, String> {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        output.push_str(&rest[..open]);
        rest = &rest[open..];
        if let Some(literal) = rest.strip_prefix("{{{{") {
            let close = literal
                .find("}}}}")
                .ok_or("unclosed escaped Quick Prompt placeholder")?;
            output.push_str("{{");
            output.push_str(&literal[..close]);
            output.push_str("}}");
            rest = &literal[close + 4..];
            continue;
        }
        let variable = &rest[2..];
        let close = variable
            .find("}}")
            .ok_or("unclosed Quick Prompt placeholder")?;
        let name = variable[..close].trim();
        if name.is_empty() || name != &variable[..close] {
            return Err("Quick Prompt placeholders must be non-empty and unpadded".into());
        }
        let value = values
            .get(name)
            .ok_or_else(|| format!("unknown Quick Prompt variable '{name}'"))?;
        output.push_str(value);
        rest = &variable[close + 2..];
    }
    output.push_str(rest);
    Ok(output)
}

/// Parse either a bare portable workflow document or the `data` payload of a
/// standard Kronn workflow sidecar, then apply the executable Rust contract.
pub fn parse_portable_workflow(content: &[u8]) -> Result<PortableWorkflow, String> {
    reject_secrets(content)?;
    let value: Value =
        serde_json::from_slice(content).map_err(|e| format!("invalid workflow JSON: {e}"))?;
    let workflow_value = if value.get("kind").is_some() || value.get("provenance").is_some() {
        let sidecar: KronnSidecar =
            serde_json::from_value(value).map_err(|e| format!("invalid workflow sidecar: {e}"))?;
        if sidecar.version != CONTRACT_VERSION || sidecar.kind != LibraryKind::Workflow {
            return Err("invalid workflow sidecar kind or version".into());
        }
        sidecar.data.ok_or("workflow sidecar has no data")?
    } else {
        value
    };
    let workflow: PortableWorkflow = serde_json::from_value(workflow_value)
        .map_err(|e| format!("invalid portable workflow: {e}"))?;
    validate_portable_workflow(&workflow)?;
    Ok(workflow)
}

pub fn validate_portable_workflow(workflow: &PortableWorkflow) -> Result<(), String> {
    if workflow.version != "1.0" {
        return Err(format!(
            "unsupported portable workflow version '{}'",
            workflow.version
        ));
    }
    if workflow.engine != "kronn" {
        return Err(format!(
            "unsupported portable workflow engine '{}'",
            workflow.engine
        ));
    }
    if workflow.steps.is_empty() {
        return Err("portable workflow requires at least one step".into());
    }

    let mut requirements = BTreeSet::new();
    for binary in &workflow.requires {
        validate_workflow_binary(binary)?;
        if !requirements.insert(binary.as_str()) {
            return Err(format!("duplicate workflow requirement '{binary}'"));
        }
    }
    if requirements.is_empty() {
        return Err("portable workflow requires an explicit binary allowlist".into());
    }

    for (name, input) in &workflow.inputs {
        validate_workflow_name(name, "input")?;
        if let Some(default) = &input.default {
            validate_workflow_input_value(name, input.kind, default)
                .map_err(|error| format!("invalid default: {error}"))?;
        }
    }
    for (name, reference) in &workflow.env {
        validate_env_name(name)?;
        parse_env_reference(reference)?;
    }

    let mut step_names = BTreeSet::new();
    let template_values: BTreeMap<_, _> = workflow
        .inputs
        .keys()
        .map(|name| (name.as_str(), "value".to_string()))
        .collect();
    for step in &workflow.steps {
        if step.name.trim().is_empty() || !step_names.insert(step.name.as_str()) {
            return Err(format!(
                "workflow step names must be non-empty and unique: '{}'",
                step.name
            ));
        }
        validate_workflow_binary(&step.command)?;
        if !requirements.contains(step.command.as_str()) {
            return Err(format!(
                "workflow command '{}' is not declared in requires",
                step.command
            ));
        }
        if step.args.len() > 128 {
            return Err(format!(
                "workflow step '{}' has more than 128 args",
                step.name
            ));
        }
        for arg in &step.args {
            validate_workflow_arg(&step.name, arg)?;
            let rendered = render_template(arg, &template_values)
                .map_err(|error| format!("invalid workflow step '{}': {error}", step.name))?;
            validate_workflow_arg(&step.name, &rendered)?;
        }
    }
    Ok(())
}

pub fn render_portable_workflow(
    workflow: &PortableWorkflow,
    supplied: &BTreeMap<String, String>,
    host_env: &BTreeMap<String, String>,
) -> Result<RenderedPortableWorkflow, String> {
    validate_portable_workflow(workflow)?;
    let input_values = resolve_workflow_inputs(workflow, supplied)?;
    let mut env = BTreeMap::new();
    for (name, reference) in &workflow.env {
        let host_name = parse_env_reference(reference)?;
        let value = host_env
            .get(host_name)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("missing required environment variable '{host_name}'"))?;
        env.insert(name.clone(), value.clone());
    }
    let steps = render_workflow_steps(workflow, &input_values)?;
    Ok(RenderedPortableWorkflow { env, steps })
}

fn render_workflow_steps(
    workflow: &PortableWorkflow,
    input_values: &BTreeMap<String, String>,
) -> Result<Vec<RenderedWorkflowStep>, String> {
    let references: BTreeMap<_, _> = input_values
        .iter()
        .map(|(name, value)| (name.as_str(), value.clone()))
        .collect();
    workflow
        .steps
        .iter()
        .map(|step| {
            let args = step
                .args
                .iter()
                .map(|arg| {
                    let rendered = render_template(arg, &references)?;
                    validate_workflow_arg(&step.name, &rendered)?;
                    Ok::<String, String>(rendered)
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(RenderedWorkflowStep {
                name: step.name.clone(),
                command: step.command.clone(),
                args,
            })
        })
        .collect()
}

pub fn portable_workflow_env_example(workflows: &[PortableWorkflow]) -> Result<String, String> {
    let mut names = BTreeSet::new();
    for workflow in workflows {
        validate_portable_workflow(workflow)?;
        for reference in workflow.env.values() {
            names.insert(parse_env_reference(reference)?);
        }
    }
    Ok(names.into_iter().map(|name| format!("{name}=\n")).collect())
}

/// CLI implementation for `kronn run`. Validation/render-only modes are safe
/// by default; process execution needs the explicit `--allow-exec` gate and
/// always spawns one declared binary with literal argv (never a shell).
pub fn run_cli_workflow(args: &[String]) -> Result<(), String> {
    let Some(workflow_arg) = args.first() else {
        return Err(
            "Usage: kronn run <workflow.kronn.json> [--var KEY=VALUE] [--check|--render-env|--allow-exec]"
                .into(),
        );
    };
    let workflow_path = PathBuf::from(workflow_arg);
    let mut supplied = BTreeMap::new();
    let mut check_only = false;
    let mut render_env = false;
    let mut allow_exec = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--var" => {
                let assignment = args.get(index + 1).ok_or("--var requires KEY=VALUE")?;
                let (name, value) = assignment
                    .split_once('=')
                    .ok_or("--var requires KEY=VALUE")?;
                reject_secrets(value.as_bytes()).map_err(|_| {
                    "--var values must not contain secrets; use ${env:VAR}".to_string()
                })?;
                if name.is_empty()
                    || supplied
                        .insert(name.to_string(), value.to_string())
                        .is_some()
                {
                    return Err(format!("duplicate or empty --var name '{name}'"));
                }
                index += 2;
            }
            "--check" => {
                check_only = true;
                index += 1;
            }
            "--render-env" => {
                render_env = true;
                index += 1;
            }
            "--allow-exec" => {
                allow_exec = true;
                index += 1;
            }
            other => return Err(format!("unknown kronn run argument '{other}'")),
        }
    }
    if [check_only, render_env, allow_exec]
        .into_iter()
        .filter(|enabled| *enabled)
        .count()
        != 1
    {
        return Err("choose exactly one of --check, --render-env, or --allow-exec".into());
    }

    let content = fs::read(&workflow_path)
        .map_err(|e| format!("cannot read workflow '{}': {e}", workflow_path.display()))?;
    let workflow = parse_portable_workflow(&content)?;
    if render_env {
        print!("{}", portable_workflow_env_example(&[workflow])?);
        return Ok(());
    }
    // Check supplied values and defaults without requiring local secrets.
    let input_values = resolve_workflow_inputs(&workflow, &supplied)?;
    render_workflow_steps(&workflow, &input_values)?;
    if check_only {
        println!("workflow validated: {} step(s)", workflow.steps.len());
        return Ok(());
    }

    let project_root = workflow_path
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(".agents"))
        .and_then(Path::parent)
        .ok_or("executable workflow must be inside a locked .agents tree")?;
    require_lock_approval(project_root)?;

    let host_env: BTreeMap<_, _> = std::env::vars().collect();
    let rendered = render_portable_workflow(&workflow, &supplied, &host_env)?;
    let working_directory = workflow_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    for step in rendered.steps {
        println!("running workflow step '{}'", step.name);
        let mut command = std::process::Command::new(&step.command);
        command
            .args(&step.args)
            .current_dir(working_directory)
            .env_clear()
            .envs(&rendered.env);
        if let Ok(path) = std::env::var("PATH") {
            command.env("PATH", path);
        }
        let status = command
            .status()
            .map_err(|e| format!("cannot execute workflow step '{}': {e}", step.name))?;
        if !status.success() {
            return Err(format!(
                "workflow step '{}' failed with status {status}",
                step.name
            ));
        }
    }
    Ok(())
}

fn resolve_workflow_inputs(
    workflow: &PortableWorkflow,
    supplied: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    for name in supplied.keys() {
        if !workflow.inputs.contains_key(name) {
            return Err(format!("unknown workflow input '{name}'"));
        }
    }
    let mut resolved = BTreeMap::new();
    for (name, input) in &workflow.inputs {
        let value = match supplied.get(name) {
            Some(raw) => parse_workflow_input(name, input.kind, raw)?,
            None => match &input.default {
                Some(default) => {
                    validate_workflow_input_value(name, input.kind, default)?;
                    default.clone()
                }
                None if input.required => {
                    return Err(format!("missing required workflow input '{name}'"));
                }
                None => {
                    resolved.insert(name.clone(), String::new());
                    continue;
                }
            },
        };
        if input.required
            && ((input.kind == WorkflowInputKind::String
                && value.as_str().unwrap().trim().is_empty())
                || (input.kind == WorkflowInputKind::List && value.as_array().unwrap().is_empty()))
        {
            return Err(format!("missing required workflow input '{name}'"));
        }
        resolved.insert(name.clone(), render_workflow_input(input.kind, &value)?);
    }
    Ok(resolved)
}

fn parse_workflow_input(name: &str, kind: WorkflowInputKind, raw: &str) -> Result<Value, String> {
    let value = match kind {
        WorkflowInputKind::String => Value::String(raw.to_string()),
        WorkflowInputKind::Boolean | WorkflowInputKind::Number | WorkflowInputKind::List => {
            serde_json::from_str(raw)
                .map_err(|_| format!("workflow input '{name}' is not valid {kind:?}"))?
        }
    };
    validate_workflow_input_value(name, kind, &value)?;
    Ok(value)
}

fn validate_workflow_input_value(
    name: &str,
    kind: WorkflowInputKind,
    value: &Value,
) -> Result<(), String> {
    let valid = match kind {
        WorkflowInputKind::Boolean => value.is_boolean(),
        WorkflowInputKind::Number => value.is_number(),
        WorkflowInputKind::String => value.is_string(),
        WorkflowInputKind::List => value.is_array(),
    };
    valid
        .then_some(())
        .ok_or_else(|| format!("workflow input '{name}' must be {kind:?}"))
}

fn render_workflow_input(kind: WorkflowInputKind, value: &Value) -> Result<String, String> {
    match kind {
        WorkflowInputKind::String => Ok(value.as_str().unwrap().to_string()),
        WorkflowInputKind::Boolean | WorkflowInputKind::Number => Ok(value.to_string()),
        WorkflowInputKind::List => serde_json::to_string(value)
            .map_err(|e| format!("cannot render workflow list input: {e}")),
    }
}

fn validate_workflow_name(name: &str, label: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(format!("workflow {label} name must not be empty"));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
    {
        return Err(format!("invalid workflow {label} name '{name}'"));
    }
    Ok(())
}

fn validate_env_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err("environment variable name must not be empty".into());
    };
    if !(first.is_ascii_uppercase() || first == '_')
        || !chars.all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
    {
        return Err(format!("invalid environment variable name '{name}'"));
    }
    Ok(())
}

fn parse_env_reference(reference: &str) -> Result<&str, String> {
    let name = reference
        .strip_prefix("${env:")
        .and_then(|value| value.strip_suffix('}'))
        .ok_or_else(|| {
            format!("embedded environment value must be a ${{env:VAR}} reference: '{reference}'")
        })?;
    validate_env_name(name)?;
    Ok(name)
}

fn validate_workflow_binary(binary: &str) -> Result<(), String> {
    if binary.is_empty()
        || binary.starts_with('-')
        || binary.contains('/')
        || binary.contains('\\')
        || !binary.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '+')
        })
    {
        return Err(format!(
            "workflow requires a bare binary name, got '{binary}'"
        ));
    }
    let normalized = binary.to_ascii_lowercase();
    if crate::core::quick_exec::DENIED_BINARIES.contains(&normalized.as_str()) {
        return Err(format!(
            "workflow binary '{binary}' is a shell or command wrapper and is forbidden"
        ));
    }
    Ok(())
}

fn validate_workflow_arg(step_name: &str, arg: &str) -> Result<(), String> {
    if arg.contains('\0') {
        return Err(format!("workflow step '{step_name}' has a NUL argument"));
    }
    let path_candidate = arg.split_once('=').map(|(_, value)| value).unwrap_or(arg);
    let path = Path::new(path_candidate);
    let bytes = path_candidate.as_bytes();
    let windows_drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    if path.is_absolute()
        || path_candidate.starts_with("\\\\")
        || windows_drive
        || path
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(format!(
            "workflow step '{step_name}' contains an absolute or parent path"
        ));
    }
    Ok(())
}

fn skill_body(content: &[u8]) -> Result<&str, String> {
    let text = std::str::from_utf8(content).map_err(|_| "SKILL.md must be UTF-8")?;
    let start = text
        .strip_prefix("---\n")
        .ok_or("SKILL.md must start with YAML frontmatter")?;
    let end = start
        .find("\n---")
        .ok_or("SKILL.md frontmatter is not closed")?;
    let body = &start[end + 4..];
    Ok(body.strip_prefix('\n').unwrap_or(body))
}

/// Validate the third-party `SKILL.md` against the official Agent Skills spec:
/// YAML frontmatter with a valid `name` equal to the skill directory and a
/// `description` within bounds, and never any Kronn key (that lives in the
/// sidecar). See <https://agentskills.io/specification>.
fn validate_skill(content: &[u8], expected_id: &str) -> Result<(), String> {
    reject_secrets(content)?;
    let text = std::str::from_utf8(content).map_err(|_| "SKILL.md must be UTF-8")?;
    let trimmed = text.trim_start();
    if !trimmed.starts_with("---\n") {
        return Err("SKILL.md must start with YAML frontmatter".into());
    }
    let end = trimmed[4..]
        .find("\n---")
        .ok_or("SKILL.md frontmatter is not closed")?;
    let frontmatter = &trimmed[4..4 + end];
    if frontmatter
        .lines()
        .any(|line| line.trim_start().starts_with("kronn"))
    {
        return Err("Kronn metadata must be stored in SKILL.kronn.json".into());
    }
    let name =
        frontmatter_value(frontmatter, "name").ok_or("SKILL.md requires a non-empty name")?;
    let description = frontmatter_value(frontmatter, "description")
        .ok_or("SKILL.md requires a non-empty description")?;
    // Agent Skills spec: name is a lowercase hyphenated slug (<=64 chars) and
    // must equal the skill's directory name.
    validate_id(&name)
        .map_err(|_| format!("SKILL.md name '{name}' is not a valid Agent Skills name"))?;
    if name != expected_id {
        return Err(format!(
            "SKILL.md name '{name}' must match its directory '{expected_id}'"
        ));
    }
    // Agent Skills spec: description is capped at 1024 characters.
    if description.chars().count() > 1024 {
        return Err("SKILL.md description exceeds 1024 characters".into());
    }
    Ok(())
}

/// Read a single-line scalar value from `SKILL.md` frontmatter, stripping one
/// layer of surrounding quotes. Returns `None` when the key is absent or empty.
fn frontmatter_value(frontmatter: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    frontmatter.lines().find_map(|line| {
        let value = line.strip_prefix(&prefix)?.trim();
        let unquoted = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value)
            .trim();
        (!unquoted.is_empty()).then(|| unquoted.to_string())
    })
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || id.starts_with('-')
        || id.ends_with('-')
        || id.contains("--")
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(format!("invalid portable library id '{id}'"));
    }
    Ok(())
}

fn reject_secrets(bytes: &[u8]) -> Result<(), String> {
    let text = String::from_utf8_lossy(bytes);
    let suspicious = ["-----BEGIN PRIVATE KEY-----", "ghp_", "AKIA", "sk-"];
    if suspicious.iter().any(|needle| text.contains(needle))
        || text.split('.').any(|part| {
            part.len() > 80
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
    {
        return Err("portable library content appears to contain a secret".into());
    }
    Ok(())
}

fn collect_auxiliary_files(
    base: &Path,
    current: &Path,
    out: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(current)
        .map_err(|e| format!("cannot read skill directory: {e}"))?
        .flatten()
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        // Refuse symlinks explicitly: `is_dir`/`is_file` follow links and would
        // let an auxiliary resource read outside the skill directory despite the
        // lexical relative-path checks.
        let file_type = entry
            .file_type()
            .map_err(|e| format!("cannot stat skill resource: {e}"))?;
        if file_type.is_symlink() {
            return Err("skill resources must not be symlinks".into());
        }
        if file_type.is_dir() {
            collect_auxiliary_files(base, &path, out)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(base)
                .map_err(|_| "auxiliary file escaped skill directory")?
                .to_path_buf();
            validate_relative(&relative)?;
            let bytes = fs::read(&path).map_err(|e| format!("cannot read skill resource: {e}"))?;
            reject_secrets(&bytes)?;
            out.insert(relative, bytes);
        }
    }
    Ok(())
}

fn read_manifest(path: &Path) -> Result<ManagedManifest, String> {
    if !path.exists() {
        return Ok(ManagedManifest {
            version: CONTRACT_VERSION,
            files: BTreeMap::new(),
        });
    }
    let manifest: ManagedManifest = serde_json::from_slice(
        &fs::read(path).map_err(|e| format!("cannot read sync manifest: {e}"))?,
    )
    .map_err(|e| format!("invalid sync manifest: {e}"))?;
    if manifest.version != CONTRACT_VERSION {
        return Err("unsupported sync manifest version".into());
    }
    for relative in manifest.files.keys() {
        validate_portable_source(relative)?;
    }
    Ok(manifest)
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| format!("cannot serialize portable item: {e}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn sha256(bytes: &[u8]) -> String {
    // sha2 0.11 returns a `GenericArray`, which does not implement `LowerHex`;
    // encode the digest bytes explicitly so the hash stays a lowercase hex string.
    use std::fmt::Write;
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for &byte in digest.iter() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Hash the semantic payload of a `*.kronn.json` sidecar, excluding its own
/// provenance block. Hashing the whole file would be self-referential: the file
/// embeds `content_sha256`, so every sync rewrite would change the hash again.
fn sidecar_payload_sha256(sidecar: &KronnSidecar) -> Result<String, String> {
    let payload = serde_json::json!({
        "version": sidecar.version,
        "kind": sidecar.kind,
        "id": sidecar.id,
        "data": sidecar.data,
    });
    let bytes = serde_json::to_vec(&payload)
        .map_err(|e| format!("cannot hash portable item payload: {e}"))?;
    Ok(sha256(&bytes))
}

fn portable_path(path: &Path) -> Result<String, String> {
    validate_relative(path)?;
    Ok(path
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn validate_portable_source(path: &str) -> Result<(), String> {
    validate_relative(Path::new(path))
}

fn validate_relative(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("portable paths must be non-empty, relative, and traversal-free".into());
    }
    Ok(())
}

fn ensure_inside(root: &Path, path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("sync target has no parent")?;
    let mut cursor = parent;
    while !cursor.exists() {
        cursor = cursor.parent().ok_or("sync target escaped root")?;
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("cannot resolve sync root: {e}"))?;
    let canonical_parent = cursor
        .canonicalize()
        .map_err(|e| format!("cannot resolve sync target: {e}"))?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err("sync target escaped .agents root".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AgentType, ModelTier, PromptVariable, QuickPrompt};
    use chrono::Utc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("kronn-portable-{name}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn skill(root: &Path, id: &str, body: &str) {
        let dir = root.join("skills").join(id);
        fs::create_dir_all(dir.join("references")).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {id}\ndescription: test skill\n---\n\n{body}\n"),
        )
        .unwrap();
        fs::write(dir.join("references/example.md"), "resource").unwrap();
    }

    fn json_item(root: &Path, folder: &str, kind: LibraryKind, id: &str) {
        fs::create_dir_all(root.join(folder)).unwrap();
        let sidecar = KronnSidecar {
            version: 1,
            kind,
            id: id.into(),
            provenance: Provenance {
                scope: LibraryScope::Global,
                source: format!("{folder}/{id}.kronn.json"),
                content_sha256: "pending".into(),
            },
            data: Some(serde_json::json!({"name": id})),
        };
        fs::write(
            root.join(folder).join(format!("{id}.kronn.json")),
            canonical_json(&sidecar).unwrap(),
        )
        .unwrap();
    }

    fn prompt(id: &str, template: &str, variables: Vec<PromptVariable>) -> QuickPrompt {
        let now = Utc::now();
        QuickPrompt {
            id: id.into(),
            name: "Portable prompt".into(),
            icon: "🧳".into(),
            prompt_template: template.into(),
            variables,
            agent: AgentType::Codex,
            project_id: Some("project-1".into()),
            skill_ids: vec!["testing".into()],
            profile_ids: vec!["reviewer".into()],
            directive_ids: vec!["concise".into()],
            tier: ModelTier::Reasoning,
            agent_settings: None,
            description: "A portable Quick Prompt".into(),
            pinned: true,
            created_at: now,
            updated_at: now,
        }
    }

    fn variable(name: &str, required: bool) -> PromptVariable {
        PromptVariable {
            name: name.into(),
            label: name.into(),
            placeholder: String::new(),
            description: Some(format!("input {name}")),
            required,
            pattern: None,
        }
    }

    fn portable_workflow() -> PortableWorkflow {
        PortableWorkflow {
            version: "1.0".into(),
            engine: "kronn".into(),
            requires: vec!["echo".into()],
            inputs: BTreeMap::from([
                (
                    "title".into(),
                    WorkflowInputDef {
                        kind: WorkflowInputKind::String,
                        description: None,
                        required: true,
                        default: None,
                    },
                ),
                (
                    "count".into(),
                    WorkflowInputDef {
                        kind: WorkflowInputKind::Number,
                        description: None,
                        required: false,
                        default: Some(serde_json::json!(2)),
                    },
                ),
                (
                    "enabled".into(),
                    WorkflowInputDef {
                        kind: WorkflowInputKind::Boolean,
                        description: None,
                        required: false,
                        default: Some(Value::Bool(true)),
                    },
                ),
                (
                    "tags".into(),
                    WorkflowInputDef {
                        kind: WorkflowInputKind::List,
                        description: None,
                        required: false,
                        default: Some(serde_json::json!(["safe", "portable"])),
                    },
                ),
            ]),
            env: BTreeMap::from([("API_TOKEN".into(), "${env:HOST_API_TOKEN}".into())]),
            steps: vec![WorkflowStepDef {
                name: "show".into(),
                command: "echo".into(),
                args: vec![
                    "{{title}}".into(),
                    "{{count}}".into(),
                    "{{enabled}}".into(),
                    "{{tags}}".into(),
                ],
            }],
        }
    }

    #[test]
    fn project_scope_wins_and_search_reuses_effective_item() {
        let global = temp("global");
        let project = temp("project");
        skill(&global, "review", "global body");
        skill(&project, "review", "project needle");
        let catalog = discover(Some(&global), Some(&project)).unwrap();
        let item = catalog.get(LibraryKind::Skill, "review").unwrap();
        assert_eq!(item.scope, LibraryScope::Project);
        assert_eq!(catalog.search("needle"), vec![item]);
    }

    #[test]
    fn discovers_all_kinds_with_explicit_provenance_and_auxiliary_files() {
        let root = temp("all");
        skill(&root, "testing", "body");
        json_item(&root, "directives", LibraryKind::Directive, "concise");
        let qp = quick_prompt_to_skill(
            &prompt(
                "review-pr",
                "Review {{subject}}",
                vec![variable("subject", true)],
            ),
            LibraryScope::Global,
        )
        .unwrap();
        let qp_dir = root.join("skills/review-pr");
        fs::create_dir_all(&qp_dir).unwrap();
        fs::write(qp_dir.join("SKILL.md"), &qp.content).unwrap();
        fs::write(
            qp_dir.join("SKILL.kronn.json"),
            canonical_json(&qp.sidecar).unwrap(),
        )
        .unwrap();
        json_item(&root, "workflows", LibraryKind::Workflow, "release");
        let catalog = discover(Some(&root), None).unwrap();
        assert_eq!(catalog.items().count(), 4);
        let item = catalog.get(LibraryKind::Skill, "testing").unwrap();
        assert_eq!(item.sidecar.provenance.source, "skills/testing/SKILL.md");
        assert!(item
            .auxiliary_files
            .contains_key(Path::new("references/example.md")));
    }

    #[test]
    fn quick_prompt_exports_as_valid_agent_skill_and_round_trips() {
        let source = temp("qp-source");
        let target = temp("qp-target");
        let original = prompt(
            "review-pr",
            "Review {{subject}} with {{depth}}",
            vec![variable("subject", true), variable("depth", false)],
        );
        let item = quick_prompt_to_skill(&original, LibraryScope::Project).unwrap();
        let skill = String::from_utf8(item.content.clone()).unwrap();
        let frontmatter = skill.split("---").nth(1).unwrap();
        assert!(frontmatter.contains("name: review-pr"));
        assert!(!frontmatter.to_lowercase().contains("kronn"));
        assert_eq!(item.sidecar.kind, LibraryKind::QuickPrompt);

        let dir = source.join("skills/review-pr");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), &item.content).unwrap();
        fs::write(
            dir.join("SKILL.kronn.json"),
            canonical_json(&item.sidecar).unwrap(),
        )
        .unwrap();
        let catalog = discover(None, Some(&source)).unwrap();
        let discovered = catalog.get(LibraryKind::QuickPrompt, "review-pr").unwrap();
        let imported = quick_prompt_from_skill(discovered).unwrap();
        assert_eq!(
            serde_json::to_value(imported).unwrap(),
            serde_json::to_value(original).unwrap()
        );

        let report = sync(&catalog, &target).unwrap();
        assert!(report
            .created
            .contains(&PathBuf::from("skills/review-pr/SKILL.md")));
        assert!(report
            .created
            .contains(&PathBuf::from("skills/review-pr/SKILL.kronn.json")));
    }

    #[test]
    fn quick_prompt_render_validates_types_defaults_required_and_escaping() {
        let mut qp = prompt(
            "typed-prompt",
            "enabled={{enabled}} count={{count}} title={{title}} tags={{tags}} default={{mode}} literal={{{{title}}}} raw={{raw}}",
            vec![
                variable("enabled", true),
                variable("count", true),
                variable("title", true),
                variable("tags", true),
                variable("mode", false),
                variable("raw", true),
            ],
        );
        qp.variables
            .iter_mut()
            .find(|variable| variable.name == "title")
            .unwrap()
            .pattern = Some("[A-Z][A-Za-z]+".into());
        let item = quick_prompt_to_skill_with_inputs(
            &qp,
            vec![
                QuickPromptInput {
                    name: "enabled".into(),
                    kind: QuickPromptInputKind::Boolean,
                    required: true,
                    default: None,
                },
                QuickPromptInput {
                    name: "count".into(),
                    kind: QuickPromptInputKind::Number,
                    required: true,
                    default: None,
                },
                QuickPromptInput {
                    name: "title".into(),
                    kind: QuickPromptInputKind::String,
                    required: true,
                    default: None,
                },
                QuickPromptInput {
                    name: "tags".into(),
                    kind: QuickPromptInputKind::List,
                    required: true,
                    default: None,
                },
                QuickPromptInput {
                    name: "mode".into(),
                    kind: QuickPromptInputKind::String,
                    required: false,
                    default: Some(Value::String("safe".into())),
                },
                QuickPromptInput {
                    name: "raw".into(),
                    kind: QuickPromptInputKind::String,
                    required: true,
                    default: None,
                },
            ],
            LibraryScope::Project,
        )
        .unwrap();
        let values = BTreeMap::from([
            ("enabled".into(), Value::Bool(true)),
            ("count".into(), serde_json::json!(2.5)),
            ("title".into(), Value::String("Audit".into())),
            ("tags".into(), serde_json::json!(["rust", "safe"])),
            ("raw".into(), Value::String("{{not-recursive}}".into())),
        ]);
        assert_eq!(
            render_quick_prompt_skill(&item, &values).unwrap(),
            "enabled=true count=2.5 title=Audit tags=[\"rust\",\"safe\"] default=safe literal={{title}} raw={{not-recursive}}"
        );

        let mut missing = values.clone();
        missing.remove("title");
        assert!(render_quick_prompt_skill(&item, &missing)
            .unwrap_err()
            .contains("missing required"));
        let mut wrong = values.clone();
        wrong.insert("count".into(), Value::String("two".into()));
        assert!(render_quick_prompt_skill(&item, &wrong)
            .unwrap_err()
            .contains("must be Number"));
        let mut empty = values.clone();
        empty.insert("title".into(), Value::String("   ".into()));
        assert!(render_quick_prompt_skill(&item, &empty)
            .unwrap_err()
            .contains("missing required"));
        let mut bad_pattern = values.clone();
        bad_pattern.insert("title".into(), Value::String("audit".into()));
        assert!(render_quick_prompt_skill(&item, &bad_pattern)
            .unwrap_err()
            .contains("does not match its pattern"));
        let mut unknown = values;
        unknown.insert("surprise".into(), Value::Bool(true));
        assert!(render_quick_prompt_skill(&item, &unknown)
            .unwrap_err()
            .contains("unknown Quick Prompt variable"));
    }

    #[test]
    fn quick_prompt_render_rejects_unknown_template_variables_and_bad_defaults() {
        let qp = prompt("bad-template", "Hello {{undeclared}}", vec![]);
        let item = quick_prompt_to_skill(&qp, LibraryScope::Global).unwrap();
        assert!(render_quick_prompt_skill(&item, &BTreeMap::new())
            .unwrap_err()
            .contains("unknown Quick Prompt variable"));

        let qp = prompt("bad-default", "{{count}}", vec![variable("count", false)]);
        let error = quick_prompt_to_skill_with_inputs(
            &qp,
            vec![QuickPromptInput {
                name: "count".into(),
                kind: QuickPromptInputKind::Number,
                required: false,
                default: Some(Value::String("not-a-number".into())),
            }],
            LibraryScope::Global,
        )
        .unwrap_err();
        assert!(error.contains("invalid default"));
    }

    #[test]
    fn legacy_quick_prompt_json_migrates_to_skill_layout() {
        let root = temp("legacy-qp");
        let target = temp("legacy-target");
        let qp = prompt(
            "legacy-review",
            "Review {{subject}}",
            vec![variable("subject", true)],
        );
        let relative = PathBuf::from("quick-prompts/legacy-review.kronn.json");
        let sidecar = KronnSidecar {
            version: CONTRACT_VERSION,
            kind: LibraryKind::QuickPrompt,
            id: qp.id.clone(),
            provenance: Provenance {
                scope: LibraryScope::Global,
                source: portable_path(&relative).unwrap(),
                content_sha256: "legacy".into(),
            },
            data: Some(serde_json::to_value(&qp).unwrap()),
        };
        fs::create_dir_all(root.join("quick-prompts")).unwrap();
        fs::write(root.join(&relative), canonical_json(&sidecar).unwrap()).unwrap();

        let catalog = discover(Some(&root), None).unwrap();
        let item = catalog
            .get(LibraryKind::QuickPrompt, "legacy-review")
            .unwrap();
        assert_eq!(
            item.relative_path,
            PathBuf::from("skills/legacy-review/SKILL.md")
        );
        let report = sync(&catalog, &target).unwrap();
        assert!(report
            .created
            .contains(&PathBuf::from("skills/legacy-review/SKILL.md")));
        assert!(target
            .join(".agents/skills/legacy-review/SKILL.kronn.json")
            .is_file());
    }

    #[test]
    fn portable_workflow_validates_renders_all_types_and_env_example() {
        let mut workflow = portable_workflow();
        workflow.inputs.insert(
            "optional".into(),
            WorkflowInputDef {
                kind: WorkflowInputKind::String,
                description: None,
                required: false,
                default: None,
            },
        );
        workflow.steps[0].args.push("{{optional}}".into());
        validate_portable_workflow(&workflow).unwrap();
        let rendered = render_portable_workflow(
            &workflow,
            &BTreeMap::from([("title".into(), "Audit".into())]),
            &BTreeMap::from([("HOST_API_TOKEN".into(), "local-value".into())]),
        )
        .unwrap();
        assert_eq!(
            rendered.steps[0].args,
            vec!["Audit", "2", "true", "[\"safe\",\"portable\"]", ""]
        );
        assert_eq!(rendered.env.get("API_TOKEN").unwrap(), "local-value");
        assert_eq!(
            portable_workflow_env_example(&[workflow]).unwrap(),
            "HOST_API_TOKEN=\n"
        );
    }

    #[test]
    fn portable_workflow_refuses_unknowns_bad_types_secrets_paths_and_shells() {
        let mut workflow = portable_workflow();
        let error = render_portable_workflow(
            &workflow,
            &BTreeMap::from([("unknown".into(), "x".into())]),
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(error.contains("unknown workflow input"));

        let error = render_portable_workflow(
            &workflow,
            &BTreeMap::from([
                ("title".into(), "Audit".into()),
                ("count".into(), "two".into()),
            ]),
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(error.contains("not valid Number"));

        let error = render_portable_workflow(
            &workflow,
            &BTreeMap::from([("title".into(), "/tmp/injected".into())]),
            &BTreeMap::from([("HOST_API_TOKEN".into(), "local-value".into())]),
        )
        .unwrap_err();
        assert!(error.contains("absolute or parent path"));

        let error = render_portable_workflow(
            &workflow,
            &BTreeMap::from([("title".into(), "   ".into())]),
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(error.contains("missing required"));

        workflow.steps[0].args.push("/tmp/escape".into());
        assert!(validate_portable_workflow(&workflow)
            .unwrap_err()
            .contains("absolute or parent path"));
        workflow.steps[0].args.pop();
        workflow.requires = vec!["sh".into()];
        workflow.steps[0].command = "sh".into();
        assert!(validate_portable_workflow(&workflow)
            .unwrap_err()
            .contains("forbidden"));

        for path in [r"C:\\temp\\escape", r"\\\\server\\share"] {
            let mut workflow = portable_workflow();
            workflow.steps[0].args.push(path.into());
            assert!(validate_portable_workflow(&workflow)
                .unwrap_err()
                .contains("absolute or parent path"));
        }
        let mut workflow = portable_workflow();
        workflow.steps[0].args.push("{{missing}}".into());
        assert!(validate_portable_workflow(&workflow)
            .unwrap_err()
            .contains("unknown Quick Prompt variable"));

        let unknown_field = br#"{
          "version":"1.0","engine":"kronn","requires":["echo"],
          "steps":[{"name":"x","command":"echo","surprise":true}]
        }"#;
        assert!(parse_portable_workflow(unknown_field)
            .unwrap_err()
            .contains("unknown field"));
        let embedded_secret = br#"{
          "version":"1.0","engine":"kronn","requires":["echo"],
          "steps":[{"name":"x","command":"echo","args":["ghp_abcdefghijklmnopqrstuvwxyz"]}]
        }"#;
        assert!(parse_portable_workflow(embedded_secret)
            .unwrap_err()
            .contains("secret"));
    }

    #[test]
    fn portable_workflow_sidecar_and_cli_check_share_the_same_contract() {
        let root = temp("workflow-cli");
        let path = root.join("audit.kronn.json");
        let workflow = portable_workflow();
        let sidecar = KronnSidecar {
            version: CONTRACT_VERSION,
            kind: LibraryKind::Workflow,
            id: "audit".into(),
            provenance: Provenance {
                scope: LibraryScope::Project,
                source: "workflows/audit.kronn.json".into(),
                content_sha256: "pending".into(),
            },
            data: Some(serde_json::to_value(&workflow).unwrap()),
        };
        fs::write(&path, canonical_json(&sidecar).unwrap()).unwrap();
        assert_eq!(
            parse_portable_workflow(&fs::read(&path).unwrap()).unwrap(),
            workflow
        );
        run_cli_workflow(&[
            path.to_string_lossy().to_string(),
            "--var".into(),
            "title=Audit".into(),
            "--check".into(),
        ])
        .unwrap();
        assert!(run_cli_workflow(&[
            path.to_string_lossy().to_string(),
            "--var".into(),
            "title=/tmp/injected".into(),
            "--check".into(),
        ])
        .unwrap_err()
        .contains("absolute or parent path"));
        assert!(run_cli_workflow(&[
            path.to_string_lossy().to_string(),
            "--var".into(),
            "title=Audit".into(),
        ])
        .unwrap_err()
        .contains("choose exactly one"));
    }

    #[test]
    fn sync_generates_deterministic_workflow_router_schema_and_scripts() {
        let source = temp("router-source");
        let target = temp("router-target");
        json_item(&source, "workflows", LibraryKind::Workflow, "release");
        json_item(&source, "workflows", LibraryKind::Workflow, "audit");
        let catalog = discover(Some(&source), None).unwrap();
        let first = sync(&catalog, &target).unwrap();
        for path in [
            "skills/kronn/SKILL.md",
            "skills/kronn/SKILL.kronn.json",
            "schema/workflow.v1.schema.json",
            "scripts/render-env.sh",
            "scripts/bootstrap.sh",
        ] {
            assert!(
                first.created.contains(&PathBuf::from(path)),
                "missing {path}"
            );
        }
        let router = fs::read_to_string(target.join(".agents/skills/kronn/SKILL.md")).unwrap();
        assert!(router.starts_with("---\nname: kronn\n"));
        let audit = router.find("`audit`").unwrap();
        let release = router.find("`release`").unwrap();
        assert!(audit < release, "inventory must be sorted");
        assert!(router.contains("--allow-exec"));
        let schema: Value = serde_json::from_str(WORKFLOW_SCHEMA).unwrap();
        assert_eq!(schema["additionalProperties"], Value::Bool(false));
        assert!(!RENDER_ENV_SCRIPT.contains("python"));
        assert!(!RENDER_ENV_SCRIPT.contains("npx"));
        assert!(!BOOTSTRAP_SCRIPT.contains("cp .env"));
        assert!(!sync(&catalog, &target).unwrap().changed());
    }

    #[test]
    fn sync_is_idempotent_and_handles_modification_and_deletion() {
        let source = temp("source");
        let target = temp("target");
        skill(&source, "one", "v1");
        skill(&source, "two", "gone");
        let first = sync(&discover(Some(&source), None).unwrap(), &target).unwrap();
        assert!(first
            .created
            .contains(&PathBuf::from("skills/one/SKILL.md")));
        let second = sync(&discover(Some(&source), None).unwrap(), &target).unwrap();
        assert!(!second.changed());
        fs::write(
            source.join("skills/one/SKILL.md"),
            "---\nname: one\ndescription: changed\n---\n\nv2\n",
        )
        .unwrap();
        fs::remove_dir_all(source.join("skills/two")).unwrap();
        let third = sync(&discover(Some(&source), None).unwrap(), &target).unwrap();
        assert!(third
            .modified
            .contains(&PathBuf::from("skills/one/SKILL.md")));
        assert!(third
            .deleted
            .contains(&PathBuf::from("skills/two/SKILL.md")));
    }

    #[test]
    fn lock_is_deterministic_and_frozen_check_detects_add_remove_and_alter() {
        let source = temp("lock-source");
        let target = temp("lock-target");
        skill(&source, "one", "v1");
        json_item(&source, "workflows", LibraryKind::Workflow, "release");
        let catalog = discover(Some(&source), None).unwrap();
        sync(&catalog, &target).unwrap();

        let first = fs::read(target.join(".agents/kronn.lock")).unwrap();
        let lock = check_frozen_hash(&target).unwrap();
        assert_eq!(lock.resources.len(), 2);
        assert!(lock.resources.iter().all(|resource| {
            resource.provenance == LockProvenance::Vendored
                && !resource.source.is_empty()
                && resource.content_sha256.len() == 64
        }));
        sync(&catalog, &target).unwrap();
        assert_eq!(first, fs::read(target.join(".agents/kronn.lock")).unwrap());

        let changed = target.join(".agents/skills/one/SKILL.md");
        let original = fs::read(&changed).unwrap();
        fs::write(&changed, "altered").unwrap();
        assert!(check_frozen_hash(&target).unwrap_err().contains("altered:"));
        fs::write(&changed, original).unwrap();

        let removed = target.join(".agents/schema/workflow.v1.schema.json");
        let original = fs::read(&removed).unwrap();
        fs::remove_file(&removed).unwrap();
        assert!(check_frozen_hash(&target).unwrap_err().contains("removed:"));
        fs::write(&removed, original).unwrap();

        fs::write(target.join(".agents/unlocked.txt"), "unexpected").unwrap();
        assert!(check_frozen_hash(&target).unwrap_err().contains("added:"));

        fs::remove_file(target.join(".agents/unlocked.txt")).unwrap();
        fs::write(target.join(".agents/.kronn-approvals.json"), "untrusted").unwrap();
        assert!(check_frozen_hash(&target).unwrap_err().contains("added:"));
    }

    #[test]
    fn tofu_approval_is_hash_bound_and_cannot_be_replayed_after_change() {
        let source = temp("approval-source");
        let target = temp("approval-target");
        let trust = temp("approval-trust");
        skill(&source, "one", "v1");
        sync(&discover(Some(&source), None).unwrap(), &target).unwrap();

        assert!(require_lock_approval_at(&target, &trust)
            .unwrap_err()
            .contains("TOFU approval required"));
        approve_lock_at(&target, &trust).unwrap();
        require_lock_approval_at(&target, &trust).unwrap();

        skill(&source, "one", "v2");
        sync(&discover(Some(&source), None).unwrap(), &target).unwrap();
        assert!(require_lock_approval_at(&target, &trust)
            .unwrap_err()
            .contains("stale"));
        approve_lock_at(&target, &trust).unwrap();
        require_lock_approval_at(&target, &trust).unwrap();
    }

    #[test]
    fn tofu_approval_cannot_be_replayed_between_identical_projects() {
        let source = temp("approval-replay-source");
        let first = temp("approval-replay-first");
        let second = temp("approval-replay-second");
        let trust = temp("approval-replay-trust");
        skill(&source, "one", "v1");
        let catalog = discover(Some(&source), None).unwrap();
        sync(&catalog, &first).unwrap();
        sync(&catalog, &second).unwrap();

        assert_eq!(
            fs::read(first.join(".agents/kronn.lock")).unwrap(),
            fs::read(second.join(".agents/kronn.lock")).unwrap()
        );
        approve_lock_at(&first, &trust).unwrap();
        require_lock_approval_at(&first, &trust).unwrap();
        assert!(require_lock_approval_at(&second, &trust)
            .unwrap_err()
            .contains("TOFU approval required"));

        let (first_approval, _) = approval_path(&first, &trust).unwrap();
        let (second_approval, _) = approval_path(&second, &trust).unwrap();
        fs::copy(first_approval, second_approval).unwrap();
        assert!(require_lock_approval_at(&second, &trust)
            .unwrap_err()
            .contains("stale"));
    }

    #[test]
    fn executable_workflow_requires_declared_binary_and_explicit_mode() {
        let mut workflow = portable_workflow();
        workflow.steps[0].command = "git".into();
        assert!(validate_portable_workflow(&workflow)
            .unwrap_err()
            .contains("not declared in requires"));

        let root = temp("exec-mode");
        let path = root.join("audit.kronn.json");
        fs::write(&path, canonical_json(&workflow).unwrap()).unwrap();
        assert!(run_cli_workflow(&[path.to_string_lossy().into_owned()])
            .unwrap_err()
            .contains("choose exactly one"));
    }

    #[test]
    fn rejects_collisions_kronn_frontmatter_absolute_paths_and_secrets() {
        let root = temp("invalid");
        skill(&root, "same", "ok");
        fs::write(
            root.join("skills/same/SKILL.md"),
            "---\nname: same\ndescription: x\nkronn_id: bad\n---\nbody",
        )
        .unwrap();
        assert!(discover(Some(&root), None)
            .unwrap_err()
            .contains("SKILL.kronn.json"));
        assert!(validate_portable_source("/tmp/nope").is_err());
        assert!(reject_secrets(b"ghp_abcdefghijklmnopqrstuvwxyz").is_err());
    }

    #[test]
    fn sync_preserves_binary_auxiliary_resources_byte_for_byte() {
        let source = temp("bin-source");
        let target = temp("bin-target");
        skill(&source, "assets", "body");
        // Invalid UTF-8 bytes: a lossy text conversion would corrupt these.
        let raw = [0x00u8, 0xff, 0xfe, 0x10, 0x80];
        fs::write(source.join("skills/assets/references/logo.bin"), raw).unwrap();
        let report = sync(&discover(Some(&source), None).unwrap(), &target).unwrap();
        assert!(report
            .created
            .contains(&PathBuf::from("skills/assets/references/logo.bin")));
        let synced = fs::read(target.join(".agents/skills/assets/references/logo.bin")).unwrap();
        assert_eq!(synced, raw);
    }

    #[test]
    fn cli_style_sync_into_project_tree_is_idempotent() {
        // Mirrors run_cli_sync: the global source and the project source/target
        // share the same tree after the first materialization.
        let global = temp("cli-global");
        let cwd = temp("cli-cwd");
        skill(&global, "review", "body");
        json_item(&global, "workflows", LibraryKind::Workflow, "release");
        let project_agents = cwd.join(".agents");

        let first = sync(
            &discover(Some(&global), Some(&project_agents)).unwrap(),
            &cwd,
        )
        .unwrap();
        assert!(first.changed());

        let cat1 = discover(Some(&global), Some(&project_agents)).unwrap();
        let wf1 = cat1
            .get(LibraryKind::Workflow, "release")
            .unwrap()
            .sidecar
            .provenance
            .clone();
        let sk1 = cat1
            .get(LibraryKind::Skill, "review")
            .unwrap()
            .sidecar
            .provenance
            .clone();

        // Rediscover from the now-populated project tree and resync.
        let second = sync(
            &discover(Some(&global), Some(&project_agents)).unwrap(),
            &cwd,
        )
        .unwrap();
        assert!(!second.changed(), "resync must be a no-op: {second:?}");

        let cat2 = discover(Some(&global), Some(&project_agents)).unwrap();
        let wf2 = cat2
            .get(LibraryKind::Workflow, "release")
            .unwrap()
            .sidecar
            .provenance
            .clone();
        let sk2 = cat2
            .get(LibraryKind::Skill, "review")
            .unwrap()
            .sidecar
            .provenance
            .clone();

        assert_eq!(wf1, wf2, "workflow provenance/hash must stay stable");
        assert_eq!(sk1, sk2, "skill provenance/hash must stay stable");
        // Origin scope preserved, not promoted global -> project on rediscovery.
        assert_eq!(wf1.scope, LibraryScope::Global);
        assert_eq!(sk1.scope, LibraryScope::Global);

        // A real project-local resource still wins over a global resource with
        // the same id; only managed copies with Global provenance are skipped.
        skill(&global, "project-wins", "global");
        skill(&project_agents, "project-wins", "project");
        let override_catalog = discover(Some(&global), Some(&project_agents)).unwrap();
        let override_item = override_catalog
            .get(LibraryKind::Skill, "project-wins")
            .unwrap();
        assert_eq!(override_item.scope, LibraryScope::Project);
        assert!(String::from_utf8_lossy(&override_item.content).contains("project"));

        // Changes in the actual global sources propagate through the managed
        // project copies instead of being shadowed by those copies.
        fs::write(
            global.join("skills/review/SKILL.md"),
            "---\nname: review\ndescription: updated\n---\n\nupdated body\n",
        )
        .unwrap();
        let workflow_path = global.join("workflows/release.kronn.json");
        let mut workflow: KronnSidecar =
            serde_json::from_slice(&fs::read(&workflow_path).unwrap()).unwrap();
        workflow.data = Some(serde_json::json!({"name": "release", "revision": 2}));
        fs::write(&workflow_path, canonical_json(&workflow).unwrap()).unwrap();

        let modified = sync(
            &discover(Some(&global), Some(&project_agents)).unwrap(),
            &cwd,
        )
        .unwrap();
        assert!(modified
            .modified
            .contains(&PathBuf::from("skills/review/SKILL.md")));
        assert!(modified
            .modified
            .contains(&PathBuf::from("workflows/release.kronn.json")));
        assert!(
            fs::read_to_string(project_agents.join("skills/review/SKILL.md"))
                .unwrap()
                .contains("updated body")
        );
        assert!(
            fs::read_to_string(project_agents.join("workflows/release.kronn.json"))
                .unwrap()
                .contains("\"revision\": 2")
        );

        let stable_after_update = sync(
            &discover(Some(&global), Some(&project_agents)).unwrap(),
            &cwd,
        )
        .unwrap();
        assert!(!stable_after_update.changed());

        // Removing global sources removes their managed project copies, while
        // the explicit project override remains present.
        fs::remove_dir_all(global.join("skills/review")).unwrap();
        fs::remove_file(&workflow_path).unwrap();
        let deleted = sync(
            &discover(Some(&global), Some(&project_agents)).unwrap(),
            &cwd,
        )
        .unwrap();
        assert!(deleted
            .deleted
            .contains(&PathBuf::from("skills/review/SKILL.md")));
        assert!(deleted
            .deleted
            .contains(&PathBuf::from("workflows/release.kronn.json")));
        assert!(!project_agents.join("skills/review/SKILL.md").exists());
        assert!(!project_agents.join("workflows/release.kronn.json").exists());
        assert!(project_agents.join("skills/project-wins/SKILL.md").exists());
    }

    #[test]
    fn skill_name_must_match_directory() {
        let err =
            validate_skill(b"---\nname: other\ndescription: ok\n---\nbody", "review").unwrap_err();
        assert!(err.contains("must match its directory"), "{err}");
    }

    #[test]
    fn skill_name_must_be_a_valid_slug() {
        let err = validate_skill(
            b"---\nname: Bad_Name\ndescription: ok\n---\nbody",
            "bad-name",
        )
        .unwrap_err();
        assert!(err.contains("not a valid Agent Skills name"), "{err}");
    }

    #[test]
    fn skill_description_out_of_bounds_is_rejected() {
        let long = "x".repeat(1025);
        let content = format!("---\nname: review\ndescription: {long}\n---\nbody");
        let err = validate_skill(content.as_bytes(), "review").unwrap_err();
        assert!(err.contains("exceeds 1024 characters"), "{err}");
        // A quoted, in-bound description with the folder name still validates.
        assert!(validate_skill(
            b"---\nname: review\ndescription: \"a bounded summary\"\n---\nbody",
            "review",
        )
        .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn auxiliary_symlinks_are_refused() {
        use std::os::unix::fs::symlink;
        let root = temp("symlink");
        skill(&root, "leaky", "body");
        let outside = temp("outside");
        fs::write(outside.join("target.txt"), "outside data").unwrap();
        symlink(
            outside.join("target.txt"),
            root.join("skills/leaky/references/link.txt"),
        )
        .unwrap();
        let err = discover(Some(&root), None).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
    }

    #[test]
    fn duplicate_ids_inside_a_scope_fail_closed() {
        let root = temp("collision");
        json_item(&root, "workflows", LibraryKind::Workflow, "same");
        let duplicate = root.join("workflows/alias.kronn.json");
        fs::copy(root.join("workflows/same.kronn.json"), duplicate).unwrap();
        assert!(discover(Some(&root), None)
            .unwrap_err()
            .contains("duplicate"));
    }
}
