use super::*;
use crate::models::{
    parse_lane_env_entry, ProjectWorkspace, WorkersConfig, MAX_LANE_FIRST_BUILD_TIMEOUT_MIN,
};

fn never_detected() -> Detected {
    panic!("a layer set the compiler environment: nothing is detected");
}

fn macos_cc() -> Detected {
    Detected::for_host(true, true)
}

const MACOS_FLAGS: [(&str, &str); 2] = [
    ("CFLAGS_aarch64_apple_darwin", "-save-temps=obj"),
    ("CFLAGS_x86_64_apple_darwin", "-save-temps=obj"),
];

fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

// ── Defaults (nothing configured) ────────────────────────────────────────

#[test]
fn nothing_configured_gives_the_detected_defaults() {
    let effective = resolve(&LaneWarmConfig::default(), None, macos_cc);

    assert!(effective.seed_alignment, "alignment is on by default");
    assert_eq!(effective.compiler_env, pairs(&MACOS_FLAGS));
    assert_eq!(effective.first_build_command, None);
    assert_eq!(effective.first_build_timeout, Duration::from_secs(60 * 60));
}

#[test]
fn the_macos_clang_default_applies_only_to_a_project_that_builds_c_through_cc() {
    // A Rust project whose lock lists `cc`, on macOS.
    assert_eq!(
        Detected::for_host(true, true).compiler_env,
        pairs(&MACOS_FLAGS)
    );
    // A Rust project without `cc`, a non-Rust project (no lock to list it), and
    // every other host: nothing, so no build script's fingerprint changes for it.
    assert!(Detected::for_host(true, false).compiler_env.is_empty());
    assert!(Detected::for_host(false, true).compiler_env.is_empty());
    assert!(Detected::for_host(false, false).compiler_env.is_empty());
}

#[test]
fn detection_reads_the_projects_own_lock_files() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    std::fs::create_dir_all(repo.join("redis-flush")).unwrap();
    std::fs::write(repo.join("redis-flush").join("Cargo.toml"), "").unwrap();
    std::fs::write(
        repo.join("redis-flush").join("Cargo.lock"),
        "[[package]]\nname = \"cc\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    // No snapshot entry for this project: detection alone decides.
    let effective = effective_for(repo, None);
    if cfg!(target_os = "macos") {
        assert_eq!(effective.compiler_env, pairs(&MACOS_FLAGS));
    } else {
        assert!(effective.compiler_env.is_empty());
    }

    let plain = tempfile::tempdir().unwrap();
    std::fs::write(plain.path().join("composer.json"), "{}").unwrap();
    assert!(
        effective_for(plain.path(), None).compiler_env.is_empty(),
        "a project that is not a Rust one gets no C flags"
    );
}

// ── The global entry ─────────────────────────────────────────────────────

#[test]
fn the_global_entry_overrides_every_detected_default() {
    let global = LaneWarmConfig {
        seed_alignment: Some(false),
        compiler_env: Some(vec!["CC=clang".into(), "CFLAGS=-O1".into()]),
        first_build_command: Some("  make deps  ".into()),
        first_build_timeout_min: Some(90),
        rust_roots: None,
    };

    let effective = resolve(&global, None, never_detected);

    assert!(!effective.seed_alignment);
    assert_eq!(
        effective.compiler_env,
        pairs(&[("CC", "clang"), ("CFLAGS", "-O1")])
    );
    assert_eq!(effective.first_build_command.as_deref(), Some("make deps"));
    assert_eq!(effective.first_build_timeout, Duration::from_secs(90 * 60));
}

#[test]
fn an_empty_global_environment_clears_the_detected_one() {
    let global = LaneWarmConfig {
        compiler_env: Some(Vec::new()),
        ..LaneWarmConfig::default()
    };
    let effective = resolve(&global, None, never_detected);
    assert!(effective.compiler_env.is_empty());
}

// ── The project's own entry ──────────────────────────────────────────────

#[test]
fn a_project_field_wins_over_the_global_one_and_the_rest_is_inherited() {
    let global = LaneWarmConfig {
        seed_alignment: Some(true),
        compiler_env: Some(vec!["CC=clang".into()]),
        first_build_command: Some("make deps".into()),
        first_build_timeout_min: Some(90),
        rust_roots: None,
    };
    let project = LaneWarmConfig {
        first_build_timeout_min: Some(20),
        seed_alignment: Some(false),
        ..LaneWarmConfig::default()
    };

    let effective = resolve(&global, Some(&project), never_detected);

    assert_eq!(effective.first_build_timeout, Duration::from_secs(20 * 60));
    assert!(!effective.seed_alignment, "the project turned it off");
    assert_eq!(
        effective.compiler_env,
        pairs(&[("CC", "clang")]),
        "inherited"
    );
    assert_eq!(effective.first_build_command.as_deref(), Some("make deps"));
}

