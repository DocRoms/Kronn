//! KT-1498 — which commits of a delivery are already upstream.
//!
//! A worker that merges `origin/main` into its branch brings in commits other
//! people wrote; the delivery identity check must not judge them as the
//! worker's. "Already upstream" cannot be read from a ref the worker can write:
//! the `refs/remotes/*` and `refs/heads/*` of a linked worktree live in the
//! shared repository, and a snapshot taken at launch is poisoned by a sibling
//! worker or stale as soon as the remote moves.
//!
//! It is decided here, at delivery time, from the remote itself: the remote is
//! the project's `repo_url` in Kronn's database (never the `origin` of the
//! checkout, whose config every worktree shares), its default branch is fetched
//! into a private bare mirror under the Kronn data directory (which the vault
//! sandbox hides from every agent process) and only the commits reachable from
//! the fetched tips are left out. No local ref is an exemption. A fetch that
//! fails, or a project without `repo_url`, leaves the caller with no tip: every
//! commit is then inspected (fail closed).
//!
//! No scan runs inside the worker's repository either: the `shallow` file, the
//! grafts, the replace refs and the config of a shared repository are all
//! writable by a worker. Every scan runs in a git directory of Kronn's own, the
//! worker's repository only lending its objects.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Longest a fetch from the remote may take before the delivery gives up.
const FETCH_TIMEOUT: Duration = Duration::from_secs(90);

/// One fetch at a time: concurrent fetches into the same mirror would fight
/// over its ref locks and fail closed for no reason.
static FETCH_LOCK: Mutex<()> = Mutex::new(());

/// The mirror of a project's remote `url` under `data_dir`. The URL is part of
/// the name: a project whose `origin` changes starts a fresh mirror instead of
/// mixing two remotes in one.
pub fn mirror_dir(data_dir: &Path, project_id: &str, url: &str) -> PathBuf {
    let name: String = project_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let digest = crate::core::skill_bundle::sha256_hex(url);
    data_dir
        .join("upstream-mirrors")
        .join(format!("{name}-{}.git", &digest[..16]))
}

/// Fetch the remote's default branch (and each of `branches` that the remote
/// has: the integration target, which may be a PR base other than the default)
/// into the mirror at `mirror`, creating it on first use. Returns the fetched
/// tips; an error when the default branch could not be fetched, never a tip
/// left over from an earlier fetch.
pub fn refresh(mirror: &Path, url: &str, branches: &[String]) -> Result<Vec<String>, String> {
    if url.starts_with('-') || url.starts_with("ext::") {
        return Err(format!("refusing remote url `{url}`"));
    }
    let _serial = FETCH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_bare(mirror)?;

    let mut tips = Vec::new();
    // The remote's own idea of its default branch: no ref name to guess.
    fetch_into(mirror, url, "+HEAD:refs/upstream/default")?;
    tips.push(tip_of(mirror, "refs/upstream/default")?);
    for branch in branches {
        if !plain_branch_name(branch) {
            continue;
        }
        let refspec = format!("+refs/heads/{branch}:refs/upstream/heads/{branch}");
        // A target that only exists locally is not on the remote: not an error.
        if fetch_into(mirror, url, &refspec).is_ok() {
            if let Ok(tip) = tip_of(mirror, &format!("refs/upstream/heads/{branch}")) {
                tips.push(tip);
            }
        }
    }
    tips.sort();
    tips.dedup();
    Ok(tips)
}

/// Create the bare repository `dir` unless it exists. Callers hold `FETCH_LOCK`.
fn ensure_bare(dir: &Path) -> Result<(), String> {
    if dir.join("HEAD").is_file() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create the upstream mirror directory: {error}"))?;
    }
    let init = crate::core::cmd::git_cmd()
        .args(["init", "--bare", "--quiet"])
        .arg(dir)
        .output()
        .map_err(|error| format!("git init failed for the upstream mirror: {error}"))?;
    if !init.status.success() {
        return Err(format!(
            "cannot create the upstream mirror: {}",
            String::from_utf8_lossy(&init.stderr).trim()
        ));
    }
    Ok(())
}

