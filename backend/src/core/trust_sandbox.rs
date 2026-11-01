//! KT-1492 — the protection of Kronn's vault from what a trust level grants.
//!
//! A line a level approved (a Quick Exec, an Exec step), a validation a level
//! reached Docker or the host for, runs as the user on the host: it could read
//! the Kronn data directory (`kronn.db`, `encryption_key`, `recovery.key`,
//! `human-admin-secret`, `config.toml`, the TOTP seed, the backups). So
//! everything a level grants runs under an OS sandbox that closes exactly one
//! door and nothing else:
//!
//! - **no read, no write** on the data directory (and on `KRONN_BACKUP_DIR`
//!   when it is set elsewhere).
//!
//! Everything else stays open, as it is in the owner's terminal: project
//! databases, the network, **the Kronn API port** (the agents' MCP bridge and
//! the owner's Quick Exec scripts go through it; the human-only routes are
//! protected by the TOTP proof, not by the network), Docker, other
//! repositories and the credentials of third-party CLIs (`~/.aws`, `gh`). The
//! risk panel says so.
//!
//! macOS uses a Seatbelt profile (`sandbox-exec`, allow by default, one deny).
//! Linux uses bubblewrap: the data directory is replaced by an empty tmpfs and
//! the host network is kept (`--share-net`). Landlock was not used: it has no
//! way to hide a directory from a command and keep the rest of the filesystem.
//!
//! Vault protection is a setting, "Protect the Kronn vault from agents"
//! (`server.protect_vault_from_agents`, on by default). It has three states
//! ([`VaultProtection`]): **active**, **disabled** in the config (the owner's
//! choice: level-granted commands run unconfined), or **unavailable** on a host
//! with no sandbox (Windows, no `bwrap`, a container that forbids user
//! namespaces). Neither of the last two refuses a level or a command: the level
//! still needs the TOTP proof and an acknowledgement that names the missing
//! protection.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::child_env::ChildRoute;
use crate::core::task_worker_env::ProbeCommand;
use crate::core::task_worker_sandbox::{
    real_path, seatbelt_literal, CommandConfinement, Mechanism,
};

static PROTECT_VAULT: AtomicBool = AtomicBool::new(true);

/// Record the "Protect the Kronn vault from agents" setting, at startup.
pub fn set_protect_vault(on: bool) {
    PROTECT_VAULT.store(on, Ordering::Relaxed);
}

/// Whether the owner wants the vault protected (the setting, not the host).
pub fn protect_vault_setting() -> bool {
    PROTECT_VAULT.load(Ordering::Relaxed)
}

/// What a level-confined command must not reach: the data directory and,
/// when it lives elsewhere, the backup directory. Each as given and as
/// resolved (Seatbelt matches real paths, bubblewrap mounts by path).
pub fn protected_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = crate::core::config::config_dir() {
        dirs.push(dir);
    }
    if let Ok(dir) = crate::core::child_env::var("KRONN_BACKUP_DIR") {
        if !dir.trim().is_empty() {
            dirs.push(PathBuf::from(dir));
        }
    }
    protected_from(dirs)
}

fn protected_from(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
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

/// The Seatbelt profile: allow by default, deny the data directory. The last
/// matching rule wins, so the deny comes last. The network is not touched.
pub fn seatbelt_profile(protected: &[PathBuf]) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n");
    profile.push_str(&seatbelt_hide_rules(protected));
    profile
}

