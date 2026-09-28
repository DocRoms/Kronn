use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as AxumPath, State},
    Json,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::models::{
    ApiErrorCode, ApiResponse, ApproveProjectRepositoryResourceRequest, ArtifactBundlePage,
    CreateLivePageDataset, ImportProjectRepositoryResourceRequest, LivePage, LivePageRevision,
    ProjectRepositoryResource, ProjectRepositoryResourceKind, ProjectRepositoryResourceLevel,
    ProjectRepositoryResourceMutation, ProjectRepositoryResourceStatus, ProjectRepositoryResources,
    ProjectRepositorySkill, ProjectRepositorySkillProvenance,
    PublishProjectRepositoryResourceRequest, QuickApi, QuickExec, QuickPrompt, Skill,
    UpdateLivePageRequest, Workflow,
};
use crate::AppState;

const PROJECT_SKILL_ROOTS: &[&str] = &[
    "kronn/skills",
    ".claude/skills",
    ".agents/skills",
    ".vibe/skills",
    ".kiro/skills",
    ".gemini/skills",
];

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
    fn identity_kind(self) -> &'static str {
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

struct AlignmentView {
    status: ProjectRepositoryResourceStatus,
    approval_required: bool,
    approved: bool,
    diff: Option<String>,
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
    let status = match alignment {
        None if repository_hash.is_some() => ProjectRepositoryResourceStatus::RepositoryModified,
        None => ProjectRepositoryResourceStatus::NotPublished,
        Some(alignment) => {
            let repository_changed = repository_hash != Some(alignment.repository_hash.as_str());
            let database_changed = rendered.hash != alignment.database_hash;
            match (repository_changed, database_changed) {
                (false, false) => ProjectRepositoryResourceStatus::UpToDate,
                (true, false) => ProjectRepositoryResourceStatus::RepositoryModified,
                (false, true) => ProjectRepositoryResourceStatus::KronnModified,
                (true, true) => ProjectRepositoryResourceStatus::Conflict,
            }
        }
    };
    let approved = match alignment {
        Some(alignment) if alignment.imported => crate::db::repository_resources::is_approved(
            conn,
            &alignment.project_key,
            &alignment.kind,
            &alignment.slug,
            &crate::core::repository_resources::approval_hash(&rendered.document),
        )?,
        Some(_) => true,
        None => false,
    };
    let diff = (status == ProjectRepositoryResourceStatus::Conflict)
        .then(|| {
            let path = entry.paths.first()?;
            let repository = std::fs::read(root.join(path)).ok()?;
            let kronn = rendered.files.values().next()?;
            Some(crate::core::repository_resources::simple_diff(
                &repository,
                kronn,
            ))
        })
        .flatten();
    Ok(AlignmentView {
        status,
        approval_required: alignment.is_some_and(|alignment| alignment.imported) && !approved,
        approved,
        diff,
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

fn take_repository_skill(
    repository_skills: &mut BTreeMap<String, RepositorySkillSeed>,
    skill_id: &str,
) -> Option<RepositorySkillSeed> {
    let slug = crate::core::native_files::slug(skill_id);
    let mut seed = repository_skills.remove(&slug);
    if let Some(custom_slug) = skill_id.strip_prefix("custom-") {
        if let Some(other) = repository_skills.remove(custom_slug) {
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

fn project_skills(
    project_id: &str,
    root: &Path,
    linked_skill_ids: &[String],
) -> anyhow::Result<(Vec<ProjectRepositorySkill>, Vec<ProjectRepositorySkill>)> {
    let catalog = crate::core::skills::list_all_skills();
    let mut repository_skills = discover_repository_skills(root);

    for detected_id in crate::api::audit::detect_project_skills(root) {
        let slug = crate::core::native_files::slug(&detected_id);
        let detected_name = catalog
            .iter()
            .find(|skill| skill.id == detected_id)
            .map(|skill| skill.name.clone())
            .unwrap_or_else(|| detected_id.clone());
        let seed = repository_skills.entry(slug).or_default();
        if seed.name.is_empty() {
            seed.name = detected_name;
        }
    }

    let linked: BTreeSet<&str> = linked_skill_ids.iter().map(String::as_str).collect();
    let mut handled = BTreeSet::new();
    let mut present = Vec::new();
    let mut available = Vec::new();

    for skill in catalog {
        let repository = take_repository_skill(&mut repository_skills, &skill.id);
        let is_linked = linked.contains(skill.id.as_str());
        handled.insert(skill.id.clone());
        let slug = crate::core::native_files::slug(&skill.id);
        let publication_path = format!("kronn/skills/{slug}/SKILL.md");
        let repository_paths = repository
            .as_ref()
            .map(|seed| seed.repository_paths.clone())
            .unwrap_or_default();
        let provenance = match (repository.is_some(), is_linked) {
            (true, true) => ProjectRepositorySkillProvenance::Both,
            (true, false) => ProjectRepositorySkillProvenance::Repository,
            (false, _) => ProjectRepositorySkillProvenance::Kronn,
        };
        let status = is_linked.then_some(if root.join(&publication_path).is_file() {
            ProjectRepositoryResourceStatus::RepositoryModified
        } else {
            ProjectRepositoryResourceStatus::NotPublished
        });
        let item = project_skill_item(
            skill,
            provenance,
            status,
            repository_paths,
            publication_path,
        );
        if repository.is_some() || is_linked {
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
        let repository = take_repository_skill(&mut repository_skills, skill_id);
        let publication_path = format!("kronn/skills/{slug}/SKILL.md");
        present.push(ProjectRepositorySkill {
            id: skill_id.clone(),
            name: repository
                .as_ref()
                .map(|seed| seed.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| skill_id.clone()),
            slug,
            description: String::new(),
            provenance: if repository.is_some() {
                ProjectRepositorySkillProvenance::Both
            } else {
                ProjectRepositorySkillProvenance::Kronn
            },
            is_builtin: None,
            status: (!root.join(&publication_path).is_file())
                .then_some(ProjectRepositoryResourceStatus::NotPublished),
            approval_required: false,
            approved: false,
            diff: None,
            repository_paths: repository
                .map(|seed| seed.repository_paths)
                .unwrap_or_default(),
            publication_path,
        });
    }

    for (slug, seed) in repository_skills {
        present.push(ProjectRepositorySkill {
            id: format!("repository:{slug}"),
            name: if seed.name.is_empty() {
                slug.clone()
            } else {
                seed.name
            },
            description: String::new(),
            provenance: ProjectRepositorySkillProvenance::Repository,
            is_builtin: None,
            status: None,
            approval_required: false,
            approved: false,
            diff: None,
            repository_paths: seed.repository_paths,
            publication_path: format!("kronn/skills/{slug}/SKILL.md"),
            slug,
        });
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

fn project_skill_item(
    skill: Skill,
    provenance: ProjectRepositorySkillProvenance,
    status: Option<ProjectRepositoryResourceStatus>,
    repository_paths: Vec<String>,
    publication_path: String,
) -> ProjectRepositorySkill {
    ProjectRepositorySkill {
        id: skill.id.clone(),
        name: skill.name,
        slug: crate::core::native_files::slug(&skill.id),
        description: skill.description,
        provenance,
        is_builtin: Some(skill.is_builtin),
        status,
        approval_required: false,
        approved: false,
        diff: None,
        repository_paths,
        publication_path,
    }
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
            let lock =
                crate::core::repository_resources::load_lock(&root).map_err(anyhow::Error::msg)?;
            let (mut skills_present, skills_available) =
                project_skills(&project_id, &root, &project.default_skill_ids)?;
            for skill in &mut skills_present {
                let Some(entry) = lock.as_ref().and_then(|lock| {
                    lock.resources.iter().find(|entry| {
                        entry.kind == ProjectRepositoryResourceKind::Skill
                            && entry.slug == skill.slug
                    })
                }) else {
                    continue;
                };
                if skill.id.starts_with("repository:") {
                    skill.status = Some(ProjectRepositoryResourceStatus::RepositoryModified);
                    skill.repository_paths = entry.paths.clone();
                    continue;
                }
                let alignment = crate::db::repository_resources::find_alignment(
                    conn,
                    &project_key,
                    "skill",
                    &skill.slug,
                )?;
                let timestamp = alignment
                    .as_ref()
                    .and_then(|alignment| parse_identity_time(&alignment.aligned_at));
                let rendered = render_database_resource(
                    conn,
                    ProjectRepositoryResourceKind::Skill,
                    &skill.id,
                    &skill.slug,
                    timestamp,
                )?;
                let view = alignment_status(conn, &root, entry, &rendered, alignment.as_ref())?;
                skill.status = Some(view.status);
                skill.approval_required = view.approval_required;
                skill.approved = view.approved;
                skill.diff = view.diff;
                skill.repository_paths = entry.paths.clone();
            }
            let mut resources = Vec::with_capacity(seeds.len());
            for seed in seeds {
                let identity = crate::db::resource_identities::find_by_target(
                    conn,
                    &project_key,
                    seed.kind.identity_kind(),
                    &seed.id,
                )?;
                let generated_slug = || {
                    let slug = crate::core::mcp_scanner::slugify_label(&seed.name);
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
                    .or(seed.slug)
                    .unwrap_or_else(generated_slug);
                let repository_paths = seed.kind.paths(&slug);
                let rendered = render_database_resource(conn, seed.kind, &seed.id, &slug, None)?;
                let alignment = crate::db::repository_resources::find_alignment(
                    conn,
                    &project_key,
                    seed.kind.identity_kind(),
                    &slug,
                )?;
                let entry = lock_entry(
                    lock.as_ref(),
                    seed.kind,
                    &slug,
                    &seed.name,
                    &repository_paths,
                );
                let view = alignment_status(conn, &root, &entry, &rendered, alignment.as_ref())?;
                resources.push(ProjectRepositoryResource {
                    id: seed.id,
                    name: seed.name,
                    slug,
                    kind: seed.kind,
                    level: seed.kind.level(),
                    status: view.status,
                    approval_required: view.approval_required,
                    approved: view.approved,
                    diff: view.diff,
                    repository_paths: entry.paths,
                });
            }
            if let Some(lock) = lock.as_ref() {
                for entry in &lock.resources {
                    if entry.kind == ProjectRepositoryResourceKind::Skill
                        || resources.iter().any(|resource| {
                            resource.kind == entry.kind && resource.slug == entry.slug
                        })
                    {
                        continue;
                    }
                    resources.push(ProjectRepositoryResource {
                        id: format!("repository:{}:{}", entry.kind.identity_kind(), entry.slug),
                        name: entry.name.clone(),
                        slug: entry.slug.clone(),
                        kind: entry.kind,
                        level: entry.kind.level(),
                        status: ProjectRepositoryResourceStatus::RepositoryModified,
                        approval_required: false,
                        approved: false,
                        diff: None,
                        repository_paths: entry.paths.clone(),
                    });
                }
            }
            resources.sort_by_key(|resource| resource.name.to_lowercase());
            Ok(Some(ProjectRepositoryResources {
                kronn_exists: root.join("kronn").is_dir(),
                skills_present,
                skills_available,
                resources,
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
        let slug = crate::core::mcp_scanner::slugify_label(&name);
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
    Ok(ProjectRepositoryResourceMutation {
        kind: request.kind,
        id: request.id,
        slug,
        status: ProjectRepositoryResourceStatus::UpToDate,
        approved: true,
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn hash_alignment_distinguishes_all_four_published_states() {
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
        assert_eq!(
            alignment_status(&conn, root.path(), &entry, &rendered, Some(&alignment))
                .unwrap()
                .status,
            ProjectRepositoryResourceStatus::UpToDate
        );

        let path = root.path().join(&entry.paths[0]);
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"repository edit").unwrap();
        assert_eq!(
            alignment_status(&conn, root.path(), &entry, &rendered, Some(&alignment))
                .unwrap()
                .status,
            ProjectRepositoryResourceStatus::RepositoryModified
        );

        std::fs::write(&path, original).unwrap();
        let mut changed = base;
        changed.description = "database edit".into();
        let changed =
            crate::core::repository_resources::render_quick_exec(&changed, "lint").unwrap();
        assert_eq!(
            alignment_status(&conn, root.path(), &entry, &changed, Some(&alignment))
                .unwrap()
                .status,
            ProjectRepositoryResourceStatus::KronnModified
        );

        std::fs::write(&path, b"repository edit").unwrap();
        let view =
            alignment_status(&conn, root.path(), &entry, &changed, Some(&alignment)).unwrap();
        assert_eq!(view.status, ProjectRepositoryResourceStatus::Conflict);
        assert!(view.diff.unwrap().contains("--- repository"));
    }
}
