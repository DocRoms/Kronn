//! Cross-platform command helpers.
//!
//! On Windows, every `Command::new()` spawns a visible console window by default.
//! These helpers apply the `CREATE_NO_WINDOW` flag so background processes (git, wsl.exe, etc.)
//! run invisibly — critical for the Tauri desktop app experience.
//!
//! On Windows, they also resolve bare program names (`"npx"`, `"git"`, `"node"`)
//! to their fully-qualified path (`npx.cmd`, `git.exe`, `node.exe`) via `which`.
//! Without this, `Command::new("npx")` fails with "program not found" because
//! Win32 `CreateProcess` refuses to execute `.cmd`/`.bat` wrappers when called
//! by their bare name — only `.exe` works without an extension. Reported by a
//! Windows user (npm-installed Node.js, `npx` accessible in PowerShell, but
//! Kronn raised "Spawn failed for npx: program not found").

use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::path::PathBuf;

/// Windows: CREATE_NO_WINDOW flag prevents a console window from appearing.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// On Windows, resolve a bare program name to its full path so `.cmd`/`.bat`
/// wrappers (npx, npm, yarn, pnpm…) can be spawned. No-op for paths that
/// already point at a real file (absolute, contains `\` or `/`, or has an
/// extension). Returns `None` when `which` can't find the binary — caller
/// falls back to the original input so the existing "program not found"
/// error path still surfaces.
#[cfg(target_os = "windows")]
fn resolve_windows_program(program: &OsStr) -> Option<PathBuf> {
    let s = program.to_str()?;
    // Already an explicit path or has an extension — let CreateProcess handle it.
    if s.contains('\\') || s.contains('/') || s.contains('.') {
        return None;
    }
    which::which(s).ok()
}

pub use crate::core::child_env::ChildRoute;

