//! KT-1127 — the `caffeinate` behind a power lease must not outlive the
//! backend while the lease is held, whether the backend is killed by SIGKILL
//! or exits normally (the static manager is never dropped, so `release()`
//! never runs). A helper process (this test binary re-run in helper mode)
//! takes the lease; its `caffeinate` must exit with it.

#![cfg(target_os = "macos")]
// The helper protocol and the ps/pgrep/lsof probes start processes directly.
#![allow(clippy::disallowed_methods)]

use std::io::{BufRead, BufReader, Write};
use std::os::fd::IntoRawFd;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SENTINEL: &str = "kt1127-inherited.lock";
const HELPER_ENV: &str = "KRONN_KT1127_POWER_HELPER";
const LEAK_ENV: &str = "KRONN_KT1127_LEAK_FILE";
const READY: &str = "KT1127_LEASE_HELD";
/// Helper mode that announces READY and then ignores the exit request.
const IGNORE_EXIT: &str = "ignore-exit";

/// Helper mode only: a no-op in a normal test run.
#[test]
fn helper_holds_a_power_lease() {
    let Some(mode) = std::env::var_os(HELPER_ENV) else {
        return;
    };
    // An inheritable descriptor, like the build-arbiter lock that kept the
    // orphan of 2026-10-09 alive: caffeinate must not receive it.
    let leak = std::fs::File::create(std::env::var_os(LEAK_ENV).unwrap()).unwrap();
    let fd = leak.into_raw_fd();
    // SAFETY: fcntl on a descriptor this process owns.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        assert!(flags != -1);
        assert!(libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) != -1);
    }
    let _lease = kronn::core::power_guard::acquire();
    println!("{READY}");
    if mode == IGNORE_EXIT {
        loop {
            std::thread::park();
        }
    }
    // Any stdin line asks for a normal exit with the lease still held;
    // `process::exit` skips every destructor, like a backend returning from
    // main. Without that line the helper waits to be killed.
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_ok() && !line.is_empty() {
        std::process::exit(0);
    }
    loop {
        std::thread::park();
    }
}

#[derive(Clone, Copy, Debug)]
enum End {
    SigKill,
    NormalExit,
}

/// Processes this test started, killed by pid on any exit path.
struct Owned {
    helper: Option<Child>,
    caffeinate: Option<i32>,
}

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(mut helper) = self.helper.take() {
            let _ = helper.kill();
            let _ = helper.wait();
        }
        if let Some(pid) = self.caffeinate.take() {
            if args_of(pid).is_some_and(|a| is_caffeinate(&a)) {
                // SAFETY: signalling a pid this test started.
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
        }
    }
}

/// Direct `caffeinate` children of `parent`. `pgrep` reads the kernel table;
/// `ps -ax` can be filtered in sandboxed sessions.
fn caffeinate_children(parent: u32) -> Vec<i32> {
    let out = Command::new("pgrep")
        .args(["-x", "-P", &parent.to_string(), "caffeinate"])
        .output()
        .expect("pgrep");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

fn args_of(pid: i32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-ww", "-o", "args=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let args = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!args.is_empty()).then_some(args)
}

fn is_caffeinate(args: &str) -> bool {
    args.split_whitespace()
        .next()
        .is_some_and(|prog| prog.rsplit('/').next() == Some("caffeinate"))
}

/// Exact `-w <pid>` match, so caffeinates of parallel tests never count.
fn waits_on(args: &str, pid: u32) -> bool {
    let tokens: Vec<_> = args.split_whitespace().collect();
    tokens
        .windows(2)
        .any(|w| w[0] == "-w" && w[1] == pid.to_string())
}

