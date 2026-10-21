use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::run_states_tests::{queued_validation, seed_project, state};

async fn post(state: crate::AppState, route: &str, body: serde_json::Value) -> serde_json::Value {
    send(state, "POST", route, body).await
}

async fn send(
    state: crate::AppState,
    method: &str,
    route: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    let response = crate::build_router_with_auth(state, false)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/api/projects/p1/{route}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn docs_writers_refuse_a_held_lease_without_changing_files_or_stopping_validation() {
    let state = state();
    let root = tempfile::tempdir().unwrap();
    seed_project(&state, root.path()).await;
    std::fs::create_dir_all(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join("docs/AGENTS.md"), "# Human instructions\n").unwrap();
    queued_validation(&state).await;
    assert!(state.audit_tracker.lock().unwrap().try_acquire_lease("p1"));
    for (route, body) in [
        ("anti-hallu/inject", serde_json::json!({})),
        ("redirectors/sync", serde_json::json!({})),
        ("migrate-docs", serde_json::json!({})),
        ("install-template", serde_json::json!({})),
        ("mark-bootstrapped", serde_json::json!({})),
        ("audit-attestation", serde_json::json!({"confirmed":true})),
        ("template-merge", serde_json::json!({})),
        (
            "template-merge/accept",
            serde_json::json!({"paths":["docs/new.md"]}),
        ),
        ("audit-state/restore", serde_json::json!({"commit":"HEAD"})),
    ] {
        let response = post(state.clone(), route, body).await;
        assert_eq!(response["success"], false, "{route}: {response}");
        assert_eq!(
            response["error"].as_str(),
            Some(crate::api::audit::docs_write::DOCS_BUSY),
            "{route}: {response}"
        );
        assert!(state.audit_tracker.lock().unwrap().leased.contains("p1"));
    }
    assert_eq!(
        std::fs::read_to_string(root.path().join("docs/AGENTS.md")).unwrap(),
        "# Human instructions\n"
    );
    assert!(!root.path().join("docs/.kronn.json").exists());
    assert!(state
        .db
        .with_read_conn(|conn| crate::db::agent_dispatch::has_active_for_discussion(conn, "d-val"))
        .await
        .unwrap());
}

#[tokio::test]
async fn docs_writer_pending_template_integration_refuses_all_audit_launches() {
    let state = state();
    let root = tempfile::tempdir().unwrap();
    seed_project(&state, root.path()).await;
    let git = |args: &[&str]| {
        let result = crate::core::cmd::git_cmd()
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args(args)
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join("docs/AGENTS.md"), "# Project\n").unwrap();
    std::fs::write(
        root.path().join("docs/briefing.md"),
        "A saved human briefing\n",
    )
    .unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "baseline"]);
    git(&[
        "checkout",
        "-q",
        "-b",
        crate::core::docs_merge::MERGE_BRANCH,
    ]);
    std::fs::write(
        root.path().join("docs/new.md"),
        "# Accepted template addition\n",
    )
    .unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "accepted template"]);
    git(&["checkout", "-q", "main"]);
    for route in ["full-audit", "partial-audit", "revalidate-audit"] {
        let response = crate::build_router_with_auth(state.clone(), false)
            .oneshot(
                Request::post(format!("/api/projects/p1/{route}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"agent":"Custom","steps":[1]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("pending integration"), "{route}: {body}");
    }
    assert!(!root.path().join("docs/new.md").exists());
    assert!(state
        .db
        .with_read_conn(|conn| crate::db::audit_runs::list_recent(conn, "p1", 10))
        .await
        .unwrap()
        .is_empty());
    git(&["merge", "--ff-only", crate::core::docs_merge::MERGE_BRANCH]);
    assert!(crate::api::projects::template_merge::audit_launch_refusal(root.path()).is_none());
}

#[tokio::test]
async fn docs_writer_stops_validation_before_mutation_and_releases_its_lease() {
    let state = state();
    let root = tempfile::tempdir().unwrap();
    seed_project(&state, root.path()).await;
    std::fs::create_dir_all(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join("docs/AGENTS.md"), "# Human instructions\n").unwrap();
    queued_validation(&state).await;
    let token = tokio_util::sync::CancellationToken::new();
    state
        .cancel_registry
        .lock()
        .unwrap()
        .insert("d-val".into(), token.clone());
    let response = post(state.clone(), "anti-hallu/inject", serde_json::json!({})).await;
    assert_eq!(response["success"], true, "{response}");
    assert!(
        token.is_cancelled(),
        "the active validation must be stopped"
    );
    assert!(!state
        .db
        .with_read_conn(|conn| crate::db::agent_dispatch::has_active_for_discussion(conn, "d-val"))
        .await
        .unwrap());
    assert!(std::fs::read_to_string(root.path().join("docs/AGENTS.md"))
        .unwrap()
        .contains("anti-hallu"));
    assert!(!state.audit_tracker.lock().unwrap().leased.contains("p1"));
}

#[tokio::test]
async fn docs_writer_late_validation_admission_checks_the_lease_and_cancelled_job() {
    let state = state();
    let root = tempfile::tempdir().unwrap();
    seed_project(&state, root.path()).await;
    queued_validation(&state).await;
    state
        .db
        .with_conn(|conn| crate::db::agent_dispatch::claim(conn, "job-val"))
        .await
        .unwrap()
        .unwrap();
    let lease = crate::api::audit::docs_write::DocsWriteLease::try_acquire(&state, "p1").unwrap();
    assert!(crate::api::audit::docs_write::validation_cancel_guard(
        &state,
        "d-val".into(),
        Some("job-val".into())
    )
    .await
    .is_err());
    assert!(state.cancel_registry.lock().unwrap().is_empty());
    drop(lease);
    let guard = crate::api::audit::docs_write::validation_cancel_guard(
        &state,
        "d-val".into(),
        Some("job-val".into()),
    )
    .await
    .unwrap();
    let lease = crate::api::audit::docs_write::DocsWriteLease::acquire(&state, "p1")
        .await
        .unwrap();
    assert!(guard.token.is_cancelled());
    drop(lease);
    assert!(crate::api::audit::docs_write::validation_cancel_guard(
        &state,
        "d-val".into(),
        Some("job-val".into())
    )
    .await
    .is_err());
}

#[tokio::test]
async fn docs_writer_linked_repo_sync_respects_source_and_reverse_target_leases() {
    let state = state();
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    seed_project(&state, source.path()).await;
    std::fs::create_dir_all(source.path().join("docs")).unwrap();
    std::fs::create_dir_all(target.path().join("docs")).unwrap();
    std::fs::write(
        target.path().join("docs/linked-repos.md"),
        "Human review in progress\n",
    )
    .unwrap();
    let target_path = target.path().display().to_string();
    let stored_path = target_path.clone();
    state
        .db
        .with_conn(move |conn| {
            let mut project = crate::db::projects::get_project(conn, "p1")?
                .ok_or_else(|| anyhow::anyhow!("fixture project missing"))?;
            project.id = "p2".into();
            project.name = "Linked target".into();
            project.path = stored_path;
            crate::db::projects::insert_project(conn, &project)
        })
        .await
        .unwrap();
    let payload = serde_json::json!([{
        "id":"link", "name":"Linked target", "kind":"api",
        "location":target_path, "description":"Shared API"
    }]);
    let call = || {
        let state = state.clone();
        let payload = payload.clone();
        async move {
            let response = crate::build_router_with_auth(state, false)
                .oneshot(
                    Request::put("/api/projects/p1/linked-repos")
                        .header("content-type", "application/json")
                        .body(Body::from(payload.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
        }
    };
    let source_lease =
        crate::api::audit::docs_write::DocsWriteLease::try_acquire(&state, "p1").unwrap();
    assert_eq!(call().await["success"], false);
    assert!(!source.path().join("docs/linked-repos.md").exists());
    assert!(state
        .db
        .with_read_conn(|conn| Ok(crate::db::projects::get_project(conn, "p1")?
            .ok_or_else(|| anyhow::anyhow!("fixture project missing"))?
            .linked_repos
            .is_empty()))
        .await
        .unwrap());
    drop(source_lease);
    let target_lease =
        crate::api::audit::docs_write::DocsWriteLease::try_acquire(&state, "p2").unwrap();
    assert_eq!(call().await["success"], true);
    assert!(source.path().join("docs/linked-repos.md").exists());
    assert_eq!(
        std::fs::read_to_string(target.path().join("docs/linked-repos.md")).unwrap(),
        "Human review in progress\n"
    );
    assert!(state.audit_tracker.lock().unwrap().leased.contains("p2"));
    drop(target_lease);
}

const BRIEFING_FORM: &str = r#"{"purpose":"News site","team":"Four devs","maturity":"Production",
    "dependencies":"Jira","traps":"Cache purge","additional":""}"#;

/// A project whose docs hold a saved human briefing, so a refused write is
/// visible both on disk (`docs/briefing.md`) and in the database.
async fn project_with_briefing(state: &crate::AppState) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    seed_project(state, root.path()).await;
    std::fs::create_dir_all(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join("docs/briefing.md"), "Saved by a human\n").unwrap();
    state
        .db
        .with_conn(|conn| {
            crate::db::projects::update_project_briefing_notes(conn, "p1", Some("Saved by a human"))
        })
        .await
        .unwrap();
    root
}

async fn saved_notes(state: &crate::AppState) -> Option<String> {
    state
        .db
        .with_read_conn(|conn| crate::db::projects::get_project_briefing_notes(conn, "p1"))
        .await
        .unwrap()
}

#[tokio::test]
async fn briefing_save_and_answers_take_the_lease_and_refuse_with_docs_busy_without_writing() {
    let state = state();
    let root = project_with_briefing(&state).await;
    let lease = crate::api::audit::docs_write::DocsWriteLease::try_acquire(&state, "p1").unwrap();
    let form: serde_json::Value = serde_json::from_str(BRIEFING_FORM).unwrap();
    for (method, route, body) in [
        (
            "PUT",
            "briefing",
            serde_json::json!({"notes":"Overwritten"}),
        ),
        ("PUT", "briefing", serde_json::json!({"notes":null})),
        ("POST", "save-briefing", form.clone()),
    ] {
        let response = send(state.clone(), method, route, body.clone()).await;
        assert_eq!(response["success"], false, "{method} {route}: {response}");
        assert_eq!(
            response["error"].as_str(),
            Some(crate::api::audit::docs_write::DOCS_BUSY),
            "{method} {route}: {response}"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("docs/briefing.md")).unwrap(),
            "Saved by a human\n",
            "{method} {route} wrote the briefing file"
        );
        assert_eq!(
            saved_notes(&state).await.as_deref(),
            Some("Saved by a human"),
            "{method} {route} wrote the briefing column"
        );
    }
    assert!(!root.path().join("docs/.kronn.json").exists());
    // The refusal came from the lease alone: once released, the same saves go through.
    drop(lease);
    let response = send(
        state.clone(),
        "PUT",
        "briefing",
        serde_json::json!({"notes":"Edited after the audit"}),
    )
    .await;
    assert_eq!(response["success"], true, "{response}");
    assert_eq!(
        std::fs::read_to_string(root.path().join("docs/briefing.md")).unwrap(),
        "Edited after the audit"
    );
    let response = send(state.clone(), "POST", "save-briefing", form).await;
    assert_eq!(response["success"], true, "{response}");
    assert!(
        std::fs::read_to_string(root.path().join("docs/briefing.md"))
            .unwrap()
            .contains("News site")
    );
    assert!(!state.audit_tracker.lock().unwrap().leased.contains("p1"));
}

#[tokio::test]
async fn validate_takes_the_lease_and_refuses_with_docs_busy_without_marking_validated() {
    let state = state();
    let root = tempfile::tempdir().unwrap();
    seed_project(&state, root.path()).await;
    std::fs::create_dir_all(root.path().join("docs")).unwrap();
    std::fs::write(root.path().join("docs/AGENTS.md"), "# Project\n").unwrap();
    let lease = crate::api::audit::docs_write::DocsWriteLease::try_acquire(&state, "p1").unwrap();
    let response = post(state.clone(), "validate-audit", serde_json::json!({})).await;
    assert_eq!(response["success"], false, "{response}");
    assert_eq!(
        response["error"].as_str(),
        Some(crate::api::audit::docs_write::DOCS_BUSY),
        "{response}"
    );
    assert!(!root.path().join("docs/.kronn.json").exists());
    assert_eq!(
        std::fs::read_to_string(root.path().join("docs/AGENTS.md")).unwrap(),
        "# Project\n"
    );
    assert!(state.audit_tracker.lock().unwrap().leased.contains("p1"));
    // Released, the route reaches its own gate (no completed run to validate):
    // the refusal above was the lease's, not the gate's.
    drop(lease);
    let response = post(state.clone(), "validate-audit", serde_json::json!({})).await;
    assert_eq!(response["success"], false, "{response}");
    assert_ne!(
        response["error"].as_str(),
        Some(crate::api::audit::docs_write::DOCS_BUSY),
        "{response}"
    );
    assert!(!state.audit_tracker.lock().unwrap().leased.contains("p1"));
}

#[tokio::test]
async fn accept_takes_the_lease_and_refuses_with_docs_busy_without_recording_the_step() {
    let state = state();
    let root = tempfile::tempdir().unwrap();
    seed_project(&state, root.path()).await;
    let docs_root = crate::core::docs_location::DocsRoot::of(root.path());
    let chain = crate::api::audit::steps_for(crate::models::AuditKind::Full, &docs_root);
    let target = chain[0].target_file;
    let target_path = root.path().join(target);
    std::fs::create_dir_all(target_path.parent().unwrap()).unwrap();
    std::fs::write(&target_path, "# Filled by the audit\n\nReal content.\n").unwrap();
    state
        .db
        .with_conn(|conn| {
            crate::db::audit_runs::insert_running(
                conn,
                "r1",
                "p1",
                "Full",
                "ClaudeCode",
                chrono::Utc::now(),
            )?;
            crate::db::audit_runs::mark_interrupted_strict(conn, "r1", "stopped")
        })
        .await
        .unwrap();
    let steps = || async {
        state
            .db
            .with_read_conn(|conn| crate::db::audit_runs::list_audit_steps(conn, "r1"))
            .await
            .unwrap()
    };
    let lease = crate::api::audit::docs_write::DocsWriteLease::try_acquire(&state, "p1").unwrap();
    let route = "audit-runs/r1/steps/1/accept";
    let response = post(state.clone(), route, serde_json::json!({"reason":"fine"})).await;
    assert_eq!(response["success"], false, "{response}");
    assert_eq!(
        response["error"].as_str(),
        Some(crate::api::audit::docs_write::DOCS_BUSY),
        "{response}"
    );
    assert!(steps().await.is_empty(), "a refused accept recorded a step");
    assert_eq!(
        std::fs::read_to_string(&target_path).unwrap(),
        "# Filled by the audit\n\nReal content.\n"
    );
    // Released, the very same request is accepted.
    drop(lease);
    let response = post(state.clone(), route, serde_json::json!({"reason":"fine"})).await;
    assert_eq!(response["success"], true, "{response}");
    let recorded = steps().await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].step_index, 1);
    assert!(!state.audit_tracker.lock().unwrap().leased.contains("p1"));
}
