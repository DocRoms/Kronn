//! KT-1360 — an audit step stays inside its remit.
//!
//! An audit writes documentation under the project's docs root and nothing
//! else. Two things escaped that in practice, and the prompt alone cannot be
//! trusted to stop either:
//!
//! - an agent following the repository's own commit conventions committed its
//!   work: the audit left commits in the user's history (a push would be one
//!   command away). Kronn leaves the changes uncommitted for the user's review;
//! - with the docs root moved (`.ai`), an agent followed the TD sheets of the
//!   old `docs/` folder and edited them, writing nothing under the new root.
//!
//! [`StepRemit::capture`] records, before the agent runs, where HEAD and the
//! branch stand and which tracked files outside the root were already dirty.
//! [`StepRemit::settle`] runs when the attempt is over:
//!
//! - HEAD moved by commits that all sit on top of the recorded HEAD, on the
//!   same branch, authored during the step, and that change nothing but the
//!   docs root, Kronn-owned pointers and another AI-doc folder → `git reset
//!   --soft` back to the recorded HEAD (the changes stay in the index and the
//!   worktree, nothing is lost). Any other move (a user commit, a commit that
//!   touches source, a rebase, a branch switch) is never touched, only
//!   reported;
//! - a tracked file inside ANOTHER AI-doc folder (see [`ai_doc_folders`]) that
//!   the step changed goes back to its pre-step content, after the current
//!   bytes were kept as a git blob. A tracked change anywhere else outside the
//!   root is only reported: Kronn cannot tell the agent's edit from the user's,
//!   who keeps working in the repository while an audit runs.
//!
//! Every action is reported, never silent, and none destroys content that
//! cannot be read back.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::core::docs_location::DocsRoot;

/// Files and folders outside the root that Kronn (or the agent CLI it
/// launches) owns: root pointers, per-agent instruction and MCP config.
pub(crate) const KRONN_OWNED: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    ".cursorrules",
    ".cursor",
    ".windsurfrules",
    ".clinerules",
    ".github/copilot-instructions.md",
    ".github/instructions",
    ".kiro",
    ".vibe",
    ".claude",
    ".codex",
    ".gemini",
    ".opencode",
    ".mcp.json",
    "opencode.json",
    "opencode.jsonc",
    ".kronn",
];

fn kronn_owned(rel: &str) -> bool {
    KRONN_OWNED
        .iter()
        .any(|owned| rel == *owned || rel.strip_prefix(owned).is_some_and(|r| r.starts_with('/')))
}

/// What [`StepRemit::settle`] did and saw.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RemitReport {
    /// Commits the step made, undone by a soft reset (short sha + subject).
    pub commits_undone: Vec<String>,
    /// Files of another AI-doc folder put back to their pre-step content.
    pub reverted: Vec<Reverted>,
    /// What Kronn saw and left alone.
    pub warnings: Vec<String>,
}

/// A file restored by [`StepRemit::settle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reverted {
    pub path: String,
    /// Blob holding the content that was overwritten (`git cat-file -p`);
    /// `None` when the step had deleted the file.
    pub blob: Option<String>,
}

impl std::fmt::Display for Reverted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.blob {
            Some(sha) => write!(
                f,
                "{} (the version before the restore is blob {sha}: `git cat-file -p {sha}`)",
                self.path
            ),
            None => write!(f, "{} (deleted by the step)", self.path),
        }
    }
}

