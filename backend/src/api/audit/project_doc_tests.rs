use std::cell::RefCell;
use std::path::PathBuf;

use super::*;
use crate::models::{BriefingEnvironment, BriefingPerson};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn sample_answers() -> BriefingProjectAnswers {
    BriefingProjectAnswers {
        tracker: BriefingTracker::Jira,
        tracker_key: "kr".into(),
        forge: BriefingForge::Github,
        forge_repo: "DocRoms/Kronn".into(),
        default_branch: BriefingDefaultBranch::Main,
        commit_convention: BriefingCommitConvention::Conventional,
        signoff_required: BriefingYesNo::Yes,
        merge_strategy: BriefingMergeStrategy::Squash,
        review_policy: BriefingReviewPolicy::OneApproval,
        release_cadence: BriefingReleaseCadence::OnDemand,
        environments: vec![
            BriefingEnvironment {
                kind: BriefingEnvironmentKind::Staging,
                url: "https://staging.example.com".into(),
            },
            BriefingEnvironment {
                kind: BriefingEnvironmentKind::Production,
                url: String::new(),
            },
        ],
        people: vec![BriefingPerson {
            role: BriefingPersonRole::Arbiter,
            name: "Alice".into(),
            contact: "alice@example.com".into(),
        }],
    }
}

/// A probe that answers from a fixed table and records every request.
struct MockConnection {
    calls: RefCell<Vec<ProbeRequest>>,
    answer: fn(&ProbeRequest) -> ProbeResponse,
}

impl MockConnection {
    fn new(answer: fn(&ProbeRequest) -> ProbeResponse) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            answer,
        }
    }

    async fn probe(&self, request: ProbeRequest) -> ProbeResponse {
        let response = (self.answer)(&request);
        self.calls.borrow_mut().push(request);
        response
    }
}

fn connections() -> Vec<(String, String)> {
    vec![
        ("mcp-atlassian".into(), "cfg-jira".into()),
        ("mcp-github".into(), "cfg-gh".into()),
    ]
}

// ─── Template ───────────────────────────────────────────────────────────────

#[test]
fn template_v2_ships_project_md_with_its_five_parts_and_is_routed() {
    let root = workspace_root();
    let body = std::fs::read_to_string(root.join("templates/docs/project.md"))
        .expect("templates/docs/project.md must exist");
    assert!(
        body.starts_with(GENERATED_MARKER),
        "the template must carry the generated marker so the audit fills it"
    );
    for heading in [
        "## Tracker",
        "## Forge",
        "## Environments",
        "## People",
        "## Process",
    ] {
        assert!(
            body.contains(heading),
            "project.md template lacks {heading}"
        );
    }
    let agents = std::fs::read_to_string(root.join("templates/docs/AGENTS.md")).unwrap();
    assert!(
        agents.contains("(project.md)"),
        "templates/docs/AGENTS.md must route to project.md (or the audit flags it as an orphan)"
    );
    let budget = crate::core::document_optimization::DocumentBudgets::default();
    assert!(
        agents.split_whitespace().count() <= budget.agents_md_max_words,
        "the AGENTS.md skeleton must stay within its word budget"
    );
}

// ─── Validation ─────────────────────────────────────────────────────────────

#[test]
fn normalize_uppercases_a_jira_key_and_rejects_a_malformed_one() {
    let normalized = normalize_answers(&sample_answers()).unwrap();
    assert_eq!(normalized.tracker_key, "KR");

    let mut bad = sample_answers();
    bad.tracker_key = "K R".into();
    assert!(normalize_answers(&bad).unwrap_err().contains("tracker_key"));

    let mut bad_repo = sample_answers();
    bad_repo.forge_repo = "only-one-segment".into();
    assert!(normalize_answers(&bad_repo)
        .unwrap_err()
        .contains("forge_repo"));

    let mut bad_url = sample_answers();
    bad_url.environments[0].url = "javascript:alert(1)".into();
    assert!(normalize_answers(&bad_url).is_err());

    let mut placeholder = sample_answers();
    placeholder.people[0].name = "{{NAME}}".into();
    assert!(normalize_answers(&placeholder).is_err());
}

