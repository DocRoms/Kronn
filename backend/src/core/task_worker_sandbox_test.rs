use super::*;
use crate::core::task_worker_env::Provisioned;
use crate::core::task_worker_lane_warm::{warm_lane, BuildGate, FirstBuild, WarmStartSettings};
use std::sync::Mutex;
use std::time::Duration;

/// Records what reaches the process level; never starts anything.
struct Recorder {
    calls: Mutex<Vec<ProbeCommand>>,
}

impl Recorder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<ProbeCommand> {
        self.calls.lock().unwrap().clone()
    }
}

impl ProbeRunner for Recorder {
    fn run(&self, probe: &ProbeCommand) -> ProbeOutcome {
        self.calls.lock().unwrap().push(probe.clone());
        ProbeOutcome {
            success: true,
            detail: String::new(),
        }
    }
}

struct Layout {
    _dir: tempfile::TempDir,
    worktree: PathBuf,
}

fn layout() -> Layout {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().canonicalize().unwrap();
    let worktree = repo
        .join(".kronn")
        .join("worktrees")
        .join("task-kt-1255-sandbox");
    std::fs::create_dir_all(worktree.join("backend")).unwrap();
    Layout {
        _dir: dir,
        worktree,
    }
}

fn policy_of(layout: &Layout) -> SandboxPolicy {
    SandboxPolicy::for_worktree(&layout.worktree).unwrap()
}

fn probe(program: &str, args: &[&str], cwd: &Path) -> ProbeCommand {
    ProbeCommand {
        program: program.into(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: cwd.to_path_buf(),
        env: Vec::new(),
        timeout: Duration::from_secs(120),
    }
}

fn provisioned() -> Provisioned {
    Provisioned {
        frontend_deps: crate::core::task_worker_env::FrontendDeps::NotApplicable,
        cargo_home_ready: false,
        shared_cargo_home: None,
        warnings: Vec::new(),
    }
}

// ── The policy ───────────────────────────────────────────────────────────

#[test]
fn the_policy_writes_only_the_workers_own_roots() {
    let layout = layout();
    let policy = policy_of(&layout);
    let lane = WorkerLane::for_worktree(&layout.worktree).unwrap();
    assert_eq!(policy.write_roots, lane.write_roots());
    assert!(policy.write_roots.contains(&layout.worktree));
    assert!(policy.write_roots.contains(&lane.cargo_target_dir));
    // The only scratch space is the lane's own, inside a write root.
    assert_eq!(policy.temp_dir, lane.tmp_dir());
    assert!(
        policy
            .write_roots
            .iter()
            .any(|root| policy.temp_dir.starts_with(root)),
        "{policy:?}"
    );
}

#[test]
fn the_writable_set_is_the_lanes_roots_and_its_temp_dir_and_no_global_temp() {
    let layout = layout();
    let policy = policy_of(&layout);
    let writable = policy.writable();

    for root in &policy.write_roots {
        assert!(writable.contains(&real_path(root)), "{writable:?}");
    }
    assert!(writable.contains(&real_path(&policy.temp_dir)));
    for path in &writable {
        let text = path.to_string_lossy();
        // The test's own layout lives under a temp dir, so only look for the
        // global directories themselves, not for paths below them.
        for global in ["/tmp", "/private/tmp", "/var/tmp", "/private/var/tmp"] {
            assert_ne!(text, global, "{writable:?}");
        }
        assert!(
            !text.starts_with("/var/folders") || text.contains("task-kt-1255-sandbox"),
            "no per-user cache or temp root: {writable:?}"
        );
    }
    let profile = seatbelt_profile(&writable);
    for global in [
        "\"/tmp\"",
        "\"/var/tmp\"",
        "\"/private/tmp\"",
        "\"/var/folders\"",
    ] {
        assert!(
            !profile.contains(&format!("(subpath {global})")),
            "{profile}"
        );
    }
}

/// The text of the `(allow <operation> …)` forms of `profile`.
fn allow_forms<'a>(profile: &'a str, operation: &str) -> Vec<&'a str> {
    let opener = format!("(allow {operation}");
    profile
        .match_indices(&opener)
        .map(|(start, _)| {
            let rest = &profile[start..];
            let mut depth = 0usize;
            for (offset, ch) in rest.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            return &rest[..=offset];
                        }
                    }
                    _ => {}
                }
            }
            rest
        })
        .collect()
}