#[test]
fn a_project_can_clear_the_global_compiler_environment_or_replace_it() {
    let global = LaneWarmConfig {
        compiler_env: Some(vec!["CC=clang".into()]),
        ..LaneWarmConfig::default()
    };
    let cleared = LaneWarmConfig {
        compiler_env: Some(Vec::new()),
        ..LaneWarmConfig::default()
    };
    assert!(resolve(&global, Some(&cleared), never_detected)
        .compiler_env
        .is_empty());

    let replaced = LaneWarmConfig {
        compiler_env: Some(vec!["CFLAGS_x_y_z=-g".into()]),
        ..LaneWarmConfig::default()
    };
    assert_eq!(
        resolve(&global, Some(&replaced), never_detected).compiler_env,
        pairs(&[("CFLAGS_x_y_z", "-g")])
    );
}

#[test]
fn a_project_with_nothing_set_follows_detection() {
    let effective = resolve(
        &LaneWarmConfig::default(),
        Some(&LaneWarmConfig::default()),
        macos_cc,
    );
    assert_eq!(effective.compiler_env, pairs(&MACOS_FLAGS));
}

/// The snapshot a lane reads: the project's entry is looked up by the path of its
/// main checkout, so every lane of that project (and no other) gets it.
#[test]
fn a_projects_entry_reaches_the_lanes_of_that_project_only() {
    let project = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let worktree = project
        .path()
        .join(".kronn")
        .join("worktrees")
        .join("task-x");
    std::fs::create_dir_all(&worktree).unwrap();
    set_project(
        project.path(),
        Some(&LaneWarmConfig {
            compiler_env: Some(vec!["CC=gcc".into()]),
            first_build_timeout_min: Some(5),
            ..LaneWarmConfig::default()
        }),
    );

    let mine = effective_for(&worktree, Some(project.path()));
    assert_eq!(mine.compiler_env, pairs(&[("CC", "gcc")]));
    assert_eq!(mine.first_build_timeout, Duration::from_secs(300));
    assert_eq!(
        compiler_env_for(&worktree, Some(project.path())),
        vec![("CC", OsString::from("gcc"))]
    );

    let theirs = effective_for(other.path(), Some(other.path()));
    assert_eq!(theirs.first_build_timeout, Duration::from_secs(60 * 60));
    assert!(theirs.compiler_env.is_empty());

    // Cleared (or emptied) in the project's settings: back to the defaults.
    set_project(project.path(), Some(&LaneWarmConfig::default()));
    let reset = effective_for(&worktree, Some(project.path()));
    assert_eq!(reset.first_build_timeout, Duration::from_secs(60 * 60));
}

// ── The Rust roots to warm ───────────────────────────────────────────────

#[test]
fn the_rust_roots_default_to_every_detected_root_and_are_a_projects_own_setting() {
    // Default: none named, so every root found from the manifests.
    let none = resolve(&LaneWarmConfig::default(), None, Detected::default);
    assert_eq!(none.rust_roots, None);

    // A project names the roots to warm.
    let project = LaneWarmConfig {
        rust_roots: Some(vec!["backend".into(), "tools/exporter".into()]),
        ..LaneWarmConfig::default()
    };
    let effective = resolve(
        &LaneWarmConfig::default(),
        Some(&project),
        Detected::default,
    );
    assert_eq!(
        effective.rust_roots,
        Some(vec!["backend".to_string(), "tools/exporter".to_string()])
    );

    // The global entry cannot name directories of repositories it does not know.
    let global = LaneWarmConfig {
        rust_roots: Some(vec!["backend".into()]),
        ..LaneWarmConfig::default()
    };
    assert_eq!(resolve(&global, None, Detected::default).rust_roots, None);
}

#[test]
fn the_rust_roots_are_normalized_and_validated_before_they_are_stored() {
    let config = LaneWarmConfig {
        rust_roots: Some(vec![
            " ./redis-flush/ ".into(),
            "redis-flush".into(),
            "".into(),
            ".".into(),
        ]),
        ..LaneWarmConfig::default()
    };
    assert_eq!(config.validate(), Ok(()));
    assert_eq!(
        config.normalized().rust_roots,
        Some(vec!["redis-flush".to_string(), ".".to_string()]),
        "trimmed, de-duplicated, blank entries dropped"
    );
    let blank = LaneWarmConfig {
        rust_roots: Some(vec!["  ".into()]),
        ..LaneWarmConfig::default()
    };
    assert_eq!(
        blank.normalized().rust_roots,
        None,
        "a blank list is every detected root, not none"
    );
    for bad in ["../outside", "/etc", "a/../../b"] {
        let config = LaneWarmConfig {
            rust_roots: Some(vec![bad.into()]),
            ..LaneWarmConfig::default()
        };
        assert!(config.validate().is_err(), "{bad} must be refused");
    }
    let many = LaneWarmConfig {
        rust_roots: Some((0..17).map(|n| format!("crate-{n}")).collect()),
        ..LaneWarmConfig::default()
    };
    assert!(many.validate().is_err());
}

