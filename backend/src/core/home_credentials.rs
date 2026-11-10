//! KT-1656 — the credential files of the owner's home, out of reach of an agent
//! that does not own them.
//!
//! An agent runs as the owner. A task worker that probed Gemini CLI printed
//! `~/.gemini/settings.json`, tokens included, into its transcript, which is
//! stored and sent to the model provider. The sandbox of an agent therefore
//! denies the credential stores of the home directory, in two tiers:
//!
//! - **other agents' stores** (`~/.gemini`, `~/.codex`, `~/.claude.json`, …) for
//!   **every** confined launch: no agent has a reason to read the login of
//!   another one. The store of the agent that is running stays open (its process
//!   needs it), unless only its *shell* is sandboxed (a Claude task worker: the
//!   CLI itself runs outside its own sandbox), in which case nothing of the
//!   kind is readable by the commands it runs;
//! - **shared stores** (`~/.ssh`, `~/.aws`, `~/.config/gh`, `~/.npmrc`,
//!   `~/.cargo/credentials*`, …) for the **sandboxed** launches only (a task
//!   worker, a read-only review, a Codex `read-only` / `workspace-write`
//!   launch). An agent the owner started in full access keeps the tools of the
//!   owner's terminal, as the vault note states
//!   (`docs/architecture/agent-vault-protection.md`).
//!
//! Only stores that exist are listed, as given and with their links resolved
//! (Seatbelt matches real paths; bubblewrap mounts by path).
//!
//! Each store is hidden **where the tools would put it on this host**: under
//! the home, and wherever the environment of the Kronn process relocates it
//! (`CODEX_HOME`, `XDG_CONFIG_HOME`, `KUBECONFIG`, …). A store moved by one of
//! those variables would otherwise stay readable while its default path, now
//! empty, is hidden. The variables are read through
//! [`crate::core::child_env::var`], like every other read of the environment.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::core::child_env::AgentFamily;
use crate::core::task_worker_sandbox::real_path;

/// Which tier of the deny list a launch gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// A sandboxed launch (task worker, review, Codex `read-only` /
    /// `workspace-write`): the stores of the other agents and the shared ones.
    Sandboxed,
    /// A launch the owner opened in full access: the stores of the other
    /// agents only.
    FullAccess,
}

