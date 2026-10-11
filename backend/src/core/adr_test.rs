use super::*;
use std::fs;

const TEMPLATE: &str = include_str!("../../../templates/docs/decisions.md");

const CI_LABEL_ADR: &str = "# Architecture decisions\n\n## ADR-0001: CI is label-triggered\n\n- **Status**: accepted\n- **Date**: 2026-08-28\n- **Covers**: quality-gate-not-enforced\n- **Source**: `.github/workflows/ci.yml:3` + user\n\n### Context\n\nA CI run costs 40 minutes of paid runners.\n\n### Decision\n\nCI runs when a pull request carries the `ci` label.\n\n### Consequences\n\nDo NOT switch to `on: push`.\n";

/// An audit run that found `decisions` and no `Decision record` line.
fn run(decisions: &str) -> Trust<'_> {
    Trust::RunBaseline {
        decisions,
        records: "",
    }
}

/// The decisions in force in `content`, all trusted.
fn force(content: &str) -> Vec<Adr> {
    in_force(&parse(content)).into_iter().cloned().collect()
}

fn sheet(id: &str, title: &str, extra: &str) -> String {
    format!(
        "---\nname: {}\ndescription: {title}\nmetadata:\n  type: tech-debt\n  audit_history:\n    - date: 2026-10-11\n      status: Verified in source\n      reviewer: Full audit\n---\n\n# {id}\n\n- **Area**: CI\n- **Severity**: Medium\n- **Status**: Verified in source\n{extra}\n## Problem (fact)\n{title}\n",
        id.to_lowercase()
    )
}

/// The ```markdown example of the template.
fn template_example() -> String {
    let start = TEMPLATE.find("```markdown\n").expect("template example") + "```markdown\n".len();
    let end = start + TEMPLATE[start..].find("\n```").expect("closed example");
    TEMPLATE[start..end].to_string()
}

#[test]
fn the_template_documents_a_complete_adr_format() {
    let example = template_example();
    assert!(problems(&example).is_empty(), "{:?}", problems(&example));
    let adrs = parse(&example);
    assert_eq!(adrs.len(), 1);
    let adr = &adrs[0];
    assert_eq!(adr.id, "ADR-0001");
    assert!(!adr.title.is_empty());
    assert_eq!(adr.date.as_deref(), Some("2026-08-28"));
    assert_eq!(adr.status, AdrStatus::Accepted);
    assert_eq!(adr.sections, vec!["consequences", "context", "decision"]);
    assert_eq!(adr.covers, vec!["quality-gate-not-enforced".to_string()]);
    for field in [
        "Status",
        "Date",
        "Supersedes",
        "Covers",
        "Source",
        "Links",
        "proposed",
        "accepted",
        "superseded",
        "### Context",
        "### Decision",
        "### Consequences",
    ] {
        assert!(
            TEMPLATE.contains(field),
            "the template must document `{field}`"
        );
    }
}

#[test]
fn the_raw_template_records_no_decision_in_force() {
    // The example lives in a fence and the placeholder entry has no status:
    // a freshly installed template can never settle a TD.
    let adrs = parse(TEMPLATE);
    assert!(in_force(&adrs).is_empty(), "{adrs:?}");
    assert!(crate::api::audit::validation::count_raw_placeholders(TEMPLATE) > 0);
}

#[test]
fn the_template_and_prompts_state_no_entry_cap() {
    let step9 = crate::api::audit::ANALYSIS_STEPS
        .iter()
        .find(|s| s.target_file == "docs/decisions.md")
        .expect("decisions step");
    for (name, text) in [("template", TEMPLATE), ("step 9 prompt", step9.prompt)] {
        for cap in ["3-8", "3–8", "3 to 8", "Target 3", "target 3"] {
            assert!(!text.contains(cap), "{name} still states a cap: {cap}");
        }
    }
    assert!(step9.prompt.contains("No count target"));
    // Twenty sourced ADRs are all valid and all in force.
    let mut many = String::from("# Decisions\n");
    for n in 1..=20 {
        many.push_str(&format!(
            "\n## ADR-{n:04}: Choice {n}\n\n- **Status**: accepted\n- **Date**: 2026-10-11\n\n### Context\n\nc\n\n### Decision\n\nd\n\n### Consequences\n\nq\n"
        ));
    }
    assert!(problems(&many).is_empty());
    assert_eq!(in_force(&parse(&many)).len(), 20);
}

