use super::*;
use crate::models::{BriefingDocsAnswers, BriefingProjectAnswers, BriefingTracker};

fn rule(text: &str) -> ProjectRule {
    ProjectRule {
        kind: ProjectRuleKind::Rule,
        text: text.into(),
        pattern: None,
    }
}

fn ban(text: &str, pattern: Option<(ProjectRulePatternKind, &str)>) -> ProjectRule {
    ProjectRule {
        kind: ProjectRuleKind::Ban,
        text: text.into(),
        pattern: pattern.map(|(kind, value)| ProjectRulePattern {
            kind,
            value: value.into(),
        }),
    }
}

fn rules(list: Vec<ProjectRule>) -> BriefingProjectRules {
    BriefingProjectRules { rules: list }
}

fn sample() -> BriefingProjectRules {
    rules(vec![
        rule("toujours passer par le service X"),
        ban(
            "ne jamais utiliser lodash",
            Some((ProjectRulePatternKind::Dependency, "lodash")),
        ),
        ban("pas de nouvelle dépendance sans accord", None),
    ])
}

// ─── Validation ─────────────────────────────────────────────────────────────

#[test]
fn normalize_trims_drops_empty_lines_and_duplicates() {
    let out = normalize(&rules(vec![
        rule("  always X  "),
        rule("   "),
        rule("always X"),
        ban(
            " never lodash ",
            Some((ProjectRulePatternKind::Import, " lodash ")),
        ),
    ]))
    .unwrap();
    assert_eq!(out.rules.len(), 2);
    assert_eq!(out.rules[0].text, "always X");
    assert_eq!(out.rules[1].pattern.as_ref().unwrap().value, "lodash");
}

#[test]
fn normalize_bounds_the_count_and_the_length() {
    let twenty: Vec<_> = (0..MAX_RULES).map(|i| rule(&format!("rule {i}"))).collect();
    assert!(normalize(&rules(twenty.clone())).is_ok());
    let mut twenty_one = twenty;
    twenty_one.push(rule("one too many"));
    assert!(normalize(&rules(twenty_one))
        .unwrap_err()
        .contains("at most 20"));

    let max = "x".repeat(TEXT_MAX_CHARS);
    assert!(normalize(&rules(vec![rule(&max)])).is_ok());
    let over = "x".repeat(TEXT_MAX_CHARS + 1);
    assert!(normalize(&rules(vec![rule(&over)])).is_err());
}

#[test]
fn normalize_refuses_unsafe_text_and_patterns() {
    for bad in ["two\nlines", "{{X}}", "a <!-- b"] {
        assert!(normalize(&rules(vec![rule(bad)])).is_err(), "{bad}");
    }
    let refused = [
        (ProjectRulePatternKind::Dependency, "lo dash"),
        (ProjectRulePatternKind::Dependency, "a`b"),
        (ProjectRulePatternKind::Import, "a;b"),
        (ProjectRulePatternKind::FileGlob, "/etc/*"),
        (ProjectRulePatternKind::FileGlob, "src/../x"),
        (ProjectRulePatternKind::FileGlob, "[ab].js"),
        (ProjectRulePatternKind::FileGlob, "*a*b*c*d"),
        (ProjectRulePatternKind::FileGlob, "  "),
    ];
    for (kind, value) in refused {
        assert!(
            normalize(&rules(vec![ban("never", Some((kind, value)))])).is_err(),
            "{value}"
        );
    }
    let p = "x".repeat(PATTERN_MAX_CHARS + 1);
    assert!(normalize(&rules(vec![ban(
        "never",
        Some((ProjectRulePatternKind::Dependency, &p))
    )]))
    .is_err());
    for (kind, value) in [
        (ProjectRulePatternKind::Dependency, "@scope/pkg"),
        (ProjectRulePatternKind::Import, "App\\Legacy\\Mailer"),
        (ProjectRulePatternKind::FileGlob, "src/**/*.min.js"),
    ] {
        assert!(
            normalize(&rules(vec![ban("never", Some((kind, value)))])).is_ok(),
            "{value}"
        );
    }
}

#[test]
fn only_a_ban_carries_a_pattern_and_it_needs_its_text() {
    let mut with_pattern = rule("always lodash");
    with_pattern.pattern = Some(ProjectRulePattern {
        kind: ProjectRulePatternKind::Dependency,
        value: "lodash".into(),
    });
    assert!(normalize(&rules(vec![with_pattern])).is_err());
    assert!(normalize(&rules(vec![ban(
        " ",
        Some((ProjectRulePatternKind::Dependency, "lodash"))
    )]))
    .is_err());
}

// ─── Briefing section ───────────────────────────────────────────────────────

