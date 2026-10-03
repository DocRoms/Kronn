// KT-926 — what a discussion hands its agent about OTHER repos.
//
// A discussion prompt goes to the model provider, so it never names a Kronn
// project other than the one the discussion is bound to: not its name, not its
// path. That holds for every kind of discussion, among them the two the audit
// opens — the briefing and the validation — which used to get the machine's whole
// project list appended to every turn.
//
// These tests register three unrelated projects, run a real discussion turn
// (`make_agent_stream`) on a scripted `claude`, and read the prompt that agent was
// handed on stdin. See `api/other_projects_fixture.rs`.
use super::streaming::make_agent_stream;
use crate::api::other_projects_fixture::{
    assert_no_candidate_pool, assert_no_unlinked_project, fresh_state, project_among_others,
    recorded_turns, recording_claude, route_claude, LINKED_LOCATION, LINKED_NAME, PROJECT_ID,
};
use crate::models::*;
use crate::AppState;
use axum::response::IntoResponse;
use chrono::Utc;

const DISCUSSION_MARKER: &str = "DISCUSSION-MARKER-KT926";

fn user_message(content: &str) -> DiscussionMessage {
    DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: uuid::Uuid::new_v4().to_string(),
        role: MessageRole::User,
        channel: MessageChannel::Main,
        content: content.to_string(),
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    }
}

/// A discussion bound to the project at hand, shaped like the ones the audit opens.
async fn insert_discussion(
    state: &AppState,
    id: &str,
    first_message: &str,
    profile_ids: Vec<String>,
) {
    let now = Utc::now();
    let message = user_message(first_message);
    let discussion = Discussion {
        connection_id: None,
        awaiting_agent: false,
        agent_running: false,
        id: id.to_string(),
        project_id: Some(PROJECT_ID.to_string()),
        title: "KT-926".into(),
        agent: AgentType::ClaudeCode,
        language: "en".into(),
        participants: vec![AgentType::ClaudeCode],
        messages: vec![message.clone()],
        message_count: 1,
        non_system_message_count: 1,
        skill_ids: vec![],
        profile_ids,
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        tier: ModelTier::default(),
        model: None,
        pin_first_message: true,
        worktree_branch: None,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: SummaryStrategy::OnDemand,
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };
    state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_discussion(conn, &discussion)?;
            crate::db::discussions::insert_message(conn, &discussion.id, &message)?;
            Ok(())
        })
        .await
        .unwrap();
}

/// Runs one agent turn of the discussion and returns the SSE stream it produced.
async fn run_turn(state: &AppState, discussion_id: &str) -> String {
    let stream = make_agent_stream(state.clone(), discussion_id.to_string(), None).await;
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        axum::body::to_bytes(stream.into_response().into_body(), 1 << 22),
    )
    .await
    .expect("the discussion stream ends")
    .unwrap();
    String::from_utf8_lossy(&body).into_owned()
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    /// An ordinary discussion on the project.
    Plain,
    /// What `start_briefing` opens.
    AuditBriefing,
    /// What a completed audit opens: the validation prompt, the audit's profiles.
    AuditValidation,
}

/// The prompt the agent of one discussion turn was handed, and the discussion's
/// project directory (kept alive by the returned guard).
async fn prompt_of(kind: Kind, linked: bool) -> (String, tempfile::TempDir) {
    let tools = tempfile::tempdir().unwrap();
    let (fixture, log) = recording_claude(tools.path());
    let state = fresh_state();
    let project = tempfile::tempdir().unwrap();
    project_among_others(&state, project.path(), linked).await;
    let _route = route_claude(project.path(), &fixture);

    let (discussion_id, expected) = match kind {
        Kind::Plain => {
            insert_discussion(
                &state,
                "disc-plain",
                &format!("{DISCUSSION_MARKER}: which repos does this project depend on?"),
                vec![],
            )
            .await;
            ("disc-plain".to_string(), DISCUSSION_MARKER.to_string())
        }
        Kind::AuditBriefing => {
            let axum::Json(response) = crate::api::audit::briefing::start_briefing(
                axum::extract::State(state.clone()),
                axum::extract::Path(PROJECT_ID.to_string()),
                axum::Json(LaunchAuditRequest {
                    agent: AgentType::ClaudeCode,
                    connection_id: None,
                    tier: None,
                    kind: None,
                    custom_prompt: None,
                    resume_run_id: None,
                }),
            )
            .await;
            let started = response.data.expect("the briefing discussion opens");
            // The briefing prompt carries the project's notes, so the turn is
            // recognisably the briefing's.
            (
                started.discussion_id,
                crate::api::other_projects_fixture::BRIEFING_MARKER.to_string(),
            )
        }
        Kind::AuditValidation => {
            let prompt = crate::api::audit::helpers::build_validation_prompt(
                "en",
                &AuditInfo {
                    files: vec![],
                    todos: vec![],
                    tech_debt_items: vec![],
                },
                false,
                &[],
            );
            insert_discussion(
                &state,
                "disc-validation",
                &prompt,
                vec![
                    "architect".into(),
                    "tech-lead".into(),
                    "qa-engineer".into(),
                    "devils-advocate".into(),
                ],
            )
            .await;
            let lead: String = prompt.chars().take(60).collect();
            ("disc-validation".to_string(), lead)
        }
    };

    let stream = run_turn(&state, &discussion_id).await;
    assert!(
        !stream.contains("event: error"),
        "{kind:?}: the discussion turn must start: {stream}"
    );
    let turns = recorded_turns(&log);
    assert_eq!(turns.len(), 1, "{kind:?}: one agent turn, got {turns:?}");
    let prompt = turns.into_iter().next().unwrap();
    assert!(
        prompt.contains(&expected),
        "{kind:?}: this is not the discussion's own prompt: {prompt}"
    );
    (prompt, project)
}

const KINDS: [Kind; 3] = [Kind::Plain, Kind::AuditBriefing, Kind::AuditValidation];

#[tokio::test]
async fn no_discussion_names_an_unlinked_kronn_project() {
    for kind in KINDS {
        let (prompt, _project) = prompt_of(kind, false).await;
        assert_no_unlinked_project(&prompt, &format!("{kind:?}"));
        assert_no_candidate_pool(&prompt, &format!("{kind:?}"));
    }
}

#[tokio::test]
async fn a_linked_repo_does_not_bring_the_unlinked_projects_along() {
    for kind in KINDS {
        let (prompt, _project) = prompt_of(kind, true).await;
        assert_no_unlinked_project(&prompt, &format!("{kind:?}"));
        assert_no_candidate_pool(&prompt, &format!("{kind:?}"));
    }
}

/// A discussion does not inline the linked repos (0.8.4, #295: the agent pulls them
/// on demand): they reach it through `docs/linked-repos.md`, which the project keeps
/// in step with its "Linked repos" list. That file is where the explicit declaration
/// stays for a discussion, and it holds the declared repo only.
#[test]
fn the_linked_repo_stays_in_the_projects_linked_repos_doc_alone() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("docs")).unwrap();
    let linked = vec![LinkedRepo {
        id: "lr-1".into(),
        name: LINKED_NAME.into(),
        kind: "design".into(),
        location: LINKED_LOCATION.into(),
        description: "the design system".into(),
    }];
    crate::api::projects::sync_linked_repos_doc(project.path(), &linked).unwrap();
    let doc = std::fs::read_to_string(project.path().join("docs/linked-repos.md")).unwrap();
    assert!(doc.contains(LINKED_NAME) && doc.contains(LINKED_LOCATION));
    assert_no_unlinked_project(&doc, "docs/linked-repos.md");
}
