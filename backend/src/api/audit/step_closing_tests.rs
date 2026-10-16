// KT-1278 — every audit step prompt ends on the closing directive. Over ACP the
// context and the step are ONE user message: a prompt that ended on a reference
// block (versions table, briefing, rules, signals) made Claude (Haiku) ask the
// human what to do instead of running the step.

use super::*;
use crate::core::docs_location::DocsRoot;
use crate::core::version_sources::{
    render_version_block_for, CiRuntime, VersionPurpose, VersionSource,
};
use crate::models::{AuditKind, BriefingProjectRules, ProjectRule, ProjectRuleKind};

const ALL_KINDS: [AuditKind; 10] = [
    AuditKind::Full,
    AuditKind::Drift,
    AuditKind::Security,
    AuditKind::Docker,
    AuditKind::Performance,
    AuditKind::Accessibility,
    AuditKind::Rgaa,
    AuditKind::Database,
    AuditKind::ApiDesign,
    AuditKind::CodeQuality,
];

/// The step prompt as the pipelines assemble it, with every optional
/// reference block present.
fn assembled_with_every_block(
    step_1_based: usize,
    step: &AnalysisStep,
    docs_root: &DocsRoot,
    first_chained_step: usize,
) -> String {
    let gate = gate_for_step(step_1_based, first_chained_step);
    let mut prompt = dated_step_prompt(
        gate,
        &step_prompt_body(step, false),
        chrono::Utc::now(),
        docs_root,
    );
    prompt.push_str(&briefing::briefing_prompt_section(
        "## Briefing\nThe team ships weekly.",
        docs_root,
    ));
    if step.target_file.ends_with("inconsistencies-tech-debt.md") {
        let signals = [crate::core::audit_detectors::DetectedSignal {
            detector_id: "zero-tests",
            dimension: "Maintainability",
            severity: crate::core::audit_detectors::Severity::Medium,
            title: "No test files".into(),
            evidence: "40 source files, 0 test files".into(),
        }];
        prompt.push_str(&step8_context_block(&signals, &[], &[], docs_root));
    }
    let runtimes = [CiRuntime {
        runtime: "node".into(),
        version: "22".into(),
        file: ".nvmrc".into(),
        line: 1,
    }];
    let version_block = |purpose| {
        format!(
            "\n\n{}\n",
            render_version_block_for(purpose, VersionSource::None, None, &runtimes, "2026-10-16")
        )
    };
    if version_context::is_stack_table_step(step.target_file) {
        prompt.push_str(&version_block(VersionPurpose::StackTable));
    } else if version_context::applies_to(step.target_file) {
        prompt.push_str(&version_block(VersionPurpose::Judgement));
    }
    let rules = BriefingProjectRules {
        rules: vec![ProjectRule {
            kind: ProjectRuleKind::Ban,
            text: "never use lodash".into(),
            pattern: None,
        }],
    };
    prompt.push_str(&briefing_rules::prompt_block(
        step.target_file,
        Some(&rules),
    ));
    prompt.push_str(&docs_root.relocate(include_str!("prompts/legacy-docs.md")));
    prompt.push_str("\n\n## Linked repositories\n- `../api` — backend\n");
    prompt
}

#[test]
fn every_step_of_every_kind_ends_on_the_directive_naming_its_target() {
    for rel in ["docs", "knowledge/ai"] {
        let docs_root = DocsRoot::from_rel(rel).expect("a valid docs root");
        for kind in ALL_KINDS {
            let steps = steps_for(kind, &docs_root);
            let first_chained = ANALYSIS_STEPS.len() + 1;
            assert!(!steps.is_empty(), "{kind:?}");
            for (index, step) in steps.iter().enumerate() {
                let n = index + 1;
                let body = assembled_with_every_block(n, step, &docs_root, first_chained);
                let directive = step_closing_directive(n, step, &docs_root);

                for feedback in [None, Some("## Citation feedback\nfix `a.rs:3`")] {
                    let sent = finish_step_prompt(&body, feedback, n, step, &docs_root);
                    assert!(
                        sent.ends_with(&directive),
                        "{kind:?} step {n} ({rel}): the prompt must end on the directive:\n…{}",
                        &sent[sent.len().saturating_sub(400)..]
                    );
                    assert_eq!(
                        sent.matches("=== RUN THIS STEP NOW ===").count(),
                        1,
                        "{kind:?} step {n}: exactly one directive"
                    );
                    // Reference blocks and the retry feedback all come before it.
                    let at = sent.rfind("=== RUN THIS STEP NOW ===").unwrap();
                    for reference in [
                        "Runtime versions pinned",
                        "Linked repositories",
                        "Citation feedback",
                    ] {
                        if let Some(found) = sent.rfind(reference) {
                            assert!(
                                found < at,
                                "{kind:?} step {n}: `{reference}` after the directive"
                            );
                        }
                    }
                }
                if step.target_file != "REVIEW" {
                    assert!(
                        directive.contains(&format!("`{}`", step.target_file)),
                        "{kind:?} step {n}: the directive names `{}`:\n{directive}",
                        step.target_file
                    );
                }
                assert!(
                    directive.contains(&format!("Run step {n} now")),
                    "{directive}"
                );
                assert!(directive.contains("no human"), "{directive}");
            }
        }
    }
}

#[test]
fn the_tech_debt_step_with_every_reference_block_ends_after_the_version_table() {
    // The incident's shape: step 8, a versions table that used to come last.
    let docs_root = DocsRoot::default_root();
    let step = &ANALYSIS_STEPS[7];
    assert!(step.target_file.ends_with("inconsistencies-tech-debt.md"));
    let body = assembled_with_every_block(8, step, &docs_root, ANALYSIS_STEPS.len() + 1);
    assert!(
        body.contains("Runtime versions pinned in the repository"),
        "the fixture must carry the version block"
    );
    let sent = finish_step_prompt(&body, None, 8, step, &docs_root);
    let table = sent
        .find("Runtime versions pinned in the repository")
        .unwrap();
    let directive = sent.find("=== RUN THIS STEP NOW ===").unwrap();
    assert!(table < directive);
    assert!(sent.contains(
        "`docs/inconsistencies-tech-debt.md` and the detail files it lists under `docs/tech-debt/`"
    ));
    assert!(sent.trim_end().ends_with("Stop once the work is written."));
}

#[test]
fn the_directive_follows_the_docs_root() {
    let docs_root = DocsRoot::from_rel("knowledge/ai").unwrap();
    let steps = steps_for(AuditKind::Full, &docs_root);
    let directive = step_closing_directive(8, &steps[7], &docs_root);
    assert!(
        directive.contains("`knowledge/ai/inconsistencies-tech-debt.md`"),
        "{directive}"
    );
    assert!(
        directive.contains("`knowledge/ai/tech-debt/`"),
        "{directive}"
    );
    assert!(!directive.contains("`docs/"), "{directive}");
}
