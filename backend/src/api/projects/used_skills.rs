//! The skills the projects use that a project's `default_skill_ids` do not
//! tell (KT-921): the native skills "Use in Kronn" pointed at (KT-897) and the
//! skills `kronn.lock` lists because Kronn published them.
//!
//! The Automation page reads all projects at once, so this is the light
//! sibling of `repository_resources`: small reads per project (the references
//! table and the lock file), no scan of the repository's skill folders. Only
//! the Kronn skills it lists are rendered, for the sync state the card shows.

use std::collections::BTreeMap;
use std::path::Path;

use axum::{
    extract::{Path as AxumPath, Query, State},
    Json,
};

use super::resources::{read_repository_file, side_text};
use crate::db::project_skill_references::SkillReference;
use crate::core::repository_skills::{
    catalog_ids_by_slug, catalog_ids_by_slug_of, project_used_skills, skill_root,
};
use crate::models::{
    ApiErrorCode, ApiResponse, ProjectRepositoryResourceStatus, ProjectSkillFile,
    ProjectUsedSkill, Skill,
};
use crate::AppState;
pub use crate::core::repository_skills::{
    parse_repository_skill_id, repository_skills_for_discussion, resolve_repository_skills,
    resolve_repository_skills_from, RepositorySkillProblem, RepositorySkillSource,
    RepositorySkills, UnresolvedRepositorySkill, MAX_INJECTED_SKILL_BYTES,
};
#[cfg(all(test, feature = "full-tests"))]
use crate::core::repository_skills::read_at_commit;

