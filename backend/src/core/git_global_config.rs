//! Kronn and the user's GLOBAL git config (KT-1746).
//!
//! Inside Docker the mounted volume's owner differs from the container user,
//! so Kronn lists a repository and its worktree as `safe.directory` in the
//! container's global config. Outside Docker git already trusts the user's own
//! directories: writing there only grows `~/.gitconfig`, and every git process
//! of the machine reads that file (34 727 entries made `git rev-parse` ~4x
//! slower on the owner's Mac).
//!
//! Three rules live here:
//! - [`ensure_safe_directory`] is the ONLY writer: it does nothing outside
//!   Docker and never adds an entry that is already listed;
//! - the test harnesses call [`isolate_global_git_config_for_tests`] before any
//!   test runs, so no test reaches the real file, and [`ensure_safe_directory`]
//!   refuses to write in a test process that was not isolated;
//! - [`cleanup_interactive`] is the one-off, user-confirmed repair of a file
//!   the earlier versions already bloated (`kronn --clean-git-config`).

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::core::cmd::git_cmd;

/// Marker of a worktree Kronn created: `<repo>/.kronn/worktrees/<name>`.
const KRONN_WORKTREE_MARKER: &str = "/.kronn/worktrees/";

/// Whether `path` must be added to the global `safe.directory` list: only in
/// Docker, and only when neither the path nor the `*` wildcard is listed.
fn must_add(in_docker: bool, existing: &[String], path: &str) -> bool {
    in_docker && !existing.iter().any(|entry| entry == path || entry == "*")
}

/// The `safe.directory` values of `file` (the global config when `None`), or
/// `None` when git cannot read it. Includes are not followed: only the file's
/// own entries matter, and only those can be rewritten.
fn read_entries(file: Option<&Path>) -> Option<Vec<String>> {
    // lane-git-allow: the global (or a named) config file, never a worktree.
    let mut cmd = git_cmd();
    match file {
        Some(file) => cmd.arg("config").arg("--file").arg(file),
        None => cmd.args(["config", "--global"]),
    };
    let output = cmd
        .args(["--no-includes", "--get-all", "safe.directory"])
        .output()
        .ok()?;
    match output.status.code() {
        // 1: the key is not set at all.
        Some(1) => Some(Vec::new()),
        Some(0) => Some(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::to_string)
                .collect(),
        ),
        _ => None,
    }
}

/// Mark `path` as a safe directory in the global git config, Docker only.
///
/// Native runs return before touching git; a listed path is not added again.
/// A test process whose global config is not the scratch file of
/// [`isolate_global_git_config_for_tests`] never writes.
pub fn ensure_safe_directory(path: &Path) {
    if !crate::core::env::is_docker() {
        return;
    }
    if crate::core::cmd::real_agents_forbidden() && !global_git_config_is_isolated() {
        return;
    }
    let path = path.to_string_lossy();
    let existing = read_entries(None).unwrap_or_default();
    if !must_add(true, &existing, &path) {
        return;
    }
    // lane-git-allow: writes the container's global config, never a worktree.
    let _ = git_cmd()
        .args(["config", "--global", "--add", "safe.directory"])
        .arg(path.as_ref())
        .output();
}

/// [`ensure_safe_directory`] for async callers: git runs on a blocking thread.
pub async fn ensure_safe_directory_async(path: PathBuf) {
    let _ = tokio::task::spawn_blocking(move || ensure_safe_directory(&path)).await;
}

// ── Test isolation ──────────────────────────────────────────────────────────

/// Whether the git config of this process points at scratch files: a global
/// file under the temporary directory and an explicit system file. The guard
/// test of the harnesses asserts it, and [`ensure_safe_directory`] checks it
/// before writing in a test process.
pub fn global_git_config_is_isolated() -> bool {
    let global = crate::core::child_env::var_os("GIT_CONFIG_GLOBAL");
    let system = crate::core::child_env::var_os("GIT_CONFIG_SYSTEM");
    let under_temp = global
        .as_ref()
        .is_some_and(|global| Path::new(global).starts_with(std::env::temp_dir()));
    under_temp && system.is_some_and(|system| !system.is_empty())
}

