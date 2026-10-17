//! KT-1266 — what a task worker started must not outlive its dispatch.
//!
//! The agent is its own process-group leader and its group is signalled when
//! its run ends, but a worker's shell commands do not all stay in that group:
//! a backend or a Vite started from the agent's shell tool runs in a group of
//! its own and kept listening on the lane's ports after the turn, the sandbox
//! denying the worker the signal to stop it. Process groups are therefore the
//! wrong handle.
//!
//! Every process of a worker's run inherits [`DISPATCH_ENV`] (the id of the
//! dispatch job, set at launch by `core::child_env`), and so does everything
//! it starts, wherever it moves in the process tree. When the dispatch ends
//! the processes carrying that id are stopped, and so is a descendant of one
//! that cleared its environment — but only one still in the session of the
//! tagged process it descends from. A daemon the worker happened to start
//! first (a tmux server, `ollama serve`, a gradle daemon) carries the tag and
//! adopts children that are not the worker's; those are born in sessions of
//! their own and are left alone.
//!
//! A process is signalled only after its start time is compared with the one
//! seen in the snapshot (a pid the system reused is never touched), and its
//! tag or session is checked again right before each signal. A process that
//! copies the variable into something it launches for the operator (a direnv
//! `.envrc` that exports it, say) makes that thing the worker's: it is
//! stopped with the dispatch.

use crate::core::agent_orphans::start_stamp;

/// The dispatch job a process was started under.
pub const DISPATCH_ENV: &str = "KRONN_TASK_DISPATCH_ID";

/// KT-1324 — the tag of the agents an audit run starts, in [`DISPATCH_ENV`]
/// like a worker's: a run's agent can outlive the backend (an ACP-adapter agent
/// has no pid recorded on the run), and boot stops whatever still carries the
/// tag of a run before settling its steps.
pub fn audit_run_tag(audit_run_id: &str) -> String {
    format!("audit-run:{audit_run_id}")
}

/// What one reap found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReapReport {
    /// Processes that were signalled.
    pub signalled: usize,
    /// Processes still alive after SIGKILL (not expected; logged by the caller).
    pub survivors: Vec<u32>,
    /// The process table could not be read: nothing is known of what runs.
    pub unverified: bool,
}

impl ReapReport {
    /// Something of the run may still be alive, or could not be looked for.
    pub fn may_still_run(&self) -> bool {
        self.unverified || !self.survivors.is_empty()
    }
}

/// One row of the process table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessRow {
    pid: u32,
    ppid: u32,
    /// The process's session; `None` when it cannot be read.
    sid: Option<u32>,
    /// `agent_orphans::start_stamp` at the time of the snapshot.
    stamp: Option<String>,
    /// The process's environment names `DISPATCH_ENV=<this dispatch>`.
    tagged: bool,
}

/// Why a process is taken as the run's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claim {
    /// Its environment carries the dispatch id.
    Tagged,
    /// It descends from a taken process and shares that process's session.
    Session(u32),
}

/// A process chosen for the reap, with what identifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    pid: u32,
    stamp: String,
    claim: Claim,
}

/// The processes of a dispatch's run, from a process table: every one carrying
/// its id, and the untagged descendants that stayed in the session of the taken
/// process they descend from. Never `own_pid` nor pid 1, nor a process whose
/// identity could not be read (it could not be told from its successor).
fn run_processes(rows: &[ProcessRow], own_pid: u32) -> Vec<Target> {
    let eligible = |row: &ProcessRow| row.pid > 1 && row.pid != own_pid && row.stamp.is_some();
    let mut found: Vec<Target> = Vec::new();
    // The session each found process anchors its descendants to.
    let mut anchors: Vec<Option<u32>> = Vec::new();
    for row in rows.iter().filter(|row| row.tagged && eligible(row)) {
        found.push(Target {
            pid: row.pid,
            stamp: row.stamp.clone().unwrap_or_default(),
            claim: Claim::Tagged,
        });
        anchors.push(row.sid);
    }
    let mut index = 0;
    while index < found.len() {
        let parent = found[index].pid;
        let anchor = anchors[index];
        index += 1;
        let Some(anchor) = anchor else { continue };
        for row in rows {
            if row.ppid != parent
                || !eligible(row)
                || row.sid != Some(anchor)
                || found.iter().any(|target| target.pid == row.pid)
            {
                continue;
            }
            found.push(Target {
                pid: row.pid,
                stamp: row.stamp.clone().unwrap_or_default(),
                claim: Claim::Session(anchor),
            });
            anchors.push(Some(anchor));
        }
    }
    found
}

