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

use crate::core::cmd::async_cmd;
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
];

/// Names that are refused even if they appear in the allowlist.
///
/// The allowlist is a source file, so it will be edited. This is what makes DoD
/// "no `sh -c`" hold against a future edit rather than against today's list: a
/// shell added above is still rejected here, and the rejection names why.
pub const DENIED_BINARIES: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "csh",
    "tcsh",
    "cmd",
    "cmd.exe",
    "powershell",
    "pwsh",
    "env",
    "eval",
    "xargs",
    "nohup",
    "setsid",
    "sudo",
    "doas",
    "ssh",
    "perl",
    "ruby",
];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum QuickExecStatus {
    /// Exited 0. The ONLY value that means success.
    Passed,
    /// Exited non-zero, or died on a signal.
    Failed,
    TimedOut,
    Cancelled,
    /// Refused before spawning. Distinct from `Failed` so "we did not run this"
    /// is never read as "this ran and found nothing".
    Rejected,
}

impl QuickExecStatus {
    /// Success is exit 0 and nothing else. A timeout, a cancellation and a
    /// rejection all produce no findings, and none of them is a pass.
    pub fn is_success(self) -> bool {
        matches!(self, Self::Passed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Diagnostic {
    pub path: Option<String>,
    pub line: Option<i64>,
    pub message: String,
}

/// Where the full streams were kept. Bytes are the real size on disk; `truncated`
/// says the process produced more than that.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArtifactRef {
    pub path: String,
    pub bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickExecResult {
    pub status: QuickExecStatus,
    /// `None` when the process died on a signal or never started. Not 0 — an
    /// unknown exit code must not read as a clean one.
    pub exit_code: Option<i32>,
    pub summary: String,
    pub failed_tests: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub artifact: Option<ArtifactRef>,
    pub duration_ms: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    /// Whether `failed_tests` and `diagnostics` can be treated as exhaustive.
    /// False when a stream was truncated or never reached EOF: an empty list
    /// drawn from a partial log is not evidence of no failures.
    pub findings_complete: bool,
}

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
    let binary = spec.binary.trim();
    check_binary(binary, ALLOWED_BINARIES)?;

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

    Ok(ValidatedSpec {
        binary: binary.to_string(),
        argv: spec.argv.clone(),
        cwd,
        timeout_secs,
        stdin: spec.stdin.clone(),
        summariser: spec.summariser,
    })
}

/// Decide whether a binary name may be spawned.
///
/// Takes the allowlist as a parameter so a test can hand it one that CONTAINS a
/// shell and still see the refusal. Without that seam, a test of the denylist
/// passes for the wrong reason — the name is simply absent from the allowlist —
/// and would keep passing if the denylist were deleted.
fn check_binary(binary: &str, allowlist: &[&str]) -> Result<(), Rejection> {
    if binary.is_empty() {
        return Err(Rejection("no binary given".into()));
    }
    // A name, not a path: an explicit path would let the caller pick any
    // executable on the machine, which is the whole thing the allowlist prevents.
    if binary.contains('/') || binary.contains('\\') || binary.starts_with('-') {
        return Err(Rejection(format!(
            "`{binary}` is a path or a flag — Quick Exec takes a bare allowlisted name"
        )));
    }
    // Before the allowlist, so a shell added to the allowlist by a later edit is
    // still refused.
    if DENIED_BINARIES.contains(&binary) {
        return Err(Rejection(format!(
            "`{binary}` can execute a command line of its own — Quick Exec runs a literal argv, never a shell"
        )));
    }
    if !allowlist.contains(&binary) {
        return Err(Rejection(format!(
            "`{binary}` is not in the Quick Exec allowlist"
        )));
    }
    Ok(())
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

enum Outcome {
    Exited(std::process::ExitStatus),
    TimedOut,
    Cancelled,
}

/// Spawn a validated spec and return its bounded result.
///
/// `artifact_dir` receives the full streams. When it is `None` the streams are
/// summarised and dropped — the result then says so via `artifact: None`, so a
/// reader can tell "not kept" from "empty".
pub async fn run(
    spec: &ValidatedSpec,
    artifact_dir: Option<&Path>,
    cancel: &CancellationToken,
) -> Result<QuickExecResult> {
    let started = std::time::Instant::now();

    let mut command = async_cmd(resolve_binary(spec));
    command
        .args(&spec.argv)
        .current_dir(&spec.cwd)
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

    let stdout_text = String::from_utf8_lossy(&stdout_kept).into_owned();
    let stderr_text = String::from_utf8_lossy(&stderr_kept).into_owned();
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

struct Extracted {
    summary: String,
    failed_tests: Vec<String>,
    diagnostics: Vec<Diagnostic>,
}

/// Turn the streams into the bounded canonical view.
///
/// `findings_complete` is threaded in so the summary can SAY that its lists are
/// partial. A "0 failures" drawn from a truncated log is the failure mode this
/// whole module exists to avoid.
fn summarise(
    summariser: Summariser,
    stdout: &str,
    stderr: &str,
    status: QuickExecStatus,
    findings_complete: bool,
) -> Extracted {
    let mut failed_tests = Vec::new();
    let mut diagnostics = Vec::new();
    let mut lines = Vec::new();

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
                    push_capped(&mut failed_tests, name.trim().to_string(), MAX_FAILED_TESTS);
                }
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
                                message,
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
                            message: message.trim_start_matches("): ").trim().to_string(),
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
                        push_capped(&mut failed_tests, name.trim().to_string(), MAX_FAILED_TESTS);
                        break;
                    }
                }
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
            for line in source
                .lines()
                .rev()
                .filter(|l| !l.trim().is_empty())
                .take(20)
            {
                lines.push(line.trim().to_string());
            }
            lines.reverse();
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
    let summary = truncate_on_char_boundary(summary, SUMMARY_MAX_BYTES);

    Extracted {
        summary,
        failed_tests,
        diagnostics,
    }
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

#[cfg(test)]
#[path = "quick_exec_test.rs"]
mod quick_exec_test;
