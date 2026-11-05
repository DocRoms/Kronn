use std::path::{Path, PathBuf};

use crate::models::AgentType;

/// Invocation-local policy shared by the adapters and direct CLI route.
#[derive(Debug)]
pub(crate) struct ReadOnlyRepos {
    roots: Vec<PathBuf>,
    /// KT-1491 — checkouts a human granted `code` on: opened writable next to
    /// the working directory, their credential files kept out of reach.
    code_roots: Vec<PathBuf>,
    /// The secrets found in each room root at the launch (selected under the
    /// room-wide settings byte budget), scanned once in `resolve_room` so
    /// `args()` walks nothing.
    scanned: Vec<(PathBuf, Vec<String>)>,
    /// `core.hooksPath` of each code root when it points inside the checkout:
    /// a hook the agent edits runs later, outside its sandbox.
    code_hooks: Vec<PathBuf>,
    /// A room's policy: the secrets of every opened checkout stay unreadable,
    /// whatever its level (`read`, `plan` or `code`).
    guard_secrets: bool,
    /// Use paths relative to the primary working directory when they are shorter.
    /// This is enabled only when the mandatory absolute-path policy would exceed
    /// the room-wide settings byte budget.
    compact_paths: bool,
}

/// What stays out of the agent's reach inside a checkout it may edit: the
/// project's secrets and the MCP configs Kronn writes there (they carry another
/// project's tokens). A code grant opens the checkout's code, never these. They
/// are denied whether or not they exist yet: a file created during the turn (by
/// the agent's shell, or by Kronn writing an MCP config) is as closed as one
/// that was there.
const CODE_ROOT_CREDENTIAL_FILES: &[&str] = &[
    ".env",
    ".mcp.json",
    ".ai/mcp",
    ".kiro/settings/mcp.json",
    ".gemini/settings.json",
    ".vibe/config.toml",
    ".codex/config.toml",
    ".kronn",
];

/// What a later program runs or loads, so a write there is a write outside the
/// sandbox: a Git hook or config (`core.fsmonitor`, `core.sshCommand`), the
/// usual hook managers' files (husky, `.githooks`, lefthook), direnv, the
/// editor's settings. Added to the agent CLIs' own files
/// (`step_remit::KRONN_OWNED`: `.claude` hooks, `.cursor`, `.opencode`,
/// `opencode.json`, the instruction files). The common Git directory of a
/// linked worktree lies outside its checkout, so it is not in `allowWrite`
/// either, unless the granted project is a linked worktree of the room's own
/// repository: then it is `<work_dir>/.git`, writable with the working
/// directory. `core.hooksPath` is read at the launch for a hooks directory
/// elsewhere in the tree (`code_hooks`).
const CODE_ROOT_EXECUTABLE_PATHS: &[&str] = &[
    ".git",
    ".envrc",
    ".vscode",
    ".husky",
    ".githooks",
    "lefthook.yml",
    "lefthook.yaml",
    "lefthook-local.yml",
    ".lefthook",
    ".lefthook.yml",
];

/// File names that are secrets wherever they sit in a checkout (a monorepo's
/// `apps/api/.env`). Mirrors `acp::secret_files::is_secret_file`, as patterns:
/// a deny rule is matched on a name, so a file that does not exist yet is covered.
/// A versioned template (`.env.example`) is refused with the rest here: a deny
/// rule cannot carve out an exception.
const SECRET_FILE_GLOBS: &[&str] = &[
    ".env*",
    "*.env",
    "*.pem",
    "*.key",
    "id_rsa*",
    "id_ed25519*",
    "id_ecdsa*",
    "id_dsa*",
    "*.p12",
    "*.pfx",
    ".netrc",
    ".npmrc",
    ".pgpass",
    "credentials",
    "credentials.json",
    ".mcp.json",
    "encryption_key",
    "human-admin-secret",
    "trust-proof.json",
    "secrets.toml",
    "kronn.db*",
    // `app.env.production`: the name has `.env.` in the middle.
    "*.env.*",
];

/// Directories whose whole content is a secret, wherever they sit (a name, or
/// two names in a row: Kronn's backups).
const SECRET_DIR_NAMES: &[&str] = &[".ssh", ".aws", ".kronn/backups"];

/// Bounds of the scan for the secrets already in a checkout.
const SCAN_MAX_DEPTH: usize = 8;
const SCAN_MAX_ENTRIES: usize = 20_000;
const SCAN_SKIPPED_DIRS: &[&str] = &[".git", "node_modules", "target"];
/// Bound the whole inline settings document, including the vault rules, rather
/// than a count per root. Linux limits one argument to 128 KiB; keeping the room
/// policy below 96 KiB leaves headroom and makes additional code roots share the
/// same finite budget.
pub(crate) const ROOM_SETTINGS_MAX_BYTES: usize = 96 * 1024;

#[derive(Debug, Default)]
struct ScannedSecrets {
    /// Existing secrets no configured pattern names. They must never be dropped:
    /// when literals do not fit, their containing directories are denied.
    unnamed: Vec<PathBuf>,
    /// Existing secrets already protected by a name pattern. Literal paths add
    /// defence in depth only while the shared byte budget has room.
    named: Vec<PathBuf>,
}

/// A Claude permission rule for `tool` on `rel`, below `root`. Absolute roots
/// keep Claude's `//abs/path` spelling; compact roots are relative to the primary
/// working directory.
fn rule(tool: &str, root: &str, rel: &str) -> String {
    let path = if root.starts_with('/') {
        format!("/{root}/{rel}")
    } else {
        format!("{root}/{rel}")
    };
    format!("{tool}({path})")
}