/// Parse `ps axeww -o pid=,ppid=,command=` output (platforms without `/proc`
/// nor libproc). The environment follows the command on the same line; the
/// dispatch id is matched as a whole token so one id never matches another
/// that it prefixes. Session and start stamp are not in that output.
///
/// An environment value may hold a newline (an exported shell function does):
/// the lines that do not open with two numbers continue the previous process.
#[cfg(any(test, not(any(target_os = "linux", target_os = "macos"))))]
fn parse_process_table(output: &str, dispatch_job_id: &str) -> Vec<ProcessRow> {
    let needle = format!("{DISPATCH_ENV}={dispatch_job_id}");
    let mut rows: Vec<ProcessRow> = Vec::new();
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let ids = fields
            .next()
            .and_then(|pid| pid.parse::<u32>().ok())
            .zip(fields.next().and_then(|ppid| ppid.parse::<u32>().ok()));
        match ids {
            Some((pid, ppid)) => rows.push(ProcessRow {
                pid,
                ppid,
                sid: None,
                stamp: None,
                tagged: carries_token(line, &needle),
            }),
            None => {
                if let Some(last) = rows.last_mut() {
                    last.tagged |= carries_token(line, &needle);
                }
            }
        }
    }
    rows
}

#[cfg(any(test, not(any(target_os = "linux", target_os = "macos"))))]
fn carries_token(line: &str, needle: &str) -> bool {
    line.match_indices(needle).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + needle.len()..].chars().next();
        before.is_none_or(char::is_whitespace) && after.is_none_or(char::is_whitespace)
    })
}

/// The session a process belongs to; `None` when it is gone or unreadable.
#[cfg(unix)]
fn session_of(pid: u32) -> Option<u32> {
    // SAFETY: a plain query about another process; failure is `-1`.
    let sid = unsafe { libc::getsid(pid as libc::pid_t) };
    (sid > 0).then_some(sid as u32)
}

/// The process's environment names this dispatch, read now.
#[cfg(target_os = "linux")]
fn carries_tag(pid: u32, dispatch_job_id: &str) -> bool {
    let needle = format!("{DISPATCH_ENV}={dispatch_job_id}");
    std::fs::read(format!("/proc/{pid}/environ"))
        .is_ok_and(|environ| environ.split(|b| *b == 0).any(|e| e == needle.as_bytes()))
}

#[cfg(target_os = "macos")]
fn carries_tag(pid: u32, dispatch_job_id: &str) -> bool {
    let needle = format!("{DISPATCH_ENV}={dispatch_job_id}");
    macos_procargs(pid as libc::pid_t)
        .is_some_and(|raw| procargs_environment(&raw).contains(&needle.as_bytes()))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn carries_tag(pid: u32, dispatch_job_id: &str) -> bool {
    crate::core::cmd::sync_tool_cmd("ps")
        .args(["eww", "-p", &pid.to_string(), "-o", "pid=,ppid=,command="])
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && parse_process_table(&String::from_utf8_lossy(&output.stdout), dispatch_job_id)
                    .iter()
                    .any(|row| row.tagged)
        })
}

/// The process table, each process with its parent, session, start stamp and
/// whether its environment carries this dispatch's id. `None` when it cannot
/// be read.
#[cfg(target_os = "linux")]
fn process_table(dispatch_job_id: &str) -> Option<Vec<ProcessRow>> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        // Field 2 (the command) may hold spaces: read after its closing ')'.
        let Some(ppid) = std::fs::read_to_string(entry.path().join("stat"))
            .ok()
            .and_then(|stat| {
                let after = stat.rsplit_once(')')?.1;
                after.split_whitespace().nth(1)?.parse::<u32>().ok()
            })
        else {
            continue;
        };
        rows.push(ProcessRow {
            pid,
            ppid,
            sid: session_of(pid),
            stamp: start_stamp(pid),
            tagged: carries_tag(pid, dispatch_job_id),
        });
    }
    Some(rows)
}

