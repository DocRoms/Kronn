use super::*;
use std::fs;
use std::path::PathBuf;

#[test]
fn a_title_is_never_cut_inside_a_citation_marker() {
    // The live case (KT-1323): the 160-char limit landed inside the marker, the
    // end-of-run gate blamed step 8, and the regenerated index failed again.
    let lead = "x".repeat(140);
    let title = format!("{lead} [src: file: helpers/lib/body-fill.js:42] and more words after");
    let make = |t: &str| parse_sheet("TD-20261001-cut", &sheet("TD-20261001-cut", t, "Backend", "High", "Open", ""));
    let parsed = make(&title);
    assert!(parsed.title.chars().count() <= 160, "{}", parsed.title);
    assert!(
        !parsed.title.to_ascii_lowercase().contains("[src:"),
        "no partial marker in {}",
        parsed.title
    );
    assert!(parsed.title.ends_with('…'), "{}", parsed.title);

    let (text, _) = resync_content(CENTRAL, &[make(&title)], &BTreeMap::new());
    assert!(!text.contains("[src: file: helpers"), "{text}");
    // What the end-of-run gate read: an opener cut open swallows the next `]`
    // of the document as a bogus citation.
    let following = format!("{text}\nSee [the guide](guide.md).\n");
    assert!(
        crate::core::anti_halluc::extract_source_markers(&following).is_empty(),
        "{following}"
    );
    // Regenerating is stable: the same index again, no new failure.
    let (again, outcome) = resync_content(&text, &[make(&title)], &BTreeMap::new());
    assert_eq!(again, text);
    assert!(!outcome.changed);

    // A marker that fits whole is kept.
    let short = "Short [src: file: a/b.rs:1] title";
    assert_eq!(make(short).title, short);
}

const NO_ADR: crate::core::adr::Trust = crate::core::adr::Trust::RunBaseline {
    decisions: "",
    records: "",
};

fn sheet(id: &str, title: &str, area: &str, severity: &str, status: &str, extra: &str) -> String {
    format!(
        "---\nname: {}\ndescription: {title}\nmetadata:\n  type: tech-debt\n  audit_history:\n    - date: 2026-10-01\n      status: {status}\n      reviewer: Full audit\n---\n\n# {id}\n\n- **Area**: {area}\n- **Severity**: {severity}\n- **Status**: {status}\n{extra}\n## Problem (fact)\nSomething.\n",
        id.to_lowercase()
    )
}

/// The central index as Step 8 leaves it: two rows (one phantom), a
/// coverage matrix whose Accessibility and Security rows predate the
/// sub-audits.
const CENTRAL: &str = "# Tech debt (index)\n\nIntro prose.\n\n## Current list\n\n| ID | Problem | Area | Severity |\n|----|---------|------|----------|\n| TD-20261001-full-a | Full finding A | Backend | High |\n| TD-20261001-phantom | Row without a sheet | Backend | Low |\n\n## Dimension coverage\n\n| Dimension | Outcome | Evidence / reason |\n|-----------|---------|-------------------|\n| Security | N/A: no auth surface | none |\n| Accessibility | scanned — nothing substantiable | looked at templates |\n| Performance | N/A: no web surface | static |\n| Code quality | findings | TD-20261001-full-a |\n";

struct Fixture {
    _dir: tempfile::TempDir,
    docs: PathBuf,
}

