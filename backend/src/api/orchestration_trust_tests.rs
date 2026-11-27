//! KT-1492 — what a trust level reaches for the integration validations runs
//! under the level's sandbox, away from Kronn's data. Autoreview 🟡 4: removing
//! the confinement of `ValidationsUnconfined` or `ValidationsDocker` must fail
//! a test here.

use super::*;
use crate::core::task_worker_sandbox::Mechanism;
use crate::core::trust_sandbox::{protected_dirs, seatbelt_profile};
use crate::models::{TrustApproval, ValidationsAccess};

fn reach(
    access: ValidationsAccess,
    socket: Option<&str>,
    by_trust: Option<TrustApproval>,
) -> ValidationReach {
    ValidationReach {
        access,
        socket: socket.map(|path| Ok(std::path::PathBuf::from(path))),
        by_trust,
    }
}

/// bubblewrap hides a directory only when it exists, so a test about what a
/// launch closes needs the data dir (the per-process test one) to be there.
fn with_data_dir_present() {
    for dir in protected_dirs() {
        std::fs::create_dir_all(&dir).expect("create the test data dir");
    }
}

fn probe_of(program: &str) -> crate::core::task_worker_env::ProbeCommand {
    crate::core::task_worker_env::ProbeCommand {
        program: program.into(),
        args: vec![],
        cwd: std::env::temp_dir(),
        env: vec![],
        timeout: std::time::Duration::from_secs(5),
        priority: crate::core::background_priority::Priority::Normal,
        clean_env: false,
    }
}

/// The command a launch builds, as `(program, args)` of the process it spawns.
fn spawned(launch: &ValidationLaunch) -> (String, Vec<String>) {
    let confinement = launch
        .confinement()
        .expect("a reach a trust level opened is confined");
    let command = confinement.command(
        &probe_of("true"),
        crate::core::child_env::ChildRoute::QuickExec,
    );
    let std = command.as_std();
    (
        std.get_program().to_string_lossy().into_owned(),
        std.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
    )
}

#[test]
#[serial_test::serial]
fn full_access_by_a_level_launches_under_the_level_sandbox_never_unconfined() {
    with_data_dir_present();
    let by_level = reach(
        ValidationsAccess::Full,
        None,
        Some(TrustApproval::ValidationsUnconfined),
    );
    assert_eq!(by_level.launch(), ValidationReachLaunch::TrustHost);
    let tree = tempfile::tempdir().unwrap();
    let launch = launch_for_reach(by_level.launch(), tree.path()).unwrap();
    assert!(
        matches!(&launch, ValidationLaunch::InLane(lane) if lane.under_a_level()),
        "the unconfined validation a level opened ran with no sandbox"
    );
    let (program, args) = spawned(&launch);
    let launcher = Mechanism::of_host().unwrap().launcher();
    assert_eq!(program, launcher);
    let joined = args.join(" ");
    for dir in protected_dirs() {
        let shown = dir.to_string_lossy();
        assert!(
            joined.contains(shown.as_ref()),
            "the data dir {shown} is not closed to the validation: {joined}"
        );
    }
    if Mechanism::of_host() == Some(Mechanism::Seatbelt) {
        assert_eq!(args[1], seatbelt_profile(&protected_dirs()));
        assert!(
            !joined.contains("network"),
            "the network and the API port stay open: {joined}"
        );
    }
}

#[test]
#[serial_test::serial]
fn a_project_s_own_full_access_runs_without_a_sandbox_in_its_lane() {
    let by_human = reach(ValidationsAccess::Full, None, None);
    assert_eq!(by_human.launch(), ValidationReachLaunch::Host);
    let tree = tempfile::tempdir().unwrap();
    assert!(matches!(
        launch_for_reach(by_human.launch(), tree.path()),
        Ok(ValidationLaunch::InLane(lane)) if !lane.under_a_level()
    ));
}

/// The environment the process of a launch gets, as built for a parent that
/// exports its own target directory (a dev server started by a launcher).
fn env_under_a_server_target(
    launch: &ValidationLaunch,
) -> std::collections::BTreeMap<String, String> {
    use crate::core::child_env::{probe, ChildRoute};
    let confinement = launch.confinement().expect("a shipped launch");
    let command = crate::core::child_env::with_parent_env(
        &[("PATH", "/usr/bin"), ("CARGO_TARGET_DIR", "/server/target")],
        || confinement.command(&probe_of("true"), ChildRoute::QuickExec),
    );
    probe::env_of(command.as_std())
}