impl RemitReport {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.commits_undone.is_empty() && self.reverted.is_empty() && self.warnings.is_empty()
    }

    /// One line per finding, ready for a step warning.
    pub fn messages(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.commits_undone.is_empty() {
            out.push(format!(
                "the agent committed during the step; Kronn undid {} commit(s) with a soft reset — the changes are kept uncommitted for your review: {}",
                self.commits_undone.len(),
                self.commits_undone.join("; ")
            ));
        }
        if !self.reverted.is_empty() {
            out.push(format!(
                "the agent edited {} tracked file(s) of another AI-doc folder outside the docs root; Kronn restored them: {}",
                self.reverted.len(),
                self.reverted
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.extend(self.warnings.iter().cloned());
        out
    }
}

/// The state of the repository when the step began.
pub(crate) struct StepRemit {
    project: PathBuf,
    /// Project folder inside the repository (`sub/` or empty).
    prefix: String,
    docs_root: DocsRoot,
    head: String,
    branch: Option<String>,
    refs: HashSet<String>,
    /// Unix seconds; a commit authored before it is not the step's.
    started: i64,
    /// Tracked files outside the root that were already modified or staged:
    /// their content at that moment (`None` = absent).
    dirty_outside: HashMap<String, Option<Vec<u8>>>,
    /// Tracked files outside the root that were already modified and could
    /// not be read safely at that moment (a link): never restored.
    refused_before: HashMap<String, String>,
    /// What the index held for each `dirty_outside` file at that moment: a
    /// staged version, a conflict or a flag is part of what the user had.
    index_before: HashMap<String, IndexRecord>,
    /// The skip-worktree / assume-unchanged tag of every flagged file at that
    /// moment: a flag the agent sets on a clean file hides its edit.
    flags_before: HashMap<String, char>,
    /// The repository's local configuration and replace refs at that moment: an
    /// agent that changes them can steer what git reports.
    git_setup_before: String,
    /// Why the state before the step could not be fully read, if it could not.
    capture_gap: Option<String>,
    /// The state before the step could not be read at all (HEAD unreadable in
    /// a repository): the settle only says so.
    unguarded: bool,
    /// Flagged files past FLAGGED_CONTENT_MAX: their bytes were not kept, so an
    /// edit behind a flag kept as it was is not seen.
    flagged_unchecked: usize,
    /// Project-relative folders outside the root that hold AI docs.
    ai_folders: Vec<String>,
    /// The tracked files of those folders stored through a filter (Git LFS)
    /// when the step started, with the filter's name. Read before the agent
    /// ran: an attribute it adds never spares its own edit.
    filtered_before: HashMap<String, String>,
}

/// `git` for the project after an agent ran (`agent_touched_git_cmd`): no
/// program its configuration names runs, no replace ref is followed. Bounded in
/// time: a configuration that blocks git (an include on a FIFO) fails the call
/// instead of hanging the settle.
fn git(project: &Path, args: &[&str]) -> Option<Vec<u8>> {
    git_with_input(project, args, &[])
}

/// What the index holds for one path: every `ls-files -s` line (all stages of
/// a conflict, top-relative paths, as `update-index --index-info` reads them
/// back), the `ls-files -v` tag (lower case = assume-unchanged, `S` =
/// skip-worktree) and whether the entry is an intent-to-add (`git add -N`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct IndexRecord {
    lines: Vec<String>,
    tag: Option<char>,
    intent_to_add: bool,
}

/// The index of the project folder (or of `only`) at one moment: the
/// [`IndexRecord`] of every path, project-relative, in four git calls whatever
/// the size of the repository. A path missing from the map is not in the
/// index. `None` when git cannot tell.
fn index_snapshot(
    project: &Path,
    prefix: &str,
    only: Option<&str>,
) -> Option<HashMap<String, IndexRecord>> {
    let run = |args: &[&str]| {
        let mut all: Vec<&str> = args.to_vec();
        if let Some(rel) = only {
            all.extend(["--", rel]);
        }
        // A path that is not UTF-8 is left out: a lossy copy of it would name
        // another entry when written back.
        git(project, &all).map(|raw| {
            raw.split(|b| *b == 0)
                .filter_map(|entry| std::str::from_utf8(entry).ok())
                .collect::<Vec<_>>()
                .join("\0")
        })
    };
    let mut records: HashMap<String, IndexRecord> = HashMap::new();
    // `<mode> SP <blob> SP <stage> TAB <top-relative path>`, as --index-info reads it.
    for line in run(&["ls-files", "-s", "-z", "--full-name"])?
        .split('\0')
        .filter(|line| !line.is_empty())
    {
        let Some(rel) = line
            .split_once('\t')
            .and_then(|(_, p)| p.strip_prefix(prefix))
        else {
            continue;
        };
        records
            .entry(rel.to_string())
            .or_default()
            .lines
            .push(line.to_string());
    }
    // `<tag> SP <cwd-relative path>`; lower case = assume-unchanged, `S` =
    // skip-worktree, `s` = both.
    for entry in run(&["ls-files", "-v", "-z"])?.split('\0') {
        if let Some((tag, rel)) = entry.split_once(' ') {
            if let Some(record) = records.get_mut(rel) {
                record.tag.get_or_insert(tag.chars().next().unwrap_or('H'));
            }
        }
    }
    // An intent-to-add entry holds the empty blob like a staged empty file;
    // only the cached diff tells them apart: listed (A or M) when the entry is
    // made visible, absent (or a deletion, over a path HEAD has) otherwise.
    let statuses = |flag: &str| -> Option<HashMap<String, String>> {
        let raw = run(&[
            "diff",
            "--cached",
            "--name-status",
            "--no-renames",
            "--ignore-submodules=all",
            "-z",
            flag,
        ])?;
        let mut fields = raw.split('\0').filter(|f| !f.is_empty());
        let mut out = HashMap::new();
        while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
            if let Some(rel) = path.strip_prefix(prefix) {
                out.insert(rel.to_string(), status.to_string());
            }
        }
        Some(out)
    };
    let visible = statuses("--ita-visible-in-index")?;
    let invisible = statuses("--ita-invisible-in-index")?;
    for (rel, record) in records.iter_mut() {
        record.intent_to_add = matches!(visible.get(rel).map(String::as_str), Some("A" | "M"))
            && matches!(invisible.get(rel).map(String::as_str), None | Some("D"));
    }
    Some(records)
}

/// The [`IndexRecord`] of `rel` (project-relative); `None` when git cannot tell.
fn index_record(project: &Path, prefix: &str, rel: &str) -> Option<IndexRecord> {
    index_snapshot(project, prefix, Some(rel)).map(|mut all| all.remove(rel).unwrap_or_default())
}

/// The flagged files of a snapshot (skip-worktree or assume-unchanged): `git
/// status` does not show them.
fn flags_of(snapshot: &HashMap<String, IndexRecord>) -> HashMap<String, char> {
    snapshot
        .iter()
        .filter_map(|(rel, record)| {
            let tag = record.tag?;
            (tag == 'S' || tag.is_ascii_lowercase()).then(|| (rel.clone(), tag))
        })
        .collect()
}

/// Above this many flagged files (a sparse checkout, `core.ignoreStat`), their
/// bytes are not kept: only their index state is.
const FLAGGED_CONTENT_MAX: usize = 2_000;

/// The largest file whose bytes the remit keeps or compares: a larger one is
/// never read whole into memory; it is left as it is and reported when it
/// changed.
pub(crate) const REMIT_CONTENT_MAX: u64 = 64 * 1024 * 1024;

/// The bytes of `rel` through the rooted primitive (`None` when absent), or an
/// error for a link, a non-regular entry or a file past [`REMIT_CONTENT_MAX`].
fn read_kept(project: &Path, rel: &str) -> std::io::Result<Option<Vec<u8>>> {
    let path = project.join(rel);
    let bytes = crate::core::fs_guard::guarded_read_head(project, &path, REMIT_CONTENT_MAX + 1)?;
    if bytes
        .as_ref()
        .is_some_and(|bytes| bytes.len() as u64 > REMIT_CONTENT_MAX)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!("larger than {} MiB", REMIT_CONTENT_MAX / (1024 * 1024)),
        ));
    }
    Ok(bytes)
}