// ── What the forms may store ─────────────────────────────────────────────

#[test]
fn the_settings_are_validated_before_they_are_stored() {
    let ok = LaneWarmConfig {
        compiler_env: Some(vec!["CFLAGS_aarch64_apple_darwin=-save-temps=obj".into()]),
        first_build_command: Some("composer install".into()),
        first_build_timeout_min: Some(45),
        rust_roots: None,
        seed_alignment: Some(true),
    };
    assert_eq!(ok.validate(), Ok(()));

    let timeout = |minutes| LaneWarmConfig {
        first_build_timeout_min: Some(minutes),
        ..LaneWarmConfig::default()
    };
    assert!(timeout(0).validate().is_err());
    assert!(timeout(MAX_LANE_FIRST_BUILD_TIMEOUT_MIN + 1)
        .validate()
        .is_err());
    assert_eq!(timeout(MAX_LANE_FIRST_BUILD_TIMEOUT_MIN).validate(), Ok(()));

    for bad in [
        "NO_EQUALS",
        "1BAD=x",
        "BAD NAME=x",
        "KRONN_PORT=1",
        "CARGO_TARGET_DIR=/tmp/x",
        "CARGO_HOME=/tmp/x",
        "PATH=/bin",
    ] {
        assert!(parse_lane_env_entry(bad).is_err(), "{bad} must be refused");
        let config = LaneWarmConfig {
            compiler_env: Some(vec![bad.into()]),
            ..LaneWarmConfig::default()
        };
        assert!(config.validate().is_err(), "{bad}");
    }
    let long = LaneWarmConfig {
        first_build_command: Some("x".repeat(5_000)),
        ..LaneWarmConfig::default()
    };
    assert!(long.validate().is_err());
    let nul = LaneWarmConfig {
        first_build_command: Some("a\0b".into()),
        ..LaneWarmConfig::default()
    };
    assert!(nul.validate().is_err());
}

#[test]
fn blank_values_are_stored_as_unset() {
    let stored = LaneWarmConfig {
        first_build_command: Some("   ".into()),
        compiler_env: Some(vec!["  ".into(), " CC = clang ".into()]),
        ..LaneWarmConfig::default()
    }
    .normalized();
    assert_eq!(stored.first_build_command, None);
    assert_eq!(stored.compiler_env, Some(vec!["CC = clang".to_string()]));
    assert!(LaneWarmConfig::default().is_empty());
    assert!(!stored.is_empty());
}

#[test]
fn the_workers_config_carries_the_global_entry_and_it_survives_a_toml_round_trip() {
    let workers = WorkersConfig {
        lane_warm: LaneWarmConfig {
            seed_alignment: Some(false),
            compiler_env: Some(vec!["CC=clang".into()]),
            first_build_command: Some("npm ci".into()),
            first_build_timeout_min: Some(30),
            rust_roots: None,
        },
        ..WorkersConfig::default()
    };
    assert_eq!(workers.validate(), Ok(()));
    let text = toml::to_string(&workers).unwrap();
    let back: WorkersConfig = toml::from_str(&text).unwrap();
    assert_eq!(back, workers);

    // A config.toml written before KT-1742 has no such table: all detected.
    let old: WorkersConfig = toml::from_str("default_max_review_rounds = 3\n").unwrap();
    assert!(old.lane_warm.is_empty());

    let mut invalid = workers.clone();
    invalid.lane_warm.first_build_timeout_min = Some(0);
    assert!(
        invalid.validate().is_err(),
        "the global form is validated too"
    );
}

#[test]
fn a_projects_workspace_settings_carry_the_lane_entry_beside_the_hooks() {
    let workspace = ProjectWorkspace {
        lane_warm: LaneWarmConfig {
            first_build_command: Some("composer install".into()),
            ..LaneWarmConfig::default()
        },
        ..ProjectWorkspace::default()
    };
    let json = serde_json::to_string(&workspace).unwrap();
    let back: ProjectWorkspace = serde_json::from_str(&json).unwrap();
    assert_eq!(back, workspace);
    // A row stored before KT-1742 holds only the hooks.
    let old: ProjectWorkspace = serde_json::from_str("{\"hooks\":{}}").unwrap();
    assert!(old.lane_warm.is_empty());
}