/// Rules refusing `tools` on a name and, for a directory, its content: at the
/// top of the checkout and at any depth (`**/`).
fn rules_for_name(tools: &[&str], root: &str, name: &str, subtree: bool) -> Vec<String> {
    let mut rules = Vec::new();
    for tool in tools {
        for prefix in ["", "**/"] {
            rules.push(rule(tool, root, &format!("{prefix}{name}")));
            if subtree {
                rules.push(rule(tool, root, &format!("{prefix}{name}/**")));
            }
        }
    }
    rules
}

/// Claude permission rules refusing the secrets of a checkout to `tools`.
fn secret_rules(tools: &[&str], root: &str) -> Vec<String> {
    let mut rules = Vec::new();
    for tool in tools {
        for name in CODE_ROOT_CREDENTIAL_FILES {
            // `.env` is the pattern `.env*`: its variants too.
            let name = if *name == ".env" { ".env*" } else { name };
            rules.push(rule(tool, root, name));
            rules.push(rule(tool, root, &format!("{name}/**")));
        }
    }
    for name in SECRET_FILE_GLOBS {
        rules.extend(rules_for_name(tools, root, name, false));
    }
    for name in SECRET_DIR_NAMES {
        rules.extend(rules_for_name(tools, root, name, true));
    }
    rules
}

/// What a later program runs, at the top and at any depth.
fn executable_names() -> impl Iterator<Item = &'static &'static str> {
    CODE_ROOT_EXECUTABLE_PATHS
        .iter()
        .chain(crate::api::audit::step_remit::KRONN_OWNED)
}

/// Claude permission rules refusing writes to what a later program runs.
fn executable_rules(root: &Path, settings_root: &str, hooks: &[PathBuf]) -> Vec<String> {
    let mut rules = Vec::new();
    for name in executable_names() {
        rules.extend(rules_for_name(
            &["Edit", "Write"],
            settings_root,
            name,
            true,
        ));
    }
    for hooks in hooks.iter().filter(|hooks| hooks.starts_with(root)) {
        if let Ok(rel) = hooks.strip_prefix(root) {
            for tool in ["Edit", "Write"] {
                rules.push(rule(tool, settings_root, &rel.display().to_string()));
                rules.push(rule(tool, settings_root, &format!("{}/**", rel.display())));
            }
        }
    }
    rules
}

/// Claude permission patterns of a code root: the secrets refused to Read, Edit
/// and Write, what a later program runs refused to Edit and Write.
fn code_root_deny_rules(root: &Path, settings_root: &str, hooks: &[PathBuf]) -> Vec<String> {
    let mut rules = secret_rules(&["Read", "Edit", "Write"], settings_root);
    rules.extend(executable_rules(root, settings_root, hooks));
    rules
}

/// The secrets of a checkout for the OS sandbox (denyRead; denyWrite on a code
/// root): the fixed credential paths whether or not they exist (Seatbelt takes a
/// path that is not there yet), patterns for the names at any depth (honoured
/// where the sandbox supports them), and the secrets found now by a bounded scan,
/// as literal paths (`scanned`, selected under the room byte budget) that hold
/// wherever patterns do not.
fn secret_sandbox_paths(settings_root: &str, scanned: &[String]) -> Vec<String> {
    let mut paths: Vec<String> = CODE_ROOT_CREDENTIAL_FILES
        .iter()
        .map(|name| format!("{settings_root}/{name}"))
        .collect();
    for name in SECRET_FILE_GLOBS.iter().chain(SECRET_DIR_NAMES) {
        paths.push(format!("{settings_root}/**/{name}"));
    }
    paths.extend(scanned.iter().cloned());
    paths
}

/// What a code root's shell may not write besides its secrets.
fn executable_sandbox_paths(
    root: &Path,
    settings_root: &str,
    hooks: &[PathBuf],
    work_dir: &Path,
    compact: bool,
) -> Vec<String> {
    let mut paths: Vec<String> = hooks
        .iter()
        .filter(|hooks| hooks.starts_with(root))
        .map(|hooks| settings_path(hooks, work_dir, compact))
        .collect();
    for name in executable_names() {
        paths.push(format!("{settings_root}/{name}"));
        paths.push(format!("{settings_root}/**/{name}"));
    }
    paths
}

/// The secrets a checkout holds now: a breadth-first, bounded walk (depth, entry
/// count, no link followed, no dependency or Git directory entered). A dangling
/// link named like a secret is one. Shallowest first.
fn existing_secrets(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut queue = std::collections::VecDeque::from([(root.to_path_buf(), 0usize)]);
    let mut seen = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > SCAN_MAX_ENTRIES {
                return found;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            if path.to_str().is_none() {
                continue;
            }
            if crate::acp::secret_files::is_secret_file(relative) {
                found.push(path);
                continue;
            }
            let skipped = entry
                .file_name()
                .to_str()
                .is_some_and(|name| SCAN_SKIPPED_DIRS.contains(&name));
            if kind.is_dir() && !skipped && depth < SCAN_MAX_DEPTH {
                queue.push_back((path, depth + 1));
            }
        }
    }
    found
}

/// `name` against a pattern with `*` only at its ends (`.env*`, `*.pem`,
/// `*.env.*`), case-sensitively, as the sandbox's own glob does.
fn name_matches(pattern: &str, name: &str) -> bool {
    let core = pattern.trim_matches('*');
    match (pattern.starts_with('*'), pattern.ends_with('*')) {
        (true, true) => name.contains(core),
        (true, false) => name.ends_with(core),
        (false, true) => name.starts_with(core),
        (false, false) => name == core,
    }
}

/// True when the `**/` patterns of [`SECRET_FILE_GLOBS`] / [`SECRET_DIR_NAMES`]
/// name this path (relative to its root) once the cap leaves it out of the
/// literal list. `is_secret_file` also matches what they do not: other letter
/// cases (`PROD.PEM`) and the like.
fn named_by_patterns(relative: &Path) -> bool {
    let names: Vec<&str> = relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    let Some(last) = names.last() else {
        return false;
    };
    SECRET_FILE_GLOBS
        .iter()
        .any(|pattern| name_matches(pattern, last))
        || SECRET_DIR_NAMES.iter().any(|dir| {
            let wanted: Vec<&str> = dir.split('/').collect();
            names.windows(wanted.len()).any(|window| window == wanted)
        })
}

