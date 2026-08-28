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
    scan_json_kind(
        &root,
        scope,
        LibraryKind::QuickPrompt,
        "quick-prompts",
        &mut out,
    )?;
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
        let sidecar = read_or_derive_sidecar(
            &sidecar_path,
            LibraryKind::Skill,
            &id,
            scope,
            &relative,
            &content,
        )?;
        if scope == LibraryScope::Project && sidecar.provenance.scope == LibraryScope::Global {
            continue;
        }
        let mut auxiliary_files = BTreeMap::new();
        collect_auxiliary_files(&folder, &folder, &mut auxiliary_files)?;
        auxiliary_files.remove(Path::new("SKILL.md"));
        auxiliary_files.remove(Path::new("SKILL.kronn.json"));
        out.push(LibraryItem {
            kind: LibraryKind::Skill,
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

fn read_or_derive_sidecar(
    path: &Path,
    kind: LibraryKind,
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
            kind,
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
    if sidecar.version != CONTRACT_VERSION || sidecar.kind != kind || sidecar.id != id {
        return Err(format!("skill sidecar does not match '{id}'"));
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
            if item.kind == LibraryKind::Skill {
                item.content.clone()
            } else {
                canonical_json(&item.sidecar)?
            },
        );
        if item.kind == LibraryKind::Skill {
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
        LibraryKind::QuickPrompt => {
            PathBuf::from("quick-prompts").join(format!("{}.kronn.json", item.id))
        }
        LibraryKind::Workflow => PathBuf::from("workflows").join(format!("{}.kronn.json", item.id)),
    }
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
        json_item(
            &root,
            "quick-prompts",
            LibraryKind::QuickPrompt,
            "review-pr",
        );
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