/// The directory of this test process under which the scratch global config
/// gives another identity (`includeIf "gitdir:"`), and the file it includes.
/// The include file is written by the tests that need it: git ignores a missing
/// one, so every other repository keeps the scratch identity.
pub fn test_work_dir() -> (PathBuf, PathBuf) {
    let work = std::env::temp_dir().join(format!("kronn-test-work-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&work);
    let work = std::fs::canonicalize(&work).unwrap_or(work);
    let include = work.with_extension("inc");
    (work, include)
}

/// Point `GIT_CONFIG_GLOBAL` at a scratch file and `GIT_CONFIG_SYSTEM` at the
/// null device, for the whole process. Called by the test harnesses before the
/// first test (a constructor in the library's test build and in
/// `tests/it.rs`); the real `~/.gitconfig` is then never read nor written by a
/// test. The scratch file carries a commit identity, and a conditional include
/// (see [`test_work_dir`]) that only the tests of a directory-specific identity
/// make real.
pub fn isolate_global_git_config_for_tests() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kronn-test-gitconfig-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let scratch = dir.join("gitconfig");
    let (work, include) = test_work_dir();
    let _ = std::fs::write(&scratch, scratch_global_config(&work, &include));
    crate::core::child_env::set_var("GIT_CONFIG_GLOBAL", &scratch);
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    crate::core::child_env::set_var("GIT_CONFIG_SYSTEM", null);
    scratch
}

/// The scratch global config: a commit identity, and the include that
/// [`test_work_dir`] makes conditional.
fn scratch_global_config(work: &Path, include: &Path) -> String {
    format!(
        "[user]\n\tname = Kronn Test\n\temail = kronn-test@example.invalid\n\
         [includeIf \"gitdir:{}/\"]\n\tpath = {}\n",
        config_path(work),
        config_path(include)
    )
}

/// A path as a git config file can hold it: a backslash is an escape there (a
/// Windows path fails the whole file), and git matches `gitdir:` against a
/// path without the `\\?\` prefix `canonicalize` adds on Windows.
fn config_path(path: &Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\")
        .unwrap_or(&text)
        .replace('\\', "/")
}

// ── One-off cleanup ─────────────────────────────────────────────────────────

/// What a cleanup would keep and why it drops the rest.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanupPlan {
    pub total: usize,
    pub keep: Vec<String>,
    /// Entries under a temporary directory (test fixtures).
    pub temp_dirs: usize,
    /// Entries naming a Kronn worktree that no longer exists.
    pub stale_worktrees: usize,
    /// Second and later copies of an entry already kept.
    pub duplicates: usize,
}

impl CleanupPlan {
    pub fn removed(&self) -> usize {
        self.total - self.keep.len()
    }
}

/// Roots under which a directory is a throw-away one.
fn temp_roots() -> Vec<PathBuf> {
    let mut roots = vec![std::env::temp_dir()];
    if let Ok(canonical) = std::env::temp_dir().canonicalize() {
        roots.push(canonical);
    }
    for fixed in [
        "/tmp",
        "/private/tmp",
        "/var/folders",
        "/private/var/folders",
    ] {
        roots.push(PathBuf::from(fixed));
    }
    roots
}

