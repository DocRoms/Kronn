//! KT-1587 — an agent launch reaches the artifacts of its own run, never of
//! another run's.
//!
//! The run artifacts root sits outside the data directory (KT-1585), so the
//! vault protection no longer hides it; without a rule of its own every agent,
//! in any project or discussion, would read every run's directory there (Exec
//! outputs, reports, screenshots). The root is therefore closed to every native
//! agent launch, and the run directory a launch was *granted* is opened again,
//! read-only.
//!
//! **The grant is typed, never parsed.** It is the `read_only_dirs` Kronn built
//! for the step (`workflows::steps::artifacts_read_only_dirs`), carried by the
//! `ReadOnlyRepos` policy to the launch. It is never read back out of an
//! argument vector: those hold the prompt, the system prompt (the repository's
//! instructions, skills, MCP context) and the ticket text, any of which can
//! spell `--add-dir <victim run>` or `"<victim run>"="read"`.
//! [`ArtifactsReach::new`] still vets every typed grant ([`vetted_grant`]).
//!
//! - **Seatbelt** (Codex, and every launch Kronn wraps): the root is denied,
//!   then the own directory allowed for reads; a later rule wins.
//! - **bubblewrap**: an empty tmpfs over the root, the own directory bound
//!   read-only on top of it.
//! - **Claude's own sandbox** (a launch that enables it cannot be wrapped: an
//!   OS sandbox does not nest, `sandbox_apply` fails inside one): `denyRead` on
//!   the root with `allowRead` on the own directory (an allow inside a denied
//!   read path re-opens it). That confines the shell. The file tools are
//!   permission rules, where a deny beats an allow: with no grant the root is
//!   denied whole; with one, every *other* run directory present at the launch
//!   is denied by its literal path, and a launch with more of them than the
//!   rules can name gets no grant at all ([`MAX_SIBLING_RULES`]).
//!
//!   What closes a run created after the launch, and the file tools over the
//!   root, is `permissions.blockReadsOutsideWorkingDirectories`, which the
//!   read-only policy always sets: per the settings schema of Claude Code
//!   2.1.285 it refuses "file-tool reads (Read, Grep, Glob, LSP) outside the
//!   working directories in every permission mode", and the own directory is
//!   the only `--add-dir` under the root. No `Glob(...)` or `Grep(...)` deny is
//!   emitted: the same release's settings guidance says path rules use
//!   `Read(path)` for reads and that `Glob(path)` rules "are not matched by
//!   file permission checks", so such a rule would be inert.
//!
//! Like the rest of the vault protection, all of it follows the owner's
//! setting and the host's sandbox: nothing is refused when either is missing.

use std::path::{Component, Path, PathBuf};

use crate::core::task_worker_sandbox::{real_path, seatbelt_literal};
use crate::core::trust_sandbox;

/// Literal denies for the Read tool of a Claude launch that has a grant. A
/// launch with more run directories than this gets no grant (fail closed): the
/// rules cannot name them all, and a run they did not name must not depend on
/// `blockReadsOutsideWorkingDirectories` alone. A rule is ~120 bytes, so the
/// cap keeps the `--settings` argument far below the per-argument limit.
const MAX_SIBLING_RULES: usize = 200;

/// What one launch may reach under the run artifacts root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactsReach {
    root: PathBuf,
    /// The run directories the launch was granted: canonical, direct children
    /// of the canonical root, named like a run.
    own: Vec<PathBuf>,
}

/// What Claude's settings carry for one launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeReach {
    /// `permissions.deny`.
    pub permission_deny: Vec<String>,
    /// `sandbox.filesystem.allowRead`.
    pub sandbox_allow_read: Vec<String>,
}

fn both(path: &Path) -> Vec<PathBuf> {
    let real = real_path(path);
    let mut all = vec![path.to_path_buf()];
    if real != path {
        all.push(real);
    }
    all
}

/// The canonical run directory `granted` names, or `None` (the launch then has
/// no grant). Defense in depth: the grant is typed, but it is checked as if it
/// were not.
///
/// - absolute, and no `..` component (a lexical `a/../b` is never resolved
///   into a grant);
/// - the path itself is a directory and not a link (`symlink_metadata`), and
///   it resolves (every other link on the way is followed by `canonicalize`);
/// - its canonical parent is the canonical root;
/// - its canonical name is a lowercase hyphenated UUID, the name Kronn gives a
///   run directory (`run_artifacts::is_run_dir_name`). Any other spelling, an
///   uppercase one included, is refused: on a case-insensitive volume (APFS by
///   default) it could name the same directory as another spelling, and the
///   rules compare paths as text.
fn vetted_grant(real_root: &Path, granted: &Path) -> Option<PathBuf> {
    if !granted.is_absolute()
        || granted
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return None;
    }
    let normalised: PathBuf = granted.components().collect();
    if !std::fs::symlink_metadata(&normalised).ok()?.is_dir() {
        return None;
    }
    let real = std::fs::canonicalize(&normalised).ok()?;
    let named_like_a_run = real
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(crate::workflows::run_artifacts::is_run_dir_name);
    (named_like_a_run && real.parent() == Some(real_root)).then_some(real)
}