/// KT-2024 — the reach changes the sandbox, never where a validation builds: at
/// every reach the command gets the lane's own target and data directories, not
/// the target the backend process inherited. Only the sandbox is offline.
#[test]
#[serial_test::serial]
fn a_validation_builds_in_its_lane_at_every_reach() {
    with_data_dir_present();
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path().canonicalize().unwrap();
    let lane = root
        .join(".kronn")
        .join("worktrees")
        .join("task-kt-2024-reach");
    std::fs::create_dir_all(&lane).unwrap();
    let target = root.join("target").join("wt-kt-2024-reach");
    let mut reaches = vec![
        ("full access", ValidationReachLaunch::Host),
        ("a trust level", ValidationReachLaunch::TrustHost),
    ];
    if let Some(mechanism) = Mechanism::of_host() {
        test_validation_mechanisms()
            .lock()
            .unwrap()
            .insert(lane.clone(), Some(mechanism));
        reaches.push(("the lane sandbox", ValidationReachLaunch::Sandbox));
    }
    for (name, reach) in reaches {
        let sandboxed = reach == ValidationReachLaunch::Sandbox;
        let launch = launch_for_reach(reach, &lane).unwrap();
        let env = env_under_a_server_target(&launch);
        assert_eq!(
            env.get("CARGO_TARGET_DIR").map(std::path::PathBuf::from),
            Some(target.clone()),
            "{name}"
        );
        assert!(
            env.get("KRONN_DATA_DIR")
                .is_some_and(|dir| std::path::Path::new(dir).starts_with(&root)),
            "{name}: {:?}",
            env.get("KRONN_DATA_DIR")
        );
        // A launch left unconfined keeps its network.
        assert_eq!(env.contains_key("CARGO_NET_OFFLINE"), sandboxed, "{name}");
        assert_eq!(
            env.contains_key("KRONN_E2E_CHROMIUM_SINGLE_PROCESS"),
            sandboxed,
            "{name}"
        );
    }
    test_validation_mechanisms().lock().unwrap().remove(&lane);
}

