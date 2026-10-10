use super::*;
use std::fs;

fn git(dir: &Path, args: &[&str]) {
    let ok = crate::core::cmd::git_cmd()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs")
        .status
        .success();
    assert!(ok, "git {args:?} failed");
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

const SOURCE: &str = "\
fn alpha() {
    let retries = 3;
}

fn beta() {
    let timeout_ms = 2500;
}

fn gamma() {
    let feature_flag = true;
}
";

/// A repo whose `docs/architecture.md` cites lines 2, 6 and 10 of `src/lib.rs`,
/// and whose `docs/glossary.md` cites line 6 only. One commit.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::write(dir.join("src/lib.rs"), SOURCE).unwrap();
    fs::write(dir.join("README.md"), "readme\n").unwrap();
    fs::write(dir.join("docs/AGENTS.md"), "# Agents\n").unwrap();
    fs::write(
        dir.join("docs/architecture.md"),
        "# Architecture\n\
         Retries are three [src: src/lib.rs:2].\n\
         The timeout is 2.5 s [src: src/lib.rs:6].\n\
         A feature flag exists [src: file: src/lib.rs:10].\n\
         The readme exists [src: README.md].\n",
    )
    .unwrap();
    fs::write(
        dir.join("docs/glossary.md"),
        "# Glossary\nTimeout [src: src/lib.rs:6].\n",
    )
    .unwrap();
    commit_all(dir, "init");
    tmp
}

fn mapping(ai_file: &str, step: usize, project: &Path) -> crate::core::checksums::ChecksumMapping {
    crate::core::checksums::ChecksumMapping {
        ai_file: ai_file.to_string(),
        audit_step: step,
        sources: vec!["__GIT_SOURCE_TREE__".to_string()],
        checksums: crate::core::checksums::compute_step_checksums(
            project,
            &["__GIT_SOURCE_TREE__"],
        ),
    }
}

fn edit_source(dir: &Path, f: impl FnOnce(&mut Vec<String>)) {
    let mut lines: Vec<String> = fs::read_to_string(dir.join("src/lib.rs"))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    f(&mut lines);
    fs::write(dir.join("src/lib.rs"), lines.join("\n") + "\n").unwrap();
}

fn states(report: &CitationDriftReport, doc: &str) -> Vec<(String, CitationState)> {
    report
        .section(doc)
        .unwrap()
        .citations
        .iter()
        .map(|c| (c.cite.clone(), c.state))
        .collect()
}

#[test]
fn recording_writes_paths_lines_and_digests_only() {
    let tmp = fixture();
    record_all(tmp.path()).unwrap();
    let raw = fs::read_to_string(tmp.path().join("docs/citations.json")).unwrap();
    assert!(!raw.contains("timeout_ms"), "no cited content is stored");
    let file = read_strict(tmp.path()).unwrap().unwrap();
    let arch = &file.docs["docs/architecture.md"];
    assert_eq!(arch.reason, RecordReason::Validation);
    let cites: Vec<&str> = arch.citations.iter().map(|c| c.cite.as_str()).collect();
    assert_eq!(
        cites,
        ["src/lib.rs:2", "src/lib.rs:6", "src/lib.rs:10", "README.md"]
    );
    assert!(arch.citations[0]
        .sha256
        .as_deref()
        .is_some_and(|h| h.len() == 64));
    assert_eq!(arch.citations[3].line, None, "a bare path has no line");
    let report = check(tmp.path());
    assert_eq!(report.sources_state, SourcesState::Clean);
    assert_eq!(report.unchanged_count, 5);
}

#[test]
fn commit_touching_no_cited_line_marks_nothing_stale() {
    let tmp = fixture();
    let dir = tmp.path();
    record_all(dir).unwrap();
    let mappings = [
        mapping("docs/architecture.md", 3, dir),
        mapping("docs/glossary.md", 2, dir),
    ];
    crate::core::checksums::write_checksums_file(dir, &mappings).unwrap();

    // A new file and an edit on an uncited line: the whole-tree sentinel moves.
    edit_source(dir, |l| l[0] = "fn alpha() { // entry".to_string());
    fs::write(dir.join("src/other.rs"), "pub fn other() {}\n").unwrap();
    commit_all(dir, "unrelated");

    let drift = crate::core::checksums::check_drift(dir);
    assert!(
        drift.stale_sections.is_empty(),
        "{:?}",
        drift.stale_sections
    );
    assert_eq!(drift.fresh_sections.len(), 2);
    assert_eq!(drift.sources_state, SourcesState::Clean);
}

