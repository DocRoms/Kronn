use super::*;
use std::sync::Mutex;

/// A managed layout in a temp dir: `<repo>/.kronn/worktrees/task-kt-1246-abc12345`.
struct Fixture {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    worktree: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().canonicalize().unwrap();
    let worktree = repo
        .join(".kronn")
        .join("worktrees")
        .join("task-kt-1246-abc12345");
    std::fs::create_dir_all(&worktree).unwrap();
    Fixture {
        _dir: dir,
        repo,
        worktree,
    }
}

impl Fixture {
    /// The main checkout has installed frontend dependencies, with vitest.
    fn with_main_node_modules(&self) -> PathBuf {
        let bin = self.repo.join("frontend").join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("vitest"), "#!/bin/sh\n").unwrap();
        self.repo.join("frontend").join("node_modules")
    }

    /// The worktree declares a frontend and a Rust crate (as the repo does).
    fn with_project_files(&self) {
        std::fs::create_dir_all(self.worktree.join("frontend")).unwrap();
        std::fs::write(self.worktree.join("frontend").join("package.json"), "{}").unwrap();
        std::fs::create_dir_all(self.worktree.join("backend")).unwrap();
        std::fs::write(self.worktree.join("backend").join("Cargo.toml"), "").unwrap();
    }

    fn lane(&self) -> WorkerLane {
        WorkerLane::for_worktree(&self.worktree).unwrap()
    }
}

/// A fake `~/.cargo` with a registry and a git cache.
fn fake_cargo_home(with_registry: bool) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let cargo = home.path().join(".cargo");
    std::fs::create_dir_all(&cargo).unwrap();
    if with_registry {
        std::fs::create_dir_all(cargo.join("registry")).unwrap();
        std::fs::create_dir_all(cargo.join("git")).unwrap();
    }
    home
}

fn caches_of(home: &tempfile::TempDir, fx: &Fixture) -> SharedCaches {
    SharedCaches::from_roots(Some(home.path()), None, None, None, Some(&fx.repo))
}

/// Records every probe; fails the ones whose command line contains `fail_on`.
struct ScriptedRunner {
    calls: Mutex<Vec<ProbeCommand>>,
    fail_on: Option<&'static str>,
}

impl ScriptedRunner {
    fn passing() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_on: None,
        }
    }

    fn failing_on(needle: &'static str) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_on: Some(needle),
        }
    }
}

impl ProbeRunner for ScriptedRunner {
    fn run(&self, probe: &ProbeCommand) -> ProbeOutcome {
        self.calls.lock().unwrap().push(probe.clone());
        let line = format!("{} {}", probe.program, probe.args.join(" "));
        match self.fail_on {
            Some(needle) if line.contains(needle) => ProbeOutcome {
                success: false,
                detail: format!("scripted failure of `{line}`"),
            },
            _ => ProbeOutcome {
                success: true,
                detail: "1.2.3".into(),
            },
        }
    }
}

fn bind_ok() -> std::io::Result<u16> {
    Ok(40_123)
}

fn bind_refused() -> std::io::Result<u16> {
    Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
}

// ── Lane layout ──────────────────────────────────────────────────────────

#[test]
fn a_managed_worktree_gets_a_target_beside_the_repo_and_outside_backend() {
    let fx = fixture();
    let lane = fx.lane();

    assert_eq!(lane.name, "kt-1246-abc12345");
    assert_eq!(lane.main_checkout.as_deref(), Some(fx.repo.as_path()));
    assert_eq!(
        lane.cargo_target_dir,
        fx.repo.join("target").join("wt-kt-1246-abc12345")
    );
    assert!(
        !lane
            .cargo_target_dir
            .starts_with(fx.worktree.join("backend")),
        "a write under backend/ restarts the dev backend"
    );
    assert!(lane.cargo_home.starts_with(&lane.cargo_target_dir));
    assert!(lane.data_dir.starts_with(&fx.worktree));
    assert_eq!(
        lane.write_roots(),
        vec![fx.worktree.clone(), lane.cargo_target_dir.clone()]
    );
}

