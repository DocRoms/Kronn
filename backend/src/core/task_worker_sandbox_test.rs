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
    assert_eq!(profile.matches("(allow file-write").count(), 1, "{profile}");
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

/// The text of the `(deny …)` forms of `profile`.
fn deny_forms(profile: &str) -> Vec<&str> {
    profile
        .match_indices("(deny ")
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

/// KT-1255 review: the profile used to write `^/dev/tty`, grant `pseudo-tty`
/// and allow any `file-ioctl` on a terminal, so a build script that kept the
/// backend's controlling terminal could `TIOCSTI` keystrokes into the
/// operator's shell.
#[test]
fn the_seatbelt_profile_cannot_write_or_ioctl_a_terminal() {
    let profile = seatbelt_profile(&[PathBuf::from("/lane/worktree")]);

    // No tty node on any write allowance, no pty grant.
    let writes = allow_forms(&profile, "file-write*");
    assert_eq!(writes.len(), 1, "{profile}");
    assert!(!writes[0].contains("/dev/tty"), "{}", writes[0]);
    assert!(!writes[0].contains("/dev/pty"), "{}", writes[0]);
    assert!(!writes[0].contains("/dev/console"), "{}", writes[0]);
    assert!(!profile.contains("pseudo-tty"), "{profile}");
    // The device nodes a build writes to are the only ones allowed.
    for node in ["/dev/null", "/dev/zero", "/dev/dtracehelper"] {
        assert!(
            writes[0].contains(&format!("(literal \"{node}\")")),
            "{node}: {}",
            writes[0]
        );
    }

    // A closing deny, the last rule, covers read, write and ioctl on every
    // terminal vnode and tty node by name.
    let denies: Vec<&str> = deny_forms(&profile)
        .into_iter()
        .filter(|form| *form != "(deny default)")
        .collect();
    assert_eq!(denies.len(), 1, "{profile}");
    let deny = denies[0];
    for operation in ["file-read-data", "file-write*", "file-ioctl"] {
        assert!(deny.contains(operation), "{operation} in {deny}");
    }
    for filter in [
        "(vnode-type TTY)",
        "(literal \"/dev/tty\")",
        "(regex #\"^/dev/tty\")",
        "(regex #\"^/dev/pty\")",
        "(literal \"/dev/ptmx\")",
        "(literal \"/dev/console\")",
    ] {
        assert!(deny.contains(filter), "{filter} in {deny}");
    }
    let last_rule = profile
        .lines()
        .rfind(|line| line.starts_with('('))
        .expect("a last rule");
    assert!(
        last_rule.starts_with("(deny file-read-data"),
        "the tty deny must be the final rule (the last match wins):\n{profile}"
    );
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

/// The deny-default profile must let an ordinary build through: otherwise
/// every warm start would silently turn cold.
#[test]
fn an_ordinary_build_succeeds_under_the_os_sandbox() {
    let Some(mechanism) = live_mechanism() else {
        return;
    };
    let layout = layout();

    let (outcome, inside) = build_under_the_os_sandbox(
        mechanism,
        &layout,
        r#"fn main() {
    std::fs::write(std::env::var("KRONN_TEST_INSIDE").unwrap(), "x").unwrap();
    let out = std::env::var("OUT_DIR").unwrap();
    std::fs::write(std::path::Path::new(&out).join("generated.rs"), "").unwrap();
}
"#,
        Vec::new(),
    );

    assert!(
        outcome.success,
        "an ordinary build must succeed under the profile: {outcome:?}"
    );
    assert!(inside.exists(), "the build script ran inside the lane");
    let lane = WorkerLane::for_worktree(&layout.worktree).unwrap();
    assert!(
        lane.cargo_target_dir.join("debug").exists(),
        "the compiled artifacts landed in the lane's target"
    );
}

/// A pty the test owns stands for the operator's terminal: the build script is
/// handed its path and must neither open it for writing nor reach the master.
#[cfg(unix)]
#[test]
fn a_build_script_writing_to_a_terminal_is_denied_by_the_os() {
    let Some(mechanism) = live_mechanism() else {
        return;
    };
    // SAFETY: plain libc pty calls on a descriptor this test owns.
    let (master, slave_path) = unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(master >= 0, "posix_openpt");
        assert_eq!(libc::grantpt(master), 0);
        assert_eq!(libc::unlockpt(master), 0);
        let name = std::ffi::CStr::from_ptr(libc::ptsname(master));
        (master, name.to_string_lossy().into_owned())
    };
    let layout = layout();

    let (outcome, inside) = build_under_the_os_sandbox(
        mechanism,
        &layout,
        r#"fn main() {
    std::fs::write(std::env::var("KRONN_TEST_INSIDE").unwrap(), "x").unwrap();
    let tty = std::env::var("KRONN_TEST_TTY").unwrap();
    let mut slave = std::fs::OpenOptions::new()
        .write(true)
        .open(&tty)
        .expect("open the terminal");
    std::io::Write::write_all(&mut slave, b"injected\n").expect("write the terminal");
}
"#,
        vec![("KRONN_TEST_TTY".into(), slave_path.into())],
    );
    let mut received = [0u8; 64];
    // SAFETY: nonblocking read of the master this test owns.
    let read = unsafe {
        let flags = libc::fcntl(master, libc::F_GETFL);
        libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
        libc::read(master, received.as_mut_ptr().cast(), received.len())
    };
    // SAFETY: closes the descriptor opened above.
    unsafe { libc::close(master) };

    assert!(
        !outcome.success,
        "the build script's terminal write must fail the build: {outcome:?}"
    );
    assert!(inside.exists(), "an ordinary build still ran");
    assert!(read <= 0, "bytes reached the terminal: {read}");
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