#[test]
fn problems_name_every_missing_field() {
    let incomplete = "## ADR-0003: Half written\n\n- **Status**: maybe\n\n### Context\n\nonly this\n\n## ADR-3: Same id\n\n- **Status**: proposed\n- **Date**: 2026-10-11\n- **Context**: c\n- **Decision**: d\n- **Consequences**: q\n";
    let found = problems(incomplete);
    for expected in [
        "ADR-0003: date is not YYYY-MM-DD",
        "ADR-0003: status is not proposed, accepted or superseded",
        "ADR-0003: no Decision section",
        "ADR-0003: no Consequences section",
        "ADR-0003: duplicate id",
    ] {
        assert!(
            found.iter().any(|p| p == expected),
            "{expected} in {found:?}"
        );
    }
    assert!(!found.iter().any(|p| p == "ADR-0003: no Context section"));
}

#[test]
fn only_accepted_unreplaced_adrs_are_in_force() {
    let content = "## ADR-0001: Old\n\n- **Status**: superseded by ADR-0003\n- **Covers**: old-slug\n\n## ADR-0002: Idea\n\n- **Status**: proposed\n- **Covers**: idea-slug\n\n## ADR-0003: New\n\n- **Status**: accepted\n- **Supersedes**: ADR-0004\n\n## ADR-0004: Replaced but still marked accepted\n\n- **Status**: accepted\n";
    let adrs = parse(content);
    assert_eq!(adrs.len(), 4);
    assert_eq!(adrs[0].superseded_by.as_deref(), Some("ADR-0003"));
    let ids: Vec<&str> = in_force(&adrs).iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, vec!["ADR-0003"]);
}

#[test]
fn slugs_ignore_the_date_and_links() {
    assert_eq!(
        td_slug("TD-20260828-quality-gate-not-enforced"),
        "quality-gate-not-enforced"
    );
    assert_eq!(td_slug("TD-quality-gate"), "quality-gate");
    assert_eq!(td_slug("quality-gate"), "quality-gate");
    assert_eq!(
        covers("[`TD-20260828-a`](tech-debt/TD-20260828-a.md), `b-slug`; Not a slug!"),
        vec!["a".to_string(), "b-slug".to_string()]
    );
}

#[test]
fn a_finding_an_accepted_adr_covers_is_a_decision() {
    let adrs = force(CI_LABEL_ADR);
    // Re-discovered on another day under the covered slug.
    let td = sheet(
        "TD-20261011-quality-gate-not-enforced",
        "CI does not run on every push",
        "",
    );
    match coverage("TD-20261011-quality-gate-not-enforced", &td, &adrs, None) {
        Coverage::Covered(adr) => assert_eq!(adr.id, "ADR-0001"),
        other => panic!("{other:?}"),
    }
    // Named by its Decision record, whatever its slug.
    let named = sheet(
        "TD-20261011-ci-optional",
        "x",
        "- **Decision record**: ADR-1\n",
    );
    assert!(matches!(
        coverage("TD-20261011-ci-optional", &named, &adrs, None),
        Coverage::Covered(_)
    ));
    // A different finding stays debt.
    let other = sheet("TD-20261011-no-lint", "no lint", "");
    assert_eq!(
        coverage("TD-20261011-no-lint", &other, &adrs, None),
        Coverage::None
    );
    // A proposed ADR covers nothing.
    let proposed = CI_LABEL_ADR.replace("**Status**: accepted", "**Status**: proposed");
    let adrs = force(&proposed);
    assert_eq!(
        coverage("TD-20261011-quality-gate-not-enforced", &td, &adrs, None),
        Coverage::None
    );
    assert_eq!(
        coverage("TD-20261011-ci-optional", &named, &adrs, None),
        Coverage::NotInForce("ADR-0001".into())
    );
}

