use super::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kronn_checksums_test_{}", name));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn compute_sha256_known_content() {
    let dir = temp_dir("sha256_known");
    let file = dir.join("hello.txt");
    fs::write(&file, "hello").unwrap();

    let hash = compute_sha256(&file).unwrap();
    // SHA-256 of "hello" (no newline)
    assert_eq!(
        hash,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compute_sha256_missing_file() {
    let result = compute_sha256(Path::new("/tmp/kronn_nonexistent_file_xyz.txt"));
    assert!(result.is_none());
}

#[test]
fn write_and_read_roundtrip() {
    let dir = temp_dir("roundtrip");

    let mut checksums = BTreeMap::new();
    checksums.insert("file_a.rs".to_string(), "abc123".to_string());
    checksums.insert("file_b.rs".to_string(), "def456".to_string());

    let mappings = vec![ChecksumMapping {
        ai_file: "ai/audit.md".to_string(),
        audit_step: 1,
        sources: vec!["src/*.rs".to_string()],
        checksums,
    }];

    write_checksums_file(&dir, &mappings).unwrap();

    let read_back = read_checksums_file(&dir).expect("should read back checksums file");
    assert_eq!(read_back.mappings.len(), 1);
    assert_eq!(read_back.mappings[0].ai_file, "ai/audit.md");
    assert_eq!(read_back.mappings[0].audit_step, 1);
    assert_eq!(read_back.mappings[0].checksums.len(), 2);
    assert_eq!(
        read_back.mappings[0].checksums.get("file_a.rs").unwrap(),
        "abc123"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_drift_no_checksums_file() {
    let dir = temp_dir("no_checksums");

    let result = check_drift(&dir);
    assert!(result.audit_date.is_none());
    assert!(result.stale_sections.is_empty());
    assert!(result.fresh_sections.is_empty());
    assert_eq!(result.total_sections, 0);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_drift_all_fresh() {
    let dir = temp_dir("all_fresh");

    // Create a source file
    let src_dir = dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("main.rs"), "fn main() {}").unwrap();

    // Generate checksums for that file
    let checksums = compute_step_checksums(&dir, &["src/main.rs"]);

    let mappings = vec![ChecksumMapping {
        ai_file: "ai/audit.md".to_string(),
        audit_step: 1,
        sources: vec!["src/main.rs".to_string()],
        checksums,
    }];

    write_checksums_file(&dir, &mappings).unwrap();

    // Check drift — file hasn't changed
    let result = check_drift(&dir);
    assert!(result.audit_date.is_some());
    assert!(result.stale_sections.is_empty());
    assert_eq!(result.fresh_sections.len(), 1);
    assert_eq!(result.total_sections, 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_drift_detects_change() {
    let dir = temp_dir("detects_change");

    // Create a source file
    let src_dir = dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("main.rs"), "fn main() {}").unwrap();

    // Generate and write checksums
    let checksums = compute_step_checksums(&dir, &["src/main.rs"]);
    let mappings = vec![ChecksumMapping {
        ai_file: "ai/audit.md".to_string(),
        audit_step: 1,
        sources: vec!["src/main.rs".to_string()],
        checksums,
    }];
    write_checksums_file(&dir, &mappings).unwrap();

    // Modify the source file
    fs::write(src_dir.join("main.rs"), "fn main() { println!(\"changed\"); }").unwrap();

    // Check drift — should detect the change
    let result = check_drift(&dir);
    assert!(result.audit_date.is_some());
    assert_eq!(result.stale_sections.len(), 1);
    assert_eq!(result.stale_sections[0].ai_file, "ai/audit.md");
    assert!(result.stale_sections[0]
        .changed_sources
        .iter()
        .any(|s| s.contains("src/main.rs")));
    assert!(result.fresh_sections.is_empty());
    assert_eq!(result.total_sections, 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_drift_detects_new_file() {
    let dir = temp_dir("detects_new");

    // Create directory but no file yet
    let src_dir = dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    // Generate checksums when file doesn't exist (will be empty)
    let checksums = compute_step_checksums(&dir, &["src/lib.rs"]);
    assert!(checksums.is_empty());

    let mappings = vec![ChecksumMapping {
        ai_file: "ai/audit.md".to_string(),
        audit_step: 2,
        sources: vec!["src/lib.rs".to_string()],
        checksums,
    }];
    write_checksums_file(&dir, &mappings).unwrap();

    // Now create the file
    fs::write(src_dir.join("lib.rs"), "pub fn lib() {}").unwrap();

    // Check drift — should detect the new file
    let result = check_drift(&dir);
    assert!(result.audit_date.is_some());
    assert_eq!(result.stale_sections.len(), 1);
    assert_eq!(result.stale_sections[0].audit_step, 2);
    assert!(result.stale_sections[0]
        .changed_sources
        .iter()
        .any(|s| s.contains("(new)")));
    assert_eq!(result.total_sections, 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_drift_detects_deleted_file() {
    let dir = temp_dir("detects_deleted");

    // Create a source file and generate checksums
    let src_dir = dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("to_delete.rs"), "fn delete_me() {}").unwrap();

    let checksums = compute_step_checksums(&dir, &["src/to_delete.rs"]);
    assert!(!checksums.is_empty(), "should have a checksum for the file");

    let mappings = vec![ChecksumMapping {
        ai_file: "ai/audit.md".to_string(),
        audit_step: 1,
        sources: vec!["src/to_delete.rs".to_string()],
        checksums,
    }];
    write_checksums_file(&dir, &mappings).unwrap();

    // Delete the file
    fs::remove_file(src_dir.join("to_delete.rs")).unwrap();

    // Check drift — deleted file should make the section stale
    let result = check_drift(&dir);
    assert!(result.audit_date.is_some());
    assert_eq!(result.stale_sections.len(), 1, "deleted file should trigger stale");
    assert_eq!(result.total_sections, 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compute_step_checksums_ignores_missing_files() {
    let dir = temp_dir("missing_files");
    fs::create_dir_all(&dir).unwrap();

    // Call with patterns for files that don't exist
    let checksums = compute_step_checksums(&dir, &["nonexistent/*.rs", "also_missing.txt"]);
    assert!(checksums.is_empty(), "missing files should produce empty checksums");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compute_step_checksums_handles_git_head() {
    // Use the Kronn repo root (works both locally and in CI)
    let kronn_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let checksums = compute_step_checksums(&kronn_root, &["__GIT_HEAD__"]);
    assert!(
        checksums.contains_key("__GIT_HEAD__"),
        "should contain __GIT_HEAD__ key, got: {:?}",
        checksums.keys().collect::<Vec<_>>()
    );
    let hash = checksums.get("__GIT_HEAD__").unwrap();
    assert!(!hash.is_empty(), "git HEAD checksum should not be empty");
}

#[test]
fn compute_step_checksums_handles_git_ls_files() {
    let kronn_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let checksums = compute_step_checksums(&kronn_root, &["__GIT_LS_FILES__"]);
    assert!(
        !checksums.is_empty(),
        "git ls-files should produce at least one checksum entry"
    );
}

#[test]
fn is_kronn_generated_path_is_tight() {
    // F27 — only the DETECTED docs dir and .kronn state are generated
    // wholesale. Root agent files are NOT excluded here (they get normalized
    // hashing so user rules count), and a project whose docs live in `docs/`
    // keeps a real `ai/` source dir counted.
    for kronn in [
        "docs/AGENTS.md", "docs/checksums.json", "docs/tech-debt/TD-1.md",
        ".kronn.json", ".kronn.lock", ".kronn/state",
    ] {
        assert!(is_kronn_generated_path(kronn, "docs"), "{kronn} must be excluded");
    }
    for source in [
        "src/main.rs", "package.json", "Dockerfile", ".github/workflows/ci.yml",
        "docsite/index.html", "README.md", "src/docs.rs",
        "ai/model.py",                       // real source when docs dir = docs/
        "CLAUDE.md", "AGENTS.md",            // normalized elsewhere, not excluded
        ".github/copilot-instructions.md",
        "sub/checksums.json",                // someone else's file, not ours
    ] {
        assert!(!is_kronn_generated_path(source, "docs"), "{source} must stay counted");
    }
    // Legacy layout: ai/ IS the detected docs dir → excluded there.
    assert!(is_kronn_generated_path("ai/AGENTS.md", "ai"));
    assert!(!is_kronn_generated_path("docsx/file.md", "docs"), "prefix must not over-match");
}

#[test]
fn strip_kronn_regions_removes_managed_blocks_keeps_user_content() {
    let content = format!(
        "{}\nkronn pointer\n{}\n# My rules\nnever use lib X\n<!-- KRONN:FACTS — regenerated -->\nTest: cargo test\n<!-- END KRONN:FACTS -->\ntail rule\n",
        super::super::root_agent_files::KRONN_BLOCK_START,
        super::super::root_agent_files::KRONN_BLOCK_END,
    );
    let stripped = strip_kronn_regions(&content);
    assert!(stripped.contains("# My rules"));
    assert!(stripped.contains("never use lib X"));
    assert!(stripped.contains("tail rule"));
    assert!(!stripped.contains("kronn pointer"));
    assert!(!stripped.contains("Test: cargo test"));
    // A file that is ONLY Kronn regions normalizes to whitespace.
    let only_block = format!(
        "{}\nbody\n{}\n",
        super::super::root_agent_files::KRONN_BLOCK_START,
        super::super::root_agent_files::KRONN_BLOCK_END,
    );
    assert!(strip_kronn_regions(&only_block).trim().is_empty());
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = super::sync_cmd("git").arg("-C").arg(dir).args(args)
        .output().expect("git runs").status.success();
    assert!(ok, "git {args:?} failed");
}

#[test]
fn source_tree_fingerprint_ignores_kronn_output_commits_but_not_source() {
    // The F27 guarantee, end to end: a commit that only versions the audit's
    // own output must NOT move the fingerprint; a source change (file OR
    // user rules in a root agent file) must.
    let dir = temp_dir("f27_fingerprint");
    git(&dir, &["init", "-q"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    fs::write(dir.join("app.js"), "console.log(1)").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "src"]);
    let fp_initial = git_source_tree_fingerprint(&dir).expect("fingerprint");

    // Simulate committing an audit's output: docs/ tree, checksums, a
    // CLAUDE.md that holds ONLY the managed block (what Phase 1 creates on a
    // project that had none). Fingerprint must be unchanged (no self-drift,
    // not even on the very first audit commit).
    fs::create_dir_all(dir.join("docs/tech-debt")).unwrap();
    fs::write(dir.join("docs/AGENTS.md"), "# audited").unwrap();
    fs::write(dir.join("docs/checksums.json"), "{}").unwrap();
    fs::write(dir.join("CLAUDE.md"), format!(
        "{}\n> Kronn context pointer\n{}\n",
        super::super::root_agent_files::KRONN_BLOCK_START,
        super::super::root_agent_files::KRONN_BLOCK_END,
    )).unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "audit output"]);
    assert_eq!(git_source_tree_fingerprint(&dir).as_deref(), Some(fp_initial.as_str()),
        "committing docs/ + a block-only CLAUDE.md must NOT move the fingerprint (F27)");

    // USER rules added to CLAUDE.md are source — fingerprint must move
    // (worktree-read: no commit needed).
    fs::write(dir.join("CLAUDE.md"), format!(
        "{}\n> Kronn context pointer\n{}\n# House rules\nnever use lib X\n",
        super::super::root_agent_files::KRONN_BLOCK_START,
        super::super::root_agent_files::KRONN_BLOCK_END,
    )).unwrap();
    let fp_with_rules = git_source_tree_fingerprint(&dir).expect("fingerprint");
    assert_ne!(fp_with_rules, fp_initial,
        "user-authored rules in a root agent file must count as source");

    // UNCOMMITTED source change — the audit reads the worktree, so drift
    // must flag before any commit (Codex round 4: HEAD-only missed this).
    fs::write(dir.join("app.js"), "console.log(2)").unwrap();
    let fp_dirty = git_source_tree_fingerprint(&dir).expect("fingerprint");
    assert_ne!(fp_dirty, fp_with_rules,
        "an uncommitted tracked modification must move the fingerprint");

    // Committing that same content is a no-op on the print (records are
    // content-derived): stable across add+commit, no re-flag after commit.
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "real change"]);
    assert_eq!(git_source_tree_fingerprint(&dir).as_deref(), Some(fp_dirty.as_str()),
        "committing unchanged content must NOT move the fingerprint");

    // A brand-new untracked source file counts too (the next audit reads it).
    fs::write(dir.join("new-module.js"), "export {}").unwrap();
    assert_ne!(git_source_tree_fingerprint(&dir).as_deref(), Some(fp_dirty.as_str()),
        "an untracked (non-ignored) source file must move the fingerprint");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn source_tree_fingerprint_survives_hostile_paths_and_tracks_mode() {
    // Codex round 3 — byte-safety: a path containing a newline must neither
    // corrupt the record encoding nor be silently altered; a chmod +x (mode
    // change, same blob) is a real source change and must move the print.
    let dir = temp_dir("f27_bytes");
    git(&dir, &["init", "-q"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    fs::write(dir.join("app.sh"), "#!/bin/sh\necho hi").unwrap();
    let weird = dir.join("we\nird.txt"); // newline is legal on POSIX filesystems
    fs::write(&weird, "v1").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "base"]);
    let fp_base = git_source_tree_fingerprint(&dir).expect("fingerprint");

    // Content change inside the newline-named file must move the print
    // (proves the record for that path is tracked, not mangled/dropped).
    fs::write(&weird, "v2").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "weird change"]);
    let fp_weird = git_source_tree_fingerprint(&dir).expect("fingerprint");
    assert_ne!(fp_weird, fp_base, "a newline-named file's change must be tracked");

    // chmod +x: same blob, different mode — must move the print too.
    let mut perms = fs::metadata(dir.join("app.sh")).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    fs::set_permissions(dir.join("app.sh"), perms).unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "exec bit"]);
    let fp_mode = git_source_tree_fingerprint(&dir).expect("fingerprint");
    assert_ne!(fp_mode, fp_weird, "a mode flip must move the fingerprint");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(target_os = "linux")]
#[test]
fn source_tree_fingerprint_keeps_non_utf8_paths() {
    // Non-UTF-8 file names exist on Linux filesystems (APFS rejects them, so
    // this can only run there). Such a path can't match any ASCII exclusion —
    // it must stay counted as source, byte-exact.
    use std::os::unix::ffi::OsStrExt;
    let dir = temp_dir("f27_non_utf8");
    git(&dir, &["init", "-q"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    let name = std::ffi::OsStr::from_bytes(b"caf\xe9.txt"); // latin-1 é
    fs::write(dir.join(name), "v1").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "base"]);
    let fp1 = git_source_tree_fingerprint(&dir).expect("fingerprint");
    fs::write(dir.join(name), "v2").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "change"]);
    assert_ne!(git_source_tree_fingerprint(&dir).unwrap(), fp1,
        "a non-UTF-8 path's change must be tracked");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compute_step_checksums_handles_source_tree_sentinel() {
    let kronn_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let checksums = compute_step_checksums(&kronn_root, &["__GIT_SOURCE_TREE__"]);
    let fp = checksums.get("__GIT_SOURCE_TREE__").expect("source-tree key present");
    assert_eq!(fp.len(), 64, "SHA-256 hex is 64 chars");
    assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn stable_fingerprint_rejects_deterministic_between_read_mutation() {
    let dir = temp_dir("f27_quiescence_mutation");
    git(&dir, &["init", "-q"]);
    fs::write(dir.join("src.rs"), "v1").unwrap();

    let result = stable_source_tree_fingerprint_test_hook(&dir, || {
        fs::write(dir.join("src.rs"), "v2").unwrap();
    });

    assert!(result.unwrap_err().contains("quiescence"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mapping_reuses_frozen_source_tree_without_rescanning() {
    let dir = temp_dir("f27_snapshot_reuse");
    git(&dir, &["init", "-q"]);
    fs::write(dir.join("src.rs"), "before").unwrap();
    let frozen = git_source_tree_fingerprint(&dir).unwrap();
    fs::write(dir.join("src.rs"), "after").unwrap();
    let current = git_source_tree_fingerprint(&dir).unwrap();
    assert_ne!(current, frozen);

    let checksums = compute_step_checksums_from_snapshot(
        &dir,
        &["__GIT_SOURCE_TREE__"],
        Some(&frozen),
    );

    assert_eq!(checksums.get("__GIT_SOURCE_TREE__"), Some(&frozen));
    assert_ne!(checksums.get("__GIT_SOURCE_TREE__"), Some(&current));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn post_publish_mutation_restores_previous_baseline_byte_exact() {
    let dir = temp_dir("f27_publish_rollback");
    git(&dir, &["init", "-q"]);
    fs::write(dir.join("src.rs"), "stable").unwrap();
    let frozen = git_source_tree_fingerprint(&dir).unwrap();
    let old = vec![ChecksumMapping {
        ai_file: "docs/old.md".into(),
        audit_step: 1,
        sources: vec!["src.rs".into()],
        checksums: BTreeMap::from([("src.rs".into(), "old".into())]),
    }];
    write_checksums_file(&dir, &old).unwrap();
    let path = dir.join("docs/checksums.json");
    let previous = fs::read(&path).unwrap();
    let replacement = vec![ChecksumMapping {
        ai_file: "docs/new.md".into(),
        audit_step: 2,
        sources: vec!["__GIT_SOURCE_TREE__".into()],
        checksums: BTreeMap::from([("__GIT_SOURCE_TREE__".into(), frozen.clone())]),
    }];

    let error = write_checksums_file_fail_closed_test_hook(
        &dir,
        &replacement,
        Some(&frozen),
        || fs::write(dir.join("src.rs"), "mutated-after-publish").unwrap(),
    ).unwrap_err();

    assert!(error.contains("previous baseline restored"));
    assert_eq!(fs::read(&path).unwrap(), previous);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn post_publish_mutation_never_clobbers_a_concurrent_baseline_writer() {
    let dir = temp_dir("f27_publish_concurrent_writer");
    git(&dir, &["init", "-q"]);
    fs::write(dir.join("src.rs"), "stable").unwrap();
    let frozen = git_source_tree_fingerprint(&dir).unwrap();
    let replacement = vec![ChecksumMapping {
        ai_file: "docs/new.md".into(),
        audit_step: 2,
        sources: vec!["__GIT_SOURCE_TREE__".into()],
        checksums: BTreeMap::from([("__GIT_SOURCE_TREE__".into(), frozen.clone())]),
    }];
    let path = dir.join("docs/checksums.json");

    let error = write_checksums_file_fail_closed_test_hook(
        &dir,
        &replacement,
        Some(&frozen),
        || {
            fs::write(dir.join("src.rs"), "mutated-after-publish").unwrap();
            fs::write(&path, "concurrent-writer").unwrap();
        },
    ).unwrap_err();

    assert!(error.contains("rollback failed"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "concurrent-writer");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn empty_sources_returns_empty_checksums() {
    let dir = temp_dir("empty_sources");
    fs::create_dir_all(&dir).unwrap();

    let checksums = compute_step_checksums(&dir, &[]);
    assert!(checksums.is_empty(), "empty sources should return empty BTreeMap");

    let _ = fs::remove_dir_all(&dir);
}

// ── sha256_of_bytes — pin the hex contract ──────────────────────────────

#[test]
fn sha256_of_bytes_known_vector_empty_string() {
    // RFC test vector: SHA-256(empty) = e3b0c44298fc1c149afbf4c8996fb924
    //                                   27ae41e4649b934ca495991b7852b855
    assert_eq!(
        sha256_of_bytes(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
}

#[test]
fn sha256_of_bytes_known_vector_abc() {
    // Standard SHA-256(b"abc") test vector.
    assert_eq!(
        sha256_of_bytes(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
}

#[test]
fn sha256_of_bytes_unicode_does_not_panic() {
    // Hashing UTF-8 bytes of a non-ASCII string — verifies the byte-level
    // hash treats text as bytes, not code points.
    let hex = sha256_of_bytes("éèàç中".as_bytes());
    assert_eq!(hex.len(), 64, "SHA-256 hex is always 64 chars");
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "all hex chars must be valid");
}

#[test]
fn sha256_of_bytes_one_byte_diff_changes_hash() {
    // Avalanche : flipping one bit must flip ~half the hash bits.
    let a = sha256_of_bytes(b"hello world");
    let b = sha256_of_bytes(b"hello worlD");
    assert_ne!(a, b);
    // At least 20 hex chars must differ — looser than the strict ~32 for
    // avalanche but safely above the "single-char change" floor.
    let diff = a.chars().zip(b.chars()).filter(|(x, y)| x != y).count();
    assert!(diff > 20, "avalanche should change many hex chars, got diff={diff}");
}

// ── matches_simple_glob — pin pattern matching contract ─────────────────

#[test]
fn glob_star_matches_everything() {
    assert!(matches_simple_glob("*", "any"));
    assert!(matches_simple_glob("*", ""));
    assert!(matches_simple_glob("*", "a/b/c.rs"));
}

#[test]
fn glob_no_wildcard_is_exact_match() {
    assert!(matches_simple_glob("foo.rs", "foo.rs"));
    assert!(!matches_simple_glob("foo.rs", "bar.rs"));
    assert!(!matches_simple_glob("foo.rs", "foo.rs.bak"));
    assert!(!matches_simple_glob("foo.rs", "prefix-foo.rs"));
}

#[test]
fn glob_single_star_in_middle() {
    // `*.md` — suffix match.
    assert!(matches_simple_glob("*.md", "README.md"));
    assert!(matches_simple_glob("*.md", ".md"));
    assert!(matches_simple_glob("*.md", "a.md"));
    assert!(!matches_simple_glob("*.md", "README.txt"));
    assert!(!matches_simple_glob("*.md", "README.md.bak"));
}

#[test]
fn glob_prefix_and_suffix() {
    // `TD-*.md` — prefix + suffix.
    assert!(matches_simple_glob("TD-*.md", "TD-001.md"));
    assert!(matches_simple_glob("TD-*.md", "TD-.md"), "zero-char middle still matches");
    assert!(!matches_simple_glob("TD-*.md", "001.md"));
    assert!(!matches_simple_glob("TD-*.md", "TD-001"));
}

#[test]
fn glob_too_short_name_for_prefix_plus_suffix() {
    // Pattern requires at least prefix + suffix length — name shorter than
    // that must not match (even if endswith is technically true).
    assert!(!matches_simple_glob("abc*xyz", "ab"));
    assert!(!matches_simple_glob("abc*xyz", "yz"));
    // Exactly prefix + suffix : matches (the `*` matched zero chars).
    assert!(matches_simple_glob("abc*xyz", "abcxyz"));
}

#[test]
fn glob_multiple_stars_falls_back_to_exact_match() {
    // The current impl only handles 0 or 1 `*`. Multi-star patterns fall
    // back to exact-match — pinning this behaviour so the fallback path
    // isn't silently broken.
    assert!(matches_simple_glob("a*b*c", "a*b*c"), "fallback is exact match on multi-star");
    assert!(!matches_simple_glob("a*b*c", "aXbYc"));
}

#[cfg(unix)]
#[test]
fn failed_checksums_write_preserves_the_previous_manifest() {
    // Codex lot-2 #4 — a truncating fs::write could corrupt the manifest:
    // read_checksums_file then returned None and the whole drift scope
    // silently disappeared. With the atomic sibling-temp+rename, a write
    // failure leaves the previous valid manifest byte-intact.
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("atomic_manifest");
    let docs = dir.join("docs");
    fs::create_dir_all(&docs).unwrap();
    let mapping = ChecksumMapping {
        ai_file: "docs/repo-map.md".into(),
        audit_step: 3,
        sources: vec!["src/main.rs".into()],
        checksums: BTreeMap::new(),
    };
    write_checksums_file(&dir, std::slice::from_ref(&mapping)).unwrap();
    let before = fs::read(docs.join("checksums.json")).unwrap();

    // Make docs/ read-only: the sibling temp file cannot be created.
    let mut perms = fs::metadata(&docs).unwrap().permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&docs, perms).unwrap();
    let err = write_checksums_file(&dir, &[mapping]).unwrap_err();
    assert!(err.contains("temp") || err.contains("denied") || err.contains("write"), "{err}");

    let mut restore = fs::metadata(&docs).unwrap().permissions();
    restore.set_mode(0o755);
    fs::set_permissions(&docs, restore).unwrap();
    assert_eq!(fs::read(docs.join("checksums.json")).unwrap(), before,
        "the previous valid manifest must survive byte-for-byte");
    assert!(read_checksums_file(&dir).is_some(), "and still parse");
    let _ = fs::remove_dir_all(&dir);
}