#[test]
fn an_unmanaged_directory_keeps_its_target_inside_itself() {
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().canonicalize().unwrap();
    let lane = WorkerLane::for_worktree(&worktree).unwrap();

    assert_eq!(lane.main_checkout, None);
    assert!(lane.cargo_target_dir.starts_with(&worktree));
    assert_eq!(lane.write_roots(), vec![worktree]);
}

#[test]
fn the_lane_id_is_a_plain_path_segment() {
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().canonicalize().unwrap().join("task-a b;c$(x)");
    let lane = WorkerLane::for_worktree(&worktree).unwrap();

    assert_eq!(lane.name, "a_b_c__x_");
    assert!(WorkerLane::for_worktree(Path::new("relative/path")).is_err());
}

// ── Environment ──────────────────────────────────────────────────────────

fn env_value(env: &[(&'static str, OsString)], name: &str) -> Option<OsString> {
    env.iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, value)| value.clone())
}

#[test]
fn a_provisioned_lane_exports_data_dir_target_offline_and_cargo_home() {
    let fx = fixture();
    let home = fake_cargo_home(true);
    let lane = fx.lane();
    let caches = caches_of(&home, &fx);
    let provisioned = lane.provision_with(&caches, false);
    let env = lane.env(&provisioned);

    assert_eq!(
        env_value(&env, KRONN_DATA_DIR_ENV),
        Some(lane.data_dir.clone().into_os_string())
    );
    assert_eq!(
        env_value(&env, CARGO_TARGET_DIR_ENV),
        Some(lane.cargo_target_dir.clone().into_os_string())
    );
    assert_eq!(
        env_value(&env, CARGO_NET_OFFLINE_ENV),
        Some(OsString::from("true"))
    );
    // The host's own home: a warm target is only reusable under the registry
    // paths it was built with (KT-1250), and the sandbox only reads it.
    assert_eq!(
        env_value(&env, CARGO_HOME_ENV),
        caches.cargo_home.clone().map(PathBuf::into_os_string)
    );
    assert!(lane.data_dir.is_dir(), "the scratch data dir exists");
    assert!(lane.cargo_target_dir.is_dir());
}

#[test]
fn a_private_cargo_home_can_still_be_requested() {
    let fx = fixture();
    let home = fake_cargo_home(true);
    let lane = fx.lane();
    let caches = caches_of(&home, &fx);

    let provisioned = lane.provision_with(&caches, true);
    let env = lane.env(&provisioned);

    assert!(provisioned.cargo_home_ready);
    assert_eq!(provisioned.shared_cargo_home, None);
    assert_eq!(
        env_value(&env, CARGO_HOME_ENV),
        Some(lane.cargo_home.clone().into_os_string())
    );
    assert!(lane.cargo_home.join("registry").exists());
}

/// A worker that starts a Kronn or a Vite gets its own ports: never the
/// host's backend (3140) or UI (5173), and its Vite proxies to its own backend.
#[test]
fn a_lane_serves_on_its_own_ports_never_the_hosts() {
    let fx = fixture();
    let home = fake_cargo_home(true);
    let lane = fx.lane();
    let env = lane.env(&lane.provision(&caches_of(&home, &fx)));
    let (backend, vite) = lane.ports();

    assert_eq!(
        env_value(&env, KRONN_PORT_ENV),
        Some(OsString::from(backend.to_string()))
    );
    assert_eq!(
        env_value(&env, KRONN_BACKEND_URL_ENV),
        Some(OsString::from(format!("http://127.0.0.1:{backend}")))
    );
    assert_eq!(
        env_value(&env, VITE_DEV_PORT_ENV),
        Some(OsString::from(vite.to_string()))
    );
    assert_ne!(backend, vite);
    assert_eq!(lane.ports(), (backend, vite), "stable for a lane");

    // Across many lanes: always in the lane range, and lanes rarely collide.
    let mut seen = std::collections::HashSet::new();
    for n in 0..200 {
        let mut other = lane.clone();
        other.name = format!("kt-{n}-abcdef12");
        let (backend, vite) = other.ports();
        for port in [backend, vite] {
            assert!((20_000..50_000).contains(&port), "{port}");
            assert!(port != 3140 && port != 5173);
        }
        seen.insert(backend);
    }
    assert!(seen.len() > 190, "lane ports spread: {}", seen.len());
}

