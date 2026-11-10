use super::*;
use crate::core::audit_validation::td_decision;
use crate::core::cross_audit::{CrossFindingRecord, Outcome};

fn target(decision: Option<HumanDecision>) -> DisputeTarget {
    let mut record: CrossFindingRecord = serde_json::from_value(serde_json::json!({
        "group_id": "F1", "td_id": "TD-20261011-sql", "title": "SQL", "category": "security",
        "severity": "High", "citation": {"file": "src/db.rs", "line": 3, "quote": "SELECT"},
        "impact": "", "remediation": "", "author": "ClaudeCode", "detected_by": ["ClaudeCode"],
        "detection_count": 1, "agents_total": 2, "members": [], "challenger": "Codex",
        "challenge": null, "reply": null, "outcome": "disputed", "question_key": "cross-audit.r.TD-20261011-sql"
    }))
    .unwrap();
    record.human_decision = decision;
    assert_eq!(record.outcome, Outcome::Disputed);
    DisputeTarget {
        audit_run_id: "r".into(),
        step_index: 1,
        file_label: "docs/inconsistencies-security.md".into(),
        project_id: "p".into(),
        project_path: "/repo".into(),
        record,
    }
}

const SHEET: &str =
    "---\nid: TD-20261011-sql\n---\n# TD-20261011-sql: SQL\n\n- **Status**: Verified in source\n";

#[test]
fn a_dismissal_rejects_the_sheet_and_records_the_card() {
    let out = decided_sheet(SHEET, &target(Some(HumanDecision::Dismiss))).unwrap();
    assert_eq!(td_decision(&out), Some(TdDecision::Rejected));
    assert!(out.contains("reviewer: Kronn cross-agent dispute card"));
    assert!(out.contains("- **Verdict**: disputed — dismissed by a human"));
}

#[test]
fn a_keep_leaves_the_status_and_records_the_card() {
    let out = decided_sheet(SHEET, &target(Some(HumanDecision::Keep))).unwrap();
    assert_eq!(td_decision(&out), None);
    assert!(out.contains("- **Status**: Verified in source"));
    assert!(out.contains("note: \"Kept through a cross-agent dispute card.\""));
    assert!(out.contains("- **Verdict**: disputed — kept by a human"));
}

#[test]
fn a_refusal_only_refreshes_the_verdict() {
    let out = decided_sheet(SHEET, &target(Some(HumanDecision::Declined))).unwrap();
    assert_eq!(td_decision(&out), None);
    assert!(!out.contains("audit_history"));
    assert!(out.contains("- **Verdict**: disputed — left undecided by a human"));
}

#[cfg(unix)] // swaps a folder for a symlink (std::os::unix)
#[test]
fn a_tech_debt_folder_swapped_after_the_walk_never_receives_the_decided_sheet() {
    use crate::core::fs_guard::swap_probe;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    let folder = root.join("docs/tech-debt");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("TD-20261011-sql.md"), SHEET).unwrap();
    std::fs::write(root.join("docs/inconsistencies-security.md"), "# Index\n").unwrap();
    let external = tmp.path().join("external");
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join("TD-20261011-sql.md"), SHEET).unwrap();
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE audit_cross_findings (audit_run_id TEXT, step_index INTEGER, \
         group_id TEXT, record_json TEXT);",
    )
    .unwrap();
    let mut disputed = target(None);
    disputed.project_path = root.to_string_lossy().into_owned();
    swap_probe::arm(&folder, &external);
    let _ = apply(&conn, &mut disputed, HumanDecision::Dismiss);
    assert!(swap_probe::fired(), "the swap really happened");
    assert_eq!(
        std::fs::read_to_string(external.join("TD-20261011-sql.md")).unwrap(),
        SHEET,
        "the outside sheet is untouched"
    );
}
