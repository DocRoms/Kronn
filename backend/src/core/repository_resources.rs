//! Versioned project resources stored under `kronn/`.
//!
//! Repository files are portable definitions. Local alignment baselines and
//! approvals stay in SQLite so a clone never imports trust from Git.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::core::export_secrets::{RedactedField, REDACTED};
use crate::models::{
    ArtifactBundlePage, ProjectRepositoryResourceKind, QuickApi, QuickExec, QuickPrompt, Skill,
    Workflow,
};

pub const SCHEMA_VERSION: u32 = 1;
pub const LOCK_PATH: &str = "kronn/kronn.lock";
const INDEX_PATH: &str = "kronn/INDEX.md";
const CONFIG_PATH: &str = "kronn/kronn.toml";
const ROUTER_PATH: &str = ".agents/skills/kronn/SKILL.md";
const AGENTS_PATH: &str = "docs/AGENTS.md";
const AGENTS_LINE: &str = "Kronn resources → `kronn/INDEX.md`";
const DOCUMENT_START: &str = "<!-- kronn:resource\n";
const DOCUMENT_END: &str = "\n-->";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryDocument {
    pub schema_version: u32,
    pub kind: ProjectRepositoryResourceKind,
    pub slug: String,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub requires: Vec<String>,
    pub resource: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redacted_fields: Vec<RedactedField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryLockResource {
    pub kind: ProjectRepositoryResourceKind,
    pub slug: String,
    pub name: String,
    pub level: String,
    pub paths: Vec<String>,
    pub sha256: String,
    #[serde(default)]
    pub required_secrets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepositoryLock {
    pub version: u32,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub resources: Vec<RepositoryLockResource>,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

impl Default for RepositoryLock {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            updated_at: Utc::now(),
            resources: Vec::new(),
            files: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderedRepositoryResource {
    pub document: RepositoryDocument,
    pub name: String,
    pub files: BTreeMap<String, Vec<u8>>,
    pub hash: String,
    pub required_secrets: Vec<String>,
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Fingerprint the executable definition while excluding instance-local
/// identity, timestamps, favorites and the workflow activation toggle.
pub fn approval_hash(document: &RepositoryDocument) -> String {
    let mut resource = document.resource.clone();
    if let Some(object) = resource.as_object_mut() {
        for key in [
            "id",
            "project_id",
            "created_at",
            "updated_at",
            "pinned",
            "enabled",
        ] {
            object.remove(key);
        }
    }
    let value = serde_json::json!({
        "schema_version": document.schema_version,
        "kind": document.kind,
        "slug": document.slug,
        "requires": document.requires,
        "resource": resource,
    });
    sha256(&serde_json::to_vec(&value).unwrap_or_default())
}

fn resource_hash(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut digest = Sha256::new();
    for (path, bytes) in files {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update(bytes);
        digest.update([0]);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn secret_name(kind: ProjectRepositoryResourceKind, slug: &str, path: &str) -> String {
    let raw = format!("KRONN_{kind:?}_{slug}_{path}");
    let mut output = String::with_capacity(raw.len());
    let mut separator = false;
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_uppercase());
            separator = false;
        } else if !separator && !output.is_empty() {
            output.push('_');
            separator = true;
        }
    }
    output.trim_matches('_').to_string()
}

fn replace_redacted_values(
    value: &mut Value,
    kind: ProjectRepositoryResourceKind,
    slug: &str,
    path: &str,
    secrets: &mut BTreeSet<String>,
) {
    match value {
        Value::String(text) => {
            if let Some(name) = text
                .strip_prefix("secret://")
                .filter(|name| !name.is_empty())
            {
                secrets.insert(name.to_string());
                return;
            }
            let name = secret_name(kind, slug, path);
            if text == REDACTED {
                *text = format!("secret://{name}");
                secrets.insert(name);
                return;
            }
            let (redacted, count) = crate::core::redact::redact_for_audit_artifact(text);
            if count > 0 {
                *text = redacted.replace("***REDACTED***", &format!("secret://{name}"));
                secrets.insert(name);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                replace_redacted_values(item, kind, slug, &format!("{path}_{index}"), secrets);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                replace_redacted_values(item, kind, slug, &format!("{path}_{key}"), secrets);
            }
        }
        _ => {}
    }
}

fn document(
    kind: ProjectRepositoryResourceKind,
    slug: &str,
    updated_at: DateTime<Utc>,
    mut resource: Value,
    redacted_fields: Vec<RedactedField>,
) -> (RepositoryDocument, Vec<String>) {
    let mut secrets = BTreeSet::new();
    replace_redacted_values(&mut resource, kind, slug, "resource", &mut secrets);
    (
        RepositoryDocument {
            schema_version: SCHEMA_VERSION,
            kind,
            slug: slug.to_string(),
            updated_at,
            requires: matches!(
                kind,
                ProjectRepositoryResourceKind::Workflow | ProjectRepositoryResourceKind::QuickApi
            )
            .then(|| vec!["kronn".to_string()])
            .unwrap_or_default(),
            resource,
            redacted_fields,
        },
        secrets.into_iter().collect(),
    )
}

fn json_file(document: &RepositoryDocument) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(document)
        .map_err(|error| format!("cannot serialize repository resource: {error}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn markdown_file(
    document: &RepositoryDocument,
    _name: &str,
    description: &str,
    body: &str,
) -> Result<Vec<u8>, String> {
    let metadata = serde_json::to_string_pretty(document)
        .map_err(|error| format!("cannot serialize repository metadata: {error}"))?;
    let description = serde_json::to_string(description)
        .map_err(|error| format!("cannot serialize resource description: {error}"))?;
    Ok(format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}{}{}\n\n{}\n",
        document.slug, description, DOCUMENT_START, metadata, DOCUMENT_END, body
    )
    .into_bytes())
}

fn rendered(
    document: RepositoryDocument,
    name: String,
    files: BTreeMap<String, Vec<u8>>,
    required_secrets: Vec<String>,
) -> RenderedRepositoryResource {
    let hash = resource_hash(&files);
    RenderedRepositoryResource {
        document,
        name,
        files,
        hash,
        required_secrets,
    }
}

pub fn render_workflow(
    workflow: &Workflow,
    slug: &str,
) -> Result<RenderedRepositoryResource, String> {
    let mut exported = workflow.clone();
    // Repository workflows are definitions, not activation state. Imports are
    // always disabled locally; omitting that local toggle from the effective
    // fingerprint lets a human approve the definition, then enable it without
    // changing the content they reviewed.
    exported.enabled = false;
    let mut redacted = Vec::new();
    crate::core::export_secrets::redact_workflow(&mut exported, &mut redacted);
    let (document, secrets) = document(
        ProjectRepositoryResourceKind::Workflow,
        slug,
        workflow.updated_at,
        serde_json::to_value(exported).map_err(|error| error.to_string())?,
        redacted,
    );
    let files = BTreeMap::from([(
        format!("kronn/workflows/{slug}.yaml"),
        json_file(&document)?,
    )]);
    Ok(rendered(document, workflow.name.clone(), files, secrets))
}

pub fn render_quick_prompt(
    prompt: &QuickPrompt,
    slug: &str,
) -> Result<RenderedRepositoryResource, String> {
    let (document, secrets) = document(
        ProjectRepositoryResourceKind::QuickPrompt,
        slug,
        prompt.updated_at,
        serde_json::to_value(prompt).map_err(|error| error.to_string())?,
        Vec::new(),
    );
    let prompt_template = document
        .resource
        .get("prompt_template")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let files = BTreeMap::from([(
        format!("kronn/prompts/{slug}.md"),
        markdown_file(
            &document,
            &prompt.name,
            &prompt.description,
            prompt_template,
        )?,
    )]);
    Ok(rendered(document, prompt.name.clone(), files, secrets))
}

pub fn render_quick_api(api: &QuickApi, slug: &str) -> Result<RenderedRepositoryResource, String> {
    let mut exported = api.clone();
    let mut redacted = Vec::new();
    crate::core::export_secrets::redact_quick_api(&mut exported, &mut redacted);
    let (document, secrets) = document(
        ProjectRepositoryResourceKind::QuickApi,
        slug,
        api.updated_at,
        serde_json::to_value(exported).map_err(|error| error.to_string())?,
        redacted,
    );
    let files = BTreeMap::from([(
        format!("kronn/quick-apis/{slug}.yaml"),
        json_file(&document)?,
    )]);
    Ok(rendered(document, api.name.clone(), files, secrets))
}

pub fn render_quick_exec(
    exec: &QuickExec,
    slug: &str,
) -> Result<RenderedRepositoryResource, String> {
    let mut exported = exec.clone();
    let mut redacted = Vec::new();
    crate::core::export_secrets::redact_quick_exec(&mut exported, &mut redacted);
    let (document, secrets) = document(
        ProjectRepositoryResourceKind::QuickExec,
        slug,
        exec.updated_at,
        serde_json::to_value(exported).map_err(|error| error.to_string())?,
        redacted,
    );
    let files = BTreeMap::from([(
        format!("kronn/quick-execs/{slug}.yaml"),
        json_file(&document)?,
    )]);
    Ok(rendered(document, exec.name.clone(), files, secrets))
}

pub fn render_artifact(
    artifact: &ArtifactBundlePage,
    updated_at: DateTime<Utc>,
    slug: &str,
) -> Result<RenderedRepositoryResource, String> {
    let (document, secrets) = document(
        ProjectRepositoryResourceKind::Artifact,
        slug,
        updated_at,
        serde_json::to_value(artifact).map_err(|error| error.to_string())?,
        Vec::new(),
    );
    let html = document
        .resource
        .get("html")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let files = BTreeMap::from([
        (
            format!("kronn/artifacts/{slug}/artifact.yaml"),
            json_file(&document)?,
        ),
        (
            format!("kronn/artifacts/{slug}/index.html"),
            html.as_bytes().to_vec(),
        ),
    ]);
    Ok(rendered(document, artifact.title.clone(), files, secrets))
}

pub fn render_skill(
    skill: &Skill,
    updated_at: DateTime<Utc>,
    slug: &str,
) -> Result<RenderedRepositoryResource, String> {
    let (document, secrets) = document(
        ProjectRepositoryResourceKind::Skill,
        slug,
        updated_at,
        serde_json::to_value(skill).map_err(|error| error.to_string())?,
        Vec::new(),
    );
    let content = document
        .resource
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let files = BTreeMap::from([(
        format!("kronn/skills/{slug}/SKILL.md"),
        markdown_file(&document, &skill.name, &skill.description, content)?,
    )]);
    Ok(rendered(document, skill.name.clone(), files, secrets))
}

/// Refuses a `kronn` path that exists but is not a directory (e.g. a launcher
/// script): the repository then has no `kronn/` resources and cannot host one.
fn ensure_kronn_dir_available(root: &Path) -> Result<(), String> {
    let dir = root.join("kronn");
    if dir.exists() && !dir.is_dir() {
        return Err(format!(
            "{} exists and is not a directory; resources cannot be published into kronn/",
            dir.display()
        ));
    }
    Ok(())
}

pub fn load_lock(root: &Path) -> Result<Option<RepositoryLock>, String> {
    if !root.join("kronn").is_dir() {
        return Ok(None);
    }
    let path = root.join(LOCK_PATH);
    crate::core::fs_guard::assert_contained_no_symlink(root, &path)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let lock: RepositoryLock = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid {}: {error}", path.display()))?;
    if lock.version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported repository lock version {}",
            lock.version
        ));
    }
    for resource in &lock.resources {
        for path in &resource.paths {
            validate_relative(path)?;
        }
    }
    for path in lock.files.keys() {
        validate_relative(path)?;
    }
    Ok(Some(lock))
}

