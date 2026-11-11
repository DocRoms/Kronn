//! Background priority for the processes Kronn spawns on a worker's behalf
//! (KT-1633).
//!
//! A task worker, and the validations that build and test its tree, can keep
//! every core busy for minutes. Nothing used to lower their priority, so the
//! owner's interactive applications competed with them. A launcher in front of
//! the program keeps the host and Kronn's own UI responsive: it sets the
//! policy and `exec`s the program, so the process id (and the process group the
//! callers kill) is the program's own, and its children inherit the policy.
//!
//! | Host    | Mechanism                                          |
//! |---------|----------------------------------------------------|
//! | macOS   | `taskpolicy -b` (background QoS: CPU, I/O, network) |
//! | Linux   | `nice -n 10`                                        |
//! | Windows | `BELOW_NORMAL_PRIORITY_CLASS` creation flag         |
//!
//! The backend itself and the UI never go through here: only the call sites
//! that ask for [`Priority::Background`] do. The owner turns the whole mechanism
//! off in Settings > Workers ([`set_enabled`]) or with
//! [`BACKGROUND_PRIORITY_ENV`] set to `0` / `false` / `off` / `no`.
//!
//! Whether the host lets the launcher run is asked once, at boot, off the async
//! path ([`check_launcher_at_boot`]); a spawn only reads the stored answer.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Environment switch of the mechanism. Unset, empty or any other value keeps
/// it on; `0`, `false`, `off` or `no` (any case) turns it off.
pub const BACKGROUND_PRIORITY_ENV: &str = "KRONN_BACKGROUND_PRIORITY";

/// `nice` increment of a Linux background process: noticeably behind the
/// owner's interactive work, still above an idle-class job.
pub const LINUX_NICE_INCREMENT: &str = "10";

/// Windows `BELOW_NORMAL_PRIORITY_CLASS` process creation flag.
#[cfg(target_os = "windows")]
pub const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// How urgently a spawned process wants the CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    /// The process inherits the backend's own priority.
    #[default]
    Normal,
    /// A task worker (first dispatch and re-dispatch after a review), a
    /// validation or a workflow Agent / Exec step: it yields to the owner's
    /// interactive applications, unless the owner turned that off.
    Background,
}

/// The host families the launcher differs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    MacOs,
    Linux,
    Windows,
    Other,
}

impl HostOs {
    pub const CURRENT: HostOs = if cfg!(target_os = "macos") {
        HostOs::MacOs
    } else if cfg!(target_os = "linux") {
        HostOs::Linux
    } else if cfg!(target_os = "windows") {
        HostOs::Windows
    } else {
        HostOs::Other
    };
}

/// A program that lowers its own priority, then `exec`s the argv that follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Launcher {
    pub program: &'static str,
    pub args: &'static [&'static str],
}

const TASKPOLICY: Launcher = Launcher {
    program: "/usr/sbin/taskpolicy",
    args: &["-b"],
};

/// Absolute paths: the child's `PATH` is a built one, and a launcher found
/// through it could be a repository's.
const NICE_CANDIDATES: &[&str] = &["/usr/bin/nice", "/bin/nice"];

const NICE_ARGS: &[&str] = &["-n", LINUX_NICE_INCREMENT];

