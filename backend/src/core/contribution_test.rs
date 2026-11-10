use super::*;
use crate::models::{AiAuditStatus, AiConfigStatus};

const REPO: &str = "acme/widgets";

fn project(id: &str, name: &str, repo_url: Option<&str>) -> Project {
    Project {
        id: id.into(),
        name: name.into(),
        path: format!("/work/{name}"),
        repo_url: repo_url.map(String::from),
        token_override: None,
        ai_config: AiConfigStatus {
            detected: false,
            configs: vec![],
        },
        audit_status: AiAuditStatus::NoTemplate,
        ai_todo_count: 0,
        tech_debt_count: 0,
        needs_docs_migration: false,
        path_exists: true,
        write_access: None,
        mcp_sync_report: None,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        workspace: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

fn no_remotes(_: &Project) -> Vec<String> {
    Vec::new()
}

fn all_good() -> Facts {
    Facts {
        gh_installed: true,
        gh_auth: Some(GhAuth {
            login: Some("octo".into()),
            scopes: Some(vec!["repo".into(), "workflow".into()]),
        }),
        can_push: Some(true),
        fork_exists: false,
        project: Some(RepoProjectFact::Linked {
            id: "p1".into(),
            name: "widgets".into(),
            path_exists: true,
        }),
        local_workers: vec![AgentType::ClaudeCode],
    }
}

fn failing(report: &PreflightReport) -> Vec<&str> {
    report
        .checks
        .iter()
        .filter(|c| !c.ok)
        .map(|c| c.id.as_str())
        .collect()
}

// ─── DoD 1: nothing hard-coded ──────────────────────────────────────────────

#[test]
fn the_kronn_url_comes_from_the_runtime_not_from_a_literal() {
    assert_eq!(kronn_url(None, 23140), "http://127.0.0.1:23140");
    assert_eq!(kronn_url(Some(""), 4001), "http://127.0.0.1:4001");
    assert_eq!(
        kronn_url(Some(" http://kronn.internal:8080/ "), 3140),
        "http://kronn.internal:8080"
    );
}

#[test]
fn the_project_is_found_by_its_remote_whatever_its_id_or_name() {
    // A second instance: other ids, other names, the repository's remote.
    let projects = vec![
        project(
            "zz-9",
            "something-else",
            Some("https://github.com/other/repo.git"),
        ),
        project("zz-10", "my-clone", Some("git@github.com:Acme/Widgets.git")),
    ];
    match find_project_by_repo(&projects, REPO, &no_remotes) {
        RepoProject::Found(found) => assert_eq!(found.id, "zz-10"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_project_is_found_by_a_checkout_remote_when_its_repo_url_is_empty() {
    let projects = vec![project("a", "clone", None)];
    let remotes = |_: &Project| vec!["https://github.com/acme/widgets".to_string()];
    assert!(matches!(
        find_project_by_repo(&projects, REPO, &remotes),
        RepoProject::Found(_)
    ));
}

#[test]
fn a_missing_or_ambiguous_repository_is_never_guessed() {
    let projects = vec![
        project("a", "one", Some("https://github.com/acme/widgets")),
        project("b", "two", Some("https://github.com/acme/widgets.git")),
    ];
    match find_project_by_repo(&projects, REPO, &no_remotes) {
        RepoProject::Ambiguous(names) => assert_eq!(names, vec!["one", "two"]),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        find_project_by_repo(&projects, "acme/other", &no_remotes),
        RepoProject::Missing
    ));
    assert!(matches!(
        find_project_by_repo(&projects, "not-a-slug", &no_remotes),
        RepoProject::Missing
    ));
}

#[test]
fn a_slug_is_normalized_for_comparison_and_refused_when_malformed() {
    assert_eq!(
        normalize_slug(" Acme/Widgets.git "),
        Some("acme/widgets".into())
    );
    for bad in ["", "acme", "acme/", "/x", "a/b/c", "a b/c", "a/b;rm"] {
        assert_eq!(normalize_slug(bad), None, "{bad:?}");
    }
}

#[test]
fn the_workflow_defaults_are_ten_rounds_on_main() {
    assert_eq!(DEFAULT_MAX_REVIEW_ROUNDS, 10);
    assert_eq!(DEFAULT_TARGET_BRANCH, "main");
}

// ─── DoD 2: a fork for a contributor without push rights ────────────────────

#[test]
fn a_maintainer_pushes_to_the_repository_and_opens_the_pull_request_from_the_branch() {
    let plan = plan_publication(REPO, "main", "octo", true, false, "autocode/kt-1");
    assert_eq!(plan.mode, PublicationMode::Maintainer);
    assert_eq!(plan.push_repo, REPO);
    assert_eq!(plan.pr_head, "autocode/kt-1");
    assert!(!plan.needs_fork);
}

#[test]
fn a_contributor_pushes_to_their_fork_and_the_pull_request_targets_the_repository() {
    let plan = plan_publication(REPO, "main", "octo", false, false, "autocode/kt-1");
    assert_eq!(plan.mode, PublicationMode::Fork);
    assert_eq!(plan.base_repo, REPO);
    assert_eq!(plan.base_branch, "main");
    assert_eq!(plan.push_repo, "octo/widgets");
    assert_eq!(plan.pr_head, "octo:autocode/kt-1");
    assert!(plan.needs_fork, "no fork yet: create it first");

    let again = plan_publication(REPO, "main", "octo", false, true, "autocode/kt-1");
    assert!(!again.needs_fork, "an existing fork is reused");
}

#[test]
fn the_preflight_reports_the_publication_it_resolved() {
    let mut facts = all_good();
    facts.can_push = Some(false);
    let report = evaluate(REPO, "main", 10, "autocode/kt-1", &facts);
    assert!(report.ok, "{:?}", report.checks);
    let plan = report.publication.expect("a plan");
    assert_eq!(plan.mode, PublicationMode::Fork);
    let access = report
        .checks
        .iter()
        .find(|c| c.id == "repo_access")
        .unwrap();
    assert!(
        access.message.contains("fork octo/widgets"),
        "{}",
        access.message
    );
    assert_eq!(report.project_id.as_deref(), Some("p1"));
}

// ─── DoD 3: prerequisites, with an actionable message ───────────────────────

#[test]
fn a_ready_machine_passes_every_check() {
    let report = evaluate(REPO, "main", 10, "b", &all_good());
    assert!(report.ok, "{:?}", report.checks);
    assert_eq!(report.checks.len(), 6);
    assert_eq!(report.max_review_rounds, 10);
    assert_eq!(report.target_branch, "main");
}

#[test]
fn a_machine_without_gh_is_told_to_install_it_and_nothing_else_is_blamed() {
    let mut facts = all_good();
    facts.gh_installed = false;
    facts.gh_auth = None;
    facts.can_push = None;
    let report = evaluate(REPO, "main", 10, "b", &facts);
    assert!(!report.ok);
    assert!(failing(&report).contains(&"gh_installed"));
    let msg = &report.checks[0].message;
    assert!(
        msg.contains("cli.github.com") && msg.contains("gh auth login"),
        "{msg}"
    );
    assert!(report.publication.is_none());
}

#[test]
fn a_logged_out_gh_is_told_to_log_in() {
    let mut facts = all_good();
    facts.gh_auth = None;
    facts.can_push = None;
    let report = evaluate(REPO, "main", 10, "b", &facts);
    assert!(failing(&report).contains(&"gh_auth"));
    let auth = report.checks.iter().find(|c| c.id == "gh_auth").unwrap();
    assert!(auth.message.contains("gh auth login"));
}

#[test]
fn a_token_without_the_repo_scope_is_told_the_refresh_command() {
    let mut facts = all_good();
    facts.gh_auth = Some(GhAuth {
        login: Some("octo".into()),
        scopes: Some(vec!["gist".into(), "read:org".into()]),
    });
    let report = evaluate(REPO, "main", 10, "b", &facts);
    assert_eq!(failing(&report), vec!["gh_scopes"]);
    let scopes = report.checks.iter().find(|c| c.id == "gh_scopes").unwrap();
    assert!(scopes.message.contains("gh auth refresh -s repo"));
}

#[test]
fn a_token_whose_scopes_gh_does_not_list_is_not_refused() {
    let mut facts = all_good();
    facts.gh_auth = Some(GhAuth {
        login: Some("octo".into()),
        scopes: None,
    });
    assert!(evaluate(REPO, "main", 10, "b", &facts).ok);
}

#[test]
fn a_missing_clone_is_told_to_clone_and_link_it() {
    let mut facts = all_good();
    facts.project = Some(RepoProjectFact::Missing);
    let report = evaluate(REPO, "main", 10, "b", &facts);
    assert_eq!(failing(&report), vec!["project"]);
    let project = report.checks.iter().find(|c| c.id == "project").unwrap();
    assert!(
        project
            .message
            .contains("git clone https://github.com/acme/widgets"),
        "{}",
        project.message
    );
    assert!(report.project_id.is_none());

    facts.project = Some(RepoProjectFact::Linked {
        id: "p1".into(),
        name: "widgets".into(),
        path_exists: false,
    });
    let moved = evaluate(REPO, "main", 10, "b", &facts);
    assert_eq!(failing(&moved), vec!["project"]);
    assert!(moved.project_id.is_none());
}

#[test]
fn no_local_worker_is_told_to_install_an_agent() {
    let mut facts = all_good();
    facts.local_workers.clear();
    let report = evaluate(REPO, "main", 10, "b", &facts);
    assert_eq!(failing(&report), vec!["worker"]);
    assert!(report
        .checks
        .last()
        .unwrap()
        .message
        .contains("Settings > Agents"));
}

#[test]
fn an_unreadable_repository_is_named() {
    let mut facts = all_good();
    facts.can_push = None;
    let report = evaluate(REPO, "main", 10, "b", &facts);
    assert_eq!(failing(&report), vec!["repo_access"]);
    assert!(report.publication.is_none());
}

// ─── DoD 5: the reviewer is named at creation ───────────────────────────────

fn remote(agent: AgentType, model: &str) -> (AgentType, Option<String>) {
    (agent, Some(model.to_string()))
}

#[test]
fn the_default_reviewer_is_the_first_configured_remote_model() {
    let reviewer = resolve_reviewer(None, &[remote(AgentType::LiteLlm, "gpt-x")]).unwrap();
    assert_eq!(reviewer.kind, CampaignReviewerKind::RemoteModel);
    assert_eq!(reviewer.agent, Some(AgentType::LiteLlm));
    assert_eq!(reviewer.model.as_deref(), Some("gpt-x"));
    assert!(reviewer.defaulted);
}

#[test]
fn without_a_remote_model_the_default_is_the_human_never_an_unserved_agent() {
    let reviewer = resolve_reviewer(None, &[]).unwrap();
    assert_eq!(reviewer.kind, CampaignReviewerKind::Human);
    assert_eq!(reviewer.agent, None);
    assert!(reviewer.defaulted);
}

#[test]
fn a_named_reviewer_is_kept_and_not_marked_defaulted() {
    let human = CampaignReviewer {
        kind: CampaignReviewerKind::Human,
        agent: Some(AgentType::ClaudeCode),
        model: Some("stale".into()),
        connection_id: None,
        defaulted: true,
    };
    let resolved = resolve_reviewer(Some(human), &[remote(AgentType::LiteLlm, "m")]).unwrap();
    assert_eq!(resolved.kind, CampaignReviewerKind::Human);
    assert_eq!(resolved.agent, None, "a human reviewer carries no agent");
    assert!(!resolved.defaulted);

    let agent = CampaignReviewer {
        kind: CampaignReviewerKind::Agent,
        agent: Some(AgentType::ClaudeCode),
        model: Some("opus".into()),
        connection_id: None,
        defaulted: true,
    };
    let resolved = resolve_reviewer(Some(agent), &[]).unwrap();
    assert_eq!(resolved.agent, Some(AgentType::ClaudeCode));
    assert!(!resolved.defaulted);
}

#[test]
fn an_agent_reviewer_needs_a_configured_model() {
    let reviewer = |agent, model: Option<&str>| CampaignReviewer {
        kind: CampaignReviewerKind::Agent,
        agent,
        model: model.map(String::from),
        connection_id: None,
        defaulted: false,
    };
    assert!(resolve_reviewer(Some(reviewer(Some(AgentType::ClaudeCode), None)), &[]).is_err());
    assert!(
        resolve_reviewer(Some(reviewer(Some(AgentType::ClaudeCode), Some("  "))), &[]).is_err()
    );
    assert!(resolve_reviewer(Some(reviewer(None, Some("opus"))), &[]).is_err());
    assert!(resolve_reviewer(Some(reviewer(Some(AgentType::LiteLlm), Some("m"))), &[]).is_err());
}

#[test]
fn a_remote_reviewer_needs_an_http_provider_and_a_custom_one_its_connection() {
    let reviewer = |agent, connection: Option<&str>| CampaignReviewer {
        kind: CampaignReviewerKind::RemoteModel,
        agent,
        model: None,
        connection_id: connection.map(String::from),
        defaulted: false,
    };
    assert!(resolve_reviewer(Some(reviewer(Some(AgentType::Nvidia), None)), &[]).is_ok());
    assert!(resolve_reviewer(Some(reviewer(Some(AgentType::ClaudeCode), None)), &[]).is_err());
    assert!(resolve_reviewer(Some(reviewer(None, None)), &[]).is_err());
    assert!(resolve_reviewer(Some(reviewer(Some(AgentType::Custom), None)), &[]).is_err());
    assert!(resolve_reviewer(Some(reviewer(Some(AgentType::Custom), Some("c1"))), &[]).is_ok());
}

#[test]
fn the_room_announcement_names_the_branch_the_rounds_and_the_reviewer() {
    let reviewer = resolve_reviewer(None, &[remote(AgentType::LiteLlm, "gpt-x")]).unwrap();
    let text = room_announcement("main", "10", &reviewer);
    assert!(
        text.contains("`main`") && text.contains("10 review round"),
        "{text}"
    );
    assert!(
        text.contains("remote model") && text.contains("gpt-x"),
        "{text}"
    );
    assert!(text.contains("default"), "{text}");

    let human = resolve_reviewer(None, &[]).unwrap();
    let text = room_announcement("main", "10", &human);
    assert!(
        text.contains("**you**") && text.contains("review card"),
        "{text}"
    );
}

#[test]
fn the_token_scopes_are_read_from_gh_auth_status() {
    let status = "github.com\n  - Active account: true\n  - Token: gho_****\n  - Token scopes: 'gist', 'read:org', 'repo', 'workflow'\n";
    assert_eq!(
        parse_token_scopes(status),
        Some(vec![
            "gist".into(),
            "read:org".into(),
            "repo".into(),
            "workflow".into()
        ])
    );
    assert_eq!(parse_token_scopes("Logged in, fine-grained token"), None);
}