impl ArtifactsReach {
    /// The reach of a launch granted the `granted` run directories (typed: the
    /// `read_only_dirs` Kronn built for the step), under `root`.
    pub fn new(root: PathBuf, granted: &[PathBuf]) -> Self {
        let mut own: Vec<PathBuf> = Vec::new();
        if let Ok(real_root) = std::fs::canonicalize(&root) {
            for dir in granted {
                if let Some(dir) = vetted_grant(&real_root, dir) {
                    if !own.contains(&dir) {
                        own.push(dir);
                    }
                }
            }
        }
        Self { root, own }
    }

    /// The reach of a launch granted `granted` on this host: `None` when the
    /// owner turned the protection off or no root exists.
    pub fn for_launch(granted: &[PathBuf]) -> Option<Self> {
        if !trust_sandbox::protect_vault_setting() {
            return None;
        }
        Some(Self::new(crate::workflows::run_artifacts::root()?, granted))
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
            rules.push_str(&format!("  (subpath {})\n", seatbelt_literal(own)));
        }
        rules.push_str(")\n");
        rules
    }

    /// `bwrap` mounts, to come after the other mounts and before `--`: an empty
    /// tmpfs over the resolved root, each own directory bound read-only on top.
    /// A root that does not exist holds nothing to read.
    pub fn bubblewrap_mounts(&self) -> Vec<String> {
        let root = real_path(&self.root);
        if !root.is_dir() {
            return Vec::new();
        }
        let mut mounts = vec!["--tmpfs".to_string(), root.to_string_lossy().into_owned()];
        for own in &self.own {
            let text = own.to_string_lossy().into_owned();
            mounts.extend(["--ro-bind".to_string(), text.clone(), text]);
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

    /// The Claude tools' rules and the sandbox's `allowRead`, decided together
    /// so a launch is never half open.
    ///
    /// Edit and Write are denied on the root: the own directory is read-only
    /// for the launch. Read is denied whole without a grant; with one, each
    /// other entry of the root is denied by its literal path, since a deny
    /// cannot be carved out by an allow. A root that cannot be listed, or that
    /// holds more entries than [`MAX_SIBLING_RULES`], leaves the launch without
    /// its grant.
    pub fn claude_reach(&self) -> ClaudeReach {
        let others = if self.own.is_empty() {
            None
        } else {
            self.other_entries()
        };
        let mut permission_deny = Vec::new();
        for dir in both(&self.root) {
            for tool in ["Edit", "Write"] {
                permission_deny.push(crate::core::vault_guard::claude_rule(tool, &dir));
            }
            if others.is_none() {
                permission_deny.push(crate::core::vault_guard::claude_rule("Read", &dir));
            }
        }
        let Some(others) = others else {
            return ClaudeReach {
                permission_deny,
                sandbox_allow_read: Vec::new(),
            };
        };
        for dir in others {
            permission_deny.push(crate::core::vault_guard::claude_rule("Read", &dir));
            // The root as given may be a link to the one just listed.
            let given = self.root.join(dir.file_name().unwrap_or_default());
            if given != dir {
                permission_deny.push(crate::core::vault_guard::claude_rule("Read", &given));
            }
        }
        ClaudeReach {
            permission_deny,
            sandbox_allow_read: self
                .own
                .iter()
                .map(|own| own.to_string_lossy().into_owned())
                .collect(),
        }
    }

    /// The entries of the root that are not an own directory, sorted; `None`
    /// when the root cannot be listed or holds too many to name.
    fn other_entries(&self) -> Option<Vec<PathBuf>> {
        let own_names: Vec<&std::ffi::OsStr> =
            self.own.iter().filter_map(|own| own.file_name()).collect();
        let mut others: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(real_path(&self.root)).ok()? {
            let entry = entry.ok()?;
            if !own_names.contains(&entry.file_name().as_os_str()) {
                others.push(entry.path());
            }
        }
        if others.len() > MAX_SIBLING_RULES {
            tracing::warn!(
                others = others.len(),
                "too many run artifacts directories to name each in the Read rules: \
                 this launch is not granted its own run's artifacts"
            );
            return None;
        }
        others.sort();
        Some(others)
    }
}

#[cfg(test)]
#[path = "artifacts_reach_tests.rs"]
mod artifacts_reach_tests;
