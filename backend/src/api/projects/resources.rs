use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as AxumPath, Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::resource_links::{
    definition_references, link_resources, ResourceKey, ResourceReferences,
};
use crate::models::{
    ApiErrorCode, ApiResponse, ApproveProjectRepositoryResourceRequest, ArtifactBundlePage,
    CreateLivePageDataset, ImportProjectRepositoryResourceRequest, LivePage, LivePageRevision,
    ProjectRepositoryResource, ProjectRepositoryResourceKind, ProjectRepositoryResourceLevel,
    ProjectRepositoryResourceMutation, ProjectRepositoryResourceStatus, ProjectRepositoryResources,
    ProjectRepositorySkill, ProjectRepositorySkillProvenance,
    PublishProjectRepositoryResourceRequest, QuickApi, QuickExec, QuickPrompt,
    RepositoryNativeSkillRequest, RepositoryResourceComparison, RepositoryResourceFileContent,
    RepositoryResourceFileDiff, ResourceAdrLevel, Skill, UpdateLivePageRequest, Workflow,
};
use crate::AppState;

/// Where skills are looked for. `kronn/skills` is where Kronn used to write
/// them: it is still read (and offered for migration), never written.
pub(crate) const PROJECT_SKILL_ROOTS: &[&str] = &[
    "kronn/skills",
    ".claude/skills",
    ".agents/skills",
    ".codex/skills",
    ".github/skills",
    ".opencode/skills",
    ".opencode/skill",
    ".cursor/skills",
    ".vibe/skills",
    ".kiro/skills",
    ".gemini/skills",
];

/// Every directory whose files the listing dates: `kronn/` and the native
/// skill folders.
fn managed_directories() -> Vec<&'static str> {
    std::iter::once("kronn")
        .chain(
            PROJECT_SKILL_ROOTS
                .iter()
                .copied()
                .filter(|root| !root.starts_with("kronn/")),
        )
        .collect()
}

#[derive(Default)]
struct RepositorySkillSeed {
    name: String,
    repository_paths: Vec<String>,
}

/// A Kronn-side resource as it was read from the database, typed once and
/// ready to be rendered. Reading and rendering are kept apart on purpose:
/// rendering masks every string of the resource and needs no connection, so a
/// caller can read under the database lock and render outside it.
pub(super) enum LoadedResource {
    Skill(Skill),
    Workflow(Workflow),
    QuickPrompt(QuickPrompt),
    QuickApi(QuickApi),
    QuickExec(QuickExec),
    /// The page as a bundle carries it, with the date of the page itself.
    Artifact {
        page: ArtifactBundlePage,
        updated_at: DateTime<Utc>,
    },
}

impl LoadedResource {
    /// The resource as Kronn would write it under `slug`. A skill carries no
    /// date of its own: `skill_updated_at` stands in for one.
    pub(super) fn render(
        &self,
        slug: &str,
        skill_updated_at: Option<DateTime<Utc>>,
    ) -> anyhow::Result<crate::core::repository_resources::RenderedRepositoryResource> {
        use crate::core::repository_resources as render;
        match self {
            Self::Skill(skill) => {
                render::render_skill(skill, skill_updated_at.unwrap_or_else(Utc::now), slug)
            }
            Self::Workflow(workflow) => render::render_workflow(workflow, slug),
            Self::QuickPrompt(prompt) => render::render_quick_prompt(prompt, slug),
            Self::QuickApi(api) => render::render_quick_api(api, slug),
            Self::QuickExec(exec) => render::render_quick_exec(exec, slug),
            Self::Artifact { page, updated_at } => render::render_artifact(page, *updated_at, slug),
        }
        .map_err(anyhow::Error::msg)
    }
}

/// A resource of a project, as the listing and the warm-up see it before it is
/// placed against the repository.
pub(super) struct ResourceSeed {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) slug: Option<String>,
    pub(super) kind: ProjectRepositoryResourceKind,
    /// What the seed was read as: rendering it reads nothing again.
    pub(super) loaded: LoadedResource,
}

impl ResourceSeed {
    fn workflow(item: Workflow) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            slug: None,
            kind: ProjectRepositoryResourceKind::Workflow,
            loaded: LoadedResource::Workflow(item),
        }
    }

    fn quick_prompt(item: QuickPrompt) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            slug: None,
            kind: ProjectRepositoryResourceKind::QuickPrompt,
            loaded: LoadedResource::QuickPrompt(item),
        }
    }

    fn quick_api(item: QuickApi) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            slug: None,
            kind: ProjectRepositoryResourceKind::QuickApi,
            loaded: LoadedResource::QuickApi(item),
        }
    }

    fn quick_exec(item: QuickExec) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            slug: None,
            kind: ProjectRepositoryResourceKind::QuickExec,
            loaded: LoadedResource::QuickExec(item),
        }
    }

    /// The seed of a page: its bundle is read here, the heaviest read of the
    /// five kinds (the revision and every dataset with its points).
    pub(super) fn artifact(conn: &rusqlite::Connection, page: LivePage) -> anyhow::Result<Self> {
        let bundle = crate::api::artifact_portability::export_page(conn, &page.id)?;
        Ok(Self {
            id: page.id,
            name: page.title,
            slug: Some(page.slug),
            kind: ProjectRepositoryResourceKind::Artifact,
            loaded: LoadedResource::Artifact {
                page: bundle,
                updated_at: page.updated_at,
            },
        })
    }
}