#[test]
fn normalize_keeps_one_entry_per_environment_and_role_and_drops_empty_people() {
    let mut answers = sample_answers();
    answers.environments.push(BriefingEnvironment {
        kind: BriefingEnvironmentKind::Staging,
        url: "https://other.example.com".into(),
    });
    answers.people.push(BriefingPerson {
        role: BriefingPersonRole::Arbiter,
        name: "Bob".into(),
        contact: String::new(),
    });
    answers.people.push(BriefingPerson {
        role: BriefingPersonRole::Qa,
        name: "  ".into(),
        contact: String::new(),
    });
    let n = normalize_answers(&answers).unwrap();
    assert_eq!(n.environments.len(), 2);
    assert_eq!(n.environments[0].url, "https://staging.example.com");
    assert_eq!(n.people.len(), 1);
    assert_eq!(n.people[0].name, "Alice");
}

// ─── Briefing section ───────────────────────────────────────────────────────

#[test]
fn facts_section_round_trips_through_the_briefing() {
    let answers = normalize_answers(&sample_answers()).unwrap();
    let section = render_facts_section(&answers).expect("answered cards render a section");
    assert!(section.starts_with(FACTS_HEADING));
    assert!(section.contains("Tracker: Jira — key `KR`"));
    let notes = format!("# Project Briefing\n\n## Purpose\nX\n\n{section}");
    assert_eq!(parse_answers(&notes), Some(answers));
}

#[test]
fn unanswered_cards_leave_the_briefing_unchanged() {
    assert_eq!(
        render_facts_section(&BriefingProjectAnswers::default()),
        None
    );
}

#[test]
fn a_value_cannot_close_the_answers_comment() {
    let mut answers = BriefingProjectAnswers::default();
    answers.people.push(BriefingPerson {
        role: BriefingPersonRole::Qa,
        name: "a --> b".into(),
        contact: String::new(),
    });
    let section = render_facts_section(&answers).unwrap();
    let marker = section.lines().last().unwrap();
    assert_eq!(marker.matches("-->").count(), 1, "{marker}");
    assert_eq!(parse_answers(&section).unwrap().people[0].name, "a --> b");
}

#[test]
fn a_review_rewrite_gets_the_form_facts_back() {
    let answers = normalize_answers(&sample_answers()).unwrap();
    let section = render_facts_section(&answers).unwrap();
    let previous = format!("# Project Briefing\n\n## Purpose\nOld\n\n{section}");
    // The reviewer rewrote the file and garbled the facts.
    let reviewed = "# Project Briefing\n\n## Purpose\nRefined\n\n## Project facts\n- Tracker: Linear\n\n## Additional Context\nNone.\n";
    let merged = reattach_facts(Some(&previous), reviewed);
    assert!(merged.contains("Refined"));
    assert!(merged.contains("## Additional Context\nNone."));
    assert!(!merged.contains("Tracker: Linear"));
    assert_eq!(parse_answers(&merged), Some(answers));
    assert_eq!(merged.matches(FACTS_HEADING).count(), 1);
    // Nothing to restore: the reviewed file is kept as written.
    assert_eq!(reattach_facts(Some("# Old"), reviewed), reviewed);
}

// ─── Prompt cap (AUD-14) ────────────────────────────────────────────────────

#[test]
fn briefing_injected_into_a_step_is_capped_with_a_marker() {
    use super::super::briefing::{briefing_prompt_section, BRIEFING_PROMPT_MAX_CHARS};
    let small = briefing_prompt_section("# Project Briefing\n## Purpose\nShort.\n");
    assert!(small.contains("## Project briefing (from the user)"));
    assert!(small.contains("Short."));
    assert!(!small.contains("truncated"));

    // Multi-byte characters: the cap counts characters and never splits one.
    let line = "é".repeat(99) + "\n";
    let huge = line.repeat(200);
    let total = huge.trim_end().chars().count();
    let capped = briefing_prompt_section(&huge);
    assert!(
        capped.contains(&format!("of {total} characters shown")),
        "{}",
        &capped[capped.len() - 200..]
    );
    assert!(capped.contains("Read docs/briefing.md for the rest."));
    let body_chars = capped.chars().count();
    assert!(
        body_chars <= BRIEFING_PROMPT_MAX_CHARS + 200,
        "capped block is {body_chars} characters"
    );
}