/// Whether `value` (the content of [`BACKGROUND_PRIORITY_ENV`]) turns the
/// mechanism off.
pub fn disabled_by(value: Option<&std::ffi::OsStr>) -> bool {
    value.and_then(|v| v.to_str()).is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

/// The Settings > Workers switch (`workers.background_priority`), published by
/// `config::save` and at boot. On until the config says otherwise.
static SETTING_ON: AtomicBool = AtomicBool::new(true);

/// Publish the owner's Settings > Workers choice to this process.
pub fn set_enabled(on: bool) {
    SETTING_ON.store(on, Ordering::Relaxed);
}

/// Whether the owner left the mechanism on: the Settings switch and the
/// environment variable must both allow it.
pub fn enabled() -> bool {
    enabled_with(
        SETTING_ON.load(Ordering::Relaxed),
        crate::core::child_env::var_os(BACKGROUND_PRIORITY_ENV).as_deref(),
    )
}

/// [`enabled`] for a given Settings value and environment value.
pub fn enabled_with(setting: bool, env: Option<&std::ffi::OsStr>) -> bool {
    setting && !disabled_by(env)
}

/// The launcher `os` puts in front of a background program, given which
/// absolute paths exist. `None` when the host has none (Windows sets a flag
/// instead; a host without the binary runs the program at normal priority
/// rather than not at all).
pub fn launcher_for(os: HostOs, installed: impl Fn(&str) -> bool) -> Option<Launcher> {
    match os {
        HostOs::MacOs => installed(TASKPOLICY.program).then_some(TASKPOLICY),
        HostOs::Linux => NICE_CANDIDATES
            .iter()
            .copied()
            .find(|path| installed(path))
            .map(|program| Launcher {
                program,
                args: NICE_ARGS,
            }),
        HostOs::Windows | HostOs::Other => None,
    }
}

/// What a spawn with `priority` does, whether the owner's setting is `enabled`.
/// Pure: the host is a parameter.
pub fn plan(
    priority: Priority,
    enabled: bool,
    os: HostOs,
    installed: impl Fn(&str) -> bool,
) -> Plan {
    if priority != Priority::Background || !enabled {
        return Plan::default();
    }
    Plan {
        launcher: launcher_for(os, installed),
        below_normal_class: os == HostOs::Windows,
    }
}

/// [`plan`] for this host and this process's setting. A launcher the host
/// refuses to run (a sandboxed backend may not lower a priority) is dropped:
/// a worker at normal priority beats a worker that never starts. Reads the
/// stored answer of [`check_launcher_at_boot`]; never starts a process on the
/// caller's thread.
pub fn plan_here(priority: Priority) -> Plan {
    if priority != Priority::Background {
        return Plan::default();
    }
    let chosen = plan(priority, enabled(), HostOs::CURRENT, |path| {
        Path::new(path).is_file()
    });
    if chosen.launcher.is_some() && LAUNCHER.get() == LauncherCheck::Unchecked {
        // Boot could not tell (or has not run): ask again, off this thread.
        recheck_later();
    }
    apply_check(chosen, LAUNCHER.get())
}

/// What the host's launcher is known to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherCheck {
    /// Not asked yet, or the last attempt was inconclusive.
    Unchecked,
    Works,
    /// The host definitively refuses to run it.
    Refused,
}

/// `plan`, with its launcher kept only when the host is known to run it. An
/// unchecked launcher is not used: a worker must never fail to start for it.
pub fn apply_check(mut plan: Plan, check: LauncherCheck) -> Plan {
    if check != LauncherCheck::Works {
        plan.launcher = None;
    }
    plan
}

/// How one probe of the launcher ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeEnd {
    Exited {
        success: bool,
    },
    /// Ended by a signal.
    Killed,
    SpawnFailed(std::io::ErrorKind),
}

/// What a probe settles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Works,
    /// The launcher is missing or ran and refused: stable, worth caching.
    Refused,
    /// A passing failure (no process slot, interrupted, killed): ask again.
    Inconclusive,
}

pub fn verdict(end: ProbeEnd) -> Verdict {
    use std::io::ErrorKind::{NotFound, PermissionDenied};
    match end {
        ProbeEnd::Exited { success: true } => Verdict::Works,
        ProbeEnd::Exited { success: false } => Verdict::Refused,
        ProbeEnd::Killed => Verdict::Inconclusive,
        ProbeEnd::SpawnFailed(NotFound | PermissionDenied) => Verdict::Refused,
        ProbeEnd::SpawnFailed(_) => Verdict::Inconclusive,
    }
}

/// The stored [`LauncherCheck`]: only a definitive verdict changes it.
pub struct LauncherState(AtomicU8);