#[test]
fn settling_rewrites_status_record_and_history() {
    let adrs = parse(CI_LABEL_ADR);
    let td = sheet("TD-20261011-quality-gate-not-enforced", "CI optional", "");
    let settled = settle_sheet(&td, &adrs[0]);
    assert_eq!(td_decision(&settled), Some(TdDecision::AcceptedDecision));
    assert!(settled.contains(
        "- **Status**: Accepted decision\n- **Decision record**: ADR-0001 — CI is label-triggered ([decisions](../decisions.md))\n"
    ));
    assert!(settled.contains("reviewer: Kronn ADR check"));
    assert_eq!(decision_record(&settled).as_deref(), Some("ADR-0001"));
    // A sheet without front matter or status still ends up settled.
    let bare = "# TD-x\n\nNo fields.\n";
    let settled_bare = settle_sheet(bare, &adrs[0]);
    assert_eq!(
        td_decision(&settled_bare),
        Some(TdDecision::AcceptedDecision)
    );
    assert_eq!(decision_record(&settled_bare).as_deref(), Some("ADR-0001"));
}

#[test]
fn legacy_table_rows_cover_the_sheets_they_link() {
    let legacy = "## Audit validation decisions\n\n| Decision | Why chosen | What NOT to do | Source |\n|---|---|---|---|\n| Accept `TD-20260901-old` as an intentional trade-off | ok | no | [`TD-20260901-old`](tech-debt/TD-20260901-old.md) |\n| Plain row | why | no | `src/a.rs:1` |\n";
    let adrs = force(legacy);
    assert_eq!(adrs.len(), 1);
    assert!(adrs[0].legacy);
    let td = sheet("TD-20260901-old", "old", "");
    assert!(matches!(
        coverage("TD-20260901-old", &td, &adrs, None),
        Coverage::Covered(_)
    ));
    assert!(problems(legacy).is_empty());
}

#[test]
fn settle_covered_sheets_writes_once_and_skips_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(docs.join("tech-debt")).unwrap();
    fs::write(docs.join(DECISIONS_FILE), CI_LABEL_ADR).unwrap();
    let covered = docs.join("tech-debt/TD-20261011-quality-gate-not-enforced.md");
    fs::write(
        &covered,
        sheet("TD-20261011-quality-gate-not-enforced", "CI optional", ""),
    )
    .unwrap();
    let rejected = sheet("TD-20261001-quality-gate-not-enforced", "older", "")
        .replace("Verified in source", "Rejected");
    let rejected_path = docs.join("tech-debt/TD-20261001-quality-gate-not-enforced.md");
    fs::write(&rejected_path, &rejected).unwrap();
    fs::write(
        docs.join("tech-debt/TD-20261011-dangling.md"),
        sheet(
            "TD-20261011-dangling",
            "d",
            "- **Decision record**: ADR-0009\n",
        ),
    )
    .unwrap();

    let first = settle_covered_sheets(&docs, run(CI_LABEL_ADR)).unwrap();
    assert_eq!(
        first.settled,
        vec![
            "TD-20261011-quality-gate-not-enforced → ADR-0001 — CI is label-triggered".to_string()
        ]
    );
    assert_eq!(
        first.not_in_force,
        vec!["TD-20261011-dangling → ADR-0009".to_string()]
    );
    assert!(first.problems.is_empty());
    assert_eq!(fs::read_to_string(&rejected_path).unwrap(), rejected);

    let after_first = fs::read_to_string(&covered).unwrap();
    let second = settle_covered_sheets(&docs, run(CI_LABEL_ADR)).unwrap();
    assert!(second.settled.is_empty(), "{second:?}");
    assert_eq!(fs::read_to_string(&covered).unwrap(), after_first);
}

#[test]
fn the_prompt_block_lists_only_decisions_in_force() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(&docs).unwrap();
    assert_eq!(in_force_prompt_block(&docs, ""), "");
    fs::write(docs.join(DECISIONS_FILE), CI_LABEL_ADR).unwrap();
    let block = in_force_prompt_block(&docs, CI_LABEL_ADR);
    assert!(
        block.contains("- ADR-0001 — CI is label-triggered (covers: quality-gate-not-enforced)")
    );
    assert!(block.contains("do NOT write a TD"));
    fs::write(
        docs.join(DECISIONS_FILE),
        CI_LABEL_ADR.replace("accepted", "proposed"),
    )
    .unwrap();
    assert_eq!(in_force_prompt_block(&docs, CI_LABEL_ADR), "");
    // Accepted during the run: not in force for this run's steps.
    fs::write(docs.join(DECISIONS_FILE), CI_LABEL_ADR).unwrap();
    assert_eq!(in_force_prompt_block(&docs, "# Decisions\n"), "");
}

