use super::*;

fn row(pid: u32, ppid: u32, sid: u32, tagged: bool) -> ProcessRow {
    ProcessRow {
        pid,
        ppid,
        sid: Some(sid),
        stamp: Some(format!("stamp-{pid}")),
        tagged,
    }
}

fn pids(targets: &[Target]) -> Vec<u32> {
    let mut found: Vec<u32> = targets.iter().map(|target| target.pid).collect();
    found.sort_unstable();
    found
}

#[test]
fn only_a_whole_token_of_the_dispatch_id_is_a_match() {
    let needle = format!("{DISPATCH_ENV}=job-1");
    assert!(carries_token(
        "1 0 node vite HOME=/x KRONN_TASK_DISPATCH_ID=job-1 PATH=/bin",
        &needle
    ));
    assert!(carries_token(
        "1 0 sh KRONN_TASK_DISPATCH_ID=job-1",
        &needle
    ));
    // Another dispatch whose id the first one prefixes.
    assert!(!carries_token(
        "1 0 sh KRONN_TASK_DISPATCH_ID=job-12 A=b",
        &needle
    ));
    // The text appearing inside another value.
    assert!(!carries_token(
        "1 0 sh X=KRONN_TASK_DISPATCH_ID=job-1",
        &needle
    ));
}

#[test]
fn the_run_is_its_tagged_processes_and_their_session_descendants() {
    let rows = [
        row(10, 1, 10, true),   // the agent's shell tool
        row(11, 10, 10, false), // a server it started, environment cleared
        row(12, 11, 10, false), // that server's worker
        row(20, 1, 20, false),  // somebody else's
        row(21, 20, 20, false),
        row(30, 1, 30, true), // the backend itself must never be signalled
    ];

    let found = run_processes(&rows, 30);

    assert_eq!(pids(&found), vec![10, 11, 12]);
    assert!(found
        .iter()
        .any(|target| target.pid == 10 && target.claim == Claim::Tagged));
    assert!(found
        .iter()
        .filter(|target| target.pid != 10)
        .all(|target| target.claim == Claim::Session(10)));
}

#[test]
fn a_tagged_daemons_children_in_their_own_sessions_are_not_the_runs() {
    // A tmux server the worker started first carries the tag; the shells the
    // operator opens later are its children but each leads a session.
    let rows = [
        row(10, 1, 10, true),   // the worker's shell
        row(40, 10, 40, true),  // tmux server (its own session, tagged)
        row(41, 40, 41, false), // an operator's pane shell
        row(42, 41, 41, false), // what runs in it
        row(43, 40, 40, false), // a helper that stayed in the daemon's session
    ];

    let found = run_processes(&rows, 1);

    assert_eq!(pids(&found), vec![10, 40, 43]);
}

#[test]
fn a_process_whose_identity_cannot_be_read_is_never_a_target() {
    let mut unreadable = row(11, 10, 10, true);
    unreadable.stamp = None;
    let mut sessionless = row(12, 10, 10, false);
    sessionless.sid = None;
    let rows = [
        row(10, 1, 10, true),
        unreadable,
        sessionless,
        row(13, 10, 10, false),
    ];

    assert_eq!(pids(&run_processes(&rows, 1)), vec![10, 13]);
}

#[test]
fn a_process_table_is_read_per_dispatch() {
    let table = "  100     1 /bin/claude KRONN_TASK_DISPATCH_ID=job-a FOO=1\n\
                 \x20 101   100 node vite KRONN_TASK_DISPATCH_ID=job-a\n\
                 \x20 200     1 /bin/claude KRONN_TASK_DISPATCH_ID=job-b\n\
                 not a row\n";

    let rows = parse_process_table(table, "job-a");

    let tagged: Vec<u32> = rows.iter().filter(|r| r.tagged).map(|r| r.pid).collect();
    assert_eq!(tagged, vec![100, 101]);
    assert_eq!(rows.len(), 3);
    // The output names neither the session nor the start time.
    assert!(rows.iter().all(|r| r.sid.is_none() && r.stamp.is_none()));
}

#[test]
fn an_environment_value_with_a_newline_does_not_hide_the_tag() {
    let table = "  300     1 sh BASH_FUNC_f%%=() {  echo hi\n\
                 }\n\
                 KRONN_TASK_DISPATCH_ID=job-a\n\
                 \x20 301     1 sleep\n";

    let rows = parse_process_table(table, "job-a");

    let seen: Vec<(u32, u32, bool)> = rows.iter().map(|r| (r.pid, r.ppid, r.tagged)).collect();
    assert_eq!(seen, vec![(300, 1, true), (301, 1, false)]);
}