#[test]
fn section_renders_lines_and_a_machine_copy_that_round_trips() {
    let section = render_section(&sample()).unwrap();
    assert!(section.starts_with("## Project rules\n"));
    assert!(section.contains("- Always: toujours passer par le service X\n"));
    assert!(section
        .contains("- Never: ne jamais utiliser lodash — checked by Kronn: dependency `lodash`\n"));
    assert_eq!(parse(&section), Some(sample()));
    assert!(render_section(&BriefingProjectRules::default()).is_none());
}

#[test]
fn a_value_never_closes_the_machine_comment() {
    let r = rules(vec![rule("a --> b <c>")]);
    let section = render_section(&r).unwrap();
    let machine = section.lines().last().unwrap();
    assert_eq!(machine.matches("-->").count(), 1, "{machine}");
    assert_eq!(parse(&section), Some(r));
}

/// A briefing written before KT-1193, composed the way the form writes it.
fn old_briefing() -> String {
    let facts = super::super::project_doc::render_facts_section(&BriefingProjectAnswers {
        tracker: BriefingTracker::Jira,
        ..Default::default()
    })
    .unwrap();
    let body = format!("# Project Briefing\n## Purpose\nA shop.\n\n{facts}");
    super::super::briefing::with_kronn_options(body, true)
}

#[test]
fn an_old_briefing_round_trips_byte_identical() {
    use super::super::project_doc::reattach_facts;
    let old = old_briefing();
    assert!(old.contains("## Kronn options"));
    assert_eq!(reattach_facts(Some(&old), &old), old);
    let reviewed = "# Project Briefing\n## Purpose\nA shop, reviewed.\n";
    assert_eq!(reattach_facts(None, reviewed), reviewed);
    assert_eq!(strip_section(&old), old);
    assert!(parse(&old).is_none());
    // A plain six-question briefing stays untouched too.
    let plain = "# Project Briefing\n## Purpose\nA shop.\n";
    assert_eq!(reattach_facts(Some(plain), plain), plain);
}

#[test]
fn reattach_restores_the_rules_after_a_review_before_kronn_options() {
    use super::super::project_doc::{reattach_facts, render_facts_section};
    let facts = render_facts_section(&BriefingProjectAnswers {
        tracker: BriefingTracker::Jira,
        ..Default::default()
    })
    .unwrap();
    let docs = super::super::briefing_docs::render_section(&BriefingDocsAnswers {
        locations: vec![crate::models::BriefingDocLocation {
            path: "README.md".into(),
            kind: crate::models::BriefingDocKind::Readme,
            ..Default::default()
        }],
    })
    .unwrap();
    let section = render_section(&sample()).unwrap();
    let previous = format!(
        "# Project Briefing\n## Purpose\nA shop.\n\n{facts}\n{docs}\n{section}\n## Kronn options\n- td\n"
    );
    // The reviewer dropped the rules and garbled the machine copy.
    let reviewed = "# Project Briefing\n## Purpose\nA shop, clarified.\n\n## Project rules\n- whatever\n<!-- kronn:project-rules v1 {broken -->\n\n## Kronn options\n- td\n";
    let out = reattach_facts(Some(&previous), reviewed);
    assert_eq!(parse(&out), Some(sample()));
    assert_eq!(out.matches(RULES_HEADING).count(), 1);
    assert!(!out.contains("whatever"));
    let rules_at = out.find(RULES_HEADING).unwrap();
    assert!(out.find("## Existing documentation").unwrap() < rules_at);
    assert!(rules_at < out.find("## Kronn options").unwrap());
    assert!(out.contains("A shop, clarified."));
}

#[test]
fn the_shared_briefing_prompt_leaves_the_rules_out() {
    let notes = format!(
        "# Project Briefing\n## Purpose\nA shop.\n\n{}",
        render_section(&sample()).unwrap()
    );
    let prompt = super::super::briefing::briefing_prompt_section(&notes);
    assert!(prompt.contains("A shop."));
    assert!(!prompt.contains("lodash"), "{prompt}");
    assert!(!prompt.contains(RULES_ANSWERS_PREFIX));
}

// ─── Audit prompt ───────────────────────────────────────────────────────────

#[test]
fn the_prompt_block_goes_to_code_judging_steps_only() {
    let r = sample();
    for target in [
        "docs/inconsistencies-tech-debt.md",
        "docs/inconsistencies-security.md",
        "docs/inconsistencies-code-quality.md",
    ] {
        let block = prompt_block(target, Some(&r));
        assert!(block.contains("## Project rules and bans"), "{target}");
        assert!(block.contains("Never: ne jamais utiliser lodash"));
        assert!(block.contains("TD with its proof (`file:line`)"));
        assert!(block.contains("Never propose a fix that breaks a ban"));
    }
    for target in [
        "docs/AGENTS.md",
        "docs/coding-rules.md",
        "docs/glossary.md",
        "docs/decisions.md",
        "REVIEW",
    ] {
        assert!(prompt_block(target, Some(&r)).is_empty(), "{target}");
    }
    assert!(prompt_block("docs/inconsistencies-tech-debt.md", None).is_empty());
    assert!(prompt_block(
        "docs/inconsistencies-tech-debt.md",
        Some(&BriefingProjectRules::default())
    )
    .is_empty());
}