#[test]
fn the_seatbelt_profile_denies_by_default_and_writes_only_the_roots() {
    let roots = vec![
        PathBuf::from("/lane/worktree"),
        PathBuf::from("/lane/target"),
    ];
    let profile = seatbelt_profile(&roots);

    assert!(
        profile.lines().any(|line| line == "(deny default)"),
        "deny by default:\n{profile}"
    );
    assert!(
        !profile.contains("(allow default)"),
        "never allow-by-default:\n{profile}"
    );
    let writes = allow_forms(&profile, "file-write*");
    assert_eq!(writes.len(), 1, "{profile}");
    assert!(writes[0].contains("(subpath \"/lane/worktree\")"));
    assert!(writes[0].contains("(subpath \"/lane/target\")"));
    assert!(
        !profile.contains("(subpath \"/Users"),
        "no host path: {profile}"
    );
    // Writes are never allowed by any other form (e.g. a bare file-write*).
    assert_eq!(profile.matches("file-write").count(), 1, "{profile}");
}

#[test]
fn the_seatbelt_profile_denies_every_outbound_connection_and_only_binds_locally() {
    let profile = seatbelt_profile(&[PathBuf::from("/lane/worktree")]);

    // Nothing may allow an outbound connection — loopback, any port, a Unix
    // socket, or the whole network family.
    assert!(!profile.contains("network-outbound"), "{profile}");
    assert!(!profile.contains("(allow network*"), "{profile}");
    assert!(!profile.contains("(allow network-bind)"), "{profile}");
    assert!(!profile.contains("(remote "), "{profile}");
    // Binding (and accepting on) loopback is the one network allowance.
    let network: Vec<&str> = profile
        .lines()
        .filter(|line| line.starts_with("(allow network"))
        .collect();
    assert_eq!(
        network,
        [
            "(allow network-bind (local ip \"localhost:*\"))",
            "(allow network-inbound (local ip \"localhost:*\"))",
        ],
        "{profile}"
    );
}

#[test]
fn the_seatbelt_profile_denies_mach_lookup_but_the_listed_services() {
    let profile = seatbelt_profile(&[PathBuf::from("/lane/worktree")]);

    let lookups = allow_forms(&profile, "mach-lookup");
    assert_eq!(lookups.len(), 1, "one lookup allowance:\n{profile}");
    // Every entry is a named service of the list: no wildcard, no regex, no
    // bare `(allow mach-lookup)`.
    let listed: Vec<String> = SEATBELT_MACH_SERVICES
        .iter()
        .map(|service| format!("(global-name \"{service}\")"))
        .collect();
    let body = lookups[0].trim_start_matches("(allow mach-lookup");
    let mut remaining = body.to_string();
    for entry in &listed {
        assert!(remaining.contains(entry.as_str()), "{entry} in {profile}");
        remaining = remaining.replace(entry.as_str(), "");
    }
    assert_eq!(
        remaining.trim().trim_end_matches(')').trim(),
        "",
        "only listed services:\n{profile}"
    );
    // The channels that start something outside the sandbox are not on it.
    for service in SEATBELT_MACH_SERVICES {
        for escape in ["launchd", "xpc", "launchservices", "lsd", "cfprefsd"] {
            assert!(!service.contains(escape), "{service}");
        }
    }
    // No other IPC family is opened.
    for operation in ["ipc-posix", "ipc-sysv", "iokit", "mach-register", "mach*"] {
        assert!(
            !profile.contains(&format!("(allow {operation}")),
            "{operation}:\n{profile}"
        );
    }
    assert!(profile.contains("(allow signal (target same-sandbox))"));
}

