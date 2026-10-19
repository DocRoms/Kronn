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
use ts_rs::TS;

use crate::core::docs_location::DocsRoot;
use crate::core::docs_merge::{self, Correspondence, MergePlan, WriteOutcome};
use crate::core::scanner;
use crate::models::ApiResponse;
use crate::AppState;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum MergeBase {
    /// The checkout's current commit.
    #[default]
    Local,
    /// The remote default branch, fetched first.
    Remote,
}

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
pub struct TemplateMergeRequest {
    #[serde(default)]
    pub base: MergeBase,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct TemplateMergeAcceptRequest {
    /// Repo-relative files to commit; the others stay uncommitted in the worktree.
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct MergeFileDiff {
    pub path: String,
    /// `None` on a read-only plan.
    pub outcome: Option<WriteOutcome>,
    pub imported_sections: usize,
    pub diff: String,
}

/// A plan warning as data: the UI words it in the user's language from `code`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct TemplateMergeWarning {
    /// Stable identifier, e.g. `remote_base_misses_local_docs`.
    pub code: String,
    /// Paths the warning is about (capped), in a stable order.
    pub paths: Vec<String>,
    /// How many more paths there are than `paths` lists.
    pub more: u32,
}

/// `TemplateMergeWarning::code` of docs a remote base cannot contain.
pub(crate) const REMOTE_BASE_MISSES_LOCAL_DOCS: &str = "remote_base_misses_local_docs";

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
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
    /// Things the reviewer must know before trusting the plan, e.g. local docs
    /// a remote base cannot contain.
    pub warnings: Vec<TemplateMergeWarning>,
    pub table_markdown: String,
    pub merge: TemplateMergeState,
}

/// Where a merge stands, read from git without fetching or writing anything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct TemplateMergeState {
    pub branch: String,
    pub worktree_exists: bool,
    pub worktree_path: Option<String>,
    pub branch_exists: bool,
    /// Commits on the merge branch that neither the checkout's current branch
    /// nor the remote default branch contains yet.
    pub branch_ahead_commits: u32,
    /// Files proposed in the merge worktree and not accepted yet.
    pub pending_files: u32,
    /// `None` when the checkout is detached or not a git repository.
    pub checkout_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
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

/// The project's configured docs root when it is not `docs/` (KT-1192):
/// the merge then writes there, with the templates' references relocated.
fn configured_root(repo: &Path) -> Result<Option<DocsRoot>, String> {
    let docs_root = DocsRoot::for_project(repo)?;
    Ok((!docs_root.is_default()).then_some(docs_root))
}

fn plan_on(
    root: &Path,
    existing: &BTreeSet<String>,
    template_docs: &Path,
    configured: Option<&DocsRoot>,
) -> Result<MergePlan, String> {
    let mut template = docs_merge::load_template(template_docs)?;
    if let Some(docs_root) = configured {
        for file in &mut template {
            if file.rel.ends_with(".md") {
                if let Ok(text) = std::str::from_utf8(&file.bytes) {
                    file.bytes = docs_root.relocate(text).into_owned().into_bytes();
                }
            }
        }
    }
    let docs_dir = docs_merge::choose_docs_dir(existing, configured.map(DocsRoot::rel));
    // What an accepted merge wrote is Kronn's, never a source for the next plan.
    let kronn_owned: BTreeSet<String> =
        crate::core::docs_folder::kronn_written(&root.join(&docs_dir))
            .into_iter()
            .map(|rel| format!("{docs_dir}/{rel}"))
            .collect();
    docs_merge::plan_merge_excluding(root, existing, &template, &docs_dir, &kronn_owned)
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
        warnings: Vec::new(),
        table_markdown: docs_merge::render_correspondence_markdown(plan),
        merge: TemplateMergeState::default(),
    }
}