const ACTIVE: &[&str] = &[
    "TD-20261001-full-a",
    "TD-20261001-full-b",
    "TD-20261002-sec-a",
    "TD-20261002-sec-b",
    "TD-20261003-a11y-a",
    "TD-20261003-a11y-b",
    "TD-20261004-db-a",
];

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    let td = docs.join("tech-debt");
    fs::create_dir_all(&td).unwrap();
    fs::write(docs.join(CENTRAL_INDEX), CENTRAL).unwrap();
    let w = |id: &str, body: String| fs::write(td.join(format!("{id}.md")), body).unwrap();
    w(
        "TD-20261001-full-a",
        sheet(
            "TD-20261001-full-a",
            "Full finding A",
            "Backend",
            "High",
            "Verified in source",
            "",
        ),
    );
    w(
        "TD-20261001-full-b",
        sheet(
            "TD-20261001-full-b",
            "Unlisted full finding | with pipe",
            "CI",
            "Medium",
            "Inferred",
            "",
        ),
    );
    w(
        "TD-20261002-sec-a",
        sheet(
            "TD-20261002-sec-a",
            "Secret in config",
            "Security",
            "Critical",
            "Verified in source",
            "",
        ),
    );
    w(
        "TD-20261002-sec-b",
        sheet(
            "TD-20261002-sec-b",
            "Missing CSP",
            "Backend",
            "Medium",
            "Confirmed by user",
            "- **Category**: security\n",
        ),
    );
    w(
        "TD-20261003-a11y-a",
        sheet(
            "TD-20261003-a11y-a",
            "Images without alt",
            "Frontend",
            "High",
            "Verified in source",
            "- **Category**: accessibility\n",
        ),
    );
    w(
        "TD-20261003-a11y-b",
        sheet(
            "TD-20261003-a11y-b",
            "Low contrast",
            "Frontend",
            "Low",
            "Deferred",
            "",
        ),
    );
    w(
        "TD-20261004-db-a",
        sheet(
            "TD-20261004-db-a",
            "No FK index",
            "Database",
            "Medium",
            "Verified in source",
            "",
        ),
    );
    w(
        "TD-20261005-rejected",
        sheet(
            "TD-20261005-rejected",
            "Rejected one",
            "Backend",
            "Low",
            "Rejected",
            "",
        ),
    );
    fs::write(td.join("README.md"), "# readme\n").unwrap();
    fs::write(td.join("_reconciliation-2026-10-05.md"), "# report\n").unwrap();
    fs::write(
        docs.join("inconsistencies-security.md"),
        "| ID | Problem | Area | Severity |\n|---|---|---|---|\n| TD-20261002-sec-a | Secret | Security | Critical |\n| TD-20261002-sec-b | CSP | Backend | Medium |\n",
    )
    .unwrap();
    fs::write(
        docs.join("inconsistencies-rgaa.md"),
        "| ID | Problem | Area | Severity |\n|---|---|---|---|\n| [TD-20261003-a11y-b](tech-debt/TD-20261003-a11y-b.md) | Contrast | Frontend | Low |\n",
    )
    .unwrap();
    fs::write(
        docs.join("inconsistencies-database.md"),
        "| ID | Problem | Area | Severity |\n|---|---|---|---|\n| `TD-20261004-db-a` | FK | Database | Medium |\n",
    )
    .unwrap();
    Fixture { _dir: dir, docs }
}

fn index(f: &Fixture) -> String {
    fs::read_to_string(f.docs.join(CENTRAL_INDEX)).unwrap()
}

fn row_count(content: &str, id: &str) -> usize {
    content
        .lines()
        .filter(|l| l.starts_with(&format!("| {id} |")))
        .count()
}

fn sheet_files(f: &Fixture) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<_> = fs::read_dir(f.docs.join("tech-debt"))
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn every_sheet_of_every_sub_audit_is_listed_exactly_once() {
    let f = fixture();
    let before = sheet_files(&f);
    let outcome = resync(&f.docs, NO_ADR).unwrap();
    let after = index(&f);
    assert!(outcome.changed);
    assert_eq!(outcome.listed, ACTIVE.len());
    assert_eq!(outcome.settled, 1);
    for id in ACTIVE {
        assert_eq!(row_count(&after, id), 1, "{id} must have one row:\n{after}");
        assert_eq!(
            after.matches(&format!("](tech-debt/{id}.md)")).count(),
            1,
            "{id} must be linked once"
        );
    }
    assert_eq!(
        row_count(&after, "TD-20261005-rejected"),
        0,
        "settled TDs leave the index"
    );
    assert!(after.contains("1 settled sheet (rejected"));
    // Title, severity and status come from the sheet; a pipe is escaped.
    assert!(after.contains(
        "| TD-20261001-full-b | Unlisted full finding \\| with pipe | CI | Medium | Inferred | — | [sheet](tech-debt/TD-20261001-full-b.md) |"
    ));
    assert!(after.contains("| TD-20261003-a11y-b | Low contrast | Frontend | Low | Deferred | [rgaa](inconsistencies-rgaa.md) |"));
    assert!(after.contains("Dimension indexes: [inconsistencies-database.md](inconsistencies-database.md), [inconsistencies-rgaa.md](inconsistencies-rgaa.md), [inconsistencies-security.md](inconsistencies-security.md)."));
    // The hand-written row without a sheet is kept, not dropped.
    assert!(after.contains("- `TD-20261001-phantom` — Row without a sheet"));
    assert_eq!(outcome.kept_without_sheet, 1);
    // Prose outside the table is untouched; no sheet is deleted or rewritten.
    assert!(after.starts_with(
        "# Tech debt (index)\n\nIntro prose.\n\n## Current list\n\n<!-- kronn:td-index:start"
    ));
    assert_eq!(sheet_files(&f), before);
}