#[test]
fn the_seatbelt_profile_is_valid_sbpl() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let profile = seatbelt_profile(&[
        PathBuf::from("/lane/worktree"),
        PathBuf::from("/lane/target"),
    ]);
    let output = std::process::Command::new(SANDBOX_EXEC)
        .args(["-p", &profile, "/usr/bin/true"])
        .output()
        .expect("sandbox-exec starts");
    let stderr = String::from_utf8_lossy(&output.stderr);
    // A well-formed profile either runs, or is only refused at the very last
    // step (`sandbox_apply`) when this process is itself sandboxed. A syntax
    // error or an unknown operation fails earlier, with another message.
    assert!(
        output.status.success() || stderr.contains("sandbox_apply: Operation not permitted"),
        "the profile does not parse: {stderr}\n{profile}"
    );
}

#[test]
fn a_seatbelt_literal_cannot_be_broken_out_of() {
    let literal = seatbelt_literal(Path::new("/a\"b)(allow default)\\c"));
    assert_eq!(literal, "\"/a\\\"b)(allow default)\\\\c\"");
}

#[test]
fn bubblewrap_binds_the_whole_filesystem_read_only_and_only_the_roots_writable() {
    let args = bubblewrap_args(&[PathBuf::from("/lane/worktree")]);
    assert_eq!(&args[..3], ["--ro-bind", "/", "/"]);
    let bind = args.iter().position(|arg| arg == "--bind").unwrap();
    assert_eq!(
        &args[bind + 1..bind + 3],
        ["/lane/worktree", "/lane/worktree"]
    );
    assert_eq!(args.iter().filter(|arg| *arg == "--bind").count(), 1);
    assert_eq!(args.last().unwrap(), "--");
}

#[test]
fn bubblewrap_unshares_net_pid_and_ipc_and_hides_the_hosts_service_sockets() {
    let args = bubblewrap_args(&[PathBuf::from("/lane/worktree")]);
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    for flag in ["--unshare-net", "--unshare-pid", "--unshare-ipc"] {
        assert!(has(flag), "{flag} in {args:?}");
    }
    assert!(has("--die-with-parent") && has("--new-session"));
    // A private /dev, never the host's (its /dev/shm is host shared memory).
    assert!(!has("--dev-bind"), "{args:?}");
    let pair = |flag: &str, value: &str| {
        args.windows(2)
            .any(|window| window[0] == flag && window[1] == value)
    };
    assert!(pair("--dev", "/dev"), "{args:?}");
    // A PID namespace needs its own /proc, or the host's pids stay visible.
    assert!(pair("--proc", "/proc"), "{args:?}");
    // /run (systemd user bus, docker.sock) and /tmp (ssh-agent, X11) are
    // emptied before the roots are bound back on top.
    let run = args.windows(2).position(|w| w == ["--tmpfs", "/run"]);
    let tmp = args.windows(2).position(|w| w == ["--tmpfs", "/tmp"]);
    let bind = args.iter().position(|arg| arg == "--bind").unwrap();
    assert!(run.is_some() && tmp.is_some(), "{args:?}");
    assert!(tmp.unwrap() < bind && run.unwrap() < bind, "{args:?}");
    // Nothing global is bound writable.
    for (index, arg) in args.iter().enumerate() {
        if arg == "--bind" {
            assert_eq!(args[index + 1], "/lane/worktree");
        }
    }
}

