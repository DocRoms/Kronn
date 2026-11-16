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
