//! Shared, refcounted power assertion (TD-20260717-run-power-assertion-sleep).
//!
//! Keeps the host awake while **≥1 run is active** so a laptop that sleeps
//! mid-run no longer freezes runs or kills them with a `__guard_timeout__`.
//! On macOS the default `pmset` sleeps the machine a few minutes after the
//! screen locks; the Claude Code harness masked this by holding its own
//! `caffeinate` during interactive sessions, so detached workflow/agent runs
//! were the ones left exposed.
//!
//! Held via RAII [`PowerLease`] handles from [`acquire`]: the **first** lease
//! engages the OS assertion, the **last** drop releases it — a single global
//! refcount, exactly as the TD prescribes ("≥1 run actif ⇒ assertion tenue").
//! macOS uses `caffeinate -i -w <backend pid>`; other OSes are a no-op for
//! now, behind the same seam so a `systemd-inhibit` /
//! `SetThreadExecutionState` impl can slot in.

use std::sync::{Mutex, OnceLock};

/// Platform action that actually prevents / permits idle system sleep. The
/// refcount manager calls `engage` only on the 0→1 transition and `release`
/// only on 1→0, so implementations never have to refcount themselves.
trait SleepInhibitor: Send {
    fn engage(&mut self);
    fn release(&mut self);
}

/// Refcount around a single inhibitor. Generic over the inhibitor so the
/// transition logic is unit-testable without spawning a real `caffeinate`.
struct Refcounted<I: SleepInhibitor> {
    count: usize,
    inhibitor: I,
}

impl<I: SleepInhibitor> Refcounted<I> {
    fn new(inhibitor: I) -> Self {
        Self {
            count: 0,
            inhibitor,
        }
    }

    fn acquire(&mut self) {
        self.count += 1;
        if self.count == 1 {
            self.inhibitor.engage();
        }
    }

    fn release(&mut self) {
        // Underflow-safe: a lease that never incremented (e.g. acquire hit a
        // poisoned lock) must not drive the count below zero.
        if self.count == 0 {
            return;
        }
        self.count -= 1;
        if self.count == 0 {
            self.inhibitor.release();
        }
    }
}

/// Real macOS inhibitor: a `caffeinate -i` child that prevents idle system
/// sleep for as long as it lives. No-op on other platforms.
#[derive(Default)]
struct Caffeinate {
    child: Option<std::process::Child>,
}

/// `caffeinate` arguments: `-w` ties it to `parent`, so it exits with the
/// backend however the backend dies (the static manager is never dropped).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn caffeinate_args(parent: u32) -> [String; 3] {
    ["-i".to_string(), "-w".to_string(), parent.to_string()]
}