#[test]
fn confining_a_probe_keeps_its_command_cwd_env_and_timeout() {
    let layout = layout();
    let policy = policy_of(&layout);
    let mut original = probe(
        "cargo",
        &["test", "--no-run"],
        &layout.worktree.join("backend"),
    );
    original.env = vec![
        ("CARGO_TARGET_DIR".into(), "/lane/target".into()),
        ("TMPDIR".into(), "/var/tmp".into()),
    ];
    let lane_tmp = real_path(&policy.temp_dir).into_os_string();

    for mechanism in [Mechanism::Seatbelt, Mechanism::Bubblewrap] {
        let confined = confine(mechanism, &policy, &original);
        assert_ne!(confined.program, "cargo");
        assert_eq!(
            &confined.args[confined.args.len() - 3..],
            ["cargo", "test", "--no-run"]
        );
        assert_eq!(confined.cwd, original.cwd);
        // Same environment, but the scratch space is the lane's own.
        assert_eq!(
            confined.env,
            vec![
                ("CARGO_TARGET_DIR".into(), "/lane/target".into()),
                ("TMPDIR".into(), lane_tmp.clone()),
            ]
        );
        assert_eq!(confined.timeout, original.timeout);
    }
    let seatbelt = confine(Mechanism::Seatbelt, &policy, &original);
    assert_eq!(seatbelt.program, SANDBOX_EXEC);
    assert_eq!(seatbelt.args[0], "-p");
    // The lane's own roots are in the profile the command runs under.
    assert!(seatbelt.args[1].contains(&layout.worktree.to_string_lossy().into_owned()));
}

// ── Fail closed ──────────────────────────────────────────────────────────

#[test]
fn a_host_without_a_sandbox_refuses_the_command_instead_of_running_it_unconfined() {
    let layout = layout();
    let inner = Recorder::new();
    let runner = SandboxedProbeRunner::new(Ok(policy_of(&layout)), None, inner.clone());

    let outcome = runner.run(&probe("cargo", &["test", "--no-run"], &layout.worktree));

    assert!(!outcome.success);
    assert!(
        outcome.detail.contains("never run unconfined"),
        "{outcome:?}"
    );
    assert!(inner.calls().is_empty(), "nothing reached a process");
}

#[test]
fn an_underivable_policy_refuses_the_command() {
    let inner = Recorder::new();
    let runner = SandboxedProbeRunner::new(
        Err("no such worktree".into()),
        Some(Mechanism::Seatbelt),
        inner.clone(),
    );

    let outcome = runner.run(&probe("cargo", &["fetch"], Path::new("/")));

    assert!(!outcome.success);
    assert!(outcome.detail.contains("no such worktree"), "{outcome:?}");
    assert!(inner.calls().is_empty());
}

#[test]
fn the_production_runner_is_confined_and_the_plain_process_runner_is_not() {
    let layout = layout();
    assert!(SandboxedProbeRunner::for_worktree(&layout.worktree).confined());
    assert!(!ProcessProbeRunner.confined());
}

// ── A relaunch ───────────────────────────────────────────────────────────

