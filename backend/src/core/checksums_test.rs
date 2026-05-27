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
