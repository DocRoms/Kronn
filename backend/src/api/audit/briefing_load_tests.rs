use super::*;
use crate::api::audit::briefing::{compose_briefing, BriefingFormRequest};
use crate::api::audit::{briefing_docs, project_doc};
use crate::models::{
    AgentType, BriefingDocKind, BriefingDocLocation, BriefingDocState, BriefingDocsAnswers,
    BriefingEnvironment, BriefingEnvironmentKind, BriefingFact, BriefingForge, BriefingPerson,
    BriefingPersonRole, BriefingProjectAnswers, BriefingReviewPolicy, BriefingTracker,
};

const ALL_FACTS: [BriefingFact; 12] = [
    BriefingFact::Tracker,
    BriefingFact::TrackerKey,
    BriefingFact::Forge,
    BriefingFact::ForgeRepo,
    BriefingFact::DefaultBranch,
    BriefingFact::CommitConvention,
    BriefingFact::Signoff,
    BriefingFact::MergeStrategy,
    BriefingFact::Review,
    BriefingFact::ReleaseCadence,
    BriefingFact::Environments,
    BriefingFact::People,
];

fn project_answers() -> BriefingProjectAnswers {
    let mut answers = BriefingProjectAnswers {
        tracker: BriefingTracker::Jira,
        tracker_key: "KR".into(),
        forge: BriefingForge::Github,
        forge_repo: "DocRoms/Kronn".into(),
        review_policy: BriefingReviewPolicy::TwoApprovals,
        environments: vec![BriefingEnvironment {
            kind: BriefingEnvironmentKind::Production,
            url: "https://example.com".into(),
        }],
        people: vec![BriefingPerson {
            role: BriefingPersonRole::Qa,
            name: "Sam".into(),
            contact: String::new(),
        }],
        ..BriefingProjectAnswers::default()
    };
    for fact in ALL_FACTS {
        answers
            .notes
            .insert(fact, format!("note for {fact:?} | <b>"));
    }
    answers.notes.insert(
        BriefingFact::Review,
        "2 approbations, CODEOWNERS sur backend/".into(),
    );
    answers
}

fn docs_answers() -> BriefingDocsAnswers {
    BriefingDocsAnswers {
        locations: vec![
            BriefingDocLocation {
                path: "README.md".into(),
                kind: BriefingDocKind::Readme,
                state: BriefingDocState::Reference,
                note: "setup".into(),
                source_of_truth: true,
                last_commit: Some("2024-03-02".into()),
            },
            BriefingDocLocation {
                path: "docs/adr".into(),
                kind: BriefingDocKind::Adr,
                state: BriefingDocState::Partial,
                ..BriefingDocLocation::default()
            },
        ],
    }
}

fn full_form() -> BriefingFormRequest {
    BriefingFormRequest {
        purpose: "News site.\n\nSecond paragraph with ### a subheading".into(),
        team: "Four devs".into(),
        maturity: "Production".into(),
        dependencies: "- Jira\n- Fastly".into(),
        traps: "Cache purge".into(),
        additional: "Ask Sam first.".into(),
        project: Some(project_answers()),
        td_to_tasks: true,
        cross_audit_agents: vec![AgentType::ClaudeCode, AgentType::Codex],
        existing_docs: Some(docs_answers()),
        project_rules: None,
    }
}

/// What the save handler writes for `form`; the documentation section is
/// rendered as already dated, like a re-save whose git history did not move.
fn save(form: &BriefingFormRequest) -> String {
    let facts = form
        .project
        .as_ref()
        .map(|p| project_doc::normalize_answers(p).unwrap())
        .and_then(|p| project_doc::render_facts_section(&p));
    let docs = form
        .existing_docs
        .as_ref()
        .and_then(briefing_docs::render_section);
    compose_briefing(form, facts, docs, None)
}

/// The form a client sends back after loading `notes` and changing nothing.
fn reload(notes: &str) -> BriefingFormRequest {
    let saved = parse_saved_briefing(notes);
    BriefingFormRequest {
        purpose: saved.purpose,
        team: saved.team,
        maturity: saved.maturity,
        dependencies: saved.dependencies,
        traps: saved.traps,
        additional: saved.additional,
        project: saved.project,
        td_to_tasks: saved.td_to_tasks,
        cross_audit_agents: saved.cross_audit_agents,
        existing_docs: saved.existing_docs,
        project_rules: None,
    }
}