impl LauncherState {
    const UNCHECKED: u8 = 0;
    const WORKS: u8 = 1;
    const REFUSED: u8 = 2;

    pub const fn new() -> Self {
        Self(AtomicU8::new(Self::UNCHECKED))
    }

    pub fn get(&self) -> LauncherCheck {
        match self.0.load(Ordering::Relaxed) {
            Self::WORKS => LauncherCheck::Works,
            Self::REFUSED => LauncherCheck::Refused,
            _ => LauncherCheck::Unchecked,
        }
    }

    #[cfg(test)]
    fn force(&self, check: LauncherCheck) {
        let stored = match check {
            LauncherCheck::Unchecked => Self::UNCHECKED,
            LauncherCheck::Works => Self::WORKS,
            LauncherCheck::Refused => Self::REFUSED,
        };
        self.0.store(stored, Ordering::Relaxed);
    }

    /// Keep a definitive answer; leave an inconclusive one unrecorded so the
    /// next attempt is made.
    pub fn record(&self, verdict: Verdict) {
        let stored = match verdict {
            Verdict::Works => Self::WORKS,
            Verdict::Refused => Self::REFUSED,
            Verdict::Inconclusive => return,
        };
        self.0.store(stored, Ordering::Relaxed);
    }
}

impl Default for LauncherState {
    fn default() -> Self {
        Self::new()
    }
}

static LAUNCHER: LauncherState = LauncherState::new();

/// Attempts of one boot probe, and the wait between two.
const PROBE_ATTEMPTS: usize = 3;
const PROBE_BACKOFF: Duration = Duration::from_millis(150);
/// Minimum time between two background re-checks of an unchecked launcher.
const RECHECK_EVERY: Duration = Duration::from_secs(30);

static LAST_RECHECK: Mutex<Option<Instant>> = Mutex::new(None);

/// Ask the host once, on a blocking thread: the launcher runs `true`. Called
/// at boot, before any worker can be dispatched.
pub async fn check_launcher_at_boot() {
    let _ = tokio::task::spawn_blocking(check_launcher_blocking).await;
}

/// [`check_launcher_at_boot`]'s body: blocks on a child process, so it never
/// runs on an async worker thread.
pub fn check_launcher_blocking() {
    let Some(launcher) = launcher_for(HostOs::CURRENT, |path| Path::new(path).is_file()) else {
        return;
    };
    for attempt in 0..PROBE_ATTEMPTS {
        match probe_launcher(launcher) {
            Verdict::Works => {
                LAUNCHER.record(Verdict::Works);
                return;
            }
            Verdict::Refused => {
                LAUNCHER.record(Verdict::Refused);
                tracing::warn!(
                    "{} cannot lower a priority here: workers and validations keep the backend's priority ({BACKGROUND_PRIORITY_ENV}=0 silences this)",
                    launcher.program
                );
                return;
            }
            Verdict::Inconclusive if attempt + 1 < PROBE_ATTEMPTS => {
                std::thread::sleep(PROBE_BACKOFF);
            }
            Verdict::Inconclusive => {}
        }
    }
    tracing::warn!(
        "could not tell whether {} can lower a priority; workers keep the backend's priority until a later check succeeds",
        launcher.program
    );
}

/// Run the launcher once on a program that exits 0.
fn probe_launcher(launcher: Launcher) -> Verdict {
    let Some(program) = probe_program() else {
        return Verdict::Inconclusive;
    };
    let end = match crate::core::cmd::sync_tool_cmd(launcher.program)
        .args(launcher.args)
        .arg(program)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.code().is_some() => ProbeEnd::Exited {
            success: status.success(),
        },
        Ok(_) => ProbeEnd::Killed,
        Err(error) => ProbeEnd::SpawnFailed(error.kind()),
    };
    verdict(end)
}

/// Re-check on a thread of its own, at most every [`RECHECK_EVERY`].
fn recheck_later() {
    claim_recheck(&LAST_RECHECK, Instant::now(), RECHECK_EVERY, || {
        std::thread::spawn(check_launcher_blocking);
    });
}

