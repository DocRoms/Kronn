//! KT-1691 — the seed build dir is pruned to its latest build, never while a
//! build uses it, never by age alone.

use super::*;
use std::fs;

const DAY: u64 = 24 * 3600;
const OLD: &str = "aaaaaaaaaaaaaaaa";
const NEW: &str = "bbbbbbbbbbbbbbbb";
const OTHER: &str = "cccccccccccccccc";
const GONE: &str = "dddddddddddddddd";

fn set_age(path: &Path, age_secs: u64) {
    let time = SystemTime::now() - Duration::from_secs(age_secs);
    fs::File::open(path).unwrap().set_modified(time).unwrap();
}

/// One unit as Cargo lays it out: a fingerprint, its deps files, its build dir.
fn unit(debug: &Path, package: &str, hash: &str, kind: &str, age_secs: u64) {
    let fingerprint = debug.join(".fingerprint").join(format!("{package}-{hash}"));
    fs::create_dir_all(&fingerprint).unwrap();
    fs::write(fingerprint.join(format!("{kind}-{package}")), b"x").unwrap();
    fs::write(fingerprint.join(format!("{kind}-{package}.json")), b"{}").unwrap();
    set_age(&fingerprint.join(format!("{kind}-{package}")), age_secs);
    set_age(
        &fingerprint.join(format!("{kind}-{package}.json")),
        age_secs,
    );
    set_age(&fingerprint, age_secs);
    let deps = debug.join("deps");
    fs::create_dir_all(&deps).unwrap();
    let crate_name = package.replace('-', "_");
    for file in [
        format!("lib{crate_name}-{hash}.rlib"),
        format!("lib{crate_name}-{hash}.rmeta"),
        format!("{crate_name}-{hash}.d"),
        format!("{crate_name}-{hash}"),
        format!("{crate_name}-{hash}.0a1b2c.rcgu.o"),
    ] {
        fs::write(deps.join(file), vec![0u8; 100]).unwrap();
    }
    let build = debug.join("build").join(format!("{package}-{hash}"));
    fs::create_dir_all(&build).unwrap();
    fs::write(build.join("output"), b"y").unwrap();
    fs::write(debug.join(".cargo-lock"), b"").unwrap();
}

fn has_unit(debug: &Path, package: &str, hash: &str) -> bool {
    debug
        .join(".fingerprint")
        .join(format!("{package}-{hash}"))
        .exists()
}

fn deps_files_of(debug: &Path, hash: &str) -> usize {
    fs::read_dir(debug.join("deps"))
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(hash))
        .count()
}

fn build_dir_exists(debug: &Path, package: &str, hash: &str) -> bool {
    debug
        .join("build")
        .join(format!("{package}-{hash}"))
        .exists()
}

#[test]
fn a_superseded_unit_goes_with_every_file_of_its_hash_and_the_latest_stays() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    // kronn's test binaries across three months of rustc and feature changes.
    unit(debug, "kronn", OLD, "test-lib", 90 * DAY);
    unit(debug, "kronn", OTHER, "test-lib", 40 * DAY);
    unit(debug, "kronn", NEW, "test-lib", DAY);

    let outcome = prune_seed(debug, None);

    let PruneOutcome::Pruned(report) = outcome else {
        panic!("not skipped: {outcome:?}");
    };
    assert_eq!(report.removed_units, 2);
    assert!(!has_unit(debug, "kronn", OLD) && !has_unit(debug, "kronn", OTHER));
    assert_eq!(deps_files_of(debug, OLD) + deps_files_of(debug, OTHER), 0);
    assert!(!build_dir_exists(debug, "kronn", OLD));
    // The latest build is whole.
    assert!(has_unit(debug, "kronn", NEW));
    assert_eq!(deps_files_of(debug, NEW), 5);
    assert!(build_dir_exists(debug, "kronn", NEW));
    assert!(report.removed_bytes >= 2 * 5 * 100, "{report:?}");
}

#[test]
fn a_unit_is_never_pruned_by_age_alone() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    // An unchanged dependency keeps its months-old mtime: it is the seed's value.
    unit(debug, "serde", OLD, "lib", 200 * DAY);
    // The library and the test build of one package are two kinds, both current.
    unit(debug, "kronn", NEW, "lib", 30 * DAY);
    unit(debug, "kronn", OTHER, "test-lib", DAY);

    let outcome = prune_seed(debug, None);

    assert_eq!(outcome, PruneOutcome::Pruned(PruneReport::default()));
    assert!(has_unit(debug, "serde", OLD));
    assert!(has_unit(debug, "kronn", NEW) && has_unit(debug, "kronn", OTHER));
    assert_eq!(deps_files_of(debug, OLD), 5);
}

