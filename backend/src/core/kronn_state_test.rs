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