#[test]
fn every_chained_finding_step_receives_the_block() {
    let chain = super::super::assemble_chained_steps(crate::models::AuditKind::Full);
    let judged: Vec<_> = chain
        .iter()
        .filter(|s| !prompt_block(s.target_file, Some(&sample())).is_empty())
        .map(|s| s.target_file)
        .collect();
    assert!(judged.contains(&"docs/inconsistencies-tech-debt.md"));
    assert!(judged.contains(&"docs/inconsistencies-security.md"));
    assert!(judged.contains(&"docs/inconsistencies-code-quality.md"));
    assert!(judged
        .iter()
        .all(|t| t.starts_with("docs/inconsistencies-")));
}

#[test]
fn both_pipelines_inject_the_block_and_resync_the_doc() {
    for (pipeline, source) in [
        ("full", include_str!("full.rs")),
        ("partial", include_str!("drift.rs")),
    ] {
        for needle in [
            "briefing_rules::prompt_block(analysis_step.target_file",
            "scan_project_with_rules(",
            "briefing_rules::sync_from_notes(",
        ] {
            assert!(source.contains(needle), "{pipeline} lacks {needle}");
        }
    }
}

// ─── Coding-rules doc ───────────────────────────────────────────────────────

const CODING_RULES: &str = "# Coding rules (AI contract)\n\n> Glossary.\n\n## Global\n\n- Prefer small diffs.\n\n## Rust\n\nHuman text.\n";

#[test]
fn the_doc_section_is_inserted_replaced_and_removed_leaving_the_rest_intact() {
    let with = upsert_doc_section(CODING_RULES, Some(&sample()));
    assert!(with.contains(DOC_SECTION_OPEN));
    assert!(with.contains(
        "- **Never:** ne jamais utiliser lodash (checked by Kronn: dependency `lodash`)"
    ));
    assert!(with.contains("- **Always:** toujours passer par le service X"));
    assert!(with.find(DOC_SECTION_OPEN).unwrap() < with.find("## Global").unwrap());
    // Idempotent.
    assert_eq!(upsert_doc_section(&with, Some(&sample())), with);
    // Replaced in place.
    let fewer = rules(vec![rule("always Y")]);
    let replaced = upsert_doc_section(&with, Some(&fewer));
    assert!(replaced.contains("always Y") && !replaced.contains("lodash"));
    assert_eq!(replaced.matches(DOC_SECTION_OPEN).count(), 1);
    // Removed: back to the human doc byte for byte.
    assert_eq!(upsert_doc_section(&with, None), CODING_RULES);
    assert_eq!(
        upsert_doc_section(&with, Some(&BriefingProjectRules::default())),
        CODING_RULES
    );
    // Nothing to do without rules and without a section.
    assert_eq!(upsert_doc_section(CODING_RULES, None), CODING_RULES);
}

#[test]
fn the_doc_section_is_never_human_owned() {
    let with = upsert_doc_section(CODING_RULES, Some(&sample()));
    assert!(!super::super::anti_hallu_enforce::contains_human_owned_section(&with));
}

#[test]
fn sync_writes_the_coding_rules_doc_in_the_detected_docs_dir() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // A project whose docs live in `doc/`, not `docs/`.
    std::fs::create_dir_all(root.join("doc")).unwrap();
    std::fs::write(root.join("doc/AGENTS.md"), "# A\n").unwrap();
    std::fs::write(root.join("doc/coding-rules.md"), CODING_RULES).unwrap();
    assert!(sync_coding_rules_doc(root, Some(&sample())).unwrap());
    let written = std::fs::read_to_string(root.join("doc/coding-rules.md")).unwrap();
    assert!(written.contains("ne jamais utiliser lodash"));
    assert!(!root.join("docs").exists());
    // Second sync: nothing to rewrite.
    assert!(!sync_coding_rules_doc(root, Some(&sample())).unwrap());
    // From the briefing notes, an agent rewrite that dropped it is repaired.
    std::fs::write(root.join("doc/coding-rules.md"), CODING_RULES).unwrap();
    let notes = render_section(&sample()).unwrap();
    sync_from_notes(root, Some(&notes));
    let repaired = std::fs::read_to_string(root.join("doc/coding-rules.md")).unwrap();
    assert_eq!(repaired, written);
}

#[test]
fn sync_is_a_no_op_without_the_doc() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    assert!(!sync_coding_rules_doc(dir.path(), Some(&sample())).unwrap());
    assert!(!dir.path().join("docs/coding-rules.md").exists());
}
