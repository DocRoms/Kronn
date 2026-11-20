use super::*;

use serial_test::serial;

// ─── KT-263: configured AI doc folder ───────────────────────────────────

#[test]
fn a_project_keeping_its_doc_in_dot_ai_is_detected_and_audited() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(root.join(".ai")).unwrap();
    std::fs::create_dir_all(root.join("ai")).unwrap();
    std::fs::write(root.join("ai/index.md"), "# legacy\n").unwrap();
    std::fs::write(root.join(".ai/AGENTS.md"), "# Project\n\n<!-- TODO x -->\n").unwrap();
    let path = root.to_string_lossy().into_owned();

    // Unconfigured: the legacy layout wins and `.ai/` is invisible.
    assert_eq!(detect_docs_dir(&root), root.join("ai"));
    assert!(needs_docs_migration(&root));

    crate::core::docs_location::set_project_override(&path, Some(".ai"));
    let dir = detect_docs_dir(&root);
    let entry = detect_docs_entry(&root);
    let migration = needs_docs_migration(&root);
    let todos = count_ai_todos(&path);
    let before_audit = detect_audit_status(&path);
    let recorded = crate::core::template_manifest::record_audit(&root, "full");
    let after_audit = detect_audit_status(&path);
    crate::core::docs_location::set_project_override(&path, None);

    assert_eq!(dir, root.join(".ai"));
    assert_eq!(entry, root.join(".ai/AGENTS.md"));
    assert!(
        !migration,
        "a configured folder is never offered a migration"
    );
    assert_eq!(todos, 1);
    assert_eq!(
        before_audit,
        crate::models::AiAuditStatus::TemplateInstalled
    );
    recorded.unwrap();
    assert!(root.join(".ai/.kronn.json").is_file());
    assert!(!root.join("ai/.kronn.json").exists());
    assert_eq!(after_audit, crate::models::AiAuditStatus::Audited);
}

#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn a_symlinked_override_reads_nothing_and_writes_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let outside = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(outside.path().join("AGENTS.md"), "# outside\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join(".ai")).unwrap();
    let path = root.to_string_lossy().into_owned();
    crate::core::docs_location::set_project_override(&path, Some(".ai"));
    let status = detect_audit_status(&path);
    let write = crate::core::template_manifest::record_audit(&root, "full");
    crate::core::docs_location::set_project_override(&path, None);
    assert_eq!(status, crate::models::AiAuditStatus::NoTemplate);
    assert!(write.unwrap_err().contains("symlink"));
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
}

// ─── scan_kronn_markers (0.8.3 Feasibility-Gated Implementation) ─────────

#[test]
fn scan_kronn_markers_finds_all_three_variants() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    std::fs::write(
        tmp.path().join("src/Service/BrandContext.php"),
        "<?php\n\
             // KRONN-ASSUMED(brand-context-impl): EventListener over CompilerPass\n\
             final class BrandContext {}\n\
             // KRONN-MOCKED(adobe-dtm-an): env var KRONN_ADOBE_DTM_AN_URL_PROD\n\
             public function getDtm(): string { return ''; }\n\
             // KRONN-TODO(adobe-visitor-an): waiting on Data team\n",
    )
    .unwrap();

    let markers = scan_kronn_markers(tmp.path());
    assert_eq!(markers.len(), 3, "expected 3 markers, got {markers:?}");
    let kinds: Vec<&str> = markers.iter().map(|m| m.kind.as_str()).collect();
    assert!(kinds.contains(&"ASSUMED"));
    assert!(kinds.contains(&"MOCKED"));
    assert!(kinds.contains(&"TODO"));
    // decision_id preserved verbatim.
    assert!(markers
        .iter()
        .any(|m| m.decision_id == "brand-context-impl"));
    assert!(markers.iter().any(|m| m.decision_id == "adobe-dtm-an"));
    assert!(markers.iter().any(|m| m.decision_id == "adobe-visitor-an"));
}

#[test]
fn scan_kronn_markers_skips_heavy_dirs() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Marker INSIDE node_modules — must be ignored.
    std::fs::create_dir_all(tmp.path().join("node_modules/foo")).unwrap();
    std::fs::write(
        tmp.path().join("node_modules/foo/index.js"),
        "// KRONN-ASSUMED(x): should not be found\n",
    )
    .unwrap();
    // Marker in src/ — must be found.
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/app.js"),
        "// KRONN-MOCKED(y): real marker\n",
    )
    .unwrap();

    let markers = scan_kronn_markers(tmp.path());
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].decision_id, "y");
}

#[test]
fn scan_kronn_markers_skips_binary_extensions() {
    let tmp = tempfile::TempDir::new().unwrap();
    // A `.png` file with the marker string in it (pretend binary)
    // — must not be read.
    std::fs::write(
        tmp.path().join("logo.png"),
        "KRONN-TODO(should-not-match): binary file\n",
    )
    .unwrap();
    let markers = scan_kronn_markers(tmp.path());
    assert!(markers.is_empty(), "binary file must be skipped");
}

#[test]
fn scan_kronn_markers_captures_line_number_and_note() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("a.rs"),
        "// line 1\n\
             fn main() {}\n\
             // KRONN-ASSUMED(decision-id): the rationale text here\n",
    )
    .unwrap();
    let markers = scan_kronn_markers(tmp.path());
    assert_eq!(markers.len(), 1);
    let m = &markers[0];
    assert_eq!(m.line, 3);
    assert_eq!(m.decision_id, "decision-id");
    assert_eq!(m.note, "the rationale text here");
}

#[test]
fn scan_kronn_markers_empty_when_no_markers() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.rs"), "fn main() {}\n").unwrap();
    assert!(scan_kronn_markers(tmp.path()).is_empty());
}