/// How an environment variable relocates a store.
#[derive(Debug, Clone, Copy)]
enum Override {
    /// The variable names the store itself (a directory or a file).
    Path(&'static str),
    /// The variable names a directory that holds `.1` (`CARGO_HOME` holds
    /// `credentials`).
    Under(&'static str, &'static str),
    /// The variable is a list of paths, each of them a store (`KUBECONFIG`).
    PathList(&'static str),
}

/// A credential store: where it is under the home, and the variables that move
/// it. A store under `.config/` also follows `XDG_CONFIG_HOME`, one under
/// `.local/share/` follows `XDG_DATA_HOME`.
struct Store {
    rel: &'static str,
    moved_by: &'static [Override],
}

const fn at(rel: &'static str) -> Store {
    Store { rel, moved_by: &[] }
}

const fn moved(rel: &'static str, moved_by: &'static [Override]) -> Store {
    Store { rel, moved_by }
}

/// The files and directories that hold the login of one agent CLI: its tokens,
/// its OAuth session, the `env` of its MCP servers.
const AGENT_STORES: &[(AgentFamily, Store)] = &[
    (
        AgentFamily::Claude,
        moved(
            ".claude/.credentials.json",
            &[Override::Under("CLAUDE_CONFIG_DIR", ".credentials.json")],
        ),
    ),
    (
        AgentFamily::Claude,
        moved(
            ".claude.json",
            &[Override::Under("CLAUDE_CONFIG_DIR", ".claude.json")],
        ),
    ),
    (
        AgentFamily::Codex,
        moved(".codex", &[Override::Path("CODEX_HOME")]),
    ),
    (
        AgentFamily::Gemini,
        moved(".gemini", &[Override::Under("GEMINI_CLI_HOME", ".gemini")]),
    ),
    (AgentFamily::Copilot, at(".copilot")),
    (AgentFamily::Copilot, at(".config/github-copilot")),
    (AgentFamily::Kiro, at(".kiro")),
    (AgentFamily::Kiro, at(".local/share/kiro-cli")),
    (AgentFamily::Vibe, at(".vibe")),
    (AgentFamily::OpenCode, at(".local/share/opencode")),
    (AgentFamily::OpenCode, at(".config/opencode")),
];

/// Credential stores of the tools an agent's shell may find in the home.
const SHARED_STORES: &[Store] = &[
    at(".ssh"),
    moved(".gnupg", &[Override::Path("GNUPGHOME")]),
    moved(
        ".aws",
        &[
            Override::Path("AWS_SHARED_CREDENTIALS_FILE"),
            Override::Path("AWS_CONFIG_FILE"),
        ],
    ),
    at(".azure"),
    moved(".config/gcloud", &[Override::Path("CLOUDSDK_CONFIG")]),
    moved(".config/gh", &[Override::Path("GH_CONFIG_DIR")]),
    at(".config/glab-cli"),
    at(".config/hub"),
    at(".config/git/credentials"),
    at(".git-credentials"),
    at(".netrc"),
    moved(".npmrc", &[Override::Path("NPM_CONFIG_USERCONFIG")]),
    at(".yarnrc.yml"),
    at(".pypirc"),
    moved(
        ".cargo/credentials",
        &[Override::Under("CARGO_HOME", "credentials")],
    ),
    moved(
        ".cargo/credentials.toml",
        &[Override::Under("CARGO_HOME", "credentials.toml")],
    ),
    moved(
        ".docker/config.json",
        &[Override::Under("DOCKER_CONFIG", "config.json")],
    ),
    moved(".kube/config", &[Override::PathList("KUBECONFIG")]),
    at(".m2/settings.xml"),
    at(".gradle/gradle.properties"),
    at(".terraform.d/credentials.tfrc.json"),
    at(".vault-token"),
];

/// The credential stores of this host's home that a launch must not reach.
/// `running` is the agent whose own store stays open; `None` for a process
/// whose own login the commands it runs never need (a Claude worker's shell).
pub fn hidden(running: Option<AgentFamily>, reach: Reach) -> Vec<PathBuf> {
    match crate::core::trust_sandbox::host_home() {
        Some(home) => hidden_in(&home, running, reach),
        None => Vec::new(),
    }
}

/// [`hidden`] for the given home directory, with the environment of the Kronn
/// process.
pub fn hidden_in(home: &Path, running: Option<AgentFamily>, reach: Reach) -> Vec<PathBuf> {
    hidden_with(home, running, reach, &|name| {
        crate::core::child_env::var_os(name)
    })
}

/// [`hidden_in`] with the environment given by `env`.
fn hidden_with(
    home: &Path,
    running: Option<AgentFamily>,
    reach: Reach,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Vec<PathBuf> {
    let others = AGENT_STORES
        .iter()
        .filter(|(owner, _)| Some(*owner) != running)
        .map(|(_, store)| store);
    let shared = SHARED_STORES.iter().filter(|_| reach == Reach::Sandboxed);
    let mut all: Vec<PathBuf> = others
        .chain(shared)
        .flat_map(|store| locations(home, store, env))
        // A dangling link still names a store; a path that is not there holds
        // nothing, and bubblewrap cannot mount over it.
        .filter(|path| path.symlink_metadata().is_ok())
        .flat_map(|path| {
            let real = real_path(&path);
            [path, real]
        })
        .collect();
    all.sort();
    all.dedup();
    all
}

/// Every place `store` can be on this host: its default under the home and
/// each place the environment moves it to. The default stays listed when a
/// variable moves the store: a tool that is run without that variable (or
/// with another) still finds it there.
fn locations(home: &Path, store: &Store, env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut places = vec![home.join(store.rel)];
    let xdg = [
        (".config/", "XDG_CONFIG_HOME"),
        (".local/share/", "XDG_DATA_HOME"),
    ];
    for (prefix, variable) in xdg {
        if let Some(rest) = store.rel.strip_prefix(prefix) {
            places.extend(variable_path(env, variable).map(|base| base.join(rest)));
        }
    }
    for rule in store.moved_by {
        match *rule {
            Override::Path(variable) => places.extend(variable_path(env, variable)),
            Override::Under(variable, suffix) => {
                places.extend(variable_path(env, variable).map(|base| base.join(suffix)));
            }
            Override::PathList(variable) => {
                if let Some(list) = env(variable) {
                    places.extend(std::env::split_paths(&list).filter(|path| path.is_absolute()));
                }
            }
        }
    }
    // A variable that names the home or one of its parents (`GNUPGHOME=$HOME`)
    // would hide everything the agent has; the stores in it are listed by
    // their own paths.
    places.retain(|path| !home.starts_with(path));
    places
}

/// The absolute path a variable holds. A relative one is relative to the
/// working directory of the tool that reads it, which is not known here, and
/// the XDG specification ignores it.
fn variable_path(env: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    env(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

#[cfg(test)]
#[path = "home_credentials_tests.rs"]
mod home_credentials_tests;
