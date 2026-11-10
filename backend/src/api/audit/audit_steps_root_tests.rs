//! KT-1603 — `GET /api/audit/steps` takes a project and labels the plan
//! through its docs root, the folder the run itself writes to.
use axum::extract::{Query, State};

use super::run_states_tests::{seed_project, state};
use super::{audit_steps, AuditStepsQuery};
use crate::models::AuditStepInfo;

fn query(project_id: Option<&str>) -> Query<AuditStepsQuery> {
    Query(AuditStepsQuery {
        project_id: project_id.map(str::to_string),
    })
}

fn labels(steps: &[AuditStepInfo]) -> Vec<String> {
    steps.iter().map(|s| s.target_file.clone()).collect()
}

#[tokio::test]
async fn without_a_project_the_labels_stay_under_docs() {
    let steps = audit_steps(State(state()), query(None))
        .await
        .0
        .data
        .unwrap();
    assert!(!steps.is_empty());
    assert!(steps
        .iter()
        .enumerate()
        .all(|(i, s)| s.index == i as u32 + 1));
    assert!(steps
        .iter()
        .filter(|s| s.target_file != "REVIEW")
        .all(|s| s.target_file.starts_with("docs/")));
}

#[tokio::test]
async fn a_project_override_relocates_every_target() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let state = state();
    seed_project(&state, &root).await;
    let default = audit_steps(State(state.clone()), query(None))
        .await
        .0
        .data
        .unwrap();

    let path = root.to_string_lossy().into_owned();
    crate::core::docs_location::set_project_override(&path, Some(".ai"));
    let relocated = audit_steps(State(state.clone()), query(Some("p1"))).await;
    crate::core::docs_location::set_project_override(&path, None);
    let relocated = relocated.0.data.unwrap();

    assert_eq!(relocated.len(), default.len());
    for (moved, plain) in relocated.iter().zip(&default) {
        assert_eq!(moved.index, plain.index);
        match plain.target_file.strip_prefix("docs/") {
            Some(rest) => assert_eq!(moved.target_file, format!(".ai/{rest}")),
            None => assert_eq!(moved.target_file, plain.target_file),
        }
    }
    assert!(relocated.iter().any(|s| s.target_file.starts_with(".ai/")));
}

#[tokio::test]
async fn a_project_without_an_override_keeps_docs() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("fresh");
    std::fs::create_dir_all(&root).unwrap();
    let state = state();
    seed_project(&state, &root).await;
    let plain = audit_steps(State(state.clone()), query(None))
        .await
        .0
        .data
        .unwrap();
    let scoped = audit_steps(State(state), query(Some("p1")))
        .await
        .0
        .data
        .unwrap();
    assert_eq!(labels(&plain), labels(&scoped));
}

#[tokio::test]
async fn an_unknown_project_is_refused() {
    let response = audit_steps(State(state()), query(Some("nope"))).await.0;
    assert!(response.data.is_none());
    assert_eq!(response.error.as_deref(), Some("Project not found"));
}

#[cfg(unix)] // creates a symlink through std::os::unix
#[tokio::test]
async fn a_refused_override_refuses_the_plan() {
    let t = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = t.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join(".ai")).unwrap();
    let state = state();
    seed_project(&state, &root).await;
    let path = root.to_string_lossy().into_owned();
    crate::core::docs_location::set_project_override(&path, Some(".ai"));
    let response = audit_steps(State(state), query(Some("p1"))).await.0;
    crate::core::docs_location::set_project_override(&path, None);
    assert!(response.data.is_none());
    assert!(response.error.unwrap().contains("symlink"));
}