/// Read-only plan on the checkout itself: nothing is written anywhere.
pub fn preview_plan(repo: &Path, template_docs: &Path) -> Result<TemplateMergeView, String> {
    let (existing, untracked) = if is_git_repo(repo) {
        (tracked_files(repo)?, untracked_docs(repo))
    } else {
        (docs_merge::snapshot_tree(repo)?, Vec::new())
    };
    let plan = plan_on(
        repo,
        &existing,
        template_docs,
        configured_root(repo)?.as_ref(),
    )?;
    let merge = merge_state(repo);
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
    let mut out = view(&plan, MergeBase::Local, "HEAD".into(), files, untracked);
    out.merge = merge;
    Ok(out)
}

/// The remote default branch as last fetched; never fetches.
fn known_remote_default_ref(repo: &Path) -> Option<String> {
    let resolves = |candidate: &str| {
        git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{candidate}^{{commit}}"),
            ],
        )
        .is_ok()
    };
    // `origin/HEAD` can dangle (default branch renamed or pruned): it then names
    // a ref that does not exist, so it is only trusted when it resolves.
    if let Ok(head) = git(
        repo,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        let head = head.trim();
        if !head.is_empty() && resolves(head) {
            return Some(head.to_string());
        }
    }
    ["origin/main", "origin/master"]
        .into_iter()
        .find(|candidate| resolves(candidate))
        .map(str::to_string)
}

fn remote_default_ref(repo: &Path) -> Result<String, String> {
    git(repo, &["fetch", "--prune", "origin"])
        .map_err(|e| format!("{e} — check the remote and its credentials, then resume"))?;
    known_remote_default_ref(repo)
        .ok_or_else(|| "the remote has no default branch (origin/HEAD, main or master)".into())
}

/// Commits only the merge branch has. The remote default branch is excluded
/// too: a remote-based merge must not count the upstream commits it started from.
/// `None` when git cannot tell: callers must then treat the merge as not integrated.
fn merge_branch_ahead(repo: &Path) -> Option<u32> {
    let branch = format!("refs/heads/{}", docs_merge::MERGE_BRANCH);
    let remote = known_remote_default_ref(repo);
    let mut args = vec!["rev-list", "--count", branch.as_str(), "--not", "HEAD"];
    if let Some(remote) = remote.as_deref() {
        args.push(remote);
    }
    git(repo, &args)
        .ok()
        .and_then(|out| out.trim().parse().ok())
}

/// The merge branch holds, or may hold, commits nothing else contains.
fn merge_branch_unintegrated(repo: &Path) -> bool {
    merge_branch_ahead(repo).is_none_or(|ahead| ahead > 0)
}

fn pending_files(worktree: &Path, configured: Option<&DocsRoot>) -> u32 {
    let Ok(existing) = tracked_files(worktree) else {
        return 0;
    };
    let docs_dir = docs_merge::choose_docs_dir(&existing, configured.map(DocsRoot::rel));
    proposed_files(worktree, &docs_dir).map_or(0, |files| files.len() as u32)
}

/// Read-only: worktree list, refs and counts; no fetch, no index write.
pub fn merge_state(repo: &Path) -> TemplateMergeState {
    let mut state = TemplateMergeState {
        branch: docs_merge::MERGE_BRANCH.to_string(),
        ..TemplateMergeState::default()
    };
    if !is_git_repo(repo) {
        return state;
    }
    state.checkout_branch = git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty());
    if let Some(worktree) = existing_merge_worktree(repo) {
        state.worktree_exists = true;
        state.pending_files =
            pending_files(&worktree, configured_root(repo).ok().flatten().as_ref());
        state.worktree_path = Some(worktree.to_string_lossy().into_owned());
    }
    state.branch_exists = branch_exists(repo);
    if state.branch_exists {
        // Unknown counts as one: the branch is never reported as integrated by guess.
        state.branch_ahead_commits = merge_branch_ahead(repo).unwrap_or(1);
    }
    state
}

/// `error_code` of the refusal below.
pub(crate) const PENDING_INTEGRATION_CODE: &str = "template_merge_pending_integration";