#[test]
fn legacy_section_without_fingerprints_keeps_tree_drift() {
    let tmp = fixture();
    let dir = tmp.path();
    crate::core::checksums::write_checksums_file(dir, &[mapping("docs/architecture.md", 3, dir)])
        .unwrap();
    // A baseline from before per-citation drift: no citations.json.
    fs::remove_file(dir.join("docs/citations.json")).unwrap();
    fs::write(dir.join("src/other.rs"), "pub fn other() {}\n").unwrap();
    commit_all(dir, "unrelated");
    let drift = crate::core::checksums::check_drift(dir);
    assert_eq!(drift.stale_sections.len(), 1);
    assert_eq!(drift.sources_state, SourcesState::Unknown);
}

#[test]
fn modified_cited_line_marks_its_section_stale_and_names_it() {
    let tmp = fixture();
    let dir = tmp.path();
    record_all(dir).unwrap();
    crate::core::checksums::write_checksums_file(
        dir,
        &[
            mapping("docs/architecture.md", 3, dir),
            mapping("docs/glossary.md", 2, dir),
        ],
    )
    .unwrap();
    // Re-indenting is not a change; a new value is.
    edit_source(dir, |l| {
        l[1] = "\t\tlet   retries = 3;".to_string();
        l[5] = "    let timeout_ms = 9000;".to_string();
    });
    commit_all(dir, "timeout");

    let report = check(dir);
    assert_eq!(report.sources_state, SourcesState::NeedsReverification);
    assert_eq!(
        states(&report, "docs/architecture.md"),
        [("src/lib.rs:6".to_string(), CitationState::Changed)]
    );
    let drift = crate::core::checksums::check_drift(dir);
    let stale: Vec<(&str, &Vec<String>)> = drift
        .stale_sections
        .iter()
        .map(|s| (s.ai_file.as_str(), &s.changed_sources))
        .collect();
    assert_eq!(stale.len(), 2, "both docs cite line 6: {stale:?}");
    assert_eq!(stale[0].1, &vec!["src/lib.rs:6 (changed)".to_string()]);
    assert_eq!(drift.broken_citations, 2);
}

#[test]
fn deleted_cited_lines_are_reported() {
    let tmp = fixture();
    let dir = tmp.path();
    record_all(dir).unwrap();
    // Remove `gamma` entirely: line 10 is now past the end of the file.
    edit_source(dir, |l| l.truncate(8));
    commit_all(dir, "drop gamma");
    let report = check(dir);
    assert_eq!(
        states(&report, "docs/architecture.md"),
        [("src/lib.rs:10".to_string(), CitationState::Removed)]
    );
    // Deleting the file breaks every citation of it, bare path included.
    fs::remove_file(dir.join("README.md")).unwrap();
    commit_all(dir, "drop readme");
    let report = check(dir);
    assert!(states(&report, "docs/architecture.md")
        .contains(&("README.md".to_string(), CitationState::FileMissing)));
}

#[test]
fn deleted_line_in_the_middle_is_changed_not_moved() {
    let tmp = fixture();
    let dir = tmp.path();
    record_all(dir).unwrap();
    edit_source(dir, |l| {
        l.remove(5);
    });
    let report = check(dir);
    let arch = states(&report, "docs/architecture.md");
    assert!(
        arch.contains(&("src/lib.rs:6".to_string(), CitationState::Changed)),
        "{arch:?}"
    );
    // Line 10 slid up by one: same content, re-anchored.
    assert!(
        arch.contains(&("src/lib.rs:10".to_string(), CitationState::Moved)),
        "{arch:?}"
    );
}

#[test]
fn moved_line_is_reanchored_not_stale() {
    let tmp = fixture();
    let dir = tmp.path();
    record_all(dir).unwrap();
    crate::core::checksums::write_checksums_file(dir, &[mapping("docs/architecture.md", 3, dir)])
        .unwrap();
    edit_source(dir, |l| {
        for i in 0..4 {
            l.insert(0, format!("// header {i}"));
        }
    });
    commit_all(dir, "header");
    let report = check(dir);
    assert_eq!(report.sources_state, SourcesState::Clean);
    let section = report.section("docs/architecture.md").unwrap();
    let moved: Vec<(String, Option<usize>)> = section
        .citations
        .iter()
        .map(|c| (c.cite.clone(), c.current_line))
        .collect();
    assert_eq!(
        moved,
        [
            ("src/lib.rs:2".to_string(), Some(6)),
            ("src/lib.rs:6".to_string(), Some(10)),
            ("src/lib.rs:10".to_string(), Some(14)),
        ]
    );
    assert!(crate::core::checksums::check_drift(dir)
        .stale_sections
        .is_empty());
}

