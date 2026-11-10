//! KT-1587 — an agent launch reaches the artifacts of its own run, never of
//! another run's.
//!
//! The run artifacts root sits outside the data directory (KT-1585), so the
//! vault protection no longer hides it; without a rule of its own every agent,
//! in any project or discussion, would read every run's directory there (Exec
//! outputs, reports, screenshots). The root is therefore closed to every native
//! agent launch, and the run directory a launch was *granted* (the
//! `--add-dir` of Claude, the `"<dir>"="read"` entry of Codex, both written by
//! `agents::read_only_repos`) is opened again, read-only.
//!
//! - **Seatbelt** (Codex, and every launch Kronn wraps): the root is denied,
//!   then the own directory allowed for reads; a later rule wins.
//! - **bubblewrap**: an empty tmpfs over the root, the own directory bound
//!   read-only on top of it.
//! - **Claude's own sandbox** (a launch that enables it cannot be wrapped):
//!   `denyRead` on the root with `allowRead` on the own directory (an allow
//!   inside a denied read path re-opens it). The Read tool is a permission
//!   rule, where a deny beats an allow: with no grant the root is denied whole,
//!   with one every *other* run directory present at the launch is denied by
//!   its literal path, on top of `blockReadsOutsideWorkingDirectories`.
//!
//! Like the rest of the vault protection, all of it follows the owner's
//! setting and the host's sandbox: nothing is refused when either is missing.

use std::path::{Path, PathBuf};

use crate::core::task_worker_sandbox::{real_path, seatbelt_literal};
use crate::core::trust_sandbox;

/// Literal denies for the Read tool of a launch that has a grant. Beyond it the
/// OS sandbox and `blockReadsOutsideWorkingDirectories` still hold.
const MAX_SIBLING_RULES: usize = 200;

/// What one launch may reach under the run artifacts root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactsReach {
    root: PathBuf,
    /// The run directories the launch was granted: direct children of the root.
    own: Vec<PathBuf>,
}

fn both(path: &Path) -> Vec<PathBuf> {
    let real = real_path(path);
    let mut all = vec![path.to_path_buf()];
    if real != path {
        all.push(real);
    }
    all
}