#[test]
fn without_a_registry_cache_cargo_home_is_left_to_the_host_default() {
    let fx = fixture();
    let home = fake_cargo_home(false);
    let lane = fx.lane();
    let provisioned = lane.provision(&caches_of(&home, &fx));
    let env = lane.env(&provisioned);

    assert!(!provisioned.cargo_home_ready);
    assert_eq!(env_value(&env, CARGO_HOME_ENV), None);
    assert!(env_value(&env, CARGO_NET_OFFLINE_ENV).is_some());
}

#[test]
fn the_launch_environment_reaches_the_spawned_command() {
    let fx = fixture();
    let mut command = std::process::Command::new("true");
    apply_launch_env(&mut command, &fx.worktree);

    let envs: std::collections::BTreeMap<_, _> = command
        .get_envs()
        .filter_map(|(name, value)| Some((name.to_str()?.to_string(), value?.to_owned())))
        .collect();
    let lane = fx.lane();
    assert_eq!(
        envs.get(KRONN_DATA_DIR_ENV),
        Some(&lane.data_dir.clone().into_os_string())
    );
    assert_eq!(
        envs.get(CARGO_TARGET_DIR_ENV),
        Some(&lane.cargo_target_dir.clone().into_os_string())
    );
    assert_eq!(
        envs.get(CARGO_NET_OFFLINE_ENV),
        Some(&OsString::from("true"))
    );
}

// ── Provisioning ─────────────────────────────────────────────────────────

#[test]
fn frontend_deps_are_a_link_to_the_main_checkout_and_never_a_write_root() {
    let fx = fixture();
    fx.with_project_files();
    let source = fx.with_main_node_modules();
    let home = fake_cargo_home(true);
    let lane = fx.lane();

    let provisioned = lane.provision(&caches_of(&home, &fx));

    let link = fx.worktree.join("frontend").join("node_modules");
    assert_eq!(
        provisioned.frontend_deps,
        FrontendDeps::Linked(source.clone())
    );
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_link(&link).unwrap(), source);
    assert!(link.join(".bin").join("vitest").exists());
    // Read-only: what the link resolves to is outside every write root, so the
    // sandbox refuses a write through it.
    for root in lane.write_roots() {
        assert!(!source.starts_with(&root), "{source:?} is under {root:?}");
    }
    assert!(lane.read_roots(&caches_of(&home, &fx)).contains(&source));
}

#[test]
fn provisioning_twice_is_idempotent() {
    let fx = fixture();
    fx.with_project_files();
    fx.with_main_node_modules();
    let home = fake_cargo_home(true);
    let lane = fx.lane();
    let caches = caches_of(&home, &fx);

    let first = lane.provision(&caches);
    let second = lane.provision(&caches);

    assert!(matches!(first.frontend_deps, FrontendDeps::Linked(_)));
    // The second pass sees the first one's link and leaves it.
    assert_eq!(second.frontend_deps, FrontendDeps::AlreadyPresent);
    assert!(second.cargo_home_ready);
    assert!(second.warnings.is_empty(), "{:?}", second.warnings);
}

#[test]
fn an_existing_node_modules_directory_is_left_alone() {
    let fx = fixture();
    fx.with_project_files();
    fx.with_main_node_modules();
    let own = fx.worktree.join("frontend").join("node_modules");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("marker"), "mine").unwrap();
    let home = fake_cargo_home(true);

    let provisioned = fx.lane().provision(&caches_of(&home, &fx));

    assert_eq!(provisioned.frontend_deps, FrontendDeps::AlreadyPresent);
    assert_eq!(std::fs::read_to_string(own.join("marker")).unwrap(), "mine");
}