#[cfg(target_os = "macos")]
fn caffeinate_command() -> std::process::Command {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    let mut cmd = crate::core::cmd::sync_tool_cmd("caffeinate");
    cmd.args(caffeinate_args(std::process::id()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Descriptors the backend itself inherited without CLOEXEC (a parent's
    // lock file) would otherwise be kept open by caffeinate after we exit.
    let ceiling = fd_ceiling();
    // SAFETY: the hook only calls fcntl, which is async-signal-safe.
    unsafe {
        cmd.pre_exec(move || {
            mark_extra_fds_cloexec(ceiling);
            Ok(())
        });
    }
    cmd
}

/// Upper bound (exclusive) for open descriptors, read before `fork` because
/// `sysconf` is not async-signal-safe.
#[cfg(target_os = "macos")]
fn fd_ceiling() -> libc::c_int {
    // SAFETY: plain libc query with no pointer arguments.
    let max = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    if max <= 0 {
        10_240
    } else {
        max.min(1 << 20) as libc::c_int
    }
}

/// Set CLOEXEC on every descriptor above stderr rather than closing them:
/// std's own exec-error pipe lives there and must stay usable until `exec`.
#[cfg(target_os = "macos")]
fn mark_extra_fds_cloexec(ceiling: libc::c_int) {
    for fd in 3..ceiling {
        // SAFETY: fcntl on any integer is defined; EBADF just returns -1.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags != -1 && flags & libc::FD_CLOEXEC == 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
}

impl SleepInhibitor for Caffeinate {
    fn engage(&mut self) {
        #[cfg(target_os = "macos")]
        {
            self.child = caffeinate_command().spawn().ok();
        }
    }

    fn release(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn manager() -> &'static Mutex<Refcounted<Caffeinate>> {
    static MANAGER: OnceLock<Mutex<Refcounted<Caffeinate>>> = OnceLock::new();
    MANAGER.get_or_init(|| Mutex::new(Refcounted::new(Caffeinate::default())))
}

/// RAII handle keeping the shared power assertion alive. Hold one for the
/// whole lifetime of an active run (workflow, detached agent, audit); drop it
/// on every exit path and the assertion is released once the last run ends.
#[must_use = "dropping the lease immediately releases the shared power assertion"]
pub struct PowerLease {
    /// True only if this lease actually incremented the refcount, so a lease
    /// born from a failed acquire never decrements on drop.
    acquired: bool,
}

/// Take a lease on the shared power assertion. The first live lease engages
/// the OS assertion; further leases just bump the refcount. Best-effort: a
/// poisoned lock degrades to a no-op lease rather than panicking.
pub fn acquire() -> PowerLease {
    let acquired = manager()
        .lock()
        .map(|mut m| {
            m.acquire();
            true
        })
        .unwrap_or(false);
    PowerLease { acquired }
}

impl Drop for PowerLease {
    fn drop(&mut self) {
        if !self.acquired {
            return;
        }
        if let Ok(mut m) = manager().lock() {
            m.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Test inhibitor recording engage/release calls and current on/off state.
    #[derive(Clone, Default)]
    struct Counting {
        engaged: Arc<AtomicUsize>,
        released: Arc<AtomicUsize>,
    }

    impl SleepInhibitor for Counting {
        fn engage(&mut self) {
            self.engaged.fetch_add(1, Ordering::SeqCst);
        }
        fn release(&mut self) {
            self.released.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn engages_only_on_first_acquire_and_releases_only_on_last() {
        let probe = Counting::default();
        let mut rc = Refcounted::new(probe.clone());

        rc.acquire(); // 0 -> 1: engage
        assert_eq!(probe.engaged.load(Ordering::SeqCst), 1);
        assert_eq!(probe.released.load(Ordering::SeqCst), 0);

        rc.acquire(); // 1 -> 2: no re-engage
        rc.acquire(); // 2 -> 3
        assert_eq!(probe.engaged.load(Ordering::SeqCst), 1);

        rc.release(); // 3 -> 2: no release
        rc.release(); // 2 -> 1
        assert_eq!(probe.released.load(Ordering::SeqCst), 0);

        rc.release(); // 1 -> 0: release
        assert_eq!(probe.released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn re_engages_after_full_release() {
        let probe = Counting::default();
        let mut rc = Refcounted::new(probe.clone());

        rc.acquire();
        rc.release();
        rc.acquire(); // second cycle re-engages
        assert_eq!(probe.engaged.load(Ordering::SeqCst), 2);
        assert_eq!(probe.released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn release_underflow_is_a_noop() {
        let probe = Counting::default();
        let mut rc = Refcounted::new(probe.clone());

        rc.release(); // count already 0: must not touch the inhibitor
        rc.release();
        assert_eq!(probe.engaged.load(Ordering::SeqCst), 0);
        assert_eq!(probe.released.load(Ordering::SeqCst), 0);
        assert_eq!(rc.count, 0);
    }

    #[test]
    fn caffeinate_waits_on_the_backend_pid() {
        assert_eq!(caffeinate_args(4242), ["-i", "-w", "4242"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn caffeinate_command_targets_this_process() {
        let cmd = caffeinate_command();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, caffeinate_args(std::process::id()));
    }

    #[test]
    fn global_lease_acquire_release_is_balanced() {
        // Exercises the real global path (no assertion spawned off-macOS).
        {
            let _a = acquire();
            let _b = acquire();
        } // both dropped here
          // A fresh lease afterwards must still work — the refcount returned to 0.
        let _c = acquire();
    }
}
