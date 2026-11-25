//! Quick Exec — KT-195.
//!
//! Runs a deterministic command and returns a BOUNDED result, so a mechanical
//! operation (a test run, a typecheck, a lint) no longer needs an agent pass
//! whose only job is to read a full output and restate it. The full streams are
//! kept as an artifact on disk; only the canonical summary enters a context.
//!
//! Symmetric with Quick Prompts / Quick APIs: a named, reusable, argument-taking
//! primitive. The difference is that this one spawns a process, so the surface it
//! exposes is a security boundary — hence the allowlist, the literal argv, and
//! the bounded cwd below.

use crate::core::cmd::{async_cmd_with, Priority};
use crate::core::task_worker_env::ProbeCommand;
use crate::core::task_worker_sandbox::CommandConfinement;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

/// Cap on the summary that enters a context. The whole point of Quick Exec is
/// that this number is small; if it ever needs raising, the primitive has stopped
/// paying for itself and the summariser is what should change.
pub const SUMMARY_MAX_BYTES: usize = 4_096;

/// Compile-time ceiling. A test can be edited to match a regression; this cannot
/// be satisfied by editing the assertion alone.
const _: () = assert!(
    SUMMARY_MAX_BYTES <= 8_192,
    "a Quick Exec summary above 8 KiB costs more context than the agent pass it replaces"
);

/// Per-stream artifact cap. Beyond it the stream is still DRAINED (so the child
/// never blocks on a full pipe) but not kept.
pub const ARTIFACT_MAX_BYTES: usize = 1_048_576;

pub const MAX_FAILED_TESTS: usize = 50;
pub const MAX_DIAGNOSTICS: usize = 50;
pub const MAX_ARGV: usize = 64;
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;
pub const MAX_TIMEOUT_SECS: u64 = 1_800;

/// How long to wait for the stream readers after the child is gone. A grandchild
/// holding the pipe open must not hang the caller.
const STREAM_DRAIN_GRACE_SECS: u64 = 5;

/// Binaries Quick Exec may spawn, by exact name.
///
/// An allowlist rather than a denylist: the set of harmful commands is open, the
/// set of useful ones is not. `echo`, `true`, `false` and `sleep` are probes —
/// they carry no capability of their own and make the security, exit-code and
/// timeout paths testable against real processes instead of mocks.
pub const ALLOWED_BINARIES: &[&str] = &[
    // Build, test, lint.
    "cargo", "make", "node", "pnpm", "npm", "tsc", "eslint", "vitest", "python3",
    // Repository and forge state.
    "git", "gh",  // Token accounting.
    "rtk", // Probes.
    "echo", "true", "false", "sleep",
    // KT-829 — plugin CLI access probes (`registry::cli_access_probe`).
    // Presence/version/auth checks only, never a mutating subcommand.
    "fastly", "glab",
];

pub use crate::models::setup::DENIED_BINARIES;

/// What a caller asks for. Every field is explicit — nothing is inherited from
/// the server process, because an inherited cwd, environment or stdin is exactly
/// how a bounded runner turns into an arbitrary one.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickExecSpec {
    /// Bare binary name, matched against [`ALLOWED_BINARIES`].
    pub binary: String,
    /// Passed as separate arguments. Never joined, never re-split, never handed
    /// to a shell — a `;` or `$(…)` in here is a literal character.
    pub argv: Vec<String>,
    /// Absolute, and required to resolve inside one of the caller's roots.
    pub cwd: PathBuf,
    pub timeout_secs: Option<u64>,
    /// `None` closes stdin. It is never inherited: a command that waits for input
    /// it will never get looks exactly like a hang.
    pub stdin: Option<String>,
    /// Which extractor turns the streams into a summary.
    pub summariser: Summariser,
}