/// The `bwrap` arguments: the host filesystem as it is, each existing
/// protected directory replaced by an empty tmpfs (what a command writes there
/// dies with it), and the host network kept.
///
/// The command gets its own PID namespace and a fresh `/proc`: without them the
/// host's `/proc` stays mounted, and `/proc/<backend pid>/root/<data dir>/…`
/// walks around the tmpfs (same uid, so the kernel allows it).
pub fn bubblewrap_args(protected: &[PathBuf]) -> Vec<String> {
    let mut args: Vec<String> = [
        "--bind",
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
    for dir in bubblewrap_mount_points(protected) {
        args.extend(["--tmpfs".to_string(), dir]);
    }
    args.extend(
        ["--share-net", "--die-with-parent", "--new-session", "--"]
            .into_iter()
            .map(String::from),
    );
    args
}

/// Where bubblewrap mounts the empty tmpfs: each existing protected directory
/// **resolved**, once. `/` is bind-mounted, so a tmpfs over a symlinked alias
/// (a linked `$HOME`, a `KRONN_DATA_DIR` through a link) would leave the real
/// directory readable through its canonical path. A directory that does not
/// exist holds nothing to read and is left out.
pub fn bubblewrap_mount_points(protected: &[PathBuf]) -> Vec<String> {
    let mut points: Vec<String> = protected
        .iter()
        .map(|dir| real_path(dir))
        .filter(|dir| dir.is_dir())
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect();
    points.sort();
    points.dedup();
    points
}

/// The Seatbelt deny rules of the protected directories alone, appended to a
/// stricter profile (a Docker validation keeps its worker sandbox and also
/// loses the data directory).
pub fn seatbelt_hide_rules(protected: &[PathBuf]) -> String {
    if protected.is_empty() {
        return String::new();
    }
    let mut rules = String::from("(deny file-read* file-write*\n");
    for dir in protected {
        rules.push_str(&format!("  (subpath {})\n", seatbelt_literal(dir)));
    }
    rules.push_str(")\n");
    rules
}

/// Whether an argument names the data directory or a directory containing it:
/// a Docker volume (`-v ~/.config:/x`, `-v /:/host`) would hand the daemon's
/// root view of it to a container. Every source is expanded the way a shell
/// would (`~`, `$HOME`, `${VAR}`), a relative one is joined to `cwd` (the
/// validation's directory, not the backend's), and each is compared as written
/// **and** with its symlinks resolved (a worktree link to the data directory
/// is the data directory). Best effort on the argv Kronn sees; the daemon
/// itself is root and cannot be confined from here, so a script, a compose
/// file or any program that runs `docker` itself is out of its reach.
pub fn argv_reaches_protected(
    argv: &[String],
    protected: &[PathBuf],
    cwd: &Path,
) -> Option<String> {
    argv_reaches_protected_from(argv, protected, cwd, host_home().as_deref())
}

/// [`argv_reaches_protected`] with the home directory given.
pub(crate) fn argv_reaches_protected_from(
    argv: &[String],
    protected: &[PathBuf],
    cwd: &Path,
    home: Option<&Path>,
) -> Option<String> {
    for arg in argv {
        for part in arg
            .split([':', '=', ','])
            .flat_map(unglue_short_flag)
            .filter(|part| !part.is_empty())
        {
            for candidate in source_candidates(part, cwd, home) {
                if protected
                    .iter()
                    .any(|dir| names_or_contains(&candidate, dir))
                {
                    return Some(arg.clone());
                }
            }
        }
    }
    None
}

/// One fragment of an argument, quotes trimmed, as every value it can carry:
/// Docker reads `-v/Users/x/.kronn:/c` and `-v~/.kronn:/c` as the volume flag
/// followed by its value, and a group of short flags (`-itv/Users/x/.kronn:/c`,
/// `-dv~/.kronn:/c`) as each letter in turn, the last one taking the value. So
/// every suffix that starts after one of the leading flag letters is a possible
/// source. A long flag (`--volume`, `--mount`) is a name, not a source: its
/// value comes after the `=` or in the next argument.
fn unglue_short_flag(part: &str) -> Vec<&str> {
    fn unquote(text: &str) -> &str {
        text.trim().trim_matches(|c| c == '"' || c == '\'')
    }
    let part = unquote(part);
    if part.starts_with("--") {
        return Vec::new();
    }
    let Some(flag) = part.strip_prefix('-') else {
        return vec![part];
    };
    let letters = flag
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    // Letters are ASCII, so a byte offset is a char offset up to `letters`.
    let first = usize::from(letters > 0);
    (first..=letters)
        .filter(|at| flag.is_char_boundary(*at))
        .map(|at| unquote(&flag[at..]))
        .collect()
}

fn host_home() -> Option<PathBuf> {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .or_else(|| crate::core::child_env::var("HOME").ok().map(PathBuf::from))
}

/// The absolute forms a mount source can take: expanded, joined to `cwd` when
/// relative, then as written, lexically normalized and with symlinks resolved.
/// A bare word (an image, a flag value) is only a source when it names
/// something that exists under `cwd`.
fn source_candidates(part: &str, cwd: &Path, home: Option<&Path>) -> Vec<String> {
    let expanded = expand_shell_word(part, home);
    let path = Path::new(&expanded);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else if expanded.starts_with('.') || cwd.join(&expanded).symlink_metadata().is_ok() {
        cwd.join(&expanded)
    } else {
        return Vec::new();
    };
    let normalized = lexical_normalize(&path);
    let mut forms: Vec<String> = [
        path.clone(),
        normalized.clone(),
        real_path(&path),
        real_path(&normalized),
    ]
    .iter()
    .map(|form| {
        let text = form.to_string_lossy();
        let trimmed = text.trim_end_matches('/');
        if trimmed.is_empty() { "/" } else { trimmed }.to_string()
    })
    .collect();
    forms.sort();
    forms.dedup();
    forms
}

/// `~`, `~/x`, `$HOME`, `${HOME}` and any other `$VAR` / `${VAR}` the process
/// environment defines; a variable that is not defined is left as written.
fn expand_shell_word(word: &str, home: Option<&Path>) -> String {
    let lookup = |name: &str| -> Option<String> {
        if name == "HOME" {
            if let Some(home) = home {
                return Some(home.to_string_lossy().into_owned());
            }
        }
        crate::core::child_env::var(name).ok()
    };
    let word = match home {
        Some(home) if word == "~" => return home.to_string_lossy().into_owned(),
        Some(home) if word.starts_with("~/") => {
            format!("{}{}", home.to_string_lossy(), &word[1..])
        }
        _ => word.to_string(),
    };
    let mut out = String::new();
    let mut rest = word.as_str();
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let (name, after) = if let Some(braced) = tail.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (&braced[..end], &braced[end + 1..]),
                None => ("", tail),
            }
        } else {
            let end = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(tail.len());
            (&tail[..end], &tail[end..])
        };
        match name.chars().next() {
            Some(first) if !first.is_ascii_digit() => match lookup(name) {
                Some(value) => out.push_str(&value),
                None => {
                    out.push('$');
                    out.push_str(name);
                }
            },
            _ => out.push('$'),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// `.` and `..` folded out of a path without touching the disk.
fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `source` is `dir`, a parent of it (`/` included) or something inside it.
fn names_or_contains(source: &str, dir: &Path) -> bool {
    let dir_text = dir.to_string_lossy();
    source == "/"
        || dir_text == source
        || dir_text.starts_with(&format!("{source}/"))
        || source.starts_with(&format!("{dir_text}/"))
}

/// Whether the vault is protected from what a level grants, on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultProtection {
    /// Level-granted commands run under the sandbox.
    Active,
    /// The owner turned "Protect the Kronn vault from agents" off in the config.
    Disabled,
    /// The host has no sandbox it can apply; the reason says which.
    Unavailable(String),
}