impl ArtifactsReach {
    /// The reach of a launch with `args`, under `root`. Pure.
    pub fn new(root: PathBuf, args: &[String]) -> Self {
        let real_root = real_path(&root);
        let mut own: Vec<PathBuf> = Vec::new();
        for dir in granted_dirs(args) {
            let name_ok = dir
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                });
            let beside = dir
                .parent()
                .is_some_and(|parent| real_path(parent) == real_root);
            if name_ok && beside && !own.contains(&dir) {
                own.push(dir);
            }
        }
        Self { root, own }
    }

    /// The reach of the launch `args` on this host: `None` when the owner
    /// turned the protection off or no root exists.
    pub fn for_launch(args: &[String]) -> Option<Self> {
        if !trust_sandbox::protect_vault_setting() {
            return None;
        }
        Some(Self::new(crate::workflows::run_artifacts::root()?, args))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn own(&self) -> &[PathBuf] {
        &self.own
    }

    /// Seatbelt rules, to come after every other rule of the profile: the root
    /// neither read nor written, then each own directory readable (its writes
    /// stay closed: an Agent step reads artifacts, it never writes there).
    pub fn seatbelt_rules(&self) -> String {
        let mut rules = String::from("(deny file-read* file-write*\n");
        for dir in both(&self.root) {
            rules.push_str(&format!("  (subpath {})\n", seatbelt_literal(&dir)));
        }
        rules.push_str(")\n");
        if self.own.is_empty() {
            return rules;
        }
        // The root itself must stay statable to reach what is inside it; its
        // entries (the other runs' names) are not listed.
        rules.push_str("(allow file-read-metadata\n");
        for dir in both(&self.root) {
            rules.push_str(&format!("  (literal {})\n", seatbelt_literal(&dir)));
        }
        rules.push_str(")\n(allow file-read*\n");
        for own in &self.own {
            for dir in both(own) {
                rules.push_str(&format!("  (subpath {})\n", seatbelt_literal(&dir)));
            }
        }
        rules.push_str(")\n");
        rules
    }

    /// `bwrap` mounts, to come after the other mounts and before `--`: an empty
    /// tmpfs over the resolved root, each existing own directory bound
    /// read-only on top. A root that does not exist holds nothing to read.
    pub fn bubblewrap_mounts(&self) -> Vec<String> {
        let root = real_path(&self.root);
        if !root.is_dir() {
            return Vec::new();
        }
        let mut mounts = vec!["--tmpfs".to_string(), root.to_string_lossy().into_owned()];
        for own in &self.own {
            let own = real_path(own);
            if own.is_dir() {
                let text = own.to_string_lossy().into_owned();
                mounts.extend(["--ro-bind".to_string(), text.clone(), text]);
            }
        }
        mounts
    }

    /// `sandbox.filesystem.denyRead` and `denyWrite` of Claude's own sandbox.
    pub fn claude_sandbox_deny(&self) -> Vec<String> {
        both(&self.root)
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect()
    }

    /// `sandbox.filesystem.allowRead`: re-opens the own directories inside the
    /// denied root.
    pub fn claude_sandbox_allow_read(&self) -> Vec<String> {
        self.own
            .iter()
            .flat_map(|own| both(own))
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect()
    }

    /// `permissions.deny` for the Claude tools. Edit and Write never need the
    /// root (the own directory is read-only for the launch). Read is denied
    /// whole without a grant; with one, each other entry of the root is denied
    /// by its literal path, since a deny cannot be carved out by an allow.
    pub fn claude_permission_deny(&self) -> Vec<String> {
        let mut rules = Vec::new();
        for dir in both(&self.root) {
            for tool in ["Edit", "Write"] {
                rules.push(crate::core::vault_guard::claude_rule(tool, &dir));
            }
            if self.own.is_empty() {
                rules.push(crate::core::vault_guard::claude_rule("Read", &dir));
            }
        }
        if !self.own.is_empty() {
            let own_names: Vec<&std::ffi::OsStr> =
                self.own.iter().filter_map(|own| own.file_name()).collect();
            let mut others: Vec<PathBuf> = std::fs::read_dir(real_path(&self.root))
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| !own_names.contains(&entry.file_name().as_os_str()))
                .map(|entry| entry.path())
                .collect();
            others.sort();
            if others.len() > MAX_SIBLING_RULES {
                tracing::warn!(
                    others = others.len(),
                    "too many run artifacts directories to name each in the Read rules; \
                     the sandbox and the working-directory limit still close them"
                );
                others.truncate(MAX_SIBLING_RULES);
            }
            for dir in others {
                rules.push(crate::core::vault_guard::claude_rule("Read", &dir));
                // The root as given may be a link to the one just listed.
                let given = self.root.join(dir.file_name().unwrap_or_default());
                if given != dir {
                    rules.push(crate::core::vault_guard::claude_rule("Read", &given));
                }
            }
        }
        rules
    }
}

/// The directories a launch's arguments open for reading: Claude's `--add-dir`
/// and the read entries of the Codex permission profile.
fn granted_dirs(args: &[String]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for (at, arg) in args.iter().enumerate() {
        if arg == "--add-dir" {
            if let Some(dir) = args.get(at + 1) {
                dirs.push(PathBuf::from(dir));
            }
        }
        if arg.contains("=\"read\"") {
            codex_read_entries(arg, &mut dirs);
        }
    }
    dirs
}

/// Each `"<absolute path>"="read"` of a Codex `-c permissions=…` value.
fn codex_read_entries(text: &str, out: &mut Vec<PathBuf>) {
    let mut from = 0;
    while let Some(found) = text[from..].find("\"/") {
        let start = from + found;
        let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<String>();
        match stream.next() {
            Some(Ok(path)) => {
                let end = start + stream.byte_offset();
                if text[end..].starts_with("=\"read\"") {
                    out.push(PathBuf::from(path));
                }
                from = end.max(start + 1);
            }
            _ => from = start + 1,
        }
    }
}

#[cfg(test)]
#[path = "artifacts_reach_tests.rs"]
mod artifacts_reach_tests;
