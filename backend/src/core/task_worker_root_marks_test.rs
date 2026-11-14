use super::*;

fn env(flag: &str) -> Vec<(String, String)> {
    vec![("CFLAGS_x".to_string(), flag.to_string())]
}

#[test]
fn a_root_key_follows_the_root_its_lock_and_the_environment_and_nothing_else() {
    let base = root_key("backend", Some("aaa"), &env("-save-temps=obj"));
    assert_eq!(
        base,
        root_key("backend", Some("aaa"), &env("-save-temps=obj"))
    );
    assert_ne!(
        base,
        root_key("desktop", Some("aaa"), &env("-save-temps=obj"))
    );
    assert_ne!(
        base,
        root_key("backend", Some("bbb"), &env("-save-temps=obj"))
    );
    assert_ne!(base, root_key("backend", Some("aaa"), &env("-O2")));
    assert_ne!(base, root_key("backend", Some("aaa"), &[]));
    // The order of the variables is not a difference.
    let two = vec![
        ("B".to_string(), "1".to_string()),
        ("A".to_string(), "2".to_string()),
    ];
    let swapped = vec![two[1].clone(), two[0].clone()];
    assert_eq!(
        root_key("backend", None, &two),
        root_key("backend", None, &swapped)
    );
}

#[test]
fn a_failure_is_cached_per_root_and_key_and_cleared_by_a_success() {
    let seed = tempfile::tempdir().unwrap();
    assert_eq!(cached_failure(seed.path(), "desktop", "k1"), None);

    remember_failure(seed.path(), "desktop", "k1", "pkg-config: gtk not found");
    assert_eq!(
        cached_failure(seed.path(), "desktop", "k1").as_deref(),
        Some("pkg-config: gtk not found")
    );
    assert_eq!(
        cached_failure(seed.path(), "desktop", "k2"),
        None,
        "another lock or environment is another key: tried again"
    );
    assert_eq!(
        cached_failure(seed.path(), "backend", "k1"),
        None,
        "another root's failure is not this one's"
    );

    remember_failure(seed.path(), "backend", "k1", "boom");
    forget_failure(seed.path(), "desktop");
    assert_eq!(cached_failure(seed.path(), "desktop", "k1"), None);
    assert_eq!(
        cached_failure(seed.path(), "backend", "k1").as_deref(),
        Some("boom")
    );
    forget_failure(seed.path(), "backend");
    assert!(
        !seed.path().join(FAILURES_FILE).exists(),
        "an empty cache leaves no file"
    );
}

#[test]
fn a_failure_is_not_cached_where_there_is_no_seed_directory() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("target");
    remember_failure(&missing, "desktop", "k1", "boom");
    assert!(!missing.exists(), "the seed's directory is never created");
}

#[test]
fn upsert_replaces_a_root_by_path() {
    let mut marks = vec![RootMark::new("backend", "k1", RootState::Built)];
    upsert(
        &mut marks,
        RootMark::new("desktop", "k1", RootState::Failed { reason: "x".into() }),
    );
    upsert(&mut marks, RootMark::new("backend", "k2", RootState::Built));
    assert_eq!(marks.len(), 2);
    assert_eq!(marks[0].key, "k2");
    let json = serde_json::to_value(&marks[1]).unwrap();
    assert_eq!(json["state"], "failed");
    assert_eq!(json["reason"], "x");
}