#[test]
fn procargs_environment_skips_the_executable_and_the_arguments() {
    let mut raw = Vec::new();
    raw.extend_from_slice(&2i32.to_ne_bytes());
    raw.extend_from_slice(b"/bin/sh\0\0\0");
    raw.extend_from_slice(b"sh\0KRONN_TASK_DISPATCH_ID=in-an-argument\0");
    raw.extend_from_slice(b"A=1\0KRONN_TASK_DISPATCH_ID=job-a\0");

    let environment = procargs_environment(&raw);

    assert_eq!(
        environment,
        vec![
            b"A=1".as_slice(),
            b"KRONN_TASK_DISPATCH_ID=job-a".as_slice()
        ]
    );
}

#[cfg(unix)]
mod processes {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    fn job_id(label: &str) -> String {
        format!("test-{label}-{}", uuid::Uuid::new_v4())
    }

    fn read_line(child: &mut std::process::Child) -> String {
        use std::io::{BufRead, BufReader};
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .unwrap();
        line.trim().to_string()
    }

    fn listening(port: u16) -> bool {
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            std::time::Duration::from_millis(300),
        )
        .is_ok()
    }

    fn python() -> Option<&'static str> {
        ["python3", "python"].into_iter().find(|name| {
            Command::new(name)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
    }

    /// What a worker's shell tool does: runs `code` with python in the
    /// background of a shell, in a group of its own, under `dispatch`. (Python
    /// rather than a system binary: macOS shows no environment for those.)
    fn start_python_under_shell(python: &str, code: &str, dispatch: &str) -> std::process::Child {
        Command::new("sh")
            .args(["-c", &format!("{python} -c \"$CODE\" & wait")])
            .env(DISPATCH_ENV, dispatch)
            .env("CODE", code)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    const PARENT_OF_A_CLEARED_CHILD: &str = "import subprocess,sys,time\n\
        c=subprocess.Popen(['sleep','300'],env={})\n\
        print(c.pid); sys.stdout.flush()\n\
        time.sleep(300)\n";

    const SLEEPER: &str = "import sys,time\n\
        print('up'); sys.stdout.flush()\n\
        time.sleep(300)\n";

    const LISTENER: &str = "import socket,sys,time\n\
        s=socket.socket()\n\
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n\
        s.bind(('127.0.0.1',0))\n\
        s.listen(5)\n\
        print(s.getsockname()[1]); sys.stdout.flush()\n\
        time.sleep(300)\n";

    #[test]
    fn a_server_the_worker_started_stops_listening_when_its_dispatch_ends() {
        let Some(python) = python() else {
            eprintln!("skipped: no python to listen with");
            return;
        };
        let dispatch = job_id("listen");
        // The shell tool starts a server, then returns: the server is left
        // behind in its own group, parented by the shell until that exits.
        let script = format!("{python} -c \"$LISTENER\" & wait");
        let mut shell = Command::new("sh")
            .args(["-c", &script])
            .env(DISPATCH_ENV, &dispatch)
            .env("LISTENER", LISTENER)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let port: u16 = read_line(&mut shell)
            .parse()
            .expect("the server reports its port");
        assert!(listening(port), "the server must be up before the reap");

        let report = reap_dispatch(&dispatch);

        assert!(report.signalled >= 1, "{report:?}");
        assert!(report.survivors.is_empty(), "{report:?}");
        assert!(
            !listening(port),
            "port {port} must not be listened on once the dispatch ended"
        );
        let _ = shell.wait();
    }

    #[test]
    fn the_whole_tree_goes_even_when_a_child_cleared_its_environment() {
        let Some(python) = python() else {
            eprintln!("skipped: no python");
            return;
        };
        let dispatch = job_id("tree");
        let mut shell = start_python_under_shell(python, PARENT_OF_A_CLEARED_CHILD, &dispatch);
        let child: u32 = read_line(&mut shell).parse().unwrap();
        assert!(is_alive(child));

        let report = reap_dispatch(&dispatch);

        assert!(report.signalled >= 2, "{report:?}");
        assert!(report.survivors.is_empty(), "{report:?}");
        let _ = shell.wait();
        let gone = wait_until_gone_pid(child, std::time::Duration::from_secs(3));
        assert!(gone, "child {child} is still alive");
    }

    /// A tagged daemon with two children that cleared their environment: one
    /// stays in the daemon's session, one is another session's (what a tmux
    /// server's later shells are).
    const DAEMON_WITH_FOREIGN_CHILD: &str = "import subprocess,sys,time\n\
        mine=subprocess.Popen(['sleep','300'],env={})\n\
        foreign=subprocess.Popen(['sleep','300'],env={},start_new_session=True)\n\
        print(mine.pid, foreign.pid); sys.stdout.flush()\n\
        time.sleep(300)\n";

    #[test]
    fn a_child_a_daemon_adopted_in_another_session_survives_the_reap() {
        let Some(python) = python() else {
            eprintln!("skipped: no python");
            return;
        };
        let dispatch = job_id("daemon");
        let mut shell = start_python_under_shell(python, DAEMON_WITH_FOREIGN_CHILD, &dispatch);
        let line = read_line(&mut shell);
        let (mine, foreign): (u32, u32) = {
            let mut pids = line
                .split_whitespace()
                .map(|pid| pid.parse::<u32>().unwrap());
            (pids.next().unwrap(), pids.next().unwrap())
        };
        assert!(is_alive(mine) && is_alive(foreign));

        let report = reap_dispatch(&dispatch);

        assert!(report.survivors.is_empty(), "{report:?}");
        let _ = shell.wait();
        let still_here = start_stamp(foreign).is_some() && is_alive(foreign);
        // Clean up before asserting so a failure leaves nothing behind.
        // SAFETY: the pid is the `sleep` this test just started.
        unsafe {
            libc::kill(foreign as libc::pid_t, libc::SIGKILL);
        }
        assert!(still_here, "the other session's child must survive");
        let gone = wait_until_gone_pid(mine, std::time::Duration::from_secs(3));
        assert!(gone, "the same-session child of the daemon must be stopped");
    }

    fn wait_until_gone_pid(pid: u32, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while is_alive(pid) {
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        true
    }

    #[test]
    fn a_reused_pid_or_a_changed_claim_is_never_signalled() {
        let mut sleeper = Command::new("sleep")
            .arg("300")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = sleeper.id();
        let stamp = start_stamp(pid).expect("the sleeper has a start stamp");
        let sid = session_of(pid).expect("the sleeper has a session");
        let target = |stamp: &str, claim: Claim| Target {
            pid,
            stamp: stamp.to_string(),
            claim,
        };

        // Another process now holds that pid: the stamp differs.
        let reused = [target("not-the-same-start", Claim::Session(sid))];
        assert!(signal_verified(&reused, "x", libc::SIGKILL).is_empty());
        // Claimed by tag, but its environment does not carry it any more.
        let untagged = [target(&stamp, Claim::Tagged)];
        assert!(signal_verified(&untagged, "x", libc::SIGKILL).is_empty());
        // Claimed by session, but it left that session.
        let moved = [target(&stamp, Claim::Session(sid.wrapping_add(1)))];
        assert!(signal_verified(&moved, "x", libc::SIGKILL).is_empty());
        assert!(is_alive(pid), "none of the above may have been signalled");

        let ours = [target(&stamp, Claim::Session(sid))];
        assert_eq!(signal_verified(&ours, "x", libc::SIGKILL).len(), 1);
        let _ = sleeper.wait();
    }

    #[test]
    fn another_dispatchs_processes_are_left_alone() {
        let Some(python) = python() else {
            eprintln!("skipped: no python");
            return;
        };
        let ours = job_id("ours");
        let theirs = job_id("theirs");
        let mut other = start_python_under_shell(python, SLEEPER, &theirs);
        assert_eq!(read_line(&mut other), "up");

        let report = reap_dispatch(&ours);
        assert_eq!(report, ReapReport::default());
        // It is theirs to stop: the run that owns it ends it.
        let theirs_report = reap_dispatch(&theirs);
        assert!(theirs_report.signalled >= 1, "{theirs_report:?}");
        let _ = other.wait();
    }

    #[test]
    fn nothing_to_reap_is_not_an_error() {
        assert_eq!(reap_dispatch(&job_id("none")), ReapReport::default());
        assert_eq!(reap_dispatch("  "), ReapReport::default());
    }
}