/// The workflows, Quick Prompts, Quick APIs and Quick Execs of a project, each
/// read once and in that order. Only the rows of this project are read.
pub(super) fn typed_seeds(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> anyhow::Result<Vec<ResourceSeed>> {
    let mut seeds = Vec::new();
    for workflow in crate::db::workflows::list_workflows_for_project(conn, project_id)? {
        seeds.push(ResourceSeed::workflow(portable_workflow(conn, workflow)?));
    }
    seeds.extend(
        crate::db::quick_prompts::list_quick_prompts_for_project(conn, project_id)?
            .into_iter()
            .map(ResourceSeed::quick_prompt),
    );
    seeds.extend(
        crate::db::quick_apis::list_quick_apis_for_project(conn, project_id)?
            .into_iter()
            .map(ResourceSeed::quick_api),
    );
    seeds.extend(
        crate::db::quick_execs::list_quick_execs_for_project(conn, project_id)?
            .into_iter()
            .map(ResourceSeed::quick_exec),
    );
    Ok(seeds)
}

/// Every Kronn-side resource of a project, read once: the typed ones, then the
/// Artifacts.
fn project_seeds(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> anyhow::Result<Vec<ResourceSeed>> {
    let mut seeds = typed_seeds(conn, project_id)?;
    for page in crate::db::live_pages::list_live_pages_for_project(conn, project_id)? {
        seeds.push(ResourceSeed::artifact(conn, page)?);
    }
    Ok(seeds)
}

/// A workflow as `kronn/` carries it: references to other resources by slug,
/// never by this instance's ids (KT-917).
pub(super) fn portable_workflow(
    conn: &rusqlite::Connection,
    mut workflow: Workflow,
) -> anyhow::Result<Workflow> {
    crate::core::resource_refs::symbolize_workflow(conn, &mut workflow)?;
    Ok(workflow)
}

/// Reads a resource by kind and id.
pub(super) fn load_database_resource(
    conn: &rusqlite::Connection,
    kind: ProjectRepositoryResourceKind,
    id: &str,
) -> anyhow::Result<LoadedResource> {
    Ok(match kind {
        ProjectRepositoryResourceKind::Skill => LoadedResource::Skill(
            crate::core::skills::get_skill(id)
                .ok_or_else(|| anyhow::anyhow!("Skill not found: {id}"))?,
        ),
        ProjectRepositoryResourceKind::Workflow => LoadedResource::Workflow(portable_workflow(
            conn,
            crate::db::workflows::get_workflow(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Workflow not found: {id}"))?,
        )?),
        ProjectRepositoryResourceKind::QuickPrompt => LoadedResource::QuickPrompt(
            crate::db::quick_prompts::get_quick_prompt(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Quick Prompt not found: {id}"))?,
        ),
        ProjectRepositoryResourceKind::QuickApi => LoadedResource::QuickApi(
            crate::db::quick_apis::get_quick_api(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Quick API not found: {id}"))?,
        ),
        ProjectRepositoryResourceKind::QuickExec => LoadedResource::QuickExec(
            crate::db::quick_execs::get_quick_exec(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Quick Exec not found: {id}"))?,
        ),
        ProjectRepositoryResourceKind::Artifact => {
            // Only the page's own date is needed from the row: the bundle
            // read below carries the revision and the datasets.
            let page = crate::db::live_pages::get_live_page_summary(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Artifact not found: {id}"))?;
            LoadedResource::Artifact {
                page: crate::api::artifact_portability::export_page(conn, id)?,
                updated_at: page.updated_at,
            }
        }
    })
}

pub(super) fn render_database_resource(
    conn: &rusqlite::Connection,
    kind: ProjectRepositoryResourceKind,
    id: &str,
    slug: &str,
    skill_updated_at: Option<DateTime<Utc>>,
) -> anyhow::Result<crate::core::repository_resources::RenderedRepositoryResource> {
    load_database_resource(conn, kind, id)?.render(slug, skill_updated_at)
}

fn lock_entry(
    lock: Option<&crate::core::repository_resources::RepositoryLock>,
    kind: ProjectRepositoryResourceKind,
    slug: &str,
    name: &str,
    paths: &[String],
) -> crate::core::repository_resources::RepositoryLockResource {
    lock.and_then(|lock| {
        lock.resources
            .iter()
            .find(|entry| entry.kind == kind && entry.slug == slug)
            .cloned()
    })
    .unwrap_or_else(
        || crate::core::repository_resources::RepositoryLockResource {
            kind,
            slug: slug.to_string(),
            name: name.to_string(),
            level: String::new(),
            paths: paths.to_vec(),
            sha256: String::new(),
            required_secrets: Vec::new(),
        },
    )
}

impl ProjectRepositoryResourceKind {
    pub(super) fn identity_kind(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Workflow => "workflow",
            Self::QuickPrompt => "quick_prompt",
            Self::QuickApi => "quick_api",
            Self::QuickExec => "quick_exec",
            Self::Artifact => "artifact",
        }
    }

    fn level(self) -> ProjectRepositoryResourceLevel {
        match self {
            Self::Skill | Self::QuickPrompt | Self::QuickExec => {
                ProjectRepositoryResourceLevel::UsableWithoutKronn
            }
            Self::Workflow | Self::QuickApi | Self::Artifact => {
                ProjectRepositoryResourceLevel::KronnRequired
            }
        }
    }

    fn paths(self, slug: &str) -> Vec<String> {
        match self {
            Self::Skill => vec![crate::core::repository_resources::skill_path(slug)],
            Self::Workflow => vec![format!("kronn/workflows/{slug}.yaml")],
            Self::QuickPrompt => vec![format!("kronn/prompts/{slug}.md")],
            Self::QuickApi => vec![format!("kronn/quick-apis/{slug}.yaml")],
            Self::QuickExec => vec![format!("kronn/quick-execs/{slug}.yaml")],
            Self::Artifact => vec![
                format!("kronn/artifacts/{slug}/artifact.yaml"),
                format!("kronn/artifacts/{slug}/index.html"),
            ],
        }
    }
}

pub(super) fn parse_identity_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .ok()
}

/// Union of two path lists, sorted and deduplicated — merging keeps every
/// path a resource is known under instead of letting one side (typically
/// `kronn.lock`'s single canonical path) overwrite the other.
fn merge_paths(existing: &[String], other: &[String]) -> Vec<String> {
    let mut merged: BTreeSet<String> = existing.iter().cloned().collect();
    merged.extend(other.iter().cloned());
    merged.into_iter().collect()
}

fn parse_adr_level(raw: &str) -> ResourceAdrLevel {
    match raw {
        "N2" => ResourceAdrLevel::N2,
        "N1" => ResourceAdrLevel::N1,
        _ => ResourceAdrLevel::N0,
    }
}

/// Every path a publish of this resource would write: its own target paths
/// plus the shared scaffold (index, config, router skill, `docs/AGENTS.md`
/// line when missing) — shown regardless of the current sync state, so the
/// target path is visible even before the file exists. The scaffold is the
/// same for every resource, so the caller reads it once per listing.
fn write_preview_paths(side_effects: &[String], own_paths: &[String]) -> Vec<String> {
    let mut preview: Vec<String> = own_paths.to_vec();
    preview.extend(side_effects.iter().cloned());
    preview.sort();
    preview.dedup();
    preview
}

/// What the listing shows of one resource's alignment. It says *that* the two
/// sides differ (`status`), never *how*: the diffs are built by
/// `resource_comparison` when someone opens the Compare sheet.
struct AlignmentView {
    status: ProjectRepositoryResourceStatus,
    /// The state of the content alone, before an approval request takes over
    /// `status`: what decides whether there is anything to compare.
    sync_status: ProjectRepositoryResourceStatus,
    approval_required: bool,
    approved: bool,
    kronn_updated_at: Option<DateTime<Utc>>,
    aligned_at: Option<DateTime<Utc>>,
    required_secret_names: Vec<String>,
    repository_fingerprint: Option<String>,
    kronn_fingerprint: Option<String>,
}

/// What one read of a resource's files in the repository leaves for the rest of
/// the listing: the hash of the files and the level their definition declares.
struct RepositoryRead {
    hash: String,
    adr_level: ResourceAdrLevel,
}

/// Kinds Kronn can execute: the only ones an imported definition must be
/// approved for. Skills and artifacts run nothing, so they never wait on it.
fn kind_needs_approval(kind: ProjectRepositoryResourceKind) -> bool {
    matches!(
        kind,
        ProjectRepositoryResourceKind::Workflow
            | ProjectRepositoryResourceKind::QuickPrompt
            | ProjectRepositoryResourceKind::QuickApi
            | ProjectRepositoryResourceKind::QuickExec
    )
}

pub(super) fn read_repository_file(root: &Path, relative: &str) -> Option<Vec<u8>> {
    let path = root.join(relative);
    crate::core::fs_guard::assert_contained_no_symlink(root, &path).ok()?;
    std::fs::read(path).ok()
}

/// What the sheet carries of one side of one file: a longer text is cut here
/// and flagged, so a large artifact cannot turn the answer into megabytes. The
/// diff is built from the whole file, whatever is cut.
const MAX_CONTENT_BYTES: usize = 512 * 1024;

/// The text of one side of one file as the answer carries it: masked first,
/// whichever side it comes from, then cut — so a secret straddling the cut is
/// never half shown.
pub(super) fn side_text(bytes: &[u8]) -> (String, bool) {
    let text = crate::core::repository_resources::masked_text(bytes);
    if text.len() <= MAX_CONTENT_BYTES {
        return (text.as_str().to_string(), false);
    }
    let mut end = MAX_CONTENT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// Every file a resource has on either side, with its text on each, both masked
/// the same way: the repository's as it stands in the file, Kronn's as the
/// rendering a publish would write. A file neither side holds is left out;
/// `first` (the main file) comes first, the rest in path order.
fn resource_file_contents(
    root: &Path,
    repository_paths: &[String],
    kronn_files: &BTreeMap<String, Vec<u8>>,
    first: Option<&String>,
) -> Vec<RepositoryResourceFileContent> {
    let paths: BTreeSet<&String> = repository_paths.iter().chain(kronn_files.keys()).collect();
    let mut ordered: Vec<&String> = paths.into_iter().collect();
    ordered.sort_by_key(|path| Some(*path) != first);
    ordered
        .into_iter()
        .filter_map(|path| {
            let repository = read_repository_file(root, path);
            let kronn = kronn_files.get(path);
            if repository.is_none() && kronn.is_none() {
                return None;
            }
            let (repository, repository_cut) = repository
                .as_deref()
                .map(side_text)
                .map_or((None, false), |(text, cut)| (Some(text), cut));
            let (kronn, kronn_cut) = kronn
                .map(Vec::as_slice)
                .map(side_text)
                .map_or((None, false), |(text, cut)| (Some(text), cut));
            Some(RepositoryResourceFileContent {
                path: path.clone(),
                repository,
                kronn,
                truncated: repository_cut || kronn_cut,
            })
        })
        .collect()
}

/// One unified diff per file the resource is written to, repository side
/// against Kronn side; a file present on one side only diffs against nothing.
/// Both sides are the masked texts, so the diff shows no more than the files'
/// contents do and a secret held by both never reads as a difference.
fn resource_file_diffs(
    root: &Path,
    entry: &crate::core::repository_resources::RepositoryLockResource,
    rendered: &crate::core::repository_resources::RenderedRepositoryResource,
) -> Vec<RepositoryResourceFileDiff> {
    let paths: BTreeSet<&String> = entry.paths.iter().chain(rendered.files.keys()).collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let repository_bytes = read_repository_file(root, path).unwrap_or_default();
            let kronn_bytes = rendered
                .files
                .get(path)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let diff = crate::core::repository_resources::unified_diff(
                crate::core::repository_resources::masked_text(&repository_bytes).as_bytes(),
                crate::core::repository_resources::masked_text(kronn_bytes).as_bytes(),
            );
            (!diff.is_empty()).then(|| RepositoryResourceFileDiff {
                path: path.clone(),
                diff,
            })
        })
        .collect()
}

/// The path a one-file diff should target: an artifact's rendered HTML
/// rather than its `artifact.yaml` metadata sidecar, else the resource's
/// (single) definition file.
fn primary_diff_path(
    entry: &crate::core::repository_resources::RepositoryLockResource,
) -> Option<&String> {
    if entry.kind == ProjectRepositoryResourceKind::Artifact {
        entry
            .paths
            .iter()
            .find(|path| path.ends_with("/index.html"))
            .or_else(|| entry.paths.first())
    } else {
        entry.paths.first()
    }
}

/// Both sides of one resource: the text of every file on each side and, when
/// the two differ, a unified diff per file and, for the kinds that have one, a
/// field-by-field diff of the definition. Only the states where the two sides
/// differ have a diff to show. Built on demand — it reads the repository files
/// and diffs the masked Kronn rendering, work the listing never does.
fn resource_comparison(
    root: &Path,
    entry: &crate::core::repository_resources::RepositoryLockResource,
    rendered: &crate::core::repository_resources::RenderedRepositoryResource,
    sync_status: ProjectRepositoryResourceStatus,
) -> RepositoryResourceComparison {
    let files = resource_file_contents(
        root,
        &entry.paths,
        &rendered.files,
        primary_diff_path(entry),
    );
    let needs_diff = matches!(
        sync_status,
        ProjectRepositoryResourceStatus::RepositoryNewer
            | ProjectRepositoryResourceStatus::KronnNewer
            | ProjectRepositoryResourceStatus::Conflict
    );
    if !needs_diff {
        return RepositoryResourceComparison {
            files,
            ..RepositoryResourceComparison::default()
        };
    }
    let file_diffs = resource_file_diffs(root, entry, rendered);
    let diff = primary_diff_path(entry)
        .and_then(|path| file_diffs.iter().find(|item| &item.path == path))
        .or_else(|| file_diffs.first())
        .map(|item| item.diff.clone());
    let field_diff = if matches!(
        entry.kind,
        ProjectRepositoryResourceKind::Workflow
            | ProjectRepositoryResourceKind::QuickApi
            | ProjectRepositoryResourceKind::QuickExec
    ) {
        crate::core::repository_resources::read_resource(root, entry)
            .ok()
            .map(|(document, _)| {
                let mut fields = crate::core::repository_resources::field_diff(
                    &document.resource,
                    &rendered.document.resource,
                );
                // The repository's values are as someone typed them: masked
                // like the file texts above. Kronn's are masked by the rendering.
                for field in &mut fields {
                    if let Some(value) = field.repository.as_mut() {
                        crate::core::repository_resources::mask_value_strings(value);
                    }
                }
                fields
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    RepositoryResourceComparison {
        files,
        diff,
        file_diffs,
        field_diff,
    }
}

/// The comparison of a skill whose repository copy is still under
/// `kronn/skills`: that copy as the standard file a migration would write
/// against the Kronn rendering, so the diff shows real differences and not the
/// change of file format. The repository text is that same standard file.
fn legacy_skill_comparison(
    root: &Path,
    slug: &str,
    legacy_path: &str,
    rendered: &crate::core::repository_resources::RenderedRepositoryResource,
    sync_status: ProjectRepositoryResourceStatus,
) -> RepositoryResourceComparison {
    let path = crate::core::repository_resources::skill_path(slug);
    let repository =
        crate::core::repository_resources::legacy_skill_as_standard(root, slug, legacy_path)
            .unwrap_or_default();
    let kronn = rendered
        .files
        .get(&path)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let (repository_text, repository_cut) = side_text(&repository);
    let (kronn_text, kronn_cut) = side_text(kronn);
    let files = vec![RepositoryResourceFileContent {
        path: path.clone(),
        repository: (!repository.is_empty()).then_some(repository_text),
        kronn: (!kronn.is_empty()).then_some(kronn_text),
        truncated: repository_cut || kronn_cut,
    }];
    if !matches!(
        sync_status,
        ProjectRepositoryResourceStatus::RepositoryNewer
            | ProjectRepositoryResourceStatus::KronnNewer
            | ProjectRepositoryResourceStatus::Conflict
    ) {
        return RepositoryResourceComparison {
            files,
            ..RepositoryResourceComparison::default()
        };
    }
    let diff = crate::core::repository_resources::unified_diff(
        crate::core::repository_resources::masked_text(&repository).as_bytes(),
        crate::core::repository_resources::masked_text(kronn).as_bytes(),
    );
    if diff.is_empty() {
        return RepositoryResourceComparison {
            files,
            ..RepositoryResourceComparison::default()
        };
    }
    RepositoryResourceComparison {
        files,
        diff: Some(diff.clone()),
        file_diffs: vec![RepositoryResourceFileDiff { path, diff }],
        field_diff: Vec::new(),
    }
}

fn alignment_status(
    conn: &rusqlite::Connection,
    root: &Path,
    entry: &crate::core::repository_resources::RepositoryLockResource,
    rendered: &crate::core::repository_resources::RenderedRepositoryResource,
    alignment: Option<&crate::db::repository_resources::ResourceAlignment>,
) -> anyhow::Result<AlignmentView> {
    let repository = crate::core::repository_resources::read_resource(root, entry).ok();
    alignment_view(
        conn,
        entry,
        rendered,
        alignment,
        repository.as_ref().map(|(_, hash)| hash.as_str()),
    )
}

/// `alignment_status` for a caller that has read the repository's side already:
/// `repository_hash` is the hash of the resource's files there, `None` when
/// they are missing or unreadable.
fn alignment_view(
    conn: &rusqlite::Connection,
    entry: &crate::core::repository_resources::RepositoryLockResource,
    rendered: &crate::core::repository_resources::RenderedRepositoryResource,
    alignment: Option<&crate::db::repository_resources::ResourceAlignment>,
    repository_hash: Option<&str>,
) -> anyhow::Result<AlignmentView> {
    let sync_status = match alignment {
        // Never aligned yet both sides exist: with no baseline neither can
        // be called newer, so equal content is in sync and anything else is
        // two versions for a human to choose between.
        None => match repository_hash {
            None => ProjectRepositoryResourceStatus::KronnOnly,
            Some(hash) if hash == rendered.hash => ProjectRepositoryResourceStatus::UpToDate,
            Some(_) => ProjectRepositoryResourceStatus::Conflict,
        },
        Some(alignment) => {
            let repository_changed = repository_hash != Some(alignment.repository_hash.as_str());
            let database_changed = rendered.hash != alignment.database_hash;
            match (repository_changed, database_changed) {
                (false, false) => ProjectRepositoryResourceStatus::UpToDate,
                (true, false) => ProjectRepositoryResourceStatus::RepositoryNewer,
                (false, true) => ProjectRepositoryResourceStatus::KronnNewer,
                (true, true) => ProjectRepositoryResourceStatus::Conflict,
            }
        }
    };
    let approved = match alignment {
        Some(alignment) if alignment.imported && kind_needs_approval(entry.kind) => {
            crate::db::repository_resources::is_approved(
                conn,
                &alignment.project_key,
                &alignment.kind,
                &alignment.slug,
                &crate::core::repository_resources::approval_hash(&rendered.document),
            )?
        }
        Some(_) => true,
        None => false,
    };
    let approval_required = kind_needs_approval(entry.kind)
        && alignment.is_some_and(|alignment| alignment.imported)
        && !approved;
    // Approval is the one action left regardless of the sync state — it
    // takes over `status` so exactly one primary action is ever shown.
    let status = if approval_required {
        ProjectRepositoryResourceStatus::ApprovalRequired
    } else {
        sync_status
    };

    let required_secret_names = if entry.required_secrets.is_empty() {
        rendered.required_secrets.clone()
    } else {
        entry.required_secrets.clone()
    };

    Ok(AlignmentView {
        status,
        sync_status,
        approval_required,
        approved,
        kronn_updated_at: Some(rendered.document.updated_at),
        aligned_at: alignment.and_then(|alignment| parse_identity_time(&alignment.aligned_at)),
        required_secret_names,
        repository_fingerprint: repository_hash.map(crate::core::repository_resources::fingerprint),
        kronn_fingerprint: Some(crate::core::repository_resources::fingerprint(
            &rendered.hash,
        )),
    })
}

fn skill_display_name(path: &Path, fallback: &str) -> String {
    let Ok(content) = std::fs::read_to_string(path) else {
        return fallback.to_string();
    };
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return fallback.to_string();
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(name) = trimmed.strip_prefix("name:") {
            let name = name
                .trim()
                .trim_matches(|character| character == '\"' || character == '\'');
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    fallback.to_string()
}

/// Skill folders present in the repository, with how many `SKILL.md` each holds.
fn discover_skill_roots(root: &Path) -> Vec<crate::models::ProjectSkillRoot> {
    PROJECT_SKILL_ROOTS
        .iter()
        .filter_map(|relative_root| {
            let entries = std::fs::read_dir(root.join(relative_root)).ok()?;
            let skill_count = entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter(|entry| {
                    !(*relative_root == ".agents/skills" && entry.file_name() == "kronn")
                })
                .filter(|entry| entry.path().join("SKILL.md").is_file())
                .count();
            Some(crate::models::ProjectSkillRoot {
                path: (*relative_root).to_string(),
                skill_count: u32::try_from(skill_count).unwrap_or(u32::MAX),
            })
        })
        .collect()
}

fn discover_repository_skills(root: &Path) -> BTreeMap<String, RepositorySkillSeed> {
    let mut skills = BTreeMap::<String, RepositorySkillSeed>::new();
    for relative_root in PROJECT_SKILL_ROOTS {
        let directory = root.join(relative_root);
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let slug = entry.file_name().to_string_lossy().into_owned();
            if slug.is_empty() {
                continue;
            }
            if *relative_root == ".agents/skills" && slug == "kronn" {
                continue;
            }
            let skill_path = entry.path().join("SKILL.md");
            if !skill_path.is_file() {
                continue;
            }
            let repository_path = format!("{relative_root}/{slug}/SKILL.md");
            let seed = skills.entry(slug.clone()).or_default();
            if seed.name.is_empty() {
                seed.name = skill_display_name(&skill_path, &slug);
            }
            if !seed.repository_paths.contains(&repository_path) {
                seed.repository_paths.push(repository_path);
            }
        }
    }
    for seed in skills.values_mut() {
        seed.repository_paths.sort();
        seed.repository_paths.dedup();
    }
    skills
}

/// The native seed(s) a catalog skill accounts for: the one sharing its own
/// slug, the one its `custom-` id was derived from, and the folder it was
/// copied from (`origin_slug`, when the copy's name no longer matches it).
fn take_repository_skill(
    repository_skills: &mut BTreeMap<String, RepositorySkillSeed>,
    skill_id: &str,
    origin_slug: Option<&str>,
) -> Option<RepositorySkillSeed> {
    let slug = crate::core::native_files::slug(skill_id);
    let mut seed = repository_skills.remove(&slug);
    for other_slug in skill_id
        .strip_prefix("custom-")
        .into_iter()
        .chain(origin_slug)
    {
        if let Some(other) = repository_skills.remove(other_slug) {
            let current = seed.get_or_insert_with(RepositorySkillSeed::default);
            if current.name.is_empty() {
                current.name = other.name;
            }
            current.repository_paths.extend(other.repository_paths);
            current.repository_paths.sort();
            current.repository_paths.dedup();
        }
    }
    seed
}

struct SkillRepositoryFacts {
    diverge: bool,
    updated_at: Option<DateTime<Utc>>,
    updated_by: Option<String>,
    fingerprint: Option<String>,
}

/// Whether the same slug's copies across native skill roots are not
/// byte-identical, when the first copy last changed on the repository side and
/// its fingerprint — all computed from the filesystem alone, no database needed.
fn skill_repository_facts(
    root: &Path,
    dates: &crate::core::repository_resources::RepositoryFileDates,
    paths: &[String],
) -> SkillRepositoryFacts {
    let mut sorted_paths = paths.to_vec();
    sorted_paths.sort();
    let mut fingerprint = None;
    let mut hashes: Vec<String> = Vec::new();
    for path in &sorted_paths {
        let Ok(bytes) = std::fs::read(root.join(path)) else {
            continue;
        };
        let hash = crate::core::repository_resources::sha256(&bytes);
        if fingerprint.is_none() {
            fingerprint = Some(crate::core::repository_resources::fingerprint(&hash));
        }
        hashes.push(hash);
    }
    hashes.sort();
    hashes.dedup();
    let (updated_at, updated_by) = sorted_paths
        .first()
        .map(|path| dates.updated_at(path))
        .unwrap_or((None, None));
    SkillRepositoryFacts {
        diverge: hashes.len() > 1,
        updated_at,
        updated_by,
        fingerprint,
    }
}

/// Where a skill attached to the project stands before any alignment is
/// known: a file already at its publication path means the repository holds
/// something Kronn has not aligned yet.
fn attached_skill_status(root: &Path, publication_path: &str) -> ProjectRepositoryResourceStatus {
    if root.join(publication_path).is_file() {
        ProjectRepositoryResourceStatus::RepositoryNewer
    } else {
        ProjectRepositoryResourceStatus::KronnOnly
    }
}

fn project_skills(
    project_id: &str,
    root: &Path,
    dates: &crate::core::repository_resources::RepositoryFileDates,
    linked_skill_ids: &[String],
    copy_origins: &BTreeMap<String, String>,
) -> anyhow::Result<(Vec<ProjectRepositorySkill>, Vec<ProjectRepositorySkill>)> {
    let catalog = crate::core::skills::list_all_skills();
    let mut repository_skills = discover_repository_skills(root);
    // What the detected stack proposes, by skill id, with the file that
    // triggered it. Kept apart from `repository_skills`: none of these is a
    // file of the repository.
    let suggestions: BTreeMap<String, String> =
        crate::api::audit::detect_project_skill_markers(root)
            .into_iter()
            .collect();

    let linked: BTreeSet<&str> = linked_skill_ids.iter().map(String::as_str).collect();
    let mut handled = BTreeSet::new();
    let mut present = Vec::new();
    let mut available = Vec::new();

    for skill in catalog {
        let repository = take_repository_skill(
            &mut repository_skills,
            &skill.id,
            copy_origins.get(&skill.id).map(String::as_str),
        );
        let is_linked = linked.contains(skill.id.as_str());
        handled.insert(skill.id.clone());
        let publication_path = crate::core::repository_resources::skill_path(
            &crate::core::native_files::slug(&skill.id),
        );
        let repository_paths = repository
            .as_ref()
            .map(|seed| seed.repository_paths.clone())
            .unwrap_or_default();
        let provenance = match (repository.is_some(), is_linked) {
            (true, true) => ProjectRepositorySkillProvenance::Both,
            (true, false) => ProjectRepositorySkillProvenance::Repository,
            (false, _) => ProjectRepositorySkillProvenance::Kronn,
        };
        let suggested_reason = if is_linked || repository.is_some() {
            None
        } else {
            suggestions.get(&skill.id).cloned()
        };
        let status = if is_linked {
            attached_skill_status(root, &publication_path)
        } else if repository.is_some() {
            ProjectRepositoryResourceStatus::RepositoryOnly
        } else {
            ProjectRepositoryResourceStatus::KronnOnly
        };
        let suggested = suggested_reason.is_some();
        let item = skill_entry(
            root,
            dates,
            SkillIdentity {
                slug: crate::core::native_files::slug(&skill.id),
                id: skill.id,
                name: skill.name,
                description: skill.description,
                is_builtin: Some(skill.is_builtin),
                suggested_reason,
            },
            provenance,
            status,
            repository_paths,
            publication_path,
        );
        if repository.is_some() || is_linked || suggested {
            present.push(item);
        } else {
            available.push(item);
        }
    }

    for skill_id in linked_skill_ids {
        if handled.contains(skill_id) {
            continue;
        }
        let slug = crate::core::native_files::slug(skill_id);
        let repository = take_repository_skill(
            &mut repository_skills,
            skill_id,
            copy_origins.get(skill_id).map(String::as_str),
        );
        let publication_path = crate::core::repository_resources::skill_path(&slug);
        let is_native = repository.is_some();
        let name = repository
            .as_ref()
            .map(|seed| seed.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| skill_id.clone());
        let repository_paths = repository
            .map(|seed| seed.repository_paths)
            .unwrap_or_default();
        let status = attached_skill_status(root, &publication_path);
        present.push(skill_entry(
            root,
            dates,
            SkillIdentity {
                id: skill_id.clone(),
                name,
                slug,
                description: String::new(),
                is_builtin: None,
                suggested_reason: None,
            },
            if is_native {
                ProjectRepositorySkillProvenance::Both
            } else {
                ProjectRepositorySkillProvenance::Kronn
            },
            status,
            repository_paths,
            publication_path,
        ));
    }

    for (slug, seed) in repository_skills {
        present.push(skill_entry(
            root,
            dates,
            SkillIdentity {
                id: format!("repository:{slug}"),
                name: if seed.name.is_empty() {
                    slug.clone()
                } else {
                    seed.name
                },
                description: String::new(),
                is_builtin: None,
                slug: slug.clone(),
                suggested_reason: None,
            },
            ProjectRepositorySkillProvenance::Repository,
            ProjectRepositoryResourceStatus::NativeSkill,
            seed.repository_paths,
            crate::core::repository_resources::skill_path(&slug),
        ));
    }

    present.sort_by_key(|skill| skill.name.to_lowercase());
    available.sort_by_key(|skill| skill.name.to_lowercase());
    tracing::debug!(
        project_id,
        present = present.len(),
        available = available.len(),
        "classified project skills"
    );
    Ok((present, available))
}

struct SkillIdentity {
    id: String,
    name: String,
    slug: String,
    description: String,
    is_builtin: Option<bool>,
    suggested_reason: Option<String>,
}

/// A skill as the listing first shows it: only what the filesystem and the
/// skill's own file can tell, alignment facts being layered on afterwards.
fn skill_entry(
    root: &Path,
    dates: &crate::core::repository_resources::RepositoryFileDates,
    identity: SkillIdentity,
    provenance: ProjectRepositorySkillProvenance,
    status: ProjectRepositoryResourceStatus,
    repository_paths: Vec<String>,
    publication_path: String,
) -> ProjectRepositorySkill {
    let facts = skill_repository_facts(root, dates, &repository_paths);
    ProjectRepositorySkill {
        kronn_updated_at: crate::core::skills::custom_skill_modified_at(&identity.id),
        id: identity.id,
        name: identity.name,
        slug: identity.slug,
        description: identity.description,
        provenance,
        is_builtin: identity.is_builtin,
        status,
        suggested: identity.suggested_reason.is_some(),
        suggested_reason: identity.suggested_reason,
        approval_required: false,
        approved: false,
        repository_paths,
        repository_paths_diverge: facts.diverge,
        publication_path,
        write_preview: Vec::new(),
        referenced: false,
        required_secrets: Vec::new(),
        adr_level: ResourceAdrLevel::N1,
        repository_updated_at: facts.updated_at,
        repository_updated_by: facts.updated_by,
        aligned_at: None,
        repository_fingerprint: facts.fingerprint,
        kronn_fingerprint: None,
    }
}

/// A skill attached to the project that `kronn.lock` also lists: rendered from
/// the catalog and set against the repository file, exactly as the listing
/// and the Compare sheet both need it.
struct AlignedSkill {
    rendered: crate::core::repository_resources::RenderedRepositoryResource,
    view: AlignmentView,
}

fn align_skill(
    conn: &rusqlite::Connection,
    root: &Path,
    project_key: &str,
    skill_id: &str,
    slug: &str,
    entry: &crate::core::repository_resources::RepositoryLockResource,
) -> anyhow::Result<AlignedSkill> {
    // A skill still under `kronn/skills` was aligned in the former file format,
    // so its baseline says nothing about today's rendering: it reads as never
    // aligned, its repository side compared as the standard file a migration to
    // `.agents/skills` would write.
    let alignment = if entry
        .paths
        .iter()
        .any(|path| crate::core::repository_resources::is_legacy_skill_path(path))
    {
        None
    } else {
        crate::db::repository_resources::find_alignment(conn, project_key, "skill", slug)?
    };
    // A skill carries no date of its own: pin the rendering to the baseline's,
    // or to the repository file's when never aligned, so identical content
    // hashes identically.
    let timestamp = alignment
        .as_ref()
        .and_then(|alignment| parse_identity_time(&alignment.aligned_at))
        .or_else(|| {
            crate::core::repository_resources::read_resource(root, entry)
                .ok()
                .map(|(document, _)| document.updated_at)
        });
    let rendered = render_database_resource(
        conn,
        ProjectRepositoryResourceKind::Skill,
        skill_id,
        slug,
        timestamp,
    )?;
    let view = alignment_status(conn, root, entry, &rendered, alignment.as_ref())?;
    Ok(AlignedSkill { rendered, view })
}

/// A Kronn-side automation or artifact placed against `kronn.lock` and the
/// baseline of its last alignment.
struct ResolvedResource {
    slug: String,
    rendered: crate::core::repository_resources::RenderedRepositoryResource,
    alignment: Option<crate::db::repository_resources::ResourceAlignment>,
    entry: crate::core::repository_resources::RepositoryLockResource,
}

/// The slug a resource is written under: the one its identity holds, else the
/// one its seed brings, else one made from its name.
pub(super) fn resource_slug(
    conn: &rusqlite::Connection,
    project_key: &str,
    seed: &ResourceSeed,
) -> anyhow::Result<String> {
    let identity = crate::db::resource_identities::find_by_target(
        conn,
        project_key,
        seed.kind.identity_kind(),
        &seed.id,
    )?;
    let generated_slug = || {
        let slug = crate::core::repository_resources::ascii_slug(&seed.name);
        if slug.is_empty() {
            let id_prefix: String = seed.id.chars().take(8).collect();
            format!("resource-{id_prefix}")
        } else {
            slug
        }
    };
    Ok(identity
        .as_ref()
        .map(|item| item.slug.clone())
        .or_else(|| seed.slug.clone())
        .unwrap_or_else(generated_slug))
}

fn resolve_resource(
    conn: &rusqlite::Connection,
    project_key: &str,
    lock: Option<&crate::core::repository_resources::RepositoryLock>,
    seed: &ResourceSeed,
) -> anyhow::Result<ResolvedResource> {
    let slug = resource_slug(conn, project_key, seed)?;
    let repository_paths = seed.kind.paths(&slug);
    let rendered = seed.loaded.render(&slug, None)?;
    let alignment = crate::db::repository_resources::find_alignment(
        conn,
        project_key,
        seed.kind.identity_kind(),
        &slug,
    )?;
    let entry = lock_entry(lock, seed.kind, &slug, &seed.name, &repository_paths);
    Ok(ResolvedResource {
        slug,
        rendered,
        alignment,
        entry,
    })
}

/// GET /api/projects/:id/repository-resources
pub async fn repository_resources(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
) -> Json<ApiResponse<ProjectRepositoryResources>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            let Some(project) = crate::db::projects::get_project(conn, &project_id)? else {
                return Ok(None);
            };
            let project_key = crate::db::resource_identities::project_key(conn, Some(&project_id))?;
            // Each resource is read once, here, and rendered from what was
            // read: the rows of the other projects are not touched at all.
            let seeds = project_seeds(conn, &project_id)?;

            let root = PathBuf::from(&project.path);
            let side_effects = crate::core::repository_resources::publish_side_effect_paths(&root);
            let dates = crate::core::repository_resources::RepositoryFileDates::read(
                &root,
                &managed_directories(),
            );
            let lock =
                crate::core::repository_resources::load_lock(&root).map_err(anyhow::Error::msg)?;
            let configured_secrets =
                crate::core::repository_resources::configured_secret_names(conn, &project_id)?;
            let copy_origins = native_copy_origins(conn, &project_key, &project.default_skill_ids)?;
            let (mut skills_present, skills_available) = project_skills(
                &project_id,
                &root,
                &dates,
                &project.default_skill_ids,
                &copy_origins,
            )?;
            for skill in &mut skills_present {
                if matches!(
                    skill.provenance,
                    ProjectRepositorySkillProvenance::Repository
                        | ProjectRepositorySkillProvenance::Both
                ) {
                    skill.referenced =
                        crate::db::project_skill_references::find(conn, &project_id, &skill.slug)?
                            .is_some();
                }
                if !skill.id.starts_with("repository:") {
                    skill.write_preview = write_preview_paths(
                        &side_effects,
                        std::slice::from_ref(&skill.publication_path),
                    );
                }
                let Some(entry) = lock.as_ref().and_then(|lock| {
                    lock.resources.iter().find(|entry| {
                        entry.kind == ProjectRepositoryResourceKind::Skill
                            && entry.slug == skill.slug
                    })
                }) else {
                    continue;
                };
                skill.repository_paths = merge_paths(&skill.repository_paths, &entry.paths);
                if skill.id.starts_with("repository:") {
                    skill.status = ProjectRepositoryResourceStatus::RepositoryOnly;
                    skill.required_secrets =
                        crate::core::repository_resources::required_secret_statuses(
                            &entry.required_secrets,
                            &configured_secrets,
                        );
                    continue;
                }
                let AlignedSkill { view, .. } =
                    align_skill(conn, &root, &project_key, &skill.id, &skill.slug, entry)?;
                skill.status = view.status;
                skill.approval_required = view.approval_required;
                skill.approved = view.approved;
                skill.required_secrets =
                    crate::core::repository_resources::required_secret_statuses(
                        &view.required_secret_names,
                        &configured_secrets,
                    );
                if let Some((updated_at, updated_by)) =
                    primary_diff_path(entry).map(|path| dates.updated_at(path))
                {
                    skill.repository_updated_at = updated_at;
                    skill.repository_updated_by = updated_by;
                }
                skill.aligned_at = view.aligned_at;
                skill.repository_fingerprint = view.repository_fingerprint;
                skill.kronn_fingerprint = view.kronn_fingerprint;
            }
            // The repository side of every non-skill entry, read once: the id
            // its file was written under and what its definition points at. What
            // the rest of the listing needs of the same files (their hash, the
            // level the definition declares) is kept from this read, in the
            // order of the lock's entries, instead of reading them again.
            let mut repository_side: HashMap<ResourceKey, ResourceReferences> = HashMap::new();
            let mut repository_reads: Vec<Option<RepositoryRead>> = Vec::new();
            for entry in lock.iter().flat_map(|lock| lock.resources.iter()) {
                let read = if entry.kind == ProjectRepositoryResourceKind::Skill {
                    None
                } else {
                    crate::core::repository_resources::read_resource(&root, entry).ok()
                };
                let Some((document, hash)) = read else {
                    repository_reads.push(None);
                    continue;
                };
                repository_reads.push(Some(RepositoryRead {
                    hash,
                    adr_level: crate::core::repository_resources::resource_adr_level(&document),
                }));
                repository_side.insert(
                    (entry.kind.identity_kind(), entry.slug.clone()),
                    ResourceReferences {
                        aliases: document
                            .resource
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                            .into_iter()
                            .collect(),
                        references: definition_references(entry.kind, &document.resource),
                    },
                );
            }
            let mut link_sources: HashMap<ResourceKey, ResourceReferences> = HashMap::new();
            let mut resources = Vec::with_capacity(seeds.len());
            for seed in seeds {
                let ResolvedResource {
                    slug,
                    rendered,
                    alignment,
                    entry,
                } = resolve_resource(conn, &project_key, lock.as_ref(), &seed)?;
                let mut link_source = repository_side
                    .get(&(seed.kind.identity_kind(), slug.clone()))
                    .cloned()
                    .unwrap_or_default();
                link_source.references.extend(definition_references(
                    seed.kind,
                    &rendered.document.resource,
                ));
                // A workflow names its Artifact by id or by slug.
                link_source.aliases.extend(seed.slug.clone());
                if seed.kind == ProjectRepositoryResourceKind::Artifact {
                    link_source.aliases.push(slug.clone());
                }
                link_sources.insert((seed.kind.identity_kind(), seed.id.clone()), link_source);
                // An entry the lock lists was read above; one it does not list
                // is the conventional path of a resource never published, which
                // may still hold a file.
                let listed_at = lock.as_ref().and_then(|lock| {
                    lock.resources
                        .iter()
                        .position(|item| item.kind == seed.kind && item.slug == slug)
                });
                let view = match listed_at {
                    Some(index) => alignment_view(
                        conn,
                        &entry,
                        &rendered,
                        alignment.as_ref(),
                        repository_reads[index]
                            .as_ref()
                            .map(|read| read.hash.as_str()),
                    )?,
                    None => alignment_status(conn, &root, &entry, &rendered, alignment.as_ref())?,
                };
                let (repository_updated_at, repository_updated_by) = primary_diff_path(&entry)
                    .map(|path| dates.updated_at(path))
                    .unwrap_or((None, None));
                let write_preview = write_preview_paths(&side_effects, &entry.paths);
                resources.push(ProjectRepositoryResource {
                    id: seed.id,
                    name: seed.name,
                    slug,
                    kind: seed.kind,
                    level: seed.kind.level(),
                    adr_level: crate::core::repository_resources::resource_adr_level(
                        &rendered.document,
                    ),
                    status: view.status,
                    approval_required: view.approval_required,
                    approved: view.approved,
                    repository_paths: entry.paths,
                    write_preview,
                    required_secrets: crate::core::repository_resources::required_secret_statuses(
                        &view.required_secret_names,
                        &configured_secrets,
                    ),
                    repository_updated_at,
                    repository_updated_by,
                    kronn_updated_at: view.kronn_updated_at,
                    aligned_at: view.aligned_at,
                    repository_fingerprint: view.repository_fingerprint,
                    kronn_fingerprint: view.kronn_fingerprint,
                    uses: Vec::new(),
                    used_by: Vec::new(),
                });
            }
            if let Some(lock) = lock.as_ref() {
                let listed: BTreeSet<(&'static str, String)> = resources
                    .iter()
                    .map(|resource| (resource.kind.identity_kind(), resource.slug.clone()))
                    .collect();
                for (index, entry) in lock.resources.iter().enumerate() {
                    if entry.kind == ProjectRepositoryResourceKind::Skill
                        || listed.contains(&(entry.kind.identity_kind(), entry.slug.clone()))
                    {
                        continue;
                    }
                    let (repository_updated_at, repository_updated_by) = primary_diff_path(entry)
                        .map(|path| dates.updated_at(path))
                        .unwrap_or((None, None));
                    let repository_file = repository_reads[index].as_ref();
                    let repository_fingerprint = repository_file
                        .map(|read| crate::core::repository_resources::fingerprint(&read.hash));
                    let id = format!("repository:{}:{}", entry.kind.identity_kind(), entry.slug);
                    link_sources.insert(
                        (entry.kind.identity_kind(), id.clone()),
                        repository_side
                            .get(&(entry.kind.identity_kind(), entry.slug.clone()))
                            .cloned()
                            .unwrap_or_default(),
                    );
                    resources.push(ProjectRepositoryResource {
                        id,
                        name: entry.name.clone(),
                        slug: entry.slug.clone(),
                        kind: entry.kind,
                        level: entry.kind.level(),
                        adr_level: repository_file
                            .map(|read| read.adr_level)
                            .unwrap_or_else(|| parse_adr_level(&entry.level)),
                        status: ProjectRepositoryResourceStatus::RepositoryOnly,
                        approval_required: false,
                        approved: false,
                        write_preview: Vec::new(),
                        required_secrets:
                            crate::core::repository_resources::required_secret_statuses(
                                &entry.required_secrets,
                                &configured_secrets,
                            ),
                        repository_paths: entry.paths.clone(),
                        repository_updated_at,
                        repository_updated_by,
                        kronn_updated_at: None,
                        aligned_at: None,
                        repository_fingerprint,
                        kronn_fingerprint: None,
                        uses: Vec::new(),
                        used_by: Vec::new(),
                    });
                }
            }
            resources.sort_by_key(|resource| resource.name.to_lowercase());
            link_resources(&mut resources, &link_sources);
            let (can_write_repository, can_write_repository_reason) =
                crate::core::repository_resources::can_write_repository(&root);
            let uncommitted_managed_paths =
                crate::core::repository_resources::uncommitted_managed_paths(
                    &root,
                    &lock.clone().unwrap_or_default(),
                );
            Ok(Some(ProjectRepositoryResources {
                kronn_exists: root.join("kronn").is_dir(),
                skill_roots: discover_skill_roots(&root),
                skills_present,
                skills_available,
                resources,
                can_write_repository,
                can_write_repository_reason,
                uncommitted_managed_paths,
            }))
        })
        .await;

    match result {
        Ok(Some(resources)) => Json(ApiResponse::ok(resources)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Project not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to read project repository resources: {error}"),
        )),
    }
}

/// Which resource the Compare sheet is opened on.
#[derive(Debug, serde::Deserialize)]
pub struct RepositoryComparisonQuery {
    pub kind: ProjectRepositoryResourceKind,
    /// The id the listing gave it: a skill's, or the Kronn resource's.
    pub id: String,
}

/// The Kronn-side resource `kind`/`id` of this project, as the listing seeds
/// it. `None` when it does not exist, or belongs to another project.
fn seed_of(
    conn: &rusqlite::Connection,
    project_id: &str,
    kind: ProjectRepositoryResourceKind,
    id: &str,
) -> anyhow::Result<Option<ResourceSeed>> {
    let ours = |owner: Option<&str>| owner == Some(project_id);
    Ok(match kind {
        ProjectRepositoryResourceKind::Skill => None,
        ProjectRepositoryResourceKind::Workflow => {
            match crate::db::workflows::get_workflow(conn, id)?
                .filter(|item| ours(item.project_id.as_deref()))
            {
                Some(workflow) => Some(ResourceSeed::workflow(portable_workflow(conn, workflow)?)),
                None => None,
            }
        }
        ProjectRepositoryResourceKind::QuickPrompt => {
            crate::db::quick_prompts::get_quick_prompt(conn, id)?
                .filter(|item| ours(item.project_id.as_deref()))
                .map(ResourceSeed::quick_prompt)
        }
        ProjectRepositoryResourceKind::QuickApi => crate::db::quick_apis::get_quick_api(conn, id)?
            .filter(|item| ours(item.project_id.as_deref()))
            .map(ResourceSeed::quick_api),
        ProjectRepositoryResourceKind::QuickExec => {
            crate::db::quick_execs::get_quick_exec(conn, id)?
                .filter(|item| ours(item.project_id.as_deref()))
                .map(ResourceSeed::quick_exec)
        }
        ProjectRepositoryResourceKind::Artifact => {
            match crate::db::live_pages::get_live_page_summary(conn, id)?
                .filter(|page| ours(page.project_id.as_deref()))
            {
                Some(page) => Some(ResourceSeed::artifact(conn, page)?),
                None => None,
            }
        }
    })
}

/// The files a resource has in the repository only, each with its text.
fn repository_only_comparison(root: &Path, paths: &[String]) -> RepositoryResourceComparison {
    RepositoryResourceComparison {
        files: resource_file_contents(root, paths, &BTreeMap::new(), paths.first()),
        ..RepositoryResourceComparison::default()
    }
}

/// Both sides of one skill row, read the way the listing reads it: a skill
/// `kronn.lock` lists is aligned against its baseline; one attached to the
/// project is compared with the file at its publication path; one only found in
/// a native folder is that folder's files and nothing in Kronn; one the catalog
/// alone holds is Kronn's rendering and nothing in the repository.
fn skill_comparison(
    conn: &rusqlite::Connection,
    project: &crate::models::Project,
    root: &Path,
    project_key: &str,
    lock: Option<&crate::core::repository_resources::RepositoryLock>,
    skill_id: &str,
) -> anyhow::Result<RepositoryResourceComparison> {
    let lock_entry_of = |slug: &str| {
        lock.and_then(|lock| {
            lock.resources.iter().find(|entry| {
                entry.kind == ProjectRepositoryResourceKind::Skill && entry.slug == slug
            })
        })
    };
    if let Some(slug) = skill_id.strip_prefix("repository:") {
        // A skill found in a native folder that the catalog does not know.
        let mut paths = discover_repository_skills(root)
            .remove(slug)
            .map(|seed| seed.repository_paths)
            .unwrap_or_default();
        if let Some(entry) = lock_entry_of(slug) {
            paths = merge_paths(&paths, &entry.paths);
        }
        return Ok(repository_only_comparison(root, &paths));
    }
    let slug = crate::core::native_files::slug(skill_id);
    let publication_path = crate::core::repository_resources::skill_path(&slug);
    let Some(skill) = crate::core::skills::get_skill(skill_id) else {
        // Attached once, gone from the catalog since: only the file is left.
        return Ok(repository_only_comparison(
            root,
            std::slice::from_ref(&publication_path),
        ));
    };
    if let Some(entry) = lock_entry_of(&slug) {
        let AlignedSkill { rendered, view } =
            align_skill(conn, root, project_key, skill_id, &slug, entry)?;
        if let Some(legacy) = entry
            .paths
            .iter()
            .find(|path| crate::core::repository_resources::is_legacy_skill_path(path))
        {
            return Ok(legacy_skill_comparison(
                root,
                &slug,
                legacy,
                &rendered,
                view.sync_status,
            ));
        }
        return Ok(resource_comparison(
            root,
            entry,
            &rendered,
            view.sync_status,
        ));
    }
    let linked = project.default_skill_ids.iter().any(|id| id == skill_id);
    if !linked {
        let copy_origins = native_copy_origins(conn, project_key, &project.default_skill_ids)?;
        if let Some(seed) = take_repository_skill(
            &mut discover_repository_skills(root),
            skill_id,
            copy_origins.get(skill_id).map(String::as_str),
        ) {
            return Ok(repository_only_comparison(root, &seed.repository_paths));
        }
    }
    // The text does not depend on the date, so a fixed one keeps the memoized
    // rendering shared between opens.
    let rendered = render_database_resource(
        conn,
        ProjectRepositoryResourceKind::Skill,
        skill_id,
        &slug,
        Some(
            crate::core::skills::custom_skill_modified_at(skill_id)
                .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
        ),
    )?;
    let entry = lock_entry(
        None,
        ProjectRepositoryResourceKind::Skill,
        &slug,
        &skill.name,
        std::slice::from_ref(&publication_path),
    );
    let status = if linked {
        attached_skill_status(root, &publication_path)
    } else {
        ProjectRepositoryResourceStatus::KronnOnly
    };
    Ok(resource_comparison(root, &entry, &rendered, status))
}

/// GET /api/projects/:id/repository-resources/comparison?kind=…&id=…
///
/// Both sides of one row of the listing, built when its sheet opens: the text
/// of each file on each side that holds it and, where the two differ, the
/// diffs. Both sides are masked: the Kronn side comes from the rendering a
/// publish would write, and every text of the answer — the repository's files
/// too, which may hold a secret typed in by hand — goes through the same
/// masking before it is cut, diffed or sent. A resource with only one side
/// carries that side's text and no diff; one with both in sync carries the same
/// text twice and no diff.
pub async fn repository_resource_comparison(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Query(query): Query<RepositoryComparisonQuery>,
) -> Json<ApiResponse<RepositoryResourceComparison>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            let Some(project) = crate::db::projects::get_project(conn, &project_id)? else {
                return Ok(None);
            };
            let project_key = crate::db::resource_identities::project_key(conn, Some(&project_id))?;
            let root = PathBuf::from(&project.path);
            let lock =
                crate::core::repository_resources::load_lock(&root).map_err(anyhow::Error::msg)?;
            if query.kind == ProjectRepositoryResourceKind::Skill {
                return skill_comparison(
                    conn,
                    &project,
                    &root,
                    &project_key,
                    lock.as_ref(),
                    &query.id,
                )
                .map(Some);
            }
            let Some(seed) = seed_of(conn, &project_id, query.kind, &query.id)? else {
                // A repository-only row (`repository:kind:slug`) has no Kronn
                // side: its files are all there is. Anything else is unknown
                // to this project.
                let Some((kind, slug)) = query
                    .id
                    .strip_prefix("repository:")
                    .and_then(|rest| rest.split_once(':'))
                else {
                    return Ok(None);
                };
                let entry = lock.as_ref().and_then(|lock| {
                    lock.resources.iter().find(|entry| {
                        entry.kind == query.kind
                            && entry.slug == slug
                            && entry.kind.identity_kind() == kind
                    })
                });
                return Ok(Some(
                    entry.map_or_else(RepositoryResourceComparison::default, |entry| {
                        repository_only_comparison(&root, &entry.paths)
                    }),
                ));
            };
            let resolved = resolve_resource(conn, &project_key, lock.as_ref(), &seed)?;
            let view = alignment_status(
                conn,
                &root,
                &resolved.entry,
                &resolved.rendered,
                resolved.alignment.as_ref(),
            )?;
            Ok(Some(resource_comparison(
                &root,
                &resolved.entry,
                &resolved.rendered,
                view.sync_status,
            )))
        })
        .await;

    match result {
        Ok(Some(comparison)) => Json(ApiResponse::ok(comparison)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Project or resource not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to compare the repository resource: {error}"),
        )),
    }
}