/// A regular entry's identity: its type, size and modification time, plus (on
/// unix) the change time, inode and device, which a same-size edit with its
/// time put back still moves.
fn entry_fingerprint(meta: &std::fs::Metadata) -> String {
    let base = format!(
        "{:?} {} {:?}",
        meta.file_type(),
        meta.len(),
        meta.modified().ok()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{base} {}.{} {}:{}",
            meta.ctime(),
            meta.ctime_nsec(),
            meta.dev(),
            meta.ino()
        )
    }
    #[cfg(not(unix))]
    base
}

/// What a refused path is, by `lstat` of each component from the project down,
/// stopping at the first link (its target is recorded, never followed): a
/// parent folder that is a link is never stat'ed through. A changed
/// fingerprint means the step touched it.
fn link_fingerprint(project: &Path, rel: &str) -> String {
    let mut at = project.to_path_buf();
    let mut kind = String::new();
    for part in Path::new(rel).components() {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return format!(
                    "link at {}: {:?}",
                    at.display(),
                    std::fs::read_link(&at).ok()
                );
            }
            Ok(meta) => kind = entry_fingerprint(&meta),
            Err(error) => return format!("missing at {}: {:?}", at.display(), error.kind()),
        }
    }
    kind
}

/// `git <args>` with `input` on stdin, through `agent_touched_git_cmd` and
/// bounded by `run_bounded`.
fn git_with_input(project: &Path, args: &[&str], input: &[u8]) -> Option<Vec<u8>> {
    let mut literal = vec!["--literal-pathspecs"];
    literal.extend_from_slice(args);
    crate::core::cmd::agent_git_output(project, &literal, input)
}

/// The repository's effective configuration and its replace refs, as text:
/// what an agent changes to steer git's answers.
fn git_setup(project: &Path) -> String {
    // The effective configuration (local, worktree, includes, with its scope):
    // any change to it during the step is reported.
    let config = git(project, &["config", "--list", "--show-scope"]).unwrap_or_default();
    let replace = git(
        project,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/replace",
        ],
    )
    .unwrap_or_default();
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&config),
        String::from_utf8_lossy(&replace)
    )
}

fn git_text(project: &Path, args: &[&str]) -> Option<String> {
    git(project, args).map(|o| String::from_utf8_lossy(&o).trim().to_string())
}

/// Tracked paths with a pending change, project-relative: `(path, ...)`
/// from `git status -z`. Untracked and ignored entries are not listed.
fn tracked_changes(project: &Path, prefix: &str) -> Option<Vec<String>> {
    let raw = git(
        project,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=no",
            "--ignore-submodules=all",
        ],
    )?;
    // Parsed on bytes: `XY SP path NUL`, and a rename or copy carries its origin
    // as one more NUL field, always consumed. A path that is not UTF-8 is left
    // out (never written back under a lossy name).
    let mut fields = raw.split(|b| *b == 0);
    let mut out = Vec::new();
    while let Some(entry) = fields.next() {
        if entry.len() < 4 || entry[2] != b' ' {
            continue;
        }
        let status = &entry[..2];
        let mut paths = vec![&entry[3..]];
        if status.contains(&b'R') || status.contains(&b'C') {
            paths.extend(fields.next());
        }
        for path in paths {
            if let Some(rel) = std::str::from_utf8(path)
                .ok()
                .and_then(|p| p.strip_prefix(prefix))
            {
                out.push(rel.to_string());
            }
        }
    }
    Some(out)
}

fn local_refs(project: &Path) -> HashSet<String> {
    git_text(
        project,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/tags",
            "refs/replace",
        ],
    )
    .map(|t| t.lines().map(str::to_string).collect())
    .unwrap_or_default()
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(9)]
}

/// Whether `<dir>/AGENTS.md` opens with Kronn's signature, read through the
/// rooted primitive (a link is refused, a FIFO never blocks) and at most 64 KiB.
fn kronn_signed(project: &Path, dir: &str) -> bool {
    let path = project.join(dir).join("AGENTS.md");
    crate::core::fs_guard::guarded_read_head(project, &path, 64 * 1024)
        .ok()
        .flatten()
        .is_some_and(|bytes| {
            crate::core::legacy_docs::is_kronn_signature(&String::from_utf8_lossy(&bytes))
        })
}

/// Folders outside the docs root that hold AI documentation written by Kronn:
/// the folder of a `.kronn.json`, of a Kronn-signed `AGENTS.md` or of an
/// `inconsistencies-*.md`, and the parent of a `tech-debt/TD-*.md`. Read from
/// the tracked files before the step, so the agent cannot define them. The
/// project folder itself and Kronn-owned paths never count.
/// The tracked files of `folders` with a `filter` attribute (Git LFS), and the
/// filter's name. With filters off such a file's bytes always differ from its
/// blob, and the raw blob (an LFS pointer) is not its content. `None` when git
/// cannot tell.
fn filtered_files(project: &Path, folders: &[String]) -> Option<HashMap<String, String>> {
    if folders.is_empty() {
        return Some(HashMap::new());
    }
    let mut args = vec!["ls-files", "-z", "--"];
    args.extend(folders.iter().map(String::as_str));
    let listed = git(project, &args)?;
    if listed.is_empty() {
        return Some(HashMap::new());
    }
    let answer = git_with_input(project, &["check-attr", "-z", "--stdin", "filter"], &listed)?;
    // `<path> NUL filter NUL <value> NUL` per file.
    let fields: Vec<&[u8]> = answer.split(|b| *b == 0).collect();
    Some(
        fields
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|[_, _, value]| !matches!(*value, b"unspecified" | b"unset"))
            .filter_map(|[path, _, value]| {
                let rel = std::str::from_utf8(path).ok()?;
                Some((rel.to_string(), String::from_utf8_lossy(value).into_owned()))
            })
            .collect(),
    )
}

