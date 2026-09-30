//! The skills the projects use that a project's `default_skill_ids` do not
//! tell (KT-921): the native skills "Use in Kronn" pointed at (KT-897) and the
//! skills `kronn.lock` lists because Kronn published them.
//!
//! The Automation page reads all projects at once, so this is the light
//! sibling of `repository_resources`: two small reads per project (the
//! references table and the lock file), no rendering, no comparison, no scan of
//! the repository's skill folders.

use std::collections::BTreeMap;
use std::path::Path;

use axum::{
    extract::{Path as AxumPath, Query, State},
    Json,
};

use super::resources::{read_repository_file, side_text};
use crate::db::project_skill_references::SkillReference;
use crate::models::{
    ApiErrorCode, ApiResponse, ProjectRepositoryResourceKind, ProjectSkillFile, ProjectUsedSkill,
};
use crate::AppState;

/// Catalog skill id by the slug Kronn writes it under: how a published skill
/// is told from one only the repository holds.
fn catalog_ids_by_slug() -> BTreeMap<String, String> {
    let mut ids = BTreeMap::new();
    for skill in crate::core::skills::list_all_skills() {
        ids.entry(crate::core::native_files::slug(&skill.id))
            .or_insert(skill.id);
    }
    ids
}

/// `.agents/skills` out of `.agents/skills/<slug>/SKILL.md`.
fn skill_root(relative_path: &str) -> String {
    Path::new(relative_path)
        .parent()
        .and_then(Path::parent)
        .map(|root| root.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The skills `project_id` uses beyond its attached ones, in a stable order:
/// what its references point at, then what its lock says Kronn published. A
/// published skill the catalog knows is that catalog skill; one it does not
/// know, or a reference, is a skill only the repository holds — and a
/// reference and a lock entry naming the same slug there are one skill.
pub(super) fn project_used_skills(
    project_id: &str,
    root: &Path,
    references: &[SkillReference],
    catalog: &BTreeMap<String, String>,
) -> Vec<ProjectUsedSkill> {
    let mut used: Vec<ProjectUsedSkill> = references
        .iter()
        .map(|reference| ProjectUsedSkill {
            project_id: project_id.to_string(),
            skill_id: None,
            slug: reference.slug.clone(),
            name: reference.name.clone(),
            root: skill_root(&reference.relative_path),
            relative_path: reference.relative_path.clone(),
            referenced: true,
            published: false,
        })
        .collect();

    // A lock that cannot be read leaves the references alone: one broken
    // repository must not hide what the others use.
    let lock = crate::core::repository_resources::load_lock(root)
        .inspect_err(|error| tracing::debug!(project_id, %error, "used skills: lock unreadable"))
        .ok()
        .flatten();
    let published = lock
        .iter()
        .flat_map(|lock| lock.resources.iter())
        .filter(|entry| entry.kind == ProjectRepositoryResourceKind::Skill);
    for entry in published {
        // A published skill keeps its native path unless only the former
        // `kronn/skills` location is left.
        let Some(relative_path) = entry
            .paths
            .iter()
            .find(|path| !crate::core::repository_resources::is_legacy_skill_path(path))
            .or_else(|| entry.paths.first())
        else {
            continue;
        };
        let skill_id = catalog.get(&entry.slug).cloned();
        let same_repository_skill = used
            .iter_mut()
            .find(|skill| skill.skill_id.is_none() && skill.slug == entry.slug);
        match (skill_id, same_repository_skill) {
            (None, Some(skill)) => skill.published = true,
            (skill_id, _) => used.push(ProjectUsedSkill {
                project_id: project_id.to_string(),
                skill_id,
                slug: entry.slug.clone(),
                name: entry.name.clone(),
                root: skill_root(relative_path),
                relative_path: relative_path.clone(),
                referenced: false,
                published: true,
            }),
        }
    }
    used
}

/// GET /api/projects/used-skills
///
/// Every project's used skills that `default_skill_ids` do not tell, for the
/// Automation page's Skills type.
pub async fn used_skills(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<ProjectUsedSkill>>> {
    let result = state
        .db
        .with_read_conn(|conn| {
            let projects = crate::db::projects::list_projects(conn)?;
            let mut references: BTreeMap<String, Vec<SkillReference>> = BTreeMap::new();
            for reference in crate::db::project_skill_references::list_all(conn)? {
                references
                    .entry(reference.project_id.clone())
                    .or_default()
                    .push(reference);
            }
            let catalog = catalog_ids_by_slug();
            Ok(projects
                .iter()
                .flat_map(|project| {
                    project_used_skills(
                        &project.id,
                        Path::new(&project.path),
                        references.get(&project.id).map_or(&[], Vec::as_slice),
                        &catalog,
                    )
                })
                .collect::<Vec<_>>())
        })
        .await;
    match result {
        Ok(skills) => Json(ApiResponse::ok(skills)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to list the skills projects use: {error}"),
        )),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct UsedSkillFileQuery {
    pub relative_path: String,
}

/// GET /api/projects/:id/repository-resources/skills/content?relative_path=…
///
/// The `SKILL.md` a project uses, read from its repository when its sheet
/// opens. Only a file the project uses is served — a reference or a published
/// skill — never an arbitrary path, and the text is masked and cut like every
/// other repository text the API carries.
pub async fn used_skill_file(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Query(query): Query<UsedSkillFileQuery>,
) -> Json<ApiResponse<ProjectSkillFile>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            let Some(project) = crate::db::projects::get_project(conn, &project_id)? else {
                return Ok(None);
            };
            let root = Path::new(&project.path);
            let references =
                crate::db::project_skill_references::list_for_project(conn, &project_id)?;
            let uses_it =
                project_used_skills(&project_id, root, &references, &catalog_ids_by_slug())
                    .iter()
                    .any(|skill| skill.relative_path == query.relative_path);
            if !uses_it {
                return Ok(None);
            }
            let bytes = read_repository_file(root, &query.relative_path)
                .ok_or_else(|| anyhow::anyhow!("cannot read {}", query.relative_path))?;
            let (content, truncated) = side_text(&bytes);
            Ok(Some(ProjectSkillFile {
                relative_path: query.relative_path,
                content,
                truncated,
            }))
        })
        .await;
    match result {
        Ok(Some(file)) => Json(ApiResponse::ok(file)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Project or skill not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to read the skill: {error}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::repository_resources::{RepositoryLock, RepositoryLockResource};

    fn write_skill(root: &Path, relative_root: &str, slug: &str, name: &str, body: &str) {
        let dir = root.join(relative_root).join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Moves a block.\n---\n{body}\n"),
        )
        .unwrap();
    }

    fn reference(project_id: &str, slug: &str, relative_path: &str, name: &str) -> SkillReference {
        SkillReference {
            project_id: project_id.into(),
            slug: slug.into(),
            relative_path: relative_path.into(),
            name: name.into(),
            created_at: "2026-09-30T00:00:00Z".into(),
        }
    }

    fn write_lock(root: &Path, skills: &[(&str, &str, &[&str])]) {
        let lock = RepositoryLock {
            resources: skills
                .iter()
                .map(|(slug, name, paths)| RepositoryLockResource {
                    kind: ProjectRepositoryResourceKind::Skill,
                    slug: (*slug).into(),
                    name: (*name).into(),
                    level: "n1".into(),
                    paths: paths.iter().map(|path| (*path).into()).collect(),
                    sha256: "0".repeat(64),
                    required_secrets: vec![],
                })
                .collect(),
            ..RepositoryLock::default()
        };
        std::fs::create_dir_all(root.join("kronn")).unwrap();
        std::fs::write(
            root.join(crate::core::repository_resources::LOCK_PATH),
            serde_json::to_vec(&lock).unwrap(),
        )
        .unwrap();
    }

    fn catalog(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(slug, id)| ((*slug).into(), (*id).into()))
            .collect()
    }

    #[test]
    fn a_referenced_native_skill_is_used_with_the_folder_it_lives_in() {
        let root = tempfile::tempdir().unwrap();
        write_skill(
            root.path(),
            ".agents/skills",
            "block-migration",
            "Block migration",
            "Steps.",
        );
        let references = [reference(
            "p1",
            "block-migration",
            ".agents/skills/block-migration/SKILL.md",
            "Block migration",
        )];
        let used = project_used_skills("p1", root.path(), &references, &catalog(&[]));
        assert_eq!(
            used,
            vec![ProjectUsedSkill {
                project_id: "p1".into(),
                skill_id: None,
                slug: "block-migration".into(),
                name: "Block migration".into(),
                root: ".agents/skills".into(),
                relative_path: ".agents/skills/block-migration/SKILL.md".into(),
                referenced: true,
                published: false,
            }]
        );
    }

    #[test]
    fn a_published_skill_is_the_catalog_skill_when_the_catalog_knows_its_slug() {
        let root = tempfile::tempdir().unwrap();
        write_lock(
            root.path(),
            &[
                ("review", "Review", &[".agents/skills/review/SKILL.md"]),
                (
                    "only-here",
                    "Only here",
                    &[
                        "kronn/skills/only-here/SKILL.md",
                        ".agents/skills/only-here/SKILL.md",
                    ],
                ),
            ],
        );
        let used = project_used_skills("p1", root.path(), &[], &catalog(&[("review", "review")]));
        assert_eq!(used.len(), 2);
        assert_eq!(used[0].skill_id.as_deref(), Some("review"));
        assert!(used[0].published && !used[0].referenced);
        assert_eq!(used[0].root, ".agents/skills");
        // Not in the catalog: a skill only the repository holds, at its native
        // path rather than the former Kronn location.
        assert_eq!(used[1].skill_id, None);
        assert_eq!(used[1].relative_path, ".agents/skills/only-here/SKILL.md");
    }

    #[test]
    fn a_reference_and_a_lock_entry_naming_one_repository_skill_are_one_skill() {
        let root = tempfile::tempdir().unwrap();
        write_lock(
            root.path(),
            &[(
                "block-migration",
                "Block migration",
                &[".agents/skills/block-migration/SKILL.md"],
            )],
        );
        let references = [reference(
            "p1",
            "block-migration",
            ".claude/skills/block-migration/SKILL.md",
            "Block migration",
        )];
        let used = project_used_skills("p1", root.path(), &references, &catalog(&[]));
        assert_eq!(used.len(), 1);
        assert!(used[0].referenced && used[0].published);
        // The reference is read from where the user pointed it.
        assert_eq!(
            used[0].relative_path,
            ".claude/skills/block-migration/SKILL.md"
        );
    }

    #[test]
    fn an_unreadable_lock_leaves_the_references_alone() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("kronn")).unwrap();
        std::fs::write(root.path().join("kronn/kronn.lock"), b"{not json").unwrap();
        let references = [reference(
            "p1",
            "review",
            ".agents/skills/review/SKILL.md",
            "Review",
        )];
        let used = project_used_skills("p1", root.path(), &references, &catalog(&[]));
        assert_eq!(used.len(), 1);
        assert!(used[0].referenced);
    }

    async fn state_with_project(root: &Path) -> AppState {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        let state = AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
        let now = chrono::Utc::now();
        let project = crate::models::Project {
            id: "p1".into(),
            name: "front_euronews".into(),
            path: root.display().to_string(),
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
        };
        state
            .db
            .with_conn(move |conn| {
                crate::db::projects::insert_project(conn, &project)?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .expect("insert project");
        state
    }

    #[tokio::test]
    async fn the_route_lists_a_skill_used_from_the_repository_and_serves_its_skill_md() {
        let root = tempfile::tempdir().unwrap();
        write_skill(
            root.path(),
            ".agents/skills",
            "block-migration",
            "Block migration",
            "Move the block.",
        );
        let state = state_with_project(root.path()).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::project_skill_references::upsert(
                    conn,
                    "p1",
                    "block-migration",
                    ".agents/skills/block-migration/SKILL.md",
                    "Block migration",
                    "2026-09-30T00:00:00Z",
                )
            })
            .await
            .unwrap();

        let listed = used_skills(State(state.clone()))
            .await
            .0
            .data
            .expect("listing succeeds");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].project_id, "p1");
        assert_eq!(listed[0].root, ".agents/skills");

        let file = used_skill_file(
            State(state.clone()),
            AxumPath("p1".into()),
            Query(UsedSkillFileQuery {
                relative_path: listed[0].relative_path.clone(),
            }),
        )
        .await
        .0
        .data
        .expect("the file is served");
        assert!(file.content.contains("Move the block."));
        assert!(!file.truncated);
    }

    #[tokio::test]
    async fn the_route_serves_no_file_a_project_does_not_use() {
        let root = tempfile::tempdir().unwrap();
        write_skill(
            root.path(),
            ".agents/skills",
            "unused",
            "Unused",
            "Not referenced.",
        );
        std::fs::write(root.path().join("secret.txt"), "top secret").unwrap();
        let state = state_with_project(root.path()).await;

        for path in [
            ".agents/skills/unused/SKILL.md",
            "secret.txt",
            "../secret.txt",
        ] {
            let response = used_skill_file(
                State(state.clone()),
                AxumPath("p1".into()),
                Query(UsedSkillFileQuery {
                    relative_path: path.into(),
                }),
            )
            .await;
            assert!(response.0.data.is_none(), "{path} must not be served");
        }
        let unknown_project = used_skill_file(
            State(state.clone()),
            AxumPath("nope".into()),
            Query(UsedSkillFileQuery {
                relative_path: ".agents/skills/unused/SKILL.md".into(),
            }),
        )
        .await;
        assert!(unknown_project.0.data.is_none());
    }

    #[tokio::test]
    async fn the_served_skill_md_is_masked_like_every_repository_text() {
        const TYPED_KEY: &str = "sk-zyxwvutsrqponmlkjihgfedcba9876543210";
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".agents/skills/leaky");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: Leaky\ndescription: d\n---\nkey {TYPED_KEY}\n"),
        )
        .unwrap();
        let state = state_with_project(root.path()).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::project_skill_references::upsert(
                    conn,
                    "p1",
                    "leaky",
                    ".agents/skills/leaky/SKILL.md",
                    "Leaky",
                    "2026-09-30T00:00:00Z",
                )
            })
            .await
            .unwrap();
        let file = used_skill_file(
            State(state),
            AxumPath("p1".into()),
            Query(UsedSkillFileQuery {
                relative_path: ".agents/skills/leaky/SKILL.md".into(),
            }),
        )
        .await
        .0
        .data
        .expect("the file is served");
        assert!(!file.content.contains(TYPED_KEY));
        assert!(file.content.contains("***REDACTED***"));
    }
}