/// A relaunch after the worker changed `Cargo.lock` and wrote a build script
/// rebuilds; whatever the runner is asked to run reaches the process level
/// only inside the sandbox command.
#[test]
fn a_relaunch_after_the_worker_changed_its_build_never_reaches_a_process_unconfined() {
    let layout = layout();
    let backend = layout.worktree.join("backend");
    std::fs::write(
        backend.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    std::fs::write(backend.join("Cargo.lock"), "version = 4\n").unwrap();
    let lane = WorkerLane::for_worktree(&layout.worktree).unwrap();
    let inner = Recorder::new();
    let runner = SandboxedProbeRunner::new(
        Ok(SandboxPolicy::for_lane(&lane)),
        Some(Mechanism::Seatbelt),
        inner.clone(),
    );
    let settings = WarmStartSettings::resolve(Some("1"), Some("1"), Some("60"), 4);
    let build = |expect_fresh: bool| {
        let record = warm_lane(
            &lane,
            &provisioned(),
            &settings,
            &BuildGate::new(),
            &runner,
            &|_, _| Ok(()),
            |_| {},
        );
        if expect_fresh {
            assert!(
                matches!(record.first_build, FirstBuild::Succeeded { .. }),
                "{record:?}"
            );
        }
    };

    // The provisioning build: HEAD == base.
    build(true);
    // The worker then wrote a build script and moved the lock file.
    std::fs::write(
        backend.join("build.rs"),
        "fn main() { std::fs::write(\"/outside-the-lane\", \"x\").unwrap(); }\n",
    )
    .unwrap();
    std::fs::write(backend.join("Cargo.lock"), "version = 4\n# worker edit\n").unwrap();
    // The lane's marker is for the old lock: the relaunch builds again.
    build(true);

    let calls = inner.calls();
    assert_eq!(calls.len(), 2, "one build per launch: {calls:?}");
    for call in &calls {
        assert_eq!(call.program, SANDBOX_EXEC, "{call:?}");
        assert_eq!(
            &call.args[call.args.len() - 5..],
            ["cargo", "test", "--no-run", "--locked", "--offline"],
            "{call:?}"
        );
        assert!(call.args[1].contains("(deny default)"), "{call:?}");
        assert!(!call.args[1].contains("(allow default)"), "{call:?}");
        assert!(!call.args[1].contains("network-outbound"), "{call:?}");
    }
}

// ── The OS enforces it (hosts where the sandbox can be applied) ──────────

/// Whether this process may apply the host's sandbox at all: it cannot from
/// inside another sandbox (macOS refuses a nested Seatbelt).
fn sandbox_can_be_applied(mechanism: Mechanism) -> bool {
    let command = match mechanism {
        Mechanism::Seatbelt => probe(
            SANDBOX_EXEC,
            &["-p", "(version 1)(allow default)", "/usr/bin/true"],
            Path::new("/"),
        ),
        Mechanism::Bubblewrap => probe(BWRAP, &["--ro-bind", "/", "/", "true"], Path::new("/")),
    };
    ProcessProbeRunner.run(&command).success
}

/// The mechanism of this host when the OS sandbox can actually be applied
/// from this process; `None` (the test self-skips) otherwise.
fn live_mechanism() -> Option<Mechanism> {
    let mechanism = Mechanism::of_host().filter(|m| sandbox_can_be_applied(*m));
    if mechanism.is_none() {
        eprintln!("skipped: the OS sandbox cannot be applied from this process");
    }
    mechanism
}

/// Builds a lane whose build script is `build_rs` under the real OS sandbox.
/// The script gets `KRONN_TEST_INSIDE` (a file inside the lane, which the
/// scripts below write first: proof the sandbox lets an ordinary build
/// through) and every `(name, value)` of `extra_env`.
fn build_under_the_os_sandbox(
    mechanism: Mechanism,
    layout: &Layout,
    build_rs: &str,
    extra_env: Vec<(String, OsString)>,
) -> (ProbeOutcome, PathBuf) {
    let backend = layout.worktree.join("backend");
    std::fs::create_dir_all(backend.join("src")).unwrap();
    std::fs::write(
        backend.join("Cargo.toml"),
        "[package]\nname = \"escape\"\nversion = \"0.0.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    )
    .unwrap();
    std::fs::write(backend.join("src").join("lib.rs"), "").unwrap();
    std::fs::write(backend.join("build.rs"), build_rs).unwrap();

    let lane = WorkerLane::for_worktree(&layout.worktree).unwrap();
    let inside = layout.worktree.join("inside.txt");
    let mut command = probe("cargo", &["build", "--offline"], &backend);
    command.env = vec![
        (
            "CARGO_TARGET_DIR".into(),
            lane.cargo_target_dir.clone().into_os_string(),
        ),
        ("KRONN_TEST_INSIDE".into(), inside.clone().into_os_string()),
    ];
    command.env.extend(extra_env);
    let runner = SandboxedProbeRunner::new(
        Ok(SandboxPolicy::for_lane(&lane)),
        Some(mechanism),
        Arc::new(ProcessProbeRunner),
    );
    (runner.run(&command), inside)
}

/// A directory outside the lane and outside every temp directory.
fn outside_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(".kronn-sandbox-test-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap()
}

#[test]
fn a_build_script_writing_outside_the_lane_is_denied_by_the_os() {
    let Some(mechanism) = live_mechanism() else {
        return;
    };
    let layout = layout();
    let outside_dir = outside_dir();
    let outside = outside_dir
        .path()
        .canonicalize()
        .unwrap()
        .join("escaped.txt");
    // A global temp dir is not a lane root either.
    let global_tmp =
        PathBuf::from("/tmp").join(format!("kronn-sandbox-escape-{}", std::process::id()));

    let (outcome, inside) = build_under_the_os_sandbox(
        mechanism,
        &layout,
        r#"fn main() {
    std::fs::write(std::env::var("KRONN_TEST_INSIDE").unwrap(), "x").unwrap();
    let _ = std::fs::write(std::env::var("KRONN_TEST_GLOBAL_TMP").unwrap(), "x");
    std::fs::write(std::env::var("KRONN_TEST_OUTSIDE").unwrap(), "x")
        .expect("write outside the lane");
}
"#,
        vec![
            (
                "KRONN_TEST_OUTSIDE".into(),
                outside.clone().into_os_string(),
            ),
            (
                "KRONN_TEST_GLOBAL_TMP".into(),
                global_tmp.clone().into_os_string(),
            ),
        ],
    );
    let leaked_tmp = global_tmp.exists();
    let _ = std::fs::remove_file(&global_tmp);

    assert!(
        !outcome.success,
        "the build script's write outside the lane must fail the build: {outcome:?}"
    );
    assert!(!outside.exists(), "the escape reached the host's disk");
    assert!(!leaked_tmp, "the global /tmp was writable");
    assert!(inside.exists(), "a write inside the lane is still allowed");
}

#[test]
fn a_build_script_connecting_to_a_loopback_port_is_denied_by_the_os() {
    let Some(mechanism) = live_mechanism() else {
        return;
    };
    // Stands for the Kronn API (3140), Ollama, a local database.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let layout = layout();

    let (outcome, inside) = build_under_the_os_sandbox(
        mechanism,
        &layout,
        r#"fn main() {
    std::fs::write(std::env::var("KRONN_TEST_INSIDE").unwrap(), "x").unwrap();
    let port = std::env::var("KRONN_TEST_PORT").unwrap();
    std::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .expect("connect to a loopback port");
}
"#,
        vec![("KRONN_TEST_PORT".into(), port.to_string().into())],
    );

    assert!(
        !outcome.success,
        "the build script's loopback connection must fail the build: {outcome:?}"
    );
    assert!(inside.exists(), "an ordinary build still ran");
    assert!(
        listener.accept().is_err(),
        "the connection reached the local service"
    );
}