/// A spec that has passed validation. Constructed only by [`validate`], so a
/// `run` cannot be reached with an unchecked binary or cwd.
#[derive(Debug, Clone)]
pub struct ValidatedSpec {
    binary: String,
    argv: Vec<String>,
    cwd: PathBuf,
    timeout_secs: u64,
    stdin: Option<String>,
    summariser: Summariser,
    /// KT-1751 — the resolved file of a repository-relative executable, so the
    /// process that runs is the file that was checked.
    local_executable: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Summariser {
    CargoTest,
    Clippy,
    Tsc,
    Vitest,
    Generic,
    /// For a command run to COLLECT data rather than to pass or fail. The summary
    /// says only how much arrived; the data itself stays in the artifact, which is
    /// the point — a JSON page of review comments must not enter a context just
    /// because something had to fetch it.
    Collected,
}

pub use crate::models::agent_jobs::{ArtifactRef, Diagnostic, QuickExecResult, QuickExecStatus};

/// Why a spec was refused. Carried as a message rather than an enum because the
/// caller shows it to a human who has to fix the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection(pub String);

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Check a spec against the allowlist, the denylist and the caller's roots.
///
/// Order matters: the denylist is consulted even when the allowlist accepts, so
/// a shell added to the allowlist by a later edit is still refused.
pub fn validate(
    spec: &QuickExecSpec,
    allowed_roots: &[PathBuf],
) -> Result<ValidatedSpec, Rejection> {
    validate_with(spec, allowed_roots, &[])
}

/// [`validate`] with the executables the project's own settings add (KT-1751):
/// a bare name (`composer`, `go`, `pytest`) or a repository-relative path
/// (`vendor/bin/phpunit`), exactly as the human wrote it. The denylist still
/// refuses a shell, and a path must resolve to a file inside the working
/// directory it runs from.
pub fn validate_with(
    spec: &QuickExecSpec,
    allowed_roots: &[PathBuf],
    extra: &[String],
) -> Result<ValidatedSpec, Rejection> {
    let binary = spec.binary.trim();
    check_binary_with(binary, ALLOWED_BINARIES, extra)?;

    if spec.argv.len() > MAX_ARGV {
        return Err(Rejection(format!(
            "{} arguments exceeds the cap of {MAX_ARGV}",
            spec.argv.len()
        )));
    }
    for arg in &spec.argv {
        if arg.contains('\0') {
            return Err(Rejection(
                "an argument contains a NUL byte, which would truncate it on exec".into(),
            ));
        }
    }

    let cwd = validate_cwd(&spec.cwd, allowed_roots)?;

    let timeout_secs = match spec.timeout_secs {
        None => DEFAULT_TIMEOUT_SECS,
        Some(0) => {
            return Err(Rejection(
                "a timeout of 0 would kill the command instantly".into(),
            ))
        }
        Some(value) if value > MAX_TIMEOUT_SECS => {
            return Err(Rejection(format!(
                "timeout {value}s exceeds the cap of {MAX_TIMEOUT_SECS}s"
            )))
        }
        Some(value) => value,
    };

    let local_executable = if binary.contains('/') {
        Some(resolve_repo_executable(binary, &cwd)?)
    } else {
        None
    };
    Ok(ValidatedSpec {
        binary: binary.to_string(),
        argv: spec.argv.clone(),
        cwd,
        timeout_secs,
        stdin: spec.stdin.clone(),
        summariser: spec.summariser,
        local_executable,
    })
}

/// A repository-relative executable, resolved: a file of the working directory
/// itself. Canonicalised on both sides, like [`validate_cwd`], so a link that
/// leaves the repository is refused whatever it is called — except into the
/// main checkout's own copy of a dependency directory a lane layers (see
/// [`in_layered_dependency_dir`]).
fn resolve_repo_executable(binary: &str, cwd: &Path) -> Result<PathBuf, Rejection> {
    let relative = binary.strip_prefix("./").unwrap_or(binary);
    let real = cwd.join(relative).canonicalize().map_err(|error| {
        Rejection(format!(
            "`{relative}` cannot be resolved in the worktree: {error}"
        ))
    })?;
    if !real.starts_with(cwd) && !in_layered_dependency_dir(relative, cwd, &real) {
        return Err(Rejection(format!(
            "`{relative}` resolves to {}, outside the worktree it runs in",
            real.display()
        )));
    }
    if !real.is_file() {
        return Err(Rejection(format!("`{relative}` is not a file")));
    }
    Ok(real)
}

/// KT-1751 review — in a task lane, a dependency directory (`vendor`,
/// `node_modules`, …) is layered from the main checkout: its entries link
/// there, so a listed `vendor/phpunit/phpunit/phpunit` or
/// `node_modules/.bin/jest` resolves outside the worktree. It is admitted when
/// `relative` lies in such a directory AND `real` lies in the main checkout's
/// copy of that same directory: nothing wider (a layer entry re-pointed
/// elsewhere is still refused). The main checkout comes from the lane's path
/// (`<repo>/.kronn/worktrees/<name>`), never from anything inside the lane.
fn in_layered_dependency_dir(relative: &str, cwd: &Path, real: &Path) -> bool {
    let Ok(lane) = crate::core::task_worker_env::WorkerLane::for_worktree(cwd) else {
        return false;
    };
    let Some(main) = lane.main_checkout.clone() else {
        return false;
    };
    lane.dependency_paths().iter().any(|dir| {
        relative.starts_with(&format!("{dir}/"))
            && main
                .join(dir)
                .canonicalize()
                .is_ok_and(|source| real.starts_with(source))
    })
}

/// Decide whether a binary name may be spawned.
///
/// Takes the allowlist as a parameter so a test can hand it one that CONTAINS a
/// shell and still see the refusal. Without that seam, a test of the denylist
/// passes for the wrong reason — the name is simply absent from the allowlist —
/// and would keep passing if the denylist were deleted.
#[cfg(any(all(test, feature = "full-tests"), feature = "test-support"))]
fn check_binary(binary: &str, allowlist: &[&str]) -> Result<(), Rejection> {
    check_binary_with(binary, allowlist, &[])
}

/// [`check_binary`] plus the project's own entries (KT-1751). An entry matches
/// the binary exactly, a leading `./` ignored on both sides: the human decided
/// that name or that path, nothing around it.
fn check_binary_with(binary: &str, allowlist: &[&str], extra: &[String]) -> Result<(), Rejection> {
    if binary.is_empty() {
        return Err(Rejection("no binary given".into()));
    }
    // A bare name the project lists (`composer`) is a program on the PATH and
    // never a repository file `./composer`; a path entry never a bare name.
    let is_path = |name: &str| name.contains('/') || name.contains('\\');
    let listed = extra.iter().any(|entry| {
        let entry = entry.trim();
        is_path(entry) == is_path(binary)
            && entry.trim_start_matches("./") == binary.trim_start_matches("./")
    });
    if binary.contains('/') || binary.contains('\\') {
        // A path is an executable OF THE REPOSITORY, never any executable on the
        // machine (which is what the allowlist prevents): relative, inside, and
        // named by the project's own list.
        if !is_repo_relative(binary) {
            return Err(Rejection(format!(
                "`{binary}` is a path or a flag — Quick Exec takes a bare allowlisted name or \
                 a repository-relative executable the project lists"
            )));
        }
        if !listed {
            return Err(Rejection(format!(
                "`{binary}` is a repository path that the project has not listed: add it to \
                 the project's Validation executables (project card, Lane warm-up) to run it"
            )));
        }
        return Ok(());
    }
    if binary.starts_with('-') {
        return Err(Rejection(format!(
            "`{binary}` is a path or a flag — Quick Exec takes a bare allowlisted name"
        )));
    }
    // Before the allowlist, so a shell added to the allowlist by a later edit is
    // still refused — and so is one a project lists.
    if DENIED_BINARIES.contains(&binary) {
        return Err(Rejection(format!(
            "`{binary}` can execute a command line of its own — Quick Exec runs a literal argv, never a shell"
        )));
    }
    if !allowlist.contains(&binary) && !listed {
        return Err(Rejection(format!(
            "`{binary}` is not in the Quick Exec allowlist"
        )));
    }
    Ok(())
}

/// `vendor/bin/phpunit`, `./tools/lint`: relative, no `..`, no empty part.
fn is_repo_relative(path: &str) -> bool {
    let path = path.strip_prefix("./").unwrap_or(path);
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Check a persisted one-line command (an execution's validation) BEFORE it is
/// accepted, against the very rules its later run applies.
///
/// The line is run by splitting on whitespace and handing the pieces to
/// [`validate`]: there is no shell, no quoting and no expansion. A command
/// written for a shell — `cd frontend && npx tsc`, `FOO=1 cargo test`,
/// `cargo test | tee out` — is therefore not "run differently", it is refused at
/// integration, after the worker was launched, reviewed and approved. This is
/// the same decision made at the moment the command is written instead.
///
/// The working directory is not part of the check: a validation always starts at
/// the root of the execution's worktree.
pub fn check_command_line(command: &str) -> Result<(), Rejection> {
    check_command_line_with(command, &[])
}

/// [`check_command_line`] with the executables the project's own settings add
/// (KT-1751).
pub fn check_command_line_with(command: &str, extra: &[String]) -> Result<(), Rejection> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let Some(&binary) = words.first() else {
        return Err(Rejection("the command is empty".into()));
    };
    let form = |reason: String| {
        Rejection(format!(
            "{reason}. A validation is ONE binary from [{}] followed by literal arguments, run \
             WITHOUT a shell from the root of the worktree: no `cd`, `&&`, `||`, `|`, `;`, \
             redirection, `$(…)` or `VAR=value` prefix, and no quoting (words are split on \
             whitespace). Target a subdirectory with the tool's own option — \
             `pnpm --dir frontend exec tsc -b --pretty false` instead of \
             `cd frontend && npx tsc -b`, `cargo test --manifest-path backend/Cargo.toml \
             --target-dir <dir>` instead of `cd backend && CARGO_TARGET_DIR=<dir> cargo test`",
            ALLOWED_BINARIES.join(", ")
        ))
    };
    if is_env_assignment(binary) {
        return Err(form(format!(
            "`{binary}` is an environment assignment, which needs a shell; the command would \
             start with it as its binary"
        )));
    }
    check_binary_with(binary, ALLOWED_BINARIES, extra).map_err(|rejection| form(rejection.0))?;
    if let Some((word, syntax)) = words
        .iter()
        .find_map(|word| shell_syntax(word).map(|syntax| (*word, syntax)))
    {
        return Err(form(format!(
            "`{word}` contains {syntax}, which only a shell interprets — here it would reach \
             `{binary}` as a literal argument"
        )));
    }
    Ok(())
}

/// `NAME=value` where a binary is expected: the shell prefix for an environment
/// variable, which a literal argv cannot express.
fn is_env_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Name the shell syntax a whitespace-separated word carries, if it clearly does.
///
/// Deliberately narrow: only spellings that have no reason to reach a binary as
/// an argument. A lone `&` or a `<` inside `--format=a<b` is left alone, so a
/// legitimate literal is never refused.
fn shell_syntax(word: &str) -> Option<&'static str> {
    if word.contains("&&") {
        return Some("the operator `&&`");
    }
    if word.contains("||") {
        return Some("the operator `||`");
    }
    if word.contains('|') {
        return Some("a pipe `|`");
    }
    if word.contains(';') {
        return Some("a command separator `;`");
    }
    if word.contains('`') || word.contains("$(") {
        return Some("a command substitution");
    }
    if word == "&" {
        return Some("the background operator `&`");
    }
    // `>`, `>>`, `<`, `2>&1`, `>out.txt`: a redirection is a word that OPENS with
    // one, after an optional file descriptor.
    if word
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .starts_with(['>', '<'])
    {
        return Some("a redirection");
    }
    None
}

/// Whether the extracted lists can be treated as exhaustive.
///
/// Its own function because it is the one boolean that decides whether an empty
/// `failed_tests` means "nothing failed" or "we did not see the whole log".
fn findings_are_complete(truncated: bool, stdout_eof: bool, stderr_eof: bool) -> bool {
    !truncated && stdout_eof && stderr_eof
}

/// Resolve the cwd and require it to sit inside a declared root.
///
/// Canonicalised on both sides before comparing: a lexical check alone is
/// defeated by a symlink, and `..` inside an existing path is resolved away by
/// the OS rather than by us.
fn validate_cwd(cwd: &Path, allowed_roots: &[PathBuf]) -> Result<PathBuf, Rejection> {
    if allowed_roots.is_empty() {
        return Err(Rejection(
            "no root is declared for this caller, so no working directory can be accepted".into(),
        ));
    }
    if !cwd.is_absolute() {
        return Err(Rejection(format!(
            "{} is relative — the working directory must be absolute",
            cwd.display()
        )));
    }
    if cwd.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(Rejection(format!(
            "{} contains a parent traversal",
            cwd.display()
        )));
    }
    let real = cwd
        .canonicalize()
        .map_err(|e| Rejection(format!("{} cannot be resolved: {e}", cwd.display())))?;
    if !real.is_dir() {
        return Err(Rejection(format!("{} is not a directory", real.display())));
    }
    for root in allowed_roots {
        if let Ok(real_root) = root.canonicalize() {
            if real.starts_with(&real_root) {
                return Ok(real);
            }
        }
    }
    Err(Rejection(format!(
        "{} is outside every declared root",
        real.display()
    )))
}