/// Decide which `entries` to keep. Only Kronn's footprint goes: temp-dir
/// entries, worktrees of Kronn that are gone, and repeated copies. `*`, relative
/// paths and every other directory stay, in their original order.
pub fn plan_cleanup(
    entries: &[String],
    temp_roots: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
) -> CleanupPlan {
    let mut plan = CleanupPlan {
        total: entries.len(),
        ..CleanupPlan::default()
    };
    let mut seen = HashSet::new();
    for entry in entries {
        let path = Path::new(entry);
        if path.is_absolute() && temp_roots.iter().any(|root| path.starts_with(root)) {
            plan.temp_dirs += 1;
        } else if path.is_absolute()
            && entry.replace('\\', "/").contains(KRONN_WORKTREE_MARKER)
            && !exists(path)
        {
            plan.stale_worktrees += 1;
        } else if !seen.insert(entry.clone()) {
            plan.duplicates += 1;
        } else {
            plan.keep.push(entry.clone());
        }
    }
    plan
}

/// The file git writes `--global` to: `GIT_CONFIG_GLOBAL`, else `~/.gitconfig`,
/// else the XDG one when that is the only one that exists.
pub fn global_config_path() -> Option<PathBuf> {
    if let Some(explicit) = crate::core::child_env::var_os("GIT_CONFIG_GLOBAL") {
        return Some(PathBuf::from(explicit));
    }
    let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
    let classic = home.join(".gitconfig");
    if classic.exists() {
        return Some(classic);
    }
    let xdg = crate::core::child_env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("git/config");
    Some(if xdg.exists() { xdg } else { classic })
}

/// Copy `file` next to itself under a name no earlier backup has.
fn backup(file: &Path) -> std::io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = file.as_os_str().to_os_string();
    name.push(format!(".kronn-backup-{stamp}"));
    let target = PathBuf::from(name);
    if target.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", target.display()),
        ));
    }
    std::fs::copy(file, &target)?;
    Ok(target)
}

/// Rewrite the `safe.directory` entries of `file` to `keep`, after a backup
/// (returned). Git itself edits the file, so every other key, comment and
/// section stays as it was; on any failure the backup is restored.
pub fn apply_cleanup(file: &Path, keep: &[String]) -> Result<PathBuf, String> {
    let saved = backup(file).map_err(|e| format!("backup failed, nothing changed: {e}"))?;
    let rewrite = || -> Result<(), String> {
        // lane-git-allow: rewrites the named global config file, never a worktree.
        let unset = git_cmd()
            .arg("config")
            .arg("--file")
            .arg(file)
            .args(["--unset-all", "safe.directory"])
            .output()
            .map_err(|e| e.to_string())?;
        if !unset.status.success() {
            return Err(String::from_utf8_lossy(&unset.stderr).trim().to_string());
        }
        for entry in keep {
            // lane-git-allow: rewrites the named global config file, never a worktree.
            let add = git_cmd()
                .arg("config")
                .arg("--file")
                .arg(file)
                .args(["--add", "safe.directory"])
                .arg(entry)
                .output()
                .map_err(|e| e.to_string())?;
            if !add.status.success() {
                return Err(String::from_utf8_lossy(&add.stderr).trim().to_string());
            }
        }
        Ok(())
    };
    match rewrite() {
        Ok(()) => Ok(saved),
        Err(reason) => {
            let restored = std::fs::copy(&saved, file);
            Err(format!(
                "{reason}; the original was {} ({})",
                if restored.is_ok() {
                    "restored"
                } else {
                    "NOT restored, copy it back by hand"
                },
                saved.display()
            ))
        }
    }
}

/// Mean wall time, in ms, of `git rev-parse` over a few runs: what every git
/// call of the machine pays to read the global config.
fn rev_parse_ms() -> f64 {
    const RUNS: u32 = 5;
    let start = std::time::Instant::now();
    for _ in 0..RUNS {
        // lane-git-allow: times git's start-up in the backend's own directory.
        let _ = git_cmd().args(["rev-parse", "--git-dir"]).output();
    }
    start.elapsed().as_secs_f64() * 1000.0 / f64::from(RUNS)
}

