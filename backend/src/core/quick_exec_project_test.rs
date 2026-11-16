//! KT-1751 — the executables a project's own validations may run.
//!
//! The built-in allowlist names Kronn's own toolchains (`cargo`, `pnpm`, …). A
//! PHP, Go or Python project needs `composer`, `go`, `pytest`, or the repository's
//! own `vendor/bin/phpunit`: the human adds them to the project's own list, and
//! only that list widens the allowlist. The denylist (no shell) still wins, and a
//! repository-relative executable must stay inside the worktree it runs in.

use super::*;

fn extra(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn spec_in(binary: &str, argv: &[&str], cwd: &Path) -> QuickExecSpec {
    QuickExecSpec {
        binary: binary.to_string(),
        argv: argv.iter().map(|arg| arg.to_string()).collect(),
        cwd: cwd.to_path_buf(),
        timeout_secs: Some(10),
        stdin: None,
        summariser: Summariser::Generic,
    }
}

#[cfg(unix)]
fn write_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_name_the_human_added_to_the_project_is_accepted_and_no_other_project_gets_it() {
    assert!(
        check_command_line("composer install").is_err(),
        "composer is not a built-in"
    );
    check_command_line_with("composer install", &extra(&["composer"])).unwrap();
    // The list widens by its entries only.
    let rejection = check_command_line_with("go test ./...", &extra(&["composer"]))
        .expect_err("go was never added");
    assert!(rejection.0.contains("`go`"), "{}", rejection.0);
    // A built-in stays available whatever the project added.
    check_command_line_with("cargo test --lib", &extra(&["composer"])).unwrap();
}

#[test]
fn the_extension_never_admits_a_shell_or_a_path_outside_the_repository() {
    for denied in DENIED_BINARIES {
        let rejection = check_command_line_with(&format!("{denied} -c true"), &extra(&[denied]))
            .expect_err(&format!("`{denied}` was accepted from a project's list"));
        assert!(rejection.0.contains("never a shell"), "{}", rejection.0);
    }
    // An absolute path or a parent traversal is never an executable of the repository.
    for path in ["/usr/bin/php", "../outside/bin/tool", "vendor/../../tool"] {
        assert!(
            check_command_line_with(path, &extra(&[path])).is_err(),
            "{path} was accepted"
        );
    }
}

#[test]
fn a_repo_relative_executable_runs_only_when_the_project_lists_it() {
    let root = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    write_executable(
        &root.path().join("vendor/bin/phpunit"),
        "#!/bin/sh\necho phpunit\n",
    );
    let roots = vec![root.path().to_path_buf()];
    let spec = spec_in("vendor/bin/phpunit", &["--version"], root.path());

    // Not listed: refused, with the reason that names the setting.
    let rejection = validate_with(&spec, &roots, &[]).expect_err("an unlisted path was accepted");
    assert!(
        rejection.0.contains("vendor/bin/phpunit"),
        "{}",
        rejection.0
    );

    // Listed: accepted, and so is the same path spelled with a leading `./`.
    check_command_line_with(
        "vendor/bin/phpunit --version",
        &extra(&["vendor/bin/phpunit"]),
    )
    .unwrap();
    check_command_line_with(
        "./vendor/bin/phpunit --version",
        &extra(&["vendor/bin/phpunit"]),
    )
    .unwrap();
    // Another path of the repository is not covered by it.
    assert!(check_command_line_with("vendor/bin/other", &extra(&["vendor/bin/phpunit"])).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn a_listed_repo_relative_executable_is_run_from_the_worktree() {
    let root = tempfile::tempdir().unwrap();
    write_executable(
        &root.path().join("vendor/bin/phpunit"),
        "#!/bin/sh\necho ran-from-the-worktree\n",
    );
    let roots = vec![root.path().to_path_buf()];
    let validated = validate_with(
        &spec_in("vendor/bin/phpunit", &[], root.path()),
        &roots,
        &extra(&["vendor/bin/phpunit"]),
    )
    .unwrap();
    let artifacts = root.path().join("artifacts");
    let result = run(&validated, Some(&artifacts), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, QuickExecStatus::Passed, "{}", result.summary);
    let log = std::fs::read_to_string(&result.artifact.unwrap().path).unwrap();
    assert!(log.contains("ran-from-the-worktree"), "{log}");
}

#[cfg(unix)]
#[test]
fn a_listed_executable_that_resolves_outside_the_worktree_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write_executable(&outside.path().join("tool"), "#!/bin/sh\necho escaped\n");
    std::fs::create_dir_all(root.path().join("bin")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("tool"), root.path().join("bin/tool")).unwrap();
    let roots = vec![root.path().to_path_buf()];

    let rejection = validate_with(
        &spec_in("bin/tool", &[], root.path()),
        &roots,
        &extra(&["bin/tool"]),
    )
    .expect_err("a link out of the repository was accepted");
    assert!(rejection.0.contains("outside"), "{}", rejection.0);

    // A listed path that does not exist is refused at run time, not spawned.
    let missing = validate_with(
        &spec_in("bin/missing", &[], root.path()),
        &roots,
        &extra(&["bin/missing"]),
    )
    .expect_err("a missing executable was accepted");
    assert!(missing.0.contains("bin/missing"), "{}", missing.0);
}

/// KT-1751 review — in a lane, a dependency directory is layered from the main
/// checkout (its entries link there), so a listed executable inside it
/// (`vendor/phpunit/phpunit/phpunit`, `node_modules/.bin/jest`) resolves into
/// the main checkout's matching dependency directory: admitted, and nothing
/// wider (a layer entry a worker re-pointed elsewhere is still refused).
#[cfg(unix)]
#[test]
fn a_listed_executable_of_a_layered_dependency_dir_is_admitted_and_nothing_wider() {
    use crate::core::task_worker_env::{SharedCaches, WorkerLane};
    let main = tempfile::tempdir().unwrap();
    let main = main.path().canonicalize().unwrap();
    let worktree = main.join(".kronn/worktrees/task-kt-1751-bbbb2222");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(worktree.join("composer.json"), "{}").unwrap();
    std::fs::write(worktree.join("package.json"), "{}").unwrap();
    std::fs::write(worktree.join("package-lock.json"), "{}").unwrap();
    write_executable(
        &main.join("vendor/phpunit/phpunit/phpunit"),
        "#!/bin/sh\necho phpunit\n",
    );
    write_executable(
        &main.join("node_modules/jest/bin/jest.js"),
        "#!/bin/sh\necho jest\n",
    );
    std::fs::create_dir_all(main.join("node_modules/.bin")).unwrap();
    std::os::unix::fs::symlink("../jest/bin/jest.js", main.join("node_modules/.bin/jest"))
        .unwrap();
    WorkerLane::for_worktree(&worktree)
        .unwrap()
        .provision(&SharedCaches::default());
    let roots = vec![worktree.clone()];
    let listed = extra(&[
        "vendor/phpunit/phpunit/phpunit",
        "node_modules/.bin/jest",
        "vendor/evil/tool",
    ]);

    let phpunit = validate_with(
        &spec_in("vendor/phpunit/phpunit/phpunit", &[], &worktree),
        &roots,
        &listed,
    )
    .unwrap();
    assert_eq!(
        phpunit.local_executable,
        Some(main.join("vendor/phpunit/phpunit/phpunit"))
    );
    let jest = validate_with(
        &spec_in("node_modules/.bin/jest", &[], &worktree),
        &roots,
        &listed,
    )
    .unwrap();
    assert_eq!(
        jest.local_executable,
        Some(main.join("node_modules/jest/bin/jest.js"))
    );

    // A layer entry re-pointed out of the main checkout's dependency dir.
    let outside = tempfile::tempdir().unwrap();
    write_executable(&outside.path().join("tool"), "#!/bin/sh\necho escaped\n");
    std::os::unix::fs::symlink(outside.path(), worktree.join("vendor/evil")).unwrap();
    let rejection = validate_with(
        &spec_in("vendor/evil/tool", &[], &worktree),
        &roots,
        &listed,
    )
    .expect_err("a re-pointed layer entry was accepted");
    assert!(rejection.0.contains("outside"), "{}", rejection.0);
}

/// KT-1751 review (blue) — a bare name the project lists (`composer`) is a
/// program on the PATH, never a file of the repository called `./composer`.
#[test]
fn a_listed_bare_name_does_not_admit_a_repository_file_of_that_name() {
    assert!(check_command_line_with("./composer install", &extra(&["composer"])).is_err());
    assert!(check_command_line_with("composer install", &extra(&["./composer"])).is_err());
    check_command_line_with("./composer install", &extra(&["./composer"])).unwrap();
    check_command_line_with("composer install", &extra(&["composer"])).unwrap();
}