/// Stable identity of a spec, for the idempotency check at the storage layer.
///
/// Covers everything that changes what the command does — including the cwd,
/// because the same argv in another directory is another run. Deliberately does
/// NOT cover the timeout: a longer timeout on the same work is the same work.
pub fn spec_fingerprint(spec: &ValidatedSpec) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(spec.binary.as_bytes());
    for arg in &spec.argv {
        hasher.update([0u8]);
        hasher.update(arg.as_bytes());
    }
    hasher.update([0u8]);
    hasher.update(spec.cwd.to_string_lossy().as_bytes());
    hasher.update([0u8]);
    hasher.update(spec.stdin.as_deref().unwrap_or("").as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..16]
        .to_string()
}

/// Read to EOF, keeping at most `cap` bytes.
///
/// Keeps reading past the cap on purpose: stopping would fill the pipe and block
/// the child, and a command that stalls because we stopped listening would be
/// reported as a timeout of its own making.
async fn drain_capped<R>(mut reader: R, cap: usize) -> (Vec<u8>, u64)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut kept = Vec::new();
    let mut total: u64 = 0;
    let mut chunk = [0u8; 8_192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                total += read as u64;
                if kept.len() < cap {
                    let room = cap - kept.len();
                    kept.extend_from_slice(&chunk[..read.min(room)]);
                }
            }
        }
    }
    (kept, total)
}

