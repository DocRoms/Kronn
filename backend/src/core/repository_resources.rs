//! Versioned project resources stored under `kronn/`.
//!
//! Repository files are portable definitions. Local alignment baselines and
//! approvals stay in SQLite so a clone never imports trust from Git.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use std::sync::{Arc, LazyLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::core::content_memo::{self, ContentMemo, MemoStats};
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
/// Where every agent looks for skills, and so where Kronn writes them: `kronn/`
/// only holds what has no native home (prompts, automations, artifacts).
pub const SKILLS_ROOT: &str = ".agents/skills";
/// Where Kronn wrote skills before they moved to [`SKILLS_ROOT`]; still read,
/// never written.
pub const LEGACY_SKILLS_ROOT: &str = "kronn/skills";
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

/// A resource as Kronn would write it. The document and the files are shared
/// with the memo that holds this rendering, so cloning one copies a few short
/// strings and never the JSON tree or the file bytes: a listing serves every
/// resource from memory on each read and would otherwise copy all of it again.
#[derive(Debug, Clone)]
pub struct RenderedRepositoryResource {
    pub document: Arc<RepositoryDocument>,
    pub name: String,
    pub files: Arc<BTreeMap<String, Vec<u8>>>,
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

/// The resource of a document as the approval fingerprint reads it: the same
/// object without its instance-local fields, borrowed rather than copied.
struct DefinitionView<'a>(&'a Value);

impl Serialize for DefinitionView<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let Some(object) = self.0.as_object() else {
            return self.0.serialize(serializer);
        };
        let kept = object
            .iter()
            .filter(|(key, _)| !INSTANCE_LOCAL_FIELDS.contains(&key.as_str()));
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in kept {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

/// Fingerprint the executable definition while excluding instance-local
/// identity, timestamps, favorites and the workflow activation toggle.
///
/// A human's approval is recorded against this value, so it is spelled exactly
/// as it always was: an object whose keys come in sorted order (`kind`,
/// `requires`, `resource`, `schema_version`, `slug`), the resource's own keys
/// sorted too, compact.
pub fn approval_hash(document: &RepositoryDocument) -> String {
    #[derive(Serialize)]
    struct Definition<'a> {
        kind: ProjectRepositoryResourceKind,
        requires: &'a [String],
        resource: DefinitionView<'a>,
        schema_version: u32,
        slug: &'a str,
    }
    let definition = Definition {
        kind: document.kind,
        requires: &document.requires,
        resource: DefinitionView(&document.resource),
        schema_version: document.schema_version,
        slug: &document.slug,
    };
    sha256(&serde_json::to_vec(&definition).unwrap_or_default())
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
            if text == REDACTED {
                let name = secret_name(kind, slug, path);
                *text = format!("secret://{name}");
                secrets.insert(name);
                return;
            }
            let (redacted, count) = crate::core::redact::redact_for_audit_artifact(text);
            if count > 0 {
                let name = secret_name(kind, slug, path);
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
        document: Arc::new(document),
        name,
        files: Arc::new(files),
        hash,
        required_secrets,
    }
}

/// Upper bound on what the render memo keeps: a project's automations are a
/// few KB each, so this holds every project of a large install at once.
const RENDER_MEMO_BUDGET: usize = 48 * 1024 * 1024;

/// Rendered resources by the fingerprint of the content they were rendered
/// from. Rendering masks every string of the resource, and a listing renders
/// each automation of a project on every read: the result depends on that
/// content alone, so it is computed once and served from here afterwards.
static RENDER_MEMO: LazyLock<ContentMemo<RenderedRepositoryResource>> =
    LazyLock::new(|| ContentMemo::new(RENDER_MEMO_BUDGET));

/// How the render memo has answered so far: `misses` counts the resources
/// whose secrets were actually masked, `hits` the ones served from memory.
pub fn render_memo_stats() -> MemoStats {
    RENDER_MEMO.stats()
}

/// Whether the render memo can take more without pushing out what it already
/// holds. The background warm-up stops here rather than evict the renderings
/// it has just computed to make room for the next ones.
pub fn render_memo_has_room() -> bool {
    RENDER_MEMO.used() < RENDER_MEMO.budget() / 8 * 7
}

/// Whether rendering `source` (the resource as JSON) under `slug` would be
/// served from the render memo right now.
#[cfg(test)]
pub(crate) fn render_is_memoized(
    kind: ProjectRepositoryResourceKind,
    slug: &str,
    updated_at: DateTime<Utc>,
    source: &Value,
) -> bool {
    render_key(kind, slug, updated_at, source).is_ok_and(|key| RENDER_MEMO.contains(&key))
}

/// Upper bound on what the masked-text memo keeps: opening a sheet masks each
/// file of the resource on both sides, and a sheet reopened later asks for the
/// same texts.
const MASKED_TEXT_MEMO_BUDGET: usize = 32 * 1024 * 1024;

static MASKED_TEXT_MEMO: LazyLock<ContentMemo<String>> =
    LazyLock::new(|| ContentMemo::new(MASKED_TEXT_MEMO_BUDGET));

/// How the masked-text memo has answered so far, like [`render_memo_stats`].
pub fn masked_text_memo_stats() -> MemoStats {
    MASKED_TEXT_MEMO.stats()
}

/// What the masking writes in place of a secret value.
const MASK: &str = "***REDACTED***";

/// A `secret://NAME` reference in `text`, as the rendering writes it in place of
/// a secret value: `NAME` is what the secret is called, not its value. A name
/// that would itself be masked is someone's secret typed in the place of one,
/// not a reference.
fn references_shielded(text: &str) -> Option<String> {
    const PREFIX: &str = "secret://";
    let mut shielded = String::with_capacity(text.len());
    let mut rest = text;
    let mut found = false;
    while let Some(start) = rest.find(PREFIX) {
        let after = &rest[start + PREFIX.len()..];
        let name_len = after
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            .count();
        let name = &after[..name_len];
        let is_reference =
            !name.is_empty() && crate::core::redact::redact_for_audit_artifact(name).1 == 0;
        shielded.push_str(&rest[..start]);
        if is_reference {
            // Already in the shape the masking writes, so the patterns leave
            // it as it is: what is left to mask is what is not a reference.
            shielded.push_str(MASK);
            found = true;
            rest = &after[name_len..];
        } else {
            shielded.push_str(PREFIX);
            rest = after;
        }
    }
    shielded.push_str(rest);
    found.then_some(shielded)
}