/// No subprocess: the kernel's own answer, which also works where `ps` may not
/// be executed (the same reasoning as `agent_orphans::start_stamp`).
#[cfg(target_os = "macos")]
fn process_table(dispatch_job_id: &str) -> Option<Vec<ProcessRow>> {
    // SAFETY: a null buffer only asks for the number of pids.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return None;
    }
    let mut pids = vec![0 as libc::pid_t; count as usize + 64];
    let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: the buffer is `bytes` long; the call returns the pids it wrote.
    let written = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    if written <= 0 {
        return None;
    }
    pids.truncate(written as usize);
    let mut rows = Vec::with_capacity(pids.len());
    for pid in pids.into_iter().filter(|pid| *pid > 0) {
        // SAFETY: a zeroed plain-data struct of exactly the size passed.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if got != size {
            continue; // gone since the listing
        }
        rows.push(ProcessRow {
            pid: pid as u32,
            ppid: info.pbi_ppid,
            sid: session_of(pid as u32),
            stamp: start_stamp(pid as u32),
            tagged: carries_tag(pid as u32, dispatch_job_id),
        });
    }
    Some(rows)
}

/// `KERN_PROCARGS2` of a process of the same user; `None` for any other.
#[cfg(target_os = "macos")]
fn macos_procargs(pid: libc::pid_t) -> Option<Vec<u8>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: a null buffer only asks for the size.
    let sized = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if sized != 0 || size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    // SAFETY: `buffer` is `size` bytes long; `size` is updated to what was written.
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    buffer.truncate(size);
    Some(buffer)
}

/// The environment strings of a `KERN_PROCARGS2` buffer: `argc`, the
/// executable path, NUL padding, `argc` arguments, then the environment.
#[cfg(any(target_os = "macos", test))]
fn procargs_environment(raw: &[u8]) -> Vec<&[u8]> {
    let Some((argc, mut rest)) = raw.split_first_chunk::<4>() else {
        return Vec::new();
    };
    let argc = i32::from_ne_bytes(*argc).max(0);
    fn skip_string(rest: &mut &[u8]) {
        let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
        *rest = &rest[(end + 1).min(rest.len())..];
    }
    skip_string(&mut rest); // the executable path
    while rest.first() == Some(&0) {
        rest = &rest[1..];
    }
    for _ in 0..argc {
        skip_string(&mut rest);
    }
    rest.split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
        .collect()
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn process_table(dispatch_job_id: &str) -> Option<Vec<ProcessRow>> {
    // BSD syntax: `e` appends the environment to the command; `ww` lifts the
    // width limit.
    let output = crate::core::cmd::sync_tool_cmd("ps")
        .args(["axeww", "-o", "pid=,ppid=,command="])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut rows = parse_process_table(&String::from_utf8_lossy(&output.stdout), dispatch_job_id);
    for row in &mut rows {
        row.sid = session_of(row.pid);
        row.stamp = start_stamp(row.pid);
    }
    Some(rows)
}

#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let exists = match unsafe { libc::kill(pid as libc::pid_t, 0) } {
        0 => true,
        _ => std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM),
    };
    exists && !is_zombie(pid)
}

/// A process that has died and waits to be reaped by its parent is gone for
/// our purpose (macOS already answers ESRCH to the stamp read for those).
#[cfg(target_os = "linux")]
fn is_zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')?
                .1
                .split_whitespace()
                .next()
                .map(|state| state == "Z")
        })
        .unwrap_or(false)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn is_zombie(_pid: u32) -> bool {
    false
}

/// The target is still the process that was seen: same start time, and still
/// what made it the run's (the tag in its environment, or its session).
#[cfg(unix)]
fn is_still_ours(target: &Target, dispatch_job_id: &str) -> bool {
    if start_stamp(target.pid).as_deref() != Some(target.stamp.as_str()) {
        return false;
    }
    match target.claim {
        Claim::Tagged => carries_tag(target.pid, dispatch_job_id),
        Claim::Session(sid) => session_of(target.pid) == Some(sid),
    }
}