/// A lexical path from `base` to `path`. Both inputs are canonical directories or
/// descendants, so no symlink is resolved here.
fn relative_path(path: &Path, base: &Path) -> Option<PathBuf> {
    let path: Vec<_> = path.components().collect();
    let base: Vec<_> = base.components().collect();
    let common = path
        .iter()
        .zip(&base)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return None;
    }
    let mut relative = PathBuf::new();
    for _ in common..base.len() {
        relative.push("..");
    }
    for component in &path[common..] {
        relative.push(component.as_os_str());
    }
    (!relative.as_os_str().is_empty()).then_some(relative)
}

/// The path placed in Claude settings. Absolute paths preserve the existing
/// policy. Under pressure, a shorter lexical path is safe because Claude anchors
/// relative permission and sandbox paths at the primary working directory.
fn settings_path(path: &Path, work_dir: &Path, compact: bool) -> String {
    let absolute = path.display().to_string();
    if !compact {
        return absolute;
    }
    if path == work_dir {
        return ".".into();
    }
    relative_path(path, work_dir)
        .map(|path| path.display().to_string())
        .filter(|relative| relative.len() < absolute.len())
        .unwrap_or(absolute)
}

/// Replace paths by their directories, or directories by their parents, one
/// level at a time. The checkout root is the last resort.
fn collapse_once(root: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    let parent_of = |path: &PathBuf| {
        path.parent()
            .filter(|parent| parent.starts_with(root))
            .unwrap_or(root)
            .to_path_buf()
    };
    let mut current: Vec<PathBuf> = Vec::new();
    for path in paths {
        let dir = parent_of(path);
        if !current.contains(&dir) {
            current.push(dir);
        }
    }
    current
}

/// The existing secrets of a room root, partitioned by whether a mandatory name
/// pattern already protects them. The shared byte budget is assigned only after
/// every root has been scanned.
fn scan_secrets(root: &Path) -> ScannedSecrets {
    let (named, unnamed) = existing_secrets(root)
        .into_iter()
        .partition(|path| path.strip_prefix(root).is_ok_and(named_by_patterns));
    ScannedSecrets { unnamed, named }
}

/// Where a checkout's `core.hooksPath` leads.
#[derive(Debug, PartialEq, Eq)]
enum HooksPath {
    /// Unset, or a directory outside the checkout (not in `allowWrite`).
    Elsewhere,
    /// Inside the checkout: every path an agent could replace to plant a hook
    /// (the configured path, where it really lies, and each link on the way).
    Inside(Vec<PathBuf>),
    /// The checkout itself: every file would be a hook.
    Root,
}

