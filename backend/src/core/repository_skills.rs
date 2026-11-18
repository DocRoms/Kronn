//! Repository skills as a project uses them: which ones a project lists, and
//! their `SKILL.md` as an agent or a workflow run receives it. Kept below the
//! API layer so a workflow can resolve them without depending on a route module.

use std::collections::BTreeMap;
use std::path::Path;

use crate::db::project_skill_references::SkillReference;
use crate::models::{ProjectRepositoryResourceKind, ProjectUsedSkill, Skill, SkillCategory};

/// Catalog skill id by the slug Kronn writes it under: how a published skill
/// is told from one only the repository holds.
pub(crate) fn catalog_ids_by_slug() -> BTreeMap<String, String> {
    catalog_ids_by_slug_of(&crate::core::skills::list_all_skills())
}

pub(crate) fn catalog_ids_by_slug_of(skills: &[Skill]) -> BTreeMap<String, String> {
    let mut ids = BTreeMap::new();
    for skill in skills {
        ids.entry(crate::core::native_files::slug(&skill.id))
            .or_insert_with(|| skill.id.clone());
    }
    ids
}

/// `.agents/skills` out of `.agents/skills/<slug>/SKILL.md`.
pub(crate) fn skill_root(relative_path: &str) -> String {
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
pub(crate) fn project_used_skills(
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
            sync_status: None,
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
                sync_status: None,
            }),
        }
    }
    used
}

pub(crate) fn read_repository_file(root: &Path, relative: &str) -> Option<Vec<u8>> {
    let path = root.join(relative);
    crate::core::fs_guard::assert_contained_no_symlink(root, &path).ok()?;
    std::fs::read(path).ok()
}

/// What the sheet carries of one side of one file: a longer text is cut here
/// and flagged, so a large artifact cannot turn the answer into megabytes. The
/// diff is built from the whole file, whatever is cut.
pub(crate) const MAX_CONTENT_BYTES: usize = 512 * 1024;

/// The text of one side of one file as the answer carries it: masked first,
/// whichever side it comes from, then cut — so a secret straddling the cut is
/// never half shown.
pub(crate) fn side_text(bytes: &[u8]) -> (String, bool) {
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
    /// A workflow reads it from the default branch, and it is not committed
    /// there (or the repository has no default branch).
    NotOnDefaultBranch,
}

/// Where a repository skill is read from. A discussion takes the checkout as it
/// is; a workflow run takes the default branch, so neither the branch the
/// checkout is on nor the run's own worktree can change the skill it loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositorySkillSource {
    WorkingTree,
    DefaultBranch,
}

impl RepositorySkillProblem {
    fn reason(self) -> &'static str {
        match self {
            Self::OtherProject => "it belongs to another project",
            Self::ProjectGone => "its project no longer exists",
            Self::NotUsed => "the project no longer uses it",
            Self::Unreadable => "its SKILL.md cannot be read from the repository",
            Self::NotOnDefaultBranch => "its SKILL.md is not committed on the default branch",
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
        project_id: None,
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
    source: RepositorySkillSource,
    commits: &mut BTreeMap<std::path::PathBuf, Option<String>>,
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
    let bytes = match source {
        RepositorySkillSource::WorkingTree => read_repository_file(root, &used.relative_path)
            .ok_or(RepositorySkillProblem::Unreadable),
        RepositorySkillSource::DefaultBranch => {
            // One commit per repository and pass, so two skills never come
            // from two states of the default branch.
            let commit = commits
                .entry(root.to_path_buf())
                .or_insert_with(|| default_branch_commit(root))
                .clone();
            commit
                .and_then(|commit| read_at_commit(root, &commit, &used.relative_path))
                .ok_or(RepositorySkillProblem::NotOnDefaultBranch)
        }
    };
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(problem) => return Ok(Err(problem)),
    };
    let (text, _) = side_text(&bytes);
    Ok(Ok((used, text)))
}

/// The commit a workflow reads repository skills at: the remote default branch
/// (`origin/HEAD`, else `origin/main` or `origin/master`). The local `main` or
/// `master` is used only when the clone has no such remote reference at all; a
/// remote reference that resolves to no commit is no default branch.
fn default_branch_commit(root: &Path) -> Option<String> {
    let git = |args: &[&str]| -> Option<String> {
        let output = crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(root)
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (output.status.success() && !text.is_empty()).then_some(text)
    };
    let exists = |reference: &str| git(&["rev-parse", "--verify", "--quiet", reference]).is_some();
    let remote = git(&["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"]).or_else(|| {
        ["refs/remotes/origin/main", "refs/remotes/origin/master"]
            .into_iter()
            .find(|reference| exists(reference))
            .map(str::to_string)
    });
    let reference = remote.or_else(|| {
        ["refs/heads/main", "refs/heads/master"]
            .into_iter()
            .find(|reference| exists(reference))
            .map(str::to_string)
    })?;
    git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{reference}^{{commit}}"),
    ])
}

/// `relative` as committed at `commit`, never a path out of the repository.
pub(crate) fn read_at_commit(root: &Path, commit: &str, relative: &str) -> Option<Vec<u8>> {
    let inside = !relative.is_empty()
        && !relative.starts_with('/')
        && relative
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    if !inside {
        return None;
    }
    crate::core::worktree::file_at_revision(root, commit, relative).ok()
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
    resolve_repository_skills_from(
        conn,
        discussion_project_id,
        skill_ids,
        RepositorySkillSource::WorkingTree,
    )
}

/// [`resolve_repository_skills`], read from `source`.
pub fn resolve_repository_skills_from(
    conn: &rusqlite::Connection,
    project_id: Option<&str>,
    skill_ids: &[String],
    source: RepositorySkillSource,
) -> anyhow::Result<RepositorySkills> {
    let discussion_project_id = project_id;
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
    let mut commits = BTreeMap::new();
    for (id, project_id, slug) in wanted {
        let outcome = if discussion_project_id == Some(project_id) {
            read_used_skill(conn, project_id, slug, &catalog, source, &mut commits)?
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