/// The text of a drained stream, for extraction. KT-2039: a stream cut at the
/// cap (`total` above what was kept) ends mid-line, and a token cut below its
/// pattern's minimum length is no longer recognised by the redaction: that
/// last partial line is dropped. The artifact still keeps every byte.
fn whole_lines(kept: &[u8], total: u64) -> String {
    let whole = if total > kept.len() as u64 {
        kept.iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(&kept[..0], |end| &kept[..=end])
    } else {
        kept
    };
    String::from_utf8_lossy(whole).into_owned()
}

enum Outcome {
    Exited(std::process::ExitStatus),
    TimedOut,
    Cancelled,
}

/// The process a validated spec starts, before stdio: the resolved binary,
/// its argv and cwd, and a built environment (KT-1006). Nothing is inherited.
///
/// With a [`CommandConfinement`] (KT-1268: the lane's OS sandbox; KT-1492: the
/// sandbox of what a trust level grants) the same process runs under it,
/// started the way the lane's first build is.
///
/// `priority` (KT-1633): a validation runs in the background, a collector the
/// owner asked for keeps the backend's own priority.
fn build_command(
    spec: &ValidatedSpec,
    confinement: Option<&dyn CommandConfinement>,
    priority: Priority,
) -> tokio::process::Command {
    let route = crate::core::child_env::ChildRoute::QuickExec;
    match confinement {
        None => {
            let mut command = async_cmd_with(resolve_binary(spec), route, priority);
            command.args(&spec.argv).current_dir(&spec.cwd);
            command
        }
        Some(confinement) => confinement.command(
            &ProbeCommand {
                program: resolve_binary(spec).to_string_lossy().into_owned(),
                args: spec.argv.clone(),
                cwd: spec.cwd.clone(),
                env: Vec::new(),
                timeout: std::time::Duration::from_secs(spec.timeout_secs),
                priority,
                clean_env: false,
            },
            route,
        ),
    }
}

/// Spawn a validated spec and return its bounded result.
///
/// `artifact_dir` receives the full streams. When it is `None` the streams are
/// summarised and dropped — the result then says so via `artifact: None`, so a
/// reader can tell "not kept" from "empty".
///
/// The command runs with the backend's own rights: use [`run_confined`] for
/// anything a task worker wrote.
pub async fn run(
    spec: &ValidatedSpec,
    artifact_dir: Option<&Path>,
    cancel: &CancellationToken,
) -> Result<QuickExecResult> {
    run_with(spec, artifact_dir, cancel, None, Priority::Normal).await
}

/// [`run`] for a validation (KT-1633): the command yields to the owner's
/// interactive applications, like a task worker's own build.
pub async fn run_background(
    spec: &ValidatedSpec,
    artifact_dir: Option<&Path>,
    cancel: &CancellationToken,
) -> Result<QuickExecResult> {
    run_with(spec, artifact_dir, cancel, None, Priority::Background).await
}

/// [`run`] under the worker's sandbox (KT-1268): what the command may write is
/// the lane's roots and its temp dir, it reaches no network (loopback included)
/// and has no terminal. A timeout or a cancel takes the command's whole process
/// group down, not only its leader. `priority` (KT-1633): a validation of a
/// worker's tree passes [`Priority::Background`].
pub async fn run_confined(
    spec: &ValidatedSpec,
    artifact_dir: Option<&Path>,
    cancel: &CancellationToken,
    confinement: &dyn CommandConfinement,
    priority: Priority,
) -> Result<QuickExecResult> {
    run_with(spec, artifact_dir, cancel, Some(confinement), priority).await
}

