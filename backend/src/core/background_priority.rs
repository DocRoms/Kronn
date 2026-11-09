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
//! that ask for [`Priority::Background`] do. [`BACKGROUND_PRIORITY_ENV`] set to
//! `0` / `false` / `off` / `no` turns the whole mechanism off.

use std::path::Path;

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
    /// A worker, a reviewer's re-dispatch or a validation: it yields to the
    /// owner's interactive applications, unless the owner turned that off.
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

/// Whether the owner left the mechanism on.
pub fn enabled() -> bool {
    !disabled_by(crate::core::child_env::var_os(BACKGROUND_PRIORITY_ENV).as_deref())
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
/// a worker at normal priority beats a worker that never starts.
pub fn plan_here(priority: Priority) -> Plan {
    if priority != Priority::Background {
        return Plan::default();
    }
    let mut chosen = plan(priority, enabled(), HostOs::CURRENT, |path| {
        Path::new(path).is_file()
    });
    if let Some(launcher) = chosen.launcher {
        if !launcher_works(launcher) {
            chosen.launcher = None;
        }
    }
    chosen
}

/// Whether `launcher` can start a program on this host. Asked once per
/// process: the answer is a property of the host's policy, not of a spawn.
fn launcher_works(launcher: Launcher) -> bool {
    static WORKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WORKS.get_or_init(|| {
        let works = crate::core::cmd::sync_tool_cmd(launcher.program)
            .args(launcher.args)
            .arg(probe_program())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !works {
            tracing::warn!(
                "{} cannot lower a priority here: workers and validations keep the backend's priority ({BACKGROUND_PRIORITY_ENV}=0 silences this)",
                launcher.program
            );
        }
        works
    })
}

/// A program every host with a launcher has, which exits 0.
fn probe_program() -> &'static str {
    ["/usr/bin/true", "/bin/true"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .unwrap_or("/usr/bin/true")
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
            Some(launcher) => {
                let mut args: Vec<std::ffi::OsString> =
                    launcher.args.iter().map(Into::into).collect();
                args.push(program.to_os_string());
                (launcher.program.into(), args)
            }
            None => (program.to_os_string(), Vec::new()),
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
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
}
