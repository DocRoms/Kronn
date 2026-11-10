use super::*;
use crate::core::task_worker_env::ProbeOutcome;
use std::sync::Mutex;

const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// A runner that answers each command by its arguments and remembers it.
struct FakeRunner {
    offline_ok: bool,
    online_ok: bool,
    seen: Mutex<Vec<ProbeCommand>>,
}

impl FakeRunner {
    fn new(offline_ok: bool, online_ok: bool) -> Self {
        Self {
            offline_ok,
            online_ok,
            seen: Mutex::new(Vec::new()),
        }
    }

    fn commands(&self) -> Vec<ProbeCommand> {
        self.seen.lock().unwrap().clone()
    }
}

impl ProbeRunner for FakeRunner {
    fn run(&self, probe: &ProbeCommand) -> ProbeOutcome {
        self.seen.lock().unwrap().push(probe.clone());
        let offline = probe.args.iter().any(|arg| arg == "--offline");
        let success = if offline {
            self.offline_ok
        } else {
            self.online_ok
        };
        ProbeOutcome {
            success,
            detail: if success { "" } else { "boom" }.into(),
        }
    }
}

fn cargo_lock(sources: &[(&str, &str)]) -> String {
    let mut lock = String::from("version = 4\n");
    for (name, source) in sources {
        lock.push_str(&format!(
            "\n[[package]]\nname = \"{name}\"\nversion = \"1.0.0\"\nsource = \"{source}\"\n"
        ));
    }
    lock
}

struct Tree {
    _dir: tempfile::TempDir,
    worktree: PathBuf,
    manifest_dir: PathBuf,
    cargo_home: PathBuf,
}

fn tree(lock: Option<&str>) -> Tree {
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().canonicalize().unwrap().join("wt");
    let manifest_dir = worktree.join("backend");
    std::fs::create_dir_all(&manifest_dir).unwrap();
    std::fs::write(manifest_dir.join("Cargo.toml"), "").unwrap();
    if let Some(lock) = lock {
        std::fs::write(manifest_dir.join("Cargo.lock"), lock).unwrap();
    }
    let cargo_home = dir.path().canonicalize().unwrap().join("cargo-home");
    std::fs::create_dir_all(&cargo_home).unwrap();
    Tree {
        worktree,
        manifest_dir,
        cargo_home,
        _dir: dir,
    }
}

fn run_cargo(tree: &Tree, runner: &FakeRunner) -> Option<DepFetchStep> {
    fetch_cargo(
        &tree.worktree,
        &tree.manifest_dir,
        Some(&tree.cargo_home),
        runner,
        &CancellationToken::new(),
    )
}