#[test]
fn every_section_of_a_saved_briefing_is_read_back() {
    let form = full_form();
    let saved = parse_saved_briefing(&save(&form));
    assert_eq!(saved.purpose, form.purpose);
    assert_eq!(saved.team, "Four devs");
    assert_eq!(saved.maturity, "Production");
    assert_eq!(saved.dependencies, "- Jira\n- Fastly");
    assert_eq!(saved.traps, "Cache purge");
    assert_eq!(saved.additional, "Ask Sam first.");
    let project = saved.project.expect("project facts");
    assert_eq!(project, project_answers());
    assert_eq!(project.notes.len(), 12);
    assert_eq!(saved.existing_docs, Some(docs_answers()));
    assert!(saved.td_to_tasks);
    assert_eq!(
        saved.cross_audit_agents,
        vec![AgentType::ClaudeCode, AgentType::Codex]
    );
}

#[test]
fn saving_again_unchanged_rewrites_the_same_bytes() {
    let mut minimal = full_form();
    minimal.additional.clear();
    minimal.project = None;
    minimal.existing_docs = None;
    minimal.td_to_tasks = false;
    minimal.cross_audit_agents.clear();
    for form in [full_form(), minimal] {
        let first = save(&form);
        let second = save(&reload(&first));
        assert_eq!(second, first);
        // And it stays stable.
        assert_eq!(save(&reload(&second)), first);
    }
}

#[test]
fn an_empty_optional_question_reads_back_empty() {
    let mut form = full_form();
    form.additional.clear();
    let notes = save(&form);
    assert!(notes.contains("## Additional Context\nNone.\n"));
    assert_eq!(parse_saved_briefing(&notes).additional, "");
}

#[test]
fn an_older_briefing_without_notes_parses_and_round_trips() {
    // Written by 0.15.0: the machine copy has no `notes` key.
    let old = "# Project Briefing\n\
        > Auto-generated by Kronn briefing form (no AI call). Source: user answers.\n\n\
        ## Purpose\nP\n\n## Team\nT\n\n## Maturity\nM\n\n## External Dependencies\nD\n\n\
        ## Traps & Fragile Areas\nX\n\n## Additional Context\nNone.\n\n\
        ## Project facts\n\
        - Tracker: Linear\n\
        - Forge: not provided\n\
        - Commits: not provided; sign-off: not provided; merge: not provided\n\
        - Review: not provided; releases: not provided\n\
        <!-- kronn:project-answers v1 {\"tracker\":\"linear\",\"tracker_key\":\"\",\"forge\":\"unknown\",\"forge_repo\":\"\",\"default_branch\":\"unknown\",\"commit_convention\":\"unknown\",\"signoff_required\":\"unknown\",\"merge_strategy\":\"unknown\",\"review_policy\":\"unknown\",\"release_cadence\":\"unknown\",\"environments\":[],\"people\":[]} -->\n";
    let saved = parse_saved_briefing(old);
    let project = saved.project.clone().expect("old machine copy parses");
    assert_eq!(project.tracker, BriefingTracker::Linear);
    assert!(project.notes.is_empty());
    assert_eq!(save(&reload(old)), old);
}

#[test]
fn a_section_the_form_does_not_know_survives_a_resave() {
    let mut form = full_form();
    form.project = None;
    form.existing_docs = None;
    let notes = save(&form).replace(
        "## Additional Context\nAsk Sam first.\n",
        "## Additional Context\nAsk Sam first.\n\n## Clarifications\nFrom the review.\n",
    );
    let saved = parse_saved_briefing(&notes);
    assert_eq!(
        saved.additional,
        "Ask Sam first.\n\n## Clarifications\nFrom the review."
    );
    assert_eq!(saved.purpose, form.purpose);
    assert_eq!(save(&reload(&notes)), notes);
}

#[test]
fn malformed_or_missing_parts_are_left_empty() {
    let notes = "Free text, no heading\n\n## Team\nOnly the team\n\n\
        ## Project facts\n<!-- kronn:project-answers v1 {not json} -->\n\
        <!-- kronn:existing-docs v1 [1,2] -->\n\
        ## Kronn options\n- Cross-agent audit (experimental): NoSuchAgent\n";
    let saved = parse_saved_briefing(notes);
    assert_eq!(saved.purpose, "");
    assert_eq!(saved.team, "Only the team");
    assert_eq!(saved.project, None);
    assert_eq!(saved.existing_docs, None);
    assert!(saved.cross_audit_agents.is_empty());
    assert!(!saved.td_to_tasks);
    assert_eq!(parse_saved_briefing(""), SavedBriefing::default());
}
