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
    Skill, SkillCategory,
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

// ─── Discussions (KT-923) ────────────────────────────────────────────────────
//
// A skill only a repository holds can be selected for a discussion under the
// id the Automation page gives it, `repository:<project>:<slug>`. The catalog
// knows nothing of it, so the discussion resolves it itself, from the
// repository, every time a message is sent — a `SKILL.md` edited between two
// sends is read again, and one that disappeared is reported.

/// How an id starts when the skill is one only a project's repository holds.
const REPOSITORY_SKILL_PREFIX: &str = "repository:";

/// What Kronn injects of one repository skill at most. The content route serves
/// up to 512 KiB for a human to read; an agent's prompt is paid for on every
/// turn, so a skill that long is cut and said to be.
pub const MAX_INJECTED_SKILL_BYTES: usize = 64 * 1024;

/// `(project id, slug)` out of `repository:<project>:<slug>`, `None` for any
/// other id (a catalog skill).
pub fn parse_repository_skill_id(id: &str) -> Option<(&str, &str)> {
    let (project_id, slug) = id.strip_prefix(REPOSITORY_SKILL_PREFIX)?.rsplit_once(':')?;
    (!project_id.is_empty() && !slug.is_empty()).then_some((project_id, slug))
}

/// Why a repository skill selected for a discussion is not loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositorySkillProblem {
    /// It belongs to another project than the discussion's, or the discussion
    /// has none: reading it would take a file from a repository the agent is
    /// not working in.
    OtherProject,
    /// Its project no longer exists.
    ProjectGone,
    /// The project no longer uses it: not referenced, not published.
    NotUsed,
    /// The project uses it but its `SKILL.md` cannot be read (gone from the
    /// repository, behind a symbolic link, unreadable).
    Unreadable,
}