/// SIGKILL to the whole process group `leader` leads. Only for a child that
/// was spawned as its own group's leader ([`CommandConfinement::command`] puts it in a
/// session of its own): the id of any other process may name an unrelated group.
#[cfg(unix)]
pub fn kill_process_group(leader: u32) {
    // SAFETY: `killpg` takes a plain process-group id.
    unsafe {
        libc::killpg(leader as libc::pid_t, libc::SIGKILL);
    }
}

async fn run_with(
    spec: &ValidatedSpec,
    artifact_dir: Option<&Path>,
    cancel: &CancellationToken,
    confinement: Option<&dyn CommandConfinement>,
    priority: Priority,
) -> Result<QuickExecResult> {
    let started = std::time::Instant::now();

    let mut command = build_command(spec, confinement, priority);
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        // A panic between spawn and wait must not leave the child running.
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            return Ok(spawn_failure(
                &spec.binary,
                &e.to_string(),
                started.elapsed().as_millis() as u64,
            ))
        }
    };

    // A confined command leads a session of its own (`Confinement::command`),
    // so its group is its whole process tree.
    #[cfg(unix)]
    let confined_group = confinement.and(child.id());

    if let (Some(data), Some(mut pipe)) = (spec.stdin.clone(), child.stdin.take()) {
        // In a task: a child that never reads its stdin would block us here.
        tokio::spawn(async move {
            let _ = pipe.write_all(data.as_bytes()).await;
            let _ = pipe.shutdown().await;
        });
    }

    let stdout = child.stdout.take().context("stdout pipe missing")?;
    let stderr = child.stderr.take().context("stderr pipe missing")?;
    let out_task = tokio::spawn(drain_capped(stdout, ARTIFACT_MAX_BYTES));
    let err_task = tokio::spawn(drain_capped(stderr, ARTIFACT_MAX_BYTES));

    let outcome = {
        let waiter = child.wait();
        tokio::pin!(waiter);
        tokio::select! {
            res = &mut waiter => match res {
                Ok(status) => Outcome::Exited(status),
                Err(_) => Outcome::TimedOut,
            },
            _ = tokio::time::sleep(std::time::Duration::from_secs(spec.timeout_secs)) => Outcome::TimedOut,
            _ = cancel.cancelled() => Outcome::Cancelled,
        }
    };

    let exit = match &outcome {
        Outcome::Exited(status) => Some(*status),
        // Both remaining paths mean we are ending the process, not that it ended.
        _ => {
            #[cfg(unix)]
            if let Some(group) = confined_group {
                kill_process_group(group);
            }
            let _ = child.start_kill();
            child.wait().await.ok()
        }
    };

    let grace = std::time::Duration::from_secs(STREAM_DRAIN_GRACE_SECS);
    let (stdout_kept, stdout_total, stdout_eof) = match tokio::time::timeout(grace, out_task).await
    {
        Ok(Ok((kept, total))) => (kept, total, true),
        // A reader that did not finish leaves us with a partial stream. It is
        // recorded as partial rather than as all there was.
        _ => (Vec::new(), 0, false),
    };
    let (stderr_kept, stderr_total, stderr_eof) = match tokio::time::timeout(grace, err_task).await
    {
        Ok(Ok((kept, total))) => (kept, total, true),
        _ => (Vec::new(), 0, false),
    };

    let truncated =
        stdout_total > stdout_kept.len() as u64 || stderr_total > stderr_kept.len() as u64;
    let findings_complete = findings_are_complete(truncated, stdout_eof, stderr_eof);

    let status = match outcome {
        Outcome::Cancelled => QuickExecStatus::Cancelled,
        Outcome::TimedOut => QuickExecStatus::TimedOut,
        Outcome::Exited(status) if status.success() => QuickExecStatus::Passed,
        // Includes death on a signal, where `code()` is None.
        Outcome::Exited(_) => QuickExecStatus::Failed,
    };
    let exit_code = exit.and_then(|status| status.code());

    let stdout_text = whole_lines(&stdout_kept, stdout_total);
    let stderr_text = whole_lines(&stderr_kept, stderr_total);
    let extracted = summarise(
        spec.summariser,
        &stdout_text,
        &stderr_text,
        status,
        findings_complete,
    );

    let artifact = match artifact_dir {
        Some(dir) => write_artifact(dir, &stdout_kept, &stderr_kept, truncated).ok(),
        None => None,
    };

    Ok(QuickExecResult {
        status,
        exit_code,
        summary: extracted.summary,
        failed_tests: extracted.failed_tests,
        diagnostics: extracted.diagnostics,
        artifact,
        duration_ms: started.elapsed().as_millis() as u64,
        stdout_bytes: stdout_total,
        stderr_bytes: stderr_total,
        findings_complete,
    })
}

/// The result of a command that never started.
///
/// `Rejected` and not `Failed`: a binary that is missing produced no output, and
/// "no output" must not be readable as "found nothing wrong".
fn spawn_failure(binary: &str, error: &str, duration_ms: u64) -> QuickExecResult {
    QuickExecResult {
        status: QuickExecStatus::Rejected,
        exit_code: None,
        summary: format!("{binary} could not be spawned: {error}"),
        failed_tests: Vec::new(),
        diagnostics: Vec::new(),
        artifact: None,
        duration_ms,
        stdout_bytes: 0,
        stderr_bytes: 0,
        findings_complete: false,
    }
}

