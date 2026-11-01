//! KT-1495 — the Kronn vault out of reach of the agent processes.
//!
//! An agent CLI runs as the user who runs Kronn, so its own tools (a shell, a
//! file reader) could read `encryption_key`, `recovery.key`,
//! `human-admin-secret`, `config.toml`, `trust-proof.json` and `kronn.db`
//! whatever Kronn's gates say. The OS confinement of KT-1492
//! ([`crate::core::trust_sandbox`]) is therefore applied to **every native
//! agent process** at its one spawn point (`agents::runner::try_spawn`: the
//! CLIs, the ACP runtimes, the audits, the task workers), and not only to the
//! lines a trust level granted.
//!
//! The owner's rule is "never bridle more than before": the sandbox denies read
//! and write on the Kronn data directory and **nothing else**. The Kronn API
//! port, the network, the rest of the disk and the agent's own tools stay as
//! they were; the agents use the API, never the files.
//!
//! Two launches start an OS sandbox of their own, and a Seatbelt profile cannot
//! be applied from inside another one (`sandbox_apply: Operation not
//! permitted`, measured on macOS):
//!
//! - a **Claude** launch whose `--settings` enable its sandbox (the task
//!   workers). Its own sandbox is told the data directory in
//!   `sandbox.filesystem.denyRead` ([`claude_sandbox_deny_read`]), which Claude
//!   enforces at the OS level;
//! - a **Codex** launch that keeps Codex's own `read-only` / `workspace-write`
//!   sandbox, which has no setting that hides one directory. Kronn **replaces**
//!   it: Codex runs `--sandbox=danger-full-access` inside a profile Kronn
//!   generates that is equivalent to the mode it would have had
//!   ([`CodexMode`]): read-only writes nothing but the temp directories and
//!   Codex's home, workspace-write adds the worktree (its `.git` stays
//!   read-only), and the data directory is denied on top of both.
//!
//! The one thing the replacement cannot keep is Codex's per-command network
//! block: the profile wraps the whole Codex process, which needs the network
//! for its own model calls, so the commands it runs get the host network. See
//! `docs/architecture/agent-vault-protection.md`.
//!
//! Every Claude launch also carries `permissions.deny` rules for the data
//! directory ([`claude_permission_deny`]): defence in depth for the Read tool.
//!
//! Without a sandbox on the host (Windows, no `bwrap`, a container that forbids
//! user namespaces) or with the setting turned off, nothing is wrapped and
//! nothing is refused: the protection is reported as unavailable or disabled
//! ([`crate::core::trust_sandbox::VaultProtection`]) and the launch runs as it
//! did.

use std::path::{Path, PathBuf};

use crate::core::task_worker_sandbox::{real_path, seatbelt_literal, Mechanism};
use crate::core::trust_sandbox::{self, LevelConfinement};

/// The sandbox mode a Codex launch would run in by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexMode {
    /// Codex's default for `exec`: reads the disk, writes nothing.
    ReadOnly,
    /// `--sandbox=workspace-write`: writes the working directory and the temp
    /// directories.
    WorkspaceWrite,
}

/// Whether the launch brings an OS sandbox of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchSandbox {
    /// The agent runs with the full privileges of the user: Kronn wraps it.
    None,
    /// The CLI starts its own OS sandbox that Kronn cannot reach into (a Claude
    /// task worker, which takes the data directory in its own settings).
    Own,
    /// Codex keeps its own sandbox: Kronn replaces it with an equivalent one
    /// that also hides the data directory.
    Codex(CodexMode),
}

/// What [`wrap_launch`] did to one launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confined {
    /// The program and arguments, behind the sandbox launcher.
    Wrapped,
    /// Codex's own sandbox replaced by Kronn's equivalent profile.
    Replaced,
    /// Left as it was, and why (surfaced in the debug log, never an error).
    Unchanged(&'static str),
}

/// The directories a replaced Codex sandbox lets the process write.
#[derive(Debug, Clone, Default)]
pub struct LaunchRoots {
    /// The agent's working directory (the worktree of a task worker).
    pub work_dir: PathBuf,
    /// Codex's own home (`CODEX_HOME`, else `~/.codex`): its sessions, logs
    /// and refreshed credentials.
    pub codex_home: Option<PathBuf>,
    /// The package cache of an `npx` launch.
    pub npm_cache: Option<PathBuf>,
    /// The temporary directories, which both Codex modes write.
    pub temp: Vec<PathBuf>,
}