fn ai_doc_folders(project: &Path, docs_root: &DocsRoot) -> Vec<String> {
    let listed = git(project, &["ls-files", "-z"]).unwrap_or_default();
    let mut folders: Vec<String> = Vec::new();
    for rel in listed.split(|b| *b == 0).map(String::from_utf8_lossy) {
        if docs_root.contains(&rel) || kronn_owned(&rel) {
            continue;
        }
        let Some((dir, name)) = rel.rsplit_once('/') else {
            continue;
        };
        let folder = match name {
            ".kronn.json" => dir,
            "AGENTS.md" if kronn_signed(project, dir) => dir,
            n if n.starts_with("inconsistencies-") && n.ends_with(".md") => dir,
            n if n.starts_with("TD-") && n.ends_with(".md") => match dir.rsplit_once('/') {
                Some((parent, "tech-debt")) => parent,
                _ => continue,
            },
            _ => continue,
        };
        if !folder.is_empty() && !folders.iter().any(|f| f == folder) {
            folders.push(folder.to_string());
        }
    }
    folders
}

impl StepRemit {
    /// Record the state before the agent runs. `None` outside a git repository
    /// or before its first commit: there is no history to protect.
    pub fn capture_blocking(project: &Path, docs_root: &DocsRoot) -> Option<Self> {
        let Some(head) = git_text(project, &["rev-parse", "--verify", "-q", "HEAD"]) else {
            // Outside a repository (or before its first commit) there is no
            // history to protect; inside one, an unreadable HEAD (an orphan
            // branch, a configuration git cannot read in time) is said, never
            // a silent unguarded step.
            if !project
                .ancestors()
                .any(|dir| dir.join(".git").symlink_metadata().is_ok())
                || git(project, &["rev-parse", "--git-dir"]).is_some()
                    && git(project, &["rev-parse", "--verify", "-q", "HEAD^{commit}"]).is_none()
                    && git(project, &["rev-list", "-n", "1", "--all"])
                        .is_some_and(|out| out.is_empty())
            {
                return None;
            }
            return Some(Self::unguarded(project, docs_root));
        };
        let prefix = git_text(project, &["rev-parse", "--show-prefix"]).unwrap_or_default();
        let branch = git_text(project, &["symbolic-ref", "-q", "HEAD"]);
        let mut dirty_outside = HashMap::new();
        let mut refused_before = HashMap::new();
        let mut index_before = HashMap::new();
        let mut capture_gap = None;
        let snapshot = index_snapshot(project, &prefix, None).unwrap_or_else(|| {
            capture_gap = Some("the index could not be read".to_string());
            HashMap::new()
        });
        let flags_before = flags_of(&snapshot);
        let mut dirty = tracked_changes(project, &prefix).unwrap_or_else(|| {
            capture_gap = Some("`git status` failed".to_string());
            Vec::new()
        });
        // A file the user hid from `git status` (skip-worktree, assume-unchanged)
        // may hold local changes too: recorded like a dirty one, its bytes kept
        // unless there are too many to keep.
        let keep_flagged = flags_before.len() <= FLAGGED_CONTENT_MAX;
        let mut flagged: Vec<&String> = flags_before.keys().collect();
        flagged.sort();
        dirty.extend(flagged.into_iter().cloned());
        let mut seen = HashSet::new();
        dirty.retain(|rel| seen.insert(rel.clone()));
        for rel in dirty {
            if docs_root.contains(&rel) || kronn_owned(&rel) {
                continue;
            }
            let record = snapshot.get(&rel).cloned().unwrap_or_default();
            if !keep_flagged && flags_before.contains_key(&rel) {
                index_before.insert(rel, record);
                continue;
            }
            // KT-1595: read through the rooted primitive. A link (or anything
            // that is not a readable regular file, or one too large to keep) is
            // recorded as refused, not as an absent file: settle never rewrites
            // or deletes through it.
            index_before.insert(rel.clone(), record);
            match read_kept(project, &rel) {
                Ok(content) => {
                    dirty_outside.insert(rel, content);
                }
                Err(_) => {
                    let fingerprint = link_fingerprint(project, &rel);
                    refused_before.insert(rel, fingerprint);
                }
            }
        }
        let flagged_unchecked = if keep_flagged { 0 } else { flags_before.len() };
        let ai_folders = ai_doc_folders(project, docs_root);
        let filtered_before = filtered_files(project, &ai_folders).unwrap_or_else(|| {
            capture_gap =
                Some("the attributes of the AI-doc folders could not be read".to_string());
            HashMap::new()
        });
        Some(Self {
            project: project.to_path_buf(),
            prefix,
            docs_root: docs_root.clone(),
            head,
            branch,
            refs: local_refs(project),
            started: chrono::Utc::now().timestamp(),
            dirty_outside,
            refused_before,
            index_before,
            flags_before,
            git_setup_before: git_setup(project),
            capture_gap,
            unguarded: false,
            flagged_unchecked,
            ai_folders,
            filtered_before,
        })
    }

    /// A remit for a repository whose state could not be read before the step:
    /// its settle only reports that the step ran without the check.
    fn unguarded(project: &Path, docs_root: &DocsRoot) -> Self {
        Self {
            project: project.to_path_buf(),
            prefix: String::new(),
            docs_root: docs_root.clone(),
            head: String::new(),
            branch: None,
            refs: HashSet::new(),
            started: chrono::Utc::now().timestamp(),
            dirty_outside: HashMap::new(),
            refused_before: HashMap::new(),
            index_before: HashMap::new(),
            flags_before: HashMap::new(),
            git_setup_before: String::new(),
            capture_gap: Some("HEAD could not be read".to_string()),
            unguarded: true,
            flagged_unchecked: 0,
            ai_folders: Vec::new(),
            filtered_before: HashMap::new(),
        }
    }

    fn in_ai_folder(&self, rel: &str) -> bool {
        self.ai_folders.iter().any(|f| {
            rel.strip_prefix(f.as_str())
                .is_some_and(|r| r.starts_with('/'))
        })
    }

    /// A path a step may legitimately touch, or whose restoration is Kronn's.
    fn within_remit(&self, rel: &str) -> bool {
        self.docs_root.contains(rel) || kronn_owned(rel) || self.in_ai_folder(rel)
    }