// ── No controlling terminal ──────────────────────────────────────────────

/// Helper of `a_probe_runs_in_a_session_of_its_own`, re-run by that test as a
/// child process: it reports its own session and process ids.
#[cfg(unix)]
#[test]
fn report_session_ids_for_the_session_test() {
    if std::env::var_os("KRONN_REPORT_SESSION").is_none() {
        return;
    }
    // SAFETY: `getsid(0)` and `getpid` take no pointer.
    let (sid, pid) = unsafe { (libc::getsid(0), libc::getpid()) };
    println!("reported sid={sid} pid={pid}");
}

/// KT-1255 review: a probe that only led its own process *group* kept the
/// backend's session, hence its controlling terminal, and `TIOCSTI` could type
/// into the operator's shell. `SandboxedProbeRunner` delegates to this runner,
/// so every confined probe starts through `setsid`.
#[cfg(unix)]
#[test]
fn a_probe_runs_in_a_session_of_its_own() {
    let module = module_path!()
        .split_once("::")
        .map(|(_, path)| path)
        .unwrap();
    let test_name = format!("{module}::report_session_ids_for_the_session_test");
    let exe = std::env::current_exe().unwrap();
    let mut command = probe(
        exe.to_str().unwrap(),
        &["--exact", &test_name, "--nocapture", "--test-threads=1"],
        &std::env::temp_dir(),
    );
    command.env = vec![("KRONN_REPORT_SESSION".into(), "1".into())];

    let outcome = ProcessProbeRunner.run(&command);
    assert!(outcome.success, "{outcome:?}");
    let field = |name: &str| -> i64 {
        let marker = format!("{name}=");
        let start = outcome
            .detail
            .find(&marker)
            .unwrap_or_else(|| panic!("no {name} in {:?}", outcome.detail))
            + marker.len();
        outcome.detail[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap()
    };
    let (sid, pid) = (field("sid"), field("pid"));
    // SAFETY: `getsid(0)` takes no pointer.
    let our_sid = unsafe { libc::getsid(0) } as i64;
    assert_eq!(sid, pid, "the probe leads its own session: {outcome:?}");
    assert_ne!(sid, our_sid, "the probe left the backend's session");
}

// ── Quick Exec validations of a worker's tree (KT-1268) ──────────────────

#[test]
fn a_host_without_a_sandbox_gets_no_confinement_and_provisions_nothing() {
    let layout = layout();
    let lane = WorkerLane::for_worktree(&layout.worktree).unwrap();

    let refusal = Confinement::with_mechanism(&layout.worktree, None).unwrap_err();

    assert!(refusal.contains("never run unconfined"), "{refusal}");
    assert!(refusal.contains("no sandbox"), "{refusal}");
    assert!(
        !lane.cargo_target_dir.exists(),
        "a refused host must not have a lane prepared for it"
    );
}

/// KT-1268 review: Linux names `bwrap` whether or not it is installed (the
/// backend image does not ship it). A launcher that is missing must be refused
/// for that reason, before anything is prepared or started, not surface as a
/// `cargo could not be spawned` of the validation.
#[test]
fn a_sandbox_launcher_that_is_not_installed_is_refused_with_its_name_and_nothing_runs() {
    for mechanism in [Mechanism::Bubblewrap, Mechanism::Seatbelt] {
        let layout = layout();
        let lane = WorkerLane::for_worktree(&layout.worktree).unwrap();
        let missing = "/nonexistent/kronn-sandbox-launcher";

        let refusal = Confinement::with_usable_mechanism(&layout.worktree, Some(mechanism), |m| {
            m.check_launcher(missing)
        })
        .unwrap_err();

        assert!(refusal.contains(missing), "{refusal}");
        assert!(refusal.contains("not installed"), "{refusal}");
        assert!(refusal.contains("never run unconfined"), "{refusal}");
        assert!(
            !refusal.contains("could not be spawned"),
            "the refusal is not a cargo spawn error: {refusal}"
        );
        assert!(
            !lane.cargo_target_dir.exists(),
            "an unusable sandbox must not have a lane prepared for it"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_sandbox_launcher_that_cannot_apply_its_sandbox_is_refused_with_its_exit_status() {
    let layout = layout();

    let refusal =
        Confinement::with_usable_mechanism(&layout.worktree, Some(Mechanism::Bubblewrap), |m| {
            m.check_launcher("/usr/bin/false")
        })
        .unwrap_err();

    assert!(refusal.contains("exited with"), "{refusal}");
    assert!(refusal.contains("never run unconfined"), "{refusal}");
}

/// A launcher the host accepts yields the confinement; the check is the real
/// profile, so it agrees with whether the OS applies a sandbox from here.
#[test]
fn the_host_mechanism_is_confined_when_the_os_applies_it_and_refused_otherwise() {
    let layout = layout();
    let Some(mechanism) = Mechanism::of_host() else {
        return;
    };
    let outcome = Confinement::with_usable_mechanism(
        &layout.worktree,
        Some(mechanism),
        Mechanism::check_usable,
    );
    if sandbox_can_be_applied(mechanism) {
        assert!(outcome.is_ok(), "{:?}", outcome.err());
    } else {
        let refusal = outcome.unwrap_err();
        assert!(refusal.contains("never run unconfined"), "{refusal}");
    }
}

#[test]
fn a_confinement_wraps_a_command_in_the_lane_profile_with_the_lane_environment() {
    let layout = layout();
    let lane = WorkerLane::for_worktree(&layout.worktree).unwrap();
    for mechanism in [Mechanism::Seatbelt, Mechanism::Bubblewrap] {
        let confinement = Confinement::with_mechanism(&layout.worktree, Some(mechanism)).unwrap();
        assert_eq!(confinement.policy(), &SandboxPolicy::for_lane(&lane));

        let mut command = probe("cargo", &["test", "--no-run"], &layout.worktree);
        command.env = vec![("CARGO_TARGET_DIR".into(), "/explicit/target".into())];
        let wrapped = confinement.wrap(&command);

        // The command is the sandbox mechanism's argument, never a program of its own.
        match mechanism {
            Mechanism::Seatbelt => {
                assert_eq!(wrapped.program, SANDBOX_EXEC);
                assert!(wrapped.args[1].contains("(deny default)"), "{wrapped:?}");
            }
            Mechanism::Bubblewrap => {
                assert_eq!(wrapped.program, BWRAP);
                assert!(wrapped.args.contains(&"--unshare-net".to_string()));
            }
        }
        assert_eq!(
            &wrapped.args[wrapped.args.len() - 3..],
            ["cargo", "test", "--no-run"]
        );
        assert_eq!(wrapped.cwd, layout.worktree);

        let value = |name: &str| -> Vec<OsString> {
            wrapped
                .env
                .iter()
                .filter(|(set, _)| set == name)
                .map(|(_, value)| value.clone())
                .collect()
        };
        // The lane's own environment is there, below what the probe sets.
        assert_eq!(
            value("CARGO_TARGET_DIR"),
            vec![OsString::from("/explicit/target")]
        );
        assert_eq!(value("CARGO_NET_OFFLINE"), vec![OsString::from("true")]);
        assert_eq!(
            value("KRONN_DATA_DIR"),
            vec![lane.data_dir.clone().into_os_string()]
        );
        // And the only scratch space is the lane's.
        assert_eq!(
            value("TMPDIR"),
            vec![real_path(&lane.tmp_dir()).into_os_string()]
        );
    }
}

/// `binary argv` validated as a Quick Exec spec rooted at `cwd`.
fn quick_spec(
    binary: &str,
    argv: &[&str],
    cwd: &Path,
    timeout_secs: u64,
) -> crate::core::quick_exec::ValidatedSpec {
    crate::core::quick_exec::validate(
        &crate::core::quick_exec::QuickExecSpec {
            binary: binary.into(),
            argv: argv.iter().map(|arg| arg.to_string()).collect(),
            cwd: cwd.to_path_buf(),
            timeout_secs: Some(timeout_secs),
            stdin: None,
            summariser: crate::core::quick_exec::Summariser::Generic,
        },
        &[cwd.to_path_buf()],
    )
    .unwrap()
}

fn run_confined_blocking(
    spec: &crate::core::quick_exec::ValidatedSpec,
    confinement: &Confinement,
) -> crate::core::quick_exec::QuickExecResult {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(crate::core::quick_exec::run_confined(
            spec,
            None,
            &CancellationToken::new(),
            confinement,
        ))
        .unwrap()
}

/// A crate at the root of the layout's worktree whose build script is
/// `build_rs`, built as a Quick Exec validation under the real OS sandbox.
fn validate_under_the_os_sandbox(
    mechanism: Mechanism,
    layout: &Layout,
    build_rs: &str,
) -> crate::core::quick_exec::QuickExecResult {
    let root = &layout.worktree;
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"escape\"\nversion = \"0.0.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src").join("lib.rs"), "").unwrap();
    std::fs::write(root.join("build.rs"), build_rs).unwrap();

    let confinement = Confinement::with_mechanism(root, Some(mechanism)).unwrap();
    run_confined_blocking(
        &quick_spec("cargo", &["build", "--offline"], root, 300),
        &confinement,
    )
}

#[test]
fn a_validation_build_script_writing_outside_the_lane_is_denied_by_the_os() {
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
    let inside = layout.worktree.join("inside.txt");

    let result = validate_under_the_os_sandbox(
        mechanism,
        &layout,
        &format!(
            "fn main() {{\n    std::fs::write({inside:?}, \"x\").unwrap();\n    \
             std::fs::write({outside:?}, \"x\").expect(\"write outside the lane\");\n}}\n"
        ),
    );

    assert!(!result.status.is_success(), "{result:?}");
    assert!(!outside.exists(), "the escape reached the host's disk");
    assert!(
        inside.exists(),
        "the validation ran and wrote inside its lane"
    );
}

#[test]
fn a_validation_build_script_connecting_to_loopback_is_denied_by_the_os() {
    let Some(mechanism) = live_mechanism() else {
        return;
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let layout = layout();
    let inside = layout.worktree.join("inside.txt");

    let result = validate_under_the_os_sandbox(
        mechanism,
        &layout,
        &format!(
            "fn main() {{\n    std::fs::write({inside:?}, \"x\").unwrap();\n    \
             std::net::TcpStream::connect(\"127.0.0.1:{port}\").expect(\"loopback\");\n}}\n"
        ),
    );

    assert!(!result.status.is_success(), "{result:?}");
    assert!(inside.exists(), "the validation ran inside its lane");
    assert!(
        listener.accept().is_err(),
        "the connection reached the local service"
    );
}

#[test]
fn an_ordinary_validation_build_passes_under_the_os_sandbox() {
    let Some(mechanism) = live_mechanism() else {
        return;
    };
    let layout = layout();

    let result = validate_under_the_os_sandbox(
        mechanism,
        &layout,
        "fn main() {\n    let out = std::env::var(\"OUT_DIR\").unwrap();\n    \
         std::fs::write(std::path::Path::new(&out).join(\"generated.rs\"), \"\").unwrap();\n}\n",
    );

    assert!(result.status.is_success(), "{result:?}");
    let lane = WorkerLane::for_worktree(&layout.worktree).unwrap();
    assert!(
        lane.cargo_target_dir.join("debug").exists(),
        "the artifacts landed in the lane's target"
    );
}

/// A timeout ends a confined command promptly and reports it as a timeout.
#[test]
fn a_confined_validation_that_times_out_is_reported_timed_out() {
    let Some(mechanism) = live_mechanism() else {
        return;
    };
    let layout = layout();
    let confinement = Confinement::with_mechanism(&layout.worktree, Some(mechanism)).unwrap();

    let started = std::time::Instant::now();
    let result = run_confined_blocking(
        &quick_spec("sleep", &["30"], &layout.worktree, 1),
        &confinement,
    );

    assert_eq!(
        result.status,
        crate::core::quick_exec::QuickExecStatus::TimedOut,
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(20), "{result:?}");
}
