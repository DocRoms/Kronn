// Template v2 install by merge into an already-documented repository
// (KT-1172). The plan comes from `core::docs_merge`; this module runs it in
// a dedicated worktree (`kronn/template-v2-merge`) so the project checkout is
// never written, returns one diff per file, and commits only what the
// reviewer accepts. Every call is resumable: re-POSTing picks the same
// worktree up again and re-proves that no existing doc changed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as AxPath, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::core::docs_merge::{self, Correspondence, MergePlan, WriteOutcome};
use crate::core::scanner;
use crate::models::ApiResponse;
use crate::AppState;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeBase {
    /// The checkout's current commit.
    #[default]
    Local,
    /// The remote default branch, fetched first.
    Remote,
}

#[derive(Debug, Default, Deserialize)]
pub struct TemplateMergeRequest {
    #[serde(default)]
    pub base: MergeBase,
}

#[derive(Debug, Deserialize)]
pub struct TemplateMergeAcceptRequest {
    /// Repo-relative files to commit; the others stay uncommitted in the worktree.
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeFileDiff {
    pub path: String,
    /// `None` on a read-only plan.
    pub outcome: Option<WriteOutcome>,
    pub imported_sections: usize,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateMergeView {
    pub docs_dir: String,
    pub base: MergeBase,
    pub base_ref: String,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub resumed: bool,
    pub correspondence: Vec<Correspondence>,
    pub files: Vec<MergeFileDiff>,
    pub existing_targets: Vec<String>,
    /// Docs that only exist untracked in the checkout: a worktree cannot see them.
    pub untracked_in_checkout: Vec<String>,
    pub table_markdown: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateMergeAccepted {
    pub branch: String,
    pub commit: String,
    pub committed: Vec<String>,
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = crate::core::cmd::git_cmd()
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn nul_list(raw: &str) -> Vec<String> {
    raw.split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_git_repo(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--is-inside-work-tree"]).is_ok_and(|out| out.trim() == "true")
}

/// The base snapshot: tracked files only, so a resumed worktree sees the same
/// base whatever the previous run created.
fn tracked_files(dir: &Path) -> Result<BTreeSet<String>, String> {
    Ok(
        nul_list(&git(dir, &["-c", "core.quotepath=off", "ls-files", "-z"])?)
            .into_iter()
            .collect(),
    )
}

fn untracked_docs(dir: &Path) -> Vec<String> {
    git(
        dir,
        &[
            "-c",
            "core.quotepath=off",
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )
    .map(|raw| {
        let all: BTreeSet<String> = nul_list(&raw).into_iter().collect();
        docs_merge::source_paths(&all)
    })
    .unwrap_or_default()
}

fn templates_docs_dir() -> Result<PathBuf, String> {
    let dir = super::resolve_templates_dir().join("docs");
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(format!("Templates directory not found: {}", dir.display()))
    }
}

fn plan_on(
    root: &Path,
    existing: &BTreeSet<String>,
    template_docs: &Path,
) -> Result<MergePlan, String> {
    let template = docs_merge::load_template(template_docs)?;
    let docs_dir = docs_merge::choose_docs_dir(existing, None);
    docs_merge::plan_merge(root, existing, &template, &docs_dir)
}

fn view(
    plan: &MergePlan,
    base: MergeBase,
    base_ref: String,
    files: Vec<MergeFileDiff>,
    untracked_in_checkout: Vec<String>,
) -> TemplateMergeView {
    TemplateMergeView {
        docs_dir: plan.docs_dir.clone(),
        base,
        base_ref,
        worktree_path: None,
        branch: None,
        resumed: false,
        correspondence: plan.correspondence.clone(),
        files,
        existing_targets: plan.existing_targets.clone(),
        untracked_in_checkout,
        table_markdown: docs_merge::render_correspondence_markdown(plan),
    }
}

/// Read-only plan on the checkout itself: nothing is written anywhere.
pub fn preview_plan(repo: &Path, template_docs: &Path) -> Result<TemplateMergeView, String> {
    let (existing, untracked) = if is_git_repo(repo) {
        (tracked_files(repo)?, untracked_docs(repo))
    } else {
        (docs_merge::snapshot_tree(repo)?, Vec::new())
    };
    let plan = plan_on(repo, &existing, template_docs)?;
    let files = plan
        .writes
        .iter()
        .map(|w| MergeFileDiff {
            path: w.path.clone(),
            outcome: None,
            imported_sections: w.imported_sections,
            diff: docs_merge::addition_diff(&w.path, &w.content),
        })
        .collect();
    Ok(view(
        &plan,
        MergeBase::Local,
        "HEAD".into(),
        files,
        untracked,
    ))
}

fn remote_default_ref(repo: &Path) -> Result<String, String> {
    git(repo, &["fetch", "--prune", "origin"])
        .map_err(|e| format!("{e} — check the remote and its credentials, then resume"))?;
    if let Ok(head) = git(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        let head = head.trim();
        if !head.is_empty() {
            return Ok(head.to_string());
        }
    }
    for candidate in ["origin/main", "origin/master"] {
        if git(repo, &["rev-parse", "--verify", "--quiet", candidate]).is_ok() {
            return Ok(candidate.to_string());
        }
    }
    Err("the remote has no default branch (origin/HEAD, main or master)".into())
}

fn existing_merge_worktree(repo: &Path) -> Option<PathBuf> {
    let listing = git(repo, &["worktree", "list", "--porcelain"]).ok()?;
    let mut current: Option<PathBuf> = None;
    for line in listing.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current = Some(PathBuf::from(path));
        } else if line == format!("branch refs/heads/{}", docs_merge::MERGE_BRANCH) {
            return current.filter(|p| p.is_dir());
        }
    }
    None
}

fn branch_exists(repo: &Path) -> bool {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", docs_merge::MERGE_BRANCH),
        ],
    )
    .is_ok()
}

/// Paths git reports as changed (anything but untracked) inside the merge's reach.
fn modified_existing_docs(worktree: &Path) -> Result<Vec<String>, String> {
    let raw = git(
        worktree,
        &[
            "-c",
            "core.quotepath=off",
            "status",
            "--porcelain=v1",
            "-z",
            "-uall",
        ],
    )?;
    Ok(nul_list(&raw)
        .into_iter()
        .filter(|entry| entry.len() > 3 && !entry.starts_with("??"))
        .map(|entry| entry[3..].to_string())
        .filter(|path| {
            let single: BTreeSet<String> = [path.clone()].into_iter().collect();
            !docs_merge::source_paths(&single).is_empty()
        })
        .collect())
}

fn untracked_under(worktree: &Path, dir: &str) -> Result<Vec<String>, String> {
    let raw = git(
        worktree,
        &[
            "-c",
            "core.quotepath=off",
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            dir,
        ],
    )?;
    Ok(nul_list(&raw))
}

/// Create (or resume) the merge worktree, apply the plan create-only, prove
/// no existing doc changed, and return one diff per file.
pub fn run_merge_in_worktree(
    repo: &Path,
    project_name: &str,
    base: MergeBase,
    template_docs: &Path,
) -> Result<TemplateMergeView, String> {
    if !is_git_repo(repo) {
        return Err(format!(
            "{} is not a git repository: the merge runs in a worktree",
            repo.display()
        ));
    }
    let untracked_in_checkout = untracked_docs(repo);
    let (worktree, base_ref, resumed) = match existing_merge_worktree(repo) {
        Some(path) => {
            let head = git(&path, &["rev-parse", "--short", "HEAD"])?;
            (path, format!("resumed at {}", head.trim()), true)
        }
        None => {
            let base_ref = match base {
                MergeBase::Local => "HEAD".to_string(),
                MergeBase::Remote => remote_default_ref(repo)?,
            };
            let info = if branch_exists(repo) {
                crate::core::worktree::reattach_worktree(
                    repo,
                    project_name,
                    docs_merge::MERGE_SLUG,
                    docs_merge::MERGE_BRANCH,
                )?
            } else {
                crate::core::worktree::create_discussion_worktree(
                    repo,
                    project_name,
                    docs_merge::MERGE_SLUG,
                    &base_ref,
                )?
            };
            (PathBuf::from(info.path), base_ref, false)
        }
    };
    // git lists canonical paths (/private/var on macOS): one spelling across resumes.
    let worktree = worktree.canonicalize().unwrap_or(worktree);

    let already_modified = modified_existing_docs(&worktree)?;
    if !already_modified.is_empty() {
        return Err(format!(
            "the merge worktree {} has edited existing docs ({}): commit or revert them, then resume",
            worktree.display(),
            already_modified.join(", ")
        ));
    }

    let existing = tracked_files(&worktree)?;
    let plan = plan_on(&worktree, &existing, template_docs)?;
    let sources = docs_merge::source_paths(&existing);
    let before = docs_merge::fingerprint(&worktree, &sources);
    let outcomes = docs_merge::apply_plan(&worktree, &plan)?;
    // Same create-only scaffold as a fresh install (conventions/, gotchas/, people/).
    let docs_root = worktree.join(&plan.docs_dir);
    super::ensure_agent_writable_subfolders(&worktree, &docs_root);
    docs_merge::verify_unchanged(&before, &docs_merge::fingerprint(&worktree, &sources))?;
    let modified = modified_existing_docs(&worktree)?;
    if !modified.is_empty() {
        return Err(format!(
            "an existing doc changed in the merge worktree, stopped: {}",
            modified.join(", ")
        ));
    }

    let mut files: Vec<MergeFileDiff> = Vec::new();
    for (path, outcome) in &outcomes {
        let content = std::fs::read_to_string(worktree.join(path)).unwrap_or_default();
        let imported = plan
            .writes
            .iter()
            .find(|w| &w.path == path)
            .map_or(0, |w| w.imported_sections);
        files.push(MergeFileDiff {
            path: path.clone(),
            outcome: Some(*outcome),
            imported_sections: imported,
            diff: docs_merge::addition_diff(path, &content),
        });
    }
    for path in untracked_under(&worktree, &plan.docs_dir)? {
        if files.iter().any(|f| f.path == path) {
            continue;
        }
        let content = std::fs::read_to_string(worktree.join(&path)).unwrap_or_default();
        files.push(MergeFileDiff {
            diff: docs_merge::addition_diff(&path, &content),
            path,
            outcome: Some(WriteOutcome::Created),
            imported_sections: 0,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut out = view(&plan, base, base_ref, files, untracked_in_checkout);
    out.worktree_path = Some(worktree.to_string_lossy().into_owned());
    out.branch = Some(docs_merge::MERGE_BRANCH.to_string());
    out.resumed = resumed;
    Ok(out)
}

/// Commit the accepted files on the merge branch. Unaccepted files stay
/// uncommitted in the worktree; nothing is deleted and the checkout is untouched.
pub fn accept_merge(repo: &Path, paths: &[String]) -> Result<TemplateMergeAccepted, String> {
    let worktree = existing_merge_worktree(repo)
        .ok_or("no template merge in progress: start it first (POST template-merge)")?;
    if paths.is_empty() {
        return Err("no file accepted: pick at least one file to commit".into());
    }
    let existing = tracked_files(&worktree)?;
    let docs_dir = docs_merge::choose_docs_dir(&existing, None);
    let proposed: BTreeSet<String> = untracked_under(&worktree, &docs_dir)?.into_iter().collect();
    let refused: Vec<&String> = paths.iter().filter(|p| !proposed.contains(*p)).collect();
    if !refused.is_empty() {
        return Err(format!(
            "only files proposed by the merge can be accepted: {}",
            refused
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let mut add = vec!["add", "--"];
    add.extend(paths.iter().map(String::as_str));
    git(&worktree, &add)?;
    let mut commit = vec![
        "commit",
        "-m",
        "docs: merge the Kronn AI-doc template v2",
        "--",
    ];
    commit.extend(paths.iter().map(String::as_str));
    git(&worktree, &commit)?;
    let sha = git(&worktree, &["rev-parse", "HEAD"])?;
    Ok(TemplateMergeAccepted {
        branch: docs_merge::MERGE_BRANCH.to_string(),
        commit: sha.trim().to_string(),
        committed: paths.to_vec(),
    })
}

pub fn discard_merge(repo: &Path) -> Result<(), String> {
    let Some(worktree) = existing_merge_worktree(repo) else {
        return Ok(());
    };
    crate::core::worktree::remove_discussion_worktree(repo, &worktree.to_string_lossy(), true)
}

async fn project_root(state: &AppState, id: String) -> Result<(PathBuf, String), String> {
    let project = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
        .map_err(|e| format!("DB error: {e}"))?
        .ok_or("Project not found")?;
    let root = scanner::resolve_host_path(&project.path);
    if !root.exists() {
        return Err(format!("Project path not found: {}", root.display()));
    }
    Ok((root, project.name))
}

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(job)
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {e}")))
}

fn respond<T: Serialize>(result: Result<T, String>) -> Json<ApiResponse<T>> {
    match result {
        Ok(value) => Json(ApiResponse::ok(value)),
        Err(e) => Json(ApiResponse::err(e)),
    }
}

/// GET /api/projects/:id/template-merge/plan — read-only correspondence table and diffs.
pub async fn template_merge_plan(
    State(state): State<AppState>,
    AxPath(id): AxPath<String>,
) -> Json<ApiResponse<TemplateMergeView>> {
    let result = match project_root(&state, id).await {
        Ok((root, _)) => {
            blocking(move || {
                let template = templates_docs_dir()?;
                preview_plan(&root, &template)
            })
            .await
        }
        Err(e) => Err(e),
    };
    respond(result)
}

/// POST /api/projects/:id/template-merge — apply (or resume) in the merge worktree.
pub async fn template_merge_start(
    State(state): State<AppState>,
    AxPath(id): AxPath<String>,
    body: Option<Json<TemplateMergeRequest>>,
) -> Json<ApiResponse<TemplateMergeView>> {
    let base = body.map(|Json(b)| b.base).unwrap_or_default();
    let result = match project_root(&state, id).await {
        Ok((root, name)) => {
            blocking(move || {
                let template = templates_docs_dir()?;
                run_merge_in_worktree(&root, &name, base, &template)
            })
            .await
        }
        Err(e) => Err(e),
    };
    respond(result)
}

/// POST /api/projects/:id/template-merge/accept — commit the accepted files on the merge branch.
pub async fn template_merge_accept(
    State(state): State<AppState>,
    AxPath(id): AxPath<String>,
    Json(body): Json<TemplateMergeAcceptRequest>,
) -> Json<ApiResponse<TemplateMergeAccepted>> {
    let result = match project_root(&state, id).await {
        Ok((root, _)) => blocking(move || accept_merge(&root, &body.paths)).await,
        Err(e) => Err(e),
    };
    respond(result)
}

/// DELETE /api/projects/:id/template-merge — drop the merge worktree and its branch.
pub async fn template_merge_discard(
    State(state): State<AppState>,
    AxPath(id): AxPath<String>,
) -> Json<ApiResponse<()>> {
    let result = match project_root(&state, id).await {
        Ok((root, _)) => blocking(move || discard_merge(&root)).await,
        Err(e) => Err(e),
    };
    respond(result)
}

#[cfg(test)]
#[path = "template_merge_test.rs"]
mod tests;