/// `core.hooksPath` of a checkout. Read with plain `git config` (it runs nothing).
fn hooks_path(root: &Path) -> Result<HooksPath, String> {
    let output = crate::core::cmd::git_cmd()
        .arg("-C")
        .arg(root)
        .args(["config", "--type=path", "--get", "core.hooksPath"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .output()
        .map_err(|error| format!("Cannot read the checkout's hooks setting: {error}"))?;
    let configured = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || configured.is_empty() {
        return Ok(HooksPath::Elsewhere);
    }
    let mut resolved = PathBuf::new();
    for component in root.join(&configured).components() {
        match component {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    let real = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());
    if real == root || resolved == root {
        return Ok(HooksPath::Root);
    }
    let mut paths = Vec::new();
    // A link on the way (`hooks -> .config/hooks`, or `tools -> scripts` for
    // `tools/hooks`) is as editable as the directory: the agent replaces the
    // link by a real directory of its own. Each one is closed, root first.
    if let Ok(relative) = resolved.strip_prefix(root) {
        let mut walked = root.to_path_buf();
        for component in relative.components() {
            walked.push(component);
            let is_link = std::fs::symlink_metadata(&walked)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
            if is_link && walked != resolved {
                paths.push(walked.clone());
            }
        }
        paths.push(resolved.clone());
    }
    if real.starts_with(root) {
        paths.push(real);
    }
    paths.dedup();
    Ok(if paths.is_empty() {
        HooksPath::Elsewhere
    } else {
        HooksPath::Inside(paths)
    })
}

/// A code root may not contain, or be inside, the Kronn data directory (or the
/// backup directory): its shell would write the vault from there.
fn refuse_protected_overlap(root: &Path) -> Result<(), String> {
    for protected in crate::core::trust_sandbox::protected_dirs() {
        if root.starts_with(&protected) || protected.starts_with(root) {
            return Err(format!(
                "the project checkout {} overlaps the Kronn data directory ({}): it is not opened for editing",
                root.display(),
                protected.display()
            ));
        }
    }
    Ok(())
}

fn push_unique_string(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}

/// A registered project's Git checkout that passed validation: the canonical
/// root and the identity (device, inode, birth time) it had then. The launch opens this
/// root, never the path the project was registered under, and refuses it if
/// the directory behind it is no longer the one that was validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCheckout {
    root: PathBuf,
    device: u64,
    inode: u64,
    /// Linux reuses a freed inode at once, so a checkout replaced at the same
    /// path keeps (device, inode); its creation time does not.
    born: Option<std::time::SystemTime>,
    /// KT-1491 — opened for the agent to edit (a `code` grant), not read-only.
    writable: bool,
}

impl VerifiedCheckout {
    /// Validate a registered project path as an openable checkout: a path
    /// that is, or passes through, a symbolic link is refused (it could lead
    /// to another project's checkout), as is anything that is not the root of
    /// a Git checkout or overlaps the working directory.
    pub(crate) fn verify(
        agent: &AgentType,
        work_dir: &Path,
        location: &str,
    ) -> Result<Self, String> {
        check_platform(agent)?;
        let work_dir = canonical_directory(work_dir)?;
        let path = crate::core::scanner::resolve_host_path(location);
        if !path.is_absolute() || location.trim().is_empty() {
            return Err(format!(
                "a project checkout needs an absolute local path: {location:?}"
            ));
        }
        refuse_redirection(&path)?;
        let root = canonical_directory(&path)?;
        refuse_overlap(&root, &work_dir)?;
        let output = crate::core::cmd::project_git_cmd()
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--path-format=absolute", "--show-toplevel"])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .output()
            .map_err(|error| format!("Cannot inspect the checkout's Git metadata: {error}"))?;
        let toplevel = String::from_utf8(output.stdout)
            .map_err(|_| "a project checkout's Git paths must be UTF-8".to_string())?;
        if !output.status.success()
            || canonical_directory(Path::new(toplevel.trim()))
                .ok()
                .as_ref()
                != Some(&root)
        {
            return Err(format!(
                "a project checkout must be the root of a Git checkout: {}",
                root.display()
            ));
        }
        let (device, inode, born) = identity(&root)?;
        Ok(Self {
            root,
            device,
            inode,
            born,
            writable: false,
        })
    }

    /// The same validated checkout, opened for editing (a `code` grant): refused
    /// when it overlaps the Kronn data directory.
    pub(crate) fn writable(mut self) -> Result<Self, String> {
        refuse_protected_overlap(&self.root)?;
        self.writable = true;
        Ok(self)
    }

    /// The checkout sets `core.hooksPath` to its own root, so every file in it
    /// would be a hook: a `code` grant cannot open it for editing. The room gets
    /// it read-only and is told why.
    pub(crate) fn hooks_at_root(&self) -> bool {
        matches!(hooks_path(&self.root), Ok(HooksPath::Root))
    }

    #[cfg(test)]
    pub(crate) fn is_writable(&self) -> bool {
        self.writable
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The directory behind the validated root is still the validated one.
    fn still_holds(&self) -> Result<(), String> {
        refuse_redirection(&self.root)?;
        let again = canonical_directory(&self.root)?;
        if again != self.root || identity(&again)? != (self.device, self.inode, self.born) {
            return Err(format!(
                "the project checkout {} changed since it was validated; it is not opened",
                self.root.display()
            ));
        }
        Ok(())
    }
}

fn check_platform(agent: &AgentType) -> Result<(), String> {
    if !matches!(agent, AgentType::ClaudeCode | AgentType::Codex) {
        return Err("read_only_repos requires Claude Code or Codex".into());
    }
    if cfg!(windows) {
        return Err("read_only_repos requires a native macOS or Linux runner".into());
    }
    Ok(())
}

fn refuse_overlap(root: &Path, work_dir: &Path) -> Result<(), String> {
    // Denying writes on an ancestor would also deny the worktree.
    if work_dir.starts_with(root) || root.starts_with(work_dir) {
        return Err(format!(
            "read_only_repos path {} overlaps the writable working directory {}",
            root.display(),
            work_dir.display()
        ));
    }
    Ok(())
}

/// No component of `path`, itself included, may be a symbolic link, nor a
/// `.`/`..` step: resolving one of them is how a registered project path ends
/// up naming another project's checkout.
fn refuse_redirection(path: &Path) -> Result<(), String> {
    let mut walked = PathBuf::new();
    for component in path.components() {
        if matches!(
            component,
            std::path::Component::CurDir | std::path::Component::ParentDir
        ) {
            return Err(format!(
                "a project checkout path cannot contain `.` or `..`: {}",
                path.display()
            ));
        }
        walked.push(component);
        let metadata = std::fs::symlink_metadata(&walked).map_err(|error| {
            format!(
                "Cannot inspect the project checkout path {}: {error}",
                walked.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "the project checkout path {} goes through a symbolic link ({}); it is not opened",
                path.display(),
                walked.display()
            ));
        }
    }
    Ok(())
}

type Identity = (u64, u64, Option<std::time::SystemTime>);

#[cfg(unix)]
fn identity(path: &Path) -> Result<Identity, String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("Cannot inspect {}: {error}", path.display()))?;
    Ok((metadata.dev(), metadata.ino(), metadata.created().ok()))
}

#[cfg(not(unix))]
fn identity(_path: &Path) -> Result<Identity, String> {
    Err("read_only_repos requires a native macOS or Linux runner".into())
}

impl ReadOnlyRepos {
    #[cfg(test)]
    pub(crate) fn resolve(
        agent: &AgentType,
        work_dir: &Path,
        declared: &[String],
    ) -> Result<Option<Self>, String> {
        Self::resolve_with_dirs(agent, work_dir, declared, &[])
    }

    /// `declared` Git checkouts plus plain `dirs` (no Git metadata, e.g. the
    /// run's artifacts directory), all read-only under one policy.
    pub(crate) fn resolve_with_dirs(
        agent: &AgentType,
        work_dir: &Path,
        declared: &[String],
        dirs: &[String],
    ) -> Result<Option<Self>, String> {
        if declared.is_empty() && dirs.is_empty() {
            return Ok(None);
        }
        check_platform(agent)?;
        let work_dir = canonical_directory(work_dir)?;
        let mut roots = Vec::new();
        for location in dirs {
            let path = Path::new(location);
            if !path.is_absolute() {
                return Err(format!(
                    "read-only directory must be absolute: {location:?}"
                ));
            }
            let root = canonical_directory(path)?;
            if work_dir.starts_with(&root) || root.starts_with(&work_dir) {
                return Err(format!(
                    "read-only directory {} overlaps the writable working directory {}",
                    root.display(),
                    work_dir.display()
                ));
            }
            push_unique(&mut roots, root);
        }
        for location in declared {
            let path = crate::core::scanner::resolve_host_path(location);
            if !path.is_absolute() || location.trim().is_empty() {
                return Err(format!(
                    "read_only_repos requires an absolute local path: {location:?}"
                ));
            }
            let root = canonical_directory(&path)?;
            // Denying writes on an ancestor would also deny the worktree.
            if work_dir.starts_with(&root) || root.starts_with(&work_dir) {
                return Err(format!(
                    "read_only_repos path {} overlaps the writable working directory {}",
                    root.display(),
                    work_dir.display()
                ));
            }
            push_unique(&mut roots, root.clone());
            // A linked Git worktree stores its history outside its checkout.
            let output = crate::core::cmd::project_git_cmd()
                .arg("-C")
                .arg(&root)
                .args([
                    "rev-parse",
                    "--path-format=absolute",
                    "--show-toplevel",
                    "--git-dir",
                    "--git-common-dir",
                ])
                .env("GIT_OPTIONAL_LOCKS", "0")
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_COMMON_DIR")
                .output()
                .map_err(|error| format!("Cannot inspect read_only_repos Git metadata: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "read_only_repos path is not a Git repository: {}",
                    root.display()
                ));
            }
            let metadata = String::from_utf8(output.stdout)
                .map_err(|_| "read_only_repos Git paths must be UTF-8".to_string())?;
            let mut paths = metadata.lines();
            let toplevel = paths
                .next()
                .ok_or("read_only_repos Git checkout root is missing")?;
            if canonical_directory(Path::new(toplevel))? != root {
                return Err(format!(
                    "read_only_repos must name a Git checkout root: {}",
                    root.display()
                ));
            }
            for line in paths {
                let path = canonical_directory(Path::new(line))?;
                if work_dir.starts_with(&path) || path.starts_with(&work_dir) {
                    return Err(
                        "read_only_repos Git metadata overlaps the writable working directory"
                            .into(),
                    );
                }
                if !path.starts_with(&root) {
                    push_unique(&mut roots, path);
                }
            }
        }
        Ok(Some(Self {
            roots,
            code_roots: Vec::new(),
            scanned: Vec::new(),
            code_hooks: Vec::new(),
            guard_secrets: false,
            compact_paths: false,
        }))
    }

    /// KT-1406 — the policy of a discussion's native Claude Code agent: the
    /// granted projects' validated checkouts as read-only roots, and (KT-1491)
    /// the ones granted `code` as writable roots. Each
    /// checkout is checked again here, at the launch: one whose directory is
    /// no longer the validated one is refused. Codex keeps its default
    /// sandbox (its read isolation is a separate work item), so it never gets
    /// a room policy.
    pub(crate) fn resolve_room(
        agent: &AgentType,
        work_dir: &Path,
        checkouts: &[VerifiedCheckout],
    ) -> Result<Option<Self>, String> {
        if checkouts.is_empty() {
            return Ok(None);
        }
        if !matches!(agent, AgentType::ClaudeCode) {
            return Err("room checkouts are only opened for Claude Code".into());
        }
        check_platform(agent)?;
        let work_dir = canonical_directory(work_dir)?;
        let mut roots = Vec::new();
        let mut code_roots = Vec::new();
        let mut code_hooks = Vec::new();
        for checkout in checkouts {
            checkout.still_holds()?;
            refuse_overlap(&checkout.root, &work_dir)?;
            if checkout.writable {
                refuse_protected_overlap(&checkout.root)?;
                // `core.hooksPath` may have changed since the plan: a checkout
                // whose hooks directory is its own root is opened read-only.
                match hooks_path(&checkout.root)? {
                    HooksPath::Root => push_unique(&mut roots, checkout.root.clone()),
                    HooksPath::Inside(paths) => {
                        for path in paths {
                            push_unique(&mut code_hooks, path);
                        }
                        push_unique(&mut code_roots, checkout.root.clone());
                    }
                    HooksPath::Elsewhere => push_unique(&mut code_roots, checkout.root.clone()),
                }
            } else {
                push_unique(&mut roots, checkout.root.clone());
            }
        }
        // A project granted both ways is read-only: the narrower wins.
        code_roots.retain(|root| !roots.contains(root));
        // The walk happens here, once, at the launch. Selection happens only
        // after every root has contributed to the same settings byte budget.
        let scans = roots
            .iter()
            .chain(&code_roots)
            .map(|root| (root.clone(), scan_secrets(root)))
            .collect::<Vec<_>>();
        let mut policy = Self {
            roots,
            code_roots,
            scanned: Vec::new(),
            code_hooks,
            guard_secrets: true,
            compact_paths: false,
        };
        policy.fit_scanned_to_room_budget(&work_dir, scans)?;
        Ok(Some(policy))
    }

    fn settings_root(&self, root: &Path, work_dir: &Path) -> String {
        settings_path(root, work_dir, self.compact_paths)
    }

    fn settings_document<'a>(&self, args: &'a [String]) -> &'a str {
        let at = args
            .iter()
            .position(|arg| arg == "--settings")
            .expect("Claude room policy has a settings document");
        &args[at + 1]
    }

    /// Select the launch-time scan paths under one byte budget shared by every
    /// room root. Mandatory tool rules, sandbox patterns and vault paths come
    /// first. Existing secrets no pattern names are then kept literally or
    /// collapsed to directories. Pattern-covered literals use only what remains.
    fn fit_scanned_to_room_budget(
        &mut self,
        work_dir: &Path,
        scans: Vec<(PathBuf, ScannedSecrets)>,
    ) -> Result<(), String> {
        let absolute = self.args(&AgentType::ClaudeCode, work_dir);
        if self.settings_document(&absolute).len() > ROOM_SETTINGS_MAX_BYTES {
            self.compact_paths = true;
        }
        let compact = self.args(&AgentType::ClaudeCode, work_dir);
        let mandatory_len = self.settings_document(&compact).len();
        if mandatory_len > ROOM_SETTINGS_MAX_BYTES {
            return Err(format!(
                "the room's required checkout rules need {mandatory_len} bytes, above the {}-byte Claude settings limit",
                ROOM_SETTINGS_MAX_BYTES
            ));
        }

        // Unnamed secrets may never disappear merely because the room has many
        // roots. Collapse every root one level at a time until all of them fit.
        let mut unnamed: Vec<(PathBuf, Vec<PathBuf>)> = scans
            .iter()
            .map(|(root, scan)| (root.clone(), scan.unnamed.clone()))
            .collect();
        loop {
            self.scanned = unnamed
                .iter()
                .map(|(root, paths)| {
                    (
                        root.clone(),
                        paths
                            .iter()
                            .map(|path| settings_path(path, work_dir, self.compact_paths))
                            .collect(),
                    )
                })
                .collect();
            let args = self.args(&AgentType::ClaudeCode, work_dir);
            if self.settings_document(&args).len() <= ROOM_SETTINGS_MAX_BYTES {
                break;
            }
            let mut changed = false;
            for (root, paths) in &mut unnamed {
                let collapsed = collapse_once(root, paths);
                if collapsed != *paths {
                    *paths = collapsed;
                    changed = true;
                }
            }
            if !changed {
                return Err(format!(
                    "the room's secret paths cannot fit the {}-byte Claude settings limit",
                    ROOM_SETTINGS_MAX_BYTES
                ));
            }
        }

        // Model the exact JSON growth of a string appended to a non-empty array.
        // Both arrays are already non-empty because every guarded root contributes
        // fixed paths and patterns.
        let current = self.args(&AgentType::ClaudeCode, work_dir);
        let settings: serde_json::Value =
            serde_json::from_str(self.settings_document(&current)).expect("settings are JSON");
        let mut deny_read: std::collections::HashSet<String> = settings["sandbox"]["filesystem"]
            ["denyRead"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect();
        let mut deny_write: std::collections::HashSet<String> = settings["sandbox"]["filesystem"]
            ["denyWrite"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect();
        let mut bytes = self.settings_document(&current).len();
        for (root, scan) in &scans {
            let code = self.code_roots.contains(root);
            for path in &scan.named {
                let path = settings_path(path, work_dir, self.compact_paths);
                let encoded = serde_json::to_string(&path).expect("validated UTF-8 path");
                let read_bytes = usize::from(!deny_read.contains(&path)) * (encoded.len() + 1);
                let write_bytes =
                    usize::from(code && !deny_write.contains(&path)) * (encoded.len() + 1);
                if bytes + read_bytes + write_bytes > ROOM_SETTINGS_MAX_BYTES {
                    continue;
                }
                bytes += read_bytes + write_bytes;
                deny_read.insert(path.clone());
                if code {
                    deny_write.insert(path.clone());
                }
                let selected = self
                    .scanned
                    .iter_mut()
                    .find(|(selected_root, _)| selected_root == root)
                    .expect("every scanned root has a selection");
                push_unique_string(&mut selected.1, path);
            }
        }
        let final_args = self.args(&AgentType::ClaudeCode, work_dir);
        let final_len = self.settings_document(&final_args).len();
        if final_len > ROOM_SETTINGS_MAX_BYTES {
            return Err(format!(
                "the room's Claude settings grew to {final_len} bytes, above the {}-byte limit",
                ROOM_SETTINGS_MAX_BYTES
            ));
        }
        Ok(())
    }

    pub(crate) fn apply(&self, agent: &AgentType, work_dir: &Path, args: &mut Vec<String>) {
        let prompt = args.pop().expect("command builder ends with a prompt");
        args.retain(|arg| {
            arg != "--dangerously-skip-permissions" && !arg.starts_with("--sandbox=")
        });
        args.extend(self.args(agent, work_dir));
        // KT-1495 — one `--settings` document: the vault rules join the policy's.
        crate::core::vault_guard::fold_vault_into_claude_settings(args);
        args.push(prompt);
    }

    fn scanned_for(&self, root: &Path) -> &[String] {
        self.scanned
            .iter()
            .find(|(scanned, _)| scanned == root)
            .map_or(&[], |(_, paths)| paths.as_slice())
    }

    pub(crate) fn args(&self, agent: &AgentType, work_dir: &Path) -> Vec<String> {
        match agent {
            AgentType::ClaudeCode => {
                let mut deny: Vec<String> = self
                    .roots
                    .iter()
                    .map(|path| rule("Edit", &self.settings_root(path, work_dir), "**"))
                    .collect();
                let mut allow_write = vec![settings_path(work_dir, work_dir, self.compact_paths)];
                allow_write.extend(
                    self.code_roots
                        .iter()
                        .map(|root| self.settings_root(root, work_dir)),
                );
                let mut deny_write: Vec<String> = self
                    .roots
                    .iter()
                    .map(|root| self.settings_root(root, work_dir))
                    .collect();
                let mut deny_read: Vec<String> = Vec::new();
                // A room's read-only checkouts keep their secrets closed too: a
                // lower level never reads what `code` cannot.
                if self.guard_secrets {
                    for root in &self.roots {
                        let settings_root = self.settings_root(root, work_dir);
                        deny.extend(secret_rules(&["Read"], &settings_root));
                        for path in secret_sandbox_paths(&settings_root, self.scanned_for(root)) {
                            push_unique_string(&mut deny_read, path);
                        }
                    }
                }
                for root in &self.code_roots {
                    let settings_root = self.settings_root(root, work_dir);
                    deny.extend(code_root_deny_rules(root, &settings_root, &self.code_hooks));
                    for path in secret_sandbox_paths(&settings_root, self.scanned_for(root)) {
                        push_unique_string(&mut deny_read, path.clone());
                        push_unique_string(&mut deny_write, path);
                    }
                    for path in executable_sandbox_paths(
                        root,
                        &settings_root,
                        &self.code_hooks,
                        work_dir,
                        self.compact_paths,
                    ) {
                        push_unique_string(&mut deny_write, path);
                    }
                }
                let mut settings = serde_json::json!({
                    "permissions": {
                        "blockReadsOutsideWorkingDirectories": true,
                        "deny": deny
                    },
                    "sandbox": {
                        "enabled": true,
                        "failIfUnavailable": true,
                        "autoAllowBashIfSandboxed": true,
                        "allowUnsandboxedCommands": false,
                        "excludedCommands": [],
                        "filesystem": {
                            "disabled": false,
                            "allowWrite": allow_write,
                            "denyWrite": deny_write
                        }
                    }
                });
                if !deny_read.is_empty() {
                    settings["sandbox"]["filesystem"]["denyRead"] = serde_json::json!(deny_read);
                }
                let mut args = vec![
                    "--setting-sources".into(),
                    String::new(),
                    "--settings".into(),
                    settings.to_string(),
                    "--permission-mode".into(),
                    "acceptEdits".into(),
                ];
                if self.guard_secrets {
                    // Budget the document the process receives, including vault
                    // paths. `apply` folds again only to remove the standalone
                    // vault flag built before this policy; the merge is idempotent.
                    crate::core::vault_guard::fold_vault_into_claude_settings(&mut args);
                }
                for root in self.roots.iter().chain(&self.code_roots) {
                    args.extend(["--add-dir".into(), root.display().to_string()]);
                }
                args
            }
            AgentType::Codex => {
                // --add-dir is a WRITE grant in Codex. Explicit read entries
                // also protect repositories located below /tmp or TMPDIR.
                let mut args = vec![
                    "--ignore-user-config".into(),
                    "--ignore-rules".into(),
                    "--strict-config".into(),
                ];
                for setting in [
                    "approval_policy=\"never\"",
                    "default_permissions=\"kronn_read_only_repos\"",
                ] {
                    args.extend(["-c".into(), setting.into()]);
                }
                let mut filesystem = vec!["\":root\"=\"read\"".to_string()];
                for root in &self.roots {
                    filesystem.push(format!(
                        "{}=\"read\"",
                        serde_json::to_string(root).expect("validated UTF-8 path")
                    ));
                }
                args.extend(["-c".into(), format!(
                    "permissions={{kronn_read_only_repos={{extends=\":workspace\",workspace_roots={{}},filesystem={{{}}}}}}}",
                    filesystem.join(",")
                )]);
                args
            }
            _ => unreachable!("provider checked at resolution"),
        }
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "Cannot resolve read_only_repos path {}: {error}",
            path.display()
        )
    })?;
    if !canonical.is_dir() || canonical.to_str().is_none() {
        return Err(format!(
            "read_only_repos requires a UTF-8 directory: {}",
            path.display()
        ));
    }
    // Claude permission paths are patterns. Refuse ambiguous literal names.
    if canonical
        .to_string_lossy()
        .contains(['*', '?', '[', ']', '(', ')', '\n', '\r', '\\'])
    {
        return Err(format!(
            "read_only_repos path contains permission-pattern characters: {}",
            path.display()
        ));
    }
    Ok(canonical)
}

