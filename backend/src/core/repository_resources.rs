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

/// The ASCII letters an accented Latin letter stands for, `None` when it has
/// no plain-ASCII reading.
fn ascii_fold(character: char) -> Option<&'static str> {
    Some(match character {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// A file-name slug made of `a-z`, `0-9` and single hyphens only: accents fold
/// to their base letter (`réduit` → `reduit`), anything else becomes a
/// separator. Used for the files Kronn writes into a repository; slugs already
/// recorded for existing resources are never recomputed.
pub fn ascii_slug(label: &str) -> String {
    let mut folded = String::with_capacity(label.len());
    for character in label.to_lowercase().chars() {
        if ('\u{300}'..='\u{36f}').contains(&character) {
            continue;
        }
        if let Some(plain) = ascii_fold(character) {
            folded.push_str(plain);
        } else if character.is_ascii_alphanumeric() {
            folded.push(character);
        } else {
            folded.push('-');
        }
    }
    folded
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Short form of a content hash, enough to tell two versions apart by eye.
pub fn fingerprint(hash: &str) -> String {
    hash.chars().take(8).collect()
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Top-level resource fields that belong to one Kronn instance, not to the
/// definition: identity, timestamps, favorites and the workflow toggle.
const INSTANCE_LOCAL_FIELDS: [&str; 6] = [
    "id",
    "project_id",
    "created_at",
    "updated_at",
    "pinned",
    "enabled",
];

/// Fingerprint the executable definition while excluding instance-local
/// identity, timestamps, favorites and the workflow activation toggle.
pub fn approval_hash(document: &RepositoryDocument) -> String {
    let mut resource = document.resource.clone();
    if let Some(object) = resource.as_object_mut() {
        for key in INSTANCE_LOCAL_FIELDS {
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

/// Version of the router skill model. Bump it whenever `ROUTER_SKILL_BODY`
/// changes so a repository can tell which model wrote its copy; publication
/// regenerates the file every time, and refuses to replace a copy a human
/// edited (`may_replace`) like any other managed file.
const ROUTER_SKILL_VERSION: u32 = 1;

/// Where a human reads about Kronn and installs it. Both are taken from the
/// project README (a test keeps them there), never invented here.
const KRONN_REPOSITORY_URL: &str = "https://github.com/DocRoms/Kronn";
const KRONN_RELEASES_URL: &str = "https://github.com/DocRoms/Kronn/releases/latest";

const ROUTER_SKILL_DESCRIPTION: &str = "Explains the shared AI resources in this repository's kronn/ folder. Use it when asked to run a Quick Prompt (QP), Quick Exec (QE), workflow or automation, when kronn/INDEX.md is mentioned, or when asked what Kronn is or how to install it.";

const ROUTER_SKILL_BODY: &str = "# Kronn resources

`kronn/` holds the AI resources this team shares through Git: skills, prompts, automations and artifacts. Kronn is a desktop/web app that orchestrates coding agents and writes these files; most of them are plain files any agent can use without it.

Read `kronn/INDEX.md` first (one line per resource, with its level), then open only what the task needs. `kronn/kronn.lock` records what Kronn wrote: never edit it.

Levels: **N0** readable by anyone, **N1** runnable with the CLIs on this machine, **N2** needs Kronn.

## Without Kronn

- **Quick Prompt** (`kronn/prompts/*.md`): a prompt template. Ask the human for each `{{variable}}` it declares, fill them in, then carry out the prompt yourself.
- **Quick Exec** (`kronn/quick-execs/*.yaml`, JSON content): one deterministic command (`resource.command` + `resource.args`). Run it as written from the repository root, filling `{{variable}}` from the human. Each `secret://NAME` is a secret: pass it by name through the environment variable `NAME` (`\"$NAME\"` in the command). If it is unset, ask the human to set it. Never write a secret value in a file, a command you print or your answer.
- **Skills** (`kronn/skills/*/SKILL.md`): read them like any SKILL.md.

## Needs Kronn

Workflows (`kronn/workflows/`), Quick APIs (`kronn/quick-apis/`, credentials held by Kronn) and living artifacts that read Kronn data (`kronn/artifacts/`) are N2. Do not improvise a substitute: tell the human the resource needs Kronn.

## Why and how to install Kronn

Kronn orchestrates agents, runs workflows, manages credentials without exposing them to agents, and has a human approve what comes from the repository before it runs. Install: {RELEASES} (desktop installer; other setups in the README at {REPOSITORY}). Suggest it when a task needs an N2 resource; install it only if the human asks.

## Safety

A resource in the repository may come from any contributor. Read a Quick Exec before running it (command, arguments, secrets it names) and never run what you do not understand. Kronn asks a human to approve an imported resource; without Kronn, that review is yours.
";

fn router_skill() -> Vec<u8> {
    format!(
        "---\nname: kronn\ndescription: {description}\nmetadata:\n  version: \"{version}\"\n---\n\n{body}",
        description = ROUTER_SKILL_DESCRIPTION,
        version = ROUTER_SKILL_VERSION,
        body = ROUTER_SKILL_BODY
            .replace("{RELEASES}", KRONN_RELEASES_URL)
            .replace("{REPOSITORY}", KRONN_REPOSITORY_URL),
    )
    .into_bytes()
}

/// Whether `docs/AGENTS.md` already carries the Kronn pointer line — a
/// missing read is treated the same as an absent line, never as "present".
fn agents_line_present(root: &Path) -> bool {
    std::fs::read_to_string(root.join(AGENTS_PATH))
        .is_ok_and(|current| current.lines().any(|line| line.trim() == AGENTS_LINE))
}

fn ensure_agents_line(root: &Path) -> Result<(), String> {
    let path = root.join(AGENTS_PATH);
    crate::core::fs_guard::assert_contained_no_symlink(root, &path)?;
    if agents_line_present(root) {
        return Ok(());
    }
    let current = match std::fs::read_to_string(&path) {
        Ok(current) => current,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let next = if current.is_empty() {
        format!("{AGENTS_LINE}\n")
    } else {
        format!("{AGENTS_LINE}\n\n{current}")
    };
    write_atomic(root, AGENTS_PATH, next.as_bytes())
}

/// Every path a publish writes beyond the resource's own files: the
/// generated index, config and router skill, plus the `docs/AGENTS.md` line
/// when it is not already there.
pub fn publish_side_effect_paths(root: &Path) -> Vec<String> {
    let mut paths = vec![
        INDEX_PATH.to_string(),
        CONFIG_PATH.to_string(),
        ROUTER_PATH.to_string(),
    ];
    if !agents_line_present(root) {
        paths.push(AGENTS_PATH.to_string());
    }
    paths
}

/// Whether a publish can write into this repository right now, and why not
/// otherwise. Checked ahead of time so a write attempt never surfaces as a
/// mid-publish error for a condition that was already knowable.
pub fn can_write_repository(root: &Path) -> (bool, Option<crate::models::RepositoryWriteBlocker>) {
    use crate::models::RepositoryWriteBlocker;
    let kronn = root.join("kronn");
    if kronn.exists() && !kronn.is_dir() {
        return (false, Some(RepositoryWriteBlocker::KronnPathIsFile));
    }
    match std::fs::metadata(root) {
        Ok(metadata) if metadata.permissions().readonly() => {
            (false, Some(RepositoryWriteBlocker::RepositoryReadOnly))
        }
        Ok(_) => (true, None),
        Err(_) => (false, Some(RepositoryWriteBlocker::RepositoryUnreadable)),
    }
}

/// Repository-relative paths Kronn wrote (per `kronn.lock`) that `git status`
/// reports as modified or untracked. Best-effort: any git failure (no repo,
/// no binary) reports no uncommitted paths rather than failing the caller.
pub fn uncommitted_managed_paths(root: &Path, lock: &RepositoryLock) -> Vec<String> {
    if lock.files.is_empty() {
        return Vec::new();
    }
    let output = crate::core::cmd::sync_cmd("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "-uall", "--"])
        .args(lock.files.keys())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.get(3..))
        .map(str::to_string)
        .collect()
}

/// The repository's own account of when its managed files last changed: the
/// author and date of each tracked file's last commit, else the filesystem
/// mtime (checkout/pull do not preserve commit dates, so the commit date is
/// always preferred when it is known).
///
/// Read once per listing — one `git ls-files` and one `git log` that stops as
/// soon as every tracked file is resolved — instead of one `git log` per file,
/// which walks the whole history for each path it cannot answer quickly. The
/// walk gets a time budget: on a very deep history the files it has not
/// reached yet fall back to their mtime rather than keep the listing waiting.
#[derive(Default)]
pub struct RepositoryFileDates {
    root: std::path::PathBuf,
    commits: BTreeMap<String, (DateTime<Utc>, String)>,
}

/// How long the listing waits for git to date the tracked files.
const FILE_DATES_BUDGET: std::time::Duration = std::time::Duration::from_millis(600);

impl RepositoryFileDates {
    /// Last commit of every tracked file under `directories` (relative to
    /// `root`). Any git failure leaves the index empty: every lookup then
    /// falls back to the file's mtime.
    pub fn read(root: &Path, directories: &[&str]) -> Self {
        Self::read_within(root, directories, FILE_DATES_BUDGET)
    }

    fn read_within(root: &Path, directories: &[&str], budget: std::time::Duration) -> Self {
        let mut dates = Self {
            root: root.to_path_buf(),
            commits: BTreeMap::new(),
        };
        let existing: Vec<&str> = directories
            .iter()
            .copied()
            .filter(|directory| root.join(directory).is_dir())
            .collect();
        if existing.is_empty() {
            return dates;
        }
        let listed = crate::core::cmd::sync_cmd("git")
            .arg("-C")
            .arg(root)
            .args(["-c", "core.quotepath=off", "ls-files", "-z", "--"])
            .args(&existing)
            .output();
        let Ok(listed) = listed else {
            return dates;
        };
        if !listed.status.success() {
            return dates;
        }
        let mut pending: BTreeSet<String> = listed
            .stdout
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect();
        if pending.is_empty() {
            return dates;
        }
        let spawned = crate::core::cmd::sync_cmd("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "core.quotepath=off",
                "log",
                "--format=%x1e%aI%x1f%an",
                "--name-only",
                "--no-renames",
                "--relative",
                "--",
            ])
            .args(&existing)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut child) = spawned else {
            return dates;
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return dates;
        };
        // A thread feeds the lines so the wait for the next one can time out:
        // git is silent while it walks commits that touch none of our files.
        let (sender, receiver) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
            {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = std::time::Instant::now() + budget;
        let mut current: Option<(DateTime<Utc>, String)> = None;
        while let Ok(line) =
            receiver.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            if let Some(header) = line.strip_prefix('\u{1e}') {
                current = header.split_once('\u{1f}').and_then(|(date, author)| {
                    parse_rfc3339(date).map(|date| (date, author.to_string()))
                });
            } else if let Some(commit) = current.as_ref() {
                if pending.remove(line.as_str()) {
                    dates.commits.insert(line, commit.clone());
                    if pending.is_empty() {
                        break;
                    }
                }
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        dates
    }

    /// Date and author of `relative`'s last change. Nothing for a path that is
    /// not there: it has no history worth looking up.
    pub fn updated_at(&self, relative: &str) -> (Option<DateTime<Utc>>, Option<String>) {
        if let Some((date, author)) = self.commits.get(relative) {
            return (Some(*date), Some(author.clone()));
        }
        let modified = std::fs::metadata(self.root.join(relative))
            .and_then(|metadata| metadata.modified())
            .ok()
            .map(DateTime::<Utc>::from);
        (modified, None)
    }
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .ok()
}

/// Required secret names with whether Kronn's encrypted store holds each one
/// (`configured` is the set from `configured_secret_names`).
pub fn required_secret_statuses(
    names: &[String],
    configured: &BTreeSet<String>,
) -> Vec<crate::models::RequiredSecretStatus> {
    names
        .iter()
        .map(|name| crate::models::RequiredSecretStatus {
            name: name.clone(),
            configured: configured.contains(name),
        })
        .collect()
}

/// Env-key names held by the MCP/API configs a project can use (its own and
/// the global ones): the store ADR-005 resolves `secret://NAME` against.
/// Only the key names are read, never a value.
pub fn configured_secret_names(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> anyhow::Result<BTreeSet<String>> {
    Ok(crate::db::mcps::configs_for_project(conn, project_id)?
        .into_iter()
        .flat_map(|config| config.env_keys)
        .collect())
}

/// ADR-005's portability tier for a resource, from its declared `requires`
/// and — for artifacts, whose dataset bindings are not visible in `requires`
/// — whether it reads a Kronn dataset. Workflow and Quick API always declare
/// `requires: [kronn]` in phase 1 (`document()`), so both land on N2 until a
/// later phase can tell a portable runbook apart from one that cannot run
/// without Kronn.
pub fn resource_adr_level(document: &RepositoryDocument) -> crate::models::ResourceAdrLevel {
    use crate::models::ResourceAdrLevel;
    if document.requires.iter().any(|item| item == "kronn") {
        return ResourceAdrLevel::N2;
    }
    match document.kind {
        ProjectRepositoryResourceKind::Skill
        | ProjectRepositoryResourceKind::QuickPrompt
        | ProjectRepositoryResourceKind::QuickExec => ResourceAdrLevel::N1,
        ProjectRepositoryResourceKind::Artifact => {
            let reads_dataset = document
                .resource
                .get("datasets")
                .and_then(Value::as_array)
                .is_some_and(|datasets| !datasets.is_empty());
            if reads_dataset {
                ResourceAdrLevel::N2
            } else {
                ResourceAdrLevel::N1
            }
        }
        ProjectRepositoryResourceKind::Workflow | ProjectRepositoryResourceKind::QuickApi => {
            ResourceAdrLevel::N2
        }
    }
}

fn adr_level_str(level: crate::models::ResourceAdrLevel) -> &'static str {
    use crate::models::ResourceAdrLevel;
    match level {
        ResourceAdrLevel::N0 => "N0",
        ResourceAdrLevel::N1 => "N1",
        ResourceAdrLevel::N2 => "N2",
    }
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
        level: adr_level_str(resource_adr_level(&resource.document)).to_string(),
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

/// Cell budget for the LCS table (`u32` per cell): about 8 MB at the cap.
/// It bounds the changed middle only, the unchanged head and tail being
/// trimmed first; above it a rewritten file falls back to a coarser diff
/// instead of an unbounded-memory comparison.
const MAX_LCS_CELLS: usize = 2_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffOp {
    Equal,
    Delete,
    Insert,
}

/// Line-based LCS table: `table[i][j]` is the LCS length of `a[i..]` and
/// `b[j..]`. `O(n*m)` time and space, bounded by `MAX_LCS_CELLS`.
fn lcs_diff_ops(a: &[&str], b: &[&str]) -> Vec<(DiffOp, usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push((DiffOp::Equal, i, j));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            ops.push((DiffOp::Delete, i, j));
            i += 1;
        } else {
            ops.push((DiffOp::Insert, i, j));
            j += 1;
        }
    }
    while i < n {
        ops.push((DiffOp::Delete, i, j));
        i += 1;
    }
    while j < m {
        ops.push((DiffOp::Insert, i, j));
        j += 1;
    }
    ops
}

/// One `@@` hunk spanning every change plus three lines of context on each
/// side. A real `diff -u` splits distant changes into separate hunks; a
/// single hunk stays correct (right lines, right line numbers) and is far
/// simpler, which single-resource files do not need the extra split for.
fn render_unified_diff(a: &[&str], b: &[&str], ops: &[(DiffOp, usize, usize)]) -> String {
    const CONTEXT: usize = 3;
    let Some(first_change) = ops.iter().position(|(op, _, _)| *op != DiffOp::Equal) else {
        return String::new();
    };
    let last_change = ops
        .iter()
        .rposition(|(op, _, _)| *op != DiffOp::Equal)
        .unwrap_or(first_change);
    let start = first_change.saturating_sub(CONTEXT);
    let end = (last_change + 1 + CONTEXT).min(ops.len());
    let window = &ops[start..end];

    let a_start = window.first().map_or(0, |(_, i, _)| *i);
    let b_start = window.first().map_or(0, |(_, _, j)| *j);
    let a_count = window
        .iter()
        .filter(|(op, _, _)| *op != DiffOp::Insert)
        .count();
    let b_count = window
        .iter()
        .filter(|(op, _, _)| *op != DiffOp::Delete)
        .count();

    let mut output = String::from("--- repository\n+++ Kronn\n");
    output.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        a_start + 1,
        a_count,
        b_start + 1,
        b_count
    ));
    for (op, i, j) in window {
        match op {
            DiffOp::Equal => {
                output.push(' ');
                output.push_str(a[*i]);
            }
            DiffOp::Delete => {
                output.push('-');
                output.push_str(a[*i]);
            }
            DiffOp::Insert => {
                output.push('+');
                output.push_str(b[*j]);
            }
        }
        output.push('\n');
    }
    output
}

fn truncated_diff(a: &[&str], b: &[&str]) -> String {
    let mut output = String::from("--- repository\n+++ Kronn\n");
    for line in a.iter().take(80) {
        output.push('-');
        output.push_str(line);
        output.push('\n');
    }
    for line in b.iter().take(80) {
        output.push('+');
        output.push_str(line);
        output.push('\n');
    }
    output
}

/// A real unified diff between two file contents (the repository's and
/// Kronn's), line by line — the artifact HTML included. Built in-house on a
/// plain LCS rather than pulling in a diff crate: no dependency is worth
/// adding for what a bounded dynamic program already gives us.
pub fn unified_diff(repository: &[u8], kronn: &[u8]) -> String {
    let repository = String::from_utf8_lossy(repository);
    let kronn = String::from_utf8_lossy(kronn);
    if repository == kronn {
        return String::new();
    }
    let a: Vec<&str> = repository.lines().collect();
    let b: Vec<&str> = kronn.lines().collect();
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a_mid, b_mid) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    if a_mid.len().saturating_mul(b_mid.len()) > MAX_LCS_CELLS {
        return truncated_diff(&a, &b);
    }
    let mut ops: Vec<(DiffOp, usize, usize)> = (0..head).map(|i| (DiffOp::Equal, i, i)).collect();
    ops.extend(
        lcs_diff_ops(a_mid, b_mid)
            .into_iter()
            .map(|(op, i, j)| (op, i + head, j + head)),
    );
    ops.extend((0..tail).map(|k| (DiffOp::Equal, a.len() - tail + k, b.len() - tail + k)));
    render_unified_diff(&a, &b, &ops)
}

fn flatten_json(value: &Value, prefix: String, out: &mut BTreeMap<String, Value>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, item) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_json(item, path, out);
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (index, item) in items.iter().enumerate() {
                flatten_json(item, format!("{prefix}.{index}"), out);
            }
        }
        _ => {
            out.insert(prefix, value.clone());
        }
    }
}

/// Field-by-field diff of a resource's definition: nested JSON flattened to
/// dotted-path leaves (`steps.0.agent`, `trigger.cron`) so a workflow's
/// trigger, an exec's command or a step's agent/model each surface as its
/// own row instead of one opaque "the file changed".
pub fn field_diff(
    repository: &Value,
    kronn: &Value,
) -> Vec<crate::models::RepositoryResourceFieldDiff> {
    let mut repository_flat = BTreeMap::new();
    flatten_json(repository, String::new(), &mut repository_flat);
    let mut kronn_flat = BTreeMap::new();
    flatten_json(kronn, String::new(), &mut kronn_flat);
    let mut fields: BTreeSet<String> = repository_flat.keys().cloned().collect();
    fields.extend(kronn_flat.keys().cloned());
    fields
        .into_iter()
        .filter(|field| !field.is_empty())
        .filter(|field| {
            !INSTANCE_LOCAL_FIELDS
                .iter()
                .any(|local| field.split('.').next() == Some(*local))
        })
        .filter_map(|field| {
            let repository_value = repository_flat.get(&field).cloned();
            let kronn_value = kronn_flat.get(&field).cloned();
            (repository_value != kronn_value).then_some(
                crate::models::RepositoryResourceFieldDiff {
                    field,
                    repository: repository_value,
                    kronn: kronn_value,
                },
            )
        })
        .collect()
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

    fn published_router(root: &Path) -> String {
        std::fs::read_to_string(root.join(ROUTER_PATH)).unwrap()
    }

    #[test]
    fn the_router_skill_explains_kronn_and_how_to_use_a_resource_without_it() {
        let root = tempfile::TempDir::new().unwrap();
        let rendered = render_quick_exec(&sample_exec("literal-token"), "deploy").unwrap();
        publish(root.path(), "repo", rendered, false).unwrap();
        let text = published_router(root.path());

        let (frontmatter, body) = text
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .expect("the router skill starts with Agent Skills frontmatter");
        assert!(frontmatter.contains("name: kronn\n"), "{frontmatter}");
        let description = frontmatter
            .lines()
            .find_map(|line| line.strip_prefix("description: "))
            .expect("the frontmatter carries a description");
        // Agent Skills caps the description at 1024 characters; it is what
        // every session loads, so it also has to stay short and specific.
        assert!(description.len() <= 400, "{}", description.len());
        for trigger in ["Quick Prompt", "Quick Exec", "kronn/INDEX.md", "install"] {
            assert!(
                description.contains(trigger),
                "{trigger} not in {description}"
            );
        }

        for expected in [
            // What it is: the folder, the index, the lock and the levels.
            "`kronn/INDEX.md`",
            "`kronn/kronn.lock`",
            "**N0**",
            "**N1**",
            "**N2**",
            // Without Kronn.
            "`kronn/prompts/*.md`",
            "`{{variable}}`",
            "`kronn/quick-execs/*.yaml`",
            "from the repository root",
            "`secret://NAME`",
            "environment variable `NAME`",
            "Never write a secret value",
            "`kronn/skills/*/SKILL.md`",
            // What needs Kronn, and what to do about it.
            "Workflows",
            "Quick APIs",
            "artifacts",
            "tell the human the resource needs Kronn",
            // Why and how to install.
            "manages credentials",
            "approve",
            KRONN_RELEASES_URL,
            KRONN_REPOSITORY_URL,
            // Safety.
            "may come from any contributor",
            "Read a Quick Exec before running it",
            "never run what you do not understand",
        ] {
            assert!(body.contains(expected), "router body lacks {expected:?}");
        }
        assert!(
            !text.contains("{RELEASES}") && !text.contains("{REPOSITORY}"),
            "an install placeholder was left unfilled"
        );
        assert!(
            text.len() <= 3_000,
            "the router is loaded on demand but must stay compact: {} bytes",
            text.len()
        );
    }

    #[test]
    fn the_router_skill_install_links_are_the_ones_in_the_readme() {
        let readme =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../README.md"))
                .unwrap();
        for url in [KRONN_RELEASES_URL, KRONN_REPOSITORY_URL] {
            assert!(readme.contains(url), "{url} is not in the README");
        }
    }

    #[test]
    fn the_router_skill_carries_its_model_version_and_is_regenerated_on_each_publication() {
        let root = tempfile::TempDir::new().unwrap();
        let rendered = render_quick_exec(&sample_exec("literal-token"), "deploy").unwrap();
        publish(root.path(), "repo", rendered, false).unwrap();
        let text = published_router(root.path());
        assert!(
            text.contains(&format!(
                "metadata:\n  version: \"{ROUTER_SKILL_VERSION}\"\n"
            )),
            "the model version is missing from the file"
        );
        assert_eq!(text.as_bytes(), router_skill().as_slice());

        // A copy written by an older model, still exactly what Kronn wrote
        // (the lock holds its hash), is replaced by the current model.
        let previous_model = b"---\nname: kronn\ndescription: Old.\n---\n\nRead the index.\n";
        std::fs::write(root.path().join(ROUTER_PATH), previous_model).unwrap();
        let mut lock = load_lock(root.path()).unwrap().unwrap();
        lock.files
            .insert(ROUTER_PATH.to_string(), sha256(previous_model));
        write_atomic(
            root.path(),
            LOCK_PATH,
            &serde_json::to_vec_pretty(&lock).unwrap(),
        )
        .unwrap();

        let rendered = render_quick_exec(&sample_exec("literal-token"), "deploy").unwrap();
        publish(root.path(), "repo", rendered, false).unwrap();
        assert_eq!(published_router(root.path()).as_bytes(), router_skill());
        let lock = load_lock(root.path()).unwrap().unwrap();
        assert_eq!(lock.files[ROUTER_PATH], sha256(&router_skill()));
    }

    #[test]
    fn a_human_edit_of_the_router_skill_is_never_overwritten_silently() {
        let root = tempfile::TempDir::new().unwrap();
        let rendered = render_quick_exec(&sample_exec("literal-token"), "deploy").unwrap();
        publish(root.path(), "repo", rendered, false).unwrap();
        let edited = format!(
            "{}\nOur team rule: ask before deploying.\n",
            published_router(root.path())
        );
        std::fs::write(root.path().join(ROUTER_PATH), &edited).unwrap();

        let rendered = render_quick_exec(&sample_exec("literal-token"), "deploy").unwrap();
        let error = publish(root.path(), "repo", rendered, false).unwrap_err();
        assert!(
            error.contains("changed since the last alignment"),
            "{error}"
        );
        assert!(error.contains(ROUTER_PATH), "{error}");
        assert_eq!(published_router(root.path()), edited);

        // Only an explicit overwrite replaces the human edit.
        let rendered = render_quick_exec(&sample_exec("literal-token"), "deploy").unwrap();
        publish(root.path(), "repo", rendered, true).unwrap();
        assert_eq!(published_router(root.path()).as_bytes(), router_skill());
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
        assert!(
            root.path().join("kronn").is_file(),
            "the launcher must stay untouched"
        );
    }

    #[test]
    fn can_write_repository_reports_the_kronn_file_conflict() {
        let root = tempfile::tempdir().unwrap();
        let (writable, reason) = can_write_repository(root.path());
        assert!(writable);
        assert!(reason.is_none());

        std::fs::write(root.path().join("kronn"), "#!/bin/sh\n").unwrap();
        let (writable, reason) = can_write_repository(root.path());
        assert!(!writable);
        assert_eq!(
            reason,
            Some(crate::models::RepositoryWriteBlocker::KronnPathIsFile)
        );
    }

    #[test]
    fn publish_side_effect_paths_drops_the_agents_line_once_present() {
        let root = tempfile::tempdir().unwrap();
        let paths = publish_side_effect_paths(root.path());
        assert!(
            paths.contains(&AGENTS_PATH.to_string()),
            "missing docs/AGENTS.md must be previewed"
        );
        assert!(paths.contains(&INDEX_PATH.to_string()));
        assert!(paths.contains(&ROUTER_PATH.to_string()));

        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        std::fs::write(root.path().join(AGENTS_PATH), format!("{AGENTS_LINE}\n")).unwrap();
        let paths = publish_side_effect_paths(root.path());
        assert!(
            !paths.contains(&AGENTS_PATH.to_string()),
            "an already-present line must not be previewed as a write"
        );
    }

    #[test]
    fn uncommitted_managed_paths_lists_only_dirty_lock_files() {
        let root = tempfile::tempdir().unwrap();
        crate::core::cmd::sync_cmd("git")
            .arg("-C")
            .arg(root.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        let rendered = render_quick_exec(&sample_exec("literal-token"), "deploy").unwrap();
        publish(root.path(), "repo", rendered, false).unwrap();
        let lock = load_lock(root.path()).unwrap().unwrap();
        // Freshly written, nothing committed yet: every managed path is dirty
        // (git reports untracked files the same as modified ones here).
        let dirty = uncommitted_managed_paths(root.path(), &lock);
        assert!(dirty.contains(&"kronn/quick-execs/deploy.yaml".to_string()));

        crate::core::cmd::sync_cmd("git")
            .arg("-C")
            .arg(root.path())
            .args(["add", "-A"])
            .status()
            .unwrap();
        crate::core::cmd::sync_cmd("git")
            .arg("-C")
            .arg(root.path())
            .args([
                "-c",
                "user.email=t@t.io",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "-m",
                "init",
            ])
            .status()
            .unwrap();
        let dirty = uncommitted_managed_paths(root.path(), &lock);
        assert!(
            dirty.is_empty(),
            "a committed managed path must not be reported: {dirty:?}"
        );
    }

    #[test]
    fn resource_adr_level_matches_the_adr_005_table() {
        use crate::models::ResourceAdrLevel;
        let exec = render_quick_exec(&sample_exec("token"), "deploy").unwrap();
        assert_eq!(resource_adr_level(&exec.document), ResourceAdrLevel::N1);

        let api = render_quick_api(&sample_api("/deploy"), "deploy").unwrap();
        assert_eq!(resource_adr_level(&api.document), ResourceAdrLevel::N2);

        let prompt = render_quick_prompt(&sample_prompt("body"), "deploy-prompt").unwrap();
        assert_eq!(resource_adr_level(&prompt.document), ResourceAdrLevel::N1);
    }

    #[test]
    fn unified_diff_reports_no_difference_when_the_files_agree() {
        assert_eq!(unified_diff(b"same\ncontent\n", b"same\ncontent\n"), "");
    }

    #[test]
    fn unified_diff_produces_a_real_hunk_around_the_changed_line() {
        let repository = b"one\ntwo\nthree\nfour\nfive\n".to_vec();
        let kronn = b"one\ntwo\nCHANGED\nfour\nfive\n".to_vec();
        let diff = unified_diff(&repository, &kronn);
        assert!(diff.starts_with("--- repository\n+++ Kronn\n@@ "), "{diff}");
        assert!(diff.contains("-three"), "{diff}");
        assert!(diff.contains("+CHANGED"), "{diff}");
        assert!(
            diff.contains(" two"),
            "unchanged context lines must stay: {diff}"
        );
    }

    #[test]
    fn field_diff_flattens_nested_paths_and_reports_only_differences() {
        let repository = serde_json::json!({
            "steps": [{"agent": "claude", "command": "cargo test"}],
            "trigger": {"cron": "0 * * * *"},
        });
        let kronn = serde_json::json!({
            "steps": [{"agent": "codex", "command": "cargo test"}],
            "trigger": {"cron": "0 * * * *"},
        });
        let diffs = field_diff(&repository, &kronn);
        assert_eq!(diffs.len(), 1, "{diffs:?}");
        assert_eq!(diffs[0].field, "steps.0.agent");
        assert_eq!(diffs[0].repository, Some(serde_json::json!("claude")));
        assert_eq!(diffs[0].kronn, Some(serde_json::json!("codex")));
    }

    #[test]
    fn required_secret_statuses_mark_each_name_configured_or_missing() {
        let names = vec!["FASTLY_TOKEN".to_string(), "SLACK_URL".to_string()];
        let configured = BTreeSet::from(["FASTLY_TOKEN".to_string()]);
        let statuses = required_secret_statuses(&names, &configured);
        assert_eq!(statuses.len(), 2);
        assert!(statuses[0].configured);
        assert_eq!(statuses[1].name, "SLACK_URL");
        assert!(!statuses[1].configured);
    }

    #[test]
    fn field_diff_ignores_instance_local_fields() {
        let repository = serde_json::json!({
            "id": "source-id", "project_id": "p-source", "updated_at": "2026-01-01",
            "command": "cargo",
        });
        let kronn = serde_json::json!({
            "id": "local-id", "project_id": "p-local", "updated_at": "2026-09-01",
            "command": "cargo",
        });
        assert!(field_diff(&repository, &kronn).is_empty());
    }

    #[test]
    fn unified_diff_stays_real_on_a_large_file_with_a_local_edit() {
        let lines: Vec<String> = (0..5_000).map(|n| format!("<p>line {n}</p>")).collect();
        let repository = lines.join("\n");
        let mut edited = lines.clone();
        edited[2_500] = "<p>EDITED</p>".to_string();
        let kronn = edited.join("\n");
        let diff = unified_diff(repository.as_bytes(), kronn.as_bytes());
        assert!(diff.contains("-<p>line 2500</p>"), "{diff}");
        assert!(diff.contains("+<p>EDITED</p>"), "{diff}");
        assert!(diff.contains("@@ -2498,7 +2498,7 @@"), "{diff}");
        assert!(
            diff.lines().count() < 20,
            "the hunk must stay local: {}",
            diff.lines().count()
        );
    }

    #[test]
    fn ascii_slug_folds_accents_and_keeps_only_plain_ascii() {
        assert_eq!(
            ascii_slug("Plan de correction — jeu réduit"),
            "plan-de-correction-jeu-reduit"
        );
        assert_eq!(
            ascii_slug("Œuvre à l'été, ÇA & Naïve"),
            "oeuvre-a-l-ete-ca-naive"
        );
        assert_eq!(ascii_slug("Straße"), "strasse");
        assert_eq!(ascii_slug("re\u{301}duit"), "reduit");
        assert_eq!(
            ascii_slug("PR #1897 — v3.3 pack-context"),
            "pr-1897-v3-3-pack-context"
        );
        assert_eq!(ascii_slug("日本語"), "");
        assert_eq!(ascii_slug("  --  "), "");
    }

    #[test]
    fn fingerprint_is_the_first_eight_characters_of_the_hash() {
        assert_eq!(fingerprint(&sha256(b"abc")), "ba7816bf");
        assert_eq!(fingerprint("ab"), "ab");
    }

    fn commit_as(root: &Path, author: &str, date: &str, message: &str) {
        let git = |args: &[&str]| {
            crate::core::cmd::sync_cmd("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .env("GIT_AUTHOR_NAME", author)
                .env("GIT_AUTHOR_EMAIL", "a@example.com")
                .env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_NAME", author)
                .env("GIT_COMMITTER_EMAIL", "a@example.com")
                .env("GIT_COMMITTER_DATE", date)
                .status()
                .unwrap()
        };
        assert!(git(&["add", "-A"]).success());
        assert!(git(&["commit", "-q", "-m", message]).success());
    }

    #[test]
    fn file_dates_come_from_each_files_last_commit_in_one_pass() {
        let root = tempfile::tempdir().unwrap();
        crate::core::cmd::sync_cmd("git")
            .arg("-C")
            .arg(root.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        std::fs::create_dir_all(root.path().join("kronn/workflows")).unwrap();
        std::fs::create_dir_all(root.path().join(".claude/skills/lint")).unwrap();
        std::fs::write(root.path().join("kronn/workflows/a.yaml"), "1").unwrap();
        commit_as(root.path(), "Alice", "2026-01-01T10:00:00+00:00", "a");
        std::fs::write(root.path().join(".claude/skills/lint/SKILL.md"), "1").unwrap();
        commit_as(root.path(), "Bob", "2026-02-01T10:00:00+00:00", "b");
        std::fs::write(root.path().join("kronn/workflows/a.yaml"), "2").unwrap();
        commit_as(root.path(), "Carol", "2026-03-01T10:00:00+00:00", "c");
        std::fs::write(root.path().join("kronn/workflows/untracked.yaml"), "x").unwrap();

        let dates = RepositoryFileDates::read(root.path(), &["kronn", ".claude/skills"]);

        let (date, author) = dates.updated_at("kronn/workflows/a.yaml");
        assert_eq!(author.as_deref(), Some("Carol"), "the last commit wins");
        assert_eq!(date.unwrap().to_rfc3339(), "2026-03-01T10:00:00+00:00");
        let (date, author) = dates.updated_at(".claude/skills/lint/SKILL.md");
        assert_eq!(author.as_deref(), Some("Bob"));
        assert_eq!(date.unwrap().to_rfc3339(), "2026-02-01T10:00:00+00:00");

        let (date, author) = dates.updated_at("kronn/workflows/untracked.yaml");
        assert!(date.is_some(), "an untracked file falls back to its mtime");
        assert!(author.is_none());
        assert_eq!(
            dates.updated_at("kronn/workflows/absent.yaml"),
            (None, None),
            "a path that is not there has no date and costs no lookup"
        );
    }

    #[test]
    fn file_dates_stop_waiting_for_git_when_the_budget_is_spent() {
        let root = tempfile::tempdir().unwrap();
        crate::core::cmd::sync_cmd("git")
            .arg("-C")
            .arg(root.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        std::fs::create_dir_all(root.path().join("kronn")).unwrap();
        std::fs::write(root.path().join("kronn/a.yaml"), "1").unwrap();
        commit_as(root.path(), "Alice", "2026-01-01T10:00:00+00:00", "a");

        let unbounded = RepositoryFileDates::read(root.path(), &["kronn"]);
        assert_eq!(
            unbounded.updated_at("kronn/a.yaml").1.as_deref(),
            Some("Alice")
        );

        let spent =
            RepositoryFileDates::read_within(root.path(), &["kronn"], std::time::Duration::ZERO);
        let (date, author) = spent.updated_at("kronn/a.yaml");
        assert!(date.is_some(), "the file still gets its mtime");
        assert!(author.is_none(), "git was not waited for");
    }

    #[test]
    fn file_dates_outside_a_git_repository_fall_back_to_the_mtime() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("kronn")).unwrap();
        std::fs::write(root.path().join("kronn/present.yaml"), "x").unwrap();
        let dates = RepositoryFileDates::read(root.path(), &["kronn"]);
        let (date, author) = dates.updated_at("kronn/present.yaml");
        assert!(date.is_some());
        assert!(author.is_none());
        assert_eq!(dates.updated_at("kronn/absent.yaml"), (None, None));
    }
}