/// Signal what is still ours; the ones that were signalled.
#[cfg(unix)]
fn signal_verified<'a>(
    targets: &'a [Target],
    dispatch_job_id: &str,
    signal: libc::c_int,
) -> Vec<&'a Target> {
    let mut sent = Vec::new();
    for target in targets {
        if !is_still_ours(target, dispatch_job_id) {
            continue;
        }
        // SAFETY: a plain signal to a process just verified to be the one that
        // carried this dispatch's id; an already gone pid answers ESRCH.
        unsafe {
            libc::kill(target.pid as libc::pid_t, signal);
        }
        sent.push(target);
    }
    sent
}

/// The targets still alive (with the same start time) after `timeout`.
#[cfg(unix)]
fn wait_until_gone<'a>(targets: &[&'a Target], timeout: std::time::Duration) -> Vec<&'a Target> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let alive: Vec<&Target> = targets
            .iter()
            .copied()
            .filter(|target| {
                is_alive(target.pid)
                    && start_stamp(target.pid).as_deref() == Some(target.stamp.as_str())
            })
            .collect();
        if alive.is_empty() || std::time::Instant::now() >= deadline {
            return alive;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Stop everything a dispatch's run started (blocking). SIGTERM first, so a
/// server closes its listener cleanly, then SIGKILL for what ignored it; the
/// table is read again afterwards for what was started meanwhile.
#[cfg(unix)]
pub fn reap_dispatch(dispatch_job_id: &str) -> ReapReport {
    const TERM_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);
    const KILL_GRACE: std::time::Duration = std::time::Duration::from_millis(1000);
    const ROUNDS: usize = 3;

    let mut report = ReapReport::default();
    if dispatch_job_id.trim().is_empty() {
        return report;
    }
    let own_pid = std::process::id();
    for _ in 0..ROUNDS {
        let Some(rows) = process_table(dispatch_job_id) else {
            tracing::warn!(
                dispatch_job_id,
                "cannot read the process table to reap a dispatch"
            );
            report.unverified = true;
            return report;
        };
        let targets = run_processes(&rows, own_pid);
        if targets.is_empty() {
            report.survivors.clear();
            return report;
        }
        let sent = signal_verified(&targets, dispatch_job_id, libc::SIGTERM);
        if sent.is_empty() {
            // Every one changed identity since the snapshot: nothing is ours.
            return report;
        }
        report.signalled += sent.len();
        let stubborn = wait_until_gone(&sent, TERM_GRACE);
        if stubborn.is_empty() {
            continue;
        }
        let owned: Vec<Target> = stubborn.into_iter().cloned().collect();
        let killed = signal_verified(&owned, dispatch_job_id, libc::SIGKILL);
        report.survivors = wait_until_gone(&killed, KILL_GRACE)
            .iter()
            .map(|target| target.pid)
            .collect();
    }
    report
}

#[cfg(not(unix))]
pub fn reap_dispatch(_dispatch_job_id: &str) -> ReapReport {
    ReapReport::default()
}

/// [`reap_dispatch`] off the async runtime's threads.
pub async fn reap_dispatch_async(dispatch_job_id: &str) -> ReapReport {
    let id = dispatch_job_id.to_string();
    let report = tokio::task::spawn_blocking(move || reap_dispatch(&id))
        .await
        .unwrap_or_default();
    if report.signalled > 0 {
        tracing::info!(
            dispatch_job_id,
            signalled = report.signalled,
            survivors = report.survivors.len(),
            "stopped the processes a worker left after its dispatch"
        );
    }
    if !report.survivors.is_empty() {
        tracing::warn!(
            dispatch_job_id,
            survivors = ?report.survivors,
            "processes of a finished dispatch survived SIGKILL"
        );
    }
    report
}

#[cfg(test)]
#[path = "dispatch_reaper_test.rs"]
mod dispatch_reaper_test;
