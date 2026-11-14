use super::*;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn labels(roots: &[RustRoot]) -> Vec<String> {
    roots.iter().map(RustRoot::label).collect()
}

#[test]
fn rust_roots_are_found_from_the_manifests_wherever_they_are() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "backend/Cargo.toml", "");
    write(repo, "redis-flush/Cargo.toml", "");
    write(repo, "desktop/src-tauri/Cargo.toml", "");
    write(repo, "docs/readme.md", "");
    write(repo, "frontend/package.json", "{}");

    let roots = rust_roots(repo);

    assert_eq!(
        labels(&roots),
        ["backend", "desktop/src-tauri", "redis-flush"]
    );
    assert_eq!(roots[0].manifest_dir, repo.join("backend"));
    assert_eq!(
        roots[0].in_checkout(Path::new("/main")),
        Path::new("/main/backend")
    );
}

#[test]
fn the_roots_a_project_names_are_kept_and_an_unknown_one_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "backend/Cargo.toml", "");
    write(repo, "redis-flush/Cargo.toml", "");
    write(repo, "desktop/src-tauri/Cargo.toml", "");

    let (all, unknown) = select_roots(rust_roots(repo), None);
    assert_eq!(all.len(), 3);
    assert!(unknown.is_empty());

    let wanted = vec![
        "redis-flush".to_string(),
        "./backend/".to_string(),
        "gone".to_string(),
    ];
    let (kept, unknown) = select_roots(rust_roots(repo), Some(&wanted));
    assert_eq!(labels(&kept), ["backend", "redis-flush"]);
    assert_eq!(unknown, ["gone"]);
}

#[test]
fn a_manifest_at_the_root_is_the_root_and_its_members_are_not_roots() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(
        repo,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n",
    );
    write(repo, "crates/a/Cargo.toml", "");
    write(repo, "crates/b/Cargo.toml", "");

    let roots = rust_roots(repo);

    assert_eq!(
        labels(&roots),
        ["."],
        "the workspace root covers its members"
    );
    assert_eq!(roots[0].rel_dir, PathBuf::new());
}

#[test]
fn dependency_build_and_hidden_trees_are_never_searched() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "node_modules/pkg/Cargo.toml", "");
    write(repo, "target/debug/build/x/Cargo.toml", "");
    write(repo, "vendor/crate/Cargo.toml", "");
    write(repo, ".kronn/worktrees/task-x/backend/Cargo.toml", "");
    write(repo, ".hidden/Cargo.toml", "");
    write(repo, "svc/Cargo.toml", "");
    // Deeper than the search goes.
    write(repo, "a/b/c/d/e/Cargo.toml", "");

    assert_eq!(labels(&rust_roots(repo)), ["svc"]);
}

#[test]
fn a_repository_without_a_manifest_has_no_rust_root() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "composer.json", "{}");
    write(dir.path(), "src/index.php", "");
    assert!(rust_roots(dir.path()).is_empty());
    assert!(!builds_c_through_cc(dir.path()));
}

#[test]
fn a_project_builds_c_through_cc_when_a_root_lock_lists_the_crate() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "svc/Cargo.toml", "");
    write(
        repo,
        "svc/Cargo.lock",
        "[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n",
    );
    assert!(!builds_c_through_cc(repo), "no cc in the lock");

    write(
        repo,
        "svc/Cargo.lock",
        "[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n\n[[package]]\nname = \"cc\"\nversion = \"1.2.0\"\n",
    );
    assert!(builds_c_through_cc(repo));

    // A package merely named like it is not it.
    write(
        repo,
        "svc/Cargo.lock",
        "[[package]]\nname = \"cc-rs-fork\"\nversion = \"1.0.0\"\n",
    );
    assert!(!builds_c_through_cc(repo));
}

#[test]
fn the_lock_hash_of_one_root_is_the_hash_of_its_lock_and_several_roots_combine() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "a/Cargo.toml", "");
    write(repo, "a/Cargo.lock", "lock a");
    write(repo, "b/Cargo.toml", "");

    let roots = rust_roots(repo);
    assert_eq!(roots.len(), 2);
    // `b` has no lock: the combined hash still reflects `a`'s and says `b` has none.
    let both = combined_lock_hash(&roots).unwrap();
    assert_ne!(Some(both.clone()), text_hash_opt("lock a"));

    let only_a = &roots[..1];
    assert_eq!(combined_lock_hash(only_a), text_hash_opt("lock a"));
    assert_eq!(combined_lock_hash(&roots[1..]), None, "no lock, no hash");

    write(repo, "a/Cargo.lock", "lock a, bumped");
    assert_ne!(
        combined_lock_hash(&roots),
        Some(both),
        "a changed lock changes it"
    );
}

fn text_hash_opt(text: &str) -> Option<String> {
    Some(text_hash(text))
}

#[test]
fn a_library_target_is_the_default_path_or_a_lib_table() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "bin/Cargo.toml", "[package]\nname = \"x\"\n");
    write(repo, "lib/Cargo.toml", "[package]\nname = \"y\"\n");
    write(repo, "lib/src/lib.rs", "");
    write(
        repo,
        "custom/Cargo.toml",
        "[package]\nname = \"z\"\n\n[lib]\npath = \"core/mod.rs\"\n",
    );
    assert!(!has_lib_target(&repo.join("bin")));
    assert!(has_lib_target(&repo.join("lib")));
    assert!(has_lib_target(&repo.join("custom")));
}

#[test]
fn cargo_metadata_gives_the_target_directory_the_owner_builds_into() {
    let json = br#"{"packages":[],"workspace_root":"/repo/redis-flush","target_directory":"/shared/cargo-target","version":1}"#;
    assert_eq!(
        parse_metadata(json).unwrap(),
        CargoMetadata {
            target_directory: PathBuf::from("/shared/cargo-target"),
            workspace_root: PathBuf::from("/repo/redis-flush"),
        }
    );
    assert!(parse_metadata(b"not json").is_err());
    assert!(parse_metadata(br#"{"workspace_root":"/x"}"#)
        .unwrap_err()
        .contains("target_directory"));
}
