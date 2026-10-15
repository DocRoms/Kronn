use super::*;

fn row(pid: u32, ppid: u32, tagged: bool) -> ProcessRow {
    ProcessRow { pid, ppid, tagged }
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
fn the_run_is_its_tagged_processes_and_their_descendants_only() {
    let rows = [
        row(10, 1, true),   // the agent's shell tool
        row(11, 10, false), // a server it started, environment cleared
        row(12, 11, false), // that server's worker
        row(20, 1, false),  // somebody else's
        row(21, 20, false),
        row(30, 1, true), // the backend itself must never be signalled
    ];

    let mut found = run_processes(&rows, 30);
    found.sort_unstable();

    assert_eq!(found, vec![10, 11, 12]);
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
}

#[test]
fn an_environment_value_with_a_newline_does_not_hide_the_tag() {
    let table = "  300     1 sh BASH_FUNC_f%%=() {  echo hi\n\
                 }\n\
                 KRONN_TASK_DISPATCH_ID=job-a\n\
                 \x20 301     1 sleep\n";

    let rows = parse_process_table(table, "job-a");

    assert_eq!(rows, vec![row(300, 1, true), row(301, 1, false)]);
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
        let gone = wait_until_gone(&[child], std::time::Duration::from_secs(3));
        assert!(gone.is_empty(), "child {child} is still alive");
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
