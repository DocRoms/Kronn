//! KT-1751 — the executables a project's validations may run beyond Kronn's own
//! allowlist: a project's own setting, decided by the human in the project's
//! workspace settings, never inherited from the global entry.

use super::*;

fn never_detected() -> Detected {
    Detected::default()
}

#[test]
fn without_a_project_entry_no_executable_is_added() {
    let effective = resolve(&LaneWarmConfig::default(), None, never_detected);
    assert!(effective.validation_executables.is_empty());
}

#[test]
fn a_project_lists_its_own_executables_and_the_global_entry_cannot() {
    let global = LaneWarmConfig {
        validation_executables: Some(vec!["composer".into()]),
        ..LaneWarmConfig::default()
    };
    let project = LaneWarmConfig {
        validation_executables: Some(vec!["go".into(), "vendor/bin/phpunit".into()]),
        ..LaneWarmConfig::default()
    };
    let own = resolve(&global, Some(&project), never_detected);
    assert_eq!(
        own.validation_executables,
        vec!["go".to_string(), "vendor/bin/phpunit".to_string()]
    );
    // What one owner trusts for every project would widen a project that never
    // asked for it.
    let inherited = resolve(&global, None, never_detected);
    assert!(inherited.validation_executables.is_empty());
}

#[test]
fn the_stored_executables_are_trimmed_unprefixed_and_deduplicated() {
    let stored = LaneWarmConfig {
        validation_executables: Some(vec![
            " composer ".into(),
            "./vendor/bin/phpunit".into(),
            "vendor/bin/phpunit".into(),
            "  ".into(),
            "composer".into(),
        ]),
        ..LaneWarmConfig::default()
    }
    .normalized();
    assert_eq!(
        stored.validation_executables,
        Some(vec![
            "composer".to_string(),
            "vendor/bin/phpunit".to_string()
        ])
    );
}

#[test]
fn an_executable_is_a_bare_name_or_a_path_inside_the_repository_and_never_a_shell() {
    for bad in [
        "bash",
        "sh",
        "env",
        "/usr/bin/php",
        "../tool",
        "vendor/../../tool",
        "-rf",
        "a b",
        "tool;rm",
        "$(x)",
    ] {
        let config = LaneWarmConfig {
            validation_executables: Some(vec![bad.into()]),
            ..LaneWarmConfig::default()
        };
        assert!(config.validate().is_err(), "`{bad}` must be refused");
    }
    let many = LaneWarmConfig {
        validation_executables: Some((0..33).map(|n| format!("tool{n}")).collect()),
        ..LaneWarmConfig::default()
    };
    assert!(many.validate().is_err());
    let ok = LaneWarmConfig {
        validation_executables: Some(vec![
            "composer".into(),
            "pytest".into(),
            "python3.12".into(),
            "vendor/bin/phpunit".into(),
        ]),
        ..LaneWarmConfig::default()
    };
    assert!(ok.validate().is_ok());
}

#[test]
fn the_executables_of_a_worktree_are_those_of_its_project() {
    let project = tempfile::tempdir().unwrap();
    let worktree = project
        .path()
        .join(".kronn/worktrees/task-kt-1751-aaaa1111");
    std::fs::create_dir_all(&worktree).unwrap();
    let other = tempfile::tempdir().unwrap();

    set_project(
        project.path(),
        Some(&LaneWarmConfig {
            validation_executables: Some(vec!["composer".into(), "vendor/bin/phpunit".into()]),
            ..LaneWarmConfig::default()
        }),
    );
    assert_eq!(
        validation_executables_for(&worktree),
        vec!["composer".to_string(), "vendor/bin/phpunit".to_string()]
    );
    // A tree that belongs to no registered project gets nothing.
    assert!(validation_executables_for(other.path()).is_empty());
    // The union (what a launch that names no project is checked against).
    assert!(all_validation_executables().contains(&"composer".to_string()));

    set_project(project.path(), None);
    assert!(validation_executables_for(&worktree).is_empty());
}

fn git_in(dir: &Path, args: &[&str]) {
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
}