#[test]
fn the_machine_copy_of_the_answers_never_reaches_a_step_prompt() {
    use super::super::briefing::briefing_prompt_section;
    let section = render_facts_section(&normalize_answers(&sample_answers()).unwrap()).unwrap();
    let block = briefing_prompt_section(&format!("# Project Briefing\n\n{section}"));
    assert!(block.contains("Tracker: Jira"));
    assert!(!block.contains("kronn:project-answers"));
}

// ─── Checks ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn without_a_connection_nothing_is_called_and_values_are_not_verified() {
    let answers = normalize_answers(&sample_answers()).unwrap();
    let mock = MockConnection::new(|_| panic!("no connection: no call"));
    let checks = run_checks(&answers, &[], |r| mock.probe(r)).await;
    assert!(mock.calls.borrow().is_empty());
    assert!(matches!(
        checks.tracker_key,
        CheckStatus::NotVerified { .. }
    ));
    assert!(matches!(checks.forge_repo, CheckStatus::NotVerified { .. }));
    assert!(matches!(
        checks.default_branch,
        CheckStatus::NotVerified { .. }
    ));
    assert!(checks.mismatches().is_empty());
}

#[tokio::test]
async fn matching_values_are_verified_with_get_requests_only() {
    let answers = normalize_answers(&sample_answers()).unwrap();
    let mock = MockConnection::new(|r| match r.plugin_id.as_str() {
        "mcp-atlassian" => ProbeResponse::Found(serde_json::json!({"key": "KR"})),
        _ => ProbeResponse::Found(
            serde_json::json!({"full_name": "DocRoms/Kronn", "default_branch": "main"}),
        ),
    });
    let checks = run_checks(&answers, &connections(), |r| mock.probe(r)).await;
    assert!(matches!(checks.tracker_key, CheckStatus::Verified { .. }));
    assert!(matches!(checks.forge_repo, CheckStatus::Verified { .. }));
    assert!(matches!(
        checks.default_branch,
        CheckStatus::Verified { .. }
    ));
    let calls = mock.calls.borrow();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].endpoint, "/rest/api/3/project/{key}");
    assert_eq!(calls[0].path_params["key"], "KR");
    assert_eq!(calls[0].config_id, "cfg-jira");
    assert_eq!(calls[1].endpoint, "/repos/{owner}/{repo}");
    assert_eq!(calls[1].path_params["owner"], "DocRoms");
}

#[tokio::test]
async fn a_mismatch_reported_by_a_connection_is_flagged() {
    let answers = normalize_answers(&sample_answers()).unwrap();
    let mock = MockConnection::new(|r| match r.plugin_id.as_str() {
        "mcp-atlassian" => ProbeResponse::NotFound,
        _ => ProbeResponse::Found(
            serde_json::json!({"full_name": "DocRoms/Kronn-renamed", "default_branch": "develop"}),
        ),
    });
    let checks = run_checks(&answers, &connections(), |r| mock.probe(r)).await;
    assert!(matches!(checks.tracker_key, CheckStatus::Mismatch { .. }));
    assert!(matches!(checks.forge_repo, CheckStatus::Mismatch { .. }));
    assert!(matches!(
        checks.default_branch,
        CheckStatus::Mismatch { .. }
    ));
    let mismatches = checks.mismatches();
    assert_eq!(mismatches.len(), 3, "{mismatches:?}");
    let doc = render_project_doc(&answers, false, &checks, "2026-10-11");
    assert_eq!(doc.matches("**Mismatch**").count(), 3, "{doc}");
    assert!(doc.contains("GitHub reports `develop`, not `main`"));
}

#[tokio::test]
async fn a_failing_connection_proves_nothing() {
    let answers = normalize_answers(&sample_answers()).unwrap();
    let mock = MockConnection::new(|_| ProbeResponse::Failed("HTTP 401 on GET …\nmore".into()));
    let checks = run_checks(&answers, &connections(), |r| mock.probe(r)).await;
    assert!(matches!(
        checks.tracker_key,
        CheckStatus::NotVerified { .. }
    ));
    assert!(matches!(checks.forge_repo, CheckStatus::NotVerified { .. }));
    assert!(checks.mismatches().is_empty());
}