    pub async fn capture(project: &Path, docs_root: &DocsRoot) -> Option<Arc<Self>> {
        let (project, docs_root) = (project.to_path_buf(), docs_root.clone());
        tokio::task::spawn_blocking(move || Self::capture_blocking(&project, &docs_root))
            .await
            .ok()
            .flatten()
            .map(Arc::new)
    }

    pub async fn settle(self: &Arc<Self>) -> RemitReport {
        let this = Arc::clone(self);
        tokio::task::spawn_blocking(move || this.settle_blocking())
            .await
            .unwrap_or_default()
    }

    /// Put the repository back inside the step's remit. Idempotent: run after
    /// every attempt, a second call finds nothing left to do.
    pub fn settle_blocking(&self) -> RemitReport {
        let mut report = RemitReport::default();
        if self.unguarded {
            report.warnings.push(
                "the repository's HEAD could not be read before the step (an orphan branch or a configuration git cannot read): this step ran without the check of commits and of the files outside the docs root"
                    .to_string(),
            );
            return report;
        }
        if let Some(gap) = &self.capture_gap {
            report.warnings.push(format!(
                "the repository could not be read before the step ({gap}): the check of the files outside the docs root is incomplete"
            ));
        }
        if self.flagged_unchecked > 0 {
            report.warnings.push(format!(
                "{} files are flagged skip-worktree / assume-unchanged (more than {FLAGGED_CONTENT_MAX}): their index state is checked, their content is not",
                self.flagged_unchecked
            ));
        }
        if git_setup(&self.project) != self.git_setup_before {
            report.warnings.push(
                "the repository's git configuration or replace refs changed during the step: Kronn turned off every program it names for its own checks, but review the change (`git config --local --list`, `git for-each-ref refs/replace`)"
                    .to_string(),
            );
        }
        let head_settled = self.settle_head(&mut report);
        self.report_new_refs(&mut report);
        if head_settled {
            self.revert_outside_root(&mut report);
        } else if git_text(&self.project, &["rev-parse", "--verify", "-q", "HEAD"]).is_some() {
            report.warnings.push(
                "HEAD moved during the step and was left as it is: the tracked files outside the docs root were not checked"
                    .to_string(),
            );
        }
        report
    }

    /// Whether HEAD is back at (or never left) the recorded commit.
    fn settle_head(&self, report: &mut RemitReport) -> bool {
        let now = match git_text(&self.project, &["rev-parse", "--verify", "-q", "HEAD"]) {
            Some(now) => now,
            None => {
                report
                    .warnings
                    .push("HEAD could not be read after the step".to_string());
                return false;
            }
        };
        if now == self.head {
            return true;
        }
        let branch = git_text(&self.project, &["symbolic-ref", "-q", "HEAD"]);
        if branch != self.branch {
            report.warnings.push(format!(
                "the checked-out branch changed during the step ({} → {}); Kronn left it as it is",
                self.branch.as_deref().unwrap_or("detached"),
                branch.as_deref().unwrap_or("detached"),
            ));
            return false;
        }
        let range = format!("{}..{now}", self.head);
        let descends = git(
            &self.project,
            &["merge-base", "--is-ancestor", &self.head, &now],
        )
        .is_some();
        // `%H %at %P`: only plain commits on a straight line are the step's.
        let log = git_text(
            &self.project,
            &["log", "--format=%H%x09%at%x09%P%x09%s", &range],
        );
        let commits: Vec<Vec<String>> = log
            .as_deref()
            .unwrap_or_default()
            .lines()
            .map(|l| l.splitn(4, '\t').map(str::to_string).collect())
            .collect();
        let all_the_step_s = descends
            && !commits.is_empty()
            && commits.iter().all(|c| {
                c.len() == 4
                    && c[1].parse::<i64>().is_ok_and(|at| at >= self.started)
                    && c[2].split_whitespace().count() == 1
            });
        let foreign = if all_the_step_s {
            self.foreign_paths(&commits)
        } else {
            Vec::new()
        };
        if !foreign.is_empty() {
            report.warnings.push(format!(
                "HEAD moved during the step ({} → {}) by commits that touch files beyond the docs root and the AI-doc folders ({}); they may be yours, so Kronn left them as they are",
                short(&self.head),
                short(&now),
                foreign.join(", "),
            ));
            return false;
        }
        if !all_the_step_s {
            report.warnings.push(format!(
                "HEAD moved during the step ({} → {}) by commits that are not all the step's own; Kronn left them as they are",
                short(&self.head),
                short(&now),
            ));
            return false;
        }
        if git(&self.project, &["reset", "--soft", &self.head]).is_none() {
            report.warnings.push(format!(
                "the agent committed during the step and Kronn could not undo it (git reset --soft {} failed)",
                short(&self.head)
            ));
            return false;
        }
        report.commits_undone.extend(
            commits
                .iter()
                .rev()
                .map(|c| format!("{} {}", short(&c[0]), c[3])),
        );
        true
    }

    /// Paths, in the commits of the range, that the audit has no business
    /// changing (sorted, a handful at most).
    fn foreign_paths(&self, commits: &[Vec<String>]) -> Vec<String> {
        let mut foreign = std::collections::BTreeSet::new();
        for c in commits {
            let Some(raw) = git(
                &self.project,
                &[
                    "diff-tree",
                    "--no-commit-id",
                    "--name-only",
                    "-r",
                    "-z",
                    &c[0],
                ],
            ) else {
                foreign.insert(format!("(commit {} unreadable)", short(&c[0])));
                continue;
            };
            for path in raw.split(|b| *b == 0).filter(|p| !p.is_empty()) {
                let path = String::from_utf8_lossy(path);
                match path.strip_prefix(self.prefix.as_str()) {
                    Some(rel) if self.within_remit(rel) => {}
                    _ => {
                        foreign.insert(path.into_owned());
                    }
                }
            }
        }
        foreign.into_iter().take(8).collect()
    }