fn push_unique(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if !roots.contains(&path) {
        roots.push(path);
    }
}

#[cfg(all(test, unix, feature = "full-tests"))]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        let repo = temp.path().join("API équipe");
        std::fs::create_dir(&work).unwrap();
        std::fs::create_dir(&repo).unwrap();
        let output = crate::core::cmd::git_cmd()
            .arg("init")
            .arg(&repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        (
            temp,
            work.canonicalize().unwrap(),
            repo.canonicalize().unwrap(),
        )
    }

    #[test]
    fn read_only_repos_empty_preserves_legacy_providers() {
        assert!(
            ReadOnlyRepos::resolve(&AgentType::Ollama, Path::new("missing"), &[])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn read_only_repos_rejects_missing_relative_non_git_and_overlapping_paths() {
        let (temp, work, repo) = fixture();
        let plain = temp.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        for path in [
            "relative".into(),
            "https://example.com/repo".into(),
            String::new(),
            temp.path().join("missing").display().to_string(),
            plain.display().to_string(),
            work.display().to_string(),
            temp.path().display().to_string(),
        ] {
            assert!(
                ReadOnlyRepos::resolve(&AgentType::ClaudeCode, &work, std::slice::from_ref(&path))
                    .is_err(),
                "{path}"
            );
        }
        assert!(
            ReadOnlyRepos::resolve(&AgentType::Ollama, &work, &[repo.display().to_string()])
                .is_err()
        );
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&work, &alias).unwrap();
        assert!(
            ReadOnlyRepos::resolve(&AgentType::Codex, &work, &[alias.display().to_string()])
                .is_err()
        );
    }

    #[test]
    fn read_only_repos_canonicalizes_aliases_and_deduplicates() {
        let (temp, work, repo) = fixture();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&repo, &alias).unwrap();
        let policy = ReadOnlyRepos::resolve(
            &AgentType::ClaudeCode,
            &work,
            &[repo.display().to_string(), alias.display().to_string()],
        )
        .unwrap()
        .unwrap();
        assert_eq!(policy.roots, vec![repo]);
    }

    #[test]
    fn read_only_repos_claude_blocks_tools_and_subprocess_writes_without_disabling_read_block() {
        let (_temp, work, repo) = fixture();
        let policy =
            ReadOnlyRepos::resolve(&AgentType::ClaudeCode, &work, &[repo.display().to_string()])
                .unwrap()
                .unwrap();
        let mut args = vec!["--dangerously-skip-permissions".into(), "prompt".into()];
        policy.apply(&AgentType::ClaudeCode, &work, &mut args);
        assert!(!args.iter().any(|a| a == "--dangerously-skip-permissions"));
        assert_eq!(args.last().unwrap(), "prompt");
        assert!(args
            .windows(2)
            .any(|p| p[0] == "--add-dir" && p[1] == repo.to_str().unwrap()));
        let settings: serde_json::Value =
            serde_json::from_str(&args[args.iter().position(|a| a == "--settings").unwrap() + 1])
                .unwrap();
        assert_eq!(
            settings["permissions"]["blockReadsOutsideWorkingDirectories"],
            true
        );
        // The read-only rule first, then the Kronn data-dir rules (KT-1495) in
        // the same document.
        let mut deny = vec![format!("Edit(/{}/**)", repo.display())];
        deny.extend(crate::core::vault_guard::claude_permission_deny());
        assert_eq!(settings["permissions"]["deny"], serde_json::json!(deny));
        assert_eq!(
            settings["sandbox"]["filesystem"]["denyRead"],
            serde_json::json!(crate::core::vault_guard::claude_sandbox_deny_read())
        );
        assert_eq!(
            settings["sandbox"]["filesystem"]["denyWrite"],
            serde_json::json!(std::iter::once(repo.display().to_string())
                .chain(crate::core::vault_guard::claude_sandbox_deny_read())
                .collect::<Vec<_>>())
        );
        assert_eq!(
            settings["sandbox"]["filesystem"]["allowWrite"],
            serde_json::json!([work])
        );
        assert_eq!(settings["sandbox"]["failIfUnavailable"], true);
        assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
        assert_eq!(
            settings["sandbox"]["excludedCommands"],
            serde_json::json!([])
        );
    }

    #[test]
    fn read_only_repos_codex_uses_read_entries_even_under_writable_temporary_roots() {
        let (_temp, work, repo) = fixture();
        let policy =
            ReadOnlyRepos::resolve(&AgentType::Codex, &work, &[repo.display().to_string()])
                .unwrap()
                .unwrap();
        let mut args = vec![
            "exec".into(),
            "--sandbox=danger-full-access".into(),
            "prompt".into(),
        ];
        policy.apply(&AgentType::Codex, &work, &mut args);
        assert!(!args
            .iter()
            .any(|arg| arg.starts_with("--sandbox") || arg == "--add-dir"));
        assert!(args.contains(&"approval_policy=\"never\"".into()));
        assert!(args.contains(&"--ignore-user-config".into()));
        assert!(args.contains(&"--strict-config".into()));
        let permissions: toml::Value = toml::from_str(
            args.iter()
                .find(|arg| arg.starts_with("permissions="))
                .unwrap(),
        )
        .unwrap();
        let profile = &permissions["permissions"]["kronn_read_only_repos"];
        assert_eq!(profile["extends"].as_str(), Some(":workspace"));
        // KT-1406: the Codex policy is the workflow one, unchanged.
        assert_eq!(profile["filesystem"][":root"].as_str(), Some("read"));
        assert_eq!(profile["filesystem"].as_table().unwrap().len(), 2);
        assert_eq!(
            profile["filesystem"][repo.to_str().unwrap()].as_str(),
            Some("read")
        );
        assert!(profile["workspace_roots"].as_table().unwrap().is_empty());
    }

    #[test]
    fn read_only_repos_includes_external_git_history_for_a_linked_worktree() {
        let (temp, work, repo) = fixture();
        let output = crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(&repo)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "fixture",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        let linked = temp.path().join("linked");
        let output = crate::core::cmd::git_cmd()
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "--detach"])
            .arg(&linked)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let policy = ReadOnlyRepos::resolve(
            &AgentType::ClaudeCode,
            &work,
            &[linked.display().to_string()],
        )
        .unwrap()
        .unwrap();
        assert!(policy.roots.contains(&linked.canonicalize().unwrap()));
        assert!(policy
            .roots
            .contains(&repo.join(".git").canonicalize().unwrap()));
    }

    #[test]
    fn a_plain_directory_is_readable_and_never_writable() {
        let (temp, work, _repo) = fixture();
        let artifacts = temp.path().join("run artifacts");
        std::fs::create_dir(&artifacts).unwrap();
        std::fs::write(artifacts.join("shot.png"), [0x89, b'P', b'N', b'G']).unwrap();
        let artifacts = artifacts.canonicalize().unwrap();
        let dirs = [artifacts.display().to_string()];
        let policy = ReadOnlyRepos::resolve_with_dirs(&AgentType::ClaudeCode, &work, &[], &dirs)
            .unwrap()
            .expect("a directory alone needs the policy");
        let args = policy.args(&AgentType::ClaudeCode, &work);
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--add-dir" && pair[1] == artifacts.to_str().unwrap()));
        let settings: serde_json::Value =
            serde_json::from_str(&args[args.iter().position(|a| a == "--settings").unwrap() + 1])
                .unwrap();
        assert_eq!(
            settings["permissions"]["deny"],
            serde_json::json!([format!("Edit(/{}/**)", artifacts.display())])
        );
        assert_eq!(
            settings["sandbox"]["filesystem"]["denyWrite"],
            serde_json::json!([artifacts])
        );
        assert_eq!(
            settings["sandbox"]["filesystem"]["allowWrite"],
            serde_json::json!([work])
        );
        let codex = policy.args(&AgentType::Codex, &work).join(" ");
        assert!(
            codex.contains(&format!(
                "{}=\"read\"",
                serde_json::to_string(&artifacts).unwrap()
            )),
            "{codex}"
        );
        assert!(
            !codex.contains("--add-dir"),
            "--add-dir is a write grant in Codex"
        );

        for bad in [
            "relative/dir".to_string(),
            work.display().to_string(),
            temp.path().display().to_string(),
        ] {
            assert!(
                ReadOnlyRepos::resolve_with_dirs(
                    &AgentType::ClaudeCode,
                    &work,
                    &[],
                    std::slice::from_ref(&bad)
                )
                .is_err(),
                "{bad}"
            );
        }
    }
}