/// A docs dir whose `decisions.md` is `decisions`, with one CI sheet.
fn run_fixture(decisions: &str, sheet_extra: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(docs.join("tech-debt")).unwrap();
    fs::write(docs.join(DECISIONS_FILE), decisions).unwrap();
    fs::write(
        docs.join("tech-debt/TD-20261011-quality-gate-not-enforced.md"),
        sheet(
            "TD-20261011-quality-gate-not-enforced",
            "CI optional",
            sheet_extra,
        ),
    )
    .unwrap();
    (dir, docs)
}

#[test]
fn an_adr_accepted_during_the_run_settles_nothing_and_goes_back_to_proposed() {
    let baseline = "# Architecture decisions\n";
    let (_dir, docs) = run_fixture(CI_LABEL_ADR, "- **Decision record**: ADR-0001\n");
    let outcome = settle_covered_sheets(&docs, run(baseline)).unwrap();
    assert!(outcome.settled.is_empty(), "{outcome:?}");
    assert_eq!(
        outcome.downgraded,
        vec!["ADR-0001 — CI is label-triggered".to_string()]
    );
    // The agent's Decision record names an ADR that may not settle.
    assert_eq!(
        outcome.not_in_force,
        vec!["TD-20261011-quality-gate-not-enforced → ADR-0001".to_string()]
    );
    let sheet = fs::read_to_string(docs.join("tech-debt/TD-20261011-quality-gate-not-enforced.md"))
        .unwrap();
    assert_eq!(td_decision(&sheet), None, "{sheet}");
    let decisions = fs::read_to_string(docs.join(DECISIONS_FILE)).unwrap();
    let adr = &parse(&decisions)[0];
    assert_eq!(adr.status, AdrStatus::Proposed, "{decisions}");
    assert!(decisions.contains("only a human accepts an ADR"));
    assert!(in_force(&parse(&decisions)).is_empty());
    // The next run starts from the downgraded file: still nothing settles.
    let again = settle_covered_sheets(&docs, run(&decisions)).unwrap();
    assert!(again.settled.is_empty() && again.downgraded.is_empty());
}

#[test]
fn a_covers_entry_added_during_the_run_does_not_count() {
    // ADR-0001 was accepted before the run, without the TD's slug.
    let baseline = CI_LABEL_ADR.replace("- **Covers**: quality-gate-not-enforced\n", "");
    let (_dir, docs) = run_fixture(CI_LABEL_ADR, "");
    let outcome = settle_covered_sheets(&docs, run(&baseline)).unwrap();
    assert!(outcome.settled.is_empty(), "{outcome:?}");
    assert!(
        outcome.downgraded.is_empty(),
        "an ADR accepted before the run stays accepted"
    );
}

#[test]
fn an_adr_accepted_before_the_run_still_settles() {
    let (_dir, docs) = run_fixture(CI_LABEL_ADR, "");
    let outcome = settle_covered_sheets(&docs, run(CI_LABEL_ADR)).unwrap();
    assert_eq!(outcome.settled.len(), 1, "{outcome:?}");
    assert!(outcome.downgraded.is_empty());
    // Superseding it during the run only removes coverage (the safe side).
    let (_dir2, docs2) = run_fixture(
        &CI_LABEL_ADR.replace("**Status**: accepted", "**Status**: superseded by ADR-0002"),
        "",
    );
    let outcome = settle_covered_sheets(&docs2, run(CI_LABEL_ADR)).unwrap();
    assert!(outcome.settled.is_empty());
}