#[tokio::test]
async fn an_open_default_branch_is_filled_from_the_forge() {
    let mut answers = normalize_answers(&sample_answers()).unwrap();
    answers.default_branch = BriefingDefaultBranch::Unknown;
    let mock = MockConnection::new(|r| match r.plugin_id.as_str() {
        "mcp-atlassian" => ProbeResponse::Found(serde_json::json!({"key": "KR"})),
        _ => ProbeResponse::Found(
            serde_json::json!({"full_name": "DocRoms/Kronn", "default_branch": "trunk"}),
        ),
    });
    let checks = run_checks(&answers, &connections(), |r| mock.probe(r)).await;
    assert_eq!(checks.default_branch, CheckStatus::NotApplicable);
    let doc = render_project_doc(&answers, false, &checks, "2026-10-11");
    assert!(
        doc.contains("| Default branch | `trunk` (reported by the forge) |"),
        "{doc}"
    );
}

// ─── docs/project.md ────────────────────────────────────────────────────────

#[tokio::test]
async fn briefing_answers_fill_project_md_deterministically() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let docs = root.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    // The installed template is what the audit overwrites.
    std::fs::copy(
        workspace_root().join("templates/docs/project.md"),
        docs.join("project.md"),
    )
    .unwrap();

    let section = render_facts_section(&normalize_answers(&sample_answers()).unwrap()).unwrap();
    let notes = format!("# Project Briefing\n\n## Purpose\nX\n\n{section}");
    let (answers, inferred) = effective_answers(parse_answers(&notes), None);
    assert!(!inferred);
    let checks = run_checks(&answers, &[], |_| async { ProbeResponse::NotFound }).await;
    let body = render_project_doc(&answers, inferred, &checks, "2026-10-11");
    assert_eq!(
        body,
        render_project_doc(&answers, inferred, &checks, "2026-10-11"),
        "same inputs, same bytes"
    );
    assert_eq!(
        write_project_doc(root, &docs, &body).unwrap(),
        ProjectDocWrite::Written
    );

    let written = std::fs::read_to_string(docs.join("project.md")).unwrap();
    for expected in [
        "| Tool | Jira | — |",
        "| Project key | `KR` | Not verified: no Jira connection configured for this project |",
        "| Repository | `DocRoms/Kronn` |",
        "| Default branch | `main` |",
        "| Commit convention | Conventional Commits | — |",
        "| Sign-off (`Signed-off-by`) | Required | — |",
        "| Staging | https://staging.example.com |",
        "| Production | Not provided — ask before assuming. |",
        "| Arbiter (final call) | Alice | alice@example.com |",
        "| Review | One approval |",
        "| Release cadence | On demand |",
    ] {
        assert!(
            written.contains(expected),
            "missing `{expected}` in:\n{written}"
        );
    }
    assert_eq!(
        crate::api::audit::validation::count_raw_placeholders(&written),
        0,
        "no template placeholder may survive the fill"
    );
}

#[tokio::test]
async fn without_answers_the_page_says_not_provided_and_takes_the_repo_from_the_remote() {
    let (answers, inferred) = effective_answers(None, Some("git@github.com:DocRoms/Kronn.git"));
    assert!(inferred);
    let checks = run_checks(&answers, &[], |_| async { ProbeResponse::NotFound }).await;
    let body = render_project_doc(&answers, inferred, &checks, "2026-10-11");
    assert!(body.contains("| Host | GitHub | — |"));
    assert!(body.contains("`DocRoms/Kronn` (from the git remote)"));
    assert!(body.contains("## People\n\nNot provided — ask before assuming."));
    assert_eq!(
        crate::api::audit::validation::count_raw_placeholders(&body),
        0
    );
}

#[test]
fn a_project_md_without_the_marker_belongs_to_humans() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let docs = root.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("project.md"), "# Project\nHand-written.\n").unwrap();
    assert_eq!(
        write_project_doc(root, &docs, &format!("{GENERATED_MARKER} -->\n# New\n")).unwrap(),
        ProjectDocWrite::KeptHumanOwned
    );
    assert_eq!(
        std::fs::read_to_string(docs.join("project.md")).unwrap(),
        "# Project\nHand-written.\n"
    );
}

#[cfg(unix)]
#[test]
fn project_md_is_never_written_through_a_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("docs")).unwrap();
    assert!(write_project_doc(&root, &root.join("docs"), "x").is_err());
    assert!(!outside.join("project.md").exists());
}