/// Run `start` when no re-check began within `every` before `now`, and record
/// `now` as the latest one. Returns whether it ran: the throttle, apart from
/// the thread it starts.
fn claim_recheck(
    last: &Mutex<Option<Instant>>,
    now: Instant,
    every: Duration,
    start: impl FnOnce(),
) -> bool {
    let due = {
        let mut last = last.lock().unwrap_or_else(|p| p.into_inner());
        let due = last.is_none_or(|at| now.saturating_duration_since(at) >= every);
        if due {
            *last = Some(now);
        }
        due
    };
    if due {
        start();
    }
    due
}

/// Test seam: the stored answer of the launcher check, restored on drop. A test
/// holding it must be `#[serial_test::serial(background_launcher)]`: the state
/// is process-wide.
#[cfg(test)]
pub struct LauncherGuard(LauncherCheck);

#[cfg(test)]
impl LauncherGuard {
    pub fn set(check: LauncherCheck) -> Self {
        let previous = LAUNCHER.get();
        LAUNCHER.force(check);
        Self(previous)
    }
}

#[cfg(test)]
impl Drop for LauncherGuard {
    fn drop(&mut self) {
        LAUNCHER.force(self.0);
    }
}

/// A program every host with a launcher has, which exits 0.
fn probe_program() -> Option<&'static str> {
    ["/usr/bin/true", "/bin/true"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
}

/// How one spawn is made background.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Plan {
    /// Put in front of the program, which becomes its first argument.
    pub launcher: Option<Launcher>,
    /// Windows: add `BELOW_NORMAL_PRIORITY_CLASS` to the creation flags.
    pub below_normal_class: bool,
}

impl Plan {
    /// `program` and `args` behind the launcher. `program` itself when there is
    /// none.
    pub fn argv(&self, program: &std::ffi::OsStr) -> (std::ffi::OsString, Vec<std::ffi::OsString>) {
        match self.launcher {
            // A missing program is spawned as is: behind the launcher its
            // spawn error would become the launcher's exit 127.
            Some(launcher) if which::which(program).is_ok() => {
                let mut args: Vec<std::ffi::OsString> =
                    launcher.args.iter().map(Into::into).collect();
                args.push(program.to_os_string());
                (launcher.program.into(), args)
            }
            _ => (program.to_os_string(), Vec::new()),
        }
    }
}