/// The private repository every identity scan can run in, under `data_dir`. It
/// holds no ref and is never fetched into: it only gives the scan a git
/// directory of its own (no `shallow`, grafts, replace refs or config written
/// by a worker), the worker's repository lending its objects.
pub fn scan_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("upstream-mirrors").join("scan.git")
}

/// `(sha, full message)` of every commit of `base..head`, read from the
/// worker's objects through the private repository `scan`, whatever the worker
/// wrote into the shared `shallow` file, replace refs or grafts. A history the
/// worker's objects cannot complete (parents missing, as in a shallow clone)
/// is an error: the caller fails closed.
pub fn commit_messages_private(
    worktree: &Path,
    scan: &Path,
    base: &str,
    head: &str,
) -> Result<Vec<(String, String)>, String> {
    {
        let _serial = FETCH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ensure_bare(scan)?;
    }
    commit_messages_outside(worktree, scan, base, head, &[])
}

fn plain_branch_name(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with('-')
        && !branch.starts_with('/')
        && !branch.ends_with('/')
        && !branch.ends_with(".lock")
        && !branch.contains("..")
        && !branch.contains("//")
        && !branch
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\@{".contains(c))
}

fn fetch_into(mirror: &Path, url: &str, refspec: &str) -> Result<(), String> {
    let mut command = crate::core::cmd::git_cmd();
    command
        .arg("--git-dir")
        .arg(mirror)
        .args([
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=30",
            "fetch",
            "--quiet",
            "--no-tags",
            url,
            refspec,
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("git fetch failed to start: {error}"))?;
    let deadline = Instant::now() + FETCH_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "fetching the remote timed out after {}s",
                    FETCH_TIMEOUT.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => return Err(format!("git fetch failed: {error}")),
        }
    };
    if status.success() {
        return Ok(());
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
    }
    Err(format!("git fetch failed: {}", stderr.trim()))
}

fn tip_of(mirror: &Path, reference: &str) -> Result<String, String> {
    let output = crate::core::cmd::git_cmd()
        .arg("--git-dir")
        .arg(mirror)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{commit}}"),
        ])
        .output()
        .map_err(|error| format!("git rev-parse failed in the upstream mirror: {error}"))?;
    let tip = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !tip.is_empty() {
        Ok(tip)
    } else {
        Err(format!("the upstream mirror has no `{reference}`"))
    }
}