/// `kronn --clean-git-config`: report what the cleanup would remove from the
/// global git config `file` ([`global_config_path`]), and apply it only after the user answers `y` or `yes`.
/// Prints the entry count and the `git rev-parse` time before and after.
pub fn cleanup_interactive(
    file: &Path,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    if !file.exists() {
        writeln!(out, "{} does not exist: nothing to clean.", file.display()).map_err(io)?;
        return Ok(());
    }
    let entries = read_entries(Some(file)).ok_or("git cannot read the global config")?;
    let plan = plan_cleanup(&entries, &temp_roots(), |p| p.exists());
    let before_ms = rev_parse_ms();
    writeln!(
        out,
        "{}: {} safe.directory entries, git rev-parse {:.0} ms.",
        file.display(),
        plan.total,
        before_ms
    )
    .map_err(io)?;
    writeln!(
        out,
        "Would remove {} ({} under a temporary directory, {} Kronn worktrees that no longer \
         exist, {} duplicates) and keep {}. No other key is touched.",
        plan.removed(),
        plan.temp_dirs,
        plan.stale_worktrees,
        plan.duplicates,
        plan.keep.len()
    )
    .map_err(io)?;
    if plan.removed() == 0 {
        return Ok(());
    }
    write!(out, "A backup is written next to the file. Apply? [y/N] ").map_err(io)?;
    out.flush().map_err(io)?;
    let mut answer = String::new();
    input.read_line(&mut answer).map_err(io)?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        writeln!(out, "Nothing changed.").map_err(io)?;
        return Ok(());
    }
    let saved = apply_cleanup(file, &plan.keep)?;
    let after = read_entries(Some(file)).map_or(0, |entries| entries.len());
    writeln!(
        out,
        "Backup: {}\nsafe.directory entries: {} -> {}; git rev-parse {:.0} ms -> {:.0} ms.",
        saved.display(),
        plan.total,
        after,
        before_ms,
        rev_parse_ms()
    )
    .map_err(io)?;
    Ok(())
}