impl VaultProtection {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }

    /// The sentence the panel, the level control and the acknowledgement use to
    /// name the missing protection; `None` when the vault is protected.
    pub fn missing(&self) -> Option<String> {
        match self {
            Self::Active => None,
            Self::Disabled => Some(
                "Kronn vault protection is turned off (protect_vault_from_agents = false in the config)"
                    .to_string(),
            ),
            Self::Unavailable(reason) => Some(format!(
                "Kronn vault protection is unavailable on this machine (no sandbox-exec or bubblewrap): {reason}"
            )),
        }
    }
}

/// The state of the vault protection on this host: the setting first, then
/// whether a sandbox can be applied.
pub fn vault_protection() -> VaultProtection {
    if !protect_vault_setting() {
        return VaultProtection::Disabled;
    }
    host_mechanism().map_or_else(VaultProtection::Unavailable, |_| VaultProtection::Active)
}

/// The sandbox of what a level grants, or `None` when the command must run
/// unconfined (the setting is off, or the host has no sandbox): a level never
/// refuses anything because the vault cannot be protected.
pub fn confinement() -> Option<LevelConfinement> {
    if !protect_vault_setting() {
        return None;
    }
    let mechanism = host_mechanism().ok()?;
    Some(LevelConfinement::with(mechanism, protected_dirs()))
}