/// `(sha, full message)` of the commits of `base..head` that are not reachable
/// from any of `excluded`. The log runs against the mirror's own refs and
/// config, the worker's repository only lending its objects, so nothing the
/// worker can write steers it. Replacement refs are never read and grafts are
/// switched off.
pub fn commit_messages_outside(
    worktree: &Path,
    mirror: &Path,
    base: &str,
    head: &str,
    excluded: &[String],
) -> Result<Vec<(String, String)>, String> {
    use crate::core::worktree::reject_option_like_rev;
    reject_option_like_rev(base)?;
    reject_option_like_rev(head)?;
    for sha in excluded {
        reject_option_like_rev(sha)?;
    }
    let range = format!("{base}..{head}");
    let mut command = crate::core::cmd::git_cmd();
    command
        .arg("--git-dir")
        .arg(mirror)
        .current_dir(mirror)
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", objects_dir(worktree)?)
        .env(
            crate::core::worktree::NO_GRAFTS_ENV.0,
            crate::core::worktree::NO_GRAFTS_ENV.1,
        );
    command.args(["--no-replace-objects", "log", "-z", "--format=%H%n%B"]);
    command.arg(&range);
    if !excluded.is_empty() {
        command.arg("--not").args(excluded);
    }
    command.arg("--");
    let output = command
        .output()
        .map_err(|error| format!("git log failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git log failed ({}): {}",
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(crate::core::worktree::parse_commit_log(&output.stdout))
}

/// The object store a linked worktree shares with its repository.
fn objects_dir(worktree: &Path) -> Result<PathBuf, String> {
    let output = crate::core::cmd::git_cmd()
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "objects",
        ])
        .current_dir(worktree)
        .output()
        .map_err(|error| format!("cannot locate the object store: {error}"))?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || path.is_empty() {
        return Err("cannot locate the object store of the worktree".into());
    }
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        // No global or system git config: a CI runner has none, a developer machine does.
        let output = crate::core::cmd::git_cmd()
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str, message: &str) -> String {
        std::fs::write(dir.join(file), message).unwrap();
        git(dir, &["add", file]);
        git(
            dir,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@t.com",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
        git(dir, &["rev-parse", "HEAD"])
    }

    /// A work repository with one commit on `main` and a bare "remote" cloned
    /// from it, `origin` of the work repository.
    fn repo_with_remote() -> (tempfile::TempDir, tempfile::TempDir) {
        let work = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        git(work.path(), &["init", "-q", "-b", "main"]);
        // The identity lives in the repository: no test depends on a global one.
        git(work.path(), &["config", "user.name", "T"]);
        git(work.path(), &["config", "user.email", "t@t.com"]);
        commit(work.path(), "a.txt", "init");
        git(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
        git(
            work.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        git(work.path(), &["push", "-q", "origin", "main"]);
        (work, remote)
    }

    #[test]
    fn mirror_names_are_confined_to_the_mirror_directory() {
        let data = Path::new("/data");
        let dir = mirror_dir(data, "../../etc/passwd", "u");
        assert_eq!(dir.parent(), Some(Path::new("/data/upstream-mirrors")));
        let name = dir.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("______etc_passwd-") && name.ends_with(".git"),
            "{name}"
        );
        assert_ne!(
            mirror_dir(data, "p", "url-a"),
            mirror_dir(data, "p", "url-b")
        );
        assert_eq!(
            mirror_dir(data, "p", "url-a"),
            mirror_dir(data, "p", "url-a")
        );
    }

    #[test]
    fn the_default_branch_of_the_remote_is_fetched_into_the_mirror() {
        let (work, remote) = repo_with_remote();
        let data = tempfile::tempdir().unwrap();
        let mirror = mirror_dir(data.path(), "p", "u");
        let first = refresh(&mirror, remote.path().to_str().unwrap(), &[]).unwrap();
        assert_eq!(first, vec![git(work.path(), &["rev-parse", "HEAD"])]);

        // The remote moves: the next fetch follows it, nothing is kept stale.
        let next = commit(work.path(), "b.txt", "next");
        git(work.path(), &["push", "-q", "origin", "main"]);
        let second = refresh(&mirror, remote.path().to_str().unwrap(), &[]).unwrap();
        assert_eq!(second, vec![next]);
    }

    #[test]
    fn a_remote_branch_is_fetched_when_the_remote_has_it_and_skipped_when_not() {
        let (work, remote) = repo_with_remote();
        git(work.path(), &["checkout", "-q", "-b", "release"]);
        let release = commit(work.path(), "r.txt", "release");
        git(work.path(), &["push", "-q", "origin", "release"]);
        let data = tempfile::tempdir().unwrap();
        let mirror = mirror_dir(data.path(), "p", "u");

        let tips = refresh(
            &mirror,
            remote.path().to_str().unwrap(),
            &["release".into(), "local-only".into(), "-x".into()],
        )
        .unwrap();
        assert_eq!(tips.len(), 2, "{tips:?}");
        assert!(tips.contains(&release), "{tips:?}");
    }

    /// An unreachable remote gives no tip, even when the mirror already holds
    /// an earlier fetch: the caller then inspects every commit.
    #[test]
    fn an_unreachable_remote_gives_no_tip_even_after_an_earlier_fetch() {
        let (_work, remote) = repo_with_remote();
        let data = tempfile::tempdir().unwrap();
        let mirror = mirror_dir(data.path(), "p", "u");
        refresh(&mirror, remote.path().to_str().unwrap(), &[]).unwrap();

        let gone = data.path().join("no-such-remote.git");
        assert!(refresh(&mirror, gone.to_str().unwrap(), &[]).is_err());
        assert!(refresh(&mirror, "-oProxyCommand=x", &[]).is_err());
        assert!(refresh(&mirror, "ext::sh -c touch% x", &[]).is_err());
    }

    /// The private scan ignores the `shallow` file of the worker's repository,
    /// and fails closed when the worker's objects cannot complete the history.
    #[test]
    fn the_private_scan_ignores_a_shallow_file_and_fails_closed_on_missing_parents() {
        let (work, _remote) = repo_with_remote();
        let base = git(work.path(), &["rev-parse", "HEAD"]);
        let foreign = commit(
            work.path(),
            "f.txt",
            "fix: foreign\n\nSigned-off-by: X <x@x.io>",
        );
        let tip = commit(work.path(), "c.txt", "chore: clean");
        let data = tempfile::tempdir().unwrap();
        let scan = scan_dir(data.path());

        std::fs::write(work.path().join(".git").join("shallow"), format!("{tip}\n")).unwrap();
        let range = format!("{base}..{tip}");
        assert_eq!(git(work.path(), &["log", "--format=%H", &range]), tip);
        let shas: Vec<String> = commit_messages_private(work.path(), &scan, &base, &tip)
            .unwrap()
            .into_iter()
            .map(|(sha, _)| sha)
            .collect();
        assert_eq!(shas, vec![tip.clone(), foreign], "{shas:?}");

        // A real shallow clone lacks the parents: the scan errors, never "no commit".
        let clone = tempfile::tempdir().unwrap();
        git(
            clone.path(),
            &[
                "clone",
                "-q",
                "--depth",
                "1",
                &format!("file://{}", work.path().display()),
                ".",
            ],
        );
        assert!(commit_messages_private(clone.path(), &scan, &base, &tip).is_err());
    }

    /// The log runs on the mirror's refs, with the worker's objects: the commits
    /// below the fetched tip are left out; a ref the worker moves changes nothing.
    #[test]
    fn commits_reachable_from_the_fetched_tip_are_left_out() {
        let (work, remote) = repo_with_remote();
        let base = git(work.path(), &["rev-parse", "HEAD"]);
        let upstream = commit(
            work.path(),
            "u.txt",
            "fix: upstream\n\nSigned-off-by: Someone Else <else@example.com>",
        );
        git(work.path(), &["push", "-q", "origin", "main"]);
        git(work.path(), &["checkout", "-q", "-b", "task", &base]);
        let own = commit(work.path(), "w.txt", "feat: worker");
        git(
            work.path(),
            &["merge", "-q", "--no-ff", "-m", "merge main", &upstream],
        );

        let head = git(work.path(), &["rev-parse", "HEAD"]);
        let data = tempfile::tempdir().unwrap();
        let mirror = mirror_dir(data.path(), "p", "u");
        let tips = refresh(&mirror, remote.path().to_str().unwrap(), &[]).unwrap();

        let shas = |excluded: &[String]| -> Vec<String> {
            commit_messages_outside(work.path(), &mirror, &base, &head, excluded)
                .unwrap()
                .into_iter()
                .map(|(sha, _)| sha)
                .collect()
        };
        assert!(shas(&[]).contains(&upstream));
        let outside = shas(&tips);
        assert!(!outside.contains(&upstream), "{outside:?}");
        assert!(outside.contains(&own), "{outside:?}");

        // The worker names its own commit `origin/main`: the mirror ignores it.
        git(
            work.path(),
            &["update-ref", "refs/remotes/origin/main", &own],
        );
        git(work.path(), &["update-ref", "refs/heads/main", &own]);
        let still = shas(&tips);
        assert!(still.contains(&own), "{still:?}");

        assert!(
            commit_messages_outside(work.path(), &mirror, &base, &head, &["-x".into()]).is_err()
        );
    }
}