/// Prefer a project-local binary over PATH.
///
/// Matches the repository rule that `tsc`, `eslint` and `vitest` are invoked from
/// `node_modules/.bin` rather than through a package runner, which in a worktree
/// can rewrite the main checkout's `node_modules`. The name still comes from the
/// allowlist — this only decides where that name resolves.
fn resolve_binary(spec: &ValidatedSpec) -> PathBuf {
    if let Some(executable) = &spec.local_executable {
        return executable.clone();
    }
    let local = spec
        .cwd
        .join("node_modules")
        .join(".bin")
        .join(&spec.binary);
    if local.is_file() {
        return local;
    }
    PathBuf::from(&spec.binary)
}

/// The bounded canonical view of a run's streams.
pub struct Extracted {
    pub summary: String,
    pub failed_tests: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Turn the streams into the bounded canonical view.
///
/// `findings_complete` is threaded in so the summary can SAY that its lists are
/// partial. A "0 failures" drawn from a truncated log is the failure mode this
/// whole module exists to avoid.
pub fn summarise(
    summariser: Summariser,
    stdout: &str,
    stderr: &str,
    status: QuickExecStatus,
    findings_complete: bool,
) -> Extracted {
    let mut failed_tests = Vec::new();
    let mut diagnostics = Vec::new();
    let mut lines = Vec::new();
    // KT-2035: why the first failing tests failed (a panic and its assertion,
    // an `AssertionError` and its diff), not only their names.
    let mut details = None;

    match summariser {
        Summariser::CargoTest => {
            for line in stdout.lines().chain(stderr.lines()) {
                // The headline is checked first: it also begins with "test ", so
                // testing for an individual case ahead of it would swallow it.
                if line.starts_with("test result:") || line.starts_with("error[") {
                    push_capped(&mut lines, line.trim().to_string(), MAX_DIAGNOSTICS);
                } else if let Some(name) = line
                    .strip_prefix("test ")
                    .and_then(|name| name.strip_suffix(" ... FAILED"))
                {
                    push_capped(&mut failed_tests, redact(name.trim()), MAX_FAILED_TESTS);
                }
            }
            if status != QuickExecStatus::Passed {
                details = failure_details(cargo_test_failure_blocks(stdout, stderr));
            }
        }
        Summariser::Clippy => {
            let mut pending: Option<String> = None;
            for line in stderr.lines() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("warning: ") || trimmed.starts_with("error: ") {
                    pending = Some(trimmed.to_string());
                } else if let Some(location) = trimmed.strip_prefix("--> ") {
                    if let Some(message) = pending.take() {
                        let (path, line_no) = split_location(location);
                        push_capped(
                            &mut diagnostics,
                            Diagnostic {
                                path,
                                line: line_no,
                                message: redact(&message),
                            },
                            MAX_DIAGNOSTICS,
                        );
                    }
                }
            }
        }
        Summariser::Tsc => {
            for line in stdout.lines().chain(stderr.lines()) {
                if let Some(index) = line.find("): error TS") {
                    let (head, message) = line.split_at(index);
                    let (path, line_no) = split_paren_location(head);
                    push_capped(
                        &mut diagnostics,
                        Diagnostic {
                            path,
                            line: line_no,
                            message: redact(message.trim_start_matches("): ").trim()),
                        },
                        MAX_DIAGNOSTICS,
                    );
                }
            }
        }
        Summariser::Vitest => {
            for line in stdout.lines().chain(stderr.lines()) {
                let trimmed = line.trim_start();
                for marker in ["FAIL ", "× ", "✗ "] {
                    if let Some(name) = trimmed.strip_prefix(marker) {
                        // A vitest name is free text.
                        push_capped(&mut failed_tests, redact(name.trim()), MAX_FAILED_TESTS);
                        break;
                    }
                }
            }
            if status != QuickExecStatus::Passed {
                details = failure_details(vitest_failure_blocks(stdout, stderr));
            }
        }
        Summariser::Collected => {
            // Deliberately not the data. Stating the size lets a reader tell an
            // empty collection from a failed one without paying for the content.
            lines.push(format!(
                "collected {} byte(s) over {} line(s) — in the artifact",
                stdout.len(),
                stdout.lines().count()
            ));
            for line in stderr.lines().filter(|l| !l.trim().is_empty()).take(5) {
                lines.push(line.trim().to_string());
            }
        }
        Summariser::Generic => {
            let source = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            let mut tail: Vec<String> = source
                .lines()
                .rev()
                .filter(|l| !l.trim().is_empty())
                .take(20)
                .map(|line| line.trim().to_string())
                .collect();
            tail.reverse();
            // KT-1438: a wrapper (`make`, `docker compose`) prints its own
            // verdict on stderr and the real failure (a phpunit diff, a Twig
            // exception) on stdout, far above the tail: the tail alone sent a
            // worker back with "Error 1" and a platform warning. A red run
            // also keeps the first error lines of BOTH streams.
            let first_errors = if status == QuickExecStatus::Passed {
                Vec::new()
            } else {
                first_error_lines(stdout, stderr, &tail)
            };
            if !first_errors.is_empty() {
                lines.push(cap_bytes_keeping_start(
                    &format!("first errors:\n{}", first_errors.join("\n")),
                    FIRST_ERRORS_MAX_BYTES,
                ));
                lines.push("… (the rest is in the artifact)".to_string());
            }
            if !tail.is_empty() {
                // Redacted before the cut, which keeps the end: cut right after
                // a secret's name, the value left behind is no assignment.
                lines.push(cap_bytes_keeping_end(
                    &redact(&tail.join("\n")),
                    TAIL_MAX_BYTES,
                ));
            }
        }
    }