/// The only place a process is constructed (clippy's `disallowed_methods`
/// refuses `Command::new` elsewhere). No environment is applied yet.
#[allow(clippy::disallowed_methods)]
fn raw_async(program: &OsStr) -> tokio::process::Command {
    #[cfg(target_os = "windows")]
    let mut cmd = match resolve_windows_program(program) {
        Some(path) => tokio::process::Command::new(path),
        None => tokio::process::Command::new(program),
    };
    #[cfg(not(target_os = "windows"))]
    let cmd = tokio::process::Command::new(program);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[allow(clippy::disallowed_methods)]
fn raw_sync(program: &OsStr) -> std::process::Command {
    #[cfg(target_os = "windows")]
    let mut cmd = match resolve_windows_program(program) {
        Some(path) => std::process::Command::new(path),
        None => std::process::Command::new(program),
    };
    #[cfg(not(target_os = "windows"))]
    let cmd = std::process::Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Environment flag that arms the real-agent guard in a build that is not the
/// library's own test build — the integration crates in `backend/tests` link the
/// library without `cfg(test)`. Set (to anything but `0`) by `tests/it.rs`
/// before any test runs; never set by Kronn itself (KT-1189).
///
/// Only read when the `test-support` cargo feature is compiled in, which only
/// the test build of the integration crates does: a shipped `kronn` ignores
/// it (see [`ignored_test_flag`]).
pub const NO_REAL_AGENTS_ENV: &str = "KRONN_TEST_NO_REAL_AGENTS";

/// Whether `flag` (the value of [`NO_REAL_AGENTS_ENV`]) arms the guard, given
/// whether the `test-support` feature is compiled in (`gate`). The gate is a
/// parameter so a test can prove a build without it ignores a set flag.
fn forbidden_by(flag: Option<&OsStr>, gate: bool) -> bool {
    gate && flag.is_some_and(|value| !value.is_empty() && value != "0")
}

/// Whether a test is running that must not start a real agent CLI: the
/// library's own test build, or an integration test binary (feature
/// `test-support`) carrying [`NO_REAL_AGENTS_ENV`]. `false` in every
/// production process, whatever its environment holds.
pub fn real_agents_forbidden() -> bool {
    cfg!(test)
        || forbidden_by(
            crate::core::child_env::var_os(NO_REAL_AGENTS_ENV).as_deref(),
            cfg!(feature = "test-support"),
        )
}

/// `true` when [`NO_REAL_AGENTS_ENV`] is set in a build that ignores it: the
/// startup of the binary warns about it once, so a user who exported it learns
/// it does nothing instead of wondering why.
pub fn ignored_test_flag() -> bool {
    !cfg!(feature = "test-support")
        && !cfg!(test)
        && crate::core::child_env::var_os(NO_REAL_AGENTS_ENV).is_some()
}

/// Whether `path` is a program a test controls: a stub written under the
/// temporary directory or a fixture of this crate. Anything else (a CLI the
/// developer installed under `~/.local/bin`, Homebrew, npm…) is the machine's,
/// not the test's. The detection consults it when [`real_agents_forbidden`] so
/// a test sees the same agents on every machine (KT-1189).
pub(crate) fn is_test_controlled(path: &std::path::Path) -> bool {
    let real = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let path = real(path);
    [
        std::env::temp_dir(),
        std::path::PathBuf::from("/tmp"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    ]
    .iter()
    .any(|root| path.starts_with(real(root)))
}

/// When [`real_agents_forbidden`]: no unit or API test may start a real agent
/// CLI (KT-1189).
///
/// A CLI installed on the developer's machine would make a test depend on that
/// machine (green here, red on CI), start slowly, reach the network, and write
/// into whatever `HOME` the process points at — the real one when a test does
/// not redirect it. Every agent launch and probe goes through [`async_cmd`] /
/// [`sync_cmd`], so this is the one place that sees them all.
pub(crate) mod real_agent_guard {
    use super::{is_test_controlled, ChildRoute};
    use crate::core::child_env::AgentFamily;
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};

    /// Programs refused although their name is not an agent family's.
    const EXTRA_AGENT_BINARIES: &[&str] = &["litellm"];

    /// Plain shells: with an explicit `-c` script they are the stand-in a test
    /// gives an agent route (a loop that never exits, an `exit 3`), never an
    /// agent CLI themselves.
    const STAND_IN_SHELLS: &[&str] = &["sh", "bash", "zsh", "dash"];

    /// Real executions refused in this process, for the guard's own test.
    static REFUSED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    #[cfg(test)]
    pub(crate) fn refused() -> Vec<String> {
        REFUSED.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// The file `program` would run as, when it exists: a path as is, a bare
    /// name looked up on `search_path`.
    fn resolve(program: &OsStr, search_path: Option<OsString>) -> Option<PathBuf> {
        let as_path = Path::new(program);
        if as_path.components().count() > 1 {
            return as_path.is_file().then(|| as_path.to_path_buf());
        }
        let cwd = std::env::current_dir().ok()?;
        which::which_in(program, search_path, cwd).ok()
    }

    /// Whether a program called `name` is an agent CLI.
    pub(crate) fn is_agent_name(name: &str) -> bool {
        let stem = Path::new(name)
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or(name);
        AgentFamily::from_launch(stem, None, "") != AgentFamily::Other
            || EXTRA_AGENT_BINARIES.contains(&stem)
    }

    /// Whether `path`, found for `name`, is an agent CLI of the machine rather
    /// than a stub the test put there: detection must not see it.
    pub(crate) fn is_machine_agent(name: &str, path: &str) -> bool {
        is_agent_name(name) && !is_test_controlled(Path::new(path))
    }

    /// The real binary `program` would start on `route`, if it is an agent
    /// launch: an agent route (a probe through `npx`, a Vibe runner started as
    /// `python3`) or a program named like an agent CLI. `None` when it is not
    /// an agent, does not exist, or is a stub the test controls.
    pub(crate) fn real_agent_binary(
        program: &OsStr,
        route: ChildRoute,
        search_path: Option<OsString>,
    ) -> Option<PathBuf> {
        let name = Path::new(program)
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or_default();
        if STAND_IN_SHELLS.contains(&name) {
            return None;
        }
        let named_like_agent = is_agent_name(name);
        let agent_route =
            matches!(route, ChildRoute::Agent(family) if family != AgentFamily::Other);
        if !named_like_agent && !agent_route {
            return None;
        }
        resolve(program, search_path).filter(|path| !is_test_controlled(path))
    }

    /// Panics when the launch would start a real agent binary. Does nothing
    /// outside a test that forbids it ([`real_agents_forbidden`]).
    ///
    /// A panic inside a spawned tokio task is swallowed until the task is
    /// joined, and a detached refresh is never joined, so there the process is
    /// aborted instead: the test fails loudly rather than passing with a real
    /// CLI started.
    pub(crate) fn refuse_real_agent(program: &OsStr, route: ChildRoute) {
        if !super::real_agents_forbidden() {
            return;
        }
        let Some(real) = real_agent_binary(program, route, crate::core::child_env::var_os("PATH"))
        else {
            return;
        };
        let message = format!(
            "a test started the real agent binary {} (program {:?}, route {:?}, thread {:?}): \
             put a stub on PATH or inject the detection instead (KT-1189)",
            real.display(),
            program,
            route,
            std::thread::current().name(),
        );
        if let Ok(mut refused) = REFUSED.lock() {
            refused.push(message.clone());
        }
        if tokio::task::try_id().is_some() {
            eprintln!("{message}");
            std::process::abort();
        }
        panic!("{message}");
    }
}

/// A `tokio::process::Command` for `program` with the environment built for
/// `route` (KT-1006): nothing of the backend's environment beyond the route's
/// allow-list. Values the caller adds afterwards are its launch's own; a
/// caller adding any must seal again (`child_env::seal`).
///
/// Accepts anything `Command::new` accepts (`&str`, `String`, `&Path`, …).
pub fn async_cmd<S: AsRef<OsStr>>(program: S, route: ChildRoute) -> tokio::process::Command {
    real_agent_guard::refuse_real_agent(program.as_ref(), route);
    let mut cmd = raw_async(program.as_ref());
    crate::core::child_env::isolate(cmd.as_std_mut(), route);
    cmd
}

/// [`async_cmd`] for a `std::process::Command`.
pub fn sync_cmd<S: AsRef<OsStr>>(program: S, route: ChildRoute) -> std::process::Command {
    real_agent_guard::refuse_real_agent(program.as_ref(), route);
    let mut cmd = raw_sync(program.as_ref());
    crate::core::child_env::isolate(&mut cmd, route);
    cmd
}

/// `git`, whose repository hooks, filters and drivers run inside it.
pub fn git_cmd() -> std::process::Command {
    sync_cmd("git", ChildRoute::Git)
}

/// [`git_cmd`] for tokio.
pub fn async_git_cmd() -> tokio::process::Command {
    async_cmd("git", ChildRoute::Git)
}

/// A program Kronn runs for itself (installer, system probe): the base
/// allow-list only.
pub fn tool_cmd<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
    async_cmd(program, ChildRoute::Tool)
}

/// [`tool_cmd`] for a `std::process::Command`.
pub fn sync_tool_cmd<S: AsRef<OsStr>>(program: S) -> std::process::Command {
    sync_cmd(program, ChildRoute::Tool)
}

/// A CLI asked for its version or its models: its agent's route, so it
/// reads its own login (home, XDG and its config directories), minus every
/// credential it would otherwise inherit (provider keys, GitHub variables,
/// secret-looking names): a probe is not a configured launch. It starts in
/// the temporary directory, never in a repository.
pub fn discovery_cmd<S: AsRef<OsStr>>(
    program: S,
    family: crate::core::child_env::AgentFamily,
) -> tokio::process::Command {
    let mut cmd = async_cmd(program, ChildRoute::Agent(family));
    crate::core::child_env::drop_credentials(cmd.as_std_mut());
    cmd.current_dir(std::env::temp_dir());
    cmd
}

/// Why a process inherits the backend's environment instead of a route's:
/// the only declared exception of the design note (§9). The test
/// `full_env_cmd_sites_are_exactly_the_declared_exceptions` lists its call
/// sites in both crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullEnvReason {
    /// The desktop app relaunching itself: it keeps its own environment but
    /// the forbidden names (the caller hands the key override back).
    SelfRestart,
}

/// A process that inherits the backend's environment minus the forbidden
/// names, for a declared [`FullEnvReason`] only.
pub fn full_env_sync_cmd<S: AsRef<OsStr>>(
    program: S,
    reason: FullEnvReason,
) -> std::process::Command {
    let FullEnvReason::SelfRestart = reason;
    let mut cmd = raw_sync(program.as_ref());
    // Kronn relaunched gets back what the desktop withheld.
    cmd.envs(crate::core::child_env::withheld_variables());
    for name in crate::core::child_env::FORBIDDEN {
        cmd.env_remove(name);
    }
    cmd
}

/// The system opener (`open`, `xdg-open`, `start`) for `target`, with the
/// base allow-list only. The `open` crate's own spawning entry points are
/// refused by clippy; its commands are taken here and rebuilt.
#[allow(clippy::disallowed_methods)] // open::commands: each command is isolated below
pub fn open_in_system<T: AsRef<OsStr>>(target: T) -> std::io::Result<()> {
    let mut last = Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no system opener",
    ));
    for command in open::commands(target) {
        let mut command = rebuild_for_tool(command);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        last = match command.status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => Err(std::io::Error::other(format!(
                "opener exited with {status}"
            ))),
            Err(error) => Err(error),
        };
    }
    last
}

