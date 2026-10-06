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

/// A `tokio::process::Command` for `program` with the environment built for
/// `route` (KT-1006): nothing of the backend's environment beyond the route's
/// allow-list. Values the caller adds afterwards are its launch's own; a
/// caller adding any must seal again (`child_env::seal`).
///
/// Accepts anything `Command::new` accepts (`&str`, `String`, `&Path`, …).
pub fn async_cmd<S: AsRef<OsStr>>(program: S, route: ChildRoute) -> tokio::process::Command {
    let mut cmd = raw_async(program.as_ref());
    crate::core::child_env::isolate(cmd.as_std_mut(), route);
    cmd
}

/// [`async_cmd`] for a `std::process::Command`.
pub fn sync_cmd<S: AsRef<OsStr>>(program: S, route: ChildRoute) -> std::process::Command {
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

    /// A repository hook runs inside git's process: it never sees a variable
    /// outside the git allow-list (synthetic sentinels, no real secret).
    #[cfg(unix)]
    #[test]
    fn a_git_hook_runs_without_the_backend_environment() {
        use std::os::unix::fs::PermissionsExt;
        let repo = tempfile::tempdir().unwrap();
        // Also in the real process environment: without the policy, git would
        // inherit it from there. A name nothing else in the suite reads.
        std::env::set_var("KRONN_HOOK_SENTINEL_API_KEY", "sentinel-real-env");
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
        std::env::remove_var("KRONN_HOOK_SENTINEL_API_KEY");
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
            // var, var_os, vars_os, take_process_environment_where: the live reads
            ("backend/src/core/child_env.rs", 4),
            // test builds only: fixtures
            ("backend/src/lib.rs", 1),
            ("backend/tests/api_tests.rs", 1),
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
                    assert!(!rustflags_lower_lints(&text), "{}", file.display());
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
        // rustc reads `-` in a lint name as `_`.
        let text = text.replace('-', "_");
        let mut table = String::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap().trim();
            if line.starts_with('[') {
                table = line.trim_matches(|c| c == '[' || c == ']').to_string();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let full = format!("{table}.{}", key.trim().trim_matches('"'));
            if !(full.starts_with("lints") || full.starts_with("workspace.lints")) {
                continue;
            }
            let lowered = value.contains("allow") || value.contains("warn");
            let names: Vec<&str> = full.split('.').collect();
            let lint = names.last().unwrap().trim_matches('"');
            let touches = [
                "disallowed_methods",
                "disallowed_method",
                "style",
                "all",
                "warnings",
            ]
            .contains(&lint)
                || value.contains("disallowed_method");
            if lowered && touches {
                return true;
            }
        }
        false
    }

    /// Rustflags allowing one of the lints, or capping every lint.
    fn rustflags_lower_lints(text: &str) -> bool {
        let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.contains("--cap-lints") {
            return true;
        }
        // rustc reads `-` in a lint name as `_` (flags themselves keep theirs).
        let compact = compact
            .replace("--allow", "\u{1}")
            .replace("-A", "\u{2}")
            .replace("-W", "\u{3}")
            .replace('-', "_")
            .replace('\u{1}', "--allow")
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
            ]
            .iter()
            .any(|form| compact.contains(form.as_str()))
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
        ] {
            assert!(rustflags_lower_lints(config), "{config}");
        }
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
                discovery_cmd("claude", AgentFamily::Claude),
                discovery_cmd("codex", AgentFamily::Codex),
                discovery_cmd("npx", AgentFamily::Other),
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
