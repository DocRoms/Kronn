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

/// Cargo's hash of the compiler, as in a real `.fingerprint/*/lib-*.json`.
const RUSTC: u64 = 13462371384788945141;
const RUSTC_UPGRADED: u64 = 4906312778246931221;

/// The fields of a fingerprint JSON that tell one unit's configuration from
/// another's (the shape Cargo writes: `rustc`, `features`, `profile`, `target`,
/// `path`, `rustflags`, `compile_kind`...).
#[derive(Clone)]
struct Fp {
    rustc: u64,
    features: &'static str,
    profile: u64,
    target: u64,
    path: u64,
    /// `None`: an older Cargo that writes neither `rustflags` nor `compile_kind`.
    rustflags: Option<&'static str>,
}

impl Fp {
    /// A library, with the default feature set.
    fn lib() -> Fp {
        Fp {
            rustc: RUSTC,
            features: "[\\\"default\\\"]",
            profile: 11056920130593593451,
            target: 7848994504142944354,
            path: 7897662090217625916,
            rustflags: Some("[]"),
        }
    }

    fn features(self, features: &'static str) -> Fp {
        Fp { features, ..self }
    }

    fn rustc(self, rustc: u64) -> Fp {
        Fp { rustc, ..self }
    }

    fn json(&self) -> String {
        let tail = self
            .rustflags
            .map(|flags| {
                format!(",\"rustflags\":{flags},\"config\":9396254390672932401,\"compile_kind\":0")
            })
            .unwrap_or_default();
        format!(
            "{{\"rustc\":{},\"features\":\"{}\",\"declared_features\":\"[]\",\"target\":{},\
             \"profile\":{},\"path\":{},\"deps\":[[1,\"serde\",false,2]],\
             \"local\":[{{\"CheckDepInfo\":{{\"dep_info\":\"debug/.fingerprint/x/dep-lib-x\",\
             \"checksum\":false}}}}]{tail}}}",
            self.rustc, self.features, self.target, self.profile, self.path
        )
    }
}

/// The run of a build script: Cargo zeroes `target`, `profile` and `path`.
fn run_build_script_json() -> String {
    format!(
        "{{\"rustc\":{RUSTC},\"features\":\"\",\"declared_features\":\"\",\"target\":0,\
         \"profile\":0,\"path\":0,\"deps\":[],\"local\":[{{\"RerunIfChanged\":\
         {{\"output\":\"debug/build/x/output\",\"paths\":[\"build.rs\"]}}}}],\
         \"rustflags\":[],\"config\":0,\"compile_kind\":0}}"
    )
}

/// One unit as Cargo lays it out, with the default fingerprint of its kind.
fn unit(debug: &Path, package: &str, hash: &str, kind: &str, age_secs: u64) {
    let json = if kind.starts_with("run-build-script") {
        run_build_script_json()
    } else {
        Fp::lib().json()
    };
    unit_json(debug, package, hash, kind, age_secs, &json);
}

fn unit_with(debug: &Path, package: &str, hash: &str, kind: &str, age_secs: u64, fp: &Fp) {
    unit_json(debug, package, hash, kind, age_secs, &fp.json());
}