#[test]
fn move_beyond_the_window_is_a_change() {
    let tmp = fixture();
    let dir = tmp.path();
    record_all(dir).unwrap();
    edit_source(dir, |l| {
        for i in 0..=REANCHOR_WINDOW {
            l.insert(0, format!("// pad {i}"));
        }
    });
    let report = check(dir);
    assert!(states(&report, "docs/architecture.md")
        .iter()
        .all(|(_, s)| *s == CitationState::Changed));
}

#[test]
fn trivial_line_is_never_reanchored() {
    let tmp = fixture();
    let dir = tmp.path();
    fs::write(
        dir.join("docs/glossary.md"),
        "# Glossary\nEnd of alpha [src: src/lib.rs:3].\n",
    )
    .unwrap();
    record_all(dir).unwrap();
    // Line 3 `}` becomes code; another `}` sits four lines below.
    edit_source(dir, |l| l[2] = "    retries + 1 }".to_string());
    let report = check(dir);
    assert_eq!(
        states(&report, "docs/glossary.md"),
        [("src/lib.rs:3".to_string(), CitationState::Changed)]
    );
}

#[test]
fn acknowledgement_refreshes_one_section_without_rewriting_the_doc() {
    let tmp = fixture();
    let dir = tmp.path();
    record_all(dir).unwrap();
    crate::core::checksums::write_checksums_file(
        dir,
        &[
            mapping("docs/architecture.md", 3, dir),
            mapping("docs/glossary.md", 2, dir),
        ],
    )
    .unwrap();
    edit_source(dir, |l| {
        l[5] = "    let timeout_ms = 2500; // ms".to_string()
    });
    commit_all(dir, "comment");
    let doc_before = fs::read(dir.join("docs/glossary.md")).unwrap();
    let doc_mtime = fs::metadata(dir.join("docs/glossary.md"))
        .unwrap()
        .modified()
        .unwrap();

    let section = acknowledge(dir, "docs/glossary.md").unwrap();
    assert!(!section.stale);
    assert_eq!(section.reason, RecordReason::Acknowledged);
    assert_eq!(fs::read(dir.join("docs/glossary.md")).unwrap(), doc_before);
    assert_eq!(
        fs::metadata(dir.join("docs/glossary.md"))
            .unwrap()
            .modified()
            .unwrap(),
        doc_mtime
    );
    let drift = crate::core::checksums::check_drift(dir);
    let stale: Vec<&str> = drift
        .stale_sections
        .iter()
        .map(|s| s.ai_file.as_str())
        .collect();
    assert_eq!(
        stale,
        ["docs/architecture.md"],
        "only the acknowledged section is fresh"
    );
}

#[test]
fn acknowledgement_refuses_paths_outside_the_docs() {
    let tmp = fixture();
    for doc in [
        "src/lib.rs",
        "../x.md",
        "/etc/hosts",
        "docs/../README.md",
        "docs/missing.md",
        "docs",
    ] {
        assert!(acknowledge(tmp.path(), doc).is_err(), "{doc}");
    }
    assert!(!tmp.path().join("docs/citations.json").exists());
}

#[test]
fn badge_is_revoked_then_restored_without_a_new_audit() {
    let tmp = fixture();
    let dir = tmp.path();
    record_all(dir).unwrap();
    crate::core::kronn_state::mark_validated(dir).unwrap();
    assert_eq!(check(dir).sources_state, SourcesState::Clean);

    edit_source(dir, |l| l[9] = "    let feature_flag = false;".to_string());
    commit_all(dir, "flip flag");
    assert_eq!(check(dir).sources_state, SourcesState::NeedsReverification);
    // The dated milestone stays: "audit complete" and "sources clean" are distinct.
    assert!(crate::core::kronn_state::read(dir)
        .unwrap()
        .validated_at
        .is_some());

    // Reverting the line restores the badge, no audit, no acknowledgement.
    edit_source(dir, |l| l[9] = "    let feature_flag = true;".to_string());
    commit_all(dir, "revert flag");
    assert_eq!(check(dir).sources_state, SourcesState::Clean);

    // Or the break is acknowledged after a human check.
    edit_source(dir, |l| l[9] = "    let feature_flag = !false;".to_string());
    assert_eq!(check(dir).sources_state, SourcesState::NeedsReverification);
    acknowledge(dir, "docs/architecture.md").unwrap();
    assert_eq!(check(dir).sources_state, SourcesState::Clean);
}