/// Refuse audits of a checkout that does not yet contain accepted template changes.
pub(crate) fn audit_launch_refusal(repo: &Path) -> Option<serde_json::Value> {
    (branch_exists(repo) && merge_branch_unintegrated(repo)).then(|| serde_json::json!({
        "error": format!(
            "Template merge `{}` is pending integration — integrate its accepted commits into this checkout before launching an audit.",
            docs_merge::MERGE_BRANCH,
        ),
        "error_code": PENDING_INTEGRATION_CODE,
    }))
}

/// The merge was integrated into the checkout (or its remote): its worktree
/// holds nothing to review and its branch nothing to integrate. Both go, so
/// the offer and the pending state end with the integration. Nothing is lost:
/// a worktree with proposed or uncommitted files, or a branch some commit of
/// which neither the checkout nor the remote default branch contains, stays.
pub(crate) fn clean_integrated_merge(repo: &Path) {
    if !is_git_repo(repo) || !branch_exists(repo) || merge_branch_unintegrated(repo) {
        return;
    }
    if let Some(worktree) = existing_merge_worktree(repo) {
        let clean = git(&worktree, &["status", "--porcelain", "-uall"])
            .is_ok_and(|out| out.trim().is_empty());
        if !clean {
            return;
        }
        if let Err(e) = crate::core::worktree::remove_discussion_worktree(
            repo,
            &worktree.to_string_lossy(),
            true,
        ) {
            tracing::warn!("Integrated template merge worktree not removed: {e}");
        }
        return;
    }
    // `branch -d` refuses a branch HEAD does not contain: the safety net.
    let _ = git(repo, &["branch", "-d", docs_merge::MERGE_BRANCH]);
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

/// The one definition of "proposed by the merge": files under the docs dir that
/// git sees as untracked in the merge worktree (sorted, ignore rules applied).
/// The apply view, the pending count and `accept_merge` all read this.
fn proposed_files(worktree: &Path, docs_dir: &str) -> Result<Vec<String>, String> {
    let mut files = untracked_under(worktree, docs_dir)?;
    files.sort();
    Ok(files)
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
    clean_integrated_merge(repo);
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
    let plan = plan_on(
        &worktree,
        &existing,
        template_docs,
        configured_root(repo)?.as_ref(),
    )?;
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

    // The proposal is exactly what `accept_merge` accepts (`proposed_files`): a
    // planned write git does not see as an untracked file — the local-excluded
    // `docs/var/.gitkeep` scratch placeholder, or a file an earlier accept already
    // committed — is on disk but is not proposed (KT-1359).
    let mut files: Vec<MergeFileDiff> = Vec::new();
    for path in proposed_files(&worktree, &plan.docs_dir)? {
        let content = std::fs::read_to_string(worktree.join(&path)).unwrap_or_default();
        let outcome = outcomes
            .iter()
            .find(|(p, _)| p == &path)
            .map_or(WriteOutcome::Created, |(_, o)| *o);
        let imported = plan
            .writes
            .iter()
            .find(|w| w.path == path)
            .map_or(0, |w| w.imported_sections);
        files.push(MergeFileDiff {
            diff: docs_merge::addition_diff(&path, &content),
            path,
            outcome: Some(outcome),
            imported_sections: imported,
        });
    }

    let mut out = view(&plan, base, base_ref, files, untracked_in_checkout);
    out.worktree_path = Some(worktree.to_string_lossy().into_owned());
    out.branch = Some(docs_merge::MERGE_BRANCH.to_string());
    out.resumed = resumed;
    out.merge = merge_state(repo);
    if base == MergeBase::Remote {
        out.warnings = unpushed_docs_warnings(repo);
    }
    Ok(out)
}

/// A remote base cannot contain what exists only here: docs committed but not
/// pushed, or untracked. The merge would not map them, and they would later
/// collide (add/add) with the v2 files it creates.
fn unpushed_docs_warnings(repo: &Path) -> Vec<TemplateMergeWarning> {
    let mut local_only: BTreeSet<String> = BTreeSet::new();
    if let Some(remote) = known_remote_default_ref(repo) {
        let range = format!("{remote}...HEAD");
        if let Ok(raw) = git(
            repo,
            &[
                "-c",
                "core.quotepath=off",
                "diff",
                "--name-only",
                "-z",
                &range,
            ],
        ) {
            let changed: BTreeSet<String> = nul_list(&raw).into_iter().collect();
            local_only.extend(docs_merge::source_paths(&changed));
        }
    }
    local_only.extend(untracked_docs(repo));
    if local_only.is_empty() {
        return Vec::new();
    }
    let paths: Vec<String> = local_only.iter().take(10).cloned().collect();
    let more = (local_only.len() - paths.len()) as u32;
    vec![TemplateMergeWarning {
        code: REMOTE_BASE_MISSES_LOCAL_DOCS.to_string(),
        paths,
        more,
    }]
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
    let configured = configured_root(repo)?;
    let docs_dir = docs_merge::choose_docs_dir(&existing, configured.as_ref().map(DocsRoot::rel));
    let proposed: BTreeSet<String> = proposed_files(&worktree, &docs_dir)?.into_iter().collect();
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
    // The files this commit brings are Kronn's: recorded in the folder's state
    // file, committed with them, so they stay Kronn's once integrated and the
    // audit gate does not take them (nor the folder) for a human's (KT-1310).
    let prefix = format!("{docs_dir}/");
    let written: Vec<String> = paths
        .iter()
        .filter_map(|p| p.strip_prefix(&prefix).map(str::to_string))
        .collect();
    crate::core::kronn_state::record_template_merge(&worktree.join(&docs_dir), &written)?;
    let state_file = format!("{prefix}{}", crate::core::kronn_state::KRONN_STATE_FILENAME);
    let mut committed_paths: Vec<&str> = paths.iter().map(String::as_str).collect();
    committed_paths.push(&state_file);
    let mut add = vec!["add", "--"];
    add.extend(committed_paths.iter().copied());
    git(&worktree, &add)?;
    let mut commit = vec![
        "commit",
        "-m",
        "docs: merge the Kronn AI-doc template v2",
        "--",
    ];
    commit.extend(committed_paths.iter().copied());
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

/// GET /api/projects/:id/template-merge/plan — read-only correspondence table, diffs and merge state.
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
    let lease = match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &id).await {
        Ok(lease) => lease,
        Err(error) => return respond(Err(error)),
    };
    let result = match project_root(&state, id).await {
        Ok((root, name)) => lease
            .blocking(move || {
                let template = templates_docs_dir()?;
                run_merge_in_worktree(&root, &name, base, &template)
            })
            .await
            .unwrap_or_else(|error| Err(format!("Task failed: {error}"))),
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
    let lease = match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &id).await {
        Ok(lease) => lease,
        Err(error) => return respond(Err(error)),
    };
    let result = match project_root(&state, id).await {
        Ok((root, _)) => lease
            .blocking(move || accept_merge(&root, &body.paths))
            .await
            .unwrap_or_else(|error| Err(format!("Task failed: {error}"))),
        Err(e) => Err(e),
    };
    respond(result)
}

/// DELETE /api/projects/:id/template-merge — drop the merge worktree and its branch.
pub async fn template_merge_discard(
    State(state): State<AppState>,
    AxPath(id): AxPath<String>,
) -> Json<ApiResponse<()>> {
    let lease = match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &id).await {
        Ok(lease) => lease,
        Err(error) => return respond(Err(error)),
    };
    let result = match project_root(&state, id).await {
        Ok((root, _)) => lease
            .blocking(move || discard_merge(&root))
            .await
            .unwrap_or_else(|error| Err(format!("Task failed: {error}"))),
        Err(e) => Err(e),
    };
    respond(result)
}

#[cfg(test)]
#[path = "template_merge_test.rs"]
mod tests;
