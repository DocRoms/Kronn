use super::*;

fn sheet(status: &str, area: &str, title: &str) -> String {
    format!(
        "---\nid: x\ndescription: {title}\nmetadata:\n  type: tech-debt\n---\n# {title}\n\n- **Area**: `{area}`\n- **Severity**: High\n- **Status**: {status}\n- **Problem**: {title}\n"
    )
}

fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    std::fs::create_dir_all(docs.join("tech-debt")).unwrap();
    (dir, docs)
}

fn write(docs: &Path, id: &str, content: &str) {
    std::fs::write(docs.join("tech-debt").join(format!("{id}.md")), content).unwrap();
}

fn read(docs: &Path, id: &str) -> String {
    std::fs::read_to_string(docs.join("tech-debt").join(format!("{id}.md"))).unwrap()
}

#[test]
fn a_decision_the_run_reverted_is_put_back() {
    let (_dir, docs) = setup();
    write(
        &docs,
        "TD-1-rej",
        &sheet("Rejected", "src/a.rs", "Unsafe call"),
    );
    write(
        &docs,
        "TD-2-conf",
        &sheet("Confirmed by user", "src/b.rs", "Leak"),
    );
    write(
        &docs,
        "TD-3-open",
        &sheet("Verified in source", "src/c.rs", "Open"),
    );
    let snap = snapshot(&docs);
    // The agent rewrote the statuses.
    write(
        &docs,
        "TD-1-rej",
        &sheet("Verified in source", "src/a.rs", "Unsafe call"),
    );
    write(&docs, "TD-2-conf", &sheet("Inferred", "src/b.rs", "Leak"));
    write(&docs, "TD-3-open", &sheet("Inferred", "src/c.rs", "Open"));

    let outcome = restore(docs.parent().unwrap(), &docs, &snap).unwrap();
    assert_eq!(outcome.reverted, vec!["TD-1-rej", "TD-2-conf"]);
    assert_eq!(
        td_decision(&read(&docs, "TD-1-rej")),
        Some(TdDecision::Rejected)
    );
    assert_eq!(
        td_decision(&read(&docs, "TD-2-conf")),
        Some(TdDecision::Confirmed)
    );
    assert!(read(&docs, "TD-1-rej").contains("reviewer: Kronn decision guard"));
    // An undecided sheet is the audit's to rewrite.
    assert!(read(&docs, "TD-3-open").contains("**Status**: Inferred"));
    // Nothing left to do on a second pass.
    assert!(restore(docs.parent().unwrap(), &docs, &snap)
        .unwrap()
        .is_empty());
}

#[test]
fn a_settled_sheet_the_run_deleted_is_written_back_a_confirmed_one_is_not() {
    let (_dir, docs) = setup();
    let rejected = sheet("Rejected", "src/a.rs", "Unsafe call");
    write(&docs, "TD-1-rej", &rejected);
    write(
        &docs,
        "TD-2-adr",
        &sheet("Accepted decision", "src/b.rs", "Choice"),
    );
    write(
        &docs,
        "TD-3-conf",
        &sheet("Confirmed by user", "src/c.rs", "Leak"),
    );
    write(&docs, "TD-4-def", &sheet("Deferred", "src/d.rs", "Later"));
    let snap = snapshot(&docs);
    for id in ["TD-1-rej", "TD-2-adr", "TD-3-conf", "TD-4-def"] {
        std::fs::remove_file(docs.join("tech-debt").join(format!("{id}.md"))).unwrap();
    }
    let outcome = restore(docs.parent().unwrap(), &docs, &snap).unwrap();
    assert_eq!(outcome.restored, vec!["TD-1-rej", "TD-2-adr"]);
    assert_eq!(read(&docs, "TD-1-rej"), rejected);
    assert!(!docs.join("tech-debt/TD-3-conf.md").exists());
    assert!(!docs.join("tech-debt/TD-4-def.md").exists());
}

#[test]
fn a_new_sheet_repeating_a_settled_one_is_flagged_once() {
    let (_dir, docs) = setup();
    write(
        &docs,
        "TD-1-rej",
        &sheet("Rejected", "src/a.rs", "Unsafe call in parser"),
    );
    write(
        &docs,
        "TD-2-adr",
        &sheet("Accepted decision", "src/b.rs", "Global state"),
    );
    let snap = snapshot(&docs);
    // Same file and problem (line and case differ), a different problem, a different file.
    write(
        &docs,
        "TD-9-dup",
        &sheet(
            "Verified in source",
            "src/a.rs:42",
            "Unsafe call in Parser.",
        ),
    );
    write(
        &docs,
        "TD-9-other",
        &sheet("Verified in source", "src/a.rs", "Other problem"),
    );
    write(
        &docs,
        "TD-9-elsewhere",
        &sheet("Verified in source", "src/z.rs", "Global state"),
    );
    let outcome = restore(docs.parent().unwrap(), &docs, &snap).unwrap();
    assert_eq!(
        outcome.settled_duplicates,
        vec![("TD-9-dup".to_string(), "TD-1-rej".to_string())]
    );
    let flagged = read(&docs, "TD-9-dup");
    assert!(flagged.contains("- **Matches a settled sheet**: `TD-1-rej` (rejected by a human)"));
    assert_eq!(
        td_decision(&flagged),
        None,
        "a flag never decides for the human"
    );
    restore(docs.parent().unwrap(), &docs, &snap).unwrap();
    assert_eq!(read(&docs, "TD-9-dup"), flagged, "flagged once");
    assert!(!read(&docs, "TD-9-other").contains(DUPLICATE_FIELD));
    assert!(!read(&docs, "TD-9-elsewhere").contains(DUPLICATE_FIELD));
}