/// `Ok` when this host can apply a sandbox, whatever the setting; otherwise why not.
pub fn host_sandbox() -> Result<(), String> {
    host_mechanism().map(|_| ())
}

fn host_mechanism() -> Result<Mechanism, String> {
    // Unit tests do not depend on the sandbox of the machine they run on
    // (a Seatbelt cannot be applied from inside another one): they choose.
    #[cfg(test)]
    {
        match test_unavailable() {
            Some(reason) => Err(reason),
            None => Ok(Mechanism::of_host().unwrap_or(Mechanism::Seatbelt)),
        }
    }
    #[cfg(not(test))]
    {
        Mechanism::usable_on_host()
    }
}

/// The confinement of a level-approved command on this host.
#[derive(Debug, Clone)]
pub struct LevelConfinement {
    mechanism: Mechanism,
    protected: Vec<PathBuf>,
}

impl LevelConfinement {
    pub fn with(mechanism: Mechanism, protected: Vec<PathBuf>) -> Self {
        Self {
            mechanism,
            protected,
        }
    }

    pub fn mechanism(&self) -> Mechanism {
        self.mechanism
    }

    pub fn protected(&self) -> &[PathBuf] {
        &self.protected
    }

    /// `program args` behind the sandbox launcher. Pure: nothing is started.
    pub fn wrap(&self, program: &str, args: &[String]) -> (String, Vec<String>) {
        let mut wrapped = match self.mechanism {
            Mechanism::Seatbelt => vec!["-p".to_string(), seatbelt_profile(&self.protected)],
            Mechanism::Bubblewrap => bubblewrap_args(&self.protected),
        };
        wrapped.push(program.to_string());
        wrapped.extend(args.iter().cloned());
        (self.mechanism.launcher().to_string(), wrapped)
    }
}

impl CommandConfinement for LevelConfinement {
    fn command(&self, probe: &ProbeCommand, route: ChildRoute) -> tokio::process::Command {
        let (program, args) = self.wrap(&probe.program, &probe.args);
        let mut command = crate::core::cmd::async_cmd(&program, route);
        command.args(&args).current_dir(&probe.cwd);
        for (name, value) in &probe.env {
            command.env(name, value);
        }
        crate::core::child_env::seal(command.as_std_mut(), route, &[]);
        // Its own session: no terminal, and a timeout takes the whole group.
        #[cfg(unix)]
        crate::core::task_worker_env::detach_from_terminal(command.as_std_mut());
        command
    }
}

#[cfg(test)]
static TEST_UNAVAILABLE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Tests pretend the host has no sandbox (`Some(reason)`), or restore it (`None`).
#[cfg(test)]
pub fn set_test_unavailable(reason: Option<&str>) {
    *TEST_UNAVAILABLE.lock().unwrap_or_else(|p| p.into_inner()) = reason.map(str::to_owned);
}

#[cfg(test)]
fn test_unavailable() -> Option<String> {
    TEST_UNAVAILABLE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

#[cfg(test)]
#[path = "trust_sandbox_tests.rs"]
mod trust_sandbox_tests;
