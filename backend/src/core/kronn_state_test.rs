use crate::core::kronn_state::*;
use std::path::PathBuf;

fn fresh_tmp(name: &str) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("kronn-state-test-{name}-{}", uuid::Uuid::new_v4()));
    let _ = std::fs::create_dir_all(tmp.join("docs"));
    tmp
}

fn cleanup(p: &PathBuf) {
    let _ = std::fs::remove_dir_all(p);
}

#[test]
fn read_missing_returns_none() {
    let tmp = fresh_tmp("missing");
    assert!(read(&tmp).is_none());
    cleanup(&tmp);
}

#[test]
fn read_malformed_returns_none() {
    let tmp = fresh_tmp("malformed");
    std::fs::write(tmp.join("docs/.kronn.json"), "{ not json").unwrap();
    assert!(read(&tmp).is_none());
    cleanup(&tmp);
}

#[test]
fn record_audit_creates_file_with_readme() {
    let tmp = fresh_tmp("record-creates");
    record_audit(&tmp, "full").unwrap();
    let state = read(&tmp).expect("file should exist");
    assert_eq!(state.audits.len(), 1);
    assert_eq!(state.audits[0].audit_type, "full");
    assert!(state.audits[0].kronn_version.chars().any(|c| c.is_ascii_digit()));
    assert!(state.readme.contains("Kronn"), "readme must mention Kronn");
    cleanup(&tmp);
}

#[test]
fn record_audit_appends_to_existing() {
    let tmp = fresh_tmp("record-appends");
    record_audit(&tmp, "full").unwrap();
    record_audit(&tmp, "partial").unwrap();
    let state = read(&tmp).unwrap();
    assert_eq!(state.audits.len(), 2);
    assert_eq!(state.audits[1].audit_type, "partial");
    cleanup(&tmp);
}

#[test]
fn mark_validated_preserves_original_date() {
    let tmp = fresh_tmp("validated-preserve");
    let mut state = KronnState {
        validated_at: Some("2020-01-01".into()),
        ..Default::default()
    };
    write(&tmp, &mut state).unwrap();

    mark_validated(&tmp).unwrap();
    let reread = read(&tmp).unwrap();
    assert_eq!(reread.validated_at.as_deref(), Some("2020-01-01"));
    cleanup(&tmp);
}

#[test]
fn mark_bootstrapped_sets_date_when_missing() {
    let tmp = fresh_tmp("bootstrap-fresh");
    mark_bootstrapped(&tmp).unwrap();
    let state = read(&tmp).unwrap();
    assert!(state.bootstrapped_at.is_some());
    cleanup(&tmp);
}

#[test]
fn write_rewrites_readme_even_if_caller_blanks_it() {
    let tmp = fresh_tmp("readme-rewrite");
    let mut state = KronnState {
        readme: String::new(),
        ..Default::default()
    };
    write(&tmp, &mut state).unwrap();
    let reread = read(&tmp).unwrap();
    assert!(reread.readme.contains("Do not delete"),
        "write() must re-inject the canonical readme, got: {:?}", reread.readme);
    cleanup(&tmp);
}

