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
};
use crate::AppState;

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