/// macOS only: `launchctl submit` asks launchd — over a Mach service the
/// profile does not list — to run a job outside the sandbox.
#[cfg(target_os = "macos")]
#[test]
fn a_build_script_asking_launchd_to_run_a_job_is_denied_by_the_os() {
    let Some(mechanism) = live_mechanism() else {
        return;
    };
    let layout = layout();
    let outside_dir = outside_dir();
    let marker = outside_dir.path().canonicalize().unwrap().join("job-ran");
    let label = format!("kronn.kt1255.escape.{}", std::process::id());

    let (outcome, inside) = build_under_the_os_sandbox(
        mechanism,
        &layout,
        r#"fn main() {
    std::fs::write(std::env::var("KRONN_TEST_INSIDE").unwrap(), "x").unwrap();
    let status = std::process::Command::new("/bin/launchctl")
        .args([
            "submit",
            "-l",
            &std::env::var("KRONN_TEST_LABEL").unwrap(),
            "--",
            "/usr/bin/touch",
            &std::env::var("KRONN_TEST_MARKER").unwrap(),
        ])
        .status()
        .expect("launchctl starts");
    assert!(status.success(), "launchctl submit refused");
}
"#,
        vec![
            ("KRONN_TEST_LABEL".into(), label.clone().into()),
            ("KRONN_TEST_MARKER".into(), marker.clone().into_os_string()),
        ],
    );
    std::thread::sleep(Duration::from_secs(2));
    let ran = marker.exists();
    // Best effort: never leave a job behind if the sandbox let it through.
    let _ = std::process::Command::new("/bin/launchctl")
        .args(["remove", &label])
        .output();

    assert!(
        !outcome.success,
        "the launchctl submit must fail the build: {outcome:?}"
    );
    assert!(!ran, "launchd ran a job outside the sandbox");
    assert!(inside.exists(), "an ordinary build still ran");
}