pub fn validate_relative(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "invalid repository resource path {}",
            path.display()
        ));
    }
    Ok(())
}

pub fn read_resource(
    root: &Path,
    entry: &RepositoryLockResource,
) -> Result<(RepositoryDocument, String), String> {
    let mut files = BTreeMap::new();
    for relative in &entry.paths {
        validate_relative(relative)?;
        let path = root.join(relative);
        crate::core::fs_guard::assert_contained_no_symlink(root, &path)?;
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        files.insert(relative.clone(), bytes);
    }
    let first = entry
        .paths
        .first()
        .and_then(|path| files.get(path))
        .ok_or_else(|| format!("resource {} has no definition file", entry.slug))?;
    let document: RepositoryDocument = if matches!(
        entry.kind,
        ProjectRepositoryResourceKind::QuickPrompt | ProjectRepositoryResourceKind::Skill
    ) {
        let text = std::str::from_utf8(first)
            .map_err(|_| format!("resource {} is not UTF-8", entry.slug))?;
        let start = text
            .find(DOCUMENT_START)
            .ok_or_else(|| format!("resource {} has no Kronn metadata", entry.slug))?
            + DOCUMENT_START.len();
        let end = text[start..]
            .find(DOCUMENT_END)
            .map(|offset| start + offset)
            .ok_or_else(|| format!("resource {} has incomplete Kronn metadata", entry.slug))?;
        let mut document: RepositoryDocument = serde_json::from_str(&text[start..end])
            .map_err(|error| format!("invalid resource metadata: {error}"))?;
        let suffix = &text[end + DOCUMENT_END.len()..];
        let body = suffix.strip_prefix("\n\n").unwrap_or(suffix);
        let body = body.strip_suffix('\n').unwrap_or(body);
        let body_field = match entry.kind {
            ProjectRepositoryResourceKind::QuickPrompt => "prompt_template",
            ProjectRepositoryResourceKind::Skill => "content",
            _ => unreachable!(),
        };
        if let Some(resource) = document.resource.as_object_mut() {
            resource.insert(body_field.into(), Value::String(body.to_string()));
        }
        document
    } else {
        serde_json::from_slice(first)
            .map_err(|error| format!("invalid repository resource: {error}"))?
    };
    if document.schema_version != SCHEMA_VERSION
        || document.kind != entry.kind
        || document.slug != entry.slug
    {
        return Err(format!("resource metadata does not match {}", entry.slug));
    }
    if entry.kind == ProjectRepositoryResourceKind::Artifact {
        let html_path = entry
            .paths
            .iter()
            .find(|path| path.ends_with("/index.html"))
            .ok_or_else(|| format!("artifact {} has no index.html", entry.slug))?;
        let html = String::from_utf8(files[html_path].clone())
            .map_err(|_| format!("artifact {} index is not UTF-8", entry.slug))?;
        let mut document = document;
        if let Some(resource) = document.resource.as_object_mut() {
            resource.insert("html".into(), Value::String(html));
        }
        return Ok((document, resource_hash(&files)));
    }
    Ok((document, resource_hash(&files)))
}