fn imported_artifact(
    conn: &rusqlite::Connection,
    document: &crate::core::repository_resources::RepositoryDocument,
    project_id: &str,
    existing_id: Option<&str>,
) -> anyhow::Result<String> {
    let exported: ArtifactBundlePage = serde::Deserialize::deserialize(&document.resource)?;
    let page_id = existing_id
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let datasets = exported
        .datasets
        .iter()
        .map(|dataset| CreateLivePageDataset {
            name: dataset.name.clone(),
            kind: dataset.kind,
            initial: if dataset.kind == crate::models::LivePageDatasetKind::TimeSeries {
                Some(serde_json::Value::Array(
                    dataset
                        .points
                        .iter()
                        .map(|point| point.payload.clone())
                        .collect(),
                ))
            } else {
                dataset.has_current.then(|| dataset.current.clone())
            },
            schema: dataset.schema.clone(),
            max_points: Some(dataset.max_points),
            max_age_days: dataset.max_age_days,
        })
        .collect::<Vec<_>>();
    if existing_id.is_some() {
        crate::db::live_pages::update_live_page(
            conn,
            &page_id,
            &UpdateLivePageRequest {
                title: Some(exported.title.clone()),
                pinned: None,
                archived: Some(false),
            },
        )?;
        crate::db::live_pages::update_live_page_html(
            conn,
            &page_id,
            &exported.html,
            exported.created_by_agent.as_deref(),
        )?;
        conn.execute(
            "DELETE FROM live_page_datasets WHERE page_id = ?1",
            [&page_id],
        )?;
        for dataset in &datasets {
            crate::db::live_pages::add_live_page_dataset(conn, &page_id, dataset)?;
        }
        conn.execute(
            "UPDATE live_pages SET project_id = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![project_id, document.updated_at.to_rfc3339(), page_id],
        )?;
    } else {
        let revision_id = Uuid::new_v4().to_string();
        let page = LivePage {
            id: page_id.clone(),
            project_id: Some(project_id.to_string()),
            title: exported.title,
            slug: document.slug.clone(),
            current_revision_id: revision_id.clone(),
            data_revision: 0,
            created_at: document.updated_at,
            updated_at: document.updated_at,
            last_published_at: None,
            pinned: false,
            archived: false,
        };
        let revision = LivePageRevision {
            id: revision_id,
            page_id: page_id.clone(),
            revision: 1,
            html: exported.html,
            created_by_agent: exported.created_by_agent,
            created_at: document.updated_at,
        };
        crate::db::live_pages::create_live_page(conn, &page, &revision, &datasets, None)?;
    }
    Ok(page_id)
}

/// Whether importing `document` would erase edits that only exist on the
/// Kronn side: the current Kronn resource differs from the last aligned
/// baseline or, when there is none, from the repository file itself.
fn kronn_changes_would_be_lost(
    conn: &rusqlite::Connection,
    project_key: &str,
    document: &crate::core::repository_resources::RepositoryDocument,
    repository_hash: &str,
) -> anyhow::Result<bool> {
    let kind = document.kind;
    let Some(existing_id) = crate::db::resource_identities::lookup(
        conn,
        project_key,
        kind.identity_kind(),
        &document.slug,
    )?
    else {
        return Ok(false);
    };
    let alignment = crate::db::repository_resources::find_alignment(
        conn,
        project_key,
        kind.identity_kind(),
        &document.slug,
    )?;
    let timestamp = alignment
        .as_ref()
        .and_then(|alignment| parse_identity_time(&alignment.aligned_at))
        .unwrap_or(document.updated_at);
    // A target that no longer renders holds nothing to lose.
    let Ok(rendered) =
        render_database_resource(conn, kind, &existing_id, &document.slug, Some(timestamp))
    else {
        return Ok(false);
    };
    Ok(match alignment {
        Some(alignment) => rendered.hash != alignment.database_hash,
        None => rendered.hash != repository_hash,
    })
}

fn import_document(
    conn: &rusqlite::Connection,
    project_id: &str,
    project_key: &str,
    document: &crate::core::repository_resources::RepositoryDocument,
) -> anyhow::Result<String> {
    let kind = document.kind.identity_kind();
    let existing_id =
        crate::db::resource_identities::lookup(conn, project_key, kind, &document.slug)?;
    let now = document.updated_at;
    let target_id = match document.kind {
        ProjectRepositoryResourceKind::Skill => {
            let skill: Skill = serde::Deserialize::deserialize(&document.resource)?;
            let target_id = match existing_id.as_deref() {
                Some(id) if id.starts_with("custom-") => crate::core::skills::update_custom_skill(
                    id,
                    &skill.name,
                    &skill.description,
                    &skill.icon,
                    &skill.category,
                    &skill.content,
                    skill.license.as_deref(),
                    skill.allowed_tools.as_deref(),
                )
                .map_err(anyhow::Error::msg)?,
                _ => crate::core::skills::save_custom_skill(
                    &skill.name,
                    &skill.description,
                    &skill.icon,
                    &skill.category,
                    &skill.content,
                    skill.license.as_deref(),
                    skill.allowed_tools.as_deref(),
                )
                .map_err(anyhow::Error::msg)?,
            };
            let project = crate::db::projects::get_project(conn, project_id)?
                .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
            let mut skill_ids = project.default_skill_ids;
            if !skill_ids.contains(&target_id) {
                skill_ids.push(target_id.clone());
                crate::db::projects::update_project_default_skills(conn, project_id, &skill_ids)?;
            }
            target_id
        }
        ProjectRepositoryResourceKind::Workflow => {
            let mut resource: Workflow = serde::Deserialize::deserialize(&document.resource)?;
            resource.id = existing_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            resource.project_id = Some(project_id.to_string());
            resource.updated_at = now;
            resource.enabled = false;
            for step in resource
                .steps
                .iter_mut()
                .chain(resource.on_failure.iter_mut())
            {
                step.gate_notify_url = None;
                if let Some(config) = step.notify_config.as_mut() {
                    if config.url.starts_with("secret://") {
                        step.notify_config = None;
                    }
                }
            }
            // Project ids are this instance's: another machine's are dropped,
            // and a list left empty makes the workflow single-project (KT-851).
            if let Some(crate::models::WorkflowProjectScope::Projects { project_ids }) =
                resource.project_scope.as_mut()
            {
                let mut known = Vec::new();
                for id in project_ids.drain(..) {
                    if crate::db::projects::get_project(conn, &id)?.is_some() {
                        known.push(id);
                    }
                }
                *project_ids = known;
            }
            if matches!(
                &resource.project_scope,
                Some(crate::models::WorkflowProjectScope::Projects { project_ids }) if project_ids.is_empty()
            ) {
                resource.project_scope = None;
            }
            // The repository names other resources by slug (KT-917).
            let keep_resource_refs = resource.project_scope.is_some();
            for steps in [&mut resource.steps, &mut resource.on_failure] {
                crate::core::resource_refs::resolve_structured_references(
                    conn,
                    steps,
                    Some(project_id),
                    keep_resource_refs,
                )
                .map_err(anyhow::Error::msg)?;
            }
            crate::api::workflows::rebind_api_configs(conn, &mut resource.steps, Some(project_id));
            crate::api::workflows::rebind_api_configs(
                conn,
                &mut resource.on_failure,
                Some(project_id),
            );
            // The editor's save-time rules: approval must never see a
            // definition the editor would have refused.
            // A re-import keeps the stored workflow's unchanged unsafe lines.
            let stored = match existing_id.as_deref() {
                Some(id) => crate::db::workflows::get_workflow(conn, id)?,
                None => None,
            };
            let kept = stored
                .as_ref()
                .map(|stored| crate::api::workflows::kept_lines(&stored.steps, &stored.on_failure))
                .unwrap_or_default();
            // The file never carries an approval: only one this instance's
            // human gave to the very same line survives. A repository file is
            // something an agent can write, so a new or changed line is the
            // agent's and waits for a human in the editor (KT-1017).
            crate::api::workflows::drop_foreign_fields(&mut resource.steps);
            crate::api::workflows::drop_foreign_fields(&mut resource.on_failure);
            let (stored_steps, stored_failure) = stored
                .map(|stored| (stored.steps, stored.on_failure))
                .unwrap_or_default();
            crate::api::workflows::blank_unpinned_hashes(&mut resource.steps, &stored_steps);
            crate::api::workflows::blank_unpinned_hashes(&mut resource.on_failure, &stored_failure);
            crate::api::workflows::keep_human_approvals(&mut resource.steps, &stored_steps);
            crate::api::workflows::keep_human_approvals(&mut resource.on_failure, &stored_failure);
            crate::api::workflows::mark_line_writers(
                &mut resource.steps,
                &stored_steps,
                crate::api::workflows::WorkflowWriter::Agent,
            );
            crate::api::workflows::mark_line_writers(
                &mut resource.on_failure,
                &stored_failure,
                crate::api::workflows::WorkflowWriter::Agent,
            );
            crate::api::workflows::validate_workflow_for_import_keeping(&resource, &kept)
                .map_err(anyhow::Error::msg)?;
            let local = crate::db::workflows::list_workflows(conn)?;
            crate::api::workflows::validate_imported_sub_workflow_graph(&resource, &local)
                .map_err(anyhow::Error::msg)?;
            if existing_id.is_some() {
                crate::db::workflows::update_workflow(conn, &resource)?;
            } else {
                resource.created_at = now;
                crate::db::workflows::insert_workflow(conn, &resource)?;
            }
            crate::db::workflows::mark_auto_disabled(
                conn,
                &resource.id,
                crate::models::AutoDisableReason::Imported,
                "import",
                "imported from the project's resource library",
            )?;
            resource.id
        }
        ProjectRepositoryResourceKind::QuickPrompt => {
            let mut resource: QuickPrompt = serde::Deserialize::deserialize(&document.resource)?;
            resource.id = existing_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            resource.project_id = Some(project_id.to_string());
            resource.updated_at = now;
            if existing_id.is_some() {
                crate::db::quick_prompts::update_quick_prompt(conn, &resource)?;
            } else {
                resource.created_at = now;
                crate::db::quick_prompts::insert_quick_prompt(conn, &resource)?;
            }
            resource.id
        }
        ProjectRepositoryResourceKind::QuickApi => {
            let mut resource: QuickApi = serde::Deserialize::deserialize(&document.resource)?;
            resource.id = existing_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            resource.project_id = Some(project_id.to_string());
            resource.updated_at = now;
            crate::api::workflows::rebind_quick_api_config(conn, &mut resource, Some(project_id));
            if existing_id.is_some() {
                crate::db::quick_apis::update_quick_api(conn, &resource)?;
            } else {
                resource.created_at = now;
                crate::db::quick_apis::insert_quick_api(conn, &resource)?;
            }
            resource.id
        }
        ProjectRepositoryResourceKind::QuickExec => {
            let mut resource: QuickExec = serde::Deserialize::deserialize(&document.resource)?;
            resource.id = existing_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            resource.project_id = Some(project_id.to_string());
            resource.updated_at = now;
            // The file never carries an approval: only one this instance's
            // human gave to the very same line survives (KT-1017).
            let stored_line = match existing_id.as_deref() {
                Some(id) => crate::db::quick_execs::get_quick_exec(conn, id)?,
                None => None,
            }
            .filter(|stored| stored.command == resource.command && stored.args == resource.args);
            resource.unmodelled_args_approved = stored_line
                .as_ref()
                .and_then(|stored| stored.unmodelled_args_approved);
            resource.agent_written = match stored_line {
                Some(stored) => stored.agent_written,
                None => {
                    crate::api::quick_execs::needs_agent_approval(&resource.command, &resource.args)
                        .then_some(true)
                }
            };
            // Same inline-code rule as the Quick Exec form; a re-import of an
            // unchanged stored line stays possible (still refused at run time).
            let unchanged = match existing_id.as_deref() {
                Some(id) => {
                    crate::db::quick_execs::get_quick_exec(conn, id)?.is_some_and(|stored| {
                        stored.name == resource.name
                            && stored.command == resource.command
                            && stored.args == resource.args
                            && stored.unmodelled_args_approved == resource.unmodelled_args_approved
                    })
                }
                None => false,
            };
            if !unchanged {
                if let Some(error) = crate::core::inline_code::quick_exec_validation_error(
                    &resource.name,
                    &resource.command,
                    &resource.args,
                    crate::api::quick_execs::quick_exec_trust(&resource),
                ) {
                    anyhow::bail!(error);
                }
            }
            if existing_id.is_some() {
                crate::db::quick_execs::update_quick_exec(conn, &resource)?;
            } else {
                resource.created_at = now;
                crate::db::quick_execs::insert_quick_exec(conn, &resource)?;
            }
            resource.id
        }
        ProjectRepositoryResourceKind::Artifact => {
            imported_artifact(conn, document, project_id, existing_id.as_deref())?
        }
    };
    crate::db::resource_identities::upsert_at(
        conn,
        project_key,
        kind,
        &document.slug,
        &target_id,
        &document.updated_at.to_rfc3339(),
    )?;
    Ok(target_id)
}

fn publish_one(
    conn: &rusqlite::Connection,
    project_id: &str,
    request: PublishProjectRepositoryResourceRequest,
) -> anyhow::Result<ProjectRepositoryResourceMutation> {
    let project = crate::db::projects::get_project(conn, project_id)?
        .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
    let project_key = crate::db::resource_identities::project_key(conn, Some(project_id))?;
    let identity = crate::db::resource_identities::find_by_target(
        conn,
        &project_key,
        request.kind.identity_kind(),
        &request.id,
    )?;
    let slug = identity.map(|identity| identity.slug).unwrap_or_else(|| {
        if request.kind == ProjectRepositoryResourceKind::Skill {
            return crate::core::native_files::slug(&request.id);
        }
        let name = match request.kind {
            ProjectRepositoryResourceKind::Skill => {
                crate::core::skills::get_skill(&request.id).map(|skill| skill.name)
            }
            ProjectRepositoryResourceKind::Workflow => {
                crate::db::workflows::get_workflow(conn, &request.id)
                    .ok()
                    .flatten()
                    .map(|resource| resource.name)
            }
            ProjectRepositoryResourceKind::QuickPrompt => {
                crate::db::quick_prompts::get_quick_prompt(conn, &request.id)
                    .ok()
                    .flatten()
                    .map(|resource| resource.name)
            }
            ProjectRepositoryResourceKind::QuickApi => {
                crate::db::quick_apis::get_quick_api(conn, &request.id)
                    .ok()
                    .flatten()
                    .map(|resource| resource.name)
            }
            ProjectRepositoryResourceKind::QuickExec => {
                crate::db::quick_execs::get_quick_exec(conn, &request.id)
                    .ok()
                    .flatten()
                    .map(|resource| resource.name)
            }
            ProjectRepositoryResourceKind::Artifact => {
                crate::db::live_pages::get_live_page(conn, &request.id)
                    .ok()
                    .flatten()
                    .map(|resource| resource.page.title)
            }
        }
        .unwrap_or_else(|| request.id.clone());
        let slug = crate::core::repository_resources::ascii_slug(&name);
        if slug.is_empty() {
            format!(
                "resource-{}",
                request.id.chars().take(8).collect::<String>()
            )
        } else {
            slug
        }
    });
    let skill_updated_at = (request.kind == ProjectRepositoryResourceKind::Skill).then(Utc::now);
    let rendered =
        render_database_resource(conn, request.kind, &request.id, &slug, skill_updated_at)?;
    let entry = crate::core::repository_resources::publish(
        Path::new(&project.path),
        &project_key,
        rendered,
        request.overwrite_repository_changes,
    )
    .map_err(anyhow::Error::msg)?;
    let rendered =
        render_database_resource(conn, request.kind, &request.id, &slug, skill_updated_at)?;
    crate::db::resource_identities::upsert_at(
        conn,
        &project_key,
        request.kind.identity_kind(),
        &slug,
        &request.id,
        &rendered.document.updated_at.to_rfc3339(),
    )?;
    crate::db::repository_resources::upsert_alignment(
        conn,
        &project_key,
        request.kind.identity_kind(),
        &slug,
        &request.id,
        &entry.sha256,
        &rendered.hash,
        &rendered.document.updated_at.to_rfc3339(),
        false,
    )?;
    let alignment = crate::db::repository_resources::find_alignment(
        conn,
        &project_key,
        request.kind.identity_kind(),
        &slug,
    )?
    .ok_or_else(|| anyhow::anyhow!("Published resource alignment not found"))?;
    let approved = !alignment.imported
        || crate::db::repository_resources::is_approved(
            conn,
            &project_key,
            request.kind.identity_kind(),
            &slug,
            &crate::core::repository_resources::approval_hash(&rendered.document),
        )?;
    Ok(ProjectRepositoryResourceMutation {
        kind: request.kind,
        id: request.id,
        slug,
        status: ProjectRepositoryResourceStatus::UpToDate,
        approved,
    })
}

/// POST /api/projects/:id/repository-resources/publish
pub async fn publish_repository_resource(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Json(request): Json<PublishProjectRepositoryResourceRequest>,
) -> Json<ApiResponse<ProjectRepositoryResourceMutation>> {
    match state
        .db
        .with_conn(move |conn| publish_one(conn, &project_id, request))
        .await
    {
        Ok(result) => Json(ApiResponse::ok(result)),
        Err(error) => Json(ApiResponse::err(format!(
            "Unable to publish resource: {error}"
        ))),
    }
}

/// POST /api/projects/:id/repository-resources/import
pub async fn import_repository_resource(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Json(request): Json<ImportProjectRepositoryResourceRequest>,
) -> Json<ApiResponse<ProjectRepositoryResourceMutation>> {
    let result = state
        .db
        .with_conn(move |conn| {
            let project = crate::db::projects::get_project(conn, &project_id)?
                .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
            let project_key = crate::db::resource_identities::project_key(conn, Some(&project_id))?;
            let root = Path::new(&project.path);
            let lock = crate::core::repository_resources::load_lock(root)
                .map_err(anyhow::Error::msg)?
                .ok_or_else(|| anyhow::anyhow!("kronn/kronn.lock not found"))?;
            let entry = lock
                .resources
                .iter()
                .find(|entry| entry.kind == request.kind && entry.slug == request.slug)
                .ok_or_else(|| anyhow::anyhow!("Resource not found in kronn.lock"))?;
            let (document, repository_hash) =
                crate::core::repository_resources::read_resource(root, entry)
                    .map_err(anyhow::Error::msg)?;
            if !request.overwrite_kronn_changes
                && kronn_changes_would_be_lost(
                    conn,
                    &project_key,
                    &document,
                    &repository_hash,
                )?
            {
                anyhow::bail!(
                    "Kronn's copy of {}:{} has changes the repository does not; confirm to replace it with the repository version",
                    request.kind.identity_kind(),
                    request.slug
                );
            }
            let target_id = import_document(conn, &project_id, &project_key, &document)?;
            let rendered = render_database_resource(
                conn,
                request.kind,
                &target_id,
                &request.slug,
                Some(document.updated_at),
            )?;
            crate::db::repository_resources::upsert_alignment(
                conn,
                &project_key,
                request.kind.identity_kind(),
                &request.slug,
                &target_id,
                &repository_hash,
                &rendered.hash,
                &document.updated_at.to_rfc3339(),
                true,
            )?;
            Ok(ProjectRepositoryResourceMutation {
                kind: request.kind,
                id: target_id,
                slug: request.slug,
                status: ProjectRepositoryResourceStatus::UpToDate,
                approved: false,
            })
        })
        .await;
    match result {
        Ok(result) => Json(ApiResponse::ok(result)),
        Err(error) => Json(ApiResponse::err(format!(
            "Unable to import resource: {error}"
        ))),
    }
}

/// POST /api/projects/:id/repository-resources/approve
pub async fn approve_repository_resource(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Json(request): Json<ApproveProjectRepositoryResourceRequest>,
) -> Json<ApiResponse<ProjectRepositoryResourceMutation>> {
    let result = state
        .db
        .with_conn(move |conn| {
            let project_key = crate::db::resource_identities::project_key(conn, Some(&project_id))?;
            let alignment = crate::db::repository_resources::find_alignment_by_target(
                conn,
                request.kind.identity_kind(),
                &request.id,
            )?
            .filter(|alignment| alignment.project_key == project_key)
            .ok_or_else(|| anyhow::anyhow!("Imported resource alignment not found"))?;
            let timestamp = parse_identity_time(&alignment.aligned_at);
            let rendered = render_database_resource(
                conn,
                request.kind,
                &request.id,
                &alignment.slug,
                timestamp,
            )?;
            crate::db::repository_resources::approve(
                conn,
                &project_key,
                request.kind.identity_kind(),
                &alignment.slug,
                &crate::core::repository_resources::approval_hash(&rendered.document),
            )?;
            Ok(ProjectRepositoryResourceMutation {
                kind: request.kind,
                id: request.id,
                slug: alignment.slug,
                status: ProjectRepositoryResourceStatus::UpToDate,
                approved: true,
            })
        })
        .await;
    match result {
        Ok(result) => Json(ApiResponse::ok(result)),
        Err(error) => Json(ApiResponse::err(format!(
            "Unable to approve resource: {error}"
        ))),
    }
}

/// Identity kind recording which native folder a catalog skill was copied
/// from: re-copying then updates the same skill, and the listing keeps the
/// origin path even when the copy's name no longer matches its folder.
const NATIVE_SKILL_COPY_KIND: &str = "native_skill_copy";

fn native_copy_origins(
    conn: &rusqlite::Connection,
    project_key: &str,
    skill_ids: &[String],
) -> anyhow::Result<BTreeMap<String, String>> {
    let mut origins = BTreeMap::new();
    for id in skill_ids.iter().filter(|id| id.starts_with("custom-")) {
        if let Some(identity) = crate::db::resource_identities::find_by_target(
            conn,
            project_key,
            NATIVE_SKILL_COPY_KIND,
            id,
        )? {
            origins.insert(id.clone(), identity.slug);
        }
    }
    Ok(origins)
}

/// Whether `relative_path` is `<skill root>/<slug>/SKILL.md` for one of the
/// project's native skill roots — not `kronn/skills`, the former Kronn location
/// that is migrated rather than referenced, and not the router skill Kronn
/// itself writes under `.agents/skills/kronn`.
fn native_skill_relative_path_ok(relative_path: &str) -> bool {
    PROJECT_SKILL_ROOTS
        .iter()
        .filter(|root| **root != "kronn/skills")
        .any(|root| {
            relative_path
                .strip_prefix(&format!("{root}/"))
                .and_then(|rest| rest.strip_suffix("/SKILL.md"))
                .is_some_and(|slug| {
                    !slug.is_empty()
                        && !slug.contains('/')
                        && !(*root == ".agents/skills" && slug == "kronn")
                })
        })
}

