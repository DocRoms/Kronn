use super::*;

#[test]
fn every_finding_step_and_consolidation_receive_the_shipped_td_schema() {
    let schema = include_str!("../../../../templates/docs/tech-debt/TEMPLATE.md");
    let steps = assemble_chained_steps(crate::models::AuditKind::Full);
    for step in &steps {
        let contract = finding_evidence_block(step);
        if step.target_file.contains("inconsistencies-") || step.target_file == "docs/decisions.md"
        {
            assert!(contract.contains(schema), "{}", step.target_file);
            assert!(contract.contains("actual Markdown link"));
            assert!(contract.contains("latest audit_history"));
            assert!(contract.contains("never positional shell arguments"));
            assert!(contract.split_whitespace().count() < 650);
        } else {
            assert!(contract.is_empty(), "{}", step.target_file);
        }
    }
    for section in [
        "Problem (fact)",
        "Impact",
        "Where (pointers)",
        "Applicability and counter-check",
        "Verification",
        "Suggested direction",
        "Next step",
    ] {
        assert_eq!(schema.matches(&format!("## {section}\n")).count(), 1);
    }
    assert!(schema.contains("status: \"Inferred\""));
    assert!(schema.contains("**Status**: Inferred"));
    assert!(schema.contains("**Effort**: {{EFFORT}}"));
}

#[test]
fn coverage_contract_includes_non_orm_clients_and_all_build_entry_points() {
    let foundation = ANALYSIS_STEPS[7].prompt;
    assert!(ANALYSIS_STEPS[0].prompt.contains("Jenkinsfiles"));
    assert!(foundation.contains("EACH discovered CI/build entry point"));
    assert!(foundation.contains("dynamic/plugin loading"));
    assert!(SECURITY_STEPS[0]
        .prompt
        .contains("Outbound HTTP, search and database clients"));
    assert!(SECURITY_STEPS[0]
        .prompt
        .contains("certificate AND hostname verification"));
    assert!(!SECURITY_STEPS[0]
        .prompt
        .contains("Use the first one whenever you actually read source"));
    assert!(PERFORMANCE_STEPS[0]
        .prompt
        .contains("TTL alone is not a capacity bound"));
    assert!(PERFORMANCE_STEPS[0]
        .prompt
        .contains("timeouts, retries/backoff, cancellation"));
    assert!(CHAINED_STEP_GATE.contains("No ORM does not exclude"));
    assert!(API_DESIGN_STEPS[0].prompt.contains("actual return shape"));
}

#[test]
fn final_review_checks_written_artifacts_instead_of_trusting_coverage_claims() {
    let final_review = ANALYSIS_STEPS.last().unwrap().prompt;
    for requirement in [
        "bare TD ID is not a link",
        "invalid field/history values",
        "rerun the checks",
        "auxiliary workflow/environment documents",
        "Repair audit-created YAML/history fields",
        "800-word budget",
    ] {
        assert!(final_review.contains(requirement), "{requirement}");
    }
    assert!(CODE_QUALITY_STEPS[0]
        .prompt
        .contains("inspection candidates, not an acceptance threshold"));
}
