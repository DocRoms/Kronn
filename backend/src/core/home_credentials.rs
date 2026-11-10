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

/// The files and directories (relative to the home) that hold the login of one
/// agent CLI: its tokens, its OAuth session, the `env` of its MCP servers.
const AGENT_STORES: &[(AgentFamily, &str)] = &[
    (AgentFamily::Claude, ".claude/.credentials.json"),
    (AgentFamily::Claude, ".claude.json"),
    (AgentFamily::Codex, ".codex"),
    (AgentFamily::Gemini, ".gemini"),
    (AgentFamily::Copilot, ".copilot"),
    (AgentFamily::Copilot, ".config/github-copilot"),
    (AgentFamily::Kiro, ".kiro"),
    (AgentFamily::Kiro, ".local/share/kiro-cli"),
    (AgentFamily::Vibe, ".vibe"),
    (AgentFamily::OpenCode, ".local/share/opencode"),
    (AgentFamily::OpenCode, ".config/opencode"),
];

/// Credential stores of the tools an agent's shell may find in the home.
const SHARED_STORES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".config/gh",
    ".config/glab-cli",
    ".config/hub",
    ".config/git/credentials",
    ".git-credentials",
    ".netrc",
    ".npmrc",
    ".yarnrc.yml",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".docker/config.json",
    ".kube/config",
    ".m2/settings.xml",
    ".gradle/gradle.properties",
    ".terraform.d/credentials.tfrc.json",
    ".vault-token",
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

/// [`hidden`] for the given home directory.
pub fn hidden_in(home: &Path, running: Option<AgentFamily>, reach: Reach) -> Vec<PathBuf> {
    let others = AGENT_STORES
        .iter()
        .filter(|(owner, _)| Some(*owner) != running)
        .map(|(_, rel)| *rel);
    let shared = SHARED_STORES
        .iter()
        .copied()
        .filter(|_| reach == Reach::Sandboxed);
    let mut all: Vec<PathBuf> = others
        .chain(shared)
        .map(|rel| home.join(rel))
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

#[cfg(test)]
#[path = "home_credentials_tests.rs"]
mod home_credentials_tests;