#[test]
fn units_written_by_one_build_are_siblings_and_stay() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    // Two feature sets of a crate, built together a few minutes apart.
    unit(debug, "syn", OLD, "lib", 10 * 60);
    unit(debug, "syn", NEW, "lib", 5);

    prune_seed(debug, None);

    assert!(has_unit(debug, "syn", OLD) && has_unit(debug, "syn", NEW));
}

#[test]
fn a_package_the_lock_no_longer_lists_is_removed_whatever_its_age() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    unit(debug, "left-pad", GONE, "lib", 2);
    unit(debug, "serde_json", NEW, "lib", 2);
    let locked = locked_package_names([
        "version = 3\n\n[[package]]\nname = \"serde-json\"\nversion = \"1.0.0\"\n\n\
         [[package]]\nname = \"kronn\"\nversion = \"0.1.0\"\n",
    ])
    .unwrap();

    prune_seed(debug, Some(&locked));

    assert!(!has_unit(debug, "left-pad", GONE));
    assert_eq!(deps_files_of(debug, GONE), 0);
    // `-` and `_` are one name.
    assert!(has_unit(debug, "serde_json", NEW));
}

#[test]
fn an_unread_lock_never_makes_every_unit_look_removed() {
    assert_eq!(locked_package_names(["not a lock file"]), None);
    assert_eq!(locked_package_names(std::iter::empty::<&str>()), None);
    let dir = tempfile::tempdir().unwrap();
    unit(dir.path(), "serde", OLD, "lib", 2);
    prune_seed(dir.path(), None);
    assert!(has_unit(dir.path(), "serde", OLD));
}

/// The DoD: "never while a build uses it". Cargo holds its lock for a build's
/// whole duration; a held lock (shared or exclusive) is a no-op that says so.
#[test]
fn a_build_in_progress_is_never_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    unit(debug, "kronn", OLD, "test-lib", 90 * DAY);
    unit(debug, "kronn", NEW, "test-lib", DAY);

    let building = fs::File::open(debug.join(".cargo-lock")).unwrap();
    FileExt::lock_exclusive(&building).unwrap();
    let during = prune_seed(debug, None);
    assert!(
        matches!(&during, PruneOutcome::Skipped(reason) if reason.contains("build")),
        "{during:?}"
    );
    assert!(has_unit(debug, "kronn", OLD), "the stale unit is untouched");
    assert_eq!(deps_files_of(debug, OLD), 5);

    // A reader of the seed (a clone in progress) holds it shared: same.
    FileExt::unlock(&building).unwrap();
    FileExt::lock_shared(&building).unwrap();
    assert!(matches!(prune_seed(debug, None), PruneOutcome::Skipped(_)));
    assert!(has_unit(debug, "kronn", OLD));

    // The build is over: the prune runs.
    FileExt::unlock(&building).unwrap();
    assert!(matches!(prune_seed(debug, None), PruneOutcome::Pruned(_)));
    assert!(!has_unit(debug, "kronn", OLD));
}

#[test]
fn the_lock_is_handed_back_shared_so_a_starting_build_waits_for_the_clone() {
    let dir = tempfile::tempdir().unwrap();
    unit(dir.path(), "kronn", NEW, "lib", DAY);

    let (lock, _) = prune_and_lock_shared(dir.path(), None).unwrap();

    let build = fs::File::open(dir.path().join(".cargo-lock")).unwrap();
    assert!(
        FileExt::try_lock_exclusive(&build).is_err(),
        "a build cannot start while the seed is being cloned"
    );
    FileExt::unlock(&lock).unwrap();
    assert!(FileExt::try_lock_exclusive(&build).is_ok());
}

#[test]
fn a_dir_without_a_cargo_lock_is_not_a_profile_dir_and_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".fingerprint/kronn-aaaaaaaaaaaaaaaa")).unwrap();
    assert!(matches!(
        prune_seed(dir.path(), None),
        PruneOutcome::Skipped(_)
    ));
    assert!(dir
        .path()
        .join(".fingerprint/kronn-aaaaaaaaaaaaaaaa")
        .exists());
}

#[test]
fn deps_entries_are_matched_to_their_hash_exactly() {
    assert_eq!(
        hash_of_deps_entry("libserde-0123456789abcdef.rlib"),
        Some("0123456789abcdef")
    );
    assert_eq!(
        hash_of_deps_entry("kronn-0123456789abcdef"),
        Some("0123456789abcdef")
    );
    assert_eq!(
        hash_of_deps_entry("kronn-0123456789abcdef.1x2y.rcgu.o"),
        Some("0123456789abcdef")
    );
    assert_eq!(hash_of_deps_entry("kronn-0123456789abcdefg.rlib"), None);
    assert_eq!(hash_of_deps_entry("kronn-0123456789ABCDEF.rlib"), None);
    assert_eq!(hash_of_deps_entry("README"), None);
}
