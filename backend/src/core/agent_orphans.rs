//! KT-1211 (M5) — an audit step's CLI agent can outlive the backend that
//! started it. The agent is its own process-group leader (`setpgid(0, 0)` at
//! spawn) and `kill_on_drop` only runs when the backend exits cleanly: after a
//! crash or a `kill -9` the agent keeps writing the docs while boot frees the
//! lease, so a resume could put a second agent on the same target.
//!
//! The run records the group and the start stamp of its leader. At boot an
//! agent is killed only when the stamp still matches: a pid the system reused
//! for another process is never signalled.

/// What boot found of a recorded agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanOutcome {
    /// The group was still alive and has been sent SIGKILL.
    Killed,
    /// Nothing runs under that pid any more.
    Gone,
    /// The pid is alive but it is not the recorded agent (reused, or the
    /// record has no stamp to compare): left alone.
    NotOurs,
}

/// A stamp that tells one process from the next one to get its pid: the start
/// time as the OS reports it. `None` when it cannot be read.
pub fn start_stamp(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        // Field 22 of /proc/<pid>/stat is the start time in clock ticks since
        // boot; the command name (field 2) may hold spaces, so read after ')'.
        // Ticks restart at every boot, so the boot id goes with them: the same
        // pid and tick count after a reboot is another process.
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
            let after = stat.rsplit_once(')')?.1;
            let ticks = after.split_whitespace().nth(19)?;
            return Some(linux_stamp(
                std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                    .ok()
                    .as_deref(),
                ticks,
            ));
        }
    }
    #[cfg(target_os = "macos")]
    {
        // The kernel's own answer: no subprocess to spawn (or to be denied).
        // SAFETY: `info` is a zeroed plain-data struct of exactly the size
        // passed; the call writes at most that many bytes into it.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let written = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if written == size {
            return Some(format!("bsd:{}.{}", info.pbi_start_tvsec, info.pbi_start_tvusec));
        }
    }
    let out = crate::core::cmd::sync_tool_cmd("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let stamp = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !stamp.is_empty()).then(|| format!("ps:{}", stamp))
}

/// `proc:<boot id>:<ticks>` (`proc:<ticks>` when the boot id is unreadable).
#[cfg(any(target_os = "linux", test))]
fn linux_stamp(boot_id: Option<&str>, ticks: &str) -> String {
    match boot_id.map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) => format!("proc:{id}:{ticks}"),
        None => format!("proc:{ticks}"),
    }
}

/// Kill the group of a recorded agent when its leader is still the recorded
/// process. `stamp_of` reads the current stamp of a pid (injected for tests).
#[cfg(unix)]
pub fn kill_recorded_group(
    pgid: i64,
    recorded: Option<&str>,
    stamp_of: impl Fn(u32) -> Option<String>,
) -> OrphanOutcome {
    let Ok(pid) = u32::try_from(pgid) else {
        return OrphanOutcome::Gone;
    };
    if pid <= 1 {
        return OrphanOutcome::Gone;
    }
    let Some(current) = stamp_of(pid) else {
        // No process answers at that pid (or none can be inspected).
        return if group_alive(pgid as i32) {
            OrphanOutcome::NotOurs
        } else {
            OrphanOutcome::Gone
        };
    };
    if recorded != Some(current.as_str()) {
        return OrphanOutcome::NotOurs;
    }
    // SAFETY: a plain signal to the group `setpgid(0, 0)` made at spawn, sent
    // only after the leader's start stamp matched the recorded one.
    let sent = unsafe { libc::kill(-(pgid as i32), libc::SIGKILL) };
    if sent == 0 {
        OrphanOutcome::Killed
    } else {
        OrphanOutcome::Gone
    }
}

#[cfg(not(unix))]
pub fn kill_recorded_group(
    _pgid: i64,
    _recorded: Option<&str>,
    _stamp_of: impl Fn(u32) -> Option<String>,
) -> OrphanOutcome {
    OrphanOutcome::Gone
}

#[cfg(unix)]
fn group_alive(pgid: i32) -> bool {
    // SAFETY: signal 0 only checks that the group exists.
    unsafe { libc::kill(-pgid, 0) == 0 }
}

/// Boot: kill the agents the previous backend left, then forget them.
/// Returns how many were killed.
pub fn reap_recorded_agents(conn: &rusqlite::Connection) -> anyhow::Result<usize> {
    let recorded = crate::db::audit_runs::recorded_agent_processes(conn)?;
    let mut killed = 0;
    for (run_id, pgid, marker) in recorded {
        let outcome = kill_recorded_group(pgid, marker.as_deref(), start_stamp);
        match outcome {
            OrphanOutcome::Killed => {
                killed += 1;
                tracing::warn!(
                    "Boot: killed the agent (process group {}) the previous backend left running for audit run {}",
                    pgid,
                    run_id
                );
            }
            OrphanOutcome::NotOurs => tracing::warn!(
                "Boot: process {} recorded for audit run {} is not that agent any more — left alone",
                pgid,
                run_id
            ),
            OrphanOutcome::Gone => {}
        }
        crate::db::audit_runs::clear_agent_process(conn, &run_id)?;
    }
    Ok(killed)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    fn spawn_group_leader() -> std::process::Child {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("60");
        // SAFETY: setpgid in the forked child, like the agent spawn does.
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
        cmd.spawn().expect("sleep")
    }

    #[test]
    fn a_recorded_agent_with_a_matching_stamp_is_killed() {
        let mut child = spawn_group_leader();
        let pid = child.id();
        let stamp = start_stamp(pid);
        assert!(stamp.is_some(), "the platform must be able to stamp a process");
        let outcome = kill_recorded_group(pid as i64, stamp.as_deref(), start_stamp);
        assert_eq!(outcome, OrphanOutcome::Killed);
        let status = child.wait().unwrap();
        assert!(!status.success(), "the agent was killed: {status:?}");
    }

    #[test]
    fn a_reused_pid_is_never_signalled() {
        let mut child = spawn_group_leader();
        let outcome =
            kill_recorded_group(child.id() as i64, Some("ps:a different process"), start_stamp);
        assert_eq!(outcome, OrphanOutcome::NotOurs);
        assert!(child.try_wait().unwrap().is_none(), "still alive");
        // A record without a stamp cannot prove it either.
        assert_eq!(
            kill_recorded_group(child.id() as i64, None, start_stamp),
            OrphanOutcome::NotOurs
        );
        assert!(child.try_wait().unwrap().is_none(), "still alive");
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_gone_agent_is_reported_gone() {
        let mut child = spawn_group_leader();
        let pid = child.id();
        let stamp = start_stamp(pid);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            kill_recorded_group(pid as i64, stamp.as_deref(), start_stamp),
            OrphanOutcome::Gone
        );
    }

    #[test]
    fn a_linux_stamp_carries_the_boot_id_so_a_reboot_never_matches() {
        let before = linux_stamp(Some("boot-a\n"), "4242");
        assert_eq!(before, "proc:boot-a:4242");
        // Same pid, same tick count, another boot: another process.
        assert_ne!(before, linux_stamp(Some("boot-b"), "4242"));
        assert_eq!(linux_stamp(None, "4242"), "proc:4242");
        assert_eq!(linux_stamp(Some("  "), "4242"), "proc:4242");
        // A recorded stamp from another boot is not ours.
        let mut child = spawn_group_leader();
        let outcome = kill_recorded_group(child.id() as i64, Some(&before), start_stamp);
        assert_eq!(outcome, OrphanOutcome::NotOurs);
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