#[test]
fn a_rerun_is_byte_identical() {
    let f = fixture();
    resync(&f.docs, NO_ADR).unwrap();
    let once = index(&f);
    let outcome = resync(&f.docs, NO_ADR).unwrap();
    assert!(!outcome.changed);
    assert_eq!(index(&f), once);
}

#[test]
fn false_no_finding_outcomes_are_recomputed_with_their_evidence() {
    let f = fixture();
    let outcome = resync(&f.docs, NO_ADR).unwrap();
    let after = index(&f);
    assert_eq!(
        outcome.reconciled_dimensions,
        vec!["Security", "Accessibility"]
    );
    assert!(after.contains(
        "| Accessibility | findings | Kronn resync: 2 active TDs on file after the dimension audits: TD-20261003-a11y-a, TD-20261003-a11y-b (previous outcome: scanned — nothing substantiable) |"
    ));
    assert!(after.contains(
        "| Security | findings | Kronn resync: 2 active TDs on file after the dimension audits: TD-20261002-sec-a, TD-20261002-sec-b (previous outcome: N/A: no auth surface) |"
    ));
    // No performance sheet: the N/A stands. A findings row is never rewritten.
    assert!(after.contains("| Performance | N/A: no web surface | static |"));
    assert!(after.contains("| Code quality | findings | TD-20261001-full-a |"));
}

#[test]
fn sheets_added_after_a_first_resync_join_the_index() {
    let f = fixture();
    resync(&f.docs, NO_ADR).unwrap();
    let td = f.docs.join("tech-debt");
    fs::write(
        td.join("TD-20261006-perf-a.md"),
        sheet(
            "TD-20261006-perf-a",
            "Huge bundle",
            "Frontend",
            "Medium",
            "Verified in source",
            "- **Category**: performance\n",
        ),
    )
    .unwrap();
    fs::remove_file(td.join("TD-20261001-full-b.md")).unwrap();
    let outcome = resync(&f.docs, NO_ADR).unwrap();
    let after = index(&f);
    assert_eq!(row_count(&after, "TD-20261006-perf-a"), 1);
    // A generated row follows its sheet away; a hand-written one stays.
    assert_eq!(row_count(&after, "TD-20261001-full-b"), 0);
    assert!(!after.contains("`TD-20261001-full-b`"));
    assert!(after.contains("- `TD-20261001-phantom` — Row without a sheet"));
    assert_eq!(outcome.reconciled_dimensions, vec!["Performance"]);
    assert_eq!(after.matches(BLOCK_END).count(), 1);
    assert!(!resync(&f.docs, NO_ADR).unwrap().changed);
}

#[test]
fn an_index_without_a_current_list_gets_one_before_the_coverage_matrix() {
    let f = fixture();
    fs::write(
        f.docs.join(CENTRAL_INDEX),
        "# Tech debt\n\n## Dimension coverage\n\n| Dimension | Outcome | Evidence |\n|---|---|---|\n",
    )
    .unwrap();
    resync(&f.docs, NO_ADR).unwrap();
    let after = index(&f);
    let list = after.find("## Current list").unwrap();
    assert!(list < after.find("## Dimension coverage").unwrap());
    assert_eq!(row_count(&after, "TD-20261002-sec-a"), 1);
    assert!(!resync(&f.docs, NO_ADR).unwrap().changed);
}

#[test]
fn a_table_quoted_in_a_code_fence_is_not_the_current_list() {
    let content = "# Index\n\n```\n## Current list\n| TD-20261001-full-a | x | y | z |\n```\n\n## Current list\n\n| ID | Problem | Area | Severity |\n|---|---|---|---|\n";
    let sheets = vec![parse_sheet(
        "TD-20261001-full-a",
        &sheet("TD-20261001-full-a", "A", "Backend", "High", "Inferred", ""),
    )];
    let (text, _) = resync_content(content, &sheets, &BTreeMap::new());
    assert!(text
        .starts_with("# Index\n\n```\n## Current list\n| TD-20261001-full-a | x | y | z |\n```\n"));
    assert_eq!(
        row_count(&text, "TD-20261001-full-a"),
        2,
        "the fenced row plus one generated row"
    );
}