/// One unit as Cargo lays it out: a fingerprint, its deps files, its build dir.
fn unit_json(debug: &Path, package: &str, hash: &str, kind: &str, age_secs: u64, json: &str) {
    let fingerprint = debug.join(".fingerprint").join(format!("{package}-{hash}"));
    fs::create_dir_all(&fingerprint).unwrap();
    fs::write(fingerprint.join(format!("{kind}-{package}")), b"x").unwrap();
    fs::write(fingerprint.join(format!("{kind}-{package}.json")), json).unwrap();
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

/// The owner's seed (2026-11-11): 341 of 397 `(package, kind)` groups held more
/// than one unit, nearly all distinct valid variants (`hashbrown` with `["raw"]`,
/// `["default-hasher"]`, `[]`...). One build needs several of them.
#[test]
fn two_feature_sets_of_one_crate_written_days_apart_are_both_kept() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    let raw = Fp::lib().features("[\\\"raw\\\"]");
    let defaults = Fp::lib();
    unit_with(debug, "hashbrown", OLD, "lib", 40 * DAY, &raw);
    unit_with(debug, "hashbrown", NEW, "lib", DAY, &defaults);
    // The same configuration for the host (another profile) is a third variant.
    let host = Fp {
        profile: 7204689502382642620,
        ..Fp::lib()
    };
    unit_with(debug, "hashbrown", OTHER, "lib", 20 * DAY, &host);
    // And a feature set of the same crate for another target (another version).
    let other_version = Fp {
        target: 9101038166729729440,
        path: 4444478083175659815,
        ..Fp::lib()
    };
    unit_with(debug, "hashbrown", GONE, "lib", 30 * DAY, &other_version);

    let outcome = prune_seed(debug, None);

    assert_eq!(outcome, PruneOutcome::Pruned(PruneReport::default()));
    for hash in [OLD, NEW, OTHER, GONE] {
        assert!(has_unit(debug, "hashbrown", hash), "{hash} was pruned");
        assert_eq!(deps_files_of(debug, hash), 5);
    }
}

#[test]
fn an_older_unit_with_an_identical_identity_is_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    // Same package, kind, features, profile, target and rustflags: one unit
    // built twice (a dependency changed underneath it); only the last is current.
    let fp = Fp::lib().features("[\\\"std\\\"]");
    unit_with(debug, "log", OLD, "lib", 30 * DAY, &fp);
    unit_with(debug, "log", NEW, "lib", DAY, &fp);
    // Another feature set of the same crate, older than both, is not a leftover.
    unit_with(
        debug,
        "log",
        OTHER,
        "lib",
        60 * DAY,
        &Fp::lib().features("[]"),
    );

    let PruneOutcome::Pruned(report) = prune_seed(debug, None) else {
        panic!("skipped");
    };

    assert_eq!(report.removed_units, 1);
    assert!(!has_unit(debug, "log", OLD));
    assert_eq!(deps_files_of(debug, OLD), 0);
    assert!(!build_dir_exists(debug, "log", OLD));
    assert!(has_unit(debug, "log", NEW) && has_unit(debug, "log", OTHER));
}

#[test]
fn an_identity_absent_from_old_cargos_still_groups_on_what_is_there() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    let old_cargo = Fp {
        rustflags: None,
        ..Fp::lib()
    };
    unit_with(debug, "log", OLD, "lib", 30 * DAY, &old_cargo);
    unit_with(debug, "log", NEW, "lib", DAY, &old_cargo);

    prune_seed(debug, None);

    assert!(!has_unit(debug, "log", OLD) && has_unit(debug, "log", NEW));
}

#[test]
fn a_unit_built_by_another_rustc_is_pruned_whatever_its_age() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    // After a toolchain upgrade: serde is the only unit of its kind, and its
    // fingerprint says the old compiler built it.
    let old_compiler = Fp::lib().rustc(RUSTC_UPGRADED);
    unit_with(debug, "serde", OLD, "lib", 200 * DAY, &old_compiler);
    unit_with(debug, "kronn", NEW, "lib", DAY, &Fp::lib());
    unit_with(debug, "regex", OTHER, "lib", DAY, &Fp::lib());

    let PruneOutcome::Pruned(report) = prune_seed(debug, None) else {
        panic!("skipped");
    };

    assert_eq!(report.removed_units, 1);
    assert!(!has_unit(debug, "serde", OLD));
    assert_eq!(deps_files_of(debug, OLD), 0);
    assert!(has_unit(debug, "kronn", NEW) && has_unit(debug, "regex", OTHER));
}

#[test]
fn two_compilers_in_the_latest_writes_judge_nothing_by_compiler() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    // A stable and a nightly build in the same hour: no latest compiler.
    unit_with(debug, "serde", OLD, "lib", 90 * DAY, &Fp::lib());
    unit_with(
        debug,
        "kronn",
        NEW,
        "lib",
        60,
        &Fp::lib().rustc(RUSTC_UPGRADED),
    );
    unit_with(debug, "regex", OTHER, "lib", 120, &Fp::lib());

    prune_seed(debug, None);

    assert!(has_unit(debug, "serde", OLD));
    assert!(has_unit(debug, "kronn", NEW) && has_unit(debug, "regex", OTHER));
}

