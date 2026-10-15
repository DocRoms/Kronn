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
//! the processes carrying that id — and whatever descends from them, in case
//! one cleared its environment — are stopped. Only the id of the dispatch that
//! ended is ever matched, so a worker of another lane is never touched.

/// The dispatch job a process was started under.
pub const DISPATCH_ENV: &str = "KRONN_TASK_DISPATCH_ID";

/// What one reap found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReapReport {
    /// Processes that were signalled.
    pub signalled: usize,
    /// Processes still alive after SIGKILL (not expected; logged by the caller).
    pub survivors: Vec<u32>,
}

/// One row of the process table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessRow {
    pid: u32,
    ppid: u32,
    /// The process's environment names `DISPATCH_ENV=<this dispatch>`.
    tagged: bool,
}

/// The processes of a dispatch's run, from a process table: those carrying its
/// id and all their descendants, never `own_pid` nor pid 1.
fn run_processes(rows: &[ProcessRow], own_pid: u32) -> Vec<u32> {
    let mut found: Vec<u32> = rows
        .iter()
        .filter(|row| row.tagged && row.pid > 1 && row.pid != own_pid)
        .map(|row| row.pid)
        .collect();
    let mut index = 0;
    while index < found.len() {
        let parent = found[index];
        index += 1;
        for row in rows {
            if row.ppid == parent && row.pid > 1 && row.pid != own_pid && !found.contains(&row.pid)
            {
                found.push(row.pid);
            }
        }
    }
    found
}

/// Parse `ps axeww -o pid=,ppid=,command=` output (platforms without `/proc`
/// nor libproc). The environment follows the command on the same line; the
/// dispatch id is matched as a whole token so one id never matches another
/// that it prefixes.
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

/// The process table, each process with its parent and whether its
/// environment carries this dispatch's id. `None` when it cannot be read.
#[cfg(target_os = "linux")]
fn process_table(dispatch_job_id: &str) -> Option<Vec<ProcessRow>> {
    let needle = format!("{DISPATCH_ENV}={dispatch_job_id}");
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
        let tagged = std::fs::read(entry.path().join("environ"))
            .is_ok_and(|environ| environ.split(|b| *b == 0).any(|e| e == needle.as_bytes()));
        rows.push(ProcessRow { pid, ppid, tagged });
    }
    Some(rows)
}

/// No subprocess: the kernel's own answer, which also works where `ps` may not
/// be executed (the same reasoning as `agent_orphans::start_stamp`).
#[cfg(target_os = "macos")]
fn process_table(dispatch_job_id: &str) -> Option<Vec<ProcessRow>> {
    let needle = format!("{DISPATCH_ENV}={dispatch_job_id}");
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
        let tagged = macos_procargs(pid)
            .is_some_and(|raw| procargs_environment(&raw).contains(&needle.as_bytes()));
        rows.push(ProcessRow {
            pid: pid as u32,
            ppid: info.pbi_ppid,
            tagged,
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
    output
        .status
        .success()
        .then(|| parse_process_table(&String::from_utf8_lossy(&output.stdout), dispatch_job_id))
}

#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    match unsafe { libc::kill(pid as libc::pid_t, 0) } {
        0 => true,
        _ => std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM),
    }
}

#[cfg(unix)]
fn signal(pids: &[u32], signal: libc::c_int) {
    for pid in pids {
        // SAFETY: a plain signal to a process found carrying this dispatch's
        // id; an already gone pid answers ESRCH, which is the normal case.
        unsafe {
            libc::kill(*pid as libc::pid_t, signal);
        }
    }
}

#[cfg(unix)]
fn wait_until_gone(pids: &[u32], timeout: std::time::Duration) -> Vec<u32> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let alive: Vec<u32> = pids.iter().copied().filter(|pid| is_alive(*pid)).collect();
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
            return report;
        };
        let pids = run_processes(&rows, own_pid);
        if pids.is_empty() {
            report.survivors.clear();
            return report;
        }
        report.signalled += pids.len();
        signal(&pids, libc::SIGTERM);
        let stubborn = wait_until_gone(&pids, TERM_GRACE);
        if stubborn.is_empty() {
            continue;
        }
        signal(&stubborn, libc::SIGKILL);
        report.survivors = wait_until_gone(&stubborn, KILL_GRACE);
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