/// KT-1751 review — a worktree under a configured worktree root (or an
/// autoCode root), outside `<repo>/.kronn/worktrees`, still gets its project's
/// executables: the project is found from the registered checkouts' own
/// worktree lists, whatever the layout.
#[test]
fn a_worktree_outside_the_kronn_layout_gets_its_projects_executables() {
    let project = tempfile::tempdir().unwrap();
    let project = project.path().canonicalize().unwrap();
    git_in(&project, &["init", "-q", "-b", "main"]);
    git_in(&project, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let roots = tempfile::tempdir().unwrap();
    let worktree = roots.path().canonicalize().unwrap().join("lane-a");
    let lane = worktree.to_str().unwrap();
    git_in(&project, &["worktree", "add", "-q", "-b", "kt-1751", lane]);
    std::fs::create_dir_all(worktree.join("sub")).unwrap();

    set_project(
        &project,
        Some(&LaneWarmConfig {
            validation_executables: Some(vec!["phpunit-outside".into()]),
            ..LaneWarmConfig::default()
        }),
    );
    let found = validation_executables_for(&worktree);
    let from_sub = validation_executables_for(&worktree.join("sub"));
    set_project(&project, None);

    assert_eq!(found, vec!["phpunit-outside".to_string()]);
    assert_eq!(from_sub, vec!["phpunit-outside".to_string()]);
}

/// KT-1751 review — a project registered inside another one (`mono/sub` in
/// `mono`) is its own: without a list it gets nothing from the outer project,
/// and with its own list it gets that list, whatever the map order.
#[test]
fn a_nested_project_never_inherits_the_outer_projects_executables() {
    let root = tempfile::tempdir().unwrap();
    let mono = root.path().canonicalize().unwrap().join("mono");
    let sub = mono.join("sub");
    std::fs::create_dir_all(sub.join("src")).unwrap();
    git_in(&sub, &["init", "-q", "-b", "main"]);

    set_project(
        &mono,
        Some(&LaneWarmConfig {
            validation_executables: Some(vec!["outer-tool".into()]),
            ..LaneWarmConfig::default()
        }),
    );
    let without_list = validation_executables_for(&sub.join("src"));
    let outer_itself = validation_executables_for(&mono);
    set_project(
        &sub,
        Some(&LaneWarmConfig {
            validation_executables: Some(vec!["inner-tool".into()]),
            ..LaneWarmConfig::default()
        }),
    );
    let with_list = validation_executables_for(&sub.join("src"));
    set_project(&sub, None);
    set_project(&mono, None);

    assert!(without_list.is_empty(), "{without_list:?}");
    assert_eq!(outer_itself, vec!["outer-tool".to_string()]);
    assert_eq!(with_list, vec!["inner-tool".to_string()]);
}

/// KT-1751 third review — the owner of a path is found among every registered
/// project, not only those with a list: a project registered as a plain
/// sub-folder of another (`mono/sub`, no repository of its own) owns its tree,
/// with no lane entry or with an entry that lists no executable, and gets
/// nothing; the outer project keeps its own list.
#[test]
fn a_registered_sub_folder_project_without_a_list_gets_nothing_from_the_outer_one() {
    let root = tempfile::tempdir().unwrap();
    let mono = root.path().canonicalize().unwrap().join("mono");
    let sub = mono.join("sub");
    std::fs::create_dir_all(sub.join("src")).unwrap();
    std::fs::create_dir_all(mono.join("other")).unwrap();
    git_in(&mono, &["init", "-q", "-b", "main"]);
    git_in(&mono, &["commit", "-q", "--allow-empty", "-m", "init"]);

    set_project(
        &mono,
        Some(&LaneWarmConfig {
            validation_executables: Some(vec!["outer-only".into()]),
            ..LaneWarmConfig::default()
        }),
    );
    register_project(&sub);
    let unlisted = validation_executables_for(&sub.join("src"));
    set_project(
        &sub,
        Some(&LaneWarmConfig {
            dependency_dirs: Some(vec!["vendor".into()]),
            ..LaneWarmConfig::default()
        }),
    );
    let configured_without_executables = validation_executables_for(&sub.join("src"));
    let outer = validation_executables_for(&mono.join("other"));
    forget_project(&sub);
    let after_forget = validation_executables_for(&sub.join("src"));
    forget_project(&mono);

    assert!(unlisted.is_empty(), "{unlisted:?}");
    assert!(
        configured_without_executables.is_empty(),
        "{configured_without_executables:?}"
    );
    assert_eq!(outer, vec!["outer-only".to_string()]);
    // Once the sub-folder project is gone, its tree is the outer project's again.
    assert_eq!(after_forget, vec!["outer-only".to_string()]);
}