#[test]
fn a_card_settles_only_with_the_adrs_it_appended() {
    // ADR-0001 was written by an agent; ADR-0002 by the card.
    let card = CI_LABEL_ADR.to_string()
        + "\n## ADR-0002: Accept `TD-20261011-other`\n\n- **Status**: accepted\n- **Date**: 2026-10-11\n- **Covers**: TD-20261011-other\n";
    let (_dir, docs) = run_fixture(&card, "");
    fs::write(
        docs.join("tech-debt/TD-20261011-other.md"),
        sheet("TD-20261011-other", "other", ""),
    )
    .unwrap();
    let ids = vec!["ADR-0002".to_string()];
    let outcome = settle_covered_sheets(&docs, Trust::CardAccepted(&ids)).unwrap();
    assert_eq!(
        outcome.settled,
        vec!["TD-20261011-other → ADR-0002 — Accept `TD-20261011-other`".to_string()]
    );
    assert!(outcome.downgraded.is_empty(), "a card never downgrades");
}

#[test]
fn downgrade_touches_only_the_named_adrs_status() {
    let two = CI_LABEL_ADR.to_string()
        + "\n## ADR-0002: Other\n\n- **Status**: accepted\n\n```\n- **Status**: accepted\n```\n";
    let lowered = downgrade(&two, &["ADR-0002".to_string()], "2026-10-11");
    let adrs = parse(&lowered);
    assert_eq!(adrs[0].status, AdrStatus::Accepted);
    assert_eq!(adrs[1].status, AdrStatus::Proposed);
    assert!(
        lowered.contains("```\n- **Status**: accepted\n```"),
        "the fence is left alone"
    );
    assert_eq!(lowered.lines().count(), two.lines().count());
}

/// Two ADRs accepted before the run, neither covering a slug by `Covers`.
const TWO_ADRS: &str = "## ADR-0001: CI is label-triggered\n\n- **Status**: accepted\n- **Date**: 2026-08-28\n\n### Context\n\nc\n\n### Decision\n\nd\n\n### Consequences\n\nq\n\n## ADR-0002: No ORM\n\n- **Status**: accepted\n- **Date**: 2026-08-28\n\n### Context\n\nc\n\n### Decision\n\nd\n\n### Consequences\n\nq\n";

fn write_sheet(docs: &std::path::Path, id: &str, extra: &str) {
    fs::write(
        docs.join(format!("tech-debt/{id}.md")),
        sheet(id, "real debt", extra),
    )
    .unwrap();
}

fn status_of(docs: &std::path::Path, id: &str) -> Option<TdDecision> {
    td_decision(&fs::read_to_string(docs.join(format!("tech-debt/{id}.md"))).unwrap())
}

#[test]
fn a_record_line_the_run_writes_on_a_new_sheet_settles_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(docs.join("tech-debt")).unwrap();
    fs::write(docs.join(DECISIONS_FILE), TWO_ADRS).unwrap();
    let start = StartOfRun::capture(&docs);
    assert_eq!(start.records, "");
    // The TD step writes a real finding and cites an unrelated accepted ADR.
    write_sheet(
        &docs,
        "TD-20261011-sql-injection",
        "- **Decision record**: ADR-0001\n",
    );
    let outcome = settle_covered_sheets(&docs, start.trust()).unwrap();
    assert!(outcome.settled.is_empty(), "{outcome:?}");
    assert_eq!(
        outcome.not_in_force,
        vec![
            "TD-20261011-sql-injection → ADR-0001 (Decision record written during the run)"
                .to_string()
        ]
    );
    assert_eq!(status_of(&docs, "TD-20261011-sql-injection"), None);
}

#[test]
fn a_record_line_the_run_adds_or_changes_on_an_existing_sheet_settles_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(docs.join("tech-debt")).unwrap();
    fs::write(docs.join(DECISIONS_FILE), TWO_ADRS).unwrap();
    write_sheet(&docs, "TD-20261001-no-record", "");
    write_sheet(
        &docs,
        "TD-20261001-human",
        "- **Decision record**: ADR-0002\n",
    );
    let start = StartOfRun::capture(&docs);
    assert_eq!(start.records, "TD-20261001-human ADR-0002\n");
    // The REVERIFY block rewrites both sheets during the run.
    write_sheet(
        &docs,
        "TD-20261001-no-record",
        "- **Decision record**: ADR-0001\n",
    );
    write_sheet(
        &docs,
        "TD-20261001-human",
        "- **Decision record**: ADR-0001\n",
    );
    let outcome = settle_covered_sheets(&docs, start.trust()).unwrap();
    assert!(outcome.settled.is_empty(), "{outcome:?}");
    assert_eq!(outcome.not_in_force.len(), 2, "{outcome:?}");
    assert_eq!(status_of(&docs, "TD-20261001-no-record"), None);
    assert_eq!(status_of(&docs, "TD-20261001-human"), None);
}