#[test]
fn a_unit_whose_fingerprint_cannot_be_parsed_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    // Three older units of one identity whose fingerprint is garbage, truncated,
    // or not a fingerprint (no `rustc`): never judged, whatever they look like.
    unit_json(debug, "log", OLD, "lib", 90 * DAY, "{\"rustc\":134,\"feat");
    unit_json(debug, "log", OTHER, "lib", 80 * DAY, "not json at all");
    unit_json(debug, "log", GONE, "lib", 70 * DAY, "{}");
    // A missing fingerprint file is unreadable too.
    unit_json(debug, "regex", "1111111111111111", "lib", 60 * DAY, "x");
    fs::remove_file(
        debug
            .join(".fingerprint/regex-1111111111111111")
            .join("lib-regex.json"),
    )
    .unwrap();
    // The current unit of the same package.
    unit_with(debug, "log", NEW, "lib", DAY, &Fp::lib());

    let outcome = prune_seed(debug, None);

    assert_eq!(outcome, PruneOutcome::Pruned(PruneReport::default()));
    for hash in [OLD, OTHER, GONE, NEW] {
        assert!(has_unit(debug, "log", hash), "{hash} was pruned");
    }
    assert!(has_unit(debug, "regex", "1111111111111111"));
    assert_eq!(deps_files_of(debug, OLD), 5);
}

/// The run of a build script is zeroed by Cargo, so two versions of one package
/// are indistinguishable by it: neither is ever judged leftover.
#[test]
fn the_zeroed_run_of_a_build_script_is_never_judged_by_identity() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    unit(
        debug,
        "getrandom",
        OLD,
        "run-build-script-build-script-build",
        30 * DAY,
    );
    unit(
        debug,
        "getrandom",
        NEW,
        "run-build-script-build-script-build",
        DAY,
    );
    unit(debug, "kronn", OTHER, "lib", DAY);

    prune_seed(debug, None);

    assert!(has_unit(debug, "getrandom", OLD) && has_unit(debug, "getrandom", NEW));
}

/// The flaw this rule replaces: grouping by `(package, kind)` pruned a still
/// valid variant, the next build recreated it and became the newest, and the
/// next lane pruned the other one, forever.
#[test]
fn a_prune_right_after_a_rebuild_removes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path();
    let raw = Fp::lib().features("[\\\"raw\\\"]");
    let defaults = Fp::lib();
    unit_with(debug, "hashbrown", OLD, "lib", 40 * DAY, &raw);
    unit_with(debug, "hashbrown", NEW, "lib", 30 * DAY, &defaults);

    // Lane 1 provisions: both variants are valid, nothing goes.
    assert_eq!(
        prune_seed(debug, None),
        PruneOutcome::Pruned(PruneReport::default())
    );

    // The main checkout rebuilds the `raw` variant (a dependency changed): a new
    // hash of the same identity, now the newest unit of the package.
    unit_with(debug, "hashbrown", OTHER, "lib", 0, &raw);

    // Lane 2: only the superseded `raw` unit goes; the default-feature variant
    // the build still needs is untouched.
    let PruneOutcome::Pruned(report) = prune_seed(debug, None) else {
        panic!("skipped");
    };
    assert_eq!(report.removed_units, 1);
    assert!(!has_unit(debug, "hashbrown", OLD));
    assert!(has_unit(debug, "hashbrown", OTHER) && has_unit(debug, "hashbrown", NEW));

    // Lane 3, and the next ones: a fixed point.
    for _ in 0..3 {
        assert_eq!(
            prune_seed(debug, None),
            PruneOutcome::Pruned(PruneReport::default())
        );
    }
    assert!(has_unit(debug, "hashbrown", OTHER) && has_unit(debug, "hashbrown", NEW));
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
