//! KT-1747 — the per-project `dependency_dirs` override of the layered
//! dependency directories.

use super::*;

fn never_detected() -> Detected {
    Detected::default()
}

#[test]
fn without_a_project_entry_the_detected_dependency_dirs_apply() {
    let effective = resolve(&LaneWarmConfig::default(), None, never_detected);
    assert_eq!(effective.dependency_dirs, None);
}

#[test]
fn a_project_names_exactly_the_dependency_dirs_and_the_global_entry_cannot() {
    let global = LaneWarmConfig {
        dependency_dirs: Some(vec!["vendor".into()]),
        ..LaneWarmConfig::default()
    };
    let project = LaneWarmConfig {
        dependency_dirs: Some(vec!["Pods".into(), "assets/front/node_modules".into()]),
        ..LaneWarmConfig::default()
    };

    let own = resolve(&global, Some(&project), never_detected);
    assert_eq!(
        own.dependency_dirs,
        Some(vec![
            "Pods".to_string(),
            "assets/front/node_modules".to_string()
        ])
    );
    // The global entry cannot name directories of a repository it does not know.
    let inherited = resolve(&global, None, never_detected);
    assert_eq!(inherited.dependency_dirs, None);
}

#[test]
fn an_empty_project_list_means_layer_nothing_not_detect() {
    let project = LaneWarmConfig {
        dependency_dirs: Some(Vec::new()),
        ..LaneWarmConfig::default()
    };
    assert!(!project.is_empty());
    let normalized = project.clone().normalized();
    assert_eq!(normalized.dependency_dirs, Some(Vec::new()));
    let effective = resolve(
        &LaneWarmConfig::default(),
        Some(&normalized),
        never_detected,
    );
    assert_eq!(effective.dependency_dirs, Some(Vec::new()));
}

#[test]
fn the_stored_dependency_dirs_are_normalized_and_deduplicated() {
    let stored = LaneWarmConfig {
        dependency_dirs: Some(vec![
            " ./vendor/ ".into(),
            "vendor".into(),
            "  ".into(),
            "Pods".into(),
        ]),
        ..LaneWarmConfig::default()
    }
    .normalized();
    assert_eq!(
        stored.dependency_dirs,
        Some(vec!["vendor".to_string(), "Pods".to_string()])
    );
}

#[test]
fn a_dependency_dir_must_be_a_directory_inside_the_repository() {
    for bad in ["../outside", "/etc", "a/../../b", "."] {
        let config = LaneWarmConfig {
            dependency_dirs: Some(vec![bad.into()]),
            ..LaneWarmConfig::default()
        };
        assert!(config.validate().is_err(), "`{bad}` must be refused");
    }
    let many = LaneWarmConfig {
        dependency_dirs: Some((0..17).map(|n| format!("dir-{n}")).collect()),
        ..LaneWarmConfig::default()
    };
    assert!(many.validate().is_err());
    let ok = LaneWarmConfig {
        dependency_dirs: Some(vec!["vendor".into(), "assets/front/node_modules".into()]),
        ..LaneWarmConfig::default()
    };
    assert!(ok.validate().is_ok());
}