fn is_tracked(root: &Path, relative: &str) -> bool {
    crate::core::cmd::sync_cmd("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--error-unmatch", "--", relative])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn may_replace(
    root: &Path,
    relative: &str,
    previous: &RepositoryLock,
    allow_changed_owned: bool,
) -> Result<(), String> {
    validate_relative(relative)?;
    let path = root.join(relative);
    crate::core::fs_guard::assert_contained_no_symlink(root, &path)?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };
    if !metadata.is_file() {
        return Err(format!("refusing to replace non-file {}", path.display()));
    }
    if relative == LOCK_PATH {
        return Ok(());
    }
    let Some(expected) = previous.files.get(relative) else {
        let tracked = is_tracked(root, relative);
        return Err(format!(
            "refusing to overwrite {} file not written by Kronn: {relative}",
            if tracked { "Git-tracked" } else { "existing" }
        ));
    };
    let current =
        std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if !allow_changed_owned && sha256(&current) != *expected {
        return Err(format!("{relative} changed since the last alignment"));
    }
    Ok(())
}

fn write_atomic(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), String> {
    let path = root.join(relative);
    crate::core::fs_guard::assert_contained_no_symlink(root, &path)?;
    if let Some(parent) = path.parent() {
        crate::core::fs_guard::guarded_create_dir_all(root, parent)?;
    }
    crate::core::mcp_scanner::atomic_write_bytes(&path, bytes)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn index_markdown(lock: &RepositoryLock) -> Vec<u8> {
    let mut output = String::from("# Kronn resources\n\n");
    if lock.resources.is_empty() {
        output.push_str("No published resources.\n");
    } else {
        for resource in &lock.resources {
            let requirements = if resource.required_secrets.is_empty() {
                String::new()
            } else {
                format!("; secrets: {}", resource.required_secrets.join(", "))
            };
            output.push_str(&format!(
                "- **{}** (`{:?}:{}`) — {}{}\n",
                resource.name, resource.kind, resource.slug, resource.level, requirements
            ));
        }
    }
    output.into_bytes()
}

fn config_toml(project_key: &str, lock: &RepositoryLock) -> Result<Vec<u8>, String> {
    #[derive(Serialize)]
    struct Config<'a> {
        schema_version: u32,
        project: &'a str,
        required_secrets: Vec<String>,
    }
    let required_secrets = lock
        .resources
        .iter()
        .flat_map(|resource| resource.required_secrets.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    toml::to_string_pretty(&Config {
        schema_version: SCHEMA_VERSION,
        project: project_key,
        required_secrets,
    })
    .map(String::into_bytes)
    .map_err(|error| format!("cannot serialize kronn.toml: {error}"))
}

fn router_skill() -> Vec<u8> {
    b"---\nname: kronn\ndescription: Discover the Kronn resources published by this repository.\n---\n\nRead `kronn/INDEX.md`, then open only the resource needed for the task.\n".to_vec()
}

fn ensure_agents_line(root: &Path) -> Result<(), String> {
    let path = root.join(AGENTS_PATH);
    crate::core::fs_guard::assert_contained_no_symlink(root, &path)?;
    let current = match std::fs::read_to_string(&path) {
        Ok(current) => current,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if current.lines().any(|line| line.trim() == AGENTS_LINE) {
        return Ok(());
    }
    let next = if current.is_empty() {
        format!("{AGENTS_LINE}\n")
    } else {
        format!("{AGENTS_LINE}\n\n{current}")
    };
    write_atomic(root, AGENTS_PATH, next.as_bytes())
}

pub fn publish(
    root: &Path,
    project_key: &str,
    resource: RenderedRepositoryResource,
    allow_changed_owned: bool,
) -> Result<RepositoryLockResource, String> {
    ensure_kronn_dir_available(root)?;
    let previous = load_lock(root)?.unwrap_or_default();
    let mut next = previous.clone();
    next.updated_at = Utc::now();
    next.resources
        .retain(|item| item.kind != resource.document.kind || item.slug != resource.document.slug);
    let entry = RepositoryLockResource {
        kind: resource.document.kind,
        slug: resource.document.slug.clone(),
        name: resource.name,
        level: if matches!(
            resource.document.kind,
            ProjectRepositoryResourceKind::QuickPrompt
                | ProjectRepositoryResourceKind::QuickExec
                | ProjectRepositoryResourceKind::Skill
        ) {
            "N1".into()
        } else {
            "N0".into()
        },
        paths: resource.files.keys().cloned().collect(),
        sha256: resource.hash,
        required_secrets: resource.required_secrets,
    };
    next.resources.push(entry.clone());
    next.resources
        .sort_by_key(|item| (format!("{:?}", item.kind), item.slug.clone()));

    let index = index_markdown(&next);
    let config = config_toml(project_key, &next)?;
    let router = router_skill();
    let mut writes = resource.files;
    writes.insert(INDEX_PATH.into(), index);
    writes.insert(CONFIG_PATH.into(), config);
    writes.insert(ROUTER_PATH.into(), router);

    for relative in writes.keys() {
        may_replace(root, relative, &previous, allow_changed_owned)?;
    }
    may_replace(root, LOCK_PATH, &previous, allow_changed_owned)?;

    ensure_agents_line(root)?;
    for (relative, bytes) in &writes {
        write_atomic(root, relative, bytes)?;
    }
    next.files = previous.files.clone();
    for (path, bytes) in &writes {
        next.files.insert(path.clone(), sha256(bytes));
    }
    let mut lock_bytes = serde_json::to_vec_pretty(&next)
        .map_err(|error| format!("cannot serialize kronn.lock: {error}"))?;
    lock_bytes.push(b'\n');
    write_atomic(root, LOCK_PATH, &lock_bytes)?;
    Ok(entry)
}

pub fn simple_diff(repository: &[u8], kronn: &[u8]) -> String {
    let repository = String::from_utf8_lossy(repository);
    let kronn = String::from_utf8_lossy(kronn);
    if repository == kronn {
        return String::new();
    }
    let mut output = String::from("--- repository\n+++ Kronn\n");
    for line in repository.lines().take(80) {
        output.push('-');
        output.push_str(line);
        output.push('\n');
    }
    for line in kronn.lines().take(80) {
        output.push('+');
        output.push_str(line);
        output.push('\n');
    }
    output
}

pub fn ensure_execution_approved(
    conn: &rusqlite::Connection,
    kind: &str,
    target_id: &str,
    current_hash: &str,
) -> Result<(), String> {
    let alignment =
        crate::db::repository_resources::find_alignment_by_target(conn, kind, target_id)
            .map_err(|error| error.to_string())?;
    let Some(alignment) = alignment else {
        return Ok(());
    };
    if !alignment.imported {
        return Ok(());
    }
    let approved = crate::db::repository_resources::is_approved(
        conn,
        &alignment.project_key,
        &alignment.kind,
        &alignment.slug,
        current_hash,
    )
    .map_err(|error| error.to_string())?;
    if approved {
        Ok(())
    } else {
        Err(format!(
            "{}:{} must be approved for its current content hash before execution",
            alignment.kind, alignment.slug
        ))
    }
}

fn ensure_rendered_execution_approved(
    conn: &rusqlite::Connection,
    kind: &str,
    target_id: &str,
    render: impl FnOnce(&str) -> Result<RenderedRepositoryResource, String>,
) -> Result<(), String> {
    let alignment =
        crate::db::repository_resources::find_alignment_by_target(conn, kind, target_id)
            .map_err(|error| error.to_string())?;
    let Some(alignment) = alignment else {
        return Ok(());
    };
    if !alignment.imported {
        return Ok(());
    }
    let rendered = render(&alignment.slug)?;
    ensure_execution_approved(conn, kind, target_id, &approval_hash(&rendered.document))
}

pub fn ensure_workflow_execution_approved(
    conn: &rusqlite::Connection,
    workflow: &Workflow,
) -> Result<(), String> {
    ensure_rendered_execution_approved(conn, "workflow", &workflow.id, |slug| {
        render_workflow(workflow, slug)
    })
}

pub fn ensure_quick_prompt_execution_approved(
    conn: &rusqlite::Connection,
    prompt: &QuickPrompt,
) -> Result<(), String> {
    ensure_rendered_execution_approved(conn, "quick_prompt", &prompt.id, |slug| {
        render_quick_prompt(prompt, slug)
    })
}

pub fn ensure_quick_api_execution_approved(
    conn: &rusqlite::Connection,
    api: &QuickApi,
) -> Result<(), String> {
    ensure_rendered_execution_approved(conn, "quick_api", &api.id, |slug| {
        render_quick_api(api, slug)
    })
}

pub fn ensure_quick_exec_execution_approved(
    conn: &rusqlite::Connection,
    exec: &QuickExec,
) -> Result<(), String> {
    ensure_rendered_execution_approved(conn, "quick_exec", &exec.id, |slug| {
        render_quick_exec(exec, slug)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AgentType, ModelTier};
    use chrono::TimeZone;

    fn sample_exec(secret: &str) -> QuickExec {
        QuickExec {
            id: "qe-1".into(),
            name: "Deploy".into(),
            icon: "terminal".into(),
            description: String::new(),
            project_id: Some("project-1".into()),
            command: "deploy".into(),
            args: vec!["--token".into(), secret.into()],
            timeout_secs: 30,
            output_format: Default::default(),
            variables: Vec::new(),
            pinned: false,
            created_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            updated_at: Utc.timestamp_opt(1_700_000_010, 0).unwrap(),
        }
    }

    fn sample_prompt(body: &str) -> QuickPrompt {
        QuickPrompt {
            id: "qp-1".into(),
            name: "Deploy prompt".into(),
            icon: "terminal".into(),
            prompt_template: body.into(),
            variables: Vec::new(),
            agent: AgentType::ClaudeCode,
            connection_id: None,
            project_id: Some("project-1".into()),
            skill_ids: Vec::new(),
            profile_ids: Vec::new(),
            directive_ids: Vec::new(),
            tier: ModelTier::default(),
            agent_settings: None,
            description: String::new(),
            pinned: false,
            created_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            updated_at: Utc.timestamp_opt(1_700_000_010, 0).unwrap(),
        }
    }

    fn sample_api(path: &str) -> QuickApi {
        QuickApi {
            id: "qa-1".into(),
            name: "Deploy API".into(),
            icon: "api".into(),
            description: String::new(),
            project_id: Some("project-1".into()),
            api_plugin_slug: "api-deploy".into(),
            api_config_id: "config-1".into(),
            api_endpoint_path: path.into(),
            api_method: Some("POST".into()),
            api_query: None,
            api_path_params: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            variables: Vec::new(),
            profile_ids: Vec::new(),
            directive_ids: Vec::new(),
            pinned: false,
            created_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            updated_at: Utc.timestamp_opt(1_700_000_010, 0).unwrap(),
        }
    }

    fn mark_imported(conn: &rusqlite::Connection, kind: &str, slug: &str, target_id: &str) {
        crate::db::repository_resources::upsert_alignment(
            conn,
            "repo-1",
            kind,
            slug,
            target_id,
            "repository-hash",
            "database-hash",
            "2026-09-28T00:00:00Z",
            true,
        )
        .unwrap();
    }

    #[test]
    fn quick_prompt_execution_requires_approval_for_the_current_fingerprint_only() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();

        let local = sample_prompt("local prompt");
        assert!(ensure_quick_prompt_execution_approved(&conn, &local).is_ok());

        let imported = sample_prompt("imported prompt");
        mark_imported(&conn, "quick_prompt", "deploy-prompt", &imported.id);
        assert!(ensure_quick_prompt_execution_approved(&conn, &imported).is_err());

        let rendered = render_quick_prompt(&imported, "deploy-prompt").unwrap();
        crate::db::repository_resources::approve(
            &conn,
            "repo-1",
            "quick_prompt",
            "deploy-prompt",
            &approval_hash(&rendered.document),
        )
        .unwrap();
        assert!(ensure_quick_prompt_execution_approved(&conn, &imported).is_ok());

        let changed = sample_prompt("changed after approval");
        assert!(ensure_quick_prompt_execution_approved(&conn, &changed).is_err());
    }

    #[test]
    fn quick_api_execution_requires_approval_for_the_current_fingerprint_only() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();

        let local = sample_api("/local");
        assert!(ensure_quick_api_execution_approved(&conn, &local).is_ok());

        let imported = sample_api("/imported");
        mark_imported(&conn, "quick_api", "deploy-api", &imported.id);
        assert!(ensure_quick_api_execution_approved(&conn, &imported).is_err());

        let rendered = render_quick_api(&imported, "deploy-api").unwrap();
        crate::db::repository_resources::approve(
            &conn,
            "repo-1",
            "quick_api",
            "deploy-api",
            &approval_hash(&rendered.document),
        )
        .unwrap();
        assert!(ensure_quick_api_execution_approved(&conn, &imported).is_ok());

        let changed = sample_api("/changed-after-approval");
        assert!(ensure_quick_api_execution_approved(&conn, &changed).is_err());
    }

    #[test]
    fn rendering_replaces_secret_values_with_named_references() {
        let rendered = render_quick_exec(&sample_exec("literal-token"), "deploy").unwrap();
        let text = String::from_utf8(rendered.files.values().next().unwrap().clone()).unwrap();
        assert!(!text.contains("literal-token"));
        assert!(text.contains("secret://KRONN_QUICKEXEC_DEPLOY_RESOURCE_ARGS_1"));
        assert_eq!(rendered.required_secrets.len(), 1);
    }

    #[test]
    fn rendering_removes_secret_literals_from_free_form_content() {
        let prompt = sample_prompt("Deploy with password=hunter2");
        let rendered = render_quick_prompt(&prompt, "deploy-prompt").unwrap();
        let text = String::from_utf8(rendered.files.values().next().unwrap().clone()).unwrap();
        assert!(!text.contains("hunter2"));
        assert!(text.contains("secret://KRONN_QUICKPROMPT_DEPLOY_PROMPT_RESOURCE_PROMPT_TEMPLATE"));
        assert_eq!(rendered.required_secrets.len(), 1);
    }

    #[test]
    fn rendering_preserves_and_declares_existing_secret_references() {
        let (_, secrets) = document(
            ProjectRepositoryResourceKind::QuickApi,
            "deploy",
            Utc.timestamp_opt(1_700_000_010, 0).unwrap(),
            serde_json::json!({"token": "secret://DEPLOY_PASSWORD"}),
            Vec::new(),
        );
        assert_eq!(secrets, vec!["DEPLOY_PASSWORD"]);
    }

    #[test]
    fn reading_markdown_imports_the_edited_body() {
        let root = tempfile::TempDir::new().unwrap();
        let rendered = render_quick_prompt(&sample_prompt("original"), "deploy-prompt").unwrap();
        let relative = "kronn/prompts/deploy-prompt.md";
        let path = root.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut text = String::from_utf8(rendered.files[relative].clone()).unwrap();
        let metadata_end = text.find(DOCUMENT_END).unwrap() + DOCUMENT_END.len();
        text.truncate(metadata_end);
        text.push_str("\n\nedited in Git\n");
        std::fs::write(&path, text).unwrap();
        let entry = RepositoryLockResource {
            kind: ProjectRepositoryResourceKind::QuickPrompt,
            slug: "deploy-prompt".into(),
            name: "Deploy prompt".into(),
            level: "N1".into(),
            paths: vec![relative.into()],
            sha256: String::new(),
            required_secrets: Vec::new(),
        };
        let (document, _) = read_resource(root.path(), &entry).unwrap();
        assert_eq!(document.resource["prompt_template"], "edited in Git");
    }

    #[test]
    fn first_publication_creates_scaffold_but_listing_alone_does_not() {
        let root = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        std::fs::write(root.path().join(AGENTS_PATH), "# Instructions\n").unwrap();
        assert!(!root.path().join("kronn").exists());
        let rendered = render_quick_exec(&sample_exec("literal-token"), "deploy").unwrap();
        publish(root.path(), "repo", rendered, false).unwrap();
        assert!(root.path().join("kronn/quick-execs/deploy.yaml").is_file());
        assert!(root.path().join(LOCK_PATH).is_file());
        assert!(root.path().join(CONFIG_PATH).is_file());
        assert!(root.path().join(INDEX_PATH).is_file());
        assert!(root.path().join(ROUTER_PATH).is_file());
        assert!(std::fs::read_to_string(root.path().join(AGENTS_PATH))
            .unwrap()
            .contains(AGENTS_LINE));
    }

    #[test]
    fn publication_refuses_an_existing_foreign_resource_file() {
        let root = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(root.path().join("kronn/quick-execs")).unwrap();
        std::fs::write(
            root.path().join("kronn/quick-execs/deploy.yaml"),
            "human content",
        )
        .unwrap();
        let rendered = render_quick_exec(&sample_exec("literal-token"), "deploy").unwrap();
        let error = publish(root.path(), "repo", rendered, false).unwrap_err();
        assert!(error.contains("not written by Kronn"), "{error}");
        assert_eq!(
            std::fs::read_to_string(root.path().join("kronn/quick-execs/deploy.yaml")).unwrap(),
            "human content"
        );
    }

    #[test]
    fn a_kronn_file_means_no_resources_and_refuses_publication() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("kronn"), "#!/bin/sh\n").unwrap();
        assert!(load_lock(root.path()).unwrap().is_none());
        let rendered = render_quick_prompt(&sample_prompt("body"), "deploy-prompt").unwrap();
        let error = publish(root.path(), "repo", rendered, false).unwrap_err();
        assert!(error.contains("is not a directory"), "{error}");
        assert!(root.path().join("kronn").is_file(), "the launcher must stay untouched");
    }
}