fn env_of(command: &ProbeCommand, name: &str) -> Option<OsString> {
    command
        .env
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

// ─── Cargo.lock ───────────────────────────────────────────────────────────

#[test]
fn a_lock_of_crates_io_packages_is_scanned_with_its_registry() {
    let lock = cargo_lock(&[("serde", CRATES_IO), ("tokio", CRATES_IO)]);
    let scan = scan_cargo_lock(&lock).unwrap();
    assert_eq!(scan.registry_packages, 2);
    assert_eq!(scan.registries, vec![CRATES_IO.to_string()]);
}

#[test]
fn the_sparse_crates_io_index_is_allowed_too() {
    let lock = cargo_lock(&[("serde", "sparse+https://index.crates.io/")]);
    assert_eq!(scan_cargo_lock(&lock).unwrap().registry_packages, 1);
}

#[test]
fn a_path_only_lock_has_nothing_to_fetch() {
    let lock = "version = 4\n\n[[package]]\nname = \"fixture\"\nversion = \"0.0.0\"\n";
    assert_eq!(scan_cargo_lock(lock).unwrap().registry_packages, 0);
    assert_eq!(
        scan_cargo_lock("version = 4\n").unwrap().registry_packages,
        0
    );
}

#[test]
fn a_git_dependency_or_another_registry_refuses_the_whole_lock() {
    for source in [
        "git+https://evil.example/x.git?rev=abc#abc",
        "registry+https://evil.example/index",
        "sparse+https://evil.example/",
    ] {
        let lock = cargo_lock(&[("serde", CRATES_IO), ("sneaky", source)]);
        let error = scan_cargo_lock(&lock).unwrap_err();
        assert!(error.contains("sneaky"), "{error}");
        assert!(error.contains(source), "{error}");
    }
}

#[test]
fn a_lock_that_is_not_toml_is_refused() {
    assert!(scan_cargo_lock("[[package\nsource =").is_err());
}

#[test]
fn a_source_hidden_in_an_inline_table_is_still_seen() {
    let lock = "version = 4\npackage = [{ name = \"x\", version = \"1\", source = \"git+https://evil.example/x\" }]\n";
    assert!(scan_cargo_lock(lock).is_err());
}

// ─── cargo step ───────────────────────────────────────────────────────────

#[test]
fn a_cache_that_holds_everything_never_reaches_the_network() {
    let tree = tree(Some(&cargo_lock(&[("serde", CRATES_IO)])));
    let runner = FakeRunner::new(true, false);
    let step = run_cargo(&tree, &runner).unwrap();
    assert_eq!(step.outcome, FetchOutcome::Cached);
    let commands = runner.commands();
    assert_eq!(commands.len(), 1, "only the offline probe ran");
    assert!(commands[0].args.contains(&"--offline".to_string()));
}

#[test]
fn a_missing_crate_is_fetched_from_the_lock_only() {
    let tree = tree(Some(&cargo_lock(&[("utoipa-swagger-ui", CRATES_IO)])));
    let runner = FakeRunner::new(false, true);
    let step = run_cargo(&tree, &runner).unwrap();
    assert_eq!(step.outcome, FetchOutcome::Fetched);
    assert_eq!(step.ecosystem, Ecosystem::Cargo);
    assert_eq!(step.lockfile, "backend/Cargo.lock");
    assert_eq!(step.registries, vec![CRATES_IO.to_string()]);

    let commands = runner.commands();
    assert_eq!(commands.len(), 2);
    let online = &commands[1];
    assert_eq!(online.program, "cargo");
    assert_eq!(online.args[0], "fetch");
    assert!(online.args.contains(&"--locked".to_string()));
    assert!(!online.args.contains(&"--offline".to_string()));
    let manifest = tree.manifest_dir.join("Cargo.toml");
    assert!(online
        .args
        .contains(&manifest.to_string_lossy().into_owned()));
    assert_eq!(env_of(online, "CARGO_NET_OFFLINE"), Some("false".into()));
    assert_eq!(
        env_of(online, "CARGO_HOME"),
        Some(tree.cargo_home.clone().into_os_string())
    );
}

#[test]
fn cargo_never_runs_where_the_worker_can_write_its_configuration() {
    let tree = tree(Some(&cargo_lock(&[("serde", CRATES_IO)])));
    let runner = FakeRunner::new(false, true);
    run_cargo(&tree, &runner).unwrap();
    for command in runner.commands() {
        assert_eq!(command.cwd, tree.cargo_home);
        assert!(!command.cwd.starts_with(&tree.worktree));
    }
}

#[test]
fn a_lock_with_a_git_dependency_fetches_nothing() {
    let tree = tree(Some(&cargo_lock(&[(
        "sneaky",
        "git+https://evil.example/x.git#abc",
    )])));
    let runner = FakeRunner::new(false, true);
    let step = run_cargo(&tree, &runner).unwrap();
    assert!(matches!(step.outcome, FetchOutcome::Refused { .. }));
    assert!(step.registries.is_empty());
    assert!(runner.commands().is_empty(), "no command, so no network");
}

#[test]
fn a_failed_fetch_is_recorded_with_its_detail() {
    let tree = tree(Some(&cargo_lock(&[("serde", CRATES_IO)])));
    let step = run_cargo(&tree, &FakeRunner::new(false, false)).unwrap();
    assert_eq!(
        step.outcome,
        FetchOutcome::Failed {
            detail: "boom".into()
        }
    );
}

#[test]
fn a_lock_without_registry_packages_or_without_a_lock_is_no_step() {
    let path_only = tree(Some("version = 4\n"));
    let runner = FakeRunner::new(false, true);
    assert!(run_cargo(&path_only, &runner).is_none());
    assert!(run_cargo(&tree(None), &runner).is_none());
    assert!(runner.commands().is_empty());
}

#[test]
fn a_host_without_a_cargo_home_has_no_cache_to_fill() {
    let tree = tree(Some(&cargo_lock(&[("serde", CRATES_IO)])));
    let runner = FakeRunner::new(false, true);
    let step = fetch_cargo(
        &tree.worktree,
        &tree.manifest_dir,
        None,
        &runner,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(matches!(step.outcome, FetchOutcome::Refused { .. }));
    assert!(runner.commands().is_empty());
}

#[test]
fn a_cancelled_launch_fetches_nothing() {
    let tree = tree(Some(&cargo_lock(&[("serde", CRATES_IO)])));
    let runner = FakeRunner::new(false, true);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let step = fetch_cargo(
        &tree.worktree,
        &tree.manifest_dir,
        Some(&tree.cargo_home),
        &runner,
        &cancel,
    )
    .unwrap();
    assert_eq!(step.outcome, FetchOutcome::Cancelled);
    assert!(runner.commands().is_empty());
}

// ─── pnpm ─────────────────────────────────────────────────────────────────

const PNPM_LOCK_OK: &str = "lockfileVersion: '9.0'\n\nimporters:\n  .: {}\n\npackages:\n\n  \
    left-pad@1.3.0:\n    resolution: {integrity: sha512-abc}\n";

struct PnpmTree {
    _dir: tempfile::TempDir,
    worktree: PathBuf,
    frontend: PathBuf,
    store: PathBuf,
    scratch_root: PathBuf,
}

fn pnpm_tree(lock: &str, workspace: Option<&str>) -> PnpmTree {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let worktree = root.join("wt");
    let frontend = worktree.join("frontend");
    std::fs::create_dir_all(&frontend).unwrap();
    std::fs::write(frontend.join("pnpm-lock.yaml"), lock).unwrap();
    // The worker's own `.npmrc` must never reach the fetch.
    std::fs::write(frontend.join(".npmrc"), "registry=https://evil.example/\n").unwrap();
    if let Some(workspace) = workspace {
        std::fs::write(frontend.join("pnpm-workspace.yaml"), workspace).unwrap();
    }
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let scratch_root = root.join("scratch");
    std::fs::create_dir_all(&scratch_root).unwrap();
    PnpmTree {
        _dir: dir,
        worktree,
        frontend,
        store,
        scratch_root,
    }
}

fn run_pnpm(tree: &PnpmTree, runner: &FakeRunner) -> Option<DepFetchStep> {
    fetch_pnpm(
        &tree.worktree,
        &tree.frontend,
        Some(&tree.store),
        &tree.scratch_root,
        runner,
        &CancellationToken::new(),
    )
}

#[test]
fn a_missing_npm_package_is_fetched_from_the_registry_into_the_host_store() {
    let tree = pnpm_tree(PNPM_LOCK_OK, Some("allowBuilds:\n  sharp: true\n"));
    let runner = FakeRunner::new(false, true);
    let step = run_pnpm(&tree, &runner).unwrap();
    assert_eq!(step.outcome, FetchOutcome::Fetched);
    assert_eq!(step.ecosystem, Ecosystem::Pnpm);
    assert_eq!(step.lockfile, "frontend/pnpm-lock.yaml");
    assert_eq!(step.registries, vec![NPM_REGISTRY.to_string()]);

    let commands = runner.commands();
    assert_eq!(commands.len(), 2);
    let online = &commands[1];
    assert_eq!(online.program, "pnpm");
    for flag in ["--ignore-scripts", "--registry", NPM_REGISTRY] {
        assert!(online.args.contains(&flag.to_string()), "{flag}");
    }
    assert!(online
        .args
        .contains(&tree.store.to_string_lossy().into_owned()));
}

#[test]
fn pnpm_runs_in_a_scratch_directory_that_is_gone_afterwards() {
    let tree = pnpm_tree(PNPM_LOCK_OK, Some("allowBuilds:\n  sharp: true\n"));
    struct Inspecting(PathBuf);
    impl ProbeRunner for Inspecting {
        fn run(&self, probe: &ProbeCommand) -> ProbeOutcome {
            assert!(probe.cwd.starts_with(&self.0), "scratch is under its root");
            assert!(probe.cwd.join("pnpm-lock.yaml").is_file());
            assert!(probe.cwd.join("pnpm-workspace.yaml").is_file());
            assert_eq!(
                std::fs::read_to_string(probe.cwd.join("package.json")).unwrap(),
                STAGED_MANIFEST
            );
            assert!(
                !probe.cwd.join(".npmrc").exists(),
                "the worker's .npmrc is not staged"
            );
            ProbeOutcome {
                success: true,
                detail: String::new(),
            }
        }
    }
    let runner = Inspecting(tree.scratch_root.clone());
    let step = fetch_pnpm(
        &tree.worktree,
        &tree.frontend,
        Some(&tree.store),
        &tree.scratch_root,
        &runner,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(step.outcome, FetchOutcome::Cached);
    assert_eq!(
        std::fs::read_dir(&tree.scratch_root).unwrap().count(),
        0,
        "the scratch project is removed"
    );
}

#[test]
fn a_pnpm_lock_pointing_elsewhere_than_the_registry_is_refused() {
    for resolution in [
        "resolution: {tarball: https://evil.example/x.tgz}",
        "resolution: {type: git, repo: https://evil.example/x.git, commit: abc}",
        "resolution: {directory: ../x, type: directory}",
    ] {
        let lock = format!("lockfileVersion: '9.0'\n\npackages:\n\n  x@1.0.0:\n    {resolution}\n");
        let tree = pnpm_tree(&lock, None);
        let runner = FakeRunner::new(false, true);
        let step = run_pnpm(&tree, &runner).unwrap();
        assert!(
            matches!(step.outcome, FetchOutcome::Refused { .. }),
            "{resolution}"
        );
        assert!(runner.commands().is_empty());
    }
}

#[test]
fn a_workspace_file_that_redirects_the_fetch_is_refused() {
    for setting in [
        "registry: https://evil.example/",
        "'@scope:registry': https://evil.example/",
        "storeDir: /Users/someone/.ssh",
        "httpsProxy: http://evil.example:8080",
        "pnpmfile: ./evil.cjs",
        "cafile: ./ca.pem",
    ] {
        let tree = pnpm_tree(
            PNPM_LOCK_OK,
            Some(&format!("allowBuilds:\n  sharp: true\n{setting}\n")),
        );
        let runner = FakeRunner::new(false, true);
        let step = run_pnpm(&tree, &runner).unwrap();
        assert!(
            matches!(step.outcome, FetchOutcome::Refused { .. }),
            "{setting}"
        );
        assert!(runner.commands().is_empty());
    }
}

#[test]
fn a_commented_workspace_setting_is_not_a_setting() {
    let tree = pnpm_tree(
        PNPM_LOCK_OK,
        Some("# registry: nope\nallowBuilds:\n  sharp: true\n"),
    );
    let step = run_pnpm(&tree, &FakeRunner::new(true, false)).unwrap();
    assert_eq!(step.outcome, FetchOutcome::Cached);
}

#[test]
fn a_pnpm_lock_without_packages_or_a_host_without_a_store_is_handled() {
    let empty = pnpm_tree("lockfileVersion: '9.0'\n\nimporters:\n  .: {}\n", None);
    let runner = FakeRunner::new(false, true);
    assert!(run_pnpm(&empty, &runner).is_none());

    let tree = pnpm_tree(PNPM_LOCK_OK, None);
    let step = fetch_pnpm(
        &tree.worktree,
        &tree.frontend,
        None,
        &tree.scratch_root,
        &runner,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(matches!(step.outcome, FetchOutcome::Refused { .. }));
    assert!(runner.commands().is_empty());
}

// ─── journal record ───────────────────────────────────────────────────────

#[test]
fn the_record_round_trips_and_says_whether_anything_was_fetched() {
    let record = DepFetchRecord {
        steps: vec![DepFetchStep {
            ecosystem: Ecosystem::Cargo,
            lockfile: "backend/Cargo.lock".into(),
            registries: vec![CRATES_IO.into()],
            outcome: FetchOutcome::Fetched,
            duration_ms: 12,
        }],
    };
    assert!(record.fetched_anything());
    assert!(!record.cancelled());
    let json = serde_json::to_value(&record).unwrap();
    assert_eq!(json["steps"][0]["outcome"]["status"], "fetched");
    assert_eq!(json["steps"][0]["ecosystem"], "cargo");
    let back: DepFetchRecord = serde_json::from_value(json).unwrap();
    assert_eq!(back, record);
}

#[test]
fn a_worktree_without_lock_files_produces_an_empty_record() {
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().join("wt");
    std::fs::create_dir_all(&worktree).unwrap();
    let runner = FakeRunner::new(false, true);
    let record = fetch_missing_dependencies(&worktree, &runner, &CancellationToken::new());
    assert!(record.steps.is_empty());
    assert!(runner.commands().is_empty());
}