/// KT-2024 — the target lookup that gates a validation asks Cargo in the same
/// environment, so the disk it measures is the lane's; a `--target-dir` of the
/// validation itself still wins.
#[test]
#[serial_test::serial]
fn the_target_lookup_of_an_unsandboxed_validation_reads_the_lane() {
    use crate::core::child_env::probe;
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path().canonicalize().unwrap();
    let lane = root
        .join(".kronn")
        .join("worktrees")
        .join("task-kt-2024-lookup");
    std::fs::create_dir_all(&lane).unwrap();
    let launch = launch_for_reach(ValidationReachLaunch::Host, &lane).unwrap();
    let target_of = |words: &[&str]| {
        let command = crate::core::child_env::with_parent_env(
            &[("PATH", "/usr/bin"), ("CARGO_TARGET_DIR", "/server/target")],
            || validation_metadata_command(words, &lane, &launch).unwrap(),
        );
        probe::env_of(command.as_std())
            .get("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
    };
    assert_eq!(
        target_of(&["test", "--lib"]),
        Some(root.join("target").join("wt-kt-2024-lookup"))
    );
    assert_eq!(
        target_of(&["test", "--target-dir", "/elsewhere"]),
        Some(std::path::PathBuf::from("/elsewhere"))
    );
}

#[test]
#[serial_test::serial]
fn docker_by_a_level_keeps_the_lane_sandbox_and_closes_the_data_dir() {
    with_data_dir_present();
    let by_level = reach(
        ValidationsAccess::Docker,
        Some("/var/run/docker.sock"),
        Some(TrustApproval::ValidationsDocker),
    );
    assert_eq!(
        by_level.launch(),
        ValidationReachLaunch::TrustDocker("/var/run/docker.sock".into())
    );
    // A human-opened Docker is the plain one.
    let by_human = reach(
        ValidationsAccess::Docker,
        Some("/var/run/docker.sock"),
        None,
    );
    assert_eq!(
        by_human.launch(),
        ValidationReachLaunch::Docker("/var/run/docker.sock".into())
    );

    let tree = tempfile::tempdir().unwrap();
    test_validation_mechanisms()
        .lock()
        .unwrap()
        .insert(tree.path().to_path_buf(), Some(Mechanism::Seatbelt));
    let launch = launch_for_reach(by_level.launch(), tree.path()).unwrap();
    let ValidationLaunch::Confined(confinement) = &launch else {
        panic!("the Docker validation a level opened lost the lane sandbox");
    };
    assert_eq!(
        confinement.policy().docker_socket.as_deref(),
        Some(std::path::Path::new("/var/run/docker.sock"))
    );
    assert_eq!(confinement.policy().hidden, protected_dirs());
    let (_, args) = spawned(&launch);
    for dir in protected_dirs() {
        let shown = dir.to_string_lossy();
        assert!(
            args[1].contains(&format!("(subpath \"{shown}\")")),
            "Seatbelt profile does not deny the data dir {shown}"
        );
    }
    // The plain human Docker keeps no such rule.
    let plain = launch_for_reach(by_human.launch(), tree.path()).unwrap();
    assert!(spawned(&plain).1[1]
        .find("(deny file-read* file-write*")
        .is_none());
}

#[test]
#[serial_test::serial]
fn docker_by_a_level_is_not_refused_when_the_lane_has_no_sandbox_to_hide_the_data_dir_in() {
    let tree = tempfile::tempdir().unwrap();
    // A directory no test registered has no lane sandbox: the vault protection is
    // unavailable there, and the reach a level opened is not refused for it.
    let launch = launch_for_reach(
        ValidationReachLaunch::TrustDocker("/var/run/docker.sock".into()),
        tree.path(),
    );
    assert!(matches!(launch, Ok(ValidationLaunch::Unconfined)));
}

#[tokio::test]
#[serial_test::serial]
async fn a_docker_volume_naming_the_data_dir_is_refused_before_it_runs() {
    let tree = tempfile::tempdir().unwrap();
    test_validation_mechanisms()
        .lock()
        .unwrap()
        .insert(tree.path().to_path_buf(), Some(Mechanism::Seatbelt));
    let data = protected_dirs()
        .into_iter()
        .next()
        .expect("the data dir is protected");
    for command in [
        format!("docker run --rm -v {}:/data alpine ls", data.display()),
        "docker run --rm -v /:/host alpine ls".to_string(),
    ] {
        let spec = crate::models::ValidationSpec {
            command,
            quick_exec_id: None,
            timeout_secs: None,
        };
        let (code, _, output) = run_one_validation_reaching(
            &spec,
            tree.path(),
            ValidationReachLaunch::TrustDocker("/var/run/docker.sock".into()),
        )
        .await;
        assert_eq!(code, None);
        assert!(
            output.starts_with("refused:") && output.contains("data directory"),
            "{output}"
        );
    }
}

/// A host with no sandbox, or the owner's choice to turn the protection off,
/// does not refuse the reach a level opened: the validation runs as the user.
#[test]
#[serial_test::serial]
fn a_host_with_no_sandbox_does_not_refuse_the_reach_a_level_opened() {
    let tree = tempfile::tempdir().unwrap();
    let as_the_user =
        |launch| matches!(launch, Ok(ValidationLaunch::InLane(lane)) if !lane.under_a_level());
    crate::core::trust_sandbox::set_test_unavailable(Some("no sandbox here"));
    let launch = launch_for_reach(ValidationReachLaunch::TrustHost, tree.path());
    crate::core::trust_sandbox::set_test_unavailable(None);
    assert!(as_the_user(launch));

    crate::core::trust_sandbox::set_protect_vault(false);
    let launch = launch_for_reach(ValidationReachLaunch::TrustHost, tree.path());
    crate::core::trust_sandbox::set_protect_vault(true);
    assert!(as_the_user(launch));
}

#[tokio::test]
#[serial_test::serial]
async fn with_the_vault_protection_off_a_docker_volume_naming_the_data_dir_is_not_refused() {
    let tree = tempfile::tempdir().unwrap();
    test_validation_mechanisms()
        .lock()
        .unwrap()
        .insert(tree.path().to_path_buf(), Some(Mechanism::Seatbelt));
    let data = protected_dirs().into_iter().next().unwrap();
    crate::core::trust_sandbox::set_protect_vault(false);
    let spec = crate::models::ValidationSpec {
        command: format!("true -v {}:/data", data.display()),
        quick_exec_id: None,
        timeout_secs: None,
    };
    let (_, _, output) = run_one_validation_reaching(
        &spec,
        tree.path(),
        ValidationReachLaunch::TrustDocker("/var/run/docker.sock".into()),
    )
    .await;
    crate::core::trust_sandbox::set_protect_vault(true);
    assert!(
        !output.contains("would mount Kronn's data directory"),
        "{output}"
    );
}