// Not behind `full-tests`: the macOS portability leg builds without it and must
// still prove the `taskpolicy -b` argv.
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn everything_installed(_: &str) -> bool {
        true
    }

    fn nothing_installed(_: &str) -> bool {
        false
    }

    #[test]
    fn macos_runs_a_background_program_under_taskpolicy_dash_b() {
        let plan = plan(
            Priority::Background,
            true,
            HostOs::MacOs,
            everything_installed,
        );
        let (program, args) = plan.argv(OsStr::new("cargo"));
        assert_eq!(program, OsStr::new("/usr/sbin/taskpolicy"));
        assert_eq!(args, [OsStr::new("-b"), OsStr::new("cargo")]);
        assert!(!plan.below_normal_class);
    }

    #[test]
    fn linux_runs_a_background_program_under_nice_ten() {
        let plan = plan(
            Priority::Background,
            true,
            HostOs::Linux,
            everything_installed,
        );
        let (program, args) = plan.argv(OsStr::new("cargo"));
        assert_eq!(program, OsStr::new("/usr/bin/nice"));
        assert_eq!(
            args,
            [OsStr::new("-n"), OsStr::new("10"), OsStr::new("cargo")]
        );
    }

    #[test]
    fn linux_falls_back_to_the_second_nice_location() {
        let only_bin = |path: &str| path == "/bin/nice";
        let plan = plan(Priority::Background, true, HostOs::Linux, only_bin);
        assert_eq!(plan.launcher.map(|l| l.program), Some("/bin/nice"));
    }

    #[test]
    fn windows_lowers_the_class_instead_of_prefixing_the_program() {
        let plan = plan(
            Priority::Background,
            true,
            HostOs::Windows,
            everything_installed,
        );
        assert!(plan.below_normal_class);
        assert_eq!(plan.launcher, None);
        let (program, args) = plan.argv(OsStr::new("cargo"));
        assert_eq!(program, OsStr::new("cargo"));
        assert!(args.is_empty());
    }

    #[test]
    fn a_normal_priority_spawn_is_never_wrapped() {
        for os in [HostOs::MacOs, HostOs::Linux, HostOs::Windows, HostOs::Other] {
            let plan = plan(Priority::Normal, true, os, everything_installed);
            assert_eq!(plan, Plan::default(), "{os:?}");
            let (program, args) = plan.argv(OsStr::new("kronn"));
            assert_eq!(program, OsStr::new("kronn"));
            assert!(args.is_empty());
        }
    }

    #[test]
    fn a_missing_program_is_not_hidden_behind_the_launcher() {
        for os in [HostOs::MacOs, HostOs::Linux] {
            let plan = plan(Priority::Background, true, os, everything_installed);
            assert!(plan.launcher.is_some(), "{os:?}");
            let (program, args) = plan.argv(OsStr::new("kronn_nonexistent_test_binary"));
            assert_eq!(
                program,
                OsStr::new("kronn_nonexistent_test_binary"),
                "{os:?}"
            );
            assert!(args.is_empty(), "{os:?}");
        }
    }

    #[test]
    fn the_owners_switch_turns_every_host_back_to_normal() {
        for os in [HostOs::MacOs, HostOs::Linux, HostOs::Windows, HostOs::Other] {
            assert_eq!(
                plan(Priority::Background, false, os, everything_installed),
                Plan::default(),
                "{os:?}"
            );
        }
    }

    #[test]
    fn a_host_without_the_launcher_runs_the_program_rather_than_refusing_it() {
        for os in [HostOs::MacOs, HostOs::Linux] {
            let plan = plan(Priority::Background, true, os, nothing_installed);
            assert_eq!(plan.launcher, None, "{os:?}");
        }
    }

    #[test]
    fn only_the_documented_values_turn_the_mechanism_off() {
        for off in ["0", "false", "FALSE", "off", "Off", "no", " 0 "] {
            assert!(disabled_by(Some(OsStr::new(off))), "{off:?}");
        }
        for on in ["1", "true", "on", "yes", "", "garbage"] {
            assert!(!disabled_by(Some(OsStr::new(on))), "{on:?}");
        }
        assert!(!disabled_by(None));
    }

    #[test]
    fn the_priority_of_a_spawn_defaults_to_normal() {
        assert_eq!(Priority::default(), Priority::Normal);
    }

    #[test]
    fn the_settings_switch_and_the_environment_must_both_allow_it() {
        assert!(enabled_with(true, None));
        assert!(enabled_with(true, Some(OsStr::new("1"))));
        assert!(!enabled_with(false, None));
        assert!(!enabled_with(true, Some(OsStr::new("0"))));
        assert!(!enabled_with(false, Some(OsStr::new("off"))));
    }

    /// The Settings value reaches `enabled()` and, through it, the plan.
    #[test]
    #[serial_test::serial(background_launcher)]
    fn publishing_the_setting_turns_a_background_spawn_back_to_normal() {
        set_enabled(false);
        assert!(!enabled());
        assert_eq!(plan_here(Priority::Background), Plan::default());
        set_enabled(true);
        assert_eq!(
            enabled(),
            !disabled_by(crate::core::child_env::var_os(BACKGROUND_PRIORITY_ENV).as_deref())
        );
    }

    #[test]
    fn a_launcher_is_used_only_once_the_host_is_known_to_run_it() {
        let wanted = plan(
            Priority::Background,
            true,
            HostOs::MacOs,
            everything_installed,
        );
        assert!(wanted.launcher.is_some());
        assert_eq!(apply_check(wanted, LauncherCheck::Works), wanted);
        assert_eq!(apply_check(wanted, LauncherCheck::Refused).launcher, None);
        assert_eq!(apply_check(wanted, LauncherCheck::Unchecked).launcher, None);
        let windows = plan(
            Priority::Background,
            true,
            HostOs::Windows,
            everything_installed,
        );
        assert!(apply_check(windows, LauncherCheck::Refused).below_normal_class);
    }

    #[test]
    fn only_a_definitive_probe_result_is_stored() {
        use std::io::ErrorKind;
        assert_eq!(verdict(ProbeEnd::Exited { success: true }), Verdict::Works);
        assert_eq!(
            verdict(ProbeEnd::Exited { success: false }),
            Verdict::Refused
        );
        assert_eq!(
            verdict(ProbeEnd::SpawnFailed(ErrorKind::NotFound)),
            Verdict::Refused
        );
        assert_eq!(
            verdict(ProbeEnd::SpawnFailed(ErrorKind::PermissionDenied)),
            Verdict::Refused
        );
        for transient in [
            ProbeEnd::Killed,
            ProbeEnd::SpawnFailed(ErrorKind::WouldBlock),
            ProbeEnd::SpawnFailed(ErrorKind::Interrupted),
            ProbeEnd::SpawnFailed(ErrorKind::OutOfMemory),
        ] {
            assert_eq!(verdict(transient), Verdict::Inconclusive, "{transient:?}");
        }

        let state = LauncherState::new();
        assert_eq!(state.get(), LauncherCheck::Unchecked);
        state.record(Verdict::Inconclusive);
        assert_eq!(
            state.get(),
            LauncherCheck::Unchecked,
            "a transient failure is retried"
        );
        state.record(Verdict::Works);
        assert_eq!(state.get(), LauncherCheck::Works);
        state.record(Verdict::Inconclusive);
        assert_eq!(
            state.get(),
            LauncherCheck::Works,
            "a transient failure keeps a known answer"
        );
        state.record(Verdict::Refused);
        assert_eq!(state.get(), LauncherCheck::Refused);
    }

    /// The re-check of an unchecked launcher starts one thread per window, and
    /// none once the answer is known.
    #[test]
    fn an_unchecked_launcher_is_rechecked_once_per_window() {
        let last = Mutex::new(None);
        let started = std::cell::Cell::new(0);
        let start = || started.set(started.get() + 1);
        let t0 = Instant::now();
        let window = RECHECK_EVERY;

        assert!(claim_recheck(&last, t0, window, start), "first ask starts");
        for offset in [
            Duration::ZERO,
            Duration::from_secs(1),
            window - Duration::from_millis(1),
        ] {
            assert!(
                !claim_recheck(&last, t0 + offset, window, start),
                "inside the window: {offset:?}"
            );
        }
        assert_eq!(started.get(), 1);
        assert!(
            claim_recheck(&last, t0 + window, window, start),
            "next window"
        );
        assert!(!claim_recheck(
            &last,
            t0 + window + Duration::from_secs(5),
            window,
            start
        ));
        assert_eq!(started.get(), 2);
    }

    /// `plan_here` never asks again once the host's answer is known. (The
    /// unknown case starts a real probe thread that would write the global
    /// state mid-suite; the throttle above covers it.)
    #[test]
    #[serial_test::serial(background_launcher)]
    fn a_known_launcher_is_never_rechecked() {
        if launcher_for(HostOs::CURRENT, |path| Path::new(path).is_file()).is_none() {
            return; // Windows, or a host without the launcher: nothing to check.
        }
        set_enabled(true);
        for known in [LauncherCheck::Works, LauncherCheck::Refused] {
            let _guard = LauncherGuard::set(known);
            *LAST_RECHECK.lock().unwrap() = None;
            let _ = plan_here(Priority::Background);
            assert!(
                LAST_RECHECK.lock().unwrap().is_none(),
                "{known:?}: no re-check was claimed"
            );
        }
    }
}
