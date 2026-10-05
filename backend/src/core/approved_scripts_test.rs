use super::*;
use crate::core::repository_resources::sha256;

fn file(path: &str, hash: &str) -> ExecScriptFile {
    ExecScriptFile {
        path: path.into(),
        sha256: hash.into(),
    }
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("scripts/lib")).unwrap();
    std::fs::write(
        dir.path().join("scripts/run.cjs"),
        "require('./lib/h.cjs');\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("scripts/lib/h.cjs"),
        "module.exports = 1;\n",
    )
    .unwrap();
    dir
}

#[test]
fn declaration_refuses_paths_that_leave_the_repository() {
    for path in [
        "../outside.js",
        "scripts/../../x.js",
        "/etc/passwd",
        "",
        " scripts/run.cjs",
        "scripts\\run.cjs",
        ".",
    ] {
        assert!(
            validate_declaration(&[file(path, "")]).is_err(),
            "{path:?} accepted"
        );
    }
    assert!(validate_declaration(&[file("scripts/été.py", "")]).is_ok());
    assert!(validate_declaration(&[file("./scripts/run.cjs", "")]).is_ok());
}

#[test]
fn declaration_refuses_duplicates_bad_hashes_and_too_many_files() {
    let err = validate_declaration(&[file("a.js", ""), file("./a.js", "")]).unwrap_err();
    assert!(err.contains("declared twice"), "{err}");
    assert!(validate_declaration(&[file("a.js", "ABC")]).is_err());
    assert!(validate_declaration(&[file("a.js", &"A".repeat(64))]).is_err());
    assert!(validate_declaration(&[file("a.js", &"a".repeat(64))]).is_ok());
    let many: Vec<_> = (0..=MAX_FILES)
        .map(|n| file(&format!("f{n}.js"), ""))
        .collect();
    assert!(validate_declaration(&many).is_err());
}

#[cfg(unix)]
#[test]
fn a_symlink_escaping_the_repository_is_refused_and_one_inside_is_read() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.js"), "x").unwrap();
    let dir = repo();
    std::os::unix::fs::symlink(outside.path().join("secret.js"), dir.path().join("leak.js"))
        .unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("linked")).unwrap();
    std::os::unix::fs::symlink(
        dir.path().join("scripts/run.cjs"),
        dir.path().join("alias.cjs"),
    )
    .unwrap();

    let mut leak = [file("leak.js", "")];
    assert!(validate_and_pin(dir.path(), &mut leak)
        .unwrap_err()
        .contains("outside the repository"));
    let mut through_dir = [file("linked/secret.js", "")];
    assert!(validate_and_pin(dir.path(), &mut through_dir).is_err());
    let mut inside = [file("alias.cjs", "")];
    validate_and_pin(dir.path(), &mut inside).unwrap();
    assert_eq!(inside[0].sha256, sha256(b"require('./lib/h.cjs');\n"));
}

#[test]
fn saving_pins_empty_hashes_and_keeps_given_ones() {
    let dir = repo();
    let kept = "b".repeat(64);
    let mut files = [
        file("scripts/run.cjs", ""),
        file("scripts/lib/h.cjs", &kept),
    ];
    validate_and_pin(dir.path(), &mut files).unwrap();
    assert_eq!(files[0].sha256, sha256(b"require('./lib/h.cjs');\n"));
    assert_eq!(files[1].sha256, kept);

    let mut missing = [file("scripts/missing.cjs", "")];
    assert!(validate_and_pin(dir.path(), &mut missing).is_err());
    let mut directory = [file("scripts/lib", "")];
    assert!(validate_and_pin(dir.path(), &mut directory)
        .unwrap_err()
        .contains("not a regular file"));
}

#[test]
fn status_reports_approved_changed_pending_and_invalid() {
    let dir = repo();
    let run_hash = sha256(b"require('./lib/h.cjs');\n");
    let statuses = status(
        Some(dir.path()),
        &[
            file("scripts/run.cjs", &run_hash),
            file("scripts/lib/h.cjs", &"c".repeat(64)),
            file("scripts/new.cjs", ""),
            file("scripts/run.cjs", ""),
        ],
    );
    assert_eq!(statuses[0].state, ExecScriptFileState::Approved);
    assert_eq!(statuses[1].state, ExecScriptFileState::Changed);
    assert_eq!(statuses[2].state, ExecScriptFileState::Invalid);
    assert_eq!(statuses[3].state, ExecScriptFileState::Pending);
    assert_eq!(
        status(None, &[file("a.js", "")])[0].state,
        ExecScriptFileState::Invalid
    );
}

#[test]
fn a_one_line_change_to_the_script_or_a_module_blocks_the_copy_and_names_the_file() {
    let dir = repo();
    let artifacts = tempfile::tempdir().unwrap();
    let mut files = vec![file("scripts/run.cjs", ""), file("scripts/lib/h.cjs", "")];
    validate_and_pin(dir.path(), &mut files).unwrap();
    let dest = copy_dir(artifacts.path(), "run");
    prepare_copy(dir.path(), &files, &dest).unwrap();
    assert_eq!(
        std::fs::read_to_string(dest.join("scripts/lib/h.cjs")).unwrap(),
        "module.exports = 1;\n"
    );

    std::fs::write(
        dir.path().join("scripts/run.cjs"),
        "require('./lib/h.cjs');\n// x\n",
    )
    .unwrap();
    let err = prepare_copy(dir.path(), &files, &dest).unwrap_err();
    assert!(
        err.contains("`scripts/run.cjs` changed since approval"),
        "{err}"
    );

    std::fs::write(
        dir.path().join("scripts/run.cjs"),
        "require('./lib/h.cjs');\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("scripts/lib/h.cjs"),
        "module.exports = 2;\n",
    )
    .unwrap();
    let err = prepare_copy(dir.path(), &files, &dest).unwrap_err();
    assert!(
        err.contains("`scripts/lib/h.cjs` changed since approval"),
        "{err}"
    );
}

#[test]
fn an_unapproved_file_is_never_copied() {
    let dir = repo();
    let artifacts = tempfile::tempdir().unwrap();
    let dest = copy_dir(artifacts.path(), "run");
    let err = prepare_copy(dir.path(), &[file("scripts/run.cjs", "")], &dest).unwrap_err();
    assert!(err.contains("no approved hash"), "{err}");
    assert!(!dest.exists());
}

#[test]
fn the_copy_replaces_a_previous_one_and_steps_get_distinct_directories() {
    let dir = repo();
    let artifacts = tempfile::tempdir().unwrap();
    let mut files = vec![file("scripts/run.cjs", "")];
    validate_and_pin(dir.path(), &mut files).unwrap();
    let dest = copy_dir(artifacts.path(), "étape");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("stale.js"), "x").unwrap();
    prepare_copy(dir.path(), &files, &dest).unwrap();
    assert!(!dest.join("stale.js").exists());
    assert!(dest.join("scripts/run.cjs").exists());
    assert_ne!(dest, copy_dir(artifacts.path(), "other"));
    assert!(dest.starts_with(artifacts.path()));
}