#[test]
fn a_human_owned_index_is_resynced_and_marked_and_a_missing_one_left_alone() {
    let f = fixture();
    let human = format!("<!-- kronn:section name=\"debt\" owner=\"human\" -->\n{CENTRAL}<!-- kronn:section:end -->\n");
    fs::write(f.docs.join(CENTRAL_INDEX), &human).unwrap();
    let outcome = resync(&f.docs, NO_ADR).unwrap();
    assert!(
        outcome.skipped.is_none() && outcome.changed,
        "never skipped"
    );
    assert_ne!(index(&f), human);
    let project = f.docs.parent().unwrap();
    let provenance = crate::core::doc_provenance::report(project, false).unwrap();
    assert_eq!(provenance.to_reconfirm, 1, "{provenance:?}");
    assert_eq!(
        provenance.docs[0].kronn_writes[0].writer,
        crate::core::doc_provenance::DocWriter::TdIndex
    );

    fs::remove_file(f.docs.join(CENTRAL_INDEX)).unwrap();
    assert_eq!(
        resync(&f.docs, NO_ADR).unwrap().skipped.as_deref(),
        Some("no central index")
    );
    assert!(
        !f.docs.join(CENTRAL_INDEX).exists(),
        "resync never creates the index"
    );
}

#[test]
fn sheet_fields_fall_back_from_front_matter_to_body() {
    let s = parse_sheet(
        "TD-20261001-x",
        "---\nname: x\ndescription: >\n  Folded\n  title\nmetadata:\n  audit_history:\n    - date: 2026-10-01\n      status: Inferred\n---\n# TD-20261001-x\n\n**Severity:** High\n",
    );
    assert_eq!(s.title, "Folded title");
    assert_eq!(s.severity, "High");
    assert_eq!(s.status, "Inferred");
    assert_eq!(s.area, UNKNOWN);

    let bare = parse_sheet(
        "TD-20261001-y",
        "# Y title\n\n- **Problem**: The real problem\n```\n- **Status**: Rejected\n```\n",
    );
    assert_eq!(bare.title, "The real problem");
    assert_eq!(
        bare.status, UNKNOWN,
        "a status quoted in a fence is not the sheet's"
    );
    assert!(bare.is_active());
}

#[test]
fn a_finding_an_accepted_adr_covers_leaves_the_index_and_references_it() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    let td = docs.join("tech-debt");
    fs::create_dir_all(&td).unwrap();
    fs::write(docs.join(CENTRAL_INDEX), CENTRAL).unwrap();
    fs::write(
        docs.join("decisions.md"),
        "# Architecture decisions\n\n## ADR-0001: CI is label-triggered\n\n- **Status**: accepted\n- **Date**: 2026-08-28\n- **Covers**: quality-gate-not-enforced\n- **Source**: user\n\n### Context\n\nPaid runners.\n\n### Decision\n\nCI runs on the `ci` label.\n\n### Consequences\n\nDo NOT run CI on every push.\n",
    )
    .unwrap();
    let covered = "TD-20261011-quality-gate-not-enforced";
    let w = |id: &str, title: &str| {
        fs::write(
            td.join(format!("{id}.md")),
            sheet(id, title, "CI", "Medium", "Verified in source", ""),
        )
        .unwrap()
    };
    w(covered, "CI does not run on every push");
    w("TD-20261011-no-lint-in-ci", "No lint step in CI");

    let baseline = fs::read_to_string(docs.join("decisions.md")).unwrap();
    let outcome = resync(
        &docs,
        crate::core::adr::Trust::RunBaseline {
            decisions: &baseline,
            records: "",
        },
    )
    .unwrap();
    assert_eq!(
        outcome.adr_settled,
        vec![format!("{covered} → ADR-0001 — CI is label-triggered")]
    );
    let index = fs::read_to_string(docs.join(CENTRAL_INDEX)).unwrap();
    assert!(!index.contains(&format!("| {covered} |")), "{index}");
    assert!(index.contains("| TD-20261011-no-lint-in-ci |"), "{index}");
    let settled = fs::read_to_string(td.join(format!("{covered}.md"))).unwrap();
    assert!(settled.contains("- **Status**: Accepted decision"));
    assert!(settled.contains("- **Decision record**: ADR-0001 — CI is label-triggered"));
    let uncovered = fs::read_to_string(td.join("TD-20261011-no-lint-in-ci.md")).unwrap();
    assert!(uncovered.contains("- **Status**: Verified in source"));
    assert!(read_sheets(&docs)
        .iter()
        .any(|s| s.id == "TD-20261011-no-lint-in-ci" && s.is_active()));
}