#[test]
fn a_record_line_present_before_the_run_still_settles() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(docs.join("tech-debt")).unwrap();
    fs::write(docs.join(DECISIONS_FILE), TWO_ADRS).unwrap();
    write_sheet(&docs, "TD-20261001-human", "- **Decision record**: ADR-2\n");
    let start = StartOfRun::capture(&docs);
    let outcome = settle_covered_sheets(&docs, start.trust()).unwrap();
    assert_eq!(
        outcome.settled,
        vec!["TD-20261001-human → ADR-0002 — No ORM".to_string()]
    );
    assert_eq!(
        status_of(&docs, "TD-20261001-human"),
        Some(TdDecision::AcceptedDecision)
    );
    // Settled lines are part of the next run's baseline.
    assert_eq!(
        parse_records(&capture_records(&docs)),
        Records::from([("TD-20261001-human".to_string(), "ADR-0002".to_string())])
    );
}

#[test]
fn coverage_honours_only_record_pairs_from_the_start_of_the_run() {
    let adrs = force(TWO_ADRS);
    let td = sheet("TD-20261011-x", "x", "- **Decision record**: ADR-0001\n");
    let pair = |td: &str, adr: &str| Records::from([(td.to_string(), adr.to_string())]);
    assert!(matches!(
        coverage(
            "TD-20261011-x",
            &td,
            &adrs,
            Some(&pair("TD-20261011-x", "ADR-0001"))
        ),
        Coverage::Covered(_)
    ));
    for other in [
        pair("TD-20261011-x", "ADR-0002"),
        pair("TD-20261011-y", "ADR-0001"),
        Records::new(),
    ] {
        assert_eq!(
            coverage("TD-20261011-x", &td, &adrs, Some(&other)),
            Coverage::WrittenDuringRun("ADR-0001".into())
        );
    }
    // A Covers entry from the start of the run still settles a new sheet.
    let covered = force(CI_LABEL_ADR);
    let new = sheet("TD-20261011-quality-gate-not-enforced", "x", "");
    assert!(matches!(
        coverage(
            "TD-20261011-quality-gate-not-enforced",
            &new,
            &covered,
            Some(&Records::new())
        ),
        Coverage::Covered(_)
    ));
}

#[test]
fn a_card_ignores_a_record_line_naming_the_adr_it_creates() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("docs");
    fs::create_dir_all(docs.join("tech-debt")).unwrap();
    // An agent guessed the id the next card would create.
    write_sheet(
        &docs,
        "TD-20261011-hidden-debt",
        "- **Decision record**: ADR-0005\n",
    );
    write_sheet(&docs, "TD-20261011-accepted", "");
    // The card for the other TD appends ADR-0005, covering only that TD.
    let card = "## ADR-0005: Accept `TD-20261011-accepted` as an intentional trade-off\n\n- **Status**: accepted\n- **Date**: 2026-10-11\n- **Covers**: TD-20261011-accepted\n";
    fs::write(docs.join(DECISIONS_FILE), card).unwrap();
    let ids = vec!["ADR-0005".to_string()];
    let outcome = settle_covered_sheets(&docs, Trust::CardAccepted(&ids)).unwrap();
    assert_eq!(
        outcome.settled,
        vec![
            "TD-20261011-accepted → ADR-0005 — Accept `TD-20261011-accepted` as an intentional trade-off"
                .to_string()
        ]
    );
    assert_eq!(
        outcome.not_in_force,
        vec![
            "TD-20261011-hidden-debt → ADR-0005 (Decision record ignored by a validation card)"
                .to_string()
        ]
    );
    assert_eq!(status_of(&docs, "TD-20261011-hidden-debt"), None);
    assert_eq!(
        status_of(&docs, "TD-20261011-accepted"),
        Some(TdDecision::AcceptedDecision)
    );
}