fn poll<T>(limit: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    loop {
        if let Some(found) = probe() {
            return Some(found);
        }
        if start.elapsed() >= limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn caffeinate_dies_with_a_sigkilled_lease_holder() {
    caffeinate_dies_with_its_lease_holder(End::SigKill);
}

#[test]
fn caffeinate_dies_with_a_lease_holder_that_exits_normally() {
    caffeinate_dies_with_its_lease_holder(End::NormalExit);
}

fn spawn_helper(mode: &str, scratch: &std::path::Path) -> Owned {
    // libtest drops the crate name from `module_path!()`; `--exact` matches the rest.
    let helper_test = match module_path!().split_once("::") {
        Some((_, module)) => format!("{module}::helper_holds_a_power_lease"),
        None => "helper_holds_a_power_lease".to_string(),
    };
    let helper = Command::new(std::env::current_exe().unwrap())
        .args([
            helper_test.as_str(),
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HELPER_ENV, mode)
        .env(LEAK_ENV, scratch.join(SENTINEL))
        .env("KRONN_DATA_DIR", scratch)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn helper");
    Owned {
        helper: Some(helper),
        caffeinate: None,
    }
}

/// Wait until READY is printed, at most 30 s.
fn wait_ready(owned: &mut Owned) {
    let stdout = owned.helper.as_mut().unwrap().stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let ready = BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            // libtest prints "test <name> ... " on the same line before it.
            .any(|line| line.trim_end().ends_with(READY));
        let _ = tx.send(ready);
    });
    let ready = rx.recv_timeout(Duration::from_secs(30)).unwrap_or(false);
    assert!(ready, "helper never reported holding the lease");
}

/// End the helper as `end` asks and reap it before `limit`. The child stays
/// in `owned` until reaped, so a failed signal still leaves it to the guard;
/// on timeout only this helper is killed and reaped.
fn end_helper(
    owned: &mut Owned,
    end: End,
    limit: Duration,
) -> Result<std::process::ExitStatus, String> {
    let helper = owned.helper.as_mut().expect("helper already reaped");
    match end {
        End::SigKill => helper.kill().map_err(|e| format!("SIGKILL helper: {e}"))?,
        End::NormalExit => {
            let mut stdin = helper.stdin.take().expect("helper stdin");
            stdin
                .write_all(b"exit\n")
                .map_err(|e| format!("ask helper to exit: {e}"))?;
        }
    }
    let deadline = Instant::now() + limit;
    loop {
        match helper.try_wait() {
            Ok(Some(status)) => {
                owned.helper = None;
                return Ok(status);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = helper.kill();
                let _ = helper.wait();
                owned.helper = None;
                return Err(format!("helper did not exit within {limit:?} ({end:?})"));
            }
            Err(e) => return Err(format!("try_wait on helper: {e}")),
        }
    }
}

fn caffeinate_dies_with_its_lease_holder(end: End) {
    let scratch = tempfile::tempdir().unwrap();
    let mut owned = spawn_helper("1", scratch.path());
    let helper_pid = owned.helper.as_ref().unwrap().id();

    // The lease is taken before READY is printed, and spawn() returns after
    // caffeinate's exec, so READY is the synchronisation point.
    wait_ready(&mut owned);

    let caf_pid = poll(Duration::from_secs(5), || {
        caffeinate_children(helper_pid).first().copied()
    })
    .expect("the lease holder never started caffeinate");
    let caf_args = args_of(caf_pid).unwrap_or_default();
    owned.caffeinate = Some(caf_pid);

    let lsof = Command::new("lsof")
        .args(["-a", "-p", &caf_pid.to_string(), "-Fn"])
        .output()
        .expect("lsof");
    let open = String::from_utf8_lossy(&lsof.stdout);
    // An empty listing proves nothing: require lsof to have read this very
    // process and listed at least one name.
    assert!(
        lsof.status.success()
            && open.lines().any(|l| l == format!("p{caf_pid}"))
            && open.lines().any(|l| l.starts_with('n')),
        "lsof could not list caffeinate {caf_pid}: {}\n{open}",
        lsof.status
    );
    assert!(
        !open.contains(SENTINEL),
        "caffeinate inherited the helper's non-CLOEXEC descriptor:\n{open}"
    );

    let status = end_helper(&mut owned, end, Duration::from_secs(10)).unwrap();
    if let End::NormalExit = end {
        assert!(status.success(), "helper did not exit normally: {status}");
    }

    let gone = poll(Duration::from_secs(10), || {
        (!args_of(caf_pid).is_some_and(|a| is_caffeinate(&a))).then_some(())
    });
    assert!(
        gone.is_some(),
        "caffeinate {caf_pid} ({caf_args}) survived its parent {helper_pid} ({end:?})"
    );
    owned.caffeinate = None;
    assert!(
        waits_on(&caf_args, helper_pid),
        "caffeinate was not tied to its parent: {caf_args}"
    );
}

/// Control for `end_helper`: a holder that ignores the exit request makes the
/// wait fail at its deadline, and the helper is killed and reaped, not leaked.
#[test]
fn a_helper_ignoring_the_exit_request_fails_fast() {
    let scratch = tempfile::tempdir().unwrap();
    let mut owned = spawn_helper(IGNORE_EXIT, scratch.path());
    let helper_pid = owned.helper.as_ref().unwrap().id() as i32;
    wait_ready(&mut owned);

    let limit = Duration::from_secs(1);
    let start = Instant::now();
    let outcome = end_helper(&mut owned, End::NormalExit, limit);
    assert!(outcome.is_err(), "the ignoring helper exited: {outcome:?}");
    assert!(
        start.elapsed() < limit + Duration::from_secs(5),
        "the bounded wait took {:?}",
        start.elapsed()
    );
    assert!(
        owned.helper.is_none(),
        "the timed-out helper was not reaped"
    );
    // SAFETY: probe only (signal 0) on the pid this test spawned and reaped.
    let alive = unsafe { libc::kill(helper_pid, 0) } == 0;
    assert!(!alive, "the timed-out helper {helper_pid} is still alive");
}