/// `used` with each Kronn skill's sync state, the project card's own (KT-1140),
/// plus the skills the project links that the repository already holds a file
/// for: the card can call those out of sync too. A state that cannot be
/// computed is logged and left out rather than failing the whole listing.
fn with_sync_states(
    conn: &rusqlite::Connection,
    project: &crate::models::Project,
    catalog: &[Skill],
    mut used: Vec<ProjectUsedSkill>,
) -> anyhow::Result<Vec<ProjectUsedSkill>> {
    let root = Path::new(&project.path);
    let project_key = crate::db::resource_identities::project_key(conn, Some(&project.id))?;
    let lock = crate::core::repository_resources::load_lock(root)
        .ok()
        .flatten();
    let mut linked: Vec<&str> = project
        .default_skill_ids
        .iter()
        .map(String::as_str)
        .collect();
    linked.extend(
        catalog
            .iter()
            .filter(|skill| skill.project_id.as_deref() == Some(project.id.as_str()))
            .map(|skill| skill.id.as_str()),
    );
    let status_of = |skill_id: &str, is_linked: bool| {
        super::resources::skill_sync_status(
            conn,
            root,
            &project_key,
            skill_id,
            is_linked,
            lock.as_ref(),
        )
        .inspect_err(|error| {
            tracing::debug!(project_id = %project.id, skill_id, %error, "used skills: sync state unknown")
        })
        .ok()
        .flatten()
    };
    for skill in &mut used {
        if let Some(skill_id) = skill.skill_id.as_deref() {
            skill.sync_status = status_of(skill_id, linked.contains(&skill_id));
        }
    }
    for skill_id in linked {
        if used
            .iter()
            .any(|skill| skill.skill_id.as_deref() == Some(skill_id))
        {
            continue;
        }
        let status = status_of(skill_id, true);
        if matches!(
            status,
            None | Some(ProjectRepositoryResourceStatus::KronnOnly)
        ) {
            continue;
        }
        let slug = crate::core::native_files::slug(skill_id);
        let relative_path = crate::core::repository_resources::skill_path(&slug);
        let name = catalog
            .iter()
            .find(|skill| skill.id == skill_id)
            .map_or_else(|| skill_id.to_string(), |skill| skill.name.clone());
        used.push(ProjectUsedSkill {
            project_id: project.id.clone(),
            skill_id: Some(skill_id.to_string()),
            slug,
            name,
            root: skill_root(&relative_path),
            relative_path,
            referenced: false,
            published: false,
            sync_status: status,
        });
    }
    Ok(used)
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
            let skills = crate::core::skills::list_all_skills();
            let catalog = catalog_ids_by_slug_of(&skills);
            let mut listed = Vec::new();
            for project in &projects {
                let used = project_used_skills(
                    &project.id,
                    Path::new(&project.path),
                    references.get(&project.id).map_or(&[], Vec::as_slice),
                    &catalog,
                );
                listed.extend(with_sync_states(conn, project, &skills, used)?);
            }
            Ok(listed)
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

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::models::ProjectRepositoryResourceKind;
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
                sync_status: None,
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
        state_with_project_attaching(root, &[]).await
    }

    async fn state_with_project_attaching(root: &Path, default_skill_ids: &[&str]) -> AppState {
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
            default_skill_ids: default_skill_ids.iter().map(|id| (*id).into()).collect(),
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
    async fn the_route_reports_each_kronn_skill_sync_state_as_the_project_card_does() {
        let root = tempfile::tempdir().unwrap();
        // Published, then edited in the repository with no baseline: a conflict.
        write_skill(
            root.path(),
            ".agents/skills",
            "rust",
            "Rust",
            "Edited here.",
        );
        write_lock(
            root.path(),
            &[("rust", "Rust", &[".agents/skills/rust/SKILL.md"])],
        );
        // Attached, never published, yet the repository holds its file.
        write_skill(root.path(), ".agents/skills", "typescript", "TS", "Theirs.");
        let state = state_with_project_attaching(root.path(), &["typescript", "python"]).await;

        let listed = used_skills(State(state.clone()))
            .await
            .0
            .data
            .expect("listing succeeds");
        let status_of = |id: &str| {
            listed
                .iter()
                .find(|skill| skill.skill_id.as_deref() == Some(id))
                .map(|skill| skill.sync_status)
        };
        assert_eq!(
            status_of("rust"),
            Some(Some(ProjectRepositoryResourceStatus::Conflict))
        );
        assert_eq!(
            status_of("typescript"),
            Some(Some(ProjectRepositoryResourceStatus::RepositoryNewer))
        );
        // Attached with nothing in the repository: nothing to report.
        assert_eq!(status_of("python"), None);

        let card = super::super::resources::repository_resources(
            State(state.clone()),
            AxumPath("p1".into()),
        )
        .await
        .0
        .data
        .expect("the card's listing succeeds");
        for id in ["rust", "typescript"] {
            let on_card = card
                .skills_present
                .iter()
                .find(|skill| skill.id == id)
                .map(|skill| skill.status);
            assert_eq!(
                on_card,
                status_of(id).flatten(),
                "{id}: card and Automation agree"
            );
        }
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

    // ─── Discussions (KT-923) ────────────────────────────────────────────────

    const BLOCK_MIGRATION: &str = ".agents/skills/block-migration/SKILL.md";

    async fn reference_block_migration(state: &AppState) {
        state
            .db
            .with_conn(|conn| {
                crate::db::project_skill_references::upsert(
                    conn,
                    "p1",
                    "block-migration",
                    BLOCK_MIGRATION,
                    "Block migration",
                    "2026-09-30T00:00:00Z",
                )
            })
            .await
            .unwrap();
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_string()).collect()
    }

    fn git(root: &Path, args: &[&str]) {
        let status = crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn write_block_migration(root: &Path, body: &str) {
        let dir = root.join(".agents/skills/block-migration");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: block-migration\ndescription: Moves a block.\n---\n{body}\n"),
        )
        .unwrap();
    }

    async fn from_default_branch(state: &AppState, project: Option<&str>) -> RepositorySkills {
        let project = project.map(str::to_string);
        state
            .db
            .with_read_conn(move |conn| {
                resolve_repository_skills_from(
                    conn,
                    project.as_deref(),
                    &ids(&["repository:p1:block-migration"]),
                    RepositorySkillSource::DefaultBranch,
                )
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_workflow_reads_a_repository_skill_from_the_default_branch_not_the_checkout() {
        let root = tempfile::tempdir().unwrap();
        let state = state_with_project(root.path()).await;
        reference_block_migration(&state).await;
        git(root.path(), &["init", "-q", "-b", "main"]);
        git(root.path(), &["config", "user.email", "t@example.com"]);
        git(root.path(), &["config", "user.name", "t"]);

        write_block_migration(root.path(), "Uncommitted.");
        let skills = from_default_branch(&state, Some("p1")).await;
        assert!(skills.resolved.is_empty());
        assert_eq!(
            skills.unresolved[0].problem,
            RepositorySkillProblem::NotOnDefaultBranch
        );
        assert!(skills
            .notice()
            .unwrap()
            .contains("not committed on the default branch"));

        write_block_migration(root.path(), "Committed on main.");
        git(root.path(), &["add", "--", "."]);
        git(root.path(), &["commit", "-q", "-m", "skill"]);
        git(root.path(), &["checkout", "-q", "-b", "feature"]);
        write_block_migration(root.path(), "Edited on a feature branch.");

        let skills = from_default_branch(&state, Some("p1")).await;
        assert_eq!(skills.resolved.len(), 1, "{:?}", skills.unresolved);
        assert!(skills.resolved[0].content.contains("Committed on main."));
        assert!(!skills.resolved[0].content.contains("feature branch"));
        let elsewhere = from_default_branch(&state, Some("p2")).await;
        assert_eq!(
            elsewhere.unresolved[0].problem,
            RepositorySkillProblem::OtherProject
        );
    }

    fn git_out(root: &Path, args: &[&str]) -> String {
        let output = crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn the_default_branch_is_the_remote_head_and_never_an_older_local_copy() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path();
        let state = state_with_project(repo).await;
        reference_block_migration(&state).await;
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "t@example.com"]);
        git(repo, &["config", "user.name", "t"]);
        write_block_migration(repo, "MAIN-BODY");
        git(repo, &["add", "--", "."]);
        git(repo, &["commit", "-q", "-m", "main"]);
        let main = git_out(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "-b", "develop"]);
        write_block_migration(repo, "DEVELOP-BODY");
        git(repo, &["commit", "-q", "-am", "develop"]);
        let develop = git_out(repo, &["rev-parse", "HEAD"]);
        git(repo, &["rm", "-q", "-r", ".agents"]);
        git(repo, &["commit", "-q", "-m", "drop the skill"]);
        let dropped = git_out(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "-b", "feature", &main]);
        write_block_migration(repo, "CHECKOUT-BODY");
        let loaded =
            |skills: RepositorySkills| skills.resolved.first().map(|skill| skill.content.clone());

        // The remote's own default wins over a local `main`, whatever its name.
        for (name, commit, body) in [
            ("develop", &develop, "DEVELOP-BODY"),
            ("release", &main, "MAIN-BODY"),
            ("stable", &develop, "DEVELOP-BODY"),
        ] {
            git(
                repo,
                &["update-ref", &format!("refs/remotes/origin/{name}"), commit],
            );
            git(
                repo,
                &[
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    &format!("refs/remotes/origin/{name}"),
                ],
            );
            let content = loaded(from_default_branch(&state, Some("p1")).await).unwrap_or_default();
            assert!(content.contains(body), "{name}: {content}");
        }

        // The remote dropped the skill: an older local `main` still holding it is not the default branch.
        git(
            repo,
            &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"],
        );
        git(repo, &["update-ref", "refs/remotes/origin/main", &dropped]);
        let skills = from_default_branch(&state, Some("p1")).await;
        assert_eq!(loaded(skills), None);
        assert_eq!(
            from_default_branch(&state, Some("p1")).await.unresolved[0].problem,
            RepositorySkillProblem::NotOnDefaultBranch
        );

        // A remote default that names no commit is no default branch either.
        git(
            repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/gone",
            ],
        );
        assert_eq!(loaded(from_default_branch(&state, Some("p1")).await), None);
    }

    #[test]
    fn a_default_branch_read_never_leaves_the_repository() {
        let root = tempfile::tempdir().unwrap();
        for path in [
            "",
            "/etc/passwd",
            "../outside/SKILL.md",
            "a//b",
            "./SKILL.md",
        ] {
            assert_eq!(read_at_commit(root.path(), "HEAD", path), None, "{path}");
        }
    }

    #[test]
    fn a_repository_skill_id_names_its_project_and_slug() {
        assert_eq!(
            parse_repository_skill_id("repository:p1:block-migration"),
            Some(("p1", "block-migration"))
        );
        // A catalog skill is not one, nor is what the id does not complete.
        for other in [
            "review",
            "custom-review",
            "repository:",
            "repository:p1",
            "repository::x",
            "repository:p1:",
        ] {
            assert_eq!(parse_repository_skill_id(other), None, "{other}");
        }
    }

    #[tokio::test]
    async fn a_skill_the_project_uses_is_resolved_masked_and_without_its_header() {
        const TYPED_KEY: &str = "sk-zyxwvutsrqponmlkjihgfedcba9876543210";
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".agents/skills/block-migration");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: block-migration\ndescription: Moves a block.\n---\nMove the block.\nkey {TYPED_KEY}\n"
            ),
        )
        .unwrap();
        let state = state_with_project(root.path()).await;
        reference_block_migration(&state).await;

        let skills = repository_skills_for_discussion(
            &state.db,
            Some("p1"),
            &ids(&["review", "repository:p1:block-migration"]),
        )
        .await;

        assert!(skills.unresolved.is_empty() && skills.truncated.is_empty());
        assert_eq!(skills.notice(), None);
        assert_eq!(
            skills.resolved.len(),
            1,
            "the catalog id is not its business"
        );
        let skill = &skills.resolved[0];
        assert_eq!(skill.id, "repository:p1:block-migration");
        assert_eq!(skill.name, "Block migration");
        assert_eq!(skill.description, "Moves a block.");
        assert!(skill.content.contains("Move the block."));
        assert!(
            !skill.content.contains("description:"),
            "the header is not injected"
        );
        assert!(!skill.content.contains(TYPED_KEY), "the text is masked");
        assert!(skill.content.contains("***REDACTED***"));
    }

    #[tokio::test]
    async fn a_skill_the_project_does_not_use_is_refused_whatever_its_id_says() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".agents/skills/unused");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: unused\ndescription: d\n---\nNot referenced.\n",
        )
        .unwrap();
        std::fs::write(root.path().join("secret.txt"), "top secret").unwrap();
        let state = state_with_project(root.path()).await;

        let skills = repository_skills_for_discussion(
            &state.db,
            Some("p1"),
            &ids(&[
                "repository:p1:unused",
                "repository:p1:../secret.txt",
                "repository:p1:secret.txt",
            ]),
        )
        .await;

        assert!(skills.resolved.is_empty());
        assert_eq!(skills.unresolved.len(), 3);
        assert!(skills
            .unresolved
            .iter()
            .all(|skill| skill.problem == RepositorySkillProblem::NotUsed));
        // Said, not dropped.
        let notice = skills.notice().expect("the user is told");
        assert!(notice.contains("`unused`") && notice.contains("no longer uses it"));
    }

    #[tokio::test]
    async fn a_reference_pointing_out_of_the_repository_is_not_read() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(
            outside.path().join("SKILL.md"),
            "---\nname: x\ndescription: d\n---\nLeak.\n",
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let state = state_with_project(root.path()).await;
        let escaping = format!(
            "../{}/SKILL.md",
            outside.path().file_name().unwrap().to_string_lossy()
        );
        state
            .db
            .with_conn(move |conn| {
                crate::db::project_skill_references::upsert(
                    conn,
                    "p1",
                    "escape",
                    &escaping,
                    "Escape",
                    "2026-09-30T00:00:00Z",
                )
            })
            .await
            .unwrap();

        let skills = repository_skills_for_discussion(
            &state.db,
            Some("p1"),
            &ids(&["repository:p1:escape"]),
        )
        .await;

        assert!(skills.resolved.is_empty());
        assert_eq!(
            skills.unresolved[0].problem,
            RepositorySkillProblem::Unreadable
        );
    }

    #[cfg(unix)] // creates symlinks through std::os::unix
    #[tokio::test]
    async fn a_skill_md_behind_a_symbolic_link_is_not_read() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(
            outside.path().join("real.md"),
            "---\nname: linked\ndescription: d\n---\nLeak.\n",
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".agents/skills/linked");
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(outside.path().join("real.md"), dir.join("SKILL.md")).unwrap();
        let state = state_with_project(root.path()).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::project_skill_references::upsert(
                    conn,
                    "p1",
                    "linked",
                    ".agents/skills/linked/SKILL.md",
                    "Linked",
                    "2026-09-30T00:00:00Z",
                )
            })
            .await
            .unwrap();

        let skills = repository_skills_for_discussion(
            &state.db,
            Some("p1"),
            &ids(&["repository:p1:linked"]),
        )
        .await;

        assert!(skills.resolved.is_empty());
        assert_eq!(
            skills.unresolved[0].problem,
            RepositorySkillProblem::Unreadable
        );
    }

    #[tokio::test]
    async fn another_projects_skill_is_reported_and_never_read() {
        let root = tempfile::tempdir().unwrap();
        write_skill(
            root.path(),
            ".agents/skills",
            "block-migration",
            "Block migration",
            "Steps.",
        );
        let state = state_with_project(root.path()).await;
        reference_block_migration(&state).await;

        for project in [Some("elsewhere"), None] {
            let skills = repository_skills_for_discussion(
                &state.db,
                project,
                &ids(&["repository:p1:block-migration"]),
            )
            .await;
            assert!(skills.resolved.is_empty(), "{project:?}");
            assert_eq!(
                skills.unresolved[0].problem,
                RepositorySkillProblem::OtherProject
            );
            assert!(skills.notice().unwrap().contains("another project"));
        }

        let gone = repository_skills_for_discussion(
            &state.db,
            Some("gone"),
            &ids(&["repository:gone:block-migration"]),
        )
        .await;
        assert_eq!(
            gone.unresolved[0].problem,
            RepositorySkillProblem::ProjectGone
        );
    }

    #[tokio::test]
    async fn a_file_edited_between_two_sends_is_read_again_and_a_removed_one_is_reported() {
        let root = tempfile::tempdir().unwrap();
        write_skill(
            root.path(),
            ".agents/skills",
            "block-migration",
            "Block migration",
            "First version.",
        );
        let state = state_with_project(root.path()).await;
        reference_block_migration(&state).await;
        let selected = ids(&["repository:p1:block-migration"]);

        let first = repository_skills_for_discussion(&state.db, Some("p1"), &selected).await;
        assert!(first.resolved[0].content.contains("First version."));

        write_skill(
            root.path(),
            ".agents/skills",
            "block-migration",
            "Block migration",
            "Second version.",
        );
        let second = repository_skills_for_discussion(&state.db, Some("p1"), &selected).await;
        assert!(second.resolved[0].content.contains("Second version."));
        assert!(!second.resolved[0].content.contains("First version."));

        std::fs::remove_file(root.path().join(BLOCK_MIGRATION)).unwrap();
        let third = repository_skills_for_discussion(&state.db, Some("p1"), &selected).await;
        assert!(third.resolved.is_empty());
        assert_eq!(
            third.unresolved[0].problem,
            RepositorySkillProblem::Unreadable
        );
        assert!(third.notice().unwrap().contains("cannot be read"));
    }

    #[tokio::test]
    async fn a_skill_longer_than_the_bound_is_cut_and_the_notice_says_so() {
        let root = tempfile::tempdir().unwrap();
        let body = "é".repeat(MAX_INJECTED_SKILL_BYTES);
        write_skill(
            root.path(),
            ".agents/skills",
            "block-migration",
            "Block migration",
            &body,
        );
        let state = state_with_project(root.path()).await;
        reference_block_migration(&state).await;

        let skills = repository_skills_for_discussion(
            &state.db,
            Some("p1"),
            &ids(&["repository:p1:block-migration"]),
        )
        .await;

        assert_eq!(skills.resolved.len(), 1);
        assert!(skills.resolved[0].content.len() <= MAX_INJECTED_SKILL_BYTES);
        assert_eq!(skills.truncated, vec!["Block migration".to_string()]);
        assert!(skills.notice().unwrap().contains("cut at 64 KiB"));
    }
}