impl LaunchRoots {
    /// The roots of a launch in `work_dir` on this host.
    pub fn discover(work_dir: &Path, npx: bool) -> Self {
        let home = crate::core::child_env::var("HOME").ok().map(PathBuf::from);
        let codex_home = crate::core::child_env::var("CODEX_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".codex")));
        let mut temp: Vec<PathBuf> = ["/tmp", "/var/tmp", "/private/tmp", "/dev/shm"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        if cfg!(target_os = "macos") {
            // `TMPDIR` and the per-user cache live under here.
            temp.push(PathBuf::from("/private/var/folders"));
        }
        if let Some(dir) = crate::core::child_env::var_os("TMPDIR") {
            temp.push(PathBuf::from(dir));
        }
        Self {
            work_dir: work_dir.to_path_buf(),
            codex_home,
            npm_cache: if npx {
                home.map(|home| home.join(".npm"))
            } else {
                None
            },
            temp,
        }
    }
}

/// Whether `args` of a launch of `binary` (or its `npx` package) make the CLI
/// start an OS sandbox of its own. Pure.
pub fn launch_sandbox(binary: &str, npx_package: Option<&str>, args: &[String]) -> LaunchSandbox {
    let claude = binary == "claude" || npx_package == Some("@anthropic-ai/claude-code");
    let codex = binary == "codex" || npx_package == Some("@openai/codex");
    if claude && claude_settings_enable_sandbox(args) {
        return LaunchSandbox::Own;
    }
    if codex && codex_keeps_its_sandbox(args) {
        return LaunchSandbox::Codex(codex_mode(args));
    }
    LaunchSandbox::None
}

fn claude_settings_enable_sandbox(args: &[String]) -> bool {
    args.windows(2).any(|pair| {
        pair[0] == "--settings"
            && serde_json::from_str::<serde_json::Value>(&pair[1])
                .is_ok_and(|settings| settings["sandbox"]["enabled"] == true)
    })
}

/// Codex keeps its own sandbox unless the launch turns it off: `exec` runs in
/// a `read-only` sandbox by default.
fn codex_keeps_its_sandbox(args: &[String]) -> bool {
    const OFF: [&str; 3] = [
        "--sandbox=danger-full-access",
        "--dangerously-bypass-approvals-and-sandbox",
        "sandbox_mode=\"danger-full-access\"",
    ];
    let spelled_apart = args.windows(2).any(|pair| {
        matches!(pair[0].as_str(), "--sandbox" | "-s") && pair[1] == "danger-full-access"
    });
    !(spelled_apart || args.iter().any(|arg| OFF.contains(&arg.as_str())))
}

/// The mode Codex would run in: the `--sandbox` it was given, else read-only.
fn codex_mode(args: &[String]) -> CodexMode {
    let given = args.iter().enumerate().find_map(|(at, arg)| {
        arg.strip_prefix("--sandbox=")
            .map(str::to_owned)
            .or_else(|| {
                matches!(arg.as_str(), "--sandbox" | "-s")
                    .then(|| args.get(at + 1).cloned())
                    .flatten()
            })
    });
    if given.as_deref() == Some("workspace-write") {
        CodexMode::WorkspaceWrite
    } else {
        CodexMode::ReadOnly
    }
}

/// `args` with Codex's own sandbox switched off: any `--sandbox` it carried is
/// dropped and `--sandbox=danger-full-access` follows `exec`. `None` when this
/// is not an `exec` launch.
fn codex_without_its_sandbox(args: &[String]) -> Option<Vec<String>> {
    args.iter().position(|arg| arg == "exec")?;
    let mut kept = Vec::with_capacity(args.len() + 1);
    let mut skip_value = false;
    for arg in args {
        if std::mem::take(&mut skip_value) {
            continue;
        }
        match arg.as_str() {
            "--sandbox" | "-s" => skip_value = true,
            other if other.starts_with("--sandbox=") => {}
            _ => kept.push(arg.clone()),
        }
    }
    let at = kept.iter().position(|arg| arg == "exec")?;
    kept.insert(at + 1, "--sandbox=danger-full-access".to_string());
    Some(kept)
}

fn with_real_paths(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = dirs
        .into_iter()
        .flat_map(|dir| {
            let real = real_path(&dir);
            [dir, real]
        })
        .collect();
    all.sort();
    all.dedup();
    all
}

/// What a Codex launch in `mode` may write, as given and as resolved.
pub fn codex_writable(mode: CodexMode, roots: &LaunchRoots) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = roots.temp.clone();
    dirs.extend(roots.codex_home.clone());
    dirs.extend(roots.npm_cache.clone());
    if mode == CodexMode::WorkspaceWrite {
        dirs.push(roots.work_dir.clone());
    }
    with_real_paths(dirs)
}

/// What stays read-only inside a writable root: the repository metadata of the
/// worktree, as Codex's own `workspace-write` keeps it.
fn codex_read_only_inside(mode: CodexMode, roots: &LaunchRoots) -> Vec<PathBuf> {
    if mode == CodexMode::WorkspaceWrite {
        with_real_paths(vec![roots.work_dir.join(".git")])
    } else {
        Vec::new()
    }
}

/// The Seatbelt profile that replaces Codex's own sandbox: allow by default
/// (the network, the Kronn API port and every read stay open, as they are for
/// Codex's own process), no write outside the mode's roots, the data directory
/// neither read nor written. The last matching rule wins.
pub fn codex_seatbelt_profile(
    mode: CodexMode,
    roots: &LaunchRoots,
    protected: &[PathBuf],
) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
    profile.push_str("(allow file-write*\n  (subpath \"/dev\")\n");
    for dir in codex_writable(mode, roots) {
        profile.push_str(&format!("  (subpath {})\n", seatbelt_literal(&dir)));
    }
    profile.push_str(")\n");
    let read_only = codex_read_only_inside(mode, roots);
    if !read_only.is_empty() {
        profile.push_str("(deny file-write*\n");
        for dir in &read_only {
            profile.push_str(&format!("  (subpath {})\n", seatbelt_literal(dir)));
        }
        profile.push_str(")\n");
    }
    profile.push_str(&trust_sandbox::seatbelt_hide_rules(protected));
    profile
}