    fn report_new_refs(&self, report: &mut RemitReport) {
        let mut created: Vec<String> = local_refs(&self.project)
            .difference(&self.refs)
            .cloned()
            .collect();
        created.sort();
        if !created.is_empty() {
            report.warnings.push(format!(
                "the agent created {} during the step; an audit never creates branches or tags — Kronn left them for you to delete",
                created.join(", ")
            ));
        }
    }

    fn revert_outside_root(&self, report: &mut RemitReport) {
        let Some(mut changed) = tracked_changes(&self.project, &self.prefix) else {
            report.warnings.push(
                "`git status` failed after the step: the tracked files outside the docs root were not checked"
                    .to_string(),
            );
            return;
        };
        // A file the user had dirty that the agent brought back to HEAD (`git
        // restore`, `checkout HEAD --`) or hid (`--assume-unchanged`) is no longer
        // listed by `git status`: it is looked at all the same.
        let mut recorded: Vec<&String> = self.dirty_outside.keys().collect();
        recorded.sort();
        changed.extend(recorded.into_iter().cloned());
        // A flag set or changed during the step hides an edit from `git status`;
        // a refused path (a link) the agent replaced is gone from it too.
        let snapshot = index_snapshot(&self.project, &self.prefix, None).unwrap_or_else(|| {
            report.warnings.push(
                "the index could not be read after the step: staged versions and flags were not checked"
                    .to_string(),
            );
            HashMap::new()
        });
        let flags_now = flags_of(&snapshot);
        let mut flagged: Vec<String> = flags_now
            .keys()
            .chain(self.flags_before.keys())
            .filter(|rel| flags_now.get(*rel) != self.flags_before.get(*rel))
            .cloned()
            .collect();
        flagged.sort();
        changed.extend(flagged);
        let mut refused: Vec<(&String, &String)> = self.refused_before.iter().collect();
        refused.sort();
        for (rel, fingerprint) in refused {
            if link_fingerprint(&self.project, rel) != *fingerprint {
                changed.push(rel.clone());
            }
        }
        let index_now = |rel: &str| snapshot.get(rel).cloned().unwrap_or_default();
        let mut seen = HashSet::new();
        let mut left_alone = Vec::new();
        for rel in changed {
            if self.docs_root.contains(&rel) || kronn_owned(&rel) || !seen.insert(rel.clone()) {
                continue;
            }
            let before = self.dirty_outside.get(&rel);
            if !self.in_ai_folder(&rel) {
                // Source, config, anything else: the user works here too.
                // KT-1595: read through the rooted primitive; a refused read
                // (link) counts as changed, never as the content it points at.
                // The index counts too: a version the agent staged is a change.
                let unchanged = match before {
                    Some(b) => read_kept(&self.project, &rel).is_ok_and(|now| *b == now),
                    None => self
                        .refused_before
                        .get(&rel)
                        .is_some_and(|fp| link_fingerprint(&self.project, &rel) == *fp),
                } && self.index_before.get(&rel) == Some(&index_now(&rel));
                if !unchanged {
                    left_alone.push(rel);
                }
                continue;
            }
            if let Some(fingerprint) = self.refused_before.get(&rel) {
                // Its bytes were not kept, its index state was: an index change
                // the agent made is put back.
                if self.index_before.get(&rel) != Some(&index_now(&rel)) {
                    match self.restore_index(&rel) {
                        Ok(false) => {}
                        Ok(true) => report.warnings.push(format!(
                            "the agent changed the index state of `{rel}`; it was put back (the working tree was left as it is)"
                        )),
                        Err(e) => report.warnings.push(e),
                    }
                }
                if link_fingerprint(&self.project, &rel) != *fingerprint {
                    report.warnings.push(format!(
                        "`{rel}` could not be kept before the step (a link, or a file larger than {} MiB) and changed during it; it was left as it is",
                        REMIT_CONTENT_MAX / (1024 * 1024)
                    ));
                }
                continue;
            }
            if before.is_none() && self.index_before.contains_key(&rel) {
                // A flagged file whose bytes were not kept (more than
                // FLAGGED_CONTENT_MAX flagged files): its index state comes back,
                // its working tree is never rewritten from HEAD.
                match self.restore_index(&rel) {
                    Ok(true) => report.warnings.push(format!(
                        "the agent changed the index state of `{rel}`; it was put back (its bytes were not recorded, the working tree was left as it is)"
                    )),
                    Ok(false) => report.warnings.push(format!(
                        "`{rel}` changed during the step; its bytes were not recorded (too many flagged files), so it was left as it is"
                    )),
                    Err(e) => report.warnings.push(e),
                }
                continue;
            }
            if before.is_none() && !self.index_before.contains_key(&rel) && !self.at_head(&rel) {
                report.warnings.push(format!(
                    "the agent added `{rel}` in another AI-doc folder during the step; it was left as it is (check `git status` and `git diff --cached -- {rel}`)"
                ));
                continue;
            }
            let restored = match before {
                // A user change that was already there: back to that content,
                // and its index entry back to what the user had staged.
                Some(before) => {
                    let restored = self.restore_content(&rel, before.as_deref());
                    // What the user had staged comes back even when the content
                    // could not be restored.
                    match self.restore_index(&rel) {
                        Ok(true) if matches!(restored, Ok(None)) => report.warnings.push(format!(
                            "the agent changed the index entry of `{rel}`; it was put back to what you had staged"
                        )),
                        Ok(_) => {}
                        Err(e) => report.warnings.push(e),
                    }
                    restored
                }
                // Clean before the step: back to the recorded commit.
                None => self.restore_from_head(&rel),
            };
            match restored {
                Ok(Some(blob)) => report.reverted.push(Reverted { path: rel, blob }),
                Ok(None) => {}
                Err(e) => report.warnings.push(format!(
                    "`{rel}` was changed outside the docs root and could not be restored: {e}"
                )),
            }
        }
        if !left_alone.is_empty() {
            report.warnings.push(format!(
                "{} tracked file(s) outside the docs root changed during the step and were left untouched (they may be your own edits; only another AI-doc folder is restored): {}",
                left_alone.len(),
                left_alone.join(", ")
            ));
        }
    }