/// The library's own test build isolates the git config before `main`, for
/// the same reason `tests/it.rs` does: every test of every module has it.
#[cfg(any(test, feature = "test-support"))]
#[used]
#[cfg_attr(
    any(target_os = "linux", target_os = "android"),
    link_section = ".init_array"
)]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(windows, link_section = ".CRT$XCU")]
static ISOLATE_THE_GIT_CONFIG: extern "C" fn() = {
    extern "C" fn isolate() {
        isolate_global_git_config_for_tests();
    }
    isolate
};

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn native_runs_never_add_an_entry() {
        assert!(!must_add(false, &[], "/home/u/repo"));
    }

    #[test]
    fn docker_adds_an_unlisted_path_only_once() {
        assert!(must_add(true, &list(&["/other"]), "/data/repo"));
        assert!(!must_add(true, &list(&["/data/repo"]), "/data/repo"));
        assert!(!must_add(true, &list(&["*"]), "/data/repo"));
    }

    #[test]
    fn the_scratch_config_with_windows_paths_is_one_git_can_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gitconfig");
        let text = scratch_global_config(
            Path::new(r"\\?\C:\Users\RUNNER~1\AppData\Local\Temp\kronn-test-work-7"),
            Path::new(r"C:\Users\RUNNER~1\AppData\Local\Temp\kronn-test-work-7.inc"),
        );
        std::fs::write(&file, text).unwrap();
        let out = git_cmd()
            .args(["config", "--file"])
            .arg(&file)
            .args(["--get-regexp", "^includeif\\."])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "includeif.gitdir:C:/Users/RUNNER~1/AppData/Local/Temp/kronn-test-work-7/.path \
             C:/Users/RUNNER~1/AppData/Local/Temp/kronn-test-work-7.inc"
        );
    }

    #[test]
    fn the_test_process_never_sees_the_real_global_config() {
        // The constructor of the harness ran before this test.
        assert!(
            global_git_config_is_isolated(),
            "GIT_CONFIG_GLOBAL/GIT_CONFIG_SYSTEM must point at scratch files in every test"
        );
        let global = crate::core::child_env::var_os("GIT_CONFIG_GLOBAL").unwrap();
        let home = directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();
        assert_ne!(PathBuf::from(global), home.join(".gitconfig"));
    }

    #[test]
    #[serial_test::serial]
    fn a_docker_run_writes_the_scratch_config_once_and_native_not_at_all() {
        let scratch = PathBuf::from(crate::core::child_env::var_os("GIT_CONFIG_GLOBAL").unwrap());
        let count = |needle: &str| {
            read_entries(Some(&scratch))
                .unwrap()
                .iter()
                .filter(|e| *e == needle)
                .count()
        };
        let old = crate::core::child_env::var("KRONN_IN_DOCKER").ok();

        crate::core::child_env::remove_var("KRONN_IN_DOCKER");
        if !Path::new("/.dockerenv").exists() && !Path::new("/run/.containerenv").exists() {
            ensure_safe_directory(Path::new("/kronn-kt1746/native"));
            assert_eq!(count("/kronn-kt1746/native"), 0, "native run wrote");
        }

        crate::core::child_env::set_var("KRONN_IN_DOCKER", "1");
        ensure_safe_directory(Path::new("/kronn-kt1746/docker"));
        ensure_safe_directory(Path::new("/kronn-kt1746/docker"));
        match old {
            Some(v) => crate::core::child_env::set_var("KRONN_IN_DOCKER", v),
            None => crate::core::child_env::remove_var("KRONN_IN_DOCKER"),
        }
        assert_eq!(
            count("/kronn-kt1746/docker"),
            1,
            "added twice or not at all"
        );
    }

    #[test]
    fn plan_drops_temp_dirs_stale_worktrees_and_duplicates_only() {
        let temp = [PathBuf::from("/var/folders")];
        let entries = list(&[
            "/home/u/repo",
            "/var/folders/ab/T/.tmpXYZ",
            "/home/u/repo",
            "*",
            "/home/u/repo/.kronn/worktrees/gone",
            "/home/u/repo/.kronn/worktrees/alive",
            "relative/dir",
            "/home/u/other",
            "*",
        ]);
        let plan = plan_cleanup(&entries, &temp, |p| p.ends_with("alive"));
        assert_eq!(plan.temp_dirs, 1);
        assert_eq!(plan.stale_worktrees, 1);
        assert_eq!(plan.duplicates, 2);
        assert_eq!(plan.removed(), 4);
        assert_eq!(
            plan.keep,
            list(&[
                "/home/u/repo",
                "*",
                "/home/u/repo/.kronn/worktrees/alive",
                "relative/dir",
                "/home/u/other"
            ])
        );
    }

    #[test]
    fn cleanup_backs_up_asks_first_and_touches_no_other_key() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gitconfig");
        let mut text = String::from("[user]\n\tname = Someone\n[core]\n\teditor = vi\n[safe]\n");
        for i in 0..50 {
            text.push_str("\tdirectory = /var/folders/zz/T/.tmp");
            text.push_str(&i.to_string());
            text.push('\n');
        }
        text.push_str("\tdirectory = /home/u/repo\n\tdirectory = /home/u/repo\n");
        std::fs::write(&file, &text).unwrap();

        // Declined: nothing changes, no backup.
        let mut out = Vec::new();
        cleanup_interactive(&file, &mut "n\n".as_bytes(), &mut out).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("52 safe.directory"));

        // Confirmed.
        let mut out = Vec::new();
        cleanup_interactive(&file, &mut "yes\n".as_bytes(), &mut out).unwrap();
        let report = String::from_utf8(out).unwrap();
        assert!(report.contains("52 -> 1"), "{report}");
        assert_eq!(read_entries(Some(&file)).unwrap(), list(&["/home/u/repo"]));
        let after = std::fs::read_to_string(&file).unwrap();
        assert!(after.contains("name = Someone") && after.contains("editor = vi"));
        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".kronn-backup-"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read_to_string(backups[0].path()).unwrap(), text);
    }
}