impl RepositorySkillProblem {
    fn reason(self) -> &'static str {
        match self {
            Self::OtherProject => "it belongs to another project than this discussion's",
            Self::ProjectGone => "its project no longer exists",
            Self::NotUsed => "the project no longer uses it",
            Self::Unreadable => "its SKILL.md cannot be read from the repository",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedRepositorySkill {
    pub id: String,
    pub problem: RepositorySkillProblem,
}

/// The repository skills a discussion selected, as they stand right now.
#[derive(Debug, Default)]
pub struct RepositorySkills {
    /// Ready to inject: the `SKILL.md` body, masked and bounded.
    pub resolved: Vec<Skill>,
    /// Ids the agent will not receive, and why.
    pub unresolved: Vec<UnresolvedRepositorySkill>,
    /// Names of the resolved skills that were longer than the bound and cut.
    pub truncated: Vec<String>,
}

impl RepositorySkills {
    /// What the discussion tells the user when a selected skill is not, or only
    /// partly, loaded. `None` when everything selected was loaded whole.
    pub fn notice(&self) -> Option<String> {
        if self.unresolved.is_empty() && self.truncated.is_empty() {
            return None;
        }
        let mut lines = Vec::new();
        for skill in &self.unresolved {
            let name = parse_repository_skill_id(&skill.id).map_or(skill.id.as_str(), |(_, s)| s);
            lines.push(format!(
                "`{name}` was not loaded: {}.",
                skill.problem.reason()
            ));
        }
        for name in &self.truncated {
            lines.push(format!(
                "`{name}` was cut at {} KiB.",
                MAX_INJECTED_SKILL_BYTES / 1024
            ));
        }
        Some(format!("⚠️ **Repository skills** — {}", lines.join(" ")))
    }
}

/// `text` cut at `max` bytes on a character boundary, and whether it was cut.
fn cut_at(text: &str, max: usize) -> (&str, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// The skill an agent receives out of a repository `SKILL.md` that has already
/// been read, masked and cut by the route's own pass: its body, without the
/// header a catalog skill does not carry either. A file with no header is taken
/// whole.
fn repository_skill(id: &str, used: &ProjectUsedSkill, text: &str) -> (Skill, bool) {
    let file = crate::core::agent_skill::parse(text).ok();
    let (body, description) = file.as_ref().map_or((text, String::new()), |file| {
        (file.body.as_str(), file.description.clone())
    });
    let (content, truncated) = cut_at(body, MAX_INJECTED_SKILL_BYTES);
    let name = if used.name.is_empty() {
        used.slug.clone()
    } else {
        used.name.clone()
    };
    let skill = Skill {
        id: id.to_string(),
        name,
        description,
        icon: "📂".into(),
        category: SkillCategory::Domain,
        content: content.to_string(),
        is_builtin: false,
        token_estimate: u32::try_from(content.len() / 4).unwrap_or(u32::MAX),
        license: file.as_ref().and_then(|file| file.license.clone()),
        allowed_tools: file.as_ref().and_then(|file| file.allowed_tools.clone()),
        auto_triggers: None,
        external: false,
        source_url: None,
        arguments: file
            .as_ref()
            .map(|file| file.arguments.clone())
            .unwrap_or_default(),
        argument_hint: file.as_ref().and_then(|file| file.argument_hint.clone()),
        variables: file
            .as_ref()
            .and_then(|file| {
                let json = file.metadata.get(crate::core::agent_skill::VARIABLES_KEY)?;
                crate::core::agent_skill::parse_variables(json, &file.arguments).ok()
            })
            .unwrap_or_default(),
    };
    (skill, truncated)
}

/// Reads the `SKILL.md` of one repository skill a project uses, with the
/// guarantees of the content route: only a path the project uses, never a
/// symbolic link or a path out of the repository, masked before anything else
/// is done with the text.
fn read_used_skill(
    conn: &rusqlite::Connection,
    project_id: &str,
    slug: &str,
    catalog: &BTreeMap<String, String>,
) -> anyhow::Result<Result<(ProjectUsedSkill, String), RepositorySkillProblem>> {
    let Some(project) = crate::db::projects::get_project(conn, project_id)? else {
        return Ok(Err(RepositorySkillProblem::ProjectGone));
    };
    let root = Path::new(&project.path);
    let references = crate::db::project_skill_references::list_for_project(conn, project_id)?;
    let Some(used) = project_used_skills(project_id, root, &references, catalog)
        .into_iter()
        .find(|skill| skill.skill_id.is_none() && skill.slug == slug)
    else {
        return Ok(Err(RepositorySkillProblem::NotUsed));
    };
    let Some(bytes) = read_repository_file(root, &used.relative_path) else {
        return Ok(Err(RepositorySkillProblem::Unreadable));
    };
    let (text, _) = side_text(&bytes);
    Ok(Ok((used, text)))
}

/// The repository skills among `skill_ids` (catalog ids are left to the
/// catalog), resolved from their project's repository as it is now. A skill is
/// loaded only when the discussion's project is the one it belongs to and that
/// project still uses it; anything else is listed in `unresolved`, never
/// dropped without a word.
pub fn resolve_repository_skills(
    conn: &rusqlite::Connection,
    discussion_project_id: Option<&str>,
    skill_ids: &[String],
) -> anyhow::Result<RepositorySkills> {
    let mut seen = std::collections::BTreeSet::new();
    let wanted: Vec<(&String, &str, &str)> = skill_ids
        .iter()
        .filter(|id| seen.insert(id.as_str()))
        .filter_map(|id| parse_repository_skill_id(id).map(|(project, slug)| (id, project, slug)))
        .collect();
    let mut result = RepositorySkills::default();
    if wanted.is_empty() {
        return Ok(result);
    }
    let catalog = catalog_ids_by_slug();
    for (id, project_id, slug) in wanted {
        let outcome = if discussion_project_id == Some(project_id) {
            read_used_skill(conn, project_id, slug, &catalog)?
        } else {
            Err(RepositorySkillProblem::OtherProject)
        };
        match outcome {
            Ok((used, text)) => {
                let (skill, truncated) = repository_skill(id, &used, &text);
                if truncated {
                    result.truncated.push(skill.name.clone());
                }
                result.resolved.push(skill);
            }
            Err(problem) => result.unresolved.push(UnresolvedRepositorySkill {
                id: id.clone(),
                problem,
            }),
        }
    }
    Ok(result)
}

/// [`resolve_repository_skills`] for a message that is about to be sent. When
/// the database cannot be read, every repository skill selected is reported as
/// unreadable rather than silently left out.
pub async fn repository_skills_for_discussion(
    db: &crate::db::Database,
    discussion_project_id: Option<&str>,
    skill_ids: &[String],
) -> RepositorySkills {
    if !skill_ids
        .iter()
        .any(|id| parse_repository_skill_id(id).is_some())
    {
        return RepositorySkills::default();
    }
    let project_id = discussion_project_id.map(str::to_string);
    let ids = skill_ids.to_vec();
    match db
        .with_read_conn(move |conn| resolve_repository_skills(conn, project_id.as_deref(), &ids))
        .await
    {
        Ok(skills) => skills,
        Err(error) => {
            tracing::warn!("repository skills: cannot resolve the selection: {error}");
            RepositorySkills {
                unresolved: skill_ids
                    .iter()
                    .filter(|id| parse_repository_skill_id(id).is_some())
                    .map(|id| UnresolvedRepositorySkill {
                        id: id.clone(),
                        problem: RepositorySkillProblem::Unreadable,
                    })
                    .collect(),
                ..RepositorySkills::default()
            }
        }
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

    #[cfg(unix)]
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