#[test]
fn a_missing_source_or_frontend_is_reported_not_installed() {
    let fx = fixture();
    let home = fake_cargo_home(true);
    let lane = fx.lane();
    // No frontend at all.
    assert_eq!(
        lane.provision(&caches_of(&home, &fx)).frontend_deps,
        FrontendDeps::NotApplicable
    );
    // A frontend, but nothing installed in the main checkout: no network install.
    fx.with_project_files();
    let provisioned = lane.provision(&caches_of(&home, &fx));
    assert_eq!(provisioned.frontend_deps, FrontendDeps::SourceMissing);
    assert!(!fx.worktree.join("frontend").join("node_modules").exists());
}

#[test]
fn cargo_home_links_the_caches_but_never_credentials() {
    let fx = fixture();
    let home = fake_cargo_home(true);
    let cargo = home.path().join(".cargo");
    std::fs::write(cargo.join("config.toml"), "[net]\n").unwrap();
    std::fs::write(cargo.join("credentials.toml"), "token = \"x\"\n").unwrap();
    let lane = fx.lane();

    let provisioned = lane.provision(&caches_of(&home, &fx));

    assert!(provisioned.cargo_home_ready);
    assert!(lane.cargo_home.join("registry").exists());
    assert!(lane.cargo_home.join("git").exists());
    assert!(lane.cargo_home.join("config.toml").exists());
    assert!(
        std::fs::symlink_metadata(lane.cargo_home.join("credentials.toml")).is_err(),
        "credentials are not part of the lane"
    );
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=kronn-test",
            "-c",
            "user.email=kronn-test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The lane must not make either checkout dirty: the integration preflight
/// refuses a dirty target, and a delivery requires a clean worktree.
#[test]
fn the_lane_leaves_the_main_checkout_and_the_worktree_clean() {
    let fx = fixture();
    // A project that ignores `node_modules/` (directories only) and has no
    // `/target/` rule at all: the worst case for both new paths.
    std::fs::write(fx.repo.join(".gitignore"), "node_modules/\n.kronn/\n").unwrap();
    std::fs::create_dir_all(fx.repo.join("frontend")).unwrap();
    std::fs::write(fx.repo.join("frontend").join("package.json"), "{}").unwrap();
    git(&fx.repo, &["init", "-q", "-b", "main"]);
    git(&fx.repo, &["add", "-A"]);
    git(&fx.repo, &["commit", "-q", "-m", "init"]);
    // `fixture()` pre-created the directory; git wants to create it itself.
    std::fs::remove_dir_all(&fx.worktree).unwrap();
    git(
        &fx.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "kt-1246",
            fx.worktree.to_str().unwrap(),
        ],
    );
    fx.with_main_node_modules();
    let home = fake_cargo_home(true);

    let provisioned = fx.lane().provision(&caches_of(&home, &fx));

    assert!(
        matches!(provisioned.frontend_deps, FrontendDeps::Linked(_)),
        "{provisioned:?}"
    );
    assert!(fx.lane().cargo_target_dir.is_dir());
    assert_eq!(
        git(&fx.repo, &["status", "--porcelain"]),
        "",
        "main checkout"
    );
    assert_eq!(
        git(&fx.worktree, &["status", "--porcelain"]),
        "",
        "worktree"
    );
    // Provisioning again writes nothing new.
    fx.lane().provision(&caches_of(&home, &fx));
    assert_eq!(git(&fx.repo, &["status", "--porcelain"]), "");
}