    let mut summary = String::new();
    if !findings_complete {
        // First line, because a reader who stops early must still see it.
        summary
            .push_str("PARTIAL OUTPUT — the lists below are not exhaustive; read the artifact.\n");
    }
    summary.push_str(&format!("{status:?}"));
    if !failed_tests.is_empty() {
        summary.push_str(&format!(" — {} failing", failed_tests.len()));
    }
    if !diagnostics.is_empty() {
        summary.push_str(&format!(" — {} diagnostics", diagnostics.len()));
    }
    summary.push('\n');
    // Right under the verdict: the room and the worker are sent the start of a
    // long output, and the failing names that follow can fill the summary.
    if let Some(details) = &details {
        summary.push_str(details);
        summary.push('\n');
    }
    for line in lines.iter().chain(failed_tests.iter()) {
        summary.push_str(line);
        summary.push('\n');
    }
    for diagnostic in &diagnostics {
        summary.push_str(&format!(
            "{}:{} {}\n",
            diagnostic.path.as_deref().unwrap_or("?"),
            diagnostic.line.map(|l| l.to_string()).unwrap_or_default(),
            diagnostic.message
        ));
    }
    // KT-2039 — the one choke point: the summary is stored as a validation's
    // output, sent to the worker's and the principal's rooms, returned by Quick
    // Exec and posted by agent resume jobs, so it leaves here redacted whole
    // (failing names, a collector's stderr, every line). The parts cut above
    // were redacted before their cut; this pass is before the last one.
    let summary = truncate_on_char_boundary(redact(&summary), SUMMARY_MAX_BYTES);

    Extracted {
        summary,
        failed_tests,
        diagnostics,
    }
}

/// The redaction every text that leaves [`summarise`] goes through.
fn redact(text: &str) -> String {
    crate::core::redact::redact_stored_text(text)
}

/// What the generic summary keeps of a red run's first error lines, and of its
/// last lines, so that neither can push the other out of [`SUMMARY_MAX_BYTES`].
const FIRST_ERRORS_MAX_BYTES: usize = 1_800;
const TAIL_MAX_BYTES: usize = 2_000;
const FIRST_ERROR_LINES: usize = 12;
const ERROR_LINE_MAX_CHARS: usize = 300;

/// The first lines of the output that name a failure (an error, an exception,
/// a failed assertion, a panic), stdout then stderr, minus those the tail
/// already shows. A line that merely says a count of `0 errors` is not one.
fn first_error_lines(stdout: &str, stderr: &str, tail: &[String]) -> Vec<String> {
    const MARKERS: &[&str] = &[
        "error",
        "exception",
        "fatal",
        "panic",
        "traceback",
        "failed",
        "failure",
        "assert",
    ];
    stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            MARKERS.iter().any(|marker| lower.contains(marker))
                && !["0 error", "no error", "0 failed", "0 failure"]
                    .iter()
                    .any(|benign| lower.contains(benign))
        })
        .filter(|line| !tail.iter().any(|shown| shown == line))
        .take(FIRST_ERROR_LINES)
        // Error lines are where a connection string or a token turns up: they
        // leave to the room and the worker redacted, before being cut.
        .map(crate::core::redact::redact_stored_text)
        .map(|line| {
            if line.chars().count() > ERROR_LINE_MAX_CHARS {
                let cut: String = line.chars().take(ERROR_LINE_MAX_CHARS).collect();
                format!("{cut}…")
            } else {
                line.to_string()
            }
        })
        .collect()
}

/// KT-2035: what a red test run keeps of the blocks that say why its first
/// failing tests failed. Small enough that the section, right under the
/// verdict, stays within the head of the output a rework message keeps.
const FAILURE_DETAILS_MAX_BYTES: usize = 1_300;
const FAILURE_BLOCKS: usize = 3;
const FAILURE_BLOCK_LINES: usize = 12;

/// The failure blocks of a libtest run: from `---- <test> stdout ----` up to
/// the next block, the `failures:` list of names or the verdict. A runner that
/// prints no such block (`cargo nextest`, a custom harness) still prints the
/// panic: then each `panicked at` line and the lines under it, up to a blank.
fn cargo_test_failure_blocks<'a>(stdout: &'a str, stderr: &'a str) -> Vec<Vec<&'a str>> {
    let lines = || stdout.lines().chain(stderr.lines());
    let libtest = failure_blocks(
        lines(),
        |line| {
            let line = line.trim();
            line.starts_with("---- ") && line.ends_with(" ----")
        },
        |line| {
            let line = line.trim();
            line == "failures:" || line.starts_with("test result:")
        },
    );
    if !libtest.is_empty() {
        return libtest;
    }
    failure_blocks(
        lines(),
        |line| line.contains("panicked at"),
        |line| line.trim().is_empty(),
    )
}

/// The failure blocks of a vitest run: from each `FAIL` heading of its failed
/// tests up to the `⎯` separator, the next heading or the closing counts.
fn vitest_failure_blocks<'a>(stdout: &'a str, stderr: &'a str) -> Vec<Vec<&'a str>> {
    failure_blocks(
        stdout.lines().chain(stderr.lines()),
        |line| line.trim_start().starts_with("FAIL "),
        |line| {
            let line = line.trim_start();
            line.starts_with('⎯') || line.starts_with("Test Files ")
        },
    )
}

