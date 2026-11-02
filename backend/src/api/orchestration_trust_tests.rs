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
    let launch = launch_for_reach(by_level.launch(), &std::env::temp_dir()).unwrap();
    assert!(
        !matches!(launch, ValidationLaunch::Unconfined),
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
fn a_project_s_own_full_access_is_a_humans_choice_and_stays_as_it_was() {
    let by_human = reach(ValidationsAccess::Full, None, None);
    assert_eq!(by_human.launch(), ValidationReachLaunch::Host);
    assert!(matches!(
        launch_for_reach(by_human.launch(), &std::env::temp_dir()),
        Ok(ValidationLaunch::Unconfined)
    ));
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
    crate::core::trust_sandbox::set_test_unavailable(Some("no sandbox here"));
    let launch = launch_for_reach(ValidationReachLaunch::TrustHost, &std::env::temp_dir());
    crate::core::trust_sandbox::set_test_unavailable(None);
    assert!(matches!(launch, Ok(ValidationLaunch::Unconfined)));

    crate::core::trust_sandbox::set_protect_vault(false);
    let launch = launch_for_reach(ValidationReachLaunch::TrustHost, &std::env::temp_dir());
    crate::core::trust_sandbox::set_protect_vault(true);
    assert!(matches!(launch, Ok(ValidationLaunch::Unconfined)));
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