#[test]
fn an_adr_accepted_during_the_run_leaves_its_finding_in_the_index_for_validation() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    let td = docs.join("tech-debt");
    fs::create_dir_all(&td).unwrap();
    fs::write(docs.join(CENTRAL_INDEX), CENTRAL).unwrap();
    let baseline = "# Architecture decisions\n";
    // Step 9 wrote this ADR as accepted, with the finding's slug in Covers.
    fs::write(
        docs.join("decisions.md"),
        "# Architecture decisions\n\n## ADR-0001: CI is label-triggered\n\n- **Status**: accepted\n- **Date**: 2026-10-11\n- **Covers**: quality-gate-not-enforced\n\n### Context\n\nc\n\n### Decision\n\nd\n\n### Consequences\n\nq\n",
    )
    .unwrap();
    let id = "TD-20261011-quality-gate-not-enforced";
    fs::write(
        td.join(format!("{id}.md")),
        sheet(
            id,
            "CI does not run on every push",
            "CI",
            "Medium",
            "Verified in source",
            "",
        ),
    )
    .unwrap();

    let outcome = resync(
        &docs,
        crate::core::adr::Trust::RunBaseline {
            decisions: baseline,
            records: "",
        },
    )
    .unwrap();
    assert!(outcome.adr_settled.is_empty(), "{outcome:?}");
    assert_eq!(
        outcome.adr_downgraded,
        vec!["ADR-0001 — CI is label-triggered".to_string()]
    );
    let index = fs::read_to_string(docs.join(CENTRAL_INDEX)).unwrap();
    assert!(index.contains(&format!("| {id} |")), "{index}");
    // Still undecided: the validation asks the human about it.
    let scope = crate::api::audit::reconciliation::td_validation_scope(
        dir.path(),
        &[format!("docs/{CENTRAL_INDEX}")],
    );
    assert!(scope.undecided.contains(&id.to_string()), "{scope:?}");
    let decisions = fs::read_to_string(docs.join("decisions.md")).unwrap();
    assert!(decisions.contains("- **Status**: proposed"), "{decisions}");
}

#[test]
fn index_rows_are_read_through_their_header_for_any_sheet_id() {
    let content = "# Index\n\n| ID | Problem | Area | Severity | Status | Sheet |\n|----|---------|------|----------|--------|-------|\n| [CQ-001](tech-debt/CQ-001.md) | Oversized | Backend | **High** | Open | x |\n| `PERF-2026-a` | Slow | — | medium | — | |\n| Not an id | ignored | | | | |\n\n| Ref | Severity | Title |\n|-----|----------|-------|\n| CQ-009 | Critical | Reordered columns |\n\n| TD-1-x | Headerless | Area | Low |\n";
    let rows = index_rows(content);
    let cq = &rows["CQ-001"];
    assert_eq!(
        (
            cq.severity.as_deref(),
            cq.status.as_deref(),
            cq.area.as_deref()
        ),
        (Some("High"), Some("Open"), Some("Backend"))
    );
    let perf = &rows["PERF-2026-a"];
    assert_eq!(perf.severity.as_deref(), Some("medium"));
    assert_eq!(perf.area, None, "an empty cell says nothing");
    assert_eq!(rows["CQ-009"].severity.as_deref(), Some("Critical"));
    // A table without a header uses the central index layout.
    assert_eq!(rows["TD-1-x"].severity.as_deref(), Some("Low"));
    assert!(!rows.contains_key("ID"));
}

#[test]
fn a_resync_keeps_the_severity_of_a_detail_file_the_index_row_gave() {
    let fx = fixture();
    fs::write(
        fx.docs.join("tech-debt/CQ-001.md"),
        "# CQ-001: Oversized controller\n",
    )
    .unwrap();
    let index = fx.docs.join(CENTRAL_INDEX);
    let mut content = fs::read_to_string(&index).unwrap();
    content = content.replace(
        "| TD-20261001-phantom |",
        "| CQ-001 | Oversized controller | Backend | High |\n| TD-20261001-phantom |",
    );
    fs::write(&index, content).unwrap();
    resync(&fx.docs, NO_ADR).unwrap();
    let after = fs::read_to_string(&index).unwrap();
    let rows = index_rows(&after);
    assert_eq!(rows["CQ-001"].severity.as_deref(), Some("High"), "{after}");
    // A second pass changes nothing.
    assert!(!resync(&fx.docs, NO_ADR).unwrap().changed);
}
