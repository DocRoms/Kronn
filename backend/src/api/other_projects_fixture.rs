// KT-926 — shared fixtures for the tests that read what an agent prompt says about
// OTHER repos. A prompt goes to the model provider, so the only repo it may name
// besides the project at hand is one the user explicitly linked to that project.
//
// The tests run a real entry point (an audit pipeline, a discussion turn, a
// workflow run) on a scripted `claude` routed to the project's directory and read
// what that agent was handed on stdin: the prompt as the model receives it, not a
// string the test assembled. No socket, so they also run where local port binding
// is refused.
use crate::AppState;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Three other projects registered in Kronn, none of them linked to the project at hand.
pub(crate) const OTHER_PROJECTS: [(&str, &str); 3] = [
    ("orchid-billing-api", "/srv/zz-clients/orchid-billing-api"),
    ("zephyr-mobile-app", "/srv/zz-clients/zephyr-mobile-app"),
    ("quartz-infra", "/srv/zz-clients/quartz-infra"),
];
pub(crate) const LINKED_NAME: &str = "linked-design-system";
pub(crate) const LINKED_LOCATION: &str = "/srv/zz-clients/linked-design-system";
/// In the project's briefing, which every audit step prompt carries: a turn that
/// holds it is a step prompt, not a retry note or a validation turn.
pub(crate) const BRIEFING_MARKER: &str = "BRIEFING-MARKER-KT926";
pub(crate) const PROJECT_ID: &str = "proj-audited";
const TURN_SEPARATOR: &str = "=====KT926-TURN=====";

pub(crate) fn fresh_state() -> AppState {
    AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        Arc::new(crate::db::Database::open_in_memory().unwrap()),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

pub(crate) async fn register_project(
    state: &AppState,
    id: &str,
    name: &str,
    path: &str,
    extra: Value,
) {
    let mut row = json!({
        "id": id, "name": name, "path": path,
        "repo_url": null, "token_override": null, "ai_config": {"detected": false, "configs": []},
        "created_at": chrono::Utc::now().to_rfc3339(), "updated_at": chrono::Utc::now().to_rfc3339()
    });
    row.as_object_mut()
        .unwrap()
        .extend(extra.as_object().cloned().unwrap_or_default());
    let row: crate::models::Project = serde_json::from_value(row).unwrap();
    state
        .db
        .with_conn(move |conn| crate::db::projects::insert_project(conn, &row))
        .await
        .unwrap();
}

/// The project at hand (with a linked repo when asked) next to the three unlinked ones.
pub(crate) async fn project_among_others(state: &AppState, dir: &Path, linked: bool) {
    let linked_repos = if linked {
        json!([{
            "id": "lr-1", "name": LINKED_NAME, "kind": "design",
            "location": LINKED_LOCATION, "description": "the design system"
        }])
    } else {
        json!([])
    };
    register_project(
        state,
        PROJECT_ID,
        "audited",
        &dir.to_string_lossy(),
        json!({"briefing_notes": BRIEFING_MARKER, "linked_repos": linked_repos}),
    )
    .await;
    for (index, (name, path)) in OTHER_PROJECTS.iter().enumerate() {
        register_project(state, &format!("proj-other-{index}"), name, path, json!({})).await;
    }
}

/// A scripted `claude` that records its stdin and its arguments, then ends a turn
/// with a one-word answer. Returns the fixture program and the log it appends to.
pub(crate) fn recording_claude(tools: &Path) -> (PathBuf, PathBuf) {
    let log = tools.join("turns.log");
    let script = format!(
        "{{ cat; printf '%s\\n' \"$*\"; printf '%s\\n' '{TURN_SEPARATOR}'; }} >> '{}'\n{}",
        log.display(),
        r#"printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Done."}}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":1,"output_tokens":1}}'"#
    );
    (
        crate::acp::test_support::write_fixture_script(tools, &script),
        log,
    )
}

/// Routes every Claude launch in `project`'s working directory to `fixture`.
pub(crate) fn route_claude(
    project: &Path,
    fixture: &Path,
) -> crate::agents::runner::test_acp_routes::RouteGuard {
    let project_path = project.to_string_lossy().into_owned();
    let work_dir =
        crate::agents::runner::resolve_agent_work_dir(Some(&project_path), &project_path).unwrap();
    crate::agents::runner::test_acp_routes::route(
        &work_dir,
        Arc::new(crate::acp::ClaudeAcpAdapter::new_with_program(
            fixture.to_string_lossy(),
            None,
            false,
        )),
    )
}

/// Every turn the scripted agent was handed, prompt then arguments.
pub(crate) fn recorded_turns(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .split(TURN_SEPARATOR)
        .map(str::trim)
        .filter(|turn| !turn.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `prompt` names none of the three unlinked projects, by name or by path.
pub(crate) fn assert_no_unlinked_project(prompt: &str, what: &str) {
    for (name, path) in OTHER_PROJECTS {
        assert!(!prompt.contains(name), "{what}: prompt names {name}");
        assert!(!prompt.contains(path), "{what}: prompt names {path}");
    }
}

/// `prompt` carries no trace of the retired candidate pool, whatever it lists.
pub(crate) fn assert_no_candidate_pool(prompt: &str, what: &str) {
    for retired in [
        "Suggested companion repos",
        "Other Kronn projects",
        "andidate pool",
        "companion-repo candidate",
        "registered in Kronn",
        "AGENTS.md at the path above",
    ] {
        assert!(
            !prompt.contains(retired),
            "{what}: prompt still says {retired:?}"
        );
    }
}
