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
}

#[test]
fn the_seatbelt_profile_denies_every_write_but_the_roots_and_keeps_the_network_on_loopback() {
    let roots = vec![
        PathBuf::from("/lane/worktree"),
        PathBuf::from("/lane/target"),
    ];
    let profile = seatbelt_profile(&roots);

    let deny = profile
        .find("(deny file-write*)")
        .expect("global write deny");
    let allow = profile.find("(allow file-write*").expect("root allowance");
    assert!(deny < allow, "the allowance follows the deny:\n{profile}");
    assert!(profile.contains("(subpath \"/lane/worktree\")"));
    assert!(profile.contains("(subpath \"/lane/target\")"));
    assert!(
        !profile.contains("(subpath \"/Users"),
        "no host path: {profile}"
    );
    let deny_network = profile.find("(deny network*)").expect("network deny");
    assert!(profile[deny_network..].contains("\"localhost:*\""));
    // No blanket network allowance after the deny.
    assert!(!profile[deny_network..].contains("(allow network*)"));
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
    assert!(args.contains(&"--unshare-net".to_string()));
    assert_eq!(args.last().unwrap(), "--");
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
    original.env = vec![("CARGO_TARGET_DIR".into(), "/lane/target".into())];

    for mechanism in [Mechanism::Seatbelt, Mechanism::Bubblewrap] {
        let confined = confine(mechanism, &policy, &original);
        assert_ne!(confined.program, "cargo");
        assert_eq!(
            &confined.args[confined.args.len() - 3..],
            ["cargo", "test", "--no-run"]
        );
        assert_eq!(confined.cwd, original.cwd);
        assert_eq!(confined.env, original.env);
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
        assert!(call.args[1].contains("(deny file-write*)"));
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

#[test]
fn a_build_script_writing_outside_the_lane_is_denied_by_the_os() {
    let Some(mechanism) = Mechanism::of_host().filter(|m| sandbox_can_be_applied(*m)) else {
        eprintln!("skipped: the OS sandbox cannot be applied from this process");
        return;
    };
    let layout = layout();
    // Outside every root: not under the lane, nor under a temp directory.
    let outside_dir = tempfile::Builder::new()
        .prefix(".kronn-sandbox-test-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap();
    let outside = outside_dir
        .path()
        .canonicalize()
        .unwrap()
        .join("escaped.txt");
    let inside = layout.worktree.join("inside.txt");

    let backend = layout.worktree.join("backend");
    std::fs::create_dir_all(backend.join("src")).unwrap();
    std::fs::write(
        backend.join("Cargo.toml"),
        "[package]\nname = \"escape\"\nversion = \"0.0.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    )
    .unwrap();
    std::fs::write(backend.join("src").join("lib.rs"), "").unwrap();
    std::fs::write(
        backend.join("build.rs"),
        "fn main() {\n\
         \x20   std::fs::write(std::env::var(\"KRONN_TEST_INSIDE\").unwrap(), \"x\").unwrap();\n\
         \x20   std::fs::write(std::env::var(\"KRONN_TEST_OUTSIDE\").unwrap(), \"x\")\n\
         \x20       .expect(\"write outside the lane\");\n\
         }\n",
    )
    .unwrap();

    let lane = WorkerLane::for_worktree(&layout.worktree).unwrap();
    let mut command = probe("cargo", &["build", "--offline"], &backend);
    command.env = vec![
        (
            "CARGO_TARGET_DIR".into(),
            lane.cargo_target_dir.clone().into_os_string(),
        ),
        ("KRONN_TEST_INSIDE".into(), inside.clone().into_os_string()),
        (
            "KRONN_TEST_OUTSIDE".into(),
            outside.clone().into_os_string(),
        ),
    ];
    let runner = SandboxedProbeRunner::new(
        Ok(SandboxPolicy::for_lane(&lane)),
        Some(mechanism),
        Arc::new(ProcessProbeRunner),
    );

    let outcome = runner.run(&command);

    assert!(
        !outcome.success,
        "the build script's write outside the lane must fail the build: {outcome:?}"
    );
    assert!(!outside.exists(), "the escape reached the host's disk");
    assert!(inside.exists(), "a write inside the lane is still allowed");
}