#[test]
fn settled_match_needs_the_same_file_and_problem() {
    let sheets = vec![
        parse_sheet("TD-1", &sheet("Rejected", "src/a.rs", "Unsafe call")),
        parse_sheet("TD-2", &sheet("Confirmed by user", "src/b.rs", "Leak")),
    ];
    assert_eq!(
        settled_match("./src/a.rs:10-12", "unsafe  CALL", &sheets).map(|s| s.id.as_str()),
        Some("TD-1")
    );
    assert!(
        settled_match("src/b.rs", "Leak", &sheets).is_none(),
        "confirmed is still debt"
    );
    assert!(settled_match("src/a.rs", "Unsafe call twice", &sheets).is_none());
    assert!(settled_match("Unknown", "Unsafe call", &sheets).is_none());
}

#[test]
fn incomplete_findings_do_not_match_an_equally_incomplete_settled_sheet() {
    for (area, title) in [
        ("Unknown", "Unsafe call"),
        ("", "Unsafe call"),
        ("src/a.rs", ""),
    ] {
        let settled = parse_sheet("TD-1", &sheet("Rejected", area, title));
        assert!(
            settled_match(area, title, &[settled]).is_none(),
            "an incomplete area/title must not flag an unrelated finding: {area:?}, {title:?}"
        );
    }
}

#[test]
fn restoring_a_sheet_without_front_matter_preserves_the_agents_other_edits() {
    let (_dir, docs) = setup();
    let original =
        "# Unsafe call\n\n- **Area**: `src/a.rs`\n- **Status**: Rejected\n\nOriginal evidence.\n";
    write(&docs, "TD-1", original);
    let snap = snapshot(&docs);
    write(
        &docs,
        "TD-1",
        &original
            .replace("Rejected", "Verified in source")
            .replace("Original evidence.", "Updated evidence from the run."),
    );

    let outcome = restore(docs.parent().unwrap(), &docs, &snap).unwrap();
    assert_eq!(outcome.reverted, vec!["TD-1"]);
    let restored = read(&docs, "TD-1");
    assert_eq!(td_decision(&restored), Some(TdDecision::Rejected));
    assert!(restored.contains("Updated evidence from the run."));
    assert!(!restored.contains("reviewer:"));
    assert!(restore(docs.parent().unwrap(), &docs, &snap)
        .unwrap()
        .is_empty());
}

/// KT-1595: `restore` runs seconds after the agent's last write, with a
/// process it left behind possibly alive. A `tech-debt/` folder swapped for a
/// link to another project's sheets between the walk and the write never takes
/// a restored, reverted or flagged sheet out of the project.
#[cfg(unix)] // swaps a folder for a symlink (std::os::unix)
mod swapped_folder {
    use super::*;
    use crate::core::fs_guard::swap_probe;

    fn assert_nothing_outside(external: &Path) {
        assert_eq!(std::fs::read_dir(external).unwrap().count(), 0);
    }

    #[test]
    fn a_deleted_sheet_is_never_written_back_through_a_swapped_folder() {
        let (_dir, docs) = setup();
        let rejected = sheet("Rejected", "src/a.rs", "Unsafe call");
        write(&docs, "TD-1-rej", &rejected);
        let snap = snapshot(&docs);
        std::fs::remove_file(docs.join("tech-debt/TD-1-rej.md")).unwrap();
        let external = tempfile::tempdir().unwrap();
        // The read of the sheet is the first guarded call; the swap lands on the write.
        swap_probe::arm_after(&docs.join("tech-debt"), external.path(), 1);
        let outcome = restore(docs.parent().unwrap(), &docs, &snap);
        assert!(swap_probe::fired(), "the swap really happened");
        assert!(outcome.is_ok(), "{outcome:?}");
        assert_nothing_outside(external.path());
        // The write went to the folder the call held (moved, still in the project).
        assert_eq!(
            std::fs::read_to_string(docs.join("tech-debt.moved/TD-1-rej.md")).unwrap(),
            rejected
        );
    }

