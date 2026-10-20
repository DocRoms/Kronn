//! KT-1310 — what follows an accepted template v2 merge (KT-1172), through the
//! HTTP handlers of the routes: plan → apply → accept → integrate → full audit.
//! The audit starts, the merge offer ends, a resumed plan never reads the
//! merge's own files as sources, and a v1-documented clone is Kronn's.
use super::*;
use crate::api::projects::template_merge as tm;
use crate::core::docs_folder::{folder_state, DocsFolderState};

fn configure_git(project: &Path) {
    git_in(project, &["config", "user.name", "Test"]);
    git_in(project, &["config", "user.email", "test@example.com"]);
}

/// A team's own docs, committed, and the briefing Kronn writes beside them
/// (never committed). Returns the human files with their bytes.
fn team_docs(project: &Path, docs: &str) -> Vec<(String, String)> {
    configure_git(project);
    let files = vec![
        (
            "AGENTS.md".to_string(),
            "# Team guide\n\n## Running tests\n\nmake iso-test\n".to_string(),
        ),
        (
            "docs/coding-rules.md".to_string(),
            "# Our coding rules\n\nNever a ticket id in code.\n".to_string(),
        ),
        (
            "docs/deploy.md".to_string(),
            "# Deployment\n\nTag then push.\n".to_string(),
        ),
    ];
    for (rel, content) in &files {
        let path = project.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    git_in(project, &["add", "-A"]);
    git_in(project, &["commit", "-q", "-m", "team docs"]);
    std::fs::create_dir_all(project.join(docs)).unwrap();
    std::fs::write(
        project.join(docs).join("briefing.md"),
        "# Project Briefing\n",
    )
    .unwrap();
    files
}

async fn plan_route(h: &Harness) -> tm::TemplateMergeView {
    let response = tm::template_merge_plan(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
    )
    .await
    .0;
    assert!(response.success, "{:?}", response.error);
    response.data.expect("a plan")
}

async fn start_route(h: &Harness) -> tm::TemplateMergeView {
    let response = tm::template_merge_start(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        None,
    )
    .await
    .0;
    assert!(response.success, "{:?}", response.error);
    response.data.expect("a merge")
}

async fn accept_route(h: &Harness, paths: Vec<String>) {
    let response = tm::template_merge_accept(
        axum::extract::State(h.state.clone()),
        axum::extract::Path(PROJECT_ID.to_string()),
        axum::Json(tm::TemplateMergeAcceptRequest { paths }),
    )
    .await
    .0;
    assert!(response.success, "{:?}", response.error);
}

/// plan → apply → accept everything → resume → integrate into the checkout.
/// Returns the files the merge wrote.
async fn merge_accepted_and_integrated(h: &Harness, docs: &str) -> Vec<String> {
    let project = h.project.path();
    let plan = plan_route(h).await;
    assert_eq!(plan.docs_dir, docs);
    // Kronn's own briefing is not an untracked doc of the team's.
    assert!(
        plan.untracked_in_checkout.is_empty(),
        "{:?}",
        plan.untracked_in_checkout
    );
    assert!(plan
        .correspondence
        .iter()
        .all(|row| !row.source.ends_with("briefing.md")));

    let applied = start_route(h).await;
    let written: Vec<String> = applied
        .files
        .iter()
        .filter(|f| f.outcome == Some(crate::core::docs_merge::WriteOutcome::Created))
        .map(|f| f.path.clone())
        .collect();
    assert!(written.iter().any(|p| p == &format!("{docs}/AGENTS.md")));
    accept_route(h, written.clone()).await;

    // Resumed after the accept: the files the merge wrote are Kronn's, not sources.
    let resumed = start_route(h).await;
    assert!(resumed.resumed);
    let sources: BTreeSet<&str> = resumed
        .correspondence
        .iter()
        .map(|row| row.source.as_str())
        .collect();
    assert_eq!(
        sources,
        [
            "AGENTS.md",
            "README.md",
            "docs/coding-rules.md",
            "docs/deploy.md"
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        "a resumed plan lists the merge's own files as sources"
    );
    assert!(resumed.files.is_empty(), "{:?}", resumed.files);

    // Accepted but not integrated: every audit route refuses, with a code.
    arm(
        h.tools.path(),
        "fill",
        &targets_for(&DocsRoot::from_rel(docs).unwrap()),
    );
    for bridge in [false, true] {
        let body = launch_route(h, bridge).await;
        let error = sse_events(&body, "error")
            .pop()
            .unwrap_or_else(|| panic!("bridge={bridge}: not refused: {body}"));
        assert_eq!(
            error["error_code"], "template_merge_pending_integration",
            "bridge={bridge}: {body}"
        );
    }
    let direct = run_full(&h.state, None).await;
    let error = sse_events(&direct, "error").pop().expect("refused");
    assert_eq!(error["error_code"], "template_merge_pending_integration");

    git_in(
        project,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "integrate the template v2",
            "kronn/template-v2-merge",
        ],
    );
    written
}

/// The offer is over: nothing pending, nothing to integrate, no briefing
/// listed as a doc to commit, nothing left to extract.
async fn assert_no_offer(h: &Harness) {
    let plan = plan_route(h).await;
    assert_eq!(plan.merge.pending_files, 0, "{:?}", plan.merge);
    assert_eq!(plan.merge.branch_ahead_commits, 0, "{:?}", plan.merge);
    assert!(
        plan.untracked_in_checkout.is_empty(),
        "{:?}",
        plan.untracked_in_checkout
    );
    assert!(plan
        .correspondence
        .iter()
        .all(|row| row.action != crate::core::docs_merge::MappingAction::Extracted));
}

async fn full_audit_starts(h: &Harness, docs_root: &DocsRoot) {
    arm(h.tools.path(), "fill", &targets_for(docs_root));
    settle_on_validation(h.tools.path());
    let body = if docs_root.is_default() {
        launch_route(h, false).await
    } else {
        run_full(&h.state, None).await
    };
    assert!(
        sse_events(&body, "error").is_empty(),
        "the audit was refused: {body}"
    );
    assert!(!sse_events(&body, "start").is_empty(), "{body}");
    let done = sse_events(&body, "done")
        .pop()
        .unwrap_or_else(|| panic!("no terminal done: {body}"));
    validation_settled(&h.state, &done).await;
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn after_an_accepted_and_integrated_merge_the_audit_starts_and_the_offer_ends() {
    let h = harness().await;
    let project = h.project.path();
    let human = team_docs(project, "docs");
    merge_accepted_and_integrated(&h, "docs").await;

    // The merge's files are Kronn's, the team's docs stay theirs.
    assert_eq!(folder_state(project, "docs"), DocsFolderState::KronnManaged);
    for (rel, content) in &human {
        assert_eq!(
            &std::fs::read_to_string(project.join(rel)).unwrap(),
            content
        );
    }
    assert_no_offer(&h).await;

    full_audit_starts(&h, &DocsRoot::default_root()).await;

    // The integration is noticed: nothing of the merge is left to resume.
    let state = tm::merge_state(project);
    assert!(!state.worktree_exists && !state.branch_exists, "{state:?}");
    for (rel, content) in &human {
        assert_eq!(
            &std::fs::read_to_string(project.join(rel)).unwrap(),
            content
        );
    }
    // The audit's own documents are new, untracked docs; the merge itself is over.
    let plan = plan_route(&h).await;
    assert_eq!(plan.merge, state);
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn after_a_merge_into_another_folder_the_audit_starts_there() {
    let h = harness().await;
    let project = h.project.path();
    let _relocated = relocate(project, "ai-docs");
    let human = team_docs(project, "ai-docs");
    merge_accepted_and_integrated(&h, "ai-docs").await;

    assert_eq!(
        folder_state(project, "ai-docs"),
        DocsFolderState::KronnManaged
    );
    assert_no_offer(&h).await;
    full_audit_starts(&h, &DocsRoot::from_rel("ai-docs").unwrap()).await;
    for (rel, content) in &human {
        assert_eq!(
            &std::fs::read_to_string(project.join(rel)).unwrap(),
            content
        );
    }
}

const V1_ENTRY: &str = "<!-- kronn:doc-version=\"1.0\" -->\n# AI agent context — Entry point\n\n\
**Project:** Fixture — Rust.\n";

fn v1_clone(project: &Path, entry: &str) {
    configure_git(project);
    for (rel, content) in [
        ("docs/AGENTS.md", entry),
        (
            "docs/architecture/overview.md",
            "# Overview\n\nA service.\n",
        ),
        ("docs/glossary.md", "# Glossary\n\nEW = Euronews\n"),
    ] {
        let path = project.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    git_in(project, &["add", "-A"]);
    git_in(project, &["commit", "-q", "-m", "v1 docs"]);
    assert!(!project.join("docs/.kronn.json").exists());
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_fresh_clone_of_a_v1_documented_repo_starts_the_audit() {
    let h = harness().await;
    let project = h.project.path();
    v1_clone(project, V1_ENTRY);
    assert_eq!(folder_state(project, "docs"), DocsFolderState::KronnManaged);
    full_audit_starts(&h, &DocsRoot::default_root()).await;
}

#[tokio::test]
#[serial_test::serial]
#[serial_test::serial(kronn_templates_env)]
async fn a_clone_whose_entry_file_only_mentions_the_v1_header_is_still_a_humans() {
    let h = harness().await;
    let project = h.project.path();
    // No leading header, or no Kronn heading: a human's file.
    v1_clone(
        project,
        "# Our docs\n\nSee `<!-- kronn:doc-version=\"1.0\" -->` in the v1 template.\n",
    );
    refused_on_both_routes(&h, "docs_folder_choice_required", "docs/legacy/").await;
}
