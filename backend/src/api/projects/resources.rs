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
    RepositoryNativeSkillRequest, RepositoryResourceComparison, RepositoryResourceFileDiff,
    ResourceAdrLevel, Skill, UpdateLivePageRequest, Workflow,
};
use crate::AppState;

const PROJECT_SKILL_ROOTS: &[&str] = &[
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

struct ResourceSeed {
    id: String,
    name: String,
    slug: Option<String>,
    kind: ProjectRepositoryResourceKind,
}

fn render_database_resource(
    conn: &rusqlite::Connection,
    kind: ProjectRepositoryResourceKind,
    id: &str,
    slug: &str,
    skill_updated_at: Option<DateTime<Utc>>,
) -> anyhow::Result<crate::core::repository_resources::RenderedRepositoryResource> {
    let rendered = match kind {
        ProjectRepositoryResourceKind::Skill => {
            let skill = crate::core::skills::get_skill(id)
                .ok_or_else(|| anyhow::anyhow!("Skill not found: {id}"))?;
            crate::core::repository_resources::render_skill(
                &skill,
                skill_updated_at.unwrap_or_else(Utc::now),
                slug,
            )
        }
        ProjectRepositoryResourceKind::Workflow => {
            let workflow = crate::db::workflows::get_workflow(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Workflow not found: {id}"))?;
            crate::core::repository_resources::render_workflow(&workflow, slug)
        }
        ProjectRepositoryResourceKind::QuickPrompt => {
            let prompt = crate::db::quick_prompts::get_quick_prompt(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Quick Prompt not found: {id}"))?;
            crate::core::repository_resources::render_quick_prompt(&prompt, slug)
        }
        ProjectRepositoryResourceKind::QuickApi => {
            let api = crate::db::quick_apis::get_quick_api(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Quick API not found: {id}"))?;
            crate::core::repository_resources::render_quick_api(&api, slug)
        }
        ProjectRepositoryResourceKind::QuickExec => {
            let exec = crate::db::quick_execs::get_quick_exec(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Quick Exec not found: {id}"))?;
            crate::core::repository_resources::render_quick_exec(&exec, slug)
        }
        ProjectRepositoryResourceKind::Artifact => {
            let detail = crate::db::live_pages::get_live_page(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Artifact not found: {id}"))?;
            let artifact = crate::api::artifact_portability::export_page(conn, id)?;
            crate::core::repository_resources::render_artifact(
                &artifact,
                detail.page.updated_at,
                slug,
            )
        }
    }
    .map_err(anyhow::Error::msg)?;
    Ok(rendered)
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
            Self::Skill => vec![format!("kronn/skills/{slug}/SKILL.md")],
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

fn parse_identity_time(value: &str) -> Option<DateTime<Utc>> {
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

fn read_repository_file(root: &Path, relative: &str) -> Option<Vec<u8>> {
    let path = root.join(relative);
    crate::core::fs_guard::assert_contained_no_symlink(root, &path).ok()?;
    std::fs::read(path).ok()
}

/// One unified diff per file the resource is written to, repository side
/// against Kronn side; a file present on one side only diffs against nothing.
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
            let diff =
                crate::core::repository_resources::unified_diff(&repository_bytes, kronn_bytes);
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

/// Both sides of one resource, compared: a unified diff per file and, for the
/// kinds that have one, a field-by-field diff of the definition. Only the
/// states where the two sides differ have anything to show. Built on demand —
/// it reads the repository files and diffs the masked Kronn rendering, work the
/// listing never does.
fn resource_comparison(
    root: &Path,
    entry: &crate::core::repository_resources::RepositoryLockResource,
    rendered: &crate::core::repository_resources::RenderedRepositoryResource,
    sync_status: ProjectRepositoryResourceStatus,
) -> RepositoryResourceComparison {
    let needs_diff = matches!(
        sync_status,
        ProjectRepositoryResourceStatus::RepositoryNewer
            | ProjectRepositoryResourceStatus::KronnNewer
            | ProjectRepositoryResourceStatus::Conflict
    );
    if !needs_diff {
        return RepositoryResourceComparison::default();
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
                crate::core::repository_resources::field_diff(
                    &document.resource,
                    &rendered.document.resource,
                )
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    RepositoryResourceComparison {
        diff,
        file_diffs,
        field_diff,
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
    let repository_hash = repository.as_ref().map(|(_, hash)| hash.as_str());
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
        let publication_path = format!(
            "kronn/skills/{}/SKILL.md",
            crate::core::native_files::slug(&skill.id)
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
        let publication_path = format!("kronn/skills/{slug}/SKILL.md");
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
            format!("kronn/skills/{slug}/SKILL.md"),
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
    let alignment =
        crate::db::repository_resources::find_alignment(conn, project_key, "skill", slug)?;
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

fn resolve_resource(
    conn: &rusqlite::Connection,
    project_key: &str,
    lock: Option<&crate::core::repository_resources::RepositoryLock>,
    seed: &ResourceSeed,
) -> anyhow::Result<ResolvedResource> {
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
    let slug = identity
        .as_ref()
        .map(|item| item.slug.clone())
        .or_else(|| seed.slug.clone())
        .unwrap_or_else(generated_slug);
    let repository_paths = seed.kind.paths(&slug);
    let rendered = render_database_resource(conn, seed.kind, &seed.id, &slug, None)?;
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
            let mut seeds = Vec::new();
            seeds.extend(
                crate::db::workflows::list_workflows(conn)?
                    .into_iter()
                    .filter(|item| item.project_id.as_deref() == Some(project_id.as_str()))
                    .map(|item| ResourceSeed {
                        id: item.id,
                        name: item.name,
                        slug: None,
                        kind: ProjectRepositoryResourceKind::Workflow,
                    }),
            );
            seeds.extend(
                crate::db::quick_prompts::list_quick_prompts(conn)?
                    .into_iter()
                    .filter(|item| item.project_id.as_deref() == Some(project_id.as_str()))
                    .map(|item| ResourceSeed {
                        id: item.id,
                        name: item.name,
                        slug: None,
                        kind: ProjectRepositoryResourceKind::QuickPrompt,
                    }),
            );
            seeds.extend(
                crate::db::quick_apis::list_quick_apis(conn)?
                    .into_iter()
                    .filter(|item| item.project_id.as_deref() == Some(project_id.as_str()))
                    .map(|item| ResourceSeed {
                        id: item.id,
                        name: item.name,
                        slug: None,
                        kind: ProjectRepositoryResourceKind::QuickApi,
                    }),
            );
            seeds.extend(
                crate::db::quick_execs::list_quick_execs(conn)?
                    .into_iter()
                    .filter(|item| item.project_id.as_deref() == Some(project_id.as_str()))
                    .map(|item| ResourceSeed {
                        id: item.id,
                        name: item.name,
                        slug: None,
                        kind: ProjectRepositoryResourceKind::QuickExec,
                    }),
            );
            seeds.extend(
                crate::db::live_pages::list_live_pages_for_project(conn, &project_id)?
                    .into_iter()
                    .map(|item| ResourceSeed {
                        id: item.id,
                        name: item.title,
                        slug: Some(item.slug),
                        kind: ProjectRepositoryResourceKind::Artifact,
                    }),
            );

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
            // its file was written under and what its definition points at.
            let mut repository_side: HashMap<ResourceKey, ResourceReferences> = HashMap::new();
            for entry in lock.iter().flat_map(|lock| lock.resources.iter()) {
                if entry.kind == ProjectRepositoryResourceKind::Skill {
                    continue;
                }
                let Ok((document, _)) =
                    crate::core::repository_resources::read_resource(&root, entry)
                else {
                    continue;
                };
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
                let view = alignment_status(conn, &root, &entry, &rendered, alignment.as_ref())?;
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
                for entry in &lock.resources {
                    if entry.kind == ProjectRepositoryResourceKind::Skill
                        || listed.contains(&(entry.kind.identity_kind(), entry.slug.clone()))
                    {
                        continue;
                    }
                    let (repository_updated_at, repository_updated_by) = primary_diff_path(entry)
                        .map(|path| dates.updated_at(path))
                        .unwrap_or((None, None));
                    let repository_file =
                        crate::core::repository_resources::read_resource(&root, entry);
                    let repository_fingerprint = repository_file
                        .as_ref()
                        .ok()
                        .map(|(_, hash)| crate::core::repository_resources::fingerprint(hash));
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
                            .map(|(document, _)| {
                                crate::core::repository_resources::resource_adr_level(&document)
                            })
                            .unwrap_or_else(|_| parse_adr_level(&entry.level)),
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
            let uncommitted_managed_paths = lock
                .as_ref()
                .map(|lock| {
                    crate::core::repository_resources::uncommitted_managed_paths(&root, lock)
                })
                .unwrap_or_default();
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
        ProjectRepositoryResourceKind::Workflow => crate::db::workflows::get_workflow(conn, id)?
            .filter(|item| ours(item.project_id.as_deref()))
            .map(|item| ResourceSeed {
                id: item.id,
                name: item.name,
                slug: None,
                kind,
            }),
        ProjectRepositoryResourceKind::QuickPrompt => {
            crate::db::quick_prompts::get_quick_prompt(conn, id)?
                .filter(|item| ours(item.project_id.as_deref()))
                .map(|item| ResourceSeed {
                    id: item.id,
                    name: item.name,
                    slug: None,
                    kind,
                })
        }
        ProjectRepositoryResourceKind::QuickApi => crate::db::quick_apis::get_quick_api(conn, id)?
            .filter(|item| ours(item.project_id.as_deref()))
            .map(|item| ResourceSeed {
                id: item.id,
                name: item.name,
                slug: None,
                kind,
            }),
        ProjectRepositoryResourceKind::QuickExec => {
            crate::db::quick_execs::get_quick_exec(conn, id)?
                .filter(|item| ours(item.project_id.as_deref()))
                .map(|item| ResourceSeed {
                    id: item.id,
                    name: item.name,
                    slug: None,
                    kind,
                })
        }
        ProjectRepositoryResourceKind::Artifact => crate::db::live_pages::get_live_page(conn, id)?
            .map(|detail| detail.page)
            .filter(|page| ours(page.project_id.as_deref()))
            .map(|page| ResourceSeed {
                id: page.id,
                name: page.title,
                slug: Some(page.slug),
                kind,
            }),
    })
}

/// GET /api/projects/:id/repository-resources/comparison?kind=…&id=…
///
/// The diffs behind one row of the listing, built when its Compare sheet
/// opens. They come from the same masked rendering a publish would write, so
/// no secret value can be read here that the repository files would not hold.
/// A resource with only one side, or with both in sync, has nothing to compare
/// and answers an empty comparison.
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
                // Only a skill attached in Kronn that `kronn.lock` also lists
                // has two sides to compare; a native one is a single file.
                let slug = crate::core::native_files::slug(&query.id);
                let entry = lock.as_ref().and_then(|lock| {
                    lock.resources.iter().find(|entry| {
                        entry.kind == ProjectRepositoryResourceKind::Skill && entry.slug == slug
                    })
                });
                let Some(entry) = entry.filter(|_| !query.id.starts_with("repository:")) else {
                    return Ok(Some(RepositoryResourceComparison::default()));
                };
                let AlignedSkill { rendered, view } =
                    align_skill(conn, &root, &project_key, &query.id, &slug, entry)?;
                return Ok(Some(resource_comparison(
                    &root,
                    entry,
                    &rendered,
                    view.sync_status,
                )));
            }
            let Some(seed) = seed_of(conn, &project_id, query.kind, &query.id)? else {
                // A repository-only row (`repository:kind:slug`) has no Kronn
                // side; anything else is unknown to this project.
                return Ok(query
                    .id
                    .starts_with("repository:")
                    .then(RepositoryResourceComparison::default));
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
    let exported: ArtifactBundlePage = serde_json::from_value(document.resource.clone())?;
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
            let skill: Skill = serde_json::from_value(document.resource.clone())?;
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
            let mut resource: Workflow = serde_json::from_value(document.resource.clone())?;
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
            crate::api::workflows::rebind_api_configs(conn, &mut resource.steps, Some(project_id));
            crate::api::workflows::rebind_api_configs(
                conn,
                &mut resource.on_failure,
                Some(project_id),
            );
            if existing_id.is_some() {
                crate::db::workflows::update_workflow(conn, &resource)?;
            } else {
                resource.created_at = now;
                crate::db::workflows::insert_workflow(conn, &resource)?;
            }
            resource.id
        }
        ProjectRepositoryResourceKind::QuickPrompt => {
            let mut resource: QuickPrompt = serde_json::from_value(document.resource.clone())?;
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
            let mut resource: QuickApi = serde_json::from_value(document.resource.clone())?;
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
            let mut resource: QuickExec = serde_json::from_value(document.resource.clone())?;
            resource.id = existing_id
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            resource.project_id = Some(project_id.to_string());
            resource.updated_at = now;
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
/// project's native skill roots — not `kronn/skills`, which Kronn manages, and
/// not the router skill Kronn itself writes under `.agents/skills/kronn`.
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
/// repository is untouched: publishing the copy into `kronn/skills/` stays a
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
        };
        let mut rendered =
            crate::core::repository_resources::render_quick_exec(&exec, "health").unwrap();
        rendered.files = BTreeMap::from([
            (
                "kronn/artifacts/health/artifact.yaml".to_string(),
                b"same\n".to_vec(),
            ),
            (
                "kronn/artifacts/health/index.html".to_string(),
                b"<p>new</p>\n".to_vec(),
            ),
        ]);
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
        std::env::set_var("KRONN_DATA_DIR", &dir);
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
        crate::core::cmd::sync_cmd("git")
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
        crate::core::cmd::sync_cmd("git")
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
        let mut child = crate::core::cmd::sync_cmd("git")
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
        crate::core::cmd::sync_cmd("git")
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
        crate::core::cmd::sync_cmd("git")
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
        crate::core::cmd::sync_cmd("git")
            .arg("-C")
            .arg(root.path())
            .args(["add", "-A"])
            .status()
            .unwrap();
        crate::core::cmd::sync_cmd("git")
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

    /// 60 prompts of about 8 KB and 20 Quick Execs, all Kronn-side (nothing
    /// published, so no diff to build): the listing of the first read, then of
    /// the reads that follow.
    async fn time_listing_of_large_automations(
        label: &'static str,
        sentences: &'static [&'static str],
    ) {
        isolate_config_dir();
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        state
            .db
            .with_conn(move |conn| {
                for index in 0..60 {
                    let mut template = format!("# {label} automation {index}\n\n");
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
                    exec.name = format!("Automation exec {index}");
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
            assert_eq!(listing.resources.len(), 80);
        }
        eprintln!("LISTING_SPEED {label}, first (cold) then repeated: {runs:?}");
    }

    /// `cargo test --lib listing_speed_with_many_large -- --ignored --nocapture`:
    /// a project with many automations holding long prompts, the shape that made
    /// the listing take seconds because every read masked all of them again.
    /// The second flavour is the worst case for the masking: a credential
    /// assignment on most lines, so no pattern is turned away cheaply.
    #[tokio::test]
    #[ignore = "timing measurement, run on demand"]
    async fn listing_speed_with_many_large_automations() {
        time_listing_of_large_automations(
            "prose",
            &[
                "Review the mapping between the shipping token budget and the pin of each dependency.",
                "Keep the answer short: list the files touched, then the tests run, then what is left.",
                "The password policy lives in the security page; use a connection, never paste credentials.",
                "Open https://example.com/docs/guide and compare it with the notes: key: value, one per line.",
                "Explain the trade-off first, then propose the smallest change; wait for approval before writing.",
                "Return JSON with the fields status, summary and next_steps; no prose outside the object.",
            ],
        )
        .await;
        time_listing_of_large_automations(
            "credentials",
            &[
                "Export API_KEY=${SERVICE_KEY} before the run; the token: ${DEPLOY_TOKEN} comes from the vault.",
                "Send Authorization: Bearer ${GITHUB_TOKEN} in the header and never log password=${DB_PASSWORD}.",
                "curl -H 'x-api-key: ${SERVICE_KEY}' https://example.com/v1/items and keep the reply short.",
                "Connect with postgres://app:${DB_PASSWORD}@db.internal/app, then list the tables that changed.",
                "Explain the trade-off first, then propose the smallest change; wait for approval before writing.",
                "Return JSON with the fields status, summary and next_steps; no prose outside the object.",
            ],
        )
        .await;
    }

    /// `cargo test --lib listing_speed_with_many_short_leaves -- --ignored --nocapture`:
    /// the other shape of a big automation — hundreds of short strings (a Quick
    /// Exec's arguments here, a workflow's steps elsewhere) rather than a few
    /// long ones. Masking walks every string, so each one is a call of its own.
    #[tokio::test]
    #[ignore = "timing measurement, run on demand"]
    async fn listing_speed_with_many_short_leaves() {
        isolate_config_dir();
        let state = test_state();
        let root = tempfile::tempdir().unwrap();
        let project_id = "project-1".to_string();
        seed_project(&state, mk_project(&project_id, root.path())).await;
        state
            .db
            .with_conn(|conn| {
                for index in 0..100 {
                    let mut exec = sample_exec("project-1");
                    exec.id = format!("qe-{index}");
                    exec.name = format!("Automation exec {index}");
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

        let mut runs = Vec::new();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let listing = list_resources(&state, &project_id).await;
            runs.push(started.elapsed());
            assert_eq!(listing.resources.len(), 100);
        }
        eprintln!("LISTING_SPEED short leaves, first (cold) then repeated: {runs:?}");
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
        crate::core::cmd::sync_cmd("git")
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
        crate::core::cmd::sync_cmd("git")
            .arg("-C")
            .arg(root.path())
            .args(["add", "-A"])
            .status()
            .unwrap();
        crate::core::cmd::sync_cmd("git")
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
}