/// `text` with every secret value masked. The references to a secret that
/// Kronn's rendering writes stay readable on a line where they are all there is
/// to hide: the masking patterns would take `password=secret://NAME` for a
/// password, and the sheet would lose the name of the secret the resource needs.
/// A line that holds anything else to mask is masked whole, references included.
fn mask_secrets(text: &str) -> String {
    use crate::core::redact::redact_for_audit_artifact as redact;
    if !text.contains("secret://") {
        return redact(text).0;
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let shielded: Vec<Option<String>> =
        lines.iter().map(|line| references_shielded(line)).collect();
    if shielded.iter().all(Option::is_none) {
        return redact(text).0;
    }
    // One pass over the whole text, with the references of every line standing
    // in as the mask: patterns that read across lines still see all of it.
    let residual = lines
        .iter()
        .zip(&shielded)
        .map(|(line, shielded)| shielded.as_deref().unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    let masked = redact(&residual).0;
    let masked_lines: Vec<&str> = masked.split('\n').collect();
    if masked_lines.len() != lines.len() {
        return redact(text).0;
    }
    // A line the pass left exactly as shielded had nothing else to hide: it
    // gets its references back. Any other line stays as the pass wrote it.
    lines
        .iter()
        .zip(&shielded)
        .zip(&masked_lines)
        .map(|((line, shielded), masked)| match shielded {
            Some(shielded) if shielded == masked => *line,
            _ => *masked,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `bytes` as text with every secret value masked: what a comparison may put in
/// an answer, whichever side the file comes from. A repository file can hold a
/// secret someone typed into it by hand, and Kronn's own rendering is masked
/// already, so the same pass on both sides keeps one from showing what the other
/// hides, and a secret present on both never reads as a difference. Memoized by
/// the content, so a file is masked once however often it is asked for.
pub fn masked_text(bytes: &[u8]) -> Arc<String> {
    let key = content_memo::fingerprint(&[b"masked-text", bytes]);
    MASKED_TEXT_MEMO
        .get_or_try_insert(
            &key,
            |text: &String| text.len(),
            || Ok::<_, std::convert::Infallible>(mask_secrets(&String::from_utf8_lossy(bytes))),
        )
        .unwrap_or_else(|never| match never {})
}

/// Masks every string of a JSON value the way [`masked_text`] masks a file.
pub fn mask_value_strings(value: &mut Value) {
    match value {
        Value::String(text) => {
            let masked = masked_text(text.as_bytes());
            if masked.as_str() != text.as_str() {
                *text = masked.as_str().to_string();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_value_strings),
        Value::Object(map) => map.values_mut().for_each(mask_value_strings),
        _ => {}
    }
}

fn rendered_weight(rendered: &RenderedRepositoryResource) -> usize {
    // The document repeats the content held by the files: count both.
    2 * rendered.files.values().map(Vec::len).sum::<usize>() + rendered.hash.len()
}

/// The memo key of a rendering: the *unmasked* content (`source`) with what
/// else shapes the output (kind, slug, timestamp), so a changed definition, or
/// the same one under another slug, is masked afresh while an unchanged one is
/// not. `source` is the resource as a JSON value, whose object keys are always
/// sorted: the maps a typed resource holds (`HashMap`s) would not serialize in
/// a stable order, and a key that changed with it would never be found again.
fn render_key(
    kind: ProjectRepositoryResourceKind,
    slug: &str,
    updated_at: DateTime<Utc>,
    source: &Value,
) -> Result<String, String> {
    let source = serde_json::to_vec(source).map_err(|error| error.to_string())?;
    Ok(content_memo::fingerprint(&[
        format!("{kind:?}").as_bytes(),
        slug.as_bytes(),
        updated_at.to_rfc3339().as_bytes(),
        &source,
    ]))
}

/// `render` behind the memo. `source` is handed to `render` on a miss, so the
/// one JSON tree serves both as the key and as the document's content; on a hit
/// nothing is copied but the shared handles of the rendering.
fn memoized_render(
    kind: ProjectRepositoryResourceKind,
    slug: &str,
    updated_at: DateTime<Utc>,
    source: Value,
    render: impl FnOnce(Value) -> Result<RenderedRepositoryResource, String>,
) -> Result<RenderedRepositoryResource, String> {
    let key = render_key(kind, slug, updated_at, &source)?;
    RENDER_MEMO
        .get_or_try_insert(&key, rendered_weight, || render(source))
        .map(|rendered| RenderedRepositoryResource::clone(&rendered))
}

pub fn render_workflow(
    workflow: &Workflow,
    slug: &str,
) -> Result<RenderedRepositoryResource, String> {
    // Repository workflows are definitions, not activation state. Imports are
    // always disabled locally; omitting that local toggle from the effective
    // fingerprint lets a human approve the definition, then enable it without
    // changing the content they reviewed.
    let mut source = serde_json::to_value(workflow).map_err(|error| error.to_string())?;
    if let Some(object) = source.as_object_mut() {
        object.insert("enabled".to_string(), Value::Bool(false));
    }
    memoized_render(
        ProjectRepositoryResourceKind::Workflow,
        slug,
        workflow.updated_at,
        source,
        |_| {
            let mut exported = workflow.clone();
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
        },
    )
}

pub fn render_quick_prompt(
    prompt: &QuickPrompt,
    slug: &str,
) -> Result<RenderedRepositoryResource, String> {
    let source = serde_json::to_value(prompt).map_err(|error| error.to_string())?;
    memoized_render(
        ProjectRepositoryResourceKind::QuickPrompt,
        slug,
        prompt.updated_at,
        source,
        |source| {
            let (document, secrets) = document(
                ProjectRepositoryResourceKind::QuickPrompt,
                slug,
                prompt.updated_at,
                source,
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
        },
    )
}

pub fn render_quick_api(api: &QuickApi, slug: &str) -> Result<RenderedRepositoryResource, String> {
    let source = serde_json::to_value(api).map_err(|error| error.to_string())?;
    memoized_render(
        ProjectRepositoryResourceKind::QuickApi,
        slug,
        api.updated_at,
        source,
        |_| {
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
        },
    )
}

pub fn render_quick_exec(
    exec: &QuickExec,
    slug: &str,
) -> Result<RenderedRepositoryResource, String> {
    let source = serde_json::to_value(exec).map_err(|error| error.to_string())?;
    memoized_render(
        ProjectRepositoryResourceKind::QuickExec,
        slug,
        exec.updated_at,
        source,
        |_| {
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
        },
    )
}

pub fn render_artifact(
    artifact: &ArtifactBundlePage,
    updated_at: DateTime<Utc>,
    slug: &str,
) -> Result<RenderedRepositoryResource, String> {
    let source = serde_json::to_value(artifact).map_err(|error| error.to_string())?;
    memoized_render(
        ProjectRepositoryResourceKind::Artifact,
        slug,
        updated_at,
        source,
        |source| {
            let (document, secrets) = document(
                ProjectRepositoryResourceKind::Artifact,
                slug,
                updated_at,
                source,
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
        },
    )
}

pub fn render_skill(
    skill: &Skill,
    updated_at: DateTime<Utc>,
    slug: &str,
) -> Result<RenderedRepositoryResource, String> {
    let source = serde_json::to_value(skill).map_err(|error| error.to_string())?;
    memoized_render(
        ProjectRepositoryResourceKind::Skill,
        slug,
        updated_at,
        source,
        |source| {
            let (document, secrets) = document(
                ProjectRepositoryResourceKind::Skill,
                slug,
                updated_at,
                source,
                Vec::new(),
            );
            let files = BTreeMap::from([(skill_path(slug), standard_skill_file(&document, slug)?)]);
            Ok(rendered(document, skill.name.clone(), files, secrets))
        },
    )
}

/// The repository path of a skill's `SKILL.md`.
pub fn skill_path(slug: &str) -> String {
    format!("{SKILLS_ROOT}/{slug}/SKILL.md")
}

/// Whether `path` is a skill file at the location Kronn used before skills
/// moved to [`SKILLS_ROOT`].
pub fn is_legacy_skill_path(path: &str) -> bool {
    path.strip_prefix(LEGACY_SKILLS_ROOT)
        .is_some_and(|rest| rest.starts_with('/'))
}

fn resource_text<'a>(resource: &'a Value, key: &str) -> &'a str {
    resource
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// A skill as a real Agent Skills file: `name` is the folder slug, the header
/// stays valid whatever the skill holds (an empty description falls back to the
/// display name, an oversized one is cut), and what is Kronn's own — display
/// name, category, icon, attribution — goes under `metadata`.
fn standard_skill_file(document: &RepositoryDocument, slug: &str) -> Result<Vec<u8>, String> {
    use crate::core::skills::{
        category_str, KRONN_CATEGORY_KEY, KRONN_EXTERNAL_KEY, KRONN_ICON_KEY, KRONN_NAME_KEY,
        KRONN_SOURCE_URL_KEY,
    };
    let resource = &document.resource;
    let display_name = resource_text(resource, "name").trim();
    let mut description = resource_text(resource, "description").trim();
    if description.is_empty() {
        description = if display_name.is_empty() {
            slug
        } else {
            display_name
        };
    }
    let mut metadata = BTreeMap::new();
    let mut keep = |key: &str, value: &str| {
        if !value.is_empty() {
            metadata.insert(key.to_string(), value.to_string());
        }
    };
    keep(KRONN_NAME_KEY, display_name);
    keep(KRONN_ICON_KEY, resource_text(resource, "icon"));
    let category = match resource_text(resource, "category") {
        "Language" => Some(crate::models::SkillCategory::Language),
        "Business" => Some(crate::models::SkillCategory::Business),
        "Domain" => Some(crate::models::SkillCategory::Domain),
        _ => None,
    };
    keep(
        KRONN_CATEGORY_KEY,
        category.as_ref().map_or("", category_str),
    );
    if resource.get("external").and_then(Value::as_bool) == Some(true) {
        keep(KRONN_EXTERNAL_KEY, "true");
    }
    keep(KRONN_SOURCE_URL_KEY, resource_text(resource, "source_url"));
    let file = crate::core::agent_skill::AgentSkillFile {
        name: slug.to_string(),
        description: description
            .chars()
            .take(crate::core::agent_skill::DESCRIPTION_MAX)
            .collect(),
        license: Some(resource_text(resource, "license").to_string()),
        compatibility: None,
        allowed_tools: Some(resource_text(resource, "allowed_tools").to_string()),
        metadata,
        body: resource_text(resource, "content").to_string(),
    };
    crate::core::agent_skill::validate(&file, slug)?;
    Ok(crate::core::agent_skill::render(&file).into_bytes())
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

/// A skill file in the Agent Skills format as a resource document. It carries
/// no date of its own, so the file's modification time stands in for one.
fn standard_skill_document(
    root: &Path,
    relative: &str,
    bytes: &[u8],
    slug: &str,
) -> Result<RepositoryDocument, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| format!("resource {slug} is not UTF-8"))?;
    let skill = crate::core::skills::parse_skill_markdown(slug, text, false)
        .ok_or_else(|| format!("resource {slug} has no valid Skill frontmatter"))?;
    let updated_at = std::fs::metadata(root.join(relative))
        .and_then(|metadata| metadata.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now());
    Ok(RepositoryDocument {
        schema_version: SCHEMA_VERSION,
        kind: ProjectRepositoryResourceKind::Skill,
        slug: slug.to_string(),
        updated_at,
        requires: Vec::new(),
        resource: serde_json::to_value(skill).map_err(|error| error.to_string())?,
        redacted_fields: Vec::new(),
    })
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
    let legacy_skill = entry.kind == ProjectRepositoryResourceKind::Skill
        && std::str::from_utf8(first).is_ok_and(|text| text.contains(DOCUMENT_START));
    if entry.kind == ProjectRepositoryResourceKind::Skill && !legacy_skill {
        let path = entry.paths.first().map(String::as_str).unwrap_or_default();
        let document = standard_skill_document(root, path, first, &entry.slug)?;
        return Ok((document, resource_hash(&files)));
    }
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
    if legacy_skill {
        // Compared as what a move to `.agents/skills` would write, so a skill
        // still under `kronn/skills` reads as in sync with its Kronn copy when
        // only the file format differs.
        let standard = standard_skill_file(&document, &entry.slug)?;
        let files = BTreeMap::from([(skill_path(&entry.slug), standard)]);
        return Ok((document, resource_hash(&files)));
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

/// A skill Kronn wrote in its former format (`kronn/skills/<slug>/SKILL.md`,
/// with the definition in a metadata comment) as the standard Agent Skills file
/// a move to `.agents/skills` writes.
pub fn legacy_skill_as_standard(
    root: &Path,
    slug: &str,
    legacy_path: &str,
) -> Result<Vec<u8>, String> {
    let entry = RepositoryLockResource {
        kind: ProjectRepositoryResourceKind::Skill,
        slug: slug.to_string(),
        name: String::new(),
        level: String::new(),
        paths: vec![legacy_path.to_string()],
        sha256: String::new(),
        required_secrets: Vec::new(),
    };
    let (document, _) = read_resource(root, &entry)?;
    standard_skill_file(&document, slug)
}

/// Points the lock at the new place of skills that moved from `kronn/skills/`
/// to `.agents/skills/`: each entry's path, the ownership record of the file
/// and its content hash follow the skill, while its slug — the identity the
/// alignment and the approvals are keyed by — does not change. The router
/// skill is refreshed when Kronn wrote it and nobody edited it since. Returns
/// the entries as they now stand.
pub fn relocate_skill_entries(
    root: &Path,
    slugs: &[String],
) -> Result<Vec<RepositoryLockResource>, String> {
    let Some(mut lock) = load_lock(root)? else {
        return Ok(Vec::new());
    };
    let mut relocated = Vec::new();
    for slug in slugs {
        let new_path = skill_path(slug);
        let Some(entry) = lock.resources.iter_mut().find(|entry| {
            entry.kind == ProjectRepositoryResourceKind::Skill && entry.slug == *slug
        }) else {
            continue;
        };
        let former: Vec<String> = entry
            .paths
            .iter()
            .filter(|path| is_legacy_skill_path(path))
            .cloned()
            .collect();
        if former.is_empty() {
            continue;
        }
        validate_relative(&new_path)?;
        let path = root.join(&new_path);
        crate::core::fs_guard::assert_contained_no_symlink(root, &path)?;
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        entry.paths = vec![new_path.clone()];
        entry.sha256 = resource_hash(&BTreeMap::from([(new_path.clone(), bytes.clone())]));
        relocated.push(entry.clone());
        for path in former {
            lock.files.remove(&path);
        }
        lock.files.insert(new_path, sha256(&bytes));
    }
    if relocated.is_empty() {
        return Ok(relocated);
    }
    lock.updated_at = Utc::now();
    let current_router = std::fs::read(root.join(ROUTER_PATH)).ok();
    let router = router_skill();
    if let (Some(expected), Some(current)) = (lock.files.get(ROUTER_PATH), current_router) {
        if sha256(&current) == *expected
            && current != router
            && write_atomic(root, ROUTER_PATH, &router).is_ok()
        {
            lock.files.insert(ROUTER_PATH.to_string(), sha256(&router));
        }
    }
    let mut lock_bytes = serde_json::to_vec_pretty(&lock)
        .map_err(|error| format!("cannot serialize kronn.lock: {error}"))?;
    lock_bytes.push(b'\n');
    write_atomic(root, LOCK_PATH, &lock_bytes)?;
    Ok(relocated)
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

/// Whether a file Kronn wrote earlier may be deleted: it is gone already, or it
/// is a plain file the lock lists and that still holds what Kronn wrote (unless
/// the caller accepts losing edits). A file the lock does not list is not
/// Kronn's to delete and is left alone.
fn may_remove(
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
        return Err(format!("refusing to remove non-file {}", path.display()));
    }
    let Some(expected) = previous.files.get(relative) else {
        return Ok(());
    };
    let current =
        std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if !allow_changed_owned && sha256(&current) != *expected {
        return Err(format!("{relative} changed since the last alignment"));
    }
    Ok(())
}

/// Deletes what [`may_remove`] allowed, then the folders that emptied.
fn remove_owned_file(root: &Path, relative: &str, previous: &RepositoryLock) -> Result<(), String> {
    if !previous.files.contains_key(relative) {
        return Ok(());
    }
    let path = root.join(relative);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot remove {}: {error}", path.display())),
    }
    remove_empty_parents(root, &path);
    Ok(())
}

/// Removes `path`'s parent folders while they are empty, never `root` itself
/// nor the top-level folder under it (`kronn/`, `.agents/`).
pub(crate) fn remove_empty_parents(root: &Path, path: &Path) {
    let mut current = path.parent();
    while let Some(directory) = current {
        let depth = directory
            .strip_prefix(root)
            .map(|relative| relative.components().count())
            .unwrap_or(0);
        if depth < 2 || std::fs::remove_dir(directory).is_err() {
            return;
        }
        current = directory.parent();
    }
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
const ROUTER_SKILL_VERSION: u32 = 2;

/// Where a human reads about Kronn and installs it. Both are taken from the
/// project README (a test keeps them there), never invented here.
const KRONN_REPOSITORY_URL: &str = "https://github.com/DocRoms/Kronn";
const KRONN_RELEASES_URL: &str = "https://github.com/DocRoms/Kronn/releases/latest";

const ROUTER_SKILL_DESCRIPTION: &str = "Explains the shared AI resources in this repository's kronn/ folder. Use it when asked to run a Quick Prompt (QP), Quick Exec (QE), workflow or automation, when kronn/INDEX.md is mentioned, or when asked what Kronn is or how to install it.";

const ROUTER_SKILL_BODY: &str = "# Kronn resources

`kronn/` holds the AI resources this team shares through Git that have no native home: prompts, automations and artifacts. Skills are real Agent Skills in `.agents/skills/`, where every agent looks for them. Kronn is a desktop/web app that orchestrates coding agents and writes these files; most of them are plain files any agent can use without it.

Read `kronn/INDEX.md` first (one line per resource, with its level), then open only what the task needs. `kronn/kronn.lock` records what Kronn wrote: never edit it.

Levels: **N0** readable by anyone, **N1** runnable with the CLIs on this machine, **N2** needs Kronn.

## Without Kronn

- **Quick Prompt** (`kronn/prompts/*.md`): a prompt template. Ask the human for each `{{variable}}` it declares, fill them in, then carry out the prompt yourself.
- **Quick Exec** (`kronn/quick-execs/*.yaml`, JSON content): one deterministic command (`resource.command` + `resource.args`). Run it as written from the repository root, filling `{{variable}}` from the human. Each `secret://NAME` is a secret: pass it by name through the environment variable `NAME` (`\"$NAME\"` in the command). If it is unset, ask the human to set it. Never write a secret value in a file, a command you print or your answer.
- **Skills** (`.agents/skills/*/SKILL.md`): plain Agent Skills, read them like any SKILL.md.

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

/// Repository-relative paths Kronn manages that `git status` reports as
/// modified, deleted or untracked: the files `kronn.lock` lists, everything in
/// the skills folder Kronn writes and migrates into, and the skills a migration
/// took out of the other native skill folders (only their deletion is
/// reported: what else sits there is the repository's own). Best-effort: any
/// git failure (no repo, no binary) reports no uncommitted paths rather than
/// failing the caller.
pub fn uncommitted_managed_paths(root: &Path, lock: &RepositoryLock) -> Vec<String> {
    if lock.files.is_empty() && !root.join(SKILLS_ROOT).is_dir() {
        return Vec::new();
    }
    let mut pathspecs: Vec<&str> = lock.files.keys().map(String::as_str).collect();
    pathspecs.push(SKILLS_ROOT);
    pathspecs.extend(crate::core::skill_migration::SOURCE_ROOTS.iter().copied());
    let output = crate::core::cmd::sync_cmd("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "-uall", "--"])
        .args(&pathspecs)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| Some((line.get(..2)?, line.get(3..)?)))
        .filter(|(status, path)| {
            let moved_out = crate::core::skill_migration::SOURCE_ROOTS
                .iter()
                .any(|source| {
                    path.strip_prefix(source)
                        .is_some_and(|rest| rest.starts_with('/'))
                });
            !moved_out || lock.files.contains_key(*path) || status.contains('D')
        })
        .map(|(_, path)| path.to_string())
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
    let mut writes = BTreeMap::clone(&resource.files);
    writes.insert(INDEX_PATH.into(), index);
    writes.insert(CONFIG_PATH.into(), config);
    writes.insert(ROUTER_PATH.into(), router);

    // Files this resource was published under before and no longer is (a skill
    // that moved from `kronn/skills/` to `.agents/skills/`): removed once the
    // new ones are in place, so publishing never leaves the resource twice.
    let stale: Vec<String> = previous
        .resources
        .iter()
        .filter(|item| item.kind == entry.kind && item.slug == entry.slug)
        .flat_map(|item| item.paths.iter().cloned())
        .filter(|path| !writes.contains_key(path))
        .collect();

    for relative in writes.keys() {
        may_replace(root, relative, &previous, allow_changed_owned)?;
    }
    may_replace(root, LOCK_PATH, &previous, allow_changed_owned)?;
    for relative in &stale {
        may_remove(root, relative, &previous, allow_changed_owned)?;
    }

    ensure_agents_line(root)?;
    for (relative, bytes) in &writes {
        write_atomic(root, relative, bytes)?;
    }
    next.files = previous.files.clone();
    for relative in &stale {
        remove_owned_file(root, relative, &previous)?;
        next.files.remove(relative);
    }
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

fn flatten_json<'a>(value: &'a Value, prefix: String, out: &mut BTreeMap<String, &'a Value>) {
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
            out.insert(prefix, value);
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
            // Compared in place: only the values that differ are copied out.
            let repository_value = repository_flat.get(&field).copied();
            let kronn_value = kronn_flat.get(&field).copied();
            (repository_value != kronn_value).then(|| crate::models::RepositoryResourceFieldDiff {
                repository: repository_value.cloned(),
                kronn: kronn_value.cloned(),
                field,
            })
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
    fn the_same_content_is_masked_once_and_changed_content_afresh() {
        let prompt = sample_prompt("memo probe one: password=hunter2");
        let mut edited = prompt.clone();
        edited.prompt_template = "memo probe one: a different sentence".into();
        let mut runs = 0;
        let mut render = |slug: &str, prompt: &QuickPrompt| {
            let source = serde_json::to_value(prompt).unwrap();
            memoized_render(
                ProjectRepositoryResourceKind::QuickPrompt,
                slug,
                prompt.updated_at,
                source,
                |_| {
                    runs += 1;
                    render_quick_prompt_unmemoized(prompt, slug)
                },
            )
            .unwrap()
        };

        let first = render("memo-probe-one", &prompt);
        let second = render("memo-probe-one", &prompt);
        assert_eq!(first.hash, second.hash);
        assert_eq!(first.files, second.files);
        assert_eq!(first.required_secrets, second.required_secrets);

        let third = render("memo-probe-one", &edited);
        assert_ne!(
            first.hash, third.hash,
            "changed content is not served stale"
        );

        render("memo-probe-other-slug", &prompt);
        assert_eq!(
            runs, 3,
            "masked for the first content, the edit and the other slug — never twice for the same"
        );
    }

    /// The prompt rendered without the memo, for the counting test above.
    fn render_quick_prompt_unmemoized(
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
        let body = document
            .resource
            .get("prompt_template")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let files = BTreeMap::from([(
            format!("kronn/prompts/{slug}.md"),
            markdown_file(&document, &prompt.name, &prompt.description, &body)?,
        )]);
        Ok(rendered(document, prompt.name.clone(), files, secrets))
    }

    #[test]
    fn a_rendering_served_from_memory_still_holds_no_secret_value() {
        let secret = "memo-leak-probe-7f3a";
        let prompt = sample_prompt(&format!(
            "Call the API with password={secret} and Authorization: Bearer abcdef0123456789abcdef"
        ));
        let before = render_memo_stats();
        let fresh = render_quick_prompt(&prompt, "memo-leak-probe").unwrap();
        let cached = render_quick_prompt(&prompt, "memo-leak-probe").unwrap();
        assert!(
            render_memo_stats().hits > before.hits,
            "the second rendering comes from the memo"
        );
        for rendered in [&fresh, &cached] {
            let everything = format!(
                "{}{}{:?}",
                rendered
                    .files
                    .values()
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .collect::<String>(),
                serde_json::to_string(&rendered.document).unwrap(),
                rendered.required_secrets,
            );
            assert!(!everything.contains(secret), "{everything}");
            assert!(
                !everything.contains("abcdef0123456789abcdef"),
                "{everything}"
            );
            assert!(everything.contains("secret://KRONN_QUICKPROMPT_MEMO_LEAK_PROBE_"));
        }
        assert_eq!(fresh.hash, cached.hash);
    }

    #[test]
    fn masked_text_keeps_the_references_of_a_rendering_and_hides_anything_else() {
        // Kronn's own rendering: every secret is a reference already, and the
        // text shows the names the resource needs.
        let rendered = render_quick_prompt(
            &sample_prompt("Use password=masked-text-probe-1 and the key sk-abcdefghijklmnopqrstuvwxyz0123456789."),
            "masked-text",
        )
        .unwrap();
        let file = rendered.files.values().next().unwrap();
        let shown = masked_text(file);
        assert_eq!(shown.as_bytes(), file.as_slice(), "nothing left to hide");
        assert!(shown.contains("password=secret://KRONN_QUICKPROMPT_MASKED_TEXT_"));
        assert!(shown.contains("\"prompt_template\": \"Use password=secret://"));

        // A secret typed into a file by hand is hidden, wherever it sits.
        for (text, secret) in [
            (
                "Use password=hunter2-typed-by-hand.",
                "hunter2-typed-by-hand",
            ),
            (r#"{"token": "tok-typed-by-hand"}"#, "tok-typed-by-hand"),
            (
                "key sk-abcdefghijklmnopqrstuvwxyz0123456789 here",
                "sk-abcdefghijklmnopqrstuvwxyz0123456789",
            ),
        ] {
            let shown = masked_text(text.as_bytes());
            assert!(!shown.contains(secret), "{secret} survived: {shown}");
            assert!(shown.contains("***REDACTED***"), "{shown}");
        }

        // Next to a reference it is hidden just the same. A line with anything
        // else to mask is masked whole, references included; the lines around
        // it keep theirs.
        let mixed = masked_text(b"password=secret://NEEDED_ONE and password=typed-beside-it");
        assert!(!mixed.contains("typed-beside-it"), "{mixed}");
        assert!(!mixed.contains("NEEDED_ONE"), "{mixed}");
        let lines = masked_text(
            b"password=secret://NEEDED_ONE\npassword=typed-on-the-next-line\n\"token\": \"secret://NEEDED_TWO\"\n",
        );
        assert!(!lines.contains("typed-on-the-next-line"), "{lines}");
        assert_eq!(
            lines.lines().collect::<Vec<_>>(),
            [
                "password=secret://NEEDED_ONE",
                "password=***REDACTED***",
                "\"token\": \"secret://NEEDED_TWO\"",
            ]
        );

        // A "reference" whose name is a secret is not one.
        let disguised = masked_text(b"token=secret://ghp_abcdefghijklmnopqrstuvwxyz1234567890");
        assert!(
            !disguised.contains("ghp_abcdefghijklmnopqrstuvwxyz1234567890"),
            "{disguised}"
        );

        // Masked once, then served from memory; invalid UTF-8 is read lossily.
        let before = masked_text_memo_stats();
        masked_text("password=memo-probe-once".as_bytes());
        masked_text("password=memo-probe-once".as_bytes());
        assert!(masked_text_memo_stats().hits > before.hits);
        assert!(!masked_text(&[b'a', 0xff, b'b']).is_empty());
    }

    #[test]
    fn masking_the_strings_of_a_value_reaches_every_level() {
        let mut value = serde_json::json!({
            "args": ["--flag", "password=nested-probe-secret"],
            "inner": {"note": "key sk-abcdefghijklmnopqrstuvwxyz0123456789", "n": 3},
            "ref": "secret://KEEP_ME",
        });
        mask_value_strings(&mut value);
        let shown = value.to_string();
        assert!(!shown.contains("nested-probe-secret"), "{shown}");
        assert!(
            !shown.contains("sk-abcdefghijklmnopqrstuvwxyz0123456789"),
            "{shown}"
        );
        assert!(
            shown.contains("secret://KEEP_ME") && shown.contains("\"n\":3"),
            "{shown}"
        );
    }

    fn skill_with(name: &str, description: &str, body: &str) -> Skill {
        let raw = format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n");
        crate::core::skills::parse_skill_markdown("review", &raw, false).unwrap()
    }

    #[test]
    fn a_rendered_skill_is_a_valid_agent_skill_whatever_it_holds() {
        let timestamp = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let long = "x".repeat(2_000);
        for description in ["", "One line.", long.as_str()] {
            let skill = skill_with("Review Diffs", description, "Read the diff.");
            let rendered = render_skill(&skill, timestamp, "review").unwrap();
            assert_eq!(
                rendered.files.keys().collect::<Vec<_>>(),
                vec![".agents/skills/review/SKILL.md"]
            );
            let text = String::from_utf8(rendered.files[".agents/skills/review/SKILL.md"].clone())
                .unwrap();
            let file = crate::core::agent_skill::parse(&text).unwrap();
            crate::core::agent_skill::validate(&file, "review")
                .unwrap_or_else(|error| panic!("{error}\n{text}"));
            assert_eq!(file.body, "Read the diff.");
            assert_eq!(file.metadata["kronn-name"], "Review Diffs");
        }
        assert!(
            render_skill(&skill_with("A", "b", "c"), timestamp, "Not A Slug").is_err(),
            "a folder name the specification rejects is never written"
        );
    }

    #[test]
    fn a_skill_read_back_from_the_repository_keeps_what_kronn_wrote() {
        let root = tempfile::TempDir::new().unwrap();
        let skill = skill_with("Review Diffs", "Review \"carefully\".", "Read the diff.");
        let rendered = render_skill(&skill, Utc::now(), "review").unwrap();
        let entry = publish(root.path(), "repo", rendered.clone(), false).unwrap();
        assert_eq!(entry.paths, vec![".agents/skills/review/SKILL.md"]);
        assert!(!root.path().join("kronn/skills").exists());
        let (document, hash) = read_resource(root.path(), &entry).unwrap();
        assert_eq!(hash, rendered.hash);
        assert_eq!(document.resource["name"], "Review Diffs");
        assert_eq!(document.resource["description"], "Review \"carefully\".");
        assert_eq!(document.resource["content"], "Read the diff.");
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
            "`.agents/skills/*/SKILL.md`",
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

    /// One resource of each kind, with secrets in the places each kind
    /// masks, as fixed inputs for the byte-for-byte regression below.
    fn one_of_each_kind() -> Vec<(&'static str, RenderedRepositoryResource)> {
        let timestamp = Utc.timestamp_opt(1_700_000_010, 0).unwrap();

        let workflow: Workflow = serde_json::from_value(serde_json::json!({
            "id": "wf-golden", "name": "Nightly", "project_id": "project-1",
            "trigger": {"type": "Manual"},
            "steps": [
                {
                    "name": "fetch", "step_type": {"type": "ApiCall"},
                    "api_headers": {
                        "Authorization": "Bearer sk-live-1234567890abcdefghij",
                        "Accept": "application/json"
                    },
                    "api_query": {"token": "golden-query-token-1"},
                    "api_body": {"password": "hunter2-golden"}
                },
                {
                    "name": "ask", "step_type": {"type": "Agent"},
                    "quick_prompt_id": "qp-1",
                    "prompt_template": "Summarize with password=hunter2-golden"
                }
            ],
            "actions": [], "safety": {}, "workspace_config": null, "concurrency_limit": null,
            "enabled": true,
            "artifacts": {
                "report": {"path": "out/report.md", "format": "markdown"},
                "summary": {"path": "out/summary.md"},
                "plan": {"path": "out/plan.md"}
            },
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:10Z"
        }))
        .unwrap();

        let mut api = sample_api("/deploy");
        api.api_headers = Some(
            [
                ("Authorization", "Bearer sk-live-1234567890abcdefghij"),
                ("Accept", "application/json"),
                ("X-Trace", "trace-me"),
            ]
            .into_iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
        );
        api.api_query = Some(
            [("api_key", "golden-api-key-123456"), ("page", "2")]
                .into_iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        );
        api.api_body = Some(serde_json::json!({"token": "golden-body-token-1234"}));

        let artifact = ArtifactBundlePage {
            id: "page-golden".into(),
            title: "Dashboard".into(),
            slug: "dashboard".into(),
            html: "<h1>Dashboard</h1><p>password=hunter2-golden</p>".into(),
            created_by_agent: Some("agent".into()),
            embed_origins: Vec::new(),
            datasets: vec![crate::models::ArtifactBundleDataset {
                name: "visits".into(),
                kind: crate::models::LivePageDatasetKind::TimeSeries,
                has_current: true,
                current: serde_json::json!({"total": 3}),
                schema: None,
                max_points: 100,
                max_age_days: Some(30),
                updated_at: timestamp,
                points: vec![crate::models::ArtifactBundlePoint {
                    observed_at: timestamp,
                    payload: serde_json::json!({"n": 3}),
                    dedupe_key: Some("k".into()),
                }],
            }],
        };

        vec![
            (
                "skill",
                render_skill(
                    &skill_with(
                        "Review Diffs",
                        "Review the diff.",
                        "Read it. Never paste password=hunter2-golden.",
                    ),
                    timestamp,
                    "review",
                )
                .unwrap(),
            ),
            ("workflow", render_workflow(&workflow, "nightly").unwrap()),
            (
                "quick_prompt",
                render_quick_prompt(
                    &sample_prompt("Deploy with password=hunter2-golden and {{env}}"),
                    "deploy-prompt",
                )
                .unwrap(),
            ),
            ("quick_api", render_quick_api(&api, "deploy-api").unwrap()),
            (
                "quick_exec",
                render_quick_exec(&sample_exec("golden-exec-token-123456"), "deploy").unwrap(),
            ),
            (
                "artifact",
                render_artifact(&artifact, timestamp, "dashboard").unwrap(),
            ),
        ]
    }

    /// Everything a rendering holds, as one comparable line: the name, the
    /// hash, the secrets it needs, the SHA-256 of each file and of the document.
    fn rendering_report(rendered: &RenderedRepositoryResource) -> String {
        let files = rendered
            .files
            .iter()
            .map(|(path, bytes)| format!("{path}={}", sha256(bytes)))
            .collect::<Vec<_>>()
            .join(",");
        let document = serde_json::to_string(&rendered.document).unwrap();
        format!(
            "{}|{}|{}|{}|{}",
            rendered.name,
            rendered.hash,
            rendered.required_secrets.join(","),
            files,
            sha256(document.as_bytes()),
        )
    }

    /// The renderings as they were before the memo stopped copying them
    /// (KT-915): the same bytes for every kind, the first time and when served
    /// from memory.
    #[test]
    fn a_rendering_of_each_kind_is_byte_for_byte_what_it_was() {
        // Captured from the code before KT-915, on the same inputs.
        const GOLDEN: &[(&str, &str)] = &[
            ("skill", "Review Diffs|25b7d61d9af630b921ee974a6869a1a534fe6c07cddbd2e0324cc789bf619b7b|KRONN_SKILL_REVIEW_RESOURCE_CONTENT|.agents/skills/review/SKILL.md=8eafc218dc94b47b4111c2782555105a7c5b047c416765939716c1b8ab6028c0|df734bb3f7c52ea936112bda29a2adccf1d3a1cd9d2bf6b1a5d6cef17a919757"),
            ("workflow", "Nightly|d7682da200ed0f772d00120da678a61075639be4571a8e8736dc795d7b58457f|KRONN_WORKFLOW_NIGHTLY_RESOURCE_STEPS_0_API_BODY_PASSWORD,KRONN_WORKFLOW_NIGHTLY_RESOURCE_STEPS_0_API_HEADERS_AUTHORIZATION,KRONN_WORKFLOW_NIGHTLY_RESOURCE_STEPS_0_API_QUERY_TOKEN,KRONN_WORKFLOW_NIGHTLY_RESOURCE_STEPS_1_PROMPT_TEMPLATE|kronn/workflows/nightly.yaml=f1637d40dbd2f9df01552e94ab6ffda91e342966c800f9a060f2d1dcc10e467b|ade233b7459dfbc71fffb31af98520e432b9a7d71281022f48173377f465dab7"),
            ("quick_prompt", "Deploy prompt|8e7a2d997c74ac2d17a0b56716de53b744f04c199dba922d1fb17a239e7a55d2|KRONN_QUICKPROMPT_DEPLOY_PROMPT_RESOURCE_PROMPT_TEMPLATE|kronn/prompts/deploy-prompt.md=4c60f7029a19acbec6d39bf74cc2482660ee3e0dbf6aac42f3917e0f518c608b|fd85ecd6fd4dcfadee43d99631edb3b6f1bdd0130e7c06f9724f46bbcc3d9494"),
            ("quick_api", "Deploy API|0e8da30d7d9dd62992126b2666ae17362e018482b5f1e4c4d64bd45c0e84dcc7|KRONN_QUICKAPI_DEPLOY_API_RESOURCE_API_BODY_TOKEN,KRONN_QUICKAPI_DEPLOY_API_RESOURCE_API_HEADERS_AUTHORIZATION,KRONN_QUICKAPI_DEPLOY_API_RESOURCE_API_QUERY_API_KEY|kronn/quick-apis/deploy-api.yaml=fd34ca4c1de2297bc7ce03460cd53c47554755689df4c8e1daf01b031396dc90|6bc0dba4fabe88b1e29ef5330b9e03fa71ed302090759170ce01169faf7016e1"),
            ("quick_exec", "Deploy|a75df74e3326b413dbd65a19de210e8536feb23023146342d6c315c3ef6c6581|KRONN_QUICKEXEC_DEPLOY_RESOURCE_ARGS_1|kronn/quick-execs/deploy.yaml=3c1c60800277774fe7e66a72b52f35c09edab68a0acb0810dcfbb9aba02fbffd|497df76e8905142dccf14134668d0a40aaf45f7429280c4495af5e42ee409ad9"),
            ("artifact", "Dashboard|49b445a3915484650fed7d4ebd1b8e7024574698ac898253da9080fa02e573dd|KRONN_ARTIFACT_DASHBOARD_RESOURCE_HTML|kronn/artifacts/dashboard/artifact.yaml=e7c8a57d1cb02dbd749d4e9e8495186ee4853389bcddc8a2935de3b84da5d67a,kronn/artifacts/dashboard/index.html=bfde334af049d419908f618093509446e0dddc03b0666f9ccca8406d325b56b8|7712afc0eb98dea2d0c6b0d758795a0fdd2e0d243d458be228db2851ef0f5d1a"),
        ];
        let first = one_of_each_kind();
        let again = one_of_each_kind();
        for ((kind, rendered), (_, memoized)) in first.iter().zip(&again) {
            assert_eq!(
                rendering_report(rendered),
                rendering_report(memoized),
                "{kind}: served from memory it reads the same"
            );
        }
        let reports: Vec<(&str, String)> = first
            .iter()
            .map(|(kind, rendered)| (*kind, rendering_report(rendered)))
            .collect();
        assert_eq!(reports.len(), GOLDEN.len(), "one golden per kind");
        for ((kind, report), (golden_kind, golden)) in reports.iter().zip(GOLDEN) {
            assert_eq!(kind, golden_kind);
            assert_eq!(report, golden, "{kind} no longer renders the same bytes");
        }
    }

    /// A human's approval is recorded against this fingerprint: it must keep
    /// its value for every kind, whatever changes in how it is computed.
    #[test]
    fn the_approval_fingerprint_of_each_kind_is_what_it_was() {
        // Captured from the code before KT-915, on the same inputs.
        const GOLDEN: &[(&str, &str)] = &[
            (
                "skill",
                "5d304a12154f6a2f6dfea10ade3146433afad0c454697598340b772bca49bbbd",
            ),
            (
                "workflow",
                "7db581d36db5956dcd9bc203904cd49ec2ea5de2beddec254e0070175069f62e",
            ),
            (
                "quick_prompt",
                "3ff492d895b5381a21e623c5cddfb04a0d95e9110e1798b78bd9ec3539fd281e",
            ),
            (
                "quick_api",
                "fb38880bd2400abd299cd03770e67656364528d4dc2e97c35f7ebde6a4a546ad",
            ),
            (
                "quick_exec",
                "da4c4edfcbc5a4d7a471e2a29a892ba69b610f9e3427184c1edc51ba3384e87d",
            ),
            (
                "artifact",
                "dd00399f571be35bc512e23eba03c29f8e7843387f77f3c431ee1eb97d8ebfd8",
            ),
        ];
        let fingerprints: Vec<(&str, String)> = one_of_each_kind()
            .iter()
            .map(|(kind, rendered)| (*kind, approval_hash(&rendered.document)))
            .collect();
        assert_eq!(fingerprints.len(), GOLDEN.len(), "one golden per kind");
        for ((kind, fingerprint), (golden_kind, golden)) in fingerprints.iter().zip(GOLDEN) {
            assert_eq!(kind, golden_kind);
            assert_eq!(fingerprint, golden, "{kind} would lose its approvals");
        }
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