/// The `bwrap` arguments that replace Codex's own sandbox: the filesystem
/// read-only, each existing writable root bound read-write, the data directory
/// an empty tmpfs, the host network kept. Later mounts win, so the tmpfs of the
/// data directory comes last.
pub fn codex_bubblewrap_args(
    mode: CodexMode,
    roots: &LaunchRoots,
    protected: &[PathBuf],
) -> Vec<String> {
    // Own PID namespace and a fresh `/proc`: the host's `/proc` would let
    // `/proc/<backend pid>/root/<data dir>` walk around the tmpfs.
    let mut args: Vec<String> = [
        "--ro-bind",
        "/",
        "/",
        "--dev-bind",
        "/dev",
        "/dev",
        "--unshare-pid",
        "--proc",
        "/proc",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    for dir in codex_writable(mode, roots)
        .iter()
        .filter(|dir| dir.is_dir())
    {
        let text = dir.to_string_lossy().into_owned();
        args.extend(["--bind".to_string(), text.clone(), text]);
    }
    for dir in codex_read_only_inside(mode, roots)
        .iter()
        .filter(|dir| dir.exists())
    {
        let text = dir.to_string_lossy().into_owned();
        args.extend(["--ro-bind".to_string(), text.clone(), text]);
    }
    for dir in trust_sandbox::bubblewrap_mount_points(protected) {
        args.extend(["--tmpfs".to_string(), dir]);
    }
    args.extend(
        ["--share-net", "--die-with-parent", "--"]
            .into_iter()
            .map(String::from),
    );
    args
}

/// `program args` behind the sandbox, or unchanged. Pure: nothing is started.
///
/// `--new-session` is dropped from the Linux arguments: the agent is its own
/// process-group leader (`setpgid` before the exec, so a cancellation takes the
/// whole tree), and `setsid` refuses a group leader. An agent has no terminal.
pub fn wrap_launch(
    confinement: Option<&LevelConfinement>,
    sandbox: LaunchSandbox,
    roots: &LaunchRoots,
    program: String,
    args: Vec<String>,
) -> (String, Vec<String>, Confined) {
    let Some(confinement) = confinement else {
        return (
            program,
            args,
            Confined::Unchanged("vault protection is off"),
        );
    };
    match sandbox {
        LaunchSandbox::Own => {
            return (
                program,
                args,
                Confined::Unchanged("the CLI starts a sandbox of its own"),
            );
        }
        LaunchSandbox::Codex(mode) => {
            let Some(without) = codex_without_its_sandbox(&args) else {
                return (
                    program,
                    args,
                    Confined::Unchanged("a Codex launch that is not `exec`"),
                );
            };
            let mut wrapped = match confinement.mechanism() {
                Mechanism::Seatbelt => vec![
                    "-p".to_string(),
                    codex_seatbelt_profile(mode, roots, confinement.protected()),
                ],
                Mechanism::Bubblewrap => {
                    codex_bubblewrap_args(mode, roots, confinement.protected())
                }
            };
            wrapped.push(program);
            wrapped.extend(without);
            return (
                confinement.mechanism().launcher().to_string(),
                wrapped,
                Confined::Replaced,
            );
        }
        LaunchSandbox::None => {}
    }
    let (launcher, mut wrapped) = confinement.wrap(&program, &args);
    wrapped.retain(|arg| arg != "--new-session");
    (launcher, wrapped, Confined::Wrapped)
}

/// The confinement an agent launch gets on this host: `None` when the owner
/// turned the protection off or the host has no sandbox. Never an error.
pub fn launch_confinement() -> Option<LevelConfinement> {
    // Unit tests spawn throwaway programs (`sh`, `python`) through the real
    // spawn point, from inside whatever sandbox the machine runs them in; the
    // ones about the wrap choose with `set_test_wrap`.
    #[cfg(test)]
    {
        if !test_wrap() {
            return None;
        }
    }
    trust_sandbox::confinement()
}

/// The data directory as Claude's rules name it: `Read(//abs/path/**)`, the
/// double slash being Claude's spelling of a path from the filesystem root.
fn claude_rule(tool: &str, dir: &Path) -> String {
    format!(
        "{tool}(/{}/**)",
        dir.to_string_lossy().trim_end_matches('/')
    )
}

/// The directories to hide, as given and as resolved, when the owner kept the
/// protection on (independent of the host's sandbox: a CLI's own rule needs no
/// OS facility).
fn hidden_dirs() -> Vec<PathBuf> {
    if trust_sandbox::protect_vault_setting() {
        trust_sandbox::protected_dirs()
    } else {
        Vec::new()
    }
}

/// `permissions.deny` for every Claude launch: the Read, Edit and Write tools
/// refuse the data directory, `--dangerously-skip-permissions` included (a deny
/// rule is checked before the permission mode).
pub fn claude_permission_deny() -> Vec<String> {
    hidden_dirs()
        .iter()
        .flat_map(|dir| {
            ["Read", "Edit", "Write"]
                .into_iter()
                .map(|tool| claude_rule(tool, dir))
        })
        .collect()
}

/// `sandbox.filesystem.denyRead` of Claude's own sandbox (the task workers).
pub fn claude_sandbox_deny_read() -> Vec<String> {
    hidden_dirs()
        .iter()
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect()
}

/// The `--settings` value every Claude launch without a sandbox of its own
/// carries, or `None` when the protection is off.
pub fn claude_vault_settings() -> Option<String> {
    let deny = claude_permission_deny();
    if deny.is_empty() {
        return None;
    }
    serde_json::to_string(&serde_json::json!({ "permissions": { "deny": deny } })).ok()
}

/// A policy that brings its own Claude `--settings` (the read-only repositories)
/// replaces the standalone vault one: two `--settings` flags are not merged by
/// Claude, so the vault rules are folded into the policy's own document instead.
/// Does nothing when the protection is off or no policy document is present.
pub fn fold_vault_into_claude_settings(args: &mut Vec<String>) {
    let Some(vault) = claude_vault_settings() else {
        return;
    };
    let standalone = args
        .windows(2)
        .position(|pair| pair[0] == "--settings" && pair[1] == vault);
    let Some(own) = args.iter().enumerate().position(|(at, arg)| {
        arg == "--settings"
            && Some(at) != standalone
            && args.get(at + 1).is_some_and(|value| value.starts_with('{'))
    }) else {
        return;
    };
    let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&args[own + 1]) else {
        return;
    };
    let push_unique = |target: &mut serde_json::Value, values: Vec<String>| {
        if !target.is_array() {
            *target = serde_json::json!([]);
        }
        let list = target.as_array_mut().expect("just made an array");
        for value in values {
            if !list.iter().any(|existing| existing == value.as_str()) {
                list.push(value.into());
            }
        }
    };
    push_unique(
        &mut settings["permissions"]["deny"],
        claude_permission_deny(),
    );
    if settings["sandbox"]["enabled"] == true {
        push_unique(
            &mut settings["sandbox"]["filesystem"]["denyRead"],
            claude_sandbox_deny_read(),
        );
    }
    args[own + 1] = settings.to_string();
    if let Some(at) = standalone {
        args.drain(at..at + 2);
    }
}

#[cfg(test)]
static TEST_WRAP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tests opt into the real wrap of `try_spawn`.
#[cfg(test)]
pub fn set_test_wrap(on: bool) {
    TEST_WRAP.store(on, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
fn test_wrap() -> bool {
    TEST_WRAP.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
#[path = "vault_guard_tests.rs"]
mod vault_guard_tests;