/// Read a native skill file outside `kronn/` by its repository-relative
/// path, deriving its slug from the containing folder — the same convention
/// `discover_repository_skills` uses.
fn read_native_skill(root: &Path, relative_path: &str) -> anyhow::Result<(String, Skill)> {
    if !native_skill_relative_path_ok(relative_path) {
        anyhow::bail!("{relative_path} is not a native skill file outside kronn/");
    }
    crate::core::repository_resources::validate_relative(relative_path)
        .map_err(anyhow::Error::msg)?;
    let path = root.join(relative_path);
    crate::core::fs_guard::assert_contained_no_symlink(root, &path).map_err(anyhow::Error::msg)?;
    let slug = Path::new(relative_path)
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|slug| !slug.is_empty())
        .ok_or_else(|| anyhow::anyhow!("cannot derive a skill slug from {relative_path}"))?;
    let content = std::fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;
    let skill = crate::core::skills::parse_skill_markdown(&slug, &content, false)
        .ok_or_else(|| anyhow::anyhow!("{relative_path} has no valid Skill frontmatter"))?;
    Ok((slug, skill))
}

/// POST /api/projects/:id/repository-resources/skills/use
///
/// "Use in Kronn": reference the native skill by path, read-only and
/// tracked at the source — no `kronn.lock` entry, no catalog copy.
pub async fn use_native_skill(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Json(request): Json<RepositoryNativeSkillRequest>,
) -> Json<ApiResponse<ProjectRepositoryResourceMutation>> {
    let result = state
        .db
        .with_conn(move |conn| {
            let project = crate::db::projects::get_project(conn, &project_id)?
                .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
            let root = Path::new(&project.path);
            let (slug, skill) = read_native_skill(root, &request.relative_path)?;
            crate::db::project_skill_references::upsert(
                conn,
                &project_id,
                &slug,
                &request.relative_path,
                &skill.name,
                &Utc::now().to_rfc3339(),
            )?;
            Ok(ProjectRepositoryResourceMutation {
                kind: ProjectRepositoryResourceKind::Skill,
                id: format!("reference:{slug}"),
                slug,
                status: ProjectRepositoryResourceStatus::NativeSkill,
                approved: false,
            })
        })
        .await;
    match result {
        Ok(result) => Json(ApiResponse::ok(result)),
        Err(error) => Json(ApiResponse::err(format!(
            "Unable to use native skill: {error}"
        ))),
    }
}