#[test]
fn shared_caches_keep_only_what_exists() {
    let fx = fixture();
    let home = tempfile::tempdir().unwrap();
    let none = SharedCaches::from_roots(Some(home.path()), None, None, None, Some(&fx.repo));
    assert_eq!(none, SharedCaches::default());

    std::fs::create_dir_all(home.path().join(".cargo").join("registry")).unwrap();
    std::fs::create_dir_all(fx.repo.join(".pnpm-store")).unwrap();
    let found = SharedCaches::from_roots(Some(home.path()), None, None, None, Some(&fx.repo));
    assert!(found.cargo_registry().is_some());
    assert_eq!(found.cargo_git(), None);
    assert_eq!(found.pnpm_stores.len(), 1);
}

// ── Smoke check ──────────────────────────────────────────────────────────

fn ready_lane() -> (Fixture, tempfile::TempDir, WorkerLane, Provisioned) {
    let fx = fixture();
    fx.with_project_files();
    fx.with_main_node_modules();
    let home = fake_cargo_home(true);
    let lane = fx.lane();
    let provisioned = lane.provision(&caches_of(&home, &fx));
    (fx, home, lane, provisioned)
}

#[test]
fn a_healthy_lane_records_every_capability_with_its_reason() {
    let (_fx, _home, lane, provisioned) = ready_lane();
    let runner = ScriptedRunner::passing();

    let capabilities = smoke_check_with(&lane, &provisioned, true, &runner, &bind_ok);

    assert!(capabilities.rust_tests.available, "{capabilities:?}");
    assert!(capabilities.frontend_tests.available, "{capabilities:?}");
    assert!(capabilities.port_binding.available, "{capabilities:?}");
    assert!(capabilities.rust_tests.reason.contains("nextest"));
    assert!(capabilities.frontend_tests.reason.contains("vitest 1.2.3"));
    assert!(capabilities.port_binding.reason.contains("40123"));

    let calls = runner.calls.lock().unwrap();
    let lines: Vec<String> = calls
        .iter()
        .map(|call| format!("{} {}", call.program, call.args.join(" ")))
        .collect();
    assert_eq!(lines[0], "cargo nextest --version");
    assert_eq!(lines[1], "cargo fetch --offline --locked");
    assert!(
        lines[2].ends_with("node_modules/.bin/vitest --version"),
        "{lines:?}"
    );
    // Every probe runs with the worker's own lane environment.
    for call in calls.iter() {
        let names: Vec<&str> = call.env.iter().map(|(name, _)| name.as_str()).collect();
        for expected in [
            KRONN_DATA_DIR_ENV,
            CARGO_TARGET_DIR_ENV,
            CARGO_NET_OFFLINE_ENV,
        ] {
            assert!(names.contains(&expected), "{call:?} lacks {expected}");
        }
    }
}

#[test]
fn a_failing_nextest_marks_rust_unavailable_and_says_why() {
    let (_fx, _home, lane, provisioned) = ready_lane();
    let runner = ScriptedRunner::failing_on("nextest");

    let capabilities = smoke_check_with(&lane, &provisioned, true, &runner, &bind_ok);

    assert!(!capabilities.rust_tests.available);
    assert!(
        capabilities
            .rust_tests
            .reason
            .contains("cargo nextest --version"),
        "{}",
        capabilities.rust_tests.reason
    );
    assert!(
        capabilities.frontend_tests.available,
        "frontend is independent"
    );
    // The offline fetch is not attempted once nextest is known missing.
    assert_eq!(runner.calls.lock().unwrap().len(), 2);
}

#[test]
fn an_offline_cache_gap_marks_rust_unavailable() {
    let (_fx, _home, lane, provisioned) = ready_lane();
    let runner = ScriptedRunner::failing_on("fetch");

    let capabilities = smoke_check_with(&lane, &provisioned, true, &runner, &bind_ok);

    assert!(!capabilities.rust_tests.available);
    assert!(capabilities
        .rust_tests
        .reason
        .contains("offline dependency resolution failed"));
}

#[test]
fn a_failing_vitest_marks_frontend_unavailable() {
    let (_fx, _home, lane, provisioned) = ready_lane();
    let runner = ScriptedRunner::failing_on("vitest");

    let capabilities = smoke_check_with(&lane, &provisioned, true, &runner, &bind_ok);

    assert!(!capabilities.frontend_tests.available);
    assert!(capabilities
        .frontend_tests
        .reason
        .contains("vitest --version"));
    assert!(capabilities.rust_tests.available);
}