    #[test]
    fn a_reverted_decision_is_never_written_through_a_swapped_folder() {
        let (_dir, docs) = setup();
        write(
            &docs,
            "TD-1-rej",
            &sheet("Rejected", "src/a.rs", "Unsafe call"),
        );
        let snap = snapshot(&docs);
        write(
            &docs,
            "TD-1-rej",
            &sheet("Verified in source", "src/a.rs", "Unsafe call"),
        );
        let external = tempfile::tempdir().unwrap();
        swap_probe::arm_after(&docs.join("tech-debt"), external.path(), 1);
        let outcome = restore(docs.parent().unwrap(), &docs, &snap);
        assert!(swap_probe::fired(), "the swap really happened");
        assert_eq!(outcome.unwrap().reverted, vec!["TD-1-rej"]);
        assert_nothing_outside(external.path());
        let held = std::fs::read_to_string(docs.join("tech-debt.moved/TD-1-rej.md")).unwrap();
        assert_eq!(td_decision(&held), Some(TdDecision::Rejected));
    }

    #[test]
    fn a_duplicate_flag_is_never_written_through_a_swapped_folder() {
        let (_dir, docs) = setup();
        write(
            &docs,
            "TD-1-rej",
            &sheet("Rejected", "src/a.rs", "Unsafe call"),
        );
        let snap = snapshot(&docs);
        write(
            &docs,
            "TD-9-dup",
            &sheet("Verified in source", "src/a.rs", "Unsafe call"),
        );
        let external = tempfile::tempdir().unwrap();
        // One read for the decided sheet, two for the listing, then the write.
        swap_probe::arm_after(&docs.join("tech-debt"), external.path(), 3);
        let outcome = restore(docs.parent().unwrap(), &docs, &snap).unwrap();
        assert!(swap_probe::fired(), "the swap really happened");
        assert_eq!(outcome.settled_duplicates.len(), 1);
        assert_nothing_outside(external.path());
        let held = std::fs::read_to_string(docs.join("tech-debt.moved/TD-9-dup.md")).unwrap();
        assert!(held.contains(DUPLICATE_FIELD), "{held}");
    }

    #[test]
    fn a_folder_already_swapped_creates_nothing_outside_the_project() {
        // The reported attack: the agent left `sleep; mv docs/tech-debt x; ln -s
        // /elsewhere docs/tech-debt` behind. The link target looks empty, so the
        // old code read NotFound, created the sheet and wrote it there.
        let (_dir, docs) = setup();
        write(
            &docs,
            "TD-1-rej",
            &sheet("Rejected", "src/a.rs", "Unsafe call"),
        );
        let snap = snapshot(&docs);
        std::fs::rename(docs.join("tech-debt"), docs.join("tech-debt.moved")).unwrap();
        let external = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(external.path(), docs.join("tech-debt")).unwrap();
        let outcome = restore(docs.parent().unwrap(), &docs, &snap);
        assert!(outcome.is_err(), "{outcome:?}");
        assert_nothing_outside(external.path());
    }

    #[test]
    fn a_linked_folder_is_neither_listed_nor_flagged() {
        let (_dir, docs) = setup();
        write(
            &docs,
            "TD-1-rej",
            &sheet("Rejected", "src/a.rs", "Unsafe call"),
        );
        let snap = snapshot(&docs);
        std::fs::remove_dir_all(docs.join("tech-debt")).unwrap();
        let external = tempfile::tempdir().unwrap();
        let foreign = [
            ("TD-1-rej.md", sheet("Rejected", "src/a.rs", "Unsafe call")),
            (
                "TD-9-dup.md",
                sheet("Verified in source", "src/a.rs", "Unsafe call"),
            ),
        ];
        for (name, text) in &foreign {
            std::fs::write(external.path().join(name), text).unwrap();
        }
        std::os::unix::fs::symlink(external.path(), docs.join("tech-debt")).unwrap();
        // The decided sheet is "missing" through the link: refused, not recreated.
        assert!(restore(docs.parent().unwrap(), &docs, &snap).is_err());
        // With nothing decided only the listing runs: it sees no sheet in a link.
        let outcome = restore(docs.parent().unwrap(), &docs, &DecisionSnapshot::default());
        assert!(outcome.unwrap().is_empty());
        for (name, text) in &foreign {
            let kept = std::fs::read_to_string(external.path().join(name)).unwrap();
            assert_eq!(&kept, text);
        }
    }

    #[test]
    fn a_docs_folder_outside_the_root_is_refused() {
        let (_dir, docs) = setup();
        let other = tempfile::tempdir().unwrap();
        let outcome = restore(other.path(), &docs, &DecisionSnapshot::default());
        assert!(outcome.is_err(), "{outcome:?}");
    }
}