/// POST /api/projects/:id/repository-resources/skills/copy
///
/// "Copy into Kronn": a managed copy. Creates a catalog skill from the native
/// file and attaches it to the project — a Kronn-side write only. The
/// repository is untouched: publishing the copy into `.agents/skills/` stays a
/// separate, previewed action. Copying again replaces an edited Kronn copy
/// only when the request says so.
pub async fn copy_native_skill(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Json(request): Json<RepositoryNativeSkillRequest>,
) -> Json<ApiResponse<ProjectRepositoryResourceMutation>> {
    let result = state
        .db
        .with_conn(move |conn| {
            let project = crate::db::projects::get_project(conn, &project_id)?
                .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
            let root = Path::new(&project.path);
            let (slug, skill) = read_native_skill(root, &request.relative_path)?;
            let project_key = crate::db::resource_identities::project_key(conn, Some(&project_id))?;
            let existing_id = crate::db::resource_identities::lookup(
                conn,
                &project_key,
                NATIVE_SKILL_COPY_KIND,
                &slug,
            )?
            .filter(|id| id.starts_with("custom-"));
            let existing = existing_id
                .as_deref()
                .and_then(crate::core::skills::get_skill);
            let target_id = match (existing_id, existing) {
                (Some(id), Some(current)) => {
                    let unchanged = current.name.trim() == skill.name.trim()
                        && current.description.trim() == skill.description.trim()
                        && current.content.trim() == skill.content.trim();
                    if !unchanged {
                        if !request.overwrite_kronn_changes {
                            anyhow::bail!(
                                "{id} is already the Kronn copy of {slug} and differs from {}; confirm to replace it with the repository version",
                                request.relative_path
                            );
                        }
                        crate::core::skills::update_custom_skill(
                            &id,
                            &skill.name,
                            &skill.description,
                            &skill.icon,
                            &skill.category,
                            &skill.content,
                            skill.license.as_deref(),
                            skill.allowed_tools.as_deref(),
                        )
                        .map_err(anyhow::Error::msg)?;
                    }
                    id
                }
                _ => crate::core::skills::save_custom_skill(
                    &skill.name,
                    &skill.description,
                    &skill.icon,
                    &skill.category,
                    &skill.content,
                    skill.license.as_deref(),
                    skill.allowed_tools.as_deref(),
                )
                .map_err(anyhow::Error::msg)?,
            };
            crate::db::resource_identities::upsert(
                conn,
                &project_key,
                NATIVE_SKILL_COPY_KIND,
                &slug,
                &target_id,
            )?;
            let mut skill_ids = project.default_skill_ids.clone();
            if !skill_ids.contains(&target_id) {
                skill_ids.push(target_id.clone());
                crate::db::projects::update_project_default_skills(conn, &project_id, &skill_ids)?;
            }
            Ok(ProjectRepositoryResourceMutation {
                kind: ProjectRepositoryResourceKind::Skill,
                slug: crate::core::native_files::slug(&target_id),
                id: target_id,
                status: ProjectRepositoryResourceStatus::KronnOnly,
                approved: false,
            })
        })
        .await;
    match result {
        Ok(result) => Json(ApiResponse::ok(result)),
        Err(error) => Json(ApiResponse::err(format!(
            "Unable to copy native skill into Kronn: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::skill_migration::{migrate_skills, skill_migration_plan};
    use super::*;

    #[test]
    fn skill_roots_cover_every_native_skill_folder_and_count_real_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let skill = |relative: &str| {
            let dir = tmp.path().join(relative);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), "---\nname: s\n---\n").unwrap();
        };
        skill(".agents/skills/review");
        skill(".agents/skills/kronn");
        skill(".github/skills/triage");
        skill(".opencode/skill/deploy");
        skill(".codex/skills/lint");
        std::fs::create_dir_all(tmp.path().join(".cursor/skills/draft")).unwrap();

        let roots = discover_skill_roots(tmp.path());
        let count = |path: &str| {
            roots
                .iter()
                .find(|root| root.path == path)
                .map(|root| root.skill_count)
        };
        assert_eq!(
            count(".agents/skills"),
            Some(1),
            "the kronn router skill is not a project skill"
        );
        assert_eq!(count(".github/skills"), Some(1));
        assert_eq!(count(".opencode/skill"), Some(1));
        assert_eq!(count(".codex/skills"), Some(1));
        assert_eq!(
            count(".cursor/skills"),
            Some(0),
            "a folder without SKILL.md holds no skill"
        );
        assert_eq!(
            count(".claude/skills"),
            None,
            "absent folders are not reported"
        );
        assert!(discover_repository_skills(tmp.path()).contains_key("triage"));
    }
    use chrono::TimeZone;

    #[test]
    fn hash_alignment_distinguishes_every_published_state_with_dates() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let root = tempfile::TempDir::new().unwrap();
        let timestamp = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let base = QuickExec {
            id: "qe-1".into(),
            name: "Lint".into(),
            icon: "terminal".into(),
            description: String::new(),
            project_id: Some("project-1".into()),
            command: "cargo".into(),
            args: vec!["check".into()],
            timeout_secs: 30,
            output_format: Default::default(),
            variables: Vec::new(),
            pinned: false,
            created_at: timestamp,
            updated_at: timestamp,
            unmodelled_args_approved: None,
            agent_written: None,
        };
        let rendered = crate::core::repository_resources::render_quick_exec(&base, "lint").unwrap();

        // Not yet published: repository-side has nothing, Kronn has the row.
        let unpublished_entry = crate::core::repository_resources::RepositoryLockResource {
            kind: crate::models::ProjectRepositoryResourceKind::QuickExec,
            slug: "lint".into(),
            name: "Lint".into(),
            level: String::new(),
            paths: vec!["kronn/quick-execs/lint.yaml".into()],
            sha256: String::new(),
            required_secrets: Vec::new(),
        };
        let unpublished =
            alignment_status(&conn, root.path(), &unpublished_entry, &rendered, None).unwrap();
        assert_eq!(
            unpublished.status,
            ProjectRepositoryResourceStatus::KronnOnly
        );
        assert_eq!(
            unpublished.kronn_updated_at,
            Some(rendered.document.updated_at)
        );

        let entry = crate::core::repository_resources::publish(
            root.path(),
            "repo",
            rendered.clone(),
            false,
        )
        .unwrap();
        crate::db::repository_resources::upsert_alignment(
            &conn,
            "repo",
            "quick_exec",
            "lint",
            "qe-1",
            &entry.sha256,
            &rendered.hash,
            &timestamp.to_rfc3339(),
            false,
        )
        .unwrap();
        let alignment =
            crate::db::repository_resources::find_alignment(&conn, "repo", "quick_exec", "lint")
                .unwrap()
                .unwrap();
        let up_to_date =
            alignment_status(&conn, root.path(), &entry, &rendered, Some(&alignment)).unwrap();
        assert_eq!(up_to_date.status, ProjectRepositoryResourceStatus::UpToDate);
        assert_eq!(
            up_to_date.aligned_at,
            parse_identity_time(&timestamp.to_rfc3339())
        );
        assert!(
            resource_comparison(root.path(), &entry, &rendered, up_to_date.sync_status)
                .diff
                .is_none(),
            "nothing to compare when both sides agree"
        );

        let path = root.path().join(&entry.paths[0]);
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"repository edit").unwrap();
        let repository_newer =
            alignment_status(&conn, root.path(), &entry, &rendered, Some(&alignment)).unwrap();
        assert_eq!(
            repository_newer.status,
            ProjectRepositoryResourceStatus::RepositoryNewer
        );
        assert!(
            resource_comparison(root.path(), &entry, &rendered, repository_newer.sync_status)
                .diff
                .unwrap()
                .contains("--- repository")
        );

        std::fs::write(&path, original).unwrap();
        let mut changed = base;
        changed.description = "database edit".into();
        let changed =
            crate::core::repository_resources::render_quick_exec(&changed, "lint").unwrap();
        let kronn_newer =
            alignment_status(&conn, root.path(), &entry, &changed, Some(&alignment)).unwrap();
        assert_eq!(
            kronn_newer.status,
            ProjectRepositoryResourceStatus::KronnNewer
        );
        assert!(
            resource_comparison(root.path(), &entry, &changed, kronn_newer.sync_status)
                .diff
                .is_some()
        );

        std::fs::write(&path, b"repository edit").unwrap();
        let view =
            alignment_status(&conn, root.path(), &entry, &changed, Some(&alignment)).unwrap();
        assert_eq!(view.status, ProjectRepositoryResourceStatus::Conflict);
        assert!(
            resource_comparison(root.path(), &entry, &changed, view.sync_status)
                .diff
                .unwrap()
                .contains("--- repository")
        );
    }

    #[test]
    fn imported_and_unapproved_reports_approval_required_over_the_sync_state() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let root = tempfile::TempDir::new().unwrap();
        let timestamp = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let exec = QuickExec {
            id: "qe-1".into(),
            name: "Lint".into(),
            icon: "terminal".into(),
            description: String::new(),
            project_id: Some("project-1".into()),
            command: "cargo".into(),
            args: vec!["check".into()],
            timeout_secs: 30,
            output_format: Default::default(),
            variables: Vec::new(),
            pinned: false,
            created_at: timestamp,
            updated_at: timestamp,
            unmodelled_args_approved: None,
            agent_written: None,
        };
        let rendered = crate::core::repository_resources::render_quick_exec(&exec, "lint").unwrap();
        let entry = crate::core::repository_resources::publish(
            root.path(),
            "repo",
            rendered.clone(),
            false,
        )
        .unwrap();
        crate::db::repository_resources::upsert_alignment(
            &conn,
            "repo",
            "quick_exec",
            "lint",
            "qe-1",
            &entry.sha256,
            &rendered.hash,
            &timestamp.to_rfc3339(),
            true,
        )
        .unwrap();
        let alignment =
            crate::db::repository_resources::find_alignment(&conn, "repo", "quick_exec", "lint")
                .unwrap()
                .unwrap();
        let view =
            alignment_status(&conn, root.path(), &entry, &rendered, Some(&alignment)).unwrap();
        assert_eq!(
            view.status,
            ProjectRepositoryResourceStatus::ApprovalRequired
        );
        assert!(view.approval_required);
        assert!(!view.approved);
    }

    fn mcp_config(
        id: &str,
        keys: &[&str],
        global: bool,
        projects: &[&str],
    ) -> crate::models::McpConfig {
        crate::models::McpConfig {
            id: id.into(),
            server_id: "srv".into(),
            label: id.into(),
            env_keys: keys.iter().map(|key| key.to_string()).collect(),
            env_encrypted: "enc".into(),
            args_override: None,
            is_global: global,
            include_general: true,
            config_hash: format!("hash-{id}"),
            project_ids: projects.iter().map(|project| project.to_string()).collect(),
            host_sync: crate::models::HostSyncMode::None,
        }
    }

    #[test]
    fn configured_secrets_are_the_key_names_of_the_configs_a_project_can_use() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let root = tempfile::TempDir::new().unwrap();
        for id in ["p1", "p2"] {
            crate::db::projects::insert_project(&conn, &mk_project(id, &root.path().join(id)))
                .unwrap();
        }
        crate::db::mcps::upsert_server(
            &conn,
            &crate::models::McpServer {
                id: "srv".into(),
                name: "Server".into(),
                description: String::new(),
                transport: crate::models::McpTransport::Sse {
                    url: "http://localhost".into(),
                },
                source: crate::models::McpSource::Manual,
                api_spec: None,
            },
        )
        .unwrap();
        for config in [
            mcp_config("own", &["FASTLY_TOKEN"], false, &["p1"]),
            mcp_config("global", &["GLOBAL_KEY"], true, &[]),
            mcp_config("elsewhere", &["OTHER_SECRET"], false, &["p2"]),
        ] {
            crate::db::mcps::insert_config(&conn, &config).unwrap();
        }

        let configured =
            crate::core::repository_resources::configured_secret_names(&conn, "p1").unwrap();
        assert_eq!(
            configured.into_iter().collect::<Vec<_>>(),
            vec!["FASTLY_TOKEN".to_string(), "GLOBAL_KEY".to_string()],
            "another project's config must not count as configured here"
        );
    }

    #[test]
    fn never_aligned_resource_with_a_repository_file_is_in_sync_or_two_versions() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let root = tempfile::TempDir::new().unwrap();
        let timestamp = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let exec = QuickExec {
            id: "qe-1".into(),
            name: "Lint".into(),
            icon: "terminal".into(),
            description: String::new(),
            project_id: Some("project-1".into()),
            command: "cargo".into(),
            args: vec!["check".into()],
            timeout_secs: 30,
            output_format: Default::default(),
            variables: Vec::new(),
            pinned: false,
            created_at: timestamp,
            updated_at: timestamp,
            unmodelled_args_approved: None,
            agent_written: None,
        };
        let rendered = crate::core::repository_resources::render_quick_exec(&exec, "lint").unwrap();
        let entry = crate::core::repository_resources::publish(
            root.path(),
            "repo",
            rendered.clone(),
            false,
        )
        .unwrap();

        let same = alignment_status(&conn, root.path(), &entry, &rendered, None).unwrap();
        assert_eq!(same.status, ProjectRepositoryResourceStatus::UpToDate);

        let mut edited = exec;
        edited.description = "edited in Kronn".into();
        let edited = crate::core::repository_resources::render_quick_exec(&edited, "lint").unwrap();
        let both = alignment_status(&conn, root.path(), &entry, &edited, None).unwrap();
        assert_eq!(
            both.status,
            ProjectRepositoryResourceStatus::Conflict,
            "with no baseline neither side can be called newer"
        );
        let comparison = resource_comparison(root.path(), &entry, &edited, both.sync_status);
        assert!(comparison.diff.is_some());
        assert!(
            comparison
                .field_diff
                .iter()
                .any(|item| item.field == "description"),
            "{:?}",
            comparison.field_diff
        );
        assert!(
            comparison
                .field_diff
                .iter()
                .all(|item| item.field != "updated_at"),
            "instance-local fields are not a difference"
        );
    }

    #[test]
    fn skills_never_wait_for_approval_even_when_imported() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let root = tempfile::TempDir::new().unwrap();
        let timestamp = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let skill = crate::core::skills::parse_skill_markdown(
            "review",
            "---\nname: Review\ndescription: Review the diff.\n---\nReview carefully.\n",
            false,
        )
        .unwrap();
        let rendered =
            crate::core::repository_resources::render_skill(&skill, timestamp, "review").unwrap();
        let entry = crate::core::repository_resources::publish(
            root.path(),
            "repo",
            rendered.clone(),
            false,
        )
        .unwrap();
        crate::db::repository_resources::upsert_alignment(
            &conn,
            "repo",
            "skill",
            "review",
            "review",
            &entry.sha256,
            &rendered.hash,
            &timestamp.to_rfc3339(),
            true,
        )
        .unwrap();
        let alignment =
            crate::db::repository_resources::find_alignment(&conn, "repo", "skill", "review")
                .unwrap()
                .unwrap();
        let view =
            alignment_status(&conn, root.path(), &entry, &rendered, Some(&alignment)).unwrap();
        assert_eq!(view.status, ProjectRepositoryResourceStatus::UpToDate);
        assert!(!view.approval_required);
    }

    #[test]
    fn file_diffs_give_each_file_of_a_resource_its_own_diff() {
        let root = tempfile::TempDir::new().unwrap();
        let dir = root.path().join("kronn/artifacts/health");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("artifact.yaml"), "same\n").unwrap();
        std::fs::write(dir.join("index.html"), "<p>old</p>\n").unwrap();
        let timestamp = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let exec = QuickExec {
            id: "qe-1".into(),
            name: "Lint".into(),
            icon: "terminal".into(),
            description: String::new(),
            project_id: None,
            command: "cargo".into(),
            args: Vec::new(),
            timeout_secs: 30,
            output_format: Default::default(),
            variables: Vec::new(),
            pinned: false,
            created_at: timestamp,
            updated_at: timestamp,
            unmodelled_args_approved: None,
            agent_written: None,
        };
        let mut rendered =
            crate::core::repository_resources::render_quick_exec(&exec, "health").unwrap();
        rendered.files = std::sync::Arc::new(BTreeMap::from([
            (
                "kronn/artifacts/health/artifact.yaml".to_string(),
                b"same\n".to_vec(),
            ),
            (
                "kronn/artifacts/health/index.html".to_string(),
                b"<p>new</p>\n".to_vec(),
            ),
        ]));
        let entry = crate::core::repository_resources::RepositoryLockResource {
            kind: ProjectRepositoryResourceKind::Artifact,
            slug: "health".into(),
            name: "Health".into(),
            level: String::new(),
            paths: rendered.files.keys().cloned().collect(),
            sha256: String::new(),
            required_secrets: Vec::new(),
        };

        let diffs = resource_file_diffs(root.path(), &entry, &rendered);
        assert_eq!(diffs.len(), 1, "the unchanged file has no diff: {diffs:?}");
        assert_eq!(diffs[0].path, "kronn/artifacts/health/index.html");
        assert!(diffs[0].diff.contains("-<p>old</p>"));
        assert!(diffs[0].diff.contains("+<p>new</p>"));
        assert_eq!(
            primary_diff_path(&entry).unwrap(),
            "kronn/artifacts/health/index.html"
        );
    }

    /// `save_custom_skill`/`update_custom_skill` write under `config_dir()`,
    /// which resolves to the real user config unless overridden: point
    /// `KRONN_DATA_DIR` at a scratch dir before any test that copies a native
    /// skill into the catalog.
    fn isolate_config_dir() {
        let dir =
            std::env::temp_dir().join(format!("kronn-resources-test-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        crate::core::child_env::set_var("KRONN_DATA_DIR", &dir);
    }

    fn test_state() -> crate::AppState {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    fn mk_project(id: &str, path: &Path) -> crate::models::Project {
        let now = Utc::now();
        crate::models::Project {
            id: id.into(),
            name: "test".into(),
            path: path.display().to_string(),
            repo_url: None,
            token_override: None,
            ai_config: crate::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: Default::default(),
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            write_access: None,
            mcp_sync_report: None,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: vec![],
            workspace: None,
            created_at: now,
            updated_at: now,
        }
    }

    async fn seed_project(state: &crate::AppState, project: crate::models::Project) {
        state
            .db
            .with_conn(move |conn| {
                crate::db::projects::insert_project(conn, &project)?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .expect("insert project");
    }

    fn write_native_skill(root: &Path, relative_dir: &str, folder: &str, name: &str, body: &str) {
        let dir = root.join(relative_dir).join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Review the diff.\n---\n{body}\n"),
        )
        .unwrap();
    }

    fn native_request(relative_path: &str, overwrite: bool) -> Json<RepositoryNativeSkillRequest> {
        Json(RepositoryNativeSkillRequest {
            relative_path: relative_path.into(),
            overwrite_kronn_changes: overwrite,
        })
    }

    async fn list_resources(
        state: &crate::AppState,
        project_id: &str,
    ) -> crate::models::ProjectRepositoryResources {
        repository_resources(State(state.clone()), AxumPath(project_id.to_string()))
            .await
            .0
            .data
            .expect("listing succeeds")
    }

    #[tokio::test]
    async fn use_native_skill_attaches_by_reference_and_flags_diverging_copies() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        write_native_skill(
            root.path(),
            ".claude/skills",
            "review",
            "Review",
            "Version A.",
        );
        write_native_skill(
            root.path(),
            ".agents/skills",
            "review",
            "Review",
            "Version B.",
        );
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;

        let response = use_native_skill(
            State(state.clone()),
            AxumPath(project_id.clone()),
            native_request(".claude/skills/review/SKILL.md", false),
        )
        .await;
        let mutation = response.0.data.expect("use_native_skill succeeds");
        assert_eq!(
            mutation.status,
            ProjectRepositoryResourceStatus::NativeSkill
        );
        assert!(
            !root.path().join("kronn").exists(),
            "a read-only reference must not create kronn/ or kronn.lock"
        );

        let reference = state
            .db
            .with_read_conn({
                let project_id = project_id.clone();
                move |conn| crate::db::project_skill_references::find(conn, &project_id, "review")
            })
            .await
            .unwrap()
            .expect("the reference row must be recorded");
        assert_eq!(reference.relative_path, ".claude/skills/review/SKILL.md");

        let listing = list_resources(&state, &project_id).await;
        let skill = listing
            .skills_present
            .iter()
            .find(|skill| skill.slug == "review")
            .expect("the referenced skill is listed");
        assert!(skill.referenced);
        assert_eq!(skill.status, ProjectRepositoryResourceStatus::NativeSkill);
        assert_eq!(
            skill.repository_paths,
            vec![
                ".agents/skills/review/SKILL.md".to_string(),
                ".claude/skills/review/SKILL.md".to_string()
            ],
            "every origin path stays exposed, grouped under the one slug"
        );
        assert!(skill.repository_paths_diverge);
        assert!(skill.repository_updated_at.is_some());
        assert!(
            skill.write_preview.is_empty(),
            "a native skill has no publish to preview"
        );
    }

    #[tokio::test]
    async fn use_native_skill_refuses_paths_that_are_not_a_native_skill_file() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        write_native_skill(root.path(), "kronn/skills", "review", "Review", "Body.");
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;

        for path in [
            "kronn/skills/review/SKILL.md",
            ".claude/skills/../../outside/SKILL.md",
            ".claude/skills/review/NOTES.md",
            ".claude/skills/SKILL.md",
            ".claude/skills/review/nested/SKILL.md",
            ".agents/skills/kronn/SKILL.md",
        ] {
            let response = use_native_skill(
                State(state.clone()),
                AxumPath(project_id.clone()),
                native_request(path, false),
            )
            .await;
            assert!(response.0.data.is_none(), "{path} must be refused");
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn copy_native_skill_stays_kronn_side_and_keeps_the_origin_path() {
        isolate_config_dir();
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        // The folder name differs from the skill's name on purpose: the copy's
        // id is derived from the name, so only the recorded origin ties it back.
        write_native_skill(
            root.path(),
            ".claude/skills",
            "cr",
            "Copied Review Alpha",
            "Original.",
        );
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;

        let response = copy_native_skill(
            State(state.clone()),
            AxumPath(project_id.clone()),
            native_request(".claude/skills/cr/SKILL.md", false),
        )
        .await;
        let mutation = response.0.data.expect("copy_native_skill succeeds");
        assert_eq!(mutation.status, ProjectRepositoryResourceStatus::KronnOnly);
        assert!(
            !root.path().join("kronn").exists(),
            "copying into Kronn must not write the repository"
        );

        let listing = list_resources(&state, &project_id).await;
        assert!(
            !listing
                .skills_present
                .iter()
                .any(|skill| skill.id == "repository:cr"),
            "the native folder is accounted for by its copy, not listed twice"
        );
        let skill = listing
            .skills_present
            .iter()
            .find(|skill| skill.id == mutation.id)
            .expect("the copied skill is listed");
        assert_eq!(skill.status, ProjectRepositoryResourceStatus::KronnOnly);
        assert_eq!(
            skill.repository_paths,
            vec![".claude/skills/cr/SKILL.md".to_string()]
        );
        assert!(skill.write_preview.contains(&skill.publication_path));
        assert!(skill.write_preview.contains(&"docs/AGENTS.md".to_string()));
        assert!(
            skill.kronn_updated_at.is_some(),
            "a custom skill's file date is its Kronn date"
        );

        // Unchanged copy: copying again is a no-op, not an error.
        let again = copy_native_skill(
            State(state.clone()),
            AxumPath(project_id.clone()),
            native_request(".claude/skills/cr/SKILL.md", false),
        )
        .await;
        assert_eq!(again.0.data.expect("idempotent").id, mutation.id);

        // Edited Kronn copy: never overwritten without confirmation.
        crate::core::skills::update_custom_skill(
            &mutation.id,
            "Copied Review Alpha",
            "Review the diff.",
            "",
            &crate::models::SkillCategory::Domain,
            "Edited in Kronn.",
            None,
            None,
        )
        .unwrap();
        let refused = copy_native_skill(
            State(state.clone()),
            AxumPath(project_id.clone()),
            native_request(".claude/skills/cr/SKILL.md", false),
        )
        .await;
        let message = refused.0.error.expect("an edited copy is not overwritten");
        assert!(message.contains("differs"), "{message}");
        assert_eq!(
            crate::core::skills::get_skill(&mutation.id)
                .unwrap()
                .content
                .trim(),
            "Edited in Kronn."
        );

        let confirmed = copy_native_skill(
            State(state.clone()),
            AxumPath(project_id.clone()),
            native_request(".claude/skills/cr/SKILL.md", true),
        )
        .await;
        assert_eq!(
            confirmed.0.data.expect("confirmed overwrite").id,
            mutation.id
        );
        assert_eq!(
            crate::core::skills::get_skill(&mutation.id)
                .unwrap()
                .content
                .trim(),
            "Original."
        );
    }

    fn sample_exec(project_id: &str) -> QuickExec {
        let timestamp = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        QuickExec {
            id: "qe-1".into(),
            name: "Lint".into(),
            icon: "terminal".into(),
            description: String::new(),
            project_id: Some(project_id.into()),
            command: "cargo".into(),
            args: vec!["check".into()],
            timeout_secs: 30,
            output_format: Default::default(),
            variables: Vec::new(),
            pinned: false,
            created_at: timestamp,
            updated_at: timestamp,
            unmodelled_args_approved: None,
            agent_written: None,
        }
    }

    fn sample_prompt_json(id: &str, name: &str, project_id: &str) -> QuickPrompt {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "icon": "zap", "prompt_template": "Review {{diff}}",
            "variables": [], "agent": "ClaudeCode", "project_id": project_id,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    fn sample_workflow_json(
        id: &str,
        name: &str,
        project_id: &str,
        steps: serde_json::Value,
    ) -> Workflow {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "project_id": project_id,
            "trigger": {"type": "Manual"}, "steps": steps, "actions": [],
            "safety": {}, "workspace_config": null, "concurrency_limit": null,
            "enabled": false,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    fn workflow_document(
        slug: &str,
        steps: serde_json::Value,
    ) -> crate::core::repository_resources::RepositoryDocument {
        crate::core::repository_resources::RepositoryDocument {
            schema_version: 1,
            kind: ProjectRepositoryResourceKind::Workflow,
            slug: slug.into(),
            updated_at: Utc::now(),
            requires: vec![],
            resource: serde_json::to_value(sample_workflow_json(
                "foreign-id",
                slug,
                "foreign-project",
                steps,
            ))
            .unwrap(),
            redacted_fields: vec![],
        }
    }

    /// KT-1017 — a `kronn/` import refuses an unsafe inline line in a
    /// workflow or a Quick Exec, and keeps one only when re-importing over
    /// the exact same stored line.
    #[tokio::test]
    async fn a_kronn_import_refuses_unsafe_inline_lines_unless_unchanged() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        seed_project(&state, mk_project("project-1", root.path())).await;
        let unsafe_steps = serde_json::json!([
            {"name": "greet", "step_type": {"type": "Exec"}, "exec_command": "bash",
             "exec_args": ["-c", "echo \"{{issue.title}}\""]}
        ]);
        let workflow_doc = |slug: &str, steps: serde_json::Value| {
            let mut document = workflow_document(slug, steps);
            document.resource["exec_allowlist"] = serde_json::json!(["bash"]);
            document
        };
        let exec_doc =
            |args: serde_json::Value| crate::core::repository_resources::RepositoryDocument {
                schema_version: 1,
                kind: ProjectRepositoryResourceKind::QuickExec,
                slug: "ticket-exec".into(),
                updated_at: Utc::now(),
                requires: vec![],
                resource: {
                    let mut exec = serde_json::to_value(sample_exec("foreign-project")).unwrap();
                    exec["command"] = serde_json::json!("python3");
                    exec["args"] = args;
                    exec
                },
                redacted_fields: vec![],
            };
        let outcome = state
            .db
            .with_conn(move |conn| {
                let key = crate::db::resource_identities::project_key(conn, Some("project-1"))?;
                let workflow = import_document(
                    conn,
                    "project-1",
                    &key,
                    &workflow_doc("unsafe", unsafe_steps.clone()),
                )
                .map_err(|e| e.to_string());
                let exec = import_document(
                    conn,
                    "project-1",
                    &key,
                    &exec_doc(serde_json::json!(["-c", "print('{{ticket}}')"])),
                )
                .map_err(|e| e.to_string());
                // A stored legacy workflow mapped to the same slug: re-importing
                // the identical line is kept, a changed placeholder is not.
                let mut stored =
                    sample_workflow_json("wf-legacy", "legacy", "project-1", unsafe_steps.clone());
                stored.exec_allowlist = vec!["bash".into()];
                crate::db::workflows::insert_workflow(conn, &stored)?;
                crate::db::resource_identities::upsert(
                    conn,
                    &key,
                    ProjectRepositoryResourceKind::Workflow.identity_kind(),
                    "legacy",
                    "wf-legacy",
                )?;
                let same = import_document(
                    conn,
                    "project-1",
                    &key,
                    &workflow_doc("legacy", unsafe_steps.clone()),
                )
                .map_err(|e| e.to_string());
                let changed = import_document(
                    conn,
                    "project-1",
                    &key,
                    &workflow_doc(
                        "legacy",
                        serde_json::json!([
                            {"name": "greet", "step_type": {"type": "Exec"}, "exec_command": "bash",
                             "exec_args": ["-c", "echo \"{{issue.body}}\""]}
                        ]),
                    ),
                )
                .map_err(|e| e.to_string());
                Ok::<_, anyhow::Error>((workflow, exec, same, changed))
            })
            .await
            .unwrap();
        let (workflow, exec, same, changed) = outcome;
        assert!(workflow.unwrap_err().contains("script inline"));
        assert!(exec.unwrap_err().contains("{{ticket}}"));
        same.expect("an unchanged stored line stays importable");
        assert!(changed.unwrap_err().contains("script inline"));
    }

    /// R6-03: a `kronn/` file is something an agent can write, so a new or
    /// changed line it brings waits for a human in the editor.
    #[tokio::test]
    async fn a_kronn_import_marks_new_lines_as_an_agent_s() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        seed_project(&state, mk_project("project-1", root.path())).await;
        let shape = serde_json::json!([
            {"name": "plan", "step_type": {"type": "Exec"}, "exec_command": "bash",
             "exec_args": ["-c", "terraform plan \"$1\"", "_", "{{issue.title}}"],
             "exec_unmodelled_args_approved": true}
        ]);
        let mut document = workflow_document("laundered", shape);
        document.resource["exec_allowlist"] = serde_json::json!(["bash"]);
        let exec = crate::core::repository_resources::RepositoryDocument {
            schema_version: 1,
            kind: ProjectRepositoryResourceKind::QuickExec,
            slug: "shape-exec".into(),
            updated_at: Utc::now(),
            requires: vec![],
            resource: {
                let mut exec = serde_json::to_value(sample_exec("foreign-project")).unwrap();
                exec["command"] = serde_json::json!("python3");
                exec["args"] = serde_json::json!(["-c", "import sys; print(sys.argv[1])", "{{x}}"]);
                exec
            },
            redacted_fields: vec![],
        };
        let (workflow, quick_exec) = state
            .db
            .with_conn(move |conn| {
                let key = crate::db::resource_identities::project_key(conn, Some("project-1"))?;
                let workflow_id = import_document(conn, "project-1", &key, &document)?;
                let exec_id = import_document(conn, "project-1", &key, &exec)?;
                Ok::<_, anyhow::Error>((
                    crate::db::workflows::get_workflow(conn, &workflow_id)?.unwrap(),
                    crate::db::quick_execs::get_quick_exec(conn, &exec_id)?.unwrap(),
                ))
            })
            .await
            .unwrap();
        let step = &workflow.steps[0];
        assert_eq!(step.exec_unmodelled_args_approved, None);
        assert_eq!(step.exec_agent_lines, vec!["main".to_string()]);
        assert!(crate::core::inline_code::runtime_refusal(step).is_some());
        assert_eq!(quick_exec.agent_written, Some(true));
    }

    /// F-04: a `kronn/` file never brings a script hash this instance did
    /// not store.
    #[tokio::test]
    async fn a_kronn_import_blanks_a_carried_script_hash() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        seed_project(&state, mk_project("project-1", root.path())).await;
        let mut document = workflow_document(
            "pinned",
            serde_json::json!([
                {"name": "run", "step_type": {"type": "Exec"}, "exec_command": "python3",
                 "exec_args": ["tool.py"],
                 "exec_script_files": [{"path": "tool.py", "sha256": "a".repeat(64)}]}
            ]),
        );
        document.resource["exec_allowlist"] = serde_json::json!(["python3"]);
        let workflow = state
            .db
            .with_conn(move |conn| {
                let key = crate::db::resource_identities::project_key(conn, Some("project-1"))?;
                let id = import_document(conn, "project-1", &key, &document)?;
                Ok::<_, anyhow::Error>(crate::db::workflows::get_workflow(conn, &id)?.unwrap())
            })
            .await
            .unwrap();
        assert_eq!(workflow.steps[0].exec_script_files[0].sha256, "");
    }

    #[tokio::test]
    async fn a_kronn_import_runs_the_editor_save_rules() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        seed_project(&state, mk_project("project-1", root.path())).await;
        let outcome = state
            .db
            .with_conn(|conn| {
                let key = crate::db::resource_identities::project_key(conn, Some("project-1"))?;
                crate::db::workflows::insert_workflow(
                    conn,
                    &sample_workflow_json(
                        "wf-local-gated",
                        "Gated",
                        "project-1",
                        serde_json::json!([
                            {"name": "review", "step_type": {"type": "Gate"}, "gate_message": "Go?"}
                        ]),
                    ),
                )?;
                let exec_outside_allowlist = import_document(
                    conn,
                    "project-1",
                    &key,
                    &workflow_document(
                        "exec-outside-allowlist",
                        serde_json::json!([
                            {"name": "wipe", "step_type": {"type": "Exec"}, "exec_command": "rm", "exec_args": ["-rf", "."]}
                        ]),
                    ),
                )
                .map(|_| ())
                .map_err(|e| e.to_string());
                let calls_gated_child = import_document(
                    conn,
                    "project-1",
                    &key,
                    &workflow_document(
                        "calls-gated-child",
                        serde_json::json!([
                            {"name": "call", "step_type": {"type": "SubWorkflow"}, "sub_workflow_id": "wf-local-gated"}
                        ]),
                    ),
                )
                .map(|_| ())
                .map_err(|e| e.to_string());
                let valid = import_document(
                    conn,
                    "project-1",
                    &key,
                    &workflow_document(
                        "valid-élan",
                        serde_json::json!([
                            {"name": "data", "step_type": {"type": "JsonData"}, "json_data_payload": {"ok": true}},
                            {"name": "call", "step_type": {"type": "SubWorkflow"}, "sub_workflow_id": "id-on-another-machine"}
                        ]),
                    ),
                )
                .map_err(|e| e.to_string());
                let stored = crate::db::workflows::list_workflows(conn)?.len();
                Ok::<_, anyhow::Error>((exec_outside_allowlist, calls_gated_child, valid, stored))
            })
            .await
            .unwrap();
        let (exec_outside_allowlist, calls_gated_child, valid, stored) = outcome;
        assert!(
            exec_outside_allowlist.is_err(),
            "{exec_outside_allowlist:?}"
        );
        let gated = calls_gated_child.unwrap_err();
        assert!(gated.contains("Gate"), "{gated}");
        valid.expect("a valid definition still imports; an unknown child id is left to KT-917");
        assert_eq!(
            stored, 2,
            "only the local child and the valid import are stored"
        );
    }

    #[tokio::test]
    async fn listing_exposes_uses_and_used_by_including_missing_references_and_loops() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::quick_execs::insert_quick_exec(conn, &sample_exec("project-1"))?;
                crate::db::quick_prompts::insert_quick_prompt(
                    conn,
                    &sample_prompt_json("qp-1", "Review", "project-1"),
                )?;
                crate::db::workflows::insert_workflow(
                    conn,
                    &sample_workflow_json(
                        "wf-1",
                        "Nightly triage",
                        "project-1",
                        serde_json::json!([
                            {"name": "ask", "step_type": {"type": "Agent"}, "quick_prompt_id": "qp-1"},
                            {"name": "lint", "step_type": {"type": "CollectApiData"},
                             "collect_api_data": {"sources": [{"alias": "lint", "quick_exec_id": "qe-1"}]}},
                            {"name": "gone", "step_type": {"type": "Agent"}, "quick_prompt_id": "qp-gone"},
                            {"name": "child", "step_type": {"type": "SubWorkflow"}, "sub_workflow_id": "wf-2"}
                        ]),
                    ),
                )?;
                crate::db::workflows::insert_workflow(
                    conn,
                    &sample_workflow_json(
                        "wf-2",
                        "Child",
                        "project-1",
                        serde_json::json!([
                            {"name": "back", "step_type": {"type": "SubWorkflow"}, "sub_workflow_id": "wf-1"}
                        ]),
                    ),
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();

        let listing = list_resources(&state, &project_id).await;
        let find = |id: &str| listing.resources.iter().find(|item| item.id == id).unwrap();

        let nightly = find("wf-1");
        let uses: Vec<(&str, bool)> = nightly
            .uses
            .iter()
            .map(|link| (link.id.as_str(), link.missing))
            .collect();
        assert_eq!(
            uses,
            vec![
                ("wf-2", false),
                ("qe-1", false),
                ("qp-1", false),
                ("qp-gone", true)
            ],
            "present links by name, then the missing one"
        );
        let child = &nightly.uses[0];
        assert_eq!(child.kind, ProjectRepositoryResourceKind::Workflow);
        assert_eq!(child.slug.as_deref(), Some("child"));
        assert_eq!(child.name, "Child");
        let gone = nightly.uses.last().unwrap();
        assert_eq!(gone.kind, ProjectRepositoryResourceKind::QuickPrompt);
        assert!(gone.slug.is_none(), "nothing to derive a stable slug from");

        for leaf in ["qp-1", "qe-1"] {
            assert!(find(leaf).uses.is_empty());
            let used_by: Vec<&str> = find(leaf)
                .used_by
                .iter()
                .map(|link| link.id.as_str())
                .collect();
            assert_eq!(used_by, vec!["wf-1"]);
            assert_eq!(
                find(leaf).used_by[0].slug.as_deref(),
                Some("nightly-triage")
            );
        }
        // The loop is visible from both sides and neither end vanishes.
        let child = find("wf-2");
        assert_eq!(child.uses.len(), 1);
        assert_eq!(child.uses[0].id, "wf-1");
        assert_eq!(child.used_by.len(), 1);
        assert_eq!(child.used_by[0].id, "wf-1");
        assert_eq!(nightly.used_by.len(), 1);
        assert_eq!(nightly.used_by[0].id, "wf-2");
    }

    #[tokio::test]
    async fn listing_resolves_definitions_written_on_another_machine_through_their_files() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        // Published elsewhere: the workflow file names the Quick Prompt by the
        // id it had on that machine, and nothing of it is in Kronn's database.
        let prompt = sample_prompt_json("foreign-qp", "Review", "elsewhere");
        let workflow = sample_workflow_json(
            "foreign-wf",
            "Nightly triage",
            "elsewhere",
            serde_json::json!([
                {"name": "ask", "step_type": {"type": "Agent"}, "quick_prompt_id": "foreign-qp"},
                {"name": "lost", "step_type": {"type": "Agent"}, "quick_prompt_id": "never-published"}
            ]),
        );
        for rendered in [
            crate::core::repository_resources::render_quick_prompt(&prompt, "review").unwrap(),
            crate::core::repository_resources::render_workflow(&workflow, "nightly-triage")
                .unwrap(),
        ] {
            crate::core::repository_resources::publish(root.path(), "repo", rendered, false)
                .unwrap();
        }
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;

        let listing = list_resources(&state, &project_id).await;
        let find = |slug: &str| {
            listing
                .resources
                .iter()
                .find(|item| item.slug == slug)
                .unwrap()
        };
        assert_eq!(
            find("nightly-triage").status,
            ProjectRepositoryResourceStatus::RepositoryOnly
        );
        let uses = &find("nightly-triage").uses;
        assert_eq!(uses.len(), 2);
        assert_eq!(uses[0].id, "repository:quick_prompt:review");
        assert_eq!(uses[0].slug.as_deref(), Some("review"));
        assert!(!uses[0].missing);
        assert_eq!(uses[1].id, "never-published");
        assert!(uses[1].missing);
        assert_eq!(find("review").used_by.len(), 1);
        assert_eq!(
            find("review").used_by[0].slug.as_deref(),
            Some("nightly-triage")
        );
    }

    #[tokio::test]
    async fn listing_previews_every_touched_path_then_reports_dates_and_uncommitted_files() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::quick_execs::insert_quick_exec(conn, &sample_exec("project-1"))?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();

        let listing = list_resources(&state, &project_id).await;
        assert!(listing.can_write_repository);
        assert!(listing.uncommitted_managed_paths.is_empty());
        let before = listing
            .resources
            .iter()
            .find(|item| item.id == "qe-1")
            .unwrap();
        assert_eq!(before.status, ProjectRepositoryResourceStatus::KronnOnly);
        assert_eq!(before.adr_level, ResourceAdrLevel::N1);
        assert_eq!(
            before.repository_paths,
            vec!["kronn/quick-execs/lint.yaml".to_string()]
        );
        for path in [
            "kronn/quick-execs/lint.yaml",
            "kronn/INDEX.md",
            "kronn/kronn.toml",
            ".agents/skills/kronn/SKILL.md",
            "docs/AGENTS.md",
        ] {
            assert!(
                before.write_preview.contains(&path.to_string()),
                "{path} missing from {:?}",
                before.write_preview
            );
        }
        assert!(before.repository_updated_at.is_none());
        assert!(before.kronn_updated_at.is_some());
        assert!(before.aligned_at.is_none());

        let published = publish_repository_resource(
            State(state.clone()),
            AxumPath(project_id.clone()),
            Json(PublishProjectRepositoryResourceRequest {
                kind: ProjectRepositoryResourceKind::QuickExec,
                id: "qe-1".into(),
                overwrite_repository_changes: false,
            }),
        )
        .await;
        assert!(published.0.data.is_some(), "{:?}", published.0.error);

        let listing = list_resources(&state, &project_id).await;
        let after = listing
            .resources
            .iter()
            .find(|item| item.id == "qe-1")
            .unwrap();
        assert_eq!(after.status, ProjectRepositoryResourceStatus::UpToDate);
        assert!(after.aligned_at.is_some());
        assert!(after.repository_updated_at.is_some());
        assert!(
            !after.write_preview.contains(&"docs/AGENTS.md".to_string()),
            "the AGENTS.md line is present now, so it is no longer a write"
        );
        assert!(
            listing
                .uncommitted_managed_paths
                .contains(&"kronn/quick-execs/lint.yaml".to_string()),
            "{:?}",
            listing.uncommitted_managed_paths
        );
    }

    #[tokio::test]
    async fn repository_only_level_comes_from_the_file_not_a_stale_lock() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let rendered =
            crate::core::repository_resources::render_quick_exec(&sample_exec("project-1"), "lint")
                .unwrap();
        crate::core::repository_resources::publish(root.path(), "repo", rendered, false).unwrap();
        let lock_path = root
            .path()
            .join(crate::core::repository_resources::LOCK_PATH);
        let original = std::fs::read_to_string(&lock_path).unwrap();
        let stale = original.replace("\"level\": \"N1\"", "\"level\": \"N0\"");
        assert_ne!(
            stale, original,
            "the lock must have carried a level to make stale"
        );
        std::fs::write(&lock_path, stale).unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;

        let listing = list_resources(&state, &project_id).await;
        let entry = listing
            .resources
            .iter()
            .find(|item| item.id == "repository:quick_exec:lint")
            .expect("the lock entry is listed");
        assert_eq!(
            entry.status,
            ProjectRepositoryResourceStatus::RepositoryOnly
        );
        assert_eq!(entry.adr_level, ResourceAdrLevel::N1);
        assert!(
            entry.write_preview.is_empty(),
            "nothing to publish for a repository-only item"
        );
        assert!(entry.repository_updated_at.is_some());
    }

    #[tokio::test]
    async fn listing_reports_why_the_repository_cannot_be_written() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("kronn"), "#!/bin/sh\n").unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;

        let listing = list_resources(&state, &project_id).await;
        assert!(!listing.can_write_repository);
        assert_eq!(
            listing.can_write_repository_reason,
            Some(crate::models::RepositoryWriteBlocker::KronnPathIsFile),
            "the reason is a code the UI words, not a raw error"
        );
    }

    fn by_id<'a>(
        skills: &'a [ProjectRepositorySkill],
        id: &str,
    ) -> Option<&'a ProjectRepositorySkill> {
        skills.iter().find(|skill| skill.id == id)
    }

    #[tokio::test]
    async fn stack_suggested_skills_are_kronn_only_with_their_reason_and_never_repository_files() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Dockerfile"), "FROM scratch\n").unwrap();
        std::fs::write(root.path().join("package.json"), "{}").unwrap();
        std::fs::write(root.path().join("tsconfig.json"), "{}").unwrap();
        write_native_skill(root.path(), ".agents/skills", "review", "Review", "Body.");
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;

        let listing = list_resources(&state, &project_id).await;

        for (id, marker) in [("devops", "Dockerfile"), ("typescript", "tsconfig.json")] {
            let skill = by_id(&listing.skills_present, id)
                .unwrap_or_else(|| panic!("{id} is suggested for this stack"));
            assert!(skill.suggested, "{id}");
            assert_eq!(skill.suggested_reason.as_deref(), Some(marker));
            assert_eq!(skill.status, ProjectRepositoryResourceStatus::KronnOnly);
            assert_eq!(skill.provenance, ProjectRepositorySkillProvenance::Kronn);
            assert!(
                skill.repository_paths.is_empty(),
                "no repository file: {id}"
            );
        }
        let native = by_id(&listing.skills_present, "repository:review").unwrap();
        assert!(!native.suggested);
        assert_eq!(native.status, ProjectRepositoryResourceStatus::NativeSkill);
        assert!(native.repository_fingerprint.is_some());
        assert_eq!(
            native.repository_fingerprint.as_ref().map(String::len),
            Some(8)
        );
    }

    #[tokio::test]
    async fn every_skill_has_a_defined_state_and_unrelated_catalog_skills_stay_available() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "[package]\n").unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;

        let listing = list_resources(&state, &project_id).await;

        let rust = by_id(&listing.skills_present, "rust").expect("rust is suggested");
        assert!(rust.suggested);
        let available = by_id(&listing.skills_available, "web-performance")
            .expect("an unrelated catalog skill stays available");
        assert!(!available.suggested);
        assert!(available.suggested_reason.is_none());
        assert_eq!(available.status, ProjectRepositoryResourceStatus::KronnOnly);
        assert!(available.repository_paths.is_empty());
        assert!(
            by_id(&listing.skills_present, "web-performance").is_none(),
            "nothing detected it"
        );
    }

    #[tokio::test]
    async fn a_suggested_skill_that_is_already_attached_is_no_longer_a_suggestion() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Dockerfile"), "FROM scratch\n").unwrap();
        let project_id = "project-1".to_string();
        let mut project = mk_project(&project_id, root.path());
        project.default_skill_ids = vec!["devops".into()];
        seed_project(&state, project).await;

        let listing = list_resources(&state, &project_id).await;

        let devops = by_id(&listing.skills_present, "devops").unwrap();
        assert!(!devops.suggested);
        assert_eq!(devops.provenance, ProjectRepositorySkillProvenance::Kronn);
        assert_eq!(devops.status, ProjectRepositoryResourceStatus::KronnOnly);
    }

    #[tokio::test]
    async fn a_catalog_skill_found_in_a_repository_folder_is_repository_only() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        write_native_skill(root.path(), ".claude/skills", "devops", "DevOps", "Body.");
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;

        let listing = list_resources(&state, &project_id).await;

        let devops = by_id(&listing.skills_present, "devops").unwrap();
        assert_eq!(
            devops.status,
            ProjectRepositoryResourceStatus::RepositoryOnly
        );
        assert_eq!(
            devops.provenance,
            ProjectRepositorySkillProvenance::Repository
        );
        assert!(!devops.suggested);
        assert_eq!(
            devops.repository_paths,
            vec![".claude/skills/devops/SKILL.md".to_string()]
        );
    }

    #[tokio::test]
    async fn both_sides_carry_an_eight_character_fingerprint_that_differs_between_two_versions() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::quick_execs::insert_quick_exec(conn, &sample_exec("project-1"))?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();

        let before = list_resources(&state, &project_id).await;
        let unwritten = &before.resources[0];
        assert!(unwritten.repository_fingerprint.is_none(), "no file yet");
        assert_eq!(
            unwritten.kronn_fingerprint.as_ref().map(String::len),
            Some(8)
        );

        let published = publish_repository_resource(
            State(state.clone()),
            AxumPath(project_id.clone()),
            Json(PublishProjectRepositoryResourceRequest {
                kind: ProjectRepositoryResourceKind::QuickExec,
                id: "qe-1".into(),
                overwrite_repository_changes: false,
            }),
        )
        .await;
        assert!(published.0.data.is_some(), "{:?}", published.0.error);

        let after = list_resources(&state, &project_id).await;
        let aligned = &after.resources[0];
        assert_eq!(aligned.status, ProjectRepositoryResourceStatus::UpToDate);
        assert_eq!(aligned.repository_fingerprint, aligned.kronn_fingerprint);

        let path = root.path().join("kronn/quick-execs/lint.yaml");
        let edited = std::fs::read_to_string(&path)
            .unwrap()
            .replace("check", "build");
        std::fs::write(&path, edited).unwrap();
        let drifted = list_resources(&state, &project_id).await;
        let item = &drifted.resources[0];
        assert_ne!(item.repository_fingerprint, item.kronn_fingerprint);
        assert_eq!(
            item.repository_fingerprint.as_ref().map(String::len),
            Some(8)
        );
    }

    #[tokio::test]
    async fn a_new_resource_slug_is_plain_ascii_and_an_existing_one_is_left_alone() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        state
            .db
            .with_conn(|conn| {
                let mut exec = sample_exec("project-1");
                exec.name = "Plan de correction jeu réduit".into();
                crate::db::quick_execs::insert_quick_exec(conn, &exec)?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();

        let listing = list_resources(&state, &project_id).await;
        assert_eq!(listing.resources[0].slug, "plan-de-correction-jeu-reduit");
        assert_eq!(
            listing.resources[0].repository_paths,
            vec!["kronn/quick-execs/plan-de-correction-jeu-reduit.yaml".to_string()]
        );

        // Once published the slug is recorded: renaming the resource later
        // must not move the file.
        let published = publish_repository_resource(
            State(state.clone()),
            AxumPath(project_id.clone()),
            Json(PublishProjectRepositoryResourceRequest {
                kind: ProjectRepositoryResourceKind::QuickExec,
                id: "qe-1".into(),
                overwrite_repository_changes: false,
            }),
        )
        .await;
        assert!(published.0.data.is_some(), "{:?}", published.0.error);
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE quick_execs SET name = 'Autre nom' WHERE id = 'qe-1'",
                    [],
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        let renamed = list_resources(&state, &project_id).await;
        assert_eq!(renamed.resources[0].slug, "plan-de-correction-jeu-reduit");
    }
    /// A repository with a long history: `git log` on a path the history never
    /// touched has to walk all of it, which is what made the listing slow.
    fn seed_history(root: &Path, commits: usize) {
        use std::io::Write;
        let mut stream = String::new();
        for index in 0..commits {
            let body = format!("revision {index}\n");
            stream.push_str(&format!(
                "commit refs/heads/main\nmark :{}\ncommitter T <t@example.com> {} +0000\ndata 2\nc\n",
                index + 1,
                1_600_000_000 + index
            ));
            if index > 0 {
                stream.push_str(&format!("from :{index}\n"));
            }
            stream.push_str(&format!(
                "M 100644 inline src/file-{}.txt\ndata {}\n{}\n",
                index % 300,
                body.len(),
                body
            ));
        }
        let mut child = crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root)
            .args(["fast-import", "--quiet"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stream.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
        crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root)
            .args(["reset", "--hard", "-q"])
            .status()
            .unwrap();
    }

    /// Reads every file under `root`. The very first read of files just written
    /// costs the filesystem (0.5 to 2 s for a couple of MB in a sandbox), not
    /// the listing: it is timed apart so the listing's own numbers are readable.
    fn read_every_file(root: &Path) {
        let started = std::time::Instant::now();
        let mut bytes = 0;
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(directory).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    bytes += std::fs::read(path).map_or(0, |content| content.len());
                }
            }
        }
        eprintln!(
            "LISTING_SPEED first raw read of the files ({bytes} bytes): {:?}",
            started.elapsed()
        );
    }

    /// `cargo test --lib listing_speed -- --ignored --nocapture`: a stand-in for
    /// a large repository with many automations (79) and a few skills.
    #[tokio::test]
    #[ignore = "timing measurement, run on demand"]
    async fn listing_speed_on_a_long_history_with_many_automations() {
        isolate_config_dir();
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root.path())
            .args(["init", "-q", "-b", "main"])
            .status()
            .unwrap();
        seed_history(root.path(), 5_000);
        for marker in [
            "Dockerfile",
            "package.json",
            "tsconfig.json",
            "vite.config.ts",
        ] {
            std::fs::write(root.path().join(marker), "{}\n").unwrap();
        }
        write_native_skill(root.path(), ".agents/skills", "review", "Review", "Body.");
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        state
            .db
            .with_conn(|conn| {
                for index in 0..79 {
                    let mut exec = sample_exec("project-1");
                    exec.id = format!("qe-{index}");
                    exec.name = format!("Automation {index}");
                    crate::db::quick_execs::insert_quick_exec(conn, &exec)?;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();

        let mut runs = Vec::new();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let listing = list_resources(&state, &project_id).await;
            runs.push(started.elapsed());
            assert_eq!(listing.resources.len(), 79);
        }
        eprintln!("LISTING_SPEED unpublished runs={runs:?}");

        // Worst case: every automation already lives in the repository, so
        // each one has a real file whose last commit must be looked up.
        for index in 0..79 {
            let published = publish_repository_resource(
                State(state.clone()),
                AxumPath(project_id.clone()),
                Json(PublishProjectRepositoryResourceRequest {
                    kind: ProjectRepositoryResourceKind::QuickExec,
                    id: format!("qe-{index}"),
                    overwrite_repository_changes: false,
                }),
            )
            .await;
            assert!(published.0.data.is_some(), "{:?}", published.0.error);
        }
        crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root.path())
            .args(["add", "-A"])
            .status()
            .unwrap();
        crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root.path())
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(["commit", "-q", "-m", "publish"])
            .status()
            .unwrap();
        read_every_file(root.path());
        let mut runs = Vec::new();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let listing = list_resources(&state, &project_id).await;
            runs.push(started.elapsed());
            assert!(listing
                .resources
                .iter()
                .all(|item| item.status == ProjectRepositoryResourceStatus::UpToDate));
        }
        eprintln!("LISTING_SPEED all-published runs={runs:?}");
    }

    fn sample_prompt(project_id: &str, id: &str, template: &str) -> QuickPrompt {
        let timestamp = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        QuickPrompt {
            id: id.into(),
            name: format!("Prompt {id}"),
            icon: "terminal".into(),
            prompt_template: template.into(),
            variables: Vec::new(),
            agent: crate::models::AgentType::ClaudeCode,
            connection_id: None,
            project_id: Some(project_id.into()),
            skill_ids: Vec::new(),
            profile_ids: Vec::new(),
            directive_ids: Vec::new(),
            tier: crate::models::ModelTier::default(),
            agent_settings: None,
            description: String::new(),
            pinned: false,
            created_at: timestamp,
            updated_at: timestamp,
        }
    }

    /// A project holding one resource of each kind Kronn holds, all with fixed
    /// content and dates, so its listing reads the same on every run.
    async fn seed_one_of_each_kind(state: &crate::AppState, project_id: &str) {
        seed_one_of_each_kind_tagged(state, project_id, "").await;
    }

    /// [`seed_one_of_each_kind`] with `tag` added to the text of each resource,
    /// for a test that needs content no other test renders (the render memo is
    /// shared by the whole process). An empty tag changes nothing.
    async fn seed_one_of_each_kind_tagged(state: &crate::AppState, project_id: &str, tag: &str) {
        let project_id = project_id.to_string();
        let tag = tag.to_string();
        state
            .db
            .with_conn(move |conn| {
                let timestamp = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
                let mut prompt = sample_prompt(
                    &project_id,
                    "qp-1",
                    &format!("Review {{{{diff}}}} password=hunter2-golden{tag}"),
                );
                prompt.name = "Review prompt".into();
                crate::db::quick_prompts::insert_quick_prompt(conn, &prompt)?;

                let mut exec = sample_exec(&project_id);
                exec.args = vec![
                    "check".into(),
                    format!("--token=golden-exec-token-123456{tag}"),
                ];
                crate::db::quick_execs::insert_quick_exec(conn, &exec)?;

                let api: QuickApi = serde_json::from_value(serde_json::json!({
                    "id": "qa-1", "name": "Traffic", "icon": "api", "description": "",
                    "project_id": project_id, "api_plugin_slug": "api-traffic",
                    "api_config_id": "config-1", "api_endpoint_path": format!("/live{tag}"),
                    "api_method": "GET",
                    "api_headers": {"Authorization": "Bearer sk-live-1234567890abcdefghij", "Accept": "application/json"},
                    "api_query": {"page": "2", "api_key": "golden-api-key-123456"},
                    "variables": [], "profile_ids": [], "directive_ids": [], "pinned": false,
                    "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:10Z"
                }))
                .unwrap();
                crate::db::quick_apis::insert_quick_api(conn, &api)?;

                let workflow = sample_workflow_json(
                    "wf-1",
                    "Nightly",
                    &project_id,
                    serde_json::json!([
                        {
                            "id": "00000000-0000-4000-8000-000000000001",
                            "name": "ask", "step_type": {"type": "Agent"},
                            "quick_prompt_id": "qp-1",
                            "prompt_template": format!("Summarize password=hunter2-golden{tag}")
                        },
                        {
                            "id": "00000000-0000-4000-8000-000000000002",
                            "name": "fetch", "step_type": {"type": "ApiCall"},
                            "quick_api_id": "qa-1",
                            "api_headers": {"Authorization": "Bearer sk-live-1234567890abcdefghij"}
                        },
                        {
                            "id": "00000000-0000-4000-8000-000000000003",
                            "name": "collect", "step_type": {"type": "CollectApiData"},
                            "collect_api_data": {"sources": [{"alias": "lint", "quick_exec_id": "qe-1"}]}
                        },
                        {
                            "id": "00000000-0000-4000-8000-000000000004",
                            "name": "again", "step_type": {"type": "SubWorkflow"},
                            "sub_workflow_id": "wf-1"
                        }
                    ]),
                );
                crate::db::workflows::insert_workflow(conn, &workflow)?;

                let page = LivePage {
                    id: "page-1".into(),
                    project_id: Some(project_id.clone()),
                    title: "Dashboard".into(),
                    slug: "dashboard".into(),
                    current_revision_id: "rev-1".into(),
                    data_revision: 0,
                    created_at: timestamp,
                    updated_at: timestamp,
                    last_published_at: None,
                    pinned: false,
                    archived: false,
                };
                let revision = LivePageRevision {
                    id: "rev-1".into(),
                    page_id: "page-1".into(),
                    revision: 1,
                    html: format!("<h1>Dashboard</h1><p>password=hunter2-golden</p>{tag}"),
                    created_by_agent: Some("agent".into()),
                    created_at: timestamp,
                };
                crate::db::live_pages::create_live_page(conn, &page, &revision, &[], None)?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .expect("seed one of each kind");
    }

    /// The listing of a project with one resource of each kind, byte for byte
    /// (KT-915: the listing stopped copying the JSON values it renders). Read
    /// twice — the second time from the memo — it must not change either, and
    /// must hold no secret value.
    #[tokio::test]
    async fn the_listing_of_one_resource_of_each_kind_is_byte_for_byte_what_it_was() {
        // Captured from the code before KT-915, on the same content; KT-917
        // changed only the workflow's fingerprint (references written by slug).
        const GOLDEN: &str = "15d19b43075a2401f4dfe47b659e6bb59bdebcf52ccfd48b6f53329c147f9418";
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        seed_project(&state, mk_project("project-1", root.path())).await;
        seed_one_of_each_kind(&state, "project-1").await;

        let first = list_resources(&state, "project-1").await;
        let second = list_resources(&state, "project-1").await;
        let workflow = first
            .resources
            .iter()
            .find(|item| item.kind == ProjectRepositoryResourceKind::Workflow)
            .expect("the workflow is listed");
        assert_eq!(
            workflow.uses.len(),
            3,
            "the workflow's references are part of what is compared: {:?}",
            workflow.uses
        );
        let first = serde_json::to_string(&first.resources).unwrap();
        let second = serde_json::to_string(&second.resources).unwrap();
        assert_eq!(first, second, "served from memory it reads the same");
        for secret in [
            "hunter2-golden",
            "golden-exec-token-123456",
            "golden-api-key-123456",
            "sk-live-1234567890abcdefghij",
        ] {
            assert!(!first.contains(secret), "{secret} leaked into the listing");
        }
        let digest = crate::core::repository_resources::sha256(first.as_bytes());
        assert_eq!(
            digest, GOLDEN,
            "the listing no longer reads the same bytes: {first}"
        );
    }

    // ── KT-915: the render memo is warmed in the background ─────────────────

    use super::super::resource_prewarm::{self, Progress, Timings};
    use tokio_util::sync::CancellationToken;

    /// Whether the render memo already holds this resource under `slug`.
    fn is_memoized(loaded: &LoadedResource, slug: &str) -> bool {
        use crate::core::repository_resources::render_is_memoized as memoized;
        fn json<T: serde::Serialize>(value: &T) -> serde_json::Value {
            serde_json::to_value(value).unwrap()
        }
        match loaded {
            LoadedResource::Workflow(workflow) => {
                // Rendered as a definition, never as an activation state.
                let mut source = json(workflow);
                source["enabled"] = serde_json::Value::Bool(false);
                memoized(
                    ProjectRepositoryResourceKind::Workflow,
                    slug,
                    workflow.updated_at,
                    &source,
                )
            }
            LoadedResource::QuickPrompt(prompt) => memoized(
                ProjectRepositoryResourceKind::QuickPrompt,
                slug,
                prompt.updated_at,
                &json(prompt),
            ),
            LoadedResource::QuickApi(api) => memoized(
                ProjectRepositoryResourceKind::QuickApi,
                slug,
                api.updated_at,
                &json(api),
            ),
            LoadedResource::QuickExec(exec) => memoized(
                ProjectRepositoryResourceKind::QuickExec,
                slug,
                exec.updated_at,
                &json(exec),
            ),
            LoadedResource::Artifact { page, updated_at } => memoized(
                ProjectRepositoryResourceKind::Artifact,
                slug,
                *updated_at,
                &json(page),
            ),
            LoadedResource::Skill(_) => unreachable!("skills are not warmed"),
        }
    }

    /// What the warm-up plans for a project, with the slugs the listing uses.
    async fn planned_resources(
        state: &crate::AppState,
        project_id: &str,
    ) -> Vec<(LoadedResource, String)> {
        let project_id = project_id.to_string();
        state
            .db
            .with_read_conn(move |conn| {
                let project_key =
                    crate::db::resource_identities::project_key(conn, Some(&project_id))?;
                let mut planned = Vec::new();
                for seed in project_seeds(conn, &project_id)? {
                    let slug = resource_slug(conn, &project_key, &seed)?;
                    planned.push((seed.loaded, slug));
                }
                Ok(planned)
            })
            .await
            .unwrap()
    }

    /// `count` Quick Prompts of about 24 KB with a credential assignment on
    /// most lines, so masking them is slow enough to observe. `nonce` keeps
    /// their text to this test.
    async fn seed_heavy_prompts(state: &crate::AppState, count: usize, nonce: &str) {
        let nonce = nonce.to_string();
        state
            .db
            .with_conn(move |conn| {
                for index in 0..count {
                    let mut body = format!("# heavy prompt {index} {nonce}\n");
                    while body.len() < 24 * 1024 {
                        body.push_str(&format!(
                            "Send Authorization: Bearer ${{TOKEN_{index}}} and keep password=${{DB_{index}}} out of the log ({nonce})\n"
                        ));
                    }
                    crate::db::quick_prompts::insert_quick_prompt(
                        conn,
                        &sample_prompt("project-1", &format!("heavy-{index}"), &body),
                    )?;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
    }

    async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        for _ in 0..500 {
            if done() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("gave up waiting for {what}");
    }

    fn fast_timings() -> Timings {
        Timings {
            startup: std::time::Duration::ZERO,
            // Wide enough that a busy machine stalling between two edits of
            // a burst does not split it.
            debounce: std::time::Duration::from_millis(500),
            max_debounce: std::time::Duration::from_secs(5),
            breath: std::time::Duration::ZERO,
        }
    }

    async fn edit_prompt(state: &crate::AppState, template: String) {
        state
            .db
            .with_conn(move |conn| {
                let mut prompt = crate::db::quick_prompts::get_quick_prompt(conn, "watched")?
                    .expect("the watched prompt exists");
                prompt.prompt_template = template;
                crate::db::quick_prompts::update_quick_prompt(conn, &prompt)?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
    }

    async fn state_with_watched_prompt(nonce: &str) -> (crate::AppState, tempfile::TempDir) {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        seed_project(&state, mk_project("project-1", root.path())).await;
        let template = format!("Watched prompt, first version ({nonce})");
        state
            .db
            .with_conn(move |conn| {
                crate::db::quick_prompts::insert_quick_prompt(
                    conn,
                    &sample_prompt("project-1", "watched", &template),
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        (state, root)
    }

    /// The warm-up renders what the listing will ask for, under the same slugs
    /// and from the same content: afterwards, the render memo holds all of it.
    #[tokio::test]
    async fn warming_a_project_renders_what_its_listing_will_ask_for() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        seed_project(&state, mk_project("project-1", root.path())).await;
        let tag = format!(" warm-probe-{}", Uuid::new_v4());
        seed_one_of_each_kind_tagged(&state, "project-1", &tag).await;

        let planned = planned_resources(&state, "project-1").await;
        assert_eq!(planned.len(), 5, "one of each kind the listing renders");
        for (loaded, slug) in &planned {
            assert!(!is_memoized(loaded, slug), "{slug} is not rendered yet");
        }

        let progress = Progress::default();
        let warmed = resource_prewarm::warm_project(
            &state.db,
            "project-1",
            &CancellationToken::new(),
            std::time::Duration::ZERO,
            &progress,
        )
        .await
        .unwrap();
        assert_eq!(warmed, 5);
        for (loaded, slug) in &planned {
            assert!(
                is_memoized(loaded, slug),
                "{slug} is rendered ahead of time"
            );
        }

        // The listing is then served from memory, and shows no secret value.
        let hits = crate::core::repository_resources::render_memo_stats().hits;
        let listing = list_resources(&state, "project-1").await;
        assert!(
            crate::core::repository_resources::render_memo_stats().hits >= hits + 5,
            "every resource of the listing came from the warmed memo"
        );
        let listed = serde_json::to_string(&listing.resources).unwrap();
        for secret in [
            "hunter2-golden",
            "golden-exec-token-123456",
            "golden-api-key-123456",
        ] {
            assert!(!listed.contains(secret), "{secret} leaked into the listing");
        }
    }

    /// Rendering is done off the database: while the warm-up masks its
    /// resources, a request finds the read connection free.
    #[tokio::test]
    async fn warming_leaves_the_database_free_for_a_request_while_it_renders() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        seed_project(&state, mk_project("project-1", root.path())).await;
        seed_heavy_prompts(&state, 150, &Uuid::new_v4().to_string()).await;

        let progress = std::sync::Arc::new(Progress::default());
        let warm = tokio::spawn({
            let (db, progress) = (state.db.clone(), progress.clone());
            async move {
                resource_prewarm::warm_project(
                    &db,
                    "project-1",
                    &CancellationToken::new(),
                    std::time::Duration::from_millis(1),
                    &progress,
                )
                .await
            }
        });
        wait_until("the warm-up to be rendering", || {
            progress.rendered.load(std::sync::atomic::Ordering::Acquire) >= 3
        })
        .await;
        assert!(!warm.is_finished(), "the run is still going");

        // A request now: it is served, and the warm-up is not done yet.
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            state.db.with_read_conn(|_| Ok(())),
        )
        .await
        .expect("a request is not made to wait for the warm-up")
        .unwrap();
        assert!(
            !warm.is_finished(),
            "the request was served while the warm-up was still rendering"
        );
        assert_eq!(warm.await.unwrap().unwrap(), 150);
    }

    #[tokio::test]
    async fn the_warm_up_never_delays_the_start_and_stops_at_shutdown() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        seed_project(&state, mk_project("project-1", root.path())).await;
        seed_heavy_prompts(&state, 150, &Uuid::new_v4().to_string()).await;

        // Started, it returns at once whatever it waits for; stopped before
        // its first run, it renders nothing.
        let shutdown = CancellationToken::new();
        let progress = std::sync::Arc::new(Progress::default());
        let started = std::time::Instant::now();
        let waiting = resource_prewarm::spawn_with(
            state.db.clone(),
            shutdown.clone(),
            Timings {
                startup: std::time::Duration::from_secs(3600),
                ..fast_timings()
            },
            progress.clone(),
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
            .await
            .expect("stops at shutdown")
            .unwrap();
        assert_eq!(
            progress.rendered.load(std::sync::atomic::Ordering::Acquire),
            0
        );

        // Stopped in the middle of a run, it leaves the rest alone.
        let shutdown = CancellationToken::new();
        let progress = std::sync::Arc::new(Progress::default());
        let running = resource_prewarm::spawn_with(
            state.db.clone(),
            shutdown.clone(),
            Timings {
                breath: std::time::Duration::from_millis(1),
                ..fast_timings()
            },
            progress.clone(),
        );
        wait_until("the run to start", || {
            progress.rendered.load(std::sync::atomic::Ordering::Acquire) >= 3
        })
        .await;
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), running)
            .await
            .expect("stops at shutdown")
            .unwrap();
        assert!(progress.rendered.load(std::sync::atomic::Ordering::Acquire) < 150);
    }

    /// The memo is bounded (48 MiB): the warm-up stops rather than push out
    /// what it has computed.
    #[tokio::test]
    async fn the_warm_up_stops_when_the_render_memo_is_full() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        seed_project(&state, mk_project("project-1", root.path())).await;
        seed_one_of_each_kind_tagged(&state, "project-1", &format!(" full-{}", Uuid::new_v4()))
            .await;

        let asked = std::sync::atomic::AtomicUsize::new(0);
        let room_for_two = || asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2;
        let warmed = resource_prewarm::warm_project_while(
            &state.db,
            "project-1",
            &CancellationToken::new(),
            std::time::Duration::ZERO,
            &Progress::default(),
            &room_for_two,
        )
        .await
        .unwrap();
        assert_eq!(warmed, 2, "no more once the memo says it is full");

        let nothing = resource_prewarm::warm_project_while(
            &state.db,
            "project-1",
            &CancellationToken::new(),
            std::time::Duration::ZERO,
            &Progress::default(),
            &|| false,
        )
        .await
        .unwrap();
        assert_eq!(nothing, 0);
    }

    /// After the start and after an edit, the memo holds the current content;
    /// a burst of edits is one run, not one per edit.
    #[tokio::test]
    async fn the_warm_up_runs_after_the_start_and_after_an_edit_once_per_burst() {
        let (state, _root) = state_with_watched_prompt(&Uuid::new_v4().to_string()).await;
        let shutdown = CancellationToken::new();
        let progress = std::sync::Arc::new(Progress::default());
        let passes = || progress.passes.load(std::sync::atomic::Ordering::Acquire);
        let warm_up = resource_prewarm::spawn_with(
            state.db.clone(),
            shutdown.clone(),
            fast_timings(),
            progress.clone(),
        );

        // After the start: what the database held is rendered.
        wait_until("the first run", || passes() >= 1).await;
        let planned = planned_resources(&state, "project-1").await;
        assert!(is_memoized(&planned[0].0, &planned[0].1));

        // After an edit: the new content is, once the writes have stopped.
        let nonce = Uuid::new_v4();
        for version in 0..5 {
            edit_prompt(&state, format!("Watched prompt, edit {version} ({nonce})")).await;
        }
        wait_until("the run after the edits", || passes() >= 2).await;
        let planned = planned_resources(&state, "project-1").await;
        assert!(
            matches!(&planned[0].0, LoadedResource::QuickPrompt(prompt) if prompt.prompt_template.contains("edit 4")),
            "the database holds the last edit"
        );
        assert!(
            is_memoized(&planned[0].0, &planned[0].1),
            "and the memo holds it too"
        );

        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        assert_eq!(passes(), 2, "five edits in a burst made one run");

        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), warm_up)
            .await
            .expect("stops at shutdown")
            .unwrap();
    }

    /// A workflow writing all day cannot keep the resources cold: the run
    /// waits for the writes to stop, but not for ever.
    #[tokio::test]
    async fn a_steady_stream_of_edits_cannot_postpone_the_warm_up_for_ever() {
        let (state, _root) = state_with_watched_prompt(&Uuid::new_v4().to_string()).await;
        let shutdown = CancellationToken::new();
        let progress = std::sync::Arc::new(Progress::default());
        let warm_up = resource_prewarm::spawn_with(
            state.db.clone(),
            shutdown.clone(),
            Timings {
                debounce: std::time::Duration::from_millis(400),
                max_debounce: std::time::Duration::from_secs(1),
                ..fast_timings()
            },
            progress.clone(),
        );
        wait_until("the first run", || {
            progress.passes.load(std::sync::atomic::Ordering::Acquire) >= 1
        })
        .await;

        // An edit every 60 ms, never quiet for the 400 ms the debounce wants.
        let started = std::time::Instant::now();
        let mut ran_during_the_stream = false;
        while started.elapsed() < std::time::Duration::from_millis(2500) {
            edit_prompt(
                &state,
                format!("Watched prompt, stream {:?}", started.elapsed()),
            )
            .await;
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            if progress.passes.load(std::sync::atomic::Ordering::Acquire) >= 2 {
                ran_during_the_stream = true;
                break;
            }
        }
        assert!(
            ran_during_the_stream,
            "a run happened while the edits kept coming"
        );

        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), warm_up)
            .await
            .expect("stops at shutdown")
            .unwrap();
    }

    async fn compare(
        state: &crate::AppState,
        project_id: &str,
        kind: ProjectRepositoryResourceKind,
        id: &str,
    ) -> Json<ApiResponse<RepositoryResourceComparison>> {
        repository_resource_comparison(
            State(state.clone()),
            AxumPath(project_id.to_string()),
            Query(RepositoryComparisonQuery {
                kind,
                id: id.to_string(),
            }),
        )
        .await
    }

    #[tokio::test]
    async fn diffs_are_built_on_demand_and_no_secret_value_reaches_the_listing_or_the_comparison() {
        const EXEC_SECRET: &str = "leak-probe-exec-secret";
        const PROMPT_SECRET: &str = "leak-probe-prompt-secret";
        const VENDOR_KEY: &str = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        state
            .db
            .with_conn(|conn| {
                let mut exec = sample_exec("project-1");
                exec.args = vec!["--token".into(), EXEC_SECRET.into()];
                crate::db::quick_execs::insert_quick_exec(conn, &exec)?;
                crate::db::quick_prompts::insert_quick_prompt(
                    conn,
                    &sample_prompt(
                        "project-1",
                        "qp-1",
                        &format!("Call it with password={PROMPT_SECRET} and the key {VENDOR_KEY}."),
                    ),
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        for (kind, id) in [
            (ProjectRepositoryResourceKind::QuickExec, "qe-1"),
            (ProjectRepositoryResourceKind::QuickPrompt, "qp-1"),
        ] {
            let published = publish_repository_resource(
                State(state.clone()),
                AxumPath(project_id.clone()),
                Json(PublishProjectRepositoryResourceRequest {
                    kind,
                    id: id.into(),
                    overwrite_repository_changes: false,
                }),
            )
            .await;
            assert!(published.0.data.is_some(), "{:?}", published.0.error);
        }

        // Both sides agree: nothing to compare, and the listing says so.
        let aligned = list_resources(&state, &project_id).await;
        assert!(aligned
            .resources
            .iter()
            .all(|item| item.status == ProjectRepositoryResourceStatus::UpToDate));
        let nothing = compare(
            &state,
            &project_id,
            ProjectRepositoryResourceKind::QuickExec,
            "qe-1",
        )
        .await
        .0
        .data
        .expect("comparison");
        assert!(nothing.diff.is_none() && nothing.file_diffs.is_empty());

        // The repository moves on both resources, Kronn on the exec only.
        let exec_path = root.path().join("kronn/quick-execs/lint.yaml");
        let edited = std::fs::read_to_string(&exec_path)
            .unwrap()
            .replace("\"timeout_secs\": 30", "\"timeout_secs\": 45");
        std::fs::write(&exec_path, edited).unwrap();
        let prompt_path = root.path().join("kronn/prompts/prompt-qp-1.md");
        let mut edited = std::fs::read_to_string(&prompt_path).unwrap();
        edited.push_str("\nEdited in Git.\n");
        std::fs::write(&prompt_path, edited).unwrap();
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE quick_execs SET description = 'Edited in Kronn' WHERE id = 'qe-1'",
                    [],
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();

        let listing = list_resources(&state, &project_id).await;
        let by_kind = |kind| {
            listing
                .resources
                .iter()
                .find(|item| item.kind == kind)
                .unwrap()
        };
        assert_eq!(
            by_kind(ProjectRepositoryResourceKind::QuickExec).status,
            ProjectRepositoryResourceStatus::Conflict
        );
        assert_eq!(
            by_kind(ProjectRepositoryResourceKind::QuickPrompt).status,
            ProjectRepositoryResourceStatus::RepositoryNewer
        );

        // The listing says that the sides differ, never how.
        let listed = serde_json::to_value(&listing).unwrap();
        for resource in listed["resources"].as_array().unwrap() {
            for key in ["diff", "file_diffs", "field_diff"] {
                assert!(resource.get(key).is_none(), "{key} is not in the listing");
            }
        }
        let listed = listed.to_string();
        for secret in [EXEC_SECRET, PROMPT_SECRET, VENDOR_KEY] {
            assert!(!listed.contains(secret), "{secret} leaked into the listing");
        }

        // Listing again masks nothing again: every resource is served from
        // the memo (other tests may add hits, never remove them).
        let hits_before = crate::core::repository_resources::render_memo_stats().hits;
        list_resources(&state, &project_id).await;
        assert!(
            crate::core::repository_resources::render_memo_stats().hits >= hits_before + 2,
            "a second read of an unchanged project reuses the masked renderings"
        );

        // The comparison shows both diffs, from the same masked rendering.
        let exec = compare(
            &state,
            &project_id,
            ProjectRepositoryResourceKind::QuickExec,
            "qe-1",
        )
        .await
        .0
        .data
        .expect("comparison");
        assert!(exec.diff.as_deref().unwrap().contains("--- repository"));
        assert!(exec
            .field_diff
            .iter()
            .any(|item| item.field == "description"));
        let prompt = compare(
            &state,
            &project_id,
            ProjectRepositoryResourceKind::QuickPrompt,
            "qp-1",
        )
        .await
        .0
        .data
        .expect("comparison");
        assert!(prompt.diff.as_deref().unwrap().contains("Edited in Git."));
        assert!(
            prompt
                .diff
                .as_deref()
                .unwrap()
                .contains("secret://KRONN_QUICKPROMPT_"),
            "the secret shows as its reference"
        );
        for comparison in [&exec, &prompt] {
            let shown = serde_json::to_string(comparison).unwrap();
            for secret in [EXEC_SECRET, PROMPT_SECRET, VENDOR_KEY] {
                assert!(
                    !shown.contains(secret),
                    "{secret} leaked into the comparison"
                );
            }
        }

        // What does not exist for this project is not compared.
        let unknown = compare(
            &state,
            &project_id,
            ProjectRepositoryResourceKind::QuickExec,
            "qe-unknown",
        )
        .await;
        assert!(unknown.0.data.is_none());
        let repository_only = compare(
            &state,
            &project_id,
            ProjectRepositoryResourceKind::QuickExec,
            "repository:quick_exec:elsewhere",
        )
        .await
        .0
        .data
        .expect("a repository-only row has an empty comparison");
        assert!(repository_only.diff.is_none() && repository_only.field_diff.is_empty());
    }

    fn file_of<'a>(
        comparison: &'a RepositoryResourceComparison,
        path: &str,
    ) -> &'a RepositoryResourceFileContent {
        comparison
            .files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("no {path} in {:?}", comparison.files))
    }

    #[tokio::test]
    async fn the_comparison_carries_the_masked_text_of_each_side_whatever_the_state() {
        const PROMPT_SECRET: &str = "leak-probe-content-secret";
        const VENDOR_KEY: &str = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::quick_prompts::insert_quick_prompt(
                    conn,
                    &sample_prompt(
                        "project-1",
                        "qp-1",
                        &format!("Use password={PROMPT_SECRET} and the key {VENDOR_KEY}."),
                    ),
                )
            })
            .await
            .unwrap();
        let path = "kronn/prompts/prompt-qp-1.md";
        let compare_prompt = || {
            compare(
                &state,
                &project_id,
                ProjectRepositoryResourceKind::QuickPrompt,
                "qp-1",
            )
        };

        // Kronn only: its text, masked, and no diff to build.
        let kronn_only = compare_prompt().await.0.data.expect("comparison");
        let file = file_of(&kronn_only, path);
        assert!(file.repository.is_none());
        assert!(file.kronn.as_deref().unwrap().contains("Use password="));
        assert!(kronn_only.diff.is_none() && kronn_only.file_diffs.is_empty());

        // In sync: the same text on both sides, still no diff.
        let published = publish_repository_resource(
            State(state.clone()),
            AxumPath(project_id.clone()),
            Json(PublishProjectRepositoryResourceRequest {
                kind: ProjectRepositoryResourceKind::QuickPrompt,
                id: "qp-1".into(),
                overwrite_repository_changes: false,
            }),
        )
        .await;
        assert!(published.0.data.is_some(), "{:?}", published.0.error);
        let in_sync = compare_prompt().await.0.data.expect("comparison");
        let file = file_of(&in_sync, path);
        assert!(file.repository.is_some() && file.repository == file.kronn);
        assert!(in_sync.diff.is_none() && in_sync.file_diffs.is_empty());

        // Two versions: both texts, told apart, and the diff beside them.
        let repository_file = root.path().join(path);
        let mut edited = std::fs::read_to_string(&repository_file).unwrap();
        edited.push_str("\nEdited in Git.\n");
        std::fs::write(&repository_file, edited).unwrap();
        let differing = compare_prompt().await.0.data.expect("comparison");
        let file = file_of(&differing, path);
        assert!(file
            .repository
            .as_deref()
            .unwrap()
            .contains("Edited in Git."));
        assert!(!file.kronn.as_deref().unwrap().contains("Edited in Git."));
        assert!(differing.diff.is_some());

        // Nothing in any of the three answers holds a secret value.
        for comparison in [&kronn_only, &in_sync, &differing] {
            let shown = serde_json::to_string(comparison).unwrap();
            for secret in [PROMPT_SECRET, VENDOR_KEY] {
                assert!(!shown.contains(secret), "{secret} leaked into the content");
            }
            assert!(shown.contains("secret://KRONN_QUICKPROMPT_"));
        }

        // The prompt leaves Kronn: the repository copy is all that is left.
        state
            .db
            .with_conn(|conn| {
                conn.execute("DELETE FROM quick_prompts WHERE id = 'qp-1'", [])?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        let listing = list_resources(&state, &project_id).await;
        let orphan = listing
            .resources
            .iter()
            .find(|item| item.status == ProjectRepositoryResourceStatus::RepositoryOnly)
            .expect("the published file is now repository only");
        let repository_only = compare(
            &state,
            &project_id,
            ProjectRepositoryResourceKind::QuickPrompt,
            &orphan.id,
        )
        .await
        .0
        .data
        .expect("comparison");
        let file = file_of(&repository_only, path);
        assert!(file
            .repository
            .as_deref()
            .unwrap()
            .contains("Edited in Git."));
        assert!(file.kronn.is_none());
    }

    #[tokio::test]
    async fn a_secret_typed_into_a_repository_file_reaches_neither_its_text_nor_a_diff() {
        const TYPED_PASSWORD: &str = "leak-probe-typed-by-hand";
        const TYPED_KEY: &str = "sk-zyxwvutsrqponmlkjihgfedcba9876543210";
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::quick_execs::insert_quick_exec(conn, &sample_exec("project-1"))?;
                crate::db::quick_prompts::insert_quick_prompt(
                    conn,
                    &sample_prompt("project-1", "qp-1", "Review the change."),
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        attach_skills(&state, &project_id, &["rust"]).await;
        for (kind, id) in [
            (ProjectRepositoryResourceKind::QuickExec, "qe-1"),
            (ProjectRepositoryResourceKind::QuickPrompt, "qp-1"),
            (ProjectRepositoryResourceKind::Skill, "rust"),
        ] {
            let published = publish_repository_resource(
                State(state.clone()),
                AxumPath(project_id.clone()),
                Json(PublishProjectRepositoryResourceRequest {
                    kind,
                    id: id.into(),
                    overwrite_repository_changes: false,
                }),
            )
            .await;
            assert!(published.0.data.is_some(), "{:?}", published.0.error);
        }

        // Someone types a secret into each file in Git; Kronn's side has none.
        let append = |relative: &str, text: String| {
            let path = root.path().join(relative);
            let mut content = std::fs::read_to_string(&path).unwrap();
            content.push_str(&text);
            std::fs::write(path, content).unwrap();
        };
        append(
            "kronn/prompts/prompt-qp-1.md",
            format!("\nDeploy with password={TYPED_PASSWORD} and the key {TYPED_KEY}.\n"),
        );
        append(
            ".agents/skills/rust/SKILL.md",
            format!("\nLog in with the key {TYPED_KEY}.\n"),
        );
        let exec_path = root.path().join("kronn/quick-execs/lint.yaml");
        let edited = std::fs::read_to_string(&exec_path).unwrap().replace(
            "\"description\": \"\"",
            &format!("\"description\": \"run with api_key={TYPED_PASSWORD}\""),
        );
        assert!(edited.contains(TYPED_PASSWORD), "the exec file holds it");
        std::fs::write(&exec_path, edited).unwrap();

        let cases = [
            (
                ProjectRepositoryResourceKind::QuickPrompt,
                "qp-1",
                "kronn/prompts/prompt-qp-1.md",
            ),
            (
                ProjectRepositoryResourceKind::Skill,
                "rust",
                ".agents/skills/rust/SKILL.md",
            ),
            (
                ProjectRepositoryResourceKind::QuickExec,
                "qe-1",
                "kronn/quick-execs/lint.yaml",
            ),
        ];
        for (kind, id, path) in cases {
            let comparison = compare(&state, &project_id, kind, id)
                .await
                .0
                .data
                .expect("comparison");
            let shown = serde_json::to_string(&comparison).unwrap();
            for secret in [TYPED_PASSWORD, TYPED_KEY] {
                assert!(
                    !shown.contains(secret),
                    "{secret} from the repository file leaked into the {id} comparison"
                );
            }
            // The repository text is there, with the secret masked out of it…
            let repository = file_of(&comparison, path).repository.as_deref().unwrap();
            assert!(repository.contains("***REDACTED***"), "{id}: {repository}");
            // …and the diff shows the masked line, not the secret.
            let diff = comparison.diff.as_deref().expect("the sides now differ");
            assert!(diff.contains("***REDACTED***"), "{id}: {diff}");
        }
        let exec = compare(
            &state,
            &project_id,
            ProjectRepositoryResourceKind::QuickExec,
            "qe-1",
        )
        .await
        .0
        .data
        .expect("comparison");
        let description = exec
            .field_diff
            .iter()
            .find(|item| item.field == "description")
            .expect("the description differs");
        assert!(description
            .repository
            .as_ref()
            .and_then(|value| value.as_str())
            .is_some_and(|text| text.contains("api_key=***REDACTED***")));
    }

    #[tokio::test]
    async fn a_skill_shows_the_side_or_sides_that_hold_it() {
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        let skill_compare = |id: &'static str| {
            compare(
                &state,
                &project_id,
                ProjectRepositoryResourceKind::Skill,
                id,
            )
        };
        let standard = ".agents/skills/rust/SKILL.md";

        // A catalog skill the project does not use: Kronn's text, nothing in the repository.
        let catalog = skill_compare("rust").await.0.data.expect("comparison");
        let file = file_of(&catalog, standard);
        assert!(file.repository.is_none());
        assert!(file
            .kronn
            .as_deref()
            .unwrap()
            .starts_with("---\nname: rust\n"));

        // A skill found in a native folder, unknown to the catalog: that text only.
        write_native_skill(
            root.path(),
            ".claude/skills",
            "review",
            "Review",
            "Version A.",
        );
        write_native_skill(
            root.path(),
            ".agents/skills",
            "review",
            "Review",
            "Version B.",
        );
        let native = skill_compare("repository:review")
            .await
            .0
            .data
            .expect("comparison");
        assert_eq!(
            native.files.len(),
            2,
            "every copy is listed under its own path"
        );
        assert!(file_of(&native, ".claude/skills/review/SKILL.md")
            .repository
            .as_deref()
            .unwrap()
            .contains("Version A."));
        assert!(native.files.iter().all(|file| file.kronn.is_none()));

        // Attached and published: the same text twice, then two versions with their diff.
        attach_skills(&state, &project_id, &["rust"]).await;
        let published = publish_repository_resource(
            State(state.clone()),
            AxumPath(project_id.clone()),
            publish_request("rust"),
        )
        .await;
        assert!(published.0.data.is_some(), "{:?}", published.0.error);
        let in_sync = skill_compare("rust").await.0.data.expect("comparison");
        let file = file_of(&in_sync, standard);
        assert!(file.repository.is_some() && file.repository == file.kronn);
        assert!(in_sync.diff.is_none());
        let path = root.path().join(standard);
        let mut edited = std::fs::read_to_string(&path).unwrap();
        edited.push_str("\nEdited in Git.\n");
        std::fs::write(&path, edited).unwrap();
        let differing = skill_compare("rust").await.0.data.expect("comparison");
        let file = file_of(&differing, standard);
        assert!(file
            .repository
            .as_deref()
            .unwrap()
            .contains("Edited in Git."));
        assert!(!file.kronn.as_deref().unwrap().contains("Edited in Git."));
        assert!(differing
            .diff
            .as_deref()
            .unwrap()
            .contains("Edited in Git."));
    }

    #[test]
    fn a_side_longer_than_the_bound_is_cut_on_a_character_and_flagged() {
        let (text, cut) = side_text("short".as_bytes());
        assert_eq!((text.as_str(), cut), ("short", false));
        let (text, cut) = side_text(&vec![b'a'; MAX_CONTENT_BYTES + 10]);
        assert_eq!((text.len(), cut), (MAX_CONTENT_BYTES, true));
        // Two-byte characters straddling the bound never split.
        let (text, cut) = side_text("é".repeat(MAX_CONTENT_BYTES).as_bytes());
        assert!(cut && text.len() <= MAX_CONTENT_BYTES && text.chars().all(|c| c == 'é'));
    }

    /// Renders the resources of `project_id` ahead of its first listing, as the
    /// backend does after it starts, and says how long that took.
    async fn time_the_warm_up(state: &crate::AppState, project_id: &str) -> std::time::Duration {
        let started = std::time::Instant::now();
        resource_prewarm::warm_project(
            &state.db,
            project_id,
            &CancellationToken::new(),
            std::time::Duration::ZERO,
            &Progress::default(),
        )
        .await
        .unwrap();
        started.elapsed()
    }

    /// 60 prompts of about 8 KB and 20 Quick Execs, all Kronn-side (nothing
    /// published, so no diff to build): the listing of the first read, then of
    /// the reads that follow. With `warm_up`, the resources are rendered ahead
    /// of that first read, the way the backend does after it starts; the two
    /// runs hold different text, so neither is served by the other's memo.
    async fn time_listing_of_large_automations(
        label: &'static str,
        sentences: &'static [&'static str],
        warm_up: bool,
    ) {
        isolate_config_dir();
        let run = if warm_up { "warmed" } else { "cold" };
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        state
            .db
            .with_conn(move |conn| {
                for index in 0..60 {
                    let mut template = format!("# {label} {run} automation {index}\n\n");
                    let mut turn = index;
                    while template.len() < 8 * 1024 {
                        template.push_str(sentences[turn % sentences.len()]);
                        template.push_str(&format!(" (step {turn})\n"));
                        turn += 1;
                    }
                    crate::db::quick_prompts::insert_quick_prompt(
                        conn,
                        &sample_prompt("project-1", &format!("qp-{index}"), &template),
                    )?;
                }
                for index in 0..20 {
                    let mut exec = sample_exec("project-1");
                    exec.id = format!("qe-{index}");
                    exec.name = format!("Automation exec {index} {run}");
                    crate::db::quick_execs::insert_quick_exec(conn, &exec)?;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();

        let warmed_in = if warm_up {
            Some(time_the_warm_up(&state, &project_id).await)
        } else {
            None
        };
        let mut runs = Vec::new();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let listing = list_resources(&state, &project_id).await;
            runs.push(started.elapsed());
            assert_eq!(listing.resources.len(), 80);
        }
        match warmed_in {
            None => eprintln!("LISTING_SPEED {label}, first (cold) then repeated: {runs:?}"),
            Some(took) => eprintln!(
                "LISTING_SPEED {label}, first after the warm-up (which took {took:?}) then repeated: {runs:?}"
            ),
        }
    }

    /// `cargo test --lib listing_speed_with_many_large -- --ignored --nocapture`:
    /// a project with many automations holding long prompts, the shape that made
    /// the listing take seconds because every read masked all of them again.
    /// The second flavour is the worst case for the masking: a credential
    /// assignment on most lines, so no pattern is turned away cheaply.
    #[tokio::test]
    #[ignore = "timing measurement, run on demand"]
    async fn listing_speed_with_many_large_automations() {
        const PROSE: &[&str] = &[
            "Review the mapping between the shipping token budget and the pin of each dependency.",
            "Keep the answer short: list the files touched, then the tests run, then what is left.",
            "The password policy lives in the security page; use a connection, never paste credentials.",
            "Open https://example.com/docs/guide and compare it with the notes: key: value, one per line.",
            "Explain the trade-off first, then propose the smallest change; wait for approval before writing.",
            "Return JSON with the fields status, summary and next_steps; no prose outside the object.",
        ];
        const CREDENTIALS: &[&str] = &[
            "Export API_KEY=${SERVICE_KEY} before the run; the token: ${DEPLOY_TOKEN} comes from the vault.",
            "Send Authorization: Bearer ${GITHUB_TOKEN} in the header and never log password=${DB_PASSWORD}.",
            "curl -H 'x-api-key: ${SERVICE_KEY}' https://example.com/v1/items and keep the reply short.",
            "Connect with postgres://app:${DB_PASSWORD}@db.internal/app, then list the tables that changed.",
            "Explain the trade-off first, then propose the smallest change; wait for approval before writing.",
            "Return JSON with the fields status, summary and next_steps; no prose outside the object.",
        ];
        for warm_up in [false, true] {
            time_listing_of_large_automations("prose", PROSE, warm_up).await;
            time_listing_of_large_automations("credentials", CREDENTIALS, warm_up).await;
        }
    }

    /// `cargo test --lib listing_speed_with_many_short_leaves -- --ignored --nocapture`:
    /// the other shape of a big automation — hundreds of short strings (a Quick
    /// Exec's arguments here, a workflow's steps elsewhere) rather than a few
    /// long ones. Masking walks every string, so each one is a call of its own.
    #[tokio::test]
    #[ignore = "timing measurement, run on demand"]
    async fn listing_speed_with_many_short_leaves() {
        isolate_config_dir();
        // Cold, then with the resources rendered ahead of the first read: the
        // two runs hold different text, so neither is served by the other's memo.
        for warm_up in [false, true] {
            let run = if warm_up { "warmed" } else { "cold" };
            let state = test_state();
            let root = tempfile::tempdir().unwrap();
            let project_id = "project-1".to_string();
            seed_project(&state, mk_project(&project_id, root.path())).await;
            state
                .db
                .with_conn(move |conn| {
                    for index in 0..100 {
                        let mut exec = sample_exec("project-1");
                        exec.id = format!("qe-{index}");
                        exec.name = format!("Automation exec {index} {run}");
                        exec.args = (0..500)
                            .map(|leaf| match leaf % 4 {
                                0 => format!("--option-{index}-{leaf}"),
                                1 => format!("value {index}/{leaf}"),
                                2 => format!("--label=step-{leaf}"),
                                _ => format!("https://example.com/{index}/{leaf}?page=2"),
                            })
                            .collect();
                        crate::db::quick_execs::insert_quick_exec(conn, &exec)?;
                    }
                    Ok::<_, anyhow::Error>(())
                })
                .await
                .unwrap();

            let warmed_in = if warm_up {
                Some(time_the_warm_up(&state, &project_id).await)
            } else {
                None
            };
            let mut runs = Vec::new();
            for _ in 0..3 {
                let started = std::time::Instant::now();
                let listing = list_resources(&state, &project_id).await;
                runs.push(started.elapsed());
                assert_eq!(listing.resources.len(), 100);
            }
            match warmed_in {
                None => eprintln!("LISTING_SPEED short leaves, first (cold) then repeated: {runs:?}"),
                Some(took) => eprintln!(
                    "LISTING_SPEED short leaves, first after the warm-up (which took {took:?}) then repeated: {runs:?}"
                ),
            }
        }
    }

    /// `cargo test --lib listing_speed_with_workflows_linking -- --ignored --nocapture`:
    /// the reference graph (`uses` / `used_by`) on top of the masking. 40
    /// workflows of 15 steps each point at 60 long prompts, 20 Quick Execs and
    /// one another, so every listing renders every workflow, walks its steps for
    /// references and, once published, reads its repository copy too.
    #[tokio::test]
    #[ignore = "timing measurement, run on demand"]
    async fn listing_speed_with_workflows_linking_many_prompts() {
        isolate_config_dir();
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root.path())
            .args(["init", "-q", "-b", "main"])
            .status()
            .unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        let sentences = [
            "Review the mapping between the shipping token budget and the pin of each dependency.",
            "Keep the answer short: list the files touched, then the tests run, then what is left.",
            "The password policy lives in the security page; use a connection, never paste credentials.",
            "Explain the trade-off first, then propose the smallest change; wait for approval before writing.",
        ];
        let prose = move |seed: usize, bytes: usize| {
            let mut text = String::new();
            let mut turn = seed;
            while text.len() < bytes {
                text.push_str(sentences[turn % sentences.len()]);
                text.push_str(&format!(" (step {turn})\n"));
                turn += 1;
            }
            text
        };
        state
            .db
            .with_conn(move |conn| {
                for index in 0..60 {
                    crate::db::quick_prompts::insert_quick_prompt(
                        conn,
                        &sample_prompt(
                            "project-1",
                            &format!("qp-{index}"),
                            &prose(index, 8 * 1024),
                        ),
                    )?;
                }
                for index in 0..20 {
                    let mut exec = sample_exec("project-1");
                    exec.id = format!("qe-{index}");
                    exec.name = format!("Automation exec {index}");
                    crate::db::quick_execs::insert_quick_exec(conn, &exec)?;
                }
                for workflow in 0..40 {
                    let mut steps: Vec<serde_json::Value> = (0..12)
                        .map(|step| {
                            serde_json::json!({
                                "name": format!("ask-{step}"),
                                "step_type": {"type": "Agent"},
                                "quick_prompt_id": format!("qp-{}", (workflow * 7 + step) % 60),
                                "prompt_template": prose(workflow + step, 1024),
                            })
                        })
                        .collect();
                    steps.push(serde_json::json!({
                        "name": "collect",
                        "step_type": {"type": "CollectApiData"},
                        "collect_api_data": {"sources": [
                            {"alias": "lint", "quick_exec_id": format!("qe-{}", workflow % 20)}
                        ]},
                    }));
                    for child in 1..=2 {
                        steps.push(serde_json::json!({
                            "name": format!("child-{child}"),
                            "step_type": {"type": "SubWorkflow"},
                            "sub_workflow_id": format!("wf-{}", (workflow + child) % 40),
                        }));
                    }
                    crate::db::workflows::insert_workflow(
                        conn,
                        &sample_workflow_json(
                            &format!("wf-{workflow}"),
                            &format!("Workflow {workflow}"),
                            "project-1",
                            serde_json::Value::Array(steps),
                        ),
                    )?;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();

        let time = |label: &'static str,
                    expected_state: Option<ProjectRepositoryResourceStatus>| {
            let state = state.clone();
            let project_id = project_id.clone();
            async move {
                let mut runs = Vec::new();
                for _ in 0..3 {
                    let started = std::time::Instant::now();
                    let listing = list_resources(&state, &project_id).await;
                    runs.push(started.elapsed());
                    assert_eq!(listing.resources.len(), 120);
                    let linked = listing
                        .resources
                        .iter()
                        .filter(|item| !item.uses.is_empty())
                        .count();
                    assert_eq!(linked, 40, "every workflow lists what it uses");
                    if let Some(expected) = expected_state {
                        assert!(listing.resources.iter().all(|item| item.status == expected));
                    }
                }
                eprintln!("LISTING_SPEED {label}, first (cold) then repeated: {runs:?}");
            }
        };
        time("workflows linking prompts, unpublished", None).await;

        for (kind, prefix, count) in [
            (ProjectRepositoryResourceKind::QuickPrompt, "qp", 60),
            (ProjectRepositoryResourceKind::QuickExec, "qe", 20),
            (ProjectRepositoryResourceKind::Workflow, "wf", 40),
        ] {
            for index in 0..count {
                let published = publish_repository_resource(
                    State(state.clone()),
                    AxumPath(project_id.clone()),
                    Json(PublishProjectRepositoryResourceRequest {
                        kind,
                        id: format!("{prefix}-{index}"),
                        overwrite_repository_changes: false,
                    }),
                )
                .await;
                assert!(published.0.data.is_some(), "{:?}", published.0.error);
            }
        }
        crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root.path())
            .args(["add", "-A"])
            .status()
            .unwrap();
        crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root.path())
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(["commit", "-q", "-m", "publish"])
            .status()
            .unwrap();
        read_every_file(root.path());
        time(
            "workflows linking prompts, all published",
            Some(ProjectRepositoryResourceStatus::UpToDate),
        )
        .await;
    }

    // ── KT-903: skills are real Agent Skills in `.agents/skills` ────────────

    fn git(root: &Path, args: &[&str]) {
        let status = crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root)
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn commit_count(root: &Path) -> usize {
        let output = crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(root)
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0)
    }

    async fn attach_skills(state: &crate::AppState, project_id: &str, ids: &[&str]) {
        let project_id = project_id.to_string();
        let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        state
            .db
            .with_conn(move |conn| {
                crate::db::projects::update_project_default_skills(conn, &project_id, &ids)
            })
            .await
            .unwrap();
    }

    fn publish_request(id: &str) -> Json<PublishProjectRepositoryResourceRequest> {
        Json(PublishProjectRepositoryResourceRequest {
            kind: ProjectRepositoryResourceKind::Skill,
            id: id.into(),
            overwrite_repository_changes: false,
        })
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn writing_a_skill_in_the_repository_makes_a_real_agent_skill_under_agents_skills() {
        isolate_config_dir();
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        // A skill written by hand in Kronn, without a description, and a
        // builtin one carrying a category, an icon and a license.
        let custom_id = crate::core::skills::save_custom_skill(
            "Reviewer Pro Kt903",
            "",
            "🔍",
            &crate::models::SkillCategory::Business,
            "Review carefully.\n\nSecond paragraph.",
            Some("MIT"),
            Some("Bash Read"),
        )
        .unwrap();
        attach_skills(&state, &project_id, &["rust", &custom_id]).await;

        for id in ["rust", custom_id.as_str()] {
            let response = publish_repository_resource(
                State(state.clone()),
                AxumPath(project_id.clone()),
                publish_request(id),
            )
            .await;
            let mutation = response.0.data.expect("publish succeeds");
            let path = root
                .path()
                .join(format!(".agents/skills/{}/SKILL.md", mutation.slug));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
                panic!("{id} must be written to .agents/skills/{}/", mutation.slug)
            });
            let file = crate::core::agent_skill::parse(&text).unwrap();
            crate::core::agent_skill::validate(&file, &mutation.slug)
                .unwrap_or_else(|error| panic!("{id} is not a valid Agent Skill: {error}\n{text}"));
            assert!(text.starts_with("---\nname: "), "{text}");
            assert!(
                !text.contains("kronn:resource"),
                "no Kronn blob in a real skill"
            );
            assert!(!file.body.is_empty());
        }
        assert!(
            !root.path().join("kronn/skills").exists(),
            "a skill is never written to kronn/skills"
        );

        let custom = std::fs::read_to_string(
            root.path()
                .join(".agents/skills/custom-reviewer-pro-kt903/SKILL.md"),
        )
        .unwrap();
        let file = crate::core::agent_skill::parse(&custom).unwrap();
        assert_eq!(
            file.description, "Reviewer Pro Kt903",
            "an empty description falls back to the name so the header stays valid"
        );
        assert_eq!(file.license.as_deref(), Some("MIT"));
        assert_eq!(file.allowed_tools.as_deref(), Some("Bash Read"));
        assert_eq!(file.metadata["kronn-name"], "Reviewer Pro Kt903");
        assert_eq!(file.metadata["kronn-icon"], "🔍");
        assert_eq!(file.metadata["kronn-category"], "business");
        assert!(file.body.contains("Second paragraph."));
        assert!(
            !custom.contains("\nicon:") && !custom.contains("\ncategory:"),
            "what is Kronn's own goes under metadata"
        );

        let lock = crate::core::repository_resources::load_lock(root.path())
            .unwrap()
            .unwrap();
        for entry in lock
            .resources
            .iter()
            .filter(|entry| entry.kind == ProjectRepositoryResourceKind::Skill)
        {
            assert_eq!(
                entry.paths,
                vec![format!(".agents/skills/{}/SKILL.md", entry.slug)]
            );
            assert!(lock.files.contains_key(&entry.paths[0]));
        }
        assert!(
            lock.files
                .keys()
                .all(|path| !path.starts_with("kronn/skills")),
            "{:?}",
            lock.files.keys().collect::<Vec<_>>()
        );

        let listing = list_resources(&state, &project_id).await;
        for slug in ["rust", "custom-reviewer-pro-kt903"] {
            let skill = listing
                .skills_present
                .iter()
                .find(|skill| skill.slug == slug)
                .unwrap();
            assert_eq!(
                skill.publication_path,
                format!(".agents/skills/{slug}/SKILL.md")
            );
            assert_eq!(skill.status, ProjectRepositoryResourceStatus::UpToDate);
            assert_eq!(
                skill.repository_paths,
                vec![format!(".agents/skills/{slug}/SKILL.md")]
            );
        }

        // Read back from the repository, the skill is what Kronn holds.
        let entry = lock
            .resources
            .iter()
            .find(|entry| entry.slug == "custom-reviewer-pro-kt903")
            .unwrap();
        let (document, _) =
            crate::core::repository_resources::read_resource(root.path(), entry).unwrap();
        let read: Skill = serde_json::from_value(document.resource).unwrap();
        assert_eq!(read.name, "Reviewer Pro Kt903");
        assert_eq!(read.icon, "🔍");
        assert_eq!(read.content, "Review carefully.\n\nSecond paragraph.");
        assert_eq!(read.license.as_deref(), Some("MIT"));
    }

    fn write_legacy_skill(root: &Path, skill: &Skill, slug: &str) -> (String, Vec<u8>) {
        let document = crate::core::repository_resources::RepositoryDocument {
            schema_version: crate::core::repository_resources::SCHEMA_VERSION,
            kind: ProjectRepositoryResourceKind::Skill,
            slug: slug.to_string(),
            updated_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            requires: Vec::new(),
            resource: serde_json::to_value(skill).unwrap(),
            redacted_fields: Vec::new(),
        };
        let text = format!(
            "---\nname: {slug}\ndescription: {}\n---\n\n<!-- kronn:resource\n{}\n-->\n\n{}\n",
            serde_json::to_string(&skill.description).unwrap(),
            serde_json::to_string_pretty(&document).unwrap(),
            skill.content
        );
        let relative = format!("kronn/skills/{slug}/SKILL.md");
        let path = root.join(&relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        (relative, text.into_bytes())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn skills_under_kronn_skills_migrate_with_lock_identity_and_approval_kept() {
        isolate_config_dir();
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-q"]);
        let skill = crate::core::skills::get_skill("rust").unwrap();
        let (legacy_path, legacy_bytes) = write_legacy_skill(root.path(), &skill, "rust");
        let lock = crate::core::repository_resources::RepositoryLock {
            version: crate::core::repository_resources::SCHEMA_VERSION,
            updated_at: Utc::now(),
            resources: vec![crate::core::repository_resources::RepositoryLockResource {
                kind: ProjectRepositoryResourceKind::Skill,
                slug: "rust".into(),
                name: skill.name.clone(),
                level: "N1".into(),
                paths: vec![legacy_path.clone()],
                sha256: "legacy".into(),
                required_secrets: Vec::new(),
            }],
            files: BTreeMap::from([(
                legacy_path.clone(),
                crate::core::repository_resources::sha256(&legacy_bytes),
            )]),
        };
        std::fs::write(
            root.path()
                .join(crate::core::repository_resources::LOCK_PATH),
            serde_json::to_vec_pretty(&lock).unwrap(),
        )
        .unwrap();
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-q", "-m", "legacy skills"]);
        let commits = commit_count(root.path());

        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        attach_skills(&state, &project_id, &["rust"]).await;
        let approval_hash = crate::core::repository_resources::approval_hash(
            &crate::core::repository_resources::render_skill(
                &skill,
                Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
                "rust",
            )
            .unwrap()
            .document,
        );
        let project_key = state
            .db
            .with_conn({
                let project_id = project_id.clone();
                let approval_hash = approval_hash.clone();
                move |conn| {
                    let key = crate::db::resource_identities::project_key(conn, Some(&project_id))?;
                    crate::db::resource_identities::upsert(conn, &key, "skill", "rust", "rust")?;
                    crate::db::repository_resources::upsert_alignment(
                        conn,
                        &key,
                        "skill",
                        "rust",
                        "rust",
                        "hash-in-the-former-format",
                        "hash-in-the-former-format",
                        "2026-09-01T00:00:00+00:00",
                        false,
                    )?;
                    crate::db::repository_resources::approve(
                        conn,
                        &key,
                        "skill",
                        "rust",
                        &approval_hash,
                    )?;
                    Ok::<_, anyhow::Error>(key)
                }
            })
            .await
            .unwrap();

        // Before: still readable where it is, proposed for the move, and never
        // presented as different only because its file format is older.
        let listing = list_resources(&state, &project_id).await;
        let before = listing
            .skills_present
            .iter()
            .find(|entry| entry.slug == "rust")
            .unwrap();
        assert_eq!(before.status, ProjectRepositoryResourceStatus::UpToDate);
        assert_eq!(before.repository_paths, vec![legacy_path.clone()]);
        assert_eq!(before.publication_path, ".agents/skills/rust/SKILL.md");
        let plan = skill_migration_plan(State(state.clone()), AxumPath(project_id.clone()))
            .await
            .0
            .data
            .unwrap();
        assert_eq!(plan.moves.len(), 1);
        assert_eq!(plan.moves[0].source, "kronn/skills/rust");
        assert_eq!(plan.moves[0].target, ".agents/skills/rust");
        assert!(plan.moves[0].converted && plan.moves[0].kronn_managed);
        assert!(
            root.path().join(&legacy_path).is_file(),
            "the recap writes nothing"
        );

        let response = migrate_skills(
            State(state.clone()),
            AxumPath(project_id.clone()),
            Json(crate::models::SkillMigrationRequest::default()),
        )
        .await;
        let result = response.0.data.expect("migration succeeds");
        assert_eq!(result.moved.len(), 1);
        assert!(result.unresolved.is_empty() && result.kept.is_empty());

        // No duplicate: the folder left `kronn/skills`, the standard skill is
        // in `.agents/skills`.
        assert!(!root.path().join("kronn/skills").exists());
        let text =
            std::fs::read_to_string(root.path().join(".agents/skills/rust/SKILL.md")).unwrap();
        let file = crate::core::agent_skill::parse(&text).unwrap();
        crate::core::agent_skill::validate(&file, "rust").unwrap();
        assert!(!text.contains("kronn:resource"));

        // The lock follows the skill.
        let lock = crate::core::repository_resources::load_lock(root.path())
            .unwrap()
            .unwrap();
        let entry = lock
            .resources
            .iter()
            .find(|entry| entry.slug == "rust")
            .unwrap();
        assert_eq!(
            entry.paths,
            vec![".agents/skills/rust/SKILL.md".to_string()]
        );
        assert_eq!(lock.resources.len(), 1);
        assert!(!lock.files.contains_key(&legacy_path));
        assert_eq!(
            lock.files[".agents/skills/rust/SKILL.md"],
            crate::core::repository_resources::sha256(text.as_bytes())
        );

        // The identity, the alignment and the approval follow it too.
        let (identity, alignment, approved) = state
            .db
            .with_read_conn({
                let project_key = project_key.clone();
                let approval_hash = approval_hash.clone();
                move |conn| {
                    Ok((
                        crate::db::resource_identities::lookup(
                            conn,
                            &project_key,
                            "skill",
                            "rust",
                        )?,
                        crate::db::repository_resources::find_alignment(
                            conn,
                            &project_key,
                            "skill",
                            "rust",
                        )?,
                        crate::db::repository_resources::is_approved(
                            conn,
                            &project_key,
                            "skill",
                            "rust",
                            &approval_hash,
                        )?,
                    ))
                }
            })
            .await
            .unwrap();
        assert_eq!(identity.as_deref(), Some("rust"));
        assert!(approved, "the approval by fingerprint does not change");
        let alignment = alignment.expect("the baseline is set against the new file");
        let (_, new_hash) =
            crate::core::repository_resources::read_resource(root.path(), entry).unwrap();
        assert_eq!(alignment.repository_hash, new_hash);
        assert_eq!(alignment.aligned_at, "2026-09-01T00:00:00+00:00");

        let listing = list_resources(&state, &project_id).await;
        let rows: Vec<_> = listing
            .skills_present
            .iter()
            .filter(|entry| entry.slug == "rust")
            .collect();
        assert_eq!(rows.len(), 1, "no duplicate row");
        assert_eq!(rows[0].status, ProjectRepositoryResourceStatus::UpToDate);
        assert_eq!(
            rows[0].repository_paths,
            vec![".agents/skills/rust/SKILL.md".to_string()]
        );

        // Nothing was committed; the banner counts the move.
        assert_eq!(commit_count(root.path()), commits);
        assert!(listing
            .uncommitted_managed_paths
            .contains(&".agents/skills/rust/SKILL.md".to_string()));
        assert!(
            listing.uncommitted_managed_paths.contains(&legacy_path),
            "{:?}",
            listing.uncommitted_managed_paths
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn publishing_a_skill_still_under_kronn_skills_moves_it_instead_of_duplicating_it() {
        isolate_config_dir();
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let skill = crate::core::skills::get_skill("rust").unwrap();
        let (legacy_path, legacy_bytes) = write_legacy_skill(root.path(), &skill, "rust");
        let lock = crate::core::repository_resources::RepositoryLock {
            version: crate::core::repository_resources::SCHEMA_VERSION,
            updated_at: Utc::now(),
            resources: vec![crate::core::repository_resources::RepositoryLockResource {
                kind: ProjectRepositoryResourceKind::Skill,
                slug: "rust".into(),
                name: skill.name.clone(),
                level: "N1".into(),
                paths: vec![legacy_path.clone()],
                sha256: "legacy".into(),
                required_secrets: Vec::new(),
            }],
            files: BTreeMap::from([(
                legacy_path.clone(),
                crate::core::repository_resources::sha256(&legacy_bytes),
            )]),
        };
        std::fs::write(
            root.path()
                .join(crate::core::repository_resources::LOCK_PATH),
            serde_json::to_vec_pretty(&lock).unwrap(),
        )
        .unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        attach_skills(&state, &project_id, &["rust"]).await;

        let response = publish_repository_resource(
            State(state.clone()),
            AxumPath(project_id.clone()),
            publish_request("rust"),
        )
        .await;
        assert!(response.0.data.is_some(), "{:?}", response.0.error);
        assert!(root.path().join(".agents/skills/rust/SKILL.md").is_file());
        assert!(
            !root.path().join("kronn/skills").exists(),
            "the former copy is removed, not left beside the new one"
        );
        let lock = crate::core::repository_resources::load_lock(root.path())
            .unwrap()
            .unwrap();
        assert!(lock
            .files
            .keys()
            .all(|path| !path.starts_with("kronn/skills")));

        // A former copy someone edited is never deleted without saying so.
        let root = tempfile::tempdir().unwrap();
        let (legacy_path, legacy_bytes) = write_legacy_skill(root.path(), &skill, "rust");
        let lock = crate::core::repository_resources::RepositoryLock {
            version: crate::core::repository_resources::SCHEMA_VERSION,
            updated_at: Utc::now(),
            resources: vec![crate::core::repository_resources::RepositoryLockResource {
                kind: ProjectRepositoryResourceKind::Skill,
                slug: "rust".into(),
                name: skill.name.clone(),
                level: "N1".into(),
                paths: vec![legacy_path.clone()],
                sha256: "legacy".into(),
                required_secrets: Vec::new(),
            }],
            files: BTreeMap::from([(
                legacy_path.clone(),
                crate::core::repository_resources::sha256(&legacy_bytes),
            )]),
        };
        std::fs::write(
            root.path()
                .join(crate::core::repository_resources::LOCK_PATH),
            serde_json::to_vec_pretty(&lock).unwrap(),
        )
        .unwrap();
        std::fs::write(root.path().join(&legacy_path), "edited by a human").unwrap();
        state
            .db
            .with_conn({
                let path = root.path().display().to_string();
                move |conn| {
                    conn.execute(
                        "UPDATE projects SET path = ?1 WHERE id = 'project-1'",
                        [path],
                    )?;
                    Ok::<_, anyhow::Error>(())
                }
            })
            .await
            .unwrap();
        let refused = publish_repository_resource(
            State(state.clone()),
            AxumPath(project_id.clone()),
            publish_request("rust"),
        )
        .await;
        assert!(
            refused
                .0
                .error
                .as_deref()
                .is_some_and(|error| error.contains("changed since the last alignment")),
            "{:?}",
            refused.0.error
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join(&legacy_path)).unwrap(),
            "edited by a human"
        );
        assert!(!root.path().join(".agents/skills/rust/SKILL.md").exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn migrate_all_lists_moves_and_conflicts_first_and_overwrites_nothing_without_a_choice() {
        isolate_config_dir();
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-q"]);
        write_native_skill(root.path(), ".claude/skills", "review", "Review", "Claude.");
        write_native_skill(root.path(), ".agents/skills", "review", "Review", "Agents.");
        write_native_skill(root.path(), ".gemini/skills", "lint", "Lint", "Lint body.");
        write_native_skill(
            root.path(),
            ".github/skills",
            "triage",
            "Triage",
            "Triage body.",
        );
        git(root.path(), &["add", "-A"]);
        git(root.path(), &["commit", "-q", "-m", "skills everywhere"]);
        let commits = commit_count(root.path());
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        // "Use in Kronn" pointed at the folder that is about to move.
        let used = use_native_skill(
            State(state.clone()),
            AxumPath(project_id.clone()),
            native_request(".gemini/skills/lint/SKILL.md", false),
        )
        .await;
        assert!(used.0.data.is_some());

        let plan = skill_migration_plan(State(state.clone()), AxumPath(project_id.clone()))
            .await
            .0
            .data
            .unwrap();
        let moves: Vec<_> = plan
            .moves
            .iter()
            .map(|entry| (entry.source.as_str(), entry.target.as_str()))
            .collect();
        assert_eq!(
            moves,
            vec![
                (".gemini/skills/lint", ".agents/skills/lint"),
                (".github/skills/triage", ".agents/skills/triage"),
            ]
        );
        assert_eq!(plan.conflicts.len(), 1);
        assert_eq!(plan.conflicts[0].slug, "review");
        assert_eq!(plan.conflicts[0].versions.len(), 2);
        assert!(
            root.path().join(".gemini/skills/lint/SKILL.md").is_file(),
            "the recap writes nothing"
        );

        // Without a choice, the conflict is skipped and nothing is overwritten.
        let response = migrate_skills(
            State(state.clone()),
            AxumPath(project_id.clone()),
            Json(crate::models::SkillMigrationRequest::default()),
        )
        .await;
        let result = response.0.data.expect("migration succeeds");
        assert_eq!(result.unresolved, vec!["review"]);
        assert_eq!(result.moved.len(), 2);
        assert!(
            std::fs::read_to_string(root.path().join(".agents/skills/review/SKILL.md"))
                .unwrap()
                .contains("Agents.")
        );
        assert!(root.path().join(".claude/skills/review/SKILL.md").is_file());
        assert!(root.path().join(".agents/skills/lint/SKILL.md").is_file());
        assert!(!root.path().join(".gemini/skills/lint").exists());

        // The read-only reference follows the skill.
        let reference = state
            .db
            .with_read_conn({
                let project_id = project_id.clone();
                move |conn| crate::db::project_skill_references::find(conn, &project_id, "lint")
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reference.relative_path, ".agents/skills/lint/SKILL.md");

        // The user chooses the Claude version for the conflict.
        let response = migrate_skills(
            State(state.clone()),
            AxumPath(project_id.clone()),
            Json(crate::models::SkillMigrationRequest {
                resolutions: vec![crate::models::SkillMigrationResolution {
                    slug: "review".into(),
                    keep: ".claude/skills/review".into(),
                }],
            }),
        )
        .await;
        let result = response.0.data.expect("second migration succeeds");
        assert_eq!(result.moved.len(), 1);
        assert!(result.unresolved.is_empty());
        assert!(
            std::fs::read_to_string(root.path().join(".agents/skills/review/SKILL.md"))
                .unwrap()
                .contains("Claude.")
        );
        assert!(!root.path().join(".claude/skills/review").exists());

        // Nothing was committed; the banner shows the work waiting.
        assert_eq!(commit_count(root.path()), commits);
        let listing = list_resources(&state, &project_id).await;
        for path in [
            ".agents/skills/lint/SKILL.md",
            ".agents/skills/triage/SKILL.md",
            ".agents/skills/review/SKILL.md",
            ".gemini/skills/lint/SKILL.md",
            ".claude/skills/review/SKILL.md",
        ] {
            assert!(
                listing
                    .uncommitted_managed_paths
                    .contains(&path.to_string()),
                "{path} missing from {:?}",
                listing.uncommitted_managed_paths
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn migrate_all_refuses_symbolic_links_and_paths_outside_the_repository() {
        isolate_config_dir();
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.md"), "outside").unwrap();
        write_native_skill(root.path(), ".claude/skills", "linked", "Linked", "Body.");
        std::os::unix::fs::symlink(
            outside.path().join("secret.md"),
            root.path().join(".claude/skills/linked/notes.md"),
        )
        .unwrap();
        write_native_skill(root.path(), ".claude/skills", "review", "Review", "Claude.");
        write_native_skill(root.path(), ".agents/skills", "review", "Review", "Agents.");
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;

        let plan = skill_migration_plan(State(state.clone()), AxumPath(project_id.clone()))
            .await
            .0
            .data
            .unwrap();
        assert_eq!(plan.blocked.len(), 1);
        assert_eq!(plan.blocked[0].path, ".claude/skills/linked");
        assert!(plan.moves.is_empty());

        let response = migrate_skills(
            State(state.clone()),
            AxumPath(project_id.clone()),
            Json(crate::models::SkillMigrationRequest {
                resolutions: vec![crate::models::SkillMigrationResolution {
                    slug: "review".into(),
                    keep: "../../etc".into(),
                }],
            }),
        )
        .await;
        let result = response.0.data.unwrap();
        assert_eq!(result.unresolved, vec!["review"]);
        assert!(root.path().join(".claude/skills/linked/SKILL.md").is_file());
        assert!(!root.path().join(".agents/skills/linked").exists());
        assert!(
            std::fs::read_to_string(root.path().join(".agents/skills/review/SKILL.md"))
                .unwrap()
                .contains("Agents.")
        );
        assert!(outside.path().join("secret.md").is_file());
    }

    #[tokio::test]
    async fn migrating_needs_a_known_project() {
        let state = test_state();
        let response = migrate_skills(
            State(state.clone()),
            AxumPath("missing".into()),
            Json(crate::models::SkillMigrationRequest::default()),
        )
        .await;
        assert!(response.0.data.is_none());
        let plan = skill_migration_plan(State(state), AxumPath("missing".into())).await;
        assert!(plan.0.data.is_none());
    }

    // ─── KT-917 — references survive publication and import ──────────────

    async fn publish_kind(
        state: &crate::AppState,
        project_id: &str,
        kind: ProjectRepositoryResourceKind,
        id: &str,
    ) -> String {
        let published = publish_repository_resource(
            State(state.clone()),
            AxumPath(project_id.to_string()),
            Json(PublishProjectRepositoryResourceRequest {
                kind,
                id: id.into(),
                overwrite_repository_changes: false,
            }),
        )
        .await;
        published.0.data.expect("published").slug
    }

    async fn import_kind(
        state: &crate::AppState,
        project_id: &str,
        kind: ProjectRepositoryResourceKind,
        slug: &str,
    ) -> Result<String, String> {
        let imported = import_repository_resource(
            State(state.clone()),
            AxumPath(project_id.to_string()),
            Json(ImportProjectRepositoryResourceRequest {
                kind,
                slug: slug.into(),
                overwrite_kronn_changes: false,
            }),
        )
        .await;
        match imported.0.data {
            Some(mutation) => Ok(mutation.id),
            None => Err(imported.0.error.unwrap_or_default()),
        }
    }

    #[tokio::test]
    async fn a_published_workflow_imported_into_a_blank_instance_targets_the_local_resources() {
        let origin = test_state();
        let root = tempfile::tempdir().unwrap();
        seed_project(&origin, mk_project("project-origin", root.path())).await;
        origin
            .db
            .with_conn(|conn| {
                crate::db::quick_prompts::insert_quick_prompt(
                    conn,
                    &sample_prompt_json("qp-origin", "Review PR", "project-origin"),
                )?;
                crate::db::workflows::insert_workflow(
                    conn,
                    &sample_workflow_json(
                        "wf-origin-child",
                        "Child Target",
                        "project-origin",
                        serde_json::json!([
                            {"name": "work", "step_type": {"type": "Agent"}, "prompt_template": "Work"}
                        ]),
                    ),
                )?;
                crate::db::workflows::insert_workflow(
                    conn,
                    &sample_workflow_json(
                        "wf-origin-parent",
                        "Parent",
                        "project-origin",
                        serde_json::json!([
                            {"name": "review", "step_type": {"type": "Agent"}, "quick_prompt_id": "qp-origin"},
                            {"name": "chain", "step_type": {"type": "TriggerWorkflow"}, "sub_workflow_id": "wf-origin-child"}
                        ]),
                    ),
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        let prompt_slug = publish_kind(
            &origin,
            "project-origin",
            ProjectRepositoryResourceKind::QuickPrompt,
            "qp-origin",
        )
        .await;
        let child_slug = publish_kind(
            &origin,
            "project-origin",
            ProjectRepositoryResourceKind::Workflow,
            "wf-origin-child",
        )
        .await;
        let parent_slug = publish_kind(
            &origin,
            "project-origin",
            ProjectRepositoryResourceKind::Workflow,
            "wf-origin-parent",
        )
        .await;
        let file = std::fs::read_to_string(
            root.path()
                .join(format!("kronn/workflows/{parent_slug}.yaml")),
        )
        .unwrap();
        assert!(file.contains("ref:workflow:child-target"), "{file}");
        assert!(file.contains("ref:prompt:review-pr"), "{file}");
        assert!(
            !file.contains("wf-origin-child") && !file.contains("qp-origin"),
            "{file}"
        );

        // Another machine: same repository, empty database.
        let blank = test_state();
        seed_project(&blank, mk_project("project-blank", root.path())).await;
        let refused = import_kind(
            &blank,
            "project-blank",
            ProjectRepositoryResourceKind::Workflow,
            &parent_slug,
        )
        .await
        .unwrap_err();
        assert!(refused.contains("ref:workflow:child-target"), "{refused}");
        let prompt_id = import_kind(
            &blank,
            "project-blank",
            ProjectRepositoryResourceKind::QuickPrompt,
            &prompt_slug,
        )
        .await
        .unwrap();
        let child_id = import_kind(
            &blank,
            "project-blank",
            ProjectRepositoryResourceKind::Workflow,
            &child_slug,
        )
        .await
        .unwrap();
        let parent_id = import_kind(
            &blank,
            "project-blank",
            ProjectRepositoryResourceKind::Workflow,
            &parent_slug,
        )
        .await
        .unwrap();
        assert_ne!(child_id, "wf-origin-child");
        let parent = blank
            .db
            .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &parent_id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            parent.steps[0].quick_prompt_id.as_deref(),
            Some(prompt_id.as_str())
        );
        assert_eq!(
            parent.steps[1].sub_workflow_id.as_deref(),
            Some(child_id.as_str())
        );

        // Rendered again, the imported workflow is the file it came from:
        // once approved it is aligned, and its execution approval holds.
        let approved = approve_repository_resource(
            State(blank.clone()),
            AxumPath("project-blank".to_string()),
            Json(crate::models::ApproveProjectRepositoryResourceRequest {
                kind: ProjectRepositoryResourceKind::Workflow,
                id: parent.id.clone(),
            }),
        )
        .await;
        assert!(approved.0.data.is_some(), "{:?}", approved.0.error);
        let listing = list_resources(&blank, "project-blank").await;
        let entry = listing
            .resources
            .iter()
            .find(|item| item.id == parent.id)
            .unwrap();
        assert_eq!(entry.status, ProjectRepositoryResourceStatus::UpToDate);
        blank
            .db
            .with_conn(move |conn| {
                crate::core::repository_resources::ensure_workflow_execution_approved(conn, &parent)
                    .map_err(anyhow::Error::msg)
            })
            .await
            .expect("approved for execution");
    }
}