/// The opener command `open::commands` built, isolated with the Tool route.
fn rebuild_for_tool(mut command: std::process::Command) -> std::process::Command {
    crate::core::child_env::isolate(&mut command, ChildRoute::Tool);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_cmd_creates_command() {
        let cmd = tool_cmd("echo");
        // Just verify it doesn't panic — creation_flags is Windows-only
        drop(cmd);
    }

    /// KT-1189: `KRONN_TEST_NO_REAL_AGENTS` only arms the guard when the
    /// `test-support` feature is compiled in. A build without it (every build a
    /// user runs) ignores the variable, so a user who exported it keeps every
    /// installed agent and no launch aborts the backend. Reverting the `gate &&`
    /// in `forbidden_by` fails the first assertion.
    #[test]
    fn the_flag_is_ignored_by_a_build_without_the_test_support_feature() {
        let set = Some(OsStr::new("1"));
        // Production build: flag set, gate off.
        assert!(!forbidden_by(set, false));
        assert!(!forbidden_by(Some(OsStr::new("anything")), false));
        // Test-support build: flag set arms it, unset / empty / "0" does not.
        assert!(forbidden_by(set, true));
        assert!(!forbidden_by(None, true));
        assert!(!forbidden_by(Some(OsStr::new("")), true));
        assert!(!forbidden_by(Some(OsStr::new("0")), true));
    }

    /// KT-1189: a program resolved outside the directories a test controls is
    /// the machine's. `/bin/ls` stands for "a CLI the developer installed":
    /// present on every Unix host, never under the temporary directory.
    #[cfg(unix)]
    #[test]
    fn a_machine_program_on_an_agent_route_is_a_real_agent() {
        use crate::core::child_env::AgentFamily;
        use real_agent_guard::{is_machine_agent, real_agent_binary};
        let sh = std::ffi::OsStr::new("/bin/ls");
        // A route that belongs to an agent makes any program an agent launch…
        assert!(real_agent_binary(sh, ChildRoute::Agent(AgentFamily::Claude), None).is_some());
        // …except a plain shell, the stand-in tests give an agent route…
        let shell = std::ffi::OsStr::new("/bin/sh");
        assert!(real_agent_binary(shell, ChildRoute::Agent(AgentFamily::Copilot), None).is_none());
        // …a Tool or Git route does not…
        assert!(real_agent_binary(sh, ChildRoute::Tool, None).is_none());
        assert!(real_agent_binary(sh, ChildRoute::Git, None).is_none());
        // …and neither does an agent route nothing is known about.
        assert!(real_agent_binary(sh, ChildRoute::Agent(AgentFamily::Other), None).is_none());
        // By name: a CLI of the machine is hidden from detection, a stub is not.
        assert!(is_machine_agent("claude", "/bin/ls"));
        assert!(is_machine_agent("vibe", "/bin/ls"));
        assert!(!is_machine_agent("git", "/bin/ls"));
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("claude");
        std::fs::write(&stub, "#!/bin/sh\n").unwrap();
        assert!(!is_machine_agent("claude", stub.to_str().unwrap()));
    }

    /// KT-1189: the guard itself. Launching a real agent binary from a test
    /// panics with the offending path, whichever builder is used.
    #[cfg(unix)]
    #[test]
    fn a_test_cannot_start_a_real_agent_binary() {
        use crate::core::child_env::AgentFamily;
        let before = real_agent_guard::refused().len();
        let route = ChildRoute::Agent(AgentFamily::Copilot);
        let refused_async = std::panic::catch_unwind(|| {
            let _ = async_cmd("/bin/ls", route);
        });
        let refused_sync = std::panic::catch_unwind(|| {
            let _ = sync_cmd("/bin/ls", route);
        });
        let refused_discovery = std::panic::catch_unwind(|| {
            let _ = discovery_cmd("/bin/ls", AgentFamily::Vibe);
        });
        assert!(refused_async.is_err() && refused_sync.is_err() && refused_discovery.is_err());
        let refused = real_agent_guard::refused();
        assert!(refused.len() >= before + 3, "each refusal is recorded");
        assert!(refused
            .iter()
            .any(|m| m.contains("/bin/ls") && m.contains("KT-1189")));
    }

    /// KT-1189: a stub the test wrote under the temporary directory is a
    /// program the test controls: it starts, on an agent route too.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_stub_agent_in_the_temporary_directory_starts() {
        use crate::core::child_env::AgentFamily;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("claude");
        std::fs::write(&stub, "#!/bin/sh\necho stub 1.0\n").unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = discovery_cmd(&stub, AgentFamily::Claude)
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "stub 1.0");
    }

    /// A repository hook runs inside git's process: it never sees a variable
    /// outside the git allow-list (synthetic sentinels, no real secret).
    #[cfg(unix)]
    #[test]
    fn a_git_hook_runs_without_the_backend_environment() {
        use std::os::unix::fs::PermissionsExt;
        let repo = tempfile::tempdir().unwrap();
        // Also in the real process environment: without the policy, git would
        // inherit it from there. A name nothing else in the suite reads.
        crate::core::child_env::set_var("KRONN_HOOK_SENTINEL_API_KEY", "sentinel-real-env");
        let git = |args: &[&str]| {
            let output = crate::core::child_env::with_parent_env(
                &[
                    ("PATH", "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin"),
                    ("HOME", repo.path().to_str().unwrap()),
                    ("GIT_AUTHOR_NAME", "Fixture"),
                    ("GIT_AUTHOR_EMAIL", "fixture@example.invalid"),
                    ("GIT_COMMITTER_NAME", "Fixture"),
                    ("GIT_COMMITTER_EMAIL", "fixture@example.invalid"),
                    ("KRONN_AUTH_TOKEN", "sentinel-admin"),
                    ("KRONN_ENCRYPTION_KEK", "sentinel-kek"),
                    ("ANTHROPIC_API_KEY", "sentinel-provider"),
                    ("GH_TOKEN", "sentinel-github"),
                    ("GIT_DIR", "/sentinel/elsewhere"),
                ],
                || git_cmd().args(args).current_dir(repo.path()).output(),
            )
            .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        let hook = repo.path().join(".git/hooks/pre-commit");
        std::fs::write(
            &hook,
            "#!/bin/sh\nfor v in KRONN_HOOK_SENTINEL_API_KEY KRONN_AUTH_TOKEN KRONN_ENCRYPTION_KEK ANTHROPIC_API_KEY GH_TOKEN; do\n  eval \"val=\\${$v-}\"\n  if [ -n \"$val\" ]; then echo \"$v leaked\" >&2; exit 1; fi\ndone\n",
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(repo.path().join("f.txt"), "x").unwrap();
        git(&["add", "f.txt"]);
        git(&["commit", "-q", "-m", "fixture"]);
        crate::core::child_env::remove_var("KRONN_HOOK_SENTINEL_API_KEY");
    }

    /// A program Kronn runs for itself gets the base allow-list only.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_tool_command_runs_without_the_backend_environment() {
        use crate::core::child_env::probe;
        probe::plant_real_sentinel();
        let dir = tempfile::tempdir().unwrap();
        let out_async = probe::env_dumping_program(dir.path(), "kronn-tool");
        let path = format!("{}:/usr/bin:/bin", dir.path().display());
        let home = dir.path().to_str().unwrap();
        let (mut async_command, mut sync_command) = probe::with_secret_parent(&path, home, || {
            (tool_cmd("kronn-tool"), sync_tool_cmd("kronn-tool"))
        });
        probe::assert_built_without_secrets(async_command.as_std(), &path, &[]);
        probe::assert_built_without_secrets(&sync_command, &path, &[]);
        assert!(async_command.status().await.unwrap().success());
        let recorded = probe::read_dump(&out_async);
        probe::assert_dump_without_secrets(&recorded, &[]);
        assert_eq!(recorded.get("HOME").map(String::as_str), Some(home));
        std::fs::remove_file(&out_async).unwrap();
        assert!(sync_command.status().unwrap().success());
        probe::assert_dump_without_secrets(&probe::read_dump(&out_async), &[]);
    }

    /// The system opener is started with the Tool environment, every
    /// candidate the platform has (B3-11).
    #[test]
    fn the_system_opener_gets_no_backend_secret() {
        use crate::core::child_env::probe;
        let commands = probe::with_secret_parent("/usr/bin", "/home/u", || {
            open::commands("https://example.invalid")
                .into_iter()
                .map(rebuild_for_tool)
                .collect::<Vec<_>>()
        });
        assert!(!commands.is_empty());
        for command in &commands {
            probe::assert_built_without_secrets(command, "/usr/bin", &[]);
        }
    }

    /// Every Rust file of the backend (sources and integration tests) and of
    /// the desktop crate, as (`crate/relative/path`, text).
    fn rust_sources() -> Vec<(String, String)> {
        let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let roots = [
            ("backend/src", backend.join("src")),
            ("backend/tests", backend.join("tests")),
            ("desktop/src", backend.join("../desktop/src-tauri/src")),
        ];
        let mut files = Vec::new();
        for (label, root) in roots {
            assert!(root.is_dir(), "{} is missing", root.display());
            let mut stack = vec![root.clone()];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.extension().is_some_and(|ext| ext == "rs") {
                        let rel = path.strip_prefix(&root).unwrap().to_string_lossy();
                        files.push((
                            format!("{label}/{}", rel.replace('\\', "/")),
                            std::fs::read_to_string(&path).unwrap(),
                        ));
                    }
                }
            }
        }
        files
    }

    /// The one declared exception (Kronn relaunching itself) is the only
    /// process that inherits the environment, in either crate.
    #[test]
    fn full_env_cmd_sites_are_exactly_the_declared_exceptions() {
        let mut found: std::collections::BTreeMap<(String, String), usize> = Default::default();
        for (rel, text) in rust_sources() {
            if rel == "backend/src/core/cmd.rs" {
                continue;
            }
            for name in ["full_env_cmd", "full_env_sync_cmd"] {
                for (at, _) in text.match_indices(name) {
                    let after = &text[at + name.len()..];
                    if after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                        continue; // a longer identifier
                    }
                    assert!(after.starts_with('('), "{rel}: {name} used as a value");
                    let reason_at = after
                        .find("FullEnvReason::")
                        .unwrap_or_else(|| panic!("{rel}: {name} without a FullEnvReason"));
                    let reason: String = after[reason_at + "FullEnvReason::".len()..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    *found.entry((rel.clone(), reason)).or_default() += 1;
                }
            }
        }
        let expected = std::collections::BTreeMap::from([(
            ("desktop/src/main.rs".to_string(), "SelfRestart".to_string()),
            1,
        )]);
        assert_eq!(found, expected);
    }

    /// The places allowed to bypass clippy's spawn ban are exactly these.
    #[test]
    fn clippy_spawn_ban_bypasses_are_exactly_these() {
        let found: std::collections::BTreeMap<String, usize> = rust_sources()
            .into_iter()
            .map(|(rel, text)| {
                let count = source_bypasses(&text);
                (rel, count)
            })
            .filter(|(_, n)| *n > 0)
            .collect();
        let expected: std::collections::BTreeMap<String, usize> = [
            // raw_async, raw_sync, open_in_system (open::commands, isolated)
            ("backend/src/core/cmd.rs", 3),
            // var, var_os, vars_os, set_var, remove_var,
            // take_process_environment_where: the live reads and writes
            ("backend/src/core/child_env.rs", 6),
            // test builds only: fixtures
            ("backend/src/lib.rs", 1),
            ("backend/tests/api_tests.rs", 1),
            // macOS test only: the helper process and the ps/pgrep/lsof probes
            ("backend/tests/power_guard_orphan.rs", 1),
            // test only: re-runs a state-owning test alone in a child of the same binary
            ("backend/tests/support/own_process.rs", 1),
        ]
        .into_iter()
        .map(|(file, n)| (file.to_string(), n))
        .collect();
        assert_eq!(found, expected);

        // Crate-wide overrides: manifests and Cargo configurations.
        let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for manifest in [
            backend.join("Cargo.toml"),
            backend.join("../desktop/src-tauri/Cargo.toml"),
        ] {
            let text = std::fs::read_to_string(&manifest).unwrap();
            for manifest in [
                "[lints]\nclippy = { all = \"allow\" }\n",
                "[ lints.clippy ]\nall = \"allow\"\n",
                "lints.clippy.all = \"allow\"\n[package]\nname = \"x\"\n",
                "[lints.clippy]\nall = { level = \"allow\", priority = -1 }\n",
                "[lints.clippy]\nstyle = \"force-warn\"\n",
            ] {
                assert!(manifest_lowers_lints(manifest), "{manifest}");
            }
            assert!(!manifest_lowers_lints(&text), "{}", manifest.display());
        }
        // Cargo reads `.cargo/config` from every parent of the crate.
        for dir in [
            backend.join(".."),
            backend.to_path_buf(),
            backend.join("../desktop"),
            backend.join("../desktop/src-tauri"),
        ] {
            for name in ["config.toml", "config"] {
                let file = dir.join(".cargo").join(name);
                if let Ok(text) = std::fs::read_to_string(&file) {
                    assert!(!config_lowers_lints(&text), "{}", file.display());
                }
            }
        }
    }

    /// Lints whose `allow` would silence the spawn ban: the lint, its old
    /// name, the groups holding it, and every warning.
    const SPAWN_BAN_LINTS: &[&str] = &[
        concat!("clippy::", "disallowed_methods"),
        concat!("clippy::", "disallowed_method"),
        concat!("clippy::", "style"),
        concat!("clippy::", "all"),
        "warnings",
    ];

    /// `allow`/`expect` lists (plain, inner, combined or under `cfg_attr`)
    /// naming one of [`SPAWN_BAN_LINTS`].
    fn source_bypasses(text: &str) -> usize {
        let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let mut count = 0;
        for opener in [concat!("allow", "("), concat!("expect", "(")] {
            for (start, _) in compact.match_indices(opener) {
                let rest = &compact[start + opener.len()..];
                let mut depth = 1usize;
                let end = rest
                    .char_indices()
                    .find(|(_, c)| {
                        match c {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            _ => {}
                        }
                        depth == 0
                    })
                    .map_or(rest.len(), |(index, _)| index);
                if rest[..end]
                    .split(',')
                    .any(|item| SPAWN_BAN_LINTS.contains(&item))
                {
                    count += 1;
                }
            }
        }
        count
    }

    /// A `[lints]` / `[workspace.lints]` table lowering one of the lints.
    fn manifest_lowers_lints(text: &str) -> bool {
        // Parsed like Cargo does; a file that does not parse is refused.
        let Ok(manifest) = toml::from_str::<toml::Table>(text) else {
            return true;
        };
        let mut tables = Vec::new();
        if let Some(lints) = manifest.get("lints") {
            tables.push(lints);
        }
        if let Some(lints) = manifest.get("workspace").and_then(|w| w.get("lints")) {
            tables.push(lints);
        }
        tables.into_iter().any(|lints| {
            lints.as_table().is_some_and(|tools| {
                tools.iter().any(|(tool, entries)| {
                    entries.as_table().is_some_and(|entries| {
                        entries.iter().any(|(lint, level)| {
                            let lint = lint.replace('-', "_");
                            let name = if tool == "rust" {
                                lint
                            } else {
                                format!("{tool}::{lint}")
                            };
                            let level = level
                                .as_str()
                                .or_else(|| level.get("level").and_then(toml::Value::as_str))
                                .unwrap_or_default()
                                .replace('-', "_");
                            SPAWN_BAN_LINTS.contains(&name.as_str())
                                && matches!(level.as_str(), "allow" | "warn" | "force_warn")
                        })
                    })
                })
            })
        })
    }

    /// Rustflags allowing one of the lints, or capping every lint.
    fn rustflags_lower_lints(text: &str) -> bool {
        let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.contains("--cap-lints") {
            return true;
        }
        // rustc reads `-` in a lint name as `_` (flags themselves keep theirs).
        let compact = compact
            .replace("--force-warn", "\u{4}")
            .replace("--allow", "\u{1}")
            .replace("--warn", "\u{5}")
            .replace("-A", "\u{2}")
            .replace("-W", "\u{3}")
            .replace('-', "_")
            .replace('\u{4}', "--force-warn")
            .replace('\u{1}', "--allow")
            .replace('\u{5}', "--warn")
            .replace('\u{2}', "-A")
            .replace('\u{3}', "-W");
        if compact.contains("__cap_lints") {
            return true;
        }
        SPAWN_BAN_LINTS.iter().any(|lint| {
            [
                format!("-A{lint}"),
                format!("\"-A\",\"{lint}\""),
                format!("--allow={lint}"),
                format!("--allow{lint}"),
                format!("\"--allow\",\"{lint}\""),
                format!("-W{lint}"),
                format!("--warn={lint}"),
                format!("--warn{lint}"),
                format!("\"--warn\",\"{lint}\""),
                format!("--force-warn={lint}"),
                format!("--force-warn{lint}"),
                format!("\"--force-warn\",\"{lint}\""),
            ]
            .iter()
            .any(|form| compact.contains(form.as_str()))
        })
    }

    /// A Cargo configuration that lowers the ban: rustflags, an alias that
    /// shadows `cargo clippy`, an `[env]` value clippy-driver reads, or a rustc
    /// wrapper that can run rustc instead of clippy-driver.
    fn config_lowers_lints(text: &str) -> bool {
        if rustflags_lower_lints(text) {
            return true;
        }
        // Parsed like Cargo does; a file that does not parse is refused.
        let Ok(config) = toml::from_str::<toml::Table>(text) else {
            return true;
        };
        let flags_lower = |flags: &toml::Value| match flags {
            toml::Value::String(flags) => rustflags_lower_lints(flags),
            toml::Value::Array(items) => {
                let joined: Vec<String> = items
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(|item| format!("\"{item}\""))
                    .collect();
                rustflags_lower_lints(&format!("[{}]", joined.join(",")))
                    || rustflags_lower_lints(&joined.join(" ").replace('"', ""))
            }
            _ => false,
        };
        if config
            .get("alias")
            .and_then(toml::Value::as_table)
            .is_some_and(|aliases| aliases.contains_key("clippy"))
        {
            return true;
        }
        if config
            .get("env")
            .and_then(toml::Value::as_table)
            .is_some_and(|env| {
                env.keys()
                    .any(|key| key.to_ascii_uppercase().starts_with("CLIPPY_"))
            })
        {
            return true;
        }
        if let Some(build) = config.get("build").and_then(toml::Value::as_table) {
            if build.contains_key("rustc-wrapper") || build.contains_key("rustc-workspace-wrapper")
            {
                return true;
            }
            if build.get("rustflags").is_some_and(flags_lower) {
                return true;
            }
        }
        config
            .get("target")
            .and_then(toml::Value::as_table)
            .is_some_and(|targets| {
                targets
                    .values()
                    .any(|target| target.get("rustflags").is_some_and(flags_lower))
            })
    }

    /// Every way to lower the spawn ban is caught (B6-05).
    #[test]
    fn each_form_of_spawn_ban_bypass_is_caught() {
        // Built at run time so this file's own text holds no bypass.
        let (allow, expect) = (concat!("allow", "("), concat!("expect", "("));
        for source in [
            format!("#[{allow}warnings)] fn f() {{}}"),
            format!("#![{allow}unused, warnings)]"),
            format!("#[{allow}renamed_and_removed_lints, clippy::disallowed_method)] fn f() {{}}"),
            format!("#[cfg_attr(test, {expect}clippy::style))] fn f() {{}}"),
            format!("#[{allow} clippy :: all )] fn f() {{}}"),
        ] {
            assert_eq!(source_bypasses(&source), 1, "{source}");
        }
        assert_eq!(
            source_bypasses(&format!("#[{allow}dead_code)] fn f() {{}}")),
            0
        );
        for manifest in [
            "[lints.clippy]\nstyle = \"allow\"\n",
            "[lints.clippy]\ndisallowed_methods = { level = \"allow\", priority = 1 }\n",
            "[lints.rust]\nwarnings = \"allow\"\n",
            "[workspace.lints.clippy]\nall = \"warn\"\n",
            "[lints]\nclippy.disallowed_methods = \"allow\"\n",
            "[lints.clippy]\ndisallowed-methods = \"allow\"\n",
        ] {
            assert!(manifest_lowers_lints(manifest), "{manifest}");
        }
        assert!(!manifest_lowers_lints(
            "[lints.clippy]\npedantic = \"warn\"\n[dependencies]\nstyle = \"1\"\n"
        ));
        for config in [
            "[build]\nrustflags = [\"-A\", \"clippy::disallowed_methods\"]\n",
            "[build]\nrustflags = [\"-Awarnings\"]\n",
            "[target.x86_64-unknown-linux-gnu]\nrustflags = [\"--cap-lints\", \"allow\"]\n",
            "[build]\nrustflags = [\"--allow=clippy::all\"]\n",
            "[build]\nrustflags = [\"-Aclippy::disallowed-methods\"]\n",
            "[build]\nrustflags = \"--allow clippy::all\"\n",
            "[build]\nrustflags = [\"--allow=clippy::disallowed-methods\"]\n",
            "[build]\nrustflags = [\"--force-warn\", \"clippy::disallowed-methods\"]\n",
            "[build]\nrustflags = \"--force-warn=clippy::all\"\n",
            "[build]\nrustflags = [\"--warn\", \"clippy::style\"]\n",
            "[alias]\nclippy = \"check\"\n",
            "[alias]\nclippy = [\"check\", \"--all-targets\"]\n",
            "[env]\nCLIPPY_CONF_DIR = \"x\"\n",
            "[env]\nCLIPPY_ARGS = { value = \"\", force = true }\n",
        ] {
            assert!(config_lowers_lints(config), "{config}");
        }
        for config in [
            "alias.clippy = \"check\"\n",
            "alias = { clippy = \"check\" }\n",
            "[alias]\n'clippy' = \"check\"\n",
            "env.CLIPPY_CONF_DIR = \"x\"\n",
            "env = { CLIPPY_ARGS = \"\" }\n",
            "[env]\nclippy_conf_dir = \"x\"\n",
            "[build]\nrustc-wrapper = \"w\"\n",
            "build.rustc-workspace-wrapper = \"w\"\n",
            "[target.'cfg(unix)']\nrustflags = [\"-A\", \"clippy::all\"]\n",
            "[build\n",
        ] {
            assert!(config_lowers_lints(config), "{config}");
        }
        assert!(!config_lowers_lints(
            "[alias]\nc = \"check\"\n[env]\nRUST_LOG = \"info\"\n"
        ));
        assert!(!rustflags_lower_lints("[build]\ntarget-dir = \"target\"\n"));
    }

    /// Both clippy files ban the same entry points, every way Tauri can
    /// relaunch the app included (B5-05).
    #[test]
    fn both_clippy_files_ban_the_same_spawn_entry_points() {
        let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let paths = |file: std::path::PathBuf| -> std::collections::BTreeSet<String> {
            std::fs::read_to_string(&file)
                .unwrap()
                .split("path = \"")
                .skip(1)
                .map(|rest| rest.split('"').next().unwrap().to_string())
                .collect()
        };
        // Clippy reads `.clippy.toml` before `clippy.toml` in each directory:
        // one there would replace the banned list unseen.
        for dir in [
            backend.to_path_buf(),
            backend.join(".."),
            backend.join("../desktop"),
            backend.join("../desktop/src-tauri"),
        ] {
            assert!(
                !dir.join(".clippy.toml").exists(),
                "{} would override clippy.toml",
                dir.join(".clippy.toml").display()
            );
        }
        let backend_paths = paths(backend.join("clippy.toml"));
        let desktop_paths = paths(backend.join("../desktop/src-tauri/clippy.toml"));
        assert_eq!(backend_paths, desktop_paths);
        let required = [
            "std::process::Command::new",
            "tokio::process::Command::new",
            "open::that",
            "open::that_detached",
            "open::that_in_background",
            "open::with",
            "open::with_detached",
            "open::with_in_background",
            "open::commands",
            "open::with_command",
            "tauri_plugin_shell::Shell::command",
            "tauri_plugin_shell::Shell::sidecar",
            "tauri_plugin_shell::Shell::open",
            "tauri::AppHandle::restart",
            "tauri::AppHandle::request_restart",
            "tauri::process::restart",
            "libc::fork",
            "libc::vfork",
            "libc::forkpty",
            "libc::rfork",
            "libc::clone",
            "libc::syscall",
            "libc::system",
            "libc::popen",
            "libc::execv",
            "libc::execve",
            "libc::execvp",
            "libc::execvpe",
            "libc::execveat",
            "libc::execvP",
            "libc::execl",
            "libc::execle",
            "libc::execlp",
            "libc::fexecve",
            "libc::posix_spawn",
            "libc::posix_spawnp",
            "libc::execlpe",
            "libc::wexecl",
            "libc::wexecle",
            "libc::wexeclp",
            "libc::wexeclpe",
            "libc::wexecv",
            "libc::wexecve",
            "libc::wexecvp",
            "libc::wexecvpe",
            "libc::exect",
            "libc::pdfork",
            "libc::daemon",
            "tauri_plugin_shell::open::open",
            "std::env::var",
            "std::env::var_os",
            "std::env::vars",
            "std::env::vars_os",
            "std::env::set_var",
            "std::env::remove_var",
            "libc::getenv",
            "libc::secure_getenv",
            "libc::_NSGetEnviron",
            "libc::setenv",
            "libc::unsetenv",
            "libc::putenv",
            "libc::clearenv",
        ];
        for entry in required {
            assert!(desktop_paths.contains(entry), "{entry} is not banned");
        }
    }

    /// A parent holding credentials under names no deny-list knows.
    const UNLISTED_CREDENTIALS: &[(&str, &str)] = &[
        ("PATH", "/usr/bin"),
        ("HOME", "/home/u"),
        ("MYSQL_PWD", "sentinel-mysql"),
        ("DATABASE_URL", "postgres://u:p@h/d"),
        ("DEPLOY_PASSPHRASE", "sentinel-passphrase"),
        ("SENTRY_DSN", "https://key@sentry.example/1"),
        ("ANTHROPIC_API_KEY", "sentinel-anthropic"),
        ("CLAUDE_CODE_OAUTH_TOKEN", "sentinel-oauth"),
        ("OPENAI_API_KEY", "sentinel-openai"),
        ("AWS_SECRET_ACCESS_KEY", "sentinel-aws"),
        ("KRONN_ENCRYPTION_KEK", "sentinel-kek"),
        ("CLAUDE_CONFIG_DIR", "/home/u/.claude"),
        ("CODEX_HOME", "/home/u/.codex"),
        ("XDG_CONFIG_HOME", "/home/u/.config"),
        ("KRONN_DOCS_LOG_LEVEL", "debug"),
        ("PYTHONPATH", "/opt/docs"),
    ];

    fn set_names(command: &std::process::Command) -> Vec<String> {
        command
            .get_envs()
            .filter(|(_, value)| value.is_some())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect()
    }

    /// Discovery probes read their CLI's own login (config directories,
    /// XDG) and nothing that is a credential, under any name (B5-08).
    #[test]
    fn discovery_probes_keep_their_login_and_no_credential() {
        use crate::core::child_env::{with_parent_env, AgentFamily};
        let (claude, codex, other) = with_parent_env(UNLISTED_CREDENTIALS, || {
            (
                // Only the built command is inspected, never run: programs
                // that exist nowhere, so no real CLI of the machine is
                // involved (KT-1189).
                discovery_cmd("kronn-absent-claude", AgentFamily::Claude),
                discovery_cmd("kronn-absent-codex", AgentFamily::Codex),
                discovery_cmd("kronn-absent-npx", AgentFamily::Other),
            )
        });
        let allowed = |family: &str| -> Vec<&str> {
            match family {
                "claude" => vec!["PATH", "HOME", "XDG_CONFIG_HOME", "CLAUDE_CONFIG_DIR"],
                "codex" => vec!["PATH", "HOME", "XDG_CONFIG_HOME", "CODEX_HOME"],
                _ => vec!["PATH", "HOME", "XDG_CONFIG_HOME"],
            }
        };
        for (family, command) in [
            ("claude", claude.as_std()),
            ("codex", codex.as_std()),
            ("other", other.as_std()),
        ] {
            let mut got = set_names(command);
            got.sort();
            let mut want = allowed(family);
            want.sort();
            assert_eq!(got, want, "{family}");
            assert_eq!(
                command.get_current_dir(),
                Some(std::env::temp_dir().as_path())
            );
        }
    }

    /// The relaunched desktop is Kronn itself: its own environment, never
    /// the forbidden names.
    #[test]
    fn the_self_restart_keeps_kronn_but_no_forbidden_name() {
        let restart = full_env_sync_cmd("kronn", FullEnvReason::SelfRestart);
        for name in crate::core::child_env::FORBIDDEN {
            assert!(restart
                .get_envs()
                .any(|(key, value)| key == *name && value.is_none()));
        }
    }

    #[test]
    fn sync_cmd_creates_command() {
        let cmd = sync_tool_cmd("echo");
        drop(cmd);
    }

    #[tokio::test]
    async fn async_cmd_runs_successfully() {
        let output = tool_cmd("echo")
            .arg("hello")
            .output()
            .await
            .expect("echo should succeed");
        assert!(output.status.success());
    }

    #[test]
    fn sync_cmd_runs_successfully() {
        let output = sync_tool_cmd("echo")
            .arg("hello")
            .output()
            .expect("echo should succeed");
        assert!(output.status.success());
    }

    /// Verify that on Windows we skip resolution for paths that are already
    /// explicit (so we don't double-resolve `C:\Program Files\nodejs\npx.cmd`
    /// or break callers that pass a `PathBuf`). This is a unit test for the
    /// guard logic only — it runs on Windows (the function is `cfg`-gated
    /// out elsewhere) and exercises the early-return cases.
    #[cfg(target_os = "windows")]
    #[test]
    fn resolve_windows_program_skips_explicit_paths() {
        use std::ffi::OsString;
        // Absolute path → skip
        let abs = OsString::from(r"C:\Program Files\nodejs\npx.cmd");
        assert!(
            resolve_windows_program(&abs).is_none(),
            "absolute path with backslash + extension must skip resolution"
        );
        // Relative path containing slash → skip
        let rel = OsString::from("./bin/foo");
        assert!(
            resolve_windows_program(&rel).is_none(),
            "path containing slash must skip resolution"
        );
        // Bare extension → skip (Windows can resolve .exe natively)
        let with_ext = OsString::from("npx.cmd");
        // We skip on extension presence — caller passes the explicit form.
        assert!(
            resolve_windows_program(&with_ext).is_none(),
            "name with extension must skip resolution"
        );
    }
}