    /// Keep the bytes about to be overwritten as a git blob. `None` when the
    /// file is absent (nothing to keep); an error means do NOT overwrite.
    ///
    /// KT-1595: the bytes are read through the rooted primitive (a link at the
    /// name or in a parent is refused, never followed) and handed to git on
    /// stdin, so git never reads the path itself.
    fn keep_current(&self, rel: &str) -> Result<Option<String>, String> {
        let bytes = match read_kept(&self.project, rel) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(None),
            // A folder or another non-regular entry: not content to keep.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => return Ok(None),
            Err(e) => {
                return Err(format!(
                    "the current content could not be read safely ({e}), so it was not overwritten"
                ))
            }
        };
        self.hash_object(&bytes).map(Some).ok_or_else(|| {
            "the current content could not be kept as a git blob, so it was not overwritten"
                .to_string()
        })
    }

    /// `git hash-object -w --stdin`: the blob id of `bytes`, written.
    fn hash_object(&self, bytes: &[u8]) -> Option<String> {
        let out = git_with_input(
            &self.project,
            &["hash-object", "-w", "--no-filters", "--stdin"],
            bytes,
        )?;
        let sha = String::from_utf8_lossy(&out).trim().to_string();
        (!sha.is_empty()).then_some(sha)
    }

    /// `Some(blob)` when the file was restored, the blob being what it held.
    fn restore_from_head(&self, rel: &str) -> Result<Option<Option<String>>, String> {
        // `./`: relative to the project folder, which may sit below the top.
        let spec = format!("{}:./{rel}", self.head);
        if git(&self.project, &["cat-file", "-e", &spec]).is_none() {
            // Added by the step itself: not content the user had.
            return Ok(None);
        }
        // KT-1595: git never writes the path. The blob's bytes are read with
        // `cat-file` and written through the rooted primitive (a link at the
        // name or in a parent is replaced or refused, never written through),
        // as the pre-step content is in `restore_content`. A link or a
        // submodule at HEAD is not bytes to write: refused, left as it is.
        let (mode, index_mode, blob_id) = self.head_file_entry(rel)?;
        if let Some(filter) = self.filtered_before.get(rel) {
            return Err(format!(
                "it goes through the `{filter}` filter, which Kronn never runs, so an edit cannot be told apart from a refreshed file; left as it is"
            ));
        }
        let bytes = git(&self.project, &["cat-file", "blob", &spec]).ok_or_else(|| {
            format!("the content of `{rel}` at the recorded commit is unreadable")
        })?;
        let blob = self.keep_current(rel)?;
        let path = self.project.join(rel);
        crate::core::fs_guard::guarded_replace(&self.project, &path, &bytes)?;
        #[cfg(unix)]
        crate::core::fs_guard::guarded_set_mode(&self.project, &path, mode)?;
        #[cfg(not(unix))]
        let _ = mode;
        // The working tree is back; so must the index be. After the
        // `reset --soft` of a committed step (and after a `git add` by the
        // agent) the index still holds the agent's version, and the user's
        // next commit would re-commit the reverted edit. `update-index
        // --cacheinfo` sets the entry from the recorded blob WITHOUT touching
        // the path (it takes a top-relative path, hence the prefix).
        let entry = format!("{index_mode},{blob_id},{}{rel}", self.prefix);
        if git(
            &self.project,
            &["update-index", "--add", "--cacheinfo", &entry],
        )
        .is_none()
        {
            return Err(format!(
                "the file was restored but its index entry could not be put back to the recorded commit (git update-index failed); run `git restore --staged -- {rel}`"
            ));
        }
        Ok(Some(blob))
    }

    /// What git records for `rel` at the recorded commit: the permission bits
    /// (`0o755` or `0o644`), the index mode string and the blob id; an error
    /// for a link, a submodule or anything else that is not a plain file.
    fn head_file_entry(&self, rel: &str) -> Result<(u32, &'static str, String), String> {
        let listing = git(&self.project, &["ls-tree", "-z", &self.head, "--", rel])
            .ok_or_else(|| format!("`{rel}` could not be looked up at the recorded commit"))?;
        let listing = String::from_utf8_lossy(&listing);
        // `<mode> SP <type> SP <blob> TAB <path>`
        let mut fields = listing.split(['\t', ' ']);
        let (mode, kind, id) = (fields.next(), fields.next(), fields.next());
        let valid_id = |id: &str| id.len() >= 40 && id.bytes().all(|b| b.is_ascii_hexdigit());
        match (mode, kind, id) {
            (Some("100644"), Some("blob"), Some(id)) if valid_id(id) => {
                Ok((0o644, "100644", id.to_string()))
            }
            (Some("100755"), Some("blob"), Some(id)) if valid_id(id) => {
                Ok((0o755, "100755", id.to_string()))
            }
            _ => Err(format!(
                "`{rel}` is not a plain file at the recorded commit (mode {}); left as it is",
                mode.unwrap_or("unknown")
            )),
        }
    }

    /// Whether `rel` exists at the recorded commit (project-relative).
    fn at_head(&self, rel: &str) -> bool {
        git(
            &self.project,
            &["cat-file", "-e", &format!("{}:./{rel}", self.head)],
        )
        .is_some()
    }

    /// Puts the index of a file that was dirty before the step back to the
    /// recorded [`IndexRecord`]; `true` when it had changed. Every stage the
    /// agent left is dropped (`--force-remove`), then the recorded lines go back
    /// through `--index-info`, an intent-to-add through `add -N`, and the
    /// assume-unchanged / skip-worktree flag is set again. Nothing reads the path.
    fn restore_index(&self, rel: &str) -> Result<bool, String> {
        let failed = || {
            let staged: Vec<&str> = self
                .index_before
                .get(rel)
                .map(|before| {
                    before
                        .lines
                        .iter()
                        .filter_map(|line| line.split(' ').nth(1))
                        .collect()
                })
                .unwrap_or_default();
            if self
                .index_before
                .get(rel)
                .is_some_and(|before| before.intent_to_add)
            {
                return format!(
                    "the index entry of `{rel}` could not be put back: it was an intent-to-add (`git add -N -- {rel}` puts it back)"
                );
            }
            format!(
                "the index entry of `{rel}` could not be put back (git update-index failed); what you had staged is kept as blob {} (`git update-index --cacheinfo <mode>,<blob>,<path>` puts it back)",
                if staged.is_empty() { "none".to_string() } else { staged.join(", ") }
            )
        };
        let Some(before) = self.index_before.get(rel) else {
            return Err(format!(
                "the index entry of `{rel}` was not recorded before the step; check `git diff --cached -- {rel}`"
            ));
        };
        if index_record(&self.project, &self.prefix, rel).as_ref() == Some(before) {
            return Ok(false);
        }
        git(
            &self.project,
            &["update-index", "--force-remove", "--", rel],
        )
        .ok_or_else(failed)?;
        if before.intent_to_add {
            // `add -N` stages what is at the path: never a folder or a link the
            // agent left there.
            let regular = std::fs::symlink_metadata(self.project.join(rel))
                .is_ok_and(|meta| meta.file_type().is_file());
            if !regular {
                return Err(format!(
                    "`{rel}` was an intent-to-add, but the agent replaced it with something that is not a file; it was not staged again (`git add -N -- {rel}` once the file is back)"
                ));
            }
            git(&self.project, &["add", "-N", "--", rel]).ok_or_else(failed)?;
        } else if !before.lines.is_empty() {
            // NUL-separated (`-z`): a path holding a newline or starting with a
            // quote is one entry, never a second one.
            let mut input = before.lines.join("\0");
            input.push('\0');
            git_with_input(
                &self.project,
                &["update-index", "-z", "--index-info"],
                input.as_bytes(),
            )
            .ok_or_else(failed)?;
        }
        let tag = before.tag.unwrap_or('H');
        if tag == 'S' || tag == 's' {
            git(
                &self.project,
                &["update-index", "--skip-worktree", "--", rel],
            )
            .ok_or_else(failed)?;
        }
        if tag.is_ascii_lowercase() {
            git(
                &self.project,
                &["update-index", "--assume-unchanged", "--", rel],
            )
            .ok_or_else(failed)?;
        }
        if index_record(&self.project, &self.prefix, rel).as_ref() != Some(before) {
            return Err(failed());
        }
        Ok(true)
    }

    fn restore_content(
        &self,
        rel: &str,
        before: Option<&[u8]>,
    ) -> Result<Option<Option<String>>, String> {
        // KT-1595: every step goes through the rooted primitive under the
        // project. A link at the name or in a parent folder (the agent may
        // have swapped an AI-doc folder for a link) is refused: nothing is
        // read, written nor deleted through it.
        use crate::core::fs_guard::{guarded_remove_file, guarded_replace, guarded_write_new};
        let path = self.project.join(rel);
        let now = match read_kept(&self.project, rel) {
            Ok(now) => now,
            Err(e) => {
                // The agent's entry cannot be read safely; the user's version
                // must not be lost with it.
                let kept = before
                    .and_then(|bytes| self.hash_object(bytes))
                    .map(|blob| format!("; your version from before the step is kept as blob {blob} (`git cat-file -p {blob}`)"))
                    .unwrap_or_default();
                return Err(format!("refused, left as it is ({e}){kept}"));
            }
        };
        if now.as_deref() == before {
            return Ok(None);
        }
        let blob = self.keep_current(rel)?;
        match (before, now.is_some()) {
            (Some(bytes), true) => guarded_replace(&self.project, &path, bytes)?,
            (Some(bytes), false) => {
                if !guarded_write_new(&self.project, &path, bytes)? {
                    return Err("an entry appeared at the name meanwhile; left as it is".into());
                }
            }
            (None, _) => match guarded_remove_file(&self.project, &path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            },
        }
        Ok(Some(blob))
    }
}