/// The first [`FAILURE_BLOCKS`] blocks that `starts` opens and `ends` (or the
/// next opening) closes, each without its blank lines, a runner's backtrace
/// hint and beyond [`FAILURE_BLOCK_LINES`] lines.
fn failure_blocks<'a>(
    lines: impl Iterator<Item = &'a str>,
    starts: impl Fn(&str) -> bool,
    ends: impl Fn(&str) -> bool,
) -> Vec<Vec<&'a str>> {
    let mut blocks: Vec<Vec<&'a str>> = Vec::new();
    let mut open = false;
    for line in lines {
        if starts(line) {
            if blocks.len() == FAILURE_BLOCKS {
                break;
            }
            blocks.push(vec![line]);
            open = true;
        } else if open && ends(line) {
            open = false;
        } else if open {
            let block = blocks.last_mut().expect("an open block exists");
            let noise = line.trim().is_empty()
                || line
                    .trim_start()
                    .starts_with("note: run with `RUST_BACKTRACE");
            if !noise && block.len() < FAILURE_BLOCK_LINES {
                block.push(line);
            }
        }
    }
    blocks
}

/// The kept failure blocks as one capped section, redacted like
/// [`first_error_lines`]: a panic message is where a connection string turns up.
fn failure_details(blocks: Vec<Vec<&str>>) -> Option<String> {
    if blocks.is_empty() {
        return None;
    }
    let text = blocks
        .iter()
        .map(|block| {
            // Redacted whole, before any cut, so no credential is split by it.
            crate::core::redact::redact_stored_text(&block.join("\n"))
                .lines()
                .map(|line| {
                    let line = line.trim_end();
                    if line.chars().count() > ERROR_LINE_MAX_CHARS {
                        let cut: String = line.chars().take(ERROR_LINE_MAX_CHARS).collect();
                        format!("{cut}…")
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(cap_bytes_keeping_start(
        &format!("failure details:\n{text}"),
        FAILURE_DETAILS_MAX_BYTES,
    ))
}

/// At most `max` bytes of the start of `text`, on a character boundary.
fn cap_bytes_keeping_start(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n… truncated", &text[..cut])
}

/// At most `max` bytes of the end of `text`, on a character boundary: a test
/// runner or a compiler prints its verdict last.
fn cap_bytes_keeping_end(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut cut = text.len() - max;
    while cut < text.len() && !text.is_char_boundary(cut) {
        cut += 1;
    }
    format!("… truncated\n{}", &text[cut..])
}

fn push_capped<T>(target: &mut Vec<T>, value: T, cap: usize) {
    if target.len() < cap {
        target.push(value);
    }
}

/// `path:line:col` → path and line.
fn split_location(location: &str) -> (Option<String>, Option<i64>) {
    let mut parts = location.split(':');
    let path = parts.next().map(|p| p.trim().to_string());
    let line = parts.next().and_then(|l| l.trim().parse().ok());
    (path, line)
}

/// `path(line,col` → path and line, as tsc writes it.
fn split_paren_location(head: &str) -> (Option<String>, Option<i64>) {
    match head.rfind('(') {
        Some(index) => {
            let (path, rest) = head.split_at(index);
            let line = rest
                .trim_start_matches('(')
                .split(',')
                .next()
                .and_then(|l| l.trim().parse().ok());
            (Some(path.trim().to_string()), line)
        }
        None => (Some(head.trim().to_string()), None),
    }
}

/// Cut to at most `max` bytes without splitting a character. Truncating a
/// multi-byte character would make the summary invalid UTF-8 at the boundary.
fn truncate_on_char_boundary(mut text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text.push_str("\n… truncated\n");
    text
}

/// Write the kept streams next to each other under `dir`.
fn write_artifact(
    dir: &Path,
    stdout: &[u8],
    stderr: &[u8],
    truncated: bool,
) -> Result<ArtifactRef> {
    std::fs::create_dir_all(dir)?;
    let name = format!("{}.log", uuid::Uuid::new_v4());
    let path = dir.join(&name);
    let mut body = Vec::with_capacity(stdout.len() + stderr.len() + 64);
    if truncated {
        body.extend_from_slice(b"# TRUNCATED - the process produced more than the artifact cap.\n");
    }
    body.extend_from_slice(b"# ---- stdout ----\n");
    body.extend_from_slice(stdout);
    body.extend_from_slice(b"\n# ---- stderr ----\n");
    body.extend_from_slice(stderr);
    std::fs::write(&path, &body)?;
    Ok(ArtifactRef {
        path: path.to_string_lossy().into_owned(),
        bytes: body.len() as u64,
        truncated,
    })
}

/// Retention: delete the oldest artifacts until the directory fits under
/// `max_total_bytes`.
///
/// Oldest-first because the newest artifact is the one someone is about to read.
/// Returns how many files were removed.
pub fn prune_artifacts(dir: &Path, max_total_bytes: u64) -> Result<usize> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut entries: Vec<(PathBuf, std::time::SystemTime, u64)> = std::fs::read_dir(dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((
                entry.path(),
                meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                meta.len(),
            ))
        })
        .collect();
    let mut total: u64 = entries.iter().map(|(_, _, size)| *size).sum();
    if total <= max_total_bytes {
        return Ok(0);
    }
    entries.sort_by_key(|(_, modified, _)| *modified);
    let mut removed = 0;
    for (path, _, size) in entries {
        if total <= max_total_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "quick_exec_test.rs"]
mod quick_exec_test;

#[cfg(all(test, feature = "full-tests"))]
#[path = "quick_exec_project_test.rs"]
mod quick_exec_project_test;
