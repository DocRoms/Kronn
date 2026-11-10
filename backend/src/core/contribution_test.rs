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

fn push(repo: &str, branch: &str) -> PublicationStep {
    PublicationStep::Push {
        repo: repo.into(),
        branch: branch.into(),
    }
}

fn pull_request(head: &str) -> PublicationStep {
    PublicationStep::OpenPullRequest {
        repo: REPO.into(),
        base: "main".into(),
        head: head.into(),
    }
}

/// The dry run of a publication: the ordered actions the publish step carries
/// out, read without touching `gh` or git.
#[test]
fn the_dry_run_of_a_maintainer_publication_pushes_then_opens_the_pull_request() {
    let mut facts = all_good();
    facts.can_push = Some(true);
    let plan = evaluate(REPO, "main", 10, "autocode/issue-7", &facts)
        .publication
        .unwrap();
    assert_eq!(
        plan.steps,
        vec![
            push(REPO, "autocode/issue-7"),
            pull_request("autocode/issue-7")
        ]
    );
}

#[test]
fn the_dry_run_of_a_fork_publication_forks_pushes_to_the_fork_and_targets_the_repository() {
    let mut facts = all_good();
    facts.can_push = Some(false);
    facts.fork_exists = false;
    let plan = evaluate(REPO, "main", 10, "autocode/issue-7", &facts)
        .publication
        .unwrap();
    assert_eq!(
        plan.steps,
        vec![
            PublicationStep::ForkRepo { repo: REPO.into() },
            push("octo/widgets", "autocode/issue-7"),
            pull_request("octo:autocode/issue-7"),
        ]
    );

    facts.fork_exists = true;
    let reused = evaluate(REPO, "main", 10, "autocode/issue-7", &facts)
        .publication
        .unwrap();
    assert_eq!(
        reused.steps,
        vec![
            push("octo/widgets", "autocode/issue-7"),
            pull_request("octo:autocode/issue-7"),
        ],
        "an existing fork is reused, not created again"
    );
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

// ─── python3, the interpreter of the shipped steps ──────────────────────────

fn executable(dir: &std::path::Path, name: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let file = dir.join(name);
    std::fs::write(&file, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn python3_resolves_only_as_an_executable_file_of_the_path() {
    let with = tempfile::tempdir().unwrap();
    let without = tempfile::tempdir().unwrap();
    executable(with.path(), "python3", 0o755);
    executable(without.path(), "python", 0o755); // `python` is not `python3`
    executable(without.path(), "python3x", 0o755);
    let joined = std::env::join_paths([without.path(), with.path()]).unwrap();

    assert!(resolves_on_path("python3", Some(joined.as_os_str())));
    assert!(!resolves_on_path(
        "python3",
        Some(without.path().as_os_str())
    ));
    assert!(!resolves_on_path("python3", None));
    assert!(python_refusal(Some(joined.as_os_str())).is_none());

    // A file that is not executable does not resolve.
    let plain = tempfile::tempdir().unwrap();
    executable(plain.path(), "python3", 0o644);
    assert!(!resolves_on_path("python3", Some(plain.path().as_os_str())));
}

#[test]
fn the_python3_message_names_what_to_install_for_each_platform() {
    let message = python_refusal(None).expect("no PATH, no python3");
    assert_eq!(message, python_missing_message());
    for needle in [
        "python3 --version",
        "https://www.python.org/downloads/",
        "brew install python3",
        "apt install python3",
        "Windows",
        "`python` and `py`",
        "Microsoft Store",
        "restart Kronn",
    ] {
        assert!(message.contains(needle), "missing `{needle}`: {message}");
    }
}

// ─── The reviewer the preflight announces, and how to override it ──────────

#[test]
fn the_preflight_says_which_remote_model_it_picked_and_how_to_change_it() {
    let report = evaluate(REPO, "main", 10, "b", &all_good());
    let reviewer = suggested_reviewer(&[remote(AgentType::LiteLlm, "gpt-x")]);
    let report = report.with_suggested_reviewer(reviewer.clone());
    assert!(report.ok, "an informational line never fails the preflight");
    assert_eq!(report.suggested_reviewer, reviewer);
    let line = report.checks.iter().find(|c| c.id == "reviewer").unwrap();
    assert!(line.ok);
    for needle in [
        "LiteLlm",
        "gpt-x",
        "first remote model configured",
        "`reviewer` variable",
        "`human`",
        "`agent:<Agent>:<model>`",
    ] {
        assert!(line.message.contains(needle), "{needle}: {}", line.message);
    }
}

#[test]
fn the_preflight_says_so_when_no_remote_model_is_configured() {
    let report = evaluate(REPO, "main", 10, "b", &all_good()).with_suggested_reviewer(None);
    assert!(report.ok && report.suggested_reviewer.is_none());
    let line = report.checks.iter().find(|c| c.id == "reviewer").unwrap();
    assert!(line.message.contains("No remote model is configured"));
    assert!(line.message.contains("review card"));
    assert!(line.message.contains("`reviewer` variable"));
}

#[test]
fn a_reviewer_named_by_the_variable_is_announced_as_such() {
    let named = parse_reviewer_variable("ollama:llama3:8b")
        .unwrap()
        .unwrap();
    let report = evaluate(REPO, "main", 10, "b", &all_good()).with_suggested_reviewer(Some(named));
    let line = report.checks.iter().find(|c| c.id == "reviewer").unwrap();
    assert!(line.message.contains("`llama3:8b`"), "{}", line.message);
    assert!(line.message.contains("as the `reviewer` variable"));
    assert!(!line.message.contains("first remote model"));
}

#[test]
fn the_reviewer_variable_accepts_the_documented_forms() {
    assert_eq!(parse_reviewer_variable("").unwrap(), None);
    assert_eq!(parse_reviewer_variable("   ").unwrap(), None);

    let human = parse_reviewer_variable("human").unwrap().unwrap();
    assert_eq!(human.kind, CampaignReviewerKind::Human);
    assert!(!human.defaulted, "it was asked for, not defaulted");

    for (text, agent, model) in [
        ("litellm:gpt-x", AgentType::LiteLlm, "gpt-x"),
        ("NVIDIA:meta/llama-3", AgentType::Nvidia, "meta/llama-3"),
        (" ollama:llama3:8b ", AgentType::Ollama, "llama3:8b"),
    ] {
        let reviewer = parse_reviewer_variable(text).unwrap().unwrap();
        assert_eq!(reviewer.kind, CampaignReviewerKind::RemoteModel, "{text}");
        assert_eq!(reviewer.agent, Some(agent), "{text}");
        assert_eq!(reviewer.model.as_deref(), Some(model), "{text}");
        assert!(!reviewer.defaulted);
    }

    let agent = parse_reviewer_variable("agent:ClaudeCode:opus")
        .unwrap()
        .unwrap();
    assert_eq!(agent.kind, CampaignReviewerKind::Agent);
    assert_eq!(agent.agent, Some(AgentType::ClaudeCode));
    assert_eq!(agent.model.as_deref(), Some("opus"));
}

#[test]
fn a_reviewer_variable_that_cannot_review_is_refused_with_the_accepted_forms() {
    for text in [
        "somebody",
        "human:x",
        "litellm",
        "nvidia:",
        "agent:ClaudeCode",
        "agent:ClaudeCode:",
        "agent:Nobody:m",
        "agent:Ollama:m",
    ] {
        let error = parse_reviewer_variable(text).expect_err(text);
        assert!(error.contains("`reviewer`"), "{text}: {error}");
    }
    assert!(parse_reviewer_variable("somebody")
        .unwrap_err()
        .contains(REVIEWER_VARIABLE_FORMS));
}

// ─── DoD 5: the reviewer is named at creation ───────────────────────────────

fn remote(agent: AgentType, model: &str) -> (AgentType, Option<String>) {
    (agent, Some(model.to_string()))
}

#[test]
fn the_suggested_reviewer_is_the_first_configured_remote_model() {
    let reviewer = suggested_reviewer(&[
        remote(AgentType::LiteLlm, "gpt-x"),
        remote(AgentType::Nvidia, "n"),
    ])
    .unwrap();
    assert_eq!(reviewer.kind, CampaignReviewerKind::RemoteModel);
    assert_eq!(reviewer.agent, Some(AgentType::LiteLlm));
    assert_eq!(reviewer.model.as_deref(), Some("gpt-x"));
    assert!(reviewer.defaulted);
    assert!(
        suggested_reviewer(&[]).is_none(),
        "none configured: no proposal"
    );
}

#[test]
fn a_room_with_no_agent_has_the_human_as_reviewer() {
    let reviewer = room_reviewer(&AgentType::ClaudeCode, Some("opus"), None, true);
    assert_eq!(reviewer.kind, CampaignReviewerKind::Human);
    assert_eq!((reviewer.agent, reviewer.model), (None, None));
    assert!(reviewer.defaulted);
}

#[test]
fn a_room_served_by_an_agent_has_that_agent_as_reviewer() {
    let local = room_reviewer(&AgentType::Codex, Some("gpt-5"), None, false);
    assert_eq!(local.kind, CampaignReviewerKind::Agent);
    assert_eq!(local.agent, Some(AgentType::Codex));
    assert_eq!(local.model.as_deref(), Some("gpt-5"));

    let remote = room_reviewer(&AgentType::Custom, None, Some("c1"), false);
    assert_eq!(remote.kind, CampaignReviewerKind::RemoteModel);
    assert_eq!(remote.connection_id.as_deref(), Some("c1"));
    assert!(remote.defaulted);
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
    let resolved = resolve_reviewer(human).unwrap();
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
    let resolved = resolve_reviewer(agent).unwrap();
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
    assert!(resolve_reviewer(reviewer(Some(AgentType::ClaudeCode), None)).is_err());
    assert!(resolve_reviewer(reviewer(Some(AgentType::ClaudeCode), Some("  "))).is_err());
    assert!(resolve_reviewer(reviewer(None, Some("opus"))).is_err());
    assert!(resolve_reviewer(reviewer(Some(AgentType::LiteLlm), Some("m"))).is_err());
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
    assert!(resolve_reviewer(reviewer(Some(AgentType::Nvidia), None)).is_ok());
    assert!(resolve_reviewer(reviewer(Some(AgentType::ClaudeCode), None)).is_err());
    assert!(resolve_reviewer(reviewer(None, None)).is_err());
    assert!(resolve_reviewer(reviewer(Some(AgentType::Custom), None)).is_err());
    assert!(resolve_reviewer(reviewer(Some(AgentType::Custom), Some("c1"))).is_ok());
}

#[test]
fn the_room_announcement_names_the_branch_the_rounds_and_the_reviewer() {
    let reviewer = suggested_reviewer(&[remote(AgentType::LiteLlm, "gpt-x")]).unwrap();
    let text = room_announcement("main", "10", &reviewer);
    assert!(
        text.contains("`main`") && text.contains("10 review round"),
        "{text}"
    );
    assert!(
        text.contains("remote model") && text.contains("gpt-x"),
        "{text}"
    );
    assert!(text.contains("as it is set up"), "{text}");

    let human = room_reviewer(&AgentType::ClaudeCode, None, None, true);
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