/// One `step_warning` event per finding, for a pipeline to relay.
pub(crate) fn warning_events(
    step: impl serde::Serialize + Copy,
    file: &str,
    report: &RemitReport,
) -> Vec<axum::response::sse::Event> {
    report
        .messages()
        .into_iter()
        .map(|reason| {
            tracing::warn!("Audit step {}: {reason}", file);
            axum::response::sse::Event::default()
                .event("step_warning")
                .data(
                    serde_json::json!({
                        "step": step, "file": file, "reason": reason,
                        "repaired_from_template": false,
                    })
                    .to_string(),
                )
        })
        .collect()
}

/// The remit told to the agent (KT-1360). Appended to every step prompt.
pub(crate) fn remit_prompt_block(docs_root: &DocsRoot) -> String {
    let root = docs_root.rel();
    format!(
        "## Remit of this audit (strict)\n\
- Write documentation ONLY under `{root}/`. Do not edit source code, configuration or any other tracked file.\n\
- Another AI-doc folder outside `{root}/` (a leftover docs/, ai/, .ai/ or doc/ folder that is not the root) is read-only history: read it as a source, never edit, append to or reuse its files. Existing TD sheets there are not to be updated — create or update the ones under `{root}/tech-debt/` instead. Kronn reports any tracked file you change outside `{root}/` and restores the ones of that other folder.\n\
- NEVER run `git commit`, `git push`, `git tag`, `git branch`, `git checkout`/`git switch`, `git stash`, `git reset` or `git rebase`. Leave every change uncommitted: Kronn leaves them for the user's review, and the repository's own commit conventions (message format, ticket prefix, sign-off) do NOT apply to this audit. Kronn undoes commits made during a step.\n"
    )
}

#[cfg(test)]
#[path = "step_remit_tests.rs"]
mod tests;