#[test]
fn port_binding_needs_both_the_policy_and_a_working_bind() {
    let (_fx, _home, lane, provisioned) = ready_lane();
    let runner = ScriptedRunner::passing();

    let denied = smoke_check_with(&lane, &provisioned, false, &runner, &bind_ok);
    assert!(!denied.port_binding.available);
    assert!(denied.port_binding.reason.contains("policy"));

    let refused = smoke_check_with(&lane, &provisioned, true, &runner, &bind_refused);
    assert!(!refused.port_binding.available);
    assert!(refused
        .port_binding
        .reason
        .contains("binding 127.0.0.1 failed"));
}

#[test]
fn without_installed_frontend_deps_the_reason_says_no_network_install() {
    let fx = fixture();
    fx.with_project_files();
    let home = fake_cargo_home(true);
    let lane = fx.lane();
    let provisioned = lane.provision(&caches_of(&home, &fx));
    let runner = ScriptedRunner::passing();

    let capabilities = smoke_check_with(&lane, &provisioned, true, &runner, &bind_ok);

    assert!(!capabilities.frontend_tests.available);
    assert!(capabilities
        .frontend_tests
        .reason
        .contains("never installs from the network"));
}

#[test]
fn a_project_without_cargo_or_frontend_reports_not_applicable() {
    let fx = fixture();
    let home = fake_cargo_home(true);
    let lane = fx.lane();
    let provisioned = lane.provision(&caches_of(&home, &fx));
    let runner = ScriptedRunner::passing();

    let capabilities = smoke_check_with(&lane, &provisioned, true, &runner, &bind_ok);

    assert!(capabilities.rust_tests.reason.starts_with("not applicable"));
    assert!(capabilities
        .frontend_tests
        .reason
        .starts_with("not applicable"));
    assert!(runner.calls.lock().unwrap().is_empty());
}

#[test]
fn an_unpreparable_lane_is_unverified_not_an_error() {
    let scratch = tempfile::tempdir().unwrap();
    let missing = scratch.path().join("kronn-kt-1246-does-not-exist");
    let capabilities = check_worktree(&missing, true, &ScriptedRunner::passing());

    assert!(!capabilities.any_available());
    assert!(capabilities
        .rust_tests
        .reason
        .contains("could not be prepared"));
}

#[test]
fn the_capability_record_roundtrips_as_json() {
    let capabilities = ToolchainCapabilities {
        rust_tests: Capability::yes("ok"),
        frontend_tests: Capability::no("no deps"),
        port_binding: Capability::yes("bound"),
    };
    let json = serde_json::to_value(&capabilities).unwrap();
    assert_eq!(json["frontend_tests"]["available"], false);
    assert_eq!(json["frontend_tests"]["reason"], "no deps");
    let back: ToolchainCapabilities = serde_json::from_value(json).unwrap();
    assert_eq!(back, capabilities);
}

#[test]
fn the_process_runner_reports_success_failure_and_a_missing_program() {
    let dir = tempfile::tempdir().unwrap();
    let probe = |program: &str, args: &[&str]| ProbeCommand {
        program: program.into(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: dir.path().to_path_buf(),
        env: Vec::new(),
        timeout: Duration::from_secs(10),
    };
    let runner = ProcessProbeRunner;

    let ok = runner.run(&probe("cargo", &["--version"]));
    assert!(ok.success, "{ok:?}");
    assert!(ok.detail.contains("cargo"), "{ok:?}");

    let failed = runner.run(&probe("cargo", &["definitely-not-a-subcommand"]));
    assert!(!failed.success);

    let missing = runner.run(&probe("kronn-no-such-program-kt-1246", &[]));
    assert!(!missing.success);
    assert!(missing.detail.contains("cannot start"), "{missing:?}");
}
