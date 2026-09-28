use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use axum::{
    extract::{Path as AxumPath, State},
    Json,
};
use chrono::{DateTime, NaiveDateTime, Utc};

use crate::models::{
    ApiErrorCode, ApiResponse, ProjectRepositoryResource, ProjectRepositoryResourceKind,
    ProjectRepositoryResourceLevel, ProjectRepositoryResourceStatus, ProjectRepositoryResources,
    ProjectRepositorySkill, ProjectRepositorySkillProvenance, Skill,
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
    updated_at: DateTime<Utc>,
}

impl ProjectRepositoryResourceKind {
    fn identity_kind(self) -> &'static str {
        match self {
            Self::Workflow => "workflow",
            Self::QuickPrompt => "quick_prompt",
            Self::QuickApi => "quick_api",
            Self::QuickExec => "quick_exec",
            Self::Artifact => "artifact",
        }
    }

    fn level(self) -> ProjectRepositoryResourceLevel {
        match self {
            Self::QuickPrompt | Self::QuickExec => {
                ProjectRepositoryResourceLevel::UsableWithoutKronn
            }
            Self::Workflow | Self::QuickApi | Self::Artifact => {
                ProjectRepositoryResourceLevel::KronnRequired
            }
        }
    }

    fn paths(self, slug: &str) -> Vec<String> {
        match self {
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
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|value| value.and_utc())
        })
}

fn repository_changed(root: &Path, paths: &[String], baseline: DateTime<Utc>) -> bool {
    paths.iter().any(|relative| {
        let Ok(metadata) = std::fs::metadata(root.join(relative)) else {
            return true;
        };
        metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|modified| modified.as_secs() > baseline.timestamp().max(0) as u64)
            .unwrap_or(false)
    })
}

fn alignment_status(
    root: &Path,
    paths: &[String],
    resource_updated_at: DateTime<Utc>,
    identity_updated_at: Option<&str>,
) -> ProjectRepositoryResourceStatus {
    let Some(baseline) = identity_updated_at.and_then(parse_identity_time) else {
        return ProjectRepositoryResourceStatus::NotPublished;
    };
    if !paths.iter().any(|relative| root.join(relative).is_file()) {
        return ProjectRepositoryResourceStatus::NotPublished;
    }
    let db_changed = resource_updated_at > baseline;
    let repo_changed = repository_changed(root, paths, baseline);
    match (repo_changed, db_changed) {
        (false, false) => ProjectRepositoryResourceStatus::UpToDate,
        (true, false) => ProjectRepositoryResourceStatus::RepositoryModified,
        (false, true) => ProjectRepositoryResourceStatus::KronnModified,
        (true, true) => ProjectRepositoryResourceStatus::Conflict,
    }
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
    conn: &rusqlite::Connection,
    project_id: &str,
    project_key: &str,
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
        let status = if is_linked {
            let identity = crate::db::resource_identities::find_by_target(
                conn,
                project_key,
                "skill",
                &skill.id,
            )?;
            match identity {
                Some(identity) => Some(alignment_status(
                    root,
                    std::slice::from_ref(&publication_path),
                    parse_identity_time(&identity.updated_at).unwrap_or_else(Utc::now),
                    Some(&identity.updated_at),
                )),
                None if !root.join(&publication_path).is_file() => {
                    Some(ProjectRepositoryResourceStatus::NotPublished)
                }
                None => None,
            }
        } else {
            None
        };
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
                        updated_at: item.updated_at,
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
                        updated_at: item.updated_at,
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
                        updated_at: item.updated_at,
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
                        updated_at: item.updated_at,
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
                        updated_at: item.updated_at,
                    }),
            );

            let root = PathBuf::from(&project.path);
            let (skills_present, skills_available) = project_skills(
                conn,
                &project_id,
                &project_key,
                &root,
                &project.default_skill_ids,
            )?;
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
                let status = alignment_status(
                    &root,
                    &repository_paths,
                    seed.updated_at,
                    identity.as_ref().map(|item| item.updated_at.as_str()),
                );
                resources.push(ProjectRepositoryResource {
                    id: seed.id,
                    name: seed.name,
                    slug,
                    kind: seed.kind,
                    level: seed.kind.level(),
                    status,
                    repository_paths,
                });
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::fs::File;
    use std::time::Duration;

    fn set_modified(path: &Path, seconds: u64) {
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn unpublished_resource_has_no_alignment_baseline() {
        assert_eq!(
            alignment_status(
                Path::new("/missing"),
                &["kronn/x.yaml".into()],
                Utc::now(),
                None
            ),
            ProjectRepositoryResourceStatus::NotPublished,
        );
    }

    #[test]
    fn imported_identity_without_repository_files_is_still_unpublished() {
        let baseline = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        assert_eq!(
            alignment_status(
                Path::new("/missing"),
                &["kronn/x.yaml".into()],
                baseline,
                Some(&baseline.to_rfc3339()),
            ),
            ProjectRepositoryResourceStatus::NotPublished,
        );
    }

    #[test]
    fn alignment_status_distinguishes_repository_kronn_and_conflicting_changes() {
        let directory = tempfile::TempDir::new().unwrap();
        let relative = "kronn/workflows/report.yaml";
        let path = directory.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "name: report\n").unwrap();

        let baseline_seconds = 1_700_000_000;
        let baseline = Utc.timestamp_opt(baseline_seconds, 0).unwrap();
        let changed = Utc.timestamp_opt(baseline_seconds + 10, 0).unwrap();
        let identity = baseline.to_rfc3339();

        set_modified(&path, baseline_seconds as u64);
        assert_eq!(
            alignment_status(
                directory.path(),
                &[relative.into()],
                baseline,
                Some(&identity)
            ),
            ProjectRepositoryResourceStatus::UpToDate,
        );
        assert_eq!(
            alignment_status(
                directory.path(),
                &[relative.into()],
                changed,
                Some(&identity)
            ),
            ProjectRepositoryResourceStatus::KronnModified,
        );

        set_modified(&path, (baseline_seconds + 10) as u64);
        assert_eq!(
            alignment_status(
                directory.path(),
                &[relative.into()],
                baseline,
                Some(&identity)
            ),
            ProjectRepositoryResourceStatus::RepositoryModified,
        );
        assert_eq!(
            alignment_status(
                directory.path(),
                &[relative.into()],
                changed,
                Some(&identity)
            ),
            ProjectRepositoryResourceStatus::Conflict,
        );
    }
}