#[test]
fn write_resolves_to_legacy_ai_dir_when_only_ai_exists() {
    let tmp = std::env::temp_dir().join(format!("kronn-state-test-legacy-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join("ai")).unwrap();
    record_audit(&tmp, "full").unwrap();
    assert!(tmp.join("ai/.kronn.json").is_file(),
        "state file should land under ai/ when that's the only docs dir");
    let _ = std::fs::remove_dir_all(&tmp);
}

// ─── 0.8.6 (#28) — backfill_from_legacy_state ──────────────────────────

/// Seed the checksums-only legacy state : `docs/checksums.json` present,
/// no marker. Expected backfill : 1 audit entry, no `validated_at` /
/// `bootstrapped_at`.
#[test]
fn backfill_seeds_audit_entry_from_checksums_alone() {
    let tmp = fresh_tmp("backfill-checksums");
    // Create a minimal checksums.json (the scanner's reader is tolerant).
    // Minimal valid checksums file (the structural reader requires
    // `audited_at` + `mappings[]` per ChecksumsFile).
    std::fs::write(
        tmp.join("docs/checksums.json"),
        r#"{"audited_at": "2026-01-01", "mappings": []}"#,
    ).unwrap();

    let did = backfill_from_legacy_state(&tmp).unwrap();
    assert!(did, "backfill should fire when checksums.json exists");

    let state = read(&tmp).expect("file should exist after backfill");
    assert_eq!(state.audits.len(), 1);
    assert_eq!(state.audits[0].audit_type, "legacy");
    assert_eq!(state.audits[0].kronn_version, "legacy");
    assert!(state.validated_at.is_none());
    assert!(state.bootstrapped_at.is_none());
    cleanup(&tmp);
}

#[test]
fn backfill_seeds_validated_when_marker_present() {
    let tmp = fresh_tmp("backfill-validated");
    std::fs::write(
        tmp.join("docs/AGENTS.md"),
        "# my project\n<!-- KRONN:VALIDATED -->\n",
    ).unwrap();

    let did = backfill_from_legacy_state(&tmp).unwrap();
    assert!(did);
    let state = read(&tmp).unwrap();
    assert_eq!(state.audits.len(), 1, "audit entry seeded as side-effect");
    assert!(state.validated_at.is_some(), "validated_at must be set");
    assert!(state.bootstrapped_at.is_none());
    cleanup(&tmp);
}

#[test]
fn backfill_seeds_bootstrapped_when_marker_present() {
    let tmp = fresh_tmp("backfill-bootstrapped");
    std::fs::write(
        tmp.join("docs/AGENTS.md"),
        "# bootstrap\n<!-- KRONN:BOOTSTRAPPED -->\n",
    ).unwrap();

    let did = backfill_from_legacy_state(&tmp).unwrap();
    assert!(did);
    let state = read(&tmp).unwrap();
    assert!(state.bootstrapped_at.is_some());
    cleanup(&tmp);
}

#[test]
fn backfill_combines_validated_and_bootstrapped_markers() {
    let tmp = fresh_tmp("backfill-both");
    std::fs::write(
        tmp.join("docs/AGENTS.md"),
        "# project\n<!-- KRONN:BOOTSTRAPPED -->\n<!-- KRONN:VALIDATED -->\n",
    ).unwrap();
    // Minimal valid checksums file (the structural reader requires
    // `audited_at` + `mappings[]` per ChecksumsFile).
    std::fs::write(
        tmp.join("docs/checksums.json"),
        r#"{"audited_at": "2026-01-01", "mappings": []}"#,
    ).unwrap();

    backfill_from_legacy_state(&tmp).unwrap();
    let state = read(&tmp).unwrap();
    assert_eq!(state.audits.len(), 1);
    assert!(state.validated_at.is_some());
    assert!(state.bootstrapped_at.is_some());
    cleanup(&tmp);
}

#[test]
fn backfill_skips_when_kronn_json_already_exists() {
    // Idempotency : a project that ALREADY has .kronn.json must not be
    // touched, no matter what legacy state is also present.
    let tmp = fresh_tmp("backfill-skip-existing");
    record_audit(&tmp, "full").unwrap();
    // Drop a checksums.json that WOULD have triggered backfill on a
    // fresh project — must still be ignored because state already exists.
    // Minimal valid checksums file (the structural reader requires
    // `audited_at` + `mappings[]` per ChecksumsFile).
    std::fs::write(
        tmp.join("docs/checksums.json"),
        r#"{"audited_at": "2026-01-01", "mappings": []}"#,
    ).unwrap();

    let did = backfill_from_legacy_state(&tmp).unwrap();
    assert!(!did, "backfill must skip when .kronn.json already exists");

    let state = read(&tmp).unwrap();
    assert_eq!(state.audits.len(), 1, "audits not overwritten");
    assert_eq!(state.audits[0].audit_type, "full", "must keep the 'full' from record_audit, not 'legacy'");
    cleanup(&tmp);
}

#[test]
fn backfill_skips_when_no_legacy_signal() {
    // A pristine project with neither checksums nor markers → backfill
    // returns false and no file is written. Avoids polluting fresh
    // projects with a stale "legacy" audit row.
    let tmp = fresh_tmp("backfill-skip-pristine");
    // No checksums.json, no AGENTS.md.
    let did = backfill_from_legacy_state(&tmp).unwrap();
    assert!(!did, "backfill must skip when no legacy signal present");
    assert!(read(&tmp).is_none(), "no .kronn.json created");
    cleanup(&tmp);
}

#[test]
fn backfill_is_idempotent_on_repeated_calls() {
    // Even with the same legacy state, calling backfill twice must NOT
    // double-write the audits array. Second call short-circuits via the
    // "kronn.json already exists" branch.
    let tmp = fresh_tmp("backfill-idempotent");
    // Minimal valid checksums file (the structural reader requires
    // `audited_at` + `mappings[]` per ChecksumsFile).
    std::fs::write(
        tmp.join("docs/checksums.json"),
        r#"{"audited_at": "2026-01-01", "mappings": []}"#,
    ).unwrap();

    let first = backfill_from_legacy_state(&tmp).unwrap();
    let second = backfill_from_legacy_state(&tmp).unwrap();

    assert!(first);
    assert!(!second);
    let state = read(&tmp).unwrap();
    assert_eq!(state.audits.len(), 1);
    cleanup(&tmp);
}