#[test]
fn audit_rerecords_only_the_docs_it_rewrote() {
    let tmp = fixture();
    let dir = tmp.path();
    record_all(dir).unwrap();
    edit_source(dir, |l| l[5] = "    let timeout_ms = 1;".to_string());
    // The partial run rewrote the glossary only.
    fs::write(
        dir.join("docs/glossary.md"),
        "# Glossary\nTimeout [src: src/lib.rs:6]\n",
    )
    .unwrap();
    record_after_audit(dir).unwrap();
    let report = check(dir);
    assert!(!report.section("docs/glossary.md").unwrap().stale);
    assert!(
        report.section("docs/architecture.md").unwrap().stale,
        "an untouched doc keeps its validation fingerprints"
    );
    let file = read_strict(dir).unwrap().unwrap();
    assert_eq!(file.docs["docs/glossary.md"].reason, RecordReason::Audit);
    assert_eq!(
        file.docs["docs/architecture.md"].reason,
        RecordReason::Validation
    );
}

#[test]
fn citation_removed_from_an_edited_doc_no_longer_counts() {
    let tmp = fixture();
    let dir = tmp.path();
    record_all(dir).unwrap();
    edit_source(dir, |l| l[5] = "    let timeout_ms = 1;".to_string());
    fs::write(
        dir.join("docs/glossary.md"),
        "# Glossary\nTimeout [src: src/lib.rs:2].\n",
    )
    .unwrap();
    let section = check(dir).section("docs/glossary.md").cloned().unwrap();
    assert!(section.doc_changed);
    assert!(!section.stale);
    assert_eq!(section.unrecorded, ["src/lib.rs:2"]);
}

#[test]
fn malformed_store_reads_as_unknown_and_blocks_writers() {
    let tmp = fixture();
    let dir = tmp.path();
    fs::write(dir.join("docs/citations.json"), "{ nope").unwrap();
    assert_eq!(check(dir).sources_state, SourcesState::Unknown);
    assert!(record_after_audit(dir).is_err());
    assert!(acknowledge(dir, "docs/glossary.md").is_err());
    assert_eq!(
        fs::read_to_string(dir.join("docs/citations.json")).unwrap(),
        "{ nope"
    );
}

#[test]
fn secret_value_never_reaches_the_digest() {
    let tmp = fixture();
    let dir = tmp.path();
    fs::write(dir.join("src/env.rs"), "API_SECRET=hunter2-very-secret\n").unwrap();
    fs::write(dir.join("docs/glossary.md"), "Secret [src: src/env.rs:1]\n").unwrap();
    record_all(dir).unwrap();
    fs::write(
        dir.join("src/env.rs"),
        "API_SECRET=another-value-entirely\n",
    )
    .unwrap();
    assert_eq!(check(dir).sources_state, SourcesState::Clean);
}

#[test]
fn citations_outside_the_project_are_not_tracked() {
    let tmp = fixture();
    let dir = tmp.path();
    fs::write(
        dir.join("docs/glossary.md"),
        "[src: ../outside.rs:1] [src: /etc/hosts:1] [src: https://x.y/a] [src: src/lib.rs:2]\n",
    )
    .unwrap();
    record_all(dir).unwrap();
    let file = read_strict(dir).unwrap().unwrap();
    let cites: Vec<&str> = file.docs["docs/glossary.md"]
        .citations
        .iter()
        .map(|c| c.cite.as_str())
        .collect();
    assert_eq!(cites, ["src/lib.rs:2"]);
}

#[test]
fn reverification_cron_is_off_by_default() {
    let config = crate::core::config::default_config_without_key();
    assert_eq!(config.audit_reverify_cron, None);
    let now = chrono::Utc::now();
    let since = now - chrono::Duration::days(2);
    assert!(!reverify_due(None, chrono_tz::UTC, since, now));
    assert!(!reverify_due(Some("  "), chrono_tz::UTC, since, now));
    assert!(reverify_due(Some("0 * * * *"), chrono_tz::UTC, since, now));
    assert!(!reverify_due(Some("0 * * * *"), chrono_tz::UTC, now, now));
}

#[test]
fn scheduled_reverification_checks_recorded_projects() {
    let tmp = fixture();
    let dir = tmp.path().to_path_buf();
    let untracked = tempfile::tempdir().unwrap();
    record_all(&dir).unwrap();
    edit_source(&dir, |l| l[1] = "    let retries = 5;".to_string());
    let results = reverify_projects(&[dir.clone(), untracked.path().to_path_buf()]);
    assert_eq!(
        results.len(),
        1,
        "a project without fingerprints is skipped"
    );
    assert_eq!(
        results[0].1.sources_state,
        SourcesState::NeedsReverification
    );
    assert_eq!(results[0].1.broken_count, 1);
    assert_eq!(last_scheduled_check(&dir), Some(results[0].1.clone()));
    assert_eq!(last_scheduled_check(untracked.path()), None);
}
