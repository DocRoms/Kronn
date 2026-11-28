//! KT-2002: a plain `cargo test` in a crate writes its ts-rs bindings to
//! `./bindings` under that crate's directory unless `TS_RS_EXPORT_DIR` is set.
//! Those folders must be git-ignored, or the worker's delivery is refused for an
//! untracked folder. The crates are found from their manifests, not hard-coded,
//! so a new exporting crate is covered without editing this test.

use std::fs;
use std::path::{Path, PathBuf};

/// `backend/`: the workspace root, where `.gitignore` names `bindings/`. The
/// same source also compiles as the `kronn-l0` package, whose manifest dir is
/// `backend/crates/l0`, so walk up to the manifest that declares `[workspace]`.
fn backend_dir() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let is_root = fs::read_to_string(dir.join("Cargo.toml"))
            .is_ok_and(|manifest| manifest.lines().any(|line| line.trim() == "[workspace]"));
        if is_root {
            return dir;
        }
        if !dir.pop() {
            return PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        }
    }
}

/// Directory names under `backend/crates/` whose `Cargo.toml` depends on `ts-rs`.
fn exporting_crates(backend: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(entries) = fs::read_dir(backend.join("crates")) else {
        return names;
    };
    for entry in entries.flatten() {
        let Ok(manifest) = fs::read_to_string(entry.path().join("Cargo.toml")) else {
            continue;
        };
        let depends_on_ts_rs = manifest
            .lines()
            .any(|line| line.split('=').next().map(str::trim) == Some("ts-rs"));
        if depends_on_ts_rs {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}

#[test]
fn ts_rs_bindings_are_git_ignored_in_every_exporting_crate() {
    let backend = backend_dir();
    let inside_checkout =
        crate::core::cmd::agent_git_output(&backend, &["rev-parse", "--is-inside-work-tree"], &[])
            .is_some();
    if !inside_checkout {
        eprintln!("skipped: {} is not a git checkout", backend.display());
        return;
    }

    let crates = exporting_crates(&backend);
    assert!(
        !crates.is_empty(),
        "no crate under backend/crates/ depends on ts-rs: the scan is broken"
    );

    // Every folder a workspace test run can write to: each exporting crate's
    // default `./bindings`, and the workspace root's `bindings/`.
    let mut written: Vec<String> = crates
        .iter()
        .map(|name| format!("crates/{name}/bindings/x.ts"))
        .collect();
    written.push("bindings/x.ts".to_string());

    let mut problems = Vec::new();
    for rel in &written {
        if !crate::core::anti_halluc::is_git_ignored(&backend, rel) {
            problems.push(format!("not git-ignored: backend/{rel}"));
        }
    }
    for name in &crates {
        let tracked = crate::core::cmd::agent_git_output(
            &backend,
            &["ls-files", "--", &format!("crates/{name}/bindings")],
            &[],
        )
        .unwrap_or_default();
        if !tracked.is_empty() {
            problems.push(format!("tracked under backend/crates/{name}/bindings"));
        }
    }
    assert!(
        problems.is_empty(),
        "ts-rs export folders a plain `cargo test` writes to must be ignored:\n{}",
        problems.join("\n")
    );
}
